//! System / status / editor / quick-dispatch / misc handlers.

use crate::models::{EpicId, TaskId, DEFAULT_BASE_BRANCH, DEFAULT_QUICK_TASK_TITLE};

use super::super::types::*;
use super::super::App;

impl App {
    pub(in crate::tui) fn handle_error(&mut self, msg: String) -> Vec<Command> {
        self.status.error_popup = Some(msg);
        vec![]
    }

    pub(in crate::tui) fn handle_task_edited(&mut self, edit: TaskEdit) -> Vec<Command> {
        if let Some(t) = self.find_task_mut(edit.id) {
            t.title = edit.title;
            t.description = edit.description;
            t.repo_path = edit.repo_path;
            // Not `set_local_status`: the editor owns sub_status separately (it
            // is not part of TaskEdit, so the current one is kept deliberately)
            // and only the leaving-Running clear applies here. The editor may
            // set any status from any other, Running -> Review included.
            if crate::models::clears_pending_stop(t.status, edit.status) {
                t.stop_pending = false;
            }
            t.status = edit.status;
            t.plan_path = edit.plan_path;
            t.tag = edit.tag;
            if let Some(bb) = edit.base_branch {
                t.base_branch = bb;
            }
            t.wrap_up_mode = edit.wrap_up_mode;
            t.url = edit.url;
            t.updated_at = chrono::Utc::now();
        }
        self.sync_board_selection();
        vec![]
    }

    pub(in crate::tui) fn handle_repo_paths_updated(&mut self, paths: Vec<String>) -> Vec<Command> {
        self.broken_repo_paths = paths
            .iter()
            .filter(|p| !std::path::Path::new(p).is_dir())
            .cloned()
            .collect();
        self.board.repo_paths = paths;
        // cursor 0 = toggle row, 1..=len = repo rows; clamp to len (not len-1)
        self.input.repo_cursor = self.input.repo_cursor.min(self.board.repo_paths.len());
        vec![]
    }

    pub(in crate::tui) fn handle_base_branches_updated(
        &mut self,
        map: std::collections::HashMap<String, Vec<String>>,
    ) -> Vec<Command> {
        self.board.repo_base_branches = map;
        vec![]
    }

    pub(in crate::tui) fn handle_quick_dispatch(
        &mut self,
        repo_path: String,
        epic_id: Option<EpicId>,
    ) -> Vec<Command> {
        vec![Command::Task(
            crate::tui::commands::TaskCommand::QuickDispatch {
                draft: TaskDraft {
                    title: DEFAULT_QUICK_TASK_TITLE.to_string(),
                    description: String::new(),
                    repo_path,
                    tag: None,
                    base_branch: DEFAULT_BASE_BRANCH.to_string(),
                    wrap_up_mode: None,
                },
                epic_id,
            },
        )]
    }

    pub(in crate::tui) fn handle_dismiss_error(&mut self) -> Vec<Command> {
        self.status.error_popup = None;
        vec![]
    }

    pub(in crate::tui) fn handle_status_info(&mut self, msg: String) -> Vec<Command> {
        self.set_status(msg);
        vec![]
    }

    pub(in crate::tui) fn handle_toggle_help(&mut self) -> Vec<Command> {
        if self.input.mode == InputMode::Help {
            self.input.mode = InputMode::Normal;
        } else {
            self.input.mode = InputMode::Help;
        }
        vec![]
    }

    pub(in crate::tui) fn exit_split_if_active(&mut self) -> Vec<Command> {
        if !self.board.split.active {
            return vec![];
        }
        let pane_id = match self.board.split.right_pane_id.take() {
            Some(id) => id,
            None => return vec![],
        };
        let restore_window = self
            .board
            .split
            .pinned_task_id
            .and_then(|id| self.find_task(id))
            .and_then(|t| t.tmux_window.clone());
        vec![Command::Split(crate::tui::commands::SplitCommand::Exit {
            pane_id,
            restore_window,
        })]
    }

    pub(in crate::tui) fn finish_task_creation(&mut self, repo_path: String) -> Vec<Command> {
        let draft = self.input.task_draft.take().unwrap_or_default();
        let base_branch = draft.base_branch.clone();
        self.input.mode = InputMode::Normal;
        self.clear_status();
        let epic_id = match self.effective_view_mode() {
            BoardViewMode::Epic { epic_id, .. } => Some(epic_id),
            BoardViewMode::Board(_) => None,
        };
        vec![
            Command::Task(crate::tui::commands::TaskCommand::Insert { draft, epic_id }),
            Command::SaveRepoPath(repo_path.clone()),
            Command::SaveBaseBranch(repo_path, base_branch),
        ]
    }

    /// A dispatch that *held* the claim ended without provisioning: drain the
    /// spinner and hand the claim back. The funnel for every failure downstream of
    /// a won claim, which is why the release rides here rather than at each
    /// producer.
    ///
    /// Anything that ended *without* holding the claim goes to
    /// [`Self::handle_dispatch_abandoned`] instead — see
    /// [`crate::tui::messages::TaskMessage::DispatchAbandoned`] for why that split
    /// is load-bearing.
    pub(in crate::tui) fn handle_dispatch_failed(&mut self, id: TaskId) -> Vec<Command> {
        self.unmark_dispatching(id);
        vec![Command::Task(
            crate::tui::commands::TaskCommand::ReleaseClaim(id),
        )]
    }

    /// A dispatch ended before it ever held the claim — it lost the claim, or
    /// gave up upstream of it (a failed repo-trust grant). Drain the spinner and
    /// touch nothing else. See [`Self::handle_dispatch_failed`] for why this must
    /// not release.
    pub(in crate::tui) fn handle_dispatch_abandoned(&mut self, id: TaskId) -> Vec<Command> {
        self.unmark_dispatching(id);
        vec![]
    }

    pub(in crate::tui) fn handle_mark_dispatching(&mut self, id: TaskId) -> Vec<Command> {
        self.mark_dispatching(id);
        vec![]
    }

    /// An epic's auto-dispatch chain claimed a subtask, failed to provision it,
    /// and released it back to backlog — so the epic has stopped progressing
    /// (`SurfaceAutoDispatchFailure` in docs/specs/epics.allium).
    ///
    /// Three surfaces, because they fail at different distances from the board:
    /// the status message reaches an operator who is looking at it, the
    /// notification one who is not, and the marker is what is still there an
    /// hour later. Only the marker is durable; see
    /// [`crate::tui::types::AgentTracking::auto_dispatch_failed`].
    ///
    /// No claim is released here: the chain released it before sending this.
    pub(in crate::tui) fn handle_auto_dispatch_failed(
        &mut self,
        task_id: TaskId,
        epic_id: crate::models::EpicId,
        reason: String,
    ) -> Vec<Command> {
        self.set_status(format!(
            "Auto-dispatch of #{} failed — epic #{} stalled: {reason}",
            task_id.0, epic_id.0
        ));
        self.agents.auto_dispatch_failed.insert(task_id, reason);

        if self.notifications_enabled {
            let title = self
                .find_task(task_id)
                .map(|t| t.title.clone())
                .unwrap_or_default();
            vec![Command::System(
                crate::tui::commands::SystemCommand::SendNotification {
                    title: format!("Task #{}: {title}", task_id.0),
                    body: "Epic auto-dispatch failed — chain stopped".to_string(),
                    // Urgent, unlike a task becoming reviewable: nothing further
                    // happens on this epic until a human acts.
                    urgent: true,
                },
            )]
        } else {
            vec![]
        }
    }

    /// Whether the subtask's last chained dispatch failed and left the epic
    /// stalled (`AutoDispatchFailureIndicator` in docs/specs/epics.allium).
    pub fn auto_dispatch_failed(&self, id: TaskId) -> bool {
        self.agents.auto_dispatch_failed.contains_key(&id)
    }

    pub(in crate::tui) fn handle_description_editor_result(
        &mut self,
        value: String,
    ) -> Vec<Command> {
        match self.input.mode {
            InputMode::InputDescription => self.handle_submit_description(value),
            InputMode::InputEpicDescription => self.handle_submit_epic_description(value),
            _ => vec![],
        }
    }

    /// Router for editor results that come back from a pop-out editor. Each
    /// `EditKind` is finalized by an `EditorCommand::FinalizeResult` command
    /// dispatched to the runtime, except the `Description` variant which
    /// threads straight through the existing description-flow messages.
    pub(in crate::tui) fn handle_editor_result(
        &mut self,
        kind: EditKind,
        outcome: EditorOutcome,
    ) -> Vec<Command> {
        use crate::tui::commands::EditorCommand;
        use crate::tui::messages::EditorMessage;
        match (&kind, &outcome) {
            (EditKind::Description { .. }, EditorOutcome::Saved(text)) => {
                let text = crate::editor::parse_description_editor_output(text);
                self.update(Message::Editor(EditorMessage::DescriptionResult(text)))
            }
            (EditKind::Description { .. }, EditorOutcome::Cancelled) => self.update(
                Message::Input(crate::tui::messages::InputMessage::CancelInput),
            ),
            _ => vec![Command::Editor(EditorCommand::FinalizeResult {
                kind,
                outcome,
            })],
        }
    }

    pub(in crate::tui) fn handle_open_in_browser(&self, url: String) -> Vec<Command> {
        vec![Command::System(
            crate::tui::commands::SystemCommand::OpenInBrowser { url },
        )]
    }
}
