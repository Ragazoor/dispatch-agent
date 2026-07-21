use std::sync::Arc;

use crate::db::{self, CreateLearningRow, LearningFilter, LearningPatch};
use crate::models::{
    Learning, LearningId, LearningKind, LearningScope, LearningStatus, LearningVerdict,
    RetrievalSource, TaskId,
};
use crate::service::embeddings::{embed_text_for_learning, serialize_embedding, EmbeddingService};

use super::{FieldUpdate, ServiceError};

// ---------------------------------------------------------------------------
// CreateLearningParams / UpdateLearningParams
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

pub struct UpdateLearningParams {
    pub id: LearningId,
    pub summary: Option<String>,
    /// `None` = don't change; `Some(FieldUpdate::Clear)` = clear; `Some(FieldUpdate::Set(v))` = set.
    pub detail: Option<FieldUpdate>,
    pub kind: Option<LearningKind>,
    pub tags: Option<Vec<String>>,
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

    pub async fn reject_learning(&self, id: LearningId) -> Result<(), ServiceError> {
        let learning = self.get_learning(id).await?;
        if learning.status.is_terminal() {
            return Err(ServiceError::Validation(format!(
                "cannot reject a {} learning",
                learning.status
            )));
        }
        self.db
            .patch_learning(id, &LearningPatch::new().status(LearningStatus::Rejected))
            .await
            .map_err(ServiceError::from)
    }

    pub async fn archive_learning(&self, id: LearningId) -> Result<(), ServiceError> {
        let learning = self.get_learning(id).await?;
        if learning.status != LearningStatus::Approved {
            return Err(ServiceError::Validation(format!(
                "can only archive an approved learning (current status: {})",
                learning.status
            )));
        }
        self.db
            .patch_learning(id, &LearningPatch::new().status(LearningStatus::Archived))
            .await
            .map_err(ServiceError::from)
    }

    pub async fn update_learning(&self, params: UpdateLearningParams) -> Result<(), ServiceError> {
        let learning = self.get_learning(params.id).await?;
        if learning.status.is_terminal() {
            return Err(ServiceError::Validation(
                "cannot edit a rejected or archived learning".to_string(),
            ));
        }
        if let Some(ref s) = params.summary {
            if s.trim().is_empty() {
                return Err(ServiceError::Validation("summary must not be empty".into()));
            }
        }
        let needs_reembed = params.summary.is_some()
            || params.detail.is_some()
            || params.kind.is_some()
            || params.tags.is_some();

        let new_emb_bytes: Option<Vec<u8>> = if needs_reembed {
            let summary = params
                .summary
                .as_deref()
                .unwrap_or(learning.summary.as_str());
            let kind = params.kind.unwrap_or(learning.kind);
            let tags = params.tags.as_deref().unwrap_or(learning.tags.as_slice());
            let detail = match &params.detail {
                Some(FieldUpdate::Set(v)) => Some(v.as_str()),
                Some(FieldUpdate::Clear) => None,
                None => learning.detail.as_deref(),
            };
            let text = embed_text_for_learning(kind, summary, tags, detail);
            let emb_vec = self
                .embedding_service
                .embed(text)
                .await
                .map_err(ServiceError::from)?;
            Some(serialize_embedding(&emb_vec))
        } else {
            None
        };

        let mut patch = LearningPatch::new();
        if let Some(ref s) = params.summary {
            patch = patch.summary(s.as_str());
        }
        if let Some(ref d) = params.detail {
            patch = patch.detail(d.as_option());
        }
        if let Some(k) = params.kind {
            patch = patch.kind(k);
        }
        if let Some(ref t) = params.tags {
            patch = patch.tags(t.as_slice());
        }
        if let Some(ref bytes) = new_emb_bytes {
            patch = patch.embedding(bytes);
        }

        self.db
            .patch_learning(params.id, &patch)
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
}

// ---------------------------------------------------------------------------
// LearningService tests
// ---------------------------------------------------------------------------

#[cfg(test)]
mod learning_tests {
    #![allow(clippy::unwrap_used, clippy::expect_used)]
    use std::sync::Arc;

    use super::{CreateLearningParams, LearningService, UpdateLearningParams};
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
    async fn reject_learning_from_proposed_succeeds() {
        let svc = service().await;
        let id = svc
            .create_learning(CreateLearningParams {
                kind: LearningKind::Pitfall,
                summary: "A pitfall".to_string(),
                detail: None,
                scope: LearningScope::User,
                scope_ref: None,
                tags: vec![],
                source_task_id: None,
            })
            .await
            .unwrap();
        svc.reject_learning(id).await.unwrap();
        let learning = svc.get_learning(id).await.unwrap();
        assert_eq!(learning.status, LearningStatus::Rejected);
    }

    #[tokio::test]
    async fn reject_learning_from_archived_fails() {
        let svc = service().await;
        let id = svc
            .create_learning(CreateLearningParams {
                kind: LearningKind::Convention,
                summary: "A convention".to_string(),
                detail: None,
                scope: LearningScope::User,
                scope_ref: None,
                tags: vec![],
                source_task_id: None,
            })
            .await
            .unwrap();
        svc.archive_learning(id).await.unwrap();
        let err = svc.reject_learning(id).await.unwrap_err();
        assert!(matches!(err, ServiceError::Validation(_)));
    }

    #[tokio::test]
    async fn archive_learning_from_approved_succeeds() {
        let svc = service().await;
        let id = svc
            .create_learning(CreateLearningParams {
                kind: LearningKind::Convention,
                summary: "A convention".to_string(),
                detail: None,
                scope: LearningScope::User,
                scope_ref: None,
                tags: vec![],
                source_task_id: None,
            })
            .await
            .unwrap();
        svc.archive_learning(id).await.unwrap();
        let learning = svc.get_learning(id).await.unwrap();
        assert_eq!(learning.status, LearningStatus::Archived);
    }

    #[tokio::test]
    async fn archive_learning_from_rejected_fails() {
        let svc = service().await;
        let id = seed_approved_learning(&svc).await;
        svc.reject_learning(id).await.unwrap();
        let err = svc.archive_learning(id).await.unwrap_err();
        assert!(matches!(err, ServiceError::Validation(_)));
    }

    #[tokio::test]
    async fn update_learning_on_rejected_fails() {
        let svc = service().await;
        let id = svc
            .create_learning(CreateLearningParams {
                kind: LearningKind::Convention,
                summary: "A convention".to_string(),
                detail: None,
                scope: LearningScope::User,
                scope_ref: None,
                tags: vec![],
                source_task_id: None,
            })
            .await
            .unwrap();
        svc.reject_learning(id).await.unwrap();
        let err = svc
            .update_learning(UpdateLearningParams {
                id,
                summary: Some("Updated".to_string()),
                detail: None,
                kind: None,
                tags: None,
            })
            .await
            .unwrap_err();
        assert!(matches!(err, ServiceError::Validation(_)));
    }

    #[tokio::test]
    async fn update_learning_rejects_empty_summary() {
        let svc = service().await;
        let id = svc
            .create_learning(CreateLearningParams {
                kind: LearningKind::Convention,
                summary: "A convention".to_string(),
                detail: None,
                scope: LearningScope::User,
                scope_ref: None,
                tags: vec![],
                source_task_id: None,
            })
            .await
            .unwrap();
        let err = svc
            .update_learning(UpdateLearningParams {
                id,
                summary: Some("".to_string()),
                detail: None,
                kind: None,
                tags: None,
            })
            .await
            .unwrap_err();
        assert!(matches!(err, ServiceError::Validation(_)));
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

    #[tokio::test]
    async fn update_learning_reembeds_when_summary_changes() {
        use crate::db::LearningPatch as DbLearningPatch;
        use crate::service::embeddings::serialize_embedding;

        let db: Arc<dyn TaskStore> = Arc::new(Database::open_in_memory().await.unwrap());
        let emb_svc = EmbeddingService::new_test();
        let svc = LearningService::new(db.clone(), emb_svc);
        let id = svc
            .create_learning(CreateLearningParams {
                kind: LearningKind::Convention,
                summary: "original summary".to_string(),
                detail: None,
                scope: LearningScope::User,
                scope_ref: None,
                tags: vec![],
                source_task_id: None,
            })
            .await
            .unwrap();

        // Replace the embedding with a sentinel value so we can detect if re-embedding
        // was actually triggered (the stub always returns vec![0.1f32; 384]).
        let sentinel: Vec<u8> = vec![0xFFu8; 1536];
        db.patch_learning(id, &DbLearningPatch::new().embedding(&sentinel))
            .await
            .unwrap();

        // Confirm sentinel is stored.
        let before = db.list_all_approved_non_task_learnings().await.unwrap();
        let emb_before = before
            .iter()
            .find(|(l, _)| l.id == id)
            .map(|(_, e)| e)
            .expect("learning must have embedding after sentinel write");
        assert_eq!(emb_before.as_slice(), sentinel.as_slice());

        // Update the summary — should trigger re-embedding, replacing the sentinel.
        svc.update_learning(UpdateLearningParams {
            id,
            summary: Some("updated summary".to_string()),
            detail: None,
            kind: None,
            tags: None,
        })
        .await
        .unwrap();

        let after = db.list_all_approved_non_task_learnings().await.unwrap();
        let emb_after = after
            .iter()
            .find(|(l, _)| l.id == id)
            .map(|(_, e)| e)
            .expect("learning must still have embedding after update");

        // The sentinel must be gone — re-embedding was called and returned the stub bytes.
        assert_ne!(
            emb_after.as_slice(),
            sentinel.as_slice(),
            "embedding must be updated from sentinel after summary change"
        );
        // The stub returns vec![0.1f32; 384]; verify the result matches.
        let expected = serialize_embedding(&vec![0.1f32; 384]);
        assert_eq!(
            emb_after.as_slice(),
            expected.as_slice(),
            "embedding must equal stub output after re-embed"
        );
    }

    #[tokio::test]
    async fn update_learning_skips_reembed_when_no_content_fields_change() {
        use crate::db::LearningPatch as DbLearningPatch;

        let db: Arc<dyn TaskStore> = Arc::new(Database::open_in_memory().await.unwrap());
        let emb_svc = EmbeddingService::new_test();
        let svc = LearningService::new(db.clone(), emb_svc);
        let id = svc
            .create_learning(CreateLearningParams {
                kind: LearningKind::Convention,
                summary: "stable summary".to_string(),
                detail: None,
                scope: LearningScope::User,
                scope_ref: None,
                tags: vec![],
                source_task_id: None,
            })
            .await
            .unwrap();

        // Replace the embedding with a sentinel value so we can detect if it gets overwritten.
        let sentinel: Vec<u8> = vec![0xAB; 1536];
        db.patch_learning(id, &DbLearningPatch::new().embedding(&sentinel))
            .await
            .unwrap();

        // Call update_learning with no content fields changed — should NOT trigger re-embed.
        svc.update_learning(UpdateLearningParams {
            id,
            summary: None,
            detail: None,
            kind: None,
            tags: None,
        })
        .await
        .unwrap();

        // Verify the sentinel embedding was NOT overwritten.
        let entries = db.list_all_approved_non_task_learnings().await.unwrap();
        let emb = entries
            .iter()
            .find(|(l, _)| l.id == id)
            .map(|(_, e)| e)
            .expect("learning must still have embedding");
        assert_eq!(
            emb.as_slice(),
            sentinel.as_slice(),
            "embedding must not be overwritten when no content fields change"
        );
    }
}
