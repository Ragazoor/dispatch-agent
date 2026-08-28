# MCP and Runtime Nesting Depth

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:executing-plans to implement this plan task-by-task.

**Goal:** Flatten the three deepest functions in the codebase — all at nesting depth 7 — two of which sit on the MCP request path that `CLAUDE.md` says must never panic.

## Context

This work package addresses findings from the whole-repository codebase review of 2026-08-28 (`docs/plans/2026-08-28-codebase-review.md`, section 4).

Depth 7 is the maximum measured anywhere in 1,312 production functions. It matters here specifically because `CLAUDE.md` states:

> **Render-panic policy**: … MCP handlers and `src/tui/input.rs` must never panic, guarded or not.

Depth 7 is where a missing `else` hides. Two of the three functions are MCP handlers.

**This is a pure refactor. Behaviour must not change.** Every arm, every error message, every emitted `Message` and `JsonRpcResponse` stays byte-identical. The suite is the contract.

## Findings

### ⚠️ `handle_mcp` — depth 7, 126 lines, cyc~30 (`src/mcp/handlers/dispatch.rs:539`)

**Issue:** The deepest function in the codebase, and the single entry point for every MCP request. It is one large `match req.method.as_str()` whose arms are of wildly different sizes: `"ping"` and `"tools/list"` are one-liners, while `"initialize"` inlines protocol-version negotiation and `"tools/call"` nests `match identity_result` → `Ok(identity)` → argument extraction → `dispatch_tool` → `if let CallerIdentity::Task(task_id)` → trajectory recording.

**Fix:** Extract the two fat arms into named helpers, leaving the top-level `match` as a thin router:

- `fn negotiate_initialize(id, params) -> JsonRpcResponse` — the `"initialize"` arm. Its protocol-version logic (`SUPPORTED_PROTOCOL_VERSIONS.contains(&v)` else `SERVER_PROTOCOL_VERSION`) is self-contained and worth being independently testable.
- `async fn handle_tools_call(state, id, identity_result, params) -> JsonRpcResponse` — the `"tools/call"` arm, including the timing and trajectory-recording tail.

Inside `handle_tools_call`, replace `match identity_result.as_ref() { Err(e) => return …, Ok(identity) => { … } }` with an early return:

```rust
let identity = match identity_result.as_ref() {
    Ok(i) => i,
    Err(e) => return JsonRpcResponse::err(id, INVALID_REQUEST, e.to_string()),
};
```

That single change removes two levels from the whole body. Keep the JSON-RPC notification early-return at the top of `handle_mcp` exactly where it is — the comment above it explains a real client-breaking bug and the code is already flat.

### ⚠️ `handle_list_tasks` — depth 7, 82 lines, cyc~23 (`src/mcp/handlers/tasks/crud.rs:172`)

**Issue:** Two separate sources of depth.

1. The `Ok(filtered)` arm of the outer `match` contains a block-expression that builds `plan_goals`, itself containing `for` → `if let Some(path)` → `if !cache.contains_key(path)`. That is four levels below the match arm.
2. The caller-identity block nests `match identity` → `CallerIdentity::Task` → `match fetch_caller_task(...)` → a `let epic = if has_explicit_scope { … } else { … }`.

**Fix:** Three extractions, no logic change:

- `async fn build_plan_goal_cache(tasks: &[Task]) -> HashMap<String, String>` — lift the block-expression out whole. Keep its comment ("Read each unique plan file once to avoid repeated I/O per task") on the new function; that comment is the reason the cache exists and must not be lost.
- `async fn resolve_list_scope(state, id, identity, parsed) -> Result<(Option<EpicId>, Option<TaskId>), JsonRpcResponse>` — the caller-identity block. The `has_explicit_scope` rule (an explicit `epic_id` or `repo_paths` suppresses the caller's inherited epic) is a real behavioural rule and deserves to be named and directly testable.
- Flip the outer `match … { Ok(filtered) => …, Err(e) => service_err_to_response(id, e) }` to a `let … else`-style early return on the error so the success path un-indents.

Preserve the empty-result early return (`"No tasks found"`) exactly — it is a distinct response shape, not a fallthrough.

### ⚠️ `spawn_refresh_epic` — depth 7 in 41 lines (`src/runtime/tasks.rs:629`)

**Issue:** The highest nesting *density* anywhere in the codebase — depth 7 packed into 41 lines. The cause is a nested `match` on two sequential DB calls inside a `tokio::spawn`:

```rust
match db.get_epic(epic_id).await {
    Ok(Some(epic)) => {
        let _ = tx.send(…Updated(epic));
        match db.list_tasks_for_epic(epic_id).await {
            Ok(tasks) => { for task in tasks { let _ = tx.send(…); } }
            Err(e) => { let _ = tx.send(…Error(…)); }
        }
    }
    Ok(None) => { TuiRuntime::do_full_board_refresh(db, tx).await; }
    Err(e) => { let _ = tx.send(…Error(…)); }
}
```

**Fix:** The cheapest, clearest win in this work package. Extract the spawned body into a plain `async fn` and use early returns:

```rust
async fn refresh_epic_into(db: Arc<…>, tx: …, epic_id: EpicId) {
    let epic = match db.get_epic(epic_id).await {
        Ok(Some(e)) => e,
        Ok(None) => return TuiRuntime::do_full_board_refresh(db, tx).await,
        Err(e) => { let _ = tx.send(/* db_error("refreshing epic", e) */); return; }
    };
    let _ = tx.send(Message::Epic(EpicMessage::Updated(epic)));
    let tasks = match db.list_tasks_for_epic(epic_id).await {
        Ok(t) => t,
        Err(e) => { let _ = tx.send(/* db_error("listing epic tasks", e) */); return; }
    };
    for task in tasks {
        let _ = tx.send(Message::Task(TaskMessage::Updated(Box::new(task))));
    }
}
```

`spawn_refresh_epic` then becomes a three-line `tokio::spawn(refresh_epic_into(db, tx, epic_id))`.

**Two things not to change.** The `let _ = tx.send(...)` discards are deliberate — a closed channel during shutdown is not an error, and turning these into `?` or an `expect` would introduce exactly the panic the policy forbids. And the `Ok(None) => do_full_board_refresh` fallback is meaningful behaviour (the epic vanished, so re-read everything), not a default case. `docs/architecture.md` explains why `do_full_board_refresh` carries no watermark guard while `exec_refresh_from_db` does — read that note before touching either.

## Changes

| File | Change |
|------|--------|
| `src/mcp/handlers/dispatch.rs` | Extract `negotiate_initialize` and `handle_tools_call` from `handle_mcp`; convert the `identity_result` match to an early return |
| `src/mcp/handlers/tasks/crud.rs` | Extract `build_plan_goal_cache` and `resolve_list_scope` from `handle_list_tasks`; early-return the service error |
| `src/runtime/tasks.rs` | Extract `refresh_epic_into` from the `tokio::spawn` body in `spawn_refresh_epic`; convert both nested matches to early returns |

## Verification

- [ ] `cargo test` — all pass. This is a behaviour-preserving refactor, so **any** test change is a signal you altered semantics, not a signal to update the test
- [ ] `cargo clippy --all-targets -- -D warnings` — clean. Watch for `clippy::needless_return` in the new early-return arms
- [ ] `cargo fmt` before committing
- [ ] Re-measure nesting depth on the three functions and confirm each is now ≤ 4
- [ ] Confirm no `let _ = tx.send(...)` became a `?`, `unwrap`, or `expect` — grep the diff for `unwrap`/`expect` and expect zero additions
- [ ] Confirm the `"No tasks found"` response and the `Ok(None) => do_full_board_refresh` fallback both still fire. If no existing test covers the `Ok(None)` path, add one — it is a real behavioural branch at depth 7 today
- [ ] Confirm the JSON-RPC notification early return (HTTP 202, no body) in `handle_mcp` is untouched
