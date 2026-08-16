# Review Tab Debugging & UX Plan

**Goal:** Fix the review board so users can tell when a fetch is in progress and why nothing is showing (loading indicator + persistent error display).

**Architecture:** The review board state lives in `App` (`src/tui/mod.rs`). Rendering is in `src/tui/ui.rs`. A new `last_review_error` field stores the last fetch error persistently (not auto-clearing like the status bar). A loading indicator replaces the empty-state message while a fetch is in progress.

**Tech Stack:** Rust, Ratatui TUI

---

## Root Cause

Two UX bugs conspire to make the review board appear broken:

1. **No loading indicator**: `render_review_board()` (`src/tui/ui.rs`) shows "No PRs awaiting your review" when `review_prs` is empty — the same message shown during the initial fetch. The user cannot tell if the data is loading or genuinely absent.

2. **Errors are transient**: `handle_review_prs_fetch_failed()` (`src/tui/mod.rs`) calls `set_status()`, which auto-clears after 5 seconds. If `gh` auth has expired or `gh` is not in PATH, the error appears briefly and vanishes.

A secondary pre-existing bug:
3. **`clamp_review_selection` off-by-one** (`src/tui/mod.rs`): uses `[usize; 3]` but `ReviewDecision::COLUMN_COUNT` is 4 — the Approved column (index 3) is never clamped.

---

## Changes Made

| File | Change |
|---|---|
| `src/tui/mod.rs` | Added `last_review_error: Option<String>` field; updated handlers; added accessor; fixed clamp_review_selection |
| `src/tui/ui.rs` | Shows "Fetching reviews..." when loading; shows persistent error in status bar |
| `src/tui/tests.rs` | Updated 1 existing test; added 4 new tests |
