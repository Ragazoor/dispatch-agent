# Test-Gate Hardening & Flaky Tests

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:executing-plans to implement this plan task-by-task.

**Goal:** Make the documented "no wall-clock in tests" rule actually enforceable, and convert the three tests that violate it to deterministic signals.

## Context

This work package addresses findings from the 2026-08-16 codebase review
(`docs/plans/2026-08-16-codebase-review.md`, commit `c05f512c`).

This is the only finding in the review backed by an **observed failure**, not
static analysis. During the review, `cargo tarpaulin` aborted:

```
feed::tests::tick_does_not_block_event_loop
test result: FAILED. 4081 passed; 1 failed; 1 ignored
```

The same test passes in 0.06s unloaded (`cargo test --lib feed::tests::tick_does_not_block_event_loop`).
It fails only under instrumentation/load — i.e. it is a genuine flake, and it
blocks coverage runs.

## Findings

### 🚨 Wall-clock threshold assertion flakes under load (`src/feed/mod.rs:409-416`)

**Issue:** The test asserts a duration threshold against the wall clock:

```rust
let start = std::time::Instant::now();
runner.tick().await;
let elapsed = start.elapsed();
assert!(elapsed < Duration::from_millis(500), "tick() blocked for {elapsed:?}");
```

`CLAUDE.md` forbids exactly this — *"Tests must never sleep on the wall clock —
not to 'wait for' `spawn_blocking` or detached `tokio::spawn` work, and **not to
cross a duration threshold**"*. The intent of the test (tick must not block the
event loop) is correct and worth keeping; the *mechanism* is what flakes.

**Fix:** Assert the deterministic ordering instead of the clock. The epic's feed
command is `sleep 5`; the property under test is that `tick()` returns *before*
the spawned child finishes. Await a completion signal (a `oneshot`/`Notify`
resolved when the background task is spawned, or observe that `tick()` returns
while the `McpEvent::Refresh` for that epic has not yet been sent) rather than
measuring elapsed time. See the "No `tokio::time::sleep` in tests" section of
`docs/conventions.md` for the canonical patterns.

### 🚨 The pre-push gate cannot detect this pattern (`scripts/check-no-test-sleep.sh:34,45`)

**Issue:** The checker greps only for `tokio::time::sleep(` and
`std::thread::sleep(`. A threshold assertion built on `Instant::now()` /
`.elapsed()` is invisible to it. The rule is documented but unenforceable, which
is why three violations accumulated.

**Fix:** Extend the checker to reject `elapsed()` used in a comparison inside
test code (test files: anything under `tests/`, under a `src/**/tests/`
directory, or named `tests.rs`; plus inline `mod tests` blocks — match the
existing script's file-scoping logic). Reuse the existing `allow-test-sleep:
<why>` escape-hatch convention so a deliberate deadline-bounded poll can opt out
(`tests/tmux_harness/mod.rs:450` is such a case and must keep passing). Update
the script's self-test alongside it — the pre-push hook runs both.

### ⚠️ Two sibling violations (`src/process.rs:859`, `src/feed/exec.rs:437`)

**Issue:** Both assert `start.elapsed() < Duration::from_secs(5)`. The 5s budget
makes them far less flaky than the 500ms one, but they are the same pattern and
will trip the new checker.

**Fix:** Convert to deterministic signals the same way. If either genuinely
needs a deadline-bounded poll, annotate with `allow-test-sleep: <why>` and state
the reason — but prefer conversion.

## Changes

| File | Change |
|------|--------|
| `scripts/check-no-test-sleep.sh` | Add a third check rejecting `.elapsed()` comparisons in test code; honour the existing `allow-test-sleep:` escape hatch; extend the script's self-test |
| `src/feed/mod.rs:409-416` | Rewrite `tick_does_not_block_event_loop` to await a deterministic signal instead of asserting `elapsed < 500ms` |
| `src/process.rs:859` | Convert the `elapsed() < 5s` assertion to a deterministic signal (or annotate if truly deadline-bounded) |
| `src/feed/exec.rs:437` | Same as above |
| `docs/conventions.md` | Extend the "No `tokio::time::sleep` in tests" section to state that duration-threshold assertions are covered by the same rule and now by the same checker |
| `tests/tmux_harness/mod.rs:450` | Verify this deliberate deadline poll still passes; add `allow-test-sleep:` if the new check flags it |

## Verification

- [ ] `./scripts/check-no-test-sleep.sh` passes, and its self-test passes
- [ ] Deliberately re-introduce the `elapsed < 500ms` assertion locally and confirm the checker **fails** — a gate that cannot fail is not a gate
- [ ] `cargo test` — full suite green (needs `tmux` on PATH)
- [ ] `cargo test --lib feed::tests::tick_does_not_block_event_loop` passes
- [ ] Confirm the rewritten test still fails if `tick()` is made blocking (e.g. temporarily `.await` the child directly) — otherwise the test no longer tests anything
- [ ] `cargo tarpaulin --out Xml --skip-clean --timeout 600` completes without a test failure (this is the run that aborted during the review)
