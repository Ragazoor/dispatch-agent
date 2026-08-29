// ---------------------------------------------------------------------------
// commands::dispatch — Command → side-effect coverage
// ---------------------------------------------------------------------------
//
// `commands::dispatch` (`src/runtime/commands.rs`) is the single entry point
// `execute_commands` uses to turn a `Command` into a side effect. The rest of
// this file exercises the `exec_*` handlers directly, which leaves the
// dispatcher itself — the wiring from variant to handler, and which arms feed
// follow-on commands back into the queue — unexecuted.
//
// Assertions here are on *observable effects* (DB rows, `App` state, returned
// follow-on commands), never on the shape of the match, so they survive a
// refactor of the dispatcher's internals.
use super::*;
use crate::models::test_tmux_window;
use crate::tui::commands::{
    EditorCommand, FeedCommand, MainSessionCommand, PersistFields, PrCommand, RepoSyncCommand,
    SplitCommand, SystemCommand, TaskCommand, TodoCommand,
};

/// Run one command through the real dispatcher and return its follow-on
/// commands (the vec `execute_commands` extends its queue with).
async fn dispatch_one(rt: &TuiRuntime, app: &mut App, cmd: Command) -> Vec<Command> {
    commands::dispatch(cmd, app, rt).await
}

/// Mirror of `execute_commands`' drain loop without the terminal/key-rx
/// plumbing: run `cmd` and every command it cascades into.
async fn drain(rt: &TuiRuntime, app: &mut App, cmd: Command) {
    let mut queue = std::collections::VecDeque::from(vec![cmd]);
    while let Some(command) = queue.pop_front() {
        queue.extend(commands::dispatch(command, app, rt).await);
    }
}

async fn seed(rt: &TuiRuntime, title: &str, status: models::TaskStatus) -> models::Task {
    create_task_returning(&**rt.db_write(), title, "desc", "/repo", None, status)
        .await
        .unwrap()
}

#[tokio::test]
async fn dispatch_save_repo_path_persists_and_updates_app() {
    let (rt, mut app) = test_runtime().await;

    let extra = dispatch_one(
        &rt,
        &mut app,
        Command::Settings(SettingsCommand::SaveRepoPath("/some/repo".into())),
    )
    .await;

    assert!(
        extra.is_empty(),
        "SaveRepoPath produces no follow-on commands"
    );
    assert!(
        rt.database
            .list_repo_paths()
            .await
            .unwrap()
            .contains(&"/some/repo".to_string()),
        "repo path should be persisted"
    );
    assert!(
        app.repo_paths().contains(&"/some/repo".to_string()),
        "App should have been refreshed with the new path"
    );
}

#[tokio::test]
async fn dispatch_save_base_branch_persists_and_updates_app() {
    let (rt, mut app) = test_runtime().await;

    dispatch_one(
        &rt,
        &mut app,
        Command::Settings(SettingsCommand::SaveBaseBranch(
            "/some/repo".into(),
            "develop".into(),
        )),
    )
    .await;

    let pairs = rt.database.list_all_base_branches().await.unwrap();
    assert!(
        pairs
            .iter()
            .any(|(repo, branch)| repo == "/some/repo" && branch == "develop"),
        "base branch should be persisted, got {pairs:?}"
    );
}

#[tokio::test]
async fn dispatch_task_persist_writes_status_to_db() {
    let (rt, mut app) = test_runtime().await;
    let mut task = seed(&rt, "Persist me", models::TaskStatus::Backlog).await;
    // Mirror what every production status move does: the sub-status travels
    // with the status (`SubStatus::default_for`), because the service
    // rejects any (status, sub_status) pair `SubStatus::is_valid_for`
    // disallows.
    task.status = models::TaskStatus::Running;
    task.sub_status = models::SubStatus::default_for(models::TaskStatus::Running);
    task.worktree = Some("/wt".into());

    dispatch_one(
        &rt,
        &mut app,
        Command::Task(TaskCommand::Persist(PersistFields::from_task(&task))),
    )
    .await;

    let stored = rt.database.get_task(task.id).await.unwrap().unwrap();
    assert_eq!(stored.status, models::TaskStatus::Running);
    assert_eq!(stored.sub_status, models::SubStatus::Active);
    assert_eq!(stored.worktree.as_deref(), Some("/wt"));
    assert!(
        app.dirty_since_refresh,
        "a successful persist must mark the board dirty so the next tick refreshes"
    );
}

#[tokio::test]
async fn dispatch_task_persist_surfaces_service_rejection_and_writes_nothing() {
    let (rt, mut app) = test_runtime().await;
    let mut task = seed(&rt, "Persist me", models::TaskStatus::Backlog).await;
    // Running + SubStatus::None is rejected by `SubStatus::is_valid_for`, so
    // the whole patch is refused — including the worktree field.
    task.status = models::TaskStatus::Running;
    task.worktree = Some("/wt".into());

    dispatch_one(
        &rt,
        &mut app,
        Command::Task(TaskCommand::Persist(PersistFields::from_task(&task))),
    )
    .await;

    let stored = rt.database.get_task(task.id).await.unwrap().unwrap();
    assert_eq!(stored.status, models::TaskStatus::Backlog);
    assert_eq!(stored.worktree, None);
    let err = app.error_popup().unwrap_or_default();
    assert!(
        err.contains("persisting task"),
        "the rejection must reach the user as an error popup, got {err:?}"
    );
}

#[tokio::test]
async fn dispatch_task_delete_removes_the_row() {
    let (rt, mut app) = test_runtime().await;
    let task = seed(&rt, "Delete me", models::TaskStatus::Backlog).await;

    dispatch_one(&rt, &mut app, Command::Task(TaskCommand::Delete(task.id))).await;

    assert!(
        rt.database.get_task(task.id).await.unwrap().is_none(),
        "the task row should be gone"
    );
}

/// `ClearWorktreePointer` is the write a *successful* teardown earns: it is the
/// only thing that clears the worktree column on the archive path.
#[tokio::test]
async fn dispatch_task_clear_worktree_pointer_clears_both_pointers() {
    let (rt, mut app) = test_runtime().await;
    let task = seed(&rt, "Torn down", models::TaskStatus::Archived).await;
    rt.db_write()
        .patch_task(
            task.id,
            &db::TaskPatch::new()
                .worktree(Some("/repo/.worktrees/1-torn-down"))
                .tmux_window(Some(&test_tmux_window("task-1"))),
        )
        .await
        .unwrap();

    dispatch_one(
        &rt,
        &mut app,
        Command::Task(TaskCommand::ClearWorktreePointer(task.id)),
    )
    .await;

    let stored = rt.database.get_task(task.id).await.unwrap().unwrap();
    assert_eq!(stored.worktree, None);
    assert_eq!(stored.tmux_window, None);
}

#[tokio::test]
async fn dispatch_task_move_to_epic_reparents_and_cascades_a_refresh() {
    let (rt, mut app) = test_runtime().await;
    let task = seed(&rt, "Adopt me", models::TaskStatus::Backlog).await;
    let epic_id = rt
        .db_write()
        .create_epic("Parent epic", "desc", None)
        .await
        .unwrap()
        .id;

    let extra = dispatch_one(
        &rt,
        &mut app,
        Command::Task(TaskCommand::MoveToEpic {
            id: task.id,
            new_epic: Some(epic_id),
        }),
    )
    .await;

    let stored = rt.database.get_task(task.id).await.unwrap().unwrap();
    assert_eq!(
        stored.epic_id,
        Some(epic_id),
        "the task should now belong to the epic"
    );
    // MoveToEpic chains into exec_refresh_from_db, so App must see the move
    // without any further command being dispatched by the caller.
    assert!(
        app.tasks().iter().any(|t| t.id == task.id),
        "the refreshed board should contain the moved task"
    );
    // Any follow-on commands the refresh produced must be drainable, not
    // silently dropped.
    for cmd in extra {
        drain(&rt, &mut app, cmd).await;
    }
}

#[tokio::test]
async fn dispatch_task_refresh_from_db_syncs_app_from_db() {
    let (rt, mut app) = test_runtime().await;
    assert!(app.tasks().is_empty(), "precondition: empty board");
    let task = seed(
        &rt,
        "Written behind App's back",
        models::TaskStatus::Backlog,
    )
    .await;

    dispatch_one(&rt, &mut app, Command::Task(TaskCommand::RefreshFromDb)).await;

    assert_eq!(app.tasks().len(), 1);
    assert_eq!(app.tasks()[0].id, task.id);
}

#[tokio::test]
async fn dispatch_task_patch_sub_status_writes_to_db() {
    let (rt, mut app) = test_runtime().await;
    let task = seed(&rt, "Patch me", models::TaskStatus::Running).await;

    dispatch_one(
        &rt,
        &mut app,
        Command::Task(TaskCommand::PatchSubStatus {
            id: task.id,
            sub_status: models::SubStatus::NeedsInput,
        }),
    )
    .await;

    let stored = rt.database.get_task(task.id).await.unwrap().unwrap();
    assert_eq!(stored.sub_status, models::SubStatus::NeedsInput);
}

#[tokio::test]
async fn dispatch_epic_insert_then_persist_writes_to_db() {
    let (rt, mut app) = test_runtime().await;

    dispatch_one(
        &rt,
        &mut app,
        Command::Epic(crate::tui::commands::EpicCommand::Insert(tui::EpicDraft {
            title: "Epic via dispatch".into(),
            description: "desc".into(),
            parent_epic_id: None,
        })),
    )
    .await;

    let epics = rt.database.list_epics().await.unwrap();
    assert_eq!(epics.len(), 1);
    assert_eq!(epics[0].title, "Epic via dispatch");
}

#[tokio::test]
async fn dispatch_persist_setting_writes_both_kinds() {
    let (rt, mut app) = test_runtime().await;

    dispatch_one(
        &rt,
        &mut app,
        Command::Settings(SettingsCommand::PersistSetting {
            key: "notifications_enabled".into(),
            value: true,
        }),
    )
    .await;
    dispatch_one(
        &rt,
        &mut app,
        Command::Settings(SettingsCommand::PersistStringSetting {
            key: "main_session_dir".into(),
            value: "/main".into(),
        }),
    )
    .await;

    assert_eq!(
        rt.database
            .get_setting_bool("notifications_enabled")
            .await
            .unwrap(),
        Some(true)
    );
    assert_eq!(
        rt.database
            .get_setting_string("main_session_dir")
            .await
            .unwrap()
            .as_deref(),
        Some("/main")
    );
}

#[tokio::test]
async fn dispatch_editor_pop_out_refuses_when_a_session_is_already_open() {
    let (rt, mut app) = test_runtime().await;
    let task = seed(&rt, "Already editing", models::TaskStatus::Backlog).await;
    *rt.editor_session.lock().unwrap() = Some(super::editor::EditorSession::occupied_for_test(
        &test_tmux_window("edit-1"),
    ));

    dispatch_one(
        &rt,
        &mut app,
        Command::Editor(EditorCommand::PopOut(crate::tui::EditKind::TaskEdit(
            Box::new(task),
        ))),
    )
    .await;

    assert_eq!(
        app.status_message(),
        Some(super::editor::EDITOR_ALREADY_OPEN_MSG),
        "the guard must surface a status message instead of opening a second editor"
    );
}

#[tokio::test]
async fn dispatch_editor_finalize_result_persists_the_edit() {
    let (rt, mut app) = test_runtime().await;
    let task = seed(&rt, "Old title", models::TaskStatus::Backlog).await;
    app.update(Message::Task(crate::tui::messages::TaskMessage::Refresh(
        vec![task.clone()],
    )));

    let edited = "--- TITLE ---\nNew title\n\
            --- DESCRIPTION ---\nNew description\n\
            --- REPO_PATH ---\n\n\
            --- STATUS ---\n\n\
            --- PLAN ---\n\n\
            --- TAG ---\n\n\
            --- BASE_BRANCH ---\n\n";

    drain(
        &rt,
        &mut app,
        Command::Editor(EditorCommand::FinalizeResult {
            kind: crate::tui::EditKind::TaskEdit(Box::new(task.clone())),
            outcome: crate::tui::EditorOutcome::Saved(edited.into()),
        }),
    )
    .await;

    let stored = rt.database.get_task(task.id).await.unwrap().unwrap();
    assert_eq!(stored.title, "New title");
    assert_eq!(stored.description, "New description");
}

#[tokio::test]
async fn dispatch_todo_create_links_to_a_task_in_the_shared_database() {
    // Regression guard for the fixture itself: `todos.task_id` has a real
    // foreign key onto `tasks(id)` (migration v68), so a todo can only be
    // linked to a task when both live in the *same* database. When
    // `make_runtime` gave `todo_svc` its own database the insert failed the
    // FK check, `exec_create_todo` swallowed the error into a warning, and
    // the whole todo↔task coupling was invisible to every test.
    let (rt, mut app) = test_runtime().await;
    let task = seed(&rt, "Linked task", models::TaskStatus::Backlog).await;

    dispatch_one(
        &rt,
        &mut app,
        Command::Todo(TodoCommand::Create {
            title: "Follow up on the linked task".into(),
            linked: Some(crate::models::TodoLink::Task(task.id)),
            reopen: false,
        }),
    )
    .await;

    let todos = rt.todo_svc.list_todos().await.unwrap();
    assert_eq!(todos.len(), 1, "the todo must have been inserted");
    assert_eq!(
        todos[0].linked,
        Some(crate::models::TodoLink::Task(task.id)),
        "both sides must observe the link"
    );
    assert!(
        rt.database.get_task(task.id).await.unwrap().is_some(),
        "the task must be readable from the same database the todo links into"
    );
    assert_eq!(app.todo_open_count(), 1);
}

#[tokio::test]
async fn dispatch_todo_update_and_delete_reach_the_service() {
    let (rt, mut app) = test_runtime().await;
    dispatch_one(
        &rt,
        &mut app,
        Command::Todo(TodoCommand::Create {
            title: "Transient".into(),
            linked: None,
            reopen: false,
        }),
    )
    .await;
    let id = rt.todo_svc.list_todos().await.unwrap()[0].id;

    dispatch_one(
        &rt,
        &mut app,
        Command::Todo(TodoCommand::Update {
            id,
            update: crate::service::todos::TodoUpdate {
                done: Some(true),
                ..Default::default()
            },
        }),
    )
    .await;
    assert!(rt.todo_svc.list_todos().await.unwrap()[0].done);

    dispatch_one(&rt, &mut app, Command::Todo(TodoCommand::ClearDone)).await;
    assert!(
        rt.todo_svc.list_todos().await.unwrap().is_empty(),
        "ClearDone should have removed the completed todo"
    );

    dispatch_one(
        &rt,
        &mut app,
        Command::Todo(TodoCommand::Create {
            title: "Doomed".into(),
            linked: None,
            reopen: false,
        }),
    )
    .await;
    let id = rt.todo_svc.list_todos().await.unwrap()[0].id;
    dispatch_one(&rt, &mut app, Command::Todo(TodoCommand::Delete(id))).await;
    assert!(rt.todo_svc.list_todos().await.unwrap().is_empty());
}

#[tokio::test]
async fn dispatch_task_insert_writes_a_new_row() {
    let (rt, mut app) = test_runtime().await;

    dispatch_one(
        &rt,
        &mut app,
        Command::Task(TaskCommand::Insert {
            draft: tui::TaskDraft {
                title: "Inserted via dispatch".into(),
                description: "desc".into(),
                repo_path: "/repo".into(),
                ..Default::default()
            },
            epic_id: None,
        }),
    )
    .await;

    let tasks = rt.database.list_all().await.unwrap();
    assert_eq!(tasks.len(), 1);
    assert_eq!(tasks[0].title, "Inserted via dispatch");
    assert_eq!(app.tasks().len(), 1);
}

#[tokio::test]
async fn dispatch_task_seed_activity_stamps_the_hook_column() {
    let (rt, mut app) = test_runtime().await;
    let task = seed(&rt, "Just dispatched", models::TaskStatus::Running).await;
    let at = chrono::Utc::now();

    dispatch_one(
        &rt,
        &mut app,
        Command::Task(TaskCommand::SeedActivity { id: task.id, at }),
    )
    .await;

    let stored = rt.database.get_task(task.id).await.unwrap().unwrap();
    assert!(
        stored.last_pre_tool_use_at.is_some(),
        "the activity timestamp must be seeded so the task is not immediately Stale"
    );
}

#[tokio::test]
async fn dispatch_task_batch_patch_sub_status_updates_every_task() {
    let (rt, mut app) = test_runtime().await;
    let a = seed(&rt, "A", models::TaskStatus::Running).await;
    let b = seed(&rt, "B", models::TaskStatus::Running).await;

    dispatch_one(
        &rt,
        &mut app,
        Command::Task(TaskCommand::BatchPatchSubStatus {
            updates: vec![
                (a.id, models::SubStatus::Stale),
                (b.id, models::SubStatus::Crashed),
            ],
        }),
    )
    .await;

    assert_eq!(
        rt.database
            .get_task(a.id)
            .await
            .unwrap()
            .unwrap()
            .sub_status,
        models::SubStatus::Stale
    );
    assert_eq!(
        rt.database
            .get_task(b.id)
            .await
            .unwrap()
            .unwrap()
            .sub_status,
        models::SubStatus::Crashed
    );
}

#[tokio::test]
async fn dispatch_epic_persist_delete_and_toggles_reach_the_db() {
    use crate::tui::commands::EpicCommand;
    let (rt, mut app) = test_runtime().await;
    let epic = rt
        .db_write()
        .create_epic("Toggled", "desc", None)
        .await
        .unwrap();

    dispatch_one(
        &rt,
        &mut app,
        Command::Epic(EpicCommand::Persist {
            id: epic.id,
            status: Some(models::TaskStatus::Review),
            sort_order: Some(3),
        }),
    )
    .await;
    dispatch_one(
        &rt,
        &mut app,
        Command::Epic(EpicCommand::ToggleAutoDispatch {
            id: epic.id,
            auto_dispatch: true,
        }),
    )
    .await;
    dispatch_one(
        &rt,
        &mut app,
        Command::Epic(EpicCommand::ToggleGroupByRepo {
            id: epic.id,
            group_by_repo: true,
        }),
    )
    .await;

    let stored = rt.database.get_epic(epic.id).await.unwrap().unwrap();
    assert_eq!(stored.status, models::TaskStatus::Review);
    assert_eq!(stored.sort_order, Some(3));
    assert!(stored.auto_dispatch);
    assert!(stored.group_by_repo);

    // RefreshFromDb must make the App agree with the DB.
    dispatch_one(&rt, &mut app, Command::Epic(EpicCommand::RefreshFromDb)).await;
    assert!(app.epics().iter().any(|e| e.id == epic.id));

    dispatch_one(&rt, &mut app, Command::Epic(EpicCommand::Delete(epic.id))).await;
    assert!(rt.database.get_epic(epic.id).await.unwrap().is_none());
}

#[tokio::test]
async fn dispatch_epic_reparent_moves_the_child() {
    use crate::tui::commands::EpicCommand;
    let (rt, mut app) = test_runtime().await;
    let parent = rt.db_write().create_epic("Parent", "", None).await.unwrap();
    let child = rt.db_write().create_epic("Child", "", None).await.unwrap();

    dispatch_one(
        &rt,
        &mut app,
        Command::Epic(EpicCommand::Reparent {
            id: child.id,
            new_parent: Some(parent.id),
        }),
    )
    .await;

    let stored = rt.database.get_epic(child.id).await.unwrap().unwrap();
    assert_eq!(stored.parent_epic_id, Some(parent.id));
}

#[tokio::test]
async fn dispatch_repo_filter_persists_then_deletes_a_preset() {
    use crate::tui::commands::RepoFilterCommand;
    let (rt, mut app) = test_runtime().await;

    dispatch_one(
        &rt,
        &mut app,
        Command::RepoFilter(RepoFilterCommand::PersistFilterPreset {
            name: "mine".into(),
            repo_paths: vec!["/repo".into()],
            mode: RepoFilterMode::Include,
        }),
    )
    .await;
    let presets = rt.database.list_filter_presets().await.unwrap();
    assert_eq!(presets.len(), 1);
    assert_eq!(presets[0].0, "mine");

    dispatch_one(
        &rt,
        &mut app,
        Command::RepoFilter(RepoFilterCommand::DeleteFilterPreset("mine".into())),
    )
    .await;
    assert!(rt.database.list_filter_presets().await.unwrap().is_empty());
}

#[tokio::test]
async fn dispatch_repo_filter_delete_repo_path_forgets_the_path() {
    use crate::tui::commands::RepoFilterCommand;
    let (rt, mut app) = test_runtime().await;
    dispatch_one(
        &rt,
        &mut app,
        Command::Settings(SettingsCommand::SaveRepoPath("/doomed".into())),
    )
    .await;

    dispatch_one(
        &rt,
        &mut app,
        Command::RepoFilter(RepoFilterCommand::DeleteRepoPath("/doomed".into())),
    )
    .await;

    assert!(
        !rt.database
            .list_repo_paths()
            .await
            .unwrap()
            .contains(&"/doomed".to_string()),
        "the path should no longer be known"
    );
}

#[tokio::test]
async fn dispatch_todo_load_populates_the_todos_view() {
    let (rt, mut app) = test_runtime().await;
    dispatch_one(
        &rt,
        &mut app,
        Command::Todo(TodoCommand::Create {
            title: "Visible".into(),
            linked: None,
            reopen: false,
        }),
    )
    .await;

    dispatch_one(&rt, &mut app, Command::Todo(TodoCommand::Load)).await;

    assert!(
        matches!(app.view_mode(), tui::ViewMode::Todos { todos, .. } if todos.len() == 1),
        "Load must switch the view to Todos with the loaded item"
    );
}

#[tokio::test]
async fn dispatch_todo_load_count_updates_the_badge_without_opening_the_view() {
    let (rt, mut app) = test_runtime().await;
    rt.todo_svc
        .create_todo("Counted".into(), None)
        .await
        .unwrap();
    assert_eq!(
        app.todo_open_count(),
        0,
        "precondition: nothing counted yet"
    );

    dispatch_one(&rt, &mut app, Command::Todo(TodoCommand::LoadCount)).await;

    assert_eq!(app.todo_open_count(), 1);
    assert!(
        !matches!(app.view_mode(), tui::ViewMode::Todos { .. }),
        "LoadCount feeds the badge only — `Load` is the arm that opens the view"
    );
}

// -----------------------------------------------------------------------
// The process-effect half
// -----------------------------------------------------------------------
//
// Everything above asserts on a DB row or on `App`. The arms below reach
// tmux, git, `gh`, `notify-send` or `xdg-open` instead, and nearly all of
// them do it from a detached `spawn_blocking` whose `JoinHandle` the
// dispatcher `drop`s. Two consequences shape this section:
//
// - There is nothing to `await`, so every test needs a completion signal.
//   Most arms send a `Message`; the ones that succeed silently
//   (`Split::FocusPane`, `Task::KillTmuxWindow`, `System::*`) are covered
//   through [`Harness::await_call`] instead.
// - A `MockProcessRunner` panic on a detached thread does *not* fail the
//   test (KB #336), so an under-scripted runner reads as a pass. Waiting on
//   a signal that only the arm under test can produce is what closes that
//   hole: the runner panicking means the signal never arrives and the wait
//   times out.

/// A `ProcessRunner` that announces every call it forwards.
///
/// The completion signal for the arms that report nothing. It wraps a real
/// [`MockProcessRunner`] rather than replacing it, so the queue semantics,
/// the out-of-band window lookup and `recorded_calls` all still apply.
struct AnnouncingRunner {
    inner: Arc<MockProcessRunner>,
    tx: mpsc::UnboundedSender<String>,
}

impl AnnouncingRunner {
    /// Announce *after* the inner call returns, so observing the signal
    /// implies the side effect already happened.
    fn announce(&self, program: &str, args: &[&str]) {
        let _ = self.tx.send(format!("{program} {}", args.join(" ")));
    }
}

impl ProcessRunner for AnnouncingRunner {
    fn run(&self, program: &str, args: &[&str]) -> anyhow::Result<std::process::Output> {
        let result = self.inner.run(program, args);
        self.announce(program, args);
        result
    }

    fn run_with_timeout(
        &self,
        program: &str,
        args: &[&str],
        timeout: Duration,
    ) -> anyhow::Result<std::process::Output> {
        let result = self.inner.run_with_timeout(program, args, timeout);
        self.announce(program, args);
        result
    }

    fn agent_binaries(&self) -> crate::process::AgentBinaries {
        self.inner.agent_binaries()
    }
}

/// A runtime, its board, and the two channels a detached arm reports on.
struct Harness {
    rt: TuiRuntime,
    app: App,
    msgs: mpsc::UnboundedReceiver<Message>,
    calls: mpsc::UnboundedReceiver<String>,
    mock: Arc<MockProcessRunner>,
    db: Arc<Database>,
}

async fn harness(mock: MockProcessRunner) -> Harness {
    let db = test_db().await;
    let (tx, msgs) = mpsc::unbounded_channel();
    let (call_tx, calls) = mpsc::unbounded_channel();
    let mock = Arc::new(mock);
    let runner: Arc<dyn ProcessRunner> = Arc::new(AnnouncingRunner {
        inner: Arc::clone(&mock),
        tx: call_tx,
    });
    let rt = make_runtime(db.clone(), tx, runner).await;
    let app = App::new(db.list_all().await.unwrap());
    Harness {
        rt,
        app,
        msgs,
        calls,
        mock,
        db,
    }
}

/// A harness whose runner is expected to run nothing at all.
async fn quiet_harness() -> Harness {
    harness(MockProcessRunner::new(vec![])).await
}

impl Harness {
    async fn dispatch(&mut self, cmd: Command) -> Vec<Command> {
        commands::dispatch(cmd, &mut self.app, &self.rt).await
    }

    /// Put `task` on the board, which several arms require before their
    /// result message is applied at all.
    fn seed_board(&mut self, task: models::Task) {
        self.app
            .update(Message::Task(crate::tui::messages::TaskMessage::Refresh(
                vec![task],
            )));
    }

    async fn next_msg(&mut self) -> Message {
        tokio::time::timeout(TEST_TIMEOUT, self.msgs.recv())
            .await
            .expect("the arm under test must report well within the timeout")
            .expect("the runtime's message sender should still be alive")
    }

    /// Await the first announced call whose command line contains `needle`.
    ///
    /// The completion signal for an arm that succeeds silently. A timeout
    /// here means the arm never ran, never reached the runner, or panicked
    /// the mock on a detached thread — all of which would otherwise pass.
    async fn await_call(&mut self, needle: &str) -> String {
        let found = tokio::time::timeout(TEST_TIMEOUT, async {
            while let Some(call) = self.calls.recv().await {
                if call.contains(needle) {
                    return Some(call);
                }
            }
            None
        })
        .await;
        match found {
            Ok(Some(call)) => call,
            Ok(None) => panic!("the runner was dropped before any call contained {needle:?}"),
            Err(_) => panic!(
                "no subprocess call containing {needle:?} was made; recorded: {:?}",
                self.mock.flattened_calls()
            ),
        }
    }
}

/// A tempdir repo with `.worktrees/<slug>` already present, i.e. the reuse
/// path a [`DispatchScript::dispatch`] shape scripts.
fn provisioned_repo(slug: &str) -> tempfile::TempDir {
    let dir = tempfile::tempdir().unwrap();
    std::fs::create_dir_all(dir.path().join(".worktrees").join(slug)).unwrap();
    dir
}

// --- SplitCommand ------------------------------------------------------

#[tokio::test]
async fn dispatch_split_enter_opens_an_unowned_pane() {
    let mut h = harness(MockProcessRunner::new(vec![
        MockProcessRunner::ok_with_stdout(b"%1\n"), // current_pane_id
        MockProcessRunner::ok_with_stdout(b"%2\n"), // split-window
    ]))
    .await;

    let extra = h.dispatch(Command::Split(SplitCommand::Enter)).await;

    assert!(extra.is_empty(), "Split arms queue no follow-on commands");
    let msg = h.next_msg().await;
    assert!(
        matches!(
            msg,
            Message::Split(crate::tui::messages::SplitMessage::PaneOpened { task_id: None, .. })
        ),
        "a bare Enter opens a pane owned by no task, got: {msg:?}"
    );
}

#[tokio::test]
async fn dispatch_split_enter_with_task_joins_that_task_window() {
    let mut h = harness(
        MockProcessRunner::new(vec![
            MockProcessRunner::ok_with_stdout(b"%1\n"), // current_pane_id
            MockProcessRunner::ok_with_stdout(b"%1 \n"), // companion_pane_ids: none
            MockProcessRunner::ok(),                    // join-pane
        ])
        .with_windows(&["task-1"]),
    )
    .await;

    h.dispatch(Command::Split(SplitCommand::EnterWithTask {
        task_id: TaskId(1),
        window: test_tmux_window("task-1"),
    }))
    .await;

    let msg = h.next_msg().await;
    assert!(
        matches!(
            msg,
            Message::Split(crate::tui::messages::SplitMessage::PaneOpened {
                task_id: Some(TaskId(1)),
                ..
            })
        ),
        "the pane must come back owned by the task it was opened for, got: {msg:?}"
    );
    assert!(
        h.mock
            .flattened_calls()
            .iter()
            .any(|c| c.contains("join-pane")),
        "the task's window is joined in, not split afresh: {:?}",
        h.mock.flattened_calls()
    );
}

#[tokio::test]
async fn dispatch_split_exit_with_a_window_to_restore_breaks_the_pane_out() {
    let mut h = harness(MockProcessRunner::new(vec![MockProcessRunner::ok()])).await;

    h.dispatch(Command::Split(SplitCommand::Exit {
        pane_id: "%2".into(),
        restore_window: Some(test_tmux_window("task-1")),
    }))
    .await;

    let msg = h.next_msg().await;
    assert!(
        matches!(
            msg,
            Message::Split(crate::tui::messages::SplitMessage::PaneClosed)
        ),
        "Expected PaneClosed, got: {msg:?}"
    );
    // The distinction the arm carries: a pane with a window to go back to is
    // broken out, never killed — killing it would take the agent with it.
    let calls = h.mock.flattened_calls();
    assert!(
        calls.iter().any(|c| c.contains("break-pane")),
        "expected break-pane, got: {calls:?}"
    );
    assert!(
        !calls.iter().any(|c| c.contains("kill-pane")),
        "a restorable pane must not be killed, got: {calls:?}"
    );
}

#[tokio::test]
async fn dispatch_split_exit_without_a_window_kills_the_pane() {
    let mut h = harness(MockProcessRunner::new(vec![MockProcessRunner::ok()])).await;

    h.dispatch(Command::Split(SplitCommand::Exit {
        pane_id: "%2".into(),
        restore_window: None,
    }))
    .await;

    assert!(matches!(
        h.next_msg().await,
        Message::Split(crate::tui::messages::SplitMessage::PaneClosed)
    ));
    assert!(
        h.mock
            .flattened_calls()
            .iter()
            .any(|c| c.contains("kill-pane")),
        "a pane with nowhere to go back to is killed, got: {:?}",
        h.mock.flattened_calls()
    );
}

#[tokio::test]
async fn dispatch_split_swap_hands_the_pane_to_the_incoming_task() {
    let mut h = harness(
        MockProcessRunner::new(vec![
            MockProcessRunner::ok(), // swap-pane
            MockProcessRunner::ok(), // kill-window (no outgoing task)
        ])
        .with_windows(&["task-1"]),
    )
    .await;

    h.dispatch(Command::Split(SplitCommand::Swap {
        task_id: TaskId(1),
        new_window: test_tmux_window("task-1"),
        old_pane_id: Some("%2".into()),
        old_task: None,
    }))
    .await;

    let msg = h.next_msg().await;
    assert!(
        matches!(
            msg,
            Message::Split(crate::tui::messages::SplitMessage::PaneOpened {
                task_id: Some(TaskId(1)),
                ..
            })
        ),
        "the swapped-in pane must be reported as the new task's, got: {msg:?}"
    );
    assert!(
        h.mock
            .flattened_calls()
            .iter()
            .any(|c| c.contains("swap-pane")),
        "got: {:?}",
        h.mock.flattened_calls()
    );
}

/// No message on success, so the runner announcement is the whole signal.
#[tokio::test]
async fn dispatch_split_focus_pane_selects_it() {
    let mut h = harness(MockProcessRunner::new(vec![MockProcessRunner::ok()])).await;

    h.dispatch(Command::Split(SplitCommand::FocusPane {
        pane_id: "%2".into(),
    }))
    .await;

    let call = h.await_call("select-pane").await;
    assert!(call.contains("%2"), "the focused pane must be ours: {call}");
}

#[tokio::test]
async fn dispatch_split_check_pane_exists_reports_a_pane_that_is_gone() {
    // A *successful* tmux call whose listing no longer holds %2 — real tmux
    // exits 0 for an unknown pane.
    let mut h = harness(MockProcessRunner::new(vec![
        MockProcessRunner::ok_with_stdout(b"%1\n%7\n"),
    ]))
    .await;

    h.dispatch(Command::Split(SplitCommand::CheckPaneExists {
        pane_id: "%2".into(),
    }))
    .await;

    assert!(matches!(
        h.next_msg().await,
        Message::Split(crate::tui::messages::SplitMessage::PaneClosed)
    ));
}

#[tokio::test]
async fn dispatch_split_respawn_pane_reports_one_it_could_not_revive() {
    let mut h = harness(MockProcessRunner::new(vec![MockProcessRunner::fail(
        "no such pane",
    )]))
    .await;

    h.dispatch(Command::Split(SplitCommand::RespawnPane {
        pane_id: "%2".into(),
    }))
    .await;

    assert!(matches!(
        h.next_msg().await,
        Message::Split(crate::tui::messages::SplitMessage::PaneClosed)
    ));
    // `PaneClosed` alone does not discriminate this arm from
    // `CheckPaneExists` — both report it on a failing tmux call. The call
    // itself must be the respawn attempt, not a pane-existence query.
    let calls = h.mock.flattened_calls();
    assert!(
        calls.iter().any(|c| c.contains("respawn-pane")),
        "expected a respawn-pane call, got: {calls:?}"
    );
}

// --- SystemCommand -----------------------------------------------------

#[tokio::test]
async fn dispatch_system_send_notification_shells_out_to_notify_send() {
    let mut h = harness(MockProcessRunner::new(vec![MockProcessRunner::ok()])).await;

    h.dispatch(Command::System(SystemCommand::SendNotification {
        title: "Task #1: Fix bug".into(),
        body: "Ready for review".into(),
        urgent: true,
    }))
    .await;

    let call = h.await_call("notify-send").await;
    assert!(call.contains("Task #1: Fix bug"), "got: {call}");
    assert!(call.contains("Ready for review"), "got: {call}");
    assert!(
        call.contains("critical"),
        "an urgent notification must carry the critical urgency: {call}"
    );
}

#[tokio::test]
async fn dispatch_system_open_in_browser_shells_out_to_xdg_open() {
    let mut h = harness(MockProcessRunner::new(vec![MockProcessRunner::ok()])).await;

    h.dispatch(Command::System(SystemCommand::OpenInBrowser {
        url: "https://github.com/org/repo/pull/1".into(),
    }))
    .await;

    let call = h.await_call("xdg-open").await;
    assert!(
        call.contains("https://github.com/org/repo/pull/1"),
        "got: {call}"
    );
}

// --- PrCommand ---------------------------------------------------------

#[tokio::test]
async fn dispatch_pr_check_status_reports_the_state_gh_returned() {
    let mut h = harness(MockProcessRunner::new(vec![
        MockProcessRunner::ok_with_stdout(b"OPEN\nAPPROVED\n"),
    ]))
    .await;

    h.dispatch(Command::Pr(PrCommand::CheckStatus {
        id: TaskId(1),
        url: "https://github.com/org/repo/pull/42".into(),
    }))
    .await;

    match h.next_msg().await {
        Message::Pr(crate::tui::messages::PrMessage::ReviewState {
            id,
            review_decision,
        }) => {
            assert_eq!(id, TaskId(1));
            assert_eq!(review_decision, Some(models::ReviewDecision::Approved));
        }
        other => panic!("expected a PR review state, got {other:?}"),
    }
}

// --- RepoSyncCommand ---------------------------------------------------

#[tokio::test]
async fn dispatch_repo_sync_refresh_reports_the_measured_drift() {
    let mut h = harness(MockProcessRunner::new(vec![
        MockProcessRunner::ok_with_stdout(b"refs/remotes/origin/main\n"),
        MockProcessRunner::ok(),                      // fetch
        MockProcessRunner::ok_with_stdout(b"3\t1\n"), // rev-list
    ]))
    .await;

    h.dispatch(Command::RepoSync(RepoSyncCommand::Refresh {
        repo_path: "/repo".into(),
        fetch_first: true,
    }))
    .await;

    match h.next_msg().await {
        Message::RepoSync(crate::tui::messages::RepoSyncMessage::Measured(m)) => {
            assert_eq!(m.repo_path, "/repo");
            assert_eq!(
                m.counts,
                Some(crate::repo_sync::AheadBehind {
                    ahead: 3,
                    behind: 1
                })
            );
        }
        other => panic!("expected a repo-sync measurement, got {other:?}"),
    }
}

/// The two `RepoSync` arms are one wire apart, so the sync arm is pinned by
/// an outcome the refresh arm cannot produce: a `Failed`.
#[tokio::test]
async fn dispatch_repo_sync_sync_reports_a_failure_the_refresh_arm_cannot() {
    let mut h = harness(MockProcessRunner::new(vec![
        MockProcessRunner::ok_with_stdout(b"git@github.com:org/repo.git\n"),
        MockProcessRunner::ok_with_stdout(b"feature\n"), // not on the base branch
    ]))
    .await;

    h.dispatch(Command::RepoSync(RepoSyncCommand::Sync {
        repo_path: "/repo".into(),
        base_branch: "main".into(),
    }))
    .await;

    match h.next_msg().await {
        Message::RepoSync(crate::tui::messages::RepoSyncMessage::Failed {
            repo_path,
            detail,
            retryable,
        }) => {
            assert_eq!(repo_path, "/repo");
            assert!(
                detail.contains("feature") && detail.contains("main"),
                "the branch found and the one expected: {detail}"
            );
            assert!(!retryable, "the operator must check out main first");
        }
        other => panic!("expected a sync failure, got {other:?}"),
    }
}

// --- MainSessionCommand ------------------------------------------------

#[tokio::test]
async fn dispatch_main_session_open_falls_back_to_the_picker_when_no_window_is_alive() {
    // has_window → false (empty listing)
    let mut h = harness(MockProcessRunner::new(vec![MockProcessRunner::ok()])).await;
    // A previously-configured dir must not stop the picker re-prompting.
    h.app.set_main_session_dir(Some("/home/user".to_string()));

    h.dispatch(Command::MainSession(MainSessionCommand::Open))
        .await;

    assert_eq!(h.app.mode(), &crate::tui::InputMode::MainSessionDir);
    assert!(
        !h.mock
            .flattened_calls()
            .iter()
            .any(|c| c.contains("new-window")),
        "Open never creates a window — that is Create's job: {:?}",
        h.mock.flattened_calls()
    );
}

#[tokio::test]
async fn dispatch_main_session_create_without_a_configured_dir_errors_and_runs_nothing() {
    let mut h = quiet_harness().await;

    h.dispatch(Command::MainSession(MainSessionCommand::Create))
        .await;

    let err = h.app.error_popup().unwrap_or_default();
    assert!(
        err.contains("Main session directory not configured"),
        "got: {err:?}"
    );
    assert!(
        h.mock.recorded_calls().is_empty(),
        "an unconfigured create must not reach tmux at all"
    );
}

#[tokio::test]
async fn dispatch_main_session_check_liveness_reports_the_live_window() {
    let mut h = harness(MockProcessRunner::new(vec![
        MockProcessRunner::ok_with_stdout(b"dispatch-main\n"),
    ]))
    .await;

    h.dispatch(Command::MainSession(MainSessionCommand::CheckLiveness))
        .await;

    assert!(
        matches!(
            h.next_msg().await,
            Message::MainSession(crate::tui::messages::MainSessionMessage::LivenessChanged(
                true
            ))
        ),
        "the poll must report the window it found"
    );
}

// --- FeedCommand -------------------------------------------------------

#[tokio::test]
async fn dispatch_feed_trigger_epic_runs_the_epics_own_feed_command() {
    let mut h = quiet_harness().await;
    let epic =
        h.db.create_epic("Security Vulnerabilities", "", None)
            .await
            .unwrap();
    set_feed_command(
            &h.db,
            epic.id,
            r#"echo '[{"external_id":"vuln:1","title":"CVE-1","description":"d","status":"backlog","tag":"fix"}]'"#,
        )
        .await;

    h.dispatch(Command::Feed(FeedCommand::TriggerEpic {
        epic_id: epic.id,
        epic_title: "Security Vulnerabilities".into(),
    }))
    .await;

    let msg = h.next_msg().await;
    assert_feed_failed_because_not_applicable(&msg);
    assert!(
        matches!(
            msg,
            Message::Feed(crate::tui::messages::FeedMessage::Refreshed { count: 1, .. })
        ),
        "expected one synced item, got: {msg:?}"
    );
    assert_eq!(
        h.db.list_tasks_for_epic(epic.id).await.unwrap().len(),
        1,
        "the cycle must have upserted the emitted item"
    );
}

/// A feed cycle that failed for a *configuration* reason never reached the
/// command, so it proves nothing about the wire. Kept as its own assertion
/// rather than folded into the match so the failure names the cause.
fn assert_feed_failed_because_not_applicable(msg: &Message) {
    if let Message::Feed(crate::tui::messages::FeedMessage::Failed { error, .. }) = msg {
        panic!("the feed cycle failed before it ran the command: {error}");
    }
}

// --- LearningCommand ---------------------------------------------------

/// Records that the sweep reached it. Only `archive_stale_learnings` is
/// implemented; every other seam method keeps the panicking stub default,
/// which is what proves the arm called this one and nothing else.
struct CountingSweep {
    calls: std::sync::atomic::AtomicUsize,
}

#[async_trait::async_trait]
impl crate::service::LearningServiceApiStub for CountingSweep {
    async fn archive_stale_learnings(
        &self,
        _cutoff: chrono::DateTime<chrono::Utc>,
    ) -> Result<u64, crate::service::ServiceError> {
        self.calls.fetch_add(1, Ordering::Relaxed);
        Ok(0)
    }
}

crate::learning_service_api!(service_api_stub_bridge, CountingSweep);

#[tokio::test]
async fn dispatch_learning_archive_stale_sweeps_through_the_learning_service() {
    let mut h = quiet_harness().await;
    let svc = Arc::new(CountingSweep {
        calls: std::sync::atomic::AtomicUsize::new(0),
    });
    h.rt.learning_svc = svc.clone();

    h.dispatch(Command::Learning(
        crate::tui::commands::LearningCommand::ArchiveStale,
    ))
    .await;

    assert_eq!(
        svc.calls.load(Ordering::Relaxed),
        1,
        "the arm must reach the injected learning seam exactly once"
    );
}

// --- UsageCommand ------------------------------------------------------

/// Await the fire-and-forget usage write by re-reading until it lands.
///
/// `Usage::Record` spawns its write and keeps no handle, sends no message
/// and has no test hook, so the row appearing is the only observable there
/// is. Each iteration awaits a real DB round-trip — which yields to the
/// runtime rather than spinning — and the whole loop is bounded by
/// `TEST_TIMEOUT` structurally, never by the wall clock (docs/conventions.md,
/// "No `tokio::time::sleep` in tests").
async fn await_usage_row(rt: &TuiRuntime, action: &str) -> crate::models::UsageSummary {
    tokio::time::timeout(TEST_TIMEOUT, async {
        loop {
            let rows = rt
                .database
                .query_usage(&db::UsageQuery::default())
                .await
                .unwrap();
            if let Some(row) = rows.into_iter().find(|r| r.action == action) {
                return row;
            }
            tokio::task::yield_now().await;
        }
    })
    .await
    .expect("the spawned usage write must land well within the timeout")
}

#[tokio::test]
async fn dispatch_usage_record_writes_the_event_in_the_background() {
    let mut h = quiet_harness().await;

    h.dispatch(Command::Usage(crate::tui::commands::UsageCommand::Record(
        crate::models::UsageEvent {
            category: crate::models::UsageCategory::Keybinding,
            action: "move_task_right".into(),
            detail: Some("l".into()),
            actor: crate::models::UsageActor::Human,
        },
    )))
    .await;

    let row = await_usage_row(&h.rt, "move_task_right").await;
    assert_eq!(row.detail.as_deref(), Some("l"));
    assert_eq!(row.count, 1);
}

// --- BudgetCommand -----------------------------------------------------

#[tokio::test]
async fn dispatch_budget_refresh_reads_the_snapshot_file_off_the_event_loop() {
    let mut h = quiet_harness().await;
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("rate-limits.json");
    std::fs::write(
        &path,
        r#"{"five_hour":{"used_percentage":42.5,"resets_at":1000},"captured_at":900}"#,
    )
    .unwrap();
    h.rt.budget_snapshot_path = path;

    h.dispatch(Command::Budget(
        crate::tui::commands::BudgetCommand::Refresh,
    ))
    .await;

    match h.next_msg().await {
        Message::Budget(crate::tui::messages::BudgetMessage::Updated(Some(snapshot))) => {
            assert_eq!(snapshot.captured_at, 900);
            assert_eq!(
                snapshot.five_hour.map(|w| w.used_percentage),
                Some(42.5),
                "the parsed window must be the one on disk"
            );
        }
        other => panic!("expected a parsed budget snapshot, got {other:?}"),
    }
}

// --- TaskCommand: the tmux/process half --------------------------------

#[tokio::test]
async fn dispatch_task_dispatch_agent_claims_then_provisions() {
    let repo = provisioned_repo("1-test-task");
    let mut h = harness(DispatchScript::dispatch().runner()).await;
    let task = create_task_returning(
        &**h.rt.db_write(),
        "Test Task",
        "desc",
        repo.path().to_str().unwrap(),
        None,
        models::TaskStatus::Backlog,
    )
    .await
    .unwrap();
    let id = task.id;

    h.dispatch(Command::Task(TaskCommand::DispatchAgent {
        task: Box::new(task),
        mode: models::DispatchMode::Dispatch,
    }))
    .await;

    let msg = h.next_msg().await;
    assert!(
        matches!(
            msg,
            Message::Task(crate::tui::messages::TaskMessage::Dispatched { .. })
        ),
        "Expected Dispatched, got: {msg:?}"
    );
    // Nothing here runs the Persist `handle_dispatched` emits, so the row can
    // only have left Backlog via the pre-provisioning claim
    // (`DispatchClaimExclusive` in docs/specs/dispatch.allium).
    let claimed = h.rt.database.get_task(id).await.unwrap().unwrap();
    assert_eq!(claimed.status, models::TaskStatus::Running);
}

/// `Research` must reach the research agent. The mode travels on the command,
/// so a dispatcher arm that dropped it would launch a standard agent instead.
/// Every launcher shares one permission mode
/// (`EveryTaskAgentLaunchesInAutoMode`), so the prompt is the marker.
#[tokio::test]
async fn dispatch_task_dispatch_agent_carries_the_mode_to_the_launcher() {
    let repo = provisioned_repo("1-test-task");
    let mut h = harness(DispatchScript::dispatch().runner()).await;
    let task = create_task_returning(
        &**h.rt.db_write(),
        "Test Task",
        "desc",
        repo.path().to_str().unwrap(),
        None,
        models::TaskStatus::Backlog,
    )
    .await
    .unwrap();

    h.dispatch(Command::Task(TaskCommand::DispatchAgent {
        task: Box::new(task),
        mode: models::DispatchMode::Research,
    }))
    .await;

    assert!(matches!(
        h.next_msg().await,
        Message::Task(crate::tui::messages::TaskMessage::Dispatched { .. })
    ));
    let prompt = std::fs::read_to_string(repo.path().join(".worktrees/1-test-task/.claude-prompt"))
        .expect("dispatch should write the prompt file");
    assert!(
        prompt.contains(crate::dispatch::RESEARCH_AGENT_INTRO),
        "research mode must reach build_research_prompt: {prompt}"
    );
}

/// The trust gate's read half. The harness's default `claude_json_path`
/// points at a nonexistent file, so the check is a deterministic
/// "untrusted" — and this arm only reads, so nothing is written even
/// there.
#[tokio::test]
async fn dispatch_task_check_trust_and_dispatch_prompts_for_an_untrusted_repo() {
    let repo = tempfile::tempdir().unwrap();
    let repo_path = repo.path().to_str().unwrap().to_string();
    let mut h = quiet_harness().await;
    let task = create_task_returning(
        &**h.rt.db_write(),
        "Needs trust",
        "desc",
        &repo_path,
        None,
        models::TaskStatus::Backlog,
    )
    .await
    .unwrap();
    h.seed_board(task.clone());

    h.dispatch(Command::Task(TaskCommand::CheckTrustAndDispatch {
        id: task.id,
        repo_path,
        mode: models::DispatchMode::Dispatch,
    }))
    .await;

    let status = h.app.status_message().unwrap_or_default();
    assert!(
        status.contains("not trusted by Claude Code"),
        "an untrusted repo must reach the confirmation prompt, got: {status:?}"
    );
    assert!(
        h.mock.recorded_calls().is_empty(),
        "nothing may be provisioned before the operator confirms"
    );
}

#[tokio::test]
async fn dispatch_task_quick_dispatch_stops_at_the_trust_prompt_for_an_untrusted_repo() {
    let repo = tempfile::tempdir().unwrap();
    let mut h = quiet_harness().await;

    h.dispatch(Command::Task(TaskCommand::QuickDispatch {
        draft: tui::TaskDraft {
            title: "Quick one".into(),
            description: String::new(),
            repo_path: repo.path().to_str().unwrap().into(),
            ..Default::default()
        },
        epic_id: None,
    }))
    .await;

    let status = h.app.status_message().unwrap_or_default();
    assert!(
        status.contains("not trusted by Claude Code"),
        "quick dispatch must gate on trust too, got: {status:?}"
    );
    assert!(
        h.rt.database.list_all().await.unwrap().is_empty(),
        "no task row may be created before the operator confirms"
    );
}

/// A tempfile path for `claude_json_path`, pointing at a file that does not
/// exist yet — the shape `trust_at` creates on its first grant.
fn tempfile_claude_json_path() -> (tempfile::TempDir, std::path::PathBuf) {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join(".claude.json");
    (dir, path)
}

#[tokio::test]
async fn dispatch_task_trust_and_dispatch_grants_trust_then_dispatches() {
    let repo = provisioned_repo("1-test-task");
    let (_claude_dir, claude_json_path) = tempfile_claude_json_path();
    let mut h = harness(DispatchScript::dispatch().runner()).await;
    h.rt.claude_json_path = claude_json_path.clone();
    let task = create_task_returning(
        &**h.rt.db_write(),
        "Test Task",
        "desc",
        repo.path().to_str().unwrap(),
        None,
        models::TaskStatus::Backlog,
    )
    .await
    .unwrap();
    let repo_path = task.repo_path.clone();

    h.dispatch(Command::Task(TaskCommand::TrustAndDispatch {
        task: Box::new(task),
        mode: models::DispatchMode::Dispatch,
    }))
    .await;

    assert!(matches!(
        h.next_msg().await,
        Message::Task(crate::tui::messages::TaskMessage::Dispatched { .. })
    ));
    assert!(
        crate::dispatch::is_trusted_at(&claude_json_path, &repo_path).unwrap(),
        "the grant must be durable, not just in-memory permission to proceed"
    );
}

/// The grant's own failure — a `claude.json` that cannot even be parsed —
/// must abandon the dispatch before anything is provisioned, distinct from
/// `DispatchFailed` because there is no claim yet to release.
#[tokio::test]
async fn dispatch_task_trust_and_dispatch_abandons_on_a_trust_write_failure() {
    let (_claude_dir, claude_json_path) = tempfile_claude_json_path();
    std::fs::write(&claude_json_path, "not json").unwrap();
    let mut h = quiet_harness().await;
    h.rt.claude_json_path = claude_json_path;
    let task = create_task_returning(
        &**h.rt.db_write(),
        "Test Task",
        "desc",
        "/repo",
        None,
        models::TaskStatus::Backlog,
    )
    .await
    .unwrap();
    let id = task.id;
    // Mirrors `handle_dispatch_task`, which marks a task dispatching before
    // ever queuing `TrustAndDispatch` — without this, "not dispatching"
    // would hold trivially before the arm even runs.
    h.app.update(Message::Task(
        crate::tui::messages::TaskMessage::MarkDispatching(id),
    ));

    h.dispatch(Command::Task(TaskCommand::TrustAndDispatch {
        task: Box::new(task),
        mode: models::DispatchMode::Dispatch,
    }))
    .await;

    // Both the abandon and the error apply directly to `app`, the same way
    // `CheckTrustAndDispatch`'s untrusted branch does — this runs
    // synchronously inside `dispatch_task`, never through `msg_tx`.
    assert!(
        !h.app.is_dispatching(id),
        "an abandoned dispatch must clear the dispatching marker"
    );
    let err = h.app.error_popup().unwrap_or_default();
    assert!(err.contains("Failed to trust repo"), "got: {err:?}");
    assert!(
        h.mock.recorded_calls().is_empty(),
        "nothing may be provisioned when the trust grant itself fails"
    );
}

#[tokio::test]
async fn dispatch_task_trust_and_quick_dispatch_grants_trust_then_dispatches() {
    let dir = tempfile::tempdir().unwrap();
    let repo_path = dir.path().to_str().unwrap().to_string();
    std::fs::create_dir_all(dir.path().join(".worktrees/1-quick-one")).unwrap();
    let (_claude_dir, claude_json_path) = tempfile_claude_json_path();
    let mut h = harness(
        DispatchScript::dispatch()
            .detecting_default_branch("main")
            .runner(),
    )
    .await;
    h.rt.claude_json_path = claude_json_path.clone();

    h.dispatch(Command::Task(TaskCommand::TrustAndQuickDispatch {
        draft: tui::TaskDraft {
            title: "Quick one".into(),
            description: String::new(),
            repo_path: repo_path.clone(),
            ..Default::default()
        },
        epic_id: None,
    }))
    .await;

    // `Created` applies to `app` directly, the same way `MarkDispatching`
    // does — only the eventual dispatch outcome travels over `msg_tx`.
    assert_eq!(
        h.app.tasks().len(),
        1,
        "quick dispatch must still create the task row on the granted path"
    );
    assert_eq!(h.app.tasks()[0].title, "Quick one");
    assert!(
        crate::dispatch::is_trusted_at(&claude_json_path, &repo_path).unwrap(),
        "the grant must be durable"
    );
    assert!(matches!(
        h.next_msg().await,
        Message::Task(crate::tui::messages::TaskMessage::Dispatched { .. })
    ));
}

/// The trusted-repo halves of `CheckTrustAndDispatch`/`QuickDispatch`: the
/// untrusted-repo tests above cover the read failing to find trust, so this
/// pair covers it finding trust and letting the dispatch through instead.
#[tokio::test]
async fn dispatch_task_check_trust_and_dispatch_proceeds_when_already_trusted() {
    let repo = provisioned_repo("1-test-task");
    let repo_path = repo.path().to_str().unwrap().to_string();
    let (_claude_dir, claude_json_path) = tempfile_claude_json_path();
    crate::dispatch::trust_at(&claude_json_path, &repo_path).unwrap();
    let mut h = harness(DispatchScript::dispatch().runner()).await;
    h.rt.claude_json_path = claude_json_path;
    let task = create_task_returning(
        &**h.rt.db_write(),
        "Test Task",
        "desc",
        &repo_path,
        None,
        models::TaskStatus::Backlog,
    )
    .await
    .unwrap();
    h.seed_board(task.clone());

    let extra = h
        .dispatch(Command::Task(TaskCommand::CheckTrustAndDispatch {
            id: task.id,
            repo_path,
            mode: models::DispatchMode::Dispatch,
        }))
        .await;
    for cmd in extra {
        drain(&h.rt, &mut h.app, cmd).await;
    }

    assert!(
        matches!(
            h.next_msg().await,
            Message::Task(crate::tui::messages::TaskMessage::Dispatched { .. })
        ),
        "an already-trusted repo must reach the real dispatcher, not a prompt"
    );
}

#[tokio::test]
async fn dispatch_task_quick_dispatch_proceeds_when_already_trusted() {
    let dir = tempfile::tempdir().unwrap();
    let repo_path = dir.path().to_str().unwrap().to_string();
    std::fs::create_dir_all(dir.path().join(".worktrees/1-quick-one")).unwrap();
    let (_claude_dir, claude_json_path) = tempfile_claude_json_path();
    crate::dispatch::trust_at(&claude_json_path, &repo_path).unwrap();
    let mut h = harness(
        DispatchScript::dispatch()
            .detecting_default_branch("main")
            .runner(),
    )
    .await;
    h.rt.claude_json_path = claude_json_path;

    h.dispatch(Command::Task(TaskCommand::QuickDispatch {
        draft: tui::TaskDraft {
            title: "Quick one".into(),
            description: String::new(),
            repo_path,
            ..Default::default()
        },
        epic_id: None,
    }))
    .await;

    assert_eq!(
        h.app.tasks().len(),
        1,
        "an already-trusted repo must proceed straight to task creation, not a prompt"
    );
    assert!(matches!(
        h.next_msg().await,
        Message::Task(crate::tui::messages::TaskMessage::Dispatched { .. })
    ));
}

#[tokio::test]
async fn dispatch_task_release_claim_returns_the_task_to_backlog() {
    let mut h = quiet_harness().await;
    let task = create_task_returning(
        &**h.rt.db_write(),
        "Claimed",
        "desc",
        "/repo",
        None,
        models::TaskStatus::Backlog,
    )
    .await
    .unwrap();
    assert!(h.rt.task_svc.claim_backlog_task(task.id).await.unwrap());

    h.dispatch(Command::Task(TaskCommand::ReleaseClaim(task.id)))
        .await;

    let released = h.rt.database.get_task(task.id).await.unwrap().unwrap();
    assert_eq!(released.status, models::TaskStatus::Backlog);
    assert!(
        released.last_pre_tool_use_at.is_none(),
        "the release clears the stamp the claim seeded"
    );
}

#[tokio::test]
async fn dispatch_task_clear_subagents_drops_the_live_count() {
    let mut h = quiet_harness().await;
    let task = create_task_returning(
        &**h.rt.db_write(),
        "Busy",
        "desc",
        "/repo",
        None,
        models::TaskStatus::Running,
    )
    .await
    .unwrap();
    h.rt.task_svc
        .record_subagent_event(
            task.id,
            models::SubagentEvent::Start {
                agent_id: "a1".into(),
                session_id: "s1".into(),
            },
        )
        .await
        .unwrap();
    assert_eq!(
        h.rt.database
            .get_task(task.id)
            .await
            .unwrap()
            .unwrap()
            .live_subagents,
        1,
        "precondition: one live subagent"
    );

    h.dispatch(Command::Task(TaskCommand::ClearSubagents {
        id: task.id,
        mode: models::DrainMode::Drain,
    }))
    .await;

    assert_eq!(
        h.rt.database
            .get_task(task.id)
            .await
            .unwrap()
            .unwrap()
            .live_subagents,
        0
    );
}

#[tokio::test]
async fn dispatch_task_check_window_reports_a_window_that_is_gone() {
    let mut h = harness(MockProcessRunner::new(vec![
        MockProcessRunner::ok_with_stdout(b"other-window\n"),
    ]))
    .await;

    h.dispatch(Command::Task(TaskCommand::CheckWindow {
        id: TaskId(1),
        window: test_tmux_window("gone-window"),
    }))
    .await;

    assert!(matches!(
        h.next_msg().await,
        Message::Task(crate::tui::messages::TaskMessage::WindowGone(TaskId(1)))
    ));
}

/// The batch arm's distinguishing property: one tmux call for N windows, and
/// a `WindowGone` for the absent one only.
#[tokio::test]
async fn dispatch_task_batch_check_windows_reports_only_the_absent_one() {
    let mut h = harness(MockProcessRunner::new(vec![
        MockProcessRunner::ok_with_stdout(b"task-1\nother-window\n"),
    ]))
    .await;

    h.dispatch(Command::Task(TaskCommand::BatchCheckWindows {
        windows: vec![
            (TaskId(1), TmuxWindow::for_task(TaskId(1))),
            (TaskId(2), TmuxWindow::for_task(TaskId(2))),
        ],
    }))
    .await;

    assert!(matches!(
        h.next_msg().await,
        Message::Task(crate::tui::messages::TaskMessage::WindowGone(TaskId(2)))
    ));
    assert_eq!(
        h.mock.recorded_calls().len(),
        1,
        "the batch arm issues one tmux call, not one per window"
    );
}

#[tokio::test]
async fn dispatch_task_resume_relaunches_the_agent_window() {
    let mut h = harness(DispatchScript::resume().runner()).await;
    let task = create_task_returning(
        &**h.rt.db_write(),
        "Resume Me",
        "desc",
        "/repo",
        None,
        models::TaskStatus::Running,
    )
    .await
    .unwrap();
    let id = task.id;
    let worktree = Some("/repo/.worktrees/1-resume-me".to_string());

    h.dispatch(Command::Task(TaskCommand::Resume { id, worktree }))
        .await;

    match h.next_msg().await {
        Message::Task(crate::tui::messages::TaskMessage::Resumed {
            id: tid,
            tmux_window,
        }) => {
            assert_eq!(tid, id);
            assert_eq!(tmux_window, TmuxWindow::for_task(id));
        }
        other => panic!("expected Resumed, got {other:?}"),
    }
}

#[tokio::test]
async fn dispatch_task_jump_to_tmux_selects_the_window() {
    let mut h =
        harness(MockProcessRunner::new(vec![MockProcessRunner::ok()]).with_windows(&["task-1"]))
            .await;

    h.dispatch(Command::Task(TaskCommand::JumpToTmux {
        window: test_tmux_window("task-1"),
    }))
    .await;

    // Synchronous, not spawned: the call is already recorded on return.
    let calls = h.mock.flattened_calls();
    assert!(
        calls.iter().any(|c| c.contains("select-window")),
        "got: {calls:?}"
    );
    assert!(h.app.error_popup().is_none());
}

#[tokio::test]
async fn dispatch_task_jump_to_tmux_surfaces_a_failed_jump() {
    let mut h = harness(MockProcessRunner::new(vec![MockProcessRunner::fail(
        "no such window",
    )]))
    .await;

    h.dispatch(Command::Task(TaskCommand::JumpToTmux {
        window: test_tmux_window("task-1"),
    }))
    .await;

    let err = h.app.error_popup().unwrap_or_default();
    assert!(err.contains("Jump failed"), "got: {err:?}");
}

#[tokio::test]
async fn dispatch_task_cleanup_carries_its_follow_up_back_on_success() {
    let mut h = harness(MockProcessRunner::new(vec![
        MockProcessRunner::ok(), // git worktree remove
        MockProcessRunner::ok(), // git branch -D
    ]))
    .await;

    h.dispatch(Command::Task(TaskCommand::Cleanup {
        id: TaskId(1),
        repo_path: "/repo".into(),
        worktree: Some("/repo/.worktrees/1-doomed".into()),
        tmux_window: None,
        follow_up: crate::tui::commands::CleanupFollowUp::DeleteRow,
    }))
    .await;

    let msg = h.next_msg().await;
    assert!(
        matches!(
            msg,
            Message::Task(crate::tui::messages::TaskMessage::CleanupSucceeded {
                id: TaskId(1),
                follow_up: crate::tui::commands::CleanupFollowUp::DeleteRow,
            })
        ),
        "the follow-up must ride the completion path, got: {msg:?}"
    );
}

/// No message at all on either outcome — the kill is best-effort — so the
/// runner announcement is the only signal this arm produces.
#[tokio::test]
async fn dispatch_task_kill_tmux_window_kills_the_window() {
    let mut h =
        harness(MockProcessRunner::new(vec![MockProcessRunner::ok()]).with_windows(&["task-1"]))
            .await;

    h.dispatch(Command::Task(TaskCommand::KillTmuxWindow {
        window: test_tmux_window("task-1"),
    }))
    .await;

    let call = h.await_call("kill-window").await;
    assert!(
        call.contains(&h.mock.pane_id_of("task-1")),
        "targeted by resolved pane ID, not by name (see `tmux::window_target`): {call}"
    );
}

// -----------------------------------------------------------------------
// Every variant reaches the dispatcher
// -----------------------------------------------------------------------

/// The dispatcher's top-level match has one arm per `Command` sub-enum, and
/// a mis-wire between two of them compiles. The tests above drive every
/// variant through `commands::dispatch`; this one is the *inventory* that
/// keeps that true — a new sub-enum added to `Command` fails to compile
/// here until it is listed, which is the prompt to write its test.
///
/// Deliberately not an assertion on the match's shape: it builds one value
/// per sub-enum and lets exhaustiveness do the work.
#[test]
fn every_command_sub_enum_is_named_by_this_module() {
    fn sub_enum_name(cmd: &Command) -> &'static str {
        match cmd {
            Command::Task(_) => "Task",
            Command::Editor(_) => "Editor",
            Command::Feed(_) => "Feed",
            Command::Settings(_) => "Settings",
            Command::MainSession(_) => "MainSession",
            Command::Epic(_) => "Epic",
            Command::System(_) => "System",
            Command::RepoFilter(_) => "RepoFilter",
            Command::RepoSync(_) => "RepoSync",
            Command::Pr(_) => "Pr",
            Command::Split(_) => "Split",
            Command::Learning(_) => "Learning",
            Command::Usage(_) => "Usage",
            Command::Todo(_) => "Todo",
            Command::Budget(_) => "Budget",
        }
    }

    // One representative per sub-enum. Every name below is driven through
    // `commands::dispatch` by a test in this module.
    let covered = [
        Command::Task(TaskCommand::RefreshFromDb),
        Command::Editor(EditorCommand::PopOut(crate::tui::EditKind::Description {
            is_epic: false,
        })),
        Command::Feed(FeedCommand::TriggerEpic {
            epic_id: crate::models::EpicId(1),
            epic_title: String::new(),
        }),
        Command::Settings(SettingsCommand::SaveRepoPath(String::new())),
        Command::MainSession(MainSessionCommand::CheckLiveness),
        Command::Epic(crate::tui::commands::EpicCommand::RefreshFromDb),
        Command::System(SystemCommand::OpenInBrowser { url: String::new() }),
        Command::RepoFilter(crate::tui::commands::RepoFilterCommand::DeleteRepoPath(
            String::new(),
        )),
        Command::RepoSync(RepoSyncCommand::Refresh {
            repo_path: String::new(),
            fetch_first: false,
        }),
        Command::Pr(PrCommand::CheckStatus {
            id: TaskId(1),
            url: String::new(),
        }),
        Command::Split(SplitCommand::Enter),
        Command::Learning(crate::tui::commands::LearningCommand::ArchiveStale),
        Command::Usage(crate::tui::commands::UsageCommand::Record(
            crate::models::UsageEvent {
                category: crate::models::UsageCategory::Keybinding,
                action: String::new(),
                detail: None,
                actor: crate::models::UsageActor::Human,
            },
        )),
        Command::Todo(TodoCommand::LoadCount),
        Command::Budget(crate::tui::commands::BudgetCommand::Refresh),
    ];

    let mut names: Vec<&str> = covered.iter().map(sub_enum_name).collect();
    names.sort_unstable();
    let before = names.len();
    names.dedup();
    assert_eq!(
        names.len(),
        before,
        "each sub-enum needs exactly one representative, got {names:?}"
    );
}
