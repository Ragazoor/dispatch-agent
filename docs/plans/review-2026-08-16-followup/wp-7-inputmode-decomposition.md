# WP-7 — `InputMode` Decomposition

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:executing-plans to implement this plan task-by-task.

**Goal:** Break the 36-variant `InputMode` into per-surface state so adding a modal stops costing edits across several files.

## Context

From the follow-up codebase review at `4bf19b04` (`docs/plans/2026-08-16-4220-codebase-review-followup.md`, carried finding L6).

Flagged in both reviews and unchanged: **36 variants**, matched exhaustively in three or more files (51 `InputMode::` references in `src/tui/input.rs`, 35 in `src/tui/ui/kanban/status_bar.rs`, 19 in `src/tui/ui/kanban/mod.rs`). The review's verdict: *"that is accidental coupling — modal state expressed as one flat enum instead of per-surface state."*

## ⚠️ This package starts with design, not code

**Do not open an editor on `src/tui/types.rs` first.** This is a cross-cutting change to interaction state, and this repo has two standing rules that both bite here:

1. **Behaviour changes start in the spec** — and per the user's standing preference, *UI and interaction behaviour is a first-class Allium surface, not a prose note*.
2. **Ambiguity is a stop condition, not a judgement call.** There is more than one defensible decomposition, and picking one silently is the failure mode this package must avoid.

The deliverable of Step 1 is a design document that the user approves *before* any test or code is written. If you reach the end of your session having produced only an approved design, that is a successful outcome for this package.

## The 36 variants, grouped as observed

A first-pass clustering, offered as input to the design — **not as the answer**:

| Cluster | Variants |
|---|---|
| Text entry | `SearchTasks`, `InputTitle`, `InputDescription`, `InputRepoPath`, `InputTag`, `InputEpicTitle`, `InputEpicDescription`, `InputPresetName`, `InputBaseBranch`, `InputWrapUpMode`, `MainSessionDir`, `TodoTitle`, `TodoQuickAdd` |
| Confirmations | `ConfirmDelete`, `ConfirmRetry`, `ConfirmArchive`, `ConfirmDone`, `ConfirmDetachTmux`, `ConfirmDeleteEpic`, `ConfirmArchiveEpic`, `ConfirmReparentEpic`, `ConfirmMoveTaskToEpic`, `ConfirmDeletePreset`, `ConfirmDeleteRepoPath`, `ConfirmQuit`, `ConfirmTrustRepo`, `ConfirmTrustRepoQuickDispatch`, `ConfirmDeleteTodo`, `ConfirmRepoSync` |
| Pickers | `QuickDispatch`, `ReparentEpic`, `MoveTaskToEpic`, `RepoFilter`, `LinkTodoToTask` |
| Other | `Normal`, `Help` |

Sixteen of thirty-six are confirmations of the shape "ask a yes/no question, then perform one action" — the most obvious candidate for collapsing into a single `Confirm(ConfirmAction)` variant. Thirteen are text entry differing mainly in which field the buffer lands in. Whether either collapse is *right* is the design question.

## Step 1 — Design (blocking)

Produce a design document under `docs/superpowers/specs/` covering:

- **The proposed shape.** Nested enums? A `Modal` struct with a payload? Per-surface state living on the surface's own struct? State each option's cost at the three exhaustive-match sites.
- **What the exhaustive matches become.** The current 36-arm matches in `input.rs` and `status_bar.rs` are the thing to improve; show what they look like after. If they stay 36 arms wide, the change has not paid for itself.
- **Migration path.** Can this land incrementally — one cluster at a time, compiling and green between steps — or is it one atomic change? Incremental is strongly preferred; if it cannot be, say why.
- **What `PendingAction` does.** It already collapsed four `pending_*` fields into one matchable value and is gated by `InputMode`. The two interact; the design must say how.
- **Test strategy.** 1,604 TUI tests reference this state heavily. How many break? Is there a shape that keeps them compiling?

Get explicit approval on this document before Step 2. Then follow the repo's ordering: **spec → tests → code.**

## Step 2 — Spec

Update the relevant `docs/specs/*.allium` surface to describe the decomposed interaction state. Use the `allium:tend` skill. Verify with `allium:weed`.

## Step 3 — Tests, then code

Per cluster, in the order the approved design sets: express the intended behaviour as tests, watch them fail, then implement.

## Changes (indicative — the design supersedes this)

| File | Change |
|------|--------|
| `src/tui/types.rs` | `InputMode` definition |
| `src/tui/input.rs` | 51 references, incl. an exhaustive match |
| `src/tui/ui/kanban/status_bar.rs` | 35 references, incl. an exhaustive match |
| `src/tui/ui/kanban/mod.rs` | 19 references |
| `src/tui/update/{epics,forms,repo_filter,move_task}.rs` | 9–14 references each |
| `src/tui/ui/kanban/popups/repo_filter.rs` | 9 references |
| `docs/specs/*.allium` | Interaction-state surface |
| `src/tui/tests/**` | Broad fallout |

## Implementation notes

- **Zero user-visible behaviour change.** Every modal must still open on the same key, render the same, and perform the same action. This is a representation change.
- **The snapshot suite is the tripwire.** 59 snapshots, many of them status-bar and footer renders driven off `InputMode`. A changed snapshot is a regression, not something to re-accept.
- **KB #201:** caches over `board.tasks`/`board.epics` go stale when tests mutate fields directly, causing widespread failures. The layout cache self-heals via fingerprint, but tests that set modal state directly are exactly the population most affected here — and WP-8 is trying to reduce that population. **Check whether WP-8 has landed**; doing WP-8 first would materially reduce this package's blast radius. Raise the sequencing with the user if both are still open.
- Expect this to exceed one session. Land the design, then the spec, then clusters incrementally.

## Verification

- [ ] Design document written and **explicitly approved** before code
- [ ] `allium:weed` reports spec and implementation aligned
- [ ] `cargo test` green — redirect, don't pipe
- [ ] `cargo clippy --all-targets -- -D warnings` clean
- [ ] **No snapshot changed**; no `.snap.new` files
- [ ] The exhaustive matches in `input.rs` and `status_bar.rs` are demonstrably narrower
- [ ] Manual smoke: open every modal from the board and confirm each renders and acts as before
