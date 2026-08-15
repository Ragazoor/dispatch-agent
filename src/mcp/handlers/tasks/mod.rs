use std::collections::HashMap;

use serde::Deserialize;

use crate::mcp::McpState;
use crate::models::{EpicId, SubStatus, Task, TaskStatus, TaskTag, WrapUpMode};

// Promoted to pub(super) so sub-modules can `use super::{parse_args, ...}`
pub(super) use super::types::{
    deserialize_flexible_i64, deserialize_nullable_flexible_i64, deserialize_nullable_wrap_up_mode,
    deserialize_optional_flexible_i64, fetch_caller_task, parse_args, service_err_to_response,
    JsonRpcResponse, StatusFilter,
};

mod crud;
// `pub(super)` so the handler tests under `handlers::tests` can reach
// `auto_dispatch_next` directly for the one branch `exit_session` cannot drive
// (a task whose epic_id does not resolve — blocked by a foreign key).
pub(super) mod dispatch;
mod verify;
mod watch;
mod wrap_up;

pub(super) use crud::{
    handle_create_task, handle_get_task, handle_list_tasks, handle_query_usage, handle_update_task,
};
pub(super) use dispatch::{handle_dispatch_task, handle_send_message};
pub(super) use verify::handle_set_verify_command;
pub(super) use watch::{handle_subscribe_to_task, handle_unsubscribe_from_task};
pub(super) use wrap_up::{handle_exit_session, handle_wrap_up};

// ---------------------------------------------------------------------------
// Typed argument structs (JSON-RPC layer)
// ---------------------------------------------------------------------------

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
pub(super) struct UpdateTaskArgs {
    #[serde(deserialize_with = "deserialize_flexible_i64")]
    pub(super) task_id: i64,
    #[serde(default)]
    pub(super) status: Option<TaskStatus>,
    #[serde(default)]
    pub(super) plan_path: Option<String>,
    #[serde(default)]
    pub(super) title: Option<String>,
    #[serde(default)]
    pub(super) description: Option<String>,
    #[serde(default)]
    pub(super) repo_path: Option<String>,
    #[serde(default, deserialize_with = "deserialize_optional_flexible_i64")]
    pub(super) sort_order: Option<i64>,
    #[serde(default)]
    pub(super) url: Option<String>,
    #[serde(default)]
    pub(super) url_type: Option<String>,
    #[serde(default)]
    pub(super) tag: Option<TaskTag>,
    #[serde(default)]
    pub(super) sub_status: Option<SubStatus>,
    #[serde(default, deserialize_with = "deserialize_optional_flexible_i64")]
    pub(super) epic_id: Option<i64>,
    #[serde(default)]
    pub(super) base_branch: Option<String>,
    #[serde(default, deserialize_with = "deserialize_nullable_wrap_up_mode")]
    pub(super) wrap_up_mode: Option<Option<WrapUpMode>>,
    #[serde(default)]
    pub(super) auto_run_plan: Option<bool>,
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
pub(super) struct GetTaskArgs {
    #[serde(deserialize_with = "deserialize_flexible_i64")]
    pub(super) task_id: i64,
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
pub(super) struct ListTasksArgs {
    #[serde(default)]
    pub(super) status: Option<StatusFilter>,
    #[serde(default, deserialize_with = "deserialize_optional_flexible_i64")]
    pub(super) epic_id: Option<i64>,
    #[serde(default)]
    pub(super) repo_paths: Option<Vec<String>>,
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
pub(super) struct CreateTaskWithEpicArgs {
    pub(super) title: String,
    pub(super) repo_path: String,
    #[serde(default)]
    pub(super) description: String,
    pub(super) plan_path: Option<String>,
    /// Double-Option distinguishes "absent" (→ outer None: inherit from
    /// CallerIdentity if Task) from "explicit null" (→ Some(None): clear /
    /// no epic).
    #[serde(default, deserialize_with = "deserialize_nullable_flexible_i64")]
    pub(super) epic_id: Option<Option<i64>>,
    #[serde(default, deserialize_with = "deserialize_optional_flexible_i64")]
    pub(super) sort_order: Option<i64>,
    #[serde(default)]
    pub(super) tag: Option<TaskTag>,
    #[serde(default)]
    pub(super) base_branch: Option<String>,
    #[serde(default)]
    pub(super) wrap_up_mode: Option<WrapUpMode>,
    #[serde(default)]
    pub(super) auto_run_plan: bool,
}

// WrapUpAction lives in `crate::mcp` — it's shared between wrap_up (which
// issues an ExitToken recording it) and exit_session (which validates
// against it), not a handler-local concept. Re-exported here so callers in
// this module can keep referring to it unqualified.
pub(super) use crate::mcp::WrapUpAction;

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
pub(super) struct WrapUpArgs {
    #[serde(deserialize_with = "deserialize_flexible_i64")]
    pub(super) task_id: i64,
    pub(super) action: WrapUpAction,
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
pub(super) struct ExitSessionArgs {
    #[serde(deserialize_with = "deserialize_flexible_i64")]
    pub(super) task_id: i64,
    #[serde(default)]
    pub(super) token: Option<String>,
    #[serde(default)]
    pub(super) action: Option<WrapUpAction>,
    #[serde(default)]
    pub(super) pr_url: Option<String>,
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
pub(super) struct SetVerifyCommandArgs {
    pub(super) repo_path: String,
    pub(super) command: Option<String>,
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
pub(super) struct SendMessageArgs {
    #[serde(deserialize_with = "deserialize_flexible_i64")]
    pub(super) from_task_id: i64,
    #[serde(deserialize_with = "deserialize_flexible_i64")]
    pub(super) to_task_id: i64,
    pub(super) body: String,
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
pub(super) struct SubscribeToTaskArgs {
    #[serde(deserialize_with = "deserialize_flexible_i64")]
    pub(super) watcher_task_id: i64,
    #[serde(deserialize_with = "deserialize_flexible_i64")]
    pub(super) target_task_id: i64,
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
pub(super) struct UnsubscribeFromTaskArgs {
    #[serde(deserialize_with = "deserialize_flexible_i64")]
    pub(super) watcher_task_id: i64,
    #[serde(deserialize_with = "deserialize_flexible_i64")]
    pub(super) target_task_id: i64,
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
pub(super) struct DispatchTaskArgs {
    #[serde(deserialize_with = "deserialize_flexible_i64")]
    pub(super) task_id: i64,
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
pub(super) struct QueryUsageArgs {
    #[serde(default)]
    pub(super) category: Option<String>,
    #[serde(default)]
    pub(super) actor: Option<String>,
    #[serde(default)]
    pub(super) since: Option<String>,
    #[serde(default)]
    pub(super) limit: Option<i64>,
}

// ---------------------------------------------------------------------------
// Response formatting (presentation layer)
// ---------------------------------------------------------------------------

async fn build_epic_titles(state: &McpState) -> HashMap<EpicId, String> {
    state
        .db
        .list_epics()
        .await
        .unwrap_or_default()
        .into_iter()
        .map(|e| (e.id, e.title))
        .collect()
}

fn format_task_detail(
    task: &Task,
    epic_titles: &HashMap<EpicId, String>,
    verify_command: Option<&str>,
) -> String {
    let mut text = format!(
        "Task {id}: {title}\nStatus: {status}\nRepo: {repo}\nDescription: {desc}",
        id = task.id,
        title = task.title,
        status = task.status.as_str(),
        repo = task.repo_path,
        desc = task.description,
    );
    text.push_str(&format!("\nSub-status: {}", task.sub_status.as_str()));
    if let Some(epic_id) = task.epic_id {
        let epic_label = match epic_titles.get(&epic_id) {
            Some(title) => format!("{title} (#{epic_id})"),
            None => format!("#{epic_id}"),
        };
        text.push_str(&format!("\nEpic: {epic_label}"));
    }
    if let Some(ref tag) = task.tag {
        text.push_str(&format!("\nTag: {tag}"));
    }
    if let Some(ref plan) = task.plan_path {
        text.push_str(&format!("\nPlan: {plan}"));
    }
    if let Some(u) = &task.url {
        text.push_str(&format!("\n{}: {}", u.label(), u.url));
    }
    if let Some(ref worktree) = task.worktree {
        text.push_str(&format!("\nWorktree: {worktree}"));
    }
    text.push_str(&format!("\nBase branch: {}", task.base_branch));
    if let Some(ref tmux_window) = task.tmux_window {
        text.push_str(&format!("\nTmux window: {tmux_window}"));
    }
    if let Some(sort_order) = task.sort_order {
        text.push_str(&format!("\nSort order: {sort_order}"));
    }
    if let Some(cmd) = verify_command {
        text.push_str(&format!("\nVerify command: {cmd}"));
    }
    if let Some(wrap_up_mode) = task.wrap_up_mode {
        text.push_str(&format!("\nWrap-up mode: {wrap_up_mode}"));
    }
    text.push_str(&format!(
        "\nCreated: {}",
        task.created_at.format("%Y-%m-%d %H:%M:%S UTC")
    ));
    text.push_str(&format!(
        "\nUpdated: {}",
        task.updated_at.format("%Y-%m-%d %H:%M:%S UTC")
    ));
    text
}

async fn plan_goal(path: &str) -> Option<String> {
    let content = tokio::fs::read_to_string(path).await.ok()?;
    let description = crate::plan::parse_plan(&content).ok()?.description;
    (!description.is_empty()).then_some(description)
}

fn description_preview(s: &str) -> String {
    if s.len() > 200 {
        let end = s
            .char_indices()
            .take_while(|(i, _)| *i < 200)
            .last()
            .map(|(i, c)| i + c.len_utf8())
            .unwrap_or(0);
        format!("{}...", &s[..end])
    } else {
        s.to_owned()
    }
}

fn format_task_line(t: &Task, epic_titles: &HashMap<EpicId, String>, goal: &str) -> String {
    let tag_indicator = match t.tag {
        Some(tag) => format!(" [{}]", tag.as_str()),
        None => String::new(),
    };
    let epic_indicator = match t.epic_id {
        Some(eid) => match epic_titles.get(&eid) {
            Some(title) => format!(" (epic:{eid} {title})"),
            None => format!(" (epic:{eid})"),
        },
        None => String::new(),
    };
    let pr_part = t
        .url
        .as_ref()
        .map(|u| format!(" | {}: {}", u.label(), u.url))
        .unwrap_or_default();
    let goal_part = if goal.is_empty() {
        String::new()
    } else {
        format!(" | Goal: {goal}")
    };
    format!(
        "- [{}] {} ({}/{}){}{}{}{}",
        t.id,
        t.title,
        t.status.as_str(),
        t.sub_status.as_str(),
        tag_indicator,
        epic_indicator,
        pr_part,
        goal_part,
    )
}

// ---------------------------------------------------------------------------
// Task tool handlers (thin wrappers over TaskService)
// ---------------------------------------------------------------------------

async fn reflection_nudge(db: &dyn crate::db::TaskReadStore) -> &'static str {
    let enabled = db
        .get_setting_bool("learning_reflection_enabled")
        .await
        .unwrap_or(None)
        .unwrap_or(true);
    if enabled {
        " Before finishing, did you discover anything non-obvious about \
this repo or task? If so, call record_learning with a brief summary."
    } else {
        ""
    }
}

#[cfg(test)]
#[allow(clippy::unwrap_used, clippy::expect_used)]
mod tests {
    use std::collections::HashMap;

    use crate::models::{SubStatus, Task, TaskId, TaskStatus};

    use super::format_task_detail;

    fn make_task(base_branch: &str) -> Task {
        let now = chrono::Utc::now();
        Task {
            id: TaskId(1),
            title: "Test task".to_string(),
            description: "A description".to_string(),
            repo_path: "/repo".to_string(),
            status: TaskStatus::Backlog,
            worktree: None,
            tmux_window: None,
            plan_path: None,
            epic_id: None,
            sub_status: SubStatus::default_for(TaskStatus::Backlog),
            url: None,
            tag: None,
            sort_order: None,
            base_branch: base_branch.into(),
            external_id: None,
            labels: Vec::new(),
            created_at: now,
            updated_at: now,
            last_pre_tool_use_at: None,
            last_notification_at: None,
            wrap_up_mode: None,
            auto_run_plan: false,
            live_subagents: 0,
            stop_pending: false,
            live_shells: 0,
            oldest_live_shell_started_at: None,        }
    }

    #[test]
    fn format_task_detail_includes_base_branch() {
        let task = make_task("develop");
        let output = format_task_detail(&task, &HashMap::new(), None);
        assert!(
            output.contains("Base branch: develop"),
            "expected 'Base branch: develop' in output, got:\n{output}"
        );
    }

    #[test]
    fn format_task_detail_includes_verify_command_when_set() {
        let task = make_task("main");
        let output = format_task_detail(&task, &HashMap::new(), Some("cargo test"));
        assert!(
            output.contains("Verify command: cargo test"),
            "expected 'Verify command: cargo test' in output, got:\n{output}"
        );
    }

    #[test]
    fn format_task_detail_omits_verify_command_when_unset() {
        let task = make_task("main");
        let output = format_task_detail(&task, &HashMap::new(), None);
        assert!(
            !output.contains("Verify command"),
            "expected no 'Verify command' line in output, got:\n{output}"
        );
    }
}
