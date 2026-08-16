use std::sync::{Arc, LazyLock};

use regex::Regex;

use crate::db::{self, CreateLearningRow, LearningFilter};
use crate::models::{
    Learning, LearningId, LearningKind, LearningScope, LearningVerdict, RetrievalSource, TaskId,
};
use crate::service::embeddings::{
    deserialize_candidate_rows, embed_text_for_learning, embed_text_for_query, rag_rank_learnings,
    serialize_embedding, EmbeddingService, RagRankParams, RAG_SIMILARITY_THRESHOLD,
};

use super::ServiceError;

// ---------------------------------------------------------------------------
// Internal-code citation detection
// ---------------------------------------------------------------------------

// Mirrors three of check-doc-symbols.sh's four candidate shapes (pathsym,
// typesym, bare) — see docs/specs/learnings.allium: RecordLearningViaMcp for
// why the fourth shape (bare backticked snake_case) is deliberately excluded.
static PATHSYM_RE: LazyLock<Regex> = LazyLock::new(|| {
    Regex::new(r"[A-Za-z0-9_./-]+\.rs::[A-Za-z_][A-Za-z0-9_]*(?:::[A-Za-z_][A-Za-z0-9_]*)*")
        .unwrap_or_else(|e| unreachable!("PATHSYM_RE is a hardcoded pattern: {e}"))
});

static TYPESYM_RE: LazyLock<Regex> = LazyLock::new(|| {
    Regex::new(r"[A-Z][A-Za-z0-9]*(?:::[A-Za-z_][A-Za-z0-9_]*)+")
        .unwrap_or_else(|e| unreachable!("TYPESYM_RE is a hardcoded pattern: {e}"))
});

// At least four underscores (five word segments) — the same threshold
// check-doc-symbols.sh measured as the lowest value with zero false positives
// across the docs/specs/ corpus.
static BARE_RE: LazyLock<Regex> = LazyLock::new(|| {
    Regex::new(r"[a-z][a-z0-9]*(?:_[a-z0-9]+){4,}")
        .unwrap_or_else(|e| unreachable!("BARE_RE is a hardcoded pattern: {e}"))
});

/// Detects an internal-code-shaped citation in learning text: a
// allow-phantom-symbol: describes the citation shape itself, not a real reference
/// `path.rs::symbol` reference, a `Type::method` reference, or a long (5+
/// segment) bare snake_case identifier. Returns the offending substring on a
/// match. See docs/specs/learnings.allium: RecordLearningViaMcp for the
/// rationale, including why short backticked identifiers (MCP tool names)
/// are deliberately not flagged.
fn find_code_citation(text: &str) -> Option<&str> {
    PATHSYM_RE
        .find(text)
        .or_else(|| TYPESYM_RE.find(text))
        .or_else(|| BARE_RE.find(text))
        .map(|m| m.as_str())
}

/// Rejects `field` (the "summary" or "detail" text of a learning) if it
/// contains an internal-code citation. See [`find_code_citation`].
fn reject_code_citation(field: &str, text: &str) -> Result<(), ServiceError> {
    if let Some(hit) = find_code_citation(text) {
        return Err(ServiceError::Validation(format!(
            "learning {field} cites internal code (`{hit}`) — this rots silently since \
             nothing re-checks the knowledge base against the codebase. Describe the \
             durable behavior in prose instead, or add the citation to the relevant \
             docs/specs/*.allium file or a Rust doc comment, both of which \
             check-doc-symbols.sh keeps accurate on every push."
        )));
    }
    Ok(())
}

// ---------------------------------------------------------------------------
// QueryLearningsParams
// ---------------------------------------------------------------------------

pub struct QueryLearningsParams {
    pub task_id: TaskId,
    /// Semantic query text. `None` derives it from the task's title + description.
    pub query: Option<String>,
    pub tag_filter: Vec<String>,
    pub limit: usize,
}

// ---------------------------------------------------------------------------
// CreateLearningParams
// ---------------------------------------------------------------------------

pub struct CreateLearningParams {
    pub kind: LearningKind,
    pub summary: String,
    pub detail: Option<String>,
    pub scope: LearningScope,
    pub scope_ref: Option<String>,
    pub tags: Vec<String>,
    pub source_task_id: Option<TaskId>,
}

// ---------------------------------------------------------------------------
// LearningService
// ---------------------------------------------------------------------------

pub struct LearningService {
    pub db: Arc<dyn db::TaskStore>,
    embedding_service: Arc<EmbeddingService>,
}

impl LearningService {
    pub fn new(db: Arc<dyn db::TaskStore>, embedding_service: Arc<EmbeddingService>) -> Self {
        Self {
            db,
            embedding_service,
        }
    }

    pub async fn create_learning(
        &self,
        params: CreateLearningParams,
    ) -> Result<LearningId, ServiceError> {
        if params.summary.trim().is_empty() {
            return Err(ServiceError::Validation("summary must not be empty".into()));
        }
        reject_code_citation("summary", &params.summary)?;
        if let Some(detail) = &params.detail {
            reject_code_citation("detail", detail)?;
        }
        match params.scope {
            LearningScope::User => {
                if params.scope_ref.is_some() {
                    return Err(ServiceError::Validation(
                        "scope_ref must be null for user-scoped learnings".into(),
                    ));
                }
            }
            _ => {
                if params.scope_ref.is_none() {
                    return Err(ServiceError::Validation(
                        "scope_ref is required for non-user-scoped learnings".into(),
                    ));
                }
            }
        }
        let text = embed_text_for_learning(
            params.kind,
            &params.summary,
            &params.tags,
            params.detail.as_deref(),
        );
        let emb_vec = self
            .embedding_service
            .embed(text)
            .await
            .map_err(ServiceError::from)?;
        let emb_bytes = serialize_embedding(&emb_vec);
        self.db
            .create_learning(CreateLearningRow {
                kind: params.kind,
                summary: &params.summary,
                detail: params.detail.as_deref(),
                scope: params.scope,
                scope_ref: params.scope_ref.as_deref(),
                tags: &params.tags,
                source_task_id: params.source_task_id,
                embedding: Some(&emb_bytes),
            })
            .await
            .map_err(ServiceError::from)
    }

    pub async fn get_learning(&self, id: LearningId) -> Result<Learning, ServiceError> {
        self.db
            .get_learning(id)
            .await?
            .ok_or_else(|| ServiceError::NotFound(format!("learning {id} not found")))
    }

    pub async fn list_learnings(
        &self,
        filter: LearningFilter,
    ) -> Result<Vec<Learning>, ServiceError> {
        self.db
            .list_learnings(filter)
            .await
            .map_err(ServiceError::from)
    }

    pub async fn record_retrieval(
        &self,
        task_id: TaskId,
        learning_id: LearningId,
        source: RetrievalSource,
    ) -> Result<(), ServiceError> {
        self.db
            .record_retrieval(task_id, learning_id, source)
            .await
            .map_err(ServiceError::from)
    }

    pub async fn apply_verdicts(
        &self,
        task_id: TaskId,
        verdicts: Vec<(LearningId, LearningVerdict)>,
    ) -> Result<(), ServiceError> {
        if verdicts.is_empty() {
            return Ok(());
        }
        let retrieved: std::collections::HashSet<LearningId> = self
            .db
            .list_retrievals_for_task(task_id)
            .await?
            .into_iter()
            .map(|r| r.learning_id)
            .collect();
        for (lid, _) in &verdicts {
            if !retrieved.contains(lid) {
                return Err(ServiceError::Validation(format!(
                    "learning {} was not retrieved during task {}",
                    lid, task_id
                )));
            }
        }
        self.db
            .apply_verdicts_tx(&verdicts)
            .await
            .map_err(ServiceError::from)
    }

    /// Archive approved learnings that have gone stale with a non-positive score
    /// (upvote_count <= 0 and updated_at <= cutoff). Returns the number archived.
    /// See docs/specs/learnings.allium: ArchiveStaleLearning.
    pub async fn archive_stale_learnings(
        &self,
        cutoff: chrono::DateTime<chrono::Utc>,
    ) -> Result<u64, ServiceError> {
        self.db
            .archive_stale_learnings(cutoff)
            .await
            .map_err(ServiceError::from)
    }

    pub async fn delete_learning(&self, id: LearningId) -> Result<(), ServiceError> {
        let deleted = self.db.delete_learning(id).await?;
        if !deleted {
            return Err(ServiceError::NotFound(format!("learning {id} not found")));
        }
        Ok(())
    }

    /// Rank approved learnings against a task's context and record a retrieval
    /// for each one returned, so `rate_learning` can later validate a verdict
    /// against what was actually surfaced. Owns the fetch→embed→rank→record
    /// orchestration so it is testable without going through JSON-RPC and is
    /// reachable from non-MCP callers (e.g. the TUI) if ever needed.
    pub async fn query_learnings(
        &self,
        params: QueryLearningsParams,
    ) -> Result<Vec<Learning>, ServiceError> {
        let task =
            self.db.get_task(params.task_id).await?.ok_or_else(|| {
                ServiceError::NotFound(format!("task {} not found", params.task_id))
            })?;

        let query_text = params
            .query
            .unwrap_or_else(|| embed_text_for_query(&task.title, &task.description));
        let query_vec = self
            .embedding_service
            .embed(query_text)
            .await
            .map_err(ServiceError::from)?;

        let candidates_raw = self.db.list_all_approved_non_task_learnings().await?;
        let candidates = deserialize_candidate_rows(candidates_raw);

        let epic_id_str = task.epic_id.map(|e| e.0.to_string());
        let ranked = rag_rank_learnings(
            &candidates,
            &RagRankParams {
                query_vec: &query_vec,
                task_epic_id: epic_id_str.as_deref(),
                task_repo: Some(task.repo_path.as_str()),
                threshold: RAG_SIMILARITY_THRESHOLD,
                tag_filter: &params.tag_filter,
                limit: params.limit,
            },
        );

        for l in &ranked {
            if let Err(e) = self
                .db
                .record_retrieval(params.task_id, l.id, RetrievalSource::QueryLearnings)
                .await
            {
                tracing::warn!(
                    task_id = params.task_id.0,
                    learning_id = l.id.0,
                    error = ?e,
                    "failed to record learning retrieval"
                );
            }
        }

        Ok(ranked.into_iter().cloned().collect())
    }
}

// ---------------------------------------------------------------------------
// LearningService tests
// ---------------------------------------------------------------------------

#[cfg(test)]
mod learning_tests {
    #![allow(clippy::unwrap_used, clippy::expect_used)]
    use std::sync::Arc;

    use super::{CreateLearningParams, LearningService, QueryLearningsParams};
    use crate::db::{CreateTaskRequest, Database, TaskStore};
    use crate::models::{
        LearningId, LearningKind, LearningScope, LearningStatus, LearningVerdict, RetrievalSource,
        TaskId, TaskStatus,
    };
    use crate::service::embeddings::EmbeddingService;
    use crate::service::ServiceError;

    async fn service() -> LearningService {
        let db = Arc::new(Database::open_in_memory().await.unwrap());
        LearningService::new(db, EmbeddingService::new_test())
    }

    async fn service_with_db() -> (LearningService, Arc<dyn TaskStore>) {
        let db: Arc<dyn TaskStore> = Arc::new(Database::open_in_memory().await.unwrap());
        (
            LearningService::new(db.clone(), EmbeddingService::new_test()),
            db,
        )
    }

    async fn seed_task(db: &Arc<dyn TaskStore>) -> TaskId {
        db.create_task(CreateTaskRequest {
            title: "test task",
            description: "",
            repo_path: "/repo",
            plan: None,
            status: TaskStatus::Backlog,
            base_branch: "main",
            epic_id: None,
            sort_order: None,
            tag: None,
            wrap_up_mode: None,
            auto_run_plan: false,
            schedule_interval_secs: None,
            pinned_branch: None,
        })
        .await
        .unwrap()
    }

    async fn seed_approved_learning(svc: &LearningService) -> LearningId {
        svc.create_learning(CreateLearningParams {
            kind: LearningKind::Convention,
            summary: "A convention".to_string(),
            detail: None,
            scope: LearningScope::User,
            scope_ref: None,
            tags: vec![],
            source_task_id: None,
        })
        .await
        .unwrap()
    }

    #[tokio::test]
    async fn create_learning_rejects_empty_summary() {
        let svc = service().await;
        let err = svc
            .create_learning(CreateLearningParams {
                kind: LearningKind::Convention,
                summary: "".to_string(),
                detail: None,
                scope: LearningScope::User,
                scope_ref: None,
                tags: vec![],
                source_task_id: None,
            })
            .await
            .unwrap_err();
        assert!(matches!(err, ServiceError::Validation(_)));
    }

    #[tokio::test]
    async fn create_learning_rejects_user_scope_with_scope_ref() {
        let svc = service().await;
        let err = svc
            .create_learning(CreateLearningParams {
                kind: LearningKind::Preference,
                summary: "Some preference".to_string(),
                detail: None,
                scope: LearningScope::User,
                scope_ref: Some("should-be-null".to_string()),
                tags: vec![],
                source_task_id: None,
            })
            .await
            .unwrap_err();
        assert!(matches!(err, ServiceError::Validation(_)));
    }

    #[tokio::test]
    async fn create_learning_rejects_non_user_scope_without_scope_ref() {
        let svc = service().await;
        let err = svc
            .create_learning(CreateLearningParams {
                kind: LearningKind::Convention,
                summary: "A convention".to_string(),
                detail: None,
                scope: LearningScope::Repo,
                scope_ref: None,
                tags: vec![],
                source_task_id: None,
            })
            .await
            .unwrap_err();
        assert!(matches!(err, ServiceError::Validation(_)));
    }

    #[tokio::test]
    async fn create_learning_succeeds_with_valid_params() {
        let svc = service().await;
        let id = svc
            .create_learning(CreateLearningParams {
                kind: LearningKind::Convention,
                summary: "Use Arc for shared state".to_string(),
                detail: None,
                scope: LearningScope::User,
                scope_ref: None,
                tags: vec![],
                source_task_id: None,
            })
            .await
            .unwrap();
        let learning = svc.get_learning(id).await.unwrap();
        assert_eq!(learning.status, LearningStatus::Approved);
    }

    #[tokio::test]
    async fn get_learning_not_found_returns_error() {
        let svc = service().await;
        let err = svc.get_learning(LearningId(99999)).await.unwrap_err();
        assert!(matches!(err, ServiceError::NotFound(_)));
    }

    #[tokio::test]
    async fn apply_verdicts_validation_rejects_unknown_retrieval() {
        let (svc, db) = service_with_db().await;
        let task_id = seed_task(&db).await;
        let learning_id = seed_approved_learning(&svc).await;
        let err = svc
            .apply_verdicts(task_id, vec![(learning_id, LearningVerdict::Helped)])
            .await
            .unwrap_err();
        assert!(matches!(err, ServiceError::Validation(_)));
    }

    #[tokio::test]
    async fn apply_verdicts_succeeds_when_retrieval_exists() {
        let (svc, db) = service_with_db().await;
        let task_id = seed_task(&db).await;
        let learning_id = seed_approved_learning(&svc).await;
        svc.record_retrieval(task_id, learning_id, RetrievalSource::PromptInjection)
            .await
            .unwrap();
        svc.apply_verdicts(task_id, vec![(learning_id, LearningVerdict::Helped)])
            .await
            .unwrap();
        let l = svc.get_learning(learning_id).await.unwrap();
        assert_eq!(l.upvote_count, 1);
    }

    #[tokio::test]
    async fn apply_verdicts_empty_is_ok() {
        let (svc, db) = service_with_db().await;
        let task_id = seed_task(&db).await;
        svc.apply_verdicts(task_id, vec![]).await.unwrap();
    }

    #[tokio::test]
    async fn delete_learning_removes_entry() {
        let svc = service().await;
        let id = seed_approved_learning(&svc).await;
        svc.delete_learning(id).await.unwrap();
        let err = svc.get_learning(id).await.unwrap_err();
        assert!(matches!(err, ServiceError::NotFound(_)));
    }

    #[tokio::test]
    async fn delete_learning_not_found_returns_error() {
        let svc = service().await;
        let err = svc.delete_learning(LearningId(99999)).await.unwrap_err();
        assert!(matches!(err, ServiceError::NotFound(_)));
    }

    #[tokio::test]
    async fn query_learnings_ranks_and_records_retrieval() {
        let (svc, db) = service_with_db().await;
        let task_id = seed_task(&db).await;
        let learning_id = seed_approved_learning(&svc).await;

        let ranked = svc
            .query_learnings(QueryLearningsParams {
                task_id,
                query: Some("A convention".to_string()),
                tag_filter: vec![],
                limit: 10,
            })
            .await
            .unwrap();

        assert!(
            ranked.iter().any(|l| l.id == learning_id),
            "expected the seeded learning among ranked results"
        );

        let retrievals = db.list_retrievals_for_task(task_id).await.unwrap();
        assert!(
            retrievals.iter().any(|r| r.learning_id == learning_id),
            "query_learnings must record a retrieval for each ranked learning"
        );
    }

    #[tokio::test]
    async fn query_learnings_unknown_task_returns_error() {
        let svc = service().await;
        let err = svc
            .query_learnings(QueryLearningsParams {
                task_id: TaskId(99999),
                query: Some("q".to_string()),
                tag_filter: vec![],
                limit: 10,
            })
            .await
            .unwrap_err();
        assert!(matches!(err, ServiceError::NotFound(_)));
    }

    #[tokio::test]
    async fn create_learning_embeds_on_write() {
        let db: Arc<dyn TaskStore> = Arc::new(Database::open_in_memory().await.unwrap());
        let emb_svc = EmbeddingService::new_test();
        let svc = LearningService::new(db.clone(), emb_svc);
        let id = svc
            .create_learning(CreateLearningParams {
                kind: LearningKind::Convention,
                summary: "test summary".to_string(),
                detail: None,
                scope: LearningScope::User,
                scope_ref: None,
                tags: vec![],
                source_task_id: None,
            })
            .await
            .unwrap();
        // Retrieve the raw row and verify embedding bytes are stored.
        let learnings_with_emb = db.list_all_approved_non_task_learnings().await.unwrap();
        let emb_entry = learnings_with_emb
            .iter()
            .find(|(l, _)| l.id == id)
            .expect("newly created learning should appear in approved non-task learnings");
        // EmbeddingService::new_test() returns vec![0.1f32; 384], which is 384 * 4 = 1536 bytes.
        assert_eq!(
            emb_entry.1.len(),
            384 * 4,
            "embedding should be 1536 bytes for 384 f32 values"
        );
    }

    #[test]
    fn find_code_citation_rejects_path_rs_symbol() {
        let hit = super::find_code_citation(
            "A step that must behave identically on both feed paths goes in \
             src/feed/cycle.rs::run_feed_cycle.",
        );
        assert_eq!(hit, Some("src/feed/cycle.rs::run_feed_cycle"));
    }

    #[test]
    fn find_code_citation_rejects_type_method() {
        let hit = super::find_code_citation("The FeedCycle::run entry point drives both paths.");
        assert_eq!(hit, Some("FeedCycle::run"));
    }

    #[test]
    fn find_code_citation_rejects_long_bare_snake_case() {
        let hit = super::find_code_citation(
            "Pinned by exec_trigger_epic_feed_quiet_command_reports_no_stderr today.",
        );
        assert_eq!(
            hit,
            Some("exec_trigger_epic_feed_quiet_command_reports_no_stderr")
        );
    }

    #[test]
    fn find_code_citation_rejects_long_bare_snake_case_even_backticked() {
        let hit = super::find_code_citation(
            "See `exec_trigger_epic_feed_quiet_command_reports_no_stderr` for the case.",
        );
        assert!(hit.is_some());
    }

    #[test]
    fn find_code_citation_allows_short_backticked_tool_names() {
        assert_eq!(
            super::find_code_citation("Call `query_learnings` before guessing."),
            None
        );
        assert_eq!(
            super::find_code_citation("Rate it with `rate_learning`, then `wrap_up`."),
            None
        );
    }

    #[test]
    fn find_code_citation_allows_plain_prose() {
        assert_eq!(
            super::find_code_citation(
                "TaskPatch double-Option means Some(None) clears a field, None leaves it unchanged."
            ),
            None
        );
        assert_eq!(
            super::find_code_citation("Feed-cycle logic must live in one shared place."),
            None
        );
    }

    #[tokio::test]
    async fn create_learning_rejects_summary_with_code_citation() {
        let svc = service().await;
        let err = svc
            .create_learning(CreateLearningParams {
                kind: LearningKind::Convention,
                summary: "A step goes in src/feed/cycle.rs::run_feed_cycle.".to_string(),
                detail: None,
                scope: LearningScope::User,
                scope_ref: None,
                tags: vec![],
                source_task_id: None,
            })
            .await
            .unwrap_err();
        assert!(matches!(err, ServiceError::Validation(_)));
    }

    #[tokio::test]
    async fn create_learning_rejects_detail_with_code_citation() {
        let svc = service().await;
        let err = svc
            .create_learning(CreateLearningParams {
                kind: LearningKind::Convention,
                summary: "Feed-cycle logic must live in one shared place.".to_string(),
                detail: Some("See FeedCycle::run for the exact entry point.".to_string()),
                scope: LearningScope::User,
                scope_ref: None,
                tags: vec![],
                source_task_id: None,
            })
            .await
            .unwrap_err();
        assert!(matches!(err, ServiceError::Validation(_)));
    }

    #[tokio::test]
    async fn create_learning_allows_tool_name_reference() {
        let svc = service().await;
        svc.create_learning(CreateLearningParams {
            kind: LearningKind::Procedural,
            summary: "Call `query_learnings` before guessing, not after.".to_string(),
            detail: None,
            scope: LearningScope::User,
            scope_ref: None,
            tags: vec![],
            source_task_id: None,
        })
        .await
        .unwrap();
    }
}
