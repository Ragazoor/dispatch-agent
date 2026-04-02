use std::sync::Arc;

use crate::db::{self, EpicPatch, TaskPatch};
use crate::models::{Epic, EpicId, SubStatus, Task, TaskId, TaskStatus, TaskTag, UsageReport};

// ---------------------------------------------------------------------------
// Service error
// ---------------------------------------------------------------------------

#[derive(Debug)]
pub enum ServiceError {
    /// Client-provided data is invalid (bad status, missing fields, etc.)
    Validation(String),
    /// Entity not found
    NotFound(String),
    /// Database or internal error
    Internal(String),
}

impl std::fmt::Display for ServiceError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            ServiceError::Validation(msg) => write!(f, "{msg}"),
            ServiceError::NotFound(msg) => write!(f, "{msg}"),
            ServiceError::Internal(msg) => write!(f, "{msg}"),
        }
    }
}

impl std::error::Error for ServiceError {}

// ---------------------------------------------------------------------------
// Parsing helpers (moved from MCP validation layer)
// ---------------------------------------------------------------------------

pub fn parse_status(s: &str) -> Result<TaskStatus, ServiceError> {
    TaskStatus::parse(s).ok_or_else(|| {
        ServiceError::Validation(format!(
            "Unknown status: {s}. Valid values: {}",
            TaskStatus::ALL
                .iter()
                .map(|s| s.as_str())
                .collect::<Vec<_>>()
                .join(", ")
        ))
    })
}

pub fn parse_tag(s: &str) -> Result<TaskTag, ServiceError> {
    TaskTag::parse(s).ok_or_else(|| {
        ServiceError::Validation(format!(
            "Invalid tag: {s}. Valid values: bug, feature, chore, epic"
        ))
    })
}

pub fn parse_substatus(s: &str) -> Result<SubStatus, ServiceError> {
    SubStatus::parse(s).ok_or_else(|| {
        ServiceError::Validation(format!(
            "Invalid sub_status: {s}. Valid values: {}",
            SubStatus::ALL
                .iter()
                .map(|s| s.as_str())
                .collect::<Vec<_>>()
                .join(", ")
        ))
    })
}

// ---------------------------------------------------------------------------
// UpdateTaskParams — transport-agnostic input for update_task
// ---------------------------------------------------------------------------

pub struct UpdateTaskParams {
    pub task_id: i64,
    pub status: Option<String>,
    pub plan: Option<String>,
    pub title: Option<String>,
    pub description: Option<String>,
    pub repo_path: Option<String>,
    pub sort_order: Option<i64>,
    pub pr_url: Option<String>,
    pub tag: Option<String>,
    pub sub_status: Option<String>,
    pub epic_id: Option<i64>,
}

impl UpdateTaskParams {
    fn has_any_field(&self) -> bool {
        self.status.is_some()
            || self.plan.is_some()
            || self.title.is_some()
            || self.description.is_some()
            || self.repo_path.is_some()
            || self.sort_order.is_some()
            || self.pr_url.is_some()
            || self.tag.is_some()
            || self.sub_status.is_some()
            || self.epic_id.is_some()
    }

    pub fn updated_field_names(&self) -> Vec<String> {
        let mut names = Vec::new();
        if let Some(ref s) = self.status {
            names.push(format!("status={s}"));
        }
        if self.plan.is_some() {
            names.push("plan".to_string());
        }
        if self.title.is_some() {
            names.push("title".to_string());
        }
        if self.description.is_some() {
            names.push("description".to_string());
        }
        if self.repo_path.is_some() {
            names.push("repo_path".to_string());
        }
        if self.sort_order.is_some() {
            names.push("sort_order".to_string());
        }
        if self.pr_url.is_some() {
            names.push("pr_url".to_string());
        }
        if self.tag.is_some() {
            names.push("tag".to_string());
        }
        if self.sub_status.is_some() {
            names.push("sub_status".to_string());
        }
        if self.epic_id.is_some() {
            names.push("epic_id".to_string());
        }
        names
    }
}

// ---------------------------------------------------------------------------
// CreateTaskParams
// ---------------------------------------------------------------------------

pub struct CreateTaskParams {
    pub title: String,
    pub description: String,
    pub repo_path: String,
    pub plan: Option<String>,
    pub epic_id: Option<i64>,
    pub sort_order: Option<i64>,
    pub tag: Option<String>,
}

// ---------------------------------------------------------------------------
// ClaimTaskParams
// ---------------------------------------------------------------------------

pub struct ClaimTaskParams {
    pub task_id: i64,
    pub worktree: String,
    pub tmux_window: String,
}

// ---------------------------------------------------------------------------
// ListTasksFilter
// ---------------------------------------------------------------------------

pub struct ListTasksFilter {
    pub statuses: Option<Vec<TaskStatus>>,
    pub epic_id: Option<EpicId>,
}

// ---------------------------------------------------------------------------
// TaskService
// ---------------------------------------------------------------------------

pub struct TaskService {
    pub db: Arc<dyn db::TaskStore>,
}

impl TaskService {
    pub fn new(db: Arc<dyn db::TaskStore>) -> Self {
        Self { db }
    }

    pub fn update_task(&self, params: UpdateTaskParams) -> Result<TaskId, ServiceError> {
        if !params.has_any_field() {
            return Err(ServiceError::Validation(
                "At least one of status, plan, title, description, repo_path, sort_order, pr_url, tag, sub_status, epic_id must be provided".into(),
            ));
        }

        let status = if let Some(ref s) = params.status {
            Some(parse_status(s)?)
        } else {
            None
        };

        if matches!(status, Some(TaskStatus::Done | TaskStatus::Archived)) {
            return Err(ServiceError::Validation(
                "Cannot set status to done or archived via MCP. Please ask the human operator to manage this from the TUI.".into(),
            ));
        }

        let expanded_repo_path = params.repo_path.as_deref().map(crate::models::expand_tilde);

        let mut patch = TaskPatch::new();
        if let Some(s) = status {
            patch = patch.status(s);
        }
        if let Some(ref p) = params.plan {
            patch = patch.plan(Some(p.as_str()));
        }
        if let Some(ref t) = params.title {
            patch = patch.title(t);
        }
        if let Some(ref d) = params.description {
            patch = patch.description(d);
        }
        if let Some(ref r) = expanded_repo_path {
            patch = patch.repo_path(r);
        }
        if let Some(so) = params.sort_order {
            patch = patch.sort_order(Some(so));
        }
        if let Some(ref url) = params.pr_url {
            patch = patch.pr_url(Some(url.as_str()));
        }
        if let Some(ref t) = params.tag {
            let tag = parse_tag(t)?;
            patch = patch.tag(Some(tag));
        }

        if let Some(ref ss_str) = params.sub_status {
            let ss = parse_substatus(ss_str)?;
            let effective_status = params
                .status
                .as_deref()
                .and_then(TaskStatus::parse)
                .or_else(|| {
                    self.db
                        .get_task(TaskId(params.task_id))
                        .ok()
                        .flatten()
                        .map(|t| t.status)
                });
            if let Some(eff) = effective_status {
                if !ss.is_valid_for(eff) {
                    return Err(ServiceError::Validation(format!(
                        "sub_status '{}' is not valid for status '{}'",
                        ss_str,
                        eff.as_str()
                    )));
                }
            }
            patch = patch.sub_status(ss);
        }

        let task_id = TaskId(params.task_id);
        self.db
            .patch_task(task_id, &patch)
            .map_err(|e| ServiceError::Internal(format!("Database error: {e}")))?;

        // Update epic linkage if requested
        if let Some(new_epic_id) = params.epic_id {
            // Recalculate old epic before reassignment
            if let Ok(Some(task)) = self.db.get_task(task_id) {
                if let Some(old_epic_id) = task.epic_id {
                    let _ = self.db.recalculate_epic_status(old_epic_id);
                }
            }
            self.db
                .set_task_epic_id(task_id, Some(EpicId(new_epic_id)))
                .map_err(|e| ServiceError::Internal(format!("Failed to link task to epic: {e}")))?;
            let _ = self.db.recalculate_epic_status(EpicId(new_epic_id));
        }

        // Recalculate parent epic status if subtask status changed
        if params.status.is_some() {
            if let Ok(Some(task)) = self.db.get_task(task_id) {
                if let Some(epic_id) = task.epic_id {
                    let _ = self.db.recalculate_epic_status(epic_id);
                }
            }
        }

        Ok(task_id)
    }

    pub fn create_task(&self, params: CreateTaskParams) -> Result<TaskId, ServiceError> {
        let repo_path = crate::models::expand_tilde(&params.repo_path);

        let plan = params.plan.as_deref().map(|p| {
            std::fs::canonicalize(p)
                .map(|abs| abs.to_string_lossy().into_owned())
                .unwrap_or_else(|_| p.to_string())
        });

        let task_id = self
            .db
            .create_task(
                &params.title,
                &params.description,
                &repo_path,
                plan.as_deref(),
                TaskStatus::Backlog,
            )
            .map_err(|e| ServiceError::Internal(format!("Database error: {e}")))?;

        if let Some(eid) = params.epic_id {
            self.db
                .set_task_epic_id(task_id, Some(EpicId(eid)))
                .map_err(|e| ServiceError::Internal(format!("Failed to link task to epic: {e}")))?;
        }
        if let Some(so) = params.sort_order {
            let _ = self
                .db
                .patch_task(task_id, &TaskPatch::new().sort_order(Some(so)));
        }
        if let Some(ref t) = params.tag {
            let tag = parse_tag(t)?;
            let _ = self
                .db
                .patch_task(task_id, &TaskPatch::new().tag(Some(tag)));
        }

        Ok(task_id)
    }

    pub fn get_task(&self, task_id: i64) -> Result<Task, ServiceError> {
        match self.db.get_task(TaskId(task_id)) {
            Ok(Some(task)) => Ok(task),
            Ok(None) => Err(ServiceError::NotFound(format!(
                "Task {} not found",
                task_id
            ))),
            Err(e) => Err(ServiceError::Internal(format!("Database error: {e}"))),
        }
    }

    pub fn list_tasks(&self, filter: ListTasksFilter) -> Result<Vec<Task>, ServiceError> {
        let tasks = self
            .db
            .list_all()
            .map_err(|e| ServiceError::Internal(format!("Database error: {e}")))?;

        let filtered: Vec<_> = tasks
            .into_iter()
            .filter(|t| match &filter.statuses {
                Some(statuses) => statuses.contains(&t.status),
                None => true,
            })
            .filter(|t| match filter.epic_id {
                Some(eid) => t.epic_id == Some(eid),
                None => true,
            })
            .collect();

        Ok(filtered)
    }

    pub fn claim_task(&self, params: ClaimTaskParams) -> Result<Task, ServiceError> {
        let task = self.get_task(params.task_id)?;

        if task.status != TaskStatus::Backlog {
            return Err(ServiceError::Validation(format!(
                "Task {} is already {}",
                params.task_id,
                task.status.as_str()
            )));
        }

        // Same-repo check: derive repo from worktree by stripping /.worktrees/<anything>
        let repo_from_worktree = params
            .worktree
            .find("/.worktrees/")
            .map(|idx| &params.worktree[..idx])
            .unwrap_or(&params.worktree);

        if repo_from_worktree != task.repo_path {
            return Err(ServiceError::Validation(format!(
                "Repo mismatch: task belongs to {}, your worktree is in {}",
                task.repo_path, repo_from_worktree
            )));
        }

        self.db
            .patch_task(
                TaskId(params.task_id),
                &TaskPatch::new()
                    .status(TaskStatus::Running)
                    .worktree(Some(&params.worktree))
                    .tmux_window(Some(&params.tmux_window)),
            )
            .map_err(|e| ServiceError::Internal(format!("Database error: {e}")))?;

        Ok(task)
    }

    pub fn validate_wrap_up(&self, task_id: i64, action: &str) -> Result<Task, ServiceError> {
        if action != "rebase" && action != "pr" {
            return Err(ServiceError::Validation(format!(
                "Unknown action: {action}. Valid values: rebase, pr"
            )));
        }

        let task = self.get_task(task_id)?;

        if !crate::dispatch::is_wrappable(&task) {
            return Err(ServiceError::Validation(format!(
                "Task {} cannot be wrapped up. Requires Running or Review status with a worktree.",
                task_id
            )));
        }

        Ok(task)
    }

    pub fn report_usage(&self, task_id: i64, usage: &UsageReport) -> Result<(), ServiceError> {
        // Verify task exists
        self.get_task(task_id)?;

        self.db
            .report_usage(TaskId(task_id), usage)
            .map_err(|e| ServiceError::Internal(format!("Failed to record usage: {e}")))
    }

    pub fn validate_send_message(
        &self,
        from_task_id: i64,
        to_task_id: i64,
    ) -> Result<(Task, Task), ServiceError> {
        let from_task = match self.db.get_task(TaskId(from_task_id)) {
            Ok(Some(t)) => t,
            Ok(None) => {
                return Err(ServiceError::NotFound(format!(
                    "sender task {} not found",
                    from_task_id
                )));
            }
            Err(e) => {
                return Err(ServiceError::Internal(format!(
                    "failed to look up sender: {e}"
                )));
            }
        };

        let to_task = match self.db.get_task(TaskId(to_task_id)) {
            Ok(Some(t)) => t,
            Ok(None) => {
                return Err(ServiceError::NotFound(format!(
                    "target task {} not found",
                    to_task_id
                )));
            }
            Err(e) => {
                return Err(ServiceError::Internal(format!(
                    "failed to look up target: {e}"
                )));
            }
        };

        if to_task.worktree.is_none() {
            return Err(ServiceError::Validation(format!(
                "target task {} has no worktree (not running)",
                to_task_id
            )));
        }

        if to_task.tmux_window.is_none() {
            return Err(ServiceError::Validation(format!(
                "target task {} has no tmux window (not running)",
                to_task_id
            )));
        }

        Ok((from_task, to_task))
    }
}

// ---------------------------------------------------------------------------
// UpdateEpicParams
// ---------------------------------------------------------------------------

pub struct UpdateEpicParams {
    pub epic_id: i64,
    pub title: Option<String>,
    pub description: Option<String>,
    pub status: Option<String>,
    pub plan: Option<String>,
    pub sort_order: Option<i64>,
    pub repo_path: Option<String>,
}

impl UpdateEpicParams {
    fn has_any_field(&self) -> bool {
        self.title.is_some()
            || self.description.is_some()
            || self.status.is_some()
            || self.plan.is_some()
            || self.sort_order.is_some()
            || self.repo_path.is_some()
    }

    pub fn updated_field_names(&self) -> Vec<&str> {
        let mut names = Vec::new();
        if self.title.is_some() {
            names.push("title");
        }
        if self.description.is_some() {
            names.push("description");
        }
        if self.status.is_some() {
            names.push("status");
        }
        if self.plan.is_some() {
            names.push("plan");
        }
        if self.sort_order.is_some() {
            names.push("sort_order");
        }
        if self.repo_path.is_some() {
            names.push("repo_path");
        }
        names
    }
}

// ---------------------------------------------------------------------------
// CreateEpicParams
// ---------------------------------------------------------------------------

pub struct CreateEpicParams {
    pub title: String,
    pub description: String,
    pub repo_path: String,
    pub sort_order: Option<i64>,
}

// ---------------------------------------------------------------------------
// EpicService
// ---------------------------------------------------------------------------

pub struct EpicService {
    pub db: Arc<dyn db::TaskStore>,
}

impl EpicService {
    pub fn new(db: Arc<dyn db::TaskStore>) -> Self {
        Self { db }
    }

    pub fn create_epic(&self, params: CreateEpicParams) -> Result<Epic, ServiceError> {
        let epic = self
            .db
            .create_epic(&params.title, &params.description, &params.repo_path)
            .map_err(|e| ServiceError::Internal(format!("Database error: {e}")))?;

        if let Some(so) = params.sort_order {
            let _ = self
                .db
                .patch_epic(epic.id, &EpicPatch::new().sort_order(Some(so)));
        }

        Ok(epic)
    }

    pub fn get_epic(&self, epic_id: i64) -> Result<Epic, ServiceError> {
        match self.db.get_epic(EpicId(epic_id)) {
            Ok(Some(epic)) => Ok(epic),
            Ok(None) => Err(ServiceError::NotFound(format!(
                "Epic {} not found",
                epic_id
            ))),
            Err(e) => Err(ServiceError::Internal(format!("Database error: {e}"))),
        }
    }

    pub fn get_epic_with_subtasks(&self, epic_id: i64) -> Result<(Epic, Vec<Task>), ServiceError> {
        let epic = self.get_epic(epic_id)?;
        let subtasks = self.db.list_tasks_for_epic(epic.id).unwrap_or_default();
        Ok((epic, subtasks))
    }

    pub fn list_epics(&self) -> Result<Vec<Epic>, ServiceError> {
        self.db
            .list_epics()
            .map_err(|e| ServiceError::Internal(format!("Database error: {e}")))
    }

    pub fn list_epics_with_progress(&self) -> Result<Vec<(Epic, usize, usize)>, ServiceError> {
        let epics = self.list_epics()?;
        let result = epics
            .into_iter()
            .map(|e| {
                let subtasks = self.db.list_tasks_for_epic(e.id).unwrap_or_default();
                let done = subtasks
                    .iter()
                    .filter(|t| t.status == TaskStatus::Done)
                    .count();
                let total = subtasks.len();
                (e, done, total)
            })
            .collect();
        Ok(result)
    }

    pub fn update_epic(&self, params: UpdateEpicParams) -> Result<EpicId, ServiceError> {
        if !params.has_any_field() {
            return Err(ServiceError::Validation(
                "At least one of title, description, status, plan, sort_order, repo_path must be provided".into(),
            ));
        }

        let repo_path = params.repo_path.as_deref().map(crate::models::expand_tilde);
        let mut patch = EpicPatch::new();
        if let Some(ref t) = params.title {
            patch = patch.title(t);
        }
        if let Some(ref d) = params.description {
            patch = patch.description(d);
        }
        if let Some(ref s) = params.status {
            let status = parse_status(s)?;
            patch = patch.status(status);
        }
        if let Some(ref p) = params.plan {
            patch = patch.plan(Some(p.as_str()));
        }
        if let Some(so) = params.sort_order {
            patch = patch.sort_order(Some(so));
        }
        if let Some(ref rp) = repo_path {
            patch = patch.repo_path(rp);
        }

        let epic_id = EpicId(params.epic_id);
        self.db
            .patch_epic(epic_id, &patch)
            .map_err(|e| ServiceError::Internal(format!("Database error: {e}")))?;

        Ok(epic_id)
    }
}

// ---------------------------------------------------------------------------
// Tests
// ---------------------------------------------------------------------------

#[cfg(test)]
mod tests {
    use super::*;
    use crate::db::Database;

    fn test_db() -> Arc<dyn db::TaskStore> {
        Arc::new(Database::open_in_memory().unwrap())
    }

    fn task_svc(db: &Arc<dyn db::TaskStore>) -> TaskService {
        TaskService::new(Arc::clone(db))
    }

    fn epic_svc(db: &Arc<dyn db::TaskStore>) -> EpicService {
        EpicService::new(Arc::clone(db))
    }

    // -- parse helpers --------------------------------------------------------

    #[test]
    fn parse_status_valid() {
        assert_eq!(parse_status("backlog").unwrap(), TaskStatus::Backlog);
        assert_eq!(parse_status("running").unwrap(), TaskStatus::Running);
    }

    #[test]
    fn parse_status_invalid() {
        let err = parse_status("invalid").unwrap_err();
        assert!(matches!(err, ServiceError::Validation(_)));
    }

    #[test]
    fn parse_tag_valid() {
        assert_eq!(parse_tag("bug").unwrap(), TaskTag::Bug);
    }

    #[test]
    fn parse_tag_invalid() {
        let err = parse_tag("nope").unwrap_err();
        assert!(matches!(err, ServiceError::Validation(_)));
    }

    #[test]
    fn parse_substatus_valid() {
        assert_eq!(parse_substatus("active").unwrap(), SubStatus::Active);
    }

    #[test]
    fn parse_substatus_invalid() {
        let err = parse_substatus("nope").unwrap_err();
        assert!(matches!(err, ServiceError::Validation(_)));
    }

    // -- TaskService ----------------------------------------------------------

    #[test]
    fn create_and_get_task() {
        let db = test_db();
        let svc = task_svc(&db);

        let id = svc
            .create_task(CreateTaskParams {
                title: "Test".into(),
                description: "desc".into(),
                repo_path: "/repo".into(),
                plan: None,
                epic_id: None,
                sort_order: None,
                tag: None,
            })
            .unwrap();

        let task = svc.get_task(id.0).unwrap();
        assert_eq!(task.title, "Test");
        assert_eq!(task.status, TaskStatus::Backlog);
    }

    #[test]
    fn create_task_with_tag() {
        let db = test_db();
        let svc = task_svc(&db);

        let id = svc
            .create_task(CreateTaskParams {
                title: "Bug fix".into(),
                description: "".into(),
                repo_path: "/repo".into(),
                plan: None,
                epic_id: None,
                sort_order: Some(5),
                tag: Some("bug".into()),
            })
            .unwrap();

        let task = svc.get_task(id.0).unwrap();
        assert_eq!(task.tag, Some(TaskTag::Bug));
        assert_eq!(task.sort_order, Some(5));
    }

    #[test]
    fn create_task_invalid_tag_returns_error() {
        let db = test_db();
        let svc = task_svc(&db);

        let err = svc
            .create_task(CreateTaskParams {
                title: "Bad".into(),
                description: "".into(),
                repo_path: "/repo".into(),
                plan: None,
                epic_id: None,
                sort_order: None,
                tag: Some("invalid_tag".into()),
            })
            .unwrap_err();
        assert!(matches!(err, ServiceError::Validation(_)));
    }

    #[test]
    fn update_task_status() {
        let db = test_db();
        let svc = task_svc(&db);

        let id = svc
            .create_task(CreateTaskParams {
                title: "T".into(),
                description: "".into(),
                repo_path: "/repo".into(),
                plan: None,
                epic_id: None,
                sort_order: None,
                tag: None,
            })
            .unwrap();

        svc.update_task(UpdateTaskParams {
            task_id: id.0,
            status: Some("running".into()),
            plan: None,
            title: None,
            description: None,
            repo_path: None,
            sort_order: None,
            pr_url: None,
            tag: None,
            sub_status: None,
            epic_id: None,
        })
        .unwrap();

        let task = svc.get_task(id.0).unwrap();
        assert_eq!(task.status, TaskStatus::Running);
    }

    #[test]
    fn update_task_rejects_done_status() {
        let db = test_db();
        let svc = task_svc(&db);

        let id = svc
            .create_task(CreateTaskParams {
                title: "T".into(),
                description: "".into(),
                repo_path: "/repo".into(),
                plan: None,
                epic_id: None,
                sort_order: None,
                tag: None,
            })
            .unwrap();

        let err = svc
            .update_task(UpdateTaskParams {
                task_id: id.0,
                status: Some("done".into()),
                plan: None,
                title: None,
                description: None,
                repo_path: None,
                sort_order: None,
                pr_url: None,
                tag: None,
                sub_status: None,
                epic_id: None,
            })
            .unwrap_err();
        assert!(matches!(err, ServiceError::Validation(_)));
    }

    #[test]
    fn update_task_no_fields_returns_error() {
        let db = test_db();
        let svc = task_svc(&db);

        let id = svc
            .create_task(CreateTaskParams {
                title: "T".into(),
                description: "".into(),
                repo_path: "/repo".into(),
                plan: None,
                epic_id: None,
                sort_order: None,
                tag: None,
            })
            .unwrap();

        let err = svc
            .update_task(UpdateTaskParams {
                task_id: id.0,
                status: None,
                plan: None,
                title: None,
                description: None,
                repo_path: None,
                sort_order: None,
                pr_url: None,
                tag: None,
                sub_status: None,
                epic_id: None,
            })
            .unwrap_err();
        assert!(matches!(err, ServiceError::Validation(_)));
    }

    #[test]
    fn update_task_invalid_substatus_for_status() {
        let db = test_db();
        let svc = task_svc(&db);

        let id = svc
            .create_task(CreateTaskParams {
                title: "T".into(),
                description: "".into(),
                repo_path: "/repo".into(),
                plan: None,
                epic_id: None,
                sort_order: None,
                tag: None,
            })
            .unwrap();

        // active is not valid for backlog
        let err = svc
            .update_task(UpdateTaskParams {
                task_id: id.0,
                status: None,
                plan: None,
                title: None,
                description: None,
                repo_path: None,
                sort_order: None,
                pr_url: None,
                tag: None,
                sub_status: Some("active".into()),
                epic_id: None,
            })
            .unwrap_err();
        assert!(matches!(err, ServiceError::Validation(_)));
    }

    #[test]
    fn claim_task_success() {
        let db = test_db();
        let svc = task_svc(&db);

        let id = svc
            .create_task(CreateTaskParams {
                title: "T".into(),
                description: "".into(),
                repo_path: "/repo".into(),
                plan: None,
                epic_id: None,
                sort_order: None,
                tag: None,
            })
            .unwrap();

        let task = svc
            .claim_task(ClaimTaskParams {
                task_id: id.0,
                worktree: "/repo/.worktrees/feature".into(),
                tmux_window: "win1".into(),
            })
            .unwrap();
        assert_eq!(task.title, "T");

        // Verify it was actually updated
        let task = svc.get_task(id.0).unwrap();
        assert_eq!(task.status, TaskStatus::Running);
        assert_eq!(task.worktree.as_deref(), Some("/repo/.worktrees/feature"));
    }

    #[test]
    fn claim_task_wrong_repo() {
        let db = test_db();
        let svc = task_svc(&db);

        let id = svc
            .create_task(CreateTaskParams {
                title: "T".into(),
                description: "".into(),
                repo_path: "/repo-a".into(),
                plan: None,
                epic_id: None,
                sort_order: None,
                tag: None,
            })
            .unwrap();

        let err = svc
            .claim_task(ClaimTaskParams {
                task_id: id.0,
                worktree: "/repo-b/.worktrees/feature".into(),
                tmux_window: "win1".into(),
            })
            .unwrap_err();
        assert!(matches!(err, ServiceError::Validation(_)));
    }

    #[test]
    fn claim_task_not_backlog() {
        let db = test_db();
        let svc = task_svc(&db);

        let id = svc
            .create_task(CreateTaskParams {
                title: "T".into(),
                description: "".into(),
                repo_path: "/repo".into(),
                plan: None,
                epic_id: None,
                sort_order: None,
                tag: None,
            })
            .unwrap();

        // Move to running first
        svc.update_task(UpdateTaskParams {
            task_id: id.0,
            status: Some("running".into()),
            plan: None,
            title: None,
            description: None,
            repo_path: None,
            sort_order: None,
            pr_url: None,
            tag: None,
            sub_status: None,
            epic_id: None,
        })
        .unwrap();

        let err = svc
            .claim_task(ClaimTaskParams {
                task_id: id.0,
                worktree: "/repo/.worktrees/feature".into(),
                tmux_window: "win1".into(),
            })
            .unwrap_err();
        assert!(matches!(err, ServiceError::Validation(_)));
    }

    #[test]
    fn list_tasks_with_filter() {
        let db = test_db();
        let svc = task_svc(&db);

        svc.create_task(CreateTaskParams {
            title: "T1".into(),
            description: "".into(),
            repo_path: "/repo".into(),
            plan: None,
            epic_id: None,
            sort_order: None,
            tag: None,
        })
        .unwrap();

        let tasks = svc
            .list_tasks(ListTasksFilter {
                statuses: Some(vec![TaskStatus::Backlog]),
                epic_id: None,
            })
            .unwrap();
        assert_eq!(tasks.len(), 1);

        let tasks = svc
            .list_tasks(ListTasksFilter {
                statuses: Some(vec![TaskStatus::Running]),
                epic_id: None,
            })
            .unwrap();
        assert!(tasks.is_empty());
    }

    #[test]
    fn get_task_not_found() {
        let db = test_db();
        let svc = task_svc(&db);
        let err = svc.get_task(999).unwrap_err();
        assert!(matches!(err, ServiceError::NotFound(_)));
    }

    #[test]
    fn report_usage_for_nonexistent_task() {
        let db = test_db();
        let svc = task_svc(&db);
        let err = svc
            .report_usage(
                999,
                &UsageReport {
                    cost_usd: 1.0,
                    input_tokens: 100,
                    output_tokens: 50,
                    cache_read_tokens: 0,
                    cache_write_tokens: 0,
                },
            )
            .unwrap_err();
        assert!(matches!(err, ServiceError::NotFound(_)));
    }

    #[test]
    fn update_task_with_epic_linkage() {
        let db = test_db();
        let task_svc = task_svc(&db);
        let epic_svc = epic_svc(&db);

        let epic = epic_svc
            .create_epic(CreateEpicParams {
                title: "Epic".into(),
                description: "".into(),
                repo_path: "/repo".into(),
                sort_order: None,
            })
            .unwrap();

        let id = task_svc
            .create_task(CreateTaskParams {
                title: "T".into(),
                description: "".into(),
                repo_path: "/repo".into(),
                plan: None,
                epic_id: None,
                sort_order: None,
                tag: None,
            })
            .unwrap();

        task_svc
            .update_task(UpdateTaskParams {
                task_id: id.0,
                status: None,
                plan: None,
                title: None,
                description: None,
                repo_path: None,
                sort_order: None,
                pr_url: None,
                tag: None,
                sub_status: None,
                epic_id: Some(epic.id.0),
            })
            .unwrap();

        let task = task_svc.get_task(id.0).unwrap();
        assert_eq!(task.epic_id, Some(epic.id));
    }

    // -- EpicService ----------------------------------------------------------

    #[test]
    fn create_and_get_epic() {
        let db = test_db();
        let svc = epic_svc(&db);

        let epic = svc
            .create_epic(CreateEpicParams {
                title: "Epic 1".into(),
                description: "desc".into(),
                repo_path: "/repo".into(),
                sort_order: None,
            })
            .unwrap();

        let fetched = svc.get_epic(epic.id.0).unwrap();
        assert_eq!(fetched.title, "Epic 1");
    }

    #[test]
    fn get_epic_not_found() {
        let db = test_db();
        let svc = epic_svc(&db);
        let err = svc.get_epic(999).unwrap_err();
        assert!(matches!(err, ServiceError::NotFound(_)));
    }

    #[test]
    fn update_epic_status() {
        let db = test_db();
        let svc = epic_svc(&db);

        let epic = svc
            .create_epic(CreateEpicParams {
                title: "E".into(),
                description: "".into(),
                repo_path: "/repo".into(),
                sort_order: None,
            })
            .unwrap();

        svc.update_epic(UpdateEpicParams {
            epic_id: epic.id.0,
            title: None,
            description: None,
            status: Some("running".into()),
            plan: None,
            sort_order: None,
            repo_path: None,
        })
        .unwrap();

        let updated = svc.get_epic(epic.id.0).unwrap();
        assert_eq!(updated.status, TaskStatus::Running);
    }

    #[test]
    fn update_epic_no_fields_returns_error() {
        let db = test_db();
        let svc = epic_svc(&db);

        let epic = svc
            .create_epic(CreateEpicParams {
                title: "E".into(),
                description: "".into(),
                repo_path: "/repo".into(),
                sort_order: None,
            })
            .unwrap();

        let err = svc
            .update_epic(UpdateEpicParams {
                epic_id: epic.id.0,
                title: None,
                description: None,
                status: None,
                plan: None,
                sort_order: None,
                repo_path: None,
            })
            .unwrap_err();
        assert!(matches!(err, ServiceError::Validation(_)));
    }

    #[test]
    fn update_epic_invalid_status() {
        let db = test_db();
        let svc = epic_svc(&db);

        let epic = svc
            .create_epic(CreateEpicParams {
                title: "E".into(),
                description: "".into(),
                repo_path: "/repo".into(),
                sort_order: None,
            })
            .unwrap();

        let err = svc
            .update_epic(UpdateEpicParams {
                epic_id: epic.id.0,
                title: None,
                description: None,
                status: Some("bogus".into()),
                plan: None,
                sort_order: None,
                repo_path: None,
            })
            .unwrap_err();
        assert!(matches!(err, ServiceError::Validation(_)));
    }

    #[test]
    fn list_epics_with_progress() {
        let db = test_db();
        let task_svc = task_svc(&db);
        let epic_svc = epic_svc(&db);

        let epic = epic_svc
            .create_epic(CreateEpicParams {
                title: "E".into(),
                description: "".into(),
                repo_path: "/repo".into(),
                sort_order: None,
            })
            .unwrap();

        task_svc
            .create_task(CreateTaskParams {
                title: "Sub1".into(),
                description: "".into(),
                repo_path: "/repo".into(),
                plan: None,
                epic_id: Some(epic.id.0),
                sort_order: None,
                tag: None,
            })
            .unwrap();

        let list = epic_svc.list_epics_with_progress().unwrap();
        assert_eq!(list.len(), 1);
        let (_, done, total) = &list[0];
        assert_eq!(*done, 0);
        assert_eq!(*total, 1);
    }

    #[test]
    fn get_epic_with_subtasks() {
        let db = test_db();
        let task_svc = task_svc(&db);
        let epic_svc = epic_svc(&db);

        let epic = epic_svc
            .create_epic(CreateEpicParams {
                title: "E".into(),
                description: "".into(),
                repo_path: "/repo".into(),
                sort_order: None,
            })
            .unwrap();

        task_svc
            .create_task(CreateTaskParams {
                title: "Sub".into(),
                description: "".into(),
                repo_path: "/repo".into(),
                plan: None,
                epic_id: Some(epic.id.0),
                sort_order: None,
                tag: None,
            })
            .unwrap();

        let (e, subtasks) = epic_svc.get_epic_with_subtasks(epic.id.0).unwrap();
        assert_eq!(e.title, "E");
        assert_eq!(subtasks.len(), 1);
    }
}
