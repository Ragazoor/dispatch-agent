# How-To Guides

## Adding a New MCP Tool

The registry is **generated**. `tool_definitions()`, the `tools/call` dispatch arm, and `TOOL_NAMES` all expand from the `mcp_tools!` macro's single declarative list (`src/mcp/handlers/dispatch.rs:39`) — do not hand-edit any of the three, and read the macro's doc comment before adding an entry.

1. **Add the argument struct** next to its siblings — task tools in `src/mcp/handlers/tasks/mod.rs`, other domains in their own handler module. Derive `Deserialize`, and annotate every integer field with `#[serde(deserialize_with = "deserialize_flexible_i64")]` (from `src/mcp/handlers/types.rs`) since Claude Code may send them as strings.

2. **Define the handler** in the module matching the tool's domain (`src/mcp/handlers/tasks/crud.rs`, `tasks/dispatch.rs`, `tasks/wrap_up.rs`, `tasks/watch.rs`, `epics.rs`, `learnings.rs`, `managed_feeds.rs`). The signature is fixed by the macro:

   ```rust
   pub(crate) async fn handle_my_tool(
       state: &McpState,
       id: Option<Value>,
       _identity: &CallerIdentity,
       args: Value,
   ) -> JsonRpcResponse {
       let parsed: MyToolArgs = match parse_args(&id, args) { Ok(v) => v, Err(e) => return e };
       // …
   }
   ```

   Reads may go through `state.db`. **Mutations must not** — task and epic writes go through `state.task_svc` / `state.epic_svc` (`TaskServiceApi` / `EpicServiceApi`), because the service layer owns invariants like epic-status recalculation. `McpState.db` is typed `Arc<dyn db::TaskReadStore>` (`src/mcp/mod.rs:95`), so `state.db.patch_task(…)` is a **compile error**, locked in by a `compile_fail` doctest at `src/db/mod.rs:621`. <!-- allow-phantom-symbol: compile_fail is a rustdoc attribute, not our symbol --> Map service errors with `service_err_to_response`, and call `state.notify()` after a successful mutation so the TUI refreshes. See the [service mutation boundary](conventions.md#service-layer-is-the-mutation-boundary).

3. **Register the tool** by adding one entry to the `mcp_tools!` list in `src/mcp/handlers/dispatch.rs`: the `sync`/`async` kind, the tool name, the handler path, the description string, and the JSON input schema.

   ```rust
   async "my_tool" => tasks::handle_my_tool,
       "What the tool does, written for the agent that will read it.",
       {
           "type": "object",
           "properties": {
               "task_id": { "type": "integer", "description": "The task ID" }
           },
           "required": ["task_id"]
       };
   ```

4. **Write tests** in `src/mcp/handlers/tests/` (the file matching the tool's domain) using the helpers from `src/mcp/handlers/tests/mod.rs`. The canonical pattern:

   ```rust
   // For tools that only read state — use test_state():
   #[tokio::test]
   // allow-phantom-symbol: illustrative test name, not a real test
   async fn my_tool_returns_expected_data() {
       let state = test_state().await;
       let resp = call(&state, "tools/call",
           Some(json!({ "name": "my_tool", "arguments": { "id": 1 } }))).await;
       assert!(resp.error.is_none());
   }

   // For tools that trigger a fire-and-forget background write (e.g. usage
   // recording) — use test_state_with_bg_done() and await the signal
   // deterministically instead of sleeping:
   #[tokio::test]
   async fn my_tool_records_usage() {
       let (state, mut bg_done) = test_state_with_bg_done().await;
       call(&state, "tools/call",
           Some(json!({ "name": "my_tool", "arguments": {} }))).await;
       // Blocks until the spawned write completes — no tokio::time::sleep needed.
       bg_done.recv().await.expect("bg write signal lost");
   }
   ```

   Never use `tokio::time::sleep` in handler tests — the pre-push hook rejects it. See the "No `tokio::time::sleep` in tests" section of `docs/conventions.md` for the full rationale.

## Removing an MCP Tool

Reverse the steps above, but note that one test does **not** self-heal from the `mcp_tools!` macro like `tools_list_returns_tools` does: `every_tool_with_args_rejects_unknown_field` (`src/mcp/handlers/tests/mod.rs`) hand-lists every tool name in its `payloads`/`no_arg_tools` arrays. Deleting a tool's macro entry without also deleting its line there fails that test's `covered == all_tools` assertion at run time, not at compile time — `cargo build`/`cargo clippy` stay green.

1. Delete the `mcp_tools!` entry in `src/mcp/handlers/dispatch.rs` and the handler function/module.
2. Delete the tool's entry from `every_tool_with_args_rejects_unknown_field`'s `payloads` (or `no_arg_tools`) list.
3. Delete the tool's dedicated test file/module, and any doc references (`docs/module-map.md`, `CLAUDE.md`, relevant `docs/specs/*.allium`).
4. Run the full `cargo test` suite — grepping for the tool name is not enough to catch every reference; this hardcoded list is the proof.

## Adding a New TUI View/Mode

<!-- allow-phantom-symbol: `MyNewView` is the placeholder name for the variant you are adding -->
1. **Add a `ViewMode` variant** in `src/tui/types.rs` (e.g., `ViewMode::MyNewView { selection, saved_board }`).
2. **Add `Message` variants** for entering/exiting and any view-specific actions.
3. **Add `Command` variants** if the view triggers side effects (DB writes, shell commands).
4. **Handle input** in `src/tui/input.rs` — add key handlers under a new match arm for your `ViewMode`.
5. **Handle messages** in `src/tui/mod.rs` `update()` — process your new messages, return commands.
6. **Render** in the appropriate `src/tui/ui/` module — the board renderer is the `src/tui/ui/kanban/` directory (`mod.rs` holds `render()`, with `cards.rs`, `columns.rs`, `status_bar.rs`, and `popups/` beneath it); full-screen overlays live in `src/tui/ui/{input_form,todos}.rs`. Add a rendering branch for your view mode in `kanban::render()`.

## Adding a New Entity (with patch builder and sub-trait)

Adding a fully integrated entity involves five layers. Work through them in order:

1. **Domain model** (`src/models/`) — define the struct and any enums in the appropriate domain file. For nullable fields that agents or the TUI can set/clear, plan to use `FieldUpdate` (service layer) and `Option<Option<T>>` double-Option (DB layer); see the [FieldUpdate](conventions.md#fieldupdate--nullable-string-fields) and [TaskPatch/EpicPatch](conventions.md#taskpatch--epicpatch--double-option-in-the-db-layer) conventions.

2. **Database migration** (`src/db/migrations.rs`) — write `migrate_vN_description(conn)` and register it in `MIGRATIONS`. See [Adding a Database Migration](#adding-a-database-migration) for the full procedure.

3. **DB trait and queries** (`src/db/mod.rs`, `src/db/queries/`):
   - Define a narrow sub-trait (e.g., `trait NewEntityCrud`) with CRUD methods. Follow the [trait-narrowing convention](conventions.md#db-trait-narrowing--take-the-narrowest-sub-trait-you-need).
   - Add `NewEntityCrud` as a supertrait of the store the holders actually carry. `McpState` and `TuiRuntime` hold `Arc<dyn TaskReadStore>` (`src/mcp/mod.rs:95`), so a **read** trait belongs on `TaskReadStore`; a **mutating** trait belongs on `TaskStore` and stays out of `TaskReadStore` — that split is what makes bypassing the service layer a compile error.
   - Implement `impl NewEntityCrud for Database` under `src/db/queries/` (a new file per domain, wired into `src/db/queries/mod.rs`). Writes go through `self.db_call(|conn| …)`, pure reads through `self.db_call_read(|conn| …)`; there is no `self.conn()` accessor. See the [`db_call` / `db_call_read` convention](conventions.md#db-access--db_call--db_call_read).
   - Define a `NewEntityPatch` builder struct with `Option<Option<T>>` for nullable fields; implement the `UPDATE` query.
   - Write a corresponding `NewEntityFilter` if list queries need filtering.

4. **Service layer** (`src/service/<entity>.rs`) — create `NewEntityService` holding `Arc<dyn NewEntityCrud>`. Add `create_`, `get_`, `list_`, `update_`, and any lifecycle methods. Use `ServiceError::Validation` for input errors, `ServiceError::NotFound` for missing rows, and `anyhow` for DB I/O errors. Accept `FieldUpdate` for nullable string fields, map to `Option<Option<T>>` before writing the patch. Declare the new module in `src/service/mod.rs` and add `pub use` re-exports so callers are unaffected.

5. **MCP handler** (if agents need to interact) — follow [Adding a New MCP Tool](#adding-a-new-mcp-tool). For read-only tools, hold the narrowest sub-trait; for mutating tools, route the write through the service layer (never `state.db`) and call `state.notify()` afterwards.

6. **Tests**:
   - DB-layer tests in `src/db/tests/` (the file matching the entity's domain) using `Database::open_in_memory()`.
   - Service-layer tests inline in the corresponding `src/service/<entity>.rs` file.
   - MCP handler tests in `src/mcp/handlers/tests/` (the file matching the tool's domain) for any new tools.

7. **Spec** (`docs/specs/`) — write or extend an Allium spec to document the entity's lifecycle, rules, and invariants. Use the `allium:tend` skill and run `allium check` to validate syntax.

## Adding a Database Migration

Migrations live in `src/db/migrations.rs` as standalone functions. We do **not** squash migrations — see the module-level doc comment in `src/db/migrations.rs` for the policy.

1. **Write the migration function**: `fn migrate_vN_description(conn: &Connection) -> Result<()>` in `src/db/migrations.rs`. Use `ALTER TABLE` for additive changes; for destructive changes (column removal, constraint changes), create a new table, copy data, drop old, rename. **Do not** wrap the body in its own `BEGIN`/`COMMIT` or toggle `PRAGMA foreign_keys` — `apply_pending_migrations` (`src/db/mod.rs`) already runs the whole function inside one `BEGIN IMMEDIATE` transaction with FK checks toggled off around it (needed for table-rebuild migrations, harmless otherwise); an inner `BEGIN` would error ("cannot start a transaction within a transaction") and an inner FK toggle would silently no-op. If a migration genuinely needs a statement SQLite refuses to run inside a transaction (e.g. `VACUUM` — see `migrate_v71_create_backup`), split that step into its own function and call it from `init_schema_sync` before `apply_pending_migrations`, gated on `current_version < N`, rather than from inside the migration body.

   **A full-table-rebuild migration (needed for a CHECK-constraint change, since SQLite can't `ALTER` one in place) must not hardcode the current column list**, the way `migrate_v30_allow_conflict_for_review` did. Several existing migration tests (e.g. `migration_v38_feed_epic_columns`, `migration_v52_adds_verify_command_to_repo_paths`) seed a deliberately partial `tasks` schema at an old version and then call `init_schema_sync`, which replays *every later migration* — including yours — against that partial schema, not just the one the test names. A rebuild written against today's full column set references columns those synthetic seeds don't have, breaking migration tests with no relation to your feature. Discover the actual current columns/types/defaults at runtime instead (`pragma_table_info('tasks')`), and capture existing indexes/triggers from `sqlite_master` to replay verbatim after the rebuild (`DROP TABLE` implicitly drops everything attached to it) — see `migrate_v86_allow_stale_shell` for the pattern.
2. **Register it** in the `MIGRATIONS` array in `src/db/migrations.rs`: add `(N, migrate_vN_description)`. `apply_pending_migrations` (called from `init_schema_sync`, `src/db/mod.rs`) applies any migration where `current_version < N` and bumps `PRAGMA user_version` atomically with it, inside the same transaction — see that function's doc comment for why (task #3724: two connections opening the DB concurrently must not both apply the same migration).
3. **No manual version bump needed anywhere else.** `LATEST_SCHEMA_VERSION` in `src/db/migrations.rs` is derived from the last entry in `MIGRATIONS`, and every test in `src/db/tests/migrations.rs` that asserts a final schema version (including `fresh_db_has_latest_schema_version`) references that constant instead of a literal. Adding migration N updates all of them automatically — do **not** add a new one-off `fresh_db_has_schema_version_N` test or hardcode `assert_eq!(version, N)`; if you find yourself typing a literal version number in a test, use `LATEST_SCHEMA_VERSION` instead.
4. **Write a migration test** in `src/db/tests/migrations.rs` that creates a DB at the pre-migration schema, inserts test data, runs the migration, and verifies the result. To call the migration function directly from there it must be `pub(super)`, not private — `src/db/tests/` is a descendant of `crate::db`, so `pub(super)` on a fn in `src/db/migrations.rs` is exactly the visibility that reaches it.
   **A data fix-up migration must guard on every column its statement touches**, not just the one its feature added. Several tests build synthetic partial `tasks` tables from older eras, and an earlier `ALTER TABLE` can leave your new column present while older ones are absent — so a `column_exists` check for only the new column still panics with `no such column` on someone else's test. Guard the whole set and return `Ok(())` when any is missing; skipping a synthetic schema is correct for a data fix-up. See `migrate_v82_resolve_stranded_pending_stops`.

   **That guarding rule is specific to data fix-ups — schema migrations are not replay-safe, so do not try to test "an old DB still migrates forward" by replaying the chain.** Each one runs exactly once, in version order, and many are bare `ALTER TABLE`s: `migrate_v38_feed_epic_columns` fails with `duplicate column name: feed_command` if replayed over the current schema, and with `no such table: epics` against a synthetic fixture holding only your feature's table. Both fixtures are dead ends. Build the pre-migration state by calling the *original* migration function (the same `pub(super)` widening described above), populate it, then call your new migration directly — see `migration_84_drops_a_populated_v36_tips_state_table`. Pair it with a fresh-DB assertion that the artefact is present/absent as intended; `fresh_db_has_latest_schema_version` already covers the chain reaching the newest version.
5. **Cross-reference superseded migrations.** When a later migration drops or replaces a table/column introduced by an earlier one (create-then-drop pattern), add an inline comment on both `MIGRATIONS` entries noting the relationship — e.g. `// superseded by vN` on the original and `// drops table created in vM` on the new one. This prevents agents from trying to re-add something that was intentionally removed.

## Knowledge Base MCP Tools

Four MCP tools manage the knowledge base from within an agent session:

- **`record_learning`** — record a new entry in the knowledge base (immediately active in future dispatch prompts)
- **`query_learnings`** — retrieve approved entries relevant to the current task's context; supports `tag_filter` and `limit`
- **`rate_learning`** — give feedback on a retrieved entry: `helped` increments `upvote_count`; `wrong` decrements it (a downvote; may go negative) without changing status
- **`delete_learning`** — permanently delete a knowledge base entry by ID; returns an error if the ID does not exist

**When to call these tools:**
- Call `query_learnings` at the right moment — not just at task start.
- Call `record_learning` when you discover a pattern worth capturing for future agents (pitfall, convention, landscape, etc.).
- Call `rate_learning` when you act on a retrieved entry — `helped` if it applied, `wrong` if it misled you. Only entries surfaced to you this task (injected or returned by `query_learnings`) can be rated.

**Scope auto-derivation:** omit `scope_ref` — the MCP handler derives it from the task's repo or epic automatically. Pass `scope_ref` explicitly only to override.

**Task-scoped learnings** are not auto-injected into dispatch prompts. Use `query_learnings` with `tag_filter` to retrieve them when needed.

**Scopes at retrieval time**: a `query_learnings` call for a task returns the union of all approved learnings where:
- `scope = user` (always included)
- `scope = repo` and `scope_ref` matches the task's repo path
- `scope = epic` and `scope_ref` matches the task's epic (only if the task belongs to an epic)

See `docs/reference.md` → *Learning Store* for the full scoping model with examples.
