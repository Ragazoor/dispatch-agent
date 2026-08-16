# WP-1 — CI Gate Hardening

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:executing-plans to implement this plan task-by-task.

**Goal:** Make every gate the repo relies on actually run in CI, and turn the coverage measurement into a ratchet instead of an artifact nobody reads.

## Context

From the follow-up codebase review at `4bf19b04` (`docs/plans/2026-08-16-4220-codebase-review-followup.md`, smells B and C; Magic Wand #2).

`.githooks/pre-push` runs seven steps. `.github/workflows/ci.yml` has five jobs. Only *one* of the five gate scripts is present in both. CLAUDE.md states plainly that the hook "is silently inert" until a clone runs `git config core.hooksPath .githooks`, and nothing performs that for you — so three checkers are, in practice, optional.

This is the highest-leverage package in the set because it protects the *other* packages: the wall-clock rule that the previous review specifically built (`check-no-test-sleep.sh`) is one of the three that CI never runs.

## Findings

### ⚠️ Three gate scripts have no CI counterpart

**Issue:** `scripts/check-doc-symbols.sh`, `scripts/check-no-test-sleep.sh` and `scripts/test-fetch-reviews.sh` (plus the self-tests `test-check-doc-symbols.sh` and `test-check-no-test-sleep.sh`) run only from `.githooks/pre-push`. CI's `doc-paths` job runs `check-doc-paths.sh` and `test-check-doc-paths.sh` and nothing else.

A contributor — or a dispatched agent — working in a clone that never set `core.hooksPath` can push code that violates the doc-symbol rule or reintroduces a wall-clock assertion in a test, and every CI job stays green.

**Fix:** Extend the existing `doc-paths` job to run all five checkers and their three self-tests. Rename the job to reflect its widened remit (e.g. `gates` / "Repo gate scripts") so the name doesn't understate what it covers.

### ⚠️ Coverage is measured but gated on nothing

**Issue:** The `coverage` job runs tarpaulin, uploads `cobertura.xml` as an artifact, and prints a summary. No step compares the number to a threshold. Between the two reviews, total coverage rose (87.99% → 90.32%) while `src/runtime/commands.rs` fell (40% → 36.8%) — a regression no gate could see.

**Fix:** Add `--fail-under 88` to the tarpaulin invocation. Current measured coverage is 90.32%, so this gives ~2.3 points of slack: it fails on a genuine regression, not on noise. Do **not** set it at or just below the current number — a too-tight floor gets raised-then-ignored.

### 💡 The coverage job runs tarpaulin twice

**Issue:** The job invokes `cargo tarpaulin` once with `--out xml --output-dir coverage/` and again with `--out stdout --skip-clean`. Tarpaulin accepts multiple `--out` values in one run; the second invocation re-executes the whole 4,281-test suite to produce a format the first run could have emitted.

**Fix:** Collapse to a single invocation with both output formats. This roughly halves the job's wall-clock time.

## Changes

| File | Change |
|------|--------|
| `.github/workflows/ci.yml` | In the `doc-paths` job: add steps for `check-doc-symbols.sh`, `test-check-doc-symbols.sh`, `check-no-test-sleep.sh`, `test-check-no-test-sleep.sh`, `test-fetch-reviews.sh`. Rename job/`name:` to cover all gates. |
| `.github/workflows/ci.yml` | In the `coverage` job: merge the two `cargo tarpaulin` steps into one carrying both output formats, and add `--fail-under 88`. |
| `CLAUDE.md` | In the First-time setup paragraph, note which gate scripts CI also enforces and which are hook-only, so the `core.hooksPath` step's stakes are explicit. |

## Implementation notes

- **Check the scripts' assumptions before wiring them up.** `test-fetch-reviews.sh` is currently invoked as `bash ./scripts/test-fetch-reviews.sh` in the hook (not as a bare executable) — match that. Run each script locally from a clean checkout first; if any needs `gh` auth or network, it belongs in a separate job (or gets skipped in CI) rather than silently failing the new step.
- The `doc-paths` job today does **not** install Rust or restore the cargo cache, because the doc checkers are shell-only. Verify the three new scripts hold that property before adding them; if one shells out to `cargo`, give it the Rust setup + cache steps the other jobs use.
- `--fail-under` is a tarpaulin flag, not a cargo one — confirm the exact spelling against the pinned tarpaulin version rather than assuming.

## Verification

- [ ] `./scripts/check-doc-paths.sh`, `./scripts/check-doc-symbols.sh`, `./scripts/check-no-test-sleep.sh` all pass locally, plus the three self-tests and `bash ./scripts/test-fetch-reviews.sh`
- [ ] `cargo test` green (this package should not touch Rust, so a change here is a red flag)
- [ ] `cargo clippy --all-targets -- -D warnings` clean
- [ ] YAML parses: `python3 -c "import yaml,sys; yaml.safe_load(open('.github/workflows/ci.yml'))"`
- [ ] Deliberately break one rule locally (e.g. add `let _ = std::time::Instant::now().elapsed() < std::time::Duration::from_secs(1);` inside a `#[cfg(test)]` fn) and confirm `check-no-test-sleep.sh` rejects it — then revert. A gate you didn't watch fail is a gate you haven't tested.
- [ ] Confirm the single tarpaulin invocation emits both the XML file the upload step expects and the stdout summary
