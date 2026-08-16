# TaskService owns its read handle (task #4217)

**Date**: 2026-08-16
**Status**: decided — implement

## The question

`TaskService::dispatch` (`src/service/tasks/dispatch.rs`) takes
`db: Arc<dyn db::TaskReadStore>` as a field on `DispatchRequest`. `TaskService`'s
own handle is `Arc<dyn db::TaskAndEpicStore>` (`TaskCrud + EpicCrud`), which is
*not* a `TaskReadStore` — that bundle adds `SettingsStore + LearningStore +
LearningRetrievalStore + UsageStore` on top of `TaskRead + EpicRead`. The
dispatch prologue (`dispatch::prepare_inputs`) needs the read bundle, so today
the caller supplies it.

The hazard: nothing ties the supplied `db` to the database the service actually
writes through. Passing a *different* handle is representable and would be
silently wrong — the prologue would read epic banners and learnings from one
database while the claim, the worktree write and the release ran against
another. All call sites pass the right one today; the type system does not say
they must.

## Decision

**Widen `TaskService.db` to `Arc<dyn db::TaskStore>` and delete
`DispatchRequest.db`.**

Rationale:

1. It makes the wrong state unrepresentable. There is exactly one handle, so
   "the prologue read a different database than the service wrote to" stops
   being expressible rather than being merely unexercised.
2. **It does not actually push against the "DB trait narrowing" convention — it
   satisfies it.** The convention is *hold the narrowest sub-trait you actually
   call*. `TaskService` calls `TaskCrud + EpicCrud` (its own writes) **and**, via
   `dispatch`, the whole `TaskReadStore` read bundle. `TaskAndEpicStore +
   TaskReadStore` is, by the bounds in `src/db/mod.rs`, exactly `TaskStore`. So
   `TaskStore` *is* the narrowest bundle that covers what `TaskService` calls.
   The narrow `TaskAndEpicStore` field was accurate before `dispatch` moved into
   the service (task #4209) and is now an understatement, patched over by a
   parameter.
3. Precedent: `LearningService` already holds `Arc<dyn db::TaskStore>`.
4. It is cheap. Of the ~21 `TaskService::new` call sites, production already
   passes `Arc<Database>` or `Arc<dyn db::TaskStore>`; only a handful of test
   sites carry an explicit `as Arc<dyn db::TaskAndEpicStore>` cast or a
   `TaskAndEpicStore`-typed local.

**`emb_svc` stays a per-call parameter.** `EmbeddingService::new_noop()` spawns
an OS thread; making it a constructor argument would spin one up in ~20 test
fixtures that never dispatch. `DispatchRequest`'s doc comment currently
justifies `db` and `emb_svc` together and must be reworded to justify `emb_svc`
alone.

**Not changed**: `EpicService` keeps `Arc<dyn TaskAndEpicStore>` — it calls
nothing outside `TaskCrud + EpicCrud`, so the narrow handle is still the
narrowest true one for it.

## Spec impact

None. This is a wiring/typing change: no domain behaviour in `docs/specs/`
changes — the same reads happen against the same database, from the same
sequence, with the same outcomes. `docs/conventions.md` is prose, not spec, and
does change (steps 4–5 below).

## Plan

### 1. Test first (red)

In `src/service/tasks/tests.rs`, module `dispatch_seam`, add:

`dispatch_reads_the_epic_banner_from_the_services_own_handle` — create an epic
and a task inside it in the test DB, build the `TaskService` on that same DB,
dispatch with `epic_ctx: None` (so the prologue must read the epic itself) and
assert the recorded runner calls contain the epic title. Constructed **without**
a `db` field on `DispatchRequest`, so before step 2 this is a compile error:
the red state.

This is the behavioural claim the change buys: the prologue's reads come from
the service's own handle, with no caller-supplied handle involved.

### 2. Widen the field (green)

- `src/service/tasks/crud.rs`: `TaskService.db: Arc<dyn db::TaskStore>`;
  `TaskService::new` and `new_with_real_runner` take the same. Extend the
  doc comment on the field/constructor with *why* it is the wide bundle
  (the dispatch prologue's read surface).
- `src/service/tasks/dispatch.rs`: delete `DispatchRequest.db`, use `&*self.db`
  in the two `prepare_inputs*` calls, and reword the struct doc comment so it
  justifies `emb_svc` only.

### 3. Fix call sites

- `src/mcp/handlers/tasks/dispatch.rs` — drop `db: state.db.clone()` from both
  `DispatchRequest` literals (2 sites).
- Test/typing sites that hand `TaskService::new` a narrowed handle:
  `src/service/tasks/tests.rs` (the `task_svc_with_runner` local and the
  ~line-4157 cast, plus the `request` helper), `src/service/api.rs::store`,
  `tests/task_watchers.rs`, `tests/active_health.rs`. Widen each to
  `TaskStore`; let the compiler find any others.

### 4. Update `docs/conventions.md`

- The DB-trait-narrowing table: `TaskService` → `Arc<dyn TaskStore>`, with a
  one-line why (writes plus the dispatch prologue's read bundle). `EpicService`
  row unchanged.
- The mutation-boundary bullet at ~line 244 ("Services keep their write
  handles…") — `TaskService` now holds `TaskStore`; `EpicService` holds
  `TaskAndEpicStore`.
- The "trait bundles do not nest" note at ~line 298 ends with "this is what
  makes `TaskService::dispatch` take the read handle as a parameter rather than
  reuse its own". That justification is being removed; the note's *substance*
  (the bundles genuinely do not nest — check the bounds before assuming) stays,
  with the closing clause rewritten to say `TaskService` therefore holds
  `TaskStore`, the only bundle covering both halves.

### 5. Verify

`cargo test` green (with `tmux` on `PATH`), then `cargo fmt` + `cargo clippy
--all-targets -- -D warnings`, plus the doc-path/doc-symbol checkers the
pre-push hook runs — step 4 edits cited symbols.
