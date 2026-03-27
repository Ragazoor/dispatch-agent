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
- Closing a tmux session preserves the worktree; press `d` to resume with `claude --continue`
- Status transitions (running/review) are handled by project-level Claude Code hooks in `.claude/settings.local.json` that extract the task ID from the git branch name
- Press `g` to jump to an agent's tmux window

> **TODO:** Project-level hooks assume worktree branches follow the `{id}-{slug}` naming convention and that `task-orchestrator` is in PATH. For the general case (multi-project dispatch, non-worktree setups), consider MCP-based status reporting or a dedicated CLI subcommand that infers context.

## MCP Server

Starts alongside TUI on `localhost:3142`. Agents use it to query and update tasks.
Tools: `update_task`, `get_task`, `create_task`.

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
- All subprocess calls go through `src/tmux.rs` or `std::process::Command` in `src/dispatch.rs`
- Tests use in-memory SQLite databases
- **App field visibility**: All `App` fields use `pub(in crate::tui)` — accessible from `input.rs`, `ui.rs`, `tests.rs` but not outside the `tui` module. External code uses public accessor methods.
- **Column count**: `TaskStatus::COLUMN_COUNT` is the canonical source. Never hardcode `5`.
- **Database abstraction**: `db::TaskStore` trait abstracts persistence. `TuiRuntime` and `McpState` hold `Arc<dyn TaskStore>`. Tests can provide mock implementations.
- **Task lookup**: Use `App::find_task(id)` / `find_task_mut(id)` instead of inline `.iter().find()`.
