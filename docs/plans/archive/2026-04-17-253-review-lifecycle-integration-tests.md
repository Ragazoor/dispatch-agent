# Review Lifecycle Integration Tests Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** Add integration tests for the full review/security board lifecycle and fix two bugs found along the way: security alerts never auto-refresh on tick, and the review agent tmux window is never cleaned up when a tracked PR disappears from the board.

**Architecture:** All state-machine tests live in a new `tests/review_lifecycle.rs`, driving `App::update()` directly — the same pattern as `tests/lifecycle.rs`. Two bugs are fixed TDD-style (failing test first, then fix). One new public accessor is added to `App` for test assertions.

**Tech Stack:** Rust, `dispatch_tui` crate, `chrono`, `std::time::Duration`, `std::time::Instant`.

---

### Task 1: Create test file and add tick-triggers-review-fetch test

This is the "Option B" tick wiring test. It verifies that `App::update(Message::Tick)` emits `Command::FetchPrs` for both Review and Authored lists when they have never been fetched (`last_fetch = None`).

**Files:**
- Create: `tests/review_lifecycle.rs`

- [ ] **Step 1: Write the test file with the tick test**

```rust
//! Integration tests: review board lifecycle through App::update() with a real (in-memory) DB.

use std::time::Duration;

use dispatch_tui::models::{CiStatus, ReviewDecision, ReviewPr};
use dispatch_tui::tui::{App, Command, Message, PrListKind, ReviewAgentRequest};
use chrono::Utc;

fn make_app() -> App {
    App::new(vec![], Duration::from_secs(300))
}

fn make_pr(number: i64, repo: &str) -> ReviewPr {
    ReviewPr {
        number,
        title: format!("PR {number}"),
        author: "alice".to_string(),
        repo: repo.to_string(),
        url: format!("https://github.com/{repo}/pull/{number}"),
        is_draft: false,
        created_at: Utc::now(),
        updated_at: Utc::now(),
        additions: 10,
        deletions: 5,
        review_decision: ReviewDecision::ReviewRequired,
        labels: vec![],
        body: String::new(),
        head_ref: "feat/thing".to_string(),
        ci_status: CiStatus::None,
        reviewers: vec![],
    }
}

// ---------------------------------------------------------------------------
// Tick wiring: App emits FetchPrs when review lists are stale
// ---------------------------------------------------------------------------

#[test]
fn tick_triggers_fetch_when_review_list_stale() {
    let mut app = make_app();
    // Both lists have last_fetch = None (never fetched) — needs_fetch returns true
    let cmds = app.update(Message::Tick);
    assert!(
        cmds.iter().any(|c| matches!(c, Command::FetchPrs(PrListKind::Review))),
        "Tick should emit FetchPrs(Review) when list is stale"
    );
    assert!(
        cmds.iter().any(|c| matches!(c, Command::FetchPrs(PrListKind::Authored))),
        "Tick should emit FetchPrs(Authored) when list is stale"
    );
}
```

- [ ] **Step 2: Run the test to verify it passes**

```bash
cargo test --test review_lifecycle tick_triggers_fetch_when_review_list_stale
```

Expected: PASS (this behavior already exists in `on_tick()`).

- [ ] **Step 3: Commit**

```bash
git add tests/review_lifecycle.rs
git commit -m "test: add review_lifecycle integration test file with tick wiring test"
```

---

### Task 2: Fix security alert auto-refresh (TDD)

`on_tick()` in `src/tui/mod.rs` refreshes review PRs automatically but never checks security alerts. Add the test first (it will fail), then fix `on_tick()`.

**Files:**
- Modify: `tests/review_lifecycle.rs`
- Modify: `src/tui/mod.rs` (lines 1933–1945, the PR refresh block)

- [ ] **Step 1: Write the failing test**

Add to `tests/review_lifecycle.rs`:

```rust
// ---------------------------------------------------------------------------
// Bug: security alerts never auto-refresh on tick
// ---------------------------------------------------------------------------

#[test]
fn tick_triggers_security_fetch_when_stale() {
    let mut app = make_app();
    // security.last_fetch = None (default) — needs_fetch(SECURITY_POLL_INTERVAL) returns true
    let cmds = app.update(Message::Tick);
    assert!(
        cmds.iter().any(|c| matches!(c, Command::FetchSecurityAlerts)),
        "Tick should emit FetchSecurityAlerts when security list is stale"
    );
}
```

- [ ] **Step 2: Run to verify it fails**

```bash
cargo test --test review_lifecycle tick_triggers_security_fetch_when_stale
```

Expected: FAIL — `FetchSecurityAlerts` is never emitted by `handle_tick()`.

- [ ] **Step 3: Fix `on_tick()` in `src/tui/mod.rs`**

Find the block ending at line 1945 (after the `FetchPrs(Authored)` push). Add the security alert check immediately after:

```rust
        // Also refresh my PRs data if stale (> 30s)
        if self.review.authored.needs_fetch(REVIEW_REFRESH_INTERVAL)
            && !self.review.authored.loading
        {
            self.review.authored.loading = true;
            cmds.push(Command::FetchPrs(PrListKind::Authored));
        }

        // Refresh security alerts if stale (> 5m)
        if self.security.needs_fetch(SECURITY_POLL_INTERVAL) && !self.security.loading {
            self.security.loading = true;
            cmds.push(Command::FetchSecurityAlerts);
        }
```

- [ ] **Step 4: Run to verify it passes**

```bash
cargo test --test review_lifecycle tick_triggers_security_fetch_when_stale
```

Expected: PASS.

- [ ] **Step 5: Run the full suite to check for regressions**

```bash
cargo test
```

Expected: all pass.

- [ ] **Step 6: Commit**

```bash
git add tests/review_lifecycle.rs src/tui/mod.rs
git commit -m "fix: auto-refresh security alerts on tick (mirrors review PR refresh)"
```

---

### Task 3: Add `review_agent_handle` public accessor to `App`

Tests for status update and PR cleanup need to inspect the `review_agents` map. Add a public read-only accessor.

**Files:**
- Modify: `src/tui/mod.rs` (around line 307, after `review_detail_visible()`)

- [ ] **Step 1: Add the accessor**

In `src/tui/mod.rs`, after `pub fn review_detail_visible(...)`:

```rust
    pub fn review_agent_handle(
        &self,
        repo: &str,
        number: i64,
    ) -> Option<&crate::tui::ReviewAgentHandle> {
        let key = crate::models::PrRef::new(repo.to_string(), number);
        self.review.review_agents.get(&key)
    }
```

- [ ] **Step 2: Verify it compiles**

```bash
cargo build
```

Expected: no errors.

- [ ] **Step 3: Commit**

```bash
git add src/tui/mod.rs
git commit -m "feat: expose review_agent_handle accessor on App for tests"
```

---

### Task 4: Tests for dispatch and agent-dispatched

Two tests: `dispatch_review_agent_emits_command` (key press → command), `review_agent_dispatched_registers_handle` (agent started → handle registered).

**Files:**
- Modify: `tests/review_lifecycle.rs`

- [ ] **Step 1: Add the tests**

```rust
// ---------------------------------------------------------------------------
// Dispatch review agent
// ---------------------------------------------------------------------------

#[test]
fn dispatch_review_agent_emits_command() {
    let mut app = make_app();
    // Set a local repo path so resolve_repo_path can match "org/app"
    app.update(Message::RepoPathsUpdated(vec!["/repos/org/app".to_string()]));
    // Load a PR so the review board has context
    app.update(Message::PrsLoaded(
        PrListKind::Review,
        vec![make_pr(42, "org/app")],
    ));

    let req = ReviewAgentRequest {
        repo: "org/app".to_string(),
        github_repo: "org/app".to_string(),
        number: 42,
        head_ref: "feat/thing".to_string(),
        is_dependabot: false,
    };
    let cmds = app.update(Message::DispatchReviewAgent(req));

    assert!(
        cmds.iter()
            .any(|c| matches!(c, Command::DispatchReviewAgent(_))),
        "DispatchReviewAgent message should emit Command::DispatchReviewAgent"
    );
}

#[test]
fn review_agent_dispatched_registers_handle() {
    let mut app = make_app();
    app.update(Message::PrsLoaded(
        PrListKind::Review,
        vec![make_pr(42, "org/app")],
    ));

    app.update(Message::ReviewAgentDispatched {
        github_repo: "org/app".to_string(),
        number: 42,
        tmux_window: "win-42".to_string(),
        worktree: "/wt/42".to_string(),
    });

    let handle = app
        .review_agent_handle("org/app", 42)
        .expect("handle should be registered after ReviewAgentDispatched");
    assert_eq!(handle.tmux_window, "win-42");
    assert_eq!(handle.worktree, "/wt/42");
}
```

- [ ] **Step 2: Run to verify both pass**

```bash
cargo test --test review_lifecycle dispatch_review_agent
cargo test --test review_lifecycle review_agent_dispatched
```

Expected: both PASS.

- [ ] **Step 3: Commit**

```bash
git add tests/review_lifecycle.rs
git commit -m "test: dispatch and agent-dispatched lifecycle tests"
```

---

### Task 5: Test for review status update

Verify that `Message::ReviewStatusUpdated` changes the status on the registered handle.

**Files:**
- Modify: `tests/review_lifecycle.rs`

- [ ] **Step 1: Add the test**

```rust
// ---------------------------------------------------------------------------
// Status update
// ---------------------------------------------------------------------------

#[test]
fn review_status_update_reflects_on_handle() {
    use dispatch_tui::models::ReviewAgentStatus;

    let mut app = make_app();
    app.update(Message::PrsLoaded(
        PrListKind::Review,
        vec![make_pr(42, "org/app")],
    ));
    app.update(Message::ReviewAgentDispatched {
        github_repo: "org/app".to_string(),
        number: 42,
        tmux_window: "win-42".to_string(),
        worktree: "/wt/42".to_string(),
    });

    app.update(Message::ReviewStatusUpdated {
        repo: "org/app".to_string(),
        number: 42,
        status: ReviewAgentStatus::FindingsReady,
    });

    let handle = app
        .review_agent_handle("org/app", 42)
        .expect("handle should still exist after status update");
    assert_eq!(handle.status, ReviewAgentStatus::FindingsReady);
}
```

- [ ] **Step 2: Run to verify it passes**

```bash
cargo test --test review_lifecycle review_status_update_reflects_on_handle
```

Expected: PASS.

- [ ] **Step 3: Commit**

```bash
git add tests/review_lifecycle.rs
git commit -m "test: review status update reflects on agent handle"
```

---

### Task 6: Test for PR approved column

Verify that `PrsLoaded` with an approved PR places it in the `Approved` review decision.

**Files:**
- Modify: `tests/review_lifecycle.rs`

- [ ] **Step 1: Add the test**

```rust
// ---------------------------------------------------------------------------
// PR approved moves to Approved decision
// ---------------------------------------------------------------------------

#[test]
fn pr_approved_updates_review_decision() {
    let mut app = make_app();

    // Load PR as ReviewRequired first
    app.update(Message::PrsLoaded(
        PrListKind::Review,
        vec![make_pr(42, "org/app")],
    ));
    assert_eq!(
        app.review_prs()[0].review_decision,
        ReviewDecision::ReviewRequired
    );

    // Reload with approved decision
    let mut approved_pr = make_pr(42, "org/app");
    approved_pr.review_decision = ReviewDecision::Approved;
    app.update(Message::PrsLoaded(
        PrListKind::Review,
        vec![approved_pr],
    ));

    assert_eq!(
        app.review_prs()[0].review_decision,
        ReviewDecision::Approved,
        "PR decision should be updated to Approved after PrsLoaded"
    );
}
```

- [ ] **Step 2: Run to verify it passes**

```bash
cargo test --test review_lifecycle pr_approved_updates_review_decision
```

Expected: PASS.

- [ ] **Step 3: Commit**

```bash
git add tests/review_lifecycle.rs
git commit -m "test: PrsLoaded updates review decision to Approved"
```

---

### Task 7: Fix PR merged cleanup (TDD)

When a PR tracked by an active review agent disappears from `PrsLoaded`, the tmux window should be killed. Currently `handle_prs_loaded` just replaces the list without checking for missing agents.

**Files:**
- Modify: `tests/review_lifecycle.rs`
- Modify: `src/tui/mod.rs` (`handle_prs_loaded`, around line 3419)

- [ ] **Step 1: Write the failing test**

```rust
// ---------------------------------------------------------------------------
// PR merged: missing PR with active agent triggers cleanup
// ---------------------------------------------------------------------------

#[test]
fn prs_loaded_without_tracked_pr_triggers_cleanup() {
    let mut app = make_app();
    // Load PR #42 and register a review agent for it
    app.update(Message::PrsLoaded(
        PrListKind::Review,
        vec![make_pr(42, "org/app")],
    ));
    app.update(Message::ReviewAgentDispatched {
        github_repo: "org/app".to_string(),
        number: 42,
        tmux_window: "win-42".to_string(),
        worktree: "/wt/42".to_string(),
    });

    // PR #42 disappears from next fetch (it was merged)
    let cmds = app.update(Message::PrsLoaded(
        PrListKind::Review,
        vec![], // no PRs
    ));

    assert!(
        cmds.iter()
            .any(|c| matches!(c, Command::KillTmuxWindow { window } if window == "win-42")),
        "KillTmuxWindow should be emitted when a tracked PR disappears from the board"
    );
    assert!(
        app.review_agent_handle("org/app", 42).is_none(),
        "Agent handle should be removed after PR disappears"
    );
}
```

- [ ] **Step 2: Run to verify it fails**

```bash
cargo test --test review_lifecycle prs_loaded_without_tracked_pr_triggers_cleanup
```

Expected: FAIL — `KillTmuxWindow` is not emitted.

- [ ] **Step 3: Fix `handle_prs_loaded` in `src/tui/mod.rs`**

In `handle_prs_loaded`, after updating the list (after `self.clamp_review_selection()`), add cleanup for any tracked agents whose PRs are no longer present:

```rust
    fn handle_prs_loaded(
        &mut self,
        kind: PrListKind,
        prs: Vec<crate::models::ReviewPr>,
    ) -> Vec<Command> {
        let mut cmds = vec![Command::PersistPrs(kind, prs.clone())];
        if kind == PrListKind::Bot {
            self.security.dependabot.prs.set_prs(prs);
            self.security.dependabot.prs.loading = false;
            self.security.dependabot.prs.last_fetch = Some(Instant::now());
            self.security.dependabot.prs.last_error = None;
            self.clamp_dependabot_selection();
        } else {
            let list = self.review.list_mut(kind).unwrap();
            list.set_prs(prs);
            list.loading = false;
            list.last_fetch = Some(Instant::now());
            list.last_error = None;
            self.clamp_review_selection();

            // Clean up review agents whose PRs no longer appear in either list
            let pr_keys: std::collections::HashSet<crate::models::PrRef> = [
                PrListKind::Review,
                PrListKind::Authored,
            ]
            .iter()
            .flat_map(|k| self.review.list(*k).into_iter().flat_map(|l| l.prs.iter()))
            .map(|pr| crate::models::PrRef::new(pr.repo.clone(), pr.number))
            .collect();

            let gone: Vec<crate::models::PrRef> = self
                .review
                .review_agents
                .keys()
                .filter(|k| !pr_keys.contains(k))
                .cloned()
                .collect();

            for key in gone {
                cmds.extend(self.cleanup_review_board_pr(key.repo.clone(), key.number));
            }
        }
        cmds
    }
```

- [ ] **Step 4: Run to verify the test passes**

```bash
cargo test --test review_lifecycle prs_loaded_without_tracked_pr_triggers_cleanup
```

Expected: PASS.

- [ ] **Step 5: Run the full suite**

```bash
cargo test
```

Expected: all pass.

- [ ] **Step 6: Commit**

```bash
git add tests/review_lifecycle.rs src/tui/mod.rs
git commit -m "fix: clean up review agent when tracked PR disappears from board"
```

---

### Task 8: Test for fetch failure error state

Verify `PrsFetchFailed` sets the error state and preserves existing PRs.

**Files:**
- Modify: `tests/review_lifecycle.rs`

- [ ] **Step 1: Add the test**

```rust
// ---------------------------------------------------------------------------
// Fetch failure: error state set, existing PRs preserved
// ---------------------------------------------------------------------------

#[test]
fn pr_fetch_failed_sets_error_state_and_preserves_prs() {
    let mut app = make_app();
    // Load some PRs first
    app.update(Message::PrsLoaded(
        PrListKind::Review,
        vec![make_pr(42, "org/app")],
    ));
    assert_eq!(app.review_prs().len(), 1);

    // Simulate fetch failure
    app.update(Message::PrsFetchFailed(
        PrListKind::Review,
        "network timeout".to_string(),
    ));

    assert_eq!(
        app.last_review_error(),
        Some("network timeout"),
        "Error message should be stored on fetch failure"
    );
    assert!(!app.review_board_loading(), "loading flag should be cleared on failure");
    assert_eq!(
        app.review_prs().len(),
        1,
        "Existing PRs should be preserved on failure — board does not go blank"
    );
}
```

- [ ] **Step 2: Run to verify it passes**

```bash
cargo test --test review_lifecycle pr_fetch_failed_sets_error_state
```

Expected: PASS.

- [ ] **Step 3: Run the full suite one last time**

```bash
cargo test
```

Expected: all pass.

- [ ] **Step 4: Commit**

```bash
git add tests/review_lifecycle.rs
git commit -m "test: PR fetch failure sets error state and preserves existing PRs"
```

---

## Summary of Changes

| File | Change |
|------|--------|
| `tests/review_lifecycle.rs` | New — 8 integration tests covering tick wiring, dispatch, status update, approved PR, merged PR cleanup, fetch failure |
| `src/tui/mod.rs` | Bug fix 1: add `FetchSecurityAlerts` to `on_tick()` staleness check |
| `src/tui/mod.rs` | Bug fix 2: `handle_prs_loaded` cleans up review agents whose PRs are gone |
| `src/tui/mod.rs` | Add `review_agent_handle()` public accessor for test assertions |
