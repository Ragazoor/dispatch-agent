# Cleanup

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:executing-plans to implement this plan task-by-task.

**Goal:** Remove unused dev-dependency to keep the dependency tree clean.

## Context

This work package addresses findings from a code review. A declared dev-dependency is not used anywhere in the test code.

## Findings

### :bulb: Unused `proptest` dev-dependency (`Cargo.toml`)

**Issue:** `proptest` is declared in `[dev-dependencies]` but is not imported or used in any test file. This adds unnecessary compile time and dependency weight.

**Fix:** Remove `proptest` from `[dev-dependencies]` in `Cargo.toml`. Run `cargo test` to confirm nothing breaks.

## Changes

| File | Change |
|------|--------|
| `Cargo.toml` | Remove `proptest` from `[dev-dependencies]` |

## Verification

- [ ] Run existing tests — all pass
- [ ] `cargo build` succeeds without proptest
- [ ] `cargo clippy -- -D warnings` passes
