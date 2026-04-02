# Dispatch

Terminal kanban board for dispatching Claude Code agents into isolated git worktrees via tmux.

**Stack**: Rust (2021 edition), ratatui TUI, SQLite (rusqlite), Axum HTTP/MCP server, tokio async runtime.

## Build & Test

```bash
cargo build
cargo test
cargo run -- tui
```

Pre-commit hook runs `cargo fmt --check` and `cargo clippy -- -D warnings` automatically — no need to run these manually.

## Allium Specification

`docs/specs/dispatch.allium` is the **source of truth** for domain logic: task lifecycle, status transitions, sub-status invariants, dispatch rules, and epic behavior. Consult it before changing core behavior. Use `allium:tend` and `allium:weed` skills to keep spec and code aligned.

## Architecture

Key patterns that aren't obvious from reading the code:

- **Message → Command**: `App::update()` processes input messages and returns `Command`s (side effects). Keep rendering pure, effects in commands.
- **Inline-mutation convention**: Input handlers in `input.rs` directly mutate `self.input.mode`, cursor positions, and other UI-only state, returning `vec![]` (no commands). This is intentional — not an Elm Architecture violation. The rule: if a state change has no side effects (no DB write, no process spawn, no network call), mutate inline and return empty. If it needs a side effect, return a `Command`.
- **ProcessRunner trait**: Abstraction over git/tmux shell commands. Tests use `MockProcessRunner` — never shell out in tests.
- **TaskPatch builder**: Selective field updates for the database. `None` = don't change, `Some(None)` = set field to NULL.
- **MCP server**: Runs on port 3142 (configurable via `DISPATCH_PORT`). Agents call JSON-RPC methods in `src/mcp/handlers/` to update task status.
- **Integration tests**: Use `Database::open_in_memory()` with a real SQLite instance — no mocking the database layer.

## Tag System

Tags (`bug`, `feature`, `chore`, `epic`) drive dispatch behavior via `DispatchMode::for_task()` in `models.rs`:

| Tag | No plan | Has plan |
|-----|---------|----------|
| `epic` | Brainstorm (ideation, no edits) | Dispatch |
| `feature` | Plan (write implementation plan) | Dispatch |
| `bug`, `chore`, none | Dispatch | Dispatch |

A task with a plan always dispatches directly regardless of tag. Tags are selected during task creation: `b`=bug, `f`=feature, `c`=chore, `e`=epic, Enter=none.

## Timing Constants

- **Tick interval** (2s): `TICK_INTERVAL` in `runtime.rs` — captures tmux output, checks staleness.
- **Status TTL** (5s): `STATUS_MESSAGE_TTL` in `tui/mod.rs` — transient status bar messages auto-clear.
- **PR poll** (30s): `PR_POLL_INTERVAL` in `tui/mod.rs` — polls PR status for tasks in review.

## Module Map

| File | Responsibility |
|------|---------------|
| `src/main.rs` | CLI entry point (clap), subcommand dispatch (`tui`, `update`, `add`) |
| `src/lib.rs` | Crate root, public module re-exports |
| `src/runtime.rs` | Async event loop (`tokio::select!`), bridges TUI ↔ MCP ↔ shell commands, executes `Command` side effects |
| `src/tui/mod.rs` | `App` struct, `update()` message dispatcher, `column_items_for_status()` render helper |
| `src/tui/input.rs` | Key event handlers, inline-mutation convention for UI-only state |
| `src/tui/ui.rs` | Rendering logic (ratatui `Frame` drawing), pure functions |
| `src/tui/types.rs` | `Message`, `Command`, `ViewMode`, `InputMode`, `AgentTracking` enums and structs |
| `src/tui/tests.rs` | TUI unit tests |
| `src/models.rs` | Domain types (`Task`, `Epic`, `TaskStatus`, `SubStatus`, `TaskTag`), `DispatchMode::for_task()` tag routing |
| `src/service.rs` | Domain service layer (`TaskService`, `EpicService`): business logic (validation, patch building, epic recalculation) decoupled from MCP/HTTP |
| `src/db/mod.rs` | `Database` struct, constructor, `TaskStore` trait, `TaskPatch`/`EpicPatch` builders |
| `src/db/migrations.rs` | Versioned schema migrations (`MIGRATIONS` array, `migrate_vN_*` functions) |
| `src/db/queries.rs` | `impl TaskStore for Database` — all CRUD operations, row helpers |
| `src/db/tests.rs` | Database unit tests |
| `src/dispatch.rs` | Worktree creation, tmux session management, agent lifecycle (dispatch/brainstorm/plan/resume/review) |
| `src/process.rs` | `ProcessRunner` trait + `RealProcessRunner` / `MockProcessRunner` for testable shell execution |
| `src/tmux.rs` | Tmux API: create windows, send keys, capture pane output, kill windows |
| `src/github.rs` | GitHub CLI (`gh`) integration: PR creation, review status polling, CI status |
| `src/editor.rs` | External `$EDITOR` integration for editing task/epic fields |
| `src/plan.rs` | Plan file parsing (extract title/description from markdown) |
| `src/setup.rs` | First-run setup: MCP config merging, hook script installation |
| `src/mcp/mod.rs` | MCP server bootstrap (Axum router), `McpState`, `McpEvent` notification enum |
| `src/mcp/handlers/dispatch.rs` | JSON-RPC entry point (`handle_mcp`), tool definitions, method routing |
| `src/mcp/handlers/tasks.rs` | Task tool handlers (thin wrappers): parse JSON-RPC args → call `TaskService` → format response |
| `src/mcp/handlers/epics.rs` | Epic tool handlers (thin wrappers): parse JSON-RPC args → call `EpicService` → format response |
| `src/mcp/handlers/types.rs` | JSON-RPC request/response types, flexible integer deserializer |
| `src/mcp/handlers/tests.rs` | MCP handler integration tests |

## MCP Notification Flow

When an MCP handler mutates the database, the TUI must refresh to show the change. This is the propagation path:

```
MCP handler (e.g. handle_update_task)
  → mutates DB via state.db
  → calls state.notify()                          # McpState method
    → sends McpEvent::Refresh via mpsc::UnboundedSender
      → runtime event loop receives it             # tokio::select! in run_event_loop()
        → calls rt.exec_refresh_from_db(app)
          → reads all tasks/epics from DB
          → calls app.update(Message::RefreshTasks(tasks))
            → App replaces its in-memory task list, re-renders
```

Key types in the chain:
- `McpEvent` (`src/mcp/mod.rs`) — enum with `Refresh` and `MessageSent` variants
- `McpState::notify()` — fire-and-forget send on the channel
- `TuiRuntime::exec_refresh_from_db()` (`src/runtime.rs`) — reloads tasks, epics, and usage from DB
- `Message::RefreshTasks` (`src/tui/types.rs`) — carries the fresh task list into the App

The `MessageSent` variant additionally triggers `Message::MessageReceived(task_id)`, which flashes the target task's card in the TUI.

## Visibility Convention

`App` fields use `pub(in crate::tui)` to restrict mutation to the TUI module. External code (runtime, MCP handlers) can only change `App` state by sending a `Message` through `app.update()`, which returns `Command`s. This keeps state transitions auditable in one place and prevents scattered mutation from outside the TUI boundary.

## How-To Guides

### Adding a New MCP Tool

1. **Define the handler** in `src/mcp/handlers/tasks.rs` (or `epics.rs` for epic tools). Follow the pattern: parse args with `types::parse_args`, call `state.db` methods, call `state.notify()` if mutating, return `JsonRpcResponse::ok`.
2. **Add the tool schema** to `tool_definitions()` in `src/mcp/handlers/dispatch.rs` — add a new entry to the `tools` array with `name`, `description`, and `inputSchema`.
3. **Wire the route** in `handle_mcp()` in `src/mcp/handlers/dispatch.rs` — add a match arm in the `tools/call` section mapping the tool name to your handler.
4. **Add types** if needed in `src/mcp/handlers/types.rs` (argument structs with serde derives, use `#[serde(deserialize_with = "deserialize_flexible_i64")]` for integer fields since Claude Code may send them as strings).
5. **Write tests** in `src/mcp/handlers/tests.rs` using `Database::open_in_memory()`.

### Adding a New TUI View/Mode

1. **Add a `ViewMode` variant** in `src/tui/types.rs` (e.g., `ViewMode::MyNewView { selection, saved_board }`).
2. **Add `Message` variants** for entering/exiting and any view-specific actions.
3. **Add `Command` variants** if the view triggers side effects (DB writes, shell commands).
4. **Handle input** in `src/tui/input.rs` — add key handlers under a new match arm for your `ViewMode`.
5. **Handle messages** in `src/tui/mod.rs` `update()` — process your new messages, return commands.
6. **Render** in `src/tui/ui.rs` — add a rendering branch for your view mode.

### Adding a Database Migration

Migrations live in `src/db/migrations.rs` as standalone functions:

1. **Write the migration function**: `fn migrate_vN_description(conn: &Connection) -> Result<()>` in `src/db/migrations.rs`. Use `ALTER TABLE` for additive changes; for destructive changes (column removal, constraint changes), create a new table, copy data, drop old, rename.
2. **Register it** in the `MIGRATIONS` array in `src/db/migrations.rs`: add `(N, migrate_vN_description)`. The loop in `Database::init_schema()` applies any migration where `current_version < N` and bumps `PRAGMA user_version` after each.
3. **Update the schema test**: `fresh_db_has_latest_schema_version` in `src/db/tests.rs` asserts the final version number — bump it to match your new N.
4. **Write a migration test** in `src/db/tests.rs` that creates a DB at the pre-migration schema, inserts test data, runs the migration, and verifies the result.

## Documentation

- `docs/reference.md` — Key bindings, configuration, environment variables, troubleshooting
- `docs/specs/` — Allium specifications for domain logic
- `docs/plans/` — Implementation plans (working artifacts, never committed)
