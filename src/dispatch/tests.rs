#![allow(clippy::unwrap_used, clippy::expect_used)]
use super::agents::prompt_launch_command;
use super::mock_sequence::{
    pr_view_reply, DispatchScript, FinishRun, PrHead, Step, COMPANION_PANE_ID,
};
use super::prompts::{
    allium_instruction, build_prompt, build_quick_dispatch_prompt, epic_preamble,
    mcp_tools_instruction, reused_rebase_preamble, spec_first_instruction, task_block,
    tdd_instruction, wrap_up_instruction, EpicContext, LearningInjections, PromptContext,
};
use super::worktree::{
    provision_worktree, BaseRef, StartPoint, FETCH_MAX_ATTEMPTS, PROVISION_MAX_SUBPROCESS_CALLS,
};
use super::*;
use crate::models::test_tmux_window;

use crate::models::{EpicId, Task, TaskId};
use crate::process::{AgentBinaries, MockProcessRunner, SUBPROCESS_TIMEOUT};
use crate::tmux;
use std::time::Duration;

// -----------------------------------------------------------------------
// Shared helper tests
// -----------------------------------------------------------------------

#[test]
fn task_block_contains_id_title_description() {
    let block = task_block(TaskId(5), "My title", "My description", None);
    assert!(block.contains("5"));
    assert!(block.contains("My title"));
    assert!(block.contains("My description"));
}

#[test]
fn task_block_includes_epic_section_when_present() {
    let ctx = EpicContext {
        epic_id: EpicId(3),
        epic_title: "Big Epic".to_string(),
    };
    let block = task_block(TaskId(1), "T", "D", Some(&ctx));
    assert!(block.contains("EpicId: 3"));
    assert!(block.contains("Big Epic"));
}

#[test]
fn tdd_instruction_mentions_tests_first() {
    let instr = tdd_instruction();
    assert!(instr.contains("tests first") || instr.contains("behaviour as tests"));
}

#[test]
fn mcp_tools_instruction_mentions_get_and_update() {
    let instr = mcp_tools_instruction();
    assert!(instr.contains("get_task"));
    assert!(instr.contains("update_task"));
}

#[test]
fn spec_first_instruction_mentions_docs_plans_and_update_task() {
    let instr = spec_first_instruction();
    assert!(instr.contains("docs/plans/"));
    assert!(instr.contains("update_task"));
}

#[test]
fn wrap_up_instruction_mentions_wrap_up_skill() {
    let instr = wrap_up_instruction();
    assert!(instr.contains("/wrap-up"));
}

#[test]
fn allium_instruction_mentions_spec_and_skills() {
    let instr = allium_instruction();
    assert!(instr.contains("docs/specs/"));
    assert!(instr.contains("allium:tend"));
    assert!(instr.contains("allium:weed"));
}

pub(super) fn make_task(repo_path: &str) -> Task {
    Task {
        id: TaskId(42),
        title: "Fix bug".to_string(),
        description: "A nasty crash".to_string(),
        repo_path: repo_path.to_string(),
        ..Default::default()
    }
}

/// A `git worktree remove` call, if the mock recorded one.
fn worktree_remove_call(calls: &[(String, Vec<String>)]) -> Option<&(String, Vec<String>)> {
    calls.iter().find(|(prog, args)| {
        prog == "git"
            && args.contains(&"worktree".to_string())
            && args.contains(&"remove".to_string())
    })
}

fn find_call_arg(calls: &[(String, Vec<String>)], call_idx: usize, pattern: &str) -> String {
    calls[call_idx]
        .1
        .iter()
        .find(|a| a.contains(pattern))
        .unwrap_or_else(|| panic!("call {call_idx} missing arg matching {pattern:?}"))
        .clone()
}

fn make_test_repo() -> (tempfile::TempDir, String) {
    let dir = tempfile::TempDir::new().unwrap();
    let path = dir.path().to_str().unwrap().to_string();
    (dir, path)
}

/// A temp repo with `.worktrees/<slug>` already created, which is what puts
/// `provision_worktree` on its reuse branch.
///
/// `pub(crate)` rather than `pub(super)`: the service-layer dispatch-seam tests
/// need the same precondition, and a second copy of it would mean the day
/// `.worktrees/` moves, one of the two silently starts exercising the
/// fresh-worktree branch instead.
pub(crate) fn make_test_repo_with_worktree(
    slug: &str,
) -> (tempfile::TempDir, String, std::path::PathBuf) {
    let (dir, repo_path) = make_test_repo();
    let worktree_dir = dir.path().join(".worktrees").join(slug);
    std::fs::create_dir_all(&worktree_dir).unwrap();
    (dir, repo_path, worktree_dir)
}

/// Turn `<base>/.worktrees/<slug>` into a real LINKED worktree: the directory,
/// its git admin directory, and the `gitdir:` pointer git writes between them.
/// Returns the worktree path and its admin directory.
///
/// `pub(crate)` for the same reason as [`make_test_repo_with_worktree`] above —
/// `dispatch::caller_identity`'s own tests need this precondition, and a second
/// copy is how the two would drift the day the pointer's shape changes.
pub(crate) fn make_linked_worktree(
    base: &std::path::Path,
    slug: &str,
) -> (String, std::path::PathBuf) {
    let worktree = base.join(".worktrees").join(slug);
    std::fs::create_dir_all(&worktree).unwrap();
    let admin = base.join(".git").join("worktrees").join(slug);
    std::fs::create_dir_all(&admin).unwrap();
    std::fs::write(
        worktree.join(".git"),
        format!("gitdir: {}\n", admin.display()),
    )
    .unwrap();
    (worktree.to_string_lossy().into_owned(), admin)
}

/// A `~/.claude.json` holding exactly the entry `dispatch setup` writes.
pub(crate) fn claude_json_with_dispatch_entry(dir: &std::path::Path) -> std::path::PathBuf {
    let path = dir.join("claude.json");
    std::fs::write(
        &path,
        serde_json::to_string(&crate::setup::merge_mcp_config(None, crate::DEFAULT_PORT).value)
            .unwrap(),
    )
    .unwrap();
    path
}

#[test]
fn find_call_arg_returns_matching_arg() {
    let calls = vec![
        (
            "git".to_string(),
            vec!["worktree".to_string(), "add".to_string()],
        ),
        (
            "tmux".to_string(),
            vec!["new-window".to_string(), "-d".to_string()],
        ),
    ];
    let arg = find_call_arg(&calls, 1, "new-window");
    assert_eq!(arg, "new-window");
}

#[test]
#[should_panic(expected = "call 0 missing arg matching \"nonexistent\"")]
fn find_call_arg_panics_with_message_on_missing() {
    let calls = vec![("git".to_string(), vec!["status".to_string()])];
    find_call_arg(&calls, 0, "nonexistent");
}

#[test]
fn make_test_repo_returns_live_directory() {
    let (dir, repo_path) = make_test_repo();
    assert!(dir.path().exists());
    assert_eq!(repo_path, dir.path().to_str().unwrap());
}

#[test]
fn make_test_repo_with_worktree_creates_directory() {
    let (dir, _repo_path, worktree_dir) = make_test_repo_with_worktree("42-fix-bug");
    assert!(worktree_dir.exists());
    assert_eq!(
        worktree_dir,
        dir.path().join(".worktrees").join("42-fix-bug")
    );
}

#[test]
fn resolve_repo_path_matches_directory_name() {
    let paths = vec![
        "/home/user/projects/frontend".to_string(),
        "/home/user/projects/backend".to_string(),
    ];
    assert_eq!(
        resolve_repo_path("org/backend", &paths),
        Some("/home/user/projects/backend".to_string()),
    );
}

#[test]
fn resolve_repo_path_returns_none_when_no_match() {
    let paths = vec!["/home/user/projects/frontend".to_string()];
    assert_eq!(resolve_repo_path("org/backend", &paths), None);
}

#[test]
fn resolve_repo_path_handles_empty_paths() {
    assert_eq!(resolve_repo_path("org/repo", &[]), None);
}

#[test]
fn build_prompt_contains_task_info() {
    let prompt = build_prompt(
        TaskId(42),
        "Fix bug",
        "A nasty crash",
        None,
        None,
        &PromptContext::default(),
    );
    assert!(prompt.contains("42"));
    assert!(prompt.contains("Fix bug"));
    assert!(prompt.contains("A nasty crash"));
    assert!(prompt.contains("TDD"));
}

#[test]
fn build_prompt_mentions_tdd() {
    let prompt = build_prompt(
        TaskId(7),
        "Title",
        "Desc",
        None,
        None,
        &PromptContext::default(),
    );
    assert!(prompt.contains("TDD"));
    assert!(prompt.contains("behaviour as tests first"));
}

#[test]
fn build_prompt_mentions_wrap_up_skill() {
    let prompt = build_prompt(
        TaskId(7),
        "Title",
        "Desc",
        Some("docs/plans/p.md"),
        None,
        &PromptContext::default(),
    );
    assert!(
        prompt.contains("/wrap-up"),
        "with-plan prompt should tell agent to use /wrap-up skill"
    );
    assert!(
        prompt.contains("finalise the task"),
        "with-plan prompt should use the universal wrap-up wording"
    );
}

#[test]
fn build_prompt_without_plan_includes_wrap_up_universally() {
    // wrap_up_instruction is universal across every dispatched-agent prompt
    // — no-plan agents must implement the plan they attach before calling
    // /wrap-up, and need the same finalise step (commit/finalise) as any
    // other implementing agent.
    let prompt = build_prompt(
        TaskId(7),
        "Title",
        "Desc",
        None,
        None,
        &PromptContext::default(),
    );
    assert!(
        prompt.contains("/wrap-up"),
        "no-plan prompt should mention /wrap-up (universal, reached after implementing)"
    );
}

#[test]
fn build_prompt_without_plan_says_where_an_optional_plan_goes() {
    let prompt = build_prompt(
        TaskId(1),
        "Task",
        "Desc",
        None,
        None,
        &PromptContext::default(),
    );
    assert!(
        prompt.contains("docs/plans/"),
        "no-plan prompt should say where a plan goes if the agent writes one"
    );
    assert!(
        prompt.contains("update_task"),
        "no-plan prompt should say how to attach that plan via MCP"
    );
}

#[test]
fn build_prompt_without_plan_sends_the_agent_to_elicit_a_spec() {
    let prompt = build_prompt(
        TaskId(1),
        "Task",
        "Desc",
        None,
        None,
        &PromptContext::default(),
    );
    assert!(
        prompt.contains("allium:elicit"),
        "no-plan prompt should name the interview skill"
    );
    assert!(
        prompt.contains("docs/specs/"),
        "no-plan prompt should name the Allium spec as the design artefact"
    );
}

/// The design step no longer *requires* a plan doc — it is offered as an
/// option, conditional on the size of the implementation.
#[test]
fn build_prompt_without_plan_makes_the_plan_doc_optional() {
    let prompt = build_prompt(
        TaskId(1),
        "Task",
        "Desc",
        None,
        None,
        &PromptContext::default(),
    );
    assert!(
        prompt.contains("only if the implementation is big enough"),
        "no-plan prompt should condition the plan doc on implementation size"
    );
    assert!(
        !prompt.contains("implementation plan directly"),
        "no-plan prompt should no longer instruct writing a plan as the design output"
    );
}

#[test]
fn build_prompt_with_plan_asks_permission_before_implementing() {
    let prompt = build_prompt(
        TaskId(1),
        "Task",
        "Desc",
        Some("docs/plans/plan.md"),
        None,
        &PromptContext::default(),
    );
    assert!(prompt.contains("docs/plans/plan.md"));
    assert!(
        prompt.contains("Shall I proceed")
            || prompt.contains("permission")
            || prompt.contains("proceed"),
        "with-plan prompt should ask for permission before implementing"
    );
    assert!(
        !prompt.contains("step by step"),
        "with-plan prompt should not say 'Follow it step by step' — agent reviews first"
    );
}

#[test]
fn build_prompt_mentions_mcp_tools() {
    let prompt = build_prompt(
        TaskId(1),
        "Task",
        "Desc",
        None,
        None,
        &PromptContext::default(),
    );
    assert!(
        prompt.contains("dispatch MCP tools"),
        "standard dispatch prompt should mention MCP tools"
    );
}

#[test]
fn validate_repo_path_existing_dir() {
    assert!(validate_repo_path("/tmp").is_ok());
}

#[test]
fn validate_repo_path_nonexistent() {
    let result = validate_repo_path("/nonexistent/path");
    assert!(result.is_err());
    assert!(result.unwrap_err().contains("does not exist"));
}

#[test]
fn validate_repo_path_not_a_dir() {
    let result = validate_repo_path("/etc/hostname");
    assert!(result.is_err());
    assert!(result.unwrap_err().contains("Not a directory"));
}

// -----------------------------------------------------------------------
// Companion pane identity (docs/specs/agent-tree.allium: HideAgentTreePane)
// -----------------------------------------------------------------------

/// The regression the active/inactive heuristic was replaced for: with focus in
/// the companion pane, "the window's single inactive pane" is the *agent's*, so
/// the toggle killed the user's live claude session.
#[test]
fn toggle_kills_the_tree_pane_even_when_the_tree_pane_is_active() {
    let mock = MockProcessRunner::new(vec![
        // pane_ids_with_option_value: %2 carries the agent-tree role, and it is
        // the active pane — which the listing does not even report, because
        // identity no longer depends on it.
        MockProcessRunner::ok_with_stdout(b"%1 \n%2 agent_tree\n"),
        MockProcessRunner::ok(), // kill-pane
    ])
    .with_windows(&["task-42"]);

    toggle_agent_tree_pane(&test_tmux_window("task-42"), &mock).unwrap();

    let calls = mock.recorded_calls();
    assert_eq!(calls[1].1, vec!["kill-pane", "-t", "%2"]);
}

/// Hiding takes the diff pane with the tree. A diff pane outliving its tree is
/// orphaned: nothing drives its open set, nothing refreshes it, and this toggle
/// does not act on it — see KillAgentTreeDiffPaneWithItsTree in
/// docs/specs/agent-tree.allium.
#[test]
fn toggle_with_a_diff_pane_open_kills_both_panes() {
    let mock = MockProcessRunner::new(vec![
        MockProcessRunner::ok_with_stdout(b"%1 \n%2 agent_tree\n%3 diff\n"),
        MockProcessRunner::ok(), // kill-pane: the diff pane
        MockProcessRunner::ok_with_stdout(b"/wt\n"), // show-options @dispatch_dir
        MockProcessRunner::ok(), // kill-pane: the tree
    ])
    .with_windows(&["task-42"]);

    toggle_agent_tree_pane(&test_tmux_window("task-42"), &mock).unwrap();

    let calls = mock.recorded_calls();
    let killed: Vec<&str> = calls
        .iter()
        .filter(|(_, args)| args[0] == "kill-pane")
        .map(|(_, args)| args[2].as_str())
        .collect();
    // The diff pane first, so it cannot briefly outlive its tree.
    assert_eq!(killed, vec!["%3", "%2"]);
}

/// The agent's own pane carries no role, so it is never touched however many
/// panes dispatch has put beside it. Presence-matching rather than
/// value-matching would have killed it here.
#[test]
fn toggle_never_kills_a_pane_dispatch_did_not_create() {
    let mock = MockProcessRunner::new(vec![
        MockProcessRunner::ok_with_stdout(b"%1 \n%2 agent_tree\n"),
        MockProcessRunner::ok(), // kill-pane
    ])
    .with_windows(&["task-42"]);

    toggle_agent_tree_pane(&test_tmux_window("task-42"), &mock).unwrap();

    let calls = mock.recorded_calls();
    assert!(
        !calls
            .iter()
            .any(|(_, args)| args.contains(&"%1".to_string())),
        "the agent's own pane must be untouched; calls: {calls:?}"
    );
}

/// With nothing open there is no set to clear, so the toggle must not pay a
/// `show-options` round-trip resolving the worktree. This is the common case.
#[test]
fn toggle_with_no_diff_pane_costs_only_a_lookup_and_a_kill() {
    let mock = MockProcessRunner::new(vec![
        MockProcessRunner::ok_with_stdout(b"%1 \n%2 agent_tree\n"),
        MockProcessRunner::ok(), // kill-pane
    ])
    .with_windows(&["task-42"]);

    toggle_agent_tree_pane(&test_tmux_window("task-42"), &mock).unwrap();

    assert_eq!(mock.recorded_calls().len(), 2);
}

/// A pane dispatch did not create carries no role, whatever it is running — so
/// the toggle re-splits rather than killing it. The lookup this replaced had to
/// defend against the *contents* of such a pane's command line: an editor opened
/// on docs/specs/agent-tree.allium matched a substring test.
#[test]
fn toggle_ignores_an_unmarked_pane_and_splits_a_tree_pane() {
    let mock = MockProcessRunner::new(vec![
        MockProcessRunner::ok_with_stdout(b"%1 \n%3 \n"),
        MockProcessRunner::ok_with_stdout(b"/wt\n"), // show-options @dispatch_dir
        MockProcessRunner::ok_with_stdout(b"%4\n"),  // split-window
        MockProcessRunner::ok(),                     // set-option
    ])
    .with_windows(&["task-42"]);

    toggle_agent_tree_pane(&test_tmux_window("task-42"), &mock).unwrap();

    let calls = mock.recorded_calls();
    assert_eq!(calls[2].1[0], "split-window", "calls: {calls:?}");
    assert!(
        !calls.iter().any(|(_, args)| args[0] == "kill-pane"),
        "must not kill a pane it did not create; calls: {calls:?}"
    );
}

#[test]
fn toggle_splits_a_tree_pane_when_the_window_has_none() {
    let mock = MockProcessRunner::new(vec![
        MockProcessRunner::ok_with_stdout(b"%1 \n"),
        MockProcessRunner::ok_with_stdout(b"/wt\n"), // show-options @dispatch_dir
        MockProcessRunner::ok_with_stdout(b"%2\n"),  // split-window
        MockProcessRunner::ok(),                     // set-option
    ])
    .with_windows(&["task-42"]);

    toggle_agent_tree_pane(&test_tmux_window("task-42"), &mock).unwrap();

    assert_eq!(mock.recorded_calls()[2].1[0], "split-window");
}

/// The spawn side writes the marker the lookup side reads — the whole mechanism
/// is these two halves agreeing, so the `set-option` is asserted verbatim.
#[test]
fn the_spawned_tree_pane_is_marked_with_its_role() {
    let mock = MockProcessRunner::new(vec![
        MockProcessRunner::ok_with_stdout(b"%1 \n"),
        MockProcessRunner::ok_with_stdout(b"/wt\n"), // show-options @dispatch_dir
        MockProcessRunner::ok_with_stdout(b"%2\n"),  // split-window returns the new pane
        MockProcessRunner::ok(),                     // set-option
    ])
    .with_windows(&["task-42"]);

    toggle_agent_tree_pane(&test_tmux_window("task-42"), &mock).unwrap();

    let calls = mock.recorded_calls();
    assert_eq!(
        calls[3].1,
        vec![
            "set-option",
            "-p",
            "-t",
            "%2",
            tmux::PANE_ROLE_OPTION,
            tmux::PANE_ROLE_AGENT_TREE
        ],
        "calls: {calls:?}"
    );
}

/// The marker matters to the *next* toggle, not this one: the pane is already
/// open and rendering, so a failed write is logged rather than reported as a
/// failed toggle. Same accepted gap as the editor pane's own marker
/// (docs/specs/agent-tree.allium: OneEditorPanePerAgentWindow).
#[test]
fn a_failing_role_marker_write_does_not_fail_the_toggle() {
    let mock = MockProcessRunner::new(vec![
        MockProcessRunner::ok_with_stdout(b"%1 \n"),
        MockProcessRunner::ok_with_stdout(b"/wt\n"), // show-options @dispatch_dir
        MockProcessRunner::ok_with_stdout(b"%2\n"),  // split-window
        MockProcessRunner::fail("bad option"),       // set-option: the marker write
    ])
    .with_windows(&["task-42"]);

    assert!(toggle_agent_tree_pane(&test_tmux_window("task-42"), &mock).is_ok());
    // The failure must land on the marker write, not on an earlier call — this
    // test is worthless if the split is what failed.
    let calls = mock.recorded_calls();
    assert_eq!(calls[3].1[0], "set-option", "calls: {calls:?}");
}

#[test]
fn toggle_is_a_no_op_for_a_window_that_is_not_a_task_window() {
    let mock = MockProcessRunner::new(vec![]).with_queued_window_lookup();
    toggle_agent_tree_pane(&test_tmux_window("TUI"), &mock).unwrap();
    assert!(mock.recorded_calls().is_empty());
}

/// Pinning moves only the agent's own pane out, so *every* pane dispatch put in
/// that window has to go with it — with an editor pane open the old
/// single-inactive-pane lookup was ambiguous and orphaned both.
#[test]
fn companion_pane_ids_returns_both_the_tree_and_the_editor_pane() {
    let mock = MockProcessRunner::new(vec![MockProcessRunner::ok_with_stdout(
        b"%1 \n%2 agent_tree\n%3 editor\n",
    )])
    .with_windows(&["task-42"]);

    let found = companion_pane_ids(&test_tmux_window("task-42"), &mock).unwrap();

    assert_eq!(found, vec!["%2".to_string(), "%3".to_string()]);
}

/// One lookup, not one per role: "a pane dispatch created" is a single question
/// about the marker's presence, and asking it per role is what the three-call
/// version did — a cost paid on every pin, and a place for a future role to be
/// forgotten.
#[test]
fn companion_pane_ids_asks_tmux_once() {
    let mock = MockProcessRunner::new(vec![MockProcessRunner::ok_with_stdout(
        b"%1 \n%2 agent_tree\n%3 editor\n",
    )])
    .with_windows(&["task-42"]);

    companion_pane_ids(&test_tmux_window("task-42"), &mock).unwrap();

    let calls = mock.recorded_calls();
    assert_eq!(calls.len(), 1, "calls: {calls:?}");
    assert_eq!(calls[0].1[0], "list-panes");
}

#[test]
fn companion_pane_ids_is_empty_for_a_window_with_only_an_agent_pane() {
    let mock = MockProcessRunner::new(vec![MockProcessRunner::ok_with_stdout(b"%1 \n")])
        .with_windows(&["task-42"]);

    assert!(companion_pane_ids(&test_tmux_window("task-42"), &mock)
        .unwrap()
        .is_empty());
}

#[test]
fn build_prompt_includes_plan_path() {
    let prompt = build_prompt(
        TaskId(1),
        "Task",
        "Desc",
        Some("docs/plans/my-plan.md"),
        None,
        &PromptContext::default(),
    );
    assert!(prompt.contains("Plan: docs/plans/my-plan.md"));
}

#[test]
fn build_prompt_without_plan_omits_plan_section() {
    let prompt = build_prompt(
        TaskId(1),
        "Task",
        "Desc",
        None,
        None,
        &PromptContext::default(),
    );
    assert!(!prompt.contains("Plan:"));
}

#[test]
fn build_quick_dispatch_prompt_includes_the_spec_first_sequence() {
    let prompt = build_quick_dispatch_prompt(
        TaskId(42),
        "Quick task",
        "",
        None,
        &PromptContext::default(),
    );
    assert!(
        prompt.contains("allium:elicit") && prompt.contains("docs/specs/"),
        "quick dispatch prompt should send the agent through the spec-first design step"
    );
    assert!(
        prompt.contains("asking the user"),
        "quick dispatch prompt should still open by asking the user what they want"
    );
}

#[test]
fn build_quick_dispatch_prompt_contains_rename_instruction() {
    let prompt = build_quick_dispatch_prompt(
        TaskId(42),
        "Quick task",
        "",
        None,
        &PromptContext::default(),
    );
    assert!(prompt.contains("42"));
    assert!(prompt.contains("Quick task"));
    assert!(prompt.contains("update_task"));
    assert!(prompt.contains("title"));
    assert!(prompt.contains("placeholder"));
}

#[test]
fn build_quick_dispatch_prompt_mentions_mcp() {
    let prompt =
        build_quick_dispatch_prompt(TaskId(1), "Quick task", "", None, &PromptContext::default());
    assert!(prompt.contains("dispatch MCP tools"));
    assert!(prompt.contains("update_task"));
    assert!(!prompt.contains("add_note"));
}

#[test]
fn build_quick_dispatch_prompt_differs_from_regular() {
    let regular = build_prompt(
        TaskId(1),
        "Task",
        "Desc",
        None,
        None,
        &PromptContext::default(),
    );
    let quick =
        build_quick_dispatch_prompt(TaskId(1), "Task", "Desc", None, &PromptContext::default());
    assert!(quick.contains("placeholder"));
    assert!(!regular.contains("placeholder"));
}

#[test]
fn build_quick_dispatch_prompt_includes_epic_context() {
    let ctx = EpicContext {
        epic_id: EpicId(7),
        epic_title: "My Epic".to_string(),
    };
    let prompt = build_quick_dispatch_prompt(
        TaskId(42),
        "Quick task",
        "",
        Some(&ctx),
        &PromptContext::default(),
    );
    assert!(prompt.contains("EpicId: 7"), "should include epic ID");
    assert!(prompt.contains("My Epic"), "should include epic title");
    assert!(
        prompt.contains("SendMessage"),
        "should tell agent how to message sibling agents"
    );
}

#[test]
fn no_plan_prompts_reference_the_elicit_skill() {
    let standard = build_prompt(TaskId(1), "T", "D", None, None, &PromptContext::default());
    let quick = build_quick_dispatch_prompt(TaskId(1), "T", "D", None, &PromptContext::default());

    for (name, prompt) in [("standard-no-plan", standard), ("quick", quick)] {
        assert!(
            prompt.contains("allium:elicit"),
            "{name} prompt should reference the allium:elicit skill"
        );
        assert!(
            !prompt.contains("/brainstorming"),
            "{name} prompt should no longer reference the retired /brainstorming skill"
        );
    }
}

const SHARED_TRAILING_LINES: &[&str] = &[
    "TDD",                           // tdd_instruction
    "Allium specs in `docs/specs/`", // allium_instruction
    "dispatch MCP tools",            // mcp_tools_instruction
    "/wrap-up",                      // wrap_up_instruction (universal)
];

fn all_aligned_prompts() -> [(&'static str, String); 3] {
    [
        (
            "standard-no-plan",
            build_prompt(
                TaskId(1),
                "Task",
                "Desc",
                None,
                None,
                &PromptContext::default(),
            ),
        ),
        (
            "standard-with-plan",
            build_prompt(
                TaskId(1),
                "Task",
                "Desc",
                Some("docs/plans/p.md"),
                None,
                &PromptContext::default(),
            ),
        ),
        (
            "quick-dispatch",
            build_quick_dispatch_prompt(
                TaskId(1),
                "Quick task",
                "",
                None,
                &PromptContext::default(),
            ),
        ),
    ]
}

#[test]
fn every_prompt_includes_shared_trailing_metadata() {
    for (name, prompt) in all_aligned_prompts() {
        for needle in SHARED_TRAILING_LINES {
            assert!(
                prompt.contains(needle),
                "{name} prompt missing shared trailing line: {needle}\n--- prompt ---\n{prompt}"
            );
        }
    }
}

#[test]
fn every_prompt_uses_task_block_format() {
    for (name, prompt) in all_aligned_prompts() {
        assert!(
            prompt.contains("Task:"),
            "{name} prompt should open task block with `Task:` (no `Epic:` header)\n{prompt}"
        );
        assert!(prompt.contains("ID:"), "{name} prompt should have `ID:`");
        assert!(
            prompt.contains("Title:"),
            "{name} prompt should have `Title:`"
        );
        assert!(
            prompt.contains("Description:"),
            "{name} prompt should have `Description:`"
        );
    }
}

/// Quick dispatch and the no-plan variant share one design instruction, so the
/// design step cannot drift apart between them.
#[test]
fn quick_dispatch_embeds_the_shared_spec_first_instruction() {
    let prompt =
        build_quick_dispatch_prompt(TaskId(1), "Quick task", "", None, &PromptContext::default());
    assert!(
        prompt.contains(spec_first_instruction()),
        "quick-dispatch prompt should embed spec_first_instruction verbatim"
    );
    assert!(
        !prompt.contains("vague or"),
        "no prompt should retain the retired vague-vs-clear branch"
    );
}

#[test]
fn wrap_up_instruction_universal_wording() {
    let text = wrap_up_instruction();
    assert!(
        text.contains("/wrap-up"),
        "wrap_up_instruction should reference the /wrap-up skill"
    );
    assert!(
        text.contains("finishing implementation") || text.contains("creating work packages"),
        "wrap_up_instruction should describe the universal trigger (impl / work-packages), got: {text}"
    );
}

/// The instruction is a numbered sequence, so it is necessarily longer than the
/// one-liner it replaced — but it must stay a compact runbook, not an essay.
#[test]
fn spec_first_instruction_stays_compact() {
    let instruction = spec_first_instruction();
    assert!(
        instruction.len() < 1200,
        "spec_first_instruction should stay a compact runbook (< 1200 chars), got {} chars",
        instruction.len()
    );
    assert!(instruction.contains("allium:elicit"));
    assert!(instruction.contains("update_task"));
    assert!(instruction.contains("docs/plans/"));
}

#[test]
fn epic_preamble_returns_empty_strings_for_none() {
    let (id_line, section) = epic_preamble(None);
    assert!(id_line.is_empty());
    assert!(section.is_empty());
}

#[test]
fn epic_preamble_returns_id_line_and_section_for_some() {
    let ctx = EpicContext {
        epic_id: EpicId(5),
        epic_title: "Auth Rework".to_string(),
    };
    let (id_line, section) = epic_preamble(Some(&ctx));
    assert!(id_line.contains("EpicId: 5"));
    assert!(section.contains("Auth Rework"));
    assert!(
        section.contains("SendMessage") && section.contains("ListAgents"),
        "should guide agent to use native cross-session messaging, got: {section}"
    );
    assert!(
        section.contains("task-<id>") || section.contains("task-{"),
        "should name the task-<id> session-naming convention, got: {section}"
    );
    assert!(
        !section.contains("send_message"),
        "the dispatch send_message MCP tool no longer exists, got: {section}"
    );
    assert!(
        !section.contains("Sibling tasks:"),
        "should not enumerate sibling tasks"
    );
}

// --- ProcessRunner-based tests ---

#[test]
fn dispatch_reuses_existing_worktree() {
    // Pre-create worktree dir — simulates a re-dispatch where the worktree
    // already exists on disk from a previous dispatch cycle.
    let (_dir, repo_path, _worktree_dir) = make_test_repo_with_worktree("42-fix-bug");

    let script = DispatchScript::dispatch();
    let mock = script.runner();

    let task = make_task(&repo_path);
    dispatch_agent(&task, &mock, None, &LearningInjections::default()).unwrap();

    let calls = mock.recorded_calls();
    assert!(
        calls
            .iter()
            .all(|(prog, args)| !(prog == "git" && args.iter().any(|a| a == "worktree"))),
        "git worktree add should be skipped for existing worktree"
    );
    assert_eq!(calls[2].0, "tmux");
    assert_eq!(calls[2].1[0], "new-window");
    assert_eq!(calls[3].0, "tmux");
    assert_eq!(calls[3].1[0], "set-option");
    assert_eq!(calls[4].0, "tmux");
    assert_eq!(calls[4].1[0], "set-hook");
}

#[test]
fn dispatch_reused_worktree_prompt_carries_the_reuse_preamble() {
    let (_dir, repo_path, worktree_dir) = make_test_repo_with_worktree("42-fix-bug");

    let script = DispatchScript::dispatch();
    let mock = script.runner();

    let task = make_task(&repo_path);
    dispatch_agent(&task, &mock, None, &LearningInjections::default()).unwrap();

    let prompt = std::fs::read_to_string(worktree_dir.join(".claude-prompt")).unwrap();
    assert!(
        prompt.contains("reused from a previous attempt"),
        "got: {prompt}"
    );
    assert!(prompt.contains("git rebase origin/main"), "got: {prompt}");
    assert!(prompt.contains("Always work from this worktree folder"));
}

#[test]
fn dispatch_sends_claude_command() {
    let (_dir, repo_path, _worktree_dir) = make_test_repo_with_worktree("42-fix-bug");

    let script = DispatchScript::dispatch();
    let mock = script.runner();

    let task = make_task(&repo_path);
    dispatch_agent(&task, &mock, None, &LearningInjections::default()).unwrap();

    let calls = mock.recorded_calls();
    // The literal send-keys call (index 5) carries the claude invocation
    assert!(
        calls[5].1.iter().any(|a| a.contains("claude")),
        "send-keys should include claude"
    );
}

#[test]
fn dispatch_agent_splits_agent_tree_companion_pane_after_send_keys() {
    let (_dir, repo_path, _worktree_dir) = make_test_repo_with_worktree("42-fix-bug");

    let script = DispatchScript::dispatch();
    let mock = script.runner();

    let task = make_task(&repo_path);
    dispatch_agent(&task, &mock, None, &LearningInjections::default()).unwrap();

    let calls = mock.recorded_calls();
    // The split, not the last call: the role marker written on the pane it
    // returns follows it (Step::CompanionRoleMark).
    let split = &calls[script.index_of(Step::CompanionSplit)];
    assert_eq!(split.0, "tmux");
    assert_eq!(split.1[0], "split-window");
    assert!(
        split.1.iter().any(|a| a == "30%"),
        "companion pane should use the 30% size, got: {:?}",
        split.1
    );
    assert_eq!(
        split.1[split.1.len() - 3..],
        vec![
            "dispatch".to_string(),
            "agent-tree".to_string(),
            "42".to_string(),
        ],
        "companion pane should run `dispatch agent-tree <task_id>`, got: {:?}",
        split.1
    );
}

#[test]
fn dispatch_agent_succeeds_even_if_companion_pane_split_fails() {
    let (_dir, repo_path, _worktree_dir) = make_test_repo_with_worktree("42-fix-bug");

    let mock = DispatchScript::dispatch()
        .fails_at(Step::CompanionSplit)
        .runner();

    let task = make_task(&repo_path);
    let result = dispatch_agent(&task, &mock, None, &LearningInjections::default());
    assert!(
        result.is_ok(),
        "a failed companion-pane split must not fail dispatch: {result:?}"
    );
}

/// One row of the launcher table below: a name, and a thunk that drives that
/// launcher end to end and hands back the `claude` command it sent.
type LaunchCase = (&'static str, fn() -> String);

/// Drive one `dispatch_with_prompt` launcher through the standard mock script
/// and return the `claude` command it sent to tmux.
fn dispatched_claude_cmd(launch: impl FnOnce(&Task, &MockProcessRunner)) -> String {
    let (_dir, repo_path, _worktree_dir) = make_test_repo_with_worktree("42-fix-bug");
    let script = DispatchScript::dispatch();
    let mock = script.runner();
    let task = make_task(&repo_path);

    launch(&task, &mock);

    find_call_arg(
        &mock.recorded_calls(),
        script.index_of(Step::SendKeysLiteral),
        "claude",
    )
}

/// `EveryTaskAgentLaunchesInAutoMode` in `docs/specs/dispatch.allium`. The
/// guarantee is "no exceptions", so the assertion is made once, over every
/// launcher, rather than per-launcher: a variant that reintroduces a permission
/// flag fails here even if it ships with a passing test of its own. Adding a
/// launcher means adding a row.
#[test]
fn no_task_agent_passes_a_permission_mode_flag() {
    let launchers: [LaunchCase; 4] = [
        ("dispatch_agent", || {
            dispatched_claude_cmd(|task, mock| {
                dispatch_agent(task, mock, None, &LearningInjections::default()).unwrap();
            })
        }),
        ("research_agent", || {
            dispatched_claude_cmd(|task, mock| {
                research_agent(task, mock, None).unwrap();
            })
        }),
        ("quick_dispatch_agent", || {
            dispatched_claude_cmd(|task, mock| {
                quick_dispatch_agent(task, mock, None, &LearningInjections::default()).unwrap();
            })
        }),
        // Not a dispatch_with_prompt caller — resume_agent hand-builds its own
        // claude command, which is exactly why it belongs in this table: the
        // parameter removal cannot reach it, so only an assertion can.
        ("resume_agent", || {
            let (_dir, worktree_path) = make_test_repo();
            let script = DispatchScript::resume();
            let mock = script.runner();
            resume_agent(TaskId(42), &worktree_path, &mock).unwrap();
            find_call_arg(
                &mock.recorded_calls(),
                script.index_of(Step::SendKeysLiteral),
                "claude",
            )
        }),
    ];

    for (name, launch) in launchers {
        let claude_cmd = launch();
        assert!(
            !claude_cmd.contains("--permission-mode"),
            "{name} must launch in auto mode with no --permission-mode flag, got: {claude_cmd}"
        );
    }
}

// --- PR-based review worktree start point ---
//
// The mock runner doesn't actually create the worktree dir, so the post-provision
// `.claude-prompt` write fails for a fresh repo. Start-point tests use a fresh
// repo and inspect the recorded `git` calls (captured before the write fails) —
// they tolerate the resulting error. The prompt-content test pre-creates the
// worktree dir so the write succeeds.

pub(super) fn pr_review_task(repo_path: &str) -> Task {
    let mut task = make_task(repo_path);
    task.tag = Some(crate::models::TaskTag::PrReview);
    task.url = Some(crate::models::TaskUrl::new(
        "https://github.com/org/repo/pull/7",
        crate::models::UrlType::Pr,
    ));
    task
}

/// The `git worktree add` start point (its last arg), from the recorded calls.
fn worktree_add_start_point(calls: &[(String, Vec<String>)]) -> String {
    calls
        .iter()
        .find(|(prog, args)| prog == "git" && args.contains(&"worktree".to_string()))
        .expect("git worktree add call")
        .1
        .last()
        .expect("start point arg")
        .clone()
}

#[test]
fn dispatch_pr_review_task_bases_worktree_on_pr_head_branch() {
    let (_dir, repo_path) = make_test_repo();

    let mock = MockProcessRunner::new(vec![
        MockProcessRunner::ok_with_stdout(b"feature-x\nfalse\n"), // gh pr view
        MockProcessRunner::ok(),                                  // git fetch origin feature-x
        MockProcessRunner::ok(), // git worktree add origin/feature-x
        MockProcessRunner::ok(), // tmux new-window
        MockProcessRunner::ok(), // tmux set-option
        MockProcessRunner::ok(), // tmux set-hook
        MockProcessRunner::ok(), // tmux list-windows (rollback's window-kill check)
        MockProcessRunner::ok(), // git worktree remove --force (fresh-worktree rollback)
        MockProcessRunner::ok(), // git branch -D (fresh-worktree rollback)
    ]);

    let task = pr_review_task(&repo_path);
    // Prompt write fails (mock didn't create the worktree dir) — that's fine, the
    // git calls we assert on were recorded during provisioning beforehand. The
    // failure then rolls the fresh worktree back, hence the two trailing
    // responses above.
    let _ = dispatch_agent(&task, &mock, None, &LearningInjections::default());

    let calls = mock.recorded_calls();
    assert_eq!(
        calls[0].0, "gh",
        "first call should resolve the PR head branch"
    );
    assert_eq!(
        worktree_add_start_point(&calls),
        "origin/feature-x",
        "worktree should start from the PR head branch"
    );
}

#[test]
fn dispatch_pr_review_task_never_measures_the_pr_head_branch() {
    // End-to-end version of `provision_worktree_never_measures_a_pr_head_branch`:
    // dispatch_agent on a pr-review task with a PR URL must construct
    // `BaseRef::PrHead`, not `BaseRef::Branch`, so no ahead/behind comparison
    // (git rev-list) ever runs against the PR's head branch.
    let (_dir, repo_path) = make_test_repo();

    let mock = MockProcessRunner::new(vec![
        MockProcessRunner::ok_with_stdout(b"feature-x\nfalse\n"), // gh pr view
        MockProcessRunner::ok(),                                  // git fetch origin feature-x
        MockProcessRunner::ok(),                                  // git worktree add
        MockProcessRunner::ok(),                                  // tmux new-window
        MockProcessRunner::ok(),                                  // tmux set-option
        MockProcessRunner::ok(),                                  // tmux set-hook
        MockProcessRunner::ok(), // tmux list-windows (rollback's window-kill check)
        MockProcessRunner::ok(), // git worktree remove --force (fresh-worktree rollback)
        MockProcessRunner::ok(), // git branch -D (fresh-worktree rollback)
    ]);

    let task = pr_review_task(&repo_path);
    // Prompt write fails (mock didn't create the worktree dir) — that's fine, the
    // calls we assert on were recorded during provisioning beforehand. The
    // failure then rolls the fresh worktree back, hence the two trailing
    // responses above.
    let _ = dispatch_agent(&task, &mock, None, &LearningInjections::default());

    let calls = mock.recorded_calls();
    assert!(
        !calls
            .iter()
            .any(|(_, args)| args.contains(&"rev-list".to_string())),
        "a PR-review dispatch must never compare the PR head branch against a local ref: {calls:?}"
    );
}

#[test]
fn dispatch_pr_review_task_prompt_rebases_onto_pr_branch() {
    let (_dir, repo_path, _worktree_dir) = make_test_repo_with_worktree("42-fix-bug");

    // A PR head base is never measured, so the script declares no rev-list —
    // the entry this vector used to carry was stale, silently absorbed by the
    // next call because new-window ignores stdout.
    let mock = DispatchScript::dispatch()
        .pr_head(PrHead::Branch("feature-x"))
        .runner();

    let task = pr_review_task(&repo_path);
    dispatch_agent(&task, &mock, None, &LearningInjections::default()).unwrap();

    let prompt =
        std::fs::read_to_string(format!("{repo_path}/.worktrees/42-fix-bug/.claude-prompt"))
            .expect("prompt file written");
    assert!(
        prompt.contains("git rebase origin/feature-x"),
        "prompt should rebase onto the PR branch, got: {prompt}"
    );
    assert!(
        !prompt.contains("git rebase main"),
        "PR review prompt must not rebase onto the base branch, got: {prompt}"
    );
}

#[test]
fn dispatch_prompt_includes_fetch_warning_when_fetch_fails() {
    // Pre-create the worktree dir so `git worktree add` is skipped (its
    // mocked response would otherwise not actually create the directory the
    // real implementation later writes `.claude-prompt` into).
    //
    // That pre-creation also puts this test on the REUSE path, so the fetch is
    // best-effort: one attempt, no `remote get-url`/`ls-remote` classification
    // probe, and a failure that warns instead of aborting. What is under test
    // either way is the threading — a fetch warning must reach the agent's own
    // prompt as a `Note:`, not just a server-side log line.
    let (_dir, repo_path, _worktree_dir) = make_test_repo_with_worktree("42-fix-bug");

    // The reuse path is best-effort, so the script declares one attempt and no
    // classification probes — the budget the sibling test defends.
    let mock = DispatchScript::dispatch().fetch_is_unreachable().runner();

    let task = make_task(&repo_path);
    dispatch_agent(&task, &mock, None, &LearningInjections::default()).unwrap();

    let prompt =
        std::fs::read_to_string(format!("{repo_path}/.worktrees/42-fix-bug/.claude-prompt"))
            .expect("prompt file written");
    assert!(
        prompt.contains("origin/main"),
        "prompt should mention the base branch that could not be fetched, got: {prompt}"
    );
    assert!(
        prompt.contains("Note:"),
        "fetch warning should be a clearly-marked note, got: {prompt}"
    );
}

#[test]
fn dispatch_prompt_has_no_warning_when_fetch_succeeds() {
    // Pre-create the worktree dir — see comment in the sibling test above.
    let (_dir, repo_path, _worktree_dir) = make_test_repo_with_worktree("42-fix-bug");

    let script = DispatchScript::dispatch();
    let mock = script.runner();

    let task = make_task(&repo_path);
    dispatch_agent(&task, &mock, None, &LearningInjections::default()).unwrap();

    let prompt =
        std::fs::read_to_string(format!("{repo_path}/.worktrees/42-fix-bug/.claude-prompt"))
            .expect("prompt file written");
    assert!(
        !prompt.contains("Note:"),
        "no fetch warning expected when fetch succeeds, got: {prompt}"
    );
}

#[test]
fn dispatch_non_review_task_skips_gh_and_bases_worktree_on_origin() {
    let (_dir, repo_path) = make_test_repo();

    let script = DispatchScript::dispatch().fresh_worktree();
    let mock = script.runner();

    // make_task has tag None / url None — a plain implementation task.
    let task = make_task(&repo_path);
    let _ = dispatch_agent(&task, &mock, None, &LearningInjections::default());

    let calls = mock.recorded_calls();
    assert!(
        calls.iter().all(|(prog, _)| prog != "gh"),
        "non-review task must not call gh"
    );
    assert_eq!(worktree_add_start_point(&calls), "origin/main");
}

#[test]
fn dispatch_review_task_pr_resolution_failure_falls_back_to_base() {
    let (_dir, repo_path) = make_test_repo();

    // An unresolvable PR leaves the dispatch on `BaseRef::Branch`, so the
    // ahead/behind measurement *does* run — unlike the PrHead::Branch shapes.
    let script = DispatchScript::dispatch()
        .pr_head(PrHead::Unresolvable)
        .fresh_worktree();
    let mock = script.runner();

    let task = pr_review_task(&repo_path);
    let _ = dispatch_agent(&task, &mock, None, &LearningInjections::default());

    assert_eq!(
        worktree_add_start_point(&mock.recorded_calls()),
        "origin/main",
        "should fall back to the base branch when PR resolution fails"
    );
}

#[test]
fn dispatch_review_task_fork_pr_falls_back_to_base() {
    let (_dir, repo_path) = make_test_repo();

    let script = DispatchScript::dispatch()
        .pr_head(PrHead::Fork("patch-1"))
        .fresh_worktree();
    let mock = script.runner();

    let task = pr_review_task(&repo_path);
    let _ = dispatch_agent(&task, &mock, None, &LearningInjections::default());

    assert_eq!(
        worktree_add_start_point(&mock.recorded_calls()),
        "origin/main",
        "fork PR should fall back to the base branch"
    );
}

#[test]
fn provision_worktree_never_measures_a_pr_head_branch() {
    let (_dir, repo_path) = make_test_repo();

    let mock = MockProcessRunner::new(vec![
        MockProcessRunner::ok(), // git fetch origin feature-x
        MockProcessRunner::ok(), // git worktree add origin/feature-x
        MockProcessRunner::ok(), // tmux new-window
        MockProcessRunner::ok(), // tmux set-option
        MockProcessRunner::ok(), // tmux set-hook
    ]);

    let task = make_task(&repo_path);
    let result = provision_worktree(
        &task,
        &mock,
        Some(BaseRef::PrHead("feature-x")),
        SUBPROCESS_TIMEOUT,
    )
    .unwrap();

    let calls = mock.recorded_calls();
    assert!(
        !calls
            .iter()
            .any(|(_, args)| args.contains(&"rev-list".to_string())),
        "a PR head branch must never be compared against a local ref: {calls:?}"
    );
    assert_eq!(
        result.start_point,
        Some(StartPoint::Remote {
            base: "feature-x".to_string()
        })
    );
}

#[test]
fn provision_worktree_creates_new_when_dir_missing() {
    let (_dir, repo_path) = make_test_repo();
    // Do NOT pre-create the worktree dir — test the "create" path

    let mock = MockProcessRunner::new(vec![
        MockProcessRunner::ok(), // git worktree add
        MockProcessRunner::ok(), // tmux new-window
        MockProcessRunner::ok(), // tmux set-option @dispatch_dir
        MockProcessRunner::ok(), // tmux set-hook (after-split-window)
    ]);

    let task = make_task(&repo_path);
    let result = provision_worktree(&task, &mock, None, SUBPROCESS_TIMEOUT).unwrap();

    let calls = mock.recorded_calls();
    assert_eq!(calls[0].0, "git", "first call should be git worktree add");
    assert!(calls[0].1.contains(&"worktree".to_string()));
    assert!(calls[0].1.contains(&"add".to_string()));
    assert_eq!(calls[1].0, "tmux");
    assert_eq!(calls[1].1[0], "new-window");

    let expected_path = format!("{repo_path}/.worktrees/42-fix-bug");
    assert_eq!(result.worktree_path, expected_path);
}

/// The premise `WorktreeIsNeverShared` rests on: the derived path is injective
/// in the task id, so two tasks can never name the same worktree.
///
/// This is the load-bearing assertion for that invariant in
/// docs/specs/tasks.allium. The surviving tripwire elsewhere
/// (`src/runtime/tests/task_exec.rs::exec_cleanup_tears_down_even_if_another_row_names_the_worktree`)
/// only asserts that consumers do not *check* for sharing; it would stay
/// green if the id prefix were dropped here. This one fails instead — pick the
/// worst case, two tasks whose titles slugify identically, so the id is the only
/// thing keeping the paths apart.
#[test]
fn provision_worktree_path_is_unique_per_task_id_even_for_identical_titles() {
    let (_dir, repo_path) = make_test_repo();

    let derive = |id: i64| {
        let mock = MockProcessRunner::new(vec![
            MockProcessRunner::ok(), // git worktree add
            MockProcessRunner::ok(), // tmux new-window
            MockProcessRunner::ok(), // tmux set-option @dispatch_dir
            MockProcessRunner::ok(), // tmux set-hook (after-split-window)
        ]);
        let mut task = make_task(&repo_path);
        task.id = TaskId(id);
        task.title = "Same title".to_string();
        provision_worktree(&task, &mock, None, SUBPROCESS_TIMEOUT)
            .unwrap()
            .worktree_path
    };

    let first = derive(7);
    let second = derive(8);

    assert_eq!(first, format!("{repo_path}/.worktrees/7-same-title"));
    assert_eq!(second, format!("{repo_path}/.worktrees/8-same-title"));
    assert_ne!(
        first, second,
        "identical titles must still yield distinct worktrees — the task id is \
         what makes the path injective, and WorktreeIsNeverShared depends on it"
    );
}

#[test]
fn provision_worktree_skips_git_when_dir_exists() {
    let (_dir, repo_path, worktree_dir) = make_test_repo_with_worktree("42-fix-bug");

    let mock = MockProcessRunner::new(vec![
        MockProcessRunner::ok(), // tmux new-window
        MockProcessRunner::ok(), // tmux set-option @dispatch_dir
        MockProcessRunner::ok(), // tmux set-hook (after-split-window)
    ]);

    let task = make_task(&repo_path);
    let result = provision_worktree(&task, &mock, None, SUBPROCESS_TIMEOUT).unwrap();

    let calls = mock.recorded_calls();
    assert!(
        calls.iter().all(|(prog, _)| prog != "git"),
        "git should be skipped"
    );
    assert_eq!(calls[0].0, "tmux");
    assert_eq!(calls[0].1[0], "new-window");
    assert_eq!(result.worktree_path, worktree_dir.to_str().unwrap());
}

#[test]
fn provision_worktree_reports_reused_worktree_false_when_dir_missing() {
    let (_dir, repo_path) = make_test_repo();
    // Do NOT pre-create the worktree dir — the "fresh" path.

    let mock = MockProcessRunner::new(vec![
        MockProcessRunner::ok(), // git worktree add
        MockProcessRunner::ok(), // tmux new-window
        MockProcessRunner::ok(), // tmux set-option @dispatch_dir
        MockProcessRunner::ok(), // tmux set-hook (after-split-window)
    ]);

    let task = make_task(&repo_path);
    let result = provision_worktree(&task, &mock, None, SUBPROCESS_TIMEOUT).unwrap();

    assert!(
        !result.reused_worktree,
        "a freshly created worktree directory must report reused_worktree == false"
    );
}

#[test]
fn provision_worktree_reports_reused_worktree_true_when_dir_exists() {
    let (_dir, repo_path, _worktree_dir) = make_test_repo_with_worktree("42-fix-bug");

    let mock = MockProcessRunner::new(vec![
        MockProcessRunner::ok(), // tmux new-window
        MockProcessRunner::ok(), // tmux set-option @dispatch_dir
        MockProcessRunner::ok(), // tmux set-hook (after-split-window)
    ]);

    let task = make_task(&repo_path);
    let result = provision_worktree(&task, &mock, None, SUBPROCESS_TIMEOUT).unwrap();

    assert!(
        result.reused_worktree,
        "a pre-existing worktree directory must report reused_worktree == true"
    );
}

#[test]
fn provision_worktree_with_base_branch_passes_start_point() {
    let (_dir, repo_path) = make_test_repo();

    let script = DispatchScript::provision().fresh_worktree();
    let mock = script.runner();

    let task = make_task(&repo_path);
    let result = provision_worktree(
        &task,
        &mock,
        Some(BaseRef::Branch("99-prev-task")),
        SUBPROCESS_TIMEOUT,
    )
    .unwrap();

    let calls = mock.recorded_calls();
    // call[0] = fetch
    assert_eq!(calls[0].0, "git");
    assert!(calls[0].1.contains(&"fetch".to_string()));
    assert!(calls[0].1.contains(&"99-prev-task".to_string()));
    // call[2] = worktree add — start point is now origin/<base>
    assert_eq!(calls[2].0, "git");
    let git_args = &calls[2].1;
    assert_eq!(
        git_args.last().unwrap(),
        "origin/99-prev-task",
        "base branch should be origin/99-prev-task as last git arg, got: {git_args:?}"
    );

    let expected_path = format!("{repo_path}/.worktrees/42-fix-bug");
    assert_eq!(result.worktree_path, expected_path);
}

#[test]
fn provision_worktree_fetches_origin_before_create() {
    // Fetch succeeds → worktree add should use origin/<base> as start point
    let (_dir, repo_path) = make_test_repo();

    let script = DispatchScript::provision().fresh_worktree();
    let mock = script.runner();

    let task = make_task(&repo_path);
    provision_worktree(
        &task,
        &mock,
        Some(BaseRef::Branch("main")),
        SUBPROCESS_TIMEOUT,
    )
    .unwrap();

    let calls = mock.recorded_calls();
    // call[0] = git fetch origin main
    assert_eq!(calls[0].0, "git");
    assert!(
        calls[0].1.contains(&"fetch".to_string()),
        "expected fetch, got: {:?}",
        calls[0].1
    );
    assert!(calls[0].1.contains(&"origin".to_string()));
    assert!(calls[0].1.contains(&"main".to_string()));
    // call[2] = git worktree add ... origin/main
    assert_eq!(calls[2].0, "git");
    assert!(calls[2].1.contains(&"worktree".to_string()));
    assert_eq!(
        calls[2].1.last().unwrap(),
        "origin/main",
        "worktree add should use origin/main as start point, got: {:?}",
        calls[2].1
    );
}

#[test]
fn provision_worktree_fetch_failure_falls_back_to_local_without_retry() {
    // A fetch that fails and classifies as "no origin ref" (a 404 from
    // ls-remote) is not retried, and the local branch is used — no error.
    let (_dir, repo_path) = make_test_repo();

    let mock = MockProcessRunner::new(vec![
        MockProcessRunner::fail("fatal: couldn't find remote ref main"), // git fetch
        MockProcessRunner::ok(),                  // git remote get-url origin
        MockProcessRunner::fail_with_code(2, ""), // git ls-remote --exit-code (404)
        MockProcessRunner::ok(),                  // git worktree add
        MockProcessRunner::ok(),                  // tmux new-window
        MockProcessRunner::ok(),                  // tmux set-option @dispatch_dir
        MockProcessRunner::ok(),                  // tmux set-hook (after-split-window)
    ]);

    let task = make_task(&repo_path);
    // Should NOT return an error — soft fail
    let result = provision_worktree(
        &task,
        &mock,
        Some(BaseRef::Branch("main")),
        SUBPROCESS_TIMEOUT,
    )
    .unwrap();

    let calls = mock.recorded_calls();
    let fetch_attempts = calls
        .iter()
        .filter(|(prog, args)| prog == "git" && args.contains(&"fetch".to_string()))
        .count();
    assert_eq!(
        fetch_attempts, 1,
        "a 404-classified fetch failure must not be retried, got: {calls:?}"
    );
    // call[3] = worktree add using local "main" (not "origin/main")
    assert_eq!(calls[3].0, "git");
    assert!(calls[3].1.contains(&"worktree".to_string()));
    assert_eq!(
        calls[3].1.last().unwrap(),
        "main",
        "fallback should use local main, got: {calls:?}"
    );
    let warning = result
        .fetch_warning
        .expect("expected a fetch_warning when there is no origin ref");
    assert!(
        warning.contains("main"),
        "warning should mention the base branch, got: {warning}"
    );
}

#[test]
fn provision_worktree_pr_head_missing_from_origin_aborts_rather_than_using_local() {
    // Mirrors provision_worktree_fetch_failure_falls_back_to_local_without_retry's
    // mock shape, but with BaseRef::PrHead: origin missing the PR's head branch
    // must abort, never fall back to a local branch of the same name.
    let (_dir, repo_path) = make_test_repo();

    let mock = MockProcessRunner::new(vec![
        MockProcessRunner::fail("fatal: couldn't find remote ref feature-x"), // git fetch
        MockProcessRunner::ok(),                  // git remote get-url origin
        MockProcessRunner::fail_with_code(2, ""), // git ls-remote --exit-code (404)
    ]);

    let task = make_task(&repo_path);
    let err = provision_worktree(
        &task,
        &mock,
        Some(BaseRef::PrHead("feature-x")),
        SUBPROCESS_TIMEOUT,
    )
    .unwrap_err();

    assert!(
        err.to_string().contains("feature-x"),
        "error should name the missing branch, got: {err}"
    );

    let calls = mock.recorded_calls();
    assert!(
        calls
            .iter()
            .all(|(prog, args)| !(prog == "git" && args.contains(&"worktree".to_string()))),
        "must not create a worktree from a stale local branch: {calls:?}"
    );
    assert!(
        calls.iter().all(|(prog, _)| prog != "tmux"),
        "must not open a tmux window when the review has no safe ref to base on: {calls:?}"
    );
}

#[test]
fn provision_worktree_retries_fetch_before_falling_back() {
    // First fetch fails and classifies as unreachable (not a 404) → retried;
    // second fetch fails too; third succeeds → no fallback, no warning.
    let (_dir, repo_path) = make_test_repo();

    let script = DispatchScript::provision()
        .fresh_worktree()
        .fetch_succeeds_on_attempt(3);
    let mock = script.runner();

    let task = make_task(&repo_path);
    let result = provision_worktree(
        &task,
        &mock,
        Some(BaseRef::Branch("main")),
        SUBPROCESS_TIMEOUT,
    )
    .unwrap();

    let calls = mock.recorded_calls();
    let fetch_attempts = calls
        .iter()
        .filter(|(prog, args)| prog == "git" && args.contains(&"fetch".to_string()))
        .count();
    assert_eq!(
        fetch_attempts, FETCH_MAX_ATTEMPTS as usize,
        "expected 2 failures + 1 success, i.e. the full budget, got: {calls:?}"
    );
    assert_eq!(
        calls[6].1.last().unwrap(),
        "origin/main",
        "should use origin/main once fetch eventually succeeds, got: {calls:?}"
    );
    assert!(
        result.fetch_warning.is_none(),
        "no warning expected when fetch eventually succeeds"
    );
}

/// `PROVISION_MAX_SUBPROCESS_CALLS` (`src/dispatch/worktree.rs`) is what
/// `DISPATCH_WATCHDOG_TIMEOUT` (`src/tui/mod.rs`) derives its budget from —
/// see #4201. Pin it to the real worst-case shape (a fetch that only
/// succeeds on its last allowed attempt) via `DispatchScript`'s own
/// step-counting, rather than trusting a hand-maintained literal: if
/// `fetch_origin`'s retry/classify logic ever grows another call, this fails
/// instead of silently under-sizing the watchdog again.
///
/// `index_of(Step::WorktreeAdd) + 1` counts only up through the last
/// `SUBPROCESS_TIMEOUT`-bounded call in the sequence — it says nothing about
/// (and doesn't need to, since `PROVISION_MAX_SUBPROCESS_CALLS` doesn't cover
/// them either) the unbounded tmux tail that follows worktree add.
#[test]
fn provision_max_subprocess_calls_matches_the_worst_case_shape() {
    let script = DispatchScript::provision()
        .fresh_worktree()
        .fetch_succeeds_on_attempt(FETCH_MAX_ATTEMPTS);

    assert_eq!(
        script.index_of(Step::WorktreeAdd) + 1,
        PROVISION_MAX_SUBPROCESS_CALLS as usize,
        "PROVISION_MAX_SUBPROCESS_CALLS must match the real worst-case number \
         of SUBPROCESS_TIMEOUT-bounded calls provision_worktree can issue"
    );
}

#[test]
fn provision_worktree_fetch_uses_custom_base_branch() {
    // Custom base_branch is used in both fetch and worktree add
    let (_dir, repo_path) = make_test_repo();

    let script = DispatchScript::provision().fresh_worktree();
    let mock = script.runner();

    let task = make_task(&repo_path);
    provision_worktree(
        &task,
        &mock,
        Some(BaseRef::Branch("develop")),
        SUBPROCESS_TIMEOUT,
    )
    .unwrap();

    let calls = mock.recorded_calls();
    assert!(
        calls[0].1.contains(&"develop".to_string()),
        "fetch should use 'develop', got: {:?}",
        calls[0].1
    );
    assert_eq!(
        calls[2].1.last().unwrap(),
        "origin/develop",
        "worktree add should use origin/develop, got: {:?}",
        calls[2].1
    );
}

#[test]
fn provision_worktree_still_fetches_when_dir_exists() {
    // Pre-existing worktree dir → fetch still runs (so origin/<base> stays
    // fresh for whatever rebases onto it later), but `git worktree add` is
    // skipped since the branch/dir already exist.
    let (_dir, repo_path, _worktree_dir) = make_test_repo_with_worktree("42-fix-bug");

    let script = DispatchScript::provision();
    let mock = script.runner();

    let task = make_task(&repo_path);
    provision_worktree(
        &task,
        &mock,
        Some(BaseRef::Branch("main")),
        SUBPROCESS_TIMEOUT,
    )
    .unwrap();

    let calls = mock.recorded_calls();
    assert_eq!(
        calls[0].0, "git",
        "first call should be git, got: {calls:?}"
    );
    assert!(
        calls[0].1.contains(&"fetch".to_string()),
        "fetch should still run when the worktree dir already exists, got: {calls:?}"
    );
    assert!(
        calls
            .iter()
            .all(|(prog, args)| !(prog == "git" && args.contains(&"worktree".to_string()))),
        "git worktree add should be skipped when the dir already exists, got: {calls:?}"
    );
}

// ---------------------------------------------------------------------------
// Reuse path vs an unreachable origin (#3843)
//
// On the reuse path `git worktree add` is skipped, so no ref is consumed to
// create anything: the resolved start point feeds only the rebase preamble.
// An unreachable origin therefore has nothing to corrupt, and aborting there
// costs an offline user a dispatch that needed no network at all.
// ---------------------------------------------------------------------------

#[test]
fn provision_worktree_reuse_survives_an_unreachable_origin() {
    // Dir already exists + every fetch fails ⇒ the dispatch still proceeds,
    // based on local <base>, with a Note: for the agent.
    let (_dir, repo_path, _worktree_dir) = make_test_repo_with_worktree("42-fix-bug");

    let mock = MockProcessRunner::new(vec![
        MockProcessRunner::fail("fatal: unable to access 'origin': network is unreachable"),
        MockProcessRunner::ok(), // tmux new-window
        MockProcessRunner::ok(), // tmux set-option @dispatch_dir
        MockProcessRunner::ok(), // tmux set-hook (after-split-window)
    ]);

    let task = make_task(&repo_path);
    let result = provision_worktree(
        &task,
        &mock,
        Some(BaseRef::Branch("main")),
        SUBPROCESS_TIMEOUT,
    )
    .expect("a reused worktree needs no network, so an unreachable origin must not abort");

    assert_eq!(
        result.start_point,
        Some(StartPoint::Local {
            base: "main".to_string()
        }),
        "an unfetchable origin/<base> must not be the rebase target: it would replay \
         local <base>'s unpushed commits under new SHAs"
    );
    let warning = result
        .fetch_warning
        .expect("the agent must be told in its prompt that origin could not be reached");
    assert!(
        warning.contains("main"),
        "the warning should name the base branch, got: {warning}"
    );

    let calls = mock.recorded_calls();
    assert!(
        calls.iter().any(|(prog, _)| prog == "tmux"),
        "the agent's tmux window must still be created, got: {calls:?}"
    );
}

#[test]
fn provision_worktree_reuse_does_not_retry_or_probe_an_unreachable_origin() {
    // The budget test. Both the retry loop and the ls-remote classification
    // probe exist to serve the abort decision — retries to smooth a transient
    // failure before aborting, the probe to tell a 404 (fall back) from infra
    // (abort). With no abort on this path both classes end the same way, so
    // each extra network call buys nothing and costs a full SUBPROCESS_TIMEOUT.
    // Downgrading the abort without this assertion would leave the ~4 minutes
    // of offline blocking exactly where they were.
    let (_dir, repo_path, _worktree_dir) = make_test_repo_with_worktree("42-fix-bug");

    let mock = MockProcessRunner::new(vec![
        MockProcessRunner::fail("fatal: unable to access 'origin': network is unreachable"),
        MockProcessRunner::ok(), // tmux new-window
        MockProcessRunner::ok(), // tmux set-option @dispatch_dir
        MockProcessRunner::ok(), // tmux set-hook (after-split-window)
    ]);

    let task = make_task(&repo_path);
    provision_worktree(
        &task,
        &mock,
        Some(BaseRef::Branch("main")),
        SUBPROCESS_TIMEOUT,
    )
    .expect("reuse + unreachable origin must not abort");

    let calls = mock.recorded_calls();
    let fetches = calls
        .iter()
        .filter(|(prog, args)| prog == "git" && args.contains(&"fetch".to_string()))
        .count();
    assert_eq!(
        fetches, 1,
        "the reuse path keeps origin fresh with a single best-effort attempt, never a \
         retry budget, got: {calls:?}"
    );
    assert!(
        calls
            .iter()
            .all(|(_, args)| !args.contains(&"ls-remote".to_string())),
        "classifying the failure changes nothing on the reuse path, so the probe must \
         not be run, got: {calls:?}"
    );
}

#[test]
fn provision_worktree_reuse_of_a_pr_head_keeps_the_remote_start_point() {
    // BaseRef::PrHead must never yield a Local start point — a stale local
    // branch of the same name would let a review examine the wrong code. The
    // existing worktree already holds the PR's code from the previous attempt,
    // so reuse is safe; only the preamble's rebase target is at stake.
    let (_dir, repo_path, _worktree_dir) = make_test_repo_with_worktree("42-fix-bug");

    let mock = MockProcessRunner::new(vec![
        MockProcessRunner::fail("fatal: unable to access 'origin': network is unreachable"),
        MockProcessRunner::ok(), // tmux new-window
        MockProcessRunner::ok(), // tmux set-option @dispatch_dir
        MockProcessRunner::ok(), // tmux set-hook (after-split-window)
    ]);

    let task = make_task(&repo_path);
    let result = provision_worktree(
        &task,
        &mock,
        Some(BaseRef::PrHead("feature-x")),
        SUBPROCESS_TIMEOUT,
    )
    .expect("a reused PR-review worktree already holds the PR's code");

    assert_eq!(
        result.start_point,
        Some(StartPoint::Remote {
            base: "feature-x".to_string()
        }),
        "a PR head must stay pinned to origin/<head>, never fall back to a local branch \
         of the same name"
    );
    assert!(
        result.fetch_warning.is_some(),
        "the agent must be told its rebase target may be unreachable"
    );
}

#[test]
fn provision_worktree_fresh_still_spends_the_full_budget_before_aborting() {
    // Regression guard: the reuse-path shortcut above must not leak onto the
    // fresh path, where the resolved ref really does create the branch and a
    // stale local ref is the failure #3810 exists to prevent.
    let (_dir, repo_path) = make_test_repo();

    let mock = MockProcessRunner::new(vec![
        MockProcessRunner::fail("fatal: unable to access 'origin': network is unreachable"),
        MockProcessRunner::ok(), // git remote get-url origin
        MockProcessRunner::fail_with_code(128, ""), // git ls-remote --exit-code (unreachable)
        MockProcessRunner::fail("fatal: unable to access 'origin': network is unreachable"),
        MockProcessRunner::fail("fatal: unable to access 'origin': network is unreachable"),
    ]);

    let task = make_task(&repo_path);
    let err = provision_worktree(
        &task,
        &mock,
        Some(BaseRef::Branch("main")),
        SUBPROCESS_TIMEOUT,
    )
    .expect_err("a fresh worktree must not be branched off a stale local ref");

    let calls = mock.recorded_calls();
    let fetches = calls
        .iter()
        .filter(|(prog, args)| prog == "git" && args.contains(&"fetch".to_string()))
        .count();
    assert_eq!(
        fetches, 3,
        "the fresh path still retries to exhaustion, got: {calls:?}"
    );
    assert!(
        calls
            .iter()
            .any(|(_, args)| args.contains(&"ls-remote".to_string())),
        "the fresh path still classifies, because 404 and infra diverge there: {calls:?}"
    );
    assert!(
        !calls.iter().any(|(prog, _)| prog == "tmux"),
        "aborting must happen before any tmux window is created, got: {calls:?}"
    );

    // The error names a next step, not just the cause and the attempt count.
    let msg = err.to_string();
    assert!(
        msg.contains("network") || msg.contains("connectivity"),
        "the error should point at what to check, got: {msg}"
    );
    assert!(
        msg.contains("retry") || msg.contains("dispatch again") || msg.contains("try again"),
        "the error should say what to do once connectivity is back, got: {msg}"
    );
}

#[test]
fn rebase_preamble_with_base_branch() {
    let sp = StartPoint::Remote {
        base: "99-prev-task".to_string(),
    };
    let preamble = reused_rebase_preamble(&sp);
    assert!(
        preamble.contains("git fetch origin 99-prev-task"),
        "should fetch the base branch first, got: {preamble}"
    );
    assert!(
        preamble.contains("git rebase origin/99-prev-task"),
        "should rebase onto the fetched origin ref, got: {preamble}"
    );
    assert!(
        !preamble.contains("origin/main"),
        "should not reference origin/main"
    );
}

#[test]
fn rebase_preamble_uses_given_target() {
    let sp = StartPoint::Remote {
        base: "develop".to_string(),
    };
    let preamble = reused_rebase_preamble(&sp);
    assert!(
        preamble.contains("git fetch origin develop"),
        "should fetch the given target, got: {preamble}"
    );
    assert!(
        preamble.contains("git rebase origin/develop"),
        "should rebase onto origin/<given target>, got: {preamble}"
    );
    assert!(
        !preamble.contains("origin/main"),
        "should not contain origin/main"
    );
}

#[test]
fn resume_skips_git_issues_tmux_continue() {
    let (_dir, worktree_path) = make_test_repo();

    let script = DispatchScript::resume();
    let mock = script.runner();

    resume_agent(TaskId(42), &worktree_path, &mock).unwrap();

    let calls = mock.recorded_calls();
    script.assert_matches(&calls);
    let new_window = &calls[script.index_of(Step::NewWindow)];
    assert_eq!(new_window.0, "tmux");
    assert_eq!(new_window.1[0], "new-window");
    assert_eq!(
        calls[script.index_of(Step::SetDispatchDir)].1[0],
        "set-option"
    );
    assert_eq!(calls[script.index_of(Step::SetSplitHook)].1[0], "set-hook");
    assert!(
        calls.iter().all(|(prog, _)| prog != "git"),
        "resume should make no git calls"
    );
    assert!(calls[script.index_of(Step::SendKeysLiteral)]
        .1
        .iter()
        .any(|a| a.contains("--continue")));
}

#[test]
fn resume_agent_splits_agent_tree_companion_pane_after_send_keys() {
    let (_dir, worktree_path) = make_test_repo();

    let script = DispatchScript::resume();
    let mock = script.runner();

    resume_agent(TaskId(42), &worktree_path, &mock).unwrap();

    let calls = mock.recorded_calls();
    let last = &calls[script.index_of(Step::CompanionSplit)];
    assert_eq!(last.0, "tmux");
    assert_eq!(last.1[0], "split-window");
    assert_eq!(
        last.1[last.1.len() - 3..],
        vec![
            "dispatch".to_string(),
            "agent-tree".to_string(),
            "42".to_string(),
        ],
        "companion pane should run `dispatch agent-tree <task_id>`, got: {:?}",
        last.1
    );
}

#[test]
fn resume_agent_succeeds_even_if_companion_pane_split_fails() {
    let (_dir, worktree_path) = make_test_repo();

    let mock = MockProcessRunner::new(vec![
        MockProcessRunner::ok(), // tmux list-windows (has_window: not alive)
        MockProcessRunner::ok(), // tmux new-window
        MockProcessRunner::ok(), // tmux set-option @dispatch_dir
        MockProcessRunner::ok(), // tmux set-hook (after-split-window)
        MockProcessRunner::ok(), // tmux send-keys -l
        MockProcessRunner::ok(), // tmux send-keys Enter
        MockProcessRunner::fail("no target pane"), // tmux split-window fails
    ]);

    let result = resume_agent(TaskId(42), &worktree_path, &mock);
    assert!(
        result.is_ok(),
        "a failed companion-pane split must not fail resume: {result:?}"
    );
}

#[test]
fn cleanup_kills_window_and_removes_worktree() {
    let mock = MockProcessRunner::new(vec![
        // has_window: list-windows returns the window name in stdout
        MockProcessRunner::ok_with_stdout(b"task-42\n"),
        MockProcessRunner::ok(), // tmux kill-window
        MockProcessRunner::ok(), // git worktree remove
        MockProcessRunner::ok(), // git branch -D (best-effort)
    ]);

    teardown_task(
        "/repo",
        Some("/repo/.worktrees/42-fix-bug"),
        Some(&test_tmux_window("task-42")),
        &mock,
    )
    .unwrap();

    let calls = mock.recorded_calls();
    assert_eq!(calls[0].0, "tmux");
    assert_eq!(calls[0].1[0], "list-windows");
    assert_eq!(calls[1].0, "tmux");
    assert_eq!(calls[1].1[0], "kill-window");
    assert_eq!(calls[2].0, "git");
    // git worktree remove is invoked with -C <repo>
    assert!(calls[2].1.contains(&"-C".to_string()));
    assert!(calls[2].1.contains(&"remove".to_string()));
}

#[test]
fn cleanup_succeeds_when_worktree_already_removed() {
    // When git says "not a working tree" the archive should still succeed,
    // not surface an error to the user.
    let mock = MockProcessRunner::new(vec![
        MockProcessRunner::fail("fatal: '/repo/.worktrees/42-fix-bug' is not a working tree"),
        MockProcessRunner::ok(), // git branch -D (best-effort)
    ]);

    teardown_task("/repo", Some("/repo/.worktrees/42-fix-bug"), None, &mock).unwrap();
}

#[test]
fn dispatch_uses_task_base_branch_in_prompt() {
    let (_dir, repo_path, worktree_dir) = make_test_repo_with_worktree("42-fix-bug");

    let script = DispatchScript::dispatch();
    let mock = script.runner();

    let mut task = make_task(&repo_path);
    task.base_branch = "master".into();
    dispatch_agent(&task, &mock, None, &LearningInjections::default()).unwrap();

    // Verify the prompt uses task.base_branch directly — no symbolic-ref call needed
    let prompt_file = worktree_dir.join(".claude-prompt");
    let prompt = std::fs::read_to_string(prompt_file).unwrap();
    assert!(
        prompt.contains("git rebase origin/master"),
        "prompt should reference task.base_branch (master), got: {prompt}"
    );
    assert!(
        !prompt.contains("git rebase origin/main"),
        "prompt should not reference main when task.base_branch is master"
    );
}

#[test]
fn dispatch_fails_fast_if_git_fails() {
    let (_dir, repo_path) = make_test_repo();

    // `fails_at` queues nothing past the failure, so a tmux call after the failed
    // worktree add panics the mock rather than passing unnoticed.
    let script = DispatchScript::dispatch()
        .fresh_worktree()
        .fails_at(Step::WorktreeAdd);
    let mock = script.runner();

    let task = make_task(&repo_path);
    let result = dispatch_agent(&task, &mock, None, &LearningInjections::default());
    assert!(result.is_err());
    let calls = mock.recorded_calls();
    assert_eq!(
        calls.len(),
        3,
        "only git fetch + rev-list + git worktree add should have been called (no detect_default_branch)"
    );
}

#[test]
fn quick_dispatch_reuses_existing_worktree() {
    let (_dir, repo_path, _worktree_dir) = make_test_repo_with_worktree("42-fix-bug");

    let script = DispatchScript::dispatch();
    let mock = script.runner();

    let task = make_task(&repo_path);
    quick_dispatch_agent(&task, &mock, None, &LearningInjections::default()).unwrap();

    let calls = mock.recorded_calls();
    assert!(
        calls
            .iter()
            .all(|(prog, args)| !(prog == "git" && args.iter().any(|a| a == "worktree"))),
        "git worktree add should be skipped for existing worktree"
    );
    assert_eq!(calls[2].0, "tmux");
    assert_eq!(calls[2].1[0], "new-window");
}

#[test]
fn quick_dispatch_sends_rename_prompt() {
    let (_dir, repo_path, worktree_dir) = make_test_repo_with_worktree("42-fix-bug");

    let script = DispatchScript::dispatch();
    let mock = script.runner();

    let task = make_task(&repo_path);
    quick_dispatch_agent(&task, &mock, None, &LearningInjections::default()).unwrap();

    let prompt_file = worktree_dir.join(".claude-prompt");
    let prompt = std::fs::read_to_string(prompt_file).unwrap();
    assert!(
        prompt.contains("placeholder"),
        "prompt should mention placeholder title"
    );
    assert!(
        prompt.contains("update_task"),
        "prompt should mention update_task for rename"
    );
}

// --- check_pr_status error-path tests ---

#[test]
fn check_pr_status_unknown_state_returns_error() {
    let mock = MockProcessRunner::new(vec![MockProcessRunner::ok_with_stdout(
        b"FUNKY_NEW_STATE\n",
    )]);
    let result = check_pr_status("https://github.com/org/repo/pull/42", &mock);
    assert!(result.is_err(), "unknown PR state should return an error");
    let msg = result.unwrap_err().to_string();
    assert!(
        msg.contains("FUNKY_NEW_STATE"),
        "error should include the unknown state, got: {msg}"
    );
}

#[test]
fn check_pr_status_empty_output_returns_error() {
    let mock = MockProcessRunner::new(vec![MockProcessRunner::ok_with_stdout(b"")]);
    let result = check_pr_status("https://github.com/org/repo/pull/42", &mock);
    assert!(
        result.is_err(),
        "empty output from gh pr view should return an error"
    );
}

// --- pr_head_branch tests ---

#[test]
fn pr_head_branch_returns_head_ref_for_same_repo_pr() {
    let mock = MockProcessRunner::new(vec![MockProcessRunner::ok_with_stdout(&pr_view_reply(
        "renovate/serde-1.x",
        false,
    ))]);
    let branch = pr_head_branch("https://github.com/org/repo/pull/7", &mock);
    assert_eq!(branch.as_deref(), Some("renovate/serde-1.x"));

    let calls = mock.recorded_calls();
    assert_eq!(calls[0].0, "gh");
    assert!(calls[0].1.contains(&"pr".to_string()));
    assert!(calls[0].1.contains(&"view".to_string()));
    assert!(
        calls[0]
            .1
            .contains(&"headRefName,isCrossRepository".to_string()),
        "should request headRefName and isCrossRepository, got: {:?}",
        calls[0].1
    );
}

#[test]
fn pr_head_branch_returns_none_for_fork_pr() {
    let mock = MockProcessRunner::new(vec![MockProcessRunner::ok_with_stdout(&pr_view_reply(
        "patch-1", true,
    ))]);
    assert_eq!(
        pr_head_branch("https://github.com/org/repo/pull/7", &mock),
        None,
        "fork (isCrossRepository=true) PR should fall back to base branch"
    );
}

#[test]
fn pr_head_branch_returns_none_on_command_failure() {
    let mock = MockProcessRunner::new(vec![MockProcessRunner::fail("gh: not authenticated")]);
    assert_eq!(
        pr_head_branch("https://github.com/org/repo/pull/7", &mock),
        None
    );
}

#[test]
fn pr_head_branch_returns_none_on_empty_output() {
    let mock = MockProcessRunner::new(vec![MockProcessRunner::ok_with_stdout(b"")]);
    assert_eq!(
        pr_head_branch("https://github.com/org/repo/pull/7", &mock),
        None
    );
}

// A hung `gh` must not park the calling thread forever (#4202).
#[test]
fn pr_head_branch_is_bounded_by_subprocess_timeout() {
    let mock = MockProcessRunner::new(vec![MockProcessRunner::ok_with_stdout(&pr_view_reply(
        "renovate/serde-1.x",
        false,
    ))]);
    pr_head_branch("https://github.com/org/repo/pull/7", &mock);
    assert_eq!(mock.recorded_timeouts(), vec![Some(SUBPROCESS_TIMEOUT)]);
}

// --- finish_task tests ---

/// Drive `finish_task` under `script`, which declares the base branch, the
/// paths and the whole subprocess sequence — and asserts the recorded calls are
/// exactly that sequence. See `docs/conventions.md`, "Driving a dispatch:
/// `DispatchScript`, never a hand-written queue"; the same rule holds for a
/// finish.
fn run_finish(script: &DispatchScript) -> FinishRun {
    script.drive_finish()
}

#[test]
fn finish_task_happy_path() {
    let (calls, result) = run_finish(&DispatchScript::finish());

    result.unwrap();
    assert!(calls.iter().any(|c| c.1.contains(&"rebase".to_string())));
    assert!(calls.iter().any(|c| c.1.contains(&"--ff-only".to_string())));
    // No worktree removal
    assert!(!calls.iter().any(|c| c.1.contains(&"remove".to_string())));
}

#[test]
fn finish_task_with_master_default_branch() {
    let script = DispatchScript::finish().base_branch("master");
    let (calls, result) = run_finish(&script);

    result.unwrap();
    // pull and rebase should both reference "master", not "main"
    for step in [Step::Pull, Step::Rebase] {
        assert!(
            calls[script.index_of(step)]
                .1
                .contains(&"master".to_string()),
            "{step:?} should target master, got: {calls:?}"
        );
    }
}

#[test]
fn finish_task_not_on_default_branch() {
    let (_calls, result) = run_finish(&DispatchScript::finish().head_branch("feature-branch"));

    let err = result.unwrap_err();
    assert!(matches!(err, FinishError::NotOnDefaultBranch { .. }));
    assert!(err.to_string().contains("feature-branch"));
}

#[test]
fn finish_task_rebase_conflict() {
    let script = DispatchScript::finish()
        .no_remote()
        .rebase_conflicts_in_stderr(&["src/main.rs"]);
    let (calls, result) = run_finish(&script);

    let err = result.unwrap_err();
    assert!(
        matches!(
            err,
            FinishError::RebaseConflict { ref files, .. } if files == &["src/main.rs".to_string()]
        ),
        "expected RebaseConflict naming src/main.rs, got: {err}"
    );
    assert!(calls.last().unwrap().1.contains(&"--abort".to_string()));
}

#[test]
fn finish_task_pull_fails() {
    let (_calls, result) = run_finish(&DispatchScript::finish().pull_fails());
    assert!(matches!(result.unwrap_err(), FinishError::Other(_)));
}

#[test]
fn finish_task_dirty_primary_worktree_returns_error_before_pull() {
    let (calls, result) =
        run_finish(&DispatchScript::finish().dirty_primary(&["src/unrelated.rs"]));

    let err = result.unwrap_err();
    assert!(
        matches!(err, FinishError::DirtyPrimaryWorktree { ref path, ref files }
            if path == "/repo" && files == &["src/unrelated.rs".to_string()]),
        "expected DirtyPrimaryWorktree naming /repo and src/unrelated.rs, got: {err}"
    );

    assert!(
        !calls.iter().any(|c| c.1.contains(&"pull".to_string())
            || c.1.contains(&"rebase".to_string())
            || c.1.contains(&"--ff-only".to_string())),
        "a dirty primary worktree must be reported before any pull/rebase/merge is attempted, got: {calls:?}"
    );
}

// --- dispatch guard tests ---

#[test]
fn dispatch_agent_fails_fast_with_empty_repo_path() {
    let mock = MockProcessRunner::new(vec![]);
    let mut task = make_task("/some/repo");
    task.repo_path = "".to_string();
    let result = dispatch_agent(&task, &mock, None, &LearningInjections::default());
    assert!(result.is_err());
    let msg = result.unwrap_err().to_string();
    assert!(
        msg.contains("Repository path"),
        "error should mention 'Repository path', got: {msg}"
    );
}

// --- check_pr_status tests ---

#[test]
fn check_pr_status_open() {
    let mock = MockProcessRunner::new(vec![MockProcessRunner::ok_with_stdout(
        b"OPEN\nREVIEW_REQUIRED\n",
    )]);
    let result = check_pr_status("https://github.com/org/repo/pull/42", &mock).unwrap();
    assert_eq!(result.state, PrState::Open);
    assert_eq!(result.review_decision, Some(ReviewDecision::ReviewRequired));
}

#[test]
fn check_pr_status_merged() {
    let mock = MockProcessRunner::new(vec![MockProcessRunner::ok_with_stdout(b"MERGED\n")]);
    let result = check_pr_status("https://github.com/org/repo/pull/42", &mock).unwrap();
    assert_eq!(result.state, PrState::Merged);
    assert_eq!(result.review_decision, None);
}

#[test]
fn check_pr_status_closed() {
    let mock = MockProcessRunner::new(vec![MockProcessRunner::ok_with_stdout(b"CLOSED\n")]);
    let result = check_pr_status("https://github.com/org/repo/pull/42", &mock).unwrap();
    assert_eq!(result.state, PrState::Closed);
    assert_eq!(result.review_decision, None);
}

#[test]
fn check_pr_status_open_approved() {
    let mock = MockProcessRunner::new(vec![MockProcessRunner::ok_with_stdout(b"OPEN\nAPPROVED\n")]);
    let result = check_pr_status("https://github.com/org/repo/pull/42", &mock).unwrap();
    assert_eq!(result.state, PrState::Open);
    assert_eq!(result.review_decision, Some(ReviewDecision::Approved));
}

#[test]
fn check_pr_status_open_changes_requested() {
    let mock = MockProcessRunner::new(vec![MockProcessRunner::ok_with_stdout(
        b"OPEN\nCHANGES_REQUESTED\n",
    )]);
    let result = check_pr_status("https://github.com/org/repo/pull/42", &mock).unwrap();
    assert_eq!(result.state, PrState::Open);
    assert_eq!(
        result.review_decision,
        Some(ReviewDecision::ChangesRequested)
    );
}

#[test]
fn finish_task_no_remote_skips_pull() {
    let (calls, result) = run_finish(&DispatchScript::finish().no_remote());

    result.unwrap();
    // Should not have a "pull" call
    assert!(!calls.iter().any(|c| c.1.contains(&"pull".to_string())));
}

// --- new TDD tests for explicit base_branch ---

#[test]
fn finish_task_uses_explicit_base_branch_not_auto_detected() {
    // "develop" is passed explicitly; no symbolic-ref (detect_default_branch) call
    let script = DispatchScript::finish().base_branch("develop").no_remote();
    let (calls, result) = run_finish(&script);

    result.unwrap();
    // No symbolic-ref call — branch was provided explicitly
    assert!(
        !calls
            .iter()
            .any(|c| c.0 == "git" && c.1.iter().any(|a| a == "symbolic-ref")),
        "symbolic-ref must not be called when base_branch is explicit"
    );
    // Rebase should target "develop"
    assert!(calls[script.index_of(Step::Rebase)]
        .1
        .contains(&"develop".to_string()));
}

#[test]
fn dispatch_agent_uses_task_base_branch_in_prompt() {
    let (_dir, repo_path, worktree_dir) = make_test_repo_with_worktree("42-fix-bug");

    // No detect_default_branch call expected — task.base_branch is used directly
    let script = DispatchScript::dispatch();
    let mock = script.runner();

    let mut task = make_task(&repo_path);
    task.base_branch = "develop".into();
    dispatch_agent(&task, &mock, None, &LearningInjections::default()).unwrap();

    let prompt_file = worktree_dir.join(".claude-prompt");
    let prompt = std::fs::read_to_string(prompt_file).unwrap();
    assert!(
        prompt.contains("git rebase origin/develop"),
        "prompt should reference task.base_branch (develop), got: {prompt}"
    );
    // No symbolic-ref call
    let calls = mock.recorded_calls();
    assert!(
        !calls
            .iter()
            .any(|c| c.0 == "git" && c.1.iter().any(|a| a == "symbolic-ref")),
        "dispatch_agent must not call symbolic-ref when task.base_branch is set"
    );
}

// --- plugin-dir tests ---

#[test]
fn dispatch_agent_includes_plugin_dir() {
    let (_dir, repo_path, _worktree_dir) = make_test_repo_with_worktree("42-fix-bug");

    let script = DispatchScript::dispatch();
    let mock = script.runner();

    let task = make_task(&repo_path);
    dispatch_agent(&task, &mock, None, &LearningInjections::default()).unwrap();

    let calls = mock.recorded_calls();
    let send_keys_arg = find_call_arg(&calls, 5, "claude");
    assert!(
        send_keys_arg.contains("--plugin-dir"),
        "dispatch_agent should include --plugin-dir, got: {send_keys_arg}"
    );
    assert!(
        send_keys_arg.contains(".claude/plugins/local/dispatch"),
        "plugin-dir should point to local dispatch plugin, got: {send_keys_arg}"
    );
}

#[test]
fn resume_agent_includes_plugin_dir() {
    let (_dir, worktree_path) = make_test_repo();

    let script = DispatchScript::resume();
    let mock = script.runner();

    resume_agent(TaskId(42), &worktree_path, &mock).unwrap();

    let calls = mock.recorded_calls();
    let send_keys_arg = find_call_arg(&calls, script.index_of(Step::SendKeysLiteral), "claude");
    assert!(
        send_keys_arg.contains("--plugin-dir"),
        "resume_agent should include --plugin-dir, got: {send_keys_arg}"
    );
    assert!(
        send_keys_arg.contains(".claude/plugins/local/dispatch"),
        "plugin-dir should point to local dispatch plugin, got: {send_keys_arg}"
    );
}

// --- session naming tests (task #4098: deterministic --name for native
// cross-session messaging addressing) ---

#[test]
fn dispatch_agent_names_the_session_after_the_task() {
    let (_dir, repo_path, _worktree_dir) = make_test_repo_with_worktree("42-fix-bug");

    let script = DispatchScript::dispatch();
    let mock = script.runner();

    let task = make_task(&repo_path);
    dispatch_agent(&task, &mock, None, &LearningInjections::default()).unwrap();

    let calls = mock.recorded_calls();
    let send_keys_arg = find_call_arg(&calls, 5, "claude");
    assert!(
        send_keys_arg.contains("--name task-42"),
        "dispatch_agent should name the session task-<id> for native \
cross-session-messaging addressing, got: {send_keys_arg}"
    );
}

#[test]
fn resume_agent_names_the_session_after_the_task() {
    let (_dir, worktree_path) = make_test_repo();

    let script = DispatchScript::resume();
    let mock = script.runner();

    resume_agent(TaskId(42), &worktree_path, &mock).unwrap();

    let calls = mock.recorded_calls();
    let send_keys_arg = find_call_arg(&calls, script.index_of(Step::SendKeysLiteral), "claude");
    assert!(
        send_keys_arg.contains("--name task-42"),
        "resume_agent should name the session task-<id>, got: {send_keys_arg}"
    );
    // Nothing may follow the flags here: the variadic `--mcp-config` would
    // swallow it instead of claude receiving it as an operand. `--continue` is
    // `-`-prefixed and ends the variadic by itself, so resume needs no `--`;
    // one that grew an operand would (see PromptIsSeparatedFromTheLaunchFlags
    // in docs/specs/dispatch.allium).
    assert!(
        send_keys_arg.ends_with("--continue"),
        "nothing may follow the resume flags, got: {send_keys_arg}"
    );
}

// --- injected binary identities ---
//
// The launchers read `claude` / `dispatch` from `ProcessRunner::agent_binaries`
// rather than hardcoding them. These tests pin argv0, which a mock test could not
// assert while the names were literals.

fn dispatch_mock() -> MockProcessRunner {
    DispatchScript::dispatch().runner()
}

#[test]
fn dispatch_agent_launches_the_runners_claude_binary() {
    let (_dir, repo_path, _worktree_dir) = make_test_repo_with_worktree("42-fix-bug");
    let mock = dispatch_mock().with_agent_binaries(AgentBinaries::stub());

    let task = make_task(&repo_path);
    dispatch_agent(&task, &mock, None, &LearningInjections::default()).unwrap();

    let calls = mock.recorded_calls();
    let send_keys_arg = find_call_arg(&calls, 5, "claude");
    // The binary rides as bash's `$0`, after the script body.
    assert!(
        send_keys_arg.ends_with("/stub/bin/claude-stub"),
        "dispatch_agent must launch the runner's claude binary, got: {send_keys_arg}"
    );
}

#[test]
fn dispatch_agent_launches_the_runners_dispatch_binary_in_the_companion_pane() {
    let (_dir, repo_path, _worktree_dir) = make_test_repo_with_worktree("42-fix-bug");
    let mock = dispatch_mock().with_agent_binaries(AgentBinaries::stub());

    let task = make_task(&repo_path);
    dispatch_agent(&task, &mock, None, &LearningInjections::default()).unwrap();

    // The companion pane is spawned via `split-window --`, so the binary is a
    // plain argv element rather than part of a shell string.
    let split = &mock.recorded_calls()[7].1;
    assert!(
        split.contains(&"/stub/bin/dispatch-stub".to_string()),
        "companion pane must exec the runner's dispatch binary, got: {split:?}"
    );
}

#[test]
fn resume_agent_launches_the_runners_claude_binary() {
    let (_dir, worktree_path) = make_test_repo();
    let script = DispatchScript::resume();
    let mock = script.runner().with_agent_binaries(AgentBinaries::stub());

    resume_agent(TaskId(42), &worktree_path, &mock).unwrap();

    let calls = mock.recorded_calls();
    let send_keys_arg = find_call_arg(&calls, script.index_of(Step::SendKeysLiteral), "claude");
    assert!(
        send_keys_arg.starts_with("/stub/bin/claude-stub --plugin-dir"),
        "resume_agent must launch the runner's claude binary, got: {send_keys_arg}"
    );
    // Nothing may follow the flags here — see the sibling resume test above.
    assert!(
        send_keys_arg.ends_with("--continue"),
        "nothing may follow the resume flags, got: {send_keys_arg}"
    );
}

/// A runner that does not override `agent_binaries` must still emit the bare,
/// unquoted names — the guarantee that this seam changed no production behaviour.
#[test]
fn agent_launchers_default_to_bare_binary_names() {
    let (_dir, repo_path, _worktree_dir) = make_test_repo_with_worktree("42-fix-bug");
    let mock = dispatch_mock();

    let task = make_task(&repo_path);
    dispatch_agent(&task, &mock, None, &LearningInjections::default()).unwrap();

    let calls = mock.recorded_calls();
    let send_keys_arg = find_call_arg(&calls, 5, "claude");
    assert!(
        send_keys_arg.ends_with("' claude"),
        "the default must be the bare, unquoted name, got: {send_keys_arg}"
    );
    assert!(
        calls[7].1.contains(&"dispatch".to_string()),
        "the default companion binary must be the bare name, got: {:?}",
        calls[7].1
    );
}

// --- provision_worktree error path ---

#[test]
fn provision_worktree_nonexistent_repo_path_returns_error_without_creating_dir() {
    let dir = tempfile::tempdir().expect("tempdir");
    let nonexistent = dir.path().to_str().unwrap().to_owned();
    drop(dir); // path is now guaranteed non-existent

    let mock = MockProcessRunner::new(vec![]);
    let task = make_task(&nonexistent);
    let result = provision_worktree(&task, &mock, None, SUBPROCESS_TIMEOUT);

    assert!(
        result.is_err(),
        "nonexistent repo_path should return an error"
    );
    assert!(
        !std::path::Path::new(&nonexistent).exists(),
        "provision_worktree must not create directories for nonexistent repo_path"
    );
}

#[test]
fn provision_worktree_git_add_fails_returns_error() {
    let (_dir, repo_path) = make_test_repo();
    // No base_branch → no fetch; first runner call is git worktree add.
    let mock = MockProcessRunner::new(vec![MockProcessRunner::fail("fatal: not a git repository")]);

    let task = make_task(&repo_path);
    let result = provision_worktree(&task, &mock, None, SUBPROCESS_TIMEOUT);

    assert!(result.is_err(), "git worktree add failure should propagate");
}

#[test]
fn provision_worktree_rolls_back_the_worktree_when_a_later_step_fails() {
    let (_dir, repo_path) = make_test_repo();
    let mock = MockProcessRunner::new(vec![
        MockProcessRunner::ok(),                      // git worktree add
        MockProcessRunner::fail("no server running"), // tmux new-window
        MockProcessRunner::ok(), // tmux list-windows (rollback's window-kill check)
        MockProcessRunner::ok(), // git worktree remove --force (rollback)
        MockProcessRunner::ok(), // git branch -D (rollback)
    ]);
    let task = make_task(&repo_path);
    let result = provision_worktree(&task, &mock, None, SUBPROCESS_TIMEOUT);
    assert!(result.is_err(), "tmux failure must abort provisioning");
    let calls = mock.recorded_calls();
    assert!(
        worktree_remove_call(&calls).is_some(),
        "the created worktree must be removed on the failure path, got: {calls:?}"
    );
}

#[test]
fn provision_worktree_does_not_remove_a_reused_worktree_on_failure() {
    let (_dir, repo_path, _worktree_dir) = make_test_repo_with_worktree("42-fix-bug");
    // Reuse path: `git worktree add` is skipped entirely, so the only call
    // before the failure is `tmux new-window`. The rollback still checks the
    // window it just tried to open (a window is never reused), hence the
    // trailing response.
    let mock = MockProcessRunner::new(vec![
        MockProcessRunner::fail("no server running"), // tmux new-window
        MockProcessRunner::ok(),                      // tmux list-windows (window-kill check)
    ]);
    let task = make_task(&repo_path);
    let result = provision_worktree(&task, &mock, None, SUBPROCESS_TIMEOUT);
    assert!(result.is_err(), "tmux failure must abort provisioning");
    let calls = mock.recorded_calls();
    assert!(
        worktree_remove_call(&calls).is_none(),
        "a reused (pre-existing) worktree must never be removed on failure, got: {calls:?}"
    );
}

// --- teardown_task edge cases ---

#[test]
fn cleanup_skips_kill_when_window_not_found() {
    // tmux_window is Some but has_window returns false (window already gone).
    let mock = MockProcessRunner::new(vec![
        MockProcessRunner::ok_with_stdout(b"\n"), // has_window: empty → false
        MockProcessRunner::ok(),                  // git worktree remove
        MockProcessRunner::ok(),                  // git branch -D (best-effort)
    ]);

    teardown_task(
        "/repo",
        Some("/repo/.worktrees/42-fix-bug"),
        Some(&test_tmux_window("task-42")),
        &mock,
    )
    .unwrap();

    let calls = mock.recorded_calls();
    assert!(
        calls
            .iter()
            .all(|(prog, args)| !(prog == "tmux" && args.iter().any(|a| a == "kill-window"))),
        "kill-window should not be called when window not found, got: {calls:?}"
    );
}

// --- finish_task edge cases ---

#[test]
fn finish_task_rebase_other_failure_aborts_and_returns_other() {
    // Rebase fails with a non-conflict stderr → maps to FinishError::Other.
    // `git rebase --abort` is still issued for cleanup.
    let (calls, result) = run_finish(&DispatchScript::finish().no_remote().rebase_fails());

    let err = result.unwrap_err();
    assert!(
        matches!(err, FinishError::Other(ref m) if m.contains("Rebase failed")),
        "non-conflict rebase failure should be FinishError::Other, got: {err}"
    );
    assert!(
        calls.last().unwrap().1.contains(&"--abort".to_string()),
        "rebase --abort must be invoked after a non-conflict rebase failure"
    );
}

#[test]
fn finish_task_ff_only_failure_returns_other() {
    let (_calls, result) = run_finish(&DispatchScript::finish().no_remote().fast_forward_fails());

    let err = result.unwrap_err();
    assert!(
        matches!(err, FinishError::Other(ref m) if m.contains("Fast-forward failed")),
        "ff-only failure should map to FinishError::Other, got: {err}"
    );
}

#[test]
fn finish_task_rev_parse_runner_error_returns_other() {
    // The runner itself errors (e.g. git binary missing) on rev-parse.
    let (_calls, result) = run_finish(&DispatchScript::finish().current_branch_cannot_run());

    let err = result.unwrap_err();
    assert!(
        matches!(err, FinishError::Other(ref m) if m.contains("Failed to check current branch")),
        "rev-parse runner error should map to FinishError::Other, got: {err}"
    );
}

#[test]
fn finish_task_no_tmux_window_skips_tmux_entirely() {
    // tmux_window=None → no list-windows or kill-window calls.
    let (calls, result) = run_finish(&DispatchScript::finish().no_remote());

    result.unwrap();
    assert!(
        !calls.iter().any(|c| c.0 == "tmux"),
        "no tmux calls expected when tmux_window is None, got: {calls:?}"
    );
}

// ---------------------------------------------------------------------------
// teardown_task — additional branch coverage
// ---------------------------------------------------------------------------

/// `TeardownIsOwedWheneverThereIsSomethingToRelease` in docs/specs/tasks.allium:
/// step 1 is owed on the window's presence alone. Before #4096 the archive/delete
/// wrapper skipped the whole teardown for this row shape and leaked the window.
#[test]
fn teardown_task_kills_window_when_there_is_no_worktree() {
    let mock = MockProcessRunner::new(vec![
        MockProcessRunner::ok_with_stdout(b"task-42\n"), // has_window → true
        MockProcessRunner::ok(),                         // tmux kill-window
    ]);

    teardown_task("/repo", None, Some(&test_tmux_window("task-42")), &mock).unwrap();

    let calls = mock.recorded_calls();
    assert!(
        calls
            .iter()
            .any(|(prog, args)| prog == "tmux" && args.iter().any(|a| a == "kill-window")),
        "the window must be killed even with no worktree, got: {calls:?}"
    );
    assert!(
        !calls.iter().any(|c| c.0 == "git"),
        "no git calls expected with no worktree — no worktree means no branch \
         either, got: {calls:?}"
    );
}

/// The other half of the same clause: a row owning neither resource runs nothing.
#[test]
fn teardown_task_with_neither_worktree_nor_window_runs_no_commands() {
    let mock = MockProcessRunner::new(vec![]);

    teardown_task("/repo", None, None, &mock).unwrap();

    assert!(
        mock.recorded_calls().is_empty(),
        "a stateless row must run no commands, got: {:?}",
        mock.recorded_calls()
    );
}

// A window-only kill failure needs no test of its own: the kill runs before the
// worktree arm, so `teardown_task_kill_window_failure_propagates` below already
// drives the identical path to the same `?`. What the *wrapper* does with that
// error is the interesting half, and lives in
// src/runtime/tests/task_exec.rs::exec_cleanup_window_only_kill_failure_still_applies_the_follow_up.

#[test]
fn teardown_task_no_tmux_window_arg_skips_tmux() {
    // tmux_window=None → cleanup goes straight to worktree remove + branch -D.
    let mock = MockProcessRunner::new(vec![
        MockProcessRunner::ok(), // git worktree remove
        MockProcessRunner::ok(), // git branch -D (best-effort)
    ]);

    teardown_task("/repo", Some("/repo/.worktrees/42-fix-bug"), None, &mock).unwrap();

    let calls = mock.recorded_calls();
    assert!(
        !calls.iter().any(|c| c.0 == "tmux"),
        "no tmux calls expected when tmux_window is None, got: {calls:?}"
    );
    assert!(calls[0].1.contains(&"remove".to_string()));
}

#[test]
fn teardown_task_other_remove_failure_propagates() {
    // git worktree remove fails with stderr that is NOT "is not a working tree"
    // → teardown_task surfaces an error to the caller.
    let mock = MockProcessRunner::new(vec![MockProcessRunner::fail(
        "fatal: some unexpected git failure",
    )]);

    let err = teardown_task("/repo", Some("/repo/.worktrees/42-fix-bug"), None, &mock).unwrap_err();

    let msg = format!("{err:#}");
    assert!(
        msg.contains("git worktree remove failed"),
        "expected 'git worktree remove failed' in error chain, got: {msg}"
    );
}

#[test]
fn teardown_task_kill_window_failure_propagates() {
    // tmux kill-window fails → teardown_task returns an error and does NOT
    // attempt the worktree remove.
    let mock = MockProcessRunner::new(vec![
        MockProcessRunner::ok_with_stdout(b"task-42\n"), // has_window → true
        MockProcessRunner::fail("can't find window"),    // kill-window fails
    ]);

    let err = teardown_task(
        "/repo",
        Some("/repo/.worktrees/42-fix-bug"),
        Some(&test_tmux_window("task-42")),
        &mock,
    )
    .unwrap_err();
    let msg = format!("{err:#}");
    assert!(
        msg.contains("failed to kill tmux window"),
        "expected kill-window failure in error chain, got: {msg}"
    );

    let calls = mock.recorded_calls();
    assert!(
        !calls
            .iter()
            .any(|c| c.0 == "git" && c.1.contains(&"remove".to_string())),
        "git worktree remove must not run after kill-window failure, got: {calls:?}"
    );
}

// --- teardown_task has_window runner error path ---

#[test]
fn teardown_task_has_window_runner_error_warns_and_continues() {
    // When runner.run() itself returns Err (e.g. tmux not installed), has_window
    // propagates the error to teardown_task, which logs a warning and continues
    // rather than aborting — worktree remove must still run.
    let mock = MockProcessRunner::new(vec![
        Err(anyhow::anyhow!("tmux not installed")), // has_window → runner error
        MockProcessRunner::ok(),                    // git worktree remove
        MockProcessRunner::ok(),                    // git branch -D (best-effort)
    ]);

    teardown_task(
        "/repo",
        Some("/repo/.worktrees/42-fix-bug"),
        Some(&test_tmux_window("task-42")),
        &mock,
    )
    .unwrap();

    let calls = mock.recorded_calls();
    assert!(
        !calls
            .iter()
            .any(|(prog, args)| prog == "tmux" && args.iter().any(|a| a == "kill-window")),
        "kill-window must not run when has_window errors, got: {calls:?}"
    );
    assert!(
        calls
            .iter()
            .any(|(prog, args)| prog == "git" && args.iter().any(|a| a == "remove")),
        "git worktree remove must still run after has_window error, got: {calls:?}"
    );
}

// ---------------------------------------------------------------------------
// branch_from_worktree — pure helper
// ---------------------------------------------------------------------------

#[test]
fn branch_from_worktree_returns_last_path_component() {
    assert_eq!(
        branch_from_worktree("/repo/.worktrees/42-fix-bug"),
        Some("42-fix-bug".to_string())
    );
}

#[test]
fn branch_from_worktree_strips_trailing_slash() {
    assert_eq!(
        branch_from_worktree("/repo/.worktrees/42-fix-bug/"),
        Some("42-fix-bug".to_string())
    );
}

#[test]
fn branch_from_worktree_returns_none_for_empty() {
    assert_eq!(branch_from_worktree(""), None);
}

#[test]
fn branch_from_worktree_returns_none_for_root() {
    assert_eq!(branch_from_worktree("/"), None);
}

// ---------------------------------------------------------------------------
// provision_worktree — timeout tests
// ---------------------------------------------------------------------------

#[test]
fn provision_worktree_kills_git_fetch_on_timeout_and_aborts() {
    // git fetch times out on every attempt → classified as unreachable
    // (never a 404), retried to exhaustion, then aborts. A worktree silently
    // based on a stale local ref is worse than a dispatch that refuses to
    // start, so no tmux window is ever created.
    let (_dir, repo_path) = make_test_repo();
    let short_timeout = Duration::from_millis(10);

    let mock = MockProcessRunner::new_with_delays(vec![
        (Some(Duration::from_millis(100)), MockProcessRunner::ok()), // git fetch attempt 1 → timeout (killed)
        (None, MockProcessRunner::ok()),                             // git remote get-url origin
        (None, MockProcessRunner::ok()), // git ls-remote --exit-code (unreachable: not code 2)
        (Some(Duration::from_millis(100)), MockProcessRunner::ok()), // git fetch attempt 2 → timeout (killed)
        (Some(Duration::from_millis(100)), MockProcessRunner::ok()), // git fetch attempt 3 → timeout (killed)
    ]);

    let task = make_task(&repo_path);
    let result = provision_worktree(&task, &mock, Some(BaseRef::Branch("main")), short_timeout);
    assert!(
        result.is_err(),
        "an unreachable origin must abort provisioning rather than fall back to local, got: {result:?}"
    );

    let calls = mock.recorded_calls();
    assert!(
        !calls.iter().any(|(prog, _)| prog == "tmux"),
        "aborting must happen before any tmux window is created, got: {calls:?}"
    );
}

#[test]
fn provision_worktree_kills_git_worktree_add_on_timeout() {
    // git worktree add times out → hard error (not soft-fail).
    // No base_branch → no git fetch; first runner call is git worktree add.
    // Before fix (using run): mock sleeps 100ms then succeeds → returns Ok().
    // After fix (using run_with_timeout): timeout error propagates → Err().
    let (_dir, repo_path) = make_test_repo();
    let short_timeout = Duration::from_millis(10);

    let mock = MockProcessRunner::new_with_delays(vec![(
        Some(Duration::from_millis(100)), // delay > short_timeout → timeout error
        MockProcessRunner::ok(),
    )]);

    let task = make_task(&repo_path);
    let result = provision_worktree(&task, &mock, None, short_timeout);
    assert!(
        result.is_err(),
        "expected error when git worktree add times out"
    );
    // anyhow error chain: use {:#} to traverse all causes
    let msg = format!("{:#}", result.unwrap_err());
    assert!(
        msg.contains("timed out") || msg.contains("killed"),
        "expected timeout in error chain, got: {msg}"
    );
}

// ---------------------------------------------------------------------------
// worktree-confinement invariant
//
// dispatch_with_prompt hard-codes the line
// "Always work from this worktree folder — do not `cd` to the parent repo
//  or other directories."
// into every prompt it writes. These tests drive the three public agent-spawn
// functions through a real `.claude-prompt` write (worktree dir pre-created so
// the write succeeds) and assert the invariant is present.
// ---------------------------------------------------------------------------

fn read_prompt(worktree_dir: &std::path::Path) -> String {
    std::fs::read_to_string(worktree_dir.join(".claude-prompt")).unwrap()
}

fn assert_worktree_confinement(prompt: &str) {
    assert!(
        prompt.contains("Always work from this worktree folder"),
        "prompt must include worktree-confinement instruction, got: {prompt}"
    );
    assert!(
        prompt.contains("do not `cd` to the parent repo"),
        "prompt must tell agent not to cd to parent repo, got: {prompt}"
    );
}

#[test]
fn dispatch_agent_prompt_includes_worktree_confinement() {
    let (_dir, repo_path, worktree_dir) = make_test_repo_with_worktree("42-fix-bug");
    let script = DispatchScript::dispatch();
    let mock = script.runner();
    let task = make_task(&repo_path);
    dispatch_agent(&task, &mock, None, &LearningInjections::default()).unwrap();
    assert_worktree_confinement(&read_prompt(&worktree_dir));
}

/// `DesignStepMatchesTheReposSpecs` (task #4409), end to end: the design step
/// in the written prompt follows what the *repo* holds, not a default. The
/// check reads the parent repo, so the worktree's own contents are irrelevant.
#[test]
fn dispatch_agent_prompt_design_step_follows_the_repos_allium_specs() {
    let (_dir, repo_path, worktree_dir) = make_test_repo_with_worktree("42-fix-bug");
    let task = make_task(&repo_path);

    // A bare repo with no docs/specs: brainstorming, and no allium line.
    let script = DispatchScript::dispatch();
    let mock = script.runner();
    dispatch_agent(&task, &mock, None, &LearningInjections::default()).unwrap();
    let prompt = read_prompt(&worktree_dir);
    assert!(
        prompt.contains("superpowers:brainstorming"),
        "a repo with no specs should get the brainstorming design step, got: {prompt}"
    );
    assert!(
        !prompt.contains("allium:elicit"),
        "a repo with no specs must not be sent to elicit a spec, got: {prompt}"
    );
    assert!(
        !prompt.contains("source of truth"),
        "a repo with no specs must not be pointed at docs/specs/, got: {prompt}"
    );

    // Same repo, now keeping a spec: the spec-first sequence, no brainstorming.
    let spec_dir = std::path::Path::new(&repo_path).join("docs/specs");
    std::fs::create_dir_all(&spec_dir).unwrap();
    std::fs::write(spec_dir.join("domain.allium"), "-- allium: 3\n").unwrap();

    let script = DispatchScript::dispatch();
    let mock = script.runner();
    dispatch_agent(&task, &mock, None, &LearningInjections::default()).unwrap();
    let prompt = read_prompt(&worktree_dir);
    assert!(
        prompt.contains("allium:elicit"),
        "a spec-keeping repo should get the spec-first sequence, got: {prompt}"
    );
    assert!(
        !prompt.contains("brainstorming"),
        "a spec-keeping repo must not name brainstorming, got: {prompt}"
    );
}

/// Quick dispatch shares the branch — it is the other prompt that names a
/// design step.
#[test]
fn quick_dispatch_agent_prompt_design_step_follows_the_repos_allium_specs() {
    let (_dir, repo_path, worktree_dir) = make_test_repo_with_worktree("42-fix-bug");
    let script = DispatchScript::dispatch();
    let mock = script.runner();
    let task = make_task(&repo_path);

    quick_dispatch_agent(&task, &mock, None, &LearningInjections::default()).unwrap();

    let prompt = read_prompt(&worktree_dir);
    assert!(
        prompt.contains("superpowers:brainstorming"),
        "quick dispatch into a spec-less repo should brainstorm, got: {prompt}"
    );
}

#[test]
fn research_agent_prompt_is_correct() {
    let (_dir, repo_path, worktree_dir) = make_test_repo_with_worktree("42-fix-bug");
    let script = DispatchScript::dispatch();
    let mock = script.runner();
    let task = make_task(&repo_path);
    research_agent(&task, &mock, None).unwrap();
    let prompt = read_prompt(&worktree_dir);
    assert_worktree_confinement(&prompt);
    assert!(
        prompt.contains("research agent"),
        "research_agent prompt should identify as a research agent, got: {prompt}"
    );
    assert!(
        prompt.contains("Do NOT make code changes"),
        "research_agent prompt must forbid code changes, got: {prompt}"
    );
}

#[test]
fn quick_dispatch_agent_prompt_includes_worktree_confinement() {
    let (_dir, repo_path, worktree_dir) = make_test_repo_with_worktree("42-fix-bug");
    let script = DispatchScript::dispatch();
    let mock = script.runner();
    let task = make_task(&repo_path);
    quick_dispatch_agent(&task, &mock, None, &LearningInjections::default()).unwrap();
    assert_worktree_confinement(&read_prompt(&worktree_dir));
}

// ---------------------------------------------------------------------------
// dispatch_agent — worktree confinement (behavior-first)
//
// The prompt-text confinement tests above cover the *instruction* to the agent.
// These assert the *mechanism*: the tmux window the agent runs in is opened with
// its working directory set to the task's worktree (under `.worktrees/`), never
// the bare parent repo. This is the worktree-escape guarantee CLAUDE.md notes
// has no test.
// ---------------------------------------------------------------------------

#[test]
fn dispatch_agent_opens_tmux_window_in_worktree_not_parent_repo() {
    let (_dir, repo_path, _worktree_dir) = make_test_repo_with_worktree("42-fix-bug");
    let script = DispatchScript::dispatch();
    let mock = script.runner();
    let task = make_task(&repo_path);
    dispatch_agent(&task, &mock, None, &LearningInjections::default()).unwrap();

    let calls = mock.recorded_calls();
    // Call 2 is `tmux new-window …` (call 0 is the git fetch, call 1 the
    // rev-list); its `-c <dir>` argument sets the window cwd.
    assert_eq!(calls[2].0, "tmux");
    assert_eq!(calls[2].1[0], "new-window");
    let c_pos = calls[2]
        .1
        .iter()
        .position(|a| a == "-c")
        .expect("new-window should pass -c <working_dir>");
    let cwd = &calls[2].1[c_pos + 1];
    // Pinning the exact worktree path both proves the window opens *inside* the
    // worktree and (transitively) that it is not the bare parent repo — the
    // worktree-escape guarantee this test exists to lock down.
    let expected_worktree = format!("{repo_path}/.worktrees/42-fix-bug");
    assert_eq!(
        cwd, &expected_worktree,
        "agent tmux window must open inside the task worktree (never the bare parent repo {repo_path}), got cwd: {cwd}"
    );
}

// ---------------------------------------------------------------------------
// dispatch_agent — tmux spawn failure paths
//
// `dispatch_fails_fast_if_git_fails` covers the git-worktree-add failure. These
// cover the two remaining tmux spawn steps: creating the window and sending the
// launch keys. Both must propagate an error (not silently succeed) with context.
// ---------------------------------------------------------------------------

#[test]
fn dispatch_agent_propagates_tmux_new_window_failure() {
    let (_dir, repo_path, _worktree_dir) = make_test_repo_with_worktree("42-fix-bug");
    let script = DispatchScript::dispatch().fails_at(Step::NewWindow);
    // The script's own queue ends at the NewWindow failure; the rollback that
    // failure now triggers still checks (and would kill) the window it just
    // tried to open, so one more response is needed for that check.
    let mut responses = script.responses();
    responses.push((None, MockProcessRunner::ok()));
    let mock = MockProcessRunner::new_with_delays(responses);
    let task = make_task(&repo_path);
    let result = dispatch_agent(&task, &mock, None, &LearningInjections::default());

    assert!(
        result.is_err(),
        "dispatch should propagate tmux new-window failure"
    );
    let msg = format!("{:#}", result.unwrap_err());
    assert!(
        msg.contains("failed to create tmux window"),
        "expected new-window context in error chain, got: {msg}"
    );
    let calls = mock.recorded_calls();
    assert!(
        worktree_remove_call(&calls).is_none(),
        "a reused (pre-existing) worktree must never be removed on failure, got: {calls:?}"
    );
}

#[test]
fn dispatch_agent_propagates_send_keys_failure() {
    let (_dir, repo_path, _worktree_dir) = make_test_repo_with_worktree("42-fix-bug");
    let script = DispatchScript::dispatch().fails_at(Step::SendKeysLiteral);
    // See dispatch_agent_propagates_tmux_new_window_failure: the rollback the
    // send-keys failure now triggers checks the (already-open) tmux window,
    // one call the script's own queue doesn't otherwise account for.
    let mut responses = script.responses();
    responses.push((None, MockProcessRunner::ok()));
    let mock = MockProcessRunner::new_with_delays(responses);
    let task = make_task(&repo_path);
    let result = dispatch_agent(&task, &mock, None, &LearningInjections::default());

    assert!(
        result.is_err(),
        "dispatch should propagate send-keys failure"
    );
    let msg = format!("{:#}", result.unwrap_err());
    assert!(
        msg.contains("failed to send keys to tmux window"),
        "expected send-keys context in error chain, got: {msg}"
    );
    let calls = mock.recorded_calls();
    assert!(
        worktree_remove_call(&calls).is_none(),
        "a reused (pre-existing) worktree must never be removed on failure, got: {calls:?}"
    );
}

// A fresh (not pre-existing) worktree whose provisioning fully succeeds but
// whose post-provisioning `.claude-prompt` write then fails, because the
// mock never actually created the directory `git worktree add` claims to
// have created — see KB #351. That mismatch is exactly the "fresh path,
// later step fails" scenario the rollback exists for, and it falls out of
// the mock naturally: no faking required.
#[test]
fn dispatch_agent_rolls_back_a_fresh_worktree_when_the_prompt_write_fails() {
    let (_dir, repo_path) = make_test_repo();
    let mock = DispatchScript::dispatch().fresh_worktree().runner();
    let task = make_task(&repo_path);
    let result = dispatch_agent(&task, &mock, None, &LearningInjections::default());

    assert!(
        result.is_err(),
        "the prompt write should fail against a directory that was never really created"
    );
    let calls = mock.recorded_calls();
    assert!(
        worktree_remove_call(&calls).is_some(),
        "a fresh worktree must be rolled back when a later step fails, got: {calls:?}"
    );
}

// ---------------------------------------------------------------------------
// resume_agent — failure path
// ---------------------------------------------------------------------------

#[test]
fn resume_agent_propagates_new_window_failure() {
    // First call (tmux list-windows, has_window check) reports no window
    // alive; second call (tmux new-window) fails — error should bubble up.
    let mock = MockProcessRunner::new(vec![
        MockProcessRunner::ok(),
        MockProcessRunner::fail("no server running on /tmp/tmux-1000/default"),
    ]);

    let result = resume_agent(TaskId(42), "/repo/.worktrees/42-fix-bug", &mock);

    assert!(
        result.is_err(),
        "resume_agent should propagate new-window failure"
    );
    let msg = format!("{:#}", result.unwrap_err());
    assert!(
        msg.contains("failed to create tmux window for resume"),
        "expected resume context in error chain, got: {msg}"
    );
}

#[test]
fn resume_agent_reattaches_to_live_window_without_creating_duplicate() {
    let script = DispatchScript::resume().window_already_alive();
    let mock = script.runner();

    let result = resume_agent(TaskId(42), "/repo/.worktrees/42-fix-bug", &mock);
    assert!(
        result.is_ok(),
        "resume_agent should succeed by reattaching, not erroring: {result:?}"
    );
    assert_eq!(result.unwrap().tmux_window, test_tmux_window("task-42"));

    script.assert_matches(&mock.recorded_calls());
}

#[test]
fn resume_agent_has_window_runner_error_falls_back_to_creating_a_window() {
    // When runner.run() itself returns Err (e.g. tmux not installed), has_window
    // propagates the error and resume_agent falls back to its pre-existing
    // unconditional-create behaviour rather than treating the task as reattached.
    let mock = MockProcessRunner::new(vec![
        Err(anyhow::anyhow!("tmux not installed")), // has_window → runner error
        MockProcessRunner::ok(),                    // tmux new-window
        MockProcessRunner::ok(),                    // tmux set-option @dispatch_dir
        MockProcessRunner::ok(),                    // tmux set-hook
        MockProcessRunner::ok(),                    // tmux send-keys -l
        MockProcessRunner::ok(),                    // tmux send-keys Enter
        MockProcessRunner::ok_with_stdout(COMPANION_PANE_ID), // tmux split-window
        MockProcessRunner::ok(),                    // tmux set-option (companion role)
    ]);

    let result = resume_agent(TaskId(42), "/repo/.worktrees/42-fix-bug", &mock);
    assert!(
        result.is_ok(),
        "a has_window query failure should fall back to creating a new window: {result:?}"
    );
}

// ---------------------------------------------------------------------------
// toggle_agent_tree_pane
// ---------------------------------------------------------------------------

#[test]
fn toggle_agent_tree_pane_is_noop_for_non_task_window() {
    let mock = MockProcessRunner::new(vec![]);
    toggle_agent_tree_pane(&test_tmux_window("TUI"), &mock).unwrap();
    assert_eq!(mock.recorded_calls().len(), 0, "should issue no tmux calls");
}

#[test]
fn toggle_agent_tree_pane_hides_when_companion_pane_present() {
    // The companion pane's id deliberately doesn't look like a positional
    // index, proving the kill target comes from the discovered pane id, not
    // an assumed index.
    let mock = MockProcessRunner::new(vec![
        MockProcessRunner::ok_with_stdout(b"%3 \n%77 agent_tree\n"), // list-panes
        MockProcessRunner::ok(),                                     // kill-pane
    ]);
    toggle_agent_tree_pane(&test_tmux_window("task-42"), &mock).unwrap();
    let calls = mock.recorded_calls();
    assert_eq!(calls.len(), 2);
    assert_eq!(calls[1].1, vec!["kill-pane", "-t", "%77"]);
}

#[test]
fn toggle_agent_tree_pane_shows_when_no_companion_pane() {
    let mock = MockProcessRunner::new(vec![
        MockProcessRunner::ok_with_stdout(b"%3 \n"), // list-panes: single pane, unmarked
        MockProcessRunner::ok_with_stdout(b"/wt\n"), // show-options @dispatch_dir
        MockProcessRunner::ok_with_stdout(COMPANION_PANE_ID), // split-window
        MockProcessRunner::ok(),                     // set-option: the role marker
    ]);
    toggle_agent_tree_pane(&test_tmux_window("task-42"), &mock).unwrap();
    let calls = mock.recorded_calls();
    assert_eq!(calls.len(), 4);
    assert_eq!(calls[2].0, "tmux");
    assert_eq!(calls[2].1[0], "split-window");
    assert!(
        calls[2].1.iter().any(|a| a == "30%"),
        "companion pane should use the 30% size, got: {:?}",
        calls[2].1
    );
    assert_eq!(
        calls[2].1[calls[2].1.len() - 3..],
        vec![
            "dispatch".to_string(),
            "agent-tree".to_string(),
            "42".to_string(),
        ],
        "companion pane should run `dispatch agent-tree <task_id>`, got: {:?}",
        calls[2].1
    );
    // The toggle holds only a window name, so it recovers the worktree from
    // @dispatch_dir and names it as the pane's start directory — which is what
    // keeps the correction hook from respawning the pane it just created.
    assert!(
        calls[2].1.windows(2).any(|w| w == ["-c", "/wt"]),
        "expected -c /wt in: {:?}",
        calls[2].1
    );
}

#[test]
fn toggle_agent_tree_pane_propagates_list_panes_query_failure() {
    let mock = MockProcessRunner::new(vec![MockProcessRunner::fail("no such window")]);
    let err = toggle_agent_tree_pane(&test_tmux_window("task-42"), &mock).unwrap_err();
    assert!(err.to_string().contains("list-panes failed"), "got: {err}");
}

/// Assert one spawn site's `tmux send-keys` payload carries both spawn flags.
///
/// Asserting on the *payload* rather than on `DISPATCH_PLUGIN_DIR` is the point:
/// a new spawn site that built its `claude` command line without interpolating
/// the constant would satisfy any assertion about the constant itself.
fn assert_spawn_flags(site: &str, payload: &str) {
    assert!(
        payload.contains("--plugin-dir ~/.claude/plugins/local/dispatch"),
        "{site} must spawn claude with the dispatch plugin dir, got: {payload}"
    );
    assert!(
        payload.contains("--settings ~/.claude/dispatch-statusline.json"),
        "{site} must spawn claude with the statusline settings overlay, got: {payload}"
    );
}

#[test]
fn all_spawn_sites_inject_the_statusline_settings_file() {
    // Every dispatch-spawned session must report budget windows, so the
    // --settings overlay has to be on the agent and resume command lines
    // alike. See docs/specs/dispatch.allium: TokenBudgetIndicator
    // and StatusLineDecorator. `claude` also refuses to start at all when that
    // settings file is absent, so a site that dropped the flag would spawn a
    // session with no budget reporting, and one that kept it while the file went
    // missing would spawn no session at all.
    //
    // These two are the whole set: `DISPATCH_PLUGIN_DIR` is interpolated in
    // exactly two places in src/dispatch/agents.rs — dispatch_with_prompt (all
    // agent dispatches funnel through it) and resume_agent.

    // 1. dispatch_agent -> dispatch_with_prompt
    let (_dir, repo_path, _worktree_dir) = make_test_repo_with_worktree("42-fix-bug");
    // Scripted: this vector previously omitted the `rev-list` that
    // `select_start_point` issues on the reuse path, so every later response was
    // consumed one step early and the split-window call ran off the end.
    let script = DispatchScript::dispatch();
    let mock = script.runner();
    let task = make_task(&repo_path);
    dispatch_agent(&task, &mock, None, &LearningInjections::default()).unwrap();
    assert_spawn_flags(
        "dispatch_agent",
        &find_call_arg(
            &mock.recorded_calls(),
            script.index_of(Step::SendKeysLiteral),
            "claude",
        ),
    );

    // 2. resume_agent
    let (_resume_dir, worktree_path) = make_test_repo();
    let resume_script = DispatchScript::resume();
    let mock = resume_script.runner();
    resume_agent(TaskId(42), &worktree_path, &mock).unwrap();
    assert_spawn_flags(
        "resume_agent",
        &find_call_arg(
            &mock.recorded_calls(),
            resume_script.index_of(Step::SendKeysLiteral),
            "claude",
        ),
    );
}

#[test]
fn spawn_constant_has_exactly_one_space_between_flags() {
    // A substring `.contains()` check only anchors on what comes AFTER
    // "--settings", so it can't catch a doubled space, a missing space, or a
    // reordering before that point — exactly the whitespace-swallowing hazard
    // a Rust string-literal line-continuation (`\`) can silently introduce.
    // Assert the full exact value so any such regression fails here instead
    // of shipping a broken `claude` command line to `tmux send-keys`.
    assert_eq!(
        crate::dispatch::prompts::DISPATCH_PLUGIN_DIR,
        "--plugin-dir ~/.claude/plugins/local/dispatch --settings ~/.claude/dispatch-statusline.json",
        "spawn constant must be exactly this string, with exactly one space between \
         the --plugin-dir and --settings flags — a doubled/missing space here breaks \
         argument splitting on the `claude` command line sent through tmux send-keys"
    );
}

#[test]
fn spawn_constant_contains_no_whitespace_hazard() {
    // The constant is interpolated into a shell command string sent through
    // tmux send_keys, so it must contain only fixed literal paths. A runtime
    // path here would break on any $HOME containing a space.
    for token in crate::dispatch::prompts::DISPATCH_PLUGIN_DIR.split_whitespace() {
        assert!(
            !token.contains('$'),
            "no runtime interpolation allowed in the spawn constant: {token}"
        );
    }
}

// ---------------------------------------------------------------------------
// Caller identity at the launch — AgentCarriesItsOwnCallerIdentity
// (docs/specs/dispatch.allium)
//
// The headersHelper cannot identify a dispatched agent: Claude Code runs a
// user-global helper from its own config directory, so `dispatch caller-headers`
// never sees the worktree and answers "session" every time. The launcher says
// it instead, through a per-task MCP config named on the `claude` command line.
// These assert the launch carries it; the file's own contents and placement are
// covered in `super::caller_identity`.
// ---------------------------------------------------------------------------

#[test]
fn dispatch_launches_claude_with_the_per_task_mcp_config() {
    let (dir, repo_path, _worktree_dir) = make_test_repo_with_worktree("42-fix-bug");
    let (_worktree, admin) = make_linked_worktree(dir.path(), "42-fix-bug");
    let script = DispatchScript::dispatch();
    let mock = script
        .runner()
        .with_claude_json(claude_json_with_dispatch_entry(dir.path()));
    let task = make_task(&repo_path);

    dispatch_agent(&task, &mock, None, &LearningInjections::default()).unwrap();

    let calls = mock.recorded_calls();
    let launch = find_call_arg(&calls, script.index_of(Step::SendKeysLiteral), "claude");
    assert!(
        launch.contains(&format!(
            "--mcp-config {}",
            admin.join("dispatch-mcp.json").display()
        )),
        "the launch must name the task's own MCP config, got: {launch}"
    );
    // Asserted on the composed command, not only on the flag builder: strict
    // mode could be added to this format string without the unit test noticing.
    assert!(
        !launch.contains("--strict-mcp-config"),
        "strict mode strips every other MCP server the operator configured, got: {launch}"
    );
}

#[test]
fn resume_launches_claude_with_the_per_task_mcp_config() {
    // Resume is the one launch path that never provisions, so it writes the
    // config itself rather than inheriting one.
    let dir = tempfile::TempDir::new().unwrap();
    let (worktree_path, admin) = make_linked_worktree(dir.path(), "42-fix-bug");
    let script = DispatchScript::resume();
    let mock = script
        .runner()
        .with_claude_json(claude_json_with_dispatch_entry(dir.path()));

    resume_agent(TaskId(42), &worktree_path, &mock).unwrap();

    let calls = mock.recorded_calls();
    let launch = find_call_arg(&calls, script.index_of(Step::SendKeysLiteral), "claude");
    assert!(
        launch.contains(&format!(
            "--mcp-config {}",
            admin.join("dispatch-mcp.json").display()
        )),
        "resume must carry caller identity too, got: {launch}"
    );
}

#[test]
fn a_dispatch_that_cannot_write_the_config_launches_without_the_flag() {
    // Degrading to the old behaviour (no caller identity) is acceptable.
    // Naming a file that does not exist is not — it breaks the launch itself.
    let (_dir, repo_path, _worktree_dir) = make_test_repo_with_worktree("42-fix-bug");
    let script = DispatchScript::dispatch();
    let mock = script.runner();
    let task = make_task(&repo_path);

    dispatch_agent(&task, &mock, None, &LearningInjections::default()).unwrap();

    let calls = mock.recorded_calls();
    let launch = find_call_arg(&calls, script.index_of(Step::SendKeysLiteral), "claude");
    assert!(
        !launch.contains("--mcp-config"),
        "no config written means no flag, got: {launch}"
    );
}

// ---------------------------------------------------------------------------
// The prompt is an operand, not a flag value —
// PromptIsSeparatedFromTheLaunchFlags (docs/specs/dispatch.allium)
// ---------------------------------------------------------------------------

/// Executed rather than string-compared, on the same terms as
/// `claude_quoted_survives_the_launcher_command_shape` in `src/process.rs`: the
/// hazard is in how a real shell splits this command and how a real CLI then
/// parses the argv it produces, and neither is visible in the format string.
///
/// `--mcp-config` is variadic (`<configs...>`) — it consumes every following
/// word up to the next `-`-prefixed one. `agent_launch_flags` ends with it, so
/// without the separator the prompt becomes a second MCP configuration, Claude
/// Code resolves it as a file path relative to the worktree, and the launch
/// fails before the agent ever starts.
#[test]
fn the_prompt_reaches_claude_as_an_operand_not_as_a_flag_value() {
    use std::os::unix::fs::PermissionsExt;

    let dir = tempfile::tempdir().unwrap();
    let stub = dir.path().join("claude-stub");
    std::fs::write(&stub, "#!/bin/sh\nfor a in \"$@\"; do echo \"$a\"; done\n").unwrap();
    std::fs::set_permissions(&stub, std::fs::Permissions::from_mode(0o755)).unwrap();
    let prompt = "This worktree was reused from a previous attempt";
    std::fs::write(dir.path().join(".claude-prompt"), prompt).unwrap();

    // The exact flag tail `agent_launch_flags` produces: the variadic one last.
    let cmd = prompt_launch_command(
        &crate::process::shell_quote(&stub.to_string_lossy()),
        "--name task-42 --mcp-config /tmp/dispatch-mcp.json",
    );
    let out = std::process::Command::new("bash")
        .arg("-c")
        .arg(&cmd)
        .current_dir(dir.path())
        .output()
        .unwrap();
    assert!(
        out.status.success(),
        "launcher command failed: {}",
        String::from_utf8_lossy(&out.stderr)
    );
    let stdout = String::from_utf8(out.stdout).unwrap();
    let argv: Vec<&str> = stdout.lines().collect();

    assert_eq!(
        argv,
        vec![
            "--name",
            "task-42",
            "--mcp-config",
            "/tmp/dispatch-mcp.json",
            "--",
            prompt,
        ],
        "the prompt must arrive as one operand behind a `--` separator, got: {argv:?}"
    );
}

/// The separator has to survive into the command the launch actually sends, not
/// just the builder: the format string in `dispatch_with_prompt` is where it
/// would be dropped.
#[test]
fn the_dispatch_launch_command_separates_the_prompt_from_the_flags() {
    let (_dir, repo_path, _worktree_dir) = make_test_repo_with_worktree("42-fix-bug");
    let script = DispatchScript::dispatch();
    let mock = script.runner();
    let task = make_task(&repo_path);

    dispatch_agent(&task, &mock, None, &LearningInjections::default()).unwrap();

    let calls = mock.recorded_calls();
    let launch = find_call_arg(&calls, script.index_of(Step::SendKeysLiteral), "claude");
    assert!(
        launch.contains(r#" -- "$prompt""#),
        "the prompt must be passed behind a `--` separator, got: {launch}"
    );
}
