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
├── main.rs          # Entry point, TUI main loop, command execution
├── models.rs        # Task, TaskStatus, Note, NoteSource, slugify
├── db.rs            # SQLite database (Mutex<Connection>), CRUD for tasks/notes/repo_paths
├── dispatch.rs      # Agent dispatch: worktree creation, tmux window, MCP config, prompt
├── tmux.rs          # tmux subprocess wrappers (new-window, send-keys, capture-pane, has-window)
├── tui/
│   ├── mod.rs       # App state, Message/Command enums, update logic
│   ├── input.rs     # Keyboard input handling per mode (normal, text input, confirm)
│   └── ui.rs        # Ratatui rendering (columns, detail panel, status bar)
└── mcp/
    ├── mod.rs       # Axum router + server setup
    └── handlers.rs  # JSON-RPC MCP handlers (update_task, add_note, get_task)
```

## Kanban Columns

Backlog → Ready → Running → Review → Done

- **Ready** = eligible for dispatch (`d` key)
- **Running** = agent dispatched, tmux output shown on card
- Tasks auto-advance from Running → Review when the tmux window exits

## MCP Server

Starts alongside TUI on `localhost:3142`. Agents use it to report status and post notes.
Tools: `update_task`, `add_note`, `get_task`.

## Configuration

| Flag | Env Var | Default |
|------|---------|---------|
| `--db` | `TASK_ORCHESTRATOR_DB` | `~/.local/share/task-orchestrator/tasks.db` |
| `--port` | `TASK_ORCHESTRATOR_PORT` | `3142` |

## Conventions

- Rust edition 2021, SQLite with bundled `libsqlite3-sys`
- Sync `rusqlite` with `Mutex` (not async wrapper)
- All subprocess calls go through `src/tmux.rs` or `std::process::Command` in `src/dispatch.rs`
- Tests use in-memory SQLite databases
