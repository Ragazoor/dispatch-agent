# TUI App Decomposition

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:executing-plans to implement this plan task-by-task.

**Goal:** Break the `App` god-struct into cohesive sub-structs, de-duplicate the column-builder filter/sort pipeline, and eliminate the six `unreachable!` arms via a `BoardViewMode` sub-enum.

## Context

This work package addresses findings from a code review in `src/tui/mod.rs` (~1606 lines). This is
the largest and riskiest package — it touches the coupling hub of the TUI. Land it incrementally
(one sub-struct extraction at a time), leaning on the extensive scenario + snapshot tests as the
safety net. Adding an `InputMode` variant re-serializes every key-sequence snapshot — this work
should NOT add modes, but be aware the snapshots are sensitive.

## Findings

### 💡 `App` god-struct (`src/tui/mod.rs:97-196`)

**Issue:** ~40 fields across ~12 responsibilities — genuine board state (`board`, `agents`) mixed with
a long tail of transient UI/interaction state (`pending_g`, `pending_todo_edit`, `pending_todo_delete`,
`pending_todo_link`, `move_task_picker`, `reparent_picker`, `managed_feed_config`) plus five layout
caches + two fingerprints. The impl runs lines 300–1603. Every handler takes `&mut App`. This is the
dominant maintainability risk and forces the hand-rolled cache-reactivity to live on the struct.

**Fix:** Extract the loose `pending_*`/picker fields into an `InteractionState` (or similar) sub-struct.
Move the five derived caches + fingerprints into a dedicated view/derived-state type that owns its own
coherence, retiring the need for the god-struct to carry both source and derived data. Do this
incrementally; keep scenario/snapshot tests green at each step.

### 💡 Column filter+sort pipeline duplicated across 3-4 builders (`src/tui/mod.rs:1029,1184,1234`)

**Issue:** `column_items_for_status_with_view_tasks` (~140 lines, two barely-shared branches),
`column_item_count` (`:1184`), and `column_items_for_visual_column` (`:1234`) each re-do their own
epic filter + sort. An epic-visibility rule change must be made in every copy.

**Fix:** Extract one shared filter+sort function that all builders call. Note per CLAUDE.md that
`column_items_for_status` is test-only — verify which builders are production vs test before merging.

### 💡 Six `unreachable!` arms from a missing sub-enum (`src/tui/mod.rs:791,1136,1210,1249,1414,…`)

**Issue:** Six copies of `unreachable!("effective_view_mode never returns TaskDetail/Learnings/Todos")`
— a missing `BoardViewMode` sub-enum forces every caller to carry impossible arms.

**Fix:** Introduce a `BoardViewMode` enum containing only the board-column-relevant variants; have
`effective_view_mode` (or a new accessor) return it, so callers match exhaustively with no
`unreachable!`. Also addresses the untyped column arithmetic (`col == 0`, `col - 1`, `COLUMN_COUNT + 1`)
scattered across `selected_column_item:1315`, `sync_board_selection:1400`,
`update_anchor_from_current:1371` — consider a small typed column index alongside.

## Changes

| File | Change |
|------|--------|
| `src/tui/mod.rs` | Extract `InteractionState` sub-struct from loose `pending_*`/picker fields. |
| `src/tui/mod.rs` | Move five layout caches + fingerprints into a derived-state type that owns its coherence. |
| `src/tui/mod.rs` | Extract shared column filter+sort; dedup across the 3-4 builders. |
| `src/tui/mod.rs` (+ callers) | Add `BoardViewMode` sub-enum; delete the six `unreachable!` arms; consider a typed column index. |

## Verification

- [ ] Land incrementally; `cargo test tui::tests::scenarios` green after each extraction
- [ ] `cargo test tui::tests::snapshots` — no layout/content diffs (re-accept only serialized-state changes, delete `.snap.new`)
- [ ] Layout-cache self-heal behaviour preserved (see architecture.md layout-cache-coherence section)
- [ ] `cargo test && ./scripts/check-doc-paths.sh` — all pass
- [ ] `cargo clippy --all-targets -- -D warnings` clean
