# Module Map

Module and subsystem responsibilities. Rows are at whatever granularity is
useful — a single file where it earns one, a directory where the files inside
share a job. It is deliberately *not* one row per file (there are over 200
`.rs` files); if a file you need is not listed, its directory's row says where
to look.

| File | Responsibility |
|------|---------------|
| `src/main.rs` | CLI entry point (clap), subcommand dispatch (`tui`, `setup`, `verify-feed`, `repo`, …), global `--db` flag, `app.log` tracing subscriber |
| `src/lib.rs` | Crate root, public module re-exports, `DEFAULT_PORT`, `default_db_path()` |
| `src/cli/mod.rs` | CLI submodule declarations (`agent_tree`, `caller_headers`) |
| `src/cli/caller_headers.rs` | `dispatch caller-headers` — pure CWD→identity-header resolver used as Claude Code's `headersHelper`, so an agent's MCP calls carry `X-Caller-Task-Id`. No DB, no network, no async |
| `src/cli/agent_tree.rs` | `dispatch agent-tree <task_id>` — standalone ratatui companion-pane renderer, deliberately not part of the board TUI's `App`/message loop. Converts subtask 3's `agent_tree::TreeNode` into `tui_tree_widget` items with `[Modified]`/`[Read]` badges (`build_tree_items`), tracks manual expand/collapse across redraws so only newly-touched directories auto-open (`RenderState`), maps one key press onto that view state in a terminal-free `handle_key` (vim motions and arrows as aliases; it takes the tree so expand/toggle can be guarded on directories — the widget's own `open()` has no leaf guard), and `run()` polls the task's file-events JSONL on a 1-second timer (see `docs/specs/agent-tree.allium`'s `AgentTreeCompanionPane` surface) |
| `src/cli/statusline.rs` | `dispatch statusline` decorator: records the subscription rate-limit windows from Claude Code's statusLine hook payload to a snapshot file, then runs the user's previous statusLine command and prints its output verbatim. Never fails (always exits 0) and never opens the database — see the module doc comment |
| `src/runtime/mod.rs` | Async event loop (`tokio::select!`), bridges TUI ↔ MCP ↔ shell commands; `TICK_INTERVAL`, `execute_commands` |
| `src/runtime/commands.rs` | `Command` side-effect dispatcher (called by `execute_commands`) |
| `src/runtime/tasks.rs` | Per-command runtime handlers for tasks (refresh, dispatch, finish, etc.) |
| `src/runtime/{editor,epics,learnings,pr,settings,split,todos}.rs` | Domain-specific runtime helpers. (`src/runtime/agents.rs` is a vestigial empty `impl TuiRuntime {}` — nothing lives there) |
| `src/tui/mod.rs` | `App` struct, lifecycle, `update()` entry point, timing constants (`STATUS_MESSAGE_TTL`, `PR_POLL_INTERVAL`, `MAIN_SESSION_POLL_TICKS`, `GG_CHORD_TIMEOUT`). Column-listing helpers: `column_items_for_status_with_stats` (production render path — requires pre-computed `EpicStatsMap`, used by kanban columns); `column_items_for_visual_column` (snapshot/archive views — filters by `VisualColumn` granularity, no stats needed). `column_items_for_status` is test-only. |
| `src/tui/dispatcher.rs` | `dispatch(app, msg)` — thin top-level router: one arm per outer `Message` domain, delegating to that domain's inner-enum `route(self, app)` method |
| `src/tui/messages/` | Per-domain inner `*Message` enums (`task.rs`, `epic.rs`, `system.rs`, `split.rs`, `todos.rs`, …). Each also owns its per-variant routing via an inherent `route(self, app) -> Vec<Command>` method co-located with the enum — see `docs/architecture.md` "Message routing (co-located)" |
| `src/tui/commands/` | Per-domain inner `*Command` enums (`task.rs`, `epic.rs`, `editor.rs`, `feed.rs`, `split.rs`, `todos.rs`, …) — the command twin of `src/tui/messages/`. Variants of the outer `Command` enum are progressively migrated here. Unlike `messages/`, these are pure data: `Command` → effect stays centralised in `src/runtime/commands.rs` |
| `src/tui/update/` | Per-message handlers (`agent.rs`, `epics.rs`, `feeds.rs`, `forms.rs`, `lifecycle.rs`, `main_session.rs`, `move_task.rs`, `navigation.rs`, `pr.rs`, `repo_filter.rs`, `retry.rs`, `selection.rs`, `split_pane.rs`, `system.rs`, `todos.rs`) |
| `src/tui/input.rs` | Key event entry point, `text_edit_message()` caret routing, inline-mutation convention for UI-only state, unconditional `dirty = true` |
| `src/tui/input/` | Per-mode key handlers: `normal.rs`, `confirm.rs`, `repo_filter.rs` |
| `src/tui/text_caret.rs` | Pure single-line caret mechanics (`insert`, `delete_before`, `move_left`, `word_left`, `byte_offset`, …) shared by every text `InputMode` — see the caret convention in `docs/conventions.md` |
| `src/tui/ui/mod.rs` | Rendering entry point — re-exports `render()`, thin dispatcher |
| `src/tui/ui/kanban/` | Kanban board rendering: `mod.rs` entry, `cards.rs`, `columns.rs`, `status_bar.rs`, `tests.rs`, `popups/` overlays (`help.rs`, `error.rs`, `task_detail.rs`, `reparent_epic.rs`, `repo_filter.rs`) |
| `src/tui/ui/shared.rs` | Cross-board helpers: `refresh_status`, `truncate`, `fair_truncate_segments`, `push_hint_spans`, `caret_line` |
| `src/tui/ui/palette.rs` | Tokyo Night color palette constants |
| `src/tui/ui/{input_form,todos}.rs` | Overlay renderers (input forms, TODO overlay) |
| `src/tui/types.rs` | `Message`, `Command`, `ViewMode`, `InputMode`, `LayoutCache`, `AgentTracking` enums and structs |
| `src/tui/tests/` | TUI unit and scenario tests, snapshots, helpers |
| `src/models/mod.rs` | Module declarations + flat re-exports of all domain types (no logic, no tests) |
| `src/models/tasks.rs` | `Task`, `TaskStatus`, `SubStatus`, `TaskTag` (+ `is_review()`), `DispatchMode::for_task()` tag routing, `slugify`, age formatting |
| `src/models/{epics,learnings,review,todos,usage}.rs` | Domain types per area. `review.rs` holds `ReviewDecision` and `pr_number_from_url` |
| `src/models/url.rs` | `TaskUrl` / `UrlType` — the typed URL on a task (PR, issue, security alert), stored explicitly rather than sniffed |
| `src/models/ids.rs` | `define_id_newtype!` macro behind `TaskId`/`EpicId`/`LearningId`/`TodoId` |
| `src/models/string_enum.rs` | `define_str_enum!` macro behind `TaskStatus`/`SubStatus`/`TaskTag`/`WrapUpMode` string conversions |
| `src/models/paths.rs` | `expand_tilde` path utility |
| `src/models/columns.rs` | `VisualColumn` kanban board layout |
| `src/service/mod.rs` | Service module root: `ServiceError`, `FieldUpdate`, `UrlUpdate`, re-exports of all sub-module types |
| `src/service/tasks/mod.rs` | `TaskService` — task business logic |
| `src/service/tasks/{crud,params,validators}.rs` | Task CRUD methods, `*Params` request types, validation helpers |
| `src/service/tasks/watchers.rs` | Task-watcher subscriptions: `subscribe`/`unsubscribe` plus the completion notice fired when a watched task reaches `Done`/`Archived` or is deleted (see `docs/specs/task-watchers.allium`) |
| `src/service/epics.rs` | `EpicService`, `UpdateEpicParams`, `CreateEpicParams` — epic business logic, including reparenting with cycle detection |
| `src/service/learnings.rs` | `LearningService`, `CreateLearningParams` — learning business logic (curated exclusively via MCP; no TUI-facing update/reject/archive path) |
| `src/service/api.rs` | Service trait objects (`TaskServiceApi`, `EpicServiceApi`, `TodoServiceApi`, `LearningServiceApi`) + `MockLearningService` for injection in tests. Each seam's signature list lives once, in a spec macro (`task_service_api!`, …) replayed into emitter macros that generate the trait, the delegating impl, and the test-only `*ServiceApiStub` mock scaffolding |
| `src/service/todos.rs` | `TodoService` — personal TODO overlay business logic |
| `src/service/grouping.rs` | Repo-grouping: routes tasks of a `group_by_repo` epic into per-repo `RepoGroup` sub-epics |
| `src/service/managed_feeds.rs` | Managed feed config read/write (`get`/`set_managed_feed_config`) |
| `src/service/embeddings.rs` | `EmbeddingService` — text embedding computation used by RAG and learning search |
| `src/service/clock.rs` | `Clock` trait + `SystemClock`/`FixedClock` for injectable time in services/tests |
| `src/service/repo_index/mod.rs` | Repo-index orchestration: `index_repo` / `search_docs` driver |
| `src/service/repo_index/{scan,chunking,embed,search}.rs` | RAG pipeline: source scan, chunking, embedding, vector search |
| `src/db/mod.rs` | `Database` struct, `db_call` (writer) / `db_call_read` (read pool), the `*Store` trait hierarchy (`TaskStore`, `TaskReadStore`, …), `patch_struct!` behind the `TaskPatch`/`EpicPatch` builders |
| `src/db/migrations.rs` | Versioned schema migrations (`MIGRATIONS` array, `migrate_vN_*` functions, `LATEST_SCHEMA_VERSION`) |
| `src/db/queries/mod.rs` | `impl TaskStore for Database` — fans out across the per-domain query files; `set_field!` macro and the soft-fail row decoders (`row_to_task`, `row_to_epic`) |
| `src/db/queries/{tasks,epics,learnings,settings,todos,usage}.rs` | CRUD per domain |
| `src/db/queries/subagents.rs` | `task_subagents` CRUD with session fencing, keeping `tasks.live_subagents` in step |
| `src/db/tests/mod.rs` | Database unit tests entry point |
| `src/db/tests/{tasks,epics,learnings,settings,todos,usage,migrations,async_handle,read_pool}.rs` | Tests per domain, plus the async-handle and read-pool behaviour tests |
| `src/dispatch/mod.rs` | Dispatch module root: PR-status polling via `gh` (`check_pr_status`, `pr_head_branch`) and repo-path/URL helpers (`repo_name_from_path`, `extract_github_repo`, `resolve_repo_path`, `resolve_feed_item_repo_paths`) |
| `src/dispatch/agents.rs` | The agent launchers — `dispatch_agent`, `research_agent`, `quick_dispatch_agent`, `resume_agent` — plus `fetch_verify_command`. Each provisions a worktree, writes the prompt file, and starts `claude` inside a tmux window |
| `src/dispatch/prompts.rs` | Prompt construction: `build_prompt` (with-plan / no-plan / review variants), `build_quick_dispatch_prompt`, `build_research_prompt`, knowledge-block and verification rendering |
| `src/dispatch/prompts/` | Markdown bodies for the two review addenda (`pr-review.md`, `dependabot.md`), inlined via `include_str!` |
| `src/dispatch/prompts_snapshots.rs` | Insta snapshot tests locking the rendered output of every `build_*_prompt` variant (snapshots in `src/dispatch/snapshots/`) |
| `src/dispatch/worktree.rs` | Worktree creation/teardown, `.dispatch/` directory + gitignore bootstrap |
| `src/dispatch/trust.rs` | Reads and writes Claude Code's per-project trust flag in `~/.claude.json` so a fresh worktree doesn't stall on the trust prompt |
| `src/dispatch/finish.rs` | Rebase + fast-forward branch onto base branch (`finish_task`); git only — the session teardown is the caller's, gated on the task's terminal write. Defines `FinishError` |
| `src/dispatch/split_panes.rs` | Multi-step tmux sequences behind the board's split-pane feature: `join_task_window_into_pane` (pin, killing the leftover agent-tree companion) and `swap_task_window_into_pane` (swap + rename/kill + companion resync). Lives here rather than in `src/tmux.rs` because it carries policy and calls `resync_agent_tree_pane`; `src/runtime/split.rs` keeps only the `spawn_blocking` + message emission |
| `src/feed/mod.rs` | `FeedRunner` struct, poll loop, `tick()` orchestration — composes exec/parse/ingest; re-exports `resolve_base_branches` |
| `src/feed/exec.rs` | `resolve_base_branches()` (cached per-path git lookup), `exec_feed_command()` (async shell spawn + stdout capture) |
| `src/feed/parse.rs` | `parse_feed_items()` — JSON → `Vec<FeedItem>` deserialization |
| `src/feed/routing.rs` | `route()` — pure signal→`FeedRole` mapping for the PR-review feed. Not to be confused with `src/feed/ingest/routing.rs`, which groups already-routed entries |
| `src/feed/ingest/mod.rs` | `FeedItemWithTarget` (shared entry type), `run_feed_sync_by_role()` / `run_feed_sync()` — dispatch an emission to the right sync strategy |
| `src/feed/ingest/grouped.rs` | `sync_grouped_feed()` — `group_by_repo` path: groups items by repo, creates/reuses sub-epics, upserts tasks |
| `src/feed/ingest/role_routed.rs` | `run_role_routed_feed_sync()` — `reviews_parent` path: role sub-epic scaffolding + subtree reconcile orchestration |
| `src/feed/ingest/routing.rs` | `route_and_group_entries()` — role-routed phase 1: route each entry to its target sub-epic and group |
| `src/feed/ingest/upsert.rs` | `upsert_role_groups()` — role-routed phase 2: insert/update present role groups |
| `src/feed/ingest/stale.rs` | `delete_stale_subtree()` / `clear_parent_stranded_tasks()` — role-routed phase 3: delete absent tasks + clear the parent |
| `src/process.rs` | `ProcessRunner` trait + `RealProcessRunner` / `MockProcessRunner` for testable shell execution |
| `src/tmux.rs` | Tmux API: create windows, send keys, capture pane output, kill windows |
| `src/git.rs` | The shared git plumbing both mutating operations gate on. `has_origin_remote` / `current_branch` / `dirty_files` are the three preflight reads that `dispatch::finish::finish_task` (`src/dispatch/finish.rs`) and `repo_sync::sync_repo` (`src/repo_sync.rs`) both run before writing — each caller decides what a failed check *means* in its own error type, but the read itself lives here once. All three return a `Result`, so "the probe could not be run" stays distinguishable from whatever the probe would have said: `has_origin_remote` in particular must never report a spawn failure as "no origin remote configured", since callers branch on that absence. `parse_porcelain_files` / `parse_unmerged_files` (over the shared `porcelain_entries` splitter) give "is this checkout dirty?" and "did that merge conflict?" exactly one answer each. Also `detect_default_branch` via `origin/HEAD`. **Look here before adding another `git status --porcelain` or `rev-parse --abbrev-ref HEAD` call** |
| `src/notify.rs` | Shared notification delivery (`write_message_file` / `notify_tmux` / `deliver`) — writes a message file into a task's worktree and injects a tmux nudge; used by `send_message` (`src/mcp/handlers/tasks/dispatch.rs`) and task-watcher completion notices (`src/service/tasks/watchers.rs`) |
| `src/editor.rs` | External `$EDITOR` integration for editing task/epic fields. Also the one editor resolver — `resolve_editor` / `editor_from_env` (`$VISUAL`, else `$EDITOR`, else `vi`, split into argv) — shared by the pop-out editor (`src/runtime/editor.rs`) and the agent-tree editor pane (`src/agent_tree_editor.rs`) |
| `src/plan.rs` | Plan file parsing (extract title/description from markdown) |
| `src/file_events.rs` | Per-task file-events JSONL log: `append_file_event` appends one `{schema_version, timestamp, task_id, tool, path, operation}` line to `<data_dir>/file-events/<task_id>.jsonl`, called from `dispatch hook-file-event` (see `docs/specs/agent-tree.allium`'s `CaptureFileEvent` rule) |
| `src/agent_tree.rs` | Pure JSONL-events→tree logic: `build_tree(root, jsonl)` parses a task's file-events log into an in-memory `TreeNode` tree with Read/Modified badges (Modified wins) and auto-expansion flags on touched directories. No I/O, no rendering (see `docs/specs/agent-tree.allium`'s `RefreshAgentTree` rule) |
| `src/setup/mod.rs` | First-run setup entry point |
| `src/setup/{config,plugins,hooks}.rs` | MCP config merging, plugin installation, git hook installation |
| `src/setup/statusline.rs` | Generates `~/.claude/dispatch-statusline.json`, the `--settings` file that wires the `dispatch statusline` decorator into every dispatch-spawned Claude session; discovers the user's pre-existing statusLine command to chain to |
| `src/mcp/mod.rs` | MCP server bootstrap (Axum router), `McpState`, `McpEvent` notification enum |
| `src/mcp/identity.rs` | `CallerIdentity` / `IdentityError` and `from_headers` — parses `X-Caller-Task-Id` / `X-Caller-Kind` into a typed caller |
| `src/mcp/middleware.rs` | `extract_caller_identity` Axum middleware — attaches `Result<CallerIdentity, IdentityError>` to every request's extensions |
| `src/mcp/trajectory.rs` | Per-task audit log of MCP tool calls, appended under the worktree's `trajectories/` dir (see `docs/specs/observability.allium`) |
| `src/mcp/handlers/dispatch.rs` | JSON-RPC entry point (`handle_mcp`) plus the `mcp_tools!` macro that generates `tool_definitions()`, `dispatch_tool()`, and `TOOL_NAMES` from one declarative tool list |
| `src/mcp/handlers/tasks/mod.rs` | Task arg structs, shared response helpers, re-exports |
| `src/mcp/handlers/tasks/crud.rs` | CRUD task handlers: `update_task`, `create_task`, `get_task`, `list_tasks`, `query_usage` |
| `src/mcp/handlers/tasks/dispatch.rs` | Dispatch handlers: `dispatch_task`, `send_message`, plus `auto_dispatch_next` (the epic chain fired by `exit_session`) |
| `src/mcp/handlers/tasks/wrap_up.rs` | Wrap-up handlers: `wrap_up`, `exit_session` |
| `src/mcp/handlers/tasks/verify.rs` | Verify handler: `set_verify_command` |
| `src/mcp/handlers/tasks/watch.rs` | Task-watcher handlers: `subscribe_to_task`, `unsubscribe_from_task` |
| `src/mcp/handlers/epics.rs` | Epic tool handlers (thin wrappers): parse JSON-RPC args → call `EpicService` → format response |
| `src/mcp/handlers/learnings.rs` | Knowledge base tool handlers |
| `src/mcp/handlers/managed_feeds.rs` | Managed feed config tool handlers (`get`/`set_managed_feed_config`) |
| `src/mcp/handlers/repo_rag.rs` | Repo-RAG tool handlers: `index_repo`, `search_docs` |
| `src/mcp/handlers/types.rs` | JSON-RPC request/response types, flexible integer deserializer |
| `src/mcp/handlers/tests/mod.rs` | MCP handler integration tests entry point |
| `src/mcp/handlers/tests/tasks/mod.rs` | Task test entry point: module declarations and shared helpers |
| `src/mcp/handlers/tests/tasks/{crud,dispatch,wrap_up,verify,watch}.rs` | Task handler tests per sub-domain |
| `src/mcp/handlers/tests/{epics,learnings,managed_feeds,repo_rag,usage}.rs` | MCP handler tests per domain |
