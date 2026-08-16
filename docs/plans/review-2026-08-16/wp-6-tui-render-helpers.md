# TUI Render Helpers & Layout Dedup

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:executing-plans to implement this plan task-by-task.

**Goal:** Replace six copies of centred-rect arithmetic with one helper, extract a render-context struct from the widest signatures, and remove a dual-maintained layout constant that nothing checks.

## Context

This work package addresses findings from the 2026-08-16 codebase review
(`docs/plans/2026-08-16-codebase-review.md`, commit `c05f512c`).

`src/tui/ui/` is render code, so per `CLAUDE.md` this is **not** a
coverage-chasing exercise. The value here is removing hand-counted magic offsets
that can silently disagree.

**Snapshot discipline:** this work package will touch rendering. Expect snapshot
diffs and review them carefully — several card snapshots are near-duplicates
differing by one badge, which makes blind re-acceptance easy. Do **not** change
the 120×40 `TestBackend` size. Always `rm src/tui/tests/snapshots/*.snap.new`
after accepting.

## Findings

### 💡 Centred-rect computation copy-pasted six times

**Issue:** There is **no `centered_rect` helper anywhere** in the codebase.
The same arithmetic appears at:

- `src/tui/ui/kanban/popups/error.rs:21-23`
- `src/tui/ui/kanban/popups/help.rs:21-23`
- `src/tui/ui/kanban/popups/repo_filter.rs:46-48`
- `src/tui/ui/todos.rs:32`
- `src/tui/ui/kanban/popups/reparent_epic.rs:85`

The scroll-window formula is likewise duplicated verbatim at
`src/tui/ui/input_form.rs:85` and `src/tui/ui/kanban/popups/repo_filter.rs:61`.

**Fix:** Add `centered_rect` to `src/tui/ui/shared.rs` and route all sites
through it. Same for the scroll-window formula.

### 💡 A dual-maintained layout constant with no check

**Issue:** `src/tui/ui/kanban/popups/repo_filter.rs` computes the same layout
budget twice, from opposite directions:

- `:42` — `+7: blank(1) + toggle_row(1) + …`
- `:55` — `non_repo_lines = preset_lines + input_line + 5`

These two constants must agree, and **nothing checks that they do**. A future
edit to one is a silent rendering bug.

**Fix:** Derive one from the other, or compute both from a single named
constant. If they genuinely cannot be unified, add a test asserting they agree.

### 💡 `render_repo_filter_overlay` is 232 lines (`src/tui/ui/kanban/popups/repo_filter.rs:14`)

**Issue:** The longest accidental-complexity function in the codebase —
straight-line layout arithmetic with four `InputMode` variants interleaved into
one body. Its siblings repeat the shape: `help.rs:14` (165 lines),
`task_detail.rs:18` (127), `todos.rs:19` (127).

**Fix:** This is a missing shared overlay-frame abstraction, not four
irreducible problems. Extract the common frame (centred rect + border + title +
scroll) and let each popup supply only its content. Split the four `InputMode`
branches in `repo_filter.rs` into separate functions.

### 💡 Seven wide render signatures share an unnamed context

**Issue:** 7 of the codebase's 16 widest signatures are in `src/tui/ui/`, all
threading variations of `(lines, buffer, cursor, height_offset, area_height, hint, …)`:

- `src/tui/ui/input_form.rs:31` (8 params), `:332` (8), `:71` (7)
- `src/tui/ui/shared.rs:352` `caret_field_line` (7)
- `src/tui/ui/kanban/columns.rs:486` `render_scroll_indicators` (7)
- `src/tui/ui/kanban/cards.rs:590` `render_epic_item` (7), `:462` `build_task_list_item` (7)

**Fix:** Extract a render-context struct. Do this **after** the overlay-frame
extraction — the frame abstraction may absorb several of these parameters, so
extracting the struct first risks designing it around a shape that is about to
change.

## Changes

| File | Change |
|------|--------|
| `src/tui/ui/shared.rs` | Add `centered_rect` and a shared scroll-window helper; add the render-context struct |
| `src/tui/ui/kanban/popups/error.rs` | Use `centered_rect` |
| `src/tui/ui/kanban/popups/help.rs` | Use `centered_rect`; adopt the shared overlay frame |
| `src/tui/ui/kanban/popups/repo_filter.rs` | Use `centered_rect` + shared scroll helper; unify the `:42`/`:55` layout constants; split the four `InputMode` branches out of the 232-line body |
| `src/tui/ui/kanban/popups/reparent_epic.rs` | Use `centered_rect` |
| `src/tui/ui/kanban/popups/task_detail.rs` | Adopt the shared overlay frame |
| `src/tui/ui/todos.rs` | Use `centered_rect`; adopt the shared overlay frame |
| `src/tui/ui/input_form.rs` | Use the shared scroll helper; adopt the render-context struct |
| `src/tui/ui/kanban/columns.rs` | `render_scroll_indicators` takes the render-context struct |
| `src/tui/ui/kanban/cards.rs` | `render_epic_item` / `build_task_list_item` take the render-context struct |

## Verification

- [ ] Run existing tests — all pass (`cargo test`)
- [ ] `cargo test tui::tests` passes
- [ ] `cargo test tui::tests::snapshots` — **snapshots should be unchanged**; this is a pure refactor, so any diff means a real rendering change. Investigate rather than accept
- [ ] If a snapshot diff is genuinely intended, review each one individually (`cargo insta review`, never `INSTA_UPDATE=always`) and state in the commit why it changed
- [ ] `rm src/tui/tests/snapshots/*.snap.new` — no stray files left behind
- [ ] `TestBackend` remains 120×40
- [ ] Add a test asserting the `repo_filter.rs` layout constants agree (or that the single derived constant is used)
- [ ] Manually run `cargo run -- --db /tmp/scratch.db tui` and open each affected popup (help, error, repo filter, todos, task detail, reparent epic) to confirm they render correctly at a non-default terminal size
- [ ] `cargo clippy --all-targets -- -D warnings` clean
