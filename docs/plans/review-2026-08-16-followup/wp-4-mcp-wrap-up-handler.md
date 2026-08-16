# WP-4 — MCP Wrap-Up Handler

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:executing-plans to implement this plan task-by-task.

**Goal:** Extract the exit-token validation gauntlet out of `handle_exit_session` so the atomicity guarantee is stated by a function signature instead of a comment on a block.

## Context

From the follow-up codebase review at `4bf19b04` (`docs/plans/2026-08-16-4220-codebase-review-followup.md`, section 5).

`handle_exit_session` (`src/mcp/handlers/tasks/wrap_up.rs`) is now the densest control flow in the codebase: **165 lines, 18 branches, nesting depth 6** — the highest branch count of any function in the repo bar `EpicService::update_epic`. Much of the length is genuinely load-bearing rationale, and that commentary must survive this change.

The shape is the problem, not the size: a single `RwLock` write scope contains **eight early-return error paths**, then the function moves into a second phase that persists the close, spawns tmux teardown, and chains the next epic subtask.

## Findings

### 💡 An eight-exit validation gauntlet lives inline inside a lock scope

**Issue:** The block that consumes the exit token holds `state.exit_tokens.write()` while performing, in order: token presence check, token equality check, action-presence check, action-vs-stored-action match, tmux-window liveness check, and `pr_url` requirement — each with its own `return JsonRpcResponse::err(...)`. The comment above it explains *why* they are all in one write-lock (so a concurrent second call cannot observe a half-consumed token), which is exactly the kind of invariant that should be enforced by structure rather than described in prose.

**Fix:** Extract the block into a function with a signature that carries the guarantee:

```rust
fn consume_exit_token(
    state: &McpState,
    id: &Option<Value>,
    task: &Task,
    parsed: &ExitSessionArgs,
    token: &str,
) -> Result<(WrapUpAction, Option<String>), JsonRpcResponse>
```

The `Result` makes every early return an `Err`, the single `?` at the call site replaces eight inline exits, and the atomicity comment moves onto the function where it describes the whole unit. `handle_exit_session` is then: parse → fetch task → `consume_exit_token(...)?` → build outcome → close → teardown → chain.

## Changes

| File | Change |
|------|--------|
| `src/mcp/handlers/tasks/wrap_up.rs` | Extract `consume_exit_token`; move the atomicity rationale onto it; reduce `handle_exit_session` to the seven-step flow above |
| `src/mcp/handlers/tests/tasks/wrap_up.rs` | Confirm each rejection path has a test; add any that are missing |

## Implementation notes — read before touching anything

**This is a pure refactor. Zero observable behaviour change.** Every JSON-RPC error message, every error code, and the exact ordering of the checks must be byte-identical afterwards. The order is load-bearing: a caller who sends both a wrong action *and* a missing `pr_url` currently gets the action-mismatch error, and that must not change.

- **Write the characterization tests first.** `src/mcp/handlers/tests/tasks/wrap_up.rs` is 2,511 lines and likely covers most of these paths already. Before refactoring, enumerate the eight rejection paths and confirm each has a test asserting *the exact message and code*. Add the missing ones and watch them pass against the current code. Only then extract. This is the TDD shape for a refactor: the tests are the safety net, written against present behaviour.
- **Preserve the rationale comments verbatim.** They document non-obvious decisions — why a failed close still returns `ok` (the token is already consumed, so an error would strand the agent with no retry path), why `SessionClosed` fires after the terminal patch, why the `(Pr, None)` arm falls back to `Done` rather than asserting. Losing these is a worse outcome than leaving the function long.
- **Keep the lock scope tight and check the guard drops correctly.** The current block relies on the guard dropping at the end of the `let (action, pr_url) = { … };` expression. In the extracted function the guard drops at return. Verify no `.await` sits between acquisition and release — the codebase currently has zero locks held across `.await` and this refactor must not introduce the first.
- Consult `docs/specs/pr-workflow.allium` (`ExitSession`) before starting. If your reading of the spec and the code disagree, **stop and ask** — per CLAUDE.md, ambiguity is a stop condition, not a judgement call. No spec edit is expected, since behaviour is unchanged.
- The other MCP-boundary hazard nearby: KB #419 — adding a method to a `*ServiceApi` seam silently breaks test mocks that intercepted the method it subsumes, and the stub default panics at runtime rather than failing to compile. This package should not add a seam method; if you find yourself wanting to, that is a scope change.

## Verification

- [ ] `cargo test` green — redirect, don't pipe
- [ ] `cargo clippy --all-targets -- -D warnings` clean
- [ ] Every one of the eight rejection paths has a test asserting the exact error message and code, and each was seen passing *before* the extraction
- [ ] `git diff` shows no change to any error string or code
- [ ] Re-measure: `handle_exit_session` branch count materially below 18, nesting below 6
- [ ] No `.await` between lock acquisition and release in the extracted function
