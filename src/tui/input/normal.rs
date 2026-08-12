//! Normal-mode (default board / epic view) key handler.

use std::time::Instant;

use crossterm::event::{KeyCode, KeyEvent};

use super::super::types::*;
use super::super::{App, PendingAction, GG_CHORD_TIMEOUT};

use super::key_event;

impl App {
    /// Return the id of the currently-selected todo item, or `None` if the list
    /// is empty or the view mode is not `Todos`.
    fn selected_todo_id(&self) -> Option<crate::models::TodoId> {
        if let ViewMode::Todos {
            todos, selected, ..
        } = &self.board.view_mode
        {
            todos.get(*selected).map(|t| t.id)
        } else {
            None
        }
    }

    pub(in crate::tui) fn handle_key_todos(&mut self, key: KeyEvent) -> Vec<Command> {
        use crate::tui::messages::TodoMessage;
        let label = super::key_label(key);
        match key.code {
            KeyCode::Char('j') | KeyCode::Down => self.dispatch_keyed(
                Message::Todo(TodoMessage::MoveSelection(1)),
                "todo_move_selection",
                &label,
            ),
            KeyCode::Char('k') | KeyCode::Up => self.dispatch_keyed(
                Message::Todo(TodoMessage::MoveSelection(-1)),
                "todo_move_selection",
                &label,
            ),
            KeyCode::Char('q') | KeyCode::Esc => {
                self.dispatch_keyed(Message::Todo(TodoMessage::Close), "close_todos", &label)
            }
            KeyCode::Char('a') => {
                self.dispatch_keyed(Message::Todo(TodoMessage::Add), "todo_add", &label)
            }
            KeyCode::Char('e') => {
                if let Some(id) = self.selected_todo_id() {
                    self.dispatch_keyed(Message::Todo(TodoMessage::Edit(id)), "todo_edit", &label)
                } else {
                    vec![]
                }
            }
            KeyCode::Char(' ') => {
                if let Some(id) = self.selected_todo_id() {
                    self.dispatch_keyed(
                        Message::Todo(TodoMessage::ToggleDone(id)),
                        "todo_toggle_done",
                        &label,
                    )
                } else {
                    vec![]
                }
            }
            KeyCode::Char('J') => self.dispatch_keyed(
                Message::Todo(TodoMessage::Reorder(1)),
                "todo_reorder",
                &label,
            ),
            KeyCode::Char('K') => self.dispatch_keyed(
                Message::Todo(TodoMessage::Reorder(-1)),
                "todo_reorder",
                &label,
            ),
            KeyCode::Char('c') => self.dispatch_keyed(
                Message::Todo(TodoMessage::ClearDone),
                "todo_clear_done",
                &label,
            ),
            KeyCode::Char('d') => {
                let Some(id) = self.selected_todo_id() else {
                    return vec![];
                };
                self.interaction.pending = PendingAction::TodoDelete(id);
                self.input.mode = crate::tui::types::InputMode::ConfirmDeleteTodo;
                vec![key_event("todo_delete_prompt", &label)]
            }
            KeyCode::Char('L') => {
                if let Some(id) = self.selected_todo_id() {
                    self.dispatch_keyed(
                        Message::Todo(crate::tui::messages::TodoMessage::LinkToTask(id)),
                        "todo_link_to_task",
                        &label,
                    )
                } else {
                    vec![]
                }
            }
            KeyCode::Char('U') => {
                use crate::tui::commands::TodoCommand;
                if let Some(id) = self.selected_todo_id() {
                    // No-op when the todo is already unlinked.
                    let is_linked = if let ViewMode::Todos { todos, .. } = &self.board.view_mode {
                        todos
                            .iter()
                            .find(|t| t.id == id)
                            .is_some_and(|t| t.linked.is_some())
                    } else {
                        false
                    };
                    if !is_linked {
                        return vec![];
                    }
                    // Optimistic in-memory clear
                    if let ViewMode::Todos { todos, .. } = &mut self.board.view_mode {
                        if let Some(t) = todos.iter_mut().find(|t| t.id == id) {
                            t.linked = None;
                        }
                    }
                    vec![
                        Command::Todo(TodoCommand::Update {
                            id,
                            update: crate::service::TodoUpdate {
                                linked: Some(None),
                                ..Default::default()
                            },
                        }),
                        key_event("todo_unlink", &label),
                    ]
                } else {
                    vec![]
                }
            }
            KeyCode::Enter | KeyCode::Char('g') => {
                let linked = self.selected_todo_id().and_then(|id| {
                    if let ViewMode::Todos { todos, .. } = &self.board.view_mode {
                        todos.iter().find(|t| t.id == id).and_then(|t| t.linked)
                    } else {
                        None
                    }
                });
                if let Some(link) = linked {
                    self.dispatch_keyed(
                        Message::Todo(crate::tui::messages::TodoMessage::JumpToLinked(link)),
                        "todo_jump_to_linked",
                        &label,
                    )
                } else {
                    vec![]
                }
            }
            KeyCode::Tab => {
                if let Some(id) = self.selected_todo_id() {
                    self.dispatch_keyed(
                        Message::Todo(crate::tui::messages::TodoMessage::Nest(id)),
                        "todo_nest",
                        &label,
                    )
                } else {
                    vec![]
                }
            }
            KeyCode::BackTab => {
                if let Some(id) = self.selected_todo_id() {
                    self.dispatch_keyed(
                        Message::Todo(crate::tui::messages::TodoMessage::Unnest(id)),
                        "todo_unnest",
                        &label,
                    )
                } else {
                    vec![]
                }
            }
            _ => vec![],
        }
    }

    pub(in crate::tui) fn handle_key_normal(&mut self, key: KeyEvent) -> Vec<Command> {
        // TaskDetail overlay captures all input when visible
        if matches!(self.board.view_mode, ViewMode::TaskDetail { .. }) {
            self.clear_pending_g_chord();
            return self.handle_key_task_detail(key);
        }

        // Todos overlay captures all input when visible
        if matches!(self.board.view_mode, ViewMode::Todos { .. }) {
            self.clear_pending_g_chord();
            return self.handle_key_todos(key);
        }

        if self.show_archived() {
            self.clear_pending_g_chord();
            return self.handle_key_archive(key);
        }

        self.handle_key_board_normal(key)
    }

    /// Abandon an armed `gg` chord if one is pending, leaving any other
    /// [`PendingAction`] untouched. Called on the overlay-entry guards where the
    // allow-phantom-symbol: removed field, cited as the behaviour this method preserves
    /// old code unconditionally cleared `pending_g`; scoping the clear to
    /// `GChord` preserves that exact semantics under the collapsed enum.
    fn clear_pending_g_chord(&mut self) {
        if matches!(self.interaction.pending, PendingAction::GChord(_)) {
            self.interaction.pending = PendingAction::None;
        }
    }

    /// The main board/epic key match, split out from [`Self::handle_key_normal`]
    /// so the `gg`-chord pre-check can recurse into it for the current key
    /// once a pending `g` has been resolved (see [`PendingAction::GChord`]).
    fn handle_key_board_normal(&mut self, key: KeyEvent) -> Vec<Command> {
        if let PendingAction::GChord(started) = self.interaction.pending {
            self.interaction.pending = PendingAction::None;
            if key.code == KeyCode::Char('g') && started.elapsed() <= GG_CHORD_TIMEOUT {
                // Completed `gg` chord: jump to top of column. Recorded under
                // the chord, not the key, so it stays separable from `[`.
                return self.dispatch_keyed(Message::NavigateRowFirst, "navigate_row_first", "gg");
            }
            // Either a different key arrived, or the chord window expired:
            // the pending chord is simply abandoned (no action fires for the
            // lone `g`), then this key is processed normally.
            return self.handle_key_board_normal(key);
        }

        let label = super::key_label(key);
        match key.code {
            KeyCode::Char('q') => {
                if matches!(self.board.view_mode, ViewMode::Epic { .. }) {
                    self.dispatch_keyed(
                        Message::Epic(crate::tui::messages::EpicMessage::Exit),
                        "exit_epic",
                        &label,
                    )
                } else {
                    self.dispatch_keyed(
                        Message::System(crate::tui::messages::SystemMessage::Quit),
                        "quit",
                        &label,
                    )
                }
            }

            KeyCode::Char('h') | KeyCode::Left => {
                self.dispatch_keyed(Message::NavigateColumn(-1), "navigate_column", &label)
            }
            KeyCode::Char('l') | KeyCode::Right => {
                self.dispatch_keyed(Message::NavigateColumn(1), "navigate_column", &label)
            }
            KeyCode::Char('j') | KeyCode::Down => {
                self.dispatch_keyed(Message::NavigateRow(1), "navigate_row", &label)
            }
            KeyCode::Char('k') | KeyCode::Up => {
                self.dispatch_keyed(Message::NavigateRow(-1), "navigate_row", &label)
            }
            KeyCode::Char('[') => {
                self.dispatch_keyed(Message::NavigateRowFirst, "navigate_row_first", &label)
            }
            KeyCode::Char(']') => {
                self.dispatch_keyed(Message::NavigateRowLast, "navigate_row_last", &label)
            }
            KeyCode::Char('J') => self.dispatch_keyed(
                Message::Task(crate::tui::messages::TaskMessage::ReorderItem(1)),
                "reorder_task_down",
                "J",
            ),
            KeyCode::Char('K') => self.dispatch_keyed(
                Message::Task(crate::tui::messages::TaskMessage::ReorderItem(-1)),
                "reorder_task_up",
                "K",
            ),

            KeyCode::Char('n') => self.dispatch_keyed(
                Message::Input(crate::tui::messages::InputMessage::StartNewTask),
                "create_task",
                "n",
            ),
            KeyCode::Char('c') => self.dispatch_keyed(
                Message::Input(crate::tui::messages::InputMessage::CopyTask),
                "copy_task",
                "c",
            ),
            KeyCode::Char('N') => self.dispatch_keyed(
                Message::System(crate::tui::messages::SystemMessage::ToggleNotifications),
                "toggle_notifications",
                "N",
            ),
            KeyCode::Char('E') => self.dispatch_keyed(
                Message::Epic(crate::tui::messages::EpicMessage::StartNew),
                "create_epic",
                "E",
            ),
            KeyCode::Char('f') => self.dispatch_keyed(
                Message::RepoFilter(crate::tui::messages::RepoFilterMessage::Start),
                "filter_repos",
                "f",
            ),
            KeyCode::Char('/') => {
                self.search.saved = Some(self.search.query.clone());
                self.input.mode = InputMode::SearchTasks;
                vec![key_event("search_tasks", "/")]
            }
            KeyCode::Char('L') => {
                if let Some(id) = self.selected_epic_id() {
                    return self.dispatch_keyed(
                        Message::Epic(crate::tui::messages::EpicMessage::MoveStatus(
                            id,
                            MoveDirection::Forward,
                        )),
                        "move_task_forward",
                        "L",
                    );
                }
                let mut cmds = self.handle_key_move(MoveDirection::Forward);
                cmds.push(key_event("move_task_forward", "L"));
                cmds
            }
            KeyCode::Char('H') => {
                if let Some(id) = self.selected_epic_id() {
                    return self.dispatch_keyed(
                        Message::Epic(crate::tui::messages::EpicMessage::MoveStatus(
                            id,
                            MoveDirection::Backward,
                        )),
                        "move_task_backward",
                        "H",
                    );
                }
                let mut cmds = self.handle_key_move(MoveDirection::Backward);
                cmds.push(key_event("move_task_backward", "H"));
                cmds
            }

            KeyCode::Char(':') => {
                // The runtime decides: jump to the main-session window if it is
                // alive, otherwise open the picker to (re)select a directory.
                vec![
                    Command::MainSession(crate::tui::commands::MainSessionCommand::Open),
                    key_event("open_main_session", ":"),
                ]
            }

            // [o] for origin: open the sync confirmation for the selected task's
            // repository (docs/specs/repo-sync.allium: rule PromptRepoSync).
            // Offered only while the drift segment is lit; with no drift the key
            // does nothing. [O] is left unbound for a future sync-all.
            KeyCode::Char('o') => self.dispatch_keyed(
                Message::RepoSync(crate::tui::messages::RepoSyncMessage::OpenPrompt),
                "open_repo_sync_prompt",
                "o",
            ),

            KeyCode::Char('g') => {
                // Start a pending `gg` chord; resolved by the next keypress
                // (above) or by `handle_tick` if the user goes idle.
                self.interaction.pending = PendingAction::GChord(Instant::now());
                vec![]
            }
            KeyCode::Char('G') => {
                self.dispatch_keyed(Message::NavigateRowLast, "navigate_row_last", &label)
            }

            KeyCode::Char('p') => {
                self.dispatch_handler_keyed(Self::handle_key_open_pr, "open_pr_url", "p")
            }
            KeyCode::Char('a') => self.dispatch_keyed(Message::SelectAllColumn, "select_all", "a"),

            KeyCode::Char('v') => {
                let mut cmds = self.dispatch_selection(
                    |s, id| {
                        s.update(Message::Task(
                            crate::tui::messages::TaskMessage::ToggleSelect(id),
                        ))
                    },
                    |s, id| {
                        s.update(Message::Epic(
                            crate::tui::messages::EpicMessage::ToggleSelect(id),
                        ))
                    },
                );
                cmds.push(key_event("toggle_select", "v"));
                cmds
            }

            KeyCode::Char(' ') => self.handle_key_activate(),

            KeyCode::Enter => self.handle_key_enter_normal(),

            KeyCode::Char('e') => {
                self.dispatch_handler_keyed(Self::handle_key_edit, "edit_task", "e")
            }

            KeyCode::Char('x') => {
                self.dispatch_handler_keyed(Self::handle_key_archive_item, "archive_task", "x")
            }

            KeyCode::Char('D') => {
                let mut cmds = self.handle_key_quick_dispatch_trigger();
                cmds.push(key_event("quick_dispatch", "D"));
                cmds
            }

            KeyCode::Char('U') => {
                if let Some(id) = self.current_epic_id() {
                    self.dispatch_keyed(
                        Message::Epic(crate::tui::messages::EpicMessage::ToggleAutoDispatch(id)),
                        "toggle_auto_dispatch",
                        "U",
                    )
                } else {
                    vec![]
                }
            }

            KeyCode::Char('R') => {
                if let Some(id) = self.current_epic_id() {
                    self.dispatch_keyed(
                        Message::Epic(crate::tui::messages::EpicMessage::ToggleGroupByRepo(id)),
                        "toggle_group_by_repo",
                        "R",
                    )
                } else {
                    vec![]
                }
            }

            KeyCode::Char('A') => self.dispatch_keyed(
                Message::RepoFilter(crate::tui::messages::RepoFilterMessage::ToggleOnlyActive),
                "filter_active",
                "A",
            ),

            KeyCode::Char('F') => self.dispatch_keyed(
                Message::Task(crate::tui::messages::TaskMessage::ToggleFlattened),
                "toggle_flattened",
                "F",
            ),

            KeyCode::Char('P') => self.dispatch_keyed(
                Message::Todo(crate::tui::messages::TodoMessage::Open),
                "open_todos",
                "P",
            ),

            KeyCode::Char('t') => {
                use crate::models::TodoLink;
                use crate::tui::messages::TodoMessage;
                use crate::tui::types::ColumnItem;
                let (title, linked) = match self.selected_column_item() {
                    Some(ColumnItem::Task(t)) => (t.title.clone(), TodoLink::Task(t.id)),
                    Some(ColumnItem::Epic(e)) => (e.title.clone(), TodoLink::Epic(e.id)),
                    _ => return vec![], // no selection — no-op
                };
                self.dispatch_keyed(
                    Message::Todo(TodoMessage::QuickAdd {
                        title,
                        linked: Some(linked),
                    }),
                    "todo_quick_add",
                    "t",
                )
            }

            KeyCode::Char('?') => self.dispatch_keyed(
                Message::System(crate::tui::messages::SystemMessage::ToggleHelp),
                "toggle_help",
                "?",
            ),

            KeyCode::Char('s') => self.dispatch_keyed(
                Message::Split(crate::tui::messages::SplitMessage::Toggle),
                "toggle_split_mode",
                "s",
            ),

            KeyCode::Char('T') => {
                self.dispatch_handler_keyed(Self::handle_key_detach, "detach_tmux", "T")
            }

            KeyCode::Char('r') => {
                self.dispatch_handler_keyed(Self::handle_key_feed_refresh, "refresh_feed", "r")
            }

            KeyCode::Char('m') => {
                if let Some(id) = self.selected_epic_id() {
                    self.dispatch_keyed(
                        Message::Epic(crate::tui::messages::EpicMessage::StartReparent(id)),
                        "reparent_epic",
                        "m",
                    )
                } else if let Some(task) = self.selected_task() {
                    // `m` on a task card moves it to another epic (or detaches it).
                    if task.status == crate::models::TaskStatus::Archived {
                        return vec![];
                    }
                    let id = task.id;
                    self.dispatch_keyed(
                        Message::Task(crate::tui::messages::TaskMessage::StartMoveToEpic(id)),
                        "move_task_to_epic",
                        "m",
                    )
                } else {
                    vec![]
                }
            }

            KeyCode::Esc => self.handle_key_esc_normal(),

            _ => vec![],
        }
    }

    /// `'p'` — open the selected task's PR URL in the browser.
    fn handle_key_open_pr(&mut self) -> Vec<Command> {
        if let Some(task) = self.selected_task() {
            if let Some(u) = &task.url {
                vec![Command::System(
                    crate::tui::commands::SystemCommand::OpenInBrowser { url: u.url.clone() },
                )]
            } else {
                self.update(Message::System(
                    crate::tui::messages::SystemMessage::StatusInfo("No URL set".to_string()),
                ))
            }
        } else {
            vec![]
        }
    }

    /// `Enter` — open task detail, or toggle off select-all.
    fn handle_key_enter_normal(&mut self) -> Vec<Command> {
        if self.selection().on_select_all {
            return self.dispatch_keyed(Message::SelectAllColumn, "clear_select_all", "Enter");
        }
        if let Some(task) = self.selected_task() {
            let id = task.id;
            let mut cmds = self.update(Message::Task(
                crate::tui::messages::TaskMessage::OpenDetail(id),
            ));
            cmds.push(key_event("open_task_detail", "Enter"));
            return cmds;
        }
        vec![]
    }

    /// `'e'` — edit the selected task or epic.
    fn handle_key_edit(&mut self) -> Vec<Command> {
        match self.selected_column_item() {
            Some(ColumnItem::Task(task)) => {
                vec![Command::Editor(
                    crate::tui::commands::EditorCommand::PopOut(
                        crate::tui::types::EditKind::TaskEdit(task.clone()),
                    ),
                )]
            }
            Some(ColumnItem::Epic(epic)) => {
                let id = epic.id;
                self.update(Message::Epic(crate::tui::messages::EpicMessage::Edit(id)))
            }
            Some(
                ColumnItem::EpicHeader(_)
                | ColumnItem::SubstatusLabel(_)
                | ColumnItem::OrphanSeparator,
            ) => vec![],
            None => {
                if let Some(id) = self.current_epic_id() {
                    self.update(Message::Epic(crate::tui::messages::EpicMessage::Edit(id)))
                } else {
                    vec![]
                }
            }
        }
    }

    /// `'x'` — complete the selected task(s), or archive them once Done.
    ///
    /// Completing is the common case and archiving the exception, so 'x'
    /// only archives a task that already sits in Done; anything else moves
    /// straight to Done via the ConfirmDone prompt. Epics always archive,
    /// including a multi-selection that contains one.
    fn handle_key_archive_item(&mut self) -> Vec<Command> {
        if self.has_selection() {
            if self.select.epics.is_empty() {
                let not_done: Vec<_> = self
                    .select
                    .tasks
                    .iter()
                    .copied()
                    .filter(|id| {
                        self.find_task(*id)
                            .is_some_and(|t| t.status != crate::models::TaskStatus::Done)
                    })
                    .collect();
                if !not_done.is_empty() {
                    self.prompt_move_to_done(not_done);
                    return vec![];
                }
            }
            let count = self.select.tasks.len() + self.select.epics.len();
            self.input.mode = InputMode::ConfirmArchive(None);
            self.set_status(format!("Archive {} items? [y/n]", count));
            vec![]
        } else {
            match self.selected_column_item() {
                Some(ColumnItem::Epic(_)) => self.update(Message::Epic(
                    crate::tui::messages::EpicMessage::ConfirmArchive,
                )),
                _ => {
                    if let Some(task) = self.selected_task() {
                        let id = task.id;
                        if task.status != crate::models::TaskStatus::Done {
                            self.prompt_move_to_done(vec![id]);
                            return vec![];
                        }
                        self.input.mode = InputMode::ConfirmArchive(Some(id));
                        self.set_status("Archive task? [y/n]".to_string());
                        vec![]
                    } else {
                        vec![]
                    }
                }
            }
        }
    }

    /// `'D'` — quick-dispatch: immediate for 1 repo, picker for multiple, error for none.
    fn handle_key_quick_dispatch_trigger(&mut self) -> Vec<Command> {
        let epic_id = self.current_epic_id();
        self.input.pending_epic_id = epic_id;
        match self.board.repo_paths.len() {
            1 => {
                let repo_path = self.board.repo_paths[0].clone();
                self.update(Message::Task(
                    crate::tui::messages::TaskMessage::QuickDispatch { repo_path, epic_id },
                ))
            }
            _ => self.update(Message::Input(
                crate::tui::messages::InputMessage::StartQuickDispatchSelection,
            )),
        }
    }

    /// `'T'` — detach tmux window(s): batch if selection active, single otherwise.
    fn handle_key_detach(&mut self) -> Vec<Command> {
        if !self.select.tasks.is_empty() {
            let ids: Vec<_> = self.select.tasks.iter().copied().collect();
            self.update(Message::Task(
                crate::tui::messages::TaskMessage::BatchDetachTmux(ids),
            ))
        } else if let Some(task) = self.selected_task() {
            if task.tmux_window.is_some() {
                let id = task.id;
                self.update(Message::Task(
                    crate::tui::messages::TaskMessage::DetachTmux(id),
                ))
            } else {
                vec![]
            }
        } else {
            vec![]
        }
    }

    /// `'r'` — trigger feed refresh for the selected or current epic.
    fn handle_key_feed_refresh(&mut self) -> Vec<Command> {
        let feed_epic_id = match self.selected_column_item() {
            Some(ColumnItem::Epic(e)) if e.feed_command.is_some() => Some(e.id),
            _ => None,
        }
        .or_else(|| {
            self.current_epic_id().and_then(|id| {
                self.find_epic(id)
                    .filter(|e| e.feed_command.is_some())
                    .map(|e| e.id)
            })
        });
        if let Some(id) = feed_epic_id {
            self.update(Message::Feed(
                crate::tui::messages::FeedMessage::TriggerEpic(id),
            ))
        } else {
            vec![]
        }
    }

    /// `Esc` — clear an active search, exit epic view, clear selection, or no-op.
    fn handle_key_esc_normal(&mut self) -> Vec<Command> {
        if self.search_active() {
            self.search.query.clear();
            self.sync_board_selection();
            return vec![key_event("clear_search", "Esc")];
        }
        if matches!(self.board.view_mode, ViewMode::Epic { .. }) {
            self.dispatch_keyed(
                Message::Epic(crate::tui::messages::EpicMessage::Exit),
                "exit_epic",
                "Esc",
            )
        } else if self.has_selection() || self.selection().on_select_all {
            self.dispatch_keyed(Message::ClearSelection, "clear_selection", "Esc")
        } else {
            vec![]
        }
    }

    pub(in crate::tui) fn handle_key_search(&mut self, key: KeyEvent) -> Vec<Command> {
        // Typing is not an action: only leaving the mode (either way) is
        // recorded, so the count reads as "searches run", not "characters
        // typed".
        let mut cmds = vec![];
        match key.code {
            KeyCode::Esc => {
                self.search.query = self.search.saved.take().unwrap_or_default();
                self.input.mode = InputMode::Normal;
                cmds.push(key_event("search_cancel", "Esc"));
            }
            KeyCode::Enter => {
                self.search.saved = None;
                self.input.mode = InputMode::Normal;
                cmds.push(key_event("search_commit", "Enter"));
            }
            KeyCode::Backspace => {
                self.search.query.pop();
            }
            KeyCode::Char(c) => {
                self.search.query.push(c);
            }
            _ => return vec![],
        }
        // Query may have changed → recompute filtered columns and re-clamp the cursor.
        self.sync_board_selection();
        cmds
    }
}
