# Input Form Duplication and Style Params

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:executing-plans to implement this plan task-by-task.

**Goal:** Remove the repeated draft-reading preamble in the input form and bundle the three `Style` values that 11 functions thread individually.

## Context

This work package addresses findings from the whole-repository codebase review of 2026-08-28 (`docs/plans/2026-08-28-codebase-review.md`, sections 5.4 and 4 "long parameter lists").

Both findings have the same root cause: `src/tui/ui/input_form.rs` renders a multi-step form as one function per step, and each function independently re-derives the same context. The codebase already has the fix pattern in two places — `RepoListCtx` (`src/tui/ui/input_form.rs:31`) and `ColRenderCtx` (`src/tui/ui/kanban/cards.rs:397`) — it just has not been applied here.

This is render-path code. `CLAUDE.md`'s "Rendering purity" rule applies: no side effects, and the render path must never block on filesystem or DB work.

## Findings

### 💡 Repeated draft-reading preamble (`src/tui/ui/input_form.rs`, 11 sites across 6 functions)

**Issue:** `app.input.task_draft` is read at lines 130, 153, 159, 185, 191, 198, 246, 252, 259, 271 and 313. Six functions each open with some subset of the same three-field extraction:

```rust
let title = app.input.task_draft.as_ref().map(|d| d.title.as_str()).unwrap_or("");
let tag = app.input.task_draft.as_ref().and_then(|d| d.tag.as_ref())
    .map(|t| t.to_string()).unwrap_or_else(|| "none".to_string());
let description = app.input.task_draft.as_ref().map(|d| d.description.as_str()).unwrap_or("");
```

Formatted across multiple lines each, this is 12+ duplicated lines in `input_description_lines`, `input_repo_path_lines`, `input_base_branch_lines` and neighbours. The `"none"` fallback for a missing tag and the `""` fallback for a missing title are **presentation decisions** currently restated in every function — so they can silently diverge.

`input_repo_path_lines` additionally derives a truncated one-line description:

```rust
let desc_first_line = description.lines().next().unwrap_or("");
let desc_display = if description.contains('\n') { format!("{desc_first_line} ...") } else { desc_first_line.to_string() };
```

**Fix:** Add a `DraftSummary` view struct near `RepoListCtx`:

```rust
/// The already-answered form fields, rendered once for every step that shows
/// them back to the user. Owns the presentation fallbacks (`""` for a missing
/// title, `"none"` for a missing tag) so they cannot diverge per step.
struct DraftSummary {
    title: String,
    tag: String,
    description: String,
    /// `description`'s first line, suffixed `" ..."` when more lines follow.
    description_oneline: String,
}

impl DraftSummary {
    fn from_input(input: &InputState) -> Self { … }
}
```

Take `&InputState`, not `&App` — it is all the data needed, and the narrower borrow is what lets this be unit-tested without building a whole `App`.

Note line 313 (`answered_step_lines`) already does the `let draft = app.input.task_draft.as_ref()` bind-once thing. Fold it into `DraftSummary` too, or leave it — judgement call, but prefer folding so there is exactly one reader.

### 💡 Three `Style` values threaded individually through 11 functions (`src/tui/ui/input_form.rs`)

**Issue:** Eleven functions take 2–3 bare `Style` parameters:

| Function | Style params |
|---|---|
| `input_title_lines` | `active`, `hint` |
| `input_tag_lines` | `completed`, `active`, `hint` |
| `input_description_lines` | `completed`, `active`, `hint` |
| `input_repo_path_lines` | `completed`, `active`, `hint` |
| `input_base_branch_lines` | `completed`, `active`, `hint` |
| `input_wrap_up_mode_lines` | `completed`, `active`, `hint` |
| `repo_picker_lines` | `active`, `hint` |
| `main_session_dir_lines` | `active`, `hint` |
| `quick_dispatch_lines` | `active`, `hint` |
| `input_epic_title_lines` | `active`, `hint` |
| `input_epic_description_lines` | `completed`, `active`, `hint` |

They are positional and same-typed, so a transposed `active`/`hint` pair compiles cleanly and only shows up as wrong colours on screen. `repo_picker_lines` reaches 8 parameters total; `run_setup_in` elsewhere in the tree needed `#[allow(clippy::too_many_arguments)]` for the same reason.

**Fix:** Introduce a `FormStyles` struct and pass `&FormStyles`:

```rust
pub(in crate::tui::ui) struct FormStyles {
    pub completed: Style,
    pub active: Style,
    pub hint: Style,
}
```

Build it once at the call site in `src/tui/ui/kanban/mod.rs::render_input_form` (which is itself cyc~26) and thread the reference. Functions that only need two of the three still take the whole struct — uniformity beats minimalism here, and it means adding a fourth style later is a one-line change.

Named fields also make the transposition bug impossible, which is the actual point.

### Out of scope — deliberately

- **`render_scroll_indicators` (`src/tui/ui/kanban/columns.rs:482`, 7 params)** takes `frame`, `list_state`, `item_heights`, `inner`, `col_area`, `indicator_color` — geometry and state, not styles. Bundling those would hide real arguments behind a bag. Leave it.
- **`render_epic_item` (`src/tui/ui/kanban/cards.rs:655`, 7 params)** and **`build_task_list_item` (`:521`)** already take `ctx: &ColRenderCtx`. They are at 7 params *because* they legitimately need `epic`, `is_cursor`, `app`, `epic_stats`, `status`. Nothing to bundle.
- **`frame_card` (`:439`)** takes `col_width`, `frame_color`, `ground` — arguably `ColRenderCtx` minus `color`. Only refactor if it falls out naturally; do not force it.

The point of this work package is `input_form.rs`, where the repetition is mechanical and the params are same-typed. Do not let it sprawl into a general render-signature rewrite.

## Changes

| File | Change |
|------|--------|
| `src/tui/ui/input_form.rs` | Add `DraftSummary` with `from_input(&InputState)`, owning the `""` / `"none"` fallbacks and the one-line description truncation |
| `src/tui/ui/input_form.rs` | Replace the 11 `task_draft` read sites with `DraftSummary` usage; fold `answered_step_lines`' bind into it |
| `src/tui/ui/input_form.rs` | Add `FormStyles { completed, active, hint }`; convert all 11 listed functions to take `&FormStyles` |
| `src/tui/ui/kanban/mod.rs` | Build `FormStyles` once in `render_input_form` and pass it down |
| `src/tui/ui/mod.rs` | Re-export `FormStyles` if the visibility scope requires it |

## Verification

- [ ] `cargo test` — all pass. The 59 `insta` snapshots are the real safety net here: **any** snapshot diff means you changed rendered output, which this refactor must not do. Do not accept a snapshot change without understanding exactly which character moved
- [ ] `cargo clippy --all-targets -- -D warnings` — clean
- [ ] `cargo fmt` before committing
- [ ] Manually exercise the form end to end: `cargo run -- --db /tmp/scratch.db tui` (needs a tmux server already running), press `n` and step through title → tag → description → repo path → base branch → wrap-up mode. Confirm the answered steps above the cursor still render in the `completed` style and the hint line still renders in `hint`
- [ ] Confirm the `"none"` tag fallback and the `" ..."` multi-line description suffix still appear — those are the two behaviours most likely to be quietly dropped in the extraction
- [ ] Confirm no filesystem or DB call entered `DraftSummary::from_input` — `CLAUDE.md`'s rendering-purity rule
