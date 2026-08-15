# Knowledge-Base Citation Gate Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** Make it structurally hard for a knowledge-base entry to carry an internal-code citation that can rot unseen, fix the two related drift bugs found while scoping this, and clean up the debt (learning #401 and any other reachable entry with the same shape).

**Architecture:** `LearningService::create_learning` rejects `summary`/`detail` text containing a `path.rs::symbol` citation, a `Type::method` citation, or a long (4+ underscore) bare snake_case name — mirroring three of `check-doc-symbols.sh`'s four candidate shapes, but as an unconditional reject (no phantom-index lookup, no escape hatch), and deliberately excluding the fourth shape (bare backticked snake_case) so MCP tool-name references stay allowed. Separately, two stale-drift bugs (`project` scope, "wrong verdict → human review") get fixed and locked down with a real equality test against the backing Rust enums, not a regex scan. `check-doc-symbols.sh` gains one new scanned file. `plugin/skills/learnings/SKILL.md` gets rewritten to state the citation rule explicitly.

**Tech Stack:** Rust (2021 edition), `regex` crate (new direct dependency, already resolved transitively at 1.12.3), rusqlite/tokio, existing MCP JSON-RPC test harness.

**Spec:** `docs/superpowers/specs/2026-08-15-kb-citation-gate-design.md` (design), `docs/specs/learnings.allium` (domain spec — Task 1 edits `RecordLearningViaMcp`).

## Global Constraints

- Spec first: `docs/specs/learnings.allium` is updated before any test/code for the new reject behavior (Task 1, Step 1).
- No escape hatch on the reject (unlike `check-doc-symbols.sh`'s `allow-phantom-symbol:`) — the KB has no human review step, so a free-text override in agent-authored content would be unenforceable.
- Do NOT reject bare backticked snake_case (e.g. `` `query_learnings` ``) — only `path.rs::symbol`, `Type::method`, and long (4+ underscore) unbackticked-or-backticked bare snake_case count as internal-code shapes.
- Inline test modules use `#[allow(clippy::unwrap_used, clippy::expect_used)]` at the top (workspace `-D warnings` policy).
- Never `git add`/commit anything under `docs/plans/` (n/a here — this plan lives under `docs/superpowers/plans/`, which IS committed per this repo's current policy).
- `check-doc-symbols.sh`'s TARGETS gets exactly one new entry (`plugin/skills/learnings/SKILL.md`), not the full `plugin/skills/*/SKILL.md` glob — the glob also breaks on `plugin/skills/allium-loop/SKILL.md` (13 unrelated findings), which is out of scope here and tracked as follow-up task #4195.

---

### Task 1: Reject internal-code citations in `create_learning`

**Files:**
- Modify: `docs/specs/learnings.allium` (`RecordLearningViaMcp` rule, ~line 397-437)
- Modify: `Cargo.toml` (add `regex` dependency)
- Modify: `src/service/learnings.rs` (new private detector function + `create_learning` wiring + inline tests)
- Modify: `src/mcp/handlers/tests/learnings.rs` (one end-to-end rejection test)

**Interfaces:**
- Produces: `fn find_code_citation(text: &str) -> Option<&str>` (private to `src/service/learnings.rs`) — returns the offending substring on a match, `None` if the text is clean. Used internally by `create_learning`; no other task depends on its signature.
- Consumes: existing `CreateLearningParams { kind, summary, detail, scope, scope_ref, tags, source_task_id }` and `ServiceError::Validation(String)` (both already defined in `src/service/learnings.rs` / `src/service/mod.rs`).

- [ ] **Step 1: Update the Allium spec first**

Edit `docs/specs/learnings.allium`. In the `RecordLearningViaMcp` rule, add a new `requires` line after the existing three, and extend the `@guidance` block:

```
rule RecordLearningViaMcp {
    when: McpRecordLearning(task, kind, summary, scope, detail?, scope_ref?, tags?)

    requires: summary != ""
    requires: scope = user implies scope_ref = null
    requires: scope in {repo, epic, task} implies scope_ref != null
    requires: not references_internal_code(summary) and not references_internal_code(detail)

    ensures: core/Learning.created(
        kind: kind,
        summary: summary,
        detail: detail,
        scope: scope,
        scope_ref: scope_ref ?? default_scope_ref(task, scope),
        tags: tags ?? {},
        status: approved,
        source_task: task,
        upvote_count: 0,
        created_at: now,
        updated_at: now
    )

    @guidance
        -- Agents call this during task execution or at wrap-up to surface
        -- non-obvious learnings. Entries land as approved and are eligible
        -- for retrieval immediately. Nobody reviews them: the active pool is
        -- curated by rate_learning scores, the ArchiveStaleLearning sweep and
        -- explicit delete_learning calls.
        -- default_scope_ref resolves when scope_ref is omitted:
        --   scope=repo    → task.repo_path
        --   scope=epic    → str(task.epic_id), error if task has no epic
        --   scope=task    → str(task.id)
        --   scope=user    → null
        -- tags defaults to the empty set when omitted.
        -- The MCP response is enriched with up to 5 similar approved
        -- learnings (matching kind, scope, and scope_ref) and warns against
        -- keeping a duplicate when one of them already captures the same
        -- knowledge. It does NOT suggest rate_learning on those entries:
        -- rate_learning requires a prior Retrieval row for (task, learning),
        -- and dedup matches are not necessarily retrievals for this task.
        --
        -- references_internal_code(text) rejects summary/detail containing:
        --   - a `path.rs::symbol` citation (a Rust file path plus a `::`-qualified
        --     symbol), or
        --   - a `Type::method` citation (a PascalCase type plus a `::`-qualified
        --     method), or
        --   - a bare snake_case identifier with four or more underscores,
        --     backticked or not.
        -- These three shapes mirror three of check-doc-symbols.sh's four
        -- candidate shapes (pathsym, typesym, bare) — see that script's header
        -- comment. Unlike that script, this is an unconditional reject, not a
        -- phantom-existence check: a citation that currently resolves is just
        -- as exposed to future rot as one that never did, since nothing
        -- re-validates the knowledge base against the codebase on a schedule.
        -- The fourth shape (a bare backticked snake_case span) is deliberately
        -- NOT rejected: this project's MCP tool names (query_learnings,
        -- wrap_up, exit_session, ...) are backticked snake_case with an
        -- underscore, and are a stable public interface an agent should be
        -- able to name in a learning, not the internal-code-detail this check
        -- exists to keep out.
        -- There is no escape hatch (unlike check-doc-symbols.sh's
        -- allow-phantom-symbol: marker): the knowledge base has no human
        -- review step, so a free-text override embedded in agent-authored
        -- content would be unenforceable. An agent that needs to cite specific
        -- code should put it in the Allium spec or a Rust doc comment instead,
        -- both of which are gated and both of which have an escape hatch.
        -- Implemented in find_code_citation() / create_learning()
        -- (src/service/learnings.rs).
}
```

- [ ] **Step 2: Add the `regex` dependency**

In `Cargo.toml`, under `[dependencies]`, add a line (alphabetical position is not enforced elsewhere in this file, so add it near the top with the other core deps):

```toml
regex = "1"
```

Run: `cargo build 2>&1 | tail -20`
Expected: builds successfully. `regex` was already resolved transitively at `1.12.3` (check `Cargo.lock`), so this should not change any other crate's resolved version — only promote `dispatch`'s own dependency edge.

- [ ] **Step 3: Write the failing detector tests**

In `src/service/learnings.rs`, inside the existing `#[cfg(test)] mod learning_tests { ... }` block (after the existing tests, before the closing `}`), add:

```rust
    #[test]
    fn find_code_citation_rejects_path_rs_symbol() {
        let hit = super::find_code_citation(
            "A step that must behave identically on both feed paths goes in \
             src/feed/cycle.rs::run_feed_cycle.",
        );
        assert_eq!(hit, Some("src/feed/cycle.rs::run_feed_cycle"));
    }

    #[test]
    fn find_code_citation_rejects_type_method() {
        let hit = super::find_code_citation("The FeedCycle::run entry point drives both paths.");
        assert_eq!(hit, Some("FeedCycle::run"));
    }

    #[test]
    fn find_code_citation_rejects_long_bare_snake_case() {
        let hit = super::find_code_citation(
            "Pinned by exec_trigger_epic_feed_quiet_command_reports_no_stderr today.",
        );
        assert_eq!(
            hit,
            Some("exec_trigger_epic_feed_quiet_command_reports_no_stderr")
        );
    }

    #[test]
    fn find_code_citation_rejects_long_bare_snake_case_even_backticked() {
        let hit = super::find_code_citation(
            "See `exec_trigger_epic_feed_quiet_command_reports_no_stderr` for the case.",
        );
        assert!(hit.is_some());
    }

    #[test]
    fn find_code_citation_allows_short_backticked_tool_names() {
        assert_eq!(
            super::find_code_citation("Call `query_learnings` before guessing."),
            None
        );
        assert_eq!(
            super::find_code_citation("Rate it with `rate_learning`, then `wrap_up`."),
            None
        );
    }

    #[test]
    fn find_code_citation_allows_plain_prose() {
        assert_eq!(
            super::find_code_citation(
                "TaskPatch double-Option means Some(None) clears a field, None leaves it unchanged."
            ),
            None
        );
        assert_eq!(
            super::find_code_citation("Feed-cycle logic must live in one shared place."),
            None
        );
    }

    #[tokio::test]
    async fn create_learning_rejects_summary_with_code_citation() {
        let svc = service().await;
        let err = svc
            .create_learning(CreateLearningParams {
                kind: LearningKind::Convention,
                summary: "A step goes in src/feed/cycle.rs::run_feed_cycle.".to_string(),
                detail: None,
                scope: LearningScope::User,
                scope_ref: None,
                tags: vec![],
                source_task_id: None,
            })
            .await
            .unwrap_err();
        assert!(matches!(err, ServiceError::Validation(_)));
    }

    #[tokio::test]
    async fn create_learning_rejects_detail_with_code_citation() {
        let svc = service().await;
        let err = svc
            .create_learning(CreateLearningParams {
                kind: LearningKind::Convention,
                summary: "Feed-cycle logic must live in one shared place.".to_string(),
                detail: Some("See FeedCycle::run for the exact entry point.".to_string()),
                scope: LearningScope::User,
                scope_ref: None,
                tags: vec![],
                source_task_id: None,
            })
            .await
            .unwrap_err();
        assert!(matches!(err, ServiceError::Validation(_)));
    }

    #[tokio::test]
    async fn create_learning_allows_tool_name_reference() {
        let svc = service().await;
        svc.create_learning(CreateLearningParams {
            kind: LearningKind::Procedural,
            summary: "Call `query_learnings` before guessing, not after.".to_string(),
            detail: None,
            scope: LearningScope::User,
            scope_ref: None,
            tags: vec![],
            source_task_id: None,
        })
        .await
        .unwrap();
    }
```

- [ ] **Step 4: Run the tests to verify they fail**

Run: `cargo test --lib service::learnings 2>&1 | tail -40`
Expected: FAIL to compile — `find_code_citation` is not defined.

- [ ] **Step 5: Implement the detector and wire it into `create_learning`**

Near the top of `src/service/learnings.rs` (after the existing `use` block, before `QueryLearningsParams`), add:

```rust
use std::sync::LazyLock;

use regex::Regex;

// ---------------------------------------------------------------------------
// Internal-code citation detection
// ---------------------------------------------------------------------------

// Mirrors three of check-doc-symbols.sh's four candidate shapes (pathsym,
// typesym, bare) — see docs/specs/learnings.allium: RecordLearningViaMcp for
// why the fourth shape (bare backticked snake_case) is deliberately excluded.
static PATHSYM_RE: LazyLock<Regex> = LazyLock::new(|| {
    Regex::new(r"[A-Za-z0-9_./-]+\.rs::[A-Za-z_][A-Za-z0-9_]*(?:::[A-Za-z_][A-Za-z0-9_]*)*")
        .expect("PATHSYM_RE must compile")
});

static TYPESYM_RE: LazyLock<Regex> = LazyLock::new(|| {
    Regex::new(r"[A-Z][A-Za-z0-9]*(?:::[A-Za-z_][A-Za-z0-9_]*)+").expect("TYPESYM_RE must compile")
});

// At least four underscores (five word segments) — the same threshold
// check-doc-symbols.sh measured as the lowest value with zero false positives
// across the docs/specs/ corpus.
static BARE_RE: LazyLock<Regex> = LazyLock::new(|| {
    Regex::new(r"[a-z][a-z0-9]*(?:_[a-z0-9]+){4,}").expect("BARE_RE must compile")
});

/// Detects an internal-code-shaped citation in learning text: a
/// `path.rs::symbol` reference, a `Type::method` reference, or a long (5+
/// segment) bare snake_case identifier. Returns the offending substring on a
/// match. See docs/specs/learnings.allium: RecordLearningViaMcp for the
/// rationale, including why short backticked identifiers (MCP tool names)
/// are deliberately not flagged.
fn find_code_citation(text: &str) -> Option<&str> {
    PATHSYM_RE
        .find(text)
        .or_else(|| TYPESYM_RE.find(text))
        .or_else(|| BARE_RE.find(text))
        .map(|m| m.as_str())
}
```

Then, in `create_learning`, right after the existing `summary` empty-check (before the scope match block), add:

```rust
        if let Some(hit) = find_code_citation(&params.summary) {
            return Err(ServiceError::Validation(format!(
                "learning summary cites internal code (`{hit}`) — this rots silently since \
                 nothing re-checks the knowledge base against the codebase. Describe the \
                 durable behavior in prose instead, or add the citation to the relevant \
                 docs/specs/*.allium file or a Rust doc comment, both of which \
                 check-doc-symbols.sh keeps accurate on every push."
            )));
        }
        if let Some(detail) = &params.detail {
            if let Some(hit) = find_code_citation(detail) {
                return Err(ServiceError::Validation(format!(
                    "learning detail cites internal code (`{hit}`) — this rots silently since \
                     nothing re-checks the knowledge base against the codebase. Describe the \
                     durable behavior in prose instead, or add the citation to the relevant \
                     docs/specs/*.allium file or a Rust doc comment, both of which \
                     check-doc-symbols.sh keeps accurate on every push."
                )));
            }
        }
```

- [ ] **Step 6: Run the tests to verify they pass**

Run: `cargo test --lib service::learnings 2>&1 | tail -40`
Expected: PASS — all `find_code_citation_*` and `create_learning_*` tests green.

- [ ] **Step 7: Write and run the end-to-end MCP test**

In `src/mcp/handlers/tests/learnings.rs`, add near the other `record_learning` tests:

```rust
#[tokio::test]
async fn record_learning_rejects_code_citation_in_summary() {
    let state = test_state().await;
    let task_id = create_task_in_repo(&state, "/repo/foo").await;

    let resp = call(
        &state,
        "tools/call",
        Some(json!({
            "name": "record_learning",
            "arguments": {
                "task_id": task_id.0,
                "kind": "convention",
                "summary": "A step goes in src/feed/cycle.rs::run_feed_cycle.",
                "scope": "user"
            }
        })),
    )
    .await;
    assert_error(&resp, "internal code");

    let learnings = state
        .db
        .list_learnings(crate::db::LearningFilter::default())
        .await
        .unwrap();
    assert!(learnings.is_empty(), "rejected learning must not be created");
}

#[tokio::test]
async fn record_learning_allows_mcp_tool_name_reference() {
    let state = test_state().await;
    let task_id = create_task_in_repo(&state, "/repo/foo").await;

    let resp = call(
        &state,
        "tools/call",
        Some(json!({
            "name": "record_learning",
            "arguments": {
                "task_id": task_id.0,
                "kind": "procedural",
                "summary": "Call `query_learnings` before guessing, not after.",
                "scope": "user"
            }
        })),
    )
    .await;
    assert!(resp.error.is_none(), "unexpected error: {:?}", resp.error);
}
```

Run: `cargo test mcp::handlers::tests::learnings 2>&1 | tail -40`
Expected: PASS.

- [ ] **Step 8: Commit**

```bash
git add docs/specs/learnings.allium Cargo.toml Cargo.lock src/service/learnings.rs src/mcp/handlers/tests/learnings.rs
git commit -m "feat(learnings): reject internal-code citations in record_learning

Learning summaries/details citing a path.rs::symbol, Type::method, or long
bare snake_case identifier rot silently — nothing re-checks the knowledge
base the way check-doc-symbols.sh re-checks docs on every push. Reject the
three shapes outright; MCP tool-name references (query_learnings, wrap_up,
...) stay allowed since they're a stable interface, not internal detail."
```

---

### Task 2: Fix the `project`-scope and human-review drift, lock with an enum-parity test

**Files:**
- Modify: `src/models/learnings.rs` (add `LearningScope::ALL`, `LearningKind::ALL`)
- Modify: `src/mcp/handlers/dispatch.rs` (`record_learning` schema, `rate_learning` tool description)
- Modify: `src/mcp/handlers/learnings.rs` (`handle_rate_learning` response text)
- Modify: `src/mcp/handlers/tests/learnings.rs` (new enum-parity test)

**Interfaces:**
- Produces: `LearningScope::ALL: &'static [LearningScope]`, `LearningKind::ALL: &'static [LearningKind]` (mirrors the existing `TaskStatus::ALL` pattern in `src/models/tasks.rs:26`).
- Consumes: `tool_definitions()` (already `pub(super)` in `src/mcp/handlers/dispatch.rs`, already imported into `src/mcp/handlers/tests/mod.rs` via `use super::dispatch::{handle_mcp, tool_definitions};` and re-exported to sibling test modules through `use super::*;`).

- [ ] **Step 1: Write the failing enum-parity test**

In `src/mcp/handlers/tests/learnings.rs`, add:

```rust
#[test]
fn record_learning_scope_and_kind_enums_match_the_rust_enums() {
    let defs = tool_definitions();
    let tools = defs["tools"].as_array().unwrap();
    let record_learning = tools
        .iter()
        .find(|t| t["name"] == "record_learning")
        .expect("record_learning tool must be registered");

    let scope_enum: Vec<&str> = record_learning["inputSchema"]["properties"]["scope"]["enum"]
        .as_array()
        .unwrap()
        .iter()
        .map(|v| v.as_str().unwrap())
        .collect();
    let expected_scopes: Vec<&str> = crate::models::LearningScope::ALL
        .iter()
        .map(|s| s.as_str())
        .collect();
    assert_eq!(
        scope_enum, expected_scopes,
        "record_learning's advertised scope enum must match LearningScope's variants exactly"
    );

    let kind_enum: Vec<&str> = record_learning["inputSchema"]["properties"]["kind"]["enum"]
        .as_array()
        .unwrap()
        .iter()
        .map(|v| v.as_str().unwrap())
        .collect();
    let expected_kinds: Vec<&str> = crate::models::LearningKind::ALL
        .iter()
        .map(|k| k.as_str())
        .collect();
    assert_eq!(
        kind_enum, expected_kinds,
        "record_learning's advertised kind enum must match LearningKind's variants exactly"
    );
}
```

- [ ] **Step 2: Run the test to verify it fails**

Run: `cargo test mcp::handlers::tests::learnings::record_learning_scope_and_kind_enums 2>&1 | tail -30`
Expected: FAIL to compile — `LearningScope::ALL` / `LearningKind::ALL` don't exist yet.

- [ ] **Step 3: Add the `ALL` consts**

In `src/models/learnings.rs`, inside `impl LearningKind { ... }` (right after the opening brace, before `pub fn as_str`), add:

```rust
    pub const ALL: &'static [LearningKind] = &[
        LearningKind::Pitfall,
        LearningKind::Convention,
        LearningKind::Preference,
        LearningKind::ToolRecommendation,
        LearningKind::Procedural,
        LearningKind::Landscape,
    ];

```

Inside `impl LearningScope { ... }` (right after the opening brace, before `pub fn as_str`), add:

```rust
    pub const ALL: &'static [LearningScope] = &[
        LearningScope::User,
        LearningScope::Repo,
        LearningScope::Epic,
        LearningScope::Task,
    ];

```

- [ ] **Step 4: Run the test — it should still fail (schema still stale)**

Run: `cargo test mcp::handlers::tests::learnings::record_learning_scope_and_kind_enums 2>&1 | tail -30`
Expected: FAIL — `scope_enum` is `["user", "repo", "project", "epic", "task"]`, `expected_scopes` is `["user", "repo", "epic", "task"]`.

- [ ] **Step 5: Fix the stale schema and tool descriptions**

In `src/mcp/handlers/dispatch.rs`, in the `record_learning` registration (around line 371-409):

Change:
```rust
        "Record a new entry in the shared knowledge base. The entry is immediately active and will be injected into future dispatch prompts for agents working in the matching scope. Omit scope_ref to auto-derive it from the calling task (recommended in most cases).",
```
to:
```rust
        "Record a new entry in the shared knowledge base. The entry is immediately active and will be injected into future dispatch prompts for agents working in the matching scope. Omit scope_ref to auto-derive it from the calling task (recommended in most cases). summary/detail must not cite a specific file, symbol, or line number — those belong in the Allium spec or a Rust doc comment, where check-doc-symbols.sh keeps them accurate; describe the durable behavior instead.",
```

Change:
```rust
                "scope": {
                    "type": "string",
                    "description": "Scope this learning applies to: user (global), repo, project, epic, or task",
                    "enum": ["user", "repo", "project", "epic", "task"]
                },
```
to:
```rust
                "scope": {
                    "type": "string",
                    "description": "Scope this learning applies to: user (global), repo, epic, or task",
                    "enum": ["user", "repo", "epic", "task"]
                },
```

In the `rate_learning` registration (around line 437-441), change:
```rust
        "Give feedback on a knowledge base entry that was surfaced to you this task. \
Call any time you act on a retrieved learning. 'helped' upvotes it (a usefulness signal that \
boosts ranking); 'wrong' flags an approved entry for human review. You can only rate a learning \
that was surfaced to you (injected into your prompt or returned by query_learnings).",
```
to:
```rust
        "Give feedback on a knowledge base entry that was surfaced to you this task. \
Call any time you act on a retrieved learning. 'helped' upvotes it (a usefulness signal that \
boosts ranking); 'wrong' downvotes it (may go negative) — neither changes its status, and \
there is no human review step. You can only rate a learning that was surfaced to you (injected \
into your prompt or returned by query_learnings).",
```

- [ ] **Step 6: Fix the response text in `handle_rate_learning`**

In `src/mcp/handlers/learnings.rs`, in `handle_rate_learning`, change:

```rust
            let note = match parsed.verdict {
                LearningVerdict::Helped => "recorded as helped (upvoted)",
                LearningVerdict::Wrong => {
                    "recorded as wrong (flagged for review if it was approved)"
                }
            };
```
to:
```rust
            let note = match parsed.verdict {
                LearningVerdict::Helped => "recorded as helped (upvoted)",
                LearningVerdict::Wrong => "recorded as wrong (downvoted; no review step)",
            };
```

- [ ] **Step 7: Run the full test suite for this area**

Run: `cargo test mcp::handlers::tests::learnings 2>&1 | tail -60`
Expected: PASS — the enum-parity test and every existing `rate_learning`/`record_learning` test still green (none of the existing tests assert on the old "project"/"human review" text, per the file already read in this task).

- [ ] **Step 8: Commit**

```bash
git add src/models/learnings.rs src/mcp/handlers/dispatch.rs src/mcp/handlers/learnings.rs src/mcp/handlers/tests/learnings.rs
git commit -m "fix(learnings): drop stale project scope, fix wrong-verdict text

record_learning's JSON schema still advertised a 'project' scope value
removed from LearningScope, and both its own tool description and
rate_learning's said a 'wrong' verdict routes to human review — neither
is true (learnings.allium: no human gate, no needs_review state, no
status change on either verdict). Locked down with an enum-parity test
comparing the advertised schema against LearningScope::ALL/LearningKind::ALL
directly, rather than a symbol-phantom regex scan."
```

---

### Task 3: Rewrite the `learnings` skill's authoring guidance

**Files:**
- Modify: `plugin/skills/learnings/SKILL.md`
- Modify: `src/setup/plugins.rs` (new skill-copy test in `mod tests`)

**Interfaces:** none (skill copy is embedded via `include_str!`/build-time inclusion already wired for every skill; no new Rust interface).

- [ ] **Step 1: Write the failing skill-copy test**

In `src/setup/plugins.rs`, inside `mod tests { ... }`, add (near the other `skill_body`-based tests):

```rust
    /// The "Do NOT record" list must name the internal-code-citation failure
    /// mode explicitly (task #4152 — learning #401 carried a stale
    /// `src/feed/cycle.rs::run_feed_cycle` citation that no gate ever caught).
    /// Scoped to that one section: the rest of the skill mentions plenty of
    /// backticked identifiers (tool names) that would make a whole-document
    /// check pass even with this specific rule missing.
    #[test]
    fn learnings_skill_forbids_code_citations() {
        let section = section_after(skill_body("learnings"), "### Do NOT record:")
            .expect("learnings skill must have a 'Do NOT record' section");
        assert!(
            section.contains("path.rs::symbol") || section.contains("path.rs"),
            "the Do NOT record list must name the path.rs::symbol citation shape: {section}"
        );
        assert!(
            section.to_lowercase().contains("rot"),
            "the rule must explain WHY (silent rot, no re-check) not just state a ban: {section}"
        );
    }

    /// The scope table must not offer `project` — LearningScope has only
    /// user/repo/epic/task (task #4152: the row was stale, and an agent
    /// passing scope=\"project\" gets a deserialization error).
    #[test]
    fn learnings_skill_scope_table_has_no_project_row() {
        let content = skill_body("learnings");
        assert!(
            !content.contains("| `project` |"),
            "the scope table must not offer a project scope row: {content}"
        );
    }

    /// The `wrong` verdict bullet must not claim a human-review step exists
    /// (learnings.allium: no human gate, no needs_review state, no status
    /// change on either verdict).
    #[test]
    fn learnings_skill_wrong_verdict_does_not_claim_human_review() {
        let content = skill_body("learnings").to_lowercase();
        assert!(
            !content.contains("human review"),
            "the learnings skill must not claim a wrong verdict triggers human review: {content}"
        );
    }
```

- [ ] **Step 2: Run the tests to verify they fail**

Run: `cargo test setup::plugins::tests::learnings_skill 2>&1 | tail -40`
Expected: FAIL — all three (current SKILL.md still has the `project` row, the "human review" claim, and no citation rule).

- [ ] **Step 3: Rewrite `plugin/skills/learnings/SKILL.md`**

Replace the file's full contents with:

```markdown
---
name: learnings
description: Manage the knowledge base lifecycle — query, rate, and record entries. Use at wrap-up or whenever you want to contribute to the shared knowledge base.
---

# Knowledge Base

Use this skill to interact with the shared knowledge base — recording new entries and rating entries that were surfaced to you.

To *query* the knowledge base mid-task, call `query_learnings` directly (with `task_id` and an optional `tag_filter` as an array of tags, e.g. `["conventions", "rust"]`). Do that when anything is unclear — before guessing or asking.

**Announce at start:** "I'm using the learnings skill to interact with the knowledge base."

## Rating entries you acted on

When you act on a knowledge base entry that was surfaced to you (injected into your prompt or returned by `query_learnings`), give feedback right away:
```
rate_learning(learning_id=<id>, task_id=<your task id>, verdict="helped")
```

- `verdict="helped"` — the entry applied and was useful (upvotes it).
- `verdict="wrong"` — the entry misled you or is inaccurate (downvotes it; may go negative). Neither verdict changes the entry's status — there is no human review step.

Do this at the moment you act on it, not deferred to wrap-up. You can only rate entries that were surfaced to you this task.

**Rate `helped` when:** an entry saved you from a pitfall, matched a convention you applied, or guided a decision you made.

**Rate `wrong` when:** an entry was misleading or no longer accurate.

**Don't rate:** entries you read but didn't act on.

## Recording new entries

Before finishing a task, ask: *Did I discover anything non-obvious that a future agent would benefit from knowing?*

### Record if:

- The user expressed a **preference** explicitly that isn't already in CLAUDE.md
- You built a **landscape understanding** of a codebase area worth sharing
- You found a **convention** that applies broadly but isn't visible from reading the code
- A specific **workflow pattern** solved a cross-repo or cross-task problem elegantly
- This epic or project has a **procedural step** every agent working here should follow

### Do NOT record:

- Code patterns readable from source code — the code is self-documenting
- Things already in CLAUDE.md, README, or existing docs
- Git history — visible via `git log` / `git blame`
- Debugging solutions where the fix is in the commit
- Things too specific to generalise — if it won't apply to other tasks, skip it
- How you fixed a specific problem — that's in the code and commit message
- **A citation to a specific file, symbol, or line number** (`path.rs::symbol`, `Type::method`, a long snake_case function name). These rot silently — nothing re-checks the knowledge base the way `check-doc-symbols.sh` re-checks docs on every push, so a correct citation today can go stale forever without anyone noticing. `record_learning` rejects these outright. If the fact is worth citing precisely, put it in the Allium spec or a Rust doc comment (both gated, both re-checked on every push) and describe the *behavior* here in prose instead. A short reference to a stable MCP tool name (`query_learnings`, `wrap_up`, ...) is fine — that's a public interface, not the internal detail this rule exists to keep out.

  Bad: "A step that must behave identically on both feed paths goes in `src/feed/cycle.rs::run_feed_cycle`."
  Good: "Feed-cycle logic shared by the auto-poll and manual-refresh paths must live in one place, not be duplicated per caller — see feeds.allium."

### Picking a kind

| Kind | Use for |
|------|---------|
| `pitfall` | Silent failures, API traps, behaviour surprises — warn future agents |
| `convention` | Preferred patterns or style for this codebase |
| `preference` | Explicit user preference expressed during the task |
| `tool_recommendation` | Specific tool or library for a problem type |
| `procedural` | Step-by-step instructions to prefix dispatch prompts (epic-level) |
| `landscape` | Codebase/system overviews — service maps, module responsibilities |

### Picking a scope

| Scope | Use when | `scope_ref` |
|-------|----------|-------------|
| `user` | Personal workflow preference, applies to all work | omit |
| `repo` | Codebase-wide convention or landscape entry | omit (auto-derived) |
| `epic` | Shared design decision for this epic only | omit (auto-derived) |
| `task` | One-off note; not auto-injected into future prompts | omit (auto-derived) |

**Default to `repo` for code conventions and `user` for workflow preferences.**

### Writing a good summary

- **One sentence only.** If you need two, the entry is too broad — split or drop it.
- **Name the specific thing.** Not "be careful with DB queries" but "TaskPatch double-Option means `Some(None)` clears a field, `None` leaves it unchanged."
- **Lead with the actionable insight.** What should a future agent do differently?
- **No file/symbol/line citations** — see "Do NOT record" above.

## Deleting stale entries

If a knowledge base entry is incorrect, outdated, or should be removed entirely, delete it:

```
delete_learning(learning_id=<id>)
```

This permanently removes the entry. Use `query_learnings` first to find the entry's ID if you only know its content.
```

- [ ] **Step 4: Run the skill-copy tests**

Run: `cargo test setup::plugins::tests::learnings_skill 2>&1 | tail -40`
Expected: PASS.

- [ ] **Step 5: Run the full pre-push-equivalent checks**

Run: `cargo fmt --check && cargo clippy --all-targets -- -D warnings 2>&1 | tail -60`
Expected: clean.

- [ ] **Step 6: Commit**

```bash
git add plugin/skills/learnings/SKILL.md src/setup/plugins.rs
git commit -m "docs(learnings-skill): forbid code citations, fix stale scope/review text

The skill's own scope table still offered the removed 'project' scope and
described a 'wrong' verdict as routing to human review (neither true), and
said nothing about the failure mode that produced learning #401 — a stale
src/feed/cycle.rs::run_feed_cycle citation. Locked all three down with
skill-copy tests scoped to their sections."
```

---

### Task 4: Extend `check-doc-symbols.sh` to `plugin/skills/learnings/SKILL.md`

**Files:**
- Modify: `scripts/check-doc-symbols.sh` (TARGETS default list)
- Modify: `scripts/test-check-doc-symbols.sh` (one new case)

**Interfaces:** none (shell scripts, no cross-task interface).

- [ ] **Step 1: Add the new case to the self-test**

In `scripts/test-check-doc-symbols.sh`, after the existing "default scan list must cover" loop (the `for needed in 'docs/specs' 'CLAUDE.md' 'src'; do ... done` block), add:

```bash
# --- The default scan list must also cover the learnings skill (#4152). ----
# Only this one file, not the full plugin/skills/*/SKILL.md glob: the glob
# also matches plugin/skills/allium-loop/SKILL.md, which describes its own
# local state-file schema in prose (field names with no backing Rust/Allium
# declaration) and would false-positive — see follow-up task #4195.
if ! grep -q 'plugin/skills/learnings/SKILL.md' "$CHECKER"; then
    echo "FAIL: check-doc-symbols.sh does not scan plugin/skills/learnings/SKILL.md" >&2
    failures=$((failures + 1))
fi
```

- [ ] **Step 2: Run the self-test to verify it fails**

Run: `bash scripts/test-check-doc-symbols.sh`
Expected: FAIL — `check-doc-symbols.sh does not scan plugin/skills/learnings/SKILL.md`.

- [ ] **Step 3: Add the target**

In `scripts/check-doc-symbols.sh`, change:
```bash
    shopt -s nullglob
    TARGETS+=(docs/*.md docs/specs/*.allium)
    shopt -u nullglob
```
to:
```bash
    shopt -s nullglob
    TARGETS+=(docs/*.md docs/specs/*.allium)
    shopt -u nullglob
    # Only this one skill file, not the full plugin/skills/*/SKILL.md glob —
    # see the comment on the same line in test-check-doc-symbols.sh (#4152 /
    # follow-up #4195).
    [[ -f plugin/skills/learnings/SKILL.md ]] && TARGETS+=(plugin/skills/learnings/SKILL.md)
```

- [ ] **Step 4: Run the self-test to verify it passes**

Run: `bash scripts/test-check-doc-symbols.sh`
Expected: `test-check-doc-symbols: all assertions passed`

- [ ] **Step 5: Confirm the real file is clean**

Task 3 already rewrote `plugin/skills/learnings/SKILL.md` to drop the stale `project` row and the "human review" claim, so this should pass cleanly now that the file is in the scanned set.

Run: `bash scripts/check-doc-symbols.sh plugin/skills/learnings/SKILL.md`
Expected: `check-doc-symbols: all symbol references resolve`

- [ ] **Step 6: Commit**

```bash
git add scripts/check-doc-symbols.sh scripts/test-check-doc-symbols.sh
git commit -m "chore(scripts): scan plugin/skills/learnings/SKILL.md for phantom symbols

Narrower than the full plugin/skills/*/SKILL.md glob, which also breaks on
allium-loop's self-described state-file schema (follow-up task #4195)."
```

---

### Task 5: Sweep and delete citation-shaped learnings (including #401)

This task is a data cleanup, not a code change — it uses this task's own MCP access (`query_learnings`, `delete_learning`), reachable only for `user`-scoped entries and entries scoped to this repo (`repo_path` = this task's own repo). Do this only after Tasks 1-4 have landed, so the write-time reject is already in place and nothing new of this shape can be re-added mid-cleanup.

**Files:** none (no code change).

- [ ] **Step 1: List every reachable approved learning**

Call `query_learnings(task_id=<this task's id>, limit=50)` (repeat with different `query` text or raise scope as needed to see everything reachable — `query_learnings` is RAG-ranked, not exhaustive, so also cross-check against a direct listing if you have DB access in this environment; if not, rely on repeated `query_learnings` calls with varied query terms covering the repo's main subsystems until no new IDs appear).

- [ ] **Step 2: Identify citation-shaped entries**

For each returned learning, check its summary/detail text for the same three shapes as Task 1's `find_code_citation` (a `path.rs::symbol`, a `Type::method`, or a long bare snake_case name). Learning #401 (`src/feed/cycle.rs::run_feed_cycle`) is a confirmed match — its advice is already correctly captured in `feeds.allium` citing the current name (`FeedCycle::run`), so this is a duplicate, not a unique fact.

- [ ] **Step 3: Delete each match**

For every matching learning (including #401), call `delete_learning(learning_id=<id>)`.

- [ ] **Step 4: Verify**

Re-run `query_learnings` (or `get_learning(401)`, expecting a not-found error) to confirm the deleted entries no longer appear.

- [ ] **Step 5: Note anything out of reach**

If any repo-scoped learning under a *different* `repo_path` is suspected of the same issue, it's out of reach from this task's MCP context — note it in the task's wrap-up summary rather than attempting to reach it (see design doc's Non-goals).
