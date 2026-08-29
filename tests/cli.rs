#![allow(clippy::unwrap_used, clippy::expect_used)]
//! Integration tests for the CLI commands (plan, hook-*, verify-feed, repo).
//!
//! Each test spins up a fresh temp-file DB and invokes the compiled binary
//! via `std::process::Command`. Task creation is no longer exposed via the
//! CLI — tests seed tasks through the DB API directly.

use std::io::Write;
use std::path::Path;
use std::process::Command;
use tempfile::NamedTempFile;

use dispatch_tui::db::{CreateTaskRequest, Database, TaskCrud, TaskPatch, TaskRead};
use dispatch_tui::models::{SubStatus, TaskId, TaskStatus};

fn binary() -> Command {
    Command::new(env!("CARGO_BIN_EXE_dispatch"))
}

fn make_plan_file(title: &str, goal: &str) -> NamedTempFile {
    let mut f = NamedTempFile::new().unwrap();
    writeln!(
        f,
        "# {title} \u{2014} Implementation Plan\n\n**Goal:** {goal}"
    )
    .unwrap();
    f
}

/// Seed a backlog task directly via the DB API so we can drive the `plan`
/// and `hook-*` subcommands without the (removed) `create` subcommand.
async fn seed_task(db_path: &Path, title: &str) -> TaskId {
    let db = Database::open(db_path).await.unwrap();
    db.create_task(CreateTaskRequest {
        title,
        description: "",
        repo_path: "/tmp/test-repo",
        plan: None,
        status: TaskStatus::Backlog,
        base_branch: "main",
        epic_id: None,
        sort_order: None,
        tag: None,
        wrap_up_mode: None,
        auto_run_plan: false,
        phoenix: false,
    })
    .await
    .unwrap()
}

// ---------------------------------------------------------------------------
// Removed subcommands
//
// `create`, `list` and `update` were CLI task-mutation surfaces. Tasks are
// created and mutated via MCP; the installed Claude Code hooks forward to the
// dedicated `hook-*` subcommands. Each must now be rejected by clap outright,
// so a stale hook script or muscle-memory invocation fails loudly instead of
// silently doing something.
// ---------------------------------------------------------------------------

/// A `--db` path that deliberately does not exist. Every assertion in this
/// section is about clap rejecting argv *before* anything opens a database, so
/// pointing at a path no `Database::open` could succeed on makes that claim
/// structural rather than merely asserted — and saves the tests a temp file
/// none of them ever reads.
const UNOPENABLE_DB: &str = "/nonexistent-dir/dispatch-test.db";

/// Assert `subcommand` is not a recognised `dispatch` subcommand.
fn assert_subcommand_removed(subcommand: &str) {
    let out = binary()
        .args(["--db", UNOPENABLE_DB, subcommand])
        .output()
        .unwrap();
    assert!(
        !out.status.success(),
        "{subcommand} must no longer be a recognised subcommand"
    );
    let stderr = String::from_utf8_lossy(&out.stderr);
    assert!(
        stderr.contains("unrecognized subcommand") || stderr.contains("invalid value"),
        "expected clap rejection for {subcommand}, got stderr: {stderr}"
    );
}

#[test]
fn create_subcommand_removed() {
    assert_subcommand_removed("create");
}

#[test]
fn list_subcommand_removed() {
    assert_subcommand_removed("list");
}

#[test]
fn update_subcommand_removed() {
    assert_subcommand_removed("update");
}

// ---------------------------------------------------------------------------
// plan
// ---------------------------------------------------------------------------

#[tokio::test]
async fn plan_attaches_to_existing_task() {
    let db = NamedTempFile::new().unwrap();
    let db_path = db.path().to_str().unwrap();
    let id = seed_task(db.path(), "Plan Target").await;

    let attach_plan = make_plan_file("Detailed Plan", "Step by step.");

    let out = binary()
        .args([
            "--db",
            db_path,
            "plan",
            &id.0.to_string(),
            attach_plan.path().to_str().unwrap(),
        ])
        .output()
        .unwrap();
    assert!(
        out.status.success(),
        "stderr: {}",
        String::from_utf8_lossy(&out.stderr)
    );
    let stdout = String::from_utf8_lossy(&out.stdout);
    assert!(
        stdout.contains(&format!("Plan attached to task #{}", id.0)),
        "Expected confirmation, got: {stdout}"
    );

    // The plan must actually be persisted (routing through the service path
    // writes it), not just echoed to stdout.
    let reopened = Database::open(db.path()).await.unwrap();
    let task = reopened.get_task(id).await.unwrap().unwrap();
    assert!(
        task.plan_path.is_some(),
        "Expected plan_path to be persisted, got None"
    );
}

#[tokio::test]
async fn plan_nonexistent_task_fails() {
    let db = NamedTempFile::new().unwrap();
    let attach_plan = make_plan_file("Orphan Plan", "No task.");
    let out = binary()
        .args([
            "--db",
            db.path().to_str().unwrap(),
            "plan",
            "9999",
            attach_plan.path().to_str().unwrap(),
        ])
        .output()
        .unwrap();
    assert!(
        !out.status.success(),
        "Expected failure attaching a plan to a missing task"
    );
    let stderr = String::from_utf8_lossy(&out.stderr);
    assert!(
        stderr.contains("not found"),
        "Expected 'not found' error, got: {stderr}"
    );
}

#[tokio::test]
async fn plan_nonexistent_file_fails() {
    let db = NamedTempFile::new().unwrap();
    let out = binary()
        .args([
            "--db",
            db.path().to_str().unwrap(),
            "plan",
            "1",
            "/tmp/nonexistent-plan-99999.md",
        ])
        .output()
        .unwrap();
    assert!(
        !out.status.success(),
        "Expected failure for missing plan file"
    );
}

// ---------------------------------------------------------------------------
// fetch-reviews / fetch-security have been removed; users wire their own
// shell scripts as feed_command. These tests pin the removal so a future
// re-introduction has to opt back in deliberately.
// ---------------------------------------------------------------------------

#[test]
fn fetch_reviews_subcommand_removed() {
    assert_subcommand_removed("fetch-reviews");
}

#[test]
fn fetch_security_subcommand_removed() {
    assert_subcommand_removed("fetch-security");
}

// ---------------------------------------------------------------------------
// pr-gate
// ---------------------------------------------------------------------------

#[tokio::test]
async fn pr_gate_blocks_first_then_allows() {
    let db = NamedTempFile::new().unwrap();
    let db_path = db.path().to_str().unwrap();
    let id = seed_task(db.path(), "gate me").await;

    // First call: blocked (exit 2) with a reminder mentioning query_learnings.
    let first = binary()
        .args(["--db", db_path, "pr-gate", &id.0.to_string()])
        .output()
        .unwrap();
    assert_eq!(first.status.code(), Some(2));
    let stderr = String::from_utf8_lossy(&first.stderr);
    assert!(
        stderr.contains("query_learnings"),
        "expected reminder mentioning query_learnings, got: {stderr}"
    );

    // Second call: allowed (exit 0).
    let second = binary()
        .args(["--db", db_path, "pr-gate", &id.0.to_string()])
        .output()
        .unwrap();
    assert_eq!(second.status.code(), Some(0));
}

#[tokio::test]
async fn pr_gate_missing_task_allows() {
    let db = NamedTempFile::new().unwrap();
    let out = binary()
        .args(["--db", db.path().to_str().unwrap(), "pr-gate", "999999"])
        .output()
        .unwrap();
    assert_eq!(out.status.code(), Some(0));
}

// ---------------------------------------------------------------------------
// hook
// ---------------------------------------------------------------------------

#[tokio::test]
async fn hook_notification_sets_needs_input_sub_status() {
    let db = NamedTempFile::new().unwrap();
    let db_path = db.path().to_str().unwrap();
    let id = seed_task(db.path(), "Hook Test").await;

    let conn = Database::open(db.path()).await.unwrap();
    conn.patch_task(
        id,
        &TaskPatch::new()
            .status(TaskStatus::Running)
            .sub_status(SubStatus::Active),
    )
    .await
    .unwrap();
    drop(conn);

    let out = binary()
        .args(["--db", db_path, "hook", &id.0.to_string(), "notification"])
        .output()
        .unwrap();
    assert!(
        out.status.success(),
        "stderr: {}",
        String::from_utf8_lossy(&out.stderr)
    );

    let conn = Database::open(db.path()).await.unwrap();
    let task = conn.get_task(id).await.unwrap().unwrap();
    assert_eq!(task.sub_status, SubStatus::NeedsInput);
    assert!(
        task.last_notification_at.is_some(),
        "expected last_notification_at to be stamped"
    );
}

/// Move a seeded task into Running with the given sub_status, for the
/// `--kind` hook tests below.
async fn seed_running_task(db_path: &std::path::Path, title: &str, sub: SubStatus) -> TaskId {
    let id = seed_task(db_path, title).await;
    let conn = Database::open(db_path).await.unwrap();
    conn.patch_task(
        id,
        &TaskPatch::new().status(TaskStatus::Running).sub_status(sub),
    )
    .await
    .unwrap();
    if sub == SubStatus::NeedsInput {
        conn.patch_task(
            id,
            &TaskPatch::new().last_notification_at(Some(chrono::Utc::now())),
        )
        .await
        .unwrap();
    }
    id
}

/// `--kind permission_prompt` is a genuine block → needs_input + stamp.
#[tokio::test]
async fn hook_notification_kind_permission_prompt_sets_needs_input() {
    let db = NamedTempFile::new().unwrap();
    let db_path = db.path().to_str().unwrap();
    let id = seed_running_task(db.path(), "kind perm", SubStatus::Active).await;

    let out = binary()
        .args([
            "--db",
            db_path,
            "hook",
            &id.0.to_string(),
            "notification",
            "--kind",
            "permission_prompt",
        ])
        .output()
        .unwrap();
    assert!(
        out.status.success(),
        "stderr: {}",
        String::from_utf8_lossy(&out.stderr)
    );

    let conn = Database::open(db.path()).await.unwrap();
    let task = conn.get_task(id).await.unwrap().unwrap();
    assert_eq!(task.sub_status, SubStatus::NeedsInput);
    assert!(task.last_notification_at.is_some());
}

/// `--kind auth_success` is informational → no state change (stays active).
#[tokio::test]
async fn hook_notification_kind_auth_success_is_noop() {
    let db = NamedTempFile::new().unwrap();
    let db_path = db.path().to_str().unwrap();
    let id = seed_running_task(db.path(), "kind auth", SubStatus::Active).await;

    let out = binary()
        .args([
            "--db",
            db_path,
            "hook",
            &id.0.to_string(),
            "notification",
            "--kind",
            "auth_success",
        ])
        .output()
        .unwrap();
    assert!(
        out.status.success(),
        "stderr: {}",
        String::from_utf8_lossy(&out.stderr)
    );

    let conn = Database::open(db.path()).await.unwrap();
    let task = conn.get_task(id).await.unwrap().unwrap();
    assert_eq!(task.sub_status, SubStatus::Active);
    assert!(task.last_notification_at.is_none());
}

/// `--kind elicitation_complete` clears a prior block → back to active, ts null.
#[tokio::test]
async fn hook_notification_kind_elicitation_complete_clears() {
    let db = NamedTempFile::new().unwrap();
    let db_path = db.path().to_str().unwrap();
    let id = seed_running_task(db.path(), "kind elic", SubStatus::NeedsInput).await;

    let out = binary()
        .args([
            "--db",
            db_path,
            "hook",
            &id.0.to_string(),
            "notification",
            "--kind",
            "elicitation_complete",
        ])
        .output()
        .unwrap();
    assert!(
        out.status.success(),
        "stderr: {}",
        String::from_utf8_lossy(&out.stderr)
    );

    let conn = Database::open(db.path()).await.unwrap();
    let task = conn.get_task(id).await.unwrap().unwrap();
    assert_eq!(task.sub_status, SubStatus::Active);
    assert!(task.last_notification_at.is_none());
}

/// An unrecognised `--kind` value falls back to the raise path (needs_input)
/// and still exits 0 — never a hard error, so hooks stay resilient.
#[tokio::test]
async fn hook_notification_unknown_kind_falls_back_to_needs_input() {
    let db = NamedTempFile::new().unwrap();
    let db_path = db.path().to_str().unwrap();
    let id = seed_running_task(db.path(), "kind unknown", SubStatus::Active).await;

    let out = binary()
        .args([
            "--db",
            db_path,
            "hook",
            &id.0.to_string(),
            "notification",
            "--kind",
            "agent_completed",
        ])
        .output()
        .unwrap();
    assert!(
        out.status.success(),
        "stderr: {}",
        String::from_utf8_lossy(&out.stderr)
    );

    let conn = Database::open(db.path()).await.unwrap();
    let task = conn.get_task(id).await.unwrap().unwrap();
    assert_eq!(task.sub_status, SubStatus::NeedsInput);
    assert!(task.last_notification_at.is_some());
}

#[tokio::test]
async fn hook_unknown_kind_fails() {
    let db = NamedTempFile::new().unwrap();
    let id = seed_task(db.path(), "Hook Bad Kind").await;
    let out = binary()
        .args([
            "--db",
            db.path().to_str().unwrap(),
            "hook",
            &id.0.to_string(),
            "bogus",
        ])
        .output()
        .unwrap();
    assert!(!out.status.success(), "expected failure for invalid kind");
}

/// `dispatch hook` initialises its own tracing subscriber writing to
/// `<data_dir>/app.log` (docs/specs/observability.allium's `DbCallSlowWarning`
/// scope note: the `hook` one-shot subcommand is the second process, besides
/// the TUI, that persists slow-db-call warnings). Real lock contention isn't
/// reproducible deterministically here, so this only asserts the subscriber
/// is wired up — the file is created regardless of whether anything slow
/// happened to be logged this run.
#[tokio::test]
async fn hook_initialises_app_log_in_data_dir() {
    assert_hook_initialises_app_log("hook", &["notification"]).await;
}

/// Run `dispatch <subcommand> <task-id> <rest…>` against a fresh data dir and
/// assert it created `app.log` there. Shared by every `hook*` command's coverage
/// of the prologue they all go through.
async fn assert_hook_initialises_app_log(subcommand: &str, rest: &[&str]) {
    let dir = tempfile::tempdir().unwrap();
    let db_path = dir.path().join("test.db");
    let id = seed_task(&db_path, "Hook App Log Test").await;

    let task_id = id.0.to_string();
    let mut args = vec!["--db", db_path.to_str().unwrap(), subcommand, &task_id];
    args.extend_from_slice(rest);
    let out = binary().args(&args).output().unwrap();
    assert!(
        out.status.success(),
        "stderr: {}",
        String::from_utf8_lossy(&out.stderr)
    );

    let log_path = dir.path().join("app.log");
    assert!(
        log_path.exists(),
        "expected `dispatch {subcommand}` to initialise a tracing subscriber writing to {}",
        log_path.display()
    );
}

#[test]
fn hook_unknown_task_skips() {
    let db = NamedTempFile::new().unwrap();
    let out = binary()
        .args([
            "--db",
            db.path().to_str().unwrap(),
            "hook",
            "99999",
            "notification",
        ])
        .output()
        .unwrap();
    assert!(
        out.status.success(),
        "expected success (skip) for unknown task, stderr: {}",
        String::from_utf8_lossy(&out.stderr)
    );
    let stderr = String::from_utf8_lossy(&out.stderr);
    assert!(
        stderr.contains("not found"),
        "expected 'not found' message, got stderr: {stderr}"
    );
}

// ---------------------------------------------------------------------------
// hook-subagent
// ---------------------------------------------------------------------------

/// The `hook` twin of `hook_initialises_app_log_in_data_dir`. Every `hook*`
/// command installs the subscriber through the same shared prologue, so both
/// tests share one assertion body — that is what catches a prologue wired up on
/// only one path, and it makes covering a third hook command a one-liner.
#[tokio::test]
async fn hook_subagent_initialises_app_log_in_data_dir() {
    assert_hook_initialises_app_log("hook-subagent", &["clear"]).await;
}

#[tokio::test]
async fn hook_subagent_start_then_stop_round_trips() {
    let db = NamedTempFile::new().unwrap();
    let db_path = db.path().to_str().unwrap();
    let id = seed_running_task(db.path(), "Subagent Test", SubStatus::Active).await;

    let out = binary()
        .args([
            "--db",
            db_path,
            "hook-subagent",
            &id.0.to_string(),
            "start",
            "--agent-id",
            "a1",
            "--session-id",
            "s1",
        ])
        .output()
        .unwrap();
    assert!(
        out.status.success(),
        "stderr: {}",
        String::from_utf8_lossy(&out.stderr)
    );

    let conn = Database::open(db.path()).await.unwrap();
    let task = conn.get_task(id).await.unwrap().unwrap();
    assert_eq!(
        task.live_subagents, 1,
        "expected live_subagents to be 1 after start"
    );
    drop(conn);

    let out = binary()
        .args([
            "--db",
            db_path,
            "hook-subagent",
            &id.0.to_string(),
            "stop",
            "--agent-id",
            "a1",
            "--session-id",
            "s1",
        ])
        .output()
        .unwrap();
    assert!(
        out.status.success(),
        "stderr: {}",
        String::from_utf8_lossy(&out.stderr)
    );

    let conn = Database::open(db.path()).await.unwrap();
    let task = conn.get_task(id).await.unwrap().unwrap();
    assert_eq!(
        task.live_subagents, 0,
        "expected live_subagents to be 0 after the matching stop"
    );
}

#[tokio::test]
async fn hook_shell_start_then_stop_round_trips() {
    let db = NamedTempFile::new().unwrap();
    let db_path = db.path().to_str().unwrap();
    let id = seed_running_task(db.path(), "Shell Test", SubStatus::Active).await;

    let out = binary()
        .args([
            "--db",
            db_path,
            "hook-shell",
            &id.0.to_string(),
            "start",
            "--shell-id",
            "bash_1",
            "--session-id",
            "s1",
        ])
        .output()
        .unwrap();
    assert!(
        out.status.success(),
        "stderr: {}",
        String::from_utf8_lossy(&out.stderr)
    );

    let conn = Database::open(db.path()).await.unwrap();
    let task = conn.get_task(id).await.unwrap().unwrap();
    assert_eq!(
        task.live_shells, 1,
        "expected live_shells to be 1 after start"
    );
    drop(conn);

    let out = binary()
        .args([
            "--db",
            db_path,
            "hook-shell",
            &id.0.to_string(),
            "stop",
            "--shell-id",
            "bash_1",
            "--session-id",
            "s1",
        ])
        .output()
        .unwrap();
    assert!(
        out.status.success(),
        "stderr: {}",
        String::from_utf8_lossy(&out.stderr)
    );

    let conn = Database::open(db.path()).await.unwrap();
    let task = conn.get_task(id).await.unwrap().unwrap();
    assert_eq!(
        task.live_shells, 0,
        "expected live_shells to be 0 after the matching stop"
    );
}

/// `SessionStart` (the only producer of `hook-subagent … clear`) must void a
/// stale pending Stop, not apply it. A resume in particular keeps the task
/// Running deliberately — `handle_retry_resume` launches `claude --continue`
/// with no prompt — so draining here would land the task in Review with a live
/// agent in its window and no `UserPromptSubmit` to rescue it.
#[tokio::test]
async fn hook_subagent_clear_voids_a_pending_stop_without_flipping_to_review() {
    let db = NamedTempFile::new().unwrap();
    let db_path = db.path().to_str().unwrap();
    let id = seed_running_task(db.path(), "Subagent Clear No Drain", SubStatus::Active).await;

    for args in [
        vec![
            "hook-subagent",
            &id.0.to_string(),
            "start",
            "--agent-id",
            "a1",
            "--session-id",
            "s1",
        ],
        vec!["hook", &id.0.to_string(), "stop"],
    ] {
        let mut full = vec!["--db", db_path];
        full.extend(args.iter().copied());
        let out = binary().args(&full).output().unwrap();
        assert!(
            out.status.success(),
            "stderr: {}",
            String::from_utf8_lossy(&out.stderr)
        );
    }

    let conn = Database::open(db.path()).await.unwrap();
    let task = conn.get_task(id).await.unwrap().unwrap();
    assert!(task.stop_pending, "precondition: the Stop must be deferred");
    assert_eq!(task.status, TaskStatus::Running);
    drop(conn);

    let out = binary()
        .args(["--db", db_path, "hook-subagent", &id.0.to_string(), "clear"])
        .output()
        .unwrap();
    assert!(
        out.status.success(),
        "stderr: {}",
        String::from_utf8_lossy(&out.stderr)
    );

    let conn = Database::open(db.path()).await.unwrap();
    let task = conn.get_task(id).await.unwrap().unwrap();
    assert_eq!(
        task.status,
        TaskStatus::Running,
        "a fresh/resumed/cleared session must not apply the previous turn's Stop"
    );
    assert!(!task.stop_pending, "the stale pending Stop must be voided");
    assert_eq!(task.live_subagents, 0, "the entries must still be cleared");
}

#[tokio::test]
async fn hook_subagent_on_missing_task_exits_zero() {
    let db = NamedTempFile::new().unwrap();
    // A hook must never fail the agent's tool call just because the task is gone.
    let out = binary()
        .args([
            "--db",
            db.path().to_str().unwrap(),
            "hook-subagent",
            "9999",
            "start",
            "--agent-id",
            "a1",
            "--session-id",
            "s1",
        ])
        .output()
        .unwrap();
    assert!(
        out.status.success(),
        "stderr: {}",
        String::from_utf8_lossy(&out.stderr)
    );
}

/// Missing `--agent-id`/`--session-id` must no-op: exit 0 and no write at all,
/// not just "exit 0 for some other reason".
#[tokio::test]
async fn hook_subagent_missing_agent_id_is_a_silent_noop() {
    let db = NamedTempFile::new().unwrap();
    let db_path = db.path().to_str().unwrap();
    let id = seed_running_task(db.path(), "Subagent Missing Agent Id", SubStatus::Active).await;

    let out = binary()
        .args(["--db", db_path, "hook-subagent", &id.0.to_string(), "start"])
        .output()
        .unwrap();
    assert!(
        out.status.success(),
        "stderr: {}",
        String::from_utf8_lossy(&out.stderr)
    );

    let conn = Database::open(db.path()).await.unwrap();
    let task = conn.get_task(id).await.unwrap().unwrap();
    assert_eq!(
        task.live_subagents, 0,
        "a start missing --agent-id/--session-id must not write anything"
    );
}

/// Assert `subcommand` rejects `bogus` as its action argument with clap's own
/// invalid-value message, and that the message enumerates `valid`.
///
/// The action is parsed at the boundary by clap (`ValueEnum`), not by a
/// hand-rolled `match` inside the handler, so this happens before any database
/// is opened — hence [`UNOPENABLE_DB`].
fn assert_action_rejected_by_clap(subcommand: &str, bogus: &str, valid: &[&str]) {
    let out = binary()
        .args(["--db", UNOPENABLE_DB, subcommand, "1", bogus])
        .output()
        .unwrap();
    assert!(
        !out.status.success(),
        "{subcommand} must reject `{bogus}` as an action"
    );
    let stderr = String::from_utf8_lossy(&out.stderr);
    assert!(
        stderr.contains("invalid value") && stderr.contains("possible values"),
        "expected clap's invalid-value message, got stderr: {stderr}"
    );
    for v in valid {
        assert!(
            stderr.contains(v),
            "clap must list `{v}` as a possible value, got stderr: {stderr}"
        );
    }
}

#[test]
fn hook_subagent_unknown_action_is_rejected_by_clap() {
    assert_action_rejected_by_clap("hook-subagent", "bogus", &["start", "stop", "clear"]);
}

/// `clear` is a valid `hook-subagent` action but must not be one here — a
/// backgrounded shell has no SessionStart-driven clear, only session fencing.
#[test]
fn hook_shell_unknown_action_is_rejected_by_clap() {
    assert_action_rejected_by_clap("hook-shell", "clear", &["start", "stop"]);
}

// ---------------------------------------------------------------------------
// hook-peer-message
// ---------------------------------------------------------------------------

#[tokio::test]
async fn hook_peer_message_stamps_sender_and_resolved_target() {
    let db = NamedTempFile::new().unwrap();
    let db_path = db.path().to_str().unwrap();
    let sender = seed_task(db.path(), "Sender").await;
    let target = seed_task(db.path(), "Target").await;

    let out = binary()
        .args([
            "--db",
            db_path,
            "hook-peer-message",
            &sender.0.to_string(),
            "--target",
            &format!("task-{}", target.0),
            "--body",
            "hello sibling",
        ])
        .output()
        .unwrap();
    assert!(
        out.status.success(),
        "stderr: {}",
        String::from_utf8_lossy(&out.stderr)
    );

    let conn = Database::open(db.path()).await.unwrap();
    let sender_task = conn.get_task(sender).await.unwrap().unwrap();
    let target_task = conn.get_task(target).await.unwrap().unwrap();
    assert!(sender_task.last_peer_message_sent_at.is_some());
    assert!(target_task.last_peer_message_received_at.is_some());
}

#[tokio::test]
async fn hook_peer_message_appends_a_trajectory_entry_for_the_sender() {
    let dir = tempfile::tempdir().unwrap();
    let db_path = dir.path().join("test.db");
    let sender = seed_task(&db_path, "Sender").await;
    let target = seed_task(&db_path, "Target").await;

    let out = binary()
        .args([
            "--db",
            db_path.to_str().unwrap(),
            "hook-peer-message",
            &sender.0.to_string(),
            "--target",
            &format!("task-{}", target.0),
            "--body",
            "hello sibling",
        ])
        .output()
        .unwrap();
    assert!(
        out.status.success(),
        "stderr: {}",
        String::from_utf8_lossy(&out.stderr)
    );

    let trajectory_path = dir
        .path()
        .join("trajectories")
        .join(format!("{}.jsonl", sender.0));
    let content = std::fs::read_to_string(&trajectory_path).unwrap_or_else(|e| {
        panic!(
            "expected a trajectory entry at {}: {e}",
            trajectory_path.display()
        )
    });
    assert!(content.contains("SendMessage"), "got: {content}");
    assert!(content.contains("hello sibling"), "got: {content}");
}

#[tokio::test]
async fn hook_peer_message_on_missing_sender_exits_zero() {
    let db = NamedTempFile::new().unwrap();
    // A hook must never fail the agent's own tool call just because dispatch
    // can't find the task it's associated with.
    let out = binary()
        .args([
            "--db",
            db.path().to_str().unwrap(),
            "hook-peer-message",
            "9999",
            "--target",
            "task-1",
            "--body",
            "hi",
        ])
        .output()
        .unwrap();
    assert!(
        out.status.success(),
        "stderr: {}",
        String::from_utf8_lossy(&out.stderr)
    );
}

#[tokio::test]
async fn hook_peer_message_initialises_app_log_in_data_dir() {
    assert_hook_initialises_app_log("hook-peer-message", &["--target", "task-1", "--body", "hi"])
        .await;
}

// ---------------------------------------------------------------------------
// verify-feed
// ---------------------------------------------------------------------------

#[tokio::test]
async fn verify_feed_empty_array_fails() {
    // An empty feed almost always means the command is misconfigured
    // (e.g. fetch-cve.sh with no repos). Treat it as a failure so the
    // operator notices, rather than silently passing.
    let db = NamedTempFile::new().unwrap();
    let out = binary()
        .args([
            "--db",
            db.path().to_str().unwrap(),
            "verify-feed",
            "echo '[]'",
        ])
        .output()
        .unwrap();
    assert!(
        !out.status.success(),
        "Expected failure when feed command returns an empty array"
    );
    let stderr = String::from_utf8_lossy(&out.stderr);
    assert!(
        stderr.contains("0 items") || stderr.contains("empty"),
        "Expected empty-feed error message, got stderr: {stderr}"
    );
}

#[tokio::test]
async fn verify_feed_valid_items_succeeds() {
    let db = NamedTempFile::new().unwrap();
    let out = binary()
        .args([
            "--db",
            db.path().to_str().unwrap(),
            "verify-feed",
            r#"echo '[{"external_id":"x1","title":"T","description":"","status":"backlog","tag":"pr-review"}]'"#,
        ])
        .output()
        .unwrap();
    assert!(
        out.status.success(),
        "stderr: {}",
        String::from_utf8_lossy(&out.stderr)
    );
    let stdout = String::from_utf8_lossy(&out.stdout);
    assert!(
        stdout.contains("x1"),
        "Expected feed item id in output, got: {stdout}"
    );
    assert!(
        stdout.contains("TAG"),
        "Expected TAG header in output, got: {stdout}"
    );
    assert!(
        stdout.contains("pr-review"),
        "Expected tag value in output, got: {stdout}"
    );
}

#[tokio::test]
async fn verify_feed_reports_dropped_unrecognised_signal() {
    // feeds.allium (FeedItem.signals): an unrecognised signal is DROPPED, not
    // fatal — so the item still counts as valid and the exit status stays 0.
    // But the drop must be REPORTED. verify-feed is the only feed entry point
    // with no app.log sink, so without a stderr tracing subscriber the
    // deserialize_lenient_signals warning goes to a no-op dispatcher and a user
    // debugging a typo'd signal sees "✓ 1 valid item" with no hint anything was
    // discarded — the tool whose whole job is printing evidence losing the
    // evidence.
    let db = NamedTempFile::new().unwrap();
    let out = binary()
        // Hermetic: the report must not depend on the developer's shell. Before
        // the filter was pinned to a fixed `warn`, RUST_LOG=dispatch_tui=error
        // suppressed the warning and this test failed.
        .env_remove("RUST_LOG")
        .args([
            "--db",
            db.path().to_str().unwrap(),
            "verify-feed",
            r#"echo '[{"external_id":"x1","title":"T","description":"","status":"backlog","tag":"pr-review","signals":["reviewed","bogus"]}]'"#,
        ])
        .output()
        .unwrap();
    assert!(
        out.status.success(),
        "A dropped signal is non-fatal per feeds.allium; stderr: {}",
        String::from_utf8_lossy(&out.stderr)
    );
    let stdout = String::from_utf8_lossy(&out.stdout);
    assert!(
        stdout.contains("x1") && stdout.contains("1 valid item"),
        "Expected the item to still count as valid, got stdout: {stdout}"
    );
    let stderr = String::from_utf8_lossy(&out.stderr);
    assert!(
        stderr.contains("dropping unrecognised feed signal"),
        "Expected the dropped-signal warning on stderr, got: {stderr}"
    );
    assert!(
        stderr.contains("bogus"),
        "Expected the offending signal value on stderr, got: {stderr}"
    );
}

#[tokio::test]
async fn verify_feed_recognised_signals_produce_no_warning() {
    // Guards against a subscriber configured so noisily that a clean feed nags
    // on every run — the warning must fire for a dropped signal only.
    let db = NamedTempFile::new().unwrap();
    let out = binary()
        .env_remove("RUST_LOG")
        .args([
            "--db",
            db.path().to_str().unwrap(),
            "verify-feed",
            r#"echo '[{"external_id":"x1","title":"T","description":"","status":"backlog","tag":"pr-review","signals":["reviewed"]}]'"#,
        ])
        .output()
        .unwrap();
    assert!(
        out.status.success(),
        "stderr: {}",
        String::from_utf8_lossy(&out.stderr)
    );
    let stderr = String::from_utf8_lossy(&out.stderr);
    assert!(
        !stderr.contains("dropping unrecognised"),
        "A fully-recognised signal list must not warn, got stderr: {stderr}"
    );
}

#[tokio::test]
async fn verify_feed_missing_tag_fails() {
    let db = NamedTempFile::new().unwrap();
    let out = binary()
        .args([
            "--db",
            db.path().to_str().unwrap(),
            "verify-feed",
            r#"echo '[{"external_id":"x1","title":"T","description":"","status":"backlog"}]'"#,
        ])
        .output()
        .unwrap();
    assert!(
        !out.status.success(),
        "Expected failure when feed item is missing tag"
    );
    let stderr = String::from_utf8_lossy(&out.stderr);
    assert!(
        stderr.contains("failed to parse") && stderr.contains("tag"),
        "Expected parse error mentioning tag, got stderr: {stderr}"
    );
}

#[tokio::test]
async fn verify_feed_invalid_tag_fails() {
    let db = NamedTempFile::new().unwrap();
    let out = binary()
        .args([
            "--db",
            db.path().to_str().unwrap(),
            "verify-feed",
            r#"echo '[{"external_id":"x1","title":"T","description":"","status":"backlog","tag":"nonsense"}]'"#,
        ])
        .output()
        .unwrap();
    assert!(
        !out.status.success(),
        "Expected failure when feed item has unknown tag value"
    );
    let stderr = String::from_utf8_lossy(&out.stderr);
    assert!(
        stderr.contains("failed to parse"),
        "Expected parse error, got stderr: {stderr}"
    );
}

#[tokio::test]
async fn verify_feed_invalid_json_fails() {
    let db = NamedTempFile::new().unwrap();
    let out = binary()
        .args([
            "--db",
            db.path().to_str().unwrap(),
            "verify-feed",
            "echo 'not json'",
        ])
        .output()
        .unwrap();
    assert!(
        !out.status.success(),
        "Expected failure for invalid JSON output"
    );
    let stderr = String::from_utf8_lossy(&out.stderr);
    assert!(
        stderr.contains("failed to parse"),
        "Expected parse error, got stderr: {stderr}"
    );
}

#[tokio::test]
async fn verify_feed_surfaces_stderr_written_on_zero_exit() {
    // feeds.allium: FeedCommandStderrOnSuccess. A command that writes to
    // stderr internally but still exits 0 must have that stderr visible in
    // verify-feed's own output, not silently discarded — this is the third
    // exec of a feed command and the one debugging path a user has left
    // once app.log points them at a failure.
    let db = NamedTempFile::new().unwrap();
    let out = binary()
        .args([
            "--db",
            db.path().to_str().unwrap(),
            "verify-feed",
            "echo 'boom' >&2; printf '[]'",
        ])
        .output()
        .unwrap();
    let stderr = String::from_utf8_lossy(&out.stderr);
    assert!(
        stderr.contains("boom"),
        "Expected the command's stderr to be surfaced, got stderr: {stderr}"
    );
}

#[tokio::test]
async fn verify_feed_command_failure_exits_nonzero() {
    let db = NamedTempFile::new().unwrap();
    let out = binary()
        .args(["--db", db.path().to_str().unwrap(), "verify-feed", "exit 7"])
        .output()
        .unwrap();
    assert!(
        !out.status.success(),
        "Expected failure when feed command exits non-zero"
    );
}

// ---------------------------------------------------------------------------
// prune-repo-paths
// ---------------------------------------------------------------------------

#[tokio::test]
async fn prune_repo_paths_removes_nonexistent_paths() {
    let db = NamedTempFile::new().unwrap();
    let db_path = db.path().to_str().unwrap();
    let bin = env!("CARGO_BIN_EXE_dispatch");

    // A path that exists on disk
    let real_dir = tempfile::tempdir().unwrap();
    let real_path = real_dir.path().to_str().unwrap();

    // A path that does not exist
    let fake_path = "/tmp/dispatch-test-nonexistent-path-99999";

    // Seed both paths into the DB via the repo sub-command (set-verify creates the row)
    std::process::Command::new(bin)
        .args(["--db", db_path, "repo", "set-verify", real_path, "echo ok"])
        .status()
        .unwrap();
    std::process::Command::new(bin)
        .args(["--db", db_path, "repo", "set-verify", fake_path, "echo ok"])
        .status()
        .unwrap();

    // Run prune
    let out = std::process::Command::new(bin)
        .args(["--db", db_path, "prune-repo-paths"])
        .output()
        .unwrap();
    assert!(
        out.status.success(),
        "stderr: {}",
        String::from_utf8_lossy(&out.stderr)
    );
    let stdout = String::from_utf8_lossy(&out.stdout);
    assert!(
        stdout.contains(fake_path),
        "expected removed path in output, got: {stdout}"
    );
    assert!(
        stdout.contains("1 path(s) removed"),
        "expected removal count in output, got: {stdout}"
    );

    // The real path should still be in the DB
    let list_out = std::process::Command::new(bin)
        .args(["--db", db_path, "repo", "list"])
        .output()
        .unwrap();
    let list_stdout = String::from_utf8_lossy(&list_out.stdout);
    assert!(
        list_stdout.contains(real_path),
        "real path must remain after prune, got: {list_stdout}"
    );
    assert!(
        !list_stdout.contains(fake_path),
        "fake path must be removed after prune, got: {list_stdout}"
    );
}

#[tokio::test]
async fn prune_repo_paths_empty_db_succeeds() {
    let db = NamedTempFile::new().unwrap();
    let out = std::process::Command::new(env!("CARGO_BIN_EXE_dispatch"))
        .args(["--db", db.path().to_str().unwrap(), "prune-repo-paths"])
        .output()
        .unwrap();
    assert!(
        out.status.success(),
        "stderr: {}",
        String::from_utf8_lossy(&out.stderr)
    );
    let stdout = String::from_utf8_lossy(&out.stdout);
    assert!(
        stdout.contains("0 path(s) removed"),
        "expected zero removals for empty DB, got: {stdout}"
    );
}

// ---------------------------------------------------------------------------
// repo set-verify / clear-verify / list
// ---------------------------------------------------------------------------

#[test]
fn dispatch_repo_set_verify_writes_command() {
    let tmp = tempfile::NamedTempFile::new().unwrap();
    let db_arg = tmp.path().to_str().unwrap();
    let bin = env!("CARGO_BIN_EXE_dispatch");

    let out = std::process::Command::new(bin)
        .args(["--db", db_arg, "repo", "set-verify", "/r", "cargo test"])
        .output()
        .unwrap();
    assert!(
        out.status.success(),
        "stderr: {}",
        String::from_utf8_lossy(&out.stderr)
    );

    let out = std::process::Command::new(bin)
        .args(["--db", db_arg, "repo", "list"])
        .output()
        .unwrap();
    let stdout = String::from_utf8_lossy(&out.stdout);
    assert!(stdout.contains("/r"), "path must appear in list");
    assert!(stdout.contains("cargo test"), "command must appear in list");
}

#[test]
fn dispatch_repo_clear_verify_removes_command() {
    let tmp = tempfile::NamedTempFile::new().unwrap();
    let db_arg = tmp.path().to_str().unwrap();
    let bin = env!("CARGO_BIN_EXE_dispatch");

    let _ = std::process::Command::new(bin)
        .args(["--db", db_arg, "repo", "set-verify", "/r", "cargo test"])
        .status()
        .unwrap();
    let status = std::process::Command::new(bin)
        .args(["--db", db_arg, "repo", "clear-verify", "/r"])
        .status()
        .unwrap();
    assert!(status.success());

    let out = std::process::Command::new(bin)
        .args(["--db", db_arg, "repo", "list"])
        .output()
        .unwrap();
    let stdout = String::from_utf8_lossy(&out.stdout);
    assert!(
        stdout.contains("/r"),
        "path row must still appear after clear"
    );
    assert!(!stdout.contains("cargo test"), "command must be cleared");
}

#[test]
fn dispatch_repo_set_verify_rejects_newline() {
    let tmp = tempfile::NamedTempFile::new().unwrap();
    let db_arg = tmp.path().to_str().unwrap();
    let bin = env!("CARGO_BIN_EXE_dispatch");

    let out = std::process::Command::new(bin)
        .args(["--db", db_arg, "repo", "set-verify", "/r", "a\nb"])
        .output()
        .unwrap();
    assert!(
        !out.status.success(),
        "expected exit failure for newline command"
    );
    let stderr = String::from_utf8_lossy(&out.stderr);
    assert!(
        stderr.to_lowercase().contains("newline"),
        "expected newline error in stderr: {stderr}"
    );
}

#[test]
fn dispatch_repo_set_verify_expands_tilde_in_path() {
    let tmp = tempfile::NamedTempFile::new().unwrap();
    let db_arg = tmp.path().to_str().unwrap();
    let bin = env!("CARGO_BIN_EXE_dispatch");

    // set-verify with a tilde-prefixed path
    let out = std::process::Command::new(bin)
        .args(["--db", db_arg, "repo", "set-verify", "~/r", "cargo test"])
        .output()
        .unwrap();
    assert!(
        out.status.success(),
        "stderr: {}",
        String::from_utf8_lossy(&out.stderr)
    );

    // list should show the expanded path, not the literal `~/r`
    let out = std::process::Command::new(bin)
        .args(["--db", db_arg, "repo", "list"])
        .output()
        .unwrap();
    let stdout = String::from_utf8_lossy(&out.stdout);
    let home = std::env::var("HOME").unwrap();
    let expanded = format!("{home}/r");
    assert!(
        stdout.contains(&expanded),
        "expected expanded path {expanded} in list output, got: {stdout}"
    );
    assert!(
        !stdout.contains("~/r"),
        "tilde path must NOT appear verbatim in list output, got: {stdout}"
    );
}

// ---------------------------------------------------------------------------
// doctor subcommand removed (self-diagnosis surface retired)
// ---------------------------------------------------------------------------

/// The `doctor` self-diagnosis surface was retired. Its only remediation worth
/// keeping — pointing git at `.githooks` — is now the documented one-liner
/// `git config core.hooksPath .githooks` in CLAUDE.md's "First-time setup".
#[test]
fn doctor_subcommand_removed() {
    let db = NamedTempFile::new().unwrap();
    let out = binary()
        .args(["--db", db.path().to_str().unwrap(), "doctor"])
        .output()
        .unwrap();
    assert!(
        !out.status.success(),
        "doctor must no longer be a recognised subcommand, stdout: {}",
        String::from_utf8_lossy(&out.stdout)
    );
    let stderr = String::from_utf8_lossy(&out.stderr);
    assert!(
        stderr.contains("unrecognized subcommand") || stderr.contains("unexpected argument"),
        "expected clap to reject `doctor` as an unknown subcommand, stderr: {stderr}"
    );
}

// ---------------------------------------------------------------------------
// toggle-agent-tree-pane
// ---------------------------------------------------------------------------

#[tokio::test]
async fn toggle_agent_tree_pane_never_fails_without_a_real_tmux_session() {
    // This process has no real tmux session (or, if it happens to run inside
    // one, "task-999999999" doesn't name a real window in it either way).
    // The command must swallow the resulting tmux failure and exit 0 —
    // best-effort, matching the companion pane's decorative, non-critical
    // role everywhere else it's touched.
    let db = NamedTempFile::new().unwrap();
    let out = binary()
        .args([
            "--db",
            db.path().to_str().unwrap(),
            "toggle-agent-tree-pane",
            "task-999999999",
        ])
        .output()
        .unwrap();
    assert_eq!(
        out.status.code(),
        Some(0),
        "stderr: {}",
        String::from_utf8_lossy(&out.stderr)
    );
}

// ---------------------------------------------------------------------------
// repo status / repo sync (docs/specs/repo-sync.allium)
// ---------------------------------------------------------------------------

/// Seed a repo path into the DB via `repo set-verify`, which creates the row.
fn seed_repo_path(db_arg: &str, path: &str) {
    let status = std::process::Command::new(env!("CARGO_BIN_EXE_dispatch"))
        .args(["--db", db_arg, "repo", "set-verify", path, "true"])
        .status()
        .unwrap();
    assert!(status.success(), "seeding {path} should succeed");
}

// surface-provides.RepoStatusCli — the command exists and is read-only.
#[test]
fn repo_status_reports_no_paths_for_an_empty_db() {
    let tmp = tempfile::NamedTempFile::new().unwrap();
    let out = binary()
        .args(["--db", tmp.path().to_str().unwrap(), "repo", "status"])
        .output()
        .unwrap();
    assert!(
        out.status.success(),
        "stderr: {}",
        String::from_utf8_lossy(&out.stderr)
    );
    let stdout = String::from_utf8_lossy(&out.stdout);
    assert!(
        stdout.contains("No repo paths configured."),
        "got: {stdout}"
    );
}

// @guarantee UnmeasuredRowsShowNoCounts + UnmeasuredIsNeverPresentedAsClean: a
// repository that cannot be measured shows no ahead/behind figures and reports
// its fetch error instead.
#[test]
fn repo_status_row_for_an_unmeasurable_repo_shows_no_counts() {
    let tmp = tempfile::NamedTempFile::new().unwrap();
    let db_arg = tmp.path().to_str().unwrap();
    let missing = "/tmp/dispatch-test-not-a-repo-77777";
    seed_repo_path(db_arg, missing);

    let out = binary()
        .args(["--db", db_arg, "repo", "status"])
        .output()
        .unwrap();
    assert!(
        out.status.success(),
        "measuring is read-only and never fails the command; stderr: {}",
        String::from_utf8_lossy(&out.stderr)
    );
    let stdout = String::from_utf8_lossy(&out.stdout);
    assert!(stdout.contains(missing), "the row names the repo: {stdout}");
    assert!(
        stdout.contains("unknown"),
        "an unmeasurable repo reads as unknown, never as in sync: {stdout}"
    );
    assert!(
        !stdout.contains('\u{2191}') && !stdout.contains('\u{2193}'),
        "no ahead/behind figures may be quoted: {stdout}"
    );
}

// @guarantee FetchesUnlessSuppressed — the default fetches, so a repository
// whose fetch fails reports that error.
#[test]
fn repo_status_fetches_by_default_and_reports_the_fetch_error() {
    let tmp = tempfile::NamedTempFile::new().unwrap();
    let db_arg = tmp.path().to_str().unwrap();
    let missing = "/tmp/dispatch-test-not-a-repo-77778";
    seed_repo_path(db_arg, missing);

    let out = binary()
        .args(["--db", db_arg, "repo", "status"])
        .output()
        .unwrap();
    let stdout = String::from_utf8_lossy(&out.stdout);
    assert!(
        stdout.to_lowercase().contains("fetch"),
        "expected the fetch failure in the row, got: {stdout}"
    );
}

// @guarantee FetchesUnlessSuppressed — --no-fetch skips the fetch, so there is
// no fetch error to report.
#[test]
fn repo_status_no_fetch_skips_the_fetch() {
    let tmp = tempfile::NamedTempFile::new().unwrap();
    let db_arg = tmp.path().to_str().unwrap();
    let missing = "/tmp/dispatch-test-not-a-repo-77779";
    seed_repo_path(db_arg, missing);

    let out = binary()
        .args(["--db", db_arg, "repo", "status", "--no-fetch"])
        .output()
        .unwrap();
    assert!(out.status.success());
    let stdout = String::from_utf8_lossy(&out.stdout);
    assert!(stdout.contains(missing), "got: {stdout}");
    assert!(
        !stdout.to_lowercase().contains("fetch"),
        "--no-fetch performs no fetch, so no fetch error exists: {stdout}"
    );
}

// rule-failure.SyncRepoViaCli.1 — `requires: targets.count > 0`.
#[test]
fn repo_sync_fails_when_there_are_no_saved_paths() {
    let tmp = tempfile::NamedTempFile::new().unwrap();
    let out = binary()
        .args(["--db", tmp.path().to_str().unwrap(), "repo", "sync"])
        .output()
        .unwrap();
    assert!(
        !out.status.success(),
        "no targets means nothing to sync, which is an error"
    );
}

#[test]
fn repo_sync_fails_for_an_unknown_path() {
    let tmp = tempfile::NamedTempFile::new().unwrap();
    let db_arg = tmp.path().to_str().unwrap();
    seed_repo_path(db_arg, "/tmp/dispatch-test-saved-77780");

    let out = binary()
        .args([
            "--db",
            db_arg,
            "repo",
            "sync",
            "/tmp/dispatch-test-never-saved-77781",
        ])
        .output()
        .unwrap();
    assert!(
        !out.status.success(),
        "a path that is not a saved repo path is not a target"
    );
}

// @guarantee FailureIsVisibleInTheExitCode + EveryTargetAttempted: every target
// is attempted and the exit code is non-zero when any of them failed.
#[test]
fn repo_sync_attempts_every_target_and_fails_the_exit_code() {
    let tmp = tempfile::NamedTempFile::new().unwrap();
    let db_arg = tmp.path().to_str().unwrap();
    let a = "/tmp/dispatch-test-not-a-repo-77782";
    let b = "/tmp/dispatch-test-not-a-repo-77783";
    seed_repo_path(db_arg, a);
    seed_repo_path(db_arg, b);

    let out = binary()
        .args(["--db", db_arg, "repo", "sync"])
        .output()
        .unwrap();
    assert!(
        !out.status.success(),
        "a failed target must fail the command"
    );
    let combined = format!(
        "{}{}",
        String::from_utf8_lossy(&out.stdout),
        String::from_utf8_lossy(&out.stderr)
    );
    assert!(
        combined.contains(a) && combined.contains(b),
        "one failure must not abandon the rest, got: {combined}"
    );
}

// ---------------------------------------------------------------------------
// statusline
// ---------------------------------------------------------------------------

const STATUS_PAYLOAD: &[u8] =
    br#"{"rate_limits":{"five_hour":{"used_percentage":5.0,"resets_at":9}}}"#;

/// Run the decorator the way Claude Code does — payload on stdin — and return
/// its output.
fn run_statusline(db: &Path, snapshot: &Path, chain: Option<&str>) -> std::process::Output {
    let mut command = binary();
    command.args([
        "--db",
        db.to_str().unwrap(),
        "statusline",
        "--snapshot",
        snapshot.to_str().unwrap(),
    ]);
    if let Some(chain) = chain {
        command.args(["--chain", chain]);
    }
    let mut child = command
        .stdin(std::process::Stdio::piped())
        .stdout(std::process::Stdio::piped())
        .spawn()
        .unwrap();
    child
        .stdin
        .take()
        .unwrap()
        .write_all(STATUS_PAYLOAD)
        .unwrap();
    child.wait_with_output().unwrap()
}

/// The decorator runs several times a second in every dispatch-spawned Claude
/// session, so any database work there would be pure waste. The module keeps no
/// `Database` import, but that is a source property; this asserts the observable
/// one — running the subcommand brings no database into existence. See
/// docs/specs/dispatch.allium: StatusLineDecorator
/// (`@guarantee NeverReadsOrWritesTheDatabase`).
#[test]
fn statusline_creates_no_database_file() {
    let tmp = tempfile::tempdir().unwrap();
    let db_path = tmp.path().join("tasks.db");
    let snapshot = tmp.path().join("rate-limits.json");

    let out = run_statusline(&db_path, &snapshot, None);

    assert!(out.status.success(), "the decorator must always exit 0");
    assert!(
        snapshot.exists(),
        "the snapshot must have been published, or this proves nothing about the DB"
    );
    assert!(
        !db_path.exists(),
        "the statusline subcommand must not create a database file"
    );
}

/// The decorator does no async work, so it must start none of the machinery for
/// it — a multi-thread tokio runtime would spin up one worker per core plus the
/// reactor on every 300 ms debounce tick, in every session. See
/// docs/specs/dispatch.allium: StatusLineDecorator (`@guarantee
/// StartsNoAsyncRuntime`).
///
/// Observed rather than asserted from source: the chained command's parent *is*
/// the `dispatch` process, so `/proc/$PPID/task` is that process's live thread
/// count while the decorator waits on the chain. `run_bounded` accounts for at
/// most three threads there (the stdin writer plus the two output drains) on top
/// of `main`, hence the bound of 4 — an inequality, because the stdin writer may
/// already have finished. Add a background thread to `run_bounded` and this
/// bound needs revisiting.
#[test]
#[cfg(target_os = "linux")]
fn statusline_starts_no_worker_thread_pool() {
    let tmp = tempfile::tempdir().unwrap();
    let out = run_statusline(
        &tmp.path().join("tasks.db"),
        &tmp.path().join("rate-limits.json"),
        Some("ls /proc/$PPID/task | wc -l"),
    );

    assert!(out.status.success(), "the decorator must always exit 0");
    let printed = String::from_utf8_lossy(&out.stdout);
    let threads: usize = printed
        .trim()
        .parse()
        .unwrap_or_else(|e| panic!("expected a thread count, got {printed:?}: {e}"));
    assert!(
        threads <= 4,
        "statusline must run without a worker-thread pool, saw {threads} threads"
    );
}

// ---------------------------------------------------------------------------
// caller-headers
// ---------------------------------------------------------------------------

/// The `headersHelper` runs on every MCP session start and reconnect. Its whole
/// contract is one line of JSON on stdout and exit 0; these lock it at the
/// process level, where the unit tests on `resolve_headers_for_path` cannot see
/// it. See docs/specs/mcp-task-tools.allium (CreateTaskViaMcp guidance).
#[test]
fn caller_headers_emits_session_header_outside_a_worktree() {
    let tmp = tempfile::tempdir().unwrap();
    let out = binary()
        .arg("caller-headers")
        .current_dir(tmp.path())
        .output()
        .unwrap();
    assert!(out.status.success(), "caller-headers must exit 0");
    let v: serde_json::Value = serde_json::from_slice(&out.stdout).unwrap();
    assert_eq!(v["X-Caller-Kind"], "session");
}

/// The helper has one answer. Standing inside a worktree — the shape it used to
/// read a task identity out of — changes nothing, because Claude Code never runs
/// it there: a user-global helper runs from Claude Code's own configuration
/// directory. A dispatched agent's identity comes from its launch instead.
#[test]
fn caller_headers_emits_session_even_inside_a_worktree() {
    let tmp = tempfile::tempdir().unwrap();
    let wt = tmp.path().join(".worktrees").join("3840-some-slug");
    std::fs::create_dir_all(&wt).unwrap();
    let out = binary()
        .arg("caller-headers")
        .current_dir(&wt)
        .output()
        .unwrap();
    assert!(out.status.success(), "caller-headers must exit 0");
    let v: serde_json::Value = serde_json::from_slice(&out.stdout).unwrap();
    assert_eq!(v["X-Caller-Kind"], "session");
    assert!(
        v.get("X-Caller-Task-Id").is_none(),
        "the helper must never claim a task identity: {v}"
    );
}
