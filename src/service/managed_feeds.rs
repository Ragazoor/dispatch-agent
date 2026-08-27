//! Idempotent provisioning of the managed feed-epic tree (WP5).
//!
//! Materialises the epics that the PR-review feed routing depends on, matched
//! by [`FeedRole`] (never by title, so user renames survive):
//!
//! - a `reviews_parent` root epic carrying the reviews `feed_command`, with
//!   `my_reviews` / `team_reviews` / `bots` sub-epics that carry **no**
//!   `feed_command` (so the `FeedRunner` never polls them independently — the
//!   B3 concurrency guard; they are reconciled only via the parent's role
//!   router in `run_role_routed_feed_sync`);
//! - a `cve` root epic carrying the CVE `feed_command`.
//!
//! Each subtree is provisioned only when its command is configured. Running
//! this repeatedly converges on the same tree without duplicates. An archived
//! managed epic is left archived (logged, not resurrected). See the
//! `ProvisionManagedEpics` rule and the `config` block in
//! `docs/specs/epics.allium` for the authoritative semantics.
//!
//! This lives in the service layer and provisions brand-new, childless epics,
//! so it goes through [`EpicCrud`] directly rather than
//! [`crate::service::EpicService`] (which does not expose `feed_role`); no
//! epic-status recalculation is needed because freshly-created epics have no
//! children.

use anyhow::Result;

use crate::db::{EpicCrud, EpicPatch};
use crate::models::{Epic, EpicId, FeedRole, TaskStatus};
use crate::service::ServiceError;

/// Default display title for a freshly-created managed epic. Consulted only on
/// creation — once an epic exists its title is owned by the user. The sub-epic
/// titles mirror `feed::ingest::role_sub_epic_title` so lazy (reconcile-time)
/// and eager (provisioning-time) creation agree.
fn managed_role_title(role: FeedRole) -> &'static str {
    match role {
        FeedRole::ReviewsParent => "PR Reviews",
        FeedRole::MyReviews => "My Reviews",
        FeedRole::TeamReviews => "Team Reviews",
        FeedRole::Bots => "Bots",
        FeedRole::Cve => "CVE",
        // Not a managed role; never passed here.
        FeedRole::None => "Reviews",
    }
}

/// Ensure the managed feed-epic tree exists, idempotently.
///
/// `*_command` is the configured feed script for each subtree; `None` skips
/// that subtree entirely (provisioning is opt-in). `*_interval_secs` is the
/// poll cadence for the command-carrying root; `None` lets the runtime fall
/// back to the default feed interval.
pub async fn ensure_managed_epics(
    db: &dyn EpicCrud,
    reviews_command: Option<&str>,
    reviews_interval_secs: Option<i64>,
    cve_command: Option<&str>,
    cve_interval_secs: Option<i64>,
) -> Result<()> {
    // Snapshot once: role lookups read from this list. Creating one role never
    // affects the lookup of another (roles are distinct), and within a single
    // call we never create the same role twice.
    let epics = db.list_epics().await?;

    if let Some(cmd) = reviews_command {
        let parent = ensure_role_epic(
            db,
            &epics,
            FeedRole::ReviewsParent,
            None,
            Some(cmd),
            reviews_interval_secs,
        )
        .await?;
        // Sub-epics only when the parent is present (an archived parent yields
        // None — we don't reparent role sub-epics under an archived root).
        if let Some(parent_id) = parent {
            for role in [FeedRole::MyReviews, FeedRole::TeamReviews, FeedRole::Bots] {
                ensure_role_epic(db, &epics, role, Some(parent_id), None, None).await?;
            }
        }
    }

    if let Some(cmd) = cve_command {
        ensure_role_epic(
            db,
            &epics,
            FeedRole::Cve,
            None,
            Some(cmd),
            cve_interval_secs,
        )
        .await?;
    }

    Ok(())
}

/// Ensure a single managed epic with `role` (under `parent`) exists.
///
/// - Active epic present: keep it (title untouched, preserving any user
///   rename); for command-carrying roles, reconcile `feed_command`/interval if
///   the configured value differs. Returns its id.
/// - Archived epic present: leave it archived, log a warning, create nothing.
///   Returns `None`.
/// - Absent: create it and stamp `feed_role` (+ command/interval for
///   command-carrying roles). Returns the new id.
async fn ensure_role_epic(
    db: &dyn EpicCrud,
    existing: &[Epic],
    role: FeedRole,
    parent: Option<EpicId>,
    command: Option<&str>,
    interval_secs: Option<i64>,
) -> Result<Option<EpicId>> {
    if let Some(epic) = existing
        .iter()
        .find(|e| e.feed_role == role && e.parent_epic_id == parent)
    {
        if epic.status == TaskStatus::Archived {
            tracing::warn!(
                epic_id = epic.id.0,
                role = %role,
                "ensure_managed_epics: managed epic is archived; leaving it archived (not resurrecting)"
            );
            return Ok(None);
        }
        // Active: never touch the title. Reconcile only the feed command /
        // interval, and only for the command-carrying roles.
        if let Some(cmd) = command {
            let mut patch = EpicPatch::new();
            if epic.feed_command.as_deref() != Some(cmd) {
                patch = patch.feed_command(Some(cmd));
            }
            if epic.feed_interval_secs != interval_secs {
                patch = patch.feed_interval_secs(interval_secs);
            }
            // patch_epic short-circuits an empty patch, so calling it
            // unconditionally is a no-op when nothing differs.
            db.patch_epic(epic.id, &patch).await?;
        }
        return Ok(Some(epic.id));
    }

    let created = db.create_epic(managed_role_title(role), "", parent).await?;
    let mut patch = EpicPatch::new().feed_role(role);
    if let Some(cmd) = command {
        patch = patch
            .feed_command(Some(cmd))
            .feed_interval_secs(interval_secs);
    }
    db.patch_epic(created.id, &patch).await?;
    Ok(Some(created.id))
}

/// The four managed-feed settings, read from the settings table and fed to
/// [`ensure_managed_epics`]. A named struct (rather than a 4-tuple) so the two
/// `(command, interval)` pairs can't be transposed at a call site.
#[derive(Debug, Clone, Default)]
pub struct ManagedFeedSettings {
    pub reviews_command: Option<String>,
    pub reviews_interval_secs: Option<i64>,
    pub cve_command: Option<String>,
    pub cve_interval_secs: Option<i64>,
}

/// Read the four managed-feed settings from the settings table. Pure reads —
/// callable through a read-only `&dyn SettingsStore` handle, so a non-service
/// consumer can fetch them and hand them to `EpicServiceApi::provision_managed_feeds`.
pub async fn read_managed_feed_settings(
    db: &dyn crate::db::SettingsStore,
) -> Result<ManagedFeedSettings> {
    Ok(ManagedFeedSettings {
        reviews_command: db.get_reviews_feed_command().await?,
        reviews_interval_secs: db.get_reviews_feed_interval_secs().await?,
        cve_command: db.get_cve_feed_command().await?,
        cve_interval_secs: db.get_cve_feed_interval_secs().await?,
    })
}

/// Partial update for the four managed-feed settings: absent (`None`) leaves
/// a field unchanged, `Some(None)` clears it, `Some(Some(v))` sets it. A named
/// struct (rather than four positional double-Option params) so the two
/// `(command, interval)` pairs can't be transposed at a call site — mirrors
/// the read-side [`ManagedFeedSettings`].
#[derive(Debug, Clone, Default)]
pub struct ManagedFeedSettingsPatch {
    pub reviews_command: Option<Option<String>>,
    pub reviews_interval_secs: Option<Option<i64>>,
    pub cve_command: Option<Option<String>>,
    pub cve_interval_secs: Option<Option<i64>>,
}

/// Persist only the provided managed-feed settings fields, leaving any absent
/// field unchanged. Mirrors [`read_managed_feed_settings`] for the write side.
pub async fn write_managed_feed_settings(
    db: &dyn crate::db::SettingsStore,
    patch: ManagedFeedSettingsPatch,
) -> std::result::Result<(), ServiceError> {
    // Both intervals are checked before ANY of the four settings is written.
    // These are separate rows, so a mid-way rejection would leave a command
    // persisted against an interval that was refused. They are also not a
    // separate kind of cadence: provisioning copies them onto the managed
    // epics' `feed_interval_secs`, so the floor that binds an epic must bind
    // them (epics.allium: SetManagedFeedConfig). `0` was blessed here as "poll
    // every tick" before the floor existed.
    crate::service::validate_feed_interval(
        "reviews_interval_secs",
        patch.reviews_interval_secs.flatten(),
    )?;
    crate::service::validate_feed_interval("cve_interval_secs", patch.cve_interval_secs.flatten())?;

    if let Some(v) = patch.reviews_command {
        db.set_reviews_feed_command(v.as_deref()).await?;
    }
    if let Some(v) = patch.reviews_interval_secs {
        db.set_reviews_feed_interval_secs(v).await?;
    }
    if let Some(v) = patch.cve_command {
        db.set_cve_feed_command(v.as_deref()).await?;
    }
    if let Some(v) = patch.cve_interval_secs {
        db.set_cve_feed_interval_secs(v).await?;
    }
    Ok(())
}

/// Read the managed-feed settings and provision accordingly. This is the
/// startup entry point (called from `run_tui`), also exercised directly in
/// tests. A no-op when neither command is configured.
// Takes the umbrella `&dyn TaskStore` so a concrete `&Database` (startup, tests)
// coerces in. Non-service consumers go through
// `EpicServiceApi::provision_managed_feeds` instead. The inner
// `ensure_managed_epics` call upcasts the trait object to `&dyn EpicCrud`.
pub async fn provision_managed_feeds_from_settings(db: &dyn crate::db::TaskStore) -> Result<()> {
    let s = read_managed_feed_settings(db).await?;
    ensure_managed_epics(
        db,
        s.reviews_command.as_deref(),
        s.reviews_interval_secs,
        s.cve_command.as_deref(),
        s.cve_interval_secs,
    )
    .await
}

#[cfg(test)]
#[allow(clippy::unwrap_used, clippy::expect_used)]
mod tests {
    use super::*;
    use crate::db::{Database, EpicRead, SettingsStore};
    use crate::models::MIN_FEED_INTERVAL_SECS;

    const REVIEWS: &str = "/scripts/fetch-reviews.sh";
    const CVE: &str = "/scripts/fetch-cve.sh";

    async fn ensure(db: &Database) {
        ensure_managed_epics(db, Some(REVIEWS), Some(300), Some(CVE), Some(900))
            .await
            .unwrap();
    }

    fn by_role(epics: &[Epic], role: FeedRole) -> Vec<&Epic> {
        epics.iter().filter(|e| e.feed_role == role).collect()
    }

    #[tokio::test]
    async fn ensure_managed_epics_creates_tree() {
        let db = Database::open_in_memory().await.unwrap();
        ensure(&db).await;
        let epics = db.list_epics().await.unwrap();

        for role in [
            FeedRole::ReviewsParent,
            FeedRole::MyReviews,
            FeedRole::TeamReviews,
            FeedRole::Bots,
            FeedRole::Cve,
        ] {
            assert_eq!(
                by_role(&epics, role).len(),
                1,
                "exactly one epic for role {role}"
            );
        }

        let parent = by_role(&epics, FeedRole::ReviewsParent)[0];
        assert_eq!(parent.parent_epic_id, None, "reviews_parent is a root epic");
        assert_eq!(parent.feed_command.as_deref(), Some(REVIEWS));
        assert_eq!(parent.feed_interval_secs, Some(300));

        for role in [FeedRole::MyReviews, FeedRole::TeamReviews, FeedRole::Bots] {
            let sub = by_role(&epics, role)[0];
            assert_eq!(
                sub.parent_epic_id,
                Some(parent.id),
                "{role} parented to reviews_parent"
            );
            assert_eq!(
                sub.feed_command, None,
                "{role} sub-epic carries no feed_command"
            );
        }

        let cve = by_role(&epics, FeedRole::Cve)[0];
        assert_eq!(cve.parent_epic_id, None, "cve is a root epic");
        assert_eq!(cve.feed_command.as_deref(), Some(CVE));
        assert_eq!(cve.feed_interval_secs, Some(900));
    }

    #[tokio::test]
    async fn ensure_is_idempotent() {
        let db = Database::open_in_memory().await.unwrap();
        ensure(&db).await;
        ensure(&db).await;
        let epics = db.list_epics().await.unwrap();
        assert_eq!(
            epics.len(),
            5,
            "two ensures must create no duplicate managed epics"
        );
    }

    #[tokio::test]
    async fn ensure_preserves_user_rename() {
        let db = Database::open_in_memory().await.unwrap();
        ensure(&db).await;
        let my_id = by_role(&db.list_epics().await.unwrap(), FeedRole::MyReviews)[0].id;
        db.patch_epic(my_id, &EpicPatch::new().title("My PRs"))
            .await
            .unwrap();

        ensure(&db).await;

        let epics = db.list_epics().await.unwrap();
        let my = by_role(&epics, FeedRole::MyReviews);
        assert_eq!(my.len(), 1, "rename must not spawn a duplicate");
        assert_eq!(my[0].id, my_id);
        assert_eq!(my[0].title, "My PRs", "user rename is preserved");
    }

    #[tokio::test]
    async fn ensure_does_not_resurrect_archived() {
        let db = Database::open_in_memory().await.unwrap();
        ensure(&db).await;
        let bots_id = by_role(&db.list_epics().await.unwrap(), FeedRole::Bots)[0].id;
        db.patch_epic(bots_id, &EpicPatch::new().status(TaskStatus::Archived))
            .await
            .unwrap();

        ensure(&db).await;

        let epics = db.list_epics().await.unwrap();
        let bots = by_role(&epics, FeedRole::Bots);
        assert_eq!(
            bots.len(),
            1,
            "archived epic must not get an empty duplicate"
        );
        assert_eq!(bots[0].id, bots_id);
        assert_eq!(
            bots[0].status,
            TaskStatus::Archived,
            "archived managed epic stays archived"
        );
    }

    #[tokio::test]
    async fn provision_from_settings_is_noop_without_config() {
        let db = Database::open_in_memory().await.unwrap();
        provision_managed_feeds_from_settings(&db).await.unwrap();
        assert!(
            db.list_epics().await.unwrap().is_empty(),
            "no config -> no managed epics"
        );
    }

    #[tokio::test]
    async fn provision_from_settings_creates_tree() {
        let db = Database::open_in_memory().await.unwrap();
        db.set_reviews_feed_command(Some(REVIEWS)).await.unwrap();
        db.set_reviews_feed_interval_secs(Some(300)).await.unwrap();
        db.set_cve_feed_command(Some(CVE)).await.unwrap();
        provision_managed_feeds_from_settings(&db).await.unwrap();

        let epics = db.list_epics().await.unwrap();
        assert_eq!(
            epics.len(),
            5,
            "settings-driven provisioning builds the tree"
        );
        let parent = by_role(&epics, FeedRole::ReviewsParent)[0];
        assert_eq!(parent.feed_command.as_deref(), Some(REVIEWS));
    }

    #[tokio::test]
    async fn write_managed_feed_settings_updates_only_provided_fields() {
        let db = Database::open_in_memory().await.unwrap();
        db.set_cve_feed_command(Some("/existing.sh")).await.unwrap();

        write_managed_feed_settings(
            &db,
            ManagedFeedSettingsPatch {
                reviews_command: Some(Some(REVIEWS.to_string())),
                reviews_interval_secs: Some(Some(300)),
                cve_command: None,
                cve_interval_secs: None,
            },
        )
        .await
        .unwrap();

        assert_eq!(
            db.get_reviews_feed_command().await.unwrap().as_deref(),
            Some(REVIEWS)
        );
        assert_eq!(
            db.get_reviews_feed_interval_secs().await.unwrap(),
            Some(300)
        );
        assert_eq!(
            db.get_cve_feed_command().await.unwrap().as_deref(),
            Some("/existing.sh"),
            "absent field must be left unchanged"
        );
    }

    #[tokio::test]
    async fn write_managed_feed_settings_clears_on_explicit_none() {
        let db = Database::open_in_memory().await.unwrap();
        db.set_reviews_feed_command(Some(REVIEWS)).await.unwrap();

        write_managed_feed_settings(
            &db,
            ManagedFeedSettingsPatch {
                reviews_command: Some(None),
                ..Default::default()
            },
        )
        .await
        .unwrap();

        assert!(
            db.get_reviews_feed_command().await.unwrap().is_none(),
            "Some(None) must clear the setting"
        );
    }

    // --- the feed-cadence floor (core.allium: "Interval literals", CLAIM 2) ---

    /// These settings are not a separate kind of cadence: they are copied onto
    /// the managed epics' `feed_interval_secs`, so a value the floor refuses on
    /// an epic must not be reachable by writing it here instead. `0` used to be
    /// blessed here as "poll every tick".
    #[tokio::test]
    async fn write_managed_feed_settings_rejects_a_sub_floor_interval() {
        for bad in [0, -5, MIN_FEED_INTERVAL_SECS - 1] {
            let db = Database::open_in_memory().await.unwrap();

            let reviews = write_managed_feed_settings(
                &db,
                ManagedFeedSettingsPatch {
                    reviews_interval_secs: Some(Some(bad)),
                    ..Default::default()
                },
            )
            .await;
            assert!(
                matches!(reviews, Err(ServiceError::Validation(_))),
                "reviews_interval_secs = {bad} should be rejected, got {reviews:?}"
            );

            let cve = write_managed_feed_settings(
                &db,
                ManagedFeedSettingsPatch {
                    cve_interval_secs: Some(Some(bad)),
                    ..Default::default()
                },
            )
            .await;
            assert!(
                matches!(cve, Err(ServiceError::Validation(_))),
                "cve_interval_secs = {bad} should be rejected, got {cve:?}"
            );
        }
    }

    /// A rejected interval must write nothing at all — not the command it
    /// arrived alongside, and not the other feed's interval.
    #[tokio::test]
    async fn write_managed_feed_settings_rejecting_an_interval_writes_nothing() {
        let db = Database::open_in_memory().await.unwrap();

        let err = write_managed_feed_settings(
            &db,
            ManagedFeedSettingsPatch {
                reviews_command: Some(Some(REVIEWS.to_string())),
                reviews_interval_secs: Some(Some(10)),
                cve_command: None,
                cve_interval_secs: Some(Some(300)),
            },
        )
        .await;
        assert!(matches!(err, Err(ServiceError::Validation(_))), "{err:?}");

        assert!(
            db.get_reviews_feed_command().await.unwrap().is_none(),
            "the command must not survive a rejected interval in the same call"
        );
        assert!(
            db.get_cve_feed_interval_secs().await.unwrap().is_none(),
            "the other feed's valid interval must not be written either"
        );
    }

    #[tokio::test]
    async fn write_managed_feed_settings_accepts_the_floor_and_clearing() {
        let db = Database::open_in_memory().await.unwrap();

        write_managed_feed_settings(
            &db,
            ManagedFeedSettingsPatch {
                reviews_interval_secs: Some(Some(MIN_FEED_INTERVAL_SECS)),
                cve_interval_secs: Some(None),
                ..Default::default()
            },
        )
        .await
        .unwrap();

        assert_eq!(
            db.get_reviews_feed_interval_secs().await.unwrap(),
            Some(MIN_FEED_INTERVAL_SECS)
        );
        // Clearing means "inherit the default", which itself clears the floor.
        assert_eq!(db.get_cve_feed_interval_secs().await.unwrap(), None);
    }
}
