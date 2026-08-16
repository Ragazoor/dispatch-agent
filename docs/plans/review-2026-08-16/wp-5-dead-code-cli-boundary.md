# Dead Code & CLI Boundary

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:executing-plans to implement this plan task-by-task.

**Goal:** Delete three verified-dead functions and the vestigial CLI task-mutation subcommands, and parse the CLI boundary into enums instead of raw strings.

## Context

This work package addresses findings from the 2026-08-16 codebase review
(`docs/plans/2026-08-16-codebase-review.md`, commit `c05f512c`).

All deletions below were verified by grep during the review, not inferred.

## Findings

### 💡 Three confirmed-dead functions

**Issue:** Each of these greps to exactly **one** hit across `src/` and `tests/` —
its own definition. No callers, no tests:

- `src/models/learnings.rs:41` — `pub fn display_label`
- `src/tui/mod.rs:749` — `pub fn split_pinned_task_id`
- `src/tui/types.rs:675` — `pub fn list_state_index`

(No stray `#[allow(dead_code)]` exists in production code — that convention is
being honoured, which is why these stood out.)

**Fix:** Delete all three. Re-run the grep first to confirm nothing has landed
on `main` since the review that uses them.

### 💡 Vestigial CLI subcommands

**Issue:** `dispatch list` and `dispatch update` appear only in `src/main.rs` and
`docs/reference.md`. They were designed as hook-script entry points, but the
installed hooks now forward to the dedicated `hook-*` subcommands — verified at
`src/setup/hooks.rs:106` (`hook-file-event`) and `:127` (`hook-peer-message`).
Agents use the MCP equivalents. The only other references are in historical plan
documents under `docs/superpowers/plans/`.

**Fix:** Remove both subcommands and their handlers. Note that
`src/service/tasks/crud.rs:471` carries a doc comment citing `dispatch update`
as its caller — update it. Prefer deletion over maintenance here: the standing
preference in this repo is to remove unused CLI task-mutation paths rather than
keep them alive.

**Check before deleting:** confirm no shipped hook template, skill, or script in
`plugin/` or `scripts/` invokes them (`grep -rn "dispatch list\|dispatch update" plugin/ scripts/ .githooks/`).

### 💡 CLI boundary is stringly-typed, violating the repo's own "Border parsing" rule

**Issue:** `Commands::Update { status: String }` (`src/main.rs:38`),
`HookSubagent { action: String }` (`:101`), and `HookShell { action: String }`
(`:124`) take raw strings and re-parse them by hand: `match action.as_str()` at
`src/main.rs:446` and `:518`, plus `if action == "start"` at `:452` — the same
literal matched twice within one function. Also `parse_status` at `src/main.rs:305`.

Compounding this, `cmd_update` (`src/main.rs:296`) takes **both**
`sub_status: Option<String>` and a redundant `needs_input: bool`, resolved by an
if/else-if precedence chain at `:307-316` — two spellings of one field.

**Fix:** Use clap's `ValueEnum` so parsing happens at the boundary as the
convention demands, and so `--help` lists the valid values. Collapse
`needs_input` into `sub_status`.

**Sequencing note:** if `dispatch update` is deleted above, its `status`/
`sub_status`/`needs_input` parsing goes with it — do the deletion first, then
apply `ValueEnum` to what remains (the `hook-*` actions).

## Changes

| File | Change |
|------|--------|
| `src/models/learnings.rs:41` | Delete `display_label` |
| `src/tui/mod.rs:749` | Delete `split_pinned_task_id` |
| `src/tui/types.rs:675` | Delete `list_state_index` |
| `src/main.rs` | Remove `Update` and `List` subcommands + handlers (`cmd_update`, `parse_status`); convert remaining `action: String` params (`:101`, `:124`) to `ValueEnum`; remove the hand-rolled `match action.as_str()` at `:446`/`:518` and `if action == "start"` at `:452` |
| `src/service/tasks/crud.rs:471` | Update the doc comment that cites `dispatch update` as a caller |
| `docs/reference.md` | Remove `dispatch list`/`update` from CLI Usage (coordinate with WP-4, which also edits this section) |
| `tests/cli.rs` | Remove tests for the deleted subcommands; add coverage for `ValueEnum` rejection of invalid actions |

## Verification

- [ ] Re-run the dead-code greps and confirm still zero callers before deleting: `grep -rn "display_label\|split_pinned_task_id\|list_state_index" src/ tests/`
- [ ] `grep -rn "dispatch list\|dispatch update" plugin/ scripts/ .githooks/ src/` returns nothing outside historical `docs/` after the change
- [ ] Run existing tests — all pass (`cargo test`)
- [ ] `cargo test --test cli` passes
- [ ] `cargo test --test githooks` passes — the hook path must be unaffected
- [ ] `dispatch --help` lists valid values for the converted enum args
- [ ] An invalid `hook-shell` / `hook-subagent` action is rejected by clap with a helpful message rather than falling through a hand-rolled `match`
- [ ] `cargo build` succeeds with no unused-import or dead-code warnings introduced
- [ ] `cargo clippy --all-targets -- -D warnings` clean
