#![allow(clippy::unwrap_used, clippy::expect_used)]
use super::*;
use crate::models::{test_tmux_window, EpicId, TaskStatus};

fn test_task(id: i64, title: &str) -> Task {
    test_task_repo(id, title, "/repo")
}

fn test_task_repo(id: i64, title: &str, repo: &str) -> Task {
    Task {
        title: title.to_string(),
        repo_path: repo.to_string(),
        ..make_task(id, TaskStatus::Backlog)
    }
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

// ---------------------------------------------------------------------------
// Finding 1 — a descendant only counts when the board would itself show it
// ---------------------------------------------------------------------------

#[test]
fn epic_search_matches_task_hidden_by_repo_filter_is_not_a_dead_end() {
    // Reviewer's repro: the epic's only query-matching subtask is excluded
    // by the repo filter, and the subtask the filter keeps does not match
    // the query. Keeping the card alive would be a dead end — entering the
    // epic view would show zero tasks.
    let mut matching = epic_child(10, 1, "Fix login bug");
    matching.repo_path = "/repo/b".to_string();
    let mut kept_by_filter = epic_child(11, 1, "Update invoices");
    kept_by_filter.repo_path = "/repo/a".to_string();
    let mut app = App::new(vec![matching, kept_by_filter]);
    app.board.epics = vec![make_epic_with_title(1, "Billing rework")];
    app.filter.repos.insert("/repo/a".to_string());
    app.filter.mode = RepoFilterMode::Include;
    app.search.query = "login".to_string();
    assert!(!app.epic_search_matches(EpicId(1)));
}

#[test]
fn epic_search_matches_task_inside_filtered_repo_keeps_the_epic() {
    // Same epic, but this time the query-matching subtask is the one the
    // repo filter keeps: the epic is a genuine drill-down target.
    let mut matching = epic_child(10, 1, "Fix login bug");
    matching.repo_path = "/repo/a".to_string();
    let mut excluded = epic_child(11, 1, "Update invoices");
    excluded.repo_path = "/repo/b".to_string();
    let mut app = App::new(vec![matching, excluded]);
    app.board.epics = vec![make_epic_with_title(1, "Billing rework")];
    app.filter.repos.insert("/repo/a".to_string());
    app.filter.mode = RepoFilterMode::Include;
    app.search.query = "login".to_string();
    assert!(app.epic_search_matches(EpicId(1)));
}

#[test]
fn epic_search_matches_task_hidden_by_only_active_is_not_a_dead_end() {
    // Only-active counterpart of the repo-filter dead-end above: the
    // query-matching subtask has no live tmux window, and the subtask that
    // does have one doesn't match the query.
    let mut matching = epic_child(10, 1, "Fix login bug");
    matching.tmux_window = None;
    let mut active = epic_child(11, 1, "Update invoices");
    active.tmux_window = Some(test_tmux_window("task-11"));
    let mut app = App::new(vec![matching, active]);
    app.board.epics = vec![make_epic_with_title(1, "Billing rework")];
    app.filter.only_active = true;
    app.search.query = "login".to_string();
    assert!(!app.epic_search_matches(EpicId(1)));
}

#[test]
fn epic_search_matches_hidden_sub_epic_does_not_keep_parent_visible() {
    // Sub-epic 2 matches by title ("login"), but its own subtask is
    // excluded by the repo filter, so epic_repo_matches(2) is false — epic 2
    // itself would not be rendered. It must not keep its parent (epic 1)
    // visible either.
    let mut hidden_task = epic_child(10, 2, "Some infra work");
    hidden_task.repo_path = "/repo/b".to_string();
    let mut app = App::new(vec![hidden_task]);
    let mut sub = make_epic_with_title(2, "Login redesign");
    sub.parent_epic_id = Some(EpicId(1));
    app.board.epics = vec![make_epic_with_title(1, "Billing rework"), sub];
    app.filter.repos.insert("/repo/a".to_string());
    app.filter.mode = RepoFilterMode::Include;
    app.search.query = "login".to_string();
    assert!(!app.epic_search_matches(EpicId(1)));
}

// ---------------------------------------------------------------------------
// Finding 3 — strict in-epic task filtering even when the epic matched by title
// ---------------------------------------------------------------------------

#[test]
fn epic_matched_by_own_title_still_strictly_filters_subtasks_in_view() {
    use crate::tui::messages::EpicMessage;
    use crate::tui::types::Message;

    let mut app = App::new(vec![epic_child(10, 1, "Refactor parser")]);
    app.board.epics = vec![make_epic_with_title(1, "Login redesign")];
    app.update(Message::Epic(EpicMessage::Enter(EpicId(1))));
    app.search.query = "login".to_string();
    // The epic is surfaced by its own title, but its only subtask does not
    // match the query — the epic view shows no tasks, which may be none.
    assert!(app.tasks_for_current_view().is_empty());
}

// ---------------------------------------------------------------------------
// Finding 4 — epic-side edge cases mirroring the task predicate's
// ---------------------------------------------------------------------------

#[test]
fn epic_search_query_non_numeric_does_not_id_match() {
    let mut app = App::new(vec![]);
    app.board.epics = vec![make_epic_with_title(3837, "alpha")];
    // "38a" has a non-digit payload, so no id matching happens and the title
    // is not a subsequence match either.
    app.search.query = "38a".to_string();
    assert!(!app.epic_search_matches(EpicId(3837)));
}

#[test]
fn epic_search_query_bare_hash_does_not_id_match() {
    let mut app = App::new(vec![]);
    app.board.epics = vec![make_epic_with_title(3837, "alpha")];
    // A lone "#" leaves an empty digit payload: title-only matching, and the
    // title does not contain '#'.
    app.search.query = "#".to_string();
    assert!(!app.epic_search_matches(EpicId(3837)));
}

#[test]
fn epic_and_task_id_namespaces_are_independent() {
    // Epic #1 and task #1 both exist; the two id sequences are separate and
    // the predicates are independent, so both cards are shown.
    let mut app = App::new(vec![test_task(1, "alpha")]);
    app.board.epics = vec![make_epic_with_title(1, "beta")];
    app.search.query = "1".to_string();
    assert_eq!(visible_epic_ids(&app), vec![1]);
    let ids: Vec<i64> = app
        .tasks_for_current_view()
        .iter()
        .map(|t| t.id.0)
        .collect();
    assert_eq!(ids, vec![1]);
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

#[test]
fn epic_ids_owning_matching_task_collects_only_board_visible_matches() {
    use crate::tui::epic_ids_owning_matching_task;

    let matching = epic_child(10, 1, "Fix login bug");
    let mut wrong_repo = epic_child(11, 2, "Fix login bug");
    wrong_repo.repo_path = "/repo/b".to_string();
    let mut archived = epic_child(12, 3, "Fix login bug");
    archived.status = TaskStatus::Archived;
    let no_match = epic_child(13, 4, "Update invoices");
    let standalone = test_task(14, "Fix login bug"); // epic_id is None

    let mut filter = FilterState::default();
    filter.repos.insert("/repo".to_string());
    filter.mode = RepoFilterMode::Include;

    let owners = epic_ids_owning_matching_task(
        &[matching, wrong_repo, archived, no_match, standalone],
        &filter,
        "login",
        None,
    );

    // Only epic 1: epic 2's match is outside the repo filter, epic 3's is
    // archived, epic 4 has no query match, and the standalone task owns no epic.
    assert_eq!(owners, [EpicId(1)].into_iter().collect());
}

// ---------------------------------------------------------------------------
// visible epic cards under a live query
// ---------------------------------------------------------------------------

#[test]
fn search_hides_non_matching_epic_cards_on_the_board() {
    let mut app = App::new(vec![]);
    app.board.epics = vec![
        make_epic_with_title(1, "Login redesign"),
        make_epic_with_title(2, "Billing rework"),
    ];
    app.search.query = "login".to_string();
    assert_eq!(visible_epic_ids(&app), vec![1]);
}

#[test]
fn empty_search_keeps_every_epic_card() {
    let mut app = App::new(vec![]);
    app.board.epics = vec![
        make_epic_with_title(1, "Login redesign"),
        make_epic_with_title(2, "Billing rework"),
    ];
    assert_eq!(visible_epic_ids(&app), vec![1, 2]);
}

#[test]
fn search_keeps_epic_whose_subtask_matches() {
    let mut app = App::new(vec![epic_child(10, 2, "Fix login bug")]);
    app.board.epics = vec![
        make_epic_with_title(1, "Docs cleanup"),
        make_epic_with_title(2, "Billing rework"),
    ];
    app.search.query = "login".to_string();
    // Epic 2 has no own match but owns the matching task; epic 1 has neither.
    assert_eq!(visible_epic_ids(&app), vec![2]);
}

#[test]
fn search_on_epics_composes_with_repo_filter() {
    let mut matching = epic_child(10, 1, "Fix login bug");
    matching.repo_path = "/repo/a".to_string();
    let mut non_matching = epic_child(11, 2, "Update invoices");
    non_matching.repo_path = "/repo/a".to_string();
    let mut app = App::new(vec![matching, non_matching]);
    app.board.epics = vec![
        make_epic_with_title(1, "Login redesign"),
        make_epic_with_title(2, "Billing rework"),
    ];
    app.filter.repos.insert("/repo/a".to_string());
    app.filter.mode = RepoFilterMode::Include;
    app.search.query = "login".to_string();
    // Both epics' subtasks pass the repo filter; only epic 1's subtask also
    // matches the query — proves the AND is real, not just "repo filter
    // hides everything".
    assert_eq!(visible_epic_ids(&app), vec![1]);
}

#[test]
fn search_on_epics_composes_with_only_active_filter() {
    let mut matching = epic_child(10, 1, "Fix login bug");
    matching.tmux_window = Some(test_tmux_window("task-10"));
    let mut non_matching = epic_child(11, 2, "Update invoices");
    non_matching.tmux_window = Some(test_tmux_window("task-11"));
    let mut app = App::new(vec![matching, non_matching]);
    app.board.epics = vec![
        make_epic_with_title(1, "Login redesign"),
        make_epic_with_title(2, "Billing rework"),
    ];
    app.filter.only_active = true;
    app.search.query = "login".to_string();
    // Both epics have an active subtask (only-active passes); only epic 1's
    // subtask also matches the query — proves the AND is real, not just
    // "only-active hides everything".
    assert_eq!(visible_epic_ids(&app), vec![1]);
}

#[test]
fn visible_epic_cards_agree_with_the_single_epic_predicate() {
    // The view pass and the per-epic predicate are two paths to the same
    // verdict: one builds an index once, the other builds a one-shot index.
    // A multi-level board with a hidden sub-epic, a matching grandchild task
    // and an unrelated subtree exercises every branch of both.
    let mut matching_grandchild = epic_child(10, 3, "Fix login bug");
    matching_grandchild.repo_path = "/repo/a".to_string();
    let mut hidden_task = epic_child(11, 4, "Some infra work");
    hidden_task.repo_path = "/repo/b".to_string();
    let mut unrelated = epic_child(12, 5, "Update invoices");
    unrelated.repo_path = "/repo/a".to_string();
    let mut app = App::new(vec![matching_grandchild, hidden_task, unrelated]);

    let mut deep = make_epic_with_title(3, "Deep");
    deep.parent_epic_id = Some(EpicId(2));
    let mut mid = make_epic_with_title(2, "Mid");
    mid.parent_epic_id = Some(EpicId(1));
    // Epic 4 matches by title but its only subtask is excluded by the repo
    // filter, so it must not keep its parent (epic 6) visible.
    let mut hidden_sub = make_epic_with_title(4, "Login redesign");
    hidden_sub.parent_epic_id = Some(EpicId(6));
    app.board.epics = vec![
        make_epic_with_title(1, "Root"),
        mid,
        deep,
        hidden_sub,
        make_epic_with_title(5, "Billing rework"),
        make_epic_with_title(6, "Docs cleanup"),
    ];
    app.filter.repos.insert("/repo/a".to_string());
    app.filter.mode = RepoFilterMode::Include;
    app.search.query = "login".to_string();

    // Epic 1 is kept by its matching great-grandchild task; epics 5 and 6 are
    // not kept (5's subtask does not match, 6's matching sub-epic is hidden).
    assert_eq!(visible_epic_ids(&app), vec![1]);

    // Same verdict, epic by epic, from the single-epic predicate. Root epics
    // only — the view pass shows root epics in Board mode.
    let via_predicate: Vec<i64> = [1, 5, 6]
        .into_iter()
        .filter(|&id| {
            let eid = EpicId(id);
            app.epic_matches(eid) && app.epic_repo_matches(eid) && app.epic_search_matches(eid)
        })
        .collect();
    assert_eq!(via_predicate, vec![1]);
}

#[test]
fn visible_epic_cards_do_not_read_the_layout_cache_for_search() {
    // The view-pass counterpart of
    // epic_search_matches_does_not_read_the_layout_cache: the index the pass
    // builds is intra-call, so a cache populated before the keystroke must not
    // answer for search.
    let mut app = App::new(vec![]);
    app.board.epics = vec![
        make_epic_with_title(1, "Login redesign"),
        make_epic_with_title(2, "Billing rework"),
    ];
    let _ = app.cached_epic_stats(); // populates layout.epic_filter_cache
    app.search.query = "zzz".to_string();
    assert!(visible_epic_ids(&app).is_empty());
    app.search.query = "login".to_string();
    assert_eq!(visible_epic_ids(&app), vec![1]);
}

#[test]
fn search_narrows_sub_epic_cards_inside_an_epic_view() {
    use crate::tui::messages::EpicMessage;
    use crate::tui::types::Message;

    let mut app = App::new(vec![]);
    let mut a = make_epic_with_title(2, "Login redesign");
    a.parent_epic_id = Some(EpicId(1));
    let mut b = make_epic_with_title(3, "Billing rework");
    b.parent_epic_id = Some(EpicId(1));
    app.board.epics = vec![make_epic_with_title(1, "Root"), a, b];
    app.update(Message::Epic(EpicMessage::Enter(EpicId(1))));
    app.search.query = "login".to_string();
    assert_eq!(visible_epic_ids(&app), vec![2]);
}

#[test]
fn search_does_not_narrow_the_move_task_epic_picker() {
    let mut app = App::new(vec![]);
    app.board.epics = vec![
        make_epic_with_title(1, "Login redesign"),
        make_epic_with_title(2, "Billing rework"),
    ];
    app.search.query = "login".to_string();
    let mut ids: Vec<i64> = app
        .move_task_target_epics()
        .iter()
        .map(|e| e.id.0)
        .collect();
    ids.sort_unstable();
    assert_eq!(ids, vec![1, 2], "the picker has its own buffer");
}

// ---------------------------------------------------------------------------
// One index per pass, not one per column
// ---------------------------------------------------------------------------

/// How many `EpicSearchIndex`es `f` builds. Reset-then-read, so each test owns
/// the counter (it is thread-local and tests get their own thread).
fn count_index_builds(f: impl FnOnce()) -> usize {
    crate::tui::EPIC_SEARCH_INDEX_BUILDS.with(|c| c.set(0));
    f();
    crate::tui::EPIC_SEARCH_INDEX_BUILDS.with(|c| c.get())
}

/// A board with epic cards in more than one status column, so every column of a
/// build pass has epic work to do, plus a live query.
fn multi_column_searchable_board() -> App {
    let mut app = App::new(vec![
        epic_child(10, 1, "Fix login bug"),
        epic_child(11, 2, "Update invoices"),
    ]);
    let mut running = make_epic_with_title(1, "Login redesign");
    running.status = TaskStatus::Running;
    let mut review = make_epic_with_title(2, "Billing rework");
    review.status = TaskStatus::Review;
    app.board.epics = vec![running, review];
    app.search.query = "login".to_string();
    app
}

#[test]
fn column_layout_build_builds_the_search_index_once() {
    let app = multi_column_searchable_board();
    let stats = app.compute_epic_stats();
    let builds = count_index_builds(|| {
        let _ = crate::tui::types::ColumnLayout::build(&app, &stats);
    });
    assert_eq!(
        builds, 1,
        "one index per frame, not one per status column: see docs on EpicSearchIndex"
    );
}

#[test]
fn column_layout_build_builds_no_search_index_without_a_query() {
    let mut app = multi_column_searchable_board();
    app.search.query = String::new();
    let stats = app.compute_epic_stats();
    let builds = count_index_builds(|| {
        let _ = crate::tui::types::ColumnLayout::build(&app, &stats);
    });
    assert_eq!(builds, 0, "a non-searching render pays nothing");
}

#[test]
fn flattened_columns_build_no_search_index() {
    // Flattened columns show tasks only and never ask an epic-visibility
    // question, so the pass must stay unbuilt even with a query live. Backlog
    // is exempt from flattening, so it is the one column that still asks.
    let mut app = multi_column_searchable_board();
    app.board.flattened = true;
    let stats = app.compute_epic_stats();
    let builds = count_index_builds(|| {
        for status in [TaskStatus::Running, TaskStatus::Review, TaskStatus::Done] {
            let _ = app.column_items_for_status_with_stats(status, Some(&stats));
        }
    });
    assert_eq!(builds, 0, "a flattened column consults no epic index");
}

#[test]
fn cached_epic_stats_builds_the_search_index_once() {
    let mut app = multi_column_searchable_board();
    let builds = count_index_builds(|| {
        let _ = app.cached_epic_stats();
    });
    assert_eq!(builds, 1, "the anchor-cache loop shares one index");
}

#[test]
fn clamp_selection_builds_the_search_index_once() {
    let mut app = multi_column_searchable_board();
    let builds = count_index_builds(|| app.clamp_selection());
    assert_eq!(builds, 1, "one index for all four column counts");
}

#[test]
fn search_does_not_narrow_the_reparent_epic_picker() {
    let mut app = App::new(vec![]);
    app.board.epics = vec![
        make_epic_with_title(1, "Login redesign"),
        make_epic_with_title(2, "Billing rework"),
        make_epic_with_title(3, "Docs cleanup"),
    ];
    app.search.query = "login".to_string();
    // Reparent targets for epic 3: everything except itself, unfiltered by search.
    let mut ids: Vec<i64> = app
        .reparent_target_epics(EpicId(3))
        .iter()
        .map(|e| e.id.0)
        .collect();
    ids.sort_unstable();
    assert_eq!(ids, vec![1, 2], "the picker has its own buffer");
}
