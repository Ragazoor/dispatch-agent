# WP-9 — Route `src/setup/` Spawns Through `ProcessRunner`

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:executing-plans to implement this plan task-by-task.

**Goal:** Bring the last of the raw subprocess spawns behind `ProcessRunner`, so setup and hook-install paths are testable without shelling out.

## Context

From the follow-up codebase review at `4bf19b04` (`docs/plans/2026-08-16-4220-codebase-review-followup.md`, carried finding L5, second half).

`docs/architecture.md` states the rule: `ProcessRunner` is the abstraction over git/tmux shell commands, and *"tests use `MockProcessRunner` — never shell out in tests."* Nine raw spawns remain. **WP-6 owns the one that matters most** (`src/feed/exec.rs`, on the production polling hot path). This package covers the remaining setup-time ones.

**The review ranks this last on purpose.** These paths run once, at install time, not in a loop — so the payoff is testability and consistency, not correctness under load. Do not let it displace WP-1, WP-2 or WP-6.

## Findings

### 💡 Eight setup-time spawns bypass `ProcessRunner`

**Issue:**

| Site | Spawns |
|---|---|
| `src/setup/hooks.rs` (4 sites) | `bash`, and one `std::process::Command::new(args[0])` |
| `src/setup/plugins.rs` (2 sites) | `bash` |
| `src/main.rs` (1 site) | `sh` |
| `src/feed/cycle.rs` (1 site) | `mkfifo` |

Coverage reflects the consequence: `src/setup/mod.rs` sits at **69.7%**, the second-worst real file after the runtime pair, and its uncovered lines are concentrated in the OS-interaction branches.

**Fix:** Route each through `ProcessRunner`. Take them one at a time, verifying after each.

**Judgement required, per site.** These are not uniformly worth converting:

- `src/setup/hooks.rs`'s `std::process::Command::new(args[0])` executes a *user-supplied* command (the chained statusLine). Confirm `ProcessRunner`'s interface can express that without weakening it.
- `src/feed/cycle.rs`'s `mkfifo` is a one-shot filesystem primitive, not a shell-out in the sense the abstraction targets. **It may be right to leave it and document why** — record that as the outcome rather than forcing it.
- `src/main.rs`'s spawn is on a CLI path that already has black-box coverage via `tests/cli.rs`.

The review's own guidance stands: *"chasing coverage on `src/setup/mod.rs` (69.7%) — OS interaction. CLAUDE.md says so and it is right."* The goal here is **consistency of the abstraction**, not the coverage number. If converting a site makes the code worse, say so and leave it.

## Changes

| File | Change |
|------|--------|
| `src/setup/hooks.rs` | Route 4 spawn sites through `ProcessRunner` (assess the `args[0]` site separately) |
| `src/setup/plugins.rs` | Route 2 `bash` spawns |
| `src/main.rs` | Route the `sh` spawn, or document why it stays |
| `src/feed/cycle.rs` | Assess `mkfifo`; document the decision either way |
| `src/setup/mod.rs` | Thread the runner through wherever setup entry points construct these calls |
| `src/setup/hooks.rs`, `src/setup/plugins.rs` (test mods) | Add `MockProcessRunner`-based tests for the newly reachable paths |

## Implementation notes

- **One site per commit.** They are independent, and setup code that breaks leaves a user with a half-installed plugin — a bad failure to bisect.
- **KB #419 is the primary hazard.** Adding a method to a runner seam silently breaks any test mock that intercepted the method the new one subsumes; the stub default panics at *runtime*, not compile time. If you extend `ProcessRunner`, grep every implementation and update each deliberately. Do not rely on the compiler.
- **KB #327:** `MockProcessRunner` tests assert argv, not semantics — they can pin a broken command string and stay green. For hook installation the argv *is* largely the behaviour, but where a test asserts that a hook was installed *correctly*, argv is not sufficient; the existing `src/setup/hooks.rs` suite asserts on embedded script content, and that style should stay.
- **KB #336 / #353:** under-scripted mocks hide failures on detached threads. Script fully.
- **Preserve `src/setup/hooks.rs`'s existing 1,026-line test module.** It asserts hook-script content and `hooks.json` metadata, and is unrelated to this change — do not disturb it. Note that **WP-3 may rename this file**; check whether that has landed and rebase rather than fighting it.
- Write the `MockProcessRunner` test for each site before converting it. The test failing to compile against the current raw spawn is the correct starting state.
- No behaviour change: setup must install exactly what it installs today. `tests/githooks.rs` and the `contains` tests in `src/setup/plugins.rs` are the guardrails.

## Verification

- [ ] `cargo test` green — redirect, don't pipe
- [ ] `cargo clippy --all-targets -- -D warnings` clean
- [ ] `bash ./scripts/test-fetch-reviews.sh` and the doc checkers pass
- [ ] Every converted site has a `MockProcessRunner` test that was seen failing first
- [ ] Every mock implementation of `ProcessRunner` updated if the trait gained a method
- [ ] Each **unconverted** site carries a comment saying why it stays
- [ ] End-to-end smoke against a throwaway home: run `cargo run -- setup` with `HOME` pointed at a temp dir and confirm the plugin, hooks and MCP config land as before
- [ ] `src/setup/mod.rs` coverage has not regressed
