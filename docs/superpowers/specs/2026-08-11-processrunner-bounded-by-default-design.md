# ProcessRunner: bounded by default

**Task**: #3868 — `ProcessRunner::run_with_timeout`'s delegating default silently un-bounds every call site
**Date**: 2026-08-11
**Status**: design approved, ready for planning

## Problem

`ProcessRunner::run_with_timeout` has a default implementation that ignores the
timeout and delegates to `run()`. Only `RealProcessRunner` and
`MockProcessRunner` override it, so a future *production* impl that forgets the
override would silently un-bound all twelve call sites #3757 bounded, with no
compile error and no failing test. `docs/specs/repo-sync.allium` asserts that
every subprocess the sync engine issues is bounded; that claim would quietly
become false.

There is a second half to the problem. Bounding is opt-in **per call site**, so
a thirteenth call added to the finish or sync path is unbounded by default. The
`*_bounds_every_subprocess_it_runs` tests only guard the paths they enumerate.

This is a robustness concern, not a performance one — the same framing as #3757.

## Decision

Invert the trait so that **no unbounded path exists**.

```rust
pub trait ProcessRunner: Send + Sync {
    /// The one required method. An impl cannot exist without deciding how to
    /// honour a deadline.
    fn run_with_timeout(&self, program: &str, args: &[&str], timeout: Duration) -> Result<Output>;

    /// Bounded at the canonical `SUBPROCESS_TIMEOUT`.
    fn run(&self, program: &str, args: &[&str]) -> Result<Output> {
        self.run_with_timeout(program, args, SUBPROCESS_TIMEOUT)
    }

    fn agent_binaries(&self) -> AgentBinaries {
        AgentBinaries::default()
    }
}
```

The defaulting direction flips from unsafe to safe. An impl that writes only the
required method inherits a *bounded* `run`, and no call site can be unbounded
because the trait no longer offers an unbounded method. Both halves of the
problem stop being things anyone has to remember.

### Alternatives rejected

- **Remove the default, forcing every impl to choose** (direction 1 in the task).
  Cheapest and compiler-driven, but it guards only *new impls*. A thirteenth
  unbounded call site on the finish or sync path stays silently unbounded, which
  is half the reported problem.
- **Invert, but keep an explicit `run_unbounded`** (direction 2). Preserves
  today's exact behaviour at the ten deliberate call sites, but re-opens the
  opt-in hazard in mirror image: a site that *should* be bounded can still reach
  for the unbounded method. Review of all ten showed none of them wants to run
  forever, so the escape hatch would serve zero real cases.
- **A `#[cfg(test)]` impl-completeness test** (direction 3). Narrowest, and
  relies on remembering to register each new impl — the same class of "remember
  to do the safe thing" that caused the bug.

## Child stdin

`RealProcessRunner::run` uses `Command::output()`, which sets the child's stdin
to `Stdio::null()`. `run_bounded` sets stdin only when given a payload, so with
`stdin: None` the child **inherits** the parent's stdin — the TUI's raw-mode
terminal.

The inversion would therefore hand ten more call sites an inherited terminal. It
also exposes an existing latent issue: #3757 moved every git call from `run()`
to `run_with_timeout`, silently switching them from null to inherited stdin, and
the doc comment justifying that (`src/process.rs:162`, "unchanged from what every
git caller here has always had") is wrong about the baseline.

**Decision**: `run_bounded` sets `Stdio::null()` when the payload is `None`.

This makes the inversion behaviour-neutral for the migrating sites and undoes
the unintended #3757 change. A prompting `git` or `gh` now sees EOF and fails
fast instead of competing with the TUI for keystrokes until the 60 s bound
fires. The only caller that passes a payload is the statusline decorator
(`src/cli/statusline.rs`), which is a separate CLI process; no caller
legitimately reads inherited stdin. The doc comment at `src/process.rs:162` is
corrected as part of the change.

An explicit per-call stdin mode (`Null` / `Inherit` / `Payload`) was considered
and rejected: no caller wants `Inherit`, so the knob would add a decision to
every site to serve nothing.

## Impls to migrate

Nine, all mechanical.

| Impl | Location | Change |
|---|---|---|
| `RealProcessRunner` | `src/process.rs` | delete `run`; keep `run_with_timeout` |
| `MockProcessRunner` | `src/process.rs` | delete `run`; keep `run_with_timeout` |
| `AlwaysFailRunner` | `src/feed/exec.rs` | rename `run` → `run_with_timeout(_: Duration)` |
| `FixedBranchRunner` | `src/feed/exec.rs` | same rename |
| `CountingRunner` | `src/feed/exec.rs` | same rename |
| `PerRepoBranchRunner` | `src/feed/mod.rs` | same rename |
| `AlwaysFailRunner` | `tests/feed_sync.rs` | same rename |
| `AlwaysFailRunner` | `tests/managed_feeds.rs` | same rename |
| `SocketRunner` | `tests/tmux_harness/mod.rs` | tmux-arg rewrite moves to `run_with_timeout`, delegating to `inner.run_with_timeout` |

Deleting `RealProcessRunner::run` is the load-bearing part. Keeping an
`.output()`-based override would re-open the hole in the one impl where it
matters.

## Call sites that become bounded

Ten production sites move from unbounded to a 60 s bound:

- `src/tmux.rs` ×4 — `run_checked` (which `run_checked_stdout` and most helpers
  route through), `window_target`, `list_all`, `focus_events_enabled`; between
  them, every tmux call in the codebase
- `src/dispatch/mod.rs` ×2 — `gh pr view` on the 30 s PR poll loop and at
  dispatch-time worktree basing
- `src/dispatch/worktree.rs` ×2 — archive cleanup
- `src/runtime/settings.rs` ×2 — `notify-send`, `xdg-open`

None is long-running; every one is a command whose hanging would wedge the TUI.
`gh pr view` is the clearest case — an unbounded network call on a poll loop is
exactly the failure #3757 was about.

## Test surface

`MockProcessRunner::recorded_timeouts()` changes from `Vec<Option<Duration>>` to
`Vec<Duration>`. With nothing unbounded, `None` cannot occur, and keeping the
`Option` would leave four tests that read as guards but can never fail — the
dead-weight pattern #3757's last commit cleaned up.

Fourteen assertion sites are updated (`src/dispatch/finish.rs`, `src/git.rs`,
`src/process.rs`, `src/repo_sync.rs`). The four `*_bounds_every_subprocess_*`
tests keep their **count** assertions — that is what still catches a thirteenth
call appearing on a path — and swap `is_some()` for exact-duration equality,
which is a strictly stronger check.

Overriding `run` on the mock to keep recording `None` was rejected: the mock
would then report a call as unbounded that production bounds, lying about the
very property the tests exist to check.

### TDD order

Tests first, in this order:

1. **The regression test for this bug.** A minimal impl defining only
   `run_with_timeout`; calling `run` on it must record `SUBPROCESS_TIMEOUT`.
   This fails against today's trait.
2. **Null stdin.** `RealProcessRunner::run("cat", &[])` returns promptly with
   empty stdout. With inherited stdin it blocks.
3. **The payload path is untouched.** The existing `run_bounded` stdin tests
   (`run_bounded_writes_stdin_and_returns_stdout`,
   `run_bounded_does_not_deadlock_on_a_large_payload`,
   `run_bounded_ignores_a_child_that_never_reads_stdin`) must stay green.
4. Then the mechanical impl migration and the fourteen assertion rewrites.

## Spec and documentation

- `docs/specs/dispatch.allium` — a new guarantee that every subprocess the
  system issues carries a deadline, placed via `allium:tend` alongside
  `ChainedCommandIsBounded`.
- `docs/specs/repo-sync.allium` — the "Every subprocess the engine issues is
  bounded" paragraph in `@guidance` shrinks to reference the global guarantee
  instead of enumerating which calls it covers.
- `docs/conventions.md`, "One bounded-child primitive" — delete the
  delegating-default warning and the "which call sites must be bounded"
  judgement paragraph; both become moot. Add the null-stdin rule.
- `CLAUDE.md` — the conventions bullet gains "no unbounded path exists".

## Accepted tradeoff: the post-EOF exit poll

`run_bounded`'s wait for *exit* polls at `EXIT_POLL_STEP` (5 ms) after stdout
reaches EOF. Usually it costs nothing: a child that closed stdout has usually
exited, so the first `try_wait` succeeds. But there is a real race — the parent
can observe EOF in the window between the child closing the pipe and the kernel
making it reapable — and losing it costs one 5 ms sleep.

Today only git and worktree calls can pay that. Afterwards every tmux call can,
and those run several times per 2 s tick per live agent.

**Accepted.** It lands on `spawn_blocking` threads, never the render path. This
is deliberately *not* pre-optimised: if it shows up in a profile, the fix is a
sub-millisecond first backoff step before settling to 5 ms, not a redesign of
the wait. It is flagged here because it is an instance of the wakeup-shape
regression learning #360 warns about, and because the current
`EXIT_POLL_STEP` comment asserts the first `try_wait` is "normally already over"
— true today, less reliably true once every subprocess goes through it.

### Where the tradeoff is recorded

Three places, deliberately, because an agent tempted to tidy the wait will reach
one of them:

1. **`EXIT_POLL_STEP`'s doc comment** (`src/process.rs`) — rewritten to name the
   race, name who pays it, and name the trigger for revisiting.
2. **`docs/conventions.md`**, in the "two properties are load-bearing" part of
   the bounded-child section — so it sits next to the wakeup-shape warning it is
   an instance of.
3. **A `record_learning` entry** — the knowledge-base form, as a sequel to
   learning #360.

Deliberately **not** in the Allium specs: "abandonment can take up to 5 ms
longer than the deadline" is implementation mechanics, not domain behaviour, and
the specs already state what the deadline guarantees.
