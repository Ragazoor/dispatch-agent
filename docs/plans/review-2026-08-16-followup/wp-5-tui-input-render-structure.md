# WP-5 — TUI Input & Render Structure

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:executing-plans to implement this plan task-by-task.

**Goal:** Turn the 300-line keybinding match into a declarative table so the keybinding↔telemetry pairing is structural, extract the render-context struct, and delete the clearest duplication in the repo.

## Context

From the follow-up codebase review at `4bf19b04` (`docs/plans/2026-08-16-4220-codebase-review-followup.md`, smell F, carried M3, and section 5).

`handle_key_board_normal` is the longest function in the codebase at 300 lines — but with only **12 branches**, which is the tell: it is a dispatch table wearing a function's clothes. The review explicitly recommends **against** splitting it by line count. The value here is entirely in making the keybinding↔telemetry-name pairing structural rather than per-arm discipline.

That pairing matters because of knowledge-base learning **#88 (↑15)**: remapping or removing a TUI keybinding touches roughly six surfaces — input handler, spec, two footer hint bars, help popup, reference doc — plus rendering-assertion tests and many footer-bar snapshots. A keymap table cannot collapse all six, but it makes the first one a single-line edit and removes the class of bug where a new binding silently ships with a copy-pasted telemetry name from the arm above it.

## Findings

### 💡 `handle_key_board_normal` is a 300-line hand-rolled dispatch table

**Issue:** Roughly 40 arms of the identical shape

```rust
KeyCode::Char('E') => self.dispatch_keyed(
    Message::Epic(crate::tui::messages::EpicMessage::StartNew),
    "create_epic",
    "E",
),
```

The fully-qualified `crate::tui::messages::…` path is repeated **40 times** in a file that imports nothing from `messages`. Nothing structurally ties a `KeyCode` to its telemetry name — that pairing is maintained by eye, forty times.

**Fix:** A `keymap!` macro (or a `const` table) over `(KeyCode, Message, telemetry_name)`. The repo already has this pattern six times over — `mcp_tools!`, `patch_struct!`, `define_str_enum!`, `service_api!`, `mcp_args!`, `set_field!` — so follow the closest existing one rather than inventing a new style. Read the chosen macro's doc comment first; several of them document non-obvious `macro_rules!` hazards.

Arms that are **not** pure `(key → message)` — the `gg` chord pre-check, `'q'` branching on `ViewMode`, `'/'` mutating `search.saved` inline, `'L'` checking `selected_epic_id()` — stay as hand-written arms. Do not contort the table to absorb them; a table plus five exceptions is the honest shape.

### 💡 Render helpers thread an unnamed context through wide signatures

**Issue:** Three helpers still take 6–7 positional parameters: `repo_picker_lines` (7), `render_scroll_indicators` (6), `render_task_prompt` (6). They thread the same unnamed cluster — lines, buffer, cursor, height offset, area height, hint — from caller to caller.

**Fix:** Extract a render-context struct. This continues work already begun in commit `8709900b` ("one centred-rect helper, one overlay frame, one row budget"), so match that commit's conventions rather than establishing new ones.

### 💡 `handle_navigate_row_first` / `_last` are 25 duplicated lines

**Issue:** `src/tui/update/navigation.rs` — the two functions are line-for-line identical except that one sets row `0` and the other `count - 1`. This is the clearest single duplication in the repo (which otherwise has **zero** cross-file duplication).

**Fix:** Extract a shared helper parameterized by which end to select — e.g. an `enum RowEnd { First, Last }` or a closure `fn(usize) -> usize` applied to the count. Keep both public entry points; only the body is shared.

## Changes

| File | Change |
|------|--------|
| `src/tui/input/normal.rs` | Introduce the keymap table; convert the ~35 pure arms; keep the ~5 conditional arms hand-written; add the missing `use` imports so no fully-qualified path remains |
| `src/tui/ui/input_form.rs` | Extract the render-context struct; convert `repo_picker_lines` |
| `src/tui/ui/kanban/columns.rs` | Convert `render_scroll_indicators` to the context struct |
| `src/dispatch/prompts.rs` | Convert `render_task_prompt` if it shares the same cluster; skip if its parameters are unrelated |
| `src/tui/update/navigation.rs` | Extract the shared body of `handle_navigate_row_first` / `_last` |

## Implementation notes — do these in order, and commit between them

The three findings are independent. Do them as **three separate commits**, smallest first: navigation dedupe → render-context struct → keymap table. If the keymap work turns out larger than expected, the first two have already landed.

- **Zero behaviour change in all three.** Every key must still produce the same `Message` and the same telemetry name.
- **The snapshot suite is your safety net and your hazard.** There are 59 snapshots, many of them footer/hint bars that a keybinding change would touch. If a snapshot changes, that is a **behaviour regression, not a snapshot to re-accept** — this package changes no rendering. KB #88 warns that many footer-bar snapshots are involved; blind `cargo insta accept` here would silently ship a real bug.
- **Prove the keymap table discriminates.** Per KB #398: after building it, remove one entry and confirm a test fails. If nothing fails, the table's arms are untested and you have converted tested code into untested code — surface that rather than proceeding.
- **Telemetry names are the point.** Before converting, capture the current 40 `(key, telemetry_name)` pairs to a scratch file. After converting, extract them again and diff. A silently changed telemetry name corrupts the keybinding-usage data that pruning passes read — see the keybinding-telemetry section of `docs/conventions.md`.
- **Watch `macro_rules!` hazards.** KB #420: in a macro field list, a marker keyword following an attribute repetition must be captured as `ident`, not `tt` — a `tt` matches `#` and makes the rule locally ambiguous.
- Consult the relevant Allium spec for the board's key surface before starting. Behaviour is unchanged, so no spec edit is expected — but if the spec and the arms you are converting disagree, **stop and ask**.
- Watch selector-style specificity of a different kind: `src/tui/input.rs` also matches `InputMode` in 51 places. This package does **not** touch `InputMode` — that is WP-7.

## Verification

- [ ] `cargo test` green — redirect, don't pipe
- [ ] `cargo clippy --all-targets -- -D warnings` clean
- [ ] **No snapshot changed.** `git status` shows no modified `.snap` and no `.snap.new` anywhere
- [ ] The captured `(key, telemetry_name)` pair list is byte-identical before and after
- [ ] `rg -c 'crate::tui::messages::' src/tui/input/normal.rs` is materially lower (target: 0)
- [ ] Removing one keymap entry causes a test failure; restored afterwards
- [ ] Manual smoke: `cargo run -- --db /tmp/claude-1000/scratch.db tui` from inside tmux; exercise `j k h l [ ] J K n c N E f / q` and the `gg` chord
