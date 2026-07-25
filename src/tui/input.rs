mod confirm;
mod managed_feeds;
mod normal;
mod repo_filter;

use crossterm::event::{KeyCode, KeyEvent, KeyModifiers};

use super::{App, ColumnItem, Command, InputMode, Message, MoveDirection, ViewMode};
use crate::models::{DispatchMode, EpicId, SubStatus, TaskId, TaskStatus, TaskTag, TipsShowMode};

fn key_event(action: &str, key: &str) -> Command {
    Command::RecordUsageEvent(crate::models::UsageEvent {
        category: crate::models::UsageCategory::Keybinding,
        action: action.to_string(),
        detail: Some(key.to_string()),
        actor: crate::models::UsageActor::Human,
    })
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
        let cmds = if self.status.error_popup.is_some() {
            self.update(Message::System(
                crate::tui::messages::SystemMessage::DismissError,
            ))
        } else if self.interaction.tips.is_some() {
            self.handle_key_tips(key)
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

                InputMode::ConfirmDone(_) => self.handle_key_confirm_done(key),
                InputMode::ConfirmWrapUp(_) => self.handle_key_confirm_wrap_up(key),
                InputMode::ConfirmEpicWrapUp(_) => self.handle_key_confirm_epic_wrap_up(key),
                InputMode::ConfirmDetachTmux(_) => self.handle_key_confirm_detach_tmux(key),
                InputMode::Help => self.handle_key_help(key),
                InputMode::RepoFilter => self.handle_key_repo_filter(key),
                InputMode::InputPresetName => self.handle_key_input_preset_name(key),
                InputMode::ConfirmDeletePreset => self.handle_key_confirm_delete_preset(key),
                InputMode::ConfirmDeleteRepoPath => self.handle_key_confirm_delete_repo_path(key),
                InputMode::ConfirmQuit => self.handle_key_confirm_quit(key),
                InputMode::InputWrapUpMode => self.handle_key_wrap_up_mode(key),
                InputMode::ReparentEpic(_) => self.handle_key_reparent_epic(key),
                InputMode::ConfirmReparentEpic { .. } => self.handle_key_confirm_reparent_epic(key),
                InputMode::MoveTaskToEpic(_) => self.handle_key_move_task_to_epic(key),
                InputMode::ConfirmMoveTaskToEpic { .. } => {
                    self.handle_key_confirm_move_task_to_epic(key)
                }
                InputMode::ManagedFeedConfig => self.handle_key_managed_feed_config(key),
                InputMode::ConfirmDeleteTodo => self.handle_key_confirm_delete_todo(key),
                InputMode::LinkTodoToTask(_) => self.handle_key_link_todo_to_task(key),
                InputMode::ConfirmTrustRepo { task_id, mode } => {
                    self.handle_key_confirm_trust_repo(key, task_id, mode)
                }
            }
        };

        self.dirty = true;
        cmds
    }

    pub(in crate::tui) fn handle_key_tips(&mut self, key: KeyEvent) -> Vec<Command> {
        match key.code {
            KeyCode::Char('l') => {
                let mut cmds = self.update(Message::Tips(crate::tui::messages::TipsMessage::Next));
                cmds.push(key_event("browse_tips_next", "l"));
                cmds
            }
            KeyCode::Right => {
                let mut cmds = self.update(Message::Tips(crate::tui::messages::TipsMessage::Next));
                cmds.push(key_event("browse_tips_next", "Right"));
                cmds
            }
            KeyCode::Char('h') => {
                let mut cmds = self.update(Message::Tips(crate::tui::messages::TipsMessage::Prev));
                cmds.push(key_event("browse_tips_prev", "h"));
                cmds
            }
            KeyCode::Left => {
                let mut cmds = self.update(Message::Tips(crate::tui::messages::TipsMessage::Prev));
                cmds.push(key_event("browse_tips_prev", "Left"));
                cmds
            }
            KeyCode::Char('n') => {
                let current_mode = self.interaction.tips.as_ref().map(|t| t.show_mode);
                let new_mode = match current_mode {
                    Some(TipsShowMode::NewOnly) => TipsShowMode::Always,
                    _ => TipsShowMode::NewOnly,
                };
                let label = match new_mode {
                    TipsShowMode::NewOnly => "Tips: will only show when there are new tips",
                    TipsShowMode::Always => "Tips: will show on every startup",
                    TipsShowMode::Never => {
                        unreachable!("n key only toggles between Always and NewOnly")
                    }
                };
                let mut cmds = self.update(Message::Tips(
                    crate::tui::messages::TipsMessage::SetMode(new_mode),
                ));
                cmds.extend(self.update(Message::System(
                    crate::tui::messages::SystemMessage::StatusInfo(label.to_string()),
                )));
                cmds.push(key_event("set_tips_mode", "n"));
                cmds
            }
            KeyCode::Char('x') => {
                let mut cmds = self.update(Message::Tips(
                    crate::tui::messages::TipsMessage::SetMode(TipsShowMode::Never),
                ));
                cmds.extend(self.update(Message::System(
                    crate::tui::messages::SystemMessage::StatusInfo(
                        "Tips: disabled on startup".to_string(),
                    ),
                )));
                cmds.push(key_event("disable_tips", "x"));
                cmds
            }
            KeyCode::Char('q') => {
                let mut cmds = self.update(Message::Tips(crate::tui::messages::TipsMessage::Close));
                cmds.push(key_event("close_tips", "q"));
                cmds
            }
            KeyCode::Esc => {
                let mut cmds = self.update(Message::Tips(crate::tui::messages::TipsMessage::Close));
                cmds.push(key_event("close_tips", "Esc"));
                cmds
            }
            _ => vec![],
        }
    }

    pub(in crate::tui) fn handle_key_task_detail(&mut self, key: KeyEvent) -> Vec<Command> {
        match key.code {
            KeyCode::Char('q') | KeyCode::Esc | KeyCode::Enter => {
                return self.update(Message::Task(
                    crate::tui::messages::TaskMessage::CloseDetail,
                ));
            }
            KeyCode::Char('j') | KeyCode::Down => {
                if let ViewMode::TaskDetail {
                    scroll, max_scroll, ..
                } = &mut self.board.view_mode
                {
                    *scroll = scroll.saturating_add(1).min(*max_scroll);
                }
            }
            KeyCode::Char('k') | KeyCode::Up => {
                if let ViewMode::TaskDetail { scroll, .. } = &mut self.board.view_mode {
                    *scroll = scroll.saturating_sub(1);
                }
            }
            KeyCode::Char('z') => {
                if let ViewMode::TaskDetail { zoomed, .. } = &mut self.board.view_mode {
                    *zoomed = !*zoomed;
                }
            }
            _ => {}
        }
        vec![]
    }

    /// Handle keys when the Archive column is focused.
    pub(in crate::tui) fn handle_key_archive(&mut self, key: KeyEvent) -> Vec<Command> {
        match key.code {
            KeyCode::Char('j') | KeyCode::Down => {
                let count = self.archived_tasks().len();
                if count > 0 {
                    let archive_col = TaskStatus::COLUMN_COUNT + 1;
                    let next = (self.selection().row(archive_col) + 1).min(count - 1);
                    self.selection_mut().set_row(archive_col, next);
                    *self.archive.list_state.selected_mut() = Some(next);
                }
                vec![]
            }
            KeyCode::Char('k') | KeyCode::Up => {
                let archive_col = TaskStatus::COLUMN_COUNT + 1;
                let prev = self.selection().row(archive_col).saturating_sub(1);
                self.selection_mut().set_row(archive_col, prev);
                *self.archive.list_state.selected_mut() = Some(prev);
                vec![]
            }
            KeyCode::Char('h') | KeyCode::Left | KeyCode::Esc => {
                self.update(Message::NavigateColumn(-1))
            }
            KeyCode::Char('x') => {
                let archived = self.archived_tasks();
                if let Some(task) = archived.get(self.selected_archive_row()) {
                    let title = super::truncate_title(&task.title, 30);
                    self.input.mode = InputMode::ConfirmDelete;
                    self.set_status(format!("Delete {title}? [y/n]"));
                }
                vec![]
            }
            KeyCode::Char('e') => {
                let archived = self.archived_tasks();
                if let Some(task) = archived
                    .get(self.selected_archive_row())
                    .map(|t| (*t).clone())
                {
                    vec![Command::Editor(
                        crate::tui::commands::EditorCommand::PopOut(
                            crate::tui::types::EditKind::TaskEdit(task),
                        ),
                    )]
                } else {
                    vec![]
                }
            }
            KeyCode::Char('q') => {
                self.update(Message::System(crate::tui::messages::SystemMessage::Quit))
            }
            KeyCode::Char('[') => self.update(Message::NavigateRowFirst),
            KeyCode::Char(']') => self.update(Message::NavigateRowLast),
            _ => vec![],
        }
    }

    /// `Space` — the unified "activate task" action (see
    /// docs/specs/split-pane.allium: JumpToAgentWindow). Priority order for a
    /// task: (1) pinned in the split pane → focus the pane; (2) a live tmux
    /// window exists → jump to it (this wins over the Stale/Crashed status
    /// check — a stale agent is usually just idle); (3) no window → route by
    /// status: Backlog dispatches, Running/Review/Done resumes (or opens the
    /// retry dialog for a windowless Stale/Crashed task), Archived shows a hint.
    /// On an epic row it enters the epic view. Replaces the former split
    /// `d` (dispatch) / `Space` (jump) keys.
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

                // Priority 2: a standalone window exists — jump to it.
                if let Some(window) = &task.tmux_window {
                    return vec![
                        Command::Task(crate::tui::commands::TaskCommand::JumpToTmux {
                            window: window.clone(),
                        }),
                        key_event("jump_to_tmux", " "),
                    ];
                }

                // Priority 3: no window — route by status.
                let status = task.status;
                let has_worktree = task.worktree.is_some();
                let is_problematic = self.find_task(id).is_some_and(|t| {
                    t.sub_status == SubStatus::Stale || t.sub_status == SubStatus::Crashed
                });

                match status {
                    TaskStatus::Backlog => {
                        let mode = DispatchMode::for_task(task);
                        let repo_path = task.repo_path.clone();
                        match crate::dispatch::is_repo_trusted(&repo_path) {
                            Err(e) => {
                                self.set_status(format!("Trust check failed: {e}"));
                                vec![]
                            }
                            Ok(true) => {
                                let mut cmds = self.update(Message::Task(
                                    crate::tui::messages::TaskMessage::Dispatch(id, mode),
                                ));
                                cmds.push(key_event("dispatch_task", " "));
                                cmds
                            }
                            Ok(false) => {
                                let expanded = crate::models::expand_tilde(&repo_path);
                                self.input.mode = InputMode::ConfirmTrustRepo { task_id: id, mode };
                                self.set_status(format!(
                                    "Repo '{expanded}' not trusted by Claude Code — trust it? [y/N]"
                                ));
                                vec![key_event("dispatch_task", " ")]
                            }
                        }
                    }
                    TaskStatus::Running | TaskStatus::Review | TaskStatus::Done => {
                        if is_problematic {
                            // Windowless Stale/Crashed: open the kill-and-retry
                            // dialog (the only path that still reaches it).
                            let mut cmds = self.update(Message::Task(
                                crate::tui::messages::TaskMessage::KillAndRetry(id),
                            ));
                            cmds.push(key_event("open_retry_dialog", " "));
                            cmds
                        } else if has_worktree {
                            let mut cmds = self.update(Message::Task(
                                crate::tui::messages::TaskMessage::Resume(id),
                            ));
                            cmds.push(key_event("resume_task", " "));
                            cmds
                        } else {
                            self.update(Message::System(
                                crate::tui::messages::SystemMessage::StatusInfo(
                                    "No worktree to resume, move to Backlog and re-dispatch"
                                        .to_string(),
                                ),
                            ))
                        }
                    }
                    TaskStatus::Archived => self.update(Message::System(
                        crate::tui::messages::SystemMessage::StatusInfo(
                            "Task is archived".to_string(),
                        ),
                    )),
                }
            }
            Some(ColumnItem::Epic(epic)) => {
                let id = epic.id;
                self.update(Message::Epic(crate::tui::messages::EpicMessage::Enter(id)))
            }
            Some(
                ColumnItem::EpicHeader(_)
                | ColumnItem::SubstatusLabel(_)
                | ColumnItem::OrphanSeparator,
            ) => vec![],
            None => {
                if let Some(id) = self.selected_epic_id() {
                    self.update(Message::Epic(crate::tui::messages::EpicMessage::Enter(id)))
                } else {
                    vec![]
                }
            }
        }
    }

    /// Handle 'm'/'M' key: move selected task(s) forward or backward.
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
                    return self.update(Message::RepoFilter(
                        crate::tui::messages::RepoFilterMessage::MoveCursor(1),
                    ))
                }
                KeyCode::Up => {
                    return self.update(Message::RepoFilter(
                        crate::tui::messages::RepoFilterMessage::MoveCursor(-1),
                    ))
                }
                _ => {}
            }
        }
        // Caret navigation / forward-delete are shared across every text field.
        if let Some(msg) = text_edit_message(key) {
            return self.update(Message::Input(msg));
        }
        match key.code {
            KeyCode::Esc => self.update(Message::Input(
                crate::tui::messages::InputMessage::CancelInput,
            )),
            KeyCode::Enter => {
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
                            InputMode::InputBaseBranch => Message::Input(
                                crate::tui::messages::InputMessage::SubmitBaseBranch(value),
                            ),
                            InputMode::MainSessionDir => Message::MainSession(
                                crate::tui::messages::MainSessionMessage::SubmitDir(value),
                            ),
                            _ => Message::Input(
                                crate::tui::messages::InputMessage::SubmitRepoPath(value),
                            ),
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
            KeyCode::Backspace => self.update(Message::Input(
                crate::tui::messages::InputMessage::InputBackspace,
            )),
            KeyCode::Char(c) => self.update(Message::Input(
                crate::tui::messages::InputMessage::InputChar(c),
            )),
            _ => vec![],
        }
    }

    /// Shared key handling for single-character option pickers (tag,
    /// wrap-up mode, …): a printable char may select an option, `Enter`
    /// confirms the default, `Esc` cancels, anything else is a no-op.
    /// `select` maps the typed char to a message, or `None` to ignore it.
    fn handle_char_picker(
        &mut self,
        key: KeyEvent,
        select: impl FnOnce(char) -> Option<Message>,
        on_enter: Message,
        on_cancel: Message,
    ) -> Vec<Command> {
        match key.code {
            KeyCode::Char(c) => match select(c) {
                Some(msg) => self.update(msg),
                None => vec![],
            },
            KeyCode::Enter => self.update(on_enter),
            KeyCode::Esc => self.update(on_cancel),
            _ => vec![],
        }
    }

    pub(in crate::tui) fn handle_key_tag(&mut self, key: KeyEvent) -> Vec<Command> {
        use crate::tui::messages::InputMessage;
        self.handle_char_picker(
            key,
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

    /// Quick-dispatch repo picker. Mirrors the shared RepoPathPicker
    /// surface contract (docs/specs/tasks.allium): every printable
    /// character filters; arrows navigate; Enter selects the cursor
    /// entry. No printable character is a navigation or select shortcut.
    pub(in crate::tui) fn handle_key_quick_dispatch(&mut self, key: KeyEvent) -> Vec<Command> {
        match key.code {
            KeyCode::Esc => self.update(Message::Input(
                crate::tui::messages::InputMessage::CancelInput,
            )),
            KeyCode::Down => self.update(Message::RepoFilter(
                crate::tui::messages::RepoFilterMessage::MoveCursor(1),
            )),
            KeyCode::Up => self.update(Message::RepoFilter(
                crate::tui::messages::RepoFilterMessage::MoveCursor(-1),
            )),
            KeyCode::Enter => {
                let idx = self.input.repo_cursor;
                self.update(Message::Input(
                    crate::tui::messages::InputMessage::SelectQuickDispatchRepo(idx),
                ))
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
            KeyCode::Char('?') | KeyCode::Esc => self.update(Message::System(
                crate::tui::messages::SystemMessage::ToggleHelp,
            )),
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
        use crate::tui::types::TreeNav;
        match key.code {
            KeyCode::Char('j') | KeyCode::Down => self.update(Message::Epic(
                crate::tui::messages::EpicMessage::ReparentNavigate(TreeNav::Down),
            )),
            KeyCode::Char('k') | KeyCode::Up => self.update(Message::Epic(
                crate::tui::messages::EpicMessage::ReparentNavigate(TreeNav::Up),
            )),
            KeyCode::Char('l') | KeyCode::Right | KeyCode::Char(' ') => self.update(Message::Epic(
                crate::tui::messages::EpicMessage::ReparentNavigate(TreeNav::Right),
            )),
            KeyCode::Char('h') | KeyCode::Left => self.update(Message::Epic(
                crate::tui::messages::EpicMessage::ReparentNavigate(TreeNav::Left),
            )),
            KeyCode::Enter => self.update(Message::Epic(
                crate::tui::messages::EpicMessage::ReparentConfirm,
            )),
            KeyCode::Esc | KeyCode::Char('q') => self.update(Message::Epic(
                crate::tui::messages::EpicMessage::ReparentCancel,
            )),
            _ => vec![],
        }
    }

    fn handle_key_confirm_reparent_epic(&mut self, key: KeyEvent) -> Vec<Command> {
        match key.code {
            KeyCode::Char('y') => self.update(Message::Epic(
                crate::tui::messages::EpicMessage::ReparentExecute,
            )),
            KeyCode::Char('n') => self.update(Message::Epic(
                crate::tui::messages::EpicMessage::ReparentCancel,
            )),
            // Esc/q cancel entirely (not just back to picker)
            KeyCode::Esc | KeyCode::Char('q') => self.update(Message::Epic(
                crate::tui::messages::EpicMessage::ReparentCancelAll,
            )),
            _ => vec![],
        }
    }

    fn handle_key_move_task_to_epic(&mut self, key: KeyEvent) -> Vec<Command> {
        use crate::tui::types::TreeNav;
        match key.code {
            KeyCode::Char('j') | KeyCode::Down => self.update(Message::Task(
                crate::tui::messages::TaskMessage::MoveToEpicNavigate(TreeNav::Down),
            )),
            KeyCode::Char('k') | KeyCode::Up => self.update(Message::Task(
                crate::tui::messages::TaskMessage::MoveToEpicNavigate(TreeNav::Up),
            )),
            KeyCode::Char('l') | KeyCode::Right | KeyCode::Char(' ') => self.update(Message::Task(
                crate::tui::messages::TaskMessage::MoveToEpicNavigate(TreeNav::Right),
            )),
            KeyCode::Char('h') | KeyCode::Left => self.update(Message::Task(
                crate::tui::messages::TaskMessage::MoveToEpicNavigate(TreeNav::Left),
            )),
            KeyCode::Enter => self.update(Message::Task(
                crate::tui::messages::TaskMessage::MoveToEpicConfirm,
            )),
            KeyCode::Esc | KeyCode::Char('q') => self.update(Message::Task(
                crate::tui::messages::TaskMessage::MoveToEpicCancel,
            )),
            _ => vec![],
        }
    }

    fn handle_key_confirm_move_task_to_epic(&mut self, key: KeyEvent) -> Vec<Command> {
        match key.code {
            KeyCode::Char('y') => self.update(Message::Task(
                crate::tui::messages::TaskMessage::MoveToEpicExecute,
            )),
            KeyCode::Char('n') => self.update(Message::Task(
                crate::tui::messages::TaskMessage::MoveToEpicCancel,
            )),
            // Esc/q cancel entirely (not just back to picker)
            KeyCode::Esc | KeyCode::Char('q') => self.update(Message::Task(
                crate::tui::messages::TaskMessage::MoveToEpicCancelAll,
            )),
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
                ]
            }
            KeyCode::Esc => {
                self.input.mode = InputMode::Normal;
                self.clear_status();
                vec![Command::Todo(crate::tui::commands::TodoCommand::Load)]
            }
            KeyCode::Char('h') | KeyCode::Left => self.update(Message::NavigateColumn(-1)),
            KeyCode::Char('l') | KeyCode::Right => self.update(Message::NavigateColumn(1)),
            KeyCode::Char('j') | KeyCode::Down => self.update(Message::NavigateRow(1)),
            KeyCode::Char('k') | KeyCode::Up => self.update(Message::NavigateRow(-1)),
            KeyCode::Char('g') => self.update(Message::NavigateRowFirst),
            KeyCode::Char('G') => self.update(Message::NavigateRowLast),
            _ => vec![],
        }
    }
}
