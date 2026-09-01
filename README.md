# Dispatch

A terminal kanban board that runs Claude Code agents. Each agent gets its own git worktree and tmux window.

![The dispatch board](docs/images/board.png)

## Why

Most agent managers are session managers: they open a terminal and let you watch. Dispatch is a workflow tool.

- **Every agent has a task** — a title, a status, a repo, and often a plan
- **Agents move their own cards** — hooks report start, finish, and blocked; you never set a status by hand
- **Work is isolated** — one worktree and one branch per agent, so your checkout stays clean
- **Agents coordinate** — an MCP server lets them create tasks, read the knowledge base, and chain the next one

## Requirements

| Tool | | Install |
|---|---|---|
| Rust | required | [rustup.rs](https://rustup.rs) |
| `tmux` | required | `dnf install tmux` / `brew install tmux` |
| `git` | required | usually already there |
| `claude` | required | [Claude Code CLI](https://claude.ai/code) |
| `gh` | optional | [GitHub CLI](https://cli.github.com), for pull requests |

## Install

```bash
git clone https://github.com/Ragazoor/dispatch-tui
cd dispatch-tui
cargo install --path .
dispatch setup          # registers the MCP server, plugin, and tmux settings
tmux new-session -s dev # dispatch must run inside tmux
dispatch tui
```

`dispatch setup` is safe to re-run after every upgrade.

## Your first task

1. Press `n` and fill in a title, a description, and a repo path.
2. Press `Space`. Dispatch cuts a worktree, opens a tmux window, and starts an agent there.
3. Watch it work: `s` splits the screen, board on the left and agent on the right.
4. The agent moves itself to Review when it is done. Type `/wrap-up` in its session to commit and either rebase onto the base branch or open a draft pull request.

In a hurry? Press `D` instead. It skips the form, dispatches at once, and the agent names the task itself.

## How it works

**Columns** — Backlog → Running → Review → Done. `Space` is the one action key: it dispatches a Backlog task, and jumps to or resumes a Running one.

**Worktrees** — each agent works in `<repo>/.worktrees/<id>-<slug>`. Closing the tmux window keeps the worktree, so `Space` resumes where the agent left off.

**Plans** — a markdown file describing what to build. Attach one and the agent implements it directly instead of designing first.

**Tags** — labels such as `bug`, `feature`, `chore`, `research`, `pr-review`. Most are just labels. Two change behaviour: `research` launches a research agent, and `pr-review` skips the design step and, given a PR link, cuts the worktree from that PR's branch.

**Epics** — a group of tasks that run in order. Turn on auto-dispatch (`U`) and finishing one subtask starts the next, so each worktree is cut from a branch that already holds its predecessor's work.

**Knowledge base** — agents record what they learn. Dispatch injects the relevant entries into the next agent's prompt.

**Feeds** — a shell command that pulls work onto the board on a schedule, such as open pull requests or CVE alerts.

## Docs

- [Reference](docs/reference.md) — key bindings, configuration, CLI, feeds, troubleshooting
- [MCP](docs/mcp.md) — the tools agents call
- [Specs](docs/specs/) — what the system is meant to do
- [CLAUDE.md](CLAUDE.md) — building, testing, and contributing

## License

[MIT](LICENSE)
