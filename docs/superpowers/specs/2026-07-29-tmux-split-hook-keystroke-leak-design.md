# Design: tmux `after-split-window` hook leaks keystrokes into the board

Task: #3781
Date: 2026-07-29

## Problem

Dispatching or resuming a task types a `cd <worktree>` shell line into the
dispatch board TUI as if the user had pressed those keys. Observed effect: the
`c` fires the Copy-Task keybinding, and the remaining characters (`d`, space,
the worktree path) are typed into the resulting repo-path field, leaving a
mangled buffer such as
`/home/ragge/Code/work/experiments/dispatchd /home/ragge/…/.worktrees/3779-quick-dispatch`
and a spurious "Directory does not exist" error. The user pressed nothing.

## Root cause

`ensure_split_hook` (`src/tmux.rs:200`) installs a session-level
`after-split-window` hook:

```
if-shell -F '#{@dispatch_dir}' 'run-shell -bC "send-keys \"cd #{@dispatch_dir}\" Enter"'
```

The `send-keys` carries **no `-t` target**. Inside `run-shell -bC` the enclosing
command's target context is lost, so `send-keys` falls back to the session's
**active pane**.

Every dispatch and resume calls `spawn_agent_tree_pane`
(`src/dispatch/agents.rs:30`), which runs `split-window -d` against the agent
window to open the agent-tree companion pane. That split fires the hook while
the board TUI is still the active pane, so the board receives the keystrokes.

The `if-shell -F '#{@dispatch_dir}'` *test* is correct — it reads the option on
the window being split, and the path expands correctly. Only the `send-keys`
*target* is wrong.

### Empirically verified

Throwaway tmux server on socket `dispatchtest` (the live session was untouched):

| Scenario | Where `cd …` landed |
|---|---|
| Split background agent window, board focused | **board TUI pane** — the reported bug |
| Split agent window while that agent is focused | the agent's own Claude pane |
| Same hook plus `send-keys -t '#{pane_id}'` | **the newly created pane** (intended); board received nothing |

So the hook has never worked as documented: it always types into whatever pane
is active, never the new one. A second, previously unnoticed consequence is that
splitting while focused on an agent types `cd …` straight into that agent's
Claude prompt.

`run-shell -C` losing its target was confirmed independently:
`run-shell -C -t agent 'split-window -h -d'` created the pane in the *active*
window, ignoring `-t agent`.

## Why the hook exists (do not delete it)

Commit `8bf36803` ("fix: split-screen goes to correct worktree instead of last
dispatched", issue #231). Its purpose: when a **user** splits a pane inside an
agent window, the new pane must start in that task's worktree.

This is load-bearing because tmux does not give it for free. For a
`split-window` invoked by an external CLI client — which is exactly how dispatch
and any script shells out to tmux — tmux resolves the start directory to the
*invoking client's* cwd, not the split pane's cwd. Verified: a split of a window
whose pane cwd was `…/fakewt` produced a pane in the caller's cwd instead.

The per-window `@dispatch_dir` option plus a conditional session hook is what
makes each agent window carry its own worktree, rather than every split landing
in the most recently dispatched one.

**Therefore the fix retargets the keystrokes; it does not remove them.** Any
future refactor that drops the hook must first replace this guarantee.

## Fix

Add an explicit target to the hook's `send-keys` so the keystrokes reach the
pane that was just created:

```
if-shell -F '#{@dispatch_dir}' 'run-shell -bC "send-keys -t #{pane_id} \"cd #{@dispatch_dir}\" Enter"'
```

`#{pane_id}` is expanded in the hook's context (the new pane) before
`run-shell` executes — confirmed by the third row of the table above.

### Accepted side effect

With the target corrected, dispatch's own agent-tree companion pane now
receives the `cd …` line. `src/cli/agent_tree.rs:236` ignores letters, but maps
`Space` and `Enter` to "toggle selected node", so the line produces two toggles
that cancel out. Cosmetically invisible; explicitly accepted to keep this change
minimal. Suppressing it (a per-window `@dispatch_suppress_cd` flag around
`spawn_agent_tree_pane`) is deliberately deferred.

## Documentation obligations

No Allium spec currently describes the hook — grep for `dispatch_dir` and
`after-split` across `docs/specs/` returns nothing. That absence is why a
refactor could silently delete a load-bearing behaviour.

1. Add a rule to `docs/specs/split-pane.allium` stating that a new tmux pane
   split inside an agent window starts in that task's worktree, and that the
   mechanism delivers keystrokes to the newly created pane only — never to the
   board or to the agent's own pane.
2. Keep the "why" (issue #231, tmux's client-cwd resolution) in the
   `ensure_split_hook` doc comment so the constraint survives a refactor.

## Test strategy

Three layers, because the failing layer was the integration boundary:

1. **Unit (mock)** — `src/tmux.rs`: assert the emitted hook string contains
   `send-keys -t #{pane_id}`. The existing test at `src/tmux.rs:693` pins the
   broken string verbatim and must be updated. Necessary but *not sufficient*:
   this layer is exactly what let the bug ship.
2. **Real-tmux integration** — new `tests/tmux_split_hook.rs`: start a tmux
   server on a unique `-L` socket, install the hook via the production
   `ensure_split_hook`/`set_window_dispatch_dir` functions, split a
   **background** agent window while another window is active, and assert the
   `cd` line arrived in the new pane and **not** in the active (board-stand-in)
   pane. Waiting is a bounded poll for the capture file to become non-empty —
   `run-shell -b` is asynchronous — never a fixed sleep. Skips with a clear
   message when `tmux` is absent so local runs without tmux stay green.
3. **CI** — add a `tmux` install step so layer 2 actually runs.

## Follow-up (separate task)

A broader real-tmux e2e harness: drive dispatch → resume → companion-pane
lifecycle end to end against a real tmux server, asserting window/pane topology
and that no keystrokes ever reach the board window. Larger than this bug fix and
better scoped on its own; the targeted test above is what proves *this* fix.
