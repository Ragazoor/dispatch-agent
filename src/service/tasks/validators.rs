//! Pure helpers that translate validated `UpdateTaskParams` into a
//! `TaskPatch` for the database layer.
//!
//! Methods that need to read DB state (sub-status legality, epic linkage,
//! etc.) live on `TaskService` in `crud.rs` because they take `&self`.

use super::params::UpdateTaskParams;
use crate::db::TaskPatch;
use crate::models::SubStatus;

/// Build a `TaskPatch` from `UpdateTaskParams`. The expanded repo path and
/// the (already-validated) sub_status are passed in separately because they
/// require either tilde-expansion or a database-bound check before being
/// committed to the patch.
pub(super) fn build_task_patch<'a>(
    params: &'a UpdateTaskParams,
    expanded_repo_path: Option<&'a str>,
    sub_status: Option<SubStatus>,
) -> TaskPatch<'a> {
    let mut patch = TaskPatch::new();
    if let Some(s) = params.status {
        patch = patch.status(s);
    }
    if let Some(update) = params.plan_path.as_ref() {
        patch = patch.plan_path(update.as_option());
    }
    if let Some(t) = params.title.as_deref() {
        patch = patch.title(t);
    }
    if let Some(d) = params.description.as_deref() {
        patch = patch.description(d);
    }
    if let Some(r) = expanded_repo_path {
        patch = patch.repo_path(r);
    }
    if let Some(so) = params.sort_order {
        patch = patch.sort_order(Some(so));
    }
    if let Some(update) = params.url.as_ref() {
        patch = patch.url(update.as_option());
    }
    if let Some(inner) = params.tag {
        patch = patch.tag(inner);
    }
    if let Some(update) = params.worktree.as_ref() {
        patch = patch.worktree(update.as_option());
    }
    if let Some(update) = params.tmux_window.as_ref() {
        patch = patch.tmux_window(update.as_option());
    }
    if let Some(bb) = params.base_branch.as_deref() {
        patch = patch.base_branch(bb);
    }
    if let Some(ss) = sub_status {
        patch = patch.sub_status(ss);
    }
    if let Some(ts) = params.last_pre_tool_use_at {
        patch = patch.last_pre_tool_use_at(ts);
    }
    if let Some(inner) = params.wrap_up_mode {
        patch = patch.wrap_up_mode(inner);
    }
    if let Some(v) = params.auto_run_plan {
        patch = patch.auto_run_plan(v);
    }
    patch
}

#[cfg(test)]
#[allow(clippy::unwrap_used, clippy::expect_used)]
mod tests {
    use super::super::params::UpdateTaskParams;
    use super::build_task_patch;
    use crate::models::{SubStatus, TaskId, TaskStatus, TaskTag, WrapUpMode};

    #[test]
    fn all_none_produces_empty_patch() {
        let params = UpdateTaskParams::for_task(TaskId(1));
        let patch = build_task_patch(&params, None, None);
        assert!(!patch.has_changes());
    }

    #[test]
    fn status_mapped_to_plain_field() {
        let params = UpdateTaskParams::for_task(TaskId(1)).status(TaskStatus::Running);
        let patch = build_task_patch(&params, None, None);
        assert_eq!(patch.status, Some(TaskStatus::Running));
    }

    #[test]
    fn plan_path_set_as_nullable_some() {
        use crate::service::FieldUpdate;
        let params = UpdateTaskParams::for_task(TaskId(1))
            .plan_path(FieldUpdate::Set("docs/plans/foo.md".to_string()));
        let patch = build_task_patch(&params, None, None);
        assert_eq!(patch.plan_path, Some(Some("docs/plans/foo.md")));
    }

    #[test]
    fn plan_path_clear_maps_to_nullable_none() {
        use crate::service::FieldUpdate;
        let params = UpdateTaskParams::for_task(TaskId(1)).plan_path(FieldUpdate::Clear);
        let patch = build_task_patch(&params, None, None);
        assert_eq!(patch.plan_path, Some(None));
    }

    #[test]
    fn title_and_description_mapped_to_plain_fields() {
        let params = UpdateTaskParams::for_task(TaskId(1))
            .title("New title".to_string())
            .description("New desc".to_string());
        let patch = build_task_patch(&params, None, None);
        assert_eq!(patch.title, Some("New title"));
        assert_eq!(patch.description, Some("New desc"));
    }

    #[test]
    fn repo_path_comes_from_expanded_param_not_params_field() {
        let params =
            UpdateTaskParams::for_task(TaskId(1)).repo_path("/unexpanded/~/path".to_string());
        let patch = build_task_patch(&params, Some("/expanded/path"), None);
        assert_eq!(patch.repo_path, Some("/expanded/path"));
    }

    #[test]
    fn repo_path_not_set_when_expanded_is_none() {
        let params = UpdateTaskParams::for_task(TaskId(1)).repo_path("/some/path".to_string());
        let patch = build_task_patch(&params, None, None);
        assert_eq!(patch.repo_path, None);
    }

    #[test]
    fn sort_order_set_as_nullable_some() {
        let params = UpdateTaskParams::for_task(TaskId(1)).sort_order(42);
        let patch = build_task_patch(&params, None, None);
        assert_eq!(patch.sort_order, Some(Some(42)));
    }

    #[test]
    fn url_set_maps_to_nullable_some() {
        use crate::models::{TaskUrl, UrlType};
        use crate::service::UrlUpdate;
        let u = TaskUrl::new("https://github.com/o/r/pull/3", UrlType::Pr);
        let params = UpdateTaskParams::for_task(TaskId(1)).url(UrlUpdate::Set(u.clone()));
        let patch = build_task_patch(&params, None, None);
        assert_eq!(patch.url, Some(Some(&u)));
    }

    #[test]
    fn url_clear_maps_to_nullable_none() {
        use crate::service::UrlUpdate;
        let params = UpdateTaskParams::for_task(TaskId(1)).url(UrlUpdate::Clear);
        let patch = build_task_patch(&params, None, None);
        assert_eq!(patch.url, Some(None));
    }

    #[test]
    fn tag_set_as_nullable_some() {
        let params = UpdateTaskParams::for_task(TaskId(1)).tag(Some(Some(TaskTag::Bug)));
        let patch = build_task_patch(&params, None, None);
        assert_eq!(patch.tag, Some(Some(TaskTag::Bug)));
    }

    #[test]
    fn tag_clear_maps_to_nullable_none() {
        let params = UpdateTaskParams::for_task(TaskId(1)).tag(Some(None));
        let patch = build_task_patch(&params, None, None);
        assert_eq!(patch.tag, Some(None));
    }

    #[test]
    fn base_branch_mapped_to_plain_field() {
        let params = UpdateTaskParams::for_task(TaskId(1)).base_branch(Some("develop".to_string()));
        let patch = build_task_patch(&params, None, None);
        assert_eq!(patch.base_branch, Some("develop"));
    }

    #[test]
    fn sub_status_comes_from_parameter_not_params_field() {
        // The sub_status argument to build_task_patch is the pre-validated value;
        // params.sub_status is not read directly.
        let params = UpdateTaskParams::for_task(TaskId(1));
        let patch = build_task_patch(&params, None, Some(SubStatus::Active));
        assert_eq!(patch.sub_status, Some(SubStatus::Active));
    }

    #[test]
    fn sub_status_not_set_when_parameter_is_none() {
        let params = UpdateTaskParams::for_task(TaskId(1));
        let patch = build_task_patch(&params, None, None);
        assert_eq!(patch.sub_status, None);
    }

    #[test]
    fn last_pre_tool_use_at_set_when_some_provided() {
        let ts = chrono::Utc::now();
        let params = UpdateTaskParams::for_task(TaskId(1)).last_pre_tool_use_at(Some(ts));
        let patch = build_task_patch(&params, None, None);
        assert_eq!(patch.last_pre_tool_use_at, Some(Some(ts)));
    }

    #[test]
    fn last_pre_tool_use_at_cleared_when_none_provided() {
        let params = UpdateTaskParams::for_task(TaskId(1)).last_pre_tool_use_at(None);
        let patch = build_task_patch(&params, None, None);
        assert_eq!(patch.last_pre_tool_use_at, Some(None));
    }

    #[test]
    fn wrap_up_mode_set_when_some_provided() {
        let params = UpdateTaskParams::for_task(TaskId(1)).wrap_up_mode(Some(WrapUpMode::Rebase));
        let patch = build_task_patch(&params, None, None);
        assert_eq!(patch.wrap_up_mode, Some(Some(WrapUpMode::Rebase)));
    }

    #[test]
    fn wrap_up_mode_cleared_when_none_provided() {
        let params = UpdateTaskParams::for_task(TaskId(1)).wrap_up_mode(None);
        let patch = build_task_patch(&params, None, None);
        assert_eq!(patch.wrap_up_mode, Some(None));
    }

    #[test]
    fn auto_run_plan_maps_to_patch() {
        let params = UpdateTaskParams::for_task(TaskId(1)).auto_run_plan(true);
        let patch = build_task_patch(&params, None, None);
        assert_eq!(patch.auto_run_plan, Some(true));
    }
}
