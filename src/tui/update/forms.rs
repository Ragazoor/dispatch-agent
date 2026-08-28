//! Form input, text-entry, creation/edit/delete-flow handlers.

use crate::models::{TaskTag, WrapUpMode};

use super::super::types::*;
use super::super::{filtered_repos, has_new_repo_option, App, PendingAction};

impl App {
    pub(in crate::tui) fn handle_copy_task(&mut self) -> Vec<Command> {
        let task = match self.selected_task() {
            Some(t) => t,
            None => return vec![],
        };
        let title = format!("Copy of: {}", task.title);
        let description = task.description.clone();
        let repo_path = task.repo_path.clone();
        let tag = task.tag;
        let wrap_up_mode = task.wrap_up_mode;
        self.input.task_draft = Some(TaskDraft {
            title,
            description,
            tag,
            wrap_up_mode,
            ..Default::default()
        });
        self.input.set_buffer(repo_path);
        self.input.repo_cursor = 0;
        self.input.mode = InputMode::InputRepoPath;
        self.set_status("Enter repo path: ".to_string());
        vec![]
    }

    pub(in crate::tui) fn handle_start_new_task(&mut self) -> Vec<Command> {
        self.input.mode = InputMode::InputTitle;
        self.input.clear_buffer();
        self.input.task_draft = None;
        self.set_status("Enter title: ".to_string());
        vec![]
    }

    pub(in crate::tui) fn handle_cancel_input(&mut self) -> Vec<Command> {
        self.input.mode = InputMode::Normal;
        self.input.clear_buffer();
        self.input.task_draft = None;
        self.input.pending_epic_id = None;
        self.interaction.pending = PendingAction::None;
        self.clear_status();
        vec![]
    }

    pub(in crate::tui) fn handle_submit_title(&mut self, value: String) -> Vec<Command> {
        self.input.clear_buffer();
        if value.is_empty() {
            self.input.mode = InputMode::Normal;
            self.input.task_draft = None;
            self.clear_status();
        } else {
            self.input.task_draft = Some(TaskDraft {
                title: value,
                description: String::new(),
                repo_path: String::new(),
                tag: None,
                base_branch: "main".to_string(),
                wrap_up_mode: None,
                phoenix: false,
            });
            self.input.mode = InputMode::InputTag;
            self.set_status(
                "Tag: [b]ug  [f]eature  [c]hore  [p]r-review  [r]esearch  [x]fix  [Enter] none"
                    .to_string(),
            );
        }
        vec![]
    }

    pub(in crate::tui) fn handle_submit_description(&mut self, value: String) -> Vec<Command> {
        self.input.clear_buffer();
        if let Some(ref mut draft) = self.input.task_draft {
            draft.description = value;
        }
        self.input.repo_cursor = 0;
        self.input.mode = InputMode::InputRepoPath;
        self.set_status("Enter repo path: ".to_string());
        vec![]
    }

    pub(in crate::tui) fn handle_submit_repo_path(&mut self, value: String) -> Vec<Command> {
        self.input.clear_buffer();
        if value.is_empty() {
            self.set_status("Repo path required (no saved paths available)".to_string());
            return vec![];
        }
        // Accepted std::fs-in-handler exception (docs/conventions.md, "No
        // std::fs inside async handlers"): a bare exists()/is_dir() stat, no
        // read or parse, on the low-frequency repo-path submit path.
        if let Err(msg) = crate::dispatch::validate_repo_path(&value) {
            self.set_status(msg);
            return vec![];
        }
        if let Some(ref mut draft) = self.input.task_draft {
            draft.repo_path = value.clone();
        }
        let default_base_branch = self
            .input
            .task_draft
            .as_ref()
            .map(|d| d.base_branch.clone())
            .unwrap_or_else(|| "main".to_string());
        // PrefillFromHistory (dispatch.allium: BaseBranchPicker): prefer the
        // most-recently-used branch for this repo; fall back to the draft
        // default when the repo has no history yet.
        let base_branch = self
            .base_branches_for(&value)
            .first()
            .cloned()
            .unwrap_or(default_base_branch);
        self.input.set_buffer(base_branch);
        self.input.repo_cursor = 0;
        self.input.mode = InputMode::InputBaseBranch;
        self.set_status("Base branch: ".to_string());
        vec![]
    }

    pub(in crate::tui) fn handle_submit_base_branch(&mut self, value: String) -> Vec<Command> {
        let base_branch = if value.is_empty() {
            self.input
                .task_draft
                .as_ref()
                .map(|d| d.base_branch.clone())
                .unwrap_or_else(|| "main".to_string())
        } else {
            value
        };
        if let Some(ref mut draft) = self.input.task_draft {
            draft.base_branch = base_branch;
        }
        self.input.clear_buffer();
        self.input.mode = InputMode::InputWrapUpMode;
        self.set_status("Wrap-up: [r]ebase  [p]r  [d]one  [Enter] skip".to_string());
        vec![]
    }

    pub(in crate::tui) fn handle_submit_wrap_up_mode(
        &mut self,
        mode: Option<WrapUpMode>,
    ) -> Vec<Command> {
        // `mode == None` means Enter was pressed without an explicit r/p/d
        // pick — leave the draft's existing value untouched rather than
        // clearing it, so CopyTask's prefilled source-task mode survives an
        // Enter-to-skip instead of being silently dropped.
        if let (Some(ref mut draft), Some(m)) = (self.input.task_draft.as_mut(), mode) {
            draft.wrap_up_mode = Some(m);
        }
        self.input.mode = InputMode::InputPhoenix;
        self.set_status(crate::tui::ui::PHOENIX_PROMPT.to_string());
        vec![]
    }

    /// The form's last step. Unlike wrap_up_mode's, this does NOT seed from a
    /// copied source task: `CopyTask` deliberately does not carry the flag (see
    /// docs/specs/tasks.allium), so the draft answers this question fresh even
    /// when the source was a phoenix.
    pub(in crate::tui) fn handle_submit_phoenix(&mut self, on: bool) -> Vec<Command> {
        if let Some(ref mut draft) = self.input.task_draft {
            draft.phoenix = on;
        }
        self.finish_task_creation()
    }

    pub(in crate::tui) fn handle_submit_tag(&mut self, tag: Option<TaskTag>) -> Vec<Command> {
        self.input.clear_buffer();
        if let Some(ref mut draft) = self.input.task_draft {
            draft.tag = tag;
        }
        self.input.mode = InputMode::InputDescription;
        self.set_status("Opening editor for description...".to_string());
        vec![Command::Editor(
            crate::tui::commands::EditorCommand::PopOut(EditKind::Description { is_epic: false }),
        )]
    }

    pub(in crate::tui) fn handle_input_char(&mut self, c: char) -> Vec<Command> {
        // Per spec (RepoPathPicker.NoPrintableShortcut): every printable
        // character filters; no digit/letter is a select shortcut.
        // The list cursor resets to 0 whenever the query changes.
        if self.input.mode.is_repo_picker() {
            self.input.repo_cursor = 0;
        }
        self.input.caret =
            crate::tui::text_caret::insert(&mut self.input.buffer, self.input.caret, c);
        vec![]
    }

    pub(in crate::tui) fn handle_input_backspace(&mut self) -> Vec<Command> {
        // Per spec: the list cursor resets to 0 whenever the query changes
        if self.input.mode.is_repo_picker() {
            self.input.repo_cursor = 0;
        }
        self.input.caret =
            crate::tui::text_caret::delete_before(&mut self.input.buffer, self.input.caret);
        vec![]
    }

    pub(in crate::tui) fn handle_input_delete_forward(&mut self) -> Vec<Command> {
        if self.input.mode.is_repo_picker() {
            self.input.repo_cursor = 0;
        }
        self.input.caret =
            crate::tui::text_caret::delete_after(&mut self.input.buffer, self.input.caret);
        vec![]
    }

    pub(in crate::tui) fn handle_cursor_left(&mut self) -> Vec<Command> {
        self.input.caret = crate::tui::text_caret::move_left(self.input.caret);
        vec![]
    }

    pub(in crate::tui) fn handle_cursor_right(&mut self) -> Vec<Command> {
        self.input.caret = crate::tui::text_caret::move_right(&self.input.buffer, self.input.caret);
        vec![]
    }

    pub(in crate::tui) fn handle_cursor_word_left(&mut self) -> Vec<Command> {
        self.input.caret = crate::tui::text_caret::word_left(&self.input.buffer, self.input.caret);
        vec![]
    }

    pub(in crate::tui) fn handle_cursor_word_right(&mut self) -> Vec<Command> {
        self.input.caret = crate::tui::text_caret::word_right(&self.input.buffer, self.input.caret);
        vec![]
    }

    pub(in crate::tui) fn handle_cursor_home(&mut self) -> Vec<Command> {
        self.input.caret = crate::tui::text_caret::home();
        vec![]
    }

    pub(in crate::tui) fn handle_cursor_end(&mut self) -> Vec<Command> {
        self.input.caret = crate::tui::text_caret::end(&self.input.buffer);
        vec![]
    }

    pub(in crate::tui) fn handle_start_quick_dispatch_selection(&mut self) -> Vec<Command> {
        self.input.mode = InputMode::QuickDispatch;
        self.input.repo_cursor = 0;
        self.input.clear_buffer();
        self.set_status("Type to filter · ↑/↓ navigate · Enter select · Esc cancel".to_string());
        vec![]
    }

    pub(in crate::tui) fn handle_select_quick_dispatch_repo(&mut self, idx: usize) -> Vec<Command> {
        let repos = filtered_repos(&self.board.repo_paths, &self.input.buffer);
        let repo_path = if idx < repos.len() {
            repos[idx].clone()
        } else if has_new_repo_option(&self.input.buffer, &repos) {
            self.input.buffer.clone()
        } else {
            return vec![];
        };
        let epic_id = self.input.pending_epic_id.take();
        self.input.mode = InputMode::Normal;
        self.input.clear_buffer();
        self.clear_status();
        self.handle_quick_dispatch(repo_path, epic_id)
    }

    pub(in crate::tui) fn handle_cancel_retry(&mut self) -> Vec<Command> {
        self.input.mode = InputMode::Normal;
        self.clear_status();
        vec![]
    }
}
