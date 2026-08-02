#![allow(clippy::unwrap_used, clippy::expect_used)]
use super::*;
use crate::models::TaskStatus;

fn test_task(id: i64, title: &str) -> Task {
    test_task_repo(id, title, "/repo")
}

fn test_task_repo(id: i64, title: &str, repo: &str) -> Task {
    let mut t = make_task(id, TaskStatus::Backlog);
    t.title = title.to_string();
    t.repo_path = repo.to_string();
    t
}

#[test]
fn new_app_has_inactive_search() {
    let app = App::new(vec![]);
    assert_eq!(app.search.query, "");
    assert!(!app.search_active());
}

#[test]
fn search_query_filters_by_title_fuzzy() {
    let mut app = App::new(vec![
        test_task(1, "Fix login bug"),
        test_task(2, "Add search feature"),
        test_task(3, "Refactor parser"),
    ]);
    // "srch" is an ordered subsequence of "Add search feature" (s…e[a]r[c]h),
    // but NOT of "Fix login bug" or "Refactor parser" (neither contains
    // s…r…c…h in order), so the single-match assertion below is meaningful.
    app.search.query = "srch".to_string();
    let titles: Vec<&str> = app
        .tasks_for_current_view()
        .iter()
        .map(|t| t.title.as_str())
        .collect();
    assert_eq!(titles, vec!["Add search feature"]);
}

#[test]
fn empty_search_query_is_noop() {
    let mut app = App::new(vec![test_task(1, "alpha"), test_task(2, "beta")]);
    app.search.query = "".to_string();
    assert_eq!(app.tasks_for_current_view().len(), 2);
}

#[test]
fn search_query_matches_task_id_prefix() {
    let mut app = App::new(vec![
        test_task(38, "alpha"),
        test_task(380, "beta"),
        test_task(3837, "gamma"),
        test_task(9, "delta"),
    ]);
    app.search.query = "38".to_string();
    let mut ids: Vec<i64> = app
        .tasks_for_current_view()
        .iter()
        .map(|t| t.id.0)
        .collect();
    ids.sort_unstable();
    assert_eq!(ids, vec![38, 380, 3837]);
}

#[test]
fn search_query_matches_task_id_with_hash_prefix() {
    let mut app = App::new(vec![test_task(3837, "alpha"), test_task(9, "beta")]);
    app.search.query = "#3837".to_string();
    let ids: Vec<i64> = app
        .tasks_for_current_view()
        .iter()
        .map(|t| t.id.0)
        .collect();
    assert_eq!(ids, vec![3837]);
}

#[test]
fn search_query_id_match_unions_with_title_match() {
    let mut app = App::new(vec![
        test_task(3837, "alpha"),
        test_task(7, "fix 3837 regression"),
        test_task(9, "beta"),
    ]);
    app.search.query = "3837".to_string();
    let mut ids: Vec<i64> = app
        .tasks_for_current_view()
        .iter()
        .map(|t| t.id.0)
        .collect();
    ids.sort_unstable();
    assert_eq!(ids, vec![7, 3837]);
}

#[test]
fn search_query_non_numeric_does_not_id_match() {
    let mut app = App::new(vec![test_task(3837, "alpha"), test_task(9, "beta")]);
    // "38a" has a non-digit payload, so no id matching happens and neither
    // title is a subsequence match.
    app.search.query = "38a".to_string();
    assert!(app.tasks_for_current_view().is_empty());
}

#[test]
fn search_query_bare_hash_does_not_id_match() {
    let mut app = App::new(vec![test_task(3837, "alpha"), test_task(9, "beta")]);
    // A lone "#" leaves an empty digit payload: title-only matching, and no
    // title contains '#'.
    app.search.query = "#".to_string();
    assert!(app.tasks_for_current_view().is_empty());
}

#[test]
fn search_query_id_prefix_is_not_a_substring_match() {
    let mut app = App::new(vec![test_task(1385, "alpha"), test_task(38, "beta")]);
    app.search.query = "38".to_string();
    let ids: Vec<i64> = app
        .tasks_for_current_view()
        .iter()
        .map(|t| t.id.0)
        .collect();
    assert_eq!(ids, vec![38]);
}

#[test]
fn search_id_match_composes_with_repo_filter() {
    let mut app = App::new(vec![
        test_task_repo(3837, "alpha", "/repo/a"),
        test_task_repo(3838, "beta", "/repo/b"),
    ]);
    app.filter.repos.insert("/repo/a".to_string());
    app.filter.mode = RepoFilterMode::Include;
    app.search.query = "383".to_string();
    let ids: Vec<i64> = app
        .tasks_for_current_view()
        .iter()
        .map(|t| t.id.0)
        .collect();
    assert_eq!(ids, vec![3837]); // repo filter AND id-prefix match
}

#[test]
fn search_composes_with_repo_filter() {
    let mut app = App::new(vec![
        test_task_repo(1, "alpha task", "/repo/a"),
        test_task_repo(2, "alpha task", "/repo/b"),
    ]);
    app.filter.repos.insert("/repo/a".to_string());
    app.filter.mode = RepoFilterMode::Include;
    app.search.query = "alpha".to_string();
    let ids: Vec<i64> = app
        .tasks_for_current_view()
        .iter()
        .map(|t| t.id.0)
        .collect();
    assert_eq!(ids, vec![1]); // repo filter AND title match
}
