//! Task-watcher subscriptions: an agent can subscribe to another task and
//! be notified once it finishes (`Done`/`Archived`) or is deleted first.
//! See `docs/specs/task-watchers.allium`.

use crate::models::{Task, TaskId, TaskStatus};
use crate::service::ServiceError;

use super::crud::TaskService;

/// Result of [`TaskService::subscribe_to_task`].
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum SubscribeOutcome {
    /// A new (or already-existing, unfinished) subscription is in place.
    Subscribed,
    /// The target had already reached this status before subscribing — no
    /// subscription was created since there's nothing left to wait for.
    AlreadyFinished(TaskStatus),
}

impl TaskService {
    pub async fn subscribe_to_task(
        &self,
        watcher_task_id: TaskId,
        target_task_id: TaskId,
    ) -> Result<SubscribeOutcome, ServiceError> {
        if watcher_task_id == target_task_id {
            return Err(ServiceError::Validation(
                "a task cannot watch itself".into(),
            ));
        }
        self.get_task(watcher_task_id).await?; // ensures watcher exists
        let target = self.get_task(target_task_id).await?;

        if matches!(target.status, TaskStatus::Done | TaskStatus::Archived) {
            return Ok(SubscribeOutcome::AlreadyFinished(target.status));
        }

        self.db
            .create_task_watcher(watcher_task_id, target_task_id)
            .await?;
        Ok(SubscribeOutcome::Subscribed)
    }

    pub async fn unsubscribe_from_task(
        &self,
        watcher_task_id: TaskId,
        target_task_id: TaskId,
    ) -> Result<(), ServiceError> {
        self.db
            .delete_task_watcher(watcher_task_id, target_task_id)
            .await
            .map_err(ServiceError::from)
    }

    /// Called after a task's status is persisted. No-ops unless the status
    /// actually changed to a finished state (`Done`/`Archived`).
    pub(super) async fn notify_watchers_if_finished(
        &self,
        task_id: TaskId,
        old_status: TaskStatus,
        new_status: TaskStatus,
    ) {
        if old_status == new_status
            || !matches!(new_status, TaskStatus::Done | TaskStatus::Archived)
        {
            return;
        }
        let Ok(watcher_ids) = self.db.list_watchers_of(task_id).await else {
            tracing::warn!(
                task_id = task_id.0,
                "failed to list watchers for finished task"
            );
            return;
        };
        if watcher_ids.is_empty() {
            return;
        }
        let Ok(Some(target)) = self.db.get_task(task_id).await else {
            tracing::warn!(
                task_id = task_id.0,
                "finished task disappeared before notifying watchers"
            );
            return;
        };
        let body = format!(
            "Task {} (\"{}\") that you were watching has reached status '{}'.",
            target.id.0,
            target.title,
            new_status.as_str()
        );
        for watcher_id in watcher_ids {
            self.deliver_watch_notification(watcher_id, target.id, &body, "finished")
                .await;
        }
        if let Err(e) = self.db.delete_watches_of_target(task_id).await {
            tracing::warn!(
                task_id = task_id.0,
                "failed to clean up watch rows after firing: {e}"
            );
        }
    }

    /// Called before a task is hard-deleted. Notifies watchers that it was
    /// deleted (not finished), then removes every watch row involving it
    /// (as target or as watcher).
    ///
    /// Unused until Tasks 8-9 wire this into `delete_task`.
    #[allow(dead_code)]
    pub(super) async fn notify_watchers_of_deletion(&self, deleted: &Task) {
        match self.db.list_watchers_of(deleted.id).await {
            Ok(watcher_ids) => {
                let body = format!(
                    "Task {} (\"{}\") that you were watching was deleted before it finished.",
                    deleted.id.0, deleted.title
                );
                for watcher_id in watcher_ids {
                    self.deliver_watch_notification(watcher_id, deleted.id, &body, "deleted")
                        .await;
                }
            }
            Err(e) => tracing::warn!(
                task_id = deleted.id.0,
                "failed to list watchers for deleted task: {e}"
            ),
        }
        if let Err(e) = self.db.delete_watches_of_target(deleted.id).await {
            tracing::warn!(
                task_id = deleted.id.0,
                "failed to clean up target watch rows on delete: {e}"
            );
        }
        if let Err(e) = self.db.delete_watches_by_watcher(deleted.id).await {
            tracing::warn!(
                task_id = deleted.id.0,
                "failed to clean up watcher-side watch rows on delete: {e}"
            );
        }
    }

    /// Deliver a one-shot notification to `watcher_id`'s tmux window, if it
    /// has one. Logs and drops (no error propagated) if the watcher has no
    /// live tmux window — this is a best-effort nudge, not a durable queue.
    ///
    /// Unused until Tasks 8-9 wire callers that invoke the two methods above.
    #[allow(dead_code)]
    async fn deliver_watch_notification(
        &self,
        watcher_id: TaskId,
        target_id: TaskId,
        body: &str,
        kind: &str,
    ) {
        let Ok(Some(watcher)) = self.db.get_task(watcher_id).await else {
            tracing::warn!(
                watcher_id = watcher_id.0,
                "watcher task disappeared before notification"
            );
            return;
        };
        let (Some(worktree), Some(tmux_window)) =
            (watcher.worktree.clone(), watcher.tmux_window.clone())
        else {
            tracing::warn!(
                watcher_id = watcher_id.0,
                "watcher has no live tmux window; dropping watch notification"
            );
            return;
        };

        let runner = self.runner.clone();
        let body = body.to_string();
        let kind = kind.to_string();
        let file_prefix = format!("watch-{kind}-{}", target_id.0);
        let result = tokio::task::spawn_blocking(move || {
            let filename = crate::notify::write_message_file(&worktree, &file_prefix, &body)?;
            let text = format!(
                "The task you were watching (#{}) just {kind}. Read .claude-messages/{filename} for details, then delete the file.",
                target_id.0
            );
            crate::notify::notify_tmux(&*runner, &worktree, &tmux_window, &filename, &text)
        })
        .await;

        match result {
            Ok(Ok(())) => {}
            Ok(Err(e)) => tracing::warn!(
                watcher_id = watcher_id.0,
                "failed to deliver watch notification: {e}"
            ),
            Err(e) => tracing::warn!(
                watcher_id = watcher_id.0,
                "watch notification delivery task panicked: {e}"
            ),
        }
    }
}
