# WP3: Base Branch — TUI, Skill & Allium Spec

## Context

Third and final work package. Adds user-facing UI for setting `base_branch` during task creation and editing. Updates the wrap-up skill text and allium spec to reflect the new behavior.

## Files to Modify

| File | Change |
|------|--------|
| `src/tui/types.rs` | Add base_branch step to creation `InputMode` |
| `src/tui/input.rs` | Handle base_branch input in creation flow |
| `src/tui/ui.rs` | Render base_branch input field |
| `src/tui/tests.rs` | Test creation flow with base_branch |
| `src/editor.rs` | Include base_branch in editor template |
| `plugin/skills/wrap-up/SKILL.md` | Update rebase text to reference base_branch |
| `docs/specs/dispatch.allium` | Add base_branch to Task entity, update rules |

## Steps

### 1. TUI task creation (test first)

**Tests in `src/tui/tests.rs`:**
- Creation flow includes base_branch step after existing steps
- Pre-filled with default branch, Enter accepts it
- Typing replaces the default
- Created task has the specified base_branch

**Implement:**
- `src/tui/types.rs`: Add a base_branch step to the `InputMode` creation flow (look at how other creation steps like tag selection work)
- `src/tui/input.rs`: Handle key events for base_branch input:
  - Pre-fill input buffer with auto-detected default branch
  - Enter confirms current value
  - Typing replaces/edits the value
  - Esc cancels creation (consistent with other steps)
- `src/tui/ui.rs`: Render the base_branch input field with prompt like "Base branch:" showing the current value
- Pass the collected base_branch to `create_task()` instead of hardcoded `"main"`

### 2. TUI task editing (test first)

**Tests:**
- base_branch appears in editor template
- Edited base_branch is parsed and saved

**Implement:**
- `src/editor.rs`: Include `base_branch: {value}` in the editor template
- Parse base_branch from editor output, include in `TaskPatch`

### 3. Wrap-up skill update

**`plugin/skills/wrap-up/SKILL.md`:**
- Update the AskUserQuestion prompt: change "rebase onto main" to dynamically reference the task's base_branch (the agent reads it from `get_task` response)
- Update step descriptions to mention base_branch where relevant

### 4. Allium spec update

**`docs/specs/dispatch.allium`:**
- Add `base_branch: string` to Task entity (required, non-null)
- Update `CreateTask` rule: base_branch is set at creation, defaults to `config.default_branch`
- Update `DispatchTask`: worktree created from `task.base_branch`
- Update `WrapUpRebase`: rebases onto `task.base_branch`
- Update `WrapUpPr`: PR base is `task.base_branch`
- Update `FinishTaskSuccess`: merges into `task.base_branch`
- Keep `config.default_branch` — it's the source of the default value

## Verification

1. `cargo test` — all tests pass
2. `cargo clippy -- -D warnings`
3. `cargo fmt --check`
4. Manual TUI test:
   - Create task → verify base_branch prompt appears with "main" pre-filled
   - Press Enter → task created with base_branch="main"
   - Create another, type "develop" → task has base_branch="develop"
   - Edit task → verify base_branch in editor, change it, verify saved
5. Review wrap-up skill reads naturally
6. Allium spec is consistent with implementation
