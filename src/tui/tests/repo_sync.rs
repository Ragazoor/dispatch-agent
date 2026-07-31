//! Tests for the board-facing half of local-first repo sync
//! (docs/specs/repo-sync.allium): the `RepoDriftIndicator` status-bar segment,
//! the `[o]` action and its `RepoSyncConfirmation`, and the refresh triggers the
//! board itself owns.
#![allow(clippy::unwrap_used, clippy::expect_used)]

use super::*;
use crate::repo_sync::{AheadBehind, RepoSyncMeasurement, RepoSyncState, SyncOutcome};
use crate::tui::commands::RepoSyncCommand;
use crate::tui::messages::RepoSyncMessage;
use crate::tui::ui::{repo_drift_segment, repo_sync_prompt_text};
use crossterm::event::KeyCode;

const REPO: &str = "/repo";

fn measurement(counts: Option<AheadBehind>, fetch_error: Option<&str>) -> RepoSyncMeasurement {
    RepoSyncMeasurement {
        repo_path: REPO.to_string(),
        base_branch: "main".to_string(),
        counts,
        fetch_error: fetch_error.map(str::to_string),
    }
}

fn drifted(ahead: u32, behind: u32) -> RepoSyncMeasurement {
    measurement(Some(AheadBehind { ahead, behind }), None)
}

/// An app whose cursor sits on a Backlog task in `/repo`, with `repo_sync`
/// primed by one measurement.
fn app_with_measurement(m: RepoSyncMeasurement) -> App {
    let mut app = make_app();
    app.selection_mut().set_column(1);
    app.selection_mut().set_row(1, 0);
    app.update(Message::RepoSync(RepoSyncMessage::Measured(m)));
    app
}

fn state_of(app: &App) -> Option<&RepoSyncState> {
    app.selected_repo_sync_state()
}

// ---------------------------------------------------------------------------
// RefreshRepoSyncState — Measured folds into the board's per-repo cache
// ---------------------------------------------------------------------------

// rule-success.RefreshRepoSyncState
#[test]
fn measured_message_records_the_state_for_the_repo() {
    let app = app_with_measurement(drifted(3, 1));
    let state = state_of(&app).expect("state recorded for the selected task's repo");
    assert_eq!(state.repo_path, REPO);
    assert_eq!(state.base_branch, "main");
    assert_eq!(
        state.counts,
        Some(AheadBehind {
            ahead: 3,
            behind: 1
        })
    );
    assert!(state.has_drift());
}

#[test]
fn measured_message_keeps_previous_counts_when_a_later_fetch_fails() {
    let mut app = app_with_measurement(drifted(3, 1));
    app.update(Message::RepoSync(RepoSyncMessage::Measured(measurement(
        None,
        Some("offline"),
    ))));
    let state = state_of(&app).expect("state still present");
    assert_eq!(
        state.counts,
        Some(AheadBehind {
            ahead: 3,
            behind: 1
        }),
        "NoStalenessBeyondTheLastMeasurement: a failed fetch never blanks the segment"
    );
    assert_eq!(state.last_fetch_error.as_deref(), Some("offline"));
}

// ---------------------------------------------------------------------------
// RefreshRepoSyncStateAfterDispatch
// ---------------------------------------------------------------------------

// rule-success.RefreshRepoSyncStateAfterDispatch: launching an agent refreshes
// the repository's drift, without a fetch (the worktree provisioning already
// refreshed the refs).
#[test]
fn dispatching_a_task_requests_a_non_fetching_refresh() {
    let mut app = make_app();
    let cmds = app.update(Message::Task(
        crate::tui::messages::TaskMessage::Dispatched {
            id: TaskId(1),
            worktree: "/repo/.worktrees/1-task".to_string(),
            tmux_window: "task-1".to_string(),
            switch_focus: false,
        },
    ));
    assert!(
        cmds.iter().any(|c| matches!(
            c,
            Command::RepoSync(RepoSyncCommand::Refresh {
                repo_path,
                fetch_first: false
            }) if repo_path == REPO
        )),
        "expected a non-fetching refresh for {REPO}, got: {cmds:?}"
    );
}

// rule-failure.RefreshRepoSyncStateAfterDispatch.1: resume provisions nothing
// and fetches nothing, so the guard `mode in {standard, research, quick}`
// excludes it and no refresh follows.
#[test]
fn resuming_a_task_requests_no_refresh() {
    let mut app = make_app();
    let cmds = app.update(Message::Task(crate::tui::messages::TaskMessage::Resumed {
        id: TaskId(1),
        tmux_window: "task-1".to_string(),
    }));
    assert!(
        !cmds
            .iter()
            .any(|c| matches!(c, Command::RepoSync(RepoSyncCommand::Refresh { .. }))),
        "resume re-reads exactly the refs the last measurement read, got: {cmds:?}"
    );
}

// ---------------------------------------------------------------------------
// RefreshRepoSyncStateAfterSync
// ---------------------------------------------------------------------------

// rule-success.RefreshRepoSyncStateAfterSync: a completed sync recounts so the
// indicator clears without waiting for another event.
#[test]
fn a_completed_sync_requests_a_non_fetching_refresh() {
    let mut app = app_with_measurement(drifted(3, 0));
    let cmds = app.update(Message::RepoSync(RepoSyncMessage::Succeeded {
        repo_path: REPO.to_string(),
        outcome: SyncOutcome::Synced {
            pulled: 0,
            pushed: 3,
        },
    }));
    assert!(
        cmds.iter().any(|c| matches!(
            c,
            Command::RepoSync(RepoSyncCommand::Refresh {
                repo_path,
                fetch_first: false
            }) if repo_path == REPO
        )),
        "expected a recount after the sync, got: {cmds:?}"
    );
}

// OutcomeAndFailureRouteDifferently: a successful outcome goes to the status bar.
#[test]
fn a_completed_sync_reports_its_counts_in_the_status_bar() {
    let mut app = app_with_measurement(drifted(3, 1));
    app.update(Message::RepoSync(RepoSyncMessage::Succeeded {
        repo_path: REPO.to_string(),
        outcome: SyncOutcome::Synced {
            pulled: 1,
            pushed: 4,
        },
    }));
    let msg = app.status_message().expect("outcome reported").to_string();
    assert!(msg.contains('1') && msg.contains('4'), "got: {msg}");
    assert!(app.error_popup().is_none(), "a success is not an error");
}

// UnmeasuredIsNeverPresentedAsClean's vocabulary carve-out: AlreadyInSync is
// worded as "nothing to do" and quotes no counts.
#[test]
fn already_in_sync_is_worded_as_nothing_to_do_and_quotes_no_counts() {
    let mut app = app_with_measurement(measurement(None, None));
    app.update(Message::RepoSync(RepoSyncMessage::Succeeded {
        repo_path: REPO.to_string(),
        outcome: SyncOutcome::AlreadyInSync,
    }));
    let msg = app
        .status_message()
        .expect("outcome reported")
        .to_lowercase();
    assert!(
        msg.contains("nothing to do"),
        "an unmeasurable sync must not claim levelness, got: {msg}"
    );
    assert!(
        !msg.contains('\u{2191}') && !msg.contains('\u{2193}'),
        "must not quote counts it never read, got: {msg}"
    );
}

// ReportRepoSyncFailure: a failure goes to the error popup, never the status bar.
#[test]
fn a_failed_sync_goes_to_the_error_popup() {
    let mut app = app_with_measurement(drifted(3, 1));
    let cmds = app.update(Message::RepoSync(RepoSyncMessage::Failed {
        repo_path: REPO.to_string(),
        detail: "Push rejected \u{2014} origin moved since the fetch".to_string(),
        retryable: true,
    }));
    let popup = app
        .error_popup()
        .expect("failures need a decision")
        .to_string();
    assert!(popup.contains("Push rejected"), "got: {popup}");
    assert!(
        !cmds
            .iter()
            .any(|c| matches!(c, Command::RepoSync(RepoSyncCommand::Refresh { .. }))),
        "RepoSyncFinished is ensured by SyncRepo, not by a failure: {cmds:?}"
    );
}

#[test]
fn a_retryable_failure_says_so() {
    let mut app = app_with_measurement(drifted(3, 1));
    app.update(Message::RepoSync(RepoSyncMessage::Failed {
        repo_path: REPO.to_string(),
        detail: "Push rejected".to_string(),
        retryable: true,
    }));
    let popup = app.error_popup().expect("popup shown").to_lowercase();
    assert!(
        popup.contains("try again") || popup.contains("retry"),
        "a retryable failure must say retrying is the fix, got: {popup}"
    );
}

// ---------------------------------------------------------------------------
// surface RepoDriftIndicator
// ---------------------------------------------------------------------------

fn segment_text(app: &App) -> Option<String> {
    repo_drift_segment(state_of(app))
        .map(|spans| spans.iter().map(|s| s.content.as_ref()).collect::<String>())
}

// surface-exposure.RepoDriftIndicator: base_branch, ahead and behind are all on
// the segment when it is lit.
#[test]
fn drift_segment_exposes_base_branch_and_both_counts() {
    let app = app_with_measurement(drifted(3, 1));
    let text = segment_text(&app).expect("visible with drift");
    assert!(text.contains("main"), "base_branch must be named: {text}");
    assert!(text.contains('3'), "ahead must be shown: {text}");
    assert!(text.contains('1'), "behind must be shown: {text}");
}

// @guarantee HiddenWhenClean
#[test]
fn drift_segment_is_hidden_when_clean() {
    let app = app_with_measurement(drifted(0, 0));
    assert_eq!(segment_text(&app), None);
}

// @guarantee HiddenWhenUnmeasured — UnmeasuredIsNeverPresentedAsClean.
#[test]
fn drift_segment_is_hidden_when_unmeasured() {
    let app = app_with_measurement(measurement(None, Some("offline")));
    assert_eq!(
        segment_text(&app),
        None,
        "an unmeasurable repo renders as unknown, never as in sync"
    );
}

// @guarantee HiddenWithoutASelectedTask — an epic row is not a task.
#[test]
fn drift_segment_is_hidden_when_an_epic_row_is_selected() {
    let mut make = make_app_with_epic_selected();
    make.update(Message::RepoSync(RepoSyncMessage::Measured(drifted(3, 1))));
    assert!(make.selected_task().is_none(), "cursor is on the epic");
    assert_eq!(segment_text(&make), None);
}

// The segment names the repository of the selected task and no other.
#[test]
fn drift_segment_is_hidden_for_a_repo_that_was_never_measured() {
    let mut app = make_app();
    app.selection_mut().set_column(1);
    app.selection_mut().set_row(1, 0);
    let mut m = drifted(3, 1);
    m.repo_path = "/some/other/repo".to_string();
    app.update(Message::RepoSync(RepoSyncMessage::Measured(m)));
    assert_eq!(segment_text(&app), None);
}

// @guarantee BehindIsAWarning
#[test]
fn drift_segment_styles_behind_as_a_warning() {
    let app = app_with_measurement(drifted(0, 2));
    let spans = repo_drift_segment(state_of(&app)).expect("visible");
    assert!(
        spans
            .iter()
            .any(|s| s.style.fg == Some(ratatui::style::Color::Yellow)),
        "behind > 0 must be styled as a warning"
    );
}

// @guarantee AheadOnlyIsNeutral
#[test]
fn drift_segment_styles_ahead_only_neutrally() {
    let app = app_with_measurement(drifted(3, 0));
    let spans = repo_drift_segment(state_of(&app)).expect("visible");
    assert!(
        !spans
            .iter()
            .any(|s| s.style.fg == Some(ratatui::style::Color::Yellow)),
        "ahead-only is the normal post-rebase state, so it is not a warning"
    );
}

// The segment is rendered into the board's status bar, not merely computable.
#[test]
fn drift_segment_appears_in_the_rendered_status_bar() {
    let mut app = app_with_measurement(drifted(3, 1));
    let buf = render_to_buffer(&mut app, 120, 40);
    assert!(
        buffer_contains(&buf, "main \u{2191}3\u{2193}1"),
        "expected the drift segment in the rendered board"
    );
}

// ---------------------------------------------------------------------------
// rule PromptRepoSync — the [o] key
// ---------------------------------------------------------------------------

// rule-success.PromptRepoSync / surface-provides.RepoDriftIndicator
#[test]
fn o_opens_the_sync_confirmation_when_the_segment_is_lit() {
    let mut app = app_with_measurement(drifted(3, 1));
    app.handle_key(make_key(KeyCode::Char('o')));
    assert_eq!(
        app.mode(),
        &InputMode::ConfirmRepoSync {
            repo_path: REPO.to_string()
        }
    );
}

// rule-failure.PromptRepoSync.1 — no measured state for the repo.
#[test]
fn o_does_nothing_for_an_unmeasured_repo() {
    let mut app = app_with_measurement(measurement(None, Some("offline")));
    app.handle_key(make_key(KeyCode::Char('o')));
    assert_eq!(app.mode(), &InputMode::Normal);
}

// rule-failure.PromptRepoSync.2 — measured but clean.
#[test]
fn o_does_nothing_when_the_repo_is_clean() {
    let mut app = app_with_measurement(drifted(0, 0));
    app.handle_key(make_key(KeyCode::Char('o')));
    assert_eq!(app.mode(), &InputMode::Normal);
}

#[test]
fn o_does_nothing_without_a_selected_task() {
    let mut app = make_app_with_epic_selected();
    app.update(Message::RepoSync(RepoSyncMessage::Measured(drifted(3, 1))));
    app.handle_key(make_key(KeyCode::Char('o')));
    assert_eq!(app.mode(), &InputMode::Normal);
}

// ---------------------------------------------------------------------------
// surface RepoSyncConfirmation
// ---------------------------------------------------------------------------

// surface-exposure.RepoSyncConfirmation + @guarantee
// PromptStatesExactlyWhatWillHappen.
#[test]
fn the_prompt_names_both_halves_when_diverged() {
    let state = RepoSyncState {
        repo_path: REPO.to_string(),
        base_branch: "main".to_string(),
        counts: Some(AheadBehind {
            ahead: 3,
            behind: 1,
        }),
        last_fetch_error: None,
    };
    let text = repo_sync_prompt_text(&state);
    assert!(text.contains("main"), "names the branch: {text}");
    assert!(text.contains("merge") && text.contains('1'), "got: {text}");
    assert!(text.contains("push") && text.contains('3'), "got: {text}");
}

#[test]
fn the_prompt_names_only_the_push_when_not_behind() {
    let state = RepoSyncState {
        repo_path: REPO.to_string(),
        base_branch: "main".to_string(),
        counts: Some(AheadBehind {
            ahead: 3,
            behind: 0,
        }),
        last_fetch_error: None,
    };
    let text = repo_sync_prompt_text(&state);
    assert!(text.contains("push"), "got: {text}");
    assert!(
        !text.contains("merge"),
        "a half that will not run is not mentioned: {text}"
    );
}

#[test]
fn the_prompt_names_only_the_merge_when_not_ahead() {
    let state = RepoSyncState {
        repo_path: REPO.to_string(),
        base_branch: "main".to_string(),
        counts: Some(AheadBehind {
            ahead: 0,
            behind: 2,
        }),
        last_fetch_error: None,
    };
    let text = repo_sync_prompt_text(&state);
    assert!(text.contains("merge"), "got: {text}");
    assert!(
        !text.contains("push"),
        "a half that will not run is not mentioned: {text}"
    );
}

#[test]
fn the_prompt_text_is_shown_while_confirming() {
    let mut app = app_with_measurement(drifted(3, 1));
    app.handle_key(make_key(KeyCode::Char('o')));
    let shown = app.status_message().expect("the prompt is shown");
    assert!(shown.contains("main"), "got: {shown}");
}

// rule-success.AcceptRepoSyncPrompt / surface-provides.RepoSyncConfirmation
#[test]
fn confirming_the_prompt_syncs_the_repository() {
    let mut app = app_with_measurement(drifted(3, 1));
    app.handle_key(make_key(KeyCode::Char('o')));
    let cmds = without_usage(app.handle_key(make_key(KeyCode::Char('y'))));
    assert!(
        cmds.iter().any(|c| matches!(
            c,
            Command::RepoSync(RepoSyncCommand::Sync { repo_path, base_branch })
                if repo_path == REPO && base_branch == "main"
        )),
        "expected a sync for {REPO}, got: {cmds:?}"
    );
    assert_eq!(app.mode(), &InputMode::Normal);
}

// @guarantee NoSyncWithoutConfirmation
#[test]
fn dismissing_the_prompt_leaves_the_repository_untouched() {
    let mut app = app_with_measurement(drifted(3, 1));
    app.handle_key(make_key(KeyCode::Char('o')));
    let cmds = without_usage(app.handle_key(make_key(KeyCode::Char('n'))));
    assert!(
        !cmds
            .iter()
            .any(|c| matches!(c, Command::RepoSync(RepoSyncCommand::Sync { .. }))),
        "dismissing must not sync: {cmds:?}"
    );
    assert_eq!(app.mode(), &InputMode::Normal);
}

// Opening the prompt alone fetches, merges and pushes nothing.
#[test]
fn opening_the_prompt_emits_no_sync_command() {
    let mut app = app_with_measurement(drifted(3, 1));
    let cmds = without_usage(app.handle_key(make_key(KeyCode::Char('o'))));
    assert!(
        !cmds
            .iter()
            .any(|c| matches!(c, Command::RepoSync(RepoSyncCommand::Sync { .. }))),
        "the prompt itself performs no work: {cmds:?}"
    );
}

// rule-failure.AcceptRepoSyncPrompt.1: a confirmation whose repo lost its drift
// between prompt and confirm performs no sync.
#[test]
fn confirming_does_not_sync_when_the_drift_is_gone() {
    let mut app = app_with_measurement(drifted(3, 1));
    app.handle_key(make_key(KeyCode::Char('o')));
    app.update(Message::RepoSync(RepoSyncMessage::Measured(drifted(0, 0))));
    let cmds = without_usage(app.handle_key(make_key(KeyCode::Char('y'))));
    assert!(
        !cmds
            .iter()
            .any(|c| matches!(c, Command::RepoSync(RepoSyncCommand::Sync { .. }))),
        "state.has_drift is required at confirm time: {cmds:?}"
    );
}
