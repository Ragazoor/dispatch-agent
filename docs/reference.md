# Dispatch Reference

## Key Bindings

### Navigation

| Key | Action |
|-----|--------|
| `h` / `l` / `←` / `→` | Move between columns |
| `j` / `k` / `↓` / `↑` | Move between tasks |
| `[` / `gg` | Jump to top of column |
| `]` / `Shift+G` | Jump to bottom of column |
| `Enter` | Toggle detail panel |
| `Esc` | Clear the current selection |
| `?` | Toggle help overlay |
| `q` | Quit (or exit epic view) |

### Tasks

| Key | Action |
|-----|--------|
| `n` | New task |
| `c` | Copy selected task |
| `e` | Edit task in editor (opens in a separate tmux window) |
| `D` | Quick dispatch — pick repo and dispatch immediately |
| `Shift+L` / `Shift+H` | Move task forward / backward |
| `Space` | Activate the task: jump to the agent's tmux window if one exists, otherwise dispatch (Backlog) or resume (Running/Review/Done with a worktree). On a Running task with no worktree at all (the `⚠ no worktree` card) it opens the kill-and-retry dialog instead. **While split view is active** the jump is replaced by an in-place swap: the selected agent's window is moved into the split pane and the board keeps focus (on the already-pinned task it focuses the pane instead). Windowless cards still dispatch/resume as normal. Replaces the former `d` and `S` keys |
| `Prefix+Space` | (tmux global) Jump back from an agent's window to the dispatch TUI — press your tmux prefix, then Space |
| `Prefix+e` | (tmux global) Show/hide the agent-tree companion pane in whichever agent window you press it in — press your tmux prefix, then `e`. A no-op in windows that aren't agent windows. Like `Prefix+Space`, it is bound while the board TUI runs and unbound when it exits, so the pane can't be toggled with the board closed |
| `s` | Toggle split view — side-by-side TUI + agent pane. With the pane open, `Space` swaps the selected task into it |
| `T` | Detach the tmux panel of every selected task that has a live tmux window (supports batch), after a confirmation |
| `m` | Move the selected task to another epic (or detach it) via the tree picker; on an epic card, reparent that epic |
| `x` | Move task to Done (with confirmation); on a task already in Done, archives it instead. On an epic, always archives. In a multi-selection: tasks only, all Done → archive; otherwise the not-yet-Done tasks move to Done |
| `v` | Toggle select |
| `a` | Select all in column |
| `J` / `K` | Reorder task up / down |
| `/` | Search the board — live bar; a card matches when the query fuzzy-matches its title **or** is a digit prefix of its id (`38` → `#38`, `#380`, `#3837`; a leading `#` is optional). Epic cards match on their own title/id, or when a descendant the board would still show matches. `Enter` keeps the query (shown as a `[/query]` badge), `Esc` in the bar restores the previous query, `Esc` on the board clears it |
| `f` | Filter by repo path |
| `A` | Toggle filter: show only tasks with an active tmux session |
| `F` | Toggle the flat view — show every task as a plain card instead of grouping subtasks under their epic |
| `N` | Toggle notification panel |
| `p` | Open the selected task's URL — its pull request, once one is set — in a browser. Reports `No URL set` when the task has none |
| `P` | Open the personal TODO overlay |
| `t` | Add a TODO linked to the selected card, using the card's title |
| `r` | Refresh a feed epic — the selected epic card if it has a feed command, otherwise the feed epic you are inside. Does nothing elsewhere |
| `:` | Open the main session — jump to it if its tmux window is alive, otherwise pick a directory (reconfigure) and open it there. The status bar shows a passive badge: `● main` when the session is running, `○ main` when a directory is configured but no session is running |
| `o` | Sync the selected task's repository with origin on its default branch: merge whatever it is behind by, push whatever it is ahead by, after a confirmation. Offered only while the status bar's drift segment is lit (`main ↑3↓1`); a clean or unmeasurable repository shows no segment and the key does nothing. See `docs/specs/repo-sync.allium` |

### Epics

| Key | Action |
|-----|--------|
| `E` | New epic |
| `Space` | Enter epic view (see subtasks) |
| `D` | Quick dispatch subtask for this epic |
| `Shift+L` / `Shift+H` | Move epic status forward / backward |
| `J` / `K` | Reorder subtasks (determines dispatch order) |
| `U` | Toggle auto-dispatch for the epic you are inside — chain the next backlog subtask when one finishes |
| `R` | Toggle group-by-repo for the epic you are inside |
| `q` | Exit epic view |

### Text fields (naming a task, editing a todo, typing a query)

| Key | Action |
|-----|--------|
| `←` / `→` | Move the caret one character |
| `Ctrl+←` / `Ctrl+→` | Jump one word (also `Alt+←`/`Alt+→` or `Alt+B`/`Alt+F`) |
| `Home` / `End` | Jump to start / end |
| `Backspace` / `Delete` | Delete the character before / at the caret |

Typing inserts at the caret. In repo-picker fields (`←`/`→` move the text caret;
`↑`/`↓` still move the repo list).

### Agent-tree companion pane

Pressed inside the pane itself (no tmux prefix) while it has tmux focus. The pane is
its own process — these keys never reach the board TUI, and all of them act on the
pane's own view only, except `Space`/`Enter` on a file, which opens an editor.

| Key | Action |
|-----|--------|
| `j` / `↓` | Move the cursor down |
| `k` / `↑` | Move the cursor up |
| `h` / `←` | Collapse the selected directory, or move to its parent |
| `l` / `→` | Expand the selected directory (a no-op on a file) |
| `Space` / `Enter` | On a directory: toggle it open/closed. On a file: open it in `$VISUAL`, else `$EDITOR`, else `vi`, in a full-width pane below taking 60% of the window height. Focus stays in the tree, so you can keep browsing; the next file you open **replaces** that pane, killing whatever was running in it |
| `q` / `Ctrl+C` | Close the pane |

The cursor position and manual expansions live in that process, so they do not survive
closing and reopening the pane. Use `Prefix+e` to toggle it (see above).

The editor pane starts in the task's worktree and is handed the file's absolute path.
`$VISUAL`/`$EDITOR` are split on whitespace and run directly, with no shell — so
`EDITOR="vim -p"` works, and nothing in the value is shell-expanded. A value that is
empty or all whitespace counts as unset. The **same** resolution applies to the board's
pop-out task/epic editor (`e` on a card), so one `$EDITOR` means one thing everywhere.
A GUI editor that
forks (`gvim`) returns immediately, so its pane closes while its own window stays open.
When opening fails — the agent deleted the file after touching it, or tmux refused the
split — the reason appears in the pane's bottom border until the next keypress, and is
logged to `app.log`.

## How Dispatch Works

Press `Space` on a Backlog task:

1. Creates a git worktree at `<repo>/.worktrees/<id>-<slug>`
2. Opens a new tmux window in your current session
3. Launches `claude` with the task description and completion instructions (the MCP server is already wired up via `~/.claude.json` from `dispatch setup`)

The agent reports progress via the MCP server running on `localhost:3142`. When it finishes, it moves the task to Review. Closing a tmux window does **not** delete the worktree — press `Space` again on a Running task to resume (or, if the window is still alive, `Space` jumps to it).

## CLI Usage

```bash
# Start the TUI (must be inside a tmux session)
dispatch tui

# CLI — used by agents and hooks
dispatch plan <task-id> <plan-path>

# statusLine decorator (wired into ~/.claude/dispatch-statusline.json by
# `dispatch setup`) — records rate-limit windows, then chains to the
# previous statusLine command
dispatch statusline --snapshot <path> [--chain <command>]

# Local-first repo sync (see docs/specs/repo-sync.allium)
dispatch repo status [--no-fetch]   # one drift row per saved repo path; read-only
dispatch repo sync [<path>]         # sync one saved repo path, or every one
```

`repo status` fetches before measuring so its counts are current; `--no-fetch`
reports whatever the local refs say. A repository that cannot be measured shows
`unknown` rather than any ahead/behind figure — it is never reported as clean.
`repo sync` attempts every target and exits non-zero if any of them failed.

Tasks are created and mutated via the MCP tools (`create_task`,
`update_task`) — there is no CLI subcommand for creating, listing or
updating a task.

## Configuration

| Flag | Env Var | Default |
|------|---------|---------|
| `--db` | `DISPATCH_DB` | `~/.local/share/dispatch/tasks.db` |
| `--port` | `DISPATCH_PORT` | `3142` |

## Timing Constants

- **Tick interval** (2s): `TICK_INTERVAL` in `src/runtime/mod.rs` — captures tmux output, checks staleness.
- **DB refresh** (event-driven + 10s fallback): `dirty_since_refresh` / `ticks_since_last_refresh` on `App` — `RefreshFromDb` emitted only when a `Persist`/`BatchPatchSubStatus` write has occurred since the last refresh, or every 5 ticks (10 s) as a fallback catch-all.
- **Status TTL** (5s): `STATUS_MESSAGE_TTL` in `src/tui/mod.rs` — transient status bar messages auto-clear.
- **PR poll** (30s): `PR_POLL_INTERVAL` in `src/tui/mod.rs` — polls PR status for tasks in review.
- **Message flash** (30s): `MESSAGE_FLASH_TTL` in `src/tui/mod.rs` — how long a task's card keeps flashing after it sends or receives a native cross-session message (task #4098; warm fill, plus a direction glyph — envelope for received, outgoing arrow for sent, both if it did both). The border is the resting neutral, never hued. Read by *both* `tick_message_flash` (the sweep, `src/tui/update/agent.rs`, which sweeps `AgentTracking::message_flash`/`message_flash_sent`) and the card renderer; they must share it or the map and the screen diverge. See "Message flash" in `docs/specs/core.allium`.
- **Main-session poll** (5 ticks / 10s): `MAIN_SESSION_POLL_TICKS` in `src/tui/mod.rs` — tick-driven tmux liveness check behind the main-session status-bar indicator; wired in `handle_tick` (`src/tui/update/agent.rs`), mirrors `config.main_session_poll_interval` in `docs/specs/core.allium`.
- **gg-chord timeout** (150ms): `GG_CHORD_TIMEOUT` in `src/tui/mod.rs` — double-tap window for the `gg` jump-to-top keybinding.
- **Dispatch watchdog** (840s / 14 min): `DISPATCH_WATCHDOG_TIMEOUT` in `src/tui/mod.rs` — force-fails a task stuck in the `dispatching` set (see `docs/specs/dispatch.allium`: `DispatchingTimeout`). Derived as `SUBPROCESS_TIMEOUT` (`src/process.rs`, 120s) times `PROVISION_MAX_SUBPROCESS_CALLS` (`src/dispatch/worktree.rs`, 7) — not a 1:1 mirror — so the watchdog can't trip while `fetch_origin`'s retry budget is still legitimately working within `FetchPolicy::Required` (#4201).

## Feeds

A **feed epic** is an epic with a `feed_command`: dispatch polls the command on
an interval, parses its stdout as a JSON array of feed items, and upserts each
as a task under the epic. The feed is the source of truth — a task whose
`external_id` is absent from the latest emission is removed (manual tasks, which
have no `external_id`, are never touched). Per-epic poll cadence is
`feed_interval_secs`, falling back to the default feed interval (30s) when unset.

Generic feeds come in two flavours, both upstream-agnostic: **flat** (one task
per item under the epic) and **group_by_repo** (items bucketed into per-repo
sub-epics). Author your own script, point an epic's `feed_command` at it, and
debug it with `dispatch verify-feed '<command>'` before wiring it on.

### Managed review & CVE feeds

Two feeds are **managed** by dispatch rather than hand-wired. Instead of
maintaining one epic per review bucket, you configure **two scripts** and
dispatch provisions and reconciles the epic tree for you:

- **Reviews script** (`reviews_feed_command`) — emits **one** deduped list of
  every open PR you're involved with (see the signal vocabulary below). The
  command lives on a managed **`PR Reviews`** parent epic. Dispatch routes that
  single emission into three sub-epics by each PR's signals:
  **My Reviews**, **Team Reviews**, and **Bots**. A PR that changes bucket
  (e.g. you start reviewing a team-requested PR) is **moved**, preserving its
  status, worktree and agent session — it is not deleted and recreated. A PR
  leaves the board only when it is **merged or closed**.
- **CVE script** (`cve_feed_command`) — emits security/CVE advisories onto a
  managed **`CVE`** epic. Advisories are not PRs, so the CVE feed stays a
  separate epic with the ordinary flat upsert.

Each script has an optional interval (`reviews_feed_interval_secs`,
`cve_feed_interval_secs`); unset falls back to the default feed interval.
Reference templates ship in `scripts/` (`fetch-reviews.sh`, `fetch-cve.sh`) with
empty repo/org placeholders — edit them before use.

Managed epics are identified by **role**, not title: rename `My Reviews` to
`My PRs` and the rename survives every reconcile. If you **archive** a managed
epic, dispatch leaves it archived (it is not resurrected); re-enable it by
unarchiving. The three review sub-epics carry **no** `feed_command` of their
own — only the parent is polled, and the parent's single emission fans out to
them.

> **Configuring the scripts.** The four settings are read **at TUI startup** to
> provision the managed tree, and are configured **only over MCP** — there is no
> in-app editor. Use the `set_managed_feed_config` tool to write them (each field
> is optional: omit to leave unchanged, pass `null` to clear) and
> `get_managed_feed_config` to read them back. A save re-provisions the managed
> tree immediately, so no restart is needed.

### Migration: remove old hand-wired review/dependabot epics

The managed reviews feed **folds in** what hand-wired review and Dependabot
feeds used to do (bot-authored PRs now flow into the `Bots` sub-epic). The
generic feed mechanism is untouched, so your old epics keep working — which
means that **until you delete them, the same PR appears twice**: once in your
old review/Dependabot epic and once in the managed sub-epics.

When you enable the managed reviews feed, **delete your old hand-wired review
and Dependabot feed epics.** Dispatch does not auto-delete them (no data-loss
risk), so this cleanup is a manual, one-time step.

### Reviews signal vocabulary (for custom scripts)

If you write your own reviews script, attach a `signals` array to each PR item.
Routing into the My/Team/Bots buckets is done by dispatch from these signals
(first match wins, top to bottom):

| Signal | Emitted when | Routes to |
|--------|-------------|-----------|
| `reviewed` / `commented` (and **not** `author-me`) | you reviewed or commented on a PR that isn't yours | **My Reviews** (engagement wins, even over `author-bot`) |
| `author-bot` | the PR author login ends in `[bot]` (Renovate/Dependabot) | **Bots** |
| `direct-request` | `user-review-requested:@me` — you were asked directly | **My Reviews** |
| `team-request` | `review-requested:@me` — your team was asked | **Team Reviews** |
| *(none of the above / empty)* | fallback | **My Reviews** (logged as a warning) |

`author-me` (the PR is yours) suppresses the engagement rule, so your own
commented-on PRs don't count as engagement. A PR matched by several GitHub
searches must appear **once** with its signals **merged** (union), not picked
arbitrarily — group by URL and union the arrays. Unrecognised signal strings
are dropped with a warning rather than failing the whole feed.

> **Known limitation — GitHub search lag.** GitHub's search API is eventually
> consistent: a just-reviewed PR can still match `review-requested:@me` for a
> poll cycle or two. Routing is correct once the signals settle, so a bucket
> move may lag the real-world action briefly. This is expected, not a bug.

## Setup

`dispatch setup` configures Claude Code integration:

1. **MCP server** — registers the dispatch server in `~/.claude.json` (user-global). Earlier dispatch versions wrote to `~/.claude/.mcp.json`, which Claude Code never read; setup now cleans that up.
2. **Plugin** — installs hooks, skills, and commands to `~/.claude/plugins/local/dispatch/`
3. **Status line** — writes `~/.claude/dispatch-statusline.json`, a `--settings` file loaded by every dispatch-spawned Claude session that wires the `dispatch statusline` decorator in as `statusLine.command`, chaining to whatever command was previously configured in `~/.claude/settings.json`. The decorator records the subscription rate-limit windows from the hook payload to `<data_dir>/rate-limits.json` (`~/.local/share/dispatch/rate-limits.json` by default), which the TUI polls to render the budget badge in the top row. `dispatch tui` also recreates this file at startup if it's missing, so a broken/deleted file self-heals on next launch — see Troubleshooting below.
4. **Tmux** — enables `focus-events` globally (needed for split-view focus indicator)

`~/.claude/settings.json` is not modified by setup — dispatch tool permissions are managed by the user or via Claude Code's interactive prompts.

The setup is idempotent — safe to run on every install or upgrade.

### Plugin contents

| Component | Purpose |
|-----------|---------|
| `/wrap-up` skill | Commit, rebase, or author + create a draft PR when a task is complete (PR title and body are written by the agent based on the actual diff) |
| `task-status-hook` | Automatically transitions task status (running/review/needs_input) |

To verify the plugin is installed:
```bash
ls ~/.claude/plugins/local/dispatch/
```

To reinstall:
```bash
dispatch setup
```

## Tmux Configuration

`dispatch setup` enables `focus-events` for the running tmux server. To persist this across tmux server restarts, add to `~/.tmux.conf`:

```
set -g focus-events on
```

This allows the split-view focus indicator to work: a colored border shows which pane has focus (cyan = TUI, dim = agent pane). Without this setting, the border will not respond to pane switches.

## Troubleshooting

**`not running inside a tmux session`**
Start a tmux session first: `tmux new-session -s dev`

**`dispatch: command not found`**
`~/.local/bin` is not in your PATH. Add to your shell profile:
```bash
export PATH="$HOME/.local/bin:$PATH"
```

**`claude: command not found`**
Install Claude Code from https://claude.ai/code

**Task status not updating automatically**
Verify the dispatch plugin is installed: `ls ~/.claude/plugins/local/dispatch/hooks/hooks.json`. If missing, run `dispatch setup` to reinstall.

**Skills not available (`/wrap-up`)**
The dispatch plugin may not be installed. Run `dispatch setup` to install it.

**Agents fail to start with `Settings file not found`**
`~/.claude/dispatch-statusline.json` is missing — every dispatch-spawned Claude session is launched with `--settings` pointing at it (see Setup above). Run `dispatch setup` to recreate it, or restart `dispatch tui`, which recreates the file automatically if it's absent.

**Budget badge not showing in the top row**
The status line isn't wired up, or no rate-limit payload has arrived yet. Run `dispatch setup` to (re)write `~/.claude/dispatch-statusline.json`, then start (or restart) a dispatch-spawned Claude session — the badge appears once its statusLine hook has fired at least once. Chain drift (an out-of-band edit to `~/.claude/settings.json`'s `statusLine.command` after setup ran) and schema drift (an unrecognised hook payload shape) are documented here rather than enforced by a `doctor` check; re-running `dispatch setup` re-discovers the current chain.

**Agent window disappeared but task is still Running**
Press `Space` on the Running task to reopen a tmux window in the existing worktree and resume the agent.

**`Ctrl+←` / `Ctrl+→` don't jump words in text fields**
Some tmux configs don't forward the modifier on arrow keys unless `xterm-keys` is
on. Either add `set -g xterm-keys on` to your `~/.tmux.conf`, or use the
modifier-free fallbacks `Alt+←`/`Alt+→` or readline-style `Alt+B`/`Alt+F`.

## Learning Store

Dispatch maintains a learning store — approved knowledge that is injected into agent prompts automatically and can be queried or recorded via MCP tools.

### Scopes

Learnings are tagged with a scope that determines which tasks see them:

| Scope     | Covers                        | Example use |
|-----------|-------------------------------|-------------|
| `user`    | All tasks for this user       | Editor preference, personal workflow rules |
| `repo`    | All tasks in a repository     | Build toolchain, test patterns |
| `epic`    | All tasks in an epic          | Shared design decisions for this feature |
| `task`    | One specific task             | Episodic notes scoped to a single agent run |

### Retrieval at Dispatch Time

When an agent is dispatched, Dispatch queries approved learnings that match the task's context and injects them into the prompt. The union includes:

- **Always**: `user`-scoped learnings
- **Always**: `repo`-scoped learnings where `scope_ref` matches the task's repo path
- **If task belongs to an epic**: `epic`-scoped learnings for that epic

`task`-scoped learnings are **not** auto-injected. They can be retrieved explicitly via `query_learnings` with a `tag_filter`.

### Ranking

The SQL candidate query orders by kind (`procedural` first), then scope proximity (epic → repo → user), then confirmation count. That selects *which* entries are candidates; the injected block is then **RAG-ranked by relevance to the task**, so `kind` gives no precedence in the final prompt and procedural entries are not injected as a verbatim prompt prefix.

The auto-inject cap is **10 learnings**. Agents can retrieve up to **50** via an explicit `query_learnings` call.

### Recording a Learning

Agents propose learnings via `record_learning`. The `scope_ref` is auto-derived from the task's context when omitted:

```
scope=user    → scope_ref: (none)
scope=repo    → scope_ref: task.repo_path
scope=epic    → scope_ref: task.epic_id  (error if task has no epic)
scope=task    → scope_ref: task.id
```

Recorded learnings are approved on creation and become eligible for injection immediately — there is no
human approval step. Curation happens after the fact: agents rate entries via `rate_learning`, entries
can be removed with `delete_learning`, and the background sweep archives approved-but-unhelpful entries
left untouched past the staleness threshold.

### Examples

**User preference** — applies to every task you run:
```
scope=user, kind=preference
summary="Always use uv to run Python scripts, never python directly"
```

**Repo convention** — applies to all tasks in this repository:
```
scope=repo, kind=convention
summary="Integration tests use Database::open_in_memory() — never mock the DB layer"
```

**Epic decision** — applies only to tasks in this epic:
```
scope=epic, kind=procedural
summary="This epic adds the learning store; consult docs/specs/core.allium before changing domain types"
```

**Task episodic note** — scoped to a single task, not auto-injected:
```
scope=task, kind=episodic
summary="Rebase on main resolved the rusqlite version conflict; use that if it recurs"
```
