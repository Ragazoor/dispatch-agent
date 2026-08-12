use anyhow::{Context, Result};
use std::fs;

use crate::git::detect_default_branch;
use crate::models::{expand_tilde, DispatchResult, ResumeResult, Task, TaskId, TaskStatus};
use crate::process::{ProcessRunner, SUBPROCESS_TIMEOUT};
use crate::tmux;

use super::prompts::{
    build_and_record_injections, build_prompt, build_quick_dispatch_prompt, build_research_prompt,
    build_tmux_window_name, compose_prompt_head, parse_tmux_window_task_id, select_preamble,
    EpicContext, LearningInjections, PromptContext, DISPATCH_PLUGIN_DIR,
};
use super::worktree::{provision_worktree, BaseRef, StartPoint};

/// Width of the `dispatch agent-tree` companion pane, as a percentage of the
/// agent window — narrower than [`tmux::split_window_horizontal`]'s 40%
/// (the board's own split-pane feature), since the agent's own `claude` CLI
/// output needs the room. Matches `agent_tree_pane_percent` in
/// docs/specs/agent-tree.allium.
const AGENT_TREE_PANE_PERCENT: u8 = 30;

/// The `dispatch` subcommand the companion pane runs. Shared between the spawn
/// side and the lookup side, so the two cannot drift.
const AGENT_TREE_SUBCOMMAND: &str = "agent-tree";

/// Whether `start_command` is a `dispatch agent-tree <id>` invocation — the
/// companion pane [`spawn_agent_tree_pane`] creates.
///
/// Matched as argv0's basename plus argv1, never as a substring: an editor pane
/// opened on `docs/specs/agent-tree.allium` contains the string "agent-tree", and
/// killing the user's editor instead of the tree would be exactly the class of
/// bug this lookup exists to fix. argv0 may be an absolute path because the
/// binary is named through `ProcessRunner::agent_binaries` — which is how the
/// real-tmux harness points it at a stub — hence comparing basenames.
fn is_agent_tree_command(start_command: &str, dispatch_bin: &str) -> bool {
    // Borrowed, not allocated: this runs once per pane per toggle, and the
    // lossy conversion the owned form implied was never semantically needed.
    let basename = |s: &str| {
        std::path::Path::new(s)
            .file_name()
            .unwrap_or_else(|| std::ffi::OsStr::new(s))
            .to_owned()
    };
    let mut argv = start_command.split_whitespace();
    let Some(argv0) = argv.next() else {
        return false;
    };
    basename(argv0) == basename(dispatch_bin) && argv.next() == Some(AGENT_TREE_SUBCOMMAND)
}

/// The companion agent-tree pane in `window`, if it has one.
///
/// Replaces a `tmux::inactive_pane_id` call, which asked "which pane is not
/// focused?" and answered "the companion" only for a two-pane window whose focus
/// had not moved. Neither holds: the user can focus the companion pane — and must,
/// to press any key in it — and an editor pane opened from the tree makes a third.
/// Identifying the pane by the command it was started with is true whatever has
/// focus and however many panes there are, and it needs no migration, since tmux
/// reports the start command of panes already running.
pub fn agent_tree_pane_id(window: &str, runner: &dyn ProcessRunner) -> Result<Option<String>> {
    let dispatch_bin = runner.agent_binaries().dispatch;
    let panes = tmux::pane_ids_with_start_command(
        window,
        |cmd| is_agent_tree_command(cmd, &dispatch_bin),
        runner,
    )?;
    Ok(panes.into_iter().next())
}

/// Every pane in `window` that dispatch put there beside the agent's own: the
/// companion tree pane, and the editor pane opened from it.
///
/// Used by the split-pane pin path, which moves only the agent's own pane out and
/// must not leave the rest behind in a window nothing owns.
pub fn companion_pane_ids(window: &str, runner: &dyn ProcessRunner) -> Result<Vec<String>> {
    // Resolve the window *once*: each lookup would otherwise re-resolve the name,
    // and a name (unlike a `%N` id) costs a real all-sessions `list-panes` scan
    // — two extra tmux processes per pin for an answer that cannot change here.
    let pane = tmux::pane_id_for_window(window, runner)?;
    let mut panes = Vec::new();
    if let Some(tree) = agent_tree_pane_id(&pane, runner)? {
        panes.push(tree);
    }
    panes.extend(tmux::pane_ids_with_option(
        &pane,
        tmux::EDITOR_PANE_OPTION,
        runner,
    )?);
    Ok(panes)
}

/// Split the agent's tmux window and launch `dispatch agent-tree <task_id>`
/// in the new pane, right after the agent's own command has been sent (see
/// docs/specs/agent-tree.allium's `SplitAgentTreePaneOnAgentLaunch`).
///
/// Best-effort: the companion pane is a decorative side view, not the
/// critical path — a failure here is logged and does not fail the caller's
/// dispatch/resume operation.
fn spawn_agent_tree_pane(tmux_window: &str, task_id: TaskId, runner: &dyn ProcessRunner) {
    let id_arg = task_id.0.to_string();
    // Passed as argv (`split-window --`), so the binary needs no shell quoting.
    let dispatch_bin = runner.agent_binaries().dispatch;
    if let Err(e) = tmux::split_window_horizontal_running(
        tmux_window,
        AGENT_TREE_PANE_PERCENT,
        &[&dispatch_bin, AGENT_TREE_SUBCOMMAND, &id_arg],
        runner,
    ) {
        tracing::warn!(
            task_id = task_id.0,
            %tmux_window,
            error = %e,
            "failed to open agent-tree companion pane"
        );
    }
}

/// Toggle the companion agent-tree pane in the given tmux window: kill it if
/// currently shown, re-split and relaunch `dispatch agent-tree <task_id>` if
/// hidden. `window` is a tmux window name (e.g. "task-42"), resolved by
/// tmux's own `#{window_name}` expansion at the moment the global toggle key
/// was pressed — see docs/specs/agent-tree.allium's ToggleAgentTreePane
/// rules.
///
/// A no-op (not an error) for any window that isn't a task-agent window —
/// pressing the toggle key in the board TUI's own window does nothing.
pub fn toggle_agent_tree_pane(window: &str, runner: &dyn ProcessRunner) -> Result<()> {
    let Some(task_id) = parse_tmux_window_task_id(window) else {
        return Ok(());
    };
    match agent_tree_pane_id(window, runner)? {
        Some(pane_id) => tmux::kill_pane(&pane_id, runner),
        None => {
            spawn_agent_tree_pane(window, task_id, runner);
            Ok(())
        }
    }
}

/// Resync a tmux window's companion pane to the task its own name implies:
/// kill whatever is currently running there and relaunch
/// `dispatch agent-tree <task_id>` for the correct task.
///
/// Used after `swap-pane` rewrites a window's task identity (via rename)
/// without touching its companion pane — left alone, that pane would keep
/// rendering the previous occupant's file tree under the window's new name
/// (see docs/specs/agent-tree.allium's ToggleVsSplitPaneInteraction).
///
/// A no-op for any window that isn't a task-agent window, or that carries no
/// companion pane to begin with — best-effort throughout, like every other
/// agent-tree pane operation.
pub fn resync_agent_tree_pane(window: &str, runner: &dyn ProcessRunner) {
    let Some(task_id) = parse_tmux_window_task_id(window) else {
        return;
    };
    match agent_tree_pane_id(window, runner) {
        Ok(Some(pane_id)) => {
            if let Err(e) = tmux::kill_pane(&pane_id, runner) {
                tracing::warn!(
                    %window,
                    error = %e,
                    "failed to kill stale agent-tree companion pane before resync"
                );
            }
            spawn_agent_tree_pane(window, task_id, runner);
        }
        Ok(None) => {}
        Err(e) => {
            tracing::warn!(
                %window,
                error = %e,
                "failed to check for companion pane before resync"
            );
        }
    }
}

/// Provision worktree, build the prompt, write the prompt file, launch Claude
/// via tmux.
///
/// The `make_prompt` closure builds the full prompt string for the agent.
/// Splitting the build step into a closure lets each agent variant compose its
/// own context (learnings, plan, etc.) while keeping the post-provision launch
/// logic in one place.
///
/// `permission_mode` controls Claude's `--permission-mode` flag:
/// `None` launches in Claude's default (auto) mode, used by every task
/// agent except research. `Some("plan")` is used by the research agent so
/// investigation stays read-only.
fn dispatch_with_prompt(
    task: &Task,
    make_prompt: impl FnOnce() -> String,
    runner: &dyn ProcessRunner,
    base_branch: Option<&str>,
    permission_mode: Option<&str>,
) -> Result<DispatchResult> {
    if task.repo_path.is_empty() {
        anyhow::bail!(
            "Repository path is not set. Edit the task (press 'e') to set it before dispatching."
        );
    }
    let repo_path = expand_tilde(&task.repo_path);

    // Resolve the repo's base branch (task base_branch or detected default).
    let resolved: String = base_branch
        .map(str::to_owned)
        .unwrap_or_else(|| detect_default_branch(&repo_path, runner));

    // Review tasks (pr-review / dependabot / renovate) that carry a PR URL base
    // their worktree on the PR's head branch so the agent sees the PR's code.
    // Soft-fall to the base branch on resolution failure or for fork PRs.
    let pr_branch: Option<String> = match (&task.tag, &task.url) {
        (Some(tag), Some(url)) if tag.is_review() && url.is_pr() => {
            super::pr_head_branch(&url.url, runner)
        }
        _ => None,
    };

    // A PR head branch is never compared against a local ref — a review must
    // see exactly the PR's code — so the two cases pick different `BaseRef`
    // variants. Borrowing throughout: `pr_branch` is still needed for
    // `select_preamble` after provisioning, and `resolved` is unused hereafter.
    let base_ref = match pr_branch.as_deref() {
        Some(branch) => BaseRef::PrHead(branch),
        None => BaseRef::Branch(&resolved),
    };

    let provision = provision_worktree(task, runner, Some(base_ref), SUBPROCESS_TIMEOUT)?;

    let preamble = select_preamble(
        pr_branch.as_deref(),
        provision.start_point.as_ref(),
        provision.reused_worktree,
    );
    let head = compose_prompt_head(&preamble, provision.fetch_warning.as_deref());

    let prompt = make_prompt();
    let full_prompt = format!(
        "{head}Always work from this worktree folder — do not `cd` to the parent repo \
         or other directories.\n\n\
         {prompt}"
    );
    let prompt_file = format!("{}/.claude-prompt", provision.worktree_path);
    fs::write(&prompt_file, &full_prompt)
        .with_context(|| format!("failed to write {prompt_file}"))?;
    let permission_flag = match permission_mode {
        Some(mode) => format!(" --permission-mode {mode}"),
        None => String::new(),
    };
    // The binary goes *after* the script as bash's `$0`, not inside it. Inside
    // the single-quoted body it would sit under two quoting layers (the pane's
    // shell strips the outer quotes, then bash parses what's left), and a path
    // with a space would need escaping twice to survive both. As `$0` it is one
    // ordinary shell word, quoted once like every other launch site.
    let claude = runner.agent_binaries().claude_quoted();
    let claude_cmd = format!(
        "bash -c 'prompt=$(cat .claude-prompt) && rm -f .claude-prompt \
         && \"$0\" {DISPATCH_PLUGIN_DIR}{permission_flag} \"$prompt\"' {claude}"
    );
    tmux::send_keys(&provision.tmux_window, &claude_cmd, runner)
        .context("failed to send keys to tmux window")?;

    spawn_agent_tree_pane(&provision.tmux_window, task.id, runner);

    tracing::info!(
        task_id = task.id.0,
        worktree = %provision.worktree_path,
        base = provision.start_point.as_ref().map(StartPoint::base),
        reused_worktree = provision.reused_worktree,
        "agent dispatched"
    );

    Ok(DispatchResult {
        worktree_path: provision.worktree_path,
        tmux_window: provision.tmux_window,
    })
}

pub fn dispatch_agent(
    task: &Task,
    runner: &dyn ProcessRunner,
    epic: Option<&EpicContext>,
    injections: &LearningInjections<'_>,
    verify_command: Option<&str>,
) -> Result<DispatchResult> {
    dispatch_with_prompt(
        task,
        || {
            let mut ctx =
                PromptContext::with_learnings(injections.clone()).with_verify(verify_command);
            ctx.tag = task.tag;
            ctx.auto_run_plan = task.auto_run_plan;
            build_prompt(
                task.id,
                &task.title,
                &task.description,
                task.plan_path.as_deref(),
                epic,
                &ctx,
            )
        },
        runner,
        Some(&task.base_branch),
        None,
    )
}

pub fn research_agent(
    task: &Task,
    runner: &dyn ProcessRunner,
    epic: Option<&EpicContext>,
    verify_command: Option<&str>,
) -> Result<DispatchResult> {
    dispatch_with_prompt(
        task,
        || {
            let ctx = PromptContext::default().with_verify(verify_command);
            build_research_prompt(task.id, &task.title, &task.description, epic, &ctx)
        },
        runner,
        Some(&task.base_branch),
        Some("plan"),
    )
}

pub fn quick_dispatch_agent(
    task: &Task,
    runner: &dyn ProcessRunner,
    epic: Option<&EpicContext>,
    injections: &LearningInjections<'_>,
    verify_command: Option<&str>,
) -> Result<DispatchResult> {
    dispatch_with_prompt(
        task,
        || {
            let ctx = PromptContext::with_learnings(injections.clone()).with_verify(verify_command);
            build_quick_dispatch_prompt(task.id, &task.title, &task.description, epic, &ctx)
        },
        runner,
        Some(&task.base_branch),
        None,
    )
}

/// The three per-task reads every dispatch entry point performs before it can
/// build a prompt: the epic banner, the learnings to inject (recording each
/// retrieval as it goes), and the repo's verify command.
///
/// Grouped because all four launch sites — `dispatch_task` and the epic chain in
/// `src/mcp/handlers/tasks/dispatch.rs`, and `exec_quick_dispatch` /
/// `exec_dispatch_agent` in `src/runtime/tasks.rs` — ran the identical
/// three-step prologue inline, so a change to it meant editing four places.
pub struct DispatchInputs {
    pub epic_ctx: Option<EpicContext>,
    pub injected: Vec<crate::models::Learning>,
    pub verify_command: Option<String>,
}

/// Run the dispatch prologue for `task`, reading its epic context from the DB.
///
/// Async and side-effecting (it records prompt-injection retrievals), so callers
/// run it before handing the task to the blocking dispatch itself.
pub async fn prepare_inputs(
    db: &dyn crate::db::TaskReadStore,
    task: &Task,
    emb_svc: &std::sync::Arc<crate::service::embeddings::EmbeddingService>,
) -> DispatchInputs {
    let epic_ctx = EpicContext::from_db(task, db).await;
    prepare_inputs_with_epic_ctx(db, task, emb_svc, epic_ctx).await
}

/// [`prepare_inputs`] for a caller that already holds the epic row — the epic
/// chain, which reads the epic to check `auto_dispatch` and would otherwise
/// re-read it here.
pub async fn prepare_inputs_with_epic_ctx(
    db: &dyn crate::db::TaskReadStore,
    task: &Task,
    emb_svc: &std::sync::Arc<crate::service::embeddings::EmbeddingService>,
    epic_ctx: Option<EpicContext>,
) -> DispatchInputs {
    let injected = build_and_record_injections(db, task, emb_svc).await;
    let verify_command = fetch_verify_command(db, &task.repo_path).await;
    DispatchInputs {
        epic_ctx,
        injected,
        verify_command,
    }
}

/// Fetch the verify command for a repository path from the settings store.
///
/// Logs a warning and returns `None` if the DB lookup fails so callers can
/// proceed without a verify command rather than aborting dispatch.
pub async fn fetch_verify_command(
    db: &dyn crate::db::TaskReadStore,
    repo_path: &str,
) -> Option<String> {
    db.get_verify_command(repo_path).await.unwrap_or_else(|e| {
        tracing::warn!(error = %e, "failed to load verify_command; proceeding without it");
        None
    })
}

/// Re-open a tmux window for an existing worktree and resume the most recent
/// Claude conversation with `claude --continue`.
///
/// This function is **synchronous** and should be called via
/// `tokio::task::spawn_blocking` from async contexts.
pub fn resume_agent(
    task_id: TaskId,
    worktree_path: &str,
    runner: &dyn ProcessRunner,
) -> Result<ResumeResult> {
    let tmux_window = build_tmux_window_name(task_id);

    tmux::new_window(&tmux_window, worktree_path, runner)
        .context("failed to create tmux window for resume")?;

    tmux::set_window_dispatch_dir(&tmux_window, worktree_path, runner)
        .context("failed to set tmux window dispatch dir")?;
    tmux::ensure_split_hook(runner).context("failed to ensure tmux split hook")?;

    let claude = runner.agent_binaries().claude_quoted();
    tmux::send_keys(
        &tmux_window,
        &format!("{claude} {DISPATCH_PLUGIN_DIR} --continue"),
        runner,
    )
    .context("failed to send resume keys to tmux window")?;

    spawn_agent_tree_pane(&tmux_window, task_id, runner);

    tracing::info!(task_id = task_id.0, %tmux_window, "agent resumed");

    Ok(ResumeResult { tmux_window })
}

/// A task can be wrapped up if it has a worktree and is either Running or Review.
pub fn is_wrappable(task: &Task) -> bool {
    task.worktree.is_some()
        && (task.status == TaskStatus::Review || task.status == TaskStatus::Running)
}

/// The fixed tmux window name used for the main claude session.
pub const MAIN_SESSION_WINDOW: &str = "dispatch-main";

/// Whether the fixed main-session window is currently alive: a live tmux check
/// on [`MAIN_SESSION_WINDOW`], never a persisted reference. A tmux query error
/// maps to "alive" (see `tmux::has_window_or_assume_present`) rather than
/// "not alive", so a transient tmux hiccup never presents as the main session
/// having disappeared. Shared by the `:` open path and the status-bar
/// liveness poll so both agree on one definition. See docs/specs/dispatch.allium:
/// MainSessionIndicator / OpenMainSession.
pub fn main_session_window_alive(runner: &dyn ProcessRunner) -> bool {
    tmux::has_window_or_assume_present(MAIN_SESSION_WINDOW, runner)
}

/// Launch a plain interactive `claude` session in a new tmux window.
///
/// Unlike task agents, this session has no task context, no prompt file, and
/// no `--permission-mode` flag — it opens as a plain interactive Claude Code
/// session with dispatch plugins available.
///
/// Returns the name of the created tmux window.
pub fn create_main_session(dir: &str, runner: &dyn ProcessRunner) -> Result<String> {
    let window = MAIN_SESSION_WINDOW;

    tmux::new_window(window, dir, runner).context("failed to create main session tmux window")?;

    let claude = runner.agent_binaries().claude_quoted();
    tmux::send_keys(window, &format!("{claude} {DISPATCH_PLUGIN_DIR}"), runner)
        .context("failed to send keys to main session tmux window")?;

    tracing::info!(%window, %dir, "main session created");

    Ok(window.to_string())
}

#[cfg(test)]
mod tests {
    #![allow(clippy::unwrap_used, clippy::expect_used)]
    use super::*;
    use crate::process::{AgentBinaries, MockProcessRunner};

    #[test]
    fn resync_agent_tree_pane_noop_for_non_task_window() {
        let mock = MockProcessRunner::new(vec![]);
        resync_agent_tree_pane("dispatch-main", &mock);
        assert!(
            mock.recorded_calls().is_empty(),
            "no tmux calls expected for a non-task window name"
        );
    }

    #[test]
    fn resync_agent_tree_pane_noop_when_no_companion_pane() {
        let mock = MockProcessRunner::new(vec![
            // list-panes: one pane, running a plain shell — no companion
            MockProcessRunner::ok_with_stdout(b"%1 \n"),
        ]);
        resync_agent_tree_pane("task-5", &mock);
        let calls = mock.recorded_calls();
        assert_eq!(calls.len(), 1, "only the companion check should run");
    }

    /// The companion pane's binary comes from the runner, not a literal — the
    /// seam that lets the real-tmux harness point it at a stub without shadowing
    /// `dispatch` on `PATH`. The listing below carries that same stub path, so
    /// this also covers the lookup matching argv0 by basename rather than
    /// requiring the bare name.
    #[test]
    fn spawn_agent_tree_pane_launches_the_runners_dispatch_binary() {
        let mock = MockProcessRunner::new(vec![
            // list-panes: the companion is %9, started with the stub binary
            MockProcessRunner::ok_with_stdout(b"%1 \n%9 /stub/bin/dispatch-stub agent-tree 5\n"),
            MockProcessRunner::ok(),                     // kill-pane
            MockProcessRunner::ok_with_stdout(b"%20\n"), // split-window
        ])
        .with_agent_binaries(AgentBinaries::stub());

        resync_agent_tree_pane("task-5", &mock);

        let split = &mock.recorded_calls()[2].1;
        assert!(
            split.contains(&"/stub/bin/dispatch-stub".to_string()),
            "companion pane must exec the runner's dispatch binary, got: {split:?}"
        );
    }

    #[test]
    fn resync_agent_tree_pane_kills_and_respawns_companion() {
        let mock = MockProcessRunner::new(vec![
            // list-panes: the companion is %9
            MockProcessRunner::ok_with_stdout(b"%1 \n%9 dispatch agent-tree 5\n"),
            MockProcessRunner::ok(),                     // kill-pane %9
            MockProcessRunner::ok_with_stdout(b"%20\n"), // split-window relaunch
        ]);
        resync_agent_tree_pane("task-5", &mock);
        let calls = mock.recorded_calls();
        assert_eq!(calls.len(), 3);
        assert_eq!(calls[1].1, vec!["kill-pane", "-t", "%9"]);
        assert!(calls[2].1.contains(&"split-window".to_string()));
        // The window being split is targeted by its resolved pane ID, not its
        // name — otherwise the companion pane could open inside a
        // prefix-matched sibling's window (see `tmux::window_target`).
        assert!(calls[2].1.contains(&mock.pane_id_of("task-5")));
        assert!(calls[2].1.contains(&"5".to_string()));
        assert!(calls[2].1.contains(&"agent-tree".to_string()));
    }

    #[test]
    fn resync_agent_tree_pane_soft_fails_when_companion_check_errors() {
        let mock = MockProcessRunner::new(vec![MockProcessRunner::fail("list-panes error")]);
        resync_agent_tree_pane("task-5", &mock);
        let calls = mock.recorded_calls();
        assert_eq!(calls.len(), 1, "no further calls after a failed check");
    }

    #[test]
    fn resync_agent_tree_pane_still_respawns_when_kill_fails() {
        let mock = MockProcessRunner::new(vec![
            MockProcessRunner::ok_with_stdout(b"%1 \n%9 dispatch agent-tree 5\n"),
            MockProcessRunner::fail("kill-pane error"),
            MockProcessRunner::ok_with_stdout(b"%20\n"),
        ]);
        resync_agent_tree_pane("task-5", &mock);
        let calls = mock.recorded_calls();
        assert_eq!(calls.len(), 3, "a respawn is still attempted");
        assert!(calls[2].1.contains(&"split-window".to_string()));
    }

    #[test]
    fn main_session_window_alive_delegates_to_has_window_or_assume_present() {
        // main_session_window_alive is a pure delegation to
        // tmux::has_window_or_assume_present — the present/absent/query-failed
        // branch logic itself is fully covered by that function's own tests
        // in src/tmux.rs. This just confirms the query-failure default
        // (assume alive) carries through the wrapper, since a false "gone"
        // here would send the user to the reconfigure flow over a hiccup.
        let mock = MockProcessRunner::new(vec![Err(anyhow::anyhow!("tmux: command not found"))]);
        assert!(main_session_window_alive(&mock));
    }

    #[test]
    fn create_main_session_creates_tmux_window_in_given_dir() {
        let mock = MockProcessRunner::new(vec![
            MockProcessRunner::ok(), // new-window
            MockProcessRunner::ok(), // send-keys -l
            MockProcessRunner::ok(), // send-keys Enter
        ]);
        let result = create_main_session("/home/user", &mock);
        assert!(result.is_ok());
        let window = result.unwrap();
        assert_eq!(window, MAIN_SESSION_WINDOW);

        let calls = mock.recorded_calls();
        // First call: tmux new-window
        assert!(calls[0].1.contains(&"new-window".to_string()));
        assert!(calls[0].1.iter().any(|a| a.contains("/home/user")));
        assert!(calls[0].1.iter().any(|a| a == MAIN_SESSION_WINDOW));
    }

    #[test]
    fn create_main_session_sends_claude_with_plugin_dir() {
        let mock = MockProcessRunner::new(vec![
            MockProcessRunner::ok(), // new-window
            MockProcessRunner::ok(), // send-keys -l
            MockProcessRunner::ok(), // send-keys Enter
        ]);
        create_main_session("/home/user", &mock).unwrap();

        let calls = mock.recorded_calls();
        // send-keys call passes "claude <plugin_dir>" as the command
        let all_args: Vec<String> = calls.iter().flat_map(|(_, args)| args.clone()).collect();
        let has_plugin_dir = all_args
            .iter()
            .any(|a| a.contains("claude") && a.contains("--plugin-dir"));
        assert!(
            has_plugin_dir,
            "expected claude with plugin dir in send-keys, got: {all_args:?}"
        );
    }

    #[test]
    fn create_main_session_launches_the_runners_claude_binary() {
        let mock = MockProcessRunner::new(vec![
            MockProcessRunner::ok(), // new-window
            MockProcessRunner::ok(), // send-keys -l
            MockProcessRunner::ok(), // send-keys Enter
        ])
        .with_agent_binaries(AgentBinaries::stub());

        create_main_session("/home/user", &mock).unwrap();

        let calls = mock.recorded_calls();
        let sent = calls[1]
            .1
            .iter()
            .find(|a| a.contains("claude"))
            .expect("send-keys payload naming claude");
        assert!(
            sent.starts_with("/stub/bin/claude-stub --plugin-dir"),
            "main session must launch the runner's claude binary, got: {sent}"
        );
    }
}
