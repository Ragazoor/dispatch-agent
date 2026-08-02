# Code Conventions

## Rendering purity

Code under `src/tui/ui/` must be pure: it reads `App` and shared helpers, writes ratatui buffers, and does nothing else.

**Allowed:**
- Immutable reads of `App` fields and shared helpers from `src/tui/ui/shared.rs` and `src/tui/ui/palette.rs`
- Writes to the ratatui `Buffer` / `Frame` passed in by the caller
- Pure formatting (`format!`, `truncate`, span construction)

**Forbidden:**
- Database access (no `self.db`, no `Database::*`, no `rusqlite`)
- File I/O (`std::fs`, `std::io`, `tokio::fs`)
- Process spawning (`std::process::Command`, `tokio::process`)
- Async runtime calls (`tokio::*`, `block_on`, channel sends/receives)
- MCP calls or network I/O
- `unwrap()` / `expect()` / `panic!` on data the render layer can't control — render must never crash the TUI on bad input

**One narrow exception:** a guarded `unreachable!()` in a match arm is acceptable when an upstream filter/type already rules that arm out — e.g. `src/tui/ui/kanban/columns.rs` matches on `ColumnItem` after a prior pass has stripped `EpicHeader`/`SubstatusLabel`/`OrphanSeparator` variants, so those arms can't be hit. This differs from an unguarded `unwrap()`: the invariant is upheld by code the reader can point to, not by hoping the data is well-formed. This exception is render-only — MCP handlers and `src/tui/input.rs` must never panic, guarded or not, since their inputs (MCP args, keystrokes) aren't invariant-checked upstream the way render's `App` state is.

If a render path needs data that isn't on `App`, compute it in the runtime/update layer and stash the result on `App` before rendering — do not reach for it from `src/tui/ui/`.

## Single-line text-field caret

Every `InputMode` that types free text into `InputState.buffer` (task/epic title,
base branch, todo title/quick-add, repo-path & quick-dispatch query, filter-preset
name) shares one caret model:

- `InputState.caret` is a **character** index into `buffer` (count of chars left
  of the caret), invariant `0..=buffer.chars().count()`. It is never a byte
  offset — conversion happens only at the edit/render call sites.
- All caret arithmetic lives in `src/tui/text_caret.rs` (pure, unit-tested):
  `insert`, `delete_before` (Backspace), `delete_after` (Delete), `move_left`,
  `move_right`, `word_left`/`word_right` (whitespace-only boundaries), `home`,
  `end`, `byte_offset`. Handlers call these; they never `buffer.push`/`pop`.
- **Every** write to the buffer goes through `InputState::set_buffer` (lands the
  caret at the end — natural for editing a prefilled value) or
  `InputState::clear_buffer` (caret to 0). Never assign `input.buffer` directly,
  including in tests — a direct assignment leaves the caret stale at 0 and the
  next Backspace/insert misbehaves.
- Key routing for caret motions is centralised in `text_edit_message()` in
  `src/tui/input.rs`, called by all three text routers (`handle_key_text_input`,
  `handle_key_quick_dispatch`, `handle_key_input_preset_name`). `Ctrl+←/→` are
  the primary word-motion keys; `Alt+←/→` and readline `Alt+B`/`Alt+F` are the
  modifier-free fallback for tmux without `xterm-keys` (see docs/reference.md).
- No handler needs to flag a caret move as render-worthy: `handle_key` ends with
  an unconditional `self.dirty = true` (`src/tui/input.rs`), so every keystroke
  schedules a redraw. The earlier opt-in dirty detector had to snapshot
  `input.caret` explicitly or the frame was skipped and the caret didn't visibly
  move — see the "Render dirty flag (fail-open)" bullet in
  `docs/architecture.md` for why that was replaced.
- Rendering uses `ui::caret_line`, which draws the caret as a reversed block cell
  and horizontally scrolls long values so the caret stays visible.

`SearchTasks` (`search.query`) and `ManagedFeedConfig` (per-field strings) use
separate buffers and are intentionally not on this shared caret yet.

Two deliberate limitations: the caret is a Unicode scalar (`char`) index, so a
move/delete can split a **grapheme cluster** (combining accents, ZWJ emoji) —
never a UTF-8 codepoint, so no panic, just a possible visual glitch for exotic
input. And word motion (`word_left`/`word_right`) treats only alphanumeric/`_`
as word chars, so punctuation and path separators (`/`, `-`, `.`) are word
boundaries — `Ctrl+←/→` steps through path segments, which is the point.

## Soft-fail decoding

Schema enum values may be added in a migration before all rows are upgraded. Never `panic!` (or `unwrap()`/`expect()`) on a value read from the DB — a poisoned row must not kill the TUI.

**Field level.** An enum that can legitimately gain variants (a newer binary writing a value this one doesn't know) defaults with a warning: `Enum::parse(&s).unwrap_or_else(|| { tracing::warn!(...); Enum::Default })`. `parse_feed_role` and `parse_epic_origin` (`src/db/queries/mod.rs`) are the canonical examples. Fields where no default is meaningful — `status`, `sub_status`, `tag`, `wrap_up_mode`, the timestamps, the `url`/`url_type` pair — instead fail the row via `unknown_enum`, and the row-level policy below decides what that costs.

**Row level — the decode-failure policy.** Which reads tolerate an undecodable row is deliberate, not incidental:

- **Bulk reads skip and warn.** `list_all`, `list_by_status`, `list_epics`, `list_root_epics`, `list_sub_epics`, `list_tasks_for_epic`, and `list_all_tasks_with_epic_id` run their `query_map` iterator through `collect_decodable` (`src/db/queries/mod.rs`), which drops each undecodable row with a `tracing::warn!` and keeps the rest. One corrupt row degrades the board instead of blanking it — before this, a single unparseable `status` made `list_all` return `Err` and the TUI rendered an empty board.
- **Single-entity reads fail loudly.** `get_task`, `get_epic`, and `find_task_by_plan` return the decode error: the caller asked for that specific row, so silently answering "not found" would be a lie. This is also how a corrupt row stays diagnosable once the bulk read has stopped surfacing it.
- **Only decode errors are skippable.** `collect_decodable` propagates anything that is not a row-content failure — a `SqliteFailure` (I/O error, interrupt) mid-iteration would otherwise silently truncate a healthy result set, and an `InvalidColumnName` would hide a mismatch between `TASK_COLUMNS` and `row_to_task`.

**Decode-fallback counter:** every field-level default and every row skipped by `collect_decodable` bumps a process-wide `AtomicU64` exposed as `crate::db::decode_fallback_count()`. The value is included in the `tracing::warn!` (`count=N`) so the warns are greppable in aggregate, and the accessor lets tests and ad-hoc debugging detect slow-bleeding decode bugs without chasing log lines. It is monotonic and never reset — assert on deltas, since the test suite shares one process. When you add a new soft-fail branch, bump it via `db::queries::bump_decode_fallback()`.

## Border parsing

Untrusted inputs — MCP JSON-RPC arguments, editor output, feed-script JSON, plan files — must be parsed into typed domain enums **at the boundary**. Business logic should never see raw `serde_json::Value` or `String` for fields that have a typed shape.

- MCP handlers in `src/mcp/handlers/` parse to typed `*Args` structs (with serde derives) before calling into the service layer.
- Feed scripts produce `FeedItem` JSON which is parsed into the typed struct in `verify-feed` and at runtime ingest.
- Plan files are parsed by `src/plan.rs` into a typed plan structure.

Parse failures must surface to the caller as a `ServiceError::Validation` (or `-32602` at the MCP layer). Silent fallback to a default value is forbidden — if the input is invalid, the caller needs to know.

## `FieldUpdate` — nullable string fields

`FieldUpdate` (`src/service/mod.rs`) replaces the `Option<String>` + empty-string sentinel anti-pattern for fields that need three states: "don't touch", "set to value", "clear to NULL":

```rust
pub enum FieldUpdate {
    Set(String),  // set the field to this value
    Clear,        // set the field to NULL
}
```

Used in `UpdateTaskParams` for `worktree` and `tmux_window`. When adding a new nullable string field to `UpdateTaskParams`, use `Option<FieldUpdate>` rather than `Option<Option<String>>`.

**When to use:** if the caller can clear the field to NULL (nullable column, user-clearable), use `FieldUpdate`. If the field is non-nullable or the update path only ever sets a value, use a plain `String` (or `Option<String>` to mean "don't touch / set"). Reserve the three-state pattern for genuinely tri-valued updates.

## `UrlUpdate` — the typed-URL sibling of `FieldUpdate`

The task URL is **not** an `Option<FieldUpdate>`. Because the URL and its type are always set together, `UpdateTaskParams.url` is an `Option<UrlUpdate>`, where `UrlUpdate` (`src/service/mod.rs`) carries a whole `TaskUrl` (`src/models/url.rs`) rather than a bare `String`:

```rust
pub enum UrlUpdate {
    Set(TaskUrl),  // set url + url_type together
    Clear,         // set the field to NULL
}
```

It mirrors `FieldUpdate` (same three-state semantics, same `Some(Some(_))`/`Some(None)`/`None` bridge to the DB patch) and is consumed in `src/service/tasks/crud.rs`.

**When `UrlUpdate` vs `FieldUpdate`:** use `UrlUpdate` for the typed task URL specifically — it keeps `url` and `url_type` in lockstep (e.g. `crud.rs` inspects `UrlUpdate::Set(u) if u.is_pr()` to drive PR-specific behaviour). Use `FieldUpdate` for plain nullable *string* fields. The distinction is not compiler-flagged: a contributor who assumes the URL uses `FieldUpdate` will not get a type error pointing them here, so reach for `UrlUpdate` whenever the field is the task URL.

## `TaskPatch` / `EpicPatch` — double-Option in the DB layer

`TaskPatch` and `EpicPatch` (`src/db/mod.rs`) use `Option<Option<T>>` for nullable fields — the DB-layer equivalent of `FieldUpdate`:

| Value | Meaning |
|-------|---------|
| `None` | Don't touch this field |
| `Some(None)` | Set the field to NULL |
| `Some(Some(v))` | Set the field to `v` |

The service layer bridges the two patterns before writing a patch: `FieldUpdate::Set(v)` becomes `Some(Some(v))` and `FieldUpdate::Clear` becomes `Some(None)`. When adding a new nullable field, use `FieldUpdate` in `UpdateTaskParams`/`UpdateEpicParams` and double-Option in the corresponding patch struct.

### OwnedTaskPatch (and OwnedCreateTaskRequest)

`db_call` closures must be `Send + 'static`, so borrowed fields from `TaskPatch<'_>` cannot
cross the boundary. `OwnedTaskPatch` and `OwnedCreateTaskRequest` in `src/db/queries/tasks.rs`
are owned mirrors that exist solely to satisfy this constraint. Convert via the `From` impl:
`OwnedTaskPatch::from(patch)`.

**Parity is compiler-enforced.** Both `From` impls use an exhaustive destructuring of the
source struct (no `..`), so adding a field to `TaskPatch` or `CreateTaskRequest` without
also updating the owned mirror and its `From` impl is a **compile error**. When you add a
field, name it in the destructuring pattern and add it to the `Self { … }` construction; the
compiler rejects anything less.

`OwnedTaskPatch` deliberately omits `labels` — labels are pre-serialised to JSON before
entering `db_call` and handled via `labels_json` in `patch_task`. The `labels: _` binding in
the `From` impl keeps the exhaustive pattern intact despite the omission.

### updated_field_names — same pattern, same reason

`UpdateTaskParams::updated_field_names` (`src/service/tasks/params.rs`) and
`UpdateEpicParams::updated_field_names` (`src/service/epics.rs`) use the identical exhaustive
destructuring, and for a load-bearing reason: `has_any_field()` is defined as
`!updated_field_names().is_empty()`, and both `update_task` and `update_epic` reject the whole
request with `"At least one field must be provided"` when it returns false. Omit one entry and
an update that sets *only* that field is refused — with a message saying the opposite of what
happened. Adding a field without naming it in the pattern is a compile error; naming it without
listing it produces an unused-binding warning (a hard error under `-D warnings`).

What the compiler *can't* catch is a wrong name — listing `"title"` against the `description`
field compiles cleanly. That is the gap the `*_every_field_covered` unit tests fill: each sets one
field and asserts `updated_field_names()` returns exactly that name. Keep them in sync when adding
a field; they are not redundant with the destructuring.

## DB trait narrowing — take the narrowest sub-trait you need

`TaskStore` (`src/db/mod.rs:570`) is a supertrait of `TaskAndEpicStore + TaskReadStore + SettingsStore + LearningStore + LearningRetrievalStore + UsageStore`. New consumers should hold the narrowest sub-trait they actually call:

| Consumer | Holds |
|----------|-------|
| `TaskService` | `Arc<dyn TaskAndEpicStore>` (write) |
| `EpicService` | `Arc<dyn TaskAndEpicStore>` (write) |
| `McpState`, `TuiRuntime` | `Arc<dyn TaskReadStore>` (no task/epic mutations — see caveat below) |
| `FeedRunner`, `TuiRuntime::feed_db` | `Arc<dyn TaskStore>` (write — sanctioned feed-mutation consumers) |

`Arc<dyn TaskStore>` coerces to any narrower trait object at call sites via Rust's trait-object upcasting (stabilised in 1.86). If you need to split a wide `Arc<dyn TaskStore>` into a narrower one, use a typed `let` binding: `let d: Arc<dyn EpicCrud> = task_store_arc.clone();`.

## Service trait narrowing — `Arc<dyn TaskServiceApi>` / `Arc<dyn EpicServiceApi>`

Parallel to DB trait narrowing, the service layer exposes these traits in `src/service/api.rs`:

| Trait | Production impl | Where held |
|-------|----------------|------------|
| `TaskServiceApi` | `TaskService` | `TuiRuntime::task_svc`, `McpState::task_svc` |
| `EpicServiceApi` | `EpicService` | `TuiRuntime::epic_svc`, `McpState::epic_svc` |
| `TodoServiceApi` | `TodoService` | `TuiRuntime::todo_svc` |
| `LearningServiceApi` | `LearningService` | `TuiRuntime::learning_svc`, `McpState::learning_svc` |

Consumers that call task or epic operations should hold `Arc<dyn TaskServiceApi>` / `Arc<dyn EpicServiceApi>` rather than the concrete struct. This lets unit tests inject a mock service without a real database — construct `McpState` directly (all fields are `pub` or `pub(crate)`) and pass a custom `Arc<dyn TaskServiceApi>`.

### Each seam is declared once — edit the spec macro, not the impls

Every seam's signature list lives in exactly one place: a `macro_rules!` *spec* macro in `src/service/api.rs` (`task_service_api!`, `epic_service_api!`, `todo_service_api!`, `learning_service_api!`). A spec macro takes the name of an *emitter* macro and replays its signature list into it, so trait, impl, and mock scaffolding are all generated from the same tokens and cannot drift:

| Emitter | Generates |
|---------|-----------|
| `service_api_trait!` | the `#[async_trait]` trait declaration |
| `service_api_delegate!` | the production impl, delegating via UFCS (`TaskService::method(self, …)`) so the inherent methods are not shadowed |
| `service_api_stub_trait!` | a test-only `*ServiceApiStub` trait whose methods all default to `panic!("… is not mocked")` |
| `service_api_stub_bridge!` | `impl <Api> for <MockType>`, forwarding every method to that mock's stub-trait impl |

**Adding or changing a method** means editing one signature list. Types there are fully qualified (`$crate::models::TaskId`, …) because `macro_rules!` resolves type paths at the *call site*, and mocks in other modules invoke the same spec macro.

**Writing a mock**: implement the `*ServiceApiStub` trait, override only the methods the test exercises, then bridge it onto the real seam:

```rust
#[async_trait::async_trait]
impl crate::service::TaskServiceApiStub for MockTaskService {
    async fn list_tasks(&self, _: ListTasksFilter) -> Result<Vec<Task>, ServiceError> {
        Ok(self.tasks.clone())
    }
}

crate::task_service_api!(service_api_stub_bridge, MockTaskService);
```

Unmocked calls panic rather than silently returning a default, and a new seam method no longer breaks unrelated mocks with `E0046`. Stub traits are generated for `TaskServiceApi` and `LearningServiceApi` (the seams with mocks); add a `#[cfg(test)] <spec>!(service_api_stub_trait);` line in `api.rs` when another seam needs one.

**`LearningServiceApi` injection is complete.** `src/service/api.rs` exports `LearningServiceApi` and a `MockLearningService` (test-only, an empty `LearningServiceApiStub` impl, so every method panics). Both `TuiRuntime` and `McpState` hold `learning_svc: Arc<dyn LearningServiceApi>`, constructed once at startup. Tests that do not exercise learning operations use `MockLearningService`; tests that need real learning behaviour (e.g. `runtime/learnings.rs`, `runtime/editor.rs` learning-editor tests) inject `Arc::new(LearningService::new(db, emb_svc))` directly.

## Service layer is the mutation boundary

Reading through `state.db` directly is fine — list, get, and other queries have no side effects beyond the read. **Mutations are different: task and epic writes go through `TaskServiceApi` / `EpicServiceApi`, not `state.db` directly.** The service layer owns the invariants that a bare DB write would skip — most importantly epic-status recalculation (see below).

**This boundary is now compiler-enforced.** `McpState.db` and `TuiRuntime.database` are typed `Arc<dyn db::TaskReadStore>`, not `Arc<dyn db::TaskStore>`. `TaskReadStore` exposes the task/epic **read** surface (`TaskRead` + `EpicRead`) plus the settings/learning/usage stores, but **not** `TaskCrud`/`EpicCrud`. So `state.db.patch_task(...)` (or `create_epic`, `set_task_epic_id`, `recalculate_epic_status`, …) from a handler is a **compile error**. A `compile_fail` doctest on `TaskReadStore` (`src/db/mod.rs`) locks this in. <!-- allow-phantom-symbol: compile_fail is a rustdoc attribute, not our symbol -->

**The name is scoped on purpose.** `TaskReadStore` seals **task/epic** writes only, not every write — settings/learning/usage writes stay reachable through it (see the caveat below). The old name `ReadStore` implied read-only-everything, which was a misnomer; the `Task` prefix makes the guarantee honest.

How the seam works:

- `TaskCrud: TaskRead` and `EpicCrud: EpicRead` — each CRUD trait splits into a read super-trait plus the mutating methods. `Database` implements both halves.
- `TaskReadStore: TaskRead + EpicRead + SettingsStore + LearningStore + LearningRetrievalStore + UsageStore`, and `TaskStore: … + TaskReadStore`, so a write-capable `Arc<dyn TaskStore>` upcasts to `Arc<dyn TaskReadStore>` for free at construction.
- Services keep their write handles (`TaskService` holds `Arc<dyn TaskAndEpicStore>`, `EpicService` holds the same), built from the still-write-capable `Arc<Database>` / `deps.db`.

Settings/learning/usage writes remain reachable through `TaskReadStore` on purpose: they carry no cross-entity invariant, so sealing them would add churn without protecting anything.

**Sanctioned direct-mutation consumers** (they manage their own invariants and hold a write-capable handle, exactly like the feed subsystem):

- `FeedRunner` (`src/feed/`) — holds its own `Arc<dyn TaskStore>` and calls `recalculate_epic_status` itself.
- `TuiRuntime::feed_db` — a write handle reserved for the manual `exec_trigger_epic_feed` path (the TUI's version of a feed tick).
- Startup / CLI paths (`runtime::bootstrap`, `src/setup/`, `src/main.rs`) — use a concrete `&Database` / `Arc<Database>` before the read-only narrowing applies.

  The sanction is a fallback for startup wiring, **not** a licence for CLI subcommands to skip the service. CLI handlers that mutate tasks route through `TaskService` like their siblings: `cmd_update` → `cli_update_task`, `cmd_hook` → `record_hook_event`, `cmd_pr_gate` → `mark_pr_learnings_gate_shown`, and `cmd_plan` → `attach_plan`. When adding a new `cmd_*` that writes a task/epic, add (or reuse) a `TaskService`/`EpicService` method rather than calling `Database::patch_task` on the concrete handle.

Tests seed fixtures via the `#[cfg(test)]` write accessors `McpState::db_write()` / `TuiRuntime::db_write()`, which are invisible to production handler code.

## `recalculate_epic_status` invariant

Any code that changes a task's **status** or its **epic linkage** (`epic_id`) must recalculate the affected epic(s). An epic's status is derived from its subtasks' statuses, so a task change that doesn't trigger a recalc leaves the parent epic showing a stale rollup.

The canonical implementation is in `TaskService` (`src/service/tasks/crud.rs` — `recalculate_epic` / `recalculate_epic_for_task`, which call `db.recalculate_epic_status(epic_id)`). Task mutations that go through the service layer get this for free; this is the main reason mutations should not bypass the service (see the mutation-boundary section above). When a task moves between epics, both the old and the new parent must be recalculated.

## DB access — `db_call` / `db_call_read`

`Database` (`src/db/mod.rs`) wraps a single writer [`tokio_rusqlite::Connection`] — a dedicated worker thread owning the underlying `rusqlite::Connection` — plus a small pool of up to 4 lazily-opened, read-only WAL connections. There is no sync handle or mutex; schema init and migrations run on the writer thread, mutations dispatch to the writer, and pure reads dispatch across the pool instead of queueing behind writer traffic.

- `Database::open(path).await` / `Database::open_in_memory().await` open the writer connection and run the migration chain on its worker thread. Pool connections are not opened here — each slot opens lazily on first use, so instances that never issue a concurrent read (most CLI subcommands, most tests) never pay for connections they don't need.
- `self.db_call(|conn| { … }).await` is the **writer** entry point. Use it for any closure that writes (`execute`/`execute_batch`/INSERT/UPDATE/DELETE), or that must read a connection-local counter like `get_total_changes` — that's a per-connection SQLite tally, so reading it from a pool connection would always return 0.
- `self.db_call_read(|conn| { … }).await` is the **read-pool** entry point, dispatched round-robin across the pool. Use it only for closures that issue no writes: pool connections are opened `SQLITE_OPEN_READ_ONLY`, so a write attempted through one fails loudly (`SQLITE_READONLY`) instead of silently succeeding or corrupting state. A write committed via `db_call` is immediately visible to a subsequent `db_call_read` — pool reads never see a stale snapshot.

Both closures receive a `&mut rusqlite::Connection`, must be `Send + 'static`, and return `Result<R>`. Errors are routed back through `tokio_rusqlite::Error::Other` and surfaced as `anyhow::Error`. Clone any borrowed `&str`/slice arguments to owned values before moving them into the closure.

**`db_call` is not a transaction, and "single writer" is per-process.** Neither entry point opens one — a closure issuing four statements runs them as four implicit transactions, and another writer can interleave between them. The single-writer connection serialises writes *within one `Database` instance*; it says nothing across processes, and dispatch routinely runs several at once (every Claude Code hook invokes its own `dispatch` CLI process, each opening the same file). So a multi-statement closure that must be atomic has to say so: open one explicitly with `conn.unchecked_transaction()`, do the work against the `tx`, and `tx.commit()`. See `src/db/queries/subagents.rs` for the read-modify-write shape (fence, mutate, recount, update) and `src/db/queries/tasks.rs` for two more. Getting this wrong is not hypothetical — a read-then-write pair split across two hook processes silently desynchronised a denormalised counter in task #3755.

Every `*Store` trait method is `async fn` and uses whichever entry point matches its access pattern — `db_call_read` for pure reads (`TaskRead`, `EpicRead`, `SettingsStore`, `LearningStore`, `LearningRetrievalStore`, `TodoStore`, `UsageStore`), `db_call` for anything that mutates. Callers `.await` each store call the same way regardless of which one it uses underneath.

## Inline-mutation boundary

Key handlers in `src/tui/input.rs` follow two different patterns:

- **Mutate inline, return `vec![]`** — for UI-only state with no side effects (cursor position, `input.mode`, selected index, text buffer). These changes don't need to be auditable and touching the DB/processes isn't required.
- **Return a `Command`** — for anything that needs a side effect: DB write, process spawn, network call, or waking the runtime.

The rule: if you're only changing what the screen looks like without touching external state, mutate inline. If the change needs to outlast the current render cycle or involve I/O, return a `Command`.

## Intentional `let _ =`

`let _ = expr` silences the `#[must_use]` warning on a result or value. The one sanctioned pattern is:

- **Fire-and-forget channel sends** — `let _ = tx.send(McpEvent::Refresh)` in `src/mcp/mod.rs`: the send can only fail if the receiver has dropped (TUI exited), which is fine to ignore

**Do not use it to discard a DB write's `Result`.** A second write that completes an entity (e.g. the follow-up `patch_epic` in `EpicService::create_epic` that applies `sort_order` / `feed_command` / `feed_interval_secs`) is part of the operation, not a "non-critical" extra: swallowing its error returns a success the caller can't trust. Propagate with `?`, and re-read (or otherwise refresh) so the returned entity reflects the write rather than the pre-patch insert result.

If you see `let _ =` and are unsure whether it's intentional, check the surrounding comment or commit message. Add a comment when adding a new one.

## `#[allow(dead_code)]`

Avoid `#[allow(dead_code)]` — dead code should be removed, not suppressed. If a type or function is unused today but is part of an in-progress feature, document it with a comment pointing at the relevant issue/task rather than silencing the warning.

## Prod-vs-test LOC split

Tests live inline behind `#[cfg(test)]` blocks (or in sibling `tests/` sub-modules) in the same file as the production code. Large files like `src/models/tasks.rs` (≈1700 LOC) are roughly half tests. If a file looks unexpectedly large, check how much of it is `#[cfg(test)]` before concluding the production code is complex.

## `unsafe`

Any `unsafe` block must have a `// SAFETY:` comment directly above it explaining why the invariant holds. Reviewer sign-off is required before merging. This policy is also stated in `CLAUDE.md`.

## Sub-status validation TOCTOU

`TaskService::update_task()` (`src/service/tasks/crud.rs`) reads the existing task to validate the requested sub-status before applying the patch. This is a TOCTOU window: a concurrent MCP call could change the task status between the read and the write. This is intentional and accepted — simultaneous status changes from two agents on the same task are considered a user error, and the window is too small to be worth a transaction-level fix.

## Reparenting an epic — three guards, no immutability

`parent_epic_id` **is** mutable: `EpicPatch` declares it `nullable` (`src/db/mod.rs:130`), `patch_epic` writes it (`src/db/queries/epics.rs:241`), `EpicService::update_epic` implements reparent-and-detach (`src/service/epics.rs:292`), and the TUI has a reparent picker (`src/tui/ui/kanban/popups/reparent_epic.rs`). Route reparenting through the service — it owns three guards a bare `patch_epic` skips:

1. **Cycle detection** — `check_no_cycle` (`src/service/epics.rs:313`) walks the proposed parent's ancestor chain and rejects with `ServiceError::Validation` if the epic being moved appears in it (self-parent included).
2. **RepoGroup guard** (`src/service/epics.rs:282`) — an auto-created `EpicOrigin::RepoGroup` sub-epic cannot be reparented *or* detached to root; either would orphan it outside its grouping root.
3. **DB `CHECK (parent_epic_id != id)`** (migration v35) — defence-in-depth against a row becoming its own parent, alongside the visited-set guard in `recalculate_epic_status_inner`.

`UpdateEpicParams.parent_epic_id` is an `Option<Option<EpicId>>`: `None` leaves the parent alone, `Some(Some(id))` reparents, `Some(None)` detaches to root.

## Clippy lint rules

Custom lint rules are configured in `[lints.clippy]` in `Cargo.toml`. The pre-push hook enforces them via `cargo clippy --all-targets -- -D warnings` — note there is **no** `--fix`; the hook checks, it does not rewrite your source. When you discover a pattern worth enforcing, add a new entry with a structured comment explaining why. Consult the `/lint` skill for the full workflow.

## Visibility convention

`App` fields use `pub(in crate::tui)` to restrict mutation to the TUI module. External code (runtime, MCP handlers) can only change `App` state by sending a `Message` through `app.update()`, which returns `Command`s. This keeps state transitions auditable in one place and prevents scattered mutation from outside the TUI boundary.

## Performance footguns

Two patterns have already caused bugs and must not be repeated:

- **`column_items_for_status` is test-only (compiler-enforced via `#[cfg(test)]`).** It calls `column_items_for_status_with_stats(status, None)`, which derives epic sort order by cloning subtasks on every invocation. In production render paths, always call `column_items_for_status_with_stats(status, Some(&stats))` with a pre-computed `EpicStatsMap` to avoid per-frame allocations.

- **No `std::fs` inside async handlers.** Blocking I/O on the async executor stalls the tokio thread pool. Any file-system operation inside an `async fn` must use `tokio::fs` or be wrapped in `tokio::task::spawn_blocking`.

  **Accepted exception:** `validate_repo_path` (`src/dispatch/worktree.rs`), called synchronously from `handle_submit_repo_path` (`src/tui/update/forms.rs`). It does only a `.exists()`/`.is_dir()` stat — no file content is read or parsed — on the low-frequency repo-path form-submit path (once per new task typed), unlike the `~/.claude.json` read-plus-`serde_json`-parse that motivated moving the `Space`-key trust check off the render thread (`CheckTrustAndDispatch` in `src/tui/commands/task.rs`). Keep it inline; don't route it through a `Command` unless it grows beyond a bare stat.

## `MockProcessRunner` vs a real tmux server

Two test styles cover tmux, and they prove different things. Picking the wrong one is not a style preference — it is how a broken command stays green.

- **`MockProcessRunner` proves *which command we sent*.** It records argv. Right for argv shape: flag ordering, target formatting, that an option is passed at all. The 100-plus inline tests in `src/tmux.rs` are this style and should stay.

- **A real tmux server proves *what tmux did with it*.** The only thing that catches wrong-pane, wrong-cwd, or wrong-pane-count bugs — because tmux resolves a loose target by **falling back**, not by failing. A `send-keys` with no `-t` inside `run-shell -bC` silently hits the session's active pane; a `%`-less pane target silently hits whatever index `pane-base-index` happens to make first. Both look like a well-formed command to a mock.

That gap is not hypothetical. #3781 was a `send-keys` target defect whose mock test pinned the broken string and passed. #3782 found three more of the same family (`pane_exists` blind, `pane_id_for_window` blind, swap by pane index). **If the assertion you want to write contains the words "which pane", "which cwd", or "how many panes", a mock cannot make it.**

Where to put a real-tmux test:

| Question | File |
|---|---|
| What topology and cwd does dispatch/resume/split actually build? | `tests/tmux_lifecycle.rs` — panes run the real shell with stub `claude`/`dispatch` binaries that record `argv`/`$PWD`/`$TMUX_PANE` |
| Which pane does a keystroke land in? | `tests/tmux_split_hook.rs` — every pane runs `cat > log`, so delivery is observable |
| Does a named target resolve to the window it names? | `tests/tmux_window_targets.rs` — colliding-prefix topology (`task-4` alongside `task-42`), so a prefix-matched target is observable as the wrong window being hit |

All three share the rig in `tests/tmux_harness/mod.rs`: a private `-L` socket, `-f /dev/null` so the developer's `~/.tmux.conf` can't change the result, and drop-guard teardown. Start from `tmux_available_or_skip()` — it skips locally when tmux is missing but **hard-fails under `CI`**, so a missing tmux in the workflow can never quietly report green. Waiting on pane output goes through `poll_for` (the sole sanctioned `// allow-test-sleep`, see below), never a fixed sleep.

### Writing a mock tmux test: `MockProcessRunner`'s window-lookup policy

Every `tmux::` helper that takes a window *name* first resolves it to a pane ID via `window_target` (`src/tmux.rs`) — an extra `list-panes` call, because tmux prefix-matches a bare `-t <name>`. `MockProcessRunner` therefore carries a `WindowLookup` policy (`src/process.rs:128`) deciding how that lookup is answered, and the default is **not** "from the response queue":

| Policy | How the lookup is answered | When to use |
|---|---|---|
| `AnyName` (default, from `MockProcessRunner::new`) | Out of band: any name resolves, pane IDs `%0`, `%1`, … in first-seen order. **Not** taken from the positional response queue and **not** recorded in `recorded_calls()`. | The default. The subject is the operation, not the resolution. |
| `with_windows(&[…])` | Also out of band, but only the declared names resolve — anything else fails as absent. Pair with `pane_id_of(name)` to assert the resolved `-t %N`. | The topology matters — above all a prefix collision: `with_windows(&["task-42"])` then an operation on `task-4`, which must fail rather than hit `task-42`. |
| `with_queued_window_lookup()` | From the positional queue, and recorded like any other call. (`pane_id_of` panics — there is no fake server to ask.) | The lookup itself is the subject, or no call at all is expected (`MockProcessRunner::unused`). |

The consequence that trips people up: under the two out-of-band policies the lookup consumes no queue entry and appears in no `recorded_calls()` index, so `calls[N]` numbers the operation's own calls only — don't budget a listing response per helper invocation. Flip to `with_queued_window_lookup()` and the indices shift, because the lookup now occupies one.

A mock still can't verify that a target *resolved* correctly — it records argv, not tmux's reading of it. Exact-name resolution is covered by the `window_target` unit tests in `src/tmux.rs` plus `tests/tmux_window_targets.rs` against a real server.

## No `tokio::time::sleep` in tests

Tests must never sleep on the wall clock to "wait for" background work or to cross a duration threshold. Wall-clock sleeps are flaky on slow CI (the work may not be done when the timer fires) and needlessly slow the suite. `./scripts/check-no-test-sleep.sh` enforces this in the pre-push hook, rejecting `tokio::time::sleep(` anywhere under `src/`/`tests/` **and** `std::thread::sleep(` in test files (anything under `tests/`, under a `src/**/tests/` directory, or named `tests.rs`). Production `std::thread::sleep` (e.g. `src/process.rs`, `src/runtime/mod.rs`) is unaffected. Inline `#[cfg(test)] mod tests` blocks inside production files are a blind spot of the grep-level check — keep sleeps out of them by review.

The `std::thread::sleep` check has one escape hatch, for the single shape a grep cannot tell apart from a fixed sleep: a short **poll step** inside a loop that polls a condition against a deadline, where only a genuine failure pays the deadline in full. Mark it with an `// allow-test-sleep: <why>` comment on the call line or the line directly above (`poll_for` in `tests/tmux_harness/mod.rs` is the only current use — it polls for output delivered through a real tmux pane's tty, where no in-process signal exists). The marker is not a way to keep a fixed sleep: if deleting the surrounding condition check would leave the test passing, it is a fixed sleep and must go.

Use whichever of these fits the thing you're waiting on:

- **An event the production code already emits.** The feed runner sends `McpEvent::EpicChanged` after each upsert, so feed tests await that instead of sleeping:

  ```rust
  let (mut runner, mut rx) = make_runner(db.clone());
  runner.tick().await;
  tokio::time::timeout(Duration::from_secs(5), rx.recv())
      .await
      .expect("timed out waiting for McpEvent")
      .expect("channel closed");
  ```

  The `timeout` is a safety net (the test fails if the signal never arrives), not a timing assumption — the test proceeds the instant the event lands.

- **A test-only completion signal for detached writes.** When production spawns fire-and-forget work with no observable signal (the MCP handler's usage + trajectory writes), add an optional sender that the spawn fires on completion — `McpState::test_hooks.bg_write_done_tx` / `BackgroundWrite`, installed via `router_with_bg_done` / `test_state_with_bg_done`. It is always `None` in production. Mirrors the existing optional `notify_tx` pattern.

- **An injected clock for time-dependent behaviour.** Hook-event timestamps persist at one-second resolution, so a test that needs two events in distinct seconds must not sleep ≥1s — inject `service::FixedClock` via `TaskService::with_clock` and `clock.advance(chrono::Duration::seconds(2))`. Production defaults to `SystemClock` (`Utc::now()`), so no call sites change.

- **An injected threshold when the behaviour under test is "did this take longer than X".** Don't sleep past the real threshold, and don't assume a trivial closure beats it either — a loaded CI box can push a no-op `db_call` past 200 ms, so asserting the *absence* of a slow-call warning is just as load-sensitive as asserting its presence. `Database::set_slow_call_threshold` (`#[cfg(test)]`, per-instance so parallel tests don't race) pins `SLOW_DB_CALL_THRESHOLD` for one `Database`: `Duration::ZERO` forces the warning, an hour forbids it. See `src/db/tests/async_handle.rs`.

## No phantom symbol references in docs

`./scripts/check-doc-symbols.sh` (pre-push) rejects a backticked snake_case identifier that occurs **nowhere in the code**. It covers `CLAUDE.md`, the topic files under `docs/`, `docs/specs/*.allium`, and the doc comments (`///`, `//!`) in `src/**/*.rs`. It exists because `check-doc-paths.sh` validates paths but never symbol names, which is how two phantom function names survived until #3806 removed them by hand.

It is a **phantom check, not a definition check** — it asks "does this identifier occur in the code at all", not "is this a function". Matching against `fn <name>` definitions drowns in false positives (struct fields, enum variants, config keys, test helpers). The accepted tradeoff is a false negative: a renamed symbol whose old name still occurs somewhere in the code passes.

Two properties are load-bearing and easy to break:

- **The identifier index is built from code only, with comments stripped.** Index raw file text and every phantom self-validates through its own doc comment. `tests/` counts as code (the docs cite helpers like `poll_for`), and Allium spec bodies are a second index source because specs declare their own namespace — `repo_group` is an `EpicOrigin` variant and `current_tmux_window()` is spec-level pseudocode, both correct references that resolve only there.
- **Matching is whole-word.** No substring fallback, so shorthand for a longer real name (`install_plugin` for `install_plugin_in`) is a finding: name the real identifier. <!-- allow-phantom-symbol: install_plugin is the illustrative bad example in this very sentence -->

Escape hatch, for deliberate references to removed code and to external-crate names: an `allow-phantom-symbol: <why>` comment on the offending line or the line directly above, mirroring `allow-test-sleep:`. In a Rust doc block the marker is a plain `//` line interleaved between `///` lines.

Two surfaces are deliberately unguarded. `docs/plans/`, `docs/superpowers/`, and `docs/research/` are dated artifacts that describe code as it stood then. And **bare (un-backticked) identifiers in Allium `--` comments are not scanned** — doing so would catch #3806's `dispatch.allium` phantom, but measured 37 hits for 1 real finding. A checker that cries wolf gets bypassed, so that one stays uncaught by design; see `docs/plans/3807-check-doc-symbols.md`.
