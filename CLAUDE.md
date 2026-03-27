# Task Orchestrator TUI

A terminal kanban board for managing development tasks and dispatching Claude Code agents into isolated git worktrees + tmux windows.

## Build & Test

```bash
cargo build
cargo test
cargo clippy
cargo run -- tui   # launch the TUI (requires running inside a tmux session)
```

Runtime dependencies: `tmux`, `git` (checked at startup). The TUI must be launched from within a tmux session for agent dispatch to work.

> **Tmux prerequisite:** If you see `not running inside a tmux session`, run `tmux new-session -d -s dev` first, then re-run `cargo run -- tui`.

## Architecture

**Elm Architecture** — events produce `Message`s, `App::update()` returns `Vec<Command>`, commands are executed by the main loop.

```
Terminal events ──┐
Async messages ───┤──▶ App::update(Message) ──▶ Vec<Command> ──▶ execute_commands()
Tick timer ───────┘                                                  │
                                                                     ├── PersistTask → SQLite
                                                                     ├── Dispatch → worktree + tmux + claude
                                                                     ├── CaptureTmux → tmux capture-pane
                                                                     └── RefreshFromDb → re-read tasks
```

## Key Files

```
src/
├── main.rs          # Entry point, CLI argument parsing
├── models.rs        # Task, TaskStatus, slugify, COLUMN_COUNT
├── db.rs            # TaskStore trait + SQLite Database impl (Mutex<Connection>)
├── dispatch.rs      # Agent dispatch: worktree creation, tmux window, MCP config, prompt
├── tmux.rs          # tmux subprocess wrappers (new-window, send-keys, capture-pane, has-window)
├── runtime.rs       # TUI main loop, TuiRuntime with exec_* command handlers
├── editor.rs        # External editor integration (format/parse task content)
├── tui/
│   ├── mod.rs       # App state, handle_* message handlers, update() routing
│   ├── types.rs     # Message, Command, InputMode, TaskDraft enums
│   ├── input.rs     # Keyboard input handling per mode (normal, text input, confirm)
│   ├── ui.rs        # Ratatui rendering (columns, detail panel, status bar)
│   └── tests.rs     # Unit tests for App state machine
└── mcp/
    ├── mod.rs       # Axum router + server setup
    └── handlers.rs  # JSON-RPC MCP handlers (update_task, get_task, create_task)
```

## Kanban Columns

Backlog → Ready → Running → Review → Done

- **Ready** = eligible for dispatch (`d` key)
- **Running** = agent dispatched in interactive mode, tmux output shown on card
- **Dispatch** (`d` on a Ready task): creates a fresh git worktree + tmux window and launches Claude with the task prompt
- **Resume** (`d` on a Running task whose window is gone): re-opens a tmux window in the existing worktree and runs `claude --continue`. Closing a tmux window does **not** delete the worktree.
- Status transitions (running/review) are handled by hooks in `.claude/settings.json` that extract the task ID from the git branch name (`{id}-{slug}` pattern)
- Press `g` to jump to an agent's tmux window

## Hooks & Branch Naming

Status update hooks in `.claude/settings.json` run when Claude Code starts or stops in a worktree. They parse the branch name, extract the task ID, and call `task-orchestrator update <id> <status>`.

**Requirements:**
- Worktree branches must follow `{id}-{slug}` (e.g. `42-fix-login-bug`). Non-conforming names silently skip status updates.
- `task-orchestrator` must be in `PATH`. Add the debug binary: `export PATH="$PATH:$(pwd)/target/debug"`

## MCP Server

Starts alongside TUI on `localhost:3142`. Agents use it to query and update tasks.
Tools: `update_task`, `get_task`, `create_task`.

To test tools manually (while TUI is running):
```bash
curl -s -X POST http://localhost:3142/mcp \
  -H 'Content-Type: application/json' \
  -d '{"jsonrpc":"2.0","id":1,"method":"tools/call","params":{"name":"get_task","arguments":{"task_id":1}}}'
```

## Configuration

| Flag | Env Var | Default |
|------|---------|---------|
| `--db` | `TASK_ORCHESTRATOR_DB` | `~/.local/share/task-orchestrator/tasks.db` |
| `--port` | `TASK_ORCHESTRATOR_PORT` | `3142` |

## Adding a New Message/Command

1. Add the `Message` variant to `src/tui/types.rs`
2. Add a `handle_<name>` private method on `App` in `src/tui/mod.rs`
3. Add the routing arm in `App::update()` (one line)
4. Add tests in `src/tui/tests.rs`
5. If side effects needed: add a `Command` variant in `src/tui/types.rs`
6. Add an `exec_<name>` method on `TuiRuntime` in `src/runtime.rs`
7. Add the routing arm in `execute_commands()` (one line)

## Conventions

- Rust edition 2021, SQLite with bundled `libsqlite3-sys`
- Sync `rusqlite` with `Mutex` (not async wrapper)
- All subprocess calls go through `src/tmux.rs` or `src/dispatch.rs`, injected with a `ProcessRunner` (`src/process.rs`). Use `MockProcessRunner` in tests.
- Tests use in-memory SQLite databases
- **App field visibility**: All `App` fields use `pub(in crate::tui)` — accessible from `input.rs`, `ui.rs`, `tests.rs` but not outside the `tui` module. External code uses public accessor methods.
- **Column count**: `TaskStatus::COLUMN_COUNT` is the canonical source. Never hardcode `5`.
- **Database abstraction**: `db::TaskStore` trait abstracts persistence. `TuiRuntime` and `McpState` hold `Arc<dyn TaskStore>`. Tests can provide mock implementations.
- **Task lookup**: Use `App::find_task(id)` / `find_task_mut(id)` instead of inline `.iter().find()`.
