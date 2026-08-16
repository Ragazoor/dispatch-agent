# 3782 — Real-tmux e2e harness for dispatch/resume pane topology

Follow-up to #3781. Generalises the real-tmux test pattern from
`tests/tmux_split_hook.rs` to the whole tmux integration surface: dispatch,
resume, split-pane pin/swap/unpin, teardown, and the main-session window.

**Revision 2** — reworked after adversarial review. See "Review findings" at the
end for what changed and why; the mechanism in Step 0 is materially different
from revision 1.

## Why

`MockProcessRunner` records argv. It can assert *"we handed tmux this string"*
but never *"tmux did what we meant"*. #3781 was a pure tmux-semantics defect
(`send-keys` with no `-t` inside `run-shell -bC` falls back to the session's
active pane) and the mock test pinned the broken string while staying green.
Anything about **which pane** receives what, **which cwd** a pane resolves, or
**how many panes** a window ends up with needs a real server.

#3781 landed that microscope for one hook. This task covers the lifecycle.

## Division of labour between the two test files

| File | Style | Windows | What it proves |
|---|---|---|---|
| `tests/tmux_split_hook.rs` (exists) | keystroke capture — every pane runs `cat > log` | synthetic | *routing*: which pane a `send-keys` lands in |
| `tests/tmux_lifecycle.rs` (new) | execution — panes run the real shell, with stub `claude`/`dispatch` recording `argv`/`$PWD`/`$TMUX_PANE` | created by production code | *topology + cwd*: what dispatch/resume/split actually build |

Style A can't be used for windows production creates: `tmux::new_window` starts
the default shell and takes no command. So the split-hook file stays the routing
microscope, and the new file lets the shell really run the command so the stub
can report its own cwd and pane. The board window is ours in both files, so the
**"no keystrokes reach the board"** invariant is assertable everywhere via a
`cat > board.log` capture pane.

## Step 0 — Shared harness

Move the reusable rig from `tests/tmux_split_hook.rs` into
`tests/tmux_harness/mod.rs`, shared via `mod tmux_harness;` from both test files
(only `tests/*.rs` are cargo auto-targets, so a subdirectory module is shared
without becoming its own binary — the pattern `tests/common/mod.rs` already
uses). Deliberately not `tests/common/mod.rs`, which drags in axum/db deps.

The module needs `#![allow(dead_code, clippy::unwrap_used, clippy::expect_used)]`
at the top, exactly as `tests/common/mod.rs` has. It is compiled once per binary
that includes it, and each binary uses only a subset of the helpers — an unused
helper is a `dead_code` warning, which the pre-push hook's `-D warnings` turns
into a hard error in whichever binary happens not to use it.

Moved as-is: `TmuxServer`, `SocketRunner`, `tmux_available`, the CI-hard-fail
skip guard, `capture_cmd`, `read_when_written`, `read_now`, `DELIVERY_DEADLINE`,
`POLL_STEP`.

Two changes to the moved code:

- **Start the server with `-f /dev/null`.** The current harness inherits the
  developer's `~/.tmux.conf`. That file can set `pane-base-index`, `prefix`,
  `default-command`, or its own hooks, so today a test's behaviour depends on
  whose machine it runs on — and CI (no config) exercises a different tmux than
  the developer does. Verified locally: this repo's own author has a `.tmux.conf`
  with a remapped prefix and `focus-events on`.
- `poll_until(deadline, pred) -> bool`, one bounded poller replacing ad-hoc
  loops. Bounded `std::thread::sleep`, which `scripts/check-no-test-sleep.sh`
  permits (it bans only `tokio::time::sleep`).

Added introspection: `pane_ids(window)`, `pane_count(window)`,
`active_pane_id(window)`, `window_names()`, `pane_cwd(pane_id)` (via
`#{pane_current_path}`), `window_option(window, "@dispatch_dir")`,
`pane_lefts(window)`.

### Stub binaries — injected through the test process's own PATH

**This is the part revision 1 got wrong.** It planned to publish the stubs with
`tmux set-environment -g PATH …`. Empirically that does not work: a tmux pane
inherits the environment of the **client that created it**, not the server's and
not the session environment. Verified against tmux 3.5a — `set-environment`
(global *and* session scope, before and after session creation) never reached a
subsequently created pane.

That same experiment shows the mechanism that *does* work, and it is simpler:
`SocketRunner` spawns `tmux` from the test process, so the test process **is**
the creating client. Setting `PATH` in the test process makes every window and
pane production creates resolve the stubs. Confirmed for all four forms this
harness depends on:

| Form | Used by | Stub resolved |
|---|---|---|
| `new-window -- cmd` | — | yes |
| `split-window -h -b -d -l 30% -- dispatch agent-tree N` | `spawn_agent_tree_pane` | yes |
| `send-keys` into the window's interactive shell | dispatch, resume | yes |
| `new-window -c <dir>` then `send-keys` | `resume_agent` | yes, and `$PWD` is the `-c` dir |

So: build the stub dir once per process (`OnceLock`, leaked for the process
lifetime — every test wants identical stubs), prepend it to `PATH` once. No
per-test env mutation, so no cross-thread race, and **no production change**.

Stubs record one line to a per-test log then `exec cat >> "$log"` to hold the
pane open (verified: the pane stays alive; a stub that exited would close its
pane and make every pane-count assertion racy). Staying alive via `cat` rather
than `sleep` needs no timer, and stray keystrokes that later hit those panes
append to the same log where an assertion can see them.

- `claude` — writes `claude pane=$TMUX_PANE cwd=$PWD args=$*`.
- `dispatch` — same shape; covers the companion pane's `dispatch agent-tree <id>`.

**Fail-fast safety guard, mandatory.** Real `claude` and real `dispatch` binaries
exist on a normal dev PATH (`~/.cargo/bin/dispatch` on the author's machine). If
stub injection ever silently breaks, these tests would execute the *real* ones:
a real `dispatch` mutating the developer's actual `~/.local/share/dispatch/tasks.db`
(the test cannot pass `--db` — the argv comes from production code), and a real
`claude` spawning a live agent that hits the network and may hang the test on
stdin. So the harness must assert its own injection before anything else: on
construction, resolve `claude` and `dispatch` and hard-fail unless both land
inside the stub dir. A skip here would be worse than a failure.

### Git fixture

`dispatch_agent` goes through `provision_worktree` (real `git worktree add`), so
tests need a real repo: `git init`, one commit, branch `main`, **plus a local
bare clone wired up as `origin`**.

The bare origin is not cosmetic. `provision_worktree` → `resolve_start_point` →
`fetch_origin_with_retry` retries `git fetch origin <base>` `FETCH_MAX_ATTEMPTS`
(3) times with `FETCH_RETRY_DELAY` between attempts. That delay is
`#[cfg(test)] 0ms` / `500ms` otherwise (`src/dispatch/worktree.rs:30`), and
`cfg(test)` is **false** for an integration test — it links the library in its
normal build. A repo with no origin therefore fails all three attempts and pays
~1s of real sleep per test, ~7s across Step 1 alone, against a 15s budget. A
working local origin makes the fetch succeed first try: no retries, no delay, and
a more realistic fixture. Use `file://` or a plain path to a bare repo in the
same tempdir.

Also confirm the fixture doesn't lean on the developer's real `HOME` or global
git config (set `GIT_CONFIG_GLOBAL=/dev/null`, `GIT_CONFIG_SYSTEM=/dev/null`, and
an explicit committer identity, so a machine without `user.email` set doesn't
fail the seed commit).

## Step 1 — Dispatch

Drive `dispatch_tui::dispatch::dispatch_agent(&task, &runner, None, &injections, None)`.

1. `dispatch_creates_agent_window_named_for_the_task` — `task-<id>` exists.
2. `dispatch_agent_window_starts_in_the_worktree_not_the_parent_repo` — the agent
   pane's `#{pane_current_path}` is the worktree. Real-server analogue of the
   argv-level `dispatch_agent_opens_tmux_window_in_worktree_not_parent_repo`.
3. `dispatch_launches_claude_in_the_worktree` — stub `claude` logged
   `cwd=<worktree>`, `args` carry `--plugin-dir` and the prompt.
4. `dispatch_opens_the_companion_agent_tree_pane` — 2 panes; stub `dispatch`
   logged `args=agent-tree <id>`.
5. `dispatch_companion_pane_is_on_the_left` — the companion's `#{pane_left}` is 0,
   locking `-b` in `split_window_horizontal_running`. Verified sound: the split is
   synchronous, and a `-b` split reports `0:0` / `1:25` immediately.
6. `dispatch_sets_dispatch_dir_on_the_agent_window` — `@dispatch_dir` is the
   worktree (the hook's precondition).
7. `dispatch_never_types_into_the_board_window` — board capture log empty.

## Step 2 — Resume

Drive `dispatch::resume_agent(task_id, worktree, &runner)` on a worktree whose
window has been killed.

8. `resume_creates_a_new_window_for_a_worktree_without_one`.
9. `resume_reaches_the_agent_pane_with_continue` — stub `claude` logged `--continue`,
   `pane` = the window's active pane, `cwd` = worktree.
10. `resume_opens_the_companion_pane`.
11. `resume_never_types_into_the_board_window`.

## Step 3 — Split-pane (pin / swap / unpin)

**Requires a production seam.** The tmux semantics live inside
`TuiRuntime::exec_enter_split_mode_with_task` and `exec_swap_split_pane`
(`src/runtime/split.rs`), interleaved with `spawn_blocking` and `msg_tx`
plumbing, so there is nothing an integration test can call. Extract both
sequences into functions over `&dyn ProcessRunner`:

- `join_task_window_into_pane(window, target_pane, runner) -> Result<String>` —
  the companion-aware join (capture `inactive_pane_id` *before* the join, then
  `join_pane`, then kill the leftover companion).
- `swap_task_window_into_pane(new_window, right_pane, old_window, runner) -> Result<String>` —
  `pane_id_for_window`, `swap_pane`, then rename + `resync_agent_tree_pane`, or
  `kill_window`.

`TuiRuntime` keeps only `spawn_blocking` + message emission. This matches the
Message→Command split in `docs/architecture.md`.

**Visibility:** `mod split;` is a *private* submodule of `runtime`
(`src/runtime/mod.rs:303`), so `pub fn` alone leaves these unreachable — Rust
caps visibility at the least-visible ancestor. Add an explicit
`pub use split::{join_task_window_into_pane, swap_task_window_into_pane};` to
`src/runtime/mod.rs` (already `pub mod runtime;` in `src/lib.rs`). This exposes
the two functions without exposing `split` or `TuiRuntime`.

12. `pin_joins_the_agent_pane_and_kills_the_leftover_companion` — board window
    ends with 2 panes; the companion's pane id is gone; the agent's pane id
    survived the move.
13. `pin_of_a_task_without_a_companion_pane_joins_cleanly`.
14. `swap_replaces_the_pinned_task_and_resyncs_the_companion` — after swapping
    task B in, the renamed window's companion logs `agent-tree <B>`, not `<A>`.
    `resync_agent_tree_pane` kills and re-splits asynchronously, so this must
    `poll_until` the new stub line appears — a single `read_now` snapshot would
    be racy.
15. `swap_works_when_pane_base_index_is_1` — **expected to fail first.**
    `exec_swap_split_pane` builds its swap source as `format!("{new_window}.0")`
    (`src/runtime/split.rs:262`). Verified against tmux 3.5a: with
    `pane-base-index 1`, `swap-pane -s w2.0` fails `can't find pane: 0`, while
    the pane-id form succeeds. Fix: use the `pane_id_for_window` result the
    function already fetched (line 251) as the swap source, and drop the
    `<window>.0` form from `swap_pane`'s doc comment. This is learning #324 in a
    spot #3781 didn't sweep. Worth shipping even if the rest of the harness is
    reworked.
16. `unpin_breaks_the_pane_back_into_its_own_window` — `break_pane_to_window`
    restores a `task-<id>` window with the pane id preserved; board back to 1 pane.
17. `split_operations_never_type_into_the_board_window`.

## Step 4 — Teardown

18. `killing_the_agent_window_removes_all_its_panes` — after
    `kill_window_if_present`, neither the agent nor the companion pane id exists.
19. `killing_the_agent_window_leaves_the_worktree_intact` — the ConfirmDone
    invariant (learning #298): worktree directory and its `.git` file survive.
    The *decision* to kill-not-clean is already unit-covered in
    `src/tui/tests/wrap_up.rs`; this asserts the tmux+filesystem effect.

## Step 5 — Main session

Per the decision on this task: **spec follows code.** `create_main_session`
deliberately gets no companion pane.

20. `main_session_window_has_a_single_pane`.
21. `splitting_the_main_session_window_sends_no_keystrokes` — anchored on a split
    that *does* fire the hook, so it isn't a race observing "nothing yet" (the
    anchoring trick from `split_hook_is_inert_for_windows_without_a_dispatch_dir`).
    Subsumes the `@dispatch_dir`-absence check: the absence is only interesting
    because it keeps the hook inert, which this asserts directly.

Spec work via `allium:tend`, then `allium:weed` to confirm alignment:

- delete rule `SplitAgentTreePaneOnMainSession` (`docs/specs/agent-tree.allium:334`)
  and its `@guidance` block;
- resolve the `MainSessionPaneScope` open question toward exclusion, recording
  why (no task id and no worktree ⇒ permanently empty tree, and the window is
  covered by neither teardown rule);
- add a rule stating the main-session window carries no `@dispatch_dir` and no
  companion pane, so the absence is specified rather than merely untested.

## Step 6 — Runtime and CI

21 tests, each starting a tmux server (~50 ms) on its own socket, so they
parallelise. Measure `cargo test --test tmux_lifecycle` wall time:

- under ~15 s → keep in the default suite (CI already installs tmux in the
  `test` and `coverage` jobs as of #3781);
- over that → move to a separate target and CI job, and record that here.

Recommendation is to keep it in the default suite: gating it risks the exact
silent-no-op failure mode the CI hard-fail guard exists to prevent. Update the
`Install tmux` comments in `.github/workflows/ci.yml` to name both files.

Socket names already embed pid and thread id, so concurrent `cargo test` runs
can't collide, and the drop guard kills each server even on panic.

## Production changes

Everything else is additive test code.

1. **The seam** — `join_task_window_into_pane` / `swap_task_window_into_pane`
   extracted out of `TuiRuntime`. Landed in `src/dispatch/split_panes.rs`, not
   `src/runtime/split.rs` as planned: `dispatch` is already a `pub` module and
   already owns `resync_agent_tree_pane`, which the swap path calls, so the two
   functions are reachable without exposing anything new about `runtime`. The
   `pub use` the plan called for is on `src/dispatch/mod.rs` instead.
2. **Swap-source fix** — `<window>.0` → the already-fetched pane id (test 15).
3. **`pane_exists` fix** — see below. Not anticipated; found while implementing.
4. `docs/specs/agent-tree.allium` main-session rules (Step 5).

### `tmux::pane_exists` was permanently blind

Found by tests 18/19 failing. `display-message -t <pane> -p ''` **succeeds for a
pane that never existed** — tmux resolves an unknown target by falling back to the
current pane, and an empty format string leaves no output to betray it. Verified
with `-t %999`. So the old exit-status check reported every pane as alive, always,
which made `exec_check_split_pane` unable to notice that the user had closed the
pinned split pane.

Fixed as a membership test over a new `list_all_pane_ids`, mirroring `has_window`.
The mock tests had pinned the broken behaviour by asserting a non-zero exit that
real tmux never returns — the same trap as #3781, in a second place.

The harness had copied the same broken technique, which is why the two teardown
tests appeared to fail: the panes were being cleaned up correctly all along and
the probe was lying.

### The stub rig needed more than PATH

`PATH` on the test process is enough for the `--` argv forms, which `execvp`
directly. It is **not** enough for `send-keys`, because tmux runs a pane's shell
as a *login* shell, which sources the user's rc files and prepends directories
ahead of the inherited `PATH`. On this machine that meant the stub `dispatch` won
(spawned via `split-window --`) while the **real `claude` launched and sat on its
trust prompt** — precisely the hazard the review flagged, reached despite the
process-level guard passing.

Fixed by pinning the pane shell (`default-command 'bash --norc --noprofile'`) and
moving the guard inside a real pane. Recorded here because the process-level check
alone reads as sufficient and is not.

## Verification

```
cargo test && ./scripts/check-doc-paths.sh
```

Plus, since the pre-push hook is not run by that line:
`cargo clippy --all-targets -- -D warnings` and
`./scripts/check-no-test-sleep.sh`.

## Review findings

An adversarial review challenged revision 1. Outcome, with what I verified
myself against a real tmux 3.5a rather than taking either side on trust:

**Upheld, and the plan changed.**

- *The stub-injection mechanism was wrong.* `tmux set-environment` does not
  reach later-created panes. Reworked: inject via the test process's PATH, which
  works because panes inherit the creating client's environment. The reviewer
  concluded this needed a redesign with no known-good mechanism; the correct
  mechanism turned out to be smaller than the one it replaced, and needs no
  production change.
- *Accidentally running the real `claude`/`dispatch`.* Legitimate and sharp —
  a real `dispatch` on PATH could mutate the developer's actual task DB. Added
  the mandatory fail-fast injection guard.
- *The `cfg(test)` retry delay doesn't apply to integration tests.* Correct, and
  it invalidated the runtime budget. Fixed by giving the fixture a working local
  bare `origin` so the fetch succeeds first try — better than absorbing the cost.
- *`pub fn` in a private module isn't reachable.* Correct. Added the `pub use`.
- *Missing `#![allow(dead_code)]` on the shared harness module.* Correct.
- *Test 14 needs `poll_until`.* Correct; `resync_agent_tree_pane` is async.
- *Tests 20 and 22 were redundant* with existing mock coverage
  (`kill_window_if_present_skips_when_absent`) or provable without a server.
  Dropped; 23 tests → 21.

**Independently found while verifying.**

- The existing harness inherits the developer's `~/.tmux.conf`, so test behaviour
  is machine-dependent. Added `-f /dev/null`.
- A `-b` split renumbers pane indices (the new left pane becomes index 0), which
  is a second, independent reason index-based pane targeting is unsafe — it
  reinforces the test-15 fix beyond the `pane-base-index` case.

**Not upheld.**

- The reviewer's verdict was "do not green-light; ~14 of 23 tests need a
  redesigned injection mechanism first." The mechanism needed replacing, not
  redesigning, and the replacement is verified working for all four command forms
  the harness uses. Steps 1–3 stand as scoped.
