# TUI Structural Improvements

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:executing-plans to implement this plan task-by-task.

**Goal:** Reduce complexity in the TUI layer by splitting the App god object, grouping the Message enum, and pre-computing repeated subtask statistics.

## Context

This work package addresses findings from a code review. The TUI layer is the primary complexity hotspot (~10K LOC across 3 files). The `App` struct manages 20 fields spanning board state, input state, agent tracking, and integrations. The `Message` enum has 60+ variants making state transitions hard to reason about.

## Findings

### :bulb: App god object (`src/tui/mod.rs:37`)

**Issue:** `App` has 20 fields covering tasks, epics, view modes, input state, agent tracking, archive state, filter state, review board, security board, usage stats, and merge queues. This violates single responsibility — every new feature adds more fields to this one struct.

**Fix:** Extract focused sub-structs:
```rust
pub struct App {
    board: BoardState,           // tasks, epics, selections, view_mode
    input: InputState,           // mode, cursor, text buffers
    agents: AgentState,          // tracking, tmux_outputs, merge_queue
    integrations: IntegrationState, // review, security, usage, filters
}
```
Keep `pub(in crate::tui)` visibility on the sub-structs. Update accessors and `update()` to delegate to the appropriate sub-struct. This is a large refactor — consider doing it incrementally (extract one sub-struct at a time).

### :bulb: Message enum has 60+ variants (`src/tui/types.rs:127`)

**Issue:** The `Message` enum is the single dispatch target for all state transitions. With 60+ variants, understanding which messages are valid in which view mode requires reading the entire `update()` function.

**Fix:** Group into logical sub-enums:
```rust
pub enum Message {
    Board(BoardMessage),    // task/epic selection, column changes
    Input(InputMessage),    // text entry, mode switches
    Agent(AgentMessage),    // dispatch, tracking, output capture
    System(SystemMessage),  // refresh, tick, error, status
}
```
Update `update()` to delegate per group. This improves readability without changing behavior.

### :bulb: Pre-compute subtask status counts (`src/tui/ui.rs:660-690`)

**Issue:** `render_columns()` and `render_epic_item()` filter the subtask list multiple times per frame to count statuses. The same filter/clone/count pattern appears 3+ times for each epic rendered.

**Fix:** Pre-compute a `HashMap<EpicId, SubtaskStats>` once per frame (or per tick in `App` state) and pass it to rendering functions. This eliminates repeated O(n) scans.

## Changes

| File | Change |
|------|--------|
| `src/tui/mod.rs` | Extract sub-structs from App, update update() delegation |
| `src/tui/types.rs` | Group Message into sub-enums, update pattern matches |
| `src/tui/ui.rs` | Accept pre-computed SubtaskStats, remove inline filtering |
| `src/tui/input.rs` | Update field access to use sub-struct paths |
| `src/tui/tests.rs` | Update test helpers and assertions for new struct layout |
| `src/runtime.rs` | Update App field access via new sub-struct paths |

## Verification

- [ ] Run existing tests — all pass (this is a pure refactor)
- [ ] TUI renders identically before and after
- [ ] No new clippy warnings
- [ ] `cargo clippy -- -D warnings` passes
