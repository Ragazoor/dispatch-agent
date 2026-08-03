#![allow(clippy::unwrap_used, clippy::expect_used)]
use super::*;

#[tokio::test]
async fn get_setting_bool_returns_none_when_absent() {
    let db = Database::open_in_memory().await.unwrap();
    assert_eq!(
        db.get_setting_bool("notifications_enabled").await.unwrap(),
        None
    );
}

#[tokio::test]
async fn set_and_get_setting_bool_roundtrips() {
    let db = Database::open_in_memory().await.unwrap();
    db.set_setting_bool("notifications_enabled", true)
        .await
        .unwrap();
    assert_eq!(
        db.get_setting_bool("notifications_enabled").await.unwrap(),
        Some(true)
    );

    db.set_setting_bool("notifications_enabled", false)
        .await
        .unwrap();
    assert_eq!(
        db.get_setting_bool("notifications_enabled").await.unwrap(),
        Some(false)
    );
}

#[tokio::test]
async fn get_setting_string_returns_none_when_absent() {
    let db = Database::open_in_memory().await.unwrap();
    assert_eq!(db.get_setting_string("repo_filter").await.unwrap(), None);
}

#[tokio::test]
async fn set_and_get_setting_string() {
    let db = Database::open_in_memory().await.unwrap();
    db.set_setting_string("repo_filter", "/repo1\n/repo2")
        .await
        .unwrap();
    assert_eq!(
        db.get_setting_string("repo_filter").await.unwrap(),
        Some("/repo1\n/repo2".to_string())
    );
}

#[tokio::test]
async fn set_setting_string_upserts() {
    let db = Database::open_in_memory().await.unwrap();
    db.set_setting_string("repo_filter", "old").await.unwrap();
    db.set_setting_string("repo_filter", "new").await.unwrap();
    assert_eq!(
        db.get_setting_string("repo_filter").await.unwrap(),
        Some("new".to_string())
    );
}

#[tokio::test]
async fn save_and_list_repo_paths() {
    let db = in_memory_db().await;
    assert!(db.list_repo_paths().await.unwrap().is_empty());
    db.save_repo_path("/home/user/project").await.unwrap();
    db.save_repo_path("/home/user/other").await.unwrap();
    let paths = db.list_repo_paths().await.unwrap();
    assert_eq!(paths.len(), 2);
    assert!(paths.contains(&"/home/user/project".to_string()));
    assert!(paths.contains(&"/home/user/other".to_string()));
}

#[tokio::test]
async fn save_repo_path_deduplicates() {
    let db = in_memory_db().await;
    db.save_repo_path("/home/user/project").await.unwrap();
    db.save_repo_path("/home/user/project").await.unwrap();
    assert_eq!(db.list_repo_paths().await.unwrap().len(), 1);
}

#[tokio::test]
async fn list_repo_paths_empty_by_default() {
    let db = in_memory_db().await;
    assert!(db.list_repo_paths().await.unwrap().is_empty());
}

#[tokio::test]
async fn list_repo_paths_returns_all_beyond_nine() {
    let db = in_memory_db().await;
    for i in 0..15 {
        db.save_repo_path(&format!("/home/user/project{i}"))
            .await
            .unwrap();
    }
    let paths = db.list_repo_paths().await.unwrap();
    assert_eq!(
        paths.len(),
        15,
        "all 15 paths should be returned, not just 9"
    );
}

#[tokio::test]
async fn filter_presets_save_and_list() {
    let db = Database::open_in_memory().await.unwrap();
    db.save_filter_preset(
        "frontend",
        &["/repo-a".to_string(), "/repo-b".to_string()],
        "include",
    )
    .await
    .unwrap();
    db.save_filter_preset("backend", &["/repo-c".to_string()], "exclude")
        .await
        .unwrap();

    let presets = db.list_filter_presets().await.unwrap();
    assert_eq!(presets.len(), 2);
    assert_eq!(presets[0].0, "backend"); // sorted by name
    assert_eq!(presets[0].2, "exclude");
    assert_eq!(presets[1].0, "frontend");
    assert_eq!(
        presets[1].1,
        vec!["/repo-a".to_string(), "/repo-b".to_string()]
    );
    assert_eq!(presets[1].2, "include");
}

#[tokio::test]
async fn filter_presets_overwrite_and_delete() {
    let db = Database::open_in_memory().await.unwrap();
    db.save_filter_preset("frontend", &["/repo-a".to_string()], "include")
        .await
        .unwrap();
    db.save_filter_preset(
        "frontend",
        &["/repo-x".to_string(), "/repo-y".to_string()],
        "exclude",
    )
    .await
    .unwrap();

    let presets = db.list_filter_presets().await.unwrap();
    assert_eq!(presets.len(), 1);
    assert_eq!(
        presets[0].1,
        vec!["/repo-x".to_string(), "/repo-y".to_string()]
    );
    assert_eq!(presets[0].2, "exclude");

    db.delete_filter_preset("frontend").await.unwrap();
    let presets = db.list_filter_presets().await.unwrap();
    assert!(presets.is_empty());
}

#[tokio::test]
async fn delete_repo_path_removes_entry() {
    let db = in_memory_db().await;
    db.save_repo_path("/home/user/project").await.unwrap();
    db.save_repo_path("/home/user/other").await.unwrap();
    assert_eq!(db.list_repo_paths().await.unwrap().len(), 2);
    db.delete_repo_path("/home/user/project").await.unwrap();
    let paths = db.list_repo_paths().await.unwrap();
    assert_eq!(paths.len(), 1);
    assert_eq!(paths[0], "/home/user/other");
}

#[tokio::test]
async fn delete_repo_path_nonexistent_is_ok() {
    let db = in_memory_db().await;
    db.delete_repo_path("/does/not/exist").await.unwrap();
}

#[tokio::test]
async fn delete_repo_path_cleans_presets() {
    let db = in_memory_db().await;
    db.save_repo_path("/home/user/a").await.unwrap();
    db.save_repo_path("/home/user/b").await.unwrap();
    db.save_filter_preset(
        "my_preset",
        &["/home/user/a".to_string(), "/home/user/b".to_string()],
        "include",
    )
    .await
    .unwrap();
    db.delete_repo_path("/home/user/a").await.unwrap();
    let presets = db.list_filter_presets().await.unwrap();
    assert_eq!(presets.len(), 1);
    assert_eq!(presets[0].0, "my_preset");
    assert_eq!(presets[0].1, vec!["/home/user/b".to_string()]);
}

#[tokio::test]
async fn delete_repo_path_removes_empty_preset() {
    let db = in_memory_db().await;
    db.save_repo_path("/home/user/solo").await.unwrap();
    db.save_filter_preset("solo_preset", &["/home/user/solo".to_string()], "include")
        .await
        .unwrap();
    db.delete_repo_path("/home/user/solo").await.unwrap();
    let presets = db.list_filter_presets().await.unwrap();
    assert!(presets.is_empty());
}

#[tokio::test]
async fn verify_command_default_is_none() {
    let db = in_memory_db().await;
    db.save_repo_path("/home/me/repo").await.unwrap();
    assert_eq!(db.get_verify_command("/home/me/repo").await.unwrap(), None);
}

#[tokio::test]
async fn verify_command_round_trip() {
    let db = in_memory_db().await;
    db.save_repo_path("/home/me/repo").await.unwrap();
    db.set_verify_command("/home/me/repo", Some("cargo test"))
        .await
        .unwrap();
    assert_eq!(
        db.get_verify_command("/home/me/repo").await.unwrap(),
        Some("cargo test".to_string())
    );
}

#[tokio::test]
async fn verify_command_empty_clears() {
    let db = in_memory_db().await;
    db.save_repo_path("/r").await.unwrap();
    db.set_verify_command("/r", Some("cargo test"))
        .await
        .unwrap();
    db.set_verify_command("/r", Some("")).await.unwrap();
    assert_eq!(db.get_verify_command("/r").await.unwrap(), None);
    db.set_verify_command("/r", Some("   ")).await.unwrap();
    assert_eq!(db.get_verify_command("/r").await.unwrap(), None);
    db.set_verify_command("/r", None).await.unwrap();
    assert_eq!(db.get_verify_command("/r").await.unwrap(), None);
}

#[tokio::test]
async fn verify_command_rejects_newline() {
    let db = in_memory_db().await;
    db.save_repo_path("/r").await.unwrap();
    let err = db.set_verify_command("/r", Some("a\nb")).await.unwrap_err();
    assert!(
        err.to_string().to_lowercase().contains("newline"),
        "expected newline error, got: {err}"
    );
    let err = db.set_verify_command("/r", Some("a\rb")).await.unwrap_err();
    assert!(
        err.to_string().to_lowercase().contains("carriage return"),
        "expected carriage return error, got: {err}"
    );
}

#[tokio::test]
async fn verify_command_set_some_creates_row() {
    let db = in_memory_db().await;
    db.set_verify_command("/new/path", Some("cargo test"))
        .await
        .unwrap();
    assert!(db
        .list_repo_paths()
        .await
        .unwrap()
        .iter()
        .any(|p| p == "/new/path"));
    assert_eq!(
        db.get_verify_command("/new/path").await.unwrap(),
        Some("cargo test".to_string())
    );
}

#[tokio::test]
async fn verify_command_set_none_on_unknown_path_is_noop() {
    let db = in_memory_db().await;
    db.set_verify_command("/unknown", None).await.unwrap();
    assert!(!db
        .list_repo_paths()
        .await
        .unwrap()
        .iter()
        .any(|p| p == "/unknown"));
}

#[tokio::test]
async fn verify_command_get_unknown_path_is_none() {
    let db = in_memory_db().await;
    assert_eq!(
        db.get_verify_command("/does/not/exist").await.unwrap(),
        None
    );
}

#[tokio::test]
async fn list_filter_presets_errors_on_corrupt_json() {
    let db = in_memory_db().await;
    db.db_call(move |conn| {
        conn.execute(
            "INSERT INTO filter_presets (name, repo_paths, mode) VALUES (?1, ?2, ?3)",
            rusqlite::params!["bad_preset", "{not json", "all"],
        )?;
        Ok(())
    })
    .await
    .unwrap();
    let result = db.list_filter_presets().await;
    assert!(
        result.is_err(),
        "expected Err on corrupt filter preset JSON, got {:?}",
        result
    );
}

#[tokio::test]
async fn delete_repo_path_errors_on_corrupt_preset_json() {
    let db = in_memory_db().await;
    db.save_repo_path("/repo").await.unwrap();
    db.db_call(move |conn| {
        conn.execute(
            "INSERT INTO filter_presets (name, repo_paths, mode) VALUES (?1, ?2, ?3)",
            rusqlite::params!["bad_preset", "{not json", "all"],
        )?;
        Ok(())
    })
    .await
    .unwrap();
    let result = db.delete_repo_path("/repo").await;
    assert!(
        result.is_err(),
        "expected Err when corrupt preset JSON is encountered during delete"
    );
}

// --- managed-feed config keys (WP5) ---

#[tokio::test]
async fn reviews_feed_command_round_trips_and_clears() {
    let db = in_memory_db().await;
    assert_eq!(db.get_reviews_feed_command().await.unwrap(), None);
    db.set_reviews_feed_command(Some("/scripts/fetch-reviews.sh"))
        .await
        .unwrap();
    assert_eq!(
        db.get_reviews_feed_command().await.unwrap(),
        Some("/scripts/fetch-reviews.sh".to_string())
    );
    db.set_reviews_feed_command(None).await.unwrap();
    assert_eq!(db.get_reviews_feed_command().await.unwrap(), None);
}

#[tokio::test]
async fn reviews_feed_interval_secs_round_trips_and_clears() {
    let db = in_memory_db().await;
    assert_eq!(db.get_reviews_feed_interval_secs().await.unwrap(), None);
    db.set_reviews_feed_interval_secs(Some(300)).await.unwrap();
    assert_eq!(
        db.get_reviews_feed_interval_secs().await.unwrap(),
        Some(300)
    );
    db.set_reviews_feed_interval_secs(None).await.unwrap();
    assert_eq!(db.get_reviews_feed_interval_secs().await.unwrap(), None);
}

#[tokio::test]
async fn cve_feed_command_round_trips_and_clears() {
    let db = in_memory_db().await;
    assert_eq!(db.get_cve_feed_command().await.unwrap(), None);
    db.set_cve_feed_command(Some("/scripts/fetch-cve.sh"))
        .await
        .unwrap();
    assert_eq!(
        db.get_cve_feed_command().await.unwrap(),
        Some("/scripts/fetch-cve.sh".to_string())
    );
    db.set_cve_feed_command(None).await.unwrap();
    assert_eq!(db.get_cve_feed_command().await.unwrap(), None);
}

#[tokio::test]
async fn cve_feed_interval_secs_round_trips_and_clears() {
    let db = in_memory_db().await;
    assert_eq!(db.get_cve_feed_interval_secs().await.unwrap(), None);
    db.set_cve_feed_interval_secs(Some(900)).await.unwrap();
    assert_eq!(db.get_cve_feed_interval_secs().await.unwrap(), Some(900));
    db.set_cve_feed_interval_secs(None).await.unwrap();
    assert_eq!(db.get_cve_feed_interval_secs().await.unwrap(), None);
}

// ---------------------------------------------------------------------------
// repo_base_branches — per-repo base_branch history. See docs/specs/dispatch.allium
// (rule RecordBaseBranch, surface BaseBranchPicker, config.max_base_branches_per_repo,
// invariant BranchHistoryCapped) and docs/specs/core.allium (entity SavedRepoBranch).
// ---------------------------------------------------------------------------

/// Overwrite a (repo_path, branch) row's `last_used` via raw SQL so recency
/// ordering tests are deterministic instead of racing datetime('now') second
/// resolution (mirrors `seed_learning_with_score_and_updated` in
/// db/tests/learnings.rs).
async fn set_base_branch_last_used(db: &Database, repo_path: &str, branch: &str, last_used: &str) {
    let repo_path = repo_path.to_string();
    let branch = branch.to_string();
    let last_used = last_used.to_string();
    db.db_call(move |conn| {
        conn.execute(
            "UPDATE repo_base_branches SET last_used = ?1 WHERE repo_path = ?2 AND branch = ?3",
            rusqlite::params![last_used, repo_path, branch],
        )?;
        Ok(())
    })
    .await
    .unwrap();
}

#[tokio::test]
async fn list_all_base_branches_empty_by_default() {
    let db = in_memory_db().await;
    assert!(db.list_all_base_branches().await.unwrap().is_empty());
}

#[tokio::test]
async fn record_base_branch_inserts_new_row() {
    let db = in_memory_db().await;
    db.record_base_branch("/repo/a", "main").await.unwrap();
    let all = db.list_all_base_branches().await.unwrap();
    assert_eq!(all, vec![("/repo/a".to_string(), "main".to_string())]);
}

#[tokio::test]
async fn record_base_branch_upserts_and_bumps_last_used() {
    let db = in_memory_db().await;
    db.record_base_branch("/repo/a", "main").await.unwrap();
    set_base_branch_last_used(&db, "/repo/a", "main", "2000-01-01 00:00:00").await;
    db.record_base_branch("/repo/a", "develop").await.unwrap();
    set_base_branch_last_used(&db, "/repo/a", "develop", "2000-01-02 00:00:00").await;

    // Re-recording "main" bumps its last_used back to now, making it
    // most-recent again, and must not create a duplicate row.
    db.record_base_branch("/repo/a", "main").await.unwrap();

    let all = db.list_all_base_branches().await.unwrap();
    assert_eq!(all.len(), 2, "upsert must not create a duplicate row");
    assert_eq!(
        all[0],
        ("/repo/a".to_string(), "main".to_string()),
        "re-recording main should bump it back to most-recently-used"
    );
}

#[tokio::test]
async fn record_base_branch_prunes_to_ten_most_recently_used_per_repo() {
    let db = in_memory_db().await;
    for i in 0..10 {
        let branch = format!("branch-{i}");
        db.record_base_branch("/repo/a", &branch).await.unwrap();
        set_base_branch_last_used(&db, "/repo/a", &branch, &format!("2000-01-01 00:00:{i:02}"))
            .await;
    }
    // The 11th write for the same repo (config.max_base_branches_per_repo = 10)
    // must prune the least-recently-used row (branch-0).
    db.record_base_branch("/repo/a", "branch-10").await.unwrap();

    let all = db.list_all_base_branches().await.unwrap();
    let branches_for_a: Vec<&str> = all
        .iter()
        .filter(|(repo, _)| repo == "/repo/a")
        .map(|(_, b)| b.as_str())
        .collect();
    assert_eq!(
        branches_for_a.len(),
        10,
        "repo history must be capped at config.max_base_branches_per_repo (10), got: {branches_for_a:?}"
    );
    assert!(
        !branches_for_a.contains(&"branch-0"),
        "oldest (least-recently-used) branch should be pruned, got: {branches_for_a:?}"
    );
    assert!(branches_for_a.contains(&"branch-10"));
}

#[tokio::test]
async fn record_base_branch_pruning_is_scoped_per_repo() {
    let db = in_memory_db().await;
    for i in 0..10 {
        let branch = format!("a-branch-{i}");
        db.record_base_branch("/repo/a", &branch).await.unwrap();
        set_base_branch_last_used(&db, "/repo/a", &branch, &format!("2000-01-01 00:00:{i:02}"))
            .await;
    }
    db.record_base_branch("/repo/b", "b-branch").await.unwrap();

    // Writing an 11th branch for repo A prunes repo A's history only —
    // repo B's single row must survive untouched.
    db.record_base_branch("/repo/a", "a-branch-10")
        .await
        .unwrap();

    let all = db.list_all_base_branches().await.unwrap();
    assert!(
        all.iter()
            .any(|(repo, branch)| repo == "/repo/b" && branch == "b-branch"),
        "repo B's history must be untouched by repo A's pruning, got: {all:?}"
    );
}

#[tokio::test]
async fn list_all_base_branches_orders_by_last_used_desc_across_repos() {
    let db = in_memory_db().await;
    db.record_base_branch("/repo/a", "main").await.unwrap();
    set_base_branch_last_used(&db, "/repo/a", "main", "2000-01-01 00:00:00").await;
    db.record_base_branch("/repo/b", "develop").await.unwrap();
    set_base_branch_last_used(&db, "/repo/b", "develop", "2000-01-03 00:00:00").await;
    db.record_base_branch("/repo/a", "feature-x").await.unwrap();
    set_base_branch_last_used(&db, "/repo/a", "feature-x", "2000-01-02 00:00:00").await;

    let all = db.list_all_base_branches().await.unwrap();
    assert_eq!(
        all,
        vec![
            ("/repo/b".to_string(), "develop".to_string()),
            ("/repo/a".to_string(), "feature-x".to_string()),
            ("/repo/a".to_string(), "main".to_string()),
        ]
    );
}
