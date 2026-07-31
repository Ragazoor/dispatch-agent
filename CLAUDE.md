# Dispatch

Terminal kanban board for dispatching Claude Code agents into isolated git worktrees via tmux.

**Stack**: Rust (2021 edition), ratatui TUI, SQLite (rusqlite), Axum HTTP/MCP server, tokio async runtime.

## Build & Test

```bash
cargo build
cargo test
cargo run -- tui
```

Other useful CLI subcommands:

```bash
cargo run -- setup              # configure Claude Code MCP integration
cargo run -- verify-feed 'gh api ...'  # run a feed command and validate its JSON output
```

### First-time setup

The pre-push hook (`.githooks/pre-push`) runs, in order: `cargo fmt` (auto-formats), `cargo clippy --all-targets -- -D warnings` (no `--fix` — it checks, it does not rewrite), `./scripts/check-doc-paths.sh` (validates every `src/…`/`docs/…` path and `file:NN` line citation in `CLAUDE.md`, every `docs/*.md`, and every `docs/specs/*.allium` — globbed, so a new doc is covered the moment it lands; the dated artifacts under `docs/plans/`, `docs/superpowers/`, and `docs/research/` are excluded), `./scripts/test-check-doc-paths.sh` (self-test for that checker), `./scripts/check-doc-symbols.sh` (rejects backticked snake_case identifiers in the agent-facing docs and in `src/**/*.rs` doc comments that occur nowhere in the code — annotate a deliberate reference to removed or external code with `allow-phantom-symbol: <why>`), `./scripts/test-check-doc-symbols.sh` (self-test for that checker), `./scripts/check-no-test-sleep.sh` (rejects `tokio::time::sleep` anywhere under `src/`/`tests/`, and `std::thread::sleep` in test files — see the async-test rule below), and `bash ./scripts/test-fetch-reviews.sh` (stub-`gh` test for the review feed script). Run `cargo test` separately before pushing.

The hook is tracked at `.githooks/pre-push`. A fresh clone must point git at it once — run `cargo run -- doctor hooks --repair` (which sets `core.hooksPath = .githooks`) or `git config core.hooksPath .githooks`. Don't add hooks to `.git/hooks/` directly: that directory is untracked and shared across all worktrees, so changes there aren't version-controlled or reviewed.

### Running tests

```bash
cargo test                                # full suite
cargo test db::tests                      # database CRUD and migrations
cargo test service::                      # domain service layer
cargo test tui::tests                     # TUI input/message handling
cargo test mcp::handlers::tests           # MCP JSON-RPC handlers
cargo test --test lifecycle               # integration: full task lifecycle
cargo test --test epic_lifecycle          # integration: full epic lifecycle
cargo test --test cli                     # CLI subcommand smoke tests
cargo test tui::tests::scenarios          # key-sequence integration tests
cargo test tui::tests::snapshots          # ratatui buffer rendering tests
```

Suite is green; if a runtime test fails locally, suspect timing — `spawn_blocking`-based tests are timing-sensitive.

### Snapshot tests

Snapshots live in `src/tui/tests/snapshots/` and render to a 120×40 `TestBackend`. **Do not change the backend size** — it breaks all existing diffs.

Agent prompt snapshots live in `src/dispatch/snapshots/` and lock the rendered output of every `build_*_prompt` variant. `src/dispatch/prompts/` holds only the two review addenda as markdown (`pr-review.md`, `dependabot.md`, inlined via `include_str!`) — the dispatch, quick-dispatch, and research bodies are string-built in `src/dispatch/prompts.rs`.

To accept intentional UI changes:

```bash
cargo insta review                                  # interactive
INSTA_UPDATE=always cargo test tui::tests::snapshots # auto-accept
INSTA_UPDATE=always cargo test dispatch::prompts_snapshots # auto-accept prompt snapshots
rm src/tui/tests/snapshots/*.snap.new                # always clean up
rm src/dispatch/snapshots/*.snap.new                 # always clean up
```

**Don't skip the `rm *.snap.new` cleanup.** A stray `.snap.new` left in the tree is picked up by the next `cargo insta review` and silently mixed into an unrelated review pass, making it easy to accept the wrong diff. Always remove them once you've accepted (or rejected) a change.

### Where new tests go

| What you're testing | Where |
|---|---|
| TUI key handling / message flow | `src/tui/tests/` |
| DB schema, CRUD, migrations | `src/db/tests/` |
| Service-layer business rules | inline in `src/service/<domain>/` |
| MCP JSON-RPC handler behaviour | `src/mcp/handlers/tests/` |
| Full task/epic lifecycle | `tests/` (integration tests) |
| Domain-type invariants | inline in the owning module |
| Agent prompt rendering (all variants) | `src/dispatch/prompts_snapshots.rs` |
| Agent-facing skill copy (`plugin/skills/*/SKILL.md`) | `mod tests` in `src/setup/plugins.rs` (via `skill_body`) |
| tmux semantics — which pane, which cwd, how many panes, which window a name resolves to | `tests/tmux_lifecycle.rs` (topology/cwd) / `tests/tmux_split_hook.rs` (keystroke routing) / `tests/tmux_window_targets.rs` (exact window-name resolution under prefix collisions), shared rig in `tests/tmux_harness/mod.rs` |
| tmux argv shape — that we sent the right command string | `MockProcessRunner` tests inline in `src/tmux.rs` |

The last two rows are a real split, not two spellings of the same thing: a mock proves *which command we sent*, a real tmux server proves *what tmux did with it*. Read the "`MockProcessRunner` vs a real tmux server" section of `docs/conventions.md` before picking one — guessing wrong is how #3781 and #3782 stayed green while broken.

Property tests live alongside unit tests in a nested `mod property_tests` block.

Skill copy is asserted with targeted `contains` checks (not snapshots) so that deleting a specific instruction reads as a regression rather than an edit. Scope each assertion to the instruction's heading section — sibling sections repeat phrases, so a whole-document `contains` can still pass after the instruction is gone.

Inline test modules (`mod tests`, `mod property_tests`) must have `#[allow(clippy::unwrap_used, clippy::expect_used)]` at the top — the workspace `-D warnings` policy otherwise rejects bare `unwrap()`/`expect()` calls. See `src/db/tests/mod.rs` for the canonical pattern.

Tests must never sleep on the wall clock — not to "wait for" `spawn_blocking` or detached `tokio::spawn` work, and not to cross a duration threshold. Instead await a deterministic completion signal (oneshot / `Notify` / an `McpEvent`), inject a clock, or inject the threshold (`Database::set_slow_call_threshold`, used by `src/db/tests/async_handle.rs`). `./scripts/check-no-test-sleep.sh` (in the pre-push hook) enforces this: no `tokio::time::sleep` anywhere under `src/`/`tests/`, and no `std::thread::sleep` in test files (anything under `tests/`, under a `src/**/tests/` directory, or named `tests.rs`). Production `std::thread::sleep` is unaffected, and a deadline-bounded poll step may carry an `// allow-test-sleep: <why>` marker. See the "No `tokio::time::sleep` in tests" section of `docs/conventions.md` for the canonical patterns.

### Coverage

CI runs `cargo tarpaulin --out xml` in the `coverage` job. Run locally with `cargo tarpaulin --out Html`. Not in the pre-push hook. Coverage is **informational** — there is no enforced threshold; it does not gate the build.

Overall line coverage sits around **85%** as an approximate snapshot (re-measure with `cargo tarpaulin`; the figure drifts and is not tracked). `src/setup/` carries substantial inline tests despite its size — the real known-low areas are its OS-interaction branches (hooks, filesystem writes) and some TUI input/popup files. Driving those to 100% is not expected. Treat the baseline as a sanity check, not a target: don't over-invest chasing full coverage on render-heavy code, and a single file below the average is not by itself a problem.

## Running & Debugging Locally

```bash
cargo run -- tui                                  # requires a running tmux server
cargo run -- --db /tmp/scratch.db tui             # throwaway DB — never point a dev run at your real one
RUST_LOG=dispatch_tui=debug cargo run -- tui      # then tail the log file (see below)
```

- **DB location**: `$XDG_DATA_HOME/dispatch/tasks.db`, else `~/.local/share/dispatch/tasks.db` (`default_db_path()` in `src/lib.rs`). Override with the global `--db` flag or `DISPATCH_DB`. To reset, delete the file — the schema is rebuilt from `MIGRATIONS` on next open.
- **Logs do not go to stderr.** `cmd_tui` installs a `tracing_subscriber` that appends to `app.log` **next to the database file** (`init_app_log_subscriber` in `src/main.rs`), because stderr belongs to the TUI. Watch it with `tail -f ~/.local/share/dispatch/app.log`. The floor is `INFO`; `RUST_LOG` (crate name `dispatch_tui`) raises it.
- **MCP port**: `DEFAULT_PORT = 3142` (`src/lib.rs`), override with `--port` on `tui`/`setup` or `DISPATCH_PORT`.
- **Exercising MCP by hand**: see `docs/mcp.md`. Identity comes from headers, and **exactly one** of the two must be set (`CallerIdentity::from_headers`, `src/mcp/identity.rs:21`, applied by the `src/mcp/middleware.rs` middleware). A bare `curl` sends neither, so it resolves to `IdentityError::Missing` and any handler that requires authorization rejects it. Send `-H 'X-Caller-Task-Id: <id>'` to act as that task's agent, or `-H 'X-Caller-Kind: session'` to act as the human session; sending both is a `Conflict`.

## External Dependencies

Required on `PATH` at runtime, with **no startup preflight** — `dispatch doctor` checks worktrees, sessions, and hooks, not binary availability, so a missing binary surfaces as a failed shell command mid-operation:

- **tmux** (`src/tmux.rs`) — every window/pane operation.
- **git** (`src/git.rs`, `src/dispatch/worktree.rs`, `src/dispatch/finish.rs`) — worktrees, rebase, branch detection.
- **gh** (`src/dispatch/mod.rs`, and the `scripts/fetch-*.sh` feed commands) — PR status and feed data. Network calls.
- **claude** — spawned inside the tmux window by `src/dispatch/agents.rs` as `claude --plugin-dir ~/.claude/plugins/local/dispatch …`; that plugin dir is installed by `cargo run -- setup`.

The agent launchers do **not** hardcode `claude` / `dispatch`: they read them from `ProcessRunner::agent_binaries()` (`src/process.rs`), which defaults to those bare names. That is the seam `tests/tmux_harness/mod.rs` uses to point them at stubs — never `PATH` manipulation. Interpolate via `claude_quoted()`, and keep every launch site at **one** quoting layer: `dispatch_with_prompt` passes the binary as bash's `$0` after its single-quoted script body precisely so it does not sit under two.

POSIX-only. Embeddings/RAG (`src/service/embeddings.rs`) also make live calls.

## Verify Command

A per-repo, single-line shell command (e.g. `cargo test`) that dispatched agents must run before declaring work complete. Stored on the `repo_paths` row for the task's `repo_path`; set via the `set_verify_command` MCP tool or `cargo run -- repo set-verify <path> <command>`. When set, `build_prompt` appends a `## Verification` section (`render_verification` in `src/dispatch/prompts.rs`); when null, nothing is emitted. Newlines and carriage returns are rejected — chain steps with `&&` or `;`.

## Test-Driven Development

Always use TDD. Express intended behaviour as tests before writing the code that satisfies them — for new features, bug fixes, and refactors alike.

## Allium Specification

The Allium specs in `docs/specs/` are the **source of truth** for domain logic:

- `core.allium` — domain model (entities, enums, config, VisualColumn)
- `tasks.allium` — task lifecycle (creation, status movement, reorder, archive, copy, editor)
- `dispatch.allium` — dispatching tasks, retry flows, repo-path persistence
- `agent-health.allium` — activity classification, crash detection, notifications, Claude Code hooks
- `pr-workflow.allium` — PR creation, polling, merge detection, wrap-up, finish paths
- `split-pane.allium` — split-pane lifecycle, focus border, jump-to-agent, pin, swap, tmux detach
- `agent-tree.allium` — agent file tree panel: PostToolUse file-event capture, companion-pane lifecycle inside each agent's tmux window, tree/badge rendering
- `mcp-task-tools.allium` — MCP tools for task management and the CLI plan-attachment surface
- `epics.allium` — epic lifecycle and MCP epic tools
- `task-watchers.allium` — task-watcher subscriptions (`subscribe_to_task` / `unsubscribe_from_task`) and the one-shot completion notice
- `learnings.allium` — knowledge base rules and MCP learning tools
- `feeds.allium` — programmable feed epics (the feed pipeline that upserts tasks from external commands)
- `todo.allium` — personal TODO overlay (lightweight checklist, separate from the kanban board)
- `repo-rag.allium` — per-repo semantic search: indexing and RAG-based doc search
- `observability.allium` — trajectory persistence (per-task audit log of MCP tool calls) and slow-db-call latency warnings
- `doctor.allium` — `dispatch doctor` self-diagnosis CLI surface
- `tips.allium` — startup tips popup (show/browse/dismiss)
- `repo-sync.allium` — local-first repo sync: ahead/behind drift measurement, the sync operation and its typed failure vocabulary, and the surfaces that expose them

Consult the relevant spec before changing core behavior. Use `allium:tend` and `allium:weed` skills to keep spec and code aligned.

## MCP Tools for Agents

The `dispatch` MCP server exposes more than task creation. Worth knowing by name:

- **Knowledge base** — the learnings most relevant to your task are already injected into your prompt at dispatch time (the "Validated knowledge for this task" section above); rate each one you act on with `rate_learning` (`helped`/`wrong`) — trajectory data shows this pre-injected set, not a manual query, is how most agents encounter learnings in practice. Call `query_learnings` yourself when you need more than what was surfaced — a different area of the repo, or a question that comes up mid-task. `record_learning` captures a pitfall/convention/tip worth remembering for future agents. See `learnings.allium`.
- **Your own task** — `get_task` / `update_task` to read or mutate the task you're running as (title, description, status, plan, tag).
- **Repo search** — `search_docs` for semantic search over an indexed repo; `index_repo` to build the index if missing. See `repo-rag.allium`.
- **Finishing** — `wrap_up` + `exit_session` to close out a session (see the `/wrap-up` skill). `exit_session` chains the epic's next backlog subtask automatically when `auto_dispatch` is on; there is no tool for you to call.

`create_task`/`create_epic` matter mainly to orchestrating agents decomposing work, not to an agent executing a single dispatched task. Full tool list and schemas: call `tools/list`, or see `docs/specs/mcp-task-tools.allium`.

## Agent Working Directory

Dispatched agents always work from their worktree folder. Every prompt includes an instruction to stay in the worktree and not `cd` to the parent repo. The tmux window's *starting* cwd is test-covered: `dispatch_agent_opens_tmux_window_in_worktree_not_parent_repo` in `src/dispatch/tests.rs` asserts the window opens inside the task worktree, never the bare parent repo. Runtime `cd`-escape prevention — an agent later `cd`ing out of the worktree — remains prompt-instruction only, with no test asserting against it.

## Tag System

Tags (`TaskTag` in `src/models/tasks.rs:438`): `Bug`, `Feature`, `Chore`, `PrReview`, `Research`, `Fix`, `Dependabot`. Most are **kanban labels only**.

Exactly two mechanisms read the tag:

- `DispatchMode::for_task()` (`src/models/tasks.rs:420`) — `Research`, and only `Research`, and only when the task has no plan, routes to the read-only research agent (`build_research_prompt`). Everything else, plan or no plan, routes to `Dispatch`. There are only two `DispatchMode` variants.
- `TaskTag::is_review()` (`src/models/tasks.rs:465`) — true for `PrReview | Dependabot`. Inside the unified `build_prompt` (`src/dispatch/prompts.rs:264`) this swaps in a review addendum from `src/dispatch/prompts/pr-review.md` or `dependabot.md`, skips the plan/implement instructions in favour of a trimmed trailing block, and — when the task carries a PR URL — bases the worktree on the PR's head branch instead of the repo's base branch, soft-falling back to the base branch if that can't be resolved (`src/dispatch/agents.rs:50`).

`Bug`, `Feature`, `Chore`, and `Fix` change nothing but the card badge.

## Timing Constants

- **Tick interval** (2s): `TICK_INTERVAL` in `src/runtime/mod.rs` — captures tmux output, checks staleness.
- **DB refresh** (event-driven + 10s fallback): `dirty_since_refresh` / `ticks_since_last_refresh` on `App` — `RefreshFromDb` emitted only when a `Persist`/`BatchPatchSubStatus` write has occurred since the last refresh, or every 5 ticks (10 s) as a fallback catch-all.
- **Status TTL** (5s): `STATUS_MESSAGE_TTL` in `src/tui/mod.rs` — transient status bar messages auto-clear.
- **PR poll** (30s): `PR_POLL_INTERVAL` in `src/tui/mod.rs` — polls PR status for tasks in review.
- **Main-session poll** (5 ticks / 10s): `MAIN_SESSION_POLL_TICKS` in `src/tui/mod.rs` — tick-driven tmux liveness check behind the main-session status-bar indicator; wired in `handle_tick` (`src/tui/update/agent.rs`), mirrors `config.main_session_poll_interval` in `docs/specs/core.allium`.
- **gg-chord timeout** (150ms): `GG_CHORD_TIMEOUT` in `src/tui/mod.rs` — double-tap window for the `gg` jump-to-top keybinding.

## Documentation

This file is intentionally slim — it is loaded into every agent's context. Read these on demand:

> **Key pattern**: `FieldUpdate` / `TaskPatch` is the most-touched pattern in the codebase (nullable field mutations). Read [docs/conventions.md](docs/conventions.md) before writing any update handler. See also the `OwnedTaskPatch` parity hazard in that doc — parity is now compiler-enforced via exhaustive destructuring.

> Bare `unwrap()`/`expect()` are clippy-warned outside tests — see the soft-fail-decoding section of `docs/conventions.md` for the canonical fallback pattern. The warning only becomes a **hard error via `-D warnings`**, which the pre-push hook applies (`cargo clippy --all-targets -- -D warnings`); a plain local `cargo build` or `cargo clippy` will *not* fail on it, so a green local build does not imply clippy-clean.

> **Mutation boundary** (compiler-enforced): reads via `state.db` are fine, but task/epic *mutations* go through `TaskServiceApi`/`EpicServiceApi`, not the DB directly — the service layer owns invariants like epic-status recalculation. `state.db` is typed `Arc<dyn db::TaskReadStore>`, so `state.db.patch_task(...)` from a handler is a **compile error**. The name is scoped on purpose: `TaskReadStore` seals **task/epic** writes only — settings/learning/usage writes stay reachable through it because they carry no cross-entity invariant. Sanctioned exceptions hold their own write handle and manage their own invariants — `FeedRunner` (`src/feed/`), `TuiRuntime::feed_db`, and startup/CLI paths (`runtime::bootstrap`, `src/setup/`, `src/cli/doctor.rs`, `src/main.rs`) — so a direct `patch_*` call in those places is not a violation. **CLI handlers still route through `TaskService`** even though `src/main.rs` is sanctioned (e.g. `cmd_plan` uses `TaskService::attach_plan`, mirroring `cmd_update`/`cmd_hook`/`cmd_pr_gate`) — the sanction is a fallback for startup wiring, not a licence to bypass the service. See the service mutation-boundary (including "Sanctioned direct-mutation consumers") and `recalculate_epic_status` sections of `docs/conventions.md`.

> **Layout-cache coherence** (self-healing, not compiler-enforced): `App.layout` (a [`LayoutCache`](src/tui/types.rs), grouping `epic_stats_cache`, `children_map_cache`, `column_anchor_cache`, `epic_filter_cache`, `task_index`, and their fingerprints) is derived from `board.tasks`/`board.epics`. Calling `invalidate_layout_cache()` after a mutation is still good practice (immediate rebuild — it delegates to `LayoutCache::invalidate()`), but `cached_epic_stats()` also fingerprints the board on every call and self-heals on mismatch — a handler that forgets to invalidate cannot serve stale data. See the layout-cache-coherence section of `docs/architecture.md`.

> **DB connection model** (first-order performance constraint): `Database` (`src/db/mod.rs`) has one writer `tokio_rusqlite::Connection` — all mutations serialize through it via `db_call` — plus a small pool of read-only connections that pure reads use via `db_call_read`, so concurrent reads don't queue behind the writer or each other. See the "DB access — `db_call` / `db_call_read`" section of `docs/conventions.md`.

> **Render-panic policy**: a guarded `unreachable!()` in a render match arm is acceptable when an upstream filter/type already rules that arm out (e.g. `ColumnItem` variants stripped before the match in `src/tui/ui/kanban/columns.rs`) — but MCP handlers and `src/tui/input.rs` must never panic, guarded or not. See the "Rendering purity" section of `docs/conventions.md`.

> **Workhorse macros**: `patch_struct!` (`src/db/mod.rs:30`) generates the `TaskPatch`/`EpicPatch` selective-update builders from a field list. `mcp_tools!` (`src/mcp/handlers/dispatch.rs:39`) generates the MCP tool registry (`tool_definitions()`, `dispatch_tool()`, `TOOL_NAMES`) from one declarative list of tools. Read the macro's doc comment before adding a patch field or an MCP tool by hand.

- [docs/architecture.md](docs/architecture.md) — Message→Command, ProcessRunner, command queue draining, editor session invariant, layout-cache coherence (self-healing), render dirty flag (fail-open), error handling, quick dispatch
- [docs/conventions.md](docs/conventions.md) — `FieldUpdate`, `TaskPatch`/`EpicPatch` double-Option, DB trait narrowing, `db_call`/`db_call_read` (writer + read-pool model), rendering-purity panic policy, service mutation boundary, `recalculate_epic_status` invariant, inline-mutation boundary, `LearningService` injection state, `let _`, dead code, sub-status TOCTOU, epic reparenting guards, Clippy, visibility, performance footguns (`column_items_for_status` test-only; no `std::fs` in async), prod-vs-test LOC split
- [docs/module-map.md](docs/module-map.md) — module and subsystem responsibilities
- [docs/how-to.md](docs/how-to.md) — adding an MCP tool, TUI view, entity, database migration; knowledge base MCP tools
- [docs/mcp.md](docs/mcp.md) — MCP notification flow, error codes, debugging handlers, feed epics, knowledge base flow
- [docs/reference.md](docs/reference.md) — key bindings, configuration, environment variables, troubleshooting, learning store
- [docs/specs/](docs/specs/) — Allium specifications for domain logic
- [docs/plans/](docs/plans/) — implementation plans and one-off analysis/review docs

Subsystem entry points (no dedicated doc page — read the source):

- `src/feed/mod.rs` — feed system: `FeedRunner` poll loop, exec/parse/ingest pipeline that upserts tasks from external commands (see also `docs/module-map.md`)
- `src/service/repo_index/` (`mod.rs` orchestration + `scan.rs`/`chunking.rs`/`embed.rs`/`search.rs`), `src/service/embeddings.rs`, `src/mcp/handlers/repo_rag.rs` — repo indexing / embeddings / RAG: `index_repo` and `search_docs` MCP tools for semantic doc search
- `src/cli/` — CLI subcommand implementations, including the `doctor` health-check subcommand (`src/cli/doctor.rs`)
- `src/mcp/trajectory.rs` — agent trajectory capture (records the agent's tool-call history for a task)
- `src/repo_sync.rs` — local-first repo sync: `ahead_behind` drift measurement and `sync_repo` (fetch, merge `origin/<base>`, push). Synchronous and `ProcessRunner`-driven like `src/dispatch/finish.rs`; local base history is never rewritten. See `docs/specs/repo-sync.allium`

## Unsafe Policy

Any `unsafe` block requires a `// SAFETY:` comment justifying why the invariant holds, and reviewer sign-off. See `docs/conventions.md` for the full policy.
