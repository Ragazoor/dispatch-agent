# Remove Cost Calculations Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** Remove the `cost_usd` field from all layers of the codebase (Allium specs, models, DB, MCP handler, TUI, tests) since cost calculations are unused.

**Architecture:** The `cost_usd` field lives in the `TaskUsage` entity and flows through: Allium spec → Rust models → SQLite DB → MCP handler → TUI state. It is stored but never rendered. We remove it at all layers and add a DB migration (v41) to drop the column from existing databases.

**Tech Stack:** Rust, rusqlite (SQLite), ratatui TUI, Axum MCP server.

---

## Pre-work

- [ ] **Rebase from main**
  ```bash
  git rebase main
  ```

---

## Task 1: Save plan and claim task

**Files:**
- Create: `docs/plans/388-remove-cost-calculations.md`

- [ ] **Step 1: Save the implementation plan**

  Copy this plan to `docs/plans/388-remove-cost-calculations.md`.

- [ ] **Step 2: Attach plan and claim task via MCP**

  Use `mcp__dispatch__update_task` with `task_id: 388` and set `plan_path` to `docs/plans/388-remove-cost-calculations.md`.
  Use `mcp__dispatch__claim_task` with `task_id: 388`.

---

## Task 2: Update Allium specs

**Files:**
- Modify: `docs/specs/core.allium` — remove `cost_usd` from `TaskUsage` entity and its guidance
- Modify: `docs/specs/tasks.allium` — remove `cost_usd` from `ReportUsageViaMcp` rule

- [ ] **Step 1: Remove `cost_usd` from `TaskUsage` in core.allium**

  In `docs/specs/core.allium`, find the `TaskUsage` entity (around line 185) and remove the `cost_usd: Float` field and update the `@guidance` comment to remove the reference to `cost_usd`:

  ```
  entity TaskUsage {
      task: Task
      input_tokens: Integer
      output_tokens: Integer
      cache_read_tokens: Integer
      cache_write_tokens: Integer
      updated_at: Timestamp

      @guidance
          -- Uses accumulation semantics: one row per task, upserted on each
          -- report. Numeric fields (input_tokens, output_tokens,
          -- cache_read_tokens, cache_write_tokens) are summed with the
          -- existing values rather than replaced. updated_at reflects the
          -- time of the most recent report.
  }
  ```

- [ ] **Step 2: Remove `cost_usd` from `ReportUsageViaMcp` rule in tasks.allium**

  In `docs/specs/tasks.allium`, around line 1191, find the `ReportUsageViaMcp` rule and remove `cost_usd` from the trigger signature and the `accumulate()` call:

  ```
  rule ReportUsageViaMcp {
      when: McpReportUsage(task, input_tokens, output_tokens, cache_read_tokens?, cache_write_tokens?)

      ensures: core/TaskUsage.accumulate(
          task: task,
          input_tokens: input_tokens,
          output_tokens: output_tokens,
          cache_read_tokens: cache_read_tokens ?? 0,
          cache_write_tokens: cache_write_tokens ?? 0,
          updated_at: now
      )

      @guidance
          -- Upserts a single TaskUsage row for the given task. If a row
          -- already exists, numeric fields are summed with existing values.
          -- cache_read_tokens and cache_write_tokens default to zero when
          -- omitted by the caller.
  }
  ```

- [ ] **Step 3: Verify specs with allium check**

  ```bash
  allium check docs/specs/core.allium
  allium check docs/specs/tasks.allium
  ```

  Expected: No errors.

- [ ] **Step 4: Commit**

  ```bash
  git add docs/specs/core.allium docs/specs/tasks.allium
  git commit -m "spec: remove cost_usd from TaskUsage and ReportUsageViaMcp"
  ```

---

## Task 3: Update tests to not reference `cost_usd` (TDD: red phase)

Before removing the implementation, update all tests that reference `cost_usd`. The goal is to have tests that pass once `cost_usd` is gone.

**Files:**
- Modify: `src/db/tests.rs`
- Modify: `src/mcp/handlers/tests.rs`
- Modify: `src/tui/tests/navigation.rs`

- [ ] **Step 1: Update DB tests in `src/db/tests.rs`**

  Find the `report_usage_first_insert` test (~line 1621). Remove `cost_usd` from `UsageReport` construction and from the assertion:

  ```rust
  let report = UsageReport {
      input_tokens: 100,
      output_tokens: 50,
      cache_read_tokens: 10,
      cache_write_tokens: 5,
  };
  ```

  And in the assertion after `get_all_usage()`:
  ```rust
  assert_eq!(usage[0].input_tokens, 100);
  assert_eq!(usage[0].output_tokens, 50);
  assert_eq!(usage[0].cache_read_tokens, 10);
  assert_eq!(usage[0].cache_write_tokens, 5);
  // Remove: assert_eq!(usage[0].cost_usd, 0.42);
  ```

  Find the `report_usage_accumulates` test (~line 1659). Remove `cost_usd` from both `UsageReport` calls and from the accumulation assertion:

  ```rust
  let first = UsageReport { input_tokens: 100, output_tokens: 50, cache_read_tokens: 0, cache_write_tokens: 0 };
  let second = UsageReport { input_tokens: 200, output_tokens: 75, cache_read_tokens: 0, cache_write_tokens: 0 };
  // Remove cost_usd assertions, keep token assertions
  ```

  Also find the schema test around line 2181/2457 that includes the `task_usage` table DDL and remove `cost_usd REAL NOT NULL DEFAULT 0.0,` from those strings.

- [ ] **Step 2: Update MCP handler tests in `src/mcp/handlers/tests.rs`**

  Find `report_usage_stores_and_accumulates` (~line 1125). Remove `cost_usd` from the JSON body of both MCP calls and from assertions:

  ```rust
  // First call: remove "cost_usd": 0.10 from JSON
  // Second call: remove "cost_usd": 0.05 from JSON
  // Remove assertion: assert_eq!(usage[0].cost_usd, 0.15);
  ```

  Find the schema validation test (~line 1337). It has three `cost_usd` references to remove:
  - The field name `"cost_usd"` in the BTreeSet of property names (~line 1341)
  - `"cost_usd"` in the required fields BTreeSet (~line 1347, appears twice on the line)
  - `"cost_usd": 0.42` in the JSON fixture (~line 1348)

  Remove all three references so `cost_usd` is absent from both the expected schema and the test payload.

- [ ] **Step 3: Update TUI tests in `src/tui/tests/navigation.rs`**

  Find the `RefreshUsage` test (~line 1060). Remove `cost_usd: 0.42` from the `TaskUsage` construction:

  ```rust
  let usage = TaskUsage {
      task_id,
      input_tokens: 100,
      output_tokens: 50,
      cache_read_tokens: 0,
      cache_write_tokens: 0,
      updated_at: chrono::Utc::now(),
  };
  ```

- [ ] **Step 4: Run tests and expect compile errors (red)**

  ```bash
  cargo test 2>&1 | head -50
  ```

  Expected: Compile errors about missing `cost_usd` field, or unknown field. This confirms the tests are now written for the target state.

---

## Task 4: Remove `cost_usd` from models

**Files:**
- Modify: `src/models.rs` (lines 1255–1273)

- [ ] **Step 1: Remove `cost_usd` from `UsageReport` and `TaskUsage`**

  In `src/models.rs`, find `UsageReport` (~line 1255) and `TaskUsage` (~line 1265):

  ```rust
  pub struct UsageReport {
      pub input_tokens: i64,
      pub output_tokens: i64,
      pub cache_read_tokens: i64,
      pub cache_write_tokens: i64,
  }

  pub struct TaskUsage {
      pub task_id: TaskId,
      pub input_tokens: i64,
      pub output_tokens: i64,
      pub cache_read_tokens: i64,
      pub cache_write_tokens: i64,
      pub updated_at: chrono::DateTime<chrono::Utc>,
  }
  ```

- [ ] **Step 2: Verify compile**

  ```bash
  cargo build 2>&1 | head -50
  ```

  Expected: Errors pointing to remaining `cost_usd` usages (in queries, handlers). This is expected — work through Tasks 5–7 to fix them.

---

## Task 5: Add DB migration v41 to drop `cost_usd` column

**Files:**
- Modify: `src/db/migrations.rs`
- Modify: `src/db/tests.rs` — update schema version assertion

- [ ] **Step 1: Add migration function**

  In `src/db/migrations.rs`, add after the last migration function:

  ```rust
  fn migrate_v41_drop_cost_usd(conn: &Connection) -> Result<()> {
      conn.execute_batch(
          "CREATE TABLE task_usage_new (
              task_id     INTEGER NOT NULL PRIMARY KEY REFERENCES tasks(id),
              input_tokens       INTEGER NOT NULL DEFAULT 0,
              output_tokens      INTEGER NOT NULL DEFAULT 0,
              cache_read_tokens  INTEGER NOT NULL DEFAULT 0,
              cache_write_tokens INTEGER NOT NULL DEFAULT 0,
              updated_at         TEXT NOT NULL DEFAULT ''
          );
          INSERT INTO task_usage_new
              SELECT task_id, input_tokens, output_tokens, cache_read_tokens, cache_write_tokens, updated_at
              FROM task_usage;
          DROP TABLE task_usage;
          ALTER TABLE task_usage_new RENAME TO task_usage;",
      )?;
      Ok(())
  }
  ```

- [ ] **Step 2: Register the migration in the MIGRATIONS array**

  In `src/db/migrations.rs`, add to the `MIGRATIONS` array:

  ```rust
  (41, migrate_v41_drop_cost_usd),
  ```

- [ ] **Step 3: Update schema version assertion in tests**

  In `src/db/tests.rs`, find `assert_eq!(version, 40)` in `fresh_db_has_latest_schema_version` and update to:

  ```rust
  assert_eq!(version, 41);
  ```

  Also update the `task_usage` DDL string in the schema tests (lines ~2181 and ~2457) to remove the `cost_usd REAL NOT NULL DEFAULT 0.0,` line from both.

- [ ] **Step 4: Add migration test for v41**

  In `src/db/tests.rs`, add a new test following the pattern of existing migration tests (e.g. `migration_v40_creates_learnings_table`):

  ```rust
  #[test]
  fn migration_v41_drops_cost_usd_column() {
      let db = Database::open_in_memory().unwrap();
      let conn = db.conn().unwrap();
      // Set up schema at v40 (task_usage with cost_usd)
      conn.execute_batch("PRAGMA user_version = 40;").unwrap();
      // Insert a task_usage row via raw SQL (cost_usd column still exists at v40)
      // First ensure tasks table exists with a row
      // Then insert usage row including cost_usd
      conn.execute_batch(
          "INSERT OR IGNORE INTO tasks (id, title, status, sort_order, created_at, updated_at)
           VALUES (999, 'test', 'backlog', 0, '', '');
           INSERT INTO task_usage (task_id, cost_usd, input_tokens, output_tokens,
               cache_read_tokens, cache_write_tokens, updated_at)
           VALUES (999, 0.42, 100, 50, 10, 5, '');",
      ).unwrap();
      drop(conn);
      // Run pending migrations (will run v41)
      db.init_schema().unwrap();
      let conn = db.conn().unwrap();
      let version: i64 = conn
          .pragma_query_value(None, "user_version", |r| r.get(0))
          .unwrap();
      assert_eq!(version, 41);
      // Verify cost_usd column is gone and data was preserved
      let row: (i64, i64, i64, i64, i64) = conn
          .query_row(
              "SELECT task_id, input_tokens, output_tokens, cache_read_tokens, cache_write_tokens
               FROM task_usage WHERE task_id = 999",
              [],
              |r| Ok((r.get(0)?, r.get(1)?, r.get(2)?, r.get(3)?, r.get(4)?)),
          )
          .unwrap();
      assert_eq!(row, (999, 100, 50, 10, 5));
      // Verify cost_usd column no longer exists
      let has_cost_usd: bool = conn
          .query_row(
              "SELECT COUNT(*) FROM pragma_table_info('task_usage') WHERE name = 'cost_usd'",
              [],
              |r| r.get::<_, i64>(0),
          )
          .map(|n| n > 0)
          .unwrap_or(false);
      assert!(!has_cost_usd, "cost_usd column should have been removed");
  }
  ```

---

## Task 6: Remove `cost_usd` from DB query layer

**Files:**
- Modify: `src/db/queries.rs` (lines 239–297)

- [ ] **Step 1: Update `report_usage()` INSERT query**

  In `src/db/queries.rs`, find the `report_usage()` method (~line 239). Remove `cost_usd` from the INSERT and ON CONFLICT clauses:

  ```rust
  conn.execute(
      "INSERT INTO task_usage (task_id, input_tokens, output_tokens, cache_read_tokens, cache_write_tokens, updated_at)
       VALUES (?1, ?2, ?3, ?4, ?5, ?6)
       ON CONFLICT(task_id) DO UPDATE SET
           input_tokens = input_tokens + excluded.input_tokens,
           output_tokens = output_tokens + excluded.output_tokens,
           cache_read_tokens = cache_read_tokens + excluded.cache_read_tokens,
           cache_write_tokens = cache_write_tokens + excluded.cache_write_tokens,
           updated_at = excluded.updated_at",
      params![
          task_id,
          usage.input_tokens,
          usage.output_tokens,
          usage.cache_read_tokens,
          usage.cache_write_tokens,
          now,
      ],
  )?;
  ```

- [ ] **Step 2: Update `get_all_usage()` SELECT query**

  In `src/db/queries.rs`, find the `get_all_usage()` method (~line 270). Remove `cost_usd` from the SELECT and from the row mapping:

  ```rust
  let sql = "SELECT task_id, input_tokens, output_tokens, cache_read_tokens, cache_write_tokens, updated_at FROM task_usage";
  // In the row mapping, remove: cost_usd: row.get(...)
  ```

---

## Task 7: Remove `cost_usd` from MCP handler

**Files:**
- Modify: `src/mcp/handlers/dispatch.rs` (lines 340–352)
- Modify: `src/mcp/handlers/tasks.rs` (lines 85, 1056–1080)

- [ ] **Step 1: Remove `cost_usd` from tool schema in `dispatch.rs`**

  In `src/mcp/handlers/dispatch.rs`, find the `report_usage` tool definition (~line 340). Remove `cost_usd` from the JSON schema `properties` and `required` arrays:

  ```rust
  // In properties: remove the "cost_usd" entry
  // In required: remove "cost_usd"
  // Update description: remove "and cost" from "Report token usage and cost for a task session."
  ```

- [ ] **Step 2: Remove `cost_usd` from `ReportUsageArgs` and handler in `tasks.rs`**

  In `src/mcp/handlers/tasks.rs`, find `ReportUsageArgs` (~line 85) and remove `cost_usd`:

  ```rust
  struct ReportUsageArgs {
      task_id: i64,
      input_tokens: i64,
      output_tokens: i64,
      cache_read_tokens: Option<i64>,
      cache_write_tokens: Option<i64>,
  }
  ```

  In `handle_report_usage()` (~line 1056), remove `cost_usd` from the `UsageReport` construction:

  ```rust
  let report = UsageReport {
      input_tokens: args.input_tokens,
      output_tokens: args.output_tokens,
      cache_read_tokens: args.cache_read_tokens.unwrap_or(0),
      cache_write_tokens: args.cache_write_tokens.unwrap_or(0),
  };
  ```

---

## Task 8: Update docs/reference.md

**Files:**
- Modify: `docs/reference.md` (~line 97)

- [ ] **Step 1: Update task-usage-hook description**

  In `docs/reference.md`, find the line that reads:
  > "Reports token usage and cost per task"

  Change it to:
  > "Reports token usage per task"

- [ ] **Step 2: Commit**

  ```bash
  git add docs/reference.md
  git commit -m "docs: remove cost reference from task-usage-hook description"
  ```

---

## Task 9: Verify and fix service layer

**Files:**
- Modify: `src/service.rs` (if needed)

- [ ] **Step 1: Check if `service.rs` references `cost_usd`**



  ```bash
  grep -n "cost_usd" src/service.rs
  ```

  If found (test fixture ~line 1491 uses `cost_usd: 1.0`), remove it from the `UsageReport` construction.

---

## Task 10: Full build and test (green phase)

- [ ] **Step 1: Run full build**

  ```bash
  cargo build
  ```

  Expected: Zero errors.

- [ ] **Step 2: Run all tests**

  ```bash
  cargo test
  ```

  Expected: All pass. If snapshot tests fail due to rendering changes, run:
  ```bash
  INSTA_UPDATE=always cargo test tui::tests::snapshots
  rm src/tui/tests/snapshots/*.snap.new
  ```

- [ ] **Step 3: Verify `cost_usd` is completely gone**

  ```bash
  grep -rn "cost_usd" src/ docs/specs/
  ```

  Expected: No output.

- [ ] **Step 4: Commit**

  ```bash
  git add src/ docs/plans/ docs/specs/
  git commit -m "feat: remove cost_usd from all layers (models, DB, MCP, TUI)"
  ```

---

## Task 11: Wrap up

- [ ] **Step 1: Use `mcp__dispatch__wrap_up` to mark task complete and open PR**

---

## Verification

End-to-end check:
1. `cargo test` — all tests pass
2. `grep -rn "cost_usd" src/ docs/specs/` — returns nothing
3. `cargo clippy --all-targets -- -D warnings` — no warnings
4. `allium check docs/specs/core.allium && allium check docs/specs/tasks.allium` — no errors
