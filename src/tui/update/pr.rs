//! PR-related message handlers: review state polling.

use crate::models::{ReviewDecision, SubStatus, TaskId, TaskStatus};

use super::super::types::*;
use super::super::App;
use crate::tui::{PR_POLL_BACKOFF_MAX, PR_POLL_INTERVAL, PR_POLL_PERMANENT_FAILURE_THRESHOLD};
use std::time::{Duration, Instant};

/// How long to wait before the nth consecutive transient failure is retried:
/// `PR_POLL_INTERVAL` doubled per failure, capped at `PR_POLL_BACKOFF_MAX`.
///
/// The cap is what keeps a long GitHub outage from pushing a task's next
/// attempt past the point anyone would still care, while the doubling is what
/// stops a spent rate limit costing one call per task per tick.
fn transient_backoff(consecutive_transient_failures: u32) -> Duration {
    // Saturating rather than wrapping: a task left running for a very long
    // outage must land on the cap, not wrap around to a 30-second retry.
    let doubling = 1u32.checked_shl(consecutive_transient_failures.saturating_sub(1));
    doubling
        .and_then(|factor| PR_POLL_INTERVAL.checked_mul(factor))
        .unwrap_or(PR_POLL_BACKOFF_MAX)
        .min(PR_POLL_BACKOFF_MAX)
}

impl App {
    /// Builds the `Persist` command, plus a `SendNotification` when
    /// notifications are enabled, for a PR-status task update. Shared by
    /// `handle_pr_merged` and `handle_pr_closed` — they differ in what they
    /// change on the task, not in how the resulting write/notification pair
    /// is emitted.
    fn persist_and_notify(
        &self,
        fields: crate::tui::commands::PersistFields,
        title: String,
        body: String,
    ) -> Vec<Command> {
        let mut cmds = vec![Command::Task(crate::tui::commands::TaskCommand::Persist(
            fields,
        ))];
        if self.notifications_enabled {
            cmds.push(Command::System(
                crate::tui::commands::SystemCommand::SendNotification {
                    title,
                    body,
                    urgent: false,
                },
            ));
        }
        cmds
    }

    pub(in crate::tui) fn handle_pr_merged(&mut self, id: TaskId) -> Vec<Command> {
        let mut cmds = Vec::new();
        self.clear_pr_poll_failures(id);

        if let Some(task) = self.find_task_mut(id) {
            if task.status != TaskStatus::Review {
                return cmds;
            }

            let pr_label = task.url.as_ref().map_or("PR".to_string(), |u| u.label());
            let task_title = task.title.clone();

            // Detach: kill tmux window but preserve worktree
            if let Some(window) = task.tmux_window.take() {
                cmds.push(Command::Task(
                    crate::tui::commands::TaskCommand::KillTmuxWindow { window },
                ));
            }
            task.status = TaskStatus::Done;
            task.sub_status = SubStatus::default_for(TaskStatus::Done);
            let fields = crate::tui::commands::PersistFields::from_task(task);

            self.clear_agent_tracking(id);
            self.sync_board_selection();
            self.set_status(format!(
                "{pr_label} merged \u{2014} task #{id} moved to Done"
            ));

            cmds.extend(self.persist_and_notify(
                fields,
                "PR merged".to_string(),
                format!("{pr_label} merged: {task_title}"),
            ));
        }

        cmds.extend(self.maybe_respawn_split_pane(id));

        cmds
    }

    /// A closed-without-merge PR is NOT a terminal event: the task stays in
    /// review — only `sub_status` changes, to `pr_closed`, so the user
    /// notices and decides what to do (reopen the PR, push a new one,
    /// archive the task). No tmux/worktree teardown, unlike `PrMerged`.
    ///
    /// `PollPrStatus` re-fires this event every tick the PR stays closed, so
    /// this is guarded on `sub_status != PrClosed` to avoid re-persisting and
    /// re-notifying on every tick. Overrides `sub_status = Conflict`
    /// unconditionally — a closed PR is a stronger, more definitive GitHub
    /// signal than a local rebase conflict.
    pub(in crate::tui) fn handle_pr_closed(&mut self, id: TaskId) -> Vec<Command> {
        let mut cmds = Vec::new();
        self.clear_pr_poll_failures(id);

        if let Some(task) = self.find_task_mut(id) {
            if task.status != TaskStatus::Review || task.sub_status == SubStatus::PrClosed {
                return cmds;
            }

            let pr_label = task.url.as_ref().map_or("PR".to_string(), |u| u.label());
            let task_title = task.title.clone();

            task.sub_status = SubStatus::PrClosed;
            let fields = crate::tui::commands::PersistFields::from_task(task);

            self.set_status(format!(
                "{pr_label} closed \u{2014} task #{id} marked \"PR closed\""
            ));

            cmds = self.persist_and_notify(
                fields,
                "PR closed".to_string(),
                format!("{pr_label} closed: {task_title}"),
            );
        }

        cmds
    }

    /// Record a failed PR read, and give up on the task once permanent failures
    /// reach the threshold.
    ///
    /// The log line is the point of the split. Before this, every failed attempt
    /// warned, so five PRs the `gh` account could not read produced 63,000
    /// identical lines over five months and told the user nothing. Now a
    /// permanent failure warns once, on the transition into giving up, and a
    /// transient one only widens the backoff at debug level.
    pub(in crate::tui) fn handle_pr_check_failed(
        &mut self,
        id: TaskId,
        permanent: bool,
        error: String,
    ) -> Vec<Command> {
        let state = self.agents.pr_poll.entry(id).or_default();

        if !permanent {
            // Transient: the permanent counter is deliberately left alone — a
            // network blip must never accumulate towards stranding a task —
            // and only the deadline moves.
            state.consecutive_transient_failures += 1;
            let wait = transient_backoff(state.consecutive_transient_failures);
            state.next_poll_at = Some(Instant::now() + wait);
            tracing::debug!(
                task_id = id.0,
                retry_in_s = wait.as_secs(),
                "PR status check failed transiently, backing off: {error}"
            );
            return vec![];
        }

        state.consecutive_permanent_failures += 1;
        if state.consecutive_permanent_failures < PR_POLL_PERMANENT_FAILURE_THRESHOLD {
            tracing::debug!(
                task_id = id.0,
                failures = state.consecutive_permanent_failures,
                "PR status check failed permanently, below give-up threshold: {error}"
            );
            return vec![];
        }
        // `gave_up` is derived from this same counter (see PrPollState::gave_up),
        // so the only way to tell "just now crossed the threshold" from "already
        // given up, this is a late in-flight result" is the exact value: the
        // counter increments by one per permanent failure, so it passes through
        // the threshold exactly once.
        if state.consecutive_permanent_failures > PR_POLL_PERMANENT_FAILURE_THRESHOLD {
            return vec![];
        }

        tracing::warn!(
            task_id = id.0,
            failures = PR_POLL_PERMANENT_FAILURE_THRESHOLD,
            "giving up on PR status polling for this task: {error}"
        );

        let Some(task) = self.find_task_mut(id) else {
            return vec![];
        };
        if task.status != TaskStatus::Review || task.sub_status == SubStatus::PrUnreachable {
            return vec![];
        }
        let pr_label = task.url.as_ref().map_or("PR".to_string(), |u| u.label());
        let task_title = task.title.clone();
        task.sub_status = SubStatus::PrUnreachable;
        let fields = crate::tui::commands::PersistFields::from_task(task);

        self.set_status(format!(
            "{pr_label} unreadable \u{2014} task #{id} marked \"PR unreachable\""
        ));

        self.persist_and_notify(
            fields,
            "PR unreachable".to_string(),
            format!("{pr_label} could not be read: {task_title}"),
        )
    }

    /// Clear every failure record for a task whose PR was just read
    /// successfully. This is the only path out of `gave_up`.
    fn clear_pr_poll_failures(&mut self, id: TaskId) {
        if let Some(state) = self.agents.pr_poll.get_mut(&id) {
            *state = crate::tui::types::PrPollState::default();
        }
    }

    pub(in crate::tui) fn handle_pr_review_state(
        &mut self,
        id: TaskId,
        review_decision: Option<ReviewDecision>,
    ) -> Vec<Command> {
        self.clear_pr_poll_failures(id);
        if let Some(task) = self.find_task_mut(id) {
            if task.status != TaskStatus::Review {
                return vec![];
            }
            // Don't overwrite attention-requiring substatuses
            if task.sub_status == SubStatus::Conflict {
                return vec![];
            }
            let new_sub = match review_decision {
                Some(ReviewDecision::Approved) => SubStatus::Approved,
                Some(ReviewDecision::ChangesRequested) => SubStatus::ChangesRequested,
                _ => SubStatus::AwaitingReview,
            };
            if task.sub_status != new_sub {
                task.sub_status = new_sub;
                let fields = crate::tui::commands::PersistFields::from_task(task);
                return vec![Command::Task(crate::tui::commands::TaskCommand::Persist(
                    fields,
                ))];
            }
        }
        vec![]
    }
}
