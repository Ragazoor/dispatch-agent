# 3741 — tmux orchestration: split agent window, launch companion pane

**Goal:** add a `split_window_horizontal_running` sibling to
`split_window_horizontal` (`src/tmux.rs:278`) — narrower (30%), takes a
command to run in the new pane — and wire it into `src/dispatch/agents.rs`
so every newly-dispatched or resumed agent gets a `dispatch agent-tree
<task_id>` companion pane split next to its `claude` pane.

**Scope decision (confirmed with user):** the main-session window
(`create_main_session`) is explicitly **excluded** from this wiring, even
though `docs/specs/agent-tree.allium`'s `SplitAgentTreePaneOnMainSession`
rule and the design doc's "all three, for consistency" phrasing both
describe including it. Reason: subtask 4's already-implemented
`dispatch agent-tree <task_id>` (`src/cli/agent_tree.rs::run`) takes a
required `task_id: i64`, looks the task up in the DB, and errors if it
isn't found. The main session has no task at all, so there is no valid ID
to pass — the spec's own `MainSessionPaneScope` open question flags this
exact tension as unresolved. Only `dispatch_with_prompt` (covers
`dispatch_agent`/`research_agent`/`quick_dispatch_agent`) and `resume_agent`
are wired. `create_main_session` is untouched. The Allium spec gets a note
(via `allium:tend`) marking `SplitAgentTreePaneOnMainSession` as deferred.

**Split timing:** per the spec's `SplitAgentTreePaneOnAgentLaunch` guidance
("split immediately after it is created **and the agent command has been
sent**"), the split call goes right after the existing `tmux::send_keys`
call in each function, not right after `tmux::new_window`. This also means
every pre-existing test's assertions on earlier call indices (worktree add,
new-window, set-option, set-hook, send-keys) are untouched — the new call
is strictly appended after send-keys succeeds.

**Failure handling:** best-effort / soft-fail. A failure to split or launch
the companion pane is logged (`tracing::warn!`) and does not fail the
dispatch/resume call — the agent's own `claude` pane is the critical path;
the file-tree pane is decorative. Matches the codebase's existing
soft-fail-decoding convention (e.g. `fetch_verify_command`).

---

## Step 1: `tmux::split_window_horizontal_running` (TDD)

Add to `src/tmux.rs`, sibling of `split_window_horizontal` (40%, no
command) and mirroring `new_window_running`'s "create + immediately run a
command" shape (exec form via `--`, no shell wrapping):

```rust
pub fn split_window_horizontal_running(
    target_pane: &str,
    size_pct: u8,
    command: &[&str],
    runner: &dyn ProcessRunner,
) -> Result<String> {
    if command.is_empty() {
        bail!("split_window_horizontal_running: command must not be empty");
    }
    let size_arg = format!("{size_pct}%");
    let mut args: Vec<&str> = vec![
        "split-window", "-h", "-d", "-l", &size_arg, "-t", target_pane,
        "-P", "-F", "#{pane_id}", "--",
    ];
    args.extend(command.iter().copied());
    run_checked_stdout(runner, &args, "split-window")
}
```

Tests first (in `src/tmux.rs`'s existing `mod tests`):
- `split_window_horizontal_running_issues_correct_args` — asserts the full
  arg list including size and trailing command.
- `split_window_horizontal_running_keeps_argv_elements_separate` — a path
  with a space stays one argv element (mirrors
  `new_window_running_keeps_argv_elements_separate`).
- `split_window_horizontal_running_rejects_empty_command`
- `split_window_horizontal_running_fails_on_nonzero_exit`

Run `cargo test tmux::tests` red, then green.

## Step 2: wire into `src/dispatch/agents.rs` (TDD)

Add:
```rust
const AGENT_TREE_PANE_PERCENT: u8 = 30; // matches agent-tree.allium's agent_tree_pane_percent

fn spawn_agent_tree_pane(tmux_window: &str, task_id: TaskId, runner: &dyn ProcessRunner) {
    let id_arg = task_id.0.to_string();
    if let Err(e) = tmux::split_window_horizontal_running(
        tmux_window,
        AGENT_TREE_PANE_PERCENT,
        &["dispatch", "agent-tree", &id_arg],
        runner,
    ) {
        tracing::warn!(task_id = task_id.0, %tmux_window, error = %e, "failed to open agent-tree companion pane");
    }
}
```

Call `spawn_agent_tree_pane(&provision.tmux_window, task.id, runner)` in
`dispatch_with_prompt`, right after the existing
`tmux::send_keys(&provision.tmux_window, &claude_cmd, runner).context(...)?`
call. This single call site covers `dispatch_agent`, `research_agent`, and
`quick_dispatch_agent` — they all funnel through `dispatch_with_prompt`.

Call `spawn_agent_tree_pane(&tmux_window, task_id, runner)` in
`resume_agent`, right after its own `tmux::send_keys(...)?` call.

Do **not** touch `create_main_session`.

New tests first (in `src/dispatch/tests.rs`):
- `dispatch_agent_splits_agent_tree_companion_pane_after_send_keys` — full
  mock sequence through send-keys, plus a `split-window` mock response;
  assert the last recorded call is `split-window` with `-l 30%`, `-t
  <window>`, and a trailing `dispatch agent-tree <task_id>`.
- `resume_agent_splits_agent_tree_companion_pane_after_send_keys` — same
  shape for `resume_agent`.
- `dispatch_agent_succeeds_even_if_companion_pane_split_fails` — mock the
  split-window call as a failure; assert `dispatch_agent` still returns
  `Ok` (soft-fail).
- `create_main_session_does_not_split_companion_pane` — existing mock
  sequence (3 calls: new-window, send-keys -l, send-keys Enter) is
  unchanged and still exhausts exactly; no extra call is made.

## Step 3: fix the ripple across existing tests

Every existing `MockProcessRunner` sequence that drives `dispatch_agent`/
`research_agent`/`quick_dispatch_agent` (via `dispatch_with_prompt`) or
`resume_agent` all the way through a successful `send-keys Enter` now needs
one more queued response (`MockProcessRunner::ok_with_stdout(b"%9\n")`) for
the new split-window call, appended after the existing "tmux send-keys
Enter" comment line. Sequences that already error out before reaching
send-keys (e.g. a failed `git worktree add`, a failed prompt-file write) are
untouched.

Known locations (found via `grep -rn "tmux send-keys Enter" src/`):
`src/dispatch/tests.rs` (~19 sequences), `src/mcp/handlers/tests/tasks/dispatch.rs`
(~7), `src/mcp/handlers/tests/tasks/wrap_up.rs` (1), `src/runtime/tests.rs`
(~6). `resume_skips_git_issues_tmux_continue`
(`src/dispatch/tests.rs:1368`) additionally asserts `calls.len() == 5`,
which becomes `6`.

Strategy: implement Steps 1–2 first, then run `cargo test`, and fix each
failing test reactively — the mock panics with a clear "no response queued
for tmux split-window ..." message naming the exact call, which is more
reliable than pre-enumerating every call site by hand.

## Step 4: Allium spec note

Use `allium:tend` on `docs/specs/agent-tree.allium` to mark
`SplitAgentTreePaneOnAgentLaunch` as implemented by this task, and annotate
`SplitAgentTreePaneOnMainSession` as **deferred** (not implemented — main
session has no task, no valid `dispatch agent-tree <task_id>` argument
exists; see the `MainSessionPaneScope` open question). Do not resolve the
open question itself — just record that this task did not implement that
rule.

## Step 5: verification

```bash
cargo test
./scripts/check-doc-paths.sh
```
