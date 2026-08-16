# Verify-Command Tiering (Full vs. Cheap) — Implementation Plan

> **For agentic workers:** Use TDD throughout — write the failing test before the code that makes it pass. Update `docs/specs/core.allium` (`SavedRepoPath`), `docs/specs/dispatch.allium`/`docs/specs/mcp-task-tools.allium` (the two agent-facing surfaces), and `docs/specs/pr-workflow.allium` (wrap_up's verify-line surface) via `allium:tend`, then verify with `allium:weed`.

**Goal:** Add `SavedRepoPath.full_verify_command`, a second, independent per-repo command alongside the existing `verify_command` — the existing one stays the cheap, per-task check; the new one is the exhaustive suite only a pipeline/scheduled agent runs.

**Architecture:** This is a close, mechanical mirror of the existing `verify_command` implementation at every one of its 6 surfaces (DB column + migration, `set_verify_command`-shaped MCP tool, CLI `repo set-verify`-shaped subcommand, `get_task`'s prompt line, `wrap_up`'s reminder line, the shared `fetch_verify_command`-shaped helper) — no new mechanism, just a second instance of an existing one, added everywhere the first one already lives.

**Tech Stack:** Rust, rusqlite, existing MCP tool-registration/CLI-subcommand patterns.

**Spec:** `docs/superpowers/specs/2026-08-16-staging-pipeline-scheduled-agents-design.md` ("Verify-command tiering" section)

## Global Constraints

- `verify_command` itself (the existing cheap-tier field, and every function/tool/CLI form touching it) must NOT be modified — this plan only adds a second, independent field and its own surfaces.
- Same no-newline/no-carriage-return validation as `verify_command`, enforced at both the DB layer (`anyhow::bail!`) and the MCP-handler layer (so an agent's bad input gets JSON-RPC `-32602`, not `-32603` — mirror `src/mcp/handlers/tasks/verify.rs:19-29`'s explicit two-layer rationale exactly).
- One tool clears-by-omission, same as today's `set_verify_command` (no separate `clear_full_verify_command` tool) — passing `command: None` clears it.
- Reuse the exact one-shared-fetch-helper pattern already documented at `src/dispatch/agents.rs:467-484` (`fetch_verify_command`'s doc comment explicitly says it exists for exactly two callers and deliberately has no prompt-path caller) — add a `fetch_full_verify_command` sibling, not inline duplication.

---

## File Structure

- Modify `src/db/mod.rs:1187` area — `full_verify_command TEXT` column on `repo_paths`.
- Modify `src/db/migrations.rs` — `migrate_v88_add_scheduling_fields` already claims v88 in the sibling scheduling-primitive plan; this plan's migration is independent and should be v89 if that plan lands first, or coordinate numbering if both land in the same window (check `MIGRATIONS`'s latest entry at execution time — do not hardcode "v89" blindly, derive it from whatever the last registered version actually is when this task starts).
- Modify `src/db/queries/settings.rs:204-221` — new `set_full_verify_command` DB function, mirroring `set_verify_command` exactly (same trim/validate/clear-on-empty logic).
- Modify `src/mcp/handlers/tasks/verify.rs` — new `handle_set_full_verify_command`, mirroring `handle_set_verify_command` (lines 8-44) exactly.
- Modify `src/mcp/handlers/dispatch.rs:514` — register `"set_full_verify_command" => tasks::handle_set_full_verify_command` in the tool dispatch table.
- Modify `src/main.rs:718-749` — `RepoAction` gains `SetFullVerify { path, command }` / `ClearFullVerify { path }` variants, mirroring `SetVerify`/`ClearVerify` (lines 721-730); `RepoAction::List` (731-743) prints the new field alongside the existing `verify: <cmd>` line.
- Modify `src/mcp/handlers/tasks/mod.rs:201-243` (`format_task_detail`) — a second `if let Some(cmd) = full_verify_command { ... }` line, "Full verify command: {cmd}".
- Modify `src/mcp/handlers/tasks/crud.rs:197-201` — `tokio::join!` picks up a second `dispatch::fetch_full_verify_command(...)` call, passed into `format_task_detail`.
- Modify `src/dispatch/agents.rs:476-484` — new `fetch_full_verify_command`, mirroring `fetch_verify_command` exactly (same doc-comment convention explaining the two callers and the deliberate absence of a prompt-path caller).
- Modify `src/mcp/handlers/tasks/wrap_up.rs:38-56` (`wrap_up_verify_line`) — this function currently builds one line from `verify_command`; extend it to also incorporate `full_verify_command` **only for a pinned-branch/scheduled task** (a normal task's wrap-up reminder is unchanged — full_verify_command is irrelevant to it). Read the function's actual current signature before deciding whether this is a second parameter or a second call site; do not guess.
- Test: `src/db/tests/migrations.rs`, `src/mcp/handlers/tests/tasks/verify.rs` (or wherever `set_verify_command`'s handler tests live), `tests/cli.rs` (CLI subcommand tests, mirroring existing `repo set-verify`/`clear-verify` coverage), `src/mcp/handlers/tests/tasks/crud.rs` or wherever `get_task`'s formatted output is tested.

---

## Task 1: DB column + migration

**Files:**
- Modify: `src/db/mod.rs` (schema), `src/db/migrations.rs`.
- Test: `src/db/tests/migrations.rs`.

- [ ] **Step 1: Write the failing migration test**, mirroring `migration_v52_adds_verify_command_to_repo_paths` exactly, substituting `full_verify_command`.

```rust
#[test]
fn migration_vNN_adds_full_verify_command_to_repo_paths() {
    let conn = seed_schema_before(/* whatever the current latest version is at execution time */);
    migrate_vNN_add_full_verify_command_to_repo_paths(&conn).expect("migration should succeed");
    let mut stmt = conn.prepare("SELECT full_verify_command FROM repo_paths LIMIT 0").unwrap();
    assert!(stmt.column_names().contains(&"full_verify_command"));
}
```

- [ ] **Step 2: Run test, confirm it fails.**

- [ ] **Step 3: Implement**, following `migrate_v52_add_verify_command_to_repo_paths`'s exact shape (a plain additive `ALTER TABLE repo_paths ADD COLUMN full_verify_command TEXT`, guarded by the same `column_exists` check the v52 migration uses). Register in `MIGRATIONS` at the next free version number (check `LATEST_SCHEMA_VERSION`/the last `MIGRATIONS` entry at the time this task actually runs — do not hardcode a version number that may already be taken by a sibling plan's migration).

- [ ] **Step 4: Run test, confirm it passes.**

- [ ] **Step 5: Commit**

```bash
git add src/db/mod.rs src/db/migrations.rs src/db/tests/migrations.rs
git commit -m "feat(db): add full_verify_command column to repo_paths"
```

---

## Task 2: `set_full_verify_command` DB function + MCP tool

**Files:**
- Modify: `src/db/queries/settings.rs` (new function mirroring `set_verify_command`, lines 204-221).
- Modify: `src/mcp/handlers/tasks/verify.rs` (new handler mirroring `handle_set_verify_command`, lines 8-44).
- Modify: `src/mcp/handlers/dispatch.rs:514` (tool registration).
- Test: mirror whatever test file covers `set_verify_command`'s handler today.

**Interfaces:**
- Consumes: the `full_verify_command` column (Task 1).
- Produces: `set_full_verify_command(repo_path: &str, command: Option<&str>) -> Result<()>` (DB layer); MCP tool `set_full_verify_command` with the same `{ repo_path, command: Option<String> }` input shape as `set_verify_command`.

- [ ] **Step 1: Write the failing DB-layer test** — same no-newline-rejection and clear-on-None/empty behavior as `set_verify_command`'s existing tests, substituting the new function/column.

- [ ] **Step 2: Run test, confirm it fails** (function doesn't exist).

- [ ] **Step 3: Implement `set_full_verify_command`**, copying `set_verify_command`'s body (`settings.rs:204-221`) verbatim except for the column name — same `\n`/`\r` `anyhow::bail!`, same trim-and-treat-blank-as-clear behavior.

- [ ] **Step 4: Run test, confirm it passes.**

- [ ] **Step 5: Write the failing MCP-handler test** — mirroring whatever test exists for `handle_set_verify_command` (a newline in `command` should produce a `-32602` JSON-RPC error, not `-32603`; a successful call should return the same style of confirmation message `set_verify_command` returns, substituting "Full verify command").

- [ ] **Step 6: Run test, confirm it fails.**

- [ ] **Step 7: Implement `handle_set_full_verify_command`**, copying `handle_set_verify_command`'s body (`verify.rs:8-44`) verbatim except for the field/function names and the confirmation message text ("Full verify command set for..." / "Full verify command cleared for...").

- [ ] **Step 8: Register the tool** in `src/mcp/handlers/dispatch.rs:514`'s dispatch table: `"set_full_verify_command" => tasks::handle_set_full_verify_command`. Add its JSON schema entry alongside `set_verify_command`'s (same input shape).

- [ ] **Step 9: Run test, confirm it passes.**

- [ ] **Step 10: Commit**

```bash
git add src/db/queries/settings.rs src/mcp/handlers/tasks/verify.rs src/mcp/handlers/dispatch.rs
git commit -m "feat(mcp): add set_full_verify_command tool"
```

---

## Task 3: CLI `repo set-verify-full` / `clear-verify-full`

**Files:**
- Modify: `src/main.rs:718-749` (`RepoAction` enum, `cmd_repo` match).
- Test: `tests/cli.rs`.

**Interfaces:**
- Consumes: `set_full_verify_command` DB function (Task 2).
- Produces: `dispatch repo set-verify-full <path> <command>`, `dispatch repo clear-verify-full <path>`; `RepoAction::List` prints a second `full_verify: <cmd>` line per repo.

- [ ] **Step 1: Write the failing CLI test**, mirroring existing `repo set-verify`/`clear-verify` CLI tests in `tests/cli.rs`.

- [ ] **Step 2: Run test, confirm it fails.**

- [ ] **Step 3: Implement** — add `RepoAction::SetFullVerify { path: String, command: String }` and `RepoAction::ClearFullVerify { path: String }` (mirroring `SetVerify`/`ClearVerify` at `main.rs:721-730` exactly, including the `expand_tilde` call), and extend `RepoAction::List`'s print loop (`main.rs:731-743`) to also print `full_verify: <cmd>` (or nothing if unset, matching how the existing `verify:` line handles the unset case).

- [ ] **Step 4: Run test, confirm it passes.**

- [ ] **Step 5: Commit**

```bash
git add src/main.rs tests/cli.rs
git commit -m "feat(cli): add repo set-verify-full/clear-verify-full subcommands"
```

---

## Task 4: Surface to the agent — `get_task` and `wrap_up`

**Files:**
- Modify: `src/mcp/handlers/tasks/mod.rs:201-243` (`format_task_detail`), `src/mcp/handlers/tasks/crud.rs:197-201`.
- Modify: `src/dispatch/agents.rs:476-484` (new `fetch_full_verify_command`).
- Modify: `src/mcp/handlers/tasks/wrap_up.rs:38-56` (`wrap_up_verify_line`).
- Test: `get_task` formatted-output tests (wherever `format_task_detail`'s "Verify command" line is asserted today), `wrap_up` response-text tests.

**Interfaces:**
- Consumes: `fetch_verify_command`'s exact existing signature (`agents.rs:476-484`) as the template.
- Produces: `pub async fn fetch_full_verify_command(db: &dyn crate::db::TaskReadStore, repo_path: &str) -> Option<String>`.

- [ ] **Step 1: Write the failing `get_task` test** — a repo with `full_verify_command` set should make `get_task`'s response include a "Full verify command: {cmd}" line, alongside (not replacing) the existing "Verify command: {cmd}" line when both are set; a repo with only `full_verify_command` unset should show no such line (mirroring `verify_command`'s existing "no line when unset" behavior).

- [ ] **Step 2: Run test, confirm it fails.**

- [ ] **Step 3: Implement.** Add `fetch_full_verify_command` to `agents.rs`, copying `fetch_verify_command`'s doc comment and body (`:467-484`) verbatim except for the column/function name — keep the same "no caller on the prompt-building path" framing in the doc comment, since this field must never leak into the dispatch prompt either (per the design doc's explicit "not in the prompt, surfaced via get_task/wrap_up only" decision). Add the second line to `format_task_detail` (`mod.rs:241-243`):

```rust
if let Some(cmd) = full_verify_command {
    text.push_str(&format!("\nFull verify command: {cmd}"));
}
```

Update `format_task_detail`'s signature to accept the new `Option<&str>` parameter, and update its one caller (`crud.rs:197-201`) to `tokio::join!` a second `dispatch::fetch_full_verify_command(&*state.db, &task.repo_path)` call and pass it through.

- [ ] **Step 4: Run test, confirm it passes.**

- [ ] **Step 5: Write the failing `wrap_up` reminder test** — for a task with `pinned_branch` set (the pipeline case) and `full_verify_command` configured, `wrap_up`'s response should mention the full verify command specifically, not just the cheap one. For an ordinary task (no `pinned_branch`), the reminder is unchanged from today (cheap `verify_command` only) — `full_verify_command` must not appear in an ordinary task's wrap-up response even if the repo has one configured, since it's irrelevant to that task's own work.

- [ ] **Step 6: Run test, confirm it fails.**

- [ ] **Step 7: Implement.** Read `wrap_up_verify_line`'s actual current signature and call site first (it currently takes `verify_command: Option<&str>` and the `WrapUpAction`/`base_branch` needed for wording — confirm exact params). Extend it to also accept `full_verify_command: Option<&str>` and `is_pinned_branch_task: bool` (or thread `task.pinned_branch.is_some()` directly), and when true and `full_verify_command` is set, append a second sentence naming it specifically. Update the one call site in `handle_wrap_up`/`issue_wrap_up_token` (`wrap_up.rs:62-72`) to fetch and pass the new value via `fetch_full_verify_command`.

- [ ] **Step 8: Run test, confirm it passes.**

- [ ] **Step 9: Commit**

```bash
git add src/mcp/handlers/tasks/mod.rs src/mcp/handlers/tasks/crud.rs src/dispatch/agents.rs src/mcp/handlers/tasks/wrap_up.rs
git commit -m "feat(mcp): surface full_verify_command via get_task and wrap_up for pinned-branch tasks"
```

---

## Task 5: `docs/specs` alignment

- [ ] Use `allium:tend` to add `full_verify_command` to `core.allium`'s `SavedRepoPath` entity (same no-newline invariant as `verify_command`), document the new MCP tool/CLI forms in `mcp-task-tools.allium`, and update `dispatch.allium`'s prompt-skeleton guidance + `pr-workflow.allium`'s wrap-up verify-line guidance to mention the second, pinned-branch-only surfacing rule.
- [ ] Run `allium:weed` to confirm alignment; fix any drift found.
- [ ] Commit spec changes separately (`docs: add full_verify_command to core.allium/mcp-task-tools.allium/pr-workflow.allium`).

---

## Self-Review Notes

- Migration version number: this plan's migration and the sibling scheduling-primitive plan's migration (v88) are independent and may land in either order. Whichever lands second must check `MIGRATIONS`'s actual latest entry at execution time rather than assuming a specific number — do not hardcode "v89" if v88 hasn't landed yet when this task starts.
- Task 4's "only for a pinned-branch task" rule in `wrap_up_verify_line` is the one place this plan makes a judgment call beyond pure mirroring — confirm this matches the design doc's intent (full_verify_command is pipeline-specific, not a second check every ordinary task should also be reminded about) before implementing; if `wrap_up_verify_line`'s actual current code structure makes a cleaner design apparent once read directly, prefer that over forcing the shape suggested here.
