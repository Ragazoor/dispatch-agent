# Dispatch

A terminal kanban board for managing development tasks and dispatching Claude Code agents. Create tasks, dispatch agents into isolated git worktrees and tmux windows, and monitor their progress — all from a single TUI.

## Prerequisites

| Dependency | Required | Install |
|---|---|---|
| `tmux` | Yes | `sudo apt install tmux` / `brew install tmux` |
| `git` | Yes | Already installed on most systems |
| `claude` | Yes | [Claude Code CLI](https://claude.ai/code) |
| `gh` | Optional | [GitHub CLI](https://cli.github.com) — needed for the Review Board |

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

The script downloads the latest release binary to `~/.local/bin/dispatch`, runs `dispatch setup` to register the MCP server and hook scripts with Claude Code, and prints a prerequisites checklist.

If `~/.local/bin` is not in your PATH, add it to your shell profile:

```bash
export PATH="$HOME/.local/bin:$PATH"
```

Verify the install:

```bash
dispatch --version
```

### Build from source

```bash
cargo build --release
cp target/release/dispatch ~/.local/bin/
dispatch setup
```

## Getting Started

**1. Open a tmux session** (Dispatch must run inside tmux):

```bash
tmux new-session -s dev
```

**2. Start the TUI:**

```bash
dispatch tui
```

### Create a task (`n`)

| Step | Key | What happens |
|------|-----|--------------|
| Create task | `n` | Enter title, description, and repo path |
| Dispatch | `d` | Agent explores your codebase, writes a plan, and implements it |
| Agent needs input *(optional)* | `g` | Desktop notification — jump to agent and interact |
| Review the work | `g` | Task is in Review — check the result in the tmux window |
| Wrap up | `W` | Commit, rebase, and open a PR. Or use `/wrap-up` from the agent's session |

### Quick dispatch (`D`)

| Step | Key | What happens |
|------|-----|--------------|
| Quick dispatch | `D` | Pick a repo from the numbered list |
| | | Task created and dispatched — agent sets its own title and description |
| Check on the agent | `g` | Jump to the agent's tmux window |
| Wrap up | `W` | Commit, rebase, and open a PR. Or use `/wrap-up` from the agent's session |

### Work with an epic (`E`)

| Step | Key | What happens |
|------|-----|--------------|
| Create epic | `E` | Enter title, description, and repo path |
| Dispatch planning | `d` | Creates a planning subtask; agent writes an implementation plan with subtasks |
| Dispatch subtasks | `d` | Each press dispatches the next Backlog subtask in order |
| Reorder subtasks | `J` / `K` | Change dispatch order within the epic |
| Wrap up each subtask | `W` | Commit, rebase, and open a PR. Or use `/wrap-up` from the agent's session |

## Key Concepts

**Tasks** — the unit of work. Each task has a title, description, status, and optionally a plan and a linked git repo.

**Plans** — markdown files describing what an agent should build. Tasks without a plan trigger "brainstorm" mode when dispatched — the agent explores and writes a plan, then implements it.

**Kanban columns:** Backlog → Running → Review → Done

- **Backlog** — tasks ready to be dispatched (`▸` = has a plan)
- **Running** — agent is active in a tmux window
- **Review** — agent finished; awaiting your review
- **Done** — merged and wrapped up

**Worktrees** — each dispatched agent gets its own git worktree at `<repo>/.worktrees/<id>-<slug>`, isolating agent work from your main branch. Closing the tmux window does **not** delete the worktree — press `d` again to resume.

**Epics** — a group of related tasks. Press `Enter` on an epic to see its subtasks. Press `d` on the epic to dispatch the next Backlog subtask automatically.

## Learn More

- **[Reference](docs/reference.md)** — key bindings, configuration, CLI usage, troubleshooting
- **[CLAUDE.md](CLAUDE.md)** — architecture, testing patterns, contribution guidelines
