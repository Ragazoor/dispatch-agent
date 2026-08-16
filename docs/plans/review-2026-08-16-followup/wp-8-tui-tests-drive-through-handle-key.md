# WP-8 — Drive TUI Tests Through `handle_key`

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:executing-plans to implement this plan task-by-task.

**Goal:** Establish the pattern and tooling for driving TUI tests through the real input path, convert the highest-value file, and leave a queue the rest can be worked through against.

## ⚠️ Scope — read this first

The finding covers **1,081 direct field assignments**. This is not a one-session task and the plan does not pretend otherwise. Converting them mechanically would also be the wrong move: many assignments are legitimate fixture setup, and a blanket rewrite would churn the largest suite in the repo for no gain.

**This package delivers three things, and stops:**

1. A written, agreed rule for when a test may set state directly and when it must drive it.
2. Any test helper that rule needs (e.g. a `press("gg")` / `press_seq(...)` driver).
3. **One** converted file — the highest-value one — proving the pattern, plus a triage list of the rest.

The remaining files become follow-up tasks sized from what conversion #1 actually cost. Do not open a second file until the first has landed and the cost is known.

## Context

From the follow-up codebase review at `4bf19b04` (`docs/plans/2026-08-16-4220-codebase-review-followup.md`, carried finding L7).

Flagged in both reviews. Current ratio: **1,081 direct `app.<field>.<field> = …` assignments against 679 `handle_key` drives**. The previous review's example remains the clearest statement of the problem:

> `snapshot_input_title_form` hand-assigns `InputMode::InputTitle` + buffer + draft, rendering a state **no key sequence is proven to reach**; contrast `snapshot_help_overlay`, which presses `?`.

Roughly half of the largest suite in the repo tests *the renderer given a state*, not the state machine that reaches it. The failure mode is silent: the renderer is verified against a state the application may no longer be able to produce, and the test stays green through a regression in the path that produces it.

## Findings

### 💡 Half the TUI suite asserts on states no key sequence is proven to reach

**Issue:** 1,081 direct field assignments vs 679 `handle_key` drives across `src/tui/tests/`. A test that assembles `InputMode` + buffer + draft by hand and then renders proves the *renderer* correct and proves nothing about the transition into that mode.

**Fix (staged, per the scope note above):**

**Stage 1 — the rule.** Direct assignment is legitimate for *seeding the world* (tasks on the board, epics, repo paths, a fixed clock). It is illegitimate for *reaching an interaction state* (input mode, caret position, selection, pending action, popup state) when a key sequence exists that reaches it. Write this down in `docs/testing.md` — that is the file the repo already points agents at for where a new test goes.

**Stage 2 — the driver.** Add a helper that presses a sequence and returns the resulting commands, so a conversion is one line rather than five `KeyEvent` constructions. Check `src/tui/tests/helpers.rs` first — something close may already exist. Twelve `make_app*` variants exist repo-wide; **do not add a thirteenth** — extend a shared one.

**Stage 3 — convert one file.** Pick by value, not by size. `src/tui/tests/snapshots.rs` is the strongest candidate: snapshot tests are precisely where "a state no key sequence reaches" is most dangerous, because the snapshot then locks in the unreachable rendering. `src/tui/tests/input_handlers.rs` is the alternative if snapshots prove entangled.

**Stage 4 — triage the rest.** Produce a table: file, assignment count, rough conversion cost, whether conversion is possible at all (some states genuinely have no key path — those stay, and should carry a one-line comment saying so). File follow-up tasks from it.

## Changes

| File | Change |
|------|--------|
| `docs/testing.md` | Write down the seed-vs-drive rule |
| `src/tui/tests/helpers.rs` | Add or extend the key-sequence driver; do not add a new `make_app*` |
| `src/tui/tests/snapshots.rs` *(or `input_handlers.rs`)* | Convert the assignments that have a key path |
| `docs/plans/review-2026-08-16-followup/wp-8-triage.md` | New — the per-file triage table |

## Implementation notes

- **A conversion that changes a snapshot has found a bug.** If pressing the keys produces a different render than the hand-assembled state did, the hand-assembled state was wrong — that is the entire point of this work. **Do not re-accept the snapshot.** Stop, investigate, and report it as a finding. This is the highest-value possible outcome of the package and the one most easily destroyed by a reflexive `cargo insta accept`.
- **KB #398 is the governing idea here** — prove a test discriminates by breaking the thing it claims to pin. A converted test should fail if you break the transition; the pre-conversion version would not have.
- **KB #201:** caches over `board.tasks`/`board.epics` go stale when tests mutate those fields directly. Conversions that replace direct mutation with driven input will *reduce* this class of fragility — worth noting in the triage where it applies.
- **KB #427:** overlay assertions that search the rendered buffer scan the whole terminal, not the overlay's rect, so a short needle can be satisfied by the board's own hint bar. If a conversion touches overlay assertions, tighten the needle at the same time.
- Some states have no key path — a mode only reachable via an MCP notification, say. Those legitimately stay as direct assignment; add a one-line comment stating why, so the next reader doesn't "fix" it.
- No behaviour change, so no Allium spec edits expected.

## Verification

- [ ] The seed-vs-drive rule is written in `docs/testing.md` and reads unambiguously
- [ ] `cargo test` green — redirect, don't pipe
- [ ] `cargo clippy --all-targets -- -D warnings` clean
- [ ] **No snapshot silently re-accepted.** Any `.snap` change is investigated and reported, not absorbed
- [ ] The converted file's assignment count is materially down; the repo-wide ratio has moved
- [ ] At least one converted test verified to fail when its transition is broken, then restored
- [ ] The triage table covers every file under `src/tui/tests/` with a count and a cost estimate
- [ ] Follow-up tasks filed for the remaining files, sized from conversion #1's actual cost
