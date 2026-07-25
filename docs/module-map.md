# Module Map

| File | Responsibility |
|------|---------------|
| `src/main.rs` | CLI entry point (clap), subcommand dispatch (`tui`, `setup`, `verify-feed`, …) |
| `src/lib.rs` | Crate root, public module re-exports |
| `src/runtime/mod.rs` | Async event loop (`tokio::select!`), bridges TUI ↔ MCP ↔ shell commands |
| `src/runtime/commands.rs` | `Command` side-effect dispatcher (called by `execute_commands`) |
| `src/runtime/tasks.rs` | Per-command runtime handlers for tasks (refresh, dispatch, finish, etc.) |
| `src/runtime/{agents,epics,learnings,pr,settings,split,editor}.rs` | Domain-specific runtime helpers |
| `src/tui/mod.rs` | `App` struct, lifecycle, `update()` entry point. Column-listing helpers: `column_items_for_status_with_stats` (production render path — requires pre-computed `EpicStatsMap`, used by kanban columns); `column_items_for_visual_column` (snapshot/archive views — filters by `VisualColumn` granularity, no stats needed). `column_items_for_status` is test-only. |
| `src/tui/dispatcher.rs` | `dispatch(app, msg)` — thin top-level router: one arm per outer `Message` domain, delegating to that domain's inner-enum `route(self, app)` method |
| `src/tui/messages/` | Per-domain inner `*Message` enums (`task.rs`, `epic.rs`, `input.rs`, `system.rs`, `split.rs`, …). Each also owns its per-variant routing via an inherent `route(self, app) -> Vec<Command>` method co-located with the enum — see `docs/architecture.md` "Message routing (co-located)" |
| `src/tui/update/` | Per-message handlers (`agent.rs`, `epics.rs`, `feeds.rs`, `forms.rs`, `learnings.rs`, `lifecycle.rs`, `main_session.rs`, `navigation.rs`, `pr.rs`, `repo_filter.rs`, `retry.rs`, `selection.rs`, `split_pane.rs`, `system.rs`, `tips_projects.rs`, `wrap_up.rs`) |
| `src/tui/input.rs` | Key event entry point, inline-mutation convention for UI-only state |
| `src/tui/input/` | Per-mode key handlers: `normal.rs`, `confirm.rs`, `projects.rs`, `repo_filter.rs` |
| `src/tui/ui/mod.rs` | Rendering entry point — re-exports `render()`, thin dispatcher |
| `src/tui/ui/kanban/` | Kanban board rendering: `mod.rs` entry, `cards.rs`, `columns.rs`, `status_bar.rs`, `projects_panel.rs`, `popups/` overlays |
| `src/tui/ui/shared.rs` | Cross-board helpers: `render_tab_bar`, `refresh_status`, `truncate`, `push_hint_spans` |
| `src/tui/ui/palette.rs` | Tokyo Night color palette constants |
| `src/tui/ui/{input_form,learnings}.rs` | Overlay renderers (input forms, knowledge base panel) |
| `src/tui/types.rs` | `Message`, `Command`, `ViewMode`, `InputMode`, `AgentTracking` enums and structs |
| `src/tui/tests/` | TUI unit and scenario tests, snapshots, helpers |
| `src/models/mod.rs` | Module declarations + flat re-exports of all domain types (no logic, no tests) |
| `src/models/tasks.rs` | `Task`, `TaskStatus`, `SubStatus`, `TaskTag`, `DispatchMode::for_task()` tag routing, `slugify`, age formatting |
| `src/models/{epics,learnings,review}.rs` | Domain types per area |
| `src/models/ids.rs` | `define_id_newtype!` macro behind `TaskId`/`EpicId`/`LearningId` |
| `src/models/string_enum.rs` | `define_str_enum!` macro behind `TaskStatus`/`SubStatus`/`TaskTag`/`WrapUpMode`/`TipsShowMode` string conversions |
| `src/models/paths.rs` | `expand_tilde` path utility |
| `src/models/columns.rs` | `VisualColumn` kanban board layout |
| `src/service/mod.rs` | Service module root: `ServiceError`, `FieldUpdate`, re-exports of all sub-module types |
| `src/service/tasks/mod.rs` | `TaskService` — task business logic |
| `src/service/tasks/{crud,params,validators}.rs` | Task CRUD methods, `*Params` request types, validation helpers |
| `src/service/epics.rs` | `EpicService`, `UpdateEpicParams`, `CreateEpicParams` — epic business logic |
| `src/service/learnings.rs` | `LearningService`, `CreateLearningParams`, `UpdateLearningParams` — learning business logic |
| `src/service/api.rs` | Service trait objects (`TaskServiceApi`, `EpicServiceApi`, `TodoServiceApi`, `LearningServiceApi`) + `MockLearningService` for injection in tests. Each seam's signature list lives once, in a spec macro (`task_service_api!`, …) replayed into emitter macros that generate the trait, the delegating impl, and the test-only `*ServiceApiStub` mock scaffolding |
| `src/service/todos.rs` | `TodoService` — personal TODO overlay business logic |
| `src/service/grouping.rs` | Repo-grouping: routes tasks of a `group_by_repo` epic into per-repo `RepoGroup` sub-epics |
| `src/service/managed_feeds.rs` | Managed feed config read/write (`get`/`set_managed_feed_config`) |
| `src/service/embeddings.rs` | `EmbeddingService` — text embedding computation used by RAG and learning search |
| `src/service/clock.rs` | `Clock` trait + `SystemClock`/`FixedClock` for injectable time in services/tests |
| `src/service/repo_index/mod.rs` | Repo-index orchestration: `index_repo` / `search_docs` driver |
| `src/service/repo_index/{scan,chunking,embed,search}.rs` | RAG pipeline: source scan, chunking, embedding, vector search |
| `src/db/mod.rs` | `Database` struct, constructor, `TaskStore` trait, `TaskPatch`/`EpicPatch` builders |
| `src/db/migrations.rs` | Versioned schema migrations (`MIGRATIONS` array, `migrate_vN_*` functions) |
| `src/db/queries/mod.rs` | `impl TaskStore for Database` — fans out across the per-domain query files |
| `src/db/queries/{tasks,epics,prs,alerts,projects,learnings,settings}.rs` | CRUD per domain |
| `src/db/tests/mod.rs` | Database unit tests entry point |
| `src/db/tests/{tasks,epics,prs,alerts,projects,learnings,settings,migrations}.rs` | Tests per domain |
| `src/dispatch/mod.rs` | Worktree creation, tmux session management, agent lifecycle (dispatch/brainstorm/plan/resume/review) |
| `src/dispatch/agents.rs` | Agent-specific dispatch helpers |
| `src/dispatch/prompts.rs` | Prompt construction (with-plan, no-plan variants, learning injection) |
| `src/dispatch/worktree.rs` | Worktree creation/teardown |
| `src/dispatch/finish.rs` | Rebase + fast-forward branch onto base branch, kill tmux window (`finish_task`); defines `FinishError` |
| `src/feed/mod.rs` | `FeedRunner` struct, poll loop, `tick()` orchestration — composes exec/parse/ingest; re-exports `resolve_base_branches` |
| `src/feed/exec.rs` | `resolve_base_branches()` (cached per-path git lookup), `exec_feed_command()` (async shell spawn + stdout capture) |
| `src/feed/parse.rs` | `parse_feed_items()` — JSON → `Vec<FeedItem>` deserialization |
| `src/feed/ingest/mod.rs` | `FeedItemWithTarget` (shared entry type), `run_feed_sync_by_role()` / `run_feed_sync()` — dispatch an emission to the right sync strategy |
| `src/feed/ingest/grouped.rs` | `sync_grouped_feed()` — `group_by_repo` path: groups items by repo, creates/reuses sub-epics, upserts tasks |
| `src/feed/ingest/role_routed.rs` | `run_role_routed_feed_sync()` — `reviews_parent` path: role sub-epic scaffolding + subtree reconcile orchestration |
| `src/feed/ingest/routing.rs` | `route_and_group_entries()` — role-routed phase 1: route each entry to its target sub-epic and group |
| `src/feed/ingest/upsert.rs` | `upsert_role_groups()` — role-routed phase 2: insert/update present role groups |
| `src/feed/ingest/stale.rs` | `delete_stale_subtree()` / `clear_parent_stranded_tasks()` — role-routed phase 3: delete absent tasks + clear the parent |
| `src/process.rs` | `ProcessRunner` trait + `RealProcessRunner` / `MockProcessRunner` for testable shell execution |
| `src/tmux.rs` | Tmux API: create windows, send keys, capture pane output, kill windows |
| `src/editor.rs` | External `$EDITOR` integration for editing task/epic fields |
| `src/plan.rs` | Plan file parsing (extract title/description from markdown) |
| `src/setup/mod.rs` | First-run setup entry point |
| `src/setup/{config,plugins,hooks}.rs` | MCP config merging, plugin installation, git hook installation |
| `src/mcp/mod.rs` | MCP server bootstrap (Axum router), `McpState`, `McpEvent` notification enum |
| `src/mcp/handlers/dispatch.rs` | JSON-RPC entry point (`handle_mcp`), tool definitions, method routing |
| `src/mcp/handlers/tasks/mod.rs` | Task arg structs, shared response helpers, re-exports; epic/learning/project tests |
| `src/mcp/handlers/tasks/crud.rs` | CRUD task handlers: `update_task`, `create_task`, `get_task`, `list_tasks`, `query_usage` |
| `src/mcp/handlers/tasks/dispatch.rs` | Dispatch handlers: `claim_task`, `dispatch_next`, `dispatch_task`, `send_message` |
| `src/mcp/handlers/tasks/wrap_up.rs` | Wrap-up handlers: `wrap_up`, `exit_session` |
| `src/mcp/handlers/tasks/verify.rs` | Verify handler: `set_verify_command` |
| `src/mcp/handlers/epics.rs` | Epic tool handlers (thin wrappers): parse JSON-RPC args → call `EpicService` → format response |
| `src/mcp/handlers/learnings.rs` | Knowledge base tool handlers |
| `src/mcp/handlers/types.rs` | JSON-RPC request/response types, flexible integer deserializer |
| `src/mcp/handlers/tests/mod.rs` | MCP handler integration tests entry point |
| `src/mcp/handlers/tests/tasks/mod.rs` | Task test entry point: module declarations, epic/learning/project tests |
| `src/mcp/handlers/tests/tasks/{crud,dispatch,wrap_up,verify}.rs` | Task handler tests per sub-domain |
| `src/mcp/handlers/tests/{epics,learnings,projects}.rs` | MCP handler tests per domain |
