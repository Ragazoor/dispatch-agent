# MockProcessRunner Behind test-support

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:executing-plans to implement this plan task-by-task.

**Goal:** Move ~310 lines of test scaffolding out of the release binary by gating `MockProcessRunner` behind a `test-support` cargo feature — the one place the codebase's otherwise-strict `#[cfg(test)]` rule cannot apply.

## Context

This work package addresses findings from the whole-repository codebase review of 2026-08-28 (`docs/plans/2026-08-28-codebase-review.md`, section 5.8).

The codebase is rigorous about keeping test doubles out of production. All of these are correctly `#[cfg(test)]`-gated:

- `src/dispatch/mock_sequence.rs` — 1,993 lines
- `MockLearningService` (`src/service/api.rs:590`)
- The entire `service_api_stub_trait!` / `service_api_stub_bridge!` family

There is exactly **one** exception: `MockProcessRunner` and its `ExitStatus` helpers in `src/process.rs`. These are ungated, so they compile into the release binary.

**This is a constraint, not carelessness.** `#[cfg(test)]` items are invisible to integration-test targets, and 10 files under `tests/` depend on these:

`tests/active_health.rs`, `tests/cli.rs`, `tests/common/mod.rs`, `tests/feed_sync.rs`, `tests/managed_feeds.rs`, `tests/task_watchers.rs`, `tests/tmux_editor_pane.rs`, `tests/tmux_harness/mod.rs`, `tests/tmux_lifecycle.rs`, `tests/tmux_window_targets.rs`

So a plain `#[cfg(test)]` will break the build. The idiomatic Rust answer is a feature that dev builds turn on and release builds do not.

**Priority: low.** This costs binary size and public API surface, not correctness. It is worth doing because it closes the last gap in an otherwise consistent rule — and because leaving it undocumented invites a future agent to "fix" it with `#[cfg(test)]` and break 10 test files.

## Findings

### 💡 ~310 lines of test scaffolding in the release binary (`src/process.rs:296`–`:630`)

**Issue:** The ungated region spans:

| Lines | Item |
|---:|---|
| 296–334 | Section header and `enum WindowLookup` (the mock's window-lookup policy) |
| 335–345 | `pub struct MockProcessRunner` |
| 346–581 | `impl MockProcessRunner` — constructors, response queue, call recording |
| 582–606 | `impl ProcessRunner for MockProcessRunner` |
| 607–630 | `pub fn exit_ok`, `pub fn exit_fail`, `pub fn exit_code` (all `#[cfg(unix)]`) |

Everything from line 635 (`#[cfg(test)] mod tests`) is already correctly gated.

The section is even labelled `// Mock implementation — for tests only`, so the intent is unambiguous; only the enforcement is missing.

**Fix:** Add an off-by-default feature and gate on it.

In `Cargo.toml`:

```toml
[features]
# Exposes `MockProcessRunner` and the `ExitStatus` helpers to integration tests.
# Off in release builds; `cargo test` turns it on via the self dev-dependency below.
test-support = []

[dev-dependencies]
# Self-dependency with the feature on, so `tests/` targets can see the mock.
# `cargo test` unifies this with the lib build; a plain `cargo build` does not.
dispatch-tui = { path = ".", features = ["test-support"] }
```

Then gate each item in `src/process.rs`:

```rust
#[cfg(any(test, feature = "test-support"))]
```

Apply it to `WindowLookup`, `MockProcessRunner`, both its `impl` blocks, and the three `exit_*` functions. Keep the existing `#[cfg(unix)]` on the `exit_*` helpers — the two attributes compose.

**Verify the self-dev-dependency trick actually works before doing the rest.** It is the load-bearing part of this plan and it can behave differently across cargo versions and with feature unification. If it does not work cleanly, the fallback is a small `test-support` crate under a workspace, and that is a bigger change than this work package justifies — **stop and report instead of escalating.**

### 💡 The exception is undocumented (`CLAUDE.md`)

**Issue:** Nothing tells a reader why this one mock is ungated while `mock_sequence.rs` (ten times larger) is not. That asymmetry looks like an oversight and invites a "fix" that breaks 10 integration-test files.

**Fix:** Add a line to `CLAUDE.md` stating the rule and its exception: test doubles are `#[cfg(test)]`-gated, except `MockProcessRunner`, which is behind `test-support` because `tests/` targets cannot see `cfg(test)` items.

WP1 also covers this. If WP1 has already landed, verify its wording matches the feature you actually introduced and adjust rather than duplicating.

### Note for WP2

WP2 (Task Fixture Consolidation) faces the same `cfg(test)`-invisibility problem — three of its 20 literal sites are in `tests/` targets — and offers `Task::fixture()` behind `test-support` as its option 2. **If this work package lands first, tell WP2 the feature exists** so it can use it instead of adding a public `Default for Task`. If WP2 landed first with `Default`, leave that alone; do not churn it.

## Changes

| File | Change |
|------|--------|
| `Cargo.toml` | Add `[features] test-support = []` and the self dev-dependency with the feature enabled |
| `src/process.rs` | Gate `WindowLookup`, `MockProcessRunner`, both `impl` blocks, and `exit_ok` / `exit_fail` / `exit_code` behind `#[cfg(any(test, feature = "test-support"))]` |
| `CLAUDE.md` | One line: the `cfg(test)` gating rule and its single named exception |

## Verification

- [ ] `cargo build` — succeeds **without** the feature. This is the whole point
- [ ] Confirm the mock is genuinely gone from a feature-less build. Either check for the symbol (`cargo build --release` then `nm -C target/release/dispatch | grep -i mockprocessrunner` — expect no output), or temporarily reference `MockProcessRunner` from a production path and confirm `cargo build` fails to compile
- [ ] `cargo test` — all pass, including every `tests/` target. All 10 listed files must still build
- [ ] `cargo test --no-fail-fast` — the `tmux_*` targets are among the heaviest users of the mock, and without this flag one blocked target hides the rest
- [ ] `cargo clippy --all-targets -- -D warnings` — clean. `--all-targets` implies the dev-dependency path, so this also proves the feature resolves
- [ ] `cargo tarpaulin --engine llvm --out stdout` — total coverage has not dropped below the 88 floor. Removing ~310 lines of *covered* mock code from the measured set can move the percentage in either direction; check rather than assume
- [ ] `cargo fmt` before committing
- [ ] `./scripts/check-doc-symbols.sh` passes — `MockProcessRunner` is cited from `docs/architecture.md` and `CLAUDE.md`, and it must still resolve
- [ ] Confirm nothing in production code referenced the mock: the only non-test hits are in `src/agent_tree_editor.rs`'s own `mod tests`, which is fine
