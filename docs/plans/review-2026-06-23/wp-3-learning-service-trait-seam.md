# LearningService Trait Seam

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:executing-plans to implement this plan task-by-task.

**Goal:** Give `LearningService` a trait seam (`LearningServiceApi`) matching the task/epic/todo pattern, and inject it into `TuiRuntime` instead of constructing it ad-hoc in `runtime/editor.rs`.

## Context

This work package addresses a finding from the 2026-06-23 codebase review. The service layer exposes `TaskServiceApi`, `EpicServiceApi`, and `TodoServiceApi` in `src/service/api.rs`, each held as `Arc<dyn ...Api>` by `TuiRuntime` / `McpState` so tests can inject mocks without a DB. `LearningService` is the odd one out: it has no trait, and `src/runtime/editor.rs:281-282` constructs one ad-hoc from `self.database`. This asymmetry means learning mutations can't be mocked the way task/epic ones can.

Update the Allium spec first if learning-service behaviour is specified (`docs/specs/learnings.allium`), then write tests against the new trait seam, then implement — per the repo's spec → tests → code discipline.

## Findings

### 💡 `LearningService` lacks a trait seam (`src/service/api.rs`, `src/service/learnings.rs`, `src/runtime/editor.rs:281`)

**Issue:** Unlike task/epic/todo services, `LearningService` has no `...Api` trait, and is built ad-hoc inside `runtime/editor.rs` rather than injected. Learning mutations are therefore not mockable in the same uniform way, and the injection pattern is inconsistent.

**Fix:** Define `LearningServiceApi` in `src/service/api.rs` mirroring the existing traits (concrete `LearningService` delegates via UFCS to avoid inherent-method shadowing). Hold `Arc<dyn LearningServiceApi>` on `TuiRuntime` (and `McpState` if it touches learnings). Replace the ad-hoc construction in `runtime/editor.rs` with the injected field.

## Changes

| File | Change |
|------|--------|
| `docs/specs/learnings.allium` | If learning-service behaviour is specified, update via the `allium:tend` skill before code; verify with `allium:weed`. |
| `src/service/api.rs` | Add `LearningServiceApi` trait + impl for `LearningService` delegating via UFCS, following the `TaskServiceApi`/`EpicServiceApi` pattern. |
| `src/service/learnings.rs` | Ensure the methods needed by the trait are present; no behaviour change. |
| `src/runtime/mod.rs` | Add `learning_svc: Arc<dyn LearningServiceApi>` to `TuiRuntime`, wire it in the constructor/bootstrap. |
| `src/runtime/editor.rs` | Replace ad-hoc `LearningService` construction (~line 281) with the injected `learning_svc`. |
| `src/mcp/mod.rs` | If `McpState` constructs/uses `LearningService`, switch to the injected `Arc<dyn LearningServiceApi>` for consistency. |

## Verification

- [ ] `cargo test service::learnings` and `cargo test runtime::` — pass
- [ ] `cargo test mcp::handlers::tests::learnings` — passes
- [ ] `cargo test` — full suite green
- [ ] `allium:weed` reports learnings spec ↔ code aligned (if spec was touched)
- [ ] A unit test injects a mock `LearningServiceApi` into the runtime/editor path without a real database
