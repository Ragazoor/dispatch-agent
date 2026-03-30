# Dispatch

A terminal kanban board for managing development tasks and dispatching Claude Code agents. Create tasks, dispatch agents into isolated git worktrees and tmux windows, and monitor their progress — all from a single TUI.

## Installation

### One-line install (Linux x86_64)

```bash
curl -fsSL https://raw.githubusercontent.com/Ragazoor/dispatch/main/install.sh | bash
```

Or clone and run locally:

```bash
git clone https://github.com/Ragazoor/dispatch
cd dispatch
bash install.sh
```

The script downloads the latest release binary to `~/.local/bin/dispatch` and runs `dispatch setup` to configure Claude Code.

### Build from source

```bash
cargo build --release
cp target/release/dispatch ~/.local/bin/
dispatch setup
```

## Prerequisites

| Dependency | Required | Purpose |
|---|---|---|
| `tmux` | Yes | Dispatch must run inside a tmux session; agents run in tmux windows |
| `git` | Yes | Agent workspaces are git worktrees |
| `claude` | Yes | Claude Code CLI — dispatched for each agent |
| `gh` | Optional | Review Board fetches open PRs via `gh api` |

After installing, run `dispatch setup` once to register the MCP server and hook scripts with Claude Code.

## Usage

```bash
# Start the TUI (must be inside a tmux session)
dispatch tui

# CLI — used by agents and hooks
dispatch update <task-id> <status>
dispatch list [--status <status>]
dispatch create --from-plan plan.md
```

If you see `not running inside a tmux session`, run `tmux new-session -d -s dev` first.

## Key Bindings

### Navigation

| Key | Action |
|-----|--------|
| `h` / `l` / `←` / `→` | Move between columns |
| `j` / `k` / `↓` / `↑` | Move between tasks |
| `Enter` | Toggle detail panel / enter epic |
| `Tab` | Switch to Review Board |
| `?` | Toggle help overlay |
| `q` | Quit (or exit epic view) |

### Tasks

| Key | Action |
|-----|--------|
| `n` | New task |
| `e` | Edit task in editor |
| `d` | Dispatch agent (Backlog task with plan) / brainstorm (without plan) / resume (Running task whose window is gone) |
| `D` | Quick dispatch — pick repo and dispatch immediately |
| `m` / `M` | Move task forward / backward |
| `W` | Wrap up — commit, rebase, open PR |
| `g` | Jump to the agent's tmux window |
| `x` | Archive task (with confirmation) |
| `H` | Toggle archive panel |
| `Space` | Toggle select |
| `a` | Select all in column |
| `J` / `K` | Reorder task up / down |
| `f` | Filter by repo path |
| `N` | Toggle notification panel |

### Epics

| Key | Action |
|-----|--------|
| `E` | New epic |
| `d` | Dispatch next backlog subtask |
| `D` | Quick dispatch subtask for this epic |
| `m` | Mark epic done (when all subtasks are done) |
| `J` / `K` | Reorder subtasks (determines dispatch order) |
| `q` | Exit epic view |

### Review Board (`Tab`)

| Key | Action |
|-----|--------|
| `h` / `l` / `j` / `k` | Navigate PRs |
| `Enter` | Open PR in browser |
| `r` | Refresh |
| `Tab` / `Esc` | Return to kanban |

## How Dispatch Works

Press `d` on a Backlog task that has a plan attached:

1. Creates a git worktree at `<repo>/.worktrees/<id>-<slug>`
2. Opens a new tmux window in your current session
3. Writes `.mcp.json` so Claude discovers the MCP server
4. Launches `claude` with the task description and completion instructions

The agent reports progress via the MCP server. When it finishes, it moves the task to Review. Closing a tmux window does **not** delete the worktree — press `d` again on a Running task to resume.

## Architecture

- **Elm architecture** — events produce `Message`s, `App::update()` returns `Vec<Command>`, commands are executed by the main loop
- **Ratatui** TUI with Crossterm for terminal input
- **SQLite** via `rusqlite` with `Mutex<Connection>` for thread-safe synchronous access
- **MCP server** on `localhost:3142` (Axum, Streamable HTTP) — agents call `update_task`, `get_task`, `create_task`, and more
- **Hooks** in `~/.local/bin/` parse the branch name to update task status when Claude Code starts or stops

## Configuration

| Flag | Env Var | Default |
|------|---------|---------|
| `--db` | `DISPATCH_DB` | `~/.local/share/dispatch/tasks.db` |
| `--port` | `DISPATCH_PORT` | `3142` |
