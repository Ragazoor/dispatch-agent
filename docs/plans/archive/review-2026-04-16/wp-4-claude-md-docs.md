# CLAUDE.md: Add missing documentation

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:executing-plans to implement this plan task-by-task.

**Goal:** Document four undocumented conventions that trip up new contributors.

## Context

This work package addresses findings from a code review.

## Findings

### :bulb: Command queue iterative draining is undocumented

**Issue:** `execute_commands` in `runtime.rs` processes commands from a `VecDeque`. Commands returned by message handlers can enqueue further commands, which are drained iteratively. This is not mentioned anywhere and would surprise a new contributor adding a handler whose command triggers another command.

**Fix:** Add a note in the Architecture section of CLAUDE.md explaining the iterative command queue.

### :bulb: QuickDispatch command-level flow is undocumented

**Issue:** The Quick Dispatch section describes the user-facing flow but not the command-level difference: `Command::QuickDispatch` goes directly to `exec_quick_dispatch` which creates the task and dispatches immediately, skipping the normal `Command::DispatchAgent` -> `Message::Dispatched` round-trip.

**Fix:** Add a note to the Quick Dispatch section clarifying the command-level shortcut.

### :bulb: `conn()` vs `conn.lock()` split is undocumented

**Issue:** `db/queries.rs` has a safe `self.conn()` accessor (at `db/mod.rs:416`) but some methods historically used `self.conn.lock().unwrap()`. No documentation steers new contributors to the safe pattern.

**Fix:** Add a note in the Architecture or Code Conventions section about always using `self.conn()?`.

### :bulb: `TaskPatch` double-Option convention is undocumented in CLAUDE.md

**Issue:** The DB layer uses `Option<Option<T>>` (double-Option) for nullable fields in `TaskPatch`/`EpicPatch`. The service layer uses the `FieldUpdate` enum for the same concept. Only `FieldUpdate` is documented in CLAUDE.md. A contributor reading only CLAUDE.md would not know the DB layer uses a different pattern.

**Fix:** Add a note in the Code Conventions section documenting both patterns and when each applies.

## Changes

| File | Change |
|------|--------|
| `CLAUDE.md` | Add "Command Queue" note to Architecture section |
| `CLAUDE.md` | Add command-level note to Quick Dispatch section |
| `CLAUDE.md` | Add `conn()` accessor note to Code Conventions section |
| `CLAUDE.md` | Add `TaskPatch` double-Option note to Code Conventions section |

## Verification

- [ ] CLAUDE.md renders correctly in a markdown viewer
- [ ] No existing documentation contradicted by the additions
