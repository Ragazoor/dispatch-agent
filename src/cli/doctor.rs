use serde::Serialize;

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum CheckKind {
    Worktrees,
    Sessions,
    Hooks,
}

impl CheckKind {
    pub fn as_str(self) -> &'static str {
        match self {
            Self::Worktrees => "worktrees",
            Self::Sessions => "sessions",
            Self::Hooks => "hooks",
        }
    }
}

#[derive(Debug, Clone, PartialEq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum FindingStatus {
    Ok,
    Warn,
    Error,
}

impl FindingStatus {
    pub fn as_str(&self) -> &'static str {
        match self {
            Self::Ok => "ok",
            Self::Warn => "warn",
            Self::Error => "error",
        }
    }
    pub fn is_problem(&self) -> bool {
        matches!(self, Self::Warn | Self::Error)
    }
}

#[derive(Debug, Clone, Serialize)]
pub struct Finding {
    pub check: CheckKind,
    pub status: FindingStatus,
    pub target: String,
    pub message: String,
    pub repair_available: bool,
}

/// Format findings as human-readable lines.
pub fn format_human(findings: &[Finding]) -> String {
    findings
        .iter()
        .map(|f| {
            let status = match f.status {
                FindingStatus::Ok => "ok   ",
                FindingStatus::Warn => "warn ",
                FindingStatus::Error => "error",
            };
            format!(
                "{}  {}  {}  {}",
                status,
                f.check.as_str(),
                f.target,
                f.message
            )
        })
        .collect::<Vec<_>>()
        .join("\n")
}

/// Serialize findings to a JSON array string.
pub fn format_json(findings: &[Finding]) -> String {
    serde_json::to_string_pretty(findings).unwrap_or_else(|_| "[]".to_string())
}

/// Returns true if any finding is warn or error.
pub fn has_problems(findings: &[Finding]) -> bool {
    findings.iter().any(|f| f.status.is_problem())
}

/// Check worktrees: compare task.worktree DB values against disk, and scan
/// .worktrees/ directories for paths that don't match any DB row.
pub fn check_worktrees(tasks: &[crate::models::Task], repo_paths: &[String]) -> Vec<Finding> {
    let mut findings = Vec::new();

    // Collect set of all worktree paths known to the DB (tilde-expanded for comparison).
    let db_worktrees: std::collections::HashSet<String> = tasks
        .iter()
        .filter_map(|t| t.worktree.as_ref().map(|w| crate::models::expand_tilde(w)))
        .collect();

    // DB orphans: task.worktree set but path doesn't exist on disk.
    for task in tasks {
        let Some(ref wt) = task.worktree else {
            continue;
        };
        let path = crate::models::expand_tilde(wt);
        if !std::path::Path::new(&path).exists() {
            findings.push(Finding {
                check: CheckKind::Worktrees,
                status: FindingStatus::Error,
                target: wt.clone(),
                message: format!(
                    "task #{} claims worktree but path does not exist",
                    task.id.0
                ),
                repair_available: true,
            });
        }
    }

    // Disk orphans: directories under .worktrees/ with no matching DB row.
    for repo in repo_paths {
        let expanded = crate::models::expand_tilde(repo);
        let worktrees_dir = std::path::Path::new(&expanded).join(".worktrees");
        let entries = match std::fs::read_dir(&worktrees_dir) {
            Ok(entries) => entries,
            // A missing .worktrees/ directory is the healthy common case — skip it.
            Err(err) if err.kind() == std::io::ErrorKind::NotFound => continue,
            // Any other error (permission denied, not-a-directory, …) is a real
            // problem the user should see rather than have silently swallowed.
            Err(err) => {
                findings.push(Finding {
                    check: CheckKind::Worktrees,
                    status: FindingStatus::Warn,
                    target: worktrees_dir.to_string_lossy().to_string(),
                    message: format!("could not read worktrees directory: {err}"),
                    repair_available: false,
                });
                continue;
            }
        };
        for entry in entries {
            // A per-entry read error means the directory listing was interrupted;
            // surface it instead of dropping the entry silently.
            let entry = match entry {
                Ok(entry) => entry,
                Err(err) => {
                    findings.push(Finding {
                        check: CheckKind::Worktrees,
                        status: FindingStatus::Warn,
                        target: worktrees_dir.to_string_lossy().to_string(),
                        message: format!("could not read worktrees directory entry: {err}"),
                        repair_available: false,
                    });
                    continue;
                }
            };
            let p = entry.path();
            if !p.is_dir() {
                continue;
            }
            let path_str = p.to_string_lossy().to_string();
            if !db_worktrees.contains(&path_str) {
                findings.push(Finding {
                    check: CheckKind::Worktrees,
                    status: FindingStatus::Warn,
                    target: path_str,
                    message: "directory exists on disk but no task row has this worktree path"
                        .to_string(),
                    repair_available: true,
                });
            }
        }
    }

    findings
}

/// Check sessions: compare tasks.tmux_window against live tmux windows.
///
/// - Task has `tmux_window` set with status running/review but window is gone → error
/// - Task has `tmux_window` set with status done/archived and window still exists → warn
pub fn check_sessions(
    tasks: &[crate::models::Task],
    runner: &dyn crate::process::ProcessRunner,
) -> Vec<Finding> {
    use crate::models::TaskStatus;

    let live_windows: std::collections::HashSet<String> =
        crate::tmux::list_all_window_names(runner)
            .unwrap_or_default()
            .into_iter()
            .collect();

    let mut findings = Vec::new();

    for task in tasks {
        let Some(ref window) = task.tmux_window else {
            continue;
        };
        let window_alive = live_windows.contains(window.as_str());
        let task_active = matches!(task.status, TaskStatus::Running | TaskStatus::Review);
        let task_terminal = matches!(task.status, TaskStatus::Done | TaskStatus::Archived);

        if task_active && !window_alive {
            findings.push(Finding {
                check: CheckKind::Sessions,
                status: FindingStatus::Error,
                target: window.clone(),
                message: format!(
                    "task #{} ({}) claims window '{}' but it no longer exists",
                    task.id.0, task.status, window
                ),
                repair_available: true,
            });
        } else if task_terminal && window_alive {
            findings.push(Finding {
                check: CheckKind::Sessions,
                status: FindingStatus::Warn,
                target: window.clone(),
                message: format!(
                    "tmux window '{}' is still alive but task #{} is {}",
                    window, task.id.0, task.status
                ),
                repair_available: true,
            });
        }
    }

    findings
}

/// Check hooks: verify git config core.hooksPath = ".githooks" for each known repo.
pub fn check_hooks(
    repo_paths: &[String],
    runner: &dyn crate::process::ProcessRunner,
) -> Vec<Finding> {
    let mut findings = Vec::new();

    for repo in repo_paths {
        let expanded = crate::models::expand_tilde(repo);
        let result = runner.run(
            "git",
            &["-C", &expanded, "config", "--get", "core.hooksPath"],
        );
        let current_value = match result {
            Ok(ref output) if output.status.success() => {
                String::from_utf8_lossy(&output.stdout).trim().to_string()
            }
            _ => String::new(),
        };

        if current_value == ".githooks" {
            findings.push(Finding {
                check: CheckKind::Hooks,
                status: FindingStatus::Ok,
                target: repo.clone(),
                message: "core.hooksPath = .githooks".to_string(),
                repair_available: false,
            });
        } else {
            let detail = if current_value.is_empty() {
                "core.hooksPath is not set".to_string()
            } else {
                format!("core.hooksPath = '{current_value}', expected '.githooks'")
            };
            findings.push(Finding {
                check: CheckKind::Hooks,
                status: FindingStatus::Warn,
                target: repo.clone(),
                message: format!("{detail}; pre-push hook will not run"),
                repair_available: true,
            });
        }
    }

    findings
}

/// Repair: set core.hooksPath = .githooks for a repo.
pub fn repair_hooks_set_path(
    repo_path: &str,
    runner: &dyn crate::process::ProcessRunner,
) -> anyhow::Result<()> {
    let expanded = crate::models::expand_tilde(repo_path);
    let output = runner.run(
        "git",
        &[
            "-C",
            &expanded,
            "config",
            "--local",
            "core.hooksPath",
            ".githooks",
        ],
    )?;
    if !output.status.success() {
        let stderr = String::from_utf8_lossy(&output.stderr);
        anyhow::bail!("git config failed: {}", stderr.trim());
    }
    Ok(())
}

/// Repair: kill a stale tmux window that belongs to a done/archived task.
pub fn repair_sessions_stale_window(
    window_name: &str,
    runner: &dyn crate::process::ProcessRunner,
) -> anyhow::Result<()> {
    crate::tmux::kill_window(window_name, runner)
}

/// Repair: clear the `tmux_window` field on a task whose window no longer exists.
pub async fn repair_sessions_orphan_db_row(
    task_id: crate::models::TaskId,
    database: &dyn crate::db::TaskCrud,
) -> anyhow::Result<()> {
    database
        .patch_task(task_id, &crate::db::TaskPatch::new().tmux_window(None))
        .await
}

/// Repair: clear the `worktree` and `tmux_window` fields on a task whose
/// worktree path no longer exists on disk.
pub async fn repair_worktrees_orphan_db_row(
    task_id: crate::models::TaskId,
    database: &dyn crate::db::TaskCrud,
) -> anyhow::Result<()> {
    database
        .patch_task(
            task_id,
            &crate::db::TaskPatch::new().worktree(None).tmux_window(None),
        )
        .await
}

/// Repair: remove an orphaned worktree directory from disk.
///
/// `repo_path` is the root of the git repo; `worktree_path` is the absolute
/// path to the `.worktrees/task-N` directory. Both are tilde-expanded before use.
pub fn repair_worktrees_remove(
    repo_path: &str,
    worktree_path: &str,
    runner: &dyn crate::process::ProcessRunner,
) -> anyhow::Result<()> {
    let expanded_repo = crate::models::expand_tilde(repo_path);
    let expanded_wt = crate::models::expand_tilde(worktree_path);
    let output = runner.run(
        "git",
        &[
            "-C",
            &expanded_repo,
            "worktree",
            "remove",
            "--force",
            &expanded_wt,
        ],
    )?;
    if !output.status.success() {
        let stderr = String::from_utf8_lossy(&output.stderr);
        anyhow::bail!("git worktree remove failed: {}", stderr.trim());
    }
    Ok(())
}

#[cfg(test)]
#[allow(clippy::unwrap_used, clippy::expect_used)]
mod tests {
    use super::*;

    fn ok_finding() -> Finding {
        Finding {
            check: CheckKind::Hooks,
            status: FindingStatus::Ok,
            target: "/repo".to_string(),
            message: "core.hooksPath = .githooks".to_string(),
            repair_available: false,
        }
    }

    fn warn_finding() -> Finding {
        Finding {
            check: CheckKind::Worktrees,
            status: FindingStatus::Warn,
            target: "/repo/.worktrees/task-5".to_string(),
            message: "directory exists but no matching DB row".to_string(),
            repair_available: true,
        }
    }

    #[test]
    fn finding_status_as_str() {
        assert_eq!(FindingStatus::Ok.as_str(), "ok");
        assert_eq!(FindingStatus::Warn.as_str(), "warn");
        assert_eq!(FindingStatus::Error.as_str(), "error");
    }

    #[test]
    fn finding_status_is_problem() {
        assert!(!FindingStatus::Ok.is_problem());
        assert!(FindingStatus::Warn.is_problem());
        assert!(FindingStatus::Error.is_problem());
    }

    #[test]
    fn format_human_ok_line() {
        let out = format_human(&[ok_finding()]);
        assert!(
            out.starts_with("ok   "),
            "expected 'ok   ' prefix, got: {out}"
        );
        assert!(out.contains("hooks"), "expected check name, got: {out}");
        assert!(out.contains("/repo"), "expected target, got: {out}");
    }

    #[test]
    fn format_human_warn_line() {
        let out = format_human(&[warn_finding()]);
        assert!(
            out.starts_with("warn "),
            "expected 'warn ' prefix, got: {out}"
        );
    }

    #[test]
    fn format_human_multiple_lines() {
        let out = format_human(&[ok_finding(), warn_finding()]);
        assert_eq!(out.lines().count(), 2);
    }

    #[test]
    fn format_json_is_valid_array() {
        let json = format_json(&[ok_finding(), warn_finding()]);
        let parsed: Vec<serde_json::Value> = serde_json::from_str(&json).unwrap();
        assert_eq!(parsed.len(), 2);
        assert_eq!(parsed[0]["check"], "hooks");
        assert_eq!(parsed[0]["status"], "ok");
        assert_eq!(parsed[0]["repair_available"], false);
        assert_eq!(parsed[1]["status"], "warn");
        assert_eq!(parsed[1]["repair_available"], true);
    }

    #[test]
    fn has_problems_false_when_all_ok() {
        assert!(!has_problems(&[ok_finding(), ok_finding()]));
    }

    #[test]
    fn has_problems_true_when_any_warn() {
        assert!(has_problems(&[ok_finding(), warn_finding()]));
    }

    #[test]
    fn has_problems_empty_slice_is_false() {
        assert!(!has_problems(&[]));
    }

    mod sessions_tests {
        use super::*;
        use crate::process::MockProcessRunner;

        fn running_task(id: i64, window: &str) -> crate::models::Task {
            crate::models::Task {
                id: crate::models::TaskId(id),
                title: format!("task {id}"),
                description: String::new(),
                repo_path: "/repo".to_string(),
                status: crate::models::TaskStatus::Running,
                worktree: Some(format!("/repo/.worktrees/task-{id}")),
                tmux_window: Some(window.to_string()),
                plan_path: None,
                epic_id: None,
                sub_status: crate::models::SubStatus::Active,
                url: None,
                tag: None,
                sort_order: None,
                base_branch: "main".into(),
                external_id: None,
                labels: vec![],
                created_at: chrono::Utc::now(),
                updated_at: chrono::Utc::now(),
                last_pre_tool_use_at: None,
                last_notification_at: None,
                wrap_up_mode: None,
                auto_run_plan: false,
            }
        }

        fn done_task(id: i64) -> crate::models::Task {
            let mut t = running_task(id, &format!("task-{id}"));
            t.status = crate::models::TaskStatus::Done;
            t.sub_status = crate::models::SubStatus::None;
            t
        }

        fn archived_task(id: i64) -> crate::models::Task {
            let mut t = running_task(id, &format!("task-{id}"));
            t.status = crate::models::TaskStatus::Archived;
            t.sub_status = crate::models::SubStatus::None;
            t
        }

        #[test]
        fn stale_db_claim_when_window_missing() {
            let mock =
                MockProcessRunner::new(vec![MockProcessRunner::ok_with_stdout(b"dispatch\n")]);
            let findings = check_sessions(&[running_task(10, "task-10")], &mock);
            assert_eq!(findings.len(), 1);
            assert_eq!(findings[0].status, FindingStatus::Error);
            assert_eq!(findings[0].check, CheckKind::Sessions);
            assert!(findings[0].repair_available);
            assert!(findings[0].target.contains("task-10"));
        }

        #[test]
        fn ok_when_window_exists() {
            let mock = MockProcessRunner::new(vec![MockProcessRunner::ok_with_stdout(
                b"dispatch\ntask-7\n",
            )]);
            let findings = check_sessions(&[running_task(7, "task-7")], &mock);
            let errors: Vec<_> = findings
                .iter()
                .filter(|f| f.status == FindingStatus::Error)
                .collect();
            assert!(
                errors.is_empty(),
                "expected no errors when window exists: {errors:?}"
            );
        }

        #[test]
        fn stale_live_window_for_done_task() {
            let mock = MockProcessRunner::new(vec![MockProcessRunner::ok_with_stdout(
                b"dispatch\ntask-3\n",
            )]);
            let findings = check_sessions(&[done_task(3)], &mock);
            let warns: Vec<_> = findings
                .iter()
                .filter(|f| f.status == FindingStatus::Warn)
                .collect();
            assert_eq!(
                warns.len(),
                1,
                "expected one warn for stale live window, got: {findings:?}"
            );
            assert!(warns[0].repair_available);
        }

        #[test]
        fn stale_live_window_for_archived_task() {
            let mock = MockProcessRunner::new(vec![MockProcessRunner::ok_with_stdout(b"task-8\n")]);
            let findings = check_sessions(&[archived_task(8)], &mock);
            let warns: Vec<_> = findings
                .iter()
                .filter(|f| f.status == FindingStatus::Warn)
                .collect();
            assert_eq!(
                warns.len(),
                1,
                "expected one warn for archived task with live window: {findings:?}"
            );
        }

        #[test]
        fn no_stale_window_warn_for_backlog_task_without_window() {
            let mock =
                MockProcessRunner::new(vec![MockProcessRunner::ok_with_stdout(b"dispatch\n")]);
            let mut task = running_task(6, "task-6");
            task.status = crate::models::TaskStatus::Backlog;
            task.sub_status = crate::models::SubStatus::None;
            task.tmux_window = None;
            let findings = check_sessions(&[task], &mock);
            assert!(
                findings.is_empty(),
                "expected no findings for backlog task without window: {findings:?}"
            );
        }

        #[test]
        fn no_findings_for_task_without_tmux_window() {
            let mock =
                MockProcessRunner::new(vec![MockProcessRunner::ok_with_stdout(b"dispatch\n")]);
            let mut task = running_task(5, "task-5");
            task.tmux_window = None;
            let findings = check_sessions(&[task], &mock);
            assert!(findings.is_empty());
        }

        #[test]
        fn stale_db_claim_is_error_when_tmux_absent() {
            let mock = MockProcessRunner::new(vec![MockProcessRunner::fail("no server")]);
            let findings = check_sessions(&[running_task(1, "task-1")], &mock);
            assert_eq!(findings.len(), 1);
            assert_eq!(findings[0].status, FindingStatus::Error);
        }
    }

    mod worktrees_tests {
        use super::*;
        use tempfile::TempDir;

        fn task_with_worktree(id: i64, worktree: &str) -> crate::models::Task {
            crate::models::Task {
                id: crate::models::TaskId(id),
                title: format!("task {id}"),
                description: String::new(),
                repo_path: "/repo".to_string(),
                status: crate::models::TaskStatus::Running,
                worktree: Some(worktree.to_string()),
                tmux_window: Some(format!("task-{id}")),
                plan_path: None,
                epic_id: None,
                sub_status: crate::models::SubStatus::Active,
                url: None,
                tag: None,
                sort_order: None,
                base_branch: "main".into(),
                external_id: None,
                labels: vec![],
                created_at: chrono::Utc::now(),
                updated_at: chrono::Utc::now(),
                last_pre_tool_use_at: None,
                last_notification_at: None,
                wrap_up_mode: None,
                auto_run_plan: false,
            }
        }

        #[test]
        fn db_orphan_when_path_missing() {
            let findings = check_worktrees(
                &[task_with_worktree(1, "/nonexistent/task-1")],
                &["/repo".to_string()],
            );
            assert_eq!(findings.len(), 1);
            assert_eq!(findings[0].status, FindingStatus::Error);
            assert_eq!(findings[0].check, CheckKind::Worktrees);
            assert!(findings[0].repair_available);
            assert!(findings[0].target.contains("task-1"));
        }

        #[test]
        fn ok_when_worktree_path_exists() {
            let dir = TempDir::new().unwrap();
            let path = dir.path().to_str().unwrap().to_string();
            let task = task_with_worktree(2, &path);
            let findings = check_worktrees(&[task], &["/repo".to_string()]);
            let errors: Vec<_> = findings
                .iter()
                .filter(|f| f.status == FindingStatus::Error)
                .collect();
            assert!(
                errors.is_empty(),
                "expected no errors for existing path, got: {errors:?}"
            );
        }

        #[test]
        fn disk_orphan_when_dir_not_in_db() {
            let repo_dir = TempDir::new().unwrap();
            let worktrees_dir = repo_dir.path().join(".worktrees");
            std::fs::create_dir_all(&worktrees_dir).unwrap();
            let orphan = worktrees_dir.join("task-99");
            std::fs::create_dir_all(&orphan).unwrap();

            let findings = check_worktrees(&[], &[repo_dir.path().to_str().unwrap().to_string()]);
            assert_eq!(findings.len(), 1, "expected one finding for disk orphan");
            assert_eq!(findings[0].status, FindingStatus::Warn);
            assert!(findings[0].repair_available);
            assert!(findings[0].target.contains("task-99"));
        }

        #[test]
        fn no_findings_for_task_without_worktree() {
            let mut task = task_with_worktree(3, "/some/path");
            task.worktree = None;
            let findings = check_worktrees(&[task], &[]);
            assert!(findings.is_empty());
        }

        #[test]
        fn no_finding_when_worktrees_dir_absent() {
            // A repo with no .worktrees/ directory is the common, healthy case:
            // read_dir returns NotFound, which must be skipped silently.
            let repo_dir = TempDir::new().unwrap();
            let findings = check_worktrees(&[], &[repo_dir.path().to_str().unwrap().to_string()]);
            assert!(
                findings.is_empty(),
                "missing .worktrees dir must not produce a finding, got: {findings:?}"
            );
        }

        #[test]
        fn warn_when_worktrees_path_unreadable() {
            // A non-NotFound read_dir error (here: .worktrees is a file, not a
            // directory) must surface as a warning rather than be swallowed.
            let repo_dir = TempDir::new().unwrap();
            let worktrees_path = repo_dir.path().join(".worktrees");
            std::fs::write(&worktrees_path, b"not a directory").unwrap();

            let findings = check_worktrees(&[], &[repo_dir.path().to_str().unwrap().to_string()]);
            assert_eq!(findings.len(), 1, "expected one finding, got: {findings:?}");
            assert_eq!(findings[0].status, FindingStatus::Warn);
            assert_eq!(findings[0].check, CheckKind::Worktrees);
            assert!(!findings[0].repair_available);
            assert!(findings[0].target.contains(".worktrees"));
        }
    }

    mod hooks_tests {
        use super::*;
        use crate::process::MockProcessRunner;

        #[test]
        fn ok_when_hooks_path_is_githooks() {
            let mock =
                MockProcessRunner::new(vec![MockProcessRunner::ok_with_stdout(b".githooks\n")]);
            let findings = check_hooks(&["/repo".to_string()], &mock);
            assert_eq!(findings.len(), 1);
            assert_eq!(findings[0].status, FindingStatus::Ok);
            assert!(!findings[0].repair_available);
        }

        #[test]
        fn warn_when_hooks_path_is_different() {
            let mock = MockProcessRunner::new(vec![MockProcessRunner::ok_with_stdout(b".husky\n")]);
            let findings = check_hooks(&["/repo".to_string()], &mock);
            assert_eq!(findings.len(), 1);
            assert_eq!(findings[0].status, FindingStatus::Warn);
            assert!(findings[0].repair_available);
        }

        #[test]
        fn warn_when_hooks_path_not_set() {
            let mock = MockProcessRunner::new(vec![MockProcessRunner::fail("")]);
            let findings = check_hooks(&["/repo".to_string()], &mock);
            assert_eq!(findings.len(), 1);
            assert_eq!(findings[0].status, FindingStatus::Warn);
            assert!(findings[0].repair_available);
        }

        #[test]
        fn issues_correct_git_args() {
            let mock =
                MockProcessRunner::new(vec![MockProcessRunner::ok_with_stdout(b".githooks\n")]);
            let _ = check_hooks(&["/my/repo".to_string()], &mock);
            let calls = mock.recorded_calls();
            assert_eq!(calls.len(), 1);
            assert_eq!(calls[0].0, "git");
            assert_eq!(
                calls[0].1,
                vec!["-C", "/my/repo", "config", "--get", "core.hooksPath"]
            );
        }

        #[test]
        fn checks_each_repo_independently() {
            let mock = MockProcessRunner::new(vec![
                MockProcessRunner::ok_with_stdout(b".githooks\n"),
                MockProcessRunner::fail(""),
            ]);
            let findings = check_hooks(&["/repo-a".to_string(), "/repo-b".to_string()], &mock);
            assert_eq!(findings.len(), 2);
            assert_eq!(findings[0].status, FindingStatus::Ok);
            assert_eq!(findings[1].status, FindingStatus::Warn);
        }
    }

    mod repair_tests {
        use super::*;
        use crate::db::{TaskCrud, TaskRead};
        use crate::process::MockProcessRunner;

        async fn task_in_db(db: &crate::db::Database) -> crate::models::TaskId {
            db.create_task(crate::db::CreateTaskRequest {
                title: "test task",
                description: "",
                repo_path: "/repo",
                plan: None,
                status: crate::models::TaskStatus::Running,
                base_branch: "main",
                epic_id: None,
                sort_order: None,
                tag: None,
                wrap_up_mode: None,
            })
            .await
            .unwrap()
        }

        #[tokio::test]
        async fn repair_sessions_stale_window_kills_window() {
            let mock = MockProcessRunner::new(vec![MockProcessRunner::ok()]);
            repair_sessions_stale_window("task-7", &mock).unwrap();
            let calls = mock.recorded_calls();
            assert_eq!(calls.len(), 1);
            assert_eq!(calls[0].0, "tmux");
            assert!(
                calls[0].1.contains(&"task-7".to_string()),
                "expected window name in tmux args, got: {:?}",
                calls[0].1
            );
        }

        #[tokio::test]
        async fn repair_sessions_orphan_db_row_clears_tmux_window() {
            let db = crate::db::Database::open_in_memory().await.unwrap();
            let task_id = task_in_db(&db).await;
            db.patch_task(
                task_id,
                &crate::db::TaskPatch::new().tmux_window(Some("task-42")),
            )
            .await
            .unwrap();
            let before = db.get_task(task_id).await.unwrap().unwrap();
            assert_eq!(before.tmux_window.as_deref(), Some("task-42"));

            repair_sessions_orphan_db_row(task_id, &db).await.unwrap();

            let after = db.get_task(task_id).await.unwrap().unwrap();
            assert!(
                after.tmux_window.is_none(),
                "expected tmux_window cleared, got: {:?}",
                after.tmux_window
            );
        }

        #[tokio::test]
        async fn repair_worktrees_orphan_db_row_clears_worktree_and_tmux_window() {
            let db = crate::db::Database::open_in_memory().await.unwrap();
            let task_id = task_in_db(&db).await;
            db.patch_task(
                task_id,
                &crate::db::TaskPatch::new()
                    .worktree(Some("/repo/.worktrees/task-42"))
                    .tmux_window(Some("task-42")),
            )
            .await
            .unwrap();
            let before = db.get_task(task_id).await.unwrap().unwrap();
            assert!(before.worktree.is_some());
            assert!(before.tmux_window.is_some());

            repair_worktrees_orphan_db_row(task_id, &db).await.unwrap();

            let after = db.get_task(task_id).await.unwrap().unwrap();
            assert!(
                after.worktree.is_none(),
                "expected worktree cleared, got: {:?}",
                after.worktree
            );
            assert!(
                after.tmux_window.is_none(),
                "expected tmux_window cleared, got: {:?}",
                after.tmux_window
            );
        }

        #[test]
        fn repair_hooks_issues_correct_git_config_command() {
            let mock = MockProcessRunner::new(vec![MockProcessRunner::ok()]);
            repair_hooks_set_path("/my/repo", &mock).unwrap();
            let calls = mock.recorded_calls();
            assert_eq!(calls.len(), 1);
            assert_eq!(calls[0].0, "git");
            assert_eq!(
                calls[0].1,
                vec![
                    "-C",
                    "/my/repo",
                    "config",
                    "--local",
                    "core.hooksPath",
                    ".githooks"
                ]
            );
        }

        #[test]
        fn repair_worktrees_remove_issues_correct_git_worktree_command() {
            let mock = MockProcessRunner::new(vec![MockProcessRunner::ok()]);
            repair_worktrees_remove("/repo", "/repo/.worktrees/task-5", &mock).unwrap();
            let calls = mock.recorded_calls();
            assert_eq!(calls.len(), 1);
            assert_eq!(calls[0].0, "git");
            assert_eq!(
                calls[0].1,
                vec![
                    "-C",
                    "/repo",
                    "worktree",
                    "remove",
                    "--force",
                    "/repo/.worktrees/task-5"
                ]
            );
        }
    }
}
