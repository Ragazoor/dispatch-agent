use super::*;
use crate::models::test_tmux_window;

#[tokio::test]
async fn exec_enter_split_mode_opens_pane() {
    let db = test_db().await;
    let (tx, mut rx) = mpsc::unbounded_channel();
    let mock = Arc::new(MockProcessRunner::new(vec![
        MockProcessRunner::ok_with_stdout(b"%1\n"), // current_pane_id
        MockProcessRunner::ok_with_stdout(b"%2\n"), // split_window_horizontal
    ]));
    let rt = make_runtime(db.clone(), tx, mock).await;

    rt.exec_enter_split_mode().await.unwrap();
    // PaneOpened message arrives via msg_tx — no error message expected.
    let msg = tokio::time::timeout(TEST_TIMEOUT, rx.recv())
        .await
        .unwrap()
        .unwrap();
    assert!(
        matches!(
            msg,
            Message::Split(crate::tui::messages::SplitMessage::PaneOpened { .. })
        ),
        "Expected PaneOpened, got: {msg:?}"
    );
}

#[tokio::test]
async fn exec_enter_split_mode_no_tmux_shows_status() {
    let db = test_db().await;
    let (tx, mut rx) = mpsc::unbounded_channel();
    let mock = Arc::new(MockProcessRunner::new(vec![
        MockProcessRunner::fail("no server"), // current_pane_id fails
    ]));
    let rt = make_runtime(db.clone(), tx, mock).await;

    rt.exec_enter_split_mode().await.unwrap();
    let msg = tokio::time::timeout(TEST_TIMEOUT, rx.recv())
        .await
        .unwrap()
        .unwrap();
    assert!(
        matches!(
            &msg,
            Message::System(crate::tui::messages::SystemMessage::StatusInfo(s)) if s == "Split mode requires tmux"
        ),
        "Expected StatusInfo, got: {msg:?}"
    );
}

#[tokio::test]
async fn exec_enter_split_mode_with_task_joins_pane() {
    let db = test_db().await;
    let (tx, mut rx) = mpsc::unbounded_channel();
    // join_pane no longer needs its own display-message: resolving the source
    // window by exact name already yields that window's pane ID, out of band.
    let mock = Arc::new(
        MockProcessRunner::new(vec![
            MockProcessRunner::ok_with_stdout(b"%1\n"), // current_pane_id
            // companion_pane_ids: no pane carries a dispatch role
            MockProcessRunner::ok_with_stdout(b"%1 \n"),
            MockProcessRunner::ok(), // join_pane: join-pane command
        ])
        .with_windows(&["task-1"]),
    );
    let rt = make_runtime(db.clone(), tx, mock.clone()).await;

    rt.exec_enter_split_mode_with_task(TaskId(1), &test_tmux_window("task-1"))
        .await
        .unwrap();
    let calls = mock.recorded_calls();
    assert!(calls[2].1.contains(&"join-pane".to_string()));
    assert!(
        calls[2].1.contains(&mock.pane_id_of("task-1")),
        "the source must be the resolved pane, not the window name"
    );
    let msg = tokio::time::timeout(TEST_TIMEOUT, rx.recv())
        .await
        .unwrap()
        .unwrap();
    assert!(
        matches!(
            &msg,
            Message::Split(crate::tui::messages::SplitMessage::PaneOpened {
                task_id: Some(TaskId(1)),
                ..
            })
        ),
        "Expected PaneOpened with task 1, got: {msg:?}"
    );
}

#[tokio::test]
async fn exec_enter_split_mode_with_task_kills_leftover_companion_panes_after_join() {
    // The window holds the agent's own pane (%1), the agent-tree companion
    // (%2) and an editor pane opened from it (%5). Once the agent's pane is
    // joined out, both companions must be killed: a lone tree pane is
    // indistinguishable from "hidden" to the agent-tree toggle, and an editor
    // pane has no owner left at all (docs/specs/agent-tree.allium:
    // ToggleVsSplitPaneInteraction).
    let db = test_db().await;
    let (tx, mut rx) = mpsc::unbounded_channel();
    let mock = Arc::new(
        MockProcessRunner::new(vec![
            MockProcessRunner::ok_with_stdout(b"%1\n"), // current_pane_id
            // companion_pane_ids: both dispatch-created panes, by their roles.
            MockProcessRunner::ok_with_stdout(b"%1 \n%2 agent_tree\n%5 editor\n"),
            MockProcessRunner::ok(), // join_pane: join-pane
            MockProcessRunner::ok(), // kill-pane %2
            MockProcessRunner::ok(), // kill-pane %5
        ])
        .with_windows(&["task-1"]),
    );
    let rt = make_runtime(db.clone(), tx, mock.clone()).await;

    rt.exec_enter_split_mode_with_task(TaskId(1), &test_tmux_window("task-1"))
        .await
        .unwrap();
    let calls = mock.recorded_calls();
    assert_eq!(calls.len(), 5, "calls: {calls:?}");
    assert_eq!(calls[3].1, vec!["kill-pane", "-t", "%2"]);
    assert_eq!(calls[4].1, vec!["kill-pane", "-t", "%5"]);
    let msg = tokio::time::timeout(TEST_TIMEOUT, rx.recv())
        .await
        .unwrap()
        .unwrap();
    assert!(
        matches!(
            &msg,
            Message::Split(crate::tui::messages::SplitMessage::PaneOpened {
                task_id: Some(TaskId(1)),
                ..
            })
        ),
        "Expected PaneOpened with task 1, got: {msg:?}"
    );
}

#[tokio::test]
async fn exec_enter_split_mode_with_task_succeeds_even_if_companion_check_fails() {
    // A failed companion-pane check must not block the join itself — it's a
    // best-effort cleanup, not the primary action.
    let db = test_db().await;
    let (tx, mut rx) = mpsc::unbounded_channel();
    let mock = Arc::new(
        MockProcessRunner::new(vec![
            MockProcessRunner::ok_with_stdout(b"%1\n"), // current_pane_id
            MockProcessRunner::fail("list-panes error"), // companion_pane_ids check
            MockProcessRunner::ok(),                    // join_pane: join-pane
        ])
        .with_windows(&["task-1"]),
    );
    let rt = make_runtime(db.clone(), tx, mock.clone()).await;

    rt.exec_enter_split_mode_with_task(TaskId(1), &test_tmux_window("task-1"))
        .await
        .unwrap();
    let calls = mock.recorded_calls();
    assert_eq!(
        calls.len(),
        3,
        "no kill-pane attempted after a failed check"
    );
    let msg = tokio::time::timeout(TEST_TIMEOUT, rx.recv())
        .await
        .unwrap()
        .unwrap();
    assert!(
        matches!(
            &msg,
            Message::Split(crate::tui::messages::SplitMessage::PaneOpened { .. })
        ),
        "Expected PaneOpened despite the failed companion check, got: {msg:?}"
    );
}

#[tokio::test]
async fn exec_enter_split_mode_with_task_succeeds_even_if_companion_kill_fails() {
    let db = test_db().await;
    let (tx, mut rx) = mpsc::unbounded_channel();
    let mock = Arc::new(
        MockProcessRunner::new(vec![
            MockProcessRunner::ok_with_stdout(b"%1\n"), // current_pane_id
            // companion_pane_ids: tree pane %2, no editor pane
            MockProcessRunner::ok_with_stdout(b"%1 \n%2 agent_tree\n"),
            MockProcessRunner::ok(), // join_pane: join-pane
            MockProcessRunner::fail("kill-pane error"),
        ])
        .with_windows(&["task-1"]),
    );
    let rt = make_runtime(db.clone(), tx, mock.clone()).await;

    rt.exec_enter_split_mode_with_task(TaskId(1), &test_tmux_window("task-1"))
        .await
        .unwrap();
    let msg = tokio::time::timeout(TEST_TIMEOUT, rx.recv())
        .await
        .unwrap()
        .unwrap();
    assert!(
        matches!(
            &msg,
            Message::Split(crate::tui::messages::SplitMessage::PaneOpened { .. })
        ),
        "Expected PaneOpened despite the failed companion kill, got: {msg:?}"
    );
}

#[tokio::test]
async fn exec_exit_split_mode_with_restore_breaks_pane() {
    let db = test_db().await;
    let (tx, mut rx) = mpsc::unbounded_channel();
    let mock = Arc::new(MockProcessRunner::new(vec![
        MockProcessRunner::ok(), // break_pane_to_window
    ]));
    let rt = make_runtime(db.clone(), tx, mock.clone()).await;

    rt.exec_exit_split_mode("%2", Some(&test_tmux_window("task-1")))
        .await
        .unwrap();
    let calls = mock.recorded_calls();
    assert!(calls[0].1.contains(&"break-pane".to_string()));
    let msg = tokio::time::timeout(TEST_TIMEOUT, rx.recv())
        .await
        .unwrap()
        .unwrap();
    assert!(
        matches!(
            msg,
            Message::Split(crate::tui::messages::SplitMessage::PaneClosed)
        ),
        "Expected PaneClosed, got: {msg:?}"
    );
}

#[tokio::test]
async fn exec_exit_split_mode_without_restore_kills_pane() {
    let db = test_db().await;
    let (tx, mut rx) = mpsc::unbounded_channel();
    let mock = Arc::new(MockProcessRunner::new(vec![
        MockProcessRunner::ok(), // kill_pane
    ]));
    let rt = make_runtime(db.clone(), tx, mock.clone()).await;

    rt.exec_exit_split_mode("%2", None).await.unwrap();
    let calls = mock.recorded_calls();
    assert!(calls[0].1.contains(&"kill-pane".to_string()));
    let msg = tokio::time::timeout(TEST_TIMEOUT, rx.recv())
        .await
        .unwrap()
        .unwrap();
    assert!(
        matches!(
            msg,
            Message::Split(crate::tui::messages::SplitMessage::PaneClosed)
        ),
        "Expected PaneClosed, got: {msg:?}"
    );
}

#[tokio::test]
async fn exec_check_split_pane_existing_pane_no_message() {
    let db = test_db().await;
    let (tx, mut rx) = mpsc::unbounded_channel();
    let mock = Arc::new(MockProcessRunner::new(vec![
        MockProcessRunner::ok_with_stdout(b"%1\n%2\n"), // pane_exists → listing contains %2
    ]));
    let rt = make_runtime(db.clone(), tx, mock).await;

    rt.exec_check_split_pane("%2").await.unwrap();
    assert!(
        rx.try_recv().is_err(),
        "expected no message when pane exists"
    );
}

#[tokio::test]
async fn exec_check_split_pane_gone_sends_closed() {
    let db = test_db().await;
    let (tx, mut rx) = mpsc::unbounded_channel();
    let mock = Arc::new(MockProcessRunner::new(vec![
        // pane_exists → the listing no longer contains %2. Note this is a
        // *successful* tmux call: real tmux exits 0 for an unknown pane, which is
        // why absence has to be detected by membership rather than exit status.
        MockProcessRunner::ok_with_stdout(b"%1\n%7\n"),
    ]));
    let rt = make_runtime(db.clone(), tx, mock).await;

    rt.exec_check_split_pane("%2").await.unwrap();
    let msg = tokio::time::timeout(TEST_TIMEOUT, rx.recv())
        .await
        .unwrap()
        .unwrap();
    assert!(
        matches!(
            msg,
            Message::Split(crate::tui::messages::SplitMessage::PaneClosed)
        ),
        "Expected PaneClosed, got: {msg:?}"
    );
}

#[tokio::test]
async fn exec_respawn_split_pane_gone_sends_closed() {
    let db = test_db().await;
    let (tx, mut rx) = mpsc::unbounded_channel();
    let mock = Arc::new(MockProcessRunner::new(vec![
        MockProcessRunner::fail("no pane"), // respawn_pane fails when pane is gone
    ]));
    let rt = make_runtime(db.clone(), tx, mock).await;

    rt.exec_respawn_split_pane("%2").await.unwrap();
    let msg = tokio::time::timeout(TEST_TIMEOUT, rx.recv())
        .await
        .unwrap()
        .unwrap();
    assert!(
        matches!(
            msg,
            Message::Split(crate::tui::messages::SplitMessage::PaneClosed)
        ),
        "Expected PaneClosed when pane gone, got: {msg:?}"
    );
}

#[tokio::test]
async fn exec_respawn_split_pane_respawn_fails_sends_closed() {
    let db = test_db().await;
    let (tx, mut rx) = mpsc::unbounded_channel();
    let mock = Arc::new(MockProcessRunner::new(vec![
        MockProcessRunner::fail("respawn err"), // respawn_pane fails
    ]));
    let rt = make_runtime(db.clone(), tx, mock).await;

    rt.exec_respawn_split_pane("%2").await.unwrap();
    let msg = tokio::time::timeout(TEST_TIMEOUT, rx.recv())
        .await
        .unwrap()
        .unwrap();
    assert!(
        matches!(
            msg,
            Message::Split(crate::tui::messages::SplitMessage::PaneClosed)
        ),
        "Expected PaneClosed when respawn fails, got: {msg:?}"
    );
}

#[tokio::test]
async fn exec_swap_split_pane_uses_swap_pane() {
    let db = test_db().await;
    let (tx, mut rx) = mpsc::unbounded_channel();
    // pane_id_for_window resolves out of band, so it is not a recorded call.
    let mock = Arc::new(
        MockProcessRunner::new(vec![
            MockProcessRunner::ok(), // swap-pane
            MockProcessRunner::ok(), // kill-window (old pane had no task)
        ])
        .with_windows(&["task-1"]),
    );
    let rt = make_runtime(db.clone(), tx, mock.clone()).await;

    rt.exec_swap_split_pane(TaskId(1), &test_tmux_window("task-1"), Some("%2"), None)
        .await
        .unwrap();
    let calls = mock.recorded_calls();
    // 1st call: swap-pane, sourced from the resolved pane ID rather than
    // `task-1.0` — a `<window>.<index>` target would prefix-match the window
    // name and depend on pane-base-index.
    assert!(calls[0].1.contains(&"swap-pane".to_string()));
    assert!(calls[0].1.contains(&mock.pane_id_of("task-1")));
    // 2nd call: kill-window (no old task to rename)
    assert!(calls[1].1.contains(&"kill-window".to_string()));
    // No 3rd call — focus must NOT be transferred
    assert_eq!(calls.len(), 2, "select-pane must not be called after swap");
    let msg = tokio::time::timeout(TEST_TIMEOUT, rx.recv())
        .await
        .unwrap()
        .unwrap();
    assert!(
        matches!(
            &msg,
            Message::Split(crate::tui::messages::SplitMessage::PaneOpened {
                task_id: Some(TaskId(1)),
                ..
            })
        ),
        "Expected PaneOpened with task 1, got: {msg:?}"
    );
}

#[tokio::test]
async fn exec_swap_split_pane_renames_old_task_window() {
    let db = test_db().await;
    let (tx, mut rx) = mpsc::unbounded_channel();
    // pane_id_for_window / the resync's own window lookups resolve out of band,
    // so they are not recorded calls.
    let mock = Arc::new(
        MockProcessRunner::new(vec![
            MockProcessRunner::ok(), // swap-pane
            MockProcessRunner::ok(), // rename-window (old task had a window)
            MockProcessRunner::ok(), // set-option -w: rewrite @dispatch_dir to task 2's worktree
            // resync: list-panes finds the companion. It is still running the
            // *incoming* task's tree (3), which is exactly why it is stale — the
            // lookup matches on the binary and subcommand, not the id.
            MockProcessRunner::ok_with_stdout(b"%10 \n%11 agent_tree\n"),
            MockProcessRunner::ok(), // resync: kill-pane %11
            MockProcessRunner::ok_with_stdout(b"/repo/.worktrees/2-some-task\n"), // resync: show-options @dispatch_dir
            MockProcessRunner::ok_with_stdout(b"%12\n"), // resync: split-window relaunch
            MockProcessRunner::ok(),                     // resync: set-option, the new pane's role
        ])
        .with_windows(&["task-3", "task-2"]),
    );
    let rt = make_runtime(db.clone(), tx, mock.clone()).await;

    rt.exec_swap_split_pane(
        TaskId(3),
        &test_tmux_window("task-3"),
        Some("%2"),
        Some((&test_tmux_window("task-2"), "/repo/.worktrees/2-some-task")),
    )
    .await
    .unwrap();
    let calls = mock.recorded_calls();
    // 2nd call should be rename-window, not kill-window
    assert!(calls[1].1.contains(&"rename-window".to_string()));
    // The rename *target* is the resolved pane ID; the new name stays a name.
    assert!(calls[1].1.contains(&mock.pane_id_of("task-3")));
    assert!(calls[1].1.contains(&"task-2".to_string()));
    // 3rd call: @dispatch_dir is rewritten to the outgoing task's worktree —
    // targeted by the *new* name ("task-2"), not the pane ID resolved in step
    // 1: swap-pane moves pane objects between windows, so that pane ID no
    // longer identifies anything in this window post-swap — only the window's
    // new name does. Without this rewrite the resync's start directory still
    // names task 3's worktree, since a rename never touches window options.
    assert_eq!(
        calls[2].1,
        vec![
            "set-option",
            "-w",
            "-t",
            &mock.pane_id_of("task-2"),
            "@dispatch_dir",
            "/repo/.worktrees/2-some-task",
        ]
    );
    // Companion pane resync: the renamed window's stale companion (still
    // showing the incoming task's tree) is killed and replaced with one for
    // the correct (old) task.
    assert!(calls[3].1.contains(&"list-panes".to_string()));
    assert_eq!(calls[4].1, vec!["kill-pane", "-t", "%11"]);
    assert!(calls[6].1.contains(&"split-window".to_string()));
    assert!(calls[6].1.contains(&"2".to_string()));
    // …and the respawned pane is marked, or the resynced window would read as
    // companion-less to the next toggle.
    assert!(calls[7].1.contains(&"set-option".to_string()));
    // No further call — focus must NOT be transferred
    assert_eq!(calls.len(), 8, "select-pane must not be called after swap");
    let msg = tokio::time::timeout(TEST_TIMEOUT, rx.recv())
        .await
        .unwrap()
        .unwrap();
    assert!(
        matches!(
            &msg,
            Message::Split(crate::tui::messages::SplitMessage::PaneOpened {
                task_id: Some(TaskId(3)),
                ..
            })
        ),
        "Expected PaneOpened with task 3, got: {msg:?}"
    );
}

/// Event-loop: split-mode functions send results via msg_tx (not app.update)
mod split_mode_via_msg_tx {
    use super::*;

    #[tokio::test]
    async fn exec_enter_split_mode_sends_pane_opened_via_msg_tx() {
        let db = test_db().await;
        let (tx, mut rx) = mpsc::unbounded_channel();
        let mock = Arc::new(MockProcessRunner::new(vec![
            MockProcessRunner::ok_with_stdout(b"%1\n"), // current_pane_id
            MockProcessRunner::ok_with_stdout(b"%2\n"), // split_window_horizontal
        ]));
        let rt = make_runtime(db.clone(), tx, mock).await;

        rt.exec_enter_split_mode().await.unwrap();

        let msg = tokio::time::timeout(TEST_TIMEOUT, rx.recv())
            .await
            .unwrap()
            .unwrap();
        assert!(
            matches!(
                &msg,
                Message::Split(crate::tui::messages::SplitMessage::PaneOpened {
                    pane_id,
                    task_id: None
                }) if pane_id == "%2"
            ),
            "Expected PaneOpened(%2), got: {msg:?}"
        );
    }

    #[tokio::test]
    async fn exec_enter_split_mode_no_tmux_sends_status_info_via_msg_tx() {
        let db = test_db().await;
        let (tx, mut rx) = mpsc::unbounded_channel();
        let mock = Arc::new(MockProcessRunner::new(vec![
            MockProcessRunner::fail("no server"), // current_pane_id fails
        ]));
        let rt = make_runtime(db.clone(), tx, mock).await;

        rt.exec_enter_split_mode().await.unwrap();

        let msg = tokio::time::timeout(TEST_TIMEOUT, rx.recv())
            .await
            .unwrap()
            .unwrap();
        assert!(
            matches!(
                &msg,
                Message::System(crate::tui::messages::SystemMessage::StatusInfo(s)) if s.contains("tmux")
            ),
            "Expected StatusInfo about tmux, got: {msg:?}"
        );
    }

    #[tokio::test]
    async fn exec_enter_split_mode_with_task_sends_pane_opened_via_msg_tx() {
        let db = test_db().await;
        let (tx, mut rx) = mpsc::unbounded_channel();
        let mock = Arc::new(
            MockProcessRunner::new(vec![
                MockProcessRunner::ok_with_stdout(b"%1\n"), // current_pane_id
                // companion_pane_ids: no pane carries a dispatch role
                MockProcessRunner::ok_with_stdout(b"%1 \n"),
                MockProcessRunner::ok(), // join_pane: join-pane command
            ])
            .with_windows(&["task-1"]),
        );
        let rt = make_runtime(db.clone(), tx, mock.clone()).await;

        rt.exec_enter_split_mode_with_task(TaskId(1), &test_tmux_window("task-1"))
            .await
            .unwrap();

        let calls = mock.recorded_calls();
        assert!(calls[2].1.contains(&"join-pane".to_string()));
        let msg = tokio::time::timeout(TEST_TIMEOUT, rx.recv())
            .await
            .unwrap()
            .unwrap();
        assert!(
            matches!(
                &msg,
                Message::Split(crate::tui::messages::SplitMessage::PaneOpened {
                    task_id: Some(TaskId(1)),
                    ..
                })
            ),
            "Expected PaneOpened with task, got: {msg:?}"
        );
    }

    #[tokio::test]
    async fn exec_exit_split_mode_with_restore_sends_pane_closed_via_msg_tx() {
        let db = test_db().await;
        let (tx, mut rx) = mpsc::unbounded_channel();
        let mock = Arc::new(MockProcessRunner::new(vec![
            MockProcessRunner::ok(), // break_pane_to_window
        ]));
        let rt = make_runtime(db.clone(), tx, mock.clone()).await;

        rt.exec_exit_split_mode("%2", Some(&test_tmux_window("task-1")))
            .await
            .unwrap();

        let calls = mock.recorded_calls();
        assert!(calls[0].1.contains(&"break-pane".to_string()));
        let msg = tokio::time::timeout(TEST_TIMEOUT, rx.recv())
            .await
            .unwrap()
            .unwrap();
        assert!(
            matches!(
                msg,
                Message::Split(crate::tui::messages::SplitMessage::PaneClosed)
            ),
            "Expected PaneClosed, got: {msg:?}"
        );
    }

    #[tokio::test]
    async fn exec_exit_split_mode_without_restore_sends_pane_closed_via_msg_tx() {
        let db = test_db().await;
        let (tx, mut rx) = mpsc::unbounded_channel();
        let mock = Arc::new(MockProcessRunner::new(vec![
            MockProcessRunner::ok(), // kill_pane
        ]));
        let rt = make_runtime(db.clone(), tx, mock.clone()).await;

        rt.exec_exit_split_mode("%2", None).await.unwrap();

        let calls = mock.recorded_calls();
        assert!(calls[0].1.contains(&"kill-pane".to_string()));
        let msg = tokio::time::timeout(TEST_TIMEOUT, rx.recv())
            .await
            .unwrap()
            .unwrap();
        assert!(
            matches!(
                msg,
                Message::Split(crate::tui::messages::SplitMessage::PaneClosed)
            ),
            "Expected PaneClosed, got: {msg:?}"
        );
    }

    #[tokio::test]
    async fn exec_swap_split_pane_kills_old_window_and_sends_pane_opened_via_msg_tx() {
        let db = test_db().await;
        let (tx, mut rx) = mpsc::unbounded_channel();
        // pane_id_for_window resolves out of band, so it is not a recorded call.
        let mock = Arc::new(
            MockProcessRunner::new(vec![
                MockProcessRunner::ok(), // swap-pane
                MockProcessRunner::ok(), // kill-window (old pane had no task)
            ])
            .with_windows(&["task-1"]),
        );
        let rt = make_runtime(db.clone(), tx, mock.clone()).await;

        rt.exec_swap_split_pane(TaskId(1), &test_tmux_window("task-1"), Some("%2"), None)
            .await
            .unwrap();

        let calls = mock.recorded_calls();
        assert!(calls[0].1.contains(&"swap-pane".to_string()));
        assert!(calls[1].1.contains(&"kill-window".to_string()));
        let msg = tokio::time::timeout(TEST_TIMEOUT, rx.recv())
            .await
            .unwrap()
            .unwrap();
        assert!(
            matches!(
                &msg,
                Message::Split(crate::tui::messages::SplitMessage::PaneOpened {
                    task_id: Some(TaskId(1)),
                    ..
                })
            ),
            "Expected PaneOpened with task, got: {msg:?}"
        );
    }

    #[tokio::test]
    async fn exec_focus_split_pane_returns_join_handle() {
        let db = test_db().await;
        let (tx, _rx) = mpsc::unbounded_channel();
        let mock = Arc::new(MockProcessRunner::new(vec![
            MockProcessRunner::ok(), // select-pane
        ]));
        let rt = make_runtime(db.clone(), tx, mock.clone()).await;

        // Must return a JoinHandle so the caller can fire-and-forget without blocking.
        rt.exec_focus_split_pane("%2".to_string()).await.unwrap();

        let calls = mock.recorded_calls();
        assert!(calls[0].1.contains(&"select-pane".to_string()));
    }
}
