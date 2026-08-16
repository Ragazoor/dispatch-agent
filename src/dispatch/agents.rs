use anyhow::{Context, Result};
use std::fs;

use crate::git::detect_default_branch;
use crate::models::{
    build_tmux_window_name, expand_tilde, parse_tmux_window_task_id, DispatchResult, ResumeResult,
    Task, TaskId,
};
use crate::process::{ProcessRunner, SUBPROCESS_TIMEOUT};
use crate::tmux;

use super::prompts::{
    build_and_record_injections, build_pipeline_prompt, build_prompt, build_quick_dispatch_prompt,
    build_research_prompt, compose_prompt_head, select_preamble, EpicContext, LearningInjections,
    PromptContext, DISPATCH_PLUGIN_DIR,
};
use super::worktree::{provision_worktree, rollback_failed_provisioning, BaseRef, StartPoint};

/// The `--name` flag Claude Code's native cross-session messaging addresses
/// this task's agent by (`ListAgents`/`SendMessage`/`@mention`), e.g.
/// `--name task-42`. Deterministic and collision-free within dispatch's own
/// id space, unlike Claude Code's default cwd-derived name — see
/// `docs/superpowers/specs/2026-08-15-send-message-native-relay-design.md`.
/// Built from [`build_tmux_window_name`] rather than re-deriving the string,
/// so the tmux window name and the messaging session name cannot drift apart
/// — `parse_peer_message_target_name` (`src/service/tasks/crud.rs`) resolves
/// an incoming `SendMessage`'s `to` field against this same convention.
/// Leading space included so callers can splice it directly after
/// [`DISPATCH_PLUGIN_DIR`] in a launch command string.
fn session_name_flag(task_id: TaskId) -> String {
    format!(" --name {}", build_tmux_window_name(task_id))
}

/// Width of the `dispatch agent-tree` companion pane, as a percentage of the
/// agent window — narrower than [`tmux::split_window_horizontal`]'s 40%
/// (the board's own split-pane feature), since the agent's own `claude` CLI
/// output needs the room. Matches `agent_tree_pane_percent` in
/// docs/specs/agent-tree.allium.
const AGENT_TREE_PANE_PERCENT: u8 = 30;

/// The `dispatch` subcommand the companion pane runs. Spawn-side only: the pane
/// is recognised afterwards by the role marker written on it
/// ([`tmux::PANE_ROLE_OPTION`]), not by what it is running.
const AGENT_TREE_SUBCOMMAND: &str = "agent-tree";

/// The companion agent-tree pane in `window`, if it has one.
///
/// Identified by the role marker [`spawn_agent_tree_pane`] writes on the pane,
/// matched on its exact value: true whatever has focus, however many panes the
/// window has, and whatever any of them happen to be running. Both heuristics
/// this replaced failed on one of those — `tmux::inactive_pane_id` asked "which
/// pane is not focused?" (the user must focus the companion pane to press a key
/// in it, and an editor pane makes a third), and matching `#{pane_start_command}`
/// re-derived identity from a command line, which had to be defended against a
/// pane merely *showing* a file named after this feature. See
/// docs/specs/agent-tree.allium's `HideAgentTreePane`, including the accepted
/// cost: a marker cannot be written to a pane that already exists, so a window
/// open when this changed has an unmarked companion pane for the rest of its life.
pub fn agent_tree_pane_id(window: &str, runner: &dyn ProcessRunner) -> Result<Option<String>> {
    let panes = tmux::pane_ids_with_option_value(
        window,
        tmux::PANE_ROLE_OPTION,
        tmux::PANE_ROLE_AGENT_TREE,
        runner,
    )?;
    Ok(panes.into_iter().next())
}

/// Every pane in `window` that dispatch put there beside the agent's own: the
/// companion tree pane, and the editor pane opened from it.
///
/// Used by the split-pane pin path, which moves only the agent's own pane out and
/// must not leave the rest behind in a window nothing owns.
///
/// One tmux call, because that is the question being asked: not "the tree pane,
/// and the editor pane, and…" — which cost a lookup per role and a window
/// pre-resolution to keep them from each re-resolving the name — but "every pane
/// carrying a role at all". A future dispatch-created pane is covered the moment
/// it is marked, without this function being taught about it.
pub fn companion_pane_ids(window: &str, runner: &dyn ProcessRunner) -> Result<Vec<String>> {
    tmux::pane_ids_with_option(window, tmux::PANE_ROLE_OPTION, runner)
}

/// Split the agent's tmux window and launch `dispatch agent-tree <task_id>`
/// in the new pane, right after the agent's own command has been sent (see
/// docs/specs/agent-tree.allium's `SplitAgentTreePaneOnAgentLaunch`).
///
/// `worktree` becomes the new pane's start directory. Naming it keeps the pane
/// out of the `after-split-window` correction hook, which would otherwise
/// respawn it and restart the `dispatch agent-tree` process this call just
/// launched — see `tmux::ensure_split_hook`. `None` is a soft fallback for a
/// window whose worktree could not be resolved: the hook then corrects the pane,
/// which costs one restart of a process that has barely started.
///
/// Best-effort: the companion pane is a decorative side view, not the
/// critical path — a failure here is logged and does not fail the caller's
/// dispatch/resume operation.
fn spawn_agent_tree_pane(
    tmux_window: &str,
    task_id: TaskId,
    worktree: Option<&str>,
    runner: &dyn ProcessRunner,
) {
    let id_arg = task_id.0.to_string();
    // Passed as argv (`split-window --`), so the binary needs no shell quoting.
    let dispatch_bin = runner.agent_binaries().dispatch;
    let pane = match tmux::split_window_horizontal_running(
        tmux_window,
        AGENT_TREE_PANE_PERCENT,
        &[&dispatch_bin, AGENT_TREE_SUBCOMMAND, &id_arg],
        worktree,
        runner,
    ) {
        Ok(pane) => pane,
        Err(e) => {
            tracing::warn!(
                task_id = task_id.0,
                %tmux_window,
                error = %e,
                "failed to open agent-tree companion pane"
            );
            return;
        }
    };
    // Mark it for the lookups that come later ([`agent_tree_pane_id`],
    // [`companion_pane_ids`]). Best-effort like the split itself: the pane is open
    // and rendering either way, and only the *next* toggle suffers — it would read
    // the window as having no companion and split a second one. Same accepted gap
    // as the editor pane's own marker (docs/specs/agent-tree.allium:
    // `OneEditorPanePerAgentWindow`).
    if let Err(e) = tmux::set_pane_option(
        &pane,
        tmux::PANE_ROLE_OPTION,
        tmux::PANE_ROLE_AGENT_TREE,
        runner,
    ) {
        tracing::warn!(
            task_id = task_id.0,
            %pane,
            error = %e,
            "failed to mark the agent-tree companion pane with its role"
        );
    }
}

/// The worktree a tmux window belongs to, read back from the `@dispatch_dir`
/// option dispatch set when it created the window.
///
/// The toggle and resync paths are handed a window *name* and nothing else, so
/// this is how they recover a start directory for the split. Best-effort by
/// design: a failure here is logged and yields `None`, which
/// [`spawn_agent_tree_pane`] treats as "let the correction hook handle it"
/// rather than as a reason to skip the pane.
fn window_worktree(window: &str, runner: &dyn ProcessRunner) -> Option<String> {
    match tmux::window_dispatch_dir(window, runner) {
        Ok(dir) => dir,
        Err(e) => {
            tracing::warn!(
                %window,
                error = %e,
                "failed to read @dispatch_dir; opening companion pane without a start directory"
            );
            None
        }
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
            let worktree = window_worktree(window, runner);
            spawn_agent_tree_pane(window, task_id, worktree.as_deref(), runner);
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
            let worktree = window_worktree(window, runner);
            spawn_agent_tree_pane(window, task_id, worktree.as_deref(), runner);
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
    //
    // Skipped entirely for a pinned task: `pr_head_branch` shells out to `gh`,
    // and the answer could not be used anyway — pinning wins the match below.
    let pr_branch: Option<String> = match (&task.pinned_branch, &task.tag, &task.url) {
        (None, Some(tag), Some(url)) if tag.is_review() && url.is_pr() => {
            super::pr_head_branch(&url.url, runner)
        }
        _ => None,
    };

    // Neither a PR head nor a pinned branch is compared against a local ref —
    // a review must see exactly the PR's code, and a pinned branch is shared —
    // so each picks its own `BaseRef` variant. `pinned_branch` takes priority:
    // the two are never both set in practice, but pinning is the more specific
    // configuration. Borrowing throughout: `pr_branch` is still needed for
    // `select_preamble` after provisioning, and `resolved` is unused hereafter.
    let base_ref = match (task.pinned_branch.as_deref(), pr_branch.as_deref()) {
        (Some(pinned), _) => BaseRef::Pinned(pinned),
        (None, Some(branch)) => BaseRef::PrHead(branch),
        (None, None) => BaseRef::Branch(&resolved),
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
    let name_flag = session_name_flag(task.id);
    let claude_cmd = format!(
        "bash -c 'prompt=$(cat .claude-prompt) && rm -f .claude-prompt \
         && \"$0\" {DISPATCH_PLUGIN_DIR}{name_flag}{permission_flag} \"$prompt\"' {claude}"
    );

    // Anything failing here happens after the worktree and tmux window both
    // exist — a fresh worktree (this attempt's own `git worktree add`) and the
    // window this same provisioning call just opened must both be rolled back
    // so a re-dispatch of the same task takes the fresh path again, not the
    // reuse path with its weaker fetch guarantee. A reused worktree predates
    // this attempt and is never touched — see rollback_failed_provisioning.
    let launch: Result<()> = (|| {
        fs::write(&prompt_file, &full_prompt)
            .with_context(|| format!("failed to write {prompt_file}"))?;
        tmux::send_keys(&provision.tmux_window, &claude_cmd, runner)
            .context("failed to send keys to tmux window")?;
        Ok(())
    })();
    if let Err(e) = launch {
        rollback_failed_provisioning(
            &repo_path,
            &provision.worktree_path,
            &provision.tmux_window,
            provision.reused_worktree,
            runner,
        );
        return Err(e);
    }

    spawn_agent_tree_pane(
        &provision.tmux_window,
        task.id,
        Some(provision.worktree_path.as_str()),
        runner,
    );

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
) -> Result<DispatchResult> {
    dispatch_with_prompt(
        task,
        || {
            let ctx = PromptContext {
                learnings: injections.clone(),
                tag: task.tag,
                auto_run_plan: task.auto_run_plan,
            };
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
) -> Result<DispatchResult> {
    dispatch_with_prompt(
        task,
        || {
            build_research_prompt(
                task.id,
                &task.title,
                &task.description,
                epic,
                &PromptContext::default(),
            )
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
) -> Result<DispatchResult> {
    dispatch_with_prompt(
        task,
        || {
            let ctx = PromptContext {
                learnings: injections.clone(),
                ..PromptContext::default()
            };
            build_quick_dispatch_prompt(task.id, &task.title, &task.description, epic, &ctx)
        },
        runner,
        Some(&task.base_branch),
        None,
    )
}

/// A **fresh** dispatch for a scheduled tick, used by
/// [`crate::scheduler::SchedulerRunner`].
///
/// Deliberately not `resume_agent`: that does `claude --continue`, resuming a
/// conversation from the previous tick. A scheduled tick is a new unit of work
/// on new commits, so it needs a new prompt every time — built the same way as
/// [`research_agent`] and [`quick_dispatch_agent`], through the shared
/// [`dispatch_with_prompt`].
///
/// The pinned branch, when set, reaches `provision_worktree` through
/// `dispatch_with_prompt`'s base-ref match, so no argument is threaded here.
pub fn pipeline_agent(task: &Task, runner: &dyn ProcessRunner) -> Result<DispatchResult> {
    dispatch_with_prompt(
        task,
        || {
            build_pipeline_prompt(
                task.id,
                &task.title,
                task.pinned_branch.as_deref(),
                &task.base_branch,
            )
        },
        runner,
        Some(&task.base_branch),
        None,
    )
}

/// Launch the agent [`DispatchMode`](crate::models::DispatchMode) selects,
/// consuming the prologue's [`DispatchInputs`].
///
/// The one place that match lives. It used to be written out once in the MCP
/// handler and once in the TUI runtime's blocking closure, which is how the two
/// could have drifted into launching different agents for the same mode.
pub fn run_agent_for_mode(
    task: &Task,
    mode: crate::models::DispatchMode,
    runner: &dyn ProcessRunner,
    inputs: DispatchInputs,
) -> Result<DispatchResult> {
    let DispatchInputs { epic_ctx, injected } = inputs;
    let injections = LearningInjections::from(injected.as_slice());
    match mode {
        crate::models::DispatchMode::Dispatch => {
            dispatch_agent(task, runner, epic_ctx.as_ref(), &injections)
        }
        crate::models::DispatchMode::Research => research_agent(task, runner, epic_ctx.as_ref()),
    }
}

/// The two per-task reads every dispatch entry point performs before it can
/// build a prompt: the epic banner, and the learnings to inject (recording each
/// retrieval as it goes).
///
/// Grouped because all four launch sites — `dispatch_task` and the epic chain in
/// `src/mcp/handlers/tasks/dispatch.rs`, and `exec_quick_dispatch` /
/// `exec_dispatch_agent` in `src/runtime/tasks.rs` — ran the identical
/// prologue inline, so a change to it meant editing four places.
///
/// Deliberately carries no verify command — see `fetch_verify_command` below.
pub struct DispatchInputs {
    pub epic_ctx: Option<EpicContext>,
    pub injected: Vec<crate::models::Learning>,
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
    DispatchInputs { epic_ctx, injected }
}

/// Fetch the verify command for a repository path from the settings store.
///
/// Called by both `get_task` (the earlier of the two surfaces — the
/// `/wrap-up` skill reads its "Verify command" line before ever calling
/// `wrap_up`) and the `wrap_up` handler itself, which echoes it back as a
/// secondary "Verify before exiting" reminder. No prompt builder reads it, so
/// this has no caller on a dispatch path despite living here. Logs a warning
/// and returns `None` if the DB lookup fails so either caller proceeds
/// without the line rather than failing.
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
/// `task.tmux_window = null` is a persisted-field precondition, not proof
/// that no live tmux window exists: the field can go stale (a race, or a
/// persist that never landed) while the window it used to name survives. So
/// before creating anything, this checks liveness live against tmux on the
/// deterministic window name; if a window already answers to it, this
/// returns immediately without creating a second one, sending keys, or
/// spawning a companion pane — reattachment then happens the normal way, via
/// the existing "has a window" branch of Space on the next keypress. A query
/// failure falls back to the unconditional-create behaviour this function
/// always had, rather than risking a task left with no live agent behind it.
/// See `docs/specs/dispatch.allium`'s `ResumeTask` rule.
///
/// This function is **synchronous** and should be called via
/// `tokio::task::spawn_blocking` from async contexts.
pub fn resume_agent(
    task_id: TaskId,
    worktree_path: &str,
    runner: &dyn ProcessRunner,
) -> Result<ResumeResult> {
    let tmux_window = build_tmux_window_name(task_id);

    if tmux::has_window(&tmux_window, runner).unwrap_or(false) {
        tracing::info!(
            task_id = task_id.0,
            %tmux_window,
            "resume found a live tmux window; reattaching instead of duplicating"
        );
        return Ok(ResumeResult { tmux_window });
    }

    tmux::new_window(&tmux_window, worktree_path, runner)
        .context("failed to create tmux window for resume")?;

    tmux::set_window_dispatch_dir(&tmux_window, worktree_path, runner)
        .context("failed to set tmux window dispatch dir")?;
    tmux::ensure_split_hook(runner).context("failed to ensure tmux split hook")?;

    let claude = runner.agent_binaries().claude_quoted();
    let name_flag = session_name_flag(task_id);
    tmux::send_keys(
        &tmux_window,
        &format!("{claude} {DISPATCH_PLUGIN_DIR}{name_flag} --continue"),
        runner,
    )
    .context("failed to send resume keys to tmux window")?;

    spawn_agent_tree_pane(&tmux_window, task_id, Some(worktree_path), runner);

    tracing::info!(task_id = task_id.0, %tmux_window, "agent resumed");

    Ok(ResumeResult { tmux_window })
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
    /// `dispatch` on `PATH`. Nothing about *finding* the pane depends on that
    /// path any more: the role marker is written on the pane whatever it exec'd.
    #[test]
    fn spawn_agent_tree_pane_launches_the_runners_dispatch_binary() {
        let mock = MockProcessRunner::new(vec![
            // list-panes: the companion is %9
            MockProcessRunner::ok_with_stdout(b"%1 \n%9 agent_tree\n"),
            MockProcessRunner::ok(),                     // kill-pane
            MockProcessRunner::ok_with_stdout(b"/wt\n"), // show-options @dispatch_dir
            MockProcessRunner::ok_with_stdout(b"%20\n"), // split-window
            MockProcessRunner::ok(),                     // set-option: the role marker
        ])
        .with_agent_binaries(AgentBinaries::stub());

        resync_agent_tree_pane("task-5", &mock);

        let split = &mock.recorded_calls()[3].1;
        assert!(
            split.contains(&"/stub/bin/dispatch-stub".to_string()),
            "companion pane must exec the runner's dispatch binary, got: {split:?}"
        );
    }

    #[test]
    fn resync_agent_tree_pane_kills_and_respawns_companion() {
        let mock = MockProcessRunner::new(vec![
            // list-panes: the companion is %9
            MockProcessRunner::ok_with_stdout(b"%1 \n%9 agent_tree\n"),
            MockProcessRunner::ok(),                     // kill-pane %9
            MockProcessRunner::ok_with_stdout(b"/wt\n"), // show-options @dispatch_dir
            MockProcessRunner::ok_with_stdout(b"%20\n"), // split-window relaunch
            MockProcessRunner::ok(),                     // set-option: the role marker
        ]);
        resync_agent_tree_pane("task-5", &mock);
        let calls = mock.recorded_calls();
        assert_eq!(calls.len(), 5);
        assert_eq!(calls[1].1, vec!["kill-pane", "-t", "%9"]);
        assert!(calls[3].1.contains(&"split-window".to_string()));
        // The window being split is targeted by its resolved pane ID, not its
        // name — otherwise the companion pane could open inside a
        // prefix-matched sibling's window (see `tmux::window_target`).
        assert!(calls[3].1.contains(&mock.pane_id_of("task-5")));
        assert!(calls[3].1.contains(&"5".to_string()));
        assert!(calls[3].1.contains(&"agent-tree".to_string()));
        // The worktree read back from @dispatch_dir becomes the new pane's
        // start directory, so the correction hook leaves it alone.
        assert!(
            calls[3].1.windows(2).any(|w| w == ["-c", "/wt"]),
            "expected -c /wt in: {:?}",
            calls[3].1
        );
        // The respawn is a *new* pane, so it needs its own marker — otherwise a
        // resynced window would look companion-less to the next toggle.
        assert_eq!(
            calls[4].1,
            vec![
                "set-option",
                "-p",
                "-t",
                "%20",
                tmux::PANE_ROLE_OPTION,
                tmux::PANE_ROLE_AGENT_TREE
            ]
        );
    }

    /// A window whose `@dispatch_dir` cannot be read still gets its companion
    /// pane — the split just goes out without a start directory, leaving the
    /// correction hook to place it.
    #[test]
    fn resync_agent_tree_pane_spawns_without_a_start_dir_when_the_option_is_unset() {
        let mock = MockProcessRunner::new(vec![
            // list-panes: the companion is %9
            MockProcessRunner::ok_with_stdout(b"%1 \n%9 agent_tree\n"),
            MockProcessRunner::ok(),                     // kill-pane
            MockProcessRunner::ok_with_stdout(b"\n"),    // show-options: unset
            MockProcessRunner::ok_with_stdout(b"%20\n"), // split-window
            MockProcessRunner::ok(),                     // set-option: the role marker
        ]);
        resync_agent_tree_pane("task-5", &mock);
        let calls = mock.recorded_calls();
        assert_eq!(calls.len(), 5);
        assert!(calls[3].1.contains(&"split-window".to_string()));
        assert!(
            !calls[3].1.contains(&"-c".to_string()),
            "no start directory to name, got: {:?}",
            calls[3].1
        );
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
            MockProcessRunner::ok_with_stdout(b"%1 \n%9 agent_tree\n"),
            MockProcessRunner::fail("kill-pane error"),
            MockProcessRunner::ok_with_stdout(b"/wt\n"),
            MockProcessRunner::ok_with_stdout(b"%20\n"),
            MockProcessRunner::ok(), // set-option: the role marker
        ]);
        resync_agent_tree_pane("task-5", &mock);
        let calls = mock.recorded_calls();
        assert_eq!(calls.len(), 5, "a respawn is still attempted");
        assert!(calls[3].1.contains(&"split-window".to_string()));
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
