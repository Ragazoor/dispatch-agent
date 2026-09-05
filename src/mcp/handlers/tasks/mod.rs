use std::collections::HashMap;

use serde::Deserialize;

use crate::mcp::McpState;
use crate::models::{EpicId, SubStatus, Task, TaskId, TaskStatus, TaskTag, UrlType, WrapUpMode};
use crate::service::{FieldUpdate, UpdateTaskParams};

// Promoted to pub(super) so sub-modules can `use super::{parse_args, ...}`
pub(super) use super::types::{
    deserialize_flexible_id, deserialize_nullable_flexible_id, deserialize_nullable_wrap_up_mode,
    deserialize_optional_flexible_i64, deserialize_optional_flexible_id,
    deserialize_optional_url_type, fetch_caller_task, parse_args, service_err_to_response,
    JsonRpcResponse, StatusFilter, INTERNAL_ERROR, INVALID_PARAMS,
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
pub(super) use dispatch::handle_dispatch_task;
pub(super) use verify::handle_set_verify_command;
pub(super) use watch::{handle_subscribe_to_task, handle_unsubscribe_from_task};
pub(super) use wrap_up::{handle_exit_session, handle_wrap_up};

// ---------------------------------------------------------------------------
// Typed argument structs (JSON-RPC layer)
// ---------------------------------------------------------------------------

mcp_args! {
    /// Arguments for the `update_task` MCP tool.
    ///
    /// The task-field set is declared exactly once, here: the struct, the
    /// tool's JSON input schema (`update_task_schema`, wired into `mcp_tools!`)
    /// and the arg→[`UpdateTaskParams`] mapping (`apply_to_params`) all expand
    /// from this list. Adding a field means adding one line.
    ///
    /// `url` and `url_type` are `[manual]` because they validate as a pair —
    /// a non-empty `url` requires a `url_type`, and an empty `url` clears
    /// instead of setting. `task_id` is `[manual]` because it is consumed
    /// constructing the builder rather than applied to it.
    pub(super) struct UpdateTaskArgs;
    schema fn update_task_schema;
    apply fn apply_to_params(UpdateTaskParams);

    #[serde(deserialize_with = "deserialize_flexible_id")]
    required task_id: TaskId = [manual] {
        "type": "integer",
        "description": "The task ID"
    };

    #[serde(default)]
    optional status: Option<TaskStatus> = [set(status)] {
        "type": "string",
        "description": "New status: backlog, running, review, or done. status=\"done\" is a dedicated close call: no other field may be set in the same call, and a dispatched agent cannot use it to close its own task (call wrap_up then exit_session for that instead). Archived is not allowed via MCP — ask the human operator to archive from the TUI.",
        "enum": crate::models::TaskStatus::MCP_UPDATABLE.iter().map(|s| s.as_str()).collect::<Vec<_>>()
    };

    #[serde(default)]
    optional plan_path: Option<String> = [set(plan_path, FieldUpdate::Set)] {
        "type": "string",
        "description": "Absolute file path to the implementation plan"
    };

    #[serde(default)]
    optional title: Option<String> = [set(title)] {
        "type": "string",
        "description": "New title for the task"
    };

    #[serde(default)]
    optional description: Option<String> = [set(description)] {
        "type": "string",
        "description": "New description for the task"
    };

    #[serde(default)]
    optional repo_path: Option<String> = [set(repo_path)] {
        "type": "string",
        "description": "New repository path for the task"
    };

    #[serde(default, deserialize_with = "deserialize_optional_flexible_i64")]
    optional sort_order: Option<i64> = [set(sort_order)] {
        "type": "integer",
        "description": "Display order within column (lower values appear first)"
    };

    #[serde(default)]
    optional url: Option<String> = [manual] {
        "type": "string",
        "description": "URL associated with this task (PR, issue, security alert, or other link). Pass an empty string to clear it. When set to a non-empty value, url_type is required."
    };

    #[serde(default, deserialize_with = "deserialize_optional_url_type")]
    optional url_type: Option<UrlType> = [manual] {
        "type": "string",
        "description": "Type of the url: 'pr' (pull request — enables PR polling/merge), 'security_alert', 'issue', or 'other'. Required when url is set.",
        "enum": crate::models::UrlType::ALL.iter().map(|u| u.as_str()).collect::<Vec<_>>()
    };

    // MCP tag semantics: absent = leave untouched, present = set. There is no
    // clear-via-MCP, so the inner `Some` is unconditional — the setter's
    // `Some(None)` (clear) is unreachable from this boundary.
    #[serde(default)]
    optional tag: Option<TaskTag> = [set(tag, |t| Some(Some(t)))] {
        "type": "string",
        "description": "Task tag: bug, feature, chore, pr-review, research, fix, or dependabot. Controls dispatch behavior. The dependabot tag is intended for feed scripts only — TUI users cannot select it from the tag picker.",
        "enum": super::dispatch::task_tag_enum_values()
    };

    #[serde(default)]
    optional sub_status: Option<SubStatus> = [set(sub_status)] {
        "type": "string",
        "description": "Sub-status within the current status column. Running: active, needs_input, stale, crashed. Review: awaiting_review, changes_requested, approved. Must be valid for the task's current (or new) status.",
        "enum": crate::models::SubStatus::MCP_ADVERTISED.iter().map(|s| s.as_str()).collect::<Vec<_>>()
    };

    #[serde(default, deserialize_with = "deserialize_optional_flexible_id")]
    optional epic_id: Option<EpicId> = [set(epic_id)] {
        "type": "integer",
        "description": "Link this task to an epic by ID"
    };

    #[serde(default)]
    optional base_branch: Option<String> = [set_some(base_branch)] {
        "type": "string",
        "description": "The base branch for rebase and PR operations (e.g. 'main', 'develop'). Defaults to 'main' if not specified."
    };

    #[serde(default, deserialize_with = "deserialize_nullable_wrap_up_mode")]
    optional wrap_up_mode: Option<Option<WrapUpMode>> = [set(wrap_up_mode)] {
        "type": ["string", "null"],
        "description": "Pre-set the wrap-up action for this task: 'rebase' (rebase onto base_branch), 'pr' (create a PR), or 'done' (mark done immediately). Pass null to clear.",
        "enum": crate::models::WrapUpMode::ALL.iter().map(|m| Some(m.as_str())).chain(std::iter::once(None)).collect::<Vec<Option<&str>>>()
    };

    #[serde(default)]
    optional auto_run_plan: Option<bool> = [set(auto_run_plan)] {
        "type": "boolean",
        "description": "When true and the task has a plan_path, the dispatched agent implements the plan immediately instead of asking for confirmation first."
    };

    #[serde(default)]
    optional phoenix: Option<bool> = [set(phoenix)] {
        "type": "boolean",
        "description": "When true, completing this task automatically recreates it as a fresh backlog copy carrying the same settings, and the flag moves to that copy. A recurring task with no schedule — nothing dispatches the copy, you do."
    };
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
pub(super) struct GetTaskArgs {
    #[serde(deserialize_with = "deserialize_flexible_id")]
    pub(super) task_id: TaskId,
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
pub(super) struct ListTasksArgs {
    #[serde(default)]
    pub(super) status: Option<StatusFilter>,
    #[serde(default, deserialize_with = "deserialize_optional_flexible_id")]
    pub(super) epic_id: Option<EpicId>,
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
    #[serde(default, deserialize_with = "deserialize_nullable_flexible_id")]
    pub(super) epic_id: Option<Option<EpicId>>,
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
    #[serde(default)]
    pub(super) phoenix: bool,
}

// WrapUpAction lives in `crate::mcp` — it's shared between wrap_up (which
// issues an ExitToken recording it) and exit_session (which validates
// against it), not a handler-local concept. Re-exported here so callers in
// this module can keep referring to it unqualified.
pub(super) use crate::mcp::WrapUpAction;

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
pub(super) struct WrapUpArgs {
    #[serde(deserialize_with = "deserialize_flexible_id")]
    pub(super) task_id: TaskId,
    pub(super) action: WrapUpAction,
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
pub(super) struct ExitSessionArgs {
    #[serde(deserialize_with = "deserialize_flexible_id")]
    pub(super) task_id: TaskId,
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
pub(super) struct SubscribeToTaskArgs {
    #[serde(deserialize_with = "deserialize_flexible_id")]
    pub(super) watcher_task_id: TaskId,
    #[serde(deserialize_with = "deserialize_flexible_id")]
    pub(super) target_task_id: TaskId,
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
pub(super) struct UnsubscribeFromTaskArgs {
    #[serde(deserialize_with = "deserialize_flexible_id")]
    pub(super) watcher_task_id: TaskId,
    #[serde(deserialize_with = "deserialize_flexible_id")]
    pub(super) target_task_id: TaskId,
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
pub(super) struct DispatchTaskArgs {
    #[serde(deserialize_with = "deserialize_flexible_id")]
    pub(super) task_id: TaskId,
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
    // Rendered only when set: it tells a wrapping-up agent it is finishing THIS
    // run of a recurring task, not the task itself, so notes for its successor
    // belong in the description and not only in the commit.
    if task.phoenix {
        text.push_str("\nPhoenix: yes — completing this task recreates it as a fresh backlog copy");
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

    use crate::models::{Task, TaskId};

    use super::format_task_detail;

    fn make_task(base_branch: &str) -> Task {
        Task {
            id: TaskId(1),
            title: "Test task".to_string(),
            description: "A description".to_string(),
            repo_path: "/repo".to_string(),
            base_branch: base_branch.into(),
            ..Default::default()
        }
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
