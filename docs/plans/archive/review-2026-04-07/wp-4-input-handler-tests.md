# Input Handler Tests

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:executing-plans to implement this plan task-by-task.

**Goal:** Add test coverage for the 1,029-line keyboard input handler module that currently has zero tests.

## Context

This work package addresses findings from a code review. `src/tui/input.rs` contains all keyboard event handling — mode transitions, navigation, text editing, confirmation dialogs — with no test coverage. Any keybinding change or refactor risks regressions that won't be caught.

## Findings

### :bulb: No tests for keyboard input handling (`src/tui/input.rs`)

**Issue:** 1,029 lines of input handling code with zero tests. This includes:
- `handle_key()` main dispatcher
- Mode-specific handlers: `handle_key_normal()`, `handle_key_text_input()`, `handle_key_confirm_delete()`, etc.
- Text editing modes (InputTitle, InputDescription, InputRepoPath)
- Confirmation dialogs for delete, archive, wrap-up
- Navigation (arrow keys, vim keys h/j/k/l)

**Fix:** Create a test harness that:
1. Constructs an `App` with known state (tasks, view mode)
2. Sends `KeyEvent`s through `handle_key()`
3. Asserts resulting state changes (mode transitions, selection changes) and returned `Message`s

Focus on high-value tests first:
- Normal mode: navigation (j/k/h/l), dispatch (d), delete (x), create (n)
- Text input mode: typing, backspace, enter to submit, escape to cancel
- Confirmation mode: y/n responses
- View mode transitions: entering/exiting detail view, help overlay, filter overlay

## Changes

| File | Change |
|------|--------|
| `src/tui/tests.rs` | Add input handler test module with tests for each input mode |
| `src/tui/input.rs` | May need minor refactoring to make testable (e.g., accept `KeyEvent` directly) |

## Verification

- [ ] Run existing tests — all pass
- [ ] New tests cover at minimum: normal navigation, text input, confirmation dialogs, view transitions
- [ ] `cargo test` includes the new input tests
- [ ] `cargo clippy -- -D warnings` passes
