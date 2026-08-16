# Global toggle keybinding for the companion pane (#3742)

Subtask 6 of epic #272 (Agent File Tree Panel). Depends on subtask 5 (#3741,
"tmux orchestration: split agent window, launch companion pane"), which adds
`tmux::split_window_horizontal_running` and wires `spawn_agent_tree_pane` into
`src/dispatch/agents.rs`.

## Dependency note

Subtask 5 (#3741) is not yet merged to `main` — it's still `running/active` in
its own worktree. Its single commit (`19d93777`, branch
`3741-tmux-orchestration-split-agent-window-launch-companion-pane`) has been
cherry-picked onto this branch as a standalone "dependency baseline" commit so
this task has `split_window_horizontal_running`/`kill_pane`/
`spawn_agent_tree_pane` to build on and test against. That commit is expected
to become a no-op once #3741 lands on `main` and this branch is rebased.

## Design recap (docs/superpowers/specs/2026-07-26-agent-file-tree-panel-design.md, "Companion Pane Mechanics")

- Global tmux key binding (`bind_key`/`unbind_key`), bound when the board TUI
  starts and unbound when it stops — same lifecycle as the existing
  prefix+Space "jump to TUI" binding (`setup_tmux_for_tui`/
  `teardown_tmux_for_tui`, `src/runtime/mod.rs`).
- On press, resolve which agent window it was pressed in via tmux
  format-string expansion (`#{window_name}`) through `run-shell` — new pattern,
  no existing precedent for a format-string-parameterized bound command.
- Toggle = kill-pane + re-split (not `resize-pane -Z` zoom), reusing subtask
  5's `spawn_agent_tree_pane`/`split_window_horizontal_running` for the
  "show" side and `tmux::kill_pane` for the "hide" side.

## Allium spec: resolving `ToggleTargetResolution`

`docs/specs/agent-tree.allium`'s `BindAgentTreeToggle`, `UnbindAgentTreeToggle`,
`HideAgentTreePane`, `ShowAgentTreePane` rules already specify the desired
behaviour (authored in subtask 1). The one open question this task answers is
`ToggleTargetResolution`:

**Decision**: no persisted `AgentTreePane` record (no DB row, no tmux user
option). The toggle resolves everything live from tmux at press time:

1. tmux itself expands `#{window_name}` into the bound `run-shell` command
   before invoking it — this process never has to ask "which window is
   focused".
2. The task id is parsed back out of the window name (`task-<id>`, the
   inverse of `build_tmux_window_name`). A window name that doesn't match the
   pattern (the TUI's own window, `dispatch-main`, anything else) makes the
   toggle a no-op — consistent with the "no retrofit" scope boundary and with
   `SplitAgentTreePaneOnMainSession` being unimplemented (see #3741's spec
   notes).
3. Shown-vs-hidden is read directly from tmux's own pane count
   (`#{window_panes}` on the window): 1 pane = hidden, 2+ = shown. This is
   exactly the "not persisted, re-derived from live tmux" framing already in
   `AgentTreePane`'s doc comment, just made concrete.
4. The companion pane, when present, is always pane index 1 (`<window>.1`) —
   guaranteed by `OneCompanionPanePerAgentWindow` plus the fact that the split
   helper's `-d` flag keeps focus/pane-0 as the agent's own pane.

This will be written into the spec as `== IMPLEMENTED ==` guidance blocks
(mirroring #3741's own additions) via `allium:tend`, done *before* writing
tests/code per this repo's spec-first convention.

## Design correction after adversarial review

An adversarial review of the first draft of this plan (general-purpose agent,
read-only, no code written) flagged that the original design hardcoded the
companion pane as tmux pane index 1 (`kill-pane -t <window>.1`). That's wrong:
tmux's `pane-base-index` option (default 0, commonly customized to 1) shifts
*which index* the agent's own pane gets, not just where numbering starts — so
under `pane-base-index 1`, the agent's own pane is index 1 and the companion
is index 2. A hardcoded `.1` target would kill the live Claude session
instead of the companion pane. Fixed by never hardcoding an index: query
tmux's own `pane_active` flag and target whichever pane in the window is
*inactive*. Every split helper in this module (`split_window_horizontal`,
`split_window_horizontal_running`, `join_pane`) passes `-d`, which keeps focus
on the target/source pane — so the freshly-split companion pane is always the
inactive one, regardless of what index tmux assigns it. This also collapses
two originally-separate tmux calls (a pane-count query, then a hardcoded kill)
into one (`list-panes`, which gives both "does a second pane exist" and,
robustly, "which one" in a single round trip).

The same review flagged two more issues, both **pre-existing gaps in the
board's split-pane feature** (`src/runtime/split.rs`, `join_pane` /
`exec_swap_split_pane`) rather than something this task can or should fix:
`join_pane` moves an agent's own (active) pane out of its window to join it
next to the board, which can leave the companion pane as the window's sole
remaining pane — indistinguishable, from live tmux state alone, from "toggled
hidden". `exec_swap_split_pane` renames a window to a different task's name
after swapping pane 0's content, but never touches pane 1, so a renamed
window's companion pane (if any) can end up rendering the *previous*
occupant's tree. Both stem from `split-pane.allium`'s join/swap mechanism
predating the companion-pane feature and assuming exactly one pane per agent
window — a cross-feature assumption break introduced when subtask #3741 added
a second pane to every agent window, not something the toggle keybinding
itself causes. Out of scope here: documented as a new open question
(`ToggleVsSplitPaneInteraction`) in `docs/specs/agent-tree.allium` and filed
as follow-up task #3770, mirroring how the design doc filed #3724 for an
unrelated discovered issue.

## Implementation

### 1. `src/tmux.rs` — new helper

```rust
pub fn inactive_pane_id(window: &str, runner: &dyn ProcessRunner) -> Result<Option<String>>
```
Runs `tmux list-panes -t <window> -F "#{pane_active} #{pane_id}"` and returns
the id of the pane whose `pane_active` flag is `0`, *iff* there is exactly one
such pane. Returns `None` for a single-pane window (nothing inactive) and,
defensively, for anything with more than one inactive pane (ambiguous — should
never occur given `OneCompanionPanePerAgentWindow`, but the function must not
guess). Sibling of `pane_id_for_window` / `current_pane_id`; no reliance on
pane index or `pane-base-index`.

### 2. `src/dispatch/prompts.rs` — inverse of `build_tmux_window_name`

```rust
pub(super) fn parse_tmux_window_task_id(window: &str) -> Option<TaskId>
```
`window.strip_prefix("task-")?.parse().ok()` (`TaskId` already has `FromStr`
via `define_id_newtype!`).

### 3. `src/dispatch/agents.rs` — toggle orchestration

```rust
pub(crate) fn toggle_agent_tree_pane(window: &str, runner: &dyn ProcessRunner) -> Result<()>
```
- No-op (`Ok(())`, zero tmux calls) if `parse_tmux_window_task_id` returns
  `None`.
- Otherwise `tmux::inactive_pane_id(window, runner)?`:
  - `Some(pane_id)` → `tmux::kill_pane(&pane_id, runner)`.
  - `None` → call the existing (private, same-module) `spawn_agent_tree_pane`
    — reuses `AGENT_TREE_PANE_PERCENT` and the exact
    `["dispatch", "agent-tree", <id>]` command already used at agent-launch
    time. Returns `Ok(())` (that function is itself best-effort/non-failing).

Re-exported from `src/dispatch/mod.rs`'s existing `pub use agents::{...}` list.

### 4. `src/runtime/mod.rs` — bind/unbind lifecycle

New constants next to `TUI_WINDOW_NAME`:
```rust
const AGENT_TREE_TOGGLE_KEY: &str = "e"; // matches config.agent_tree_toggle_key
const AGENT_TREE_TOGGLE_COMMAND: &str =
    "run-shell -b \"dispatch toggle-agent-tree-pane '#{window_name}'\"";
```
- `setup_tmux_for_tui`: add `let _ = tmux::bind_key(AGENT_TREE_TOGGLE_KEY, AGENT_TREE_TOGGLE_COMMAND, runner);` after the existing Space bind — same best-effort (`let _ =`) style.
- `teardown_tmux_for_tui`: add `let _ = tmux::unbind_key(AGENT_TREE_TOGGLE_KEY, runner);` alongside the Space unbind.

### 5. `src/main.rs` — new CLI subcommand

```rust
/// Toggle the companion agent-tree pane in the given tmux window.
ToggleAgentTreePane {
    /// tmux window name (e.g. "task-42"), supplied by tmux's own
    /// #{window_name} expansion — see the global toggle keybinding.
    window: String,
},
```
Handler:
```rust
fn cmd_toggle_agent_tree_pane(db: &Path, window: String) -> Result<()> {
    let data_dir = db.parent().unwrap_or(Path::new("."));
    let _ = init_app_log_subscriber(data_dir);
    let runner = dispatch_tui::process::RealProcessRunner;
    if let Err(e) = dispatch_tui::dispatch::toggle_agent_tree_pane(&window, &runner) {
        tracing::warn!(%window, error = %e, "failed to toggle agent-tree companion pane");
    }
    Ok(())
}
```
Always returns `Ok(())` — best-effort, matching `spawn_agent_tree_pane` and the
surface's `@guidance` ("a failing tmux call must not prevent the board TUI
from starting", extended here to "must not surface as a CLI error either,
since it runs detached via `run-shell -b`").

## Tests (written first)

- `src/tmux.rs`: `inactive_pane_id_finds_the_inactive_pane`,
  `inactive_pane_id_returns_none_for_single_pane_window`,
  `inactive_pane_id_returns_none_when_ambiguous` (2+ inactive panes),
  `inactive_pane_id_fails_on_nonzero_exit`.
- `src/dispatch/prompts.rs` (or wherever `build_tmux_window_name` is tested):
  `parse_tmux_window_task_id_roundtrips`, `parse_tmux_window_task_id_rejects_non_task_windows`
  (`"TUI"`, `"dispatch-main"`, `"task-"`, `"task-abc"`).
- `src/dispatch/tests.rs`: `toggle_agent_tree_pane_*`
  - `_is_noop_for_non_task_window` — zero recorded tmux calls.
  - `_hides_when_companion_pane_present` — mock sequence [list-panes →
    inactive pane found], asserts a single `kill-pane -t <that pane id>` call,
    nothing else. Include a variant where the inactive pane's id doesn't look
    like a positional index (e.g. `%7`), proving no index assumption leaked in.
  - `_shows_when_no_companion_pane` — mock sequence [list-panes → no inactive
    pane, split-window ok], asserts the split call matches
    `spawn_agent_tree_pane`'s existing shape (30%, `dispatch agent-tree 42`).
  - `_propagates_list_panes_query_failure` — list-panes call fails, error
    surfaces from `toggle_agent_tree_pane` (the CLI layer is what swallows it,
    not this function — keeps the core function's contract testable).
- `src/runtime/tests.rs`: update
  `setup_tmux_for_tui_renames_window_and_binds_key` (now 4 calls: pane-id,
  rename, bind space, bind toggle-key) and both teardown tests (extra
  unbind-key call).
- `tests/cli.rs`: one smoke test spawning the real binary —
  `toggle_agent_tree_pane_subcommand_never_fails_without_tmux` — asserts exit
  code 0 even though there's no real tmux session, proving the best-effort
  contract holds end-to-end (can't meaningfully assert tmux side effects here
  without a real tmux session, so this only checks "never crashes").

## Verification

`cargo test && ./scripts/check-doc-paths.sh`, plus `cargo clippy --all-targets -- -D warnings`
locally since clippy is pre-push-hook-enforced, not part of a plain build.

## Rebase note for wrap-up

Once #3741 merges to `main`, this branch's dependency-baseline commit becomes
redundant and should collapse away cleanly on rebase (same content, so `git
rebase` should treat it as an empty/no-op patch). Flag this in the PR
description.
