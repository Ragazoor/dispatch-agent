# Knowledge base prune — record of what was deleted

Task #4402, run 2026-08-27. This is the audit trail for a one-off batch delete of
300 of the 371 approved knowledge base entries. `delete_learning` is permanent
and leaves nothing behind, so the list below is the only surviving record of what
was in the pool.

A snapshot of the database as it stood immediately before the batch was taken to
`/tmp/claude-1000/kb.db`. That is scratch space and will not survive a reboot —
if anything here needs recovering, do it from that file or from a
`~/.local/share/dispatch/tasks.db.bak-*` backup, not from this document.

Neither doc checker scans `docs/plans/`, which is why the stale symbol names
quoted below do not fail the push gate.

## The decision this implements

The user, 2026-08-27:

> Naming functions/implementation details risk going stale at any moment due to
> a refactor. We should not save these, does not matter if some are upvoted.

The rule: an entry describes durable behaviour, a convention, or a domain fact.
It does not name the function, type, macro, fixture, test, or file that
currently implements it. Upvote count does not exempt an entry — the argument is
rot, not present usefulness. Where a precise claim is worth keeping precisely, it
belongs in the Allium spec or a Rust doc comment, both of which
`check-doc-symbols.sh` re-checks on every push. Nothing re-checks the knowledge
base.

Salvage policy, decided in the same session: **delete only.** No rewrites, no
migration of the underlying facts into docs. "If it is worth keeping we will find
the learning again."

## Result

| | |
|---|---:|
| Approved before | 371 |
| Deleted | 300 |
| Survivors | 71 |

Four entries (#494–#497) were recorded by concurrent sessions while the batch
ran, and #492 was deleted by another session, so the live count immediately after
was 74.

## Filters

Counts overlap — an entry can match more than one.

| Filter | Matched |
|---|---:|
| Names implementation detail — in the **summary** | 159 |
| Names implementation detail — in the **detail** only | 130 |
| Old and never rated (created before 2026-06-28, score ≤ 0) | 50 |
| Transient / one-shot | 4 |
| Superseded or wrong | 1 |

### How "names implementation detail" was decided

A token-shape scan over summary and detail: `path.rs::symbol`, `Type::method`,
snake_case with an underscore, CamelCase with two or more humps, `foo()`,
`macro!`, and file paths or source filenames.

That scan cannot tell an internal symbol from a product name, so three classes
were allowlisted and do **not** count as implementation detail:

1. **Product and proper nouns** — GitHub, BigQuery, LilyPond, SvelteKit,
   MusicXML, CodeRabbit, Docker, Renovate, and so on.
2. **Interfaces an agent is told to call** — dispatch's own MCP tool and
   parameter names, and Claude Code's tool and hook names (`ToolSearch`,
   `run_in_background`, `subagent_type`, `SessionStart`).
3. **Dispatch domain vocabulary** — every entity field name declared in
   `core.allium`, plus the status values. Naming `wrap_up_mode` or `base_branch`
   is domain knowledge; naming `handle_tick` is implementation detail. This
   distinction saved four entries, including the pool's second-highest-rated one.

External library and standard-library names (`spawn_blocking`, `PartialEq`,
`RefCell`) were **not** allowlisted. Naming them is still implementation detail
under the rule, even though they belong to somebody else's code.

### The other three filters

**Old and never rated** was applied as a one-off at 60 days rather than the
90-day `ArchiveStaleLearning` sweep's own gate. The sweep itself was left
unchanged — it keeps archiving at 90 days and score ≤ 0 for everything recorded
from now on.

**Transient** and **superseded** were hand-applied after reading every entry that
survived the two mechanical filters. Both lists are short and are marked in the
groups below.

## What survived

71 entries. They are the ones whose whole claim is prose — a convention, a trap,
a user preference, a domain fact — with no symbol in either field.

## Deleted entries

Grouped by the first filter that caught each one. Additional filters that also
matched are shown after the scope.

### names-impl-detail(summary) (159)

- **#32** ↑43 2026-05-10 `convention` repo:dispatch — Inline `#[cfg(test)] mod` blocks need `#[allow(clippy::unwrap_used, clippy::expect_used)]` on the module — the workspace-wide -D warnings policy makes test-only `expect("…")` fail clippy otherwise.
- **#281** ↑19 2026-07-09 `pitfall` repo:dispatch — check-doc-paths.sh only validates that referenced link paths exist — it does NOT check doc prose for staleness, so behaviour changes need a manual grep of docs/ for outdated content.
- **#327** ↑17 2026-07-29 `pitfall` repo:dispatch — MockProcessRunner tests assert argv, not tmux semantics — they can pin a broken command string and stay green. Bugs in tmux command *behaviour* (wrong pane targeted, wrong cwd resolved) need a real tmux server: private `-L` socket, drop-guard teardown, panes running `cat > file` to capture keystrokes. See tests/tmux_split_hook.rs.
- **#298** ↑15 2026-07-14 `landscape` repo:dispatch — ConfirmDone (moving a task Review→Done) only kills the tmux window (task.tmux_window = null) — it never removes the git worktree, unlike Archive/Delete which do full Cleanup.
- **#201** ↑9 2026-06-24 `pitfall` repo:dispatch — A cache over `board.tasks`/`board.epics` fields goes stale when tests mutate those fields directly (bypassing the message system and `invalidate_layout_cache`), causing widespread test failures.
- **#283** ↑9 2026-07-10 `landscape` repo:dispatch — Two independent skill-discovery paths exist: plugin/skills/* is embedded in the dispatch binary via include_dir! and only reaches ~/.claude/plugins/local/dispatch/ on `cargo run -- setup`; .claude/skills/* is a plain tracked directory Claude Code auto-discovers for any session cwd'd inside the repo, no build/install step.
- **#121** ↑8 2026-06-11 `pitfall` repo:dispatch — For group_by_repo feed epics, feed-as-source-of-truth must reconcile the WHOLE subtree: sync_grouped_feed must clear feed tasks from active sub-epics absent from the current emission, not just upsert the repos present in it.
- **#210** ↑8 2026-06-25 `pitfall` repo:dispatch — Calling runner.run() or tmux::* synchronously in an async fn blocks the tokio event loop — always wrap in spawn_blocking
- **#159** ↑7 2026-06-18 `landscape` repo:frontend-erp — frontend-erp has no mocking framework (no MSW/fixtures) — it hits real staging APIs; to mock a backend, add a mock client returning Promise<BaseResponse<T>> behind the api/<domain>/{client,models,hooks} convention.
- **#189** ↑6 2026-06-22 `pitfall` repo:dispatch — New ViewMode/InputMode variants that span multiple files (dispatcher, runtime, input) form a single compile unit — add Message and Command types together with all their exhaustive match arms in one commit, or the tree won't compile mid-task.
- **#384** ↑6 2026-08-12 `pitfall` repo:dispatch — Derive mock-call indices (script.index_of), never hardcode calls[N] or calls.last() — adding one tmux call breaks queues in unrelated-looking tests
- **#163** ↑5 2026-06-18 `pitfall` repo:frontend-erp — frontend-erp jest config lacks a setup file, so component/hook tests must polyfill TextEncoder/TextDecoder (react-router needs them) and load @testing-library/jest-dom — add jest/setup.ts wired via setupFilesAfterEnv.
- **#351** ↑5 2026-08-02 `pitfall` repo:dispatch — The fresh-worktree branch of dispatch_with_prompt is unreachable with MockProcessRunner — pre-creating the dir to make the prompt write succeed is exactly what flips reused_worktree to true.
- **#126** ↑4 2026-06-11 `convention` repo:dispatch — When a typed nullable field maps to two DB columns, carry ONE compound patch field (Option<Option<T>>) and split at the SQL bind site, not two sibling FieldUpdates.
- **#173** ↑4 2026-06-21 `pitfall` epic:166 — In the docs/specs/*.allium parse-fix step, the recurring errors are: prose `@invariant Name` blocks (only valid as a swallowed trailing annotation, so the 2nd+ errors), `@guidance` placed INSIDE an `invariant {}` body, and `default Entity {}` blocks — none are valid Allium 3.5.0.</summary>
<parameter name="detail">Fixes that work (verified against allium 3.5.0): (1) Convert prose `@invariant` to a real `invariant Name { for x in Entity where ...: <bool-expr> }` block (combat.allium #2617 did this; multiple `for` loops in one body are allowed). Put explanatory prose as `--` comments ABOVE the invariant, NOT as `@guidance` inside it (`@guidance` inside `invariant {}` is a parse error). (2) Replace `default Entity { field: val }` blocks with inline field defaults on the entity declaration: `field: Type = val` (e.g. `weakness: player/SpellElement = fire`, `max_health: Integer = config.default_max_health` — forward refs to config work). For Godot-init details that aren't checkable state predicates (e.g. group-based player targeting), keep the prose as a labelled `-- NOTE` comment rather than forcing a fake invariant. Note: `unreachableTrigger`/`field.unused`/`entity.unused`/single-file `use.unresolvedPath` are info/warning, NOT errors — "zero errors" is achievable while leaving them; the use-path warning disappears when checking files as a set. Also: `allium plan <spec>` gives the obligation list and `allium analyse <spec>` the findings for the propagate step.
- **#284** ↑4 2026-07-10 `convention` repo:dispatch — Any decision that must stay identical across two dispatch code paths (FeedRunner::tick auto-poll vs exec_trigger_epic_feed manual "r") must live in one shared function both call — never duplicate the branch/match, even with a comment asserting they're kept in sync.
- **#471** ↑4 2026-08-22 `landscape` epic:299 — The agent draft's staged plan snapshot has no slot for deliverable config fields (total_volume, desired_margin, dates overrides, article config, etc.) — only capacity entries, requests, project deliveries and planned uploads — so a from-scratch plan-creation tool can only stage the plan half of its planner rule's write; persisting the config half is a separate, later work package's job.
- **#110** ↑3 2026-06-10 `pitfall` repo:dispatch — Migration column_exists/table_exists guards are needed even for foundational columns because some migration tests build minimal schemas from a specific version point
- **#111** ↑3 2026-06-10 `pitfall` repo:dispatch — Adding a newtype wrapper to a domain model field has wide blast radius: needs FromSql, PartialEq<str/String>, and bulk updates to all struct literals and command types
- **#130** ↑3 2026-06-12 `pitfall` repo:dispatch — Splitting a src/foo.rs into a src/foo/ module breaks check-doc-paths.sh because CLAUDE.md and docs reference the old file path.
- **#140** ↑3 2026-06-15 `pitfall` repo:dispatch — In dispatch_with_prompt flow tests you can't assert both the git worktree start point and the .claude-prompt content in one call — the worktree-dir-exists check gates them oppositely.
- **#165** ↑3 2026-06-18 `pitfall` epic:70 — @kognic/ui-core component-test gotchas for production-plan surfaces (dialog portal, Button title, InputField labels, date inputs)
- **#192** ↑3 2026-06-22 `convention` repo:dispatch — New InputMode variants that trigger destructive confirms (e.g. ConfirmDeleteTodo) MUST render a visible [y/n] prompt in status_bar.rs — a blank arm leaves the user staring at an empty bar with no guidance during an irreversible action.
- **#208** ↑3 2026-06-24 `pitfall` repo:dispatch — Dirty-flag snapshot in handle_key must cover ALL rendering-relevant fields, including nested enum variant fields
- **#237** ↑3 2026-07-05 `pitfall` repo:oratorie — oratorie: `db` in src/lib/server/db/client.ts is constructed eagerly at module load via getEnv(), so importing it throws without DATABASE_URL — tests rely on src/test-setup.ts setting a dummy DATABASE_URL to stay green on a clean checkout/CI.
- **#253** ↑3 2026-07-06 `pitfall` epic:212 — OMR parity.test.ts is a deliberately-red north-star gate, so `npm run test` cannot fully pass until the whole exact-parity initiative (P1–P6) lands — use index.test.ts as the green safety net.
- **#333** ↑3 2026-07-29 `pitfall` repo:dispatch — When locking a specific instruction in plugin/skills/*/SKILL.md, scope the assertion to its heading section — sibling sections repeat phrases like "Do not retry", so a whole-document contains() passes even after the instruction is deleted.
- **#336** ↑3 2026-07-31 `pitfall` repo:dispatch — A MockProcessRunner panic on a detached spawn_blocking thread does not fail the test — under-scripted runners hide silently
- **#340** ↑3 2026-07-31 `pitfall` repo:dispatch — check-doc-symbols.sh builds its identifier index from code with comments STRIPPED — indexing raw file text makes every phantom symbol self-validate via its own doc comment.
- **#355** ↑3 2026-08-02 `pitfall` repo:dispatch — db_call opens no transaction, and the "single writer connection" only serialises within one process — every Claude Code hook is a separate dispatch process, so a multi-statement closure needs an explicit unchecked_transaction()
- **#385** ↑3 2026-08-12 `pitfall` repo:dispatch — Before deleting a keybinding's help/footer line because a sibling task "removed" the key, grep src/tui/input/normal.rs for the arm — epic sequencing drifts from the plan, and an archived task does not mean the code landed.
- **#114** ↑2 2026-06-10 `convention` repo:dispatch — A "key does nothing" test needs only make_app() — no column navigation, no elaborate task setup. Navigation is irrelevant when a key has no match arm at all.
- **#117** ↑2 2026-06-10 `pitfall` repo:dispatch — tui_tree_widget TreeState navigation methods (key_up, key_down, key_left, key_right) return bool, not () — discard with { } blocks to avoid unused-result warnings.
- **#144** ↑2 2026-06-15 `convention` epic:157 — Role-routed feed reconcile (run_role_routed_feed_sync) preserves in-flight task state by doing explicit moves (set_task_epic_id + patch_task) BEFORE per-role upsert, then deleting exactly once via delete_stale_subtree_feed_tasks(parent, union-of-emitted-ids).
- **#185** ↑2 2026-06-22 `convention` repo:frontend-erp — frontend-erp already has a d3 charting pattern (invoice-specification histogram) — reuse it for new charts, and jest needs a d3 alias + ResizeObserver stub to test them.
- **#206** ↑2 2026-06-24 `convention` repo:dispatch — delete_stale_subtree_feed_tasks is one level deep by design — for a 3-level tree (parent → role sub-epics → repo-group sub-epics), call it at each level that has feed children
- **#227** ↑2 2026-06-27 `pitfall` repo:dispatch — handle_load_filter_preset mutates filter state but does not set self.dirty = true — loading a preset may not refresh the popup
- **#251** ↑2 2026-07-06 `tool_recommendation` epic:212 — For OMR of LilyPond PDFs, identify glyphs by semantic Feta name (from the embedded font), not by pdf.js subset code — codes differ per PDF.
- **#279** ↑2 2026-07-09 `landscape` repo:dispatch — src/tui/ui/kanban is a directory module (mod.rs, popups/, cards.rs, columns.rs, status_bar.rs, tests.rs), not a flat kanban.rs — most render functions including all popup renderers live there, not in src/tui/ui/mod.rs which only re-exports.
- **#302** ↑2 2026-07-16 `pitfall` repo:frontend-erp — In ConfigSurface.test.tsx (and likely other RTL suites using @kognic/ui-core's BasicDialog/MultiSelect), leftover DOM from an earlier test's render is not fully removed even by an explicit testing-library cleanup() call — document.body.children count persists/grows across tests.
- **#343** ↑2 2026-07-31 `pitfall` repo:dispatch — check-doc-symbols.sh only accepts an allow-phantom-symbol marker on the offending line or the single line directly above it — inside a /// block that usually means rewording, not annotating
- **#346** ↑2 2026-07-31 `pitfall` repo:dispatch — A tui-tree-widget TreeState assertion can encode a key that matches no node and still pass — cover expansion by rendering and asserting the leaf is visible.
- **#377** ↑2 2026-08-11 `pitfall` repo:dispatch — After a split-pane swap, the renamed agent window's @dispatch_dir still names the *incoming* task's worktree — swap-pane moves panes, not window options.
- **#398** ↑2 2026-08-13 `convention` repo:dispatch — Prove a test discriminates by breaking the thing it claims to pin — a green suite routinely hides vacuous tests, and this repo's mock/spawn_blocking layering makes it easy
- **#433** ↑2 2026-08-16 `pitfall` repo:dispatch — Adding fields to the Task model can trip clippy::large_enum_variant on the TUI's outer Message/Command enums, which a plain cargo build never shows.
- **#453** ↑2 2026-08-21 `convention` repo:scala-common — Scala test files/classes in scala-common are suffixed `Test`, not `Spec` (e.g. `JsonRpcRequestTest`, not `JsonRpcRequestSpec`), matching the existing pattern (`PatchDtoJsonTest`, `CacheControlOpsTest`).
- **#461** ↑2 2026-08-21 `pitfall` repo:scala-common — RouteAuthDirectives.withUser silently enforces ScopeChecker's per-HTTP-method read/write scope check (GET/HEAD=read, else write); a route that multiplexes several logical operations behind one HTTP method (e.g. a JSON-RPC POST endpoint) gets every call misclassified as a write operation unless its path is added to baseapi.scope-check.bypassed-paths, so the route can enforce its own finer-grained scope check instead.
- **#465** ↑2 2026-08-21 `landscape` epic:299 — WP3's local plan store deliberately does NOT persist request overrides or request-inputs (two of the seven kinds localStore.ts persists) — only workforce plan, project deliveries, planned uploads, learning curves, and deliverable config/change-log moved. Those two stay on browser localStorage in every mode.
- **#83** ↑1 2026-05-31 `pitfall` repo:dispatch — query_learnings returns at most 50 entries — for full-corpus analysis of the knowledge base, query the SQLite DB directly at ~/.local/share/dispatch/tasks.db
- **#85** ↑1 2026-05-31 `convention` repo:dispatch — Shared test helper functions used across multiple tui test files belong in src/tui/tests/helpers.rs (pub(in crate::tui)), not duplicated in individual test modules.
- **#87** ↑1 2026-05-31 `pitfall` repo:dispatch — DB store delete methods that return Result<()> silently no-op when the row doesn't exist — use Result<bool> (rows_affected > 0) to atomically distinguish "deleted" from "not found" without a separate get call.
- **#91** ↑1 2026-06-03 `landscape` repo:frontend — The weekly-turnaround API endpoint only exposes p25/p50/p75 — true p99 percentiles require querying BigQuery directly via order_execution_api_data.request_input_progress (57M rows, partitioned by updated_at)
- **#104** ↑1 2026-06-08 `convention` repo:dispatch — Derive PartialEq on model structs (Task, Epic) to avoid manual field-by-field comparison functions
- **#109** ↑1 2026-06-10 `pitfall` repo:dispatch — macro_rules! defined in a module is not auto-visible in child modules — use #[macro_export] plus `use crate::macro_name;` at each call site
- **#112** ↑1 2026-06-10 `pitfall` repo:dispatch — When changing previously-silent behavior (like PrState::Closed), search for existing tests that assert the old no-op — they must be replaced, not just complemented by new tests.
- **#135** ↑1 2026-06-12 `procedural` repo:dispatch — Adding a serde-defaulted field to FeedItem requires updating ~20 struct literals across src/feed/ingest.rs and src/db/tests/tasks.rs; run `cargo test --no-run` first to get the full E0063 site list, then fix mechanically.
- **#137** ↑1 2026-06-14 `tool_recommendation` repo:wizard_game — GDScript editor-plugin tools (@tool EditorScript suites) can't be run headlessly from CLI; verify .gd via intelligence_script_analyze (LSP parse), and a Claude Code RESTART (not just MCP reconnect) is needed before newly-added MCP tools become callable.
- **#142** ↑1 2026-06-15 `convention` repo:dispatch — Epic/Task SELECT column lists are centralized in TASK_COLUMNS and EPIC_COLUMNS consts in src/db/queries/mod.rs — when adding a column, update the const, not individual queries.
- **#147** ↑1 2026-06-16 `pitfall` repo:dispatch — The partial unique index on epics(parent_epic_id, feed_role) does NOT dedupe root managed epics (NULL parent), since SQLite treats NULLs as distinct in unique indexes — root-epic idempotency relies on select-then-create, not the index.
- **#152** ↑1 2026-06-17 `pitfall` repo:dispatch — To call a service fn from the runtime with its Arc<dyn TaskStore> handle, type the fn param as &dyn TaskStore (the umbrella supertrait), not a generic <D: SettingsStore + EpicCrud> — generics impose Sized and reject the trait object.
- **#158** ↑1 2026-06-18 `convention` repo:dispatch — A write-only per-task flag can skip the Task/TaskPatch/TASK_COLUMNS ripple entirely: add a narrow TaskCrud method doing one atomic UPDATE ... WHERE col IS NULL returning Result<bool>.
- **#167** ↑1 2026-06-20 `convention` repo:wizard_game — Enemies flip via child-node Scale (BlackKnightVisual) or sprite FlipH (EmberShade), never by scaling the CharacterBody2D itself.
- **#169** ↑1 2026-06-21 `convention` repo:wizard_game — EnemyBase water/impact mechanics use short lease timers refreshed each physics tick by the emitter, with effective values exposed as private computed properties.
- **#190** ↑1 2026-06-22 `convention` repo:dispatch — Runtime exec functions that load async data take `&mut App` and call `app.update(Message::...)` directly — never use `msg_tx.send`. See exec_load_learnings and exec_load_todos for the canonical pattern.
- **#191** ↑1 2026-06-22 `pitfall` repo:dispatch — Startup count/state loads must call the already-constructed runtime method (e.g. runtime.exec_load_todo_count(&mut app)) AFTER TuiRuntime is built — constructing a fresh inline service before the struct exists duplicates logic and diverges from future changes.
- **#209** ↑1 2026-06-25 `pitfall` repo:dispatch — The dirty-flag check in handle_key only snapshots board-level cursor state — overlay views (Todos, Learnings) carry their own `selected` cursor that must be tracked via ViewMode::view_selected() or navigation keypresses silently drop frames.
- **#219** ↑1 2026-06-25 `convention` repo:dispatch — When tracking "ticks since last X", store the elapsed counter directly (ticks_since_last_refresh: u64, reset to 0 on each refresh) rather than a monotonic tick_count + last_refresh_tick pair — one field instead of two, no subtraction needed, and the field name matches the semantics.
- **#220** ↑1 2026-06-25 `convention` repo:dispatch — Use a panicking MockXxxService (guard mock) for test helpers that should never call the service; use not_mocked()-style Err for partial mocks where some methods are exercised.
- **#228** ↑1 2026-06-28 `pitfall` repo:dispatch — A bare Option<T> field in UpdateTaskParams cannot express "clear to NULL" — build_task_patch reads None as "don't touch", so an editor/MCP clear silently no-ops in the DB. Use FieldUpdate (strings) or Option<Option<T>> (enums) for clearable fields.
- **#238** ↑1 2026-07-05 `tool_recommendation` repo:oratorie — oratorie: Svelte 5 component tests under Vitest+jsdom need `resolve.conditions:['browser']` (gated by process.env.VITEST) in vite.config.ts, else the SSR build resolves and `mount` is missing.
- **#243** ↑1 2026-07-05 `pitfall` repo:oratorie — oratorie: `npm run check` (svelte-check) does NOT type-check `scripts/` — the generated tsconfig `include` only covers src/test/tests, so script files (seed.ts, pipeline-worker.ts) are validated only by running them.
- **#244** ↑1 2026-07-05 `convention` repo:oratorie — oratorie: import the app singletons `db` (src/lib/server/db/client.ts) and `storage` (src/lib/server/storage/index.ts) — both built from getEnv() — instead of reconstructing makeClient()/new LocalDiskStorage() in server code or scripts.
- **#252** ↑1 2026-07-06 `tool_recommendation` repo:oratorie — In OMR, get semantic Feta/Emmentaler glyph names from pdf.js `font.differences` (parsed PDF Encoding), not the rebuilt OTF `post` table.
- **#286** ↑1 2026-07-10 `pitfall` repo:dispatch — A length-only staleness check for a Vec-backed id→index cache (e.g. App.task_index) misses a same-length wholesale replacement of the Vec with a different id set.
- **#291** ↑1 2026-07-13 `pitfall` repo:dispatch — Rebasing hook-event classification changes onto main can conflict with concurrent hook-event additions in HookEventKind/record_hook_event/task-status-hook
- **#303** ↑1 2026-07-16 `pitfall` repo:dispatch — Adding a DB migration requires bumping schema-version assertions at MANY call sites, not just the one `fresh_db_has_latest_schema_version` test docs/how-to.md mentions.
- **#308** ↑1 2026-07-20 `pitfall` repo:oratorie — oratorie: importing from src/lib/server/pipeline/omr/index.ts (even a constant like NOTE_MS) drags the whole OMR/pdfjs-dist module graph into the importer's bundle — keep request-path modules out of that import chain.
- **#313** ↑1 2026-07-25 `convention` repo:dispatch — Service seams (TaskServiceApi etc.) are macro-generated: edit the spec macro in src/service/api.rs, never the trait or impl directly; test mocks implement the *ServiceApiStub trait and override only what they exercise.
- **#347** ↑1 2026-07-31 `pitfall` repo:dispatch — Testing tui_tree_widget cursor movement requires rendering first — key_up/key_down resolve against the identifiers captured by the last render, so a key test with no draw silently leaves the selection empty.
- **#353** ↑1 2026-08-02 `pitfall` repo:dispatch — An under-scripted MockProcessRunner can make a spawn_blocking error-path test pass on the mock's own panic — sharper than #336, because the JoinError IS the asserted error
- **#396** ↑1 2026-08-13 `landscape` repo:dispatch — Don't rebuild send_message on Claude Code's native cross-session messaging — protocol is undocumented, and it's already live anyway
- **#403** ↑1 2026-08-13 `convention` repo:dispatch — Task teardown is one primitive, dispatch::teardown_task, whose two resources are independent optionals; exec_cleanup has exactly two exits now (the shared-worktree/detach_only branch is gone) and gates on TeardownFailure.worktree_left, never on its own arguments.
- **#410** ↑1 2026-08-14 `pitfall` repo:dispatch — A helper that maps a failed query to an empty result (e.g. list_all_window_names's Err treated as "no windows") silently disables every fail-safe written against its error arm.
- **#455** ↑1 2026-08-21 `preference` repo:scala-common — Put spray-json `RootJsonFormat`/`JsonFormat` implicits in the companion object of the case class/trait they serialize, not in a separate shared `*JsonProtocol` object — consumers then get the implicit for free via Scala's implicit scope, with no extra import needed.
- **#484** ↑1 2026-08-25 `convention` repo:dispatch — When a task retires a config mechanism (a setting, a generated file's key, a subsystem), also sweep the knowledge base with query_learnings for entries describing it — not just docs/CLAUDE.md/the Allium spec.
- **#84** ↑0 2026-05-31 `convention` repo:dispatch +old-and-never-rated — key_event() for RecordUsageEvent is defined once in src/tui/input.rs; child modules (input/normal.rs, etc.) must call super::key_event() rather than defining their own copy.
- **#97** ↑0 2026-06-03 `convention` repo:frontend +old-and-never-rated — @kognic/ui-core component prop shapes: SingleSelect uses data/getOptionValue/getOptionLabel (NOT options/getOptionKey); Button takes a `title` prop for its text (not children); InputField needs an explicit `id` for label association.
- **#99** ↑0 2026-06-04 `pitfall` repo:annotell-data-warehouse +old-and-never-rated — validate_metadata "columns missing from the model yml" failures mean an upstream source table gained a column that flows through select * models — document it in the .yml.
- **#105** ↑0 2026-06-09 `convention` repo:dispatch +old-and-never-rated — Changing ServiceError::Internal(String) to Internal(anyhow::Error) requires updating test mock stubs from "msg".into() to anyhow::anyhow!("msg") since anyhow::Error does not implement From<&str> or From<String>
- **#106** ↑0 2026-06-09 `pitfall` repo:dispatch +old-and-never-rated — Tests asserting on error message strings like "Database error" become brittle after removing format!() prefixes — update them to assert the actual underlying DB error message instead
- **#107** ↑0 2026-06-09 `landscape` epic:129 +old-and-never-rated — Airflow DAGs at Kognic live in the airflow-dags repo (/home/ragge/Code/work/airflow/airflow-dags, github.com/annotell/airflow-dags) — uv-based, with kognic_dags/, schemas/, tests/, and CI deploy in .github/workflows/deploy.yaml.
- **#113** ↑0 2026-06-10 `convention` repo:dispatch +old-and-never-rated — When removing a TUI feature, check src/dispatch/mod.rs for dead lower-level helpers — pub functions won't be flagged by the dead-code lint.
- **#116** ↑0 2026-06-10 `pitfall` repo:dispatch +old-and-never-rated — RefCell&lt;TreeState&lt;String&gt;&gt; cannot live in InputState because InputState derives Clone — store it on App directly instead.
- **#139** ↑0 2026-06-14 `convention` repo:wizard_game +old-and-never-rated — In godot-dotnet-mcp, an MCP tool's surfaced name is composed as <category>_<basename> via PluginRuntimeState.compose_tool_name(); the `intelligence` category is prefix-less (surfaces bare names like project_state). Route every name composition through compose_tool_name and category membership through ToolCatalogService.tool_belongs_to_category — never hand-roll "%s_%s" or begins_with(category+"_").
- **#145** ↑0 2026-06-16 `pitfall` repo:dispatch +old-and-never-rated — scripts/fetch-dependabot.sh is include_str!'d into src/setup/plugins.rs as the seeded example feed script — deleting it breaks compilation and ~10 tests.
- **#166** ↑0 2026-06-20 `pitfall` repo:dispatch +old-and-never-rated — Enabling managed-feed config (set_managed_feed_config) on an instance whose PR/CVE epics were hand-wired creates a duplicate parallel tree, because ProvisionManagedEpics matches epics by feed_role (never title) and pre-existing epics have feed_role=none.
- **#168** ↑0 2026-06-21 `pitfall` repo:wizard_game +old-and-never-rated — Water drive flow must use AverageForwardVelocityInAabb, not AverageVelocityInAabb, near walls — the plain average collapses because wall-reflected particles cancel the incoming stream.
- **#171** ↑0 2026-06-21 `pitfall` repo:wizard_game +old-and-never-rated — A spell Cast() that spawns a controller node must pass data via an explicit Begin()/Init() called AFTER AddChild, not in _Ready (Godot 4 runs _Ready during AddChild, before data is passed).
- **#177** ↑0 2026-06-21 `procedural` epic:166 +old-and-never-rated — Allium 3.5.0 rejects the `when: x: Entity where <cond> and <timer> <= now` binding-with-where trigger form; convert timer/multi-condition rules to `when: PhysicsTick(x, delta)` + `requires:` guards + `ensures: if <timer> <= now:`.
- **#193** ↑0 2026-06-23 `convention` repo:dispatch +old-and-never-rated — Overlay renderers (like render_todos) must check app.input.mode and render their own input row when an input mode is active — render_input_form only handles non-overlay modes and returns false for overlay input modes.
- **#194** ↑0 2026-06-23 `pitfall` repo:dispatch +old-and-never-rated — handle_show_X overlay functions must preserve the real pre-overlay previous using std::mem::take + match, not mem::replace — otherwise a second Show call nests the overlay inside itself, causing effective_view_mode() to return Todos/Learnings and crash via unreachable!()
- **#195** ↑0 2026-06-23 `convention` repo:dispatch +old-and-never-rated — In provision_worktree, use validate_repo_path() instead of expand_tilde() + inline existence check — it covers tilde expansion, exists(), and is_dir() in one call and keeps error messages consistent.
- **#198** ↑0 2026-06-23 `pitfall` repo:dispatch +old-and-never-rated — When extracting TuiRuntime construction into bootstrap(), sibling tasks that add new fields to TuiRuntime will cause rebase conflicts — resolve by adding the new field to bootstrap's TuiRuntime literal, not back into run_tui.
- **#200** ↑0 2026-06-23 `landscape` repo:dispatch +old-and-never-rated — Adding an enum column to the Epic model (mirroring feed_role) touches a fixed set of sites: the EpicOrigin-style enum in src/models/epics.rs (derive + as_str/parse/Display/FromStr), a guarded migration + optional partial index in src/db/migrations.rs, EPIC_COLUMNS in src/db/queries/mod.rs, a soft-fail parse_* helper + the field in row_to_epic, and the EpicPatch patch_struct! field + its set_field! bind in patch_epic.
- **#212** ↑0 2026-06-25 `pitfall` repo:dispatch +old-and-never-rated — in_memory_db() runs ALL migrations, so migration tests can't INSERT rows that would violate later triggers — use NULL-then-UPDATE pattern instead
- **#213** ↑0 2026-06-25 `convention` repo:dispatch +old-and-never-rated — Use raw db_call SQL for test setup when API helpers set wrong defaults (create_epic sets origin='manual'; CreateTaskRequest has no external_id field)
- **#214** ↑0 2026-06-25 `convention` repo:dispatch +old-and-never-rated — Make VACUUM INTO migrations idempotent by checking Path::exists() first — VACUUM INTO aborts if target already exists, bricking re-runs after partial failures
- **#218** ↑0 2026-06-25 `convention` repo:dispatch +old-and-never-rated — Both unwrap_used and expect_used are linted as errors (-D warnings); avoid both in production code by using `if let Some(ref x) = opt { ... } else { unreachable!(...) }` or an early return pattern when an Option is logically guaranteed Some.
- **#223** ↑0 2026-06-26 `pitfall` repo:dispatch +old-and-never-rated — Blocking syscalls in the ratatui render closure (e.g. is_dir(), file I/O) stall the entire tokio async executor, freezing key handling.
- **#229** ↑0 2026-06-28 `pitfall` repo:dispatch — Adding a new InputMode variant causes rebase conflicts when main has also added InputMode arms since the branch was cut — the exhaustive match in src/tui/input.rs is the hot spot.
- **#234** ↑0 2026-06-30 `convention` repo:wizard_game — Godot _Draw/visual selection logic IS unit-testable here — extract the state→geometry/colour mapping into a pure core/ class (SpellOrbColors pattern) rather than treating it as an untestable Godot no-op.
- **#236** ↑0 2026-07-05 `convention` epic:166 — To fix a spec-weed bug where a Godot node mutation (e.g. disabling a CollisionShape2D) is done inconsistently across code paths, extract the decision as a pure predicate in core/ (TDD-testable), then drive the Godot mutation through one private helper that reads node state and is called from every path.
- **#242** ↑0 2026-07-05 `convention` repo:oratorie — oratorie: the root +layout.server.ts returns `signedIn`, which merges into every route's PageData — component tests that render a +page.svelte with a `data` prop must include `signedIn` or svelte-check fails.
- **#247** ↑0 2026-07-05 `pitfall` repo:oratorie — SvelteKit forbids a `default` form action alongside any named action in the same +page.server.ts — mixing them makes every submit to the default action 500 with "When using named actions, the default action cannot be used". Name all actions (e.g. signin/signout) and point each form at ?/name.
- **#256** ↑0 2026-07-06 `pitfall` repo:oratorie — pdfjs getDocument transfers (detaches) the input Uint8Array to its worker; calling getDocument a second time on the same buffer throws DataCloneError — read a fresh copy of the bytes per document.
- **#258** ↑0 2026-07-07 `pitfall` epic:212 — OMR recognise.ts band thresholds should land in the SPACE between ledger lines, not on a ledger line, or boundary-exact notes are dropped by sub-pixel noise.
- **#264** ↑0 2026-07-07 `pitfall` repo:oratorie — pdf.js getTextContent silently drops glyphs whose Unicode maps to a control code (e.g. a LilyPond breve notehead maps to "\n"), so OMR must recover notehead glyphs from the operator list
- **#285** ↑0 2026-07-10 `tool_recommendation` repo:dispatch — To diagnose a live dispatch feed-routing bug, read the sqlite DB directly (~/.local/share/dispatch/tasks.db) plus app.log, rather than only reading code — the mismatch between spec/code intent and actual stored state (duplicate external_ids, tasks stuck in the wrong epic) and the recurring trigger-abort warnings in app.log were what revealed the real bug, not static reading.
- **#289** ↑0 2026-07-10 `pitfall` repo:staff_engineer — Low-link vault notes are usually caused by stale hub files, not individual missing links — check moc/staff-engineer.md, interviews/_index.md, and people/_index.md first before editing each orphan note individually.
- **#293** ↑0 2026-07-14 `landscape` repo:dispatch — BoardSelection.selected_row is a sticky per-column array (one row index per nav column, not per-task), and on_select_all is a single board-wide flag, not per-column — column switches (handle_navigate_column in src/tui/update/navigation.rs) previously reused whatever row/toggle state was last left in a column.
- **#299** ↑0 2026-07-15 `pitfall` repo:dispatch — PollPrStatus's first PR-status check fires on the very next tick (2s), not after the full 30s pr_poll_interval, because last_pr_poll.get(&id).is_none_or(...) treats a missing entry as "poll now".
- **#300** ↑0 2026-07-15 `pitfall` repo:dispatch — A `pub(super)` enum declared in a handler submodule cannot be used as a field type in a struct defined two levels up (e.g. mcp::mod.rs) — Rust visibility scoping requires widening to `pub(crate)` or moving the type to the shared module.
- **#307** ↑0 2026-07-17 `convention` repo:oratorie — oratorie: src/lib/config.ts mirrors the Allium `config {}` block and config.test.ts asserts they match — add any new config value to both, plus the spec.
- **#329** ↑0 2026-07-29 `pitfall` repo:dispatch — TaskStatus::next()/prev() saturate — Done.next() == Done, so `status.next() == Done` is NOT equivalent to `status == Review`
- **#331** ↑0 2026-07-29 `pitfall` repo:dispatch — In runtime tests, exec_persist_task fails silently into an error popup if the task snapshot's sub_status isn't valid for its new status — always set sub_status = SubStatus::default_for(new_status) alongside status.
- **#338** ↑0 2026-07-31 `convention` repo:dispatch — Keep every agent-launch command string at ONE shell quoting layer — dispatch_with_prompt passes the claude binary as bash's `$0` after the script body, not inside the single-quoted body
- **#342** ↑0 2026-07-31 `pitfall` repo:dispatch — TUI make_task fixture now provisions Running/Review tasks — it used to be unprovisioned, silently matching new "no worktree" branches
- **#345** ↑0 2026-07-31 `procedural` repo:dispatch — When wrap_up(rebase) fails with a rebase conflict, finish_task has already aborted cleanly — resolve by rebasing onto main yourself, re-verify, then call wrap_up again; do NOT call exit_session, you have no token.
- **#352** ↑0 2026-08-02 `pitfall` repo:dispatch — MockProcessRunner CAN inject a spawn failure: its response queue takes Result<Output>, so push Err(anyhow!("git not on PATH")) — only the `fail` helper is limited to Ok(non-zero).
- **#357** ↑0 2026-08-02 `pitfall` repo:dispatch — The order of the tick_* sub-steps in handle_tick is NOT load-bearing — a sub-step only emits Commands, and a Command's effect can never be observed within the same tick, so no two sub-steps can interact through it.
- **#361** ↑0 2026-08-03 `convention` repo:dispatch — Allium has no `transitions_from`: express "on leaving state X" with a let-bound pre-state guard per rule, tied together by an entity-level invariant using `implies`.
- **#363** ↑0 2026-08-03 `convention` repo:dispatch — `main` routes fully synchronous subcommands (statusline, caller-headers, verify-feed, uninstall, toggle-agent-tree-pane) before any tokio runtime is built — a new sync, hot-path subcommand belongs in that match, not only in `run_async`.
- **#365** ↑0 2026-08-03 `pitfall` repo:dispatch — To inspect a repo's RAG index by hand, open it read-only as 'file:<path>/.dispatch/rag.db?immutable=1' — a plain sqlite3 open fails, and the index lives in the parent repo, not your worktree.
- **#367** ↑0 2026-08-03 `pitfall` repo:dispatch — Migrations in this repo are NOT replay-safe, so a "does an old DB migrate forward?" test cannot be written by winding user_version back on a migrated DB
- **#369** ↑0 2026-08-11 `pitfall` repo:dispatch — Never backtick an Allium spec block name (snake_case, e.g. board_search_filter) in a Rust doc comment — check-doc-symbols.sh rejects it, and the repo verify command does not run that checker, so it only surfaces at push.
- **#371** ↑0 2026-08-11 `pitfall` repo:dispatch — Sweeping a removed TUI keybinding: docs/mcp.md is a 7th surface, and neither doc checker will catch it
- **#372** ↑0 2026-08-11 `convention` repo:dispatch — When a refactor removes the last production caller of a `pub(in crate::tui)` helper that inline tests still use, clippy's dead_code fails the -D warnings gate — mark it `#[cfg(test)]` (as `column_items_for_status` in src/tui/mod.rs already is), don't delete it or the tests.
- **#375** ↑0 2026-08-11 `convention` repo:dispatch — To pin a "computed once per frame/pass" perf property in the TUI, use a #[cfg(test)] thread_local Cell counter bumped inside the builder, and prefer OnceCell over eager construction so the property is structural.
- **#376** ↑0 2026-08-11 `pitfall` repo:dispatch — Instrumenting a TUI key arm breaks every test asserting an exact command list — wrap them in without_usage
- **#379** ↑0 2026-08-12 `pitfall` repo:dispatch — list-panes rows lose their trailing separator when the last field is unset, because run_checked_stdout trims
- **#381** ↑0 2026-08-12 `convention` repo:dispatch — To test a TuiRuntime exec_* that spawns a watcher thread (e.g. exec_pop_out_editor), assert only on recorded_calls()[0] — issued synchronously — and build the mock with .with_windows(&[]) so the watcher's cleanup can't exhaust the queued responses.
- **#388** ↑0 2026-08-12 `pitfall` repo:dispatch — `gh pr list` and `gh search prs` report bot authors differently: `.author.login` is `app/kognic-renovate` from `pr list` but `kognic-renovate[bot]` from `search prs` — the `[bot]$` test in fetch-reviews.sh only works on the latter.
- **#394** ↑0 2026-08-12 `pitfall` repo:dispatch — src/main.rs is a separate [[bin]] crate from the dispatch_tui lib — a lib item must be fully `pub` to be callable from it; `pub(crate)` compiles for in-crate callers and fails only at the CLI call site.
- **#397** ↑0 2026-08-13 `pitfall` repo:dispatch — A rusqlite DELETE ... RETURNING only executes as its rows are stepped, so the query_map iterator must be fully drained or rows are silently skipped.
- **#404** ↑0 2026-08-13 `procedural` repo:dispatch — To cover a DB-error arm in tests, fault-inject by renaming a table through a second rusqlite connection to a file-backed Database — do NOT try to write a fake TaskStore
- **#407** ↑0 2026-08-14 `pitfall` user: — Grafana MCP rule-listing filters are ignored: alerting_manage_rules' search_rule_name returns every rule, and /api/prometheus/<ds>/api/v1/rules ignores rule_name[] and exclude_alerts — always narrow with a jq expression that selects by .name, or the response blows the token limit.
- **#408** ↑0 2026-08-14 `landscape` repo:kognic-cd — Kognic KubePod*/KubeContainerWaiting/KubeDeploymentReplicasMismatch alerts are per-app Mimir rules generated by the kognic-deployment Helm chart, tunable per environment via monitoring.kubeAlerts.<alert>.{enabled,forDuration,severity} in the app's kognic-cd values.yaml — they are not Grafana-managed rules.
- **#409** ↑0 2026-08-14 `pitfall` repo:airflow-dags — Narrowing a custom Airflow operator's __init__ signature must keep **kwargs: BaseOperatorMeta.apply_defaults injects dag/task_group/params/default_args into kwargs, and DAG default_args only reach params named in the signature.
- **#415** ↑0 2026-08-15 `pitfall` repo:dispatch — A full-table-rebuild migration (e.g. for a CHECK-constraint change) must discover columns/indexes/triggers at runtime via pragma_table_info/sqlite_master, not hardcode today's schema — several existing migration tests replay every later migration against a deliberately partial synthetic seed, so a hardcoded rebuild breaks unrelated tests far from your feature.
- **#416** ↑0 2026-08-15 `pitfall` repo:dispatch — Removing an inherent method used by a `service_api_delegate!`-generated impl (src/service/api.rs) without also removing it from the spec macro's signature list causes silent self-recursion, not a compile error.
- **#419** ↑0 2026-08-16 `pitfall` repo:dispatch — Adding a method to a *ServiceApi seam silently breaks any test mock that intercepted the method the new one now subsumes — the stub default panics at runtime, not compile time.
- **#420** ↑0 2026-08-16 `pitfall` repo:dispatch — In a macro_rules field list, capture a marker keyword that follows an attribute repetition as `ident`, not `tt` — a `tt` matches `#` and makes the rule locally ambiguous.
- **#436** ↑0 2026-08-16 `pitfall` repo:dispatch — A single oversized payload inflates every enum that transitively wraps it, so one boxing fix can retire large_enum_variant allows in unrelated-looking modules.
- **#451** ↑0 2026-08-21 `pitfall` repo:dispatch — section_after (src/setup/plugins.rs test helper) truncates a section early when it contains a fenced code example with literal Markdown headings (e.g. a `## Summary` example inside a skill's prose) — it splits on the first "\n#" it finds, fenced or not
- **#456** ↑0 2026-08-21 `preference` repo:scala-common — When a domain model has a "these fields are mutually exclusive" invariant (e.g. exactly one of result/error), prefer making it a type-level ADT (a sealed trait with one subtype per case) over a runtime `require()` check in the case class — `require()` only guards the primary constructor and is silently bypassed by `.copy()`, while an ADT field makes the invalid state unconstructable.
- **#464** ↑0 2026-08-21 `pitfall` repo:frontend-erp — The env/.env.default parser (util/getEnvValues.js) splits every line containing "=" with no comment-line exclusion, so a "#"-prefixed comment that itself contains an "=" (e.g. an example env-var assignment) gets parsed as a bogus key/value pair and corrupts generated env-config.js/env-config.types.ts.
- **#483** ↑0 2026-08-25 `pitfall` repo:dispatch — An edit to ~/.claude/dispatch-statusline.json doesn't take effect in the session that made the edit — a fresh session (or dispatch setup rerun) is needed to pick it up.
- **#490** ↑0 2026-08-27 `pitfall` user: — The dependabot-review checklist's dep-only file whitelist has no Gradle entries, so a pure Gradle version bump (e.g. gradle/libs.versions.toml, settings.gradle) will always look like it "fails" the file check even though it's dependency-only.
- **#215** ↑-1 2026-06-25 `pitfall` repo:dispatch +old-and-never-rated — Tasks with epic_id are invisible to sync_board_selection in non-flattened Board mode — call handle_enter_epic() before setting a Task anchor pointing to an epic-owned task

### names-impl-detail(detail) (130)

- **#12** ↑101 2026-05-07 `pitfall` repo:dispatch — Verify a code-review work-package's premise against current code before implementing — earlier commits or repo-wide lints may have already addressed it.
- **#15** ↑44 2026-05-07 `landscape` repo:dispatch — Codebase map: layered architecture and concern→file index for the dispatch repo (router into CLAUDE.md / docs/specs).
- **#314** ↑33 2026-07-25 `pitfall` repo:dispatch — wrap_up(action="rebase") rebases your branch onto the current main, so sibling epic work can break your build after the tool returns — always re-run cargo test between wrap_up and exit_session, and don't chain it with `;` after another command or the failure hides behind exit code 0.
- **#88** ↑20 2026-05-31 `landscape` repo:dispatch — Remapping/removing a TUI keybinding touches ~6 surfaces — input handler, spec, two footer hint bars, help popup, reference doc, plus rendering-assertion tests and many footer-bar snapshots.
- **#392** ↑16 2026-08-12 `pitfall` repo:frontend-erp — In a frontend-erp dispatch worktree with no node_modules, `npm run tsc` does not fail — it silently resolves up to the PARENT checkout's node_modules and reports plausible-but-bogus type errors in files you never touched. Run `npm ci` in the worktree before trusting tsc/eslint/test output.
- **#233** ↑11 2026-06-30 `pitfall` repo:dispatch — wrap_up(rebase) fast-forwards LOCAL main, which can be far ahead of origin; a clean git rebase can still leave a compile-broken tree, so always run the verify command before exit_session.
- **#170** ↑8 2026-06-21 `pitfall` repo:wizard_game — The docs/specs/*.allium specs were authored as prose-Allium and mostly do NOT compile (92 errors across 9/11 files); also require allium CLI ≥ 3.5.0.
- **#225** ↑8 2026-06-26 `convention` repo:dispatch — Popup navigation handlers that mutate state invisible to the central dirty detector must call self.dirty = true directly
- **#175** ↑7 2026-06-21 `procedural` epic:166 — In the spec-completeness epic, "cover gaps with TDD tests" is often a no-op: the pure logic is already extracted to core/ and tested, so the gap is spec-only.
- **#301** ↑7 2026-07-16 `pitfall` repo:frontend-erp — Real production-plan deliverables (deliverableAdapter.toProductionDeliverable) always start with bpoAssignments: [] until a planner configures a BPO — any calc code that does deliverable.bpoAssignments[0] must handle undefined, not assume a BPO exists.
- **#324** ↑7 2026-07-27 `pitfall` repo:dispatch — Never target a tmux pane by hardcoded index (e.g. `<window>.1`) to identify "the other pane" — tmux's `pane-base-index` option shifts which index a window's first pane gets, so a fixed-index target can hit the wrong pane.
- **#127** ↑6 2026-06-12 `convention` repo:dispatch — Don't persist a reference whose value is a fixed constant — derive its existence from a live check instead.
- **#131** ↑6 2026-06-12 `pitfall` repo:dispatch — Running `cargo fmt` in a worktree reformats pre-existing formatting drift across many unrelated files, not just your changes — stage only your intended files before committing.
- **#332** ↑6 2026-07-29 `pitfall` repo:dispatch — A follow-up task's description may describe code from its parent task's unmerged branch — re-verify the premise against origin/main before implementing, and re-check after any mid-task rebase.
- **#174** ↑5 2026-06-21 `procedural` epic:166 — During the per-spec weed, watch for a spec carrying a STALE duplicate model of a subsystem that another spec already owns — the fix is to delete the stale copy, not update it (one source of truth), and notify the owning subtask.
- **#241** ↑5 2026-07-05 `pitfall` repo:oratorie — oratorie: a fresh worktree may have missing node_modules (e.g. `tone`) — `npm run test` fails with a missing-package error until you run `npm install` first.
- **#320** ↑5 2026-07-26 `pitfall` repo:dispatch — When docs describe a feature, check it still exists in src/ before trusting them — the Projects feature and the `project` learning scope were both dropped from the code but left documented as live.
- **#119** ↑4 2026-06-11 `landscape` repo:dispatch — Restructuring PR/feed epics is usually a scripts-and-data change, not a Rust change: feed epics support arbitrary nesting, each carries its own feed_command run via `sh -c` (so commands take args), and no production code hardcodes epic names.
- **#123** ↑4 2026-06-11 `tool_recommendation` repo:dispatch — Feed scripts run from the data dir (~/.local/share/dispatch/scripts/), not the repo's tracked scripts/; verify wiring with `cargo run -- verify-feed '<feed_command>'`.
- **#156** ↑4 2026-06-17 `procedural` repo:bigquery-export — When reviewing a Dependabot bump PR, first check whether master already moved past the PR's base version — stale Dependabot PRs (auto-rebase disabled after 30 days) often conflict and propose a redundant jump.
- **#261** ↑4 2026-07-07 `pitfall` repo:frontend-erp — frontend-erp has no root jest config and no `lint` script — run tests via `npm test -- <path>` and lint via `npm run eslint`.
- **#305** ↑4 2026-07-17 `procedural` user: — The allium-loop convergence gate blocks on ANY non-empty `open questions` in the target spec — including pre-existing ones unrelated to the current change.
- **#309** ↑4 2026-07-20 `pitfall` repo:dispatch — Run the start-of-task `git rebase main` before any codebase review or analysis — a branch behind main yields findings already fixed upstream.
- **#387** ↑4 2026-08-12 `landscape` repo:dispatch — A feed command is executed from three places, not two — the auto-poll path, the manual "r" path, and the verify-feed CLI.
- **#89** ↑3 2026-06-01 `pitfall` repo:staff_engineer — wrap_up rebase fails if the parent checkout has unstaged changes or the branch conflicts with advanced main; resolve by rebasing in the worktree, then retry.
- **#93** ↑3 2026-06-03 `pitfall` repo:scala-common — release-please's "commit could not be parsed" log lines are non-fatal — the workflow still succeeds and opens the release PR
- **#176** ↑3 2026-06-21 `procedural` epic:166 — Allium 3.5.0 parse-fix recipes for the per-spec specs: postfix list indexing only parses inside lambdas, undeclared cross-file types must be qualified, and rule bindings come from triggers/lets.
- **#224** ↑3 2026-06-26 `convention` repo:dispatch — Snapshot tests that directly construct picker state structs should use the opener handler instead, so cached fields are properly initialized.
- **#260** ↑3 2026-07-07 `landscape` epic:70 — Real data sources for the production-plan mock: every surface has a read-only backend except the forward-looking workforce/forecast (session-mock).
- **#266** ↑3 2026-07-07 `tool_recommendation` repo:oratorie — For a real-Postgres integration test in oratorie, use PGlite (@electric-sql/pglite + drizzle-orm/pglite) with migrate({migrationsFolder:'./drizzle'}) — the repo otherwise has no DB test harness.
- **#267** ↑3 2026-07-07 `convention` repo:oratorie — In oratorie, the uploading→processing/parsing transition is defined once in lifecycle.startProcessing; callers build the patch via startProcessing({status:'uploading', stage:null}) rather than inlining {status:'processing', stage:'parsing'}.
- **#349** ↑3 2026-08-02 `pitfall` repo:dispatch — Removing a TUI keybinding can orphan a whole subsystem; the compiler will not find it for you
- **#470** ↑3 2026-08-21 `convention` repo:frontend-erp — This repo's eslint max-params rule caps function signatures at 3 params; functions needing more must take a single destructured options object, not an eslint-disable.
- **#136** ↑2 2026-06-14 `landscape` repo:wizard_game — godot-dotnet-mcp exposes only the `intelligence` category to agents by default; the curated agent-facing tools live in tools/intelligence/impl_*.gd, not the raw category executors.
- **#270** ↑2 2026-07-08 `convention` user: — Kognic/annotell Renovate config comes from an org preset (globalExtends → github>annotell/renovate-config); repos need no renovate.json but a repo-local one layers on top, and rationale goes in the packageRule `description` field (strict JSON, no inline comments).
- **#277** ↑2 2026-07-08 `procedural` repo:exec-planning-scala — To fix a conflicting PR by merging master into a long-lived branch (e.g. migrate-coordinated-app-workload), push HEAD directly with `git push origin HEAD:<branch-name>` since the dispatch worktree's local branch name differs from the actual target branch.
- **#334** ↑2 2026-07-29 `pitfall` repo:dispatch — core.allium's `transitions status` graph is descriptive only — no code anywhere enforces status edges, so never assume a status write is edge-validated
- **#358** ↑2 2026-08-02 `pitfall` repo:dispatch — For a "test X fails on main" task, re-fetch and re-run the test on current origin/main at WRAP-UP, not just at start — a sibling agent can land the fix mid-session, making your whole session redundant.
- **#360** ↑2 2026-08-03 `pitfall` repo:dispatch — When consolidating two subprocess/wait implementations, compare their wakeup shapes before picking one — a blocking recv on a drain channel is not a slow poll loop, and replacing it with polling silently regresses hot-path latency.
- **#373** ↑2 2026-08-11 `pitfall` repo:dispatch — Deleting a redundant code path can silently orphan the only test covering an invariant the surviving path still relies on — the suite stays green while the invariant becomes unenforced.
- **#395** ↑2 2026-08-12 `pitfall` repo:dispatch — Changing what a dispatch prompt contains touches plugin/skills/ too — retro's surfaces table and allium-loop's verify-command resolution list both read prompt contents, and only a grep finds them.
- **#399** ↑2 2026-08-13 `pitfall` repo:frontend-erp — The allium specs record settled decisions about what data external Kognic services CANNOT provide; verify those claims against the owning service's source before accepting them, as some are already stale.
- **#466** ↑2 2026-08-21 `landscape` epic:299 — frontend-erp's Kognic platform API clients (deliverable, article, project-management, sales-orders etc.) authenticate only via a browser-set OAuth session cookie; there is no service-account or bearer-token path, so a standalone Node process cannot call them directly.
- **#92** ↑1 2026-06-03 `landscape` epic:127 — All data needed for the three SLO types (input count, client review time, phase dwell time) is available today via existing project-management-api endpoints — no new data pipeline needed for the frontend PoC
- **#102** ↑1 2026-06-08 `pitfall` repo:dispatch — wrap_up(rebase) fails with \"Cannot rebase onto multiple branches\" when pull.rebase=true is set in git config and FETCH_HEAD has stale entries
- **#103** ↑1 2026-06-08 `pitfall` repo:dispatch — SQLite datetime('now') has 1-second granularity — using updated_at as a change-detection sentinel misses rapid DB writes within the same second
- **#120** ↑1 2026-06-11 `tool_recommendation` user: — Verify kognic-slides decks by screenshotting individual slides headless via the deep-link hash (#N).
- **#160** ↑1 2026-06-18 `pitfall` user: — The dispatch MCP has no tool to set kanban blockedBy/blocks dependencies — create_task and update_task lack dependency fields; encode ordering in task descriptions + sort_order, and set formal blockedBy links manually in the TUI.
- **#172** ↑1 2026-06-21 `pitfall` epic:166 — In Allium lang v3, top-level `@invariant Name` + prose does NOT parse; use `invariant Name { for x in Entity[ where ...]: <bool expr> }` with prose as `--` comments (NOT @guidance, which is rejected inside top-level invariant blocks).
- **#202** ↑1 2026-06-24 `convention` repo:dispatch — Internalize cache-priming inside the method that reads the cache; never leave it as a caller precondition.
- **#230** ↑1 2026-06-28 `pitfall` repo:dispatch — Arc<dyn Sub> only upcasts to Arc<dyn Super> along a DECLARED supertrait edge, not via a blanket impl
- **#231** ↑1 2026-06-28 `pitfall` repo:dispatch — Splitting a DB trait into read/write halves breaks only TEST modules (concrete &Database), not production (trait objects)
- **#239** ↑1 2026-07-05 `pitfall` repo:oratorie — oratorie: a Svelte 5 `$state(config.X)` where config is `as const` infers a literal type, so later numeric assignment fails svelte-check — annotate `$state&lt;number&gt;(config.X)`.
- **#248** ↑1 2026-07-06 `pitfall` repo:oratorie — pdfjs-dist in vitest: use the legacy build and pin OMR tests to the node environment, or vector extraction silently returns nothing.
- **#265** ↑1 2026-07-07 `pitfall` repo:dispatch — `cargo fmt` reformats the whole crate and sweeps up pre-existing unformatted files unrelated to your change — revert them before committing so the diff/PR stays focused.
- **#296** ↑1 2026-07-14 `pitfall` repo:dispatch — gh's review-requested:@me also matches team-based review requests, not just personal ones — use user-review-requested:@me for personal-only, especially when widening scope from a repo list to whole orgs.
- **#297** ↑1 2026-07-14 `convention` repo:dispatch — The dispatch feed scripts in scripts/ are reference templates only — the live script driving a running feed epic is a separate, untracked copy at ~/.local/share/dispatch/scripts/, which may have already diverged.
- **#323** ↑1 2026-07-27 `landscape` repo:dispatch — config.learning_reflection_enabled gates an undocumented record_learning nudge fired from update_task PR-finalisation, not from wrap_up/exit_session
- **#328** ↑1 2026-07-29 `pitfall` repo:dispatch — The pre-push hook's `cargo fmt` auto-formats but never re-stages, so rustfmt drift can still land on main — run `cargo fmt --check` yourself before committing.
- **#337** ↑1 2026-07-31 `pitfall` repo:dispatch — tmux's `=` exact-match sigil only works for target-window commands; use a pane ID as the general fix for prefix-matched window targets
- **#339** ↑1 2026-07-31 `pitfall` repo:dispatch — query_usage all-time keybinding counts are misleading: the keymap changed 2026-07-25 (d/g retired for Space), so always window with `since` and cross-check against git log -S for the binding's introduction.
- **#344** ↑1 2026-07-31 `pitfall` repo:dispatch — Moving where a skill step runs is not just a SKILL.md edit — MCP handler response text can instruct the same ordering at runtime, and tool output outweighs skill prose.
- **#359** ↑1 2026-08-03 `pitfall` repo:dispatch — Retiring a tick reconciler needs BOTH a fold-into-one-transaction fix and a one-shot migration — making the two racing writes conditional is not sufficient on its own.
- **#362** ↑1 2026-08-03 `convention` repo:dispatch — To de-flake a concurrency test here, gate the reader on a writer-progress signal (std mpsc handshake) and then make every read an assertion — an iteration budget with `continue` on "not ready yet" is the flake.
- **#370** ↑1 2026-08-11 `convention` repo:dispatch — When a parent card stays visible because a descendant matched, gate that descendant by the same filters the board applies to it — an ungated descendant produces a card that opens onto an empty view.
- **#374** ↑1 2026-08-11 `landscape` repo:dispatch — tmux resolves a new pane's cwd from the SESSION (client splits) or the invoking process (external-CLI splits) — never from the pane being split; only `split-window -c <dir>` is reliable.
- **#413** ↑1 2026-08-15 `pitfall` repo:dispatch — A rendering test can pass while measuring something other than what it names — confine the measurement by region, source and metric, because mutation-testing the code will not catch a fault that lives in the test.
- **#454** ↑1 2026-08-21 `preference` repo:scala-common — Split domain model types one case class (+ its companion object) per file, rather than grouping several related types in one file — apply this to both main sources and their test files (one test class per model under test).
- **#94** ↑0 2026-06-03 `procedural` repo:annotell-data-warehouse +old-and-never-rated — Verify a dbt model reorg/rename created no duplicate or orphaned BQ tables by diffing the parsed manifest against INFORMATION_SCHEMA
- **#96** ↑0 2026-06-03 `pitfall` repo:frontend +old-and-never-rated — Jest tests run i18next in cimode, so t('key') returns the literal key string (e.g. 'slos.form.name'), not translated text — RTL queries like getByText/findByRole must match the key, not the English copy.
- **#98** ↑0 2026-06-03 `pitfall` repo:frontend +old-and-never-rated — A fresh git worktree has no local node_modules, and the repo's oxlint/eslint can't run standalone there — their JS plugins (kognic, sonarjs, risxss, testing-library) fail to load without a full npm install.
- **#124** ↑0 2026-06-11 `pitfall` repo:predictive-coding-pekko +old-and-never-rated — dispatch wrap_up rebase fast-forwards local main to your commit even if the rebased result no longer compiles — when origin/main advanced with a breaking API change during your session, run the build AFTER the rebase, not just before.
- **#125** ↑0 2026-06-11 `pitfall` repo:dispatch +old-and-never-rated — For migrations that drop a tasks column, prefer ALTER TABLE ... DROP COLUMN over the table-rebuild pattern — the rebuild silently drops later-added indexes.
- **#128** ↑0 2026-06-12 `pitfall` repo:dispatch +old-and-never-rated — Test-only hooks must be plain optional fields on production types, not #[cfg(test)] — integration tests in tests/ compile against the non-cfg(test) library and can't see cfg-gated fields.
- **#138** ↑0 2026-06-14 `pitfall` repo:wizard_game +old-and-never-rated — To verify godot-dotnet-mcp changes live, the Godot editor must be open on THE SAME worktree you edited — the embedded MCP server binds a single shared port (3000), so only one editor serves at a time and it reflects only that checkout's code.
- **#155** ↑0 2026-06-17 `pitfall` repo:staff_engineer +old-and-never-rated — Base64-inline deck SVGs by reading files inside python, not via shell args — the assets are ~180KB and blow past ARG_MAX
- **#179** ↑0 2026-06-21 `procedural` epic:166 +old-and-never-rated — Per-spell specs: the visual-phase logic is pre-extracted/tested, but the projectile motion curve (acceleration/velocity-clamp) often still lives untested inside the Godot _PhysicsProcess and IS a real extractable gap.
- **#180** ↑0 2026-06-21 `pitfall` epic:166 +old-and-never-rated — Allium uses `else if`, not `elif`, in conditional expressions — the prose-Allium specs use `elif` and fail to parse.
- **#181** ↑0 2026-06-21 `landscape` epic:166 +old-and-never-rated — checkpoint.allium is fully Godot-bound — no core/ logic, so step 5 (TDD coverage) was a no-op; the only work was parse-fix + weed alignment.
- **#182** ↑0 2026-06-21 `landscape` epic:166 +old-and-never-rated — world.allium is entirely Godot-bound: all propagate obligations map to node lifecycle/autoload/signal wiring with no core/ logic, so step-5 TDD is a no-op.
- **#183** ↑0 2026-06-21 `pitfall` epic:166 +old-and-never-rated — When a spec models a per-frame config the code inlines but the shared cross-file value type lacks a field for it, model it via a dedicated rule+config, not the data-init literal.
- **#184** ↑0 2026-06-22 `pitfall` epic:166 +old-and-never-rated — In this epic, some 'gaps' are real code bugs: a core helper is implemented AND unit-tested but never wired into its consuming state — the fix is wiring, not new logic.
- **#186** ↑0 2026-06-22 `pitfall` epic:70 +old-and-never-rated — On a scalePoint chart over aggregated day/week/month buckets, milestone/reference lines silently vanish unless their date is snapped to the active resolution's bucket key and unioned into the x-domain.
- **#187** ↑0 2026-06-22 `pitfall` repo:dispatch +old-and-never-rated — In feed scripts, a multi-term gh search qualifier (e.g. `commenter:@me -author:@me`) must go after `--` and be word-split, not passed as one quoted positional arg.
- **#203** ↑0 2026-06-24 `convention` repo:dispatch +old-and-never-rated — Use `Arc<T>` when a `&mut self` method needs to return a reference to cached data that callers also read via `&self` methods.
- **#207** ↑0 2026-06-24 `convention` repo:dispatch +old-and-never-rated — Use std::mem::discriminant to compare enum variants without cloning heap-allocating payloads
- **#232** ↑0 2026-06-30 `pitfall` repo:wizard_game — When unit-testing a float time-accumulator that fires every N seconds, avoid deltas that land exactly on the interval boundary — float drift (e.g. 0.4f-0.3f != 0.1f) makes a `>= interval` check miss and the test fail spuriously.
- **#235** ↑0 2026-07-03 `procedural` repo:wizard_game — A long-lived dispatch worktree that diverges from main is best reconciled by rebuilding on main (reset --hard main + checkout feature files from a backup branch) rather than a multi-conflict rebase.
- **#246** ↑0 2026-07-05 `tool_recommendation` user: — CPDL/ChoralWiki (cpdl.org) is the best source for free SATB scores with matching MIDI + PDF (+ often MusicXML); WebFetch gets 403, so fetch pages with curl + a browser User-Agent.
- **#254** ↑0 2026-07-06 `landscape` repo:oratorie — In OMR, LilyPond/Emmentaler draws barlines and stems as filled rectangles (the PDF fill layer), not stroked lines — detect them via extract.rects, not strokes.
- **#255** ↑0 2026-07-06 `landscape` repo:oratorie — Emmentaler accidentals use two placement conventions the OMR attach logic must handle: standard (glyph at the note's pitch ~1–1.5 interline left) and editorial/musica-ficta (glyph floating above the staff directly over its note, same x, variable Δy).
- **#257** ↑0 2026-07-06 `pitfall` epic:212 — OMR repeat unfolding: Emmentaler draws repeat dots and augmentation dots with the SAME glyph name 'dots.dot'; distinguish repeat dots by geometry (≥2 dots forming a pair beside a barline rect, aligned across a system's staves), not by name.
- **#259** ↑0 2026-07-07 `pitfall` repo:k8s-platform-gitops — Airflow triggerer with KEDA enabled scales to 0 by default (minReplicaCount 0), so the UI permanently reports the triggerer unhealthy — set triggerer.keda.minReplicaCount: 1 to keep one warm pod.
- **#273** ↑0 2026-07-08 `landscape` epic:70 — Production-plan real-data swap coexists with the mock-based change-log/time-travel feature via "Variant B": core data is real, the change-log stays on the mock store+seed.
- **#287** ↑0 2026-07-10 `convention` repo:dispatch — Prefer a cheap self-healing fingerprint over disciplined manual invalidation for caches derived from mutable shared state when many call sites can mutate the source.
- **#292** ↑0 2026-07-13 `pitfall` repo:dispatch — SQLite's datetime('now') has 1-second resolution, so recency-ordered tables (ORDER BY last_used DESC) can tie on rapid successive writes within a test or a fast user action.
- **#304** ↑0 2026-07-16 `pitfall` repo:dispatch — `cargo fmt -- <specific-file-paths>` reformatted the whole crate in this repo, not just the named files, silently touching unrelated pre-existing formatting debt.
- **#306** ↑0 2026-07-17 `convention` repo:oratorie — oratorie: to make a SvelteKit form action testable, extract its persistence into a deps-injected handler in $lib/server (e.g. enqueueUploadedPiece({db,storage}, input)) and test it with PGlite.
- **#310** ↑0 2026-07-21 `pitfall` repo:oratorie — oratorie's docker-compose db binds host port 5432, so running `npm run dev` in two worktrees at once fails with "port is already allocated" — the fix is to notice another worktree's db container already up, not to reset the current one
- **#315** ↑0 2026-07-25 `pitfall` epic:269 — Epic #269 doc-fix tasks describe stale text that may already be fixed on an unmerged sibling task's branch — check sibling branches (git branch --all + git log -p) before assuming the described stale text exists in your own worktree.
- **#316** ↑0 2026-07-26 `landscape` repo:dispatch — .claude/skills/allium-weed-loop/ has the same raw-iteration-counter problem as allium-loop (pre-fix) plus a stale hardcoded 3-file spec list
- **#330** ↑0 2026-07-29 `pitfall` repo:dispatch — Runtime test fixtures must give every service ONE shared Database — a second in-memory DB silently breaks cross-entity FKs and the error is swallowed into a tracing::warn
- **#335** ↑0 2026-07-29 `pitfall` repo:dispatch — Never bump a counter inline as a `tracing::warn!` field value — the macro skips evaluating field expressions when no subscriber has the event enabled, so the counter silently stops counting
- **#348** ↑0 2026-08-02 `pitfall` repo:dispatch — Plan code snippets get rendered verbatim by implementers — review them as code, not illustration
- **#356** ↑0 2026-08-02 `pitfall` repo:dispatch — origin/main can be many commits behind local main — the dispatch prompt says rebase onto origin/main, but wrap_up rebases onto local main
- **#364** ↑0 2026-08-03 `pitfall` repo:dispatch — Don't file an intermittent test failure here as a benign flake — investigate the write/flush path first. Two knowledge-base entries labelled the mcp::trajectory tests "flaky, not a regression" for weeks while they were actually catching real audit-log data loss.
- **#366** ↑0 2026-08-03 `pitfall` repo:dispatch — To order two racing hook processes' writes, stamp and compare each hook's EVENT time — a generation counter or any predicate over current columns cannot do it, because the stamping hook reads the pre-increment value.
- **#368** ↑0 2026-08-11 `landscape` repo:dispatch — wrap_up latency is entirely action="rebase" and is one unavoidable git-pull network round-trip, not a defect — measure MCP call durations from the trajectory JSONL before theorising.
- **#382** ↑0 2026-08-12 `pitfall` repo:annotell-data-warehouse — uv pip compile reads the existing output requirements.txt as version preferences, so recompiling while it still has merge-conflict markers fails with "Unexpected '<'"
- **#383** ↑0 2026-08-12 `convention` repo:annotell-data-warehouse — Stack sibling CVE-bump PRs with a merge commit, not a rebase, since they all conflict on requirements.{in,txt} and are already under review
- **#386** ↑0 2026-08-12 `convention` repo:dispatch — Allium `requires:` clauses use single `=` for equality and lowercase `and`/`or` — `==` and `AND` do not parse.
- **#391** ↑0 2026-08-12 `pitfall` user: — Grafana's Airflow StatsD duration metrics can't answer "which tasks ran over X" — use the Airflow /api/v2 REST API instead
- **#402** ↑0 2026-08-13 `convention` repo:dispatch — Deleting a guard as unreachable? Pin the premise that makes it unreachable, not just that consumers stopped checking
- **#412** ↑0 2026-08-15 `pitfall` repo:dispatch — tmux swap-pane moves pane *objects* between windows — a pane ID resolved before the swap identifies a pane in the *other* window afterward, not the one it started in.
- **#417** ↑0 2026-08-15 `pitfall` repo:dispatch — Hook-payload field/tool-name assumptions written from training-data knowledge, not a captured live payload, silently never fire — always verify with a real hook capture before trusting them.
- **#421** ↑0 2026-08-16 `pitfall` repo:dispatch — Before adding a test for behaviour you are moving, search for existing coverage by the mocked subprocess output, not by the Rust type name — argv-level mock tests never mention the error enum they exercise.
- **#422** ↑0 2026-08-16 `convention` repo:dispatch — Asserting on warn-level tracing output has one shared test harness — use it instead of writing another in-memory subscriber sink.
- **#430** ↑0 2026-08-16 `convention` repo:dispatch — Adding a declarative mock-sequence shape? Sweep the repo for the hand-written response queues it obsoletes — stale ones pass silently
- **#437** ↑0 2026-08-17 `pitfall` repo:dispatch — Inserting a step into the TUI task-creation chain breaks every test that drove the old terminal transition — the compiler cannot find them, because the chain is hand-linked by mode assignment rather than typed.
- **#443** ↑0 2026-08-18 `procedural` repo:scala-common — When a CVE hits a transitive dependency pulled in by a BOM-managed SDK (e.g. AWS SDK v2's apache5-client → httpclient5), check whether a newer BOM version already pins that dependency past the fix version before adding an explicit force-pin override.
- **#444** ↑0 2026-08-18 `pitfall` repo:dispatch — Two reference feed scripts can cover the same upstream data source; a field added to one on a later commit can silently miss its sibling.
- **#449** ↑0 2026-08-21 `pitfall` repo:scala-common — In scala-common, a module that depends only on `libs.pekko.testkit` (no other Pekko library) fails Gradle dependency resolution with a blank version, because pekko-testkit's version comes from BOM/platform alignment with another resolved Pekko artifact in the graph, not from the version catalog.
- **#472** ↑0 2026-08-23 `pitfall` repo:frontend-erp — A plain-node (non-tsx, non-ts-jest) script importing @modelcontextprotocol/sdk deep subpaths needs the .js extension to resolve at runtime, the opposite of the tsx/ts-jest convention.
- **#474** ↑0 2026-08-23 `pitfall` repo:frontend-erp — In the production-plan model, absent capacity and zero capacity are equivalent — clearing a capacity range is a zero-write, not a row deletion.
- **#480** ↑0 2026-08-24 `pitfall` repo:frontend-erp — In frontend-erp's installed MCP SDK version, registering a tool's inputSchema as a z.discriminatedUnion makes tools/list report empty properties, not the real per-branch schema — even though runtime call validation against that union still works correctly.
- **#491** ↑0 2026-08-27 `convention` repo:dispatch — Assert kanban column-priority ordering as a relative chain, never as literal integers — the slot numbers exist to be renumbered.
- **#157** ↑-1 2026-06-18 `convention` repo:dispatch +old-and-never-rated — To gate a specific tool/command via a Claude Code hook, add a second PreToolUse script + a thin `dispatch <subcommand>` that exits 2 to block (stderr is shown to the agent) or 0 to allow.
- **#350** ↑-1 2026-08-02 `landscape` repo:dispatch — Since #3810, a worktree branches from local <base> when it is ahead of origin/<base> — so entries #288/#326/#149/#341 describe pre-#3810 behaviour and no longer apply.
- **#445** ↑-1 2026-08-18 `pitfall` repo:user-scala — In user-scala, repeated password-login attempts against the same email in local dev/e2e testing quickly trip an in-memory per-email login rate limiter (limit 10), which silently rejects further login POSTs rather than erroring clearly.
- **#467** ↑-1 2026-08-21 `pitfall` repo:frontend-erp — In frontend-erp, @modelcontextprotocol/sdk deep subpath imports (e.g. server/mcp, client/index, inMemory) must be written WITHOUT a trailing .js extension, or eslint's import/no-unresolved fires even though tsc and tsx both resolve the .js form fine.

### transient (4)

- **#150** ↑1 2026-06-16 `pitfall` repo:airflow-dags — apache-beam hard-caps cryptography<48.0.0, blocking cryptography>=48 security bumps; the beam provider is unused in airflow-dags (its Dataflow DAG was split out in #462) and can be removed to clear the ceiling.
- **#425** ↑0 2026-08-16 `landscape` repo:dispatch — The service layer's remaining dependency on the dispatch adapter is real orchestration, not layering inversion — the pure predicates have moved to the domain model.
- **#476** ↑0 2026-08-23 `landscape` repo:frontend-erp — An unwired deleteProjectDelivery client function and React-Query hook already exist outside the MCP layer, called by nothing but tests.
- **#485** ↑0 2026-08-26 `convention` repo:export-scala — The exports database is managed by dbmate; the Flyway migration directory still sitting in the infrastructure module is dead and nothing references Flyway.

### superseded (1)

- **#100** ↑2 2026-06-07 `pitfall` user: — Allium checker (v3.2.x) only detects a chained-trigger emission when it is a top-level statement or an if/else branch placed BEFORE any state-change or for-loop in the ensures block; emissions inside for-loops or bare if (no else), or after a state-change/for-loop, are missed and produce false-positive unreachableTrigger info.

### old-and-never-rated (6)

- **#90** ↑0 2026-06-03 `pitfall` repo:staff_engineer — Vault frontmatter merges can conflict when main adds a new field (e.g. people:) to the same file you modified (e.g. adding related:) — resolve by keeping both fields rather than choosing one
- **#95** ↑0 2026-06-03 `pitfall` user: — BigQuery does not support CTE column-name lists; use UNNEST with a typed STRUCT to build an inline literal table with named columns
- **#143** ↑0 2026-06-15 `pitfall` repo:dispatch — Editor file-writes can silently land in the parent checkout instead of the worktree, leaving worktree files empty and blocking the wrap-up fast-forward.
- **#196** ↑0 2026-06-23 `pitfall` repo:dispatch — When decompose-review runs from a worktree, write plan files with the main-repo absolute path (not a relative path) so plan_path and the actual file location agree.
- **#226** ↑0 2026-06-26 `pitfall` repo:staff_engineer — OKF (Open Knowledge Format) is a data catalog format for BigQuery tables/APIs, not a personal knowledge management format — migrating a markdown vault to OKF would be a category error
- **#211** ↑-1 2026-06-25 `pitfall` repo:dispatch — wrap_up(action: "rebase") on a research-tagged task moved it to review/awaiting_review instead of done, making the exit token invalid
