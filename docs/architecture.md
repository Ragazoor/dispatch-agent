# Architecture

Key patterns that aren't obvious from reading the code:

- **Message → Command**: `App::update()` processes input messages and returns `Command`s (side effects). Keep rendering pure, effects in commands.
- **Message routing (co-located)**: `src/tui/dispatcher.rs` only routes the outer `Message` enum to its per-domain inner enum (`Message::Task(tm) => tm.route(app)`). The per-variant wiring lives **beside each `*Message` enum** in `src/tui/messages/<domain>.rs` as an inherent `route(self, app: &mut App) -> Vec<Command>` method (named `route`, not `dispatch`, to stay grep-distinct from the top-level `dispatcher::dispatch`). This makes `messages/*.rs` deliberately *not* a pure-data layer — each domain enum owns the wiring to `App`'s `handle_*` methods. Adding a TUI interaction is a single-file edit here (variant + its `route` arm) plus the `handle_*` implementation in `src/tui/update/<domain>.rs`; only a brand-new *domain* enum adds a line to `dispatcher.rs`. Every `route` arm delegates to a `handle_*` method (no inline `app.board` mutation, no direct `Command` construction) so the shape is uniform across all domains — see `src/tui/messages/split.rs` for the canonical example. Note the *command* side (`Command` → runtime effect) deliberately stays centralized in `src/runtime/commands.rs` rather than mirroring this co-location: `Command` handlers need `TuiRuntime`, so co-locating a `route` on `Command` would invert the `tui → runtime` dependency. The two routing layers use opposite conventions on purpose.
- **Inline-mutation convention**: Input handlers in `input.rs` directly mutate `self.input.mode`, cursor positions, and other UI-only state, returning `vec![]` (no commands). This is intentional — not an Elm Architecture violation. The rule: if a state change has no side effects (no DB write, no process spawn, no network call), mutate inline and return empty. If it needs a side effect, return a `Command`. If a UI handler in `src/tui/input.rs` returns `vec![]` after mutating `self.input.mode`, cursor positions, or selected indices, that is intentional — do not change it to a `Message`.
- **ProcessRunner trait**: Abstraction over git/tmux shell commands. Tests use `MockProcessRunner` — never shell out in tests.
- **TaskPatch builder**: Selective field updates for the database. `None` = don't change, `Some(None)` = set field to NULL.
- **MCP server**: Runs on port 3142 (configurable via `DISPATCH_PORT`). Agents call JSON-RPC methods in `src/mcp/handlers/` to update task status. Caller identity is established via `X-Caller-Task-Id` / `X-Caller-Kind` HTTP headers, parsed by the `extract_caller_identity` middleware (`src/mcp/middleware.rs`) and attached to the request as `Result<CallerIdentity, IdentityError>` — every handler that requires authorization extracts this extension rather than accepting an argument.
- **Integration tests**: Use `Database::open_in_memory()` with a real SQLite instance — no mocking the database layer.
- **Command queue draining**: `execute_commands` (`src/runtime/mod.rs`) loads the initial `Vec<Command>` into a `VecDeque` and drains it iteratively. Most `commands::dispatch` arms return `vec![]`; returning additional commands to trigger a cascade is the exception. Any handler that does produce extra commands extends the queue with `queue.extend(extra)`, so a single message can cascade into multiple commands without recursive calls:

  ```rust
  let mut queue = std::collections::VecDeque::from(cmds);
  while let Some(command) = queue.pop_front() {
      let extra = commands::dispatch(command, app, rt);
      queue.extend(extra);
  }
  ```
- **Editor session invariant**: `TuiRuntime` holds an `editor_session: Arc<Mutex<Option<EditorSession>>>` (`src/runtime/mod.rs`). At most one pop-out editor can be open at a time — the runtime refuses to start a new one while the slot is occupied. The slot is `None` when idle.
- **Layout-cache coherence (self-healing)**: `App.layout` is a [`LayoutCache`](../src/tui/types.rs) grouping five caches derived from `board.tasks`/`board.epics` — `epic_stats_cache`, `children_map_cache`, `column_anchor_cache`, `epic_filter_cache`, `task_index` — plus their fingerprints, so the fields that must stay coherent with each other can only be invalidated as a unit (`LayoutCache::invalidate()`). Handlers that mutate the board should still call `App::invalidate_layout_cache()` (directly or via `sync_board_selection()`) as a perf optimization — it forces an immediate rebuild — but it is **not required for correctness**. `cached_epic_stats()` computes a cheap fingerprint (`compute_layout_fingerprint()`: a plain FNV-1a-style fold over id/status/epic-membership/sort_order of every task and epic, deliberately cheaper than a cryptographic hash) on every call, including the cache-hit fast path, and self-heals — discarding and rebuilding the four `HashMap`-backed caches — whenever the fingerprint no longer matches the one captured at the last rebuild. `task_index` (used by `find_task_mut`) uses the same pattern with its own lighter fingerprint, `compute_task_ids_fingerprint()` (task ids only, no epics/status/sort_order — `task_index` only maps id→Vec position), so a same-length wholesale replacement of `board.tasks` with a different id set is caught too, not just length changes. This means a handler that forgets to invalidate can no longer produce silently stale UI on any of the five caches; it only pays for one extra rebuild on the next read.
- **Render dirty flag (fail-open)**: `App::handle_key()` (`src/tui/input.rs`) unconditionally sets `self.dirty = true` after dispatching a key to its mode-specific handler; the render loop in `src/runtime/mod.rs` only redraws when `frame_ready` sees `dirty && elapsed_since_render >= MIN_FRAME_INTERVAL` (16ms). A prior version tried to skip redraws for true no-ops (e.g. `j` at the last row) by snapshotting a handful of fields — row, column, mode/view discriminant, caret, buffer length — before and after the handler ran, only marking dirty when one of them changed. That opt-in snapshot was fixed for missed handlers three separate times (popup cursor state, tree-view open/collapse state living in a `RefCell`, edit buffers — all invisible to the snapshot) before being replaced outright: any mutating handler that forgot to also call `self.dirty = true` produced a keystroke with no visible effect until an unrelated event (the next keypress, a tick, an MCP notification) happened to trigger a redraw, at which point the missed change and the new one both appeared at once. `Message`/`Mcp` events already set `dirty = true` unconditionally in `apply_loop_event`; `Key` events now follow the same fail-open rule instead of an opt-in one — the 16ms cap already bounds the cost of a redundant redraw, so there is no correctness or perf reason to special-case no-ops.

## Review/Security Agent State Machine

Review agents (dispatched for PRs) and fix agents (dispatched for security alerts) track their lifecycle via `ReviewAgentStatus` (`src/models/review.rs`):

| Status | DB value | Card badge | Meaning |
|--------|----------|------------|---------|
| `Reviewing` | `"reviewing"` | `[reviewing]` yellow | Agent session is active; agent is analyzing |
| *(none)* | `NULL` | *(no badge)* | No agent dispatched |

`FindingsReady` and `Idle` variants still exist on the enum so legacy DB rows parse, but no current code path writes them.

State transitions:

```
dispatch (d) → Reviewing → detach (T) or PR merge → NULL (agent_status cleared)
```

Key bindings on a PR/alert card:
- `d` — dispatch agent (blocked when `Reviewing`)
- `g` — jump to the active tmux session
- `T` — detach: kills the tmux window and clears `tmux_window`, `worktree`, and `agent_status` atomically

`set_pr_agent` / `set_alert_agent` write `ReviewAgentStatus = Reviewing` atomically with `tmux_window` and `worktree`; detach and PR-merge detection clear all three. See `docs/specs/2026-04-07-review-agent-ux-design.md` for the full specification.

## Error Handling

The codebase uses three error types at different layers:

- **`anyhow::Result`** — infrastructure and IO errors (file operations, shell commands, DB initialization). Used at the outer edges where errors propagate up to the caller.
- **`ServiceError`** (`Validation` / `NotFound` / `Internal`) — business logic errors in `src/service/mod.rs`. MCP handlers match on these to return appropriate JSON-RPC error codes.
- **Domain-specific errors** (`FinishError`, `PrError`) — operations with distinct failure modes that callers need to handle differently (e.g., rebase conflicts vs. push failures).

Rule of thumb: use `ServiceError` for request validation and business rules, domain-specific errors when callers branch on the variant, and `anyhow` for everything else.

## Quick Dispatch

`Shift+D` creates and immediately dispatches a task without going through the task creation dialog. The flow:

1. Key handler emits `Command::QuickDispatch { draft: TaskDraft { title: DEFAULT_QUICK_TASK_TITLE, repo_path, .. }, epic_id }` (`src/tui/mod.rs`)
2. Runtime handles it in `exec_quick_dispatch()` (`src/runtime/tasks.rs`) — calls `create_task()` then immediately dispatches
3. The created task gets title `"Quick task"` (`DEFAULT_QUICK_TASK_TITLE` in `src/models/tasks.rs`), no tag, no plan
4. If the board has multiple repo paths, `Shift+D` first enters `InputMode::QuickDispatchRepo` (repo picker), then emits `Message::SelectQuickDispatchRepo(idx)` to resolve the repo before dispatching

Quick dispatch is the same code path as normal dispatch — the difference is the task is created with defaults and skips the creation dialog entirely.

**Command-level shortcut:** `Command::QuickDispatch` bypasses the normal `Command::DispatchAgent` → `Message::Dispatched` round-trip. `exec_quick_dispatch()` calls `create_task()` and immediately dispatches in a single step — there is no intermediate message back through `app.update()`.
