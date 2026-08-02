//! Consumer-facing service seams (`*ServiceApi` traits).
//!
//! Each seam is declared **once**, as a `macro_rules!` "spec" macro holding the
//! doc comments and the signature list ([`crate::task_service_api!`],
//! [`crate::epic_service_api!`], [`crate::todo_service_api!`],
//! [`crate::learning_service_api!`]). A spec macro takes the name of an
//! *emitter* macro and replays its signature list into it, so every artefact
//! derived from the surface is generated from the same tokens and cannot drift:
//!
//! | Emitter | Generates |
//! |---------|-----------|
//! | [`crate::service_api_trait!`] | the `#[async_trait]` trait declaration |
//! | [`crate::service_api_delegate!`] | the production impl, delegating via UFCS to the concrete service |
//! | [`crate::service_api_stub_trait!`] | a test-only stub trait whose methods all default to `panic!` |
//! | [`crate::service_api_stub_bridge!`] | `impl <Api> for <MockType>`, forwarding to that stub trait |
//!
//! Adding a method to a seam therefore means editing exactly one signature
//! list. Test mocks implement the *stub* trait and override only the methods
//! they exercise, so a new method never breaks an unrelated mock.
//!
//! Types in the signature lists are fully qualified (`$crate::…`) because
//! `macro_rules!` resolves type paths at the *call site*: mocks living in other
//! modules invoke the same spec macro.

// The signature lists below name their types through `$crate::…` paths, so the
// only imports this module needs are the concrete services the production impls
// delegate to.
use super::{learnings::LearningService, todos::TodoService, EpicService, TaskService};

// ---------------------------------------------------------------------------
// Emitters — consume a spec macro's signature list, emit one artefact each
// ---------------------------------------------------------------------------

/// Emit the `#[async_trait]` trait declaration for a service seam.
///
/// Invoke through a spec macro: `task_service_api!(service_api_trait);`
#[macro_export]
macro_rules! service_api_trait {
    (
        []
        $(#[$trait_attr:meta])*
        trait $Api:ident for $Concrete:ident, stub $Stub:ident;
        $(
            $(#[$method_attr:meta])*
            async fn $method:ident(&self $(, $arg:ident : $arg_ty:ty)*) -> $ret:ty;
        )*
    ) => {
        $(#[$trait_attr])*
        #[async_trait::async_trait]
        pub trait $Api: Send + Sync {
            $(
                $(#[$method_attr])*
                async fn $method(&self $(, $arg: $arg_ty)*) -> $ret;
            )*
        }
    };
}

/// Emit the production impl, delegating each method via UFCS to the concrete
/// service so the inherent methods are not shadowed.
///
/// Invoke through a spec macro: `task_service_api!(service_api_delegate);`
#[macro_export]
macro_rules! service_api_delegate {
    (
        []
        $(#[$trait_attr:meta])*
        trait $Api:ident for $Concrete:ident, stub $Stub:ident;
        $(
            $(#[$method_attr:meta])*
            async fn $method:ident(&self $(, $arg:ident : $arg_ty:ty)*) -> $ret:ty;
        )*
    ) => {
        #[async_trait::async_trait]
        impl $Api for $Concrete {
            $(
                async fn $method(&self $(, $arg: $arg_ty)*) -> $ret {
                    $Concrete::$method(self $(, $arg)*).await
                }
            )*
        }
    };
}

/// Emit a test-only stub trait mirroring the seam, where every method defaults
/// to `panic!`. Mocks implement this trait and override only what they
/// exercise; unmocked calls fail loudly instead of silently succeeding.
///
/// Invoke through a spec macro: `task_service_api!(service_api_stub_trait);`
#[macro_export]
macro_rules! service_api_stub_trait {
    (
        []
        $(#[$trait_attr:meta])*
        trait $Api:ident for $Concrete:ident, stub $Stub:ident;
        $(
            $(#[$method_attr:meta])*
            async fn $method:ident(&self $(, $arg:ident : $arg_ty:ty)*) -> $ret:ty;
        )*
    ) => {
        /// Test-only mirror of the service seam with panicking defaults.
        ///
        /// Implement this instead of the `*ServiceApi` trait in mocks, override
        /// the methods the test exercises, then bridge the mock onto the real
        /// seam with `service_api_stub_bridge`.
        #[async_trait::async_trait]
        pub trait $Stub: Send + Sync {
            $(
                async fn $method(&self $(, _: $arg_ty)*) -> $ret {
                    panic!(concat!(
                        stringify!($Stub),
                        "::",
                        stringify!($method),
                        " is not mocked — override it in the mock impl"
                    ))
                }
            )*
        }
    };
}

/// Emit `impl <Api> for <MockType>`, forwarding every method to the mock's
/// stub-trait impl.
///
/// Invoke through a spec macro:
/// `task_service_api!(service_api_stub_bridge, MockTaskService);`
#[macro_export]
macro_rules! service_api_stub_bridge {
    (
        [$Mock:ident]
        $(#[$trait_attr:meta])*
        trait $Api:ident for $Concrete:ident, stub $Stub:ident;
        $(
            $(#[$method_attr:meta])*
            async fn $method:ident(&self $(, $arg:ident : $arg_ty:ty)*) -> $ret:ty;
        )*
    ) => {
        #[async_trait::async_trait]
        impl $crate::service::api::$Api for $Mock {
            $(
                async fn $method(&self $(, $arg: $arg_ty)*) -> $ret {
                    <$Mock as $crate::service::api::$Stub>::$method(self $(, $arg)*).await
                }
            )*
        }
    };
}

// ---------------------------------------------------------------------------
// Specs — the single source of truth for each seam's surface
// ---------------------------------------------------------------------------

/// The `TaskServiceApi` surface. Pass an emitter macro name; see the module
/// docs for the available emitters.
#[macro_export]
macro_rules! task_service_api {
    ($emit:ident $(, $extra:tt)*) => {
        $crate::$emit! {
            [$($extra)*]

            /// Consumer-facing seam for task operations.
            ///
            /// Mirrors the public async surface of [`TaskService`]. Callers should hold
            /// `Arc<dyn TaskServiceApi>` so unit tests can inject a mock without spinning
            /// up a real database. See `docs/conventions.md §"Service trait narrowing"`.
            trait TaskServiceApi for TaskService, stub TaskServiceApiStub;

            async fn update_task(
                &self,
                params: $crate::service::UpdateTaskParams
            ) -> Result<$crate::service::UpdateTaskResult, $crate::service::ServiceError>;

            /// Move a task to a different epic, or detach it (`new_epic = None`).
            /// Recalculates the status of both the previous and new epic.
            async fn move_task_to_epic(
                &self,
                task_id: $crate::models::TaskId,
                new_epic: Option<$crate::models::EpicId>
            ) -> Result<(), $crate::service::ServiceError>;

            async fn cli_update_task(
                &self,
                task_id: $crate::models::TaskId,
                new_status: $crate::models::TaskStatus,
                only_if: Option<$crate::models::TaskStatus>,
                sub_status: Option<$crate::models::SubStatus>
            ) -> Result<bool, $crate::service::ServiceError>;

            async fn create_task(
                &self,
                params: $crate::service::CreateTaskParams
            ) -> Result<$crate::models::TaskId, $crate::service::ServiceError>;

            async fn create_task_returning(
                &self,
                params: $crate::service::CreateTaskParams
            ) -> Result<$crate::models::Task, $crate::service::ServiceError>;

            async fn delete_task(
                &self,
                task_id: $crate::models::TaskId
            ) -> Result<(), $crate::service::ServiceError>;

            /// Batch-update `sub_status` for many tasks in one transaction (tick-driven
            /// activity reclassification). Carries no epic-recalc obligation.
            async fn batch_patch_sub_status(
                &self,
                updates: &[($crate::models::TaskId, $crate::models::SubStatus)]
            ) -> Result<(), $crate::service::ServiceError>;

            async fn get_task(
                &self,
                task_id: $crate::models::TaskId
            ) -> Result<$crate::models::Task, $crate::service::ServiceError>;

            async fn list_tasks(
                &self,
                filter: $crate::service::ListTasksFilter
            ) -> Result<Vec<$crate::models::Task>, $crate::service::ServiceError>;

            async fn validate_wrap_up(
                &self,
                task_id: $crate::models::TaskId
            ) -> Result<$crate::models::Task, $crate::service::ServiceError>;

            /// Apply a session close as one patch and return the tmux window it
            /// cleared. `Err` means exactly "the terminal write did not land",
            /// which is what makes it safe to gate the teardown and the epic
            /// chain on — see `ExitSession` in `docs/specs/pr-workflow.allium`.
            /// Do not replace call sites with a generic `update_task`.
            async fn close_session(
                &self,
                task_id: $crate::models::TaskId,
                outcome: $crate::service::CloseSessionOutcome
            ) -> Result<$crate::service::ClosedSession, $crate::service::ServiceError>;

            async fn validate_send_message(
                &self,
                from_task_id: $crate::models::TaskId,
                to_task_id: $crate::models::TaskId
            ) -> Result<($crate::models::Task, $crate::models::Task), $crate::service::ServiceError>;

            async fn record_hook_event(
                &self,
                id: $crate::models::TaskId,
                kind: $crate::models::HookEventKind
            ) -> Result<(), $crate::service::ServiceError>;

            /// Record a subagent lifecycle event and, when it drains the last
            /// subagent for a task carrying a deferred Stop, apply that Stop.
            /// See `HookSubagentStart` / `HookSubagentStop` in
            /// `docs/specs/agent-health.allium`.
            async fn record_subagent_event(
                &self,
                id: $crate::models::TaskId,
                event: $crate::models::SubagentEvent
            ) -> Result<(), $crate::service::ServiceError>;

            /// Select and atomically claim the epic's next backlog subtask,
            /// moving it to `Running` before any provisioning happens. Exclusive
            /// under concurrency — see `AutoDispatchNextSubtask` in
            /// `docs/specs/epics.allium`.
            ///
            /// Deliberately the only selection method on this seam: a
            /// non-atomic "find the next backlog task" sibling would be an easy
            /// way to reintroduce the double-dispatch this replaced.
            async fn claim_next_backlog_task(
                &self,
                epic_id: $crate::models::EpicId
            ) -> Result<Option<$crate::models::Task>, $crate::service::ServiceError>;

            /// Atomically claim one specific `Backlog` task for dispatch,
            /// moving it to `Running` before any provisioning happens. Returns
            /// whether the claim was won; `false` (or `Err`, which writes
            /// nothing) means this caller must provision nothing and has no
            /// claim to release.
            ///
            /// Every dispatch entry point goes through this — see
            /// `DispatchClaimExclusive` in `docs/specs/dispatch.allium`. Do not
            /// add a "read the status, then dispatch" path alongside it: that is
            /// the double-provisioning hole this closes.
            async fn claim_backlog_task(
                &self,
                task_id: $crate::models::TaskId
            ) -> Result<bool, $crate::service::ServiceError>;

            /// Undo an unprovisioned claim, returning the subtask to `Backlog`.
            /// Conditional on the task still being claimed-and-unprovisioned;
            /// returns whether it applied.
            async fn release_claim(
                &self,
                task_id: $crate::models::TaskId
            ) -> Result<bool, $crate::service::ServiceError>;

            async fn subscribe_to_task(
                &self,
                watcher_task_id: $crate::models::TaskId,
                target_task_id: $crate::models::TaskId
            ) -> Result<$crate::service::SubscribeOutcome, $crate::service::ServiceError>;

            async fn unsubscribe_from_task(
                &self,
                watcher_task_id: $crate::models::TaskId,
                target_task_id: $crate::models::TaskId
            ) -> Result<(), $crate::service::ServiceError>;
        }
    };
}

/// The `EpicServiceApi` surface. Pass an emitter macro name; see the module
/// docs for the available emitters.
#[macro_export]
macro_rules! epic_service_api {
    ($emit:ident $(, $extra:tt)*) => {
        $crate::$emit! {
            [$($extra)*]

            /// Consumer-facing seam for epic operations.
            ///
            /// Mirrors the public async surface of [`EpicService`]. See
            /// `docs/conventions.md §"Service trait narrowing"`.
            trait EpicServiceApi for EpicService, stub EpicServiceApiStub;

            async fn create_epic(
                &self,
                params: $crate::service::CreateEpicParams
            ) -> Result<$crate::models::Epic, $crate::service::ServiceError>;

            async fn get_epic(
                &self,
                epic_id: $crate::models::EpicId
            ) -> Result<$crate::models::Epic, $crate::service::ServiceError>;

            async fn get_epic_with_subtasks(
                &self,
                epic_id: $crate::models::EpicId
            ) -> Result<($crate::models::Epic, Vec<$crate::models::Task>), $crate::service::ServiceError>;

            async fn get_epic_with_progress(
                &self,
                epic_id: $crate::models::EpicId
            ) -> Result<($crate::models::Epic, usize, usize), $crate::service::ServiceError>;

            async fn list_epics(
                &self
            ) -> Result<Vec<$crate::models::Epic>, $crate::service::ServiceError>;

            async fn list_root_epics(
                &self
            ) -> Result<Vec<$crate::models::Epic>, $crate::service::ServiceError>;

            async fn list_sub_epics(
                &self,
                parent_id: $crate::models::EpicId
            ) -> Result<Vec<$crate::models::Epic>, $crate::service::ServiceError>;

            async fn list_epics_with_progress(
                &self
            ) -> Result<Vec<($crate::models::Epic, usize, usize)>, $crate::service::ServiceError>;

            async fn update_epic(
                &self,
                params: $crate::service::UpdateEpicParams
            ) -> Result<$crate::service::UpdateEpicResult, $crate::service::ServiceError>;

            async fn delete_epic(
                &self,
                epic_id: $crate::models::EpicId
            ) -> Result<(), $crate::service::ServiceError>;

            async fn regroup_epic(
                &self,
                root: $crate::models::EpicId
            ) -> Result<(), $crate::service::ServiceError>;

            async fn flatten_epic(
                &self,
                root: $crate::models::EpicId
            ) -> Result<(), $crate::service::ServiceError>;

            async fn reroute_on_repo_change(
                &self,
                task: $crate::models::TaskId,
                new_repo: &str
            ) -> Result<(), $crate::service::ServiceError>;

            /// Materialise the managed feed-epic tree from already-read settings. The
            /// caller reads the four settings via its read-only handle
            /// ([`crate::service::read_managed_feed_settings`]) and passes them in; the
            /// epic writes happen here, behind the service boundary.
            async fn provision_managed_feeds(
                &self,
                settings: $crate::service::ManagedFeedSettings
            ) -> Result<(), $crate::service::ServiceError>;
        }
    };
}

/// The `TodoServiceApi` surface. Pass an emitter macro name; see the module
/// docs for the available emitters.
#[macro_export]
macro_rules! todo_service_api {
    ($emit:ident $(, $extra:tt)*) => {
        $crate::$emit! {
            [$($extra)*]

            /// Consumer-facing seam for todo operations.
            ///
            /// Mirrors the public async surface of [`TodoService`]. See
            /// `docs/conventions.md §"Service trait narrowing"`.
            trait TodoServiceApi for TodoService, stub TodoServiceApiStub;

            async fn list_todos(
                &self
            ) -> Result<Vec<$crate::models::Todo>, $crate::service::ServiceError>;

            async fn create_todo(
                &self,
                title: String,
                linked: Option<$crate::models::TodoLink>
            ) -> Result<$crate::models::Todo, $crate::service::ServiceError>;

            async fn update_todo(
                &self,
                id: $crate::models::TodoId,
                update: $crate::service::TodoUpdate
            ) -> Result<(), $crate::service::ServiceError>;

            async fn delete_todo(
                &self,
                id: $crate::models::TodoId
            ) -> Result<(), $crate::service::ServiceError>;

            async fn clear_done(&self) -> Result<(), $crate::service::ServiceError>;
        }
    };
}

/// The `LearningServiceApi` surface. Pass an emitter macro name; see the module
/// docs for the available emitters.
#[macro_export]
macro_rules! learning_service_api {
    ($emit:ident $(, $extra:tt)*) => {
        $crate::$emit! {
            [$($extra)*]

            /// Consumer-facing seam for learning operations.
            ///
            /// Mirrors the public async surface of [`LearningService`]. Callers should hold
            /// `Arc<dyn LearningServiceApi>` so unit tests can inject a mock without spinning
            /// up a real database. See `docs/conventions.md §"Service trait narrowing"`.
            trait LearningServiceApi for LearningService, stub LearningServiceApiStub;

            async fn create_learning(
                &self,
                params: $crate::service::CreateLearningParams
            ) -> Result<$crate::models::LearningId, $crate::service::ServiceError>;

            async fn get_learning(
                &self,
                id: $crate::models::LearningId
            ) -> Result<$crate::models::Learning, $crate::service::ServiceError>;

            async fn list_learnings(
                &self,
                filter: $crate::db::LearningFilter
            ) -> Result<Vec<$crate::models::Learning>, $crate::service::ServiceError>;

            async fn reject_learning(
                &self,
                id: $crate::models::LearningId
            ) -> Result<(), $crate::service::ServiceError>;

            async fn archive_learning(
                &self,
                id: $crate::models::LearningId
            ) -> Result<(), $crate::service::ServiceError>;

            async fn update_learning(
                &self,
                params: $crate::service::UpdateLearningParams
            ) -> Result<(), $crate::service::ServiceError>;

            async fn record_retrieval(
                &self,
                task_id: $crate::models::TaskId,
                learning_id: $crate::models::LearningId,
                source: $crate::models::RetrievalSource
            ) -> Result<(), $crate::service::ServiceError>;

            async fn apply_verdicts(
                &self,
                task_id: $crate::models::TaskId,
                verdicts: Vec<($crate::models::LearningId, $crate::models::LearningVerdict)>
            ) -> Result<(), $crate::service::ServiceError>;

            async fn archive_stale_learnings(
                &self,
                cutoff: chrono::DateTime<chrono::Utc>
            ) -> Result<u64, $crate::service::ServiceError>;

            async fn delete_learning(
                &self,
                id: $crate::models::LearningId
            ) -> Result<(), $crate::service::ServiceError>;

            async fn query_learnings(
                &self,
                params: $crate::service::QueryLearningsParams
            ) -> Result<Vec<$crate::models::Learning>, $crate::service::ServiceError>;
        }
    };
}

// ---------------------------------------------------------------------------
// Trait declarations
// ---------------------------------------------------------------------------

task_service_api!(service_api_trait);
epic_service_api!(service_api_trait);
todo_service_api!(service_api_trait);
learning_service_api!(service_api_trait);

// ---------------------------------------------------------------------------
// Production impls — delegate to the concrete structs
// ---------------------------------------------------------------------------

task_service_api!(service_api_delegate);
epic_service_api!(service_api_delegate);
todo_service_api!(service_api_delegate);
learning_service_api!(service_api_delegate);

// ---------------------------------------------------------------------------
// Test-only stub traits — mocks override only what they exercise
// ---------------------------------------------------------------------------

#[cfg(test)]
task_service_api!(service_api_stub_trait);
#[cfg(test)]
learning_service_api!(service_api_stub_trait);

/// No-op [`LearningServiceApi`] for tests that construct a [`TuiRuntime`] or
/// [`McpState`] but never exercise learning operations. Overrides nothing, so
/// every method keeps `LearningServiceApiStub`'s panicking default — accidental
/// learning-service calls in non-learning tests are caught.
#[cfg(test)]
pub struct MockLearningService;

#[cfg(test)]
impl LearningServiceApiStub for MockLearningService {}

#[cfg(test)]
learning_service_api!(service_api_stub_bridge, MockLearningService);

// ---------------------------------------------------------------------------
// Tests
// ---------------------------------------------------------------------------

#[cfg(test)]
#[allow(clippy::unwrap_used, clippy::expect_used)]
mod tests {
    use super::*;
    use crate::db::Database;
    use crate::models::{Task, TaskId};
    use crate::service::{
        CreateEpicParams, CreateLearningParams, CreateTaskParams, ListTasksFilter, ServiceError,
    };
    use std::sync::Arc;

    async fn store() -> Arc<dyn crate::db::TaskAndEpicStore> {
        Arc::new(Database::open_in_memory().await.unwrap())
    }

    // -----------------------------------------------------------------------
    // Delegation guards — every `*Api` trait must reach the concrete service
    // -----------------------------------------------------------------------

    #[tokio::test]
    async fn task_service_api_delegates_to_task_service() {
        let svc: Arc<dyn TaskServiceApi> = Arc::new(TaskService::new(
            store().await,
            crate::process::MockProcessRunner::unused(),
        ));

        let id = svc
            .create_task(CreateTaskParams {
                title: "delegated".to_string(),
                description: String::new(),
                repo_path: "/repo".to_string(),
                plan_path: None,
                epic_id: None,
                sort_order: None,
                tag: None,
                base_branch: None,
                wrap_up_mode: None,
                auto_run_plan: false,
            })
            .await
            .unwrap();

        assert_eq!(svc.get_task(id).await.unwrap().title, "delegated");
        assert_eq!(
            svc.list_tasks(ListTasksFilter::default())
                .await
                .unwrap()
                .len(),
            1
        );
    }

    #[tokio::test]
    async fn epic_service_api_delegates_to_epic_service() {
        let svc: Arc<dyn EpicServiceApi> = Arc::new(EpicService::new(store().await));

        let epic = svc
            .create_epic(CreateEpicParams {
                title: "delegated epic".to_string(),
                description: String::new(),
                sort_order: None,
                parent_epic_id: None,
                feed_command: None,
                feed_interval_secs: None,
            })
            .await
            .unwrap();

        assert_eq!(svc.get_epic(epic.id).await.unwrap().title, "delegated epic");
        assert_eq!(svc.list_epics().await.unwrap().len(), 1);
    }

    #[tokio::test]
    async fn todo_service_api_delegates_to_todo_service() {
        let db = Arc::new(Database::open_in_memory().await.unwrap());
        let svc: Arc<dyn TodoServiceApi> = Arc::new(TodoService::new(db));

        let todo = svc
            .create_todo("delegated todo".to_string(), None)
            .await
            .unwrap();

        assert_eq!(svc.list_todos().await.unwrap().len(), 1);
        svc.delete_todo(todo.id).await.unwrap();
        assert!(svc.list_todos().await.unwrap().is_empty());
    }

    #[tokio::test]
    async fn learning_service_api_delegates_to_learning_service() {
        let db: Arc<dyn crate::db::TaskStore> = Arc::new(Database::open_in_memory().await.unwrap());
        let svc: Arc<dyn LearningServiceApi> = Arc::new(LearningService::new(
            db,
            crate::service::embeddings::EmbeddingService::new_test(),
        ));

        let id = svc
            .create_learning(CreateLearningParams {
                kind: crate::models::LearningKind::Convention,
                summary: "delegated learning".to_string(),
                detail: None,
                scope: crate::models::LearningScope::Repo,
                scope_ref: Some("/repo".to_string()),
                tags: vec![],
                source_task_id: None,
            })
            .await
            .unwrap();

        assert_eq!(
            svc.get_learning(id).await.unwrap().summary,
            "delegated learning"
        );
    }

    // -----------------------------------------------------------------------
    // Stub-trait machinery — mocks declare only the methods they exercise
    // -----------------------------------------------------------------------

    /// A partial mock: overrides one method, inherits panicking defaults for
    /// the rest. Adding a method to `TaskServiceApi` must not break this.
    struct PartialTaskStub;

    #[async_trait::async_trait]
    impl TaskServiceApiStub for PartialTaskStub {
        async fn list_tasks(&self, _: ListTasksFilter) -> Result<Vec<Task>, ServiceError> {
            Ok(vec![])
        }
    }

    crate::task_service_api!(service_api_stub_bridge, PartialTaskStub);

    #[tokio::test]
    async fn stub_override_is_reached_through_the_api_trait() {
        let svc: Arc<dyn TaskServiceApi> = Arc::new(PartialTaskStub);

        assert!(svc
            .list_tasks(ListTasksFilter::default())
            .await
            .unwrap()
            .is_empty());
    }

    #[tokio::test]
    #[should_panic(expected = "not mocked")]
    async fn stub_default_panics_for_a_method_the_mock_did_not_override() {
        let svc: Arc<dyn TaskServiceApi> = Arc::new(PartialTaskStub);

        let _ = svc.get_task(TaskId(1)).await;
    }
}
