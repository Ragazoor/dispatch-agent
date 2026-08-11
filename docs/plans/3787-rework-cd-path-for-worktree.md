# 3787 — Rework the worktree `cd` path for splits

**Goal:** Stop synthesising `cd <worktree>` keystrokes into tmux panes. Deliver the
same "a split inside an agent window starts in that task's worktree" guarantee
structurally — `split-window -c` for dispatch's own splits, a guarded
`respawn-pane -c` for user-initiated ones — so the mechanism can no longer
interact with whatever program happens to be running in the target pane.

---

## 1. Grounding: why the `cd` exists, and what is actually broken

### 1.1 The reported symptom is already fixed

The task reports `cd path_to_worktree` appearing in the Claude session on
`prefix + e`. That was task #3781's defect (an untargeted `send-keys` following
the session's *active* pane), fixed in `52d7b77d` — which landed 2026-07-29
14:19 CEST, roughly half an hour *before* this task was filed. It is regression-
tested in `tests/tmux_split_hook.rs`.

Replayed on the current tree against a real tmux 3.7b server — nested outer
server hosting an attached client, the real `bind-key e` binding, the real
`dispatch toggle-agent-tree-pane` binary, every pane running `cat >> log` so
keystrokes are captured — the `cd` lands in the newly created companion pane and
in no other pane. The board log and the agent log are both empty.

So there is nothing left to fix in *routing*. What remains is the mechanism.

### 1.2 What is still broken: the keystrokes themselves

The hook types `cd <worktree>` + Enter into the new pane. On `prefix + e` that
pane is the `dispatch agent-tree` companion TUI. Its key handler
(`src/cli/agent_tree.rs:267`) opens with:

```rust
KeyCode::Char('q') => return KeyAction::Exit,
```

**Any worktree path containing the letter `q` closes the companion pane the
instant it opens.** `j`/`k`/`h`/`l` move its cursor and `Enter`/space toggle
nodes, so every other path perturbs it too.

`docs/specs/split-pane.allium:505-509` explicitly accepts this side effect, and
its reasoning is incomplete — it accounts for Space and Enter ("two cancelling
toggles, no net state change") but not for `q`. That paragraph is wrong as
written, not merely dated.

This is the general shape of the problem: typing characters at a pane is only
correct if you know a shell is reading them, and the hook cannot know that.

### 1.3 Why the guarantee cannot simply be dropped

Three controlled experiments on tmux 3.7b (session cwd, window cwd and client
cwd all set to *different* directories, so the winner is unambiguous):

| Split | New pane's cwd |
|---|---|
| Client-initiated (`prefix + "`) on the agent window | **session** cwd — not the pane's, not the client's |
| External-CLI `split-window` (how dispatch shells out) | the **invoking process's** cwd |
| External-CLI `split-window -c <dir>` | `<dir>` ✓ |

So the spec's claim that the guarantee is load-bearing holds, and holds for user
splits too — a `prefix`-split inside an agent window does *not* inherit the
agent pane's worktree, it lands in whatever directory the tmux session was
started from. `-c` fixes dispatch's own splits outright; user splits still need
a correction after the fact.

### 1.4 The replacement, verified end to end

```
after-split-window:
  if-shell -F '#{&&:#{@dispatch_dir},#{!=:#{pane_start_path},#{@dispatch_dir}}}'
    'run-shell -bC "respawn-pane -k -t #{pane_id} -c \"#{@dispatch_dir}\""'
```

Measured against a real server:

- user `prefix`-split of the agent window → new pane respawned in the worktree ✓
- dispatch's own `-c` split → guard is false, hook inert, its command untouched ✓
- agent pane's captured input → empty ✓
- companion pane's captured input → empty ✓

`#{pane_start_path}` is populated at hook time (verified), which is what makes
the guard reliable: it is the directory tmux started the pane in, not a value
that depends on the pane's process having reported anything yet.

**The inner quoting around `#{@dispatch_dir}` is load-bearing.** With a worktree
path containing a space, the quoted form respawns into the correct directory
while the unquoted form silently drops the pane into `$HOME`. Both were measured;
the plan below pins this with a test.

Blast radius of a mis-evaluated guard is bounded to the pane that was just
created: `respawn-pane` names `#{pane_id}` explicitly, so it can never touch the
agent's Claude pane or the board.

Accepted cost: a user split that launched a command (`split-window -- vim`) has
that command restarted once, in the right directory. For a pane that is
milliseconds old this is preferable to typing characters into it blind.

---

## 2. Design

Two tiers, replacing one:

1. **Creation time.** Every dispatch-initiated split into an agent window passes
   `-c <worktree>`. Those panes are correct by construction; no hook, no
   keystrokes, no race. This covers `prefix + e` — the reported trigger.
2. **Correction time.** The `after-split-window` hook survives *only* for splits
   dispatch did not make, is guarded to fire only when the pane did not already
   start in the worktree, and corrects with `respawn-pane -c` rather than typed
   characters.

`@dispatch_dir` remains the single source of truth for a window's worktree, and
gains a reader so the callers that only know a window name can use it.

The board's own `split_window_horizontal` (`src/runtime/split.rs:118`) splits the
*board* window, which carries no `@dispatch_dir`; the hook is already inert there
and it needs no change.

---

## 3. Implementation plan (spec → tests → code)

### Step 0 — Spec first

`docs/specs/split-pane.allium`:

- `AgentWindowSplitStartsInTaskWorktree`: the rule body (`when`/`requires`/
  `ensures: PaneWorkingDirectorySet`) is mechanism-independent and **stays as
  is**. Rewrite the `@guidance` to describe the two-tier mechanism: `-c` at
  creation for dispatch-created panes, guarded `respawn-pane -c` for everything
  else. Keep the #231 / `8bf36803` history and the "cannot be deleted without
  replacing the guarantee" warning — record that the guarantee is now *met by
  `-c`* for dispatch's own splits, which is what makes the hook a fallback.
- Delete the "Accepted side effect" paragraph (505-509). It documents a
  keystroke side effect that no longer exists, and its reasoning was wrong (`q`).
- `SplitDirectoryTargetsNewPaneOnly` invariant (515-520): restate in terms of the
  *correction* rather than "the worktree `cd`" — it must reach the new pane and
  no other. Add that the correction is never delivered as keystrokes, so a pane
  running something other than a shell cannot misinterpret it.
- The prose at 460-463 mentions keystrokes; update to match.

`docs/specs/agent-tree.allium` (~350-370): the companion-pane split now carries a
start directory. Note it where the helper is described.

Run `allium:tend` for the edits and `allium:weed` afterwards to confirm alignment.

### Step 1 — Tests: the hook contract (real tmux)

`tests/tmux_split_hook.rs`. This file currently observes *keystroke routing*;
it becomes a file that observes *resulting working directories* plus the absence
of keystrokes everywhere. A helper reading `#{pane_current_path}` for a pane id
is needed in `tests/tmux_harness/mod.rs`.

Note the existing tests all split via `split_window_horizontal_running`, which
will now pass `-c` and therefore make the hook inert — they must be re-pointed,
not merely re-asserted. A raw `split-window` with no `-c` (i.e.
`pane_start_path != @dispatch_dir`) is the faithful stand-in for a user's
`prefix`-split and keeps the suite free of a nested tmux client.

New/rewritten tests:

1. `user_split_without_a_start_dir_is_respawned_into_the_worktree` — raw
   `split-window` on the agent window; assert the new pane's
   `pane_current_path` is the worktree.
2. `dispatch_split_with_a_start_dir_leaves_the_hook_inert` — split via
   `split_window_horizontal_running`; assert the new pane's cwd is the worktree
   **and** that its process was never restarted (pane runs a `cat >> log`
   marker; a respawn would truncate/restart it — assert the original process
   survives, e.g. by writing a sentinel into the log at startup and asserting it
   appears exactly once).
3. `split_hook_never_types_into_any_pane` — replaces the three routing tests:
   after both split kinds, the board log, the agent log and the new pane's log
   are all empty. This is the test that kills the `q` class of bug.
4. `split_hook_is_inert_for_windows_without_a_dispatch_dir` — keep, retargeted
   to assert cwd rather than absence of keystrokes.
5. `split_hook_handles_a_worktree_path_containing_a_space` — worktree named
   `work tree`; assert the respawned pane's cwd is that directory. Pins the
   inner quoting from §1.4; fails (lands in `$HOME`) if the quoting is dropped.

Every negative assertion needs a happens-before anchor, as the current file
already does via `read_when_written` — for a respawn the anchor is polling the
pane's `pane_current_path` until it changes, deadline-bounded through the
harness's existing `wait_until` helper. No sleeps
(`scripts/check-no-test-sleep.sh`).

### Step 2 — Tests: argv shape (mocks)

- `src/tmux.rs` inline tests:
  - `ensure_split_hook_issues_the_guarded_respawn_hook` — update the existing
    hook-string assertion (`src/tmux.rs:992`) to the new string. Per learning
    #327 this test alone proves nothing about behaviour; step 1 is what does.
  - `split_window_horizontal_running_passes_the_start_directory` — asserts
    `-c <dir>` is present.
  - `window_dispatch_dir_*` — reads the option back, and returns `None` for a
    window without it.
- `src/dispatch/agents.rs` inline tests: `spawn_agent_tree_pane` passes the
  worktree as the start directory; the toggle/resync paths resolve it from
  `@dispatch_dir`.
- `src/dispatch/tests.rs` and `src/dispatch/mock_sequence.rs`: the toggle and
  resync paths gain one `show-options` call, so scripted sequences need the extra
  step. Use `DispatchScript`, never a hand-written `vec![ok(), ok(), …]`.

### Step 3 — Code

1. `tmux::window_dispatch_dir(window, runner) -> Result<Option<String>>` — new;
   `show-options -w -v -t <window_target> @dispatch_dir`, empty output → `None`.
   Resolve the window through `window_target` like every other name-taking
   helper, so a prefix-matched sibling cannot answer.
2. `tmux::split_window_horizontal_running` gains a `start_dir: Option<&str>`
   parameter, emitting `-c <dir>` when present. Update its doc comment.
3. `spawn_agent_tree_pane` gains the worktree and forwards it.
   - `dispatch_with_prompt` / `resume_agent` / research dispatch already hold
     `worktree_path` — pass it directly.
   - `toggle_agent_tree_pane` / `resync_agent_tree_pane` only hold a window
     name — read `@dispatch_dir` via the new helper. Best-effort: `None` means
     no `-c`, and the hook's guard then corrects the pane, so the fallback path
     stays behaviourally identical to today minus the keystrokes.
4. `tmux::ensure_split_hook` — new hook string from §1.4, with the inner quoting.
   Rewrite the doc comment: it currently explains at length why `send-keys` needs
   `-t #{pane_id}` (`src/tmux.rs:365-375`). Keep the *lesson* (a hook's target
   context is lost inside `run-shell -bC`, so the target must be named
   explicitly — it applies verbatim to `respawn-pane -t`), and add why the
   mechanism is no longer keystrokes.
5. `src/cli/agent_tree.rs` needs no change — the fix removes the input rather
   than hardening the consumer.

### Step 4 — Docs and verification

- Re-read `CLAUDE.md` for references to the hook (the tmux-testing table rows
  still apply as written).
- `cargo fmt --check && cargo test && ./scripts/check-doc-paths.sh`
- `cargo clippy --all-targets -- -D warnings` (pre-push gate; a plain build will
  not catch it).
- `./scripts/check-doc-symbols.sh` — the new `window_dispatch_dir` and the
  removed `send-keys` references in specs must both line up.
- Manual smoke on a live board: dispatch a task whose slug contains a `q`,
  press `prefix + e` twice, confirm the companion pane opens and *stays* open;
  then `prefix + "` inside the agent window and confirm the new shell is in the
  worktree.

---

## 4. Risks

| Risk | Mitigation |
|---|---|
| `respawn-pane -k` restarts a command a user launched in their split | Bounded to the pane just created; documented in spec guidance. Guard means it never touches dispatch-created panes. |
| Guard string-compares `pane_start_path` to `@dispatch_dir`; a trailing-slash or symlink mismatch causes a needless respawn | Harmless — respawns into the same directory. Both values originate from the same `worktree_path` string for dispatch-created panes. |
| Older tmux without `#{pane_start_path}` or `#{&&:}`/`#{!=:}` | Verified on 3.7b. `pane_start_path` exists since tmux 3.1, the format operators since 2.9. Worth stating the floor in the spec guidance. |
| Mock-level hook-string test passes while behaviour breaks (#3781, #3782) | Step 1's real-tmux tests are the actual gate; learning #327. |

## 5. Out of scope

- Hardening `dispatch agent-tree` against stray keystrokes — the fix removes the
  stray keystrokes instead.
- The board's `split_window_horizontal` / `join_pane` paths, which split a window
  without `@dispatch_dir`.
- `toggle_agent_tree_pane`'s behaviour when the *companion* pane is focused
  (`inactive_pane_id` then names the Claude pane). Noticed while tracing this
  bug; unrelated to the `cd` mechanism and worth its own task.
