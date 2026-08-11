#![allow(clippy::unwrap_used, clippy::expect_used)]
use super::*;
use crate::models::{EpicId, TaskStatus};

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

// ---------------------------------------------------------------------------
// epic_search_matches — see board_search_filter in docs/specs/core.allium
// ---------------------------------------------------------------------------

/// A subtask of `epic` with an explicit title.
fn epic_child(id: i64, epic: i64, title: &str) -> Task {
    let mut t = test_task(id, title);
    t.epic_id = Some(EpicId(epic));
    t
}

#[test]
fn epic_search_matches_empty_query_matches_every_epic() {
    let mut app = App::new(vec![]);
    app.board.epics = vec![make_epic_with_title(1, "Billing rework")];
    assert!(app.epic_search_matches(EpicId(1)));
}

#[test]
fn epic_search_matches_own_title_fuzzy() {
    let mut app = App::new(vec![]);
    app.board.epics = vec![
        make_epic_with_title(1, "Login redesign"),
        make_epic_with_title(2, "Billing rework"),
    ];
    app.search.query = "lgn".to_string();
    assert!(app.epic_search_matches(EpicId(1)));
    assert!(!app.epic_search_matches(EpicId(2)));
}

#[test]
fn epic_search_matches_own_id_prefix() {
    let mut app = App::new(vec![]);
    app.board.epics = vec![
        make_epic_with_title(38, "alpha"),
        make_epic_with_title(380, "beta"),
        make_epic_with_title(1385, "gamma"),
    ];
    app.search.query = "38".to_string();
    assert!(app.epic_search_matches(EpicId(38)));
    assert!(app.epic_search_matches(EpicId(380)));
    // Prefix, not substring: 1385 contains "38" but does not start with it.
    assert!(!app.epic_search_matches(EpicId(1385)));
}

#[test]
fn epic_search_matches_own_id_with_hash_prefix() {
    let mut app = App::new(vec![]);
    app.board.epics = vec![make_epic_with_title(38, "alpha")];
    app.search.query = "#38".to_string();
    assert!(app.epic_search_matches(EpicId(38)));
}

#[test]
fn epic_search_matches_descendant_task_title() {
    let mut app = App::new(vec![epic_child(10, 1, "Fix login bug")]);
    app.board.epics = vec![make_epic_with_title(1, "Billing rework")];
    app.search.query = "login".to_string();
    // The epic's own title has no match; its subtask carries it.
    assert!(app.epic_search_matches(EpicId(1)));
}

#[test]
fn epic_search_matches_descendant_task_id() {
    let mut app = App::new(vec![epic_child(3837, 1, "alpha")]);
    app.board.epics = vec![make_epic_with_title(1, "Billing rework")];
    app.search.query = "3837".to_string();
    assert!(app.epic_search_matches(EpicId(1)));
}

#[test]
fn epic_search_matches_descendant_sub_epic_title() {
    let mut app = App::new(vec![]);
    let mut child = make_epic_with_title(2, "Login redesign");
    child.parent_epic_id = Some(EpicId(1));
    app.board.epics = vec![make_epic_with_title(1, "Billing rework"), child];
    app.search.query = "login".to_string();
    // Root epic kept because a sub-epic in its subtree matches.
    assert!(app.epic_search_matches(EpicId(1)));
}

#[test]
fn epic_search_matches_grandchild_task_title() {
    let mut app = App::new(vec![epic_child(10, 2, "Fix login bug")]);
    let mut child = make_epic_with_title(2, "Sub");
    child.parent_epic_id = Some(EpicId(1));
    app.board.epics = vec![make_epic_with_title(1, "Root"), child];
    app.search.query = "login".to_string();
    assert!(app.epic_search_matches(EpicId(1)));
}

#[test]
fn epic_search_matches_no_match_anywhere_is_false() {
    let mut app = App::new(vec![epic_child(10, 1, "Update invoices")]);
    app.board.epics = vec![make_epic_with_title(1, "Billing rework")];
    app.search.query = "login".to_string();
    assert!(!app.epic_search_matches(EpicId(1)));
}

#[test]
fn epic_search_matches_ignores_archived_descendant_task() {
    let mut archived = epic_child(10, 1, "Fix login bug");
    archived.status = TaskStatus::Archived;
    let mut app = App::new(vec![archived]);
    app.board.epics = vec![make_epic_with_title(1, "Billing rework")];
    app.search.query = "login".to_string();
    assert!(!app.epic_search_matches(EpicId(1)));
}

#[test]
fn epic_search_matches_ignores_task_in_a_different_epic() {
    let mut app = App::new(vec![epic_child(10, 2, "Fix login bug")]);
    app.board.epics = vec![
        make_epic_with_title(1, "Billing rework"),
        make_epic_with_title(2, "Auth"),
    ];
    app.search.query = "login".to_string();
    // Epic 2 is not a descendant of epic 1 (no parent link).
    assert!(!app.epic_search_matches(EpicId(1)));
    assert!(app.epic_search_matches(EpicId(2)));
}

#[test]
fn epic_search_matches_does_not_read_the_layout_cache() {
    // A stale cache must never answer for search: the layout fingerprint
    // covers neither titles nor the query.
    let mut app = App::new(vec![]);
    app.board.epics = vec![make_epic_with_title(1, "Login redesign")];
    let _ = app.cached_epic_stats(); // populates layout.epic_filter_cache
    app.search.query = "zzz".to_string();
    assert!(!app.epic_search_matches(EpicId(1)));
}
