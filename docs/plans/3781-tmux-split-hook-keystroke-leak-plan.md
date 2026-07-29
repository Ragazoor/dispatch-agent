# Plan: fix tmux split-hook keystroke leak into the board

Task: #3781
Design: `docs/superpowers/specs/2026-07-29-tmux-split-hook-keystroke-leak-design.md`

Order follows the repo convention: spec → tests → code.

## Step 1 — Spec first

Add a rule to `docs/specs/split-pane.allium` (via the `allium:tend` skill)
covering new-pane cwd inheritance inside an agent window:

- A tmux pane split inside an agent window starts in that task's worktree.
- The mechanism targets the **newly created pane only** — never the board
  window, never the agent's own pane.
- `@guidance` records *why* it cannot be deleted: tmux resolves a CLI-invoked
  `split-window` start directory to the invoking client's cwd, so without this
  every split would land in the dispatch process's cwd (issue #231).

Verify with `allium:weed` that spec and code agree once step 3 lands.

## Step 2 — Tests (must fail before step 3)

### 2a. Unit, mock layer

`src/tmux.rs` inline tests:

- Update `ensure_split_hook_issues_correct_args` (currently `src/tmux.rs:693`)
  to expect the `send-keys -t #{pane_id}` form. It pins the broken string today.
- Add an assertion that the hook string contains `-t #{pane_id}` between
  `send-keys` and the `cd` payload, so a future edit cannot silently drop the
  target while still looking plausible.

Expected before step 3: fails on the changed expectation.

### 2b. Real-tmux integration (the test that actually catches this class)

New `tests/tmux_split_hook.rs`:

1. Skip early with a clear message if `tmux` is not on `PATH`.
2. Start a server on a unique socket name (`-L dispatch-test-<pid>`); kill it in
   a guard so no server leaks even on panic.
3. Window `board` (active) and window `agent` (background), each running
   `cat > <file>` so anything typed into them is captured on disk.
4. Set `@dispatch_dir` on `agent` and install the hook through the **production**
   functions (`set_window_dispatch_dir`, `ensure_split_hook`) with a real
   `ProcessRunner` — not a hand-written tmux string, or the test would drift
   from what ships.
5. `split-window -d` against `agent` (mirroring `spawn_agent_tree_pane`), with
   the new pane also capturing to a file.
6. Poll (bounded deadline, e.g. 5 s) until the new pane's capture file is
   non-empty; `run-shell -b` is asynchronous. No `tokio::time::sleep` — the
   pre-push hook rejects it, and this is condition-based waiting anyway.
7. Assert: new pane's file contains `cd <dispatch_dir>`; the **board** file is
   empty; the agent's own pane file is empty.

Expected before step 3: the board file contains the `cd` line — reproducing the
reported bug as a failing test.

## Step 3 — Code

`src/tmux.rs`, `ensure_split_hook`: add `-t #{pane_id}` to the hook's
`send-keys`.

Extend the doc comment to state that the target is required (`run-shell -bC`
loses the enclosing target and `send-keys` would otherwise hit the active pane,
i.e. the board), and to record the issue-#231 reason the hook exists at all.

Both tests from step 2 go green. No other production file changes.

## Step 4 — CI

`.github/workflows/ci.yml`, `test` job: install `tmux` before `cargo test`, so
step 2b runs in CI rather than silently skipping.

## Step 5 — Verify

```
cargo test && ./scripts/check-doc-paths.sh
```

Plus `cargo clippy --all-targets -- -D warnings` and `cargo fmt`, which the
pre-push hook enforces.

Manual confirmation: dispatch a task in the real TUI and check that no `cd` text
appears in the board and no Copy-Task dialog opens.

## Step 6 — Follow-up task

Create a task for the broader real-tmux e2e harness (dispatch → resume →
companion-pane topology, asserting no keystrokes ever reach the board window),
as scoped in the design doc's Follow-up section.

## Out of scope

- Suppressing the `cd` line for dispatch's own companion pane (the two
  cancelling tree toggles). Deliberately deferred; recorded in the design doc.
- Removing the hook in favour of `-c <dir>` on dispatch's own splits. That would
  regress user-initiated splits, which the hook exists to serve.
