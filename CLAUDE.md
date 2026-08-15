# Dispatch

Terminal kanban board for dispatching Claude Code agents into isolated git worktrees via tmux.

**Stack**: Rust (2021 edition), ratatui TUI, SQLite (rusqlite), Axum HTTP/MCP server, tokio async runtime.

## Build & Test

```bash
cargo build
cargo test
cargo run -- tui
```

**`main` moves while you work.** Other agents land on it during your session, so
the snapshot you read at startup goes stale. Before wrapping up, run `git log
--oneline main..HEAD` **and** `git log --oneline HEAD..main` — the second is the
one that catches a base that moved under you. If it is non-empty, merge `main`
into your branch and re-run the suite before reporting completion; a green run
against a stale base proves nothing. Assume a function you did not write may have
been rewritten since you read it.

Other useful CLI subcommands:

```bash
cargo run -- setup              # configure Claude Code MCP integration
cargo run -- verify-feed 'gh api ...'  # run a feed command and validate its JSON output
```

### First-time setup

A fresh clone must point git at the tracked hooks once: `git config core.hooksPath .githooks`. Nothing does this for you, and until it is run the whole gate below is silently inert. Don't add hooks to `.git/hooks/` directly — that directory is untracked and shared across all worktrees, so changes there aren't version-controlled or reviewed.

The pre-push hook (`.githooks/pre-push`) runs, in order: `cargo fmt`, `cargo clippy --all-targets -- -D warnings` (no `--fix`), `./scripts/check-doc-paths.sh` + its self-test (validates every path and `file:NN` citation in `CLAUDE.md`/`docs/*.md`/`docs/specs/*.allium`, excluding the dated `docs/plans/`, `docs/superpowers/`, `docs/research/`), `./scripts/check-doc-symbols.sh` + its self-test (rejects citations — backticked identifiers, `path.rs::symbol`, `Type::method`, bare snake_case names of five-plus words — that no longer resolve; annotate a deliberate exception with `allow-phantom-symbol: <why>` on the citing line or the one directly above), `./scripts/check-no-test-sleep.sh` (see the async-test rule below), and `bash ./scripts/test-fetch-reviews.sh`. Run `cargo test` separately before pushing — it is not part of the hook.

Prefer `path::symbol` citations (`src/feed/exec.rs::exec_feed_command`) over `file:NN` line numbers in docs: a line number is only bounds-checked (confirmed to exist, never that it still says what the doc claims), while a symbol is checked against the real file. See "`file:NN` vs `path::symbol` citations" in `docs/conventions.md` for what each checker does and doesn't catch.

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
cargo test --test tmux_lifecycle          # real tmux: window/pane topology and cwd
cargo test --test tmux_split_hook         # real tmux: split-pane cwd, and that nothing is typed at a pane
cargo test --test tmux_window_targets     # real tmux: window-name resolution
cargo test --test tmux_editor_pane        # real tmux: agent-tree editor pane, toggle target
```

**The full suite needs `tmux` on `PATH`.** The `--test tmux_*` targets drive a real tmux server (private `-L` socket, `-f /dev/null`, drop-guard teardown — see `tests/tmux_harness/mod.rs`). Without tmux they print `skipping: tmux not available on PATH` and pass, so a green local run isn't proof they ran; CI hard-fails instead of skipping (tmux is installed in both the `test` and `coverage` jobs).

**Don't pipe `cargo test` into `tail`/`head`/`grep`.** A shell pipeline's exit code is the LAST command's, so `cargo test | tail -40` reports `tail`'s exit status (always 0) — combined with a truncation that happens to cut the summary lines, a failing suite reads as a clean pass. Redirect instead: `cargo test > /tmp/t.txt 2>&1; echo $?`, then `grep -E "^(test result|failures:)" /tmp/t.txt`.

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
| DB schema, CRUD | `src/db/tests/` |
| A database migration | `src/db/tests/migrations.rs` — the migration fn must be `pub(super)` to be callable from there. See "Adding a Database Migration" in `docs/how-to.md` for the column-guard rule. |
| Service-layer business rules | inline in `src/service/<domain>/` |
| MCP JSON-RPC handler behaviour | `src/mcp/handlers/tests/` |
| Full task/epic lifecycle | `tests/` (integration tests) |
| Domain-type invariants | inline in the owning module |
| Agent prompt rendering (all variants) | `src/dispatch/prompts_snapshots.rs` |
| Agent-facing skill copy (`plugin/skills/*/SKILL.md`) | `mod tests` in `src/setup/plugins.rs` (via `skill_body`) |
| tmux semantics — which pane, which cwd, how many panes, which window a name resolves to | `tests/tmux_lifecycle.rs` (topology/cwd) / `tests/tmux_split_hook.rs` (split-pane cwd and keystroke absence) / `tests/tmux_window_targets.rs` (exact window-name resolution under prefix collisions) / `tests/tmux_editor_pane.rs` (agent-tree editor pane, and which pane the toggle kills), shared rig in `tests/tmux_harness/mod.rs` |
| tmux argv shape — that we sent the right command string | `MockProcessRunner` tests inline in `src/tmux.rs` |
| Anything that drives a dispatch/resume/provision through a mock | wherever the behaviour lives, but script the runner with `DispatchScript` (`src/dispatch/mock_sequence.rs`) — never a hand-written `vec![ok(), ok(), …]` |
| A `pub(in crate::tui::ui)`-or-narrower helper (unreachable from `src/tui/tests/`) | inline in the owning module, e.g. `staleness_color`/`feed_role_label` in `src/tui/ui/shared.rs`, `budget_spans` in `src/tui/ui/budget.rs` |

The two tmux rows are a real split, not two spellings of the same thing: a mock proves *which command we sent*, a real tmux server proves *what tmux did with it*. Read the "`MockProcessRunner` vs a real tmux server" section of `docs/conventions.md` before picking one — guessing wrong is how #3781 and #3782 stayed green while broken.

Property tests live alongside unit tests in a nested `mod property_tests` block.

Skill copy is asserted with targeted `contains` checks (not snapshots) so that deleting a specific instruction reads as a regression rather than an edit. Scope each assertion to the instruction's heading section — sibling sections repeat phrases, so a whole-document `contains` can still pass after the instruction is gone.

Inline test modules (`mod tests`, `mod property_tests`) must have `#[allow(clippy::unwrap_used, clippy::expect_used)]` at the top — the workspace `-D warnings` policy otherwise rejects bare `unwrap()`/`expect()` calls. See `src/db/tests/mod.rs` for the canonical pattern.

Tests must never sleep on the wall clock — not to "wait for" `spawn_blocking` or detached `tokio::spawn` work, and not to cross a duration threshold. Instead await a deterministic completion signal (oneshot / `Notify` / an `McpEvent`), inject a clock, or inject the threshold (`Database::set_slow_call_threshold`, used by `src/db/tests/async_handle.rs`). `./scripts/check-no-test-sleep.sh` (in the pre-push hook) enforces this: no `tokio::time::sleep` anywhere under `src/`/`tests/`, and no `std::thread::sleep` in test files (anything under `tests/`, under a `src/**/tests/` directory, or named `tests.rs`). Production `std::thread::sleep` is unaffected, and a deadline-bounded poll step may carry an `// allow-test-sleep: <why>` marker. See the "No `tokio::time::sleep` in tests" section of `docs/conventions.md` for the canonical patterns.

### Coverage

`cargo tarpaulin --out xml` runs in CI's `coverage` job (`--out Html` locally) — informational only, not gated, not in the pre-push hook. Line coverage sits around 85% as a rough, undated snapshot. Don't chase 100% on render-heavy code or `src/setup/`'s OS-interaction branches (hooks, filesystem writes) — a single file below the average is not by itself a problem.

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

Required on `PATH` at runtime, with **no startup preflight** — nothing checks binary availability, so a missing binary surfaces as a failed shell command mid-operation:

- **tmux** (`src/tmux.rs`) — every window/pane operation. Also a **test** dependency: the `tmux_*` integration targets need it (see "Running tests").
- **git** (`src/git.rs`, `src/dispatch/worktree.rs`, `src/dispatch/finish.rs`) — worktrees, rebase, branch detection.
- **gh** (`src/dispatch/mod.rs`, and the `scripts/fetch-*.sh` feed commands) — PR status and feed data. Network calls.
- **claude** — spawned inside the tmux window by `src/dispatch/agents.rs` as `claude --plugin-dir ~/.claude/plugins/local/dispatch --settings ~/.claude/dispatch-statusline.json …`. Both flags are always present and both are load-bearing: the plugin dir is installed by `cargo run -- setup`, and a **missing settings file aborts `claude` outright**, so `runtime::bootstrap` recreates it best-effort at TUI startup (generated by `src/setup/statusline.rs`; it is never `~/.claude/settings.json`).

The agent launchers read `claude`/`dispatch` from `ProcessRunner::agent_binaries()` (`src/process.rs`) rather than hardcoding them — see "One quoting layer per launch site" in `docs/conventions.md` before touching a launch site.

POSIX-only. Embeddings/RAG (`src/service/embeddings.rs`) run **locally** — `fastembed` does inference in-process on a dedicated OS thread, with no API key and no per-call network I/O. The only network activity is a one-time model download on first init.

## Verify Command

A per-repo, single-line shell command (e.g. `cargo test`) that dispatched agents must run before declaring work complete. Stored on the `repo_paths` row for the task's `repo_path`; set via the `set_verify_command` MCP tool or `cargo run -- repo set-verify <path> <command>`. Newlines and carriage returns are rejected — chain steps with `&&` or `;`. It never appears in the dispatch prompt. Instead it reaches the agent through two surfaces: `get_task`'s "Verify command" line (read by the `/wrap-up` skill in its Step 2, and acted on in its Step 7, before the closing sequence ever calls `wrap_up`), and, as a secondary reminder, the `wrap_up` response's action-specific "Verify before exiting" line (`src/mcp/handlers/tasks/wrap_up.rs::wrap_up_verify_line`). See `docs/specs/dispatch.allium` for why both exist.

## Working With the User

The most important thing is to stay aligned with the user. The Allium specs in `docs/specs/` are the shared source of truth that alignment is expressed in — when the spec and your intent agree, you are aligned; when they don't, one of them is wrong and it must be resolved before code is written.

- **Ambiguity is a stop condition, not a judgement call.** If the spec is silent, contradictory, or open to more than one reading, ask. Do not pick the plausible interpretation and proceed.
- **Behaviour changes start in the spec.** Spec first, then tests, then code (see the two sections below). This applies to UI and interaction behaviour too — that is a first-class Allium surface, not a prose note.
- **Agreement gets recorded, in one of two places.** A decision about *what the system does* goes into the relevant `docs/specs/*.allium` file. A decision about *how to work in this repo* — a convention, a pitfall, a gotcha that would trip the next agent — goes into the knowledge base via `record_learning`. A decision that lives only in the conversation is lost the moment the session ends.

## Test-Driven Development

Always use TDD. Express intended behaviour as tests before writing the code that satisfies them — for new features, bug fixes, and refactors alike.

## Allium Specification

The Allium specs in `docs/specs/` are the **source of truth** for domain and interaction behaviour:

- `core.allium` — domain model (entities, enums, config, VisualColumn)
- `tasks.allium` — task lifecycle (creation, status movement, reorder, archive, copy, editor)
- `dispatch.allium` — dispatching tasks, retry flows, repo-path persistence
- `agent-health.allium` — activity classification, crash detection, notifications, Claude Code hooks
- `pr-workflow.allium` — PR creation, polling, merge detection, and the agent-driven wrap-up/exit-session flow (there is no board-initiated finish path — wrap-up is MCP-only)
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
- `repo-sync.allium` — local-first repo sync: ahead/behind drift measurement, the sync operation and its typed failure vocabulary, and the surfaces that expose them

Consult the relevant spec before changing core behavior. Use `allium:tend` and `allium:weed` skills to keep spec and code aligned.

## MCP Tools for Agents

The `dispatch` MCP server exposes more than task creation. Worth knowing by name:

- **Knowledge base** — relevant learnings are already injected into your prompt at dispatch time ("Validated knowledge for this task" above); rate each one you act on with `rate_learning` (`helped`/`wrong`). Call `query_learnings` yourself for anything not already surfaced, and `record_learning` to capture a new pitfall/convention/tip. See `learnings.allium`.
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

Tick interval, DB refresh, status TTL, PR poll, message flash, main-session poll, and the gg-chord timeout are documented in "Timing Constants" in `docs/reference.md`.

## Documentation

This file is intentionally slim — it is loaded into every agent's context. Read these on demand:

> **`FieldUpdate`/`TaskPatch`** (nullable field mutations) is the most-touched pattern here — read [docs/conventions.md](docs/conventions.md) before writing an update handler; it also covers the `OwnedTaskPatch` parity hazard, now compiler-enforced via exhaustive destructuring.

> Bare `unwrap()`/`expect()` outside tests are clippy-warned but only hard-fail via `-D warnings` (the pre-push hook's `cargo clippy --all-targets -- -D warnings`) — a plain local `cargo build`/`cargo clippy` won't catch it. See the soft-fail-decoding section of `docs/conventions.md` for the fallback pattern.

> **Mutation boundary** (compiler-enforced): reads via `state.db` are fine, but task/epic mutations go through `TaskServiceApi`/`EpicServiceApi`, never the DB directly — `state.db` is typed `Arc<dyn db::TaskReadStore>`, so `state.db.patch_task(...)` from a handler is a compile error. `TaskReadStore` only seals task/epic writes; settings/learning/usage writes stay reachable through it. Sanctioned direct-mutation exceptions (own write handle, own invariants): `FeedRunner`, `TuiRuntime::feed_db`, and startup/CLI paths (`runtime::bootstrap`, `src/setup/`, `src/main.rs`) — but CLI handlers still route through `TaskService` (e.g. `cmd_plan` → `TaskService::attach_plan`). See "Sanctioned direct-mutation consumers" and `recalculate_epic_status` in `docs/conventions.md`.

> **Layout-cache coherence** (self-healing, not compiler-enforced): `App.layout` (`LayoutCache` — `epic_stats_cache`, `children_map_cache`, `column_anchor_cache`, `epic_filter_cache`, `task_index`, plus fingerprints) derives from `board.tasks`/`board.epics`. Call `invalidate_layout_cache()` after a mutation as a perf optimization, but `cached_epic_stats()` fingerprints the board on every call and self-heals on mismatch — a handler that forgets to invalidate cannot serve stale data. See `docs/architecture.md`.

> **DB connection model**: one writer `tokio_rusqlite::Connection` (`src/db/mod.rs`) — mutations serialize through it via `db_call` — plus a read-only pool via `db_call_read`, so concurrent reads don't queue behind the writer. See "DB access" in `docs/conventions.md`.

> **Render-panic policy**: a guarded `unreachable!()` in a render match arm is fine when an upstream filter/type already rules that arm out (e.g. `ColumnItem` variants stripped before the match in `src/tui/ui/kanban/columns.rs`) — but MCP handlers and `src/tui/input.rs` must never panic, guarded or not. See "Rendering purity" in `docs/conventions.md`.

> **Workhorse macros**: `patch_struct!` (`src/db/mod.rs:30`) generates `TaskPatch`/`EpicPatch`; `mcp_tools!` (`src/mcp/handlers/dispatch.rs:39`) generates the MCP tool registry. Read the macro's doc comment before adding a patch field or an MCP tool by hand.

- [docs/architecture.md](docs/architecture.md) — Message→Command, ProcessRunner, command queue draining, editor session invariant, layout-cache coherence (self-healing), render dirty flag (fail-open), error handling, quick dispatch
- [docs/conventions.md](docs/conventions.md) — the full convention set: `FieldUpdate`/`TaskPatch` double-Option, DB/service trait narrowing, the `run_bounded` primitive, keybinding telemetry, Clippy/visibility rules, and more (see the file's own headings for the complete list)
- [docs/module-map.md](docs/module-map.md) — module and subsystem responsibilities
- [docs/how-to.md](docs/how-to.md) — adding an MCP tool, TUI view, entity, database migration; knowledge base MCP tools
- [docs/mcp.md](docs/mcp.md) — MCP notification flow, error codes, debugging handlers, feed epics, knowledge base flow
- [docs/reference.md](docs/reference.md) — key bindings, configuration, environment variables, troubleshooting, learning store
- [docs/specs/](docs/specs/) — Allium specifications for domain logic
- [docs/plans/](docs/plans/) — implementation plans and one-off analysis/review docs

Subsystem entry points (no dedicated doc page — read the source):

- `src/feed/mod.rs` — feed system: `FeedRunner` poll loop, exec/parse/ingest pipeline that upserts tasks from external commands (see also `docs/module-map.md`)
- `src/service/repo_index/` (`mod.rs` orchestration + `scan.rs`/`chunking.rs`/`embed.rs`/`search.rs`), `src/service/embeddings.rs`, `src/mcp/handlers/repo_rag.rs` — repo indexing / embeddings / RAG: `index_repo` and `search_docs` MCP tools for semantic doc search
- `src/cli/` — CLI subcommand implementations (`agent_tree`, `caller_headers`)
- `src/mcp/trajectory.rs` — agent trajectory capture (records the agent's tool-call history for a task)
- `src/repo_sync.rs` — local-first repo sync: `ahead_behind` drift measurement and `sync_repo` (fetch, merge `origin/<base>`, push). Synchronous and `ProcessRunner`-driven like `src/dispatch/finish.rs`; local base history is never rewritten. See `docs/specs/repo-sync.allium`

## Unsafe Policy

Any `unsafe` block requires a `// SAFETY:` comment justifying why the invariant holds, and reviewer sign-off. See `docs/conventions.md` for the full policy.
