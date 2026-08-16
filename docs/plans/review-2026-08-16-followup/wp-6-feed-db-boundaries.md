# WP-6 — Feed & DB Boundaries

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:executing-plans to implement this plan task-by-task.

**Goal:** Make the feed's production polling path mockable, and replace a runtime length-invariant check with a type that cannot express the mismatch.

## Context

From the follow-up codebase review at `4bf19b04` (`docs/plans/2026-08-16-4220-codebase-review-followup.md`, carried findings L5 and M8).

Both findings survived the previous review's work packages. They are grouped here because they sit on the same boundary — the feed's ingest path into the database — and because each is small on its own.

## Findings

### 💡 The feed's production spawn bypasses `ProcessRunner`

**Issue:** `src/feed/exec.rs` spawns `tokio::process::Command::new("sh")` directly. `ProcessRunner` exists precisely so shell-outs are mockable, and `docs/architecture.md` states the rule: *"Tests use `MockProcessRunner` — never shell out in tests."*

Nine raw spawns remain repo-wide, but this is the one that matters: the others are setup-time (`src/setup/hooks.rs`, `src/setup/plugins.rs`, `src/main.rs`) and run once, while **this one is on the feed's production polling hot path**, executed on every tick. It is the only bypass that is both hot and unmockable, which is why it is scoped here and the setup-time ones are WP-9.

**Fix:** Route the spawn through `ProcessRunner`. Check the trait's existing surface first — `exec_feed_command` captures stdout and needs a timeout, so confirm `ProcessRunner` already exposes both before adding a method. Adding a trait method has a known hazard (see notes).

### 💡 `upsert_feed_tasks_inner` takes three parallel slices

**Issue:** The signature is

```rust
async fn upsert_feed_tasks_inner(
    &self,
    epic_id: EpicId,
    items: &[FeedItem],
    repo_paths: &[String],
    base_branches: &[String],
    delete_absent: bool,
) -> Result<Vec<RemovedFeedTask>>
```

The three slices are parallel-by-contract, enforced by a runtime `bail!` on length mismatch. The code's own comment concedes a mismatch would otherwise silently truncate. `src/feed/ingest/grouped.rs::upsert_sub_epic_and_recalc` has the same shape at 7 parameters.

**Fix:** Replace the three slices with one collection of a struct — `&[FeedTaskUpsert { item, repo_path, base_branch }]` or equivalent. The runtime check then becomes unnecessary because the mismatch is unrepresentable. Delete the `bail!` and its test once the type makes it dead.

## Changes

| File | Change |
|------|--------|
| `src/feed/exec.rs` | Route `exec_feed_command`'s spawn through `ProcessRunner` |
| `src/feed/mod.rs` | Thread the runner into `FeedRunner` if it doesn't already hold one |
| `src/db/queries/tasks.rs` | Replace `upsert_feed_tasks_inner`'s three slices with one struct slice; delete the length `bail!` |
| `src/feed/ingest/grouped.rs` | Update the call site; apply the same treatment to `upsert_sub_epic_and_recalc` if it shares the cluster |
| `src/db/mod.rs` | Update the `TaskStore` trait signature if the method is on it |
| `src/feed/ingest/tests.rs`, `src/db/tests/tasks.rs` | Update call sites; delete the now-unreachable mismatch test |

## Implementation notes

Do these as **two separate commits** — they are independent, and the `ProcessRunner` change is the riskier one.

**On the `ProcessRunner` change:**

- **This is the higher-risk half.** `FeedRunner` is a production poll loop; a mistake here shows up as feeds silently not ingesting, which no test currently catches well (`src/feed/mod.rs` is at 87.4% coverage).
- **KB #419 applies directly:** adding a method to a service/runner seam silently breaks any test mock that intercepted the method the new one subsumes — the stub default panics at *runtime*, not compile time. If you add a `ProcessRunner` method, grep for every mock implementation and update each one deliberately.
- **KB #336 / #353:** a `MockProcessRunner` panic on a detached `spawn_blocking` thread does not fail the test, and an under-scripted runner can make an error-path test pass on the mock's own panic. Script the runner fully.
- **KB #327:** `MockProcessRunner` tests assert argv, not semantics — they can pin a broken command string and stay green. For the feed exec path, argv assertion is the right level (the behaviour *is* "which command was issued"), but be explicit that that is what you are testing.
- Write the test that proves the feed exec path is now reachable from `MockProcessRunner` **before** making it so. That test failing to compile is the correct starting state.

**On the struct-ification:**

- Mechanical but wide. Build the struct, change the signature, let the compiler enumerate the call sites, fix each.
- The `bail!` and any test asserting it become dead once the type lands. **Delete them** — a runtime check for an unrepresentable state is noise, and leaving it implies the invariant is still fragile.
- Check `docs/conventions.md` on `FieldUpdate`/`TaskPatch` before touching anything in `src/db/queries/tasks.rs`; the file is the widest-fan-in surface in the codebase and the patch conventions are compiler-enforced.
- No behaviour change in either half, so no Allium spec edits expected. `docs/specs/feeds.allium` is the reference if you need to confirm intent.

## Verification

- [ ] `cargo test` green — redirect, don't pipe
- [ ] `cargo clippy --all-targets -- -D warnings` clean
- [ ] A test drives the feed exec path through `MockProcessRunner` and asserts the issued argv
- [ ] That test was seen failing (or failing to compile) before the change
- [ ] `rg 'process::Command::new' src/feed/` returns nothing
- [ ] The length-mismatch `bail!` and its test are gone, and the compiler rejects an attempt to reconstruct the mismatch
- [ ] Every mock implementation of `ProcessRunner` updated if the trait gained a method
