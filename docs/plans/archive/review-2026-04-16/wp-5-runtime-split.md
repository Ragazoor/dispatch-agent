# Runtime: Split into submodules

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:executing-plans to implement this plan task-by-task.

**Goal:** Split the 4,246-line `runtime.rs` into domain-grouped submodules under `src/runtime/`.

## Context

This work package addresses findings from a code review. `runtime.rs` is the largest non-test file and contains 46 `exec_*` methods that naturally group by domain. The split is purely structural — no logic changes, no interface changes.

## Findings

### :bulb: `runtime.rs` at 4,246 lines is a navigation tax (`src/runtime.rs`)

**Issue:** The file contains the `TuiRuntime` struct, the event loop, 46 `exec_*` methods, `execute_commands` routing, and 2,349 lines of tests. While internally well-structured, finding a specific method requires knowing what you're looking for. The `exec_*` methods naturally group along the same boundaries as the `Command` variants.

**Fix:** Convert `src/runtime.rs` into `src/runtime/mod.rs` + domain submodules. Each submodule contains `impl TuiRuntime` blocks for its domain. The `execute_commands` match stays in `mod.rs` as the routing table.

## Module Plan

| File | Methods | Purpose |
|------|---------|---------|
| `runtime/mod.rs` | `TuiRuntime` struct, `new()`, `run_loop()`, `execute_commands()` | Core struct, event loop, command routing |
| `runtime/tasks.rs` | `exec_insert_task`, `exec_delete_task`, `exec_persist_task`, `exec_patch_sub_status`, `exec_quick_dispatch`, `exec_dispatch_agent`, `exec_capture_tmux`, `exec_refresh_from_db`, `exec_save_repo_path`, `exec_delete_repo_path`, `exec_finish` | Task lifecycle |
| `runtime/epics.rs` | `exec_insert_epic`, `exec_delete_epic`, `exec_persist_epic`, `exec_edit_epic_in_editor`, `exec_refresh_epics_from_db`, `exec_dispatch_epic`, `exec_toggle_epic_auto_dispatch` | Epic lifecycle |
| `runtime/split.rs` | `exec_enter_split_mode`, `exec_enter_split_mode_with_task`, `exec_exit_split_mode`, `exec_swap_split_pane`, `exec_check_split_pane`, `exec_respawn_split_pane`, `exec_jump_to_tmux`, `exec_kill_tmux_window` | Split pane and tmux navigation |
| `runtime/pr.rs` | `exec_create_pr`, `exec_check_pr_status`, `exec_merge_pr`, `exec_fetch_prs`, `exec_persist_prs`, `exec_approve_bot_pr`, `exec_merge_bot_pr` | PR and merge operations |
| `runtime/agents.rs` | `exec_dispatch_review_agent`, `exec_dispatch_fix_agent` | Review and fix agent dispatch |
| `runtime/security.rs` | `exec_fetch_security_alerts`, `exec_persist_security_alerts` | Security alert operations |
| `runtime/settings.rs` | `exec_persist_setting`, `exec_persist_string_setting`, `exec_persist_filter_preset`, `exec_delete_filter_preset`, `exec_send_notification`, `exec_refresh_usage_from_db`, `exec_open_in_browser`, `exec_edit_in_editor`, `exec_description_editor`, `exec_edit_github_queries` | Settings, editor, misc IO |
| `runtime/tests.rs` | All existing `#[cfg(test)]` tests | Test module |

## Approach

1. Rename `src/runtime.rs` to `src/runtime/mod.rs`
2. For each submodule, create the file with `use super::*;` to inherit imports
3. Move the `exec_*` methods as `impl TuiRuntime` blocks into the appropriate submodule
4. Keep `execute_commands` in `mod.rs` — it calls methods across all submodules
5. Move the `#[cfg(test)] mod tests` block to `runtime/tests.rs`
6. Add `mod tasks; mod epics; mod split; mod pr; mod agents; mod security; mod settings;` declarations to `mod.rs`
7. Any inlined command handling in `execute_commands` that doesn't call an `exec_*` method stays in `mod.rs`

## Changes

| File | Change |
|------|--------|
| `src/runtime.rs` | Rename to `src/runtime/mod.rs` |
| `src/runtime/mod.rs` | Keep struct, new(), run_loop(), execute_commands(). Add mod declarations. |
| `src/runtime/tasks.rs` | Extract 11 task exec_ methods |
| `src/runtime/epics.rs` | Extract 7 epic exec_ methods |
| `src/runtime/split.rs` | Extract 8 split/tmux exec_ methods |
| `src/runtime/pr.rs` | Extract 7 PR exec_ methods |
| `src/runtime/agents.rs` | Extract 2 agent dispatch exec_ methods |
| `src/runtime/security.rs` | Extract 2 security exec_ methods |
| `src/runtime/settings.rs` | Extract 10 settings/editor/misc exec_ methods |
| `src/runtime/tests.rs` | Extract test module (~2,349 lines) |

## Verification

- [ ] `cargo test` — full suite passes (no logic changes)
- [ ] `cargo clippy -- -D warnings` passes
- [ ] `cargo fmt --check` passes
- [ ] `wc -l src/runtime/mod.rs` is under 500 lines
- [ ] No `exec_*` methods remain in `mod.rs` (all moved to submodules)
