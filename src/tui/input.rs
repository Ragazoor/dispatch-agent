mod confirm;
mod normal;
mod repo_filter;

use crossterm::event::{KeyCode, KeyEvent, KeyModifiers};

use super::{App, ColumnItem, Command, InputMode, Message, MoveDirection, ViewMode};
use crate::models::{
    DispatchMode, EpicId, SubStatus, TaskId, TaskStatus, TaskTag, UsageActor, UsageCategory,
    UsageEvent,
};
use crate::tui::commands::UsageCommand;

fn key_event(action: &str, key: &str) -> Command {
    Command::Usage(UsageCommand::Record(UsageEvent {
        category: UsageCategory::Keybinding,
        action: action.to_string(),
        detail: Some(key.to_string()),
        actor: UsageActor::Human,
    }))
}

/// The `detail` of a keybinding usage event: the key as the user typed it.
/// Two bindings for one action share an `action` and are told apart by this,
/// so `j` and `Down` stay separable in the recorded data (see
/// `KeypressRecordsFeatureUsage` in `docs/specs/observability.allium`).
fn key_label(key: KeyEvent) -> String {
    match key.code {
        KeyCode::Char(c) => c.to_string(),
        KeyCode::Enter => "Enter".to_string(),
        KeyCode::Esc => "Esc".to_string(),
        KeyCode::Tab => "Tab".to_string(),
        KeyCode::BackTab => "BackTab".to_string(),
        KeyCode::Backspace => "Backspace".to_string(),
        KeyCode::Delete => "Delete".to_string(),
        KeyCode::Up => "Up".to_string(),
        KeyCode::Down => "Down".to_string(),
        KeyCode::Left => "Left".to_string(),
        KeyCode::Right => "Right".to_string(),
        KeyCode::Home => "Home".to_string(),
        KeyCode::End => "End".to_string(),
        other => format!("{other:?}"),
    }
}

/// The tree movement a key requests in either tree picker (reparent-epic,
/// move-task-to-epic), or `None` for a key that is not a movement. Both
/// pickers navigate identically; only the message they wrap it in differs.
fn tree_nav_for(key: KeyEvent) -> Option<crate::tui::types::TreeNav> {
    use crate::tui::types::TreeNav;
    match key.code {
        KeyCode::Char('j') | KeyCode::Down => Some(TreeNav::Down),
        KeyCode::Char('k') | KeyCode::Up => Some(TreeNav::Up),
        KeyCode::Char('l') | KeyCode::Right | KeyCode::Char(' ') => Some(TreeNav::Right),
        KeyCode::Char('h') | KeyCode::Left => Some(TreeNav::Left),
        _ => None,
    }
}

/// Map a key event to the caret-navigation / forward-delete message shared by
/// every single-line text field (title, todo, epic, base branch, repo-path
/// query, preset name, quick-dispatch query). Returns `None` for keys that are
/// not caret motions so the caller can handle them (Char/Backspace/Enter/Esc).
///
/// `Ctrl+←/→` are the primary word-motion keys; `Alt+←/→` and the readline
/// `Alt+B`/`Alt+F` are modifier-free fallbacks for terminals (notably tmux
/// without `xterm-keys`) that drop the Ctrl modifier on arrow keys.
fn text_edit_message(key: KeyEvent) -> Option<crate::tui::messages::InputMessage> {
    use crate::tui::messages::InputMessage;
    let ctrl = key.modifiers.contains(KeyModifiers::CONTROL);
    let alt = key.modifiers.contains(KeyModifiers::ALT);
    match key.code {
        KeyCode::Left if ctrl || alt => Some(InputMessage::CursorWordLeft),
        KeyCode::Right if ctrl || alt => Some(InputMessage::CursorWordRight),
        KeyCode::Left => Some(InputMessage::CursorLeft),
        KeyCode::Right => Some(InputMessage::CursorRight),
        KeyCode::Home => Some(InputMessage::CursorHome),
        KeyCode::End => Some(InputMessage::CursorEnd),
        KeyCode::Delete => Some(InputMessage::InputDeleteForward),
        KeyCode::Char('b') | KeyCode::Char('B') if alt => Some(InputMessage::CursorWordLeft),
        KeyCode::Char('f') | KeyCode::Char('F') if alt => Some(InputMessage::CursorWordRight),
        _ => None,
    }
}

impl App {
    /// Dispatch `msg` through [`Self::update`], then record the keybinding usage
    /// event. Collapses the update-then-`key_event`-push pattern shared by the
    /// message-dispatch arms of every key handler into a single call, so those
    /// arms can't silently forget the telemetry push. Arms that delegate to a
    /// `handle_key_*` sub-handler use [`Self::dispatch_handler_keyed`] instead.
    pub(in crate::tui) fn dispatch_keyed(
        &mut self,
        msg: Message,
        action: &str,
        key: &str,
    ) -> Vec<Command> {
        let mut cmds = self.update(msg);
        cmds.push(key_event(action, key));
        cmds
    }

    /// Run a `handle_key_*` sub-handler, then record the keybinding usage event
    /// only if the handler produced commands. Collapses the run-then-conditional-
    /// `key_event`-push pattern shared by the sub-handler arms (where a no-op
    /// handler must not emit telemetry), mirroring [`Self::dispatch_keyed`] for
    /// that cluster.
    pub(in crate::tui) fn dispatch_handler_keyed(
        &mut self,
        handler: impl FnOnce(&mut Self) -> Vec<Command>,
        action: &str,
        key: &str,
    ) -> Vec<Command> {
        let mut cmds = handler(self);
        if !cmds.is_empty() {
            cmds.push(key_event(action, key));
        }
        cmds
    }

    /// Translate a terminal key event into zero or more commands, depending on current mode.
    ///
    /// Always sets `self.dirty = true` after handling a key. An earlier revision tried to
    /// skip the redraw for no-op keys (e.g. `j` at the last row) by snapshotting which
    /// fields changed, but that opt-in mechanism proved fragile: popup/overlay handlers
    /// routinely mutate state invisible to the snapshot (tree-view open/collapse state,
    /// edit buffers, cursor positions in popups) and silently drop frames when they forget
    /// to set dirty themselves. The `frame_ready` 16ms cap already bounds the cost of
    /// redrawing on a true no-op, so unconditionally marking dirty is both correct and cheap.
    pub fn handle_key(&mut self, key: KeyEvent) -> Vec<Command> {
        // TEMPORARY: debugging a copy-task overlay that reportedly appears
        // without a 'c' keypress. Remove once root-caused.
        tracing::debug!(code = ?key.code, modifiers = ?key.modifiers, mode = ?self.input.mode, "handle_key");
        let cmds = if self.status.error_popup.is_some() {
            // Any key dismisses the popup, so the key that did it is the
            // interesting part of the record, not the action.
            self.dispatch_keyed(
                Message::System(crate::tui::messages::SystemMessage::DismissError),
                "dismiss_error",
                &key_label(key),
            )
        } else {
            match self.input.mode.clone() {
                InputMode::Normal => self.handle_key_normal(key),
                InputMode::SearchTasks => self.handle_key_search(key),
                InputMode::InputTitle
                | InputMode::InputDescription
                | InputMode::InputRepoPath
                | InputMode::InputEpicTitle
                | InputMode::InputEpicDescription
                | InputMode::InputBaseBranch
                | InputMode::MainSessionDir
                | InputMode::TodoTitle
                | InputMode::TodoQuickAdd => self.handle_key_text_input(key),
                InputMode::ConfirmDelete => self.handle_key_confirm_delete(key),
                InputMode::InputTag => self.handle_key_tag(key),
                InputMode::QuickDispatch => self.handle_key_quick_dispatch(key),
                InputMode::ConfirmRetry(id) => self.handle_key_confirm_retry(key, id),
                InputMode::ConfirmArchive(task_id) => self.handle_key_confirm_archive(key, task_id),
                InputMode::ConfirmDeleteEpic => self.handle_key_confirm_delete_epic(key),
                InputMode::ConfirmArchiveEpic => self.handle_key_confirm_archive_epic(key),

                InputMode::ConfirmDone => self.handle_key_confirm_done(key),
                InputMode::ConfirmDetachTmux(_) => self.handle_key_confirm_detach_tmux(key),
                InputMode::Help => self.handle_key_help(key),
                InputMode::RepoFilter => self.handle_key_repo_filter(key),
                InputMode::InputPresetName => self.handle_key_input_preset_name(key),
                InputMode::ConfirmDeletePreset => self.handle_key_confirm_delete_preset(key),
                InputMode::ConfirmDeleteRepoPath => self.handle_key_confirm_delete_repo_path(key),
                InputMode::ConfirmQuit => self.handle_key_confirm_quit(key),
                InputMode::InputWrapUpMode => self.handle_key_wrap_up_mode(key),
                InputMode::InputPhoenix => self.handle_key_phoenix(key),
                InputMode::ReparentEpic(_) => self.handle_key_reparent_epic(key),
                InputMode::ConfirmReparentEpic { .. } => self.handle_key_confirm_reparent_epic(key),
                InputMode::MoveTaskToEpic(_) => self.handle_key_move_task_to_epic(key),
                InputMode::ConfirmMoveTaskToEpic { .. } => {
                    self.handle_key_confirm_move_task_to_epic(key)
                }
                InputMode::ConfirmDeleteTodo => self.handle_key_confirm_delete_todo(key),
                InputMode::LinkTodoToTask(_) => self.handle_key_link_todo_to_task(key),
                InputMode::ConfirmTrustRepo { task_id, mode } => {
                    self.handle_key_confirm_trust_repo(key, task_id, mode)
                }
                InputMode::ConfirmTrustRepoQuickDispatch { draft, epic_id } => {
                    self.handle_key_confirm_trust_repo_quick_dispatch(key, draft, epic_id)
                }
                InputMode::ConfirmRepoSync { repo_path } => {
                    self.handle_key_confirm_repo_sync(key, repo_path)
                }
            }
        };

        self.dirty = true;
        cmds
    }

    pub(in crate::tui) fn handle_key_task_detail(&mut self, key: KeyEvent) -> Vec<Command> {
        let label = key_label(key);
        match key.code {
            KeyCode::Char('q') | KeyCode::Esc | KeyCode::Enter => {
                return self.dispatch_keyed(
                    Message::Task(crate::tui::messages::TaskMessage::CloseDetail),
                    "close_detail",
                    &label,
                );
            }
            KeyCode::Char('j') | KeyCode::Down => {
                if let ViewMode::TaskDetail {
                    scroll, max_scroll, ..
                } = &mut self.board.view_mode
                {
                    *scroll = scroll.saturating_add(1).min(*max_scroll);
                }
                return vec![key_event("scroll_detail", &label)];
            }
            KeyCode::Char('k') | KeyCode::Up => {
                if let ViewMode::TaskDetail { scroll, .. } = &mut self.board.view_mode {
                    *scroll = scroll.saturating_sub(1);
                }
                return vec![key_event("scroll_detail", &label)];
            }
            KeyCode::Char('z') => {
                if let ViewMode::TaskDetail { zoomed, .. } = &mut self.board.view_mode {
                    *zoomed = !*zoomed;
                }
                return vec![key_event("zoom_detail", &label)];
            }
            _ => {}
        }
        vec![]
    }

    /// Handle keys when the Archive column is focused.
    pub(in crate::tui) fn handle_key_archive(&mut self, key: KeyEvent) -> Vec<Command> {
        let label = key_label(key);
        match key.code {
            KeyCode::Char('j') | KeyCode::Down => {
                let count = self.archived_tasks().len();
                // An empty archive has no row to move to — nothing happened.
                if count == 0 {
                    return vec![];
                }
                let archive_col = TaskStatus::COLUMN_COUNT + 1;
                let next = (self.selection().row(archive_col) + 1).min(count - 1);
                self.selection_mut().set_row(archive_col, next);
                *self.archive.list_state.selected_mut() = Some(next);
                vec![key_event("archive_navigate_row", &label)]
            }
            KeyCode::Char('k') | KeyCode::Up => {
                if self.archived_tasks().is_empty() {
                    return vec![];
                }
                let archive_col = TaskStatus::COLUMN_COUNT + 1;
                let prev = self.selection().row(archive_col).saturating_sub(1);
                self.selection_mut().set_row(archive_col, prev);
                *self.archive.list_state.selected_mut() = Some(prev);
                vec![key_event("archive_navigate_row", &label)]
            }
            KeyCode::Char('h') | KeyCode::Left | KeyCode::Esc => {
                self.dispatch_keyed(Message::NavigateColumn(-1), "leave_archive", &label)
            }
            KeyCode::Char('x') => {
                let archived = self.archived_tasks();
                let Some(task) = archived.get(self.selected_archive_row()) else {
                    return vec![];
                };
                let title = super::truncate_title(&task.title, 30);
                self.input.mode = InputMode::ConfirmDelete;
                self.set_status(format!("Delete {title}? [y/n]"));
                vec![key_event("delete_archived", &label)]
            }
            KeyCode::Char('e') => {
                let archived = self.archived_tasks();
                if let Some(task) = archived
                    .get(self.selected_archive_row())
                    .map(|t| (*t).clone())
                {
                    vec![
                        Command::Editor(crate::tui::commands::EditorCommand::PopOut(
                            crate::tui::types::EditKind::TaskEdit(Box::new(task)),
                        )),
                        key_event("edit_archived", &label),
                    ]
                } else {
                    vec![]
                }
            }
            KeyCode::Char('q') => self.dispatch_keyed(
                Message::System(crate::tui::messages::SystemMessage::Quit),
                "quit",
                &label,
            ),
            KeyCode::Char('[') => self.dispatch_keyed(
                Message::NavigateRowFirst,
                "archive_navigate_row_first",
                &label,
            ),
            KeyCode::Char(']') => self.dispatch_keyed(
                Message::NavigateRowLast,
                "archive_navigate_row_last",
                &label,
            ),
            _ => vec![],
        }
    }

    /// `Space` — the unified "activate task" action (see
    /// docs/specs/split-pane.allium: JumpToAgentWindow). Priority order for a
    /// task: (1) pinned in the split pane → focus the pane; (2) split mode
    /// active and a live tmux window exists → swap that window into the pane
    /// in place, without transferring focus; (3) a live tmux window exists →
    /// jump to it (2 and 3 both win over the Stale/Crashed status check — a
    /// stale agent is usually just idle); (4) no window → route by status:
    /// Backlog dispatches, Running/Review/Done resumes (or opens the retry
    /// dialog for a windowless Stale/Crashed task), Archived shows a hint.
    /// Split mode overrides only the jump, so a windowless card still
    /// dispatches or resumes with the pane open.
    /// On an epic row it enters the epic view. Replaces the former split
    /// `d` (dispatch) / `Space` (jump) keys, and the retired `S` (swap) key.
    pub(in crate::tui) fn handle_key_activate(&mut self) -> Vec<Command> {
        match self.selected_column_item() {
            Some(ColumnItem::Task(task)) => {
                let id = task.id;

                // Priority 1: the task is pinned in the split pane — its window
                // was joined into the dispatch window via join-pane, so focus
                // the pane directly instead of the (now-absent) window.
                if self.board.split.active && self.board.split.pinned_task_id == Some(id) {
                    if let Some(pane_id) = self.board.split.right_pane_id.clone() {
                        return vec![
                            Command::Split(crate::tui::commands::SplitCommand::FocusPane {
                                pane_id,
                            }),
                            key_event("jump_to_tmux", " "),
                        ];
                    }
                }

                // Priority 2: split mode is open and this task has a window —
                // swap it into the pane in place rather than taking the user
                // away to it. Replaces the retired [S] key; see
                // SwapSplitPane in docs/specs/split-pane.allium.
                if self.board.split.active && task.tmux_window.is_some() {
                    return self.dispatch_handler_keyed(
                        |app| {
                            app.update(Message::Split(crate::tui::messages::SplitMessage::Swap(id)))
                        },
                        "swap_split_pane",
                        " ",
                    );
                }

                // Priority 3: a standalone window exists — jump to it.
                if let Some(window) = &task.tmux_window {
                    return vec![
                        Command::Task(crate::tui::commands::TaskCommand::JumpToTmux {
                            window: window.clone(),
                        }),
                        key_event("jump_to_tmux", " "),
                    ];
                }

                // Priority 4: no window — route by status.
                let status = task.status;
                let has_worktree = task.worktree.is_some();
                // Stale/Crashed, or Running with nothing provisioned behind it.
                // The latter never becomes Stale or Crashed — both tick
                // classifications skip windowless tasks — so without this it
                // has no in-place recovery at all. Running only: RetryFresh
                // refuses every other status, so widening further would open a
                // dialog that no-ops. See RetryReachableInPlace in
                // docs/specs/dispatch.allium.
                //
                // Excluded while a dispatch may still be in flight (see
                // App::dispatch_may_be_in_flight): RetryFresh would move the
                // task back to Backlog and fire a SECOND DispatchAgent
                // alongside the one already provisioning it.
                // DispatchingOutranksIt governs the key, not only the label.
                let now = chrono::Utc::now();
                let is_problematic = self.find_task(id).is_some_and(|t| {
                    t.sub_status == SubStatus::Stale
                        || t.sub_status == SubStatus::Crashed
                        || (t.status == TaskStatus::Running
                            && t.is_unprovisioned()
                            && !self.dispatch_may_be_in_flight(t, now))
                });

                match status {
                    TaskStatus::Backlog => {
                        let mode = DispatchMode::for_task(task);
                        let repo_path = task.repo_path.clone();
                        vec![
                            Command::Task(
                                crate::tui::commands::TaskCommand::CheckTrustAndDispatch {
                                    id,
                                    repo_path,
                                    mode,
                                },
                            ),
                            key_event("dispatch_task", " "),
                        ]
                    }
                    TaskStatus::Running | TaskStatus::Review | TaskStatus::Done => {
                        if is_problematic {
                            // Windowless Stale/Crashed, or an unprovisioned
                            // Running task: open the kill-and-retry dialog.
                            let mut cmds = self.update(Message::Task(
                                crate::tui::messages::TaskMessage::KillAndRetry(id),
                            ));
                            cmds.push(key_event("open_retry_dialog", " "));
                            cmds
                        } else if !has_worktree
                            && self
                                .find_task(id)
                                .is_some_and(|t| self.dispatch_may_be_in_flight(t, now))
                        {
                            // Space did something — it answered — even though
                            // the answer is "not yet". Counting it keeps the
                            // key's total honest about how often it is pressed.
                            self.dispatch_keyed(
                                Message::System(crate::tui::messages::SystemMessage::StatusInfo(
                                    "Dispatch in progress\u{2026}".to_string(),
                                )),
                                "activate_unavailable",
                                " ",
                            )
                        } else if has_worktree {
                            let mut cmds = self.update(Message::Task(
                                crate::tui::messages::TaskMessage::Resume(id),
                            ));
                            cmds.push(key_event("resume_task", " "));
                            cmds
                        } else {
                            self.dispatch_keyed(
                                Message::System(crate::tui::messages::SystemMessage::StatusInfo(
                                    "No worktree to resume, move to Backlog and re-dispatch"
                                        .to_string(),
                                )),
                                "activate_unavailable",
                                " ",
                            )
                        }
                    }
                    TaskStatus::Archived => self.dispatch_keyed(
                        Message::System(crate::tui::messages::SystemMessage::StatusInfo(
                            "Task is archived".to_string(),
                        )),
                        "activate_unavailable",
                        " ",
                    ),
                }
            }
            Some(ColumnItem::Epic(epic)) => {
                let id = epic.id;
                self.dispatch_keyed(
                    Message::Epic(crate::tui::messages::EpicMessage::Enter(id)),
                    "enter_epic",
                    " ",
                )
            }
            Some(
                ColumnItem::EpicHeader(_)
                | ColumnItem::SubstatusLabel(_)
                | ColumnItem::OrphanSeparator,
            ) => vec![],
            None => {
                if let Some(id) = self.selected_epic_id() {
                    self.dispatch_keyed(
                        Message::Epic(crate::tui::messages::EpicMessage::Enter(id)),
                        "enter_epic",
                        " ",
                    )
                } else {
                    vec![]
                }
            }
        }
    }

    /// Handle the 'L'/'H' keys: move selected task(s) forward or backward.
    /// (`m` is the move-to-epic tree picker, not a status move.)
    pub(in crate::tui) fn handle_key_move(&mut self, direction: MoveDirection) -> Vec<Command> {
        if self.has_selection() {
            if self.select.tasks.is_empty() {
                // Only epics selected — can't move since status is derived
                return self.update(Message::System(
                    crate::tui::messages::SystemMessage::StatusInfo(
                        "Epic status is derived from subtasks".to_string(),
                    ),
                ));
            }
            let ids: Vec<_> = self.select.tasks.iter().copied().collect();
            self.update(Message::Task(
                crate::tui::messages::TaskMessage::BatchMove { ids, direction },
            ))
        } else if let Some(task) = self.selected_task() {
            let id = task.id;
            self.update(Message::Task(crate::tui::messages::TaskMessage::Move {
                id,
                direction,
            }))
        } else {
            vec![]
        }
    }

    pub(in crate::tui) fn handle_key_text_input(&mut self, key: KeyEvent) -> Vec<Command> {
        // In picker modes (repo path, main-session dir, base branch), j/k
        // navigate the filtered candidate list.
        let is_picker_mode = self.picker_candidates().is_some();
        if is_picker_mode {
            match key.code {
                KeyCode::Down => {
                    return self.dispatch_keyed(
                        Message::RepoFilter(crate::tui::messages::RepoFilterMessage::MoveCursor(1)),
                        "picker_move_cursor",
                        "Down",
                    )
                }
                KeyCode::Up => {
                    return self.dispatch_keyed(
                        Message::RepoFilter(crate::tui::messages::RepoFilterMessage::MoveCursor(
                            -1,
                        )),
                        "picker_move_cursor",
                        "Up",
                    )
                }
                _ => {}
            }
        }
        // Caret navigation / forward-delete are shared across every text field.
        if let Some(msg) = text_edit_message(key) {
            return self.update(Message::Input(msg));
        }
        match key.code {
            KeyCode::Esc => self.dispatch_keyed(
                Message::Input(crate::tui::messages::InputMessage::CancelInput),
                "cancel_input",
                "Esc",
            ),
            // Typing is data entry, not a keybinding use: only the commit and
            // the cancel of a text mode are recorded.
            KeyCode::Enter => {
                let mut cmds = self.submit_text_input();
                cmds.push(key_event("submit_input", "Enter"));
                cmds
            }
            KeyCode::Backspace => self.update(Message::Input(
                crate::tui::messages::InputMessage::InputBackspace,
            )),
            KeyCode::Char(c) => self.update(Message::Input(
                crate::tui::messages::InputMessage::InputChar(c),
            )),
            _ => vec![],
        }
    }

    /// `Enter` in a text-entry mode: submit the picker selection when the mode
    /// has one, otherwise the trimmed buffer, routed by mode.
    fn submit_text_input(&mut self) -> Vec<Command> {
        // In picker modes, Enter selects the item at the cursor position in
        // the effective list (filtered candidates + optional new entry at
        // the end) — see docs/specs/dispatch.allium: RepoPathPicker,
        // BaseBranchPicker.
        if let Some(candidates) = self.picker_candidates() {
            let selected = super::resolve_picker_selection(
                candidates,
                &self.input.buffer,
                self.input.repo_cursor,
            );
            if let Some(value) = selected {
                let msg = match self.input.mode {
                    InputMode::InputBaseBranch => {
                        Message::Input(crate::tui::messages::InputMessage::SubmitBaseBranch(value))
                    }
                    InputMode::MainSessionDir => Message::MainSession(
                        crate::tui::messages::MainSessionMessage::SubmitDir(value),
                    ),
                    _ => Message::Input(crate::tui::messages::InputMessage::SubmitRepoPath(value)),
                };
                return self.update(msg);
            }
            // effective is empty — fall through to submit the empty buffer and
            // let the mode-specific submit handler apply its fallback/error.
        }
        let value = self.input.buffer.trim().to_string();
        match self.input.mode.clone() {
            InputMode::InputTitle => self.update(Message::Input(
                crate::tui::messages::InputMessage::SubmitTitle(value),
            )),
            InputMode::InputDescription => self.update(Message::Input(
                crate::tui::messages::InputMessage::SubmitDescription(value),
            )),
            InputMode::InputRepoPath => self.update(Message::Input(
                crate::tui::messages::InputMessage::SubmitRepoPath(value),
            )),
            InputMode::InputEpicTitle => self.update(Message::Epic(
                crate::tui::messages::EpicMessage::SubmitTitle(value),
            )),
            InputMode::InputEpicDescription => self.update(Message::Epic(
                crate::tui::messages::EpicMessage::SubmitDescription(value),
            )),
            InputMode::InputBaseBranch => self.update(Message::Input(
                crate::tui::messages::InputMessage::SubmitBaseBranch(value),
            )),
            InputMode::MainSessionDir => self.update(Message::MainSession(
                crate::tui::messages::MainSessionMessage::SubmitDir(value),
            )),
            InputMode::TodoTitle => self.update(Message::Todo(
                crate::tui::messages::TodoMessage::SubmitTitle(value),
            )),
            InputMode::TodoQuickAdd => self.update(Message::Todo(
                crate::tui::messages::TodoMessage::SubmitQuickAdd(value),
            )),
            _ => vec![],
        }
    }

    /// Shared key handling for single-character option pickers (tag,
    /// wrap-up mode, …): a printable char may select an option, `Enter`
    /// confirms the default, `Esc` cancels, anything else is a no-op.
    /// `select` maps the typed char to a message, or `None` to ignore it.
    /// Usage is recorded as `<action>_select` / `_default` / `_cancel`; a char
    /// the picker does not recognise records nothing.
    fn handle_char_picker(
        &mut self,
        key: KeyEvent,
        action: &str,
        select: impl FnOnce(char) -> Option<Message>,
        on_enter: Message,
        on_cancel: Message,
    ) -> Vec<Command> {
        match key.code {
            KeyCode::Char(c) => match select(c) {
                Some(msg) => self.dispatch_keyed(msg, &format!("{action}_select"), &c.to_string()),
                None => vec![],
            },
            KeyCode::Enter => self.dispatch_keyed(on_enter, &format!("{action}_default"), "Enter"),
            KeyCode::Esc => self.dispatch_keyed(on_cancel, &format!("{action}_cancel"), "Esc"),
            _ => vec![],
        }
    }

    pub(in crate::tui) fn handle_key_tag(&mut self, key: KeyEvent) -> Vec<Command> {
        use crate::tui::messages::InputMessage;
        self.handle_char_picker(
            key,
            "tag_picker",
            |c| {
                let tag = match c {
                    'b' => TaskTag::Bug,
                    'f' => TaskTag::Feature,
                    'c' => TaskTag::Chore,
                    'p' => TaskTag::PrReview,
                    'r' => TaskTag::Research,
                    'x' => TaskTag::Fix,
                    _ => return None,
                };
                Some(Message::Input(InputMessage::SubmitTag(Some(tag))))
            },
            Message::Input(InputMessage::SubmitTag(None)),
            Message::Input(InputMessage::CancelInput),
        )
    }

    pub(in crate::tui) fn handle_key_wrap_up_mode(&mut self, key: KeyEvent) -> Vec<Command> {
        use crate::models::WrapUpMode;
        use crate::tui::messages::InputMessage;
        self.handle_char_picker(
            key,
            "wrap_up_mode_picker",
            |c| {
                let mode = match c {
                    'r' => WrapUpMode::Rebase,
                    'p' => WrapUpMode::Pr,
                    'd' => WrapUpMode::Done,
                    _ => return None,
                };
                Some(Message::Input(InputMessage::SubmitWrapUpMode(Some(mode))))
            },
            Message::Input(InputMessage::SubmitWrapUpMode(None)),
            Message::Input(InputMessage::CancelInput),
        )
    }

    /// The creation form's last step (CreateTask in `docs/specs/tasks.allium`):
    /// a single-key picker arming the phoenix recurrence.
    ///
    /// Built on `handle_char_picker` like the tag and wrap-up steps before it,
    /// so declining still costs one keypress and no decision (Enter) while a
    /// stray key can no longer answer the question on the operator's behalf.
    pub(in crate::tui) fn handle_key_phoenix(&mut self, key: KeyEvent) -> Vec<Command> {
        use crate::tui::messages::InputMessage;
        self.handle_char_picker(
            key,
            "phoenix_picker",
            |c| (c == 'p').then_some(Message::Input(InputMessage::SubmitPhoenix(true))),
            Message::Input(InputMessage::SubmitPhoenix(false)),
            Message::Input(InputMessage::CancelInput),
        )
    }

    /// Quick-dispatch repo picker. Mirrors the shared RepoPathPicker
    /// surface contract (docs/specs/tasks.allium): every printable
    /// character filters; arrows navigate; Enter selects the cursor
    /// entry. No printable character is a navigation or select shortcut.
    pub(in crate::tui) fn handle_key_quick_dispatch(&mut self, key: KeyEvent) -> Vec<Command> {
        match key.code {
            KeyCode::Esc => self.dispatch_keyed(
                Message::Input(crate::tui::messages::InputMessage::CancelInput),
                "quick_dispatch_cancel",
                "Esc",
            ),
            KeyCode::Down => self.dispatch_keyed(
                Message::RepoFilter(crate::tui::messages::RepoFilterMessage::MoveCursor(1)),
                "quick_dispatch_move_cursor",
                "Down",
            ),
            KeyCode::Up => self.dispatch_keyed(
                Message::RepoFilter(crate::tui::messages::RepoFilterMessage::MoveCursor(-1)),
                "quick_dispatch_move_cursor",
                "Up",
            ),
            KeyCode::Enter => {
                let idx = self.input.repo_cursor;
                self.dispatch_keyed(
                    Message::Input(crate::tui::messages::InputMessage::SelectQuickDispatchRepo(
                        idx,
                    )),
                    "quick_dispatch_select",
                    "Enter",
                )
            }
            // Backspace/Char delegate to the shared edit handlers, which edit at
            // the caret and reset repo_cursor for QuickDispatch (a repo-picker
            // mode) — same path as the other text routers.
            KeyCode::Backspace => self.update(Message::Input(
                crate::tui::messages::InputMessage::InputBackspace,
            )),
            KeyCode::Char(c) if !key.modifiers.contains(KeyModifiers::ALT) => self.update(
                Message::Input(crate::tui::messages::InputMessage::InputChar(c)),
            ),
            _ => {
                if let Some(msg) = text_edit_message(key) {
                    return self.update(Message::Input(msg));
                }
                vec![]
            }
        }
    }

    pub(in crate::tui) fn handle_key_help(&mut self, key: KeyEvent) -> Vec<Command> {
        match key.code {
            KeyCode::Char('?') | KeyCode::Esc => self.dispatch_keyed(
                Message::System(crate::tui::messages::SystemMessage::ToggleHelp),
                "close_help",
                &key_label(key),
            ),
            _ => vec![],
        }
    }

    pub(in crate::tui) fn dispatch_selection<F, G>(
        &mut self,
        on_task: F,
        on_epic: G,
    ) -> Vec<Command>
    where
        F: FnOnce(&mut Self, TaskId) -> Vec<Command>,
        G: FnOnce(&mut Self, EpicId) -> Vec<Command>,
    {
        match self.selected_column_item() {
            Some(ColumnItem::Task(task)) => {
                let id = task.id;
                on_task(self, id)
            }
            Some(ColumnItem::Epic(epic)) => {
                let id = epic.id;
                on_epic(self, id)
            }
            Some(
                ColumnItem::EpicHeader(_)
                | ColumnItem::SubstatusLabel(_)
                | ColumnItem::OrphanSeparator,
            ) => vec![],
            None => vec![],
        }
    }

    /// Returns the ID of the currently selected epic, or `None` if the cursor is not on an epic.
    pub(in crate::tui) fn selected_epic_id(&self) -> Option<EpicId> {
        match self.selected_column_item() {
            Some(ColumnItem::Epic(epic)) => Some(epic.id),
            _ => None,
        }
    }

    /// Returns the epic ID when inside an epic view, or `None` in board view.
    pub(in crate::tui) fn current_epic_id(&self) -> Option<EpicId> {
        match &self.board.view_mode {
            ViewMode::Epic { epic_id, .. } => Some(*epic_id),
            _ => None,
        }
    }

    fn handle_key_reparent_epic(&mut self, key: KeyEvent) -> Vec<Command> {
        use crate::tui::messages::EpicMessage::ReparentNavigate;
        let label = key_label(key);
        if let Some(nav) = tree_nav_for(key) {
            return self.dispatch_keyed(
                Message::Epic(ReparentNavigate(nav)),
                "reparent_picker_navigate",
                &label,
            );
        }
        match key.code {
            KeyCode::Enter => self.dispatch_keyed(
                Message::Epic(crate::tui::messages::EpicMessage::ReparentConfirm),
                "reparent_picker_confirm",
                &label,
            ),
            KeyCode::Esc | KeyCode::Char('q') => self.dispatch_keyed(
                Message::Epic(crate::tui::messages::EpicMessage::ReparentCancel),
                "reparent_picker_cancel",
                &label,
            ),
            _ => vec![],
        }
    }

    fn handle_key_confirm_reparent_epic(&mut self, key: KeyEvent) -> Vec<Command> {
        let label = key_label(key);
        match key.code {
            KeyCode::Char('y') => self.dispatch_keyed(
                Message::Epic(crate::tui::messages::EpicMessage::ReparentExecute),
                "confirm_reparent_epic_yes",
                &label,
            ),
            KeyCode::Char('n') => self.dispatch_keyed(
                Message::Epic(crate::tui::messages::EpicMessage::ReparentCancel),
                "confirm_reparent_epic_no",
                &label,
            ),
            // Esc/q cancel entirely (not just back to picker)
            KeyCode::Esc | KeyCode::Char('q') => self.dispatch_keyed(
                Message::Epic(crate::tui::messages::EpicMessage::ReparentCancelAll),
                "confirm_reparent_epic_cancel_all",
                &label,
            ),
            _ => vec![],
        }
    }

    fn handle_key_move_task_to_epic(&mut self, key: KeyEvent) -> Vec<Command> {
        use crate::tui::messages::TaskMessage::MoveToEpicNavigate;
        let label = key_label(key);
        if let Some(nav) = tree_nav_for(key) {
            return self.dispatch_keyed(
                Message::Task(MoveToEpicNavigate(nav)),
                "move_to_epic_picker_navigate",
                &label,
            );
        }
        match key.code {
            KeyCode::Enter => self.dispatch_keyed(
                Message::Task(crate::tui::messages::TaskMessage::MoveToEpicConfirm),
                "move_to_epic_picker_confirm",
                &label,
            ),
            KeyCode::Esc | KeyCode::Char('q') => self.dispatch_keyed(
                Message::Task(crate::tui::messages::TaskMessage::MoveToEpicCancel),
                "move_to_epic_picker_cancel",
                &label,
            ),
            _ => vec![],
        }
    }

    fn handle_key_confirm_move_task_to_epic(&mut self, key: KeyEvent) -> Vec<Command> {
        let label = key_label(key);
        match key.code {
            KeyCode::Char('y') => self.dispatch_keyed(
                Message::Task(crate::tui::messages::TaskMessage::MoveToEpicExecute),
                "confirm_move_task_to_epic_yes",
                &label,
            ),
            KeyCode::Char('n') => self.dispatch_keyed(
                Message::Task(crate::tui::messages::TaskMessage::MoveToEpicCancel),
                "confirm_move_task_to_epic_no",
                &label,
            ),
            // Esc/q cancel entirely (not just back to picker)
            KeyCode::Esc | KeyCode::Char('q') => self.dispatch_keyed(
                Message::Task(crate::tui::messages::TaskMessage::MoveToEpicCancelAll),
                "confirm_move_task_to_epic_cancel_all",
                &label,
            ),
            _ => vec![],
        }
    }

    pub(in crate::tui) fn handle_key_link_todo_to_task(&mut self, key: KeyEvent) -> Vec<Command> {
        use crate::models::TodoLink;
        use crate::tui::commands::TodoCommand;
        use crate::tui::types::InputMode;
        match key.code {
            KeyCode::Enter => {
                let todo_id = match self.input.mode {
                    InputMode::LinkTodoToTask(id) => id,
                    _ => return vec![],
                };
                let linked = match self.selected_column_item() {
                    Some(ColumnItem::Task(t)) => Some(TodoLink::Task(t.id)),
                    Some(ColumnItem::Epic(e)) => Some(TodoLink::Epic(e.id)),
                    _ => return vec![], // nothing selectable focused
                };
                self.input.mode = InputMode::Normal;
                self.clear_status();
                vec![
                    Command::Todo(TodoCommand::Update {
                        id: todo_id,
                        update: crate::service::TodoUpdate {
                            linked: Some(linked),
                            ..Default::default()
                        },
                    }),
                    Command::Todo(TodoCommand::Load),
                    key_event("link_todo_confirm", "Enter"),
                ]
            }
            KeyCode::Esc => {
                self.input.mode = InputMode::Normal;
                self.clear_status();
                vec![
                    Command::Todo(crate::tui::commands::TodoCommand::Load),
                    key_event("link_todo_cancel", "Esc"),
                ]
            }
            _ => {
                // The picker borrows the board's own movement keys, so every
                // one of them records under a single navigate action.
                let msg = match key.code {
                    KeyCode::Char('h') | KeyCode::Left => Message::NavigateColumn(-1),
                    KeyCode::Char('l') | KeyCode::Right => Message::NavigateColumn(1),
                    KeyCode::Char('j') | KeyCode::Down => Message::NavigateRow(1),
                    KeyCode::Char('k') | KeyCode::Up => Message::NavigateRow(-1),
                    KeyCode::Char('g') => Message::NavigateRowFirst,
                    KeyCode::Char('G') => Message::NavigateRowLast,
                    _ => return vec![],
                };
                self.dispatch_keyed(msg, "link_todo_navigate", &key_label(key))
            }
        }
    }
}
