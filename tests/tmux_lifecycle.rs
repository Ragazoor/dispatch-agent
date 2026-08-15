#![allow(clippy::unwrap_used, clippy::expect_used)]
//! Real-tmux integration tests for the **topology** dispatch builds: which
//! windows and panes exist after an operation, on which side of a split, and
//! with which resolved cwd.
//!
//! These drive the production entry points (`dispatch_agent`, `resume_agent`,
//! `join_task_window_into_pane`, …) against a real tmux server and then ask the
//! server what it actually built. Windows created by production run the default
//! shell — `tmux::new_window` takes no command — so their panes launch stub
//! `claude` / `dispatch` binaries (named to production by the harness runner)
//! that report their own cwd, pane and argv.
//!
//! Its sibling `tests/tmux_split_hook.rs` covers the complementary question,
//! *routing*: which pane a keystroke reached, observed with capture panes it
//! creates itself.
//!
//! The board window is ours in both files, so the invariant that **no keystroke
//! ever reaches the board** is assertable throughout. It matters because the
//! board is a TUI: a stray `cd <path>` is read as keybindings (`c` opens Copy
//! Task), which is precisely what #3781 did to users.
//!
//! Why a real server is needed at all, plus the stub and isolation mechanisms:
//! tests/tmux_harness/mod.rs.

mod tmux_harness;

use std::path::{Path, PathBuf};

use dispatch_tui::dispatch;
use dispatch_tui::models::{SubStatus, Task, TaskId, TaskStatus};
use dispatch_tui::process::ProcessRunner;
use dispatch_tui::tmux;

use tmux_harness::{
    await_stub_line, canonical, capture_cmd, read_now, stub_lines, tmux_available_or_skip,
    StubLine, TmuxServer,
};

/// The board TUI window. Created first so it is the session's active window —
/// the state during any dispatch or resume triggered from the board, and the
/// pane that must never receive keystrokes.
const BOARD_WINDOW: &str = "board";
const TASK_ID: i64 = 42;

// ---------------------------------------------------------------------------
// Fixture
// ---------------------------------------------------------------------------

struct Fixture {
    // Declared before `dir`: fields drop in declaration order, so the server
    // (and the pane processes holding cwds inside `dir`) dies before the temp
    // dir is unlinked.
    server: TmuxServer,
    dir: tempfile::TempDir,
    /// The repo `task.repo_path` points at. Worktrees land in `<repo>/.worktrees`.
    repo: PathBuf,
    board_log: PathBuf,
}

fn setup_or_skip() -> Option<Fixture> {
    if !tmux_available_or_skip() {
        return None;
    }
    Some(setup())
}

fn setup() -> Fixture {
    setup_with(seed_repo)
}

/// The no-`origin`-remote variant of `setup_or_skip`, for the fallback path
/// that has no working local `origin` to fetch from at all.
fn setup_no_origin_or_skip() -> Option<Fixture> {
    if !tmux_available_or_skip() {
        return None;
    }
    Some(setup_with(seed_repo_no_origin))
}

fn setup_with(seed: fn(&Path) -> PathBuf) -> Fixture {
    let dir = tempfile::tempdir().unwrap();
    let repo = seed(dir.path());
    let board_log = dir.path().join("board.log");

    let server = TmuxServer::start();
    server.tmux_ok(&[
        "new-session",
        "-d",
        "-s",
        "t",
        "-n",
        BOARD_WINDOW,
        "--",
        "sh",
        "-c",
        &capture_cmd(&board_log),
    ]);

    Fixture {
        server,
        dir,
        repo,
        board_log,
    }
}

/// A git repo with a working local `origin`, which is what `provision_worktree`
/// expects: on a successful fetch `select_start_point` compares local `<base>`
/// against `origin/<base>` and only prefers local when it holds commits origin
/// lacks; otherwise `git worktree add` is given `origin/<base>` as its start
/// point.
///
/// The origin is not cosmetic. Without it, `classify_fetch_failure`'s
/// `has_origin_remote` check short-circuits `fetch_origin` to a missing-ref
/// outcome on the first attempt — one call, no retries, no sleep — and every
/// dispatch would exercise the stale-fallback path instead of the normal one.
/// (`seed_repo_no_origin`, below, is the fixture that deliberately does that.)
fn seed_repo(root: &Path) -> PathBuf {
    let origin = root.join("origin.git");
    let repo = root.join("repo");
    git(root, &["init", "-q", "--bare", origin.to_str().unwrap()]);
    git(root, &["init", "-q", "-b", "main", repo.to_str().unwrap()]);
    std::fs::write(repo.join("README.md"), "hello\n").unwrap();
    git(&repo, &["add", "README.md"]);
    git(&repo, &["commit", "-qm", "seed"]);
    git(
        &repo,
        &["remote", "add", "origin", origin.to_str().unwrap()],
    );
    git(&repo, &["push", "-q", "origin", "main"]);
    repo
}

/// A git repo with a commit on `main` and **no `origin` remote at all** — the
/// state `classify_fetch_failure`'s `has_origin_remote` check falls back on.
/// Kept separate from `seed_repo` rather than parameterising it, since other
/// tests depend on `seed_repo`'s existing shape (a working local `origin`).
fn seed_repo_no_origin(root: &Path) -> PathBuf {
    let repo = root.join("repo");
    git(root, &["init", "-q", "-b", "main", repo.to_str().unwrap()]);
    std::fs::write(repo.join("README.md"), "hello\n").unwrap();
    git(&repo, &["add", "README.md"]);
    git(&repo, &["commit", "-qm", "seed"]);
    repo
}

/// Run git with a sanitised environment. The identity vars matter: without them
/// the seed commit fails outright on a machine with no `user.email` configured,
/// which is what a fresh CI container is. The config overrides keep a
/// developer's global `commit.gpgsign` / `init.defaultBranch` / hooks path from
/// changing what the fixture builds.
fn git(cwd: &Path, args: &[&str]) {
    let out = std::process::Command::new("git")
        .args(args)
        .current_dir(cwd)
        .env("GIT_CONFIG_GLOBAL", "/dev/null")
        .env("GIT_CONFIG_SYSTEM", "/dev/null")
        .env("GIT_AUTHOR_NAME", "test")
        .env("GIT_AUTHOR_EMAIL", "test@example.com")
        .env("GIT_COMMITTER_NAME", "test")
        .env("GIT_COMMITTER_EMAIL", "test@example.com")
        .output()
        .expect("run git");
    assert!(
        out.status.success(),
        "git {:?} failed: {}",
        args,
        String::from_utf8_lossy(&out.stderr)
    );
}

/// Like `git`, but returns trimmed stdout — for queries such as `rev-parse`.
fn git_stdout(cwd: &Path, args: &[&str]) -> String {
    let out = std::process::Command::new("git")
        .args(args)
        .current_dir(cwd)
        .env("GIT_CONFIG_GLOBAL", "/dev/null")
        .env("GIT_CONFIG_SYSTEM", "/dev/null")
        .output()
        .expect("run git");
    assert!(
        out.status.success(),
        "git {:?} failed: {}",
        args,
        String::from_utf8_lossy(&out.stderr)
    );
    String::from_utf8_lossy(&out.stdout).trim().to_string()
}

fn task(id: i64, repo: &Path) -> Task {
    Task {
        id: TaskId(id),
        title: "Some task".to_string(),
        description: "Do the thing".to_string(),
        repo_path: repo.to_string_lossy().into_owned(),
        status: TaskStatus::Backlog,
        worktree: None,
        tmux_window: None,
        plan_path: None,
        epic_id: None,
        sub_status: SubStatus::None,
        url: None,
        tag: None,
        sort_order: None,
        base_branch: "main".to_string(),
        external_id: None,
        labels: Vec::new(),
        created_at: chrono::Utc::now(),
        updated_at: chrono::Utc::now(),
        last_pre_tool_use_at: None,
        last_notification_at: None,
        wrap_up_mode: None,
        auto_run_plan: false,
        live_subagents: 0,
        stop_pending: false,
    }
}

impl Fixture {
    fn window(&self, id: i64) -> String {
        format!("task-{id}")
    }

    /// Dispatch a task through the production entry point.
    fn dispatch(&self, id: i64) -> dispatch_tui::models::DispatchResult {
        dispatch::dispatch_agent(
            &task(id, &self.repo),
            &self.server.runner(),
            None,
            &Default::default(),
        )
        .expect("dispatch_agent")
    }

    /// Resume a task whose worktree exists but whose window does not.
    fn resume(&self, id: i64, worktree: &Path) -> dispatch_tui::models::ResumeResult {
        dispatch::resume_agent(
            TaskId(id),
            worktree.to_str().unwrap(),
            &self.server.runner(),
        )
        .expect("resume_agent")
    }

    /// Pin `window`'s agent pane into the board window. Returns the pinned pane.
    fn pin(&self, window: &str) -> String {
        dispatch::join_task_window_into_pane(window, &self.board_pane(), &self.server.runner())
            .expect("join_task_window_into_pane")
    }

    /// Swap `into_window`'s task into the already-pinned `pane`, renaming the
    /// displaced window back to `old_task`'s window name and rewriting its
    /// `@dispatch_dir` to `old_task`'s worktree — `(window_name,
    /// worktree_path)` of the outgoing task, when it has one.
    fn swap(&self, into_window: &str, pane: &str, old_task: Option<(&str, &str)>) -> String {
        dispatch::swap_task_window_into_pane(into_window, pane, old_task, &self.server.runner())
            .expect("swap_task_window_into_pane")
    }

    /// An agent window with no companion pane — the state after the user toggles
    /// the tree pane off. Its pane holds open on `cat` so it cannot vanish
    /// mid-assertion.
    fn bare_agent_window(&self, id: i64) -> String {
        let window = self.window(id);
        self.server
            .tmux_ok(&["new-window", "-d", "-n", &window, "--", "sh", "-c", "cat"]);
        window
    }

    /// Block until the companion pane for `id` has started and logged itself.
    ///
    /// Doubles as the happens-before anchor for negative assertions: the
    /// companion split is what fires the `after-split-window` hook, so once this
    /// returns, anything the hook misrouted has already been written too.
    fn await_companion(&self, id: i64) -> StubLine {
        let want = format!("agent-tree {id}");
        await_stub_line(&self.server, |l| l.args == want).unwrap_or_else(|| {
            panic!(
                "companion pane for task {id} never ran `{want}`; log: {:?}",
                stub_lines(&self.server)
            )
        })
    }

    /// Provision a worktree without dispatching — the state a detached task is
    /// in (worktree on disk, no live window), which is what `resume_agent`
    /// expects. Done with git directly rather than by dispatching and killing
    /// the window, so resume is observed in isolation.
    fn add_worktree(&self, id: i64) -> PathBuf {
        let branch = format!("{id}-some-task");
        let path = self.repo.join(".worktrees").join(&branch);
        git(
            &self.repo,
            &[
                "worktree",
                "add",
                "-q",
                "-b",
                &branch,
                path.to_str().unwrap(),
                "origin/main",
            ],
        );
        path
    }

    /// The board window's own pane — the target a pin joins into.
    fn board_pane(&self) -> String {
        self.server
            .active_pane_id(BOARD_WINDOW)
            .expect("board pane")
    }

    /// Drop everything recorded so far, so a later assertion cannot be satisfied
    /// by a stub invocation from an earlier step (e.g. the `agent-tree <id>` the
    /// original dispatch logged, when the point is that a *resync* relaunched it).
    fn clear_stub_log(&self) {
        let _ = std::fs::remove_file(self.server.stub_log());
    }

    /// The board must never receive keystrokes. Safe to call only after the
    /// operation under test has been observed to complete, so it is not a race
    /// that happens to look clean.
    fn assert_board_untouched(&self) {
        let board = read_now(&self.board_log);
        assert!(
            board.trim().is_empty(),
            "the board TUI must never receive keystrokes — it reads them as \
             keybindings (`c` opens Copy Task). got: {board:?}"
        );
    }
}

/// Wait for the invocation of `binary` (`claude` / `dispatch`).
fn stub_for(fx: &Fixture, binary: &str) -> StubLine {
    await_stub_line(&fx.server, |l| l.name == binary).unwrap_or_else(|| {
        panic!(
            "no `{binary}` stub invocation recorded; log was: {:?}",
            stub_lines(&fx.server)
        )
    })
}

// ---------------------------------------------------------------------------
// Harness self-test
// ---------------------------------------------------------------------------

/// The `-f /dev/null` isolation is invisible when it works, so assert it. A
/// developer's `~/.tmux.conf` setting `pane-base-index`, `default-command` or a
/// hook would otherwise silently change what every test below observes, and CI
/// (which has no config) would be exercising a different tmux.
#[test]
fn harness_ignores_the_developers_tmux_config() {
    if !tmux_available_or_skip() {
        return;
    }
    let server = TmuxServer::start();
    server.tmux_ok(&["new-session", "-d", "-s", "t", "-n", "w"]);

    assert_eq!(
        server.tmux_stdout(&["show-options", "-gv", "prefix"]),
        "C-b",
        "test servers must run with tmux defaults, not the developer's config"
    );
    assert_eq!(
        server.tmux_stdout(&["show-options", "-gv", "pane-base-index"]),
        "0",
        "pane-base-index must be the default — tests that care set it explicitly"
    );
}

/// The stub seam is invisible when it works, and the failure mode is severe: a
/// real `claude` spawns a live agent and can hang on its trust prompt, a real
/// `dispatch` opens the developer's database. This replaces the two `PATH`-era
/// resolution guards with one assertion about the mechanism that now does the
/// work — the runner naming the binaries.
#[test]
fn harness_runner_names_its_own_stub_binaries() {
    if !tmux_available_or_skip() {
        return;
    }
    let server = TmuxServer::start();
    let bins = server.runner().agent_binaries();

    let stub_root = server.stub_log().parent().unwrap().to_path_buf();
    for path in [&bins.claude, &bins.dispatch] {
        let path = Path::new(path);
        assert!(
            path.starts_with(&stub_root),
            "agent binary {path:?} must live in this server's stub dir {stub_root:?}"
        );
        assert!(path.is_file(), "stub {path:?} was not written");
    }
    assert_ne!(
        bins,
        Default::default(),
        "the bare names must be overridden"
    );
}

// ---------------------------------------------------------------------------
// Step 1 — dispatch
// ---------------------------------------------------------------------------

#[test]
fn dispatch_creates_agent_window_named_for_the_task() {
    let Some(fx) = setup_or_skip() else { return };

    let result = fx.dispatch(TASK_ID);

    assert_eq!(result.tmux_window, fx.window(TASK_ID));
    assert!(
        fx.server.has_window(&fx.window(TASK_ID)),
        "expected a task-{TASK_ID} window, got: {:?}",
        fx.server.window_names()
    );
}

/// The real-server analogue of the argv-level
/// `dispatch_agent_opens_tmux_window_in_worktree_not_parent_repo`: that test
/// proves we passed `-c <worktree>`, this one proves tmux resolved it.
#[test]
fn dispatch_agent_window_starts_in_the_worktree_not_the_parent_repo() {
    let Some(fx) = setup_or_skip() else { return };

    let result = fx.dispatch(TASK_ID);
    let agent_pane = fx
        .server
        .active_pane_id(&fx.window(TASK_ID))
        .expect("agent pane");

    let cwd = fx.server.pane_cwd(&agent_pane);
    assert_eq!(
        canonical(&cwd),
        canonical(&result.worktree_path),
        "agent pane must open in the task worktree, not the parent repo"
    );
}

#[test]
fn dispatch_launches_claude_in_the_worktree() {
    let Some(fx) = setup_or_skip() else { return };

    let result = fx.dispatch(TASK_ID);

    let line = stub_for(&fx, "claude");
    assert_eq!(
        canonical(&line.cwd),
        canonical(&result.worktree_path),
        "claude must run from the worktree; line: {line:?}"
    );
    assert!(
        line.args.contains("--plugin-dir"),
        "claude must be launched with the dispatch plugin dir; line: {line:?}"
    );
}

#[test]
fn dispatch_opens_the_companion_agent_tree_pane() {
    let Some(fx) = setup_or_skip() else { return };

    fx.dispatch(TASK_ID);

    fx.await_companion(TASK_ID);
    let window = fx.window(TASK_ID);
    assert_eq!(
        fx.server.pane_count(&window),
        2,
        "agent window should hold the agent pane plus its companion"
    );
    // The marker every later lookup depends on, read back off a real server:
    // the companion pane carries the agent-tree role and the agent's own pane
    // carries none (docs/specs/agent-tree.allium: HideAgentTreePane). Located by
    // start command rather than by the marker itself, so this cannot pass by
    // agreeing with production about the wrong pane.
    let companion = fx
        .server
        .pane_ids(&window)
        .into_iter()
        .find(|id| fx.server.pane_start_command(id).contains("agent-tree"))
        .expect("companion pane");
    let agent = fx.server.active_pane_id(&window).expect("agent pane");
    assert_eq!(
        fx.server
            .pane_option(&companion, dispatch_tui::tmux::PANE_ROLE_OPTION),
        dispatch_tui::tmux::PANE_ROLE_AGENT_TREE,
    );
    assert_eq!(
        fx.server
            .pane_option(&agent, dispatch_tui::tmux::PANE_ROLE_OPTION),
        "",
        "the agent's own pane is not dispatch-created and must carry no role"
    );
}

/// Locks the `-b` in `split_window_horizontal_running`: the companion goes on
/// the left. Asserted via `pane_left` rather than pane index, because a `-b`
/// split renumbers indices — the very reason index-based targeting is unsafe.
#[test]
fn dispatch_companion_pane_is_on_the_left() {
    let Some(fx) = setup_or_skip() else { return };

    fx.dispatch(TASK_ID);
    let window = fx.window(TASK_ID);
    let companion = fx.await_companion(TASK_ID);

    assert_eq!(
        fx.server.leftmost_pane_id(&window).as_deref(),
        Some(companion.pane.as_str()),
        "companion pane should be leftmost; panes: {:?}",
        fx.server.pane_lefts(&window)
    );
}

#[test]
fn dispatch_sets_dispatch_dir_on_the_agent_window() {
    let Some(fx) = setup_or_skip() else { return };

    let result = fx.dispatch(TASK_ID);

    assert_eq!(
        canonical(
            &fx.server
                .window_option(&fx.window(TASK_ID), "@dispatch_dir")
        ),
        canonical(&result.worktree_path),
        "@dispatch_dir is the split hook's precondition"
    );
}

#[test]
fn dispatch_never_types_into_the_board_window() {
    let Some(fx) = setup_or_skip() else { return };

    fx.dispatch(TASK_ID);
    fx.await_companion(TASK_ID);

    fx.assert_board_untouched();
}

// ---------------------------------------------------------------------------
// Step 2 — resume
// ---------------------------------------------------------------------------

#[test]
fn resume_creates_a_new_window_for_a_worktree_without_one() {
    let Some(fx) = setup_or_skip() else { return };
    let worktree = fx.add_worktree(TASK_ID);
    assert!(!fx.server.has_window(&fx.window(TASK_ID)));

    let result = fx.resume(TASK_ID, &worktree);

    assert_eq!(result.tmux_window, fx.window(TASK_ID));
    assert!(fx.server.has_window(&fx.window(TASK_ID)));
}

/// The stale-field/live-window case this whole change fixes: the worktree's
/// window is already alive under its deterministic name (e.g. a persist that
/// never landed, or a race clearing the DB field) even though resume is being
/// asked to create one. Resume must reattach, not spawn a duplicate.
#[test]
fn resume_reattaches_to_a_live_window_without_creating_a_duplicate() {
    let Some(fx) = setup_or_skip() else { return };
    let worktree = fx.add_worktree(TASK_ID);
    let window = fx.bare_agent_window(TASK_ID);
    fx.clear_stub_log();

    let result = fx.resume(TASK_ID, &worktree);

    assert_eq!(result.tmux_window, window);
    assert_eq!(
        fx.server
            .window_names()
            .iter()
            .filter(|n| **n == window)
            .count(),
        1,
        "resume must not create a second window sharing the live one's name"
    );
    assert_eq!(
        fx.server.pane_count(&window),
        1,
        "resume must not spawn a companion pane into an already-live window"
    );
    assert!(
        stub_lines(&fx.server).is_empty(),
        "resume must not launch a fresh claude process into an already-live window"
    );
}

/// `--continue` has to reach the agent's own pane, in the worktree. If it landed
/// in the companion pane or resolved the wrong cwd, resume would silently start
/// a fresh conversation instead of continuing the task's.
#[test]
fn resume_reaches_the_agent_pane_with_continue() {
    let Some(fx) = setup_or_skip() else { return };
    let worktree = fx.add_worktree(TASK_ID);

    fx.resume(TASK_ID, &worktree);

    let line = stub_for(&fx, "claude");
    assert!(
        line.args.contains("--continue"),
        "resume must launch claude with --continue; line: {line:?}"
    );
    assert_eq!(
        canonical(&line.cwd),
        canonical(worktree.to_str().unwrap()),
        "claude must continue from inside the worktree; line: {line:?}"
    );
    assert_eq!(
        fx.server.active_pane_id(&fx.window(TASK_ID)).as_deref(),
        Some(line.pane.as_str()),
        "--continue must reach the agent's own pane, not the companion"
    );
}

#[test]
fn resume_opens_the_companion_pane() {
    let Some(fx) = setup_or_skip() else { return };
    let worktree = fx.add_worktree(TASK_ID);

    fx.resume(TASK_ID, &worktree);

    fx.await_companion(TASK_ID);
    assert_eq!(fx.server.pane_count(&fx.window(TASK_ID)), 2);
}

#[test]
fn resume_never_types_into_the_board_window() {
    let Some(fx) = setup_or_skip() else { return };
    let worktree = fx.add_worktree(TASK_ID);

    fx.resume(TASK_ID, &worktree);
    fx.await_companion(TASK_ID);

    fx.assert_board_untouched();
}

// ---------------------------------------------------------------------------
// Step 3 — split-pane: pin / swap / unpin
// ---------------------------------------------------------------------------

/// Pinning moves only the agent's own pane. The companion left behind would
/// become its window's sole pane — indistinguishable from "hidden" to the
/// agent-tree toggle — so it must be killed (docs/specs/agent-tree.allium:
/// ToggleVsSplitPaneInteraction).
#[test]
fn pin_joins_the_agent_pane_and_kills_the_leftover_companion() {
    let Some(fx) = setup_or_skip() else { return };
    fx.dispatch(TASK_ID);
    let window = fx.window(TASK_ID);
    // Wait for the companion, so the pin genuinely has one to clean up.
    fx.await_companion(TASK_ID);
    // Located by the harness, not by production's own lookup: an oracle that
    // called `dispatch::agent_tree_pane_id` would assert that pin killed
    // whatever that function names, and would keep passing if it named the
    // wrong pane — which is the failure mode #3856 was about.
    let companion = fx
        .server
        .pane_ids(&window)
        .into_iter()
        .find(|id| fx.server.pane_start_command(id).contains("agent-tree"))
        .expect("companion pane should exist before pinning");
    let agent_pane = fx.server.active_pane_id(&window).expect("agent pane");

    let joined = fx.pin(&window);

    assert_eq!(
        joined, agent_pane,
        "tmux preserves pane ids across a move, so the pinned pane is the agent's"
    );
    assert_eq!(
        fx.server.pane_count(BOARD_WINDOW),
        2,
        "board should hold its own pane plus the pinned agent"
    );
    assert!(
        !fx.server.pane_exists(&companion),
        "the leftover companion pane must be killed, not left as a phantom window"
    );
}

#[test]
fn pin_of_a_task_without_a_companion_pane_joins_cleanly() {
    let Some(fx) = setup_or_skip() else { return };
    let window = fx.bare_agent_window(TASK_ID);
    let agent_pane = fx.server.active_pane_id(&window).expect("agent pane");

    let joined = fx.pin(&window);

    assert_eq!(joined, agent_pane);
    assert_eq!(fx.server.pane_count(BOARD_WINDOW), 2);
}

/// After a swap the standalone window is renamed to the outgoing task, but
/// `swap-pane` never touched its companion — which would keep rendering the
/// previous occupant's tree under the new name. `resync_agent_tree_pane` must
/// relaunch it for the task the window now represents.
#[test]
fn swap_replaces_the_pinned_task_and_resyncs_the_companion() {
    let Some(fx) = setup_or_skip() else { return };
    let (a, b) = (TASK_ID, TASK_ID + 1);
    let dispatched_a = fx.dispatch(a);
    fx.dispatch(b);
    fx.await_companion(b);

    // Pin A, then swap B in over it.
    let pinned = fx.pin(&fx.window(a));
    fx.clear_stub_log();

    fx.swap(
        &fx.window(b),
        &pinned,
        Some((&fx.window(a), &dispatched_a.worktree_path)),
    );

    // The window holding the outgoing content is renamed to A, and its companion
    // must be relaunched for A. Polls, because the resync kills and re-splits
    // asynchronously relative to the call returning.
    let companion = fx.await_companion(a);
    // `swap_task_window_into_pane` rewrites the renamed window's @dispatch_dir
    // to A's worktree immediately after the rename and before resyncing — see
    // docs/specs/split-pane.allium's SwapSplitPane — so the resync's start
    // directory reflects the window's new identity rather than racing whatever
    // @dispatch_dir happened to hold from when the window was still task B's.
    assert!(
        canonical(&companion.cwd).ends_with(&format!("{a}-some-task")),
        "expected the companion to start in A's own worktree, got: {:?}",
        companion.cwd
    );
    assert!(
        fx.server.has_window(&fx.window(a)),
        "the outgoing task's window should exist under its own name again; got {:?}",
        fx.server.window_names()
    );
}

/// `pane-base-index 1` is a common user setting, and it makes the `<window>.0`
/// target form unresolvable — no pane has index 0. Regression test for the swap
/// source, which must address the pane by id.
///
/// This is learning #324 (never target a pane by hardcoded index) in a spot
/// #3781 did not sweep. A `-b` split also renumbers indices, so index-based
/// targeting is unsafe even at the default base index.
#[test]
fn swap_works_when_pane_base_index_is_1() {
    let Some(fx) = setup_or_skip() else { return };
    fx.server
        .tmux_ok(&["set-option", "-g", "pane-base-index", "1"]);
    let (a, b) = (TASK_ID, TASK_ID + 1);
    let dispatched_a = fx.dispatch(a);
    fx.dispatch(b);
    fx.await_companion(b);

    let pinned = fx.pin(&fx.window(a));

    // Panics with "can't find pane: 0" if the swap source is ever an index again.
    fx.swap(
        &fx.window(b),
        &pinned,
        Some((&fx.window(a), &dispatched_a.worktree_path)),
    );
}

#[test]
fn unpin_breaks_the_pane_back_into_its_own_window() {
    let Some(fx) = setup_or_skip() else { return };
    let window = fx.bare_agent_window(TASK_ID);
    let pinned = fx.pin(&window);
    assert!(!fx.server.has_window(&window), "window consumed by the pin");

    tmux::break_pane_to_window(&pinned, &window, &fx.server.runner()).expect("unpin");

    assert!(
        fx.server.has_window(&window),
        "unpin should restore the task's own window; got {:?}",
        fx.server.window_names()
    );
    assert_eq!(
        fx.server.pane_ids(&window),
        vec![pinned],
        "the same pane should be restored, not a fresh one"
    );
    assert_eq!(
        fx.server.pane_count(BOARD_WINDOW),
        1,
        "board should be back to just its own pane"
    );
}

#[test]
fn split_operations_never_type_into_the_board_window() {
    let Some(fx) = setup_or_skip() else { return };
    let window = fx.window(TASK_ID);
    fx.dispatch(TASK_ID);
    fx.await_companion(TASK_ID);

    let pinned = fx.pin(&window);
    tmux::break_pane_to_window(&pinned, &window, &fx.server.runner()).expect("unpin");

    // The board's pane is the one that would absorb a mistargeted keystroke,
    // and it is still the same `cat > board.log` process throughout.
    fx.assert_board_untouched();
}

// ---------------------------------------------------------------------------
// Step 4 — teardown
// ---------------------------------------------------------------------------

#[test]
fn killing_the_agent_window_removes_all_its_panes() {
    let Some(fx) = setup_or_skip() else { return };
    fx.dispatch(TASK_ID);
    let window = fx.window(TASK_ID);
    fx.await_companion(TASK_ID);
    let panes = fx.server.pane_ids(&window);
    assert_eq!(panes.len(), 2, "expected agent + companion");

    tmux::kill_window_if_present(&window, &fx.server.runner()).expect("kill window");

    assert!(!fx.server.has_window(&window));
    for pane in panes {
        assert!(
            !fx.server.pane_exists(&pane),
            "pane {pane} outlived its window"
        );
    }
}

/// The ConfirmDone invariant: moving a task Review→Done kills the tmux window
/// but never removes the worktree — unlike Archive/Delete, which do full
/// cleanup. The *decision* is unit-covered in src/tui/tests/wrap_up.rs; this
/// asserts the tmux and filesystem effect.
#[test]
fn killing_the_agent_window_leaves_the_worktree_intact() {
    let Some(fx) = setup_or_skip() else { return };
    let result = fx.dispatch(TASK_ID);
    let worktree = PathBuf::from(&result.worktree_path);

    tmux::kill_window_if_present(&fx.window(TASK_ID), &fx.server.runner()).expect("kill window");

    assert!(!fx.server.has_window(&fx.window(TASK_ID)));
    assert!(worktree.is_dir(), "worktree directory must survive");
    assert!(
        worktree.join(".git").exists(),
        "worktree must still be a git worktree, not an orphaned directory"
    );
}

// The #4096 window-only teardown gets no real-server test of its own: what tmux
// does there is `kill_window_if_present`, already proven against this server by
// the two tests above. The only new fact is that `dispatch::teardown_task` does
// not skip step 1 when its worktree argument is `None` — control flow, so it sits
// on the mock side of the split in docs/conventions.md
// (`src/dispatch/tests.rs::teardown_task_kills_window_when_there_is_no_worktree`).

// ---------------------------------------------------------------------------
// Step 5 — main session
// ---------------------------------------------------------------------------

/// The main session deliberately gets no companion agent-tree pane: it has no
/// task id and no worktree, so the tree would be permanently empty, and the
/// window is covered by neither teardown rule. Specified in
/// docs/specs/agent-tree.allium's `SplitAgentTreePaneOnAgentLaunch`
/// ("Resolves MainSessionPaneScope").
#[test]
fn main_session_window_has_a_single_pane() {
    let Some(fx) = setup_or_skip() else { return };

    let window =
        dispatch::create_main_session(fx.dir.path().to_str().unwrap(), &fx.server.runner())
            .expect("create_main_session");

    // Anchor on the session's own claude launch, so "one pane" is not observed
    // before a companion split would have happened.
    stub_for(&fx, "claude");
    assert_eq!(
        fx.server.pane_count(&window),
        1,
        "main session must not get a companion pane; panes: {:?}",
        fx.server.pane_lefts(&window)
    );
}

/// Carrying no `@dispatch_dir` is what keeps the split-correction hook's
/// `if-shell -F` guard inert for this window, so a user splitting it by hand
/// gets a plain pane rather than one dragged into some task's worktree.
#[test]
fn splitting_the_main_session_window_is_never_corrected() {
    let Some(fx) = setup_or_skip() else { return };
    // Dispatch first: it installs the hook, and gives us a window that *does*
    // fire it to anchor against.
    let dispatched = fx.dispatch(TASK_ID);
    let window =
        dispatch::create_main_session(fx.dir.path().to_str().unwrap(), &fx.server.runner())
            .expect("create_main_session");
    assert_eq!(
        fx.server.window_option(&window, "@dispatch_dir"),
        "",
        "main session must carry no @dispatch_dir"
    );

    // `None` for the start directory on both splits: naming one would make the
    // hook skip them, and it is precisely the hook's behaviour under test here.
    let main_log = fx.dir.path().join("main_split.log");
    let main_pane = tmux::split_window_horizontal_running(
        &window,
        30,
        &["sh", "-c", &capture_cmd(&main_log)],
        None,
        &fx.server.runner(),
    )
    .expect("split main session");

    // Anchor: a split in the agent window *does* fire the hook, so by the time
    // that pane has been moved into the worktree, anything the main-session
    // split had coming would have arrived too.
    let agent_log = fx.dir.path().join("agent_split.log");
    let agent_pane = tmux::split_window_horizontal_running(
        &fx.window(TASK_ID),
        30,
        &["sh", "-c", &capture_cmd(&agent_log)],
        None,
        &fx.server.runner(),
    )
    .expect("split agent window");
    let worktree = canonical(&dispatched.worktree_path);
    assert!(
        tmux_harness::poll_until(|| canonical(&fx.server.pane_cwd(&agent_pane)) == worktree),
        "anchor split was never corrected, so this test proves nothing"
    );

    assert_ne!(
        canonical(&fx.server.pane_cwd(&main_pane)),
        worktree,
        "a split in the main-session window must not be pulled into a task's \
         worktree"
    );
    let got = read_now(&main_log);
    assert!(
        got.trim().is_empty(),
        "a split in the main-session window must receive nothing, got: {got:?}"
    );
    fx.assert_board_untouched();
}

// ---------------------------------------------------------------------------
// Worktree start point — local vs. origin `<base>`
// ---------------------------------------------------------------------------

/// This asks a *git* question, not a tmux one. It lives in this file only
/// because this is the one harness with a real repo, a real `origin` and a real
/// dispatch — a mock cannot answer it, because a mock never runs
/// `git worktree add` for real. Do not "simplify" it onto `MockProcessRunner`.
#[test]
fn fresh_dispatch_prefers_local_base_when_it_is_ahead_of_origin() {
    // `setup_or_skip` wraps `tmux_available_or_skip`, which skips locally when
    // tmux is missing but hard-fails under `CI` — so this cannot quietly stop
    // running. This is the file's established guard pattern.
    let Some(fx) = setup_or_skip() else { return };

    // Land a commit on local main WITHOUT pushing — exactly what the rebase
    // wrap-up path produces, and the drift this task exists to respect.
    std::fs::write(fx.repo.join("landed.txt"), "from a finished task\n").unwrap();
    git(&fx.repo, &["add", "landed.txt"]);
    git(&fx.repo, &["commit", "-qm", "landed but unpushed"]);

    let local_main = git_stdout(&fx.repo, &["rev-parse", "main"]);
    let origin_main = git_stdout(&fx.repo, &["rev-parse", "origin/main"]);
    assert_ne!(local_main, origin_main, "fixture must actually be ahead");

    let result = fx.dispatch(4242);

    let branch = git_stdout(&fx.repo, &["rev-parse", "4242-some-task"]);
    assert_eq!(
        branch, local_main,
        "worktree must start from local main, which holds the landed work"
    );
    assert_ne!(
        branch, origin_main,
        "must not start from the stale origin ref"
    );
    assert!(std::path::Path::new(&result.worktree_path).exists());
}

/// #3804's premise, preserved: with local and origin level, the branch is the
/// start point, which is what makes a fresh dispatch's rebase a no-op.
#[test]
fn fresh_dispatch_with_level_base_starts_from_origin() {
    let Some(fx) = setup_or_skip() else { return };
    let result = fx.dispatch(4243);

    let branch = git_stdout(&fx.repo, &["rev-parse", "4243-some-task"]);
    assert_eq!(branch, git_stdout(&fx.repo, &["rev-parse", "origin/main"]));
    assert!(std::path::Path::new(&result.worktree_path).exists());
}

/// The no-`origin`-remote row of the start-point behaviour table: with no
/// `origin` configured at all, `classify_fetch_failure`'s `has_origin_remote`
/// check must short-circuit straight to the local-branch fallback, without
/// burning the fetch retry budget, and the dispatch must still succeed.
///
/// The agent's prompt carries a `Note:` line explaining the local-only base
/// (`fetch_origin`'s `NoOriginRef` warning, surfaced by `dispatch_with_prompt`),
/// but reading `.claude-prompt` back out is racy: the launched shell consumes
/// and deletes it, and there is no deterministic signal to wait on without a
/// wall-clock sleep (banned by `check-no-test-sleep.sh`). So this test asserts
/// only the git-level facts; the prompt text itself is unit-tested elsewhere
/// (`src/dispatch/prompts.rs`'s `Note: origin has no branch` assertions).
#[test]
fn fresh_dispatch_falls_back_to_local_base_with_no_origin_remote() {
    let Some(fx) = setup_no_origin_or_skip() else {
        return;
    };

    let local_main = git_stdout(&fx.repo, &["rev-parse", "main"]);

    let result = fx.dispatch(4244);

    let branch = git_stdout(&fx.repo, &["rev-parse", "4244-some-task"]);
    assert_eq!(
        branch, local_main,
        "with no origin remote at all, the worktree must fall back to local main"
    );
    assert!(std::path::Path::new(&result.worktree_path).exists());
}
