//! Integration tests for the CLI commands (list, update, create).
//!
//! Each test spins up a fresh temp-file DB and invokes the compiled binary
//! via `std::process::Command`.

use std::io::Write;
use std::process::Command;
use tempfile::NamedTempFile;

fn binary() -> Command {
    Command::new(env!("CARGO_BIN_EXE_task-orchestrator"))
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

// ---------------------------------------------------------------------------
// list
// ---------------------------------------------------------------------------

#[test]
fn list_empty_db() {
    let db = NamedTempFile::new().unwrap();
    let out = binary()
        .args(["--db", db.path().to_str().unwrap(), "list"])
        .output()
        .unwrap();
    assert!(out.status.success());
    let stdout = String::from_utf8_lossy(&out.stdout);
    assert!(
        stdout.contains("No tasks found."),
        "Expected 'No tasks found.', got: {stdout}"
    );
}

#[test]
fn list_unknown_status_fails() {
    let db = NamedTempFile::new().unwrap();
    let out = binary()
        .args([
            "--db",
            db.path().to_str().unwrap(),
            "list",
            "--status",
            "bogus",
        ])
        .output()
        .unwrap();
    assert!(!out.status.success(), "Expected failure for unknown status");
}

// ---------------------------------------------------------------------------
// create
// ---------------------------------------------------------------------------

#[test]
fn create_from_plan() {
    let db = NamedTempFile::new().unwrap();
    let plan = make_plan_file("My Feature", "Implement X.");

    let out = binary()
        .args([
            "--db",
            db.path().to_str().unwrap(),
            "create",
            "--from-plan",
            plan.path().to_str().unwrap(),
            "--repo-path",
            "/tmp/test-repo",
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
        stdout.contains("Created task #"),
        "Expected task creation, got: {stdout}"
    );
    assert!(
        stdout.contains("My Feature"),
        "Expected title in output, got: {stdout}"
    );
    assert!(
        stdout.contains("[ready]"),
        "Expected [ready] status, got: {stdout}"
    );
}

#[test]
fn create_then_list() {
    let db = NamedTempFile::new().unwrap();
    let plan = make_plan_file("Auth Bug Fix", "Fix login.");
    let db_path = db.path().to_str().unwrap();

    let create_out = binary()
        .args([
            "--db",
            db_path,
            "create",
            "--from-plan",
            plan.path().to_str().unwrap(),
            "--repo-path",
            "/tmp/test-repo",
        ])
        .output()
        .unwrap();
    assert!(
        create_out.status.success(),
        "create failed: {}",
        String::from_utf8_lossy(&create_out.stderr)
    );

    let out = binary()
        .args(["--db", db_path, "list"])
        .output()
        .unwrap();
    assert!(out.status.success());
    let stdout = String::from_utf8_lossy(&out.stdout);
    assert!(
        stdout.contains("Auth Bug Fix"),
        "Expected task title in list, got: {stdout}"
    );
    assert!(
        stdout.contains("ready"),
        "Expected ready status in list, got: {stdout}"
    );
}

#[test]
fn list_filter_by_status() {
    let db = NamedTempFile::new().unwrap();
    let plan = make_plan_file("Filter Test", "Filter tasks.");
    let db_path = db.path().to_str().unwrap();

    binary()
        .args([
            "--db",
            db_path,
            "create",
            "--from-plan",
            plan.path().to_str().unwrap(),
            "--repo-path",
            "/tmp/test-repo",
        ])
        .output()
        .unwrap();

    // list --status ready -> shows the task
    let out = binary()
        .args(["--db", db_path, "list", "--status", "ready"])
        .output()
        .unwrap();
    assert!(out.status.success());
    let stdout = String::from_utf8_lossy(&out.stdout);
    assert!(
        stdout.contains("Filter Test"),
        "Expected task in ready list, got: {stdout}"
    );

    // list --status backlog -> empty
    let out = binary()
        .args(["--db", db_path, "list", "--status", "backlog"])
        .output()
        .unwrap();
    assert!(out.status.success());
    let stdout = String::from_utf8_lossy(&out.stdout);
    assert!(
        stdout.contains("No tasks found."),
        "Expected no backlog tasks, got: {stdout}"
    );
}

#[test]
fn create_idempotent_for_same_plan() {
    let db = NamedTempFile::new().unwrap();
    let plan = make_plan_file("Idempotent Task", "No duplicates.");
    let db_path = db.path().to_str().unwrap();
    let plan_path = plan.path().to_str().unwrap();

    // First create
    binary()
        .args([
            "--db", db_path, "create", "--from-plan", plan_path, "--repo-path", "/tmp/test-repo",
        ])
        .output()
        .unwrap();

    // Second create with same plan
    let out = binary()
        .args([
            "--db", db_path, "create", "--from-plan", plan_path, "--repo-path", "/tmp/test-repo",
        ])
        .output()
        .unwrap();
    assert!(out.status.success());
    let stdout = String::from_utf8_lossy(&out.stdout);
    assert!(
        stdout.contains("already exists"),
        "Expected idempotency message, got: {stdout}"
    );

    // Only one task in list
    let out = binary()
        .args(["--db", db_path, "list"])
        .output()
        .unwrap();
    let stdout = String::from_utf8_lossy(&out.stdout);
    let task_lines: Vec<_> = stdout
        .lines()
        .filter(|l| l.contains("Idempotent Task"))
        .collect();
    assert_eq!(
        task_lines.len(),
        1,
        "Expected exactly one task, got: {stdout}"
    );
}

#[test]
fn create_with_title_and_description_overrides() {
    let db = NamedTempFile::new().unwrap();
    let plan = make_plan_file("Original Title", "Original goal.");
    let db_path = db.path().to_str().unwrap();

    let out = binary()
        .args([
            "--db",
            db_path,
            "create",
            "--from-plan",
            plan.path().to_str().unwrap(),
            "--repo-path",
            "/tmp/test-repo",
            "--title",
            "Custom Title",
            "--description",
            "Custom description",
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
        stdout.contains("Custom Title"),
        "Expected custom title, got: {stdout}"
    );

    // Verify in list output
    let out = binary()
        .args(["--db", db_path, "list"])
        .output()
        .unwrap();
    let stdout = String::from_utf8_lossy(&out.stdout);
    assert!(
        stdout.contains("Custom Title"),
        "Expected custom title in list, got: {stdout}"
    );
}

#[test]
fn create_missing_plan_file_fails() {
    let db = NamedTempFile::new().unwrap();
    let out = binary()
        .args([
            "--db",
            db.path().to_str().unwrap(),
            "create",
            "--from-plan",
            "/tmp/nonexistent-plan-file-12345.md",
            "--repo-path",
            "/tmp/test-repo",
        ])
        .output()
        .unwrap();
    assert!(
        !out.status.success(),
        "Expected failure for missing plan file"
    );
}

// ---------------------------------------------------------------------------
// update
// ---------------------------------------------------------------------------

#[test]
fn update_changes_status() {
    let db = NamedTempFile::new().unwrap();
    let plan = make_plan_file("Update Test", "Test update.");
    let db_path = db.path().to_str().unwrap();

    binary()
        .args([
            "--db",
            db_path,
            "create",
            "--from-plan",
            plan.path().to_str().unwrap(),
            "--repo-path",
            "/tmp/test-repo",
        ])
        .output()
        .unwrap();

    // Update to running
    let out = binary()
        .args(["--db", db_path, "update", "1", "running"])
        .output()
        .unwrap();
    assert!(
        out.status.success(),
        "stderr: {}",
        String::from_utf8_lossy(&out.stderr)
    );
    let stdout = String::from_utf8_lossy(&out.stdout);
    assert!(
        stdout.contains("Task 1 updated to running"),
        "Expected update confirmation, got: {stdout}"
    );

    // Verify via list
    let out = binary()
        .args(["--db", db_path, "list", "--status", "running"])
        .output()
        .unwrap();
    let stdout = String::from_utf8_lossy(&out.stdout);
    assert!(
        stdout.contains("Update Test"),
        "Expected task in running list, got: {stdout}"
    );
}

#[test]
fn update_unknown_status_fails() {
    let db = NamedTempFile::new().unwrap();
    let plan = make_plan_file("Error Test", "Test errors.");
    let db_path = db.path().to_str().unwrap();

    binary()
        .args([
            "--db",
            db_path,
            "create",
            "--from-plan",
            plan.path().to_str().unwrap(),
            "--repo-path",
            "/tmp/test-repo",
        ])
        .output()
        .unwrap();

    let out = binary()
        .args(["--db", db_path, "update", "1", "bogus-status"])
        .output()
        .unwrap();
    assert!(
        !out.status.success(),
        "Expected failure for unknown status"
    );
}
