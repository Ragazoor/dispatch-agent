# Task #4216 — A `DispatchScript` finish/rebase shape

## Problem

`dispatch::finish_task`'s subprocess sequence is hand-written as a positional
`MockProcessRunner` response vector at ~20 sites across four files:

- `src/dispatch/finish.rs` (8 inline tests)
- `src/dispatch/tests.rs` (10 `finish_task_*` tests)
- `src/mcp/handlers/tests/tasks/wrap_up.rs` (`rebase_ok_runner` + 2 inline)
- `src/mcp/handlers/tests/tasks/dispatch.rs` (1 inline)

`docs/testing.md`'s "Where new tests go" table and `docs/conventions.md`'s
"Driving a dispatch: `DispatchScript`, never a hand-written queue" already forbid
this shape — but only `dispatch`/`resume`/`provision` have a script, so the
finish tests have no compliant option. The dirty-worktree preflight landing
earlier is the exact failure mode the convention names: one new call means
splicing a response into every vector at the right offset, and a miss reads as a
confusing off-by-one mock pop.

## The real call sequence

`finish_task` (`src/dispatch/finish.rs::finish_task`) issues, in order:

| # | Call | Conditional on |
|---|------|----------------|
| 1 | `git -C <repo> rev-parse --abbrev-ref HEAD` (`git::current_branch`) | always |
| 2 | `git -C <repo> status --porcelain` (`git::dirty_files`) | HEAD == base branch |
| 3 | `git -C <repo> remote get-url origin` (`git::has_origin_remote`) | tree clean |
| 4 | `git -C <repo> pull --no-rebase origin <base>` | remote present |
| 5 | `git -C <worktree> rebase <base>` | pull ok |
| 6 | `git -C <worktree> status --porcelain` | rebase failed **and** it looks like a conflict |
| 7 | `git -C <worktree> rebase --abort` | rebase failed |
| 8 | `git -C <repo> merge --ff-only <branch>` | rebase ok |

Five of the eight are conditional, which is exactly the property that makes a
hand-written vector fragile.

## Design

Add the shape to the existing `DispatchScript` (`src/dispatch/mock_sequence.rs`)
rather than a parallel type, so `index_of` / `assert_matches` / `runner` are
reused verbatim. The finish configuration lives in its own nested `Finish`
struct behind one `Option` field, because a finish's axes (pull outcome, rebase
outcome, fast-forward outcome) and a dispatch's (fetch policy, PR head, fresh
worktree) are disjoint — folding them into one flat field set would put a
`fails_at(Step::NewWindow)` in reach of a finish shape.

```rust
DispatchScript::finish()                       // remote present, pull/rebase/ff all ok
    .no_remote()                               // remote get-url fails → pull skipped
    .base_branch("develop")                    // the branch HEAD is on and rebase targets
    .head_branch("feature-x")                  // HEAD is elsewhere → stops after step 1
    .dirty_primary(&["src/a.rs"])              // → stops after step 2
    .rebase_conflicts_in_stdout(&["lib.rs"])   // → status read + abort
    .rebase_conflicts_in_stderr(&["foo.rs"])   // same, marker on the other stream
    .rebase_fails()                            // non-conflict failure → abort, no status read
    .pull_fails() / .pull_cannot_run() / .pull_times_out(d)
    .rebase_cannot_run() / .rebase_times_out(d)
    .fast_forward_fails() / .fast_forward_cannot_run() / .fast_forward_times_out(d)
    .current_branch_cannot_run() / .remote_probe_cannot_run()
```

New `Step` variants: `CurrentBranch`, `DirtyCheck`, `Pull`, `Rebase`,
`ConflictStatus`, `RebaseAbort`, `FastForward`. `OriginProbe` is reused — it is
the same `git remote get-url origin` call, just reached from a different
operation.

`DirtyCheck` and `ConflictStatus` are both `git status --porcelain` and differ
only in the `-C` path (repo root vs worktree), which `Step::matches` cannot see
because it matches on program + argv tokens only. They therefore share a
predicate; positional ordering still separates them (the rebase sits between),
and the repo-vs-worktree scope is asserted by the tests that care via the error's
own `path` field.

## Steps (TDD — tests first at every step)

1. **Self-tests for the new shape.** Add tests to `mock_sequence.rs`'s module
   that drive a *real* `finish_task` for each shape and assert
   `script.assert_matches(&mock.recorded_calls())` — the load-bearing
   counterpart to `dispatch_script_matches_a_real_dispatch`. `finish_task` needs
   no repo on disk (it only runs subprocesses), so every variation is cheap to
   self-test: happy path with and without a remote, wrong branch, dirty primary,
   each conflict stream, non-conflict rebase failure, each `cannot_run`, each
   fast-forward failure. Plus `index_of` bookkeeping tests showing the optional
   `Pull` shifts everything after it. These fail to compile first.
2. **Implement** `Finish`, the `Step` variants, `Finish::steps()`,
   `Finish::responses()`, the modifiers, and the `failure_stderr` arms — the
   minimum to turn step 1 green.
3. **Convert the four call sites**, file by file, running `cargo test` after
   each: `src/dispatch/finish.rs`, `src/dispatch/tests.rs`,
   `src/mcp/handlers/tests/tasks/wrap_up.rs`,
   `src/mcp/handlers/tests/tasks/dispatch.rs`. Each converted test keeps its own
   assertions unchanged; only the runner construction changes, and
   `assert_matches` is added wherever the sequence itself is load-bearing (the
   "stops before any pull" and "issues no tmux call" tests).
4. **Document**: extend the `DispatchScript` section of `docs/conventions.md`
   with the finish family and update the module doc comment. No Allium spec
   change — this is test infrastructure, and `finish_task`'s behaviour is
   untouched.

## Verification

`cargo test` green. The conversion is behaviour-preserving by construction: a
converted test that asserted on a specific response (a conflict file name, a
timeout) still asserts the same thing, and `assert_matches` proves the derived
queue is the same sequence the hand-written one described.
