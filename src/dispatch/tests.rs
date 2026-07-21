#![allow(clippy::unwrap_used, clippy::expect_used)]
use super::prompts::{
    allium_instruction, build_prompt, build_quick_dispatch_prompt, build_tmux_window_name,
    epic_preamble, mcp_tools_instruction, plan_and_attach_instruction, rebase_preamble, task_block,
    tdd_instruction, wrap_up_instruction, EpicContext, LearningInjections, PromptContext,
};
use super::worktree::provision_worktree;
use super::*;

use crate::models::{EpicId, Task, TaskId, TaskStatus};
use crate::process::{exit_fail, MockProcessRunner, SUBPROCESS_TIMEOUT};
use chrono::Utc;
use std::process::Output;
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
fn plan_and_attach_instruction_mentions_docs_plans_and_update_task() {
    let instr = plan_and_attach_instruction();
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

fn make_task(repo_path: &str) -> Task {
    Task {
        id: TaskId(42),
        title: "Fix bug".to_string(),
        description: "A nasty crash".to_string(),
        repo_path: repo_path.to_string(),
        status: TaskStatus::Backlog,
        worktree: None,
        tmux_window: None,
        plan_path: None,
        epic_id: None,
        sub_status: crate::models::SubStatus::None,
        url: None,
        tag: None,
        sort_order: None,
        base_branch: "main".into(),
        external_id: None,
        labels: Vec::new(),
        created_at: Utc::now(),
        updated_at: Utc::now(),
        last_pre_tool_use_at: None,
        last_notification_at: None,
        wrap_up_mode: None,
        auto_run_plan: false,
    }
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

fn make_test_repo_with_worktree(slug: &str) -> (tempfile::TempDir, String, std::path::PathBuf) {
    let (dir, repo_path) = make_test_repo();
    let worktree_dir = dir.path().join(".worktrees").join(slug);
    std::fs::create_dir_all(&worktree_dir).unwrap();
    (dir, repo_path, worktree_dir)
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
    // — no-plan agents may end by attaching a plan and need the same finalise
    // step (commit/finalise) as implementing agents.
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
        "no-plan prompt should mention /wrap-up (universal, covers plan-attach finish)"
    );
}

#[test]
fn build_prompt_without_plan_includes_planning_instruction() {
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
        "no-plan prompt should instruct agent to write a plan"
    );
    assert!(
        prompt.contains("update_task"),
        "no-plan prompt should instruct agent to attach plan via MCP"
    );
    assert!(
        prompt.contains("ask") || prompt.contains("permission") || prompt.contains("proceed"),
        "no-plan prompt should ask for permission before implementing"
    );
}

#[test]
fn build_prompt_without_plan_mentions_brainstorm_if_vague() {
    let prompt = build_prompt(
        TaskId(1),
        "Task",
        "Desc",
        None,
        None,
        &PromptContext::default(),
    );
    assert!(
        prompt.contains("/brainstorming"),
        "no-plan prompt should mention /brainstorming for vague descriptions"
    );
    assert!(
        prompt.contains("vague"),
        "no-plan prompt should mention vagueness as the condition for brainstorming"
    );
}

#[test]
fn build_prompt_without_plan_mentions_direct_plan_alternative() {
    let prompt = build_prompt(
        TaskId(1),
        "Task",
        "Desc",
        None,
        None,
        &PromptContext::default(),
    );
    assert!(
        prompt.contains("implementation plan directly"),
        "no-plan prompt should offer writing a plan directly for clear descriptions"
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
fn is_wrappable_running_with_worktree() {
    let task = Task {
        status: TaskStatus::Running,
        worktree: Some("/tmp/wt".to_string()),
        ..make_task("/repo")
    };
    assert!(is_wrappable(&task));
}

#[test]
fn is_wrappable_review_with_worktree() {
    let task = Task {
        status: TaskStatus::Review,
        worktree: Some("/tmp/wt".to_string()),
        ..make_task("/repo")
    };
    assert!(is_wrappable(&task));
}

#[test]
fn is_wrappable_running_without_worktree() {
    let task = Task {
        status: TaskStatus::Running,
        worktree: None,
        ..make_task("/repo")
    };
    assert!(!is_wrappable(&task));
}

#[test]
fn is_wrappable_backlog_with_worktree() {
    let task = Task {
        status: TaskStatus::Backlog,
        worktree: Some("/tmp/wt".to_string()),
        ..make_task("/repo")
    };
    assert!(!is_wrappable(&task));
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

#[test]
fn resume_window_name_matches_dispatch() {
    // The resume window name should use the same naming convention as dispatch
    assert_eq!(build_tmux_window_name(TaskId(42)), "task-42");
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
fn build_quick_dispatch_prompt_includes_planning_instruction() {
    let prompt = build_quick_dispatch_prompt(
        TaskId(42),
        "Quick task",
        "",
        None,
        &PromptContext::default(),
    );
    assert!(
        prompt.contains("docs/plans/") || prompt.contains("plan"),
        "quick dispatch prompt should instruct agent to write a plan before implementing"
    );
    assert!(
        prompt.contains("ask") || prompt.contains("permission") || prompt.contains("proceed"),
        "quick dispatch prompt should ask for permission before implementing"
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
        prompt.contains("send_message"),
        "should tell agent how to message sibling agents"
    );
}

#[test]
fn rebase_preamble_prepended_to_all_prompts() {
    let body = build_prompt(
        TaskId(1),
        "Task",
        "Desc",
        None,
        None,
        &PromptContext::default(),
    );
    let full = format!(
        "{}\n\n\
         Always work from this worktree folder — do not `cd` to the parent repo \
         or other directories.\n\n\
         {body}",
        rebase_preamble("origin/main")
    );
    assert!(full.contains("rebase your branch from origin/main"));
    assert!(full.starts_with("Before starting work"));
    assert!(full.contains("Always work from this worktree folder"));
}

#[test]
fn no_plan_prompts_reference_brainstorming_skill() {
    let standard = build_prompt(TaskId(1), "T", "D", None, None, &PromptContext::default());
    let quick = build_quick_dispatch_prompt(TaskId(1), "T", "D", None, &PromptContext::default());

    for (name, prompt) in [("standard-no-plan", standard), ("quick", quick)] {
        assert!(
            prompt.contains("/brainstorming"),
            "{name} prompt should reference /brainstorming skill"
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

#[test]
fn quick_dispatch_uses_unconditional_plan_and_attach_instruction() {
    let prompt =
        build_quick_dispatch_prompt(TaskId(1), "Quick task", "", None, &PromptContext::default());
    assert!(
        prompt.contains(plan_and_attach_instruction()),
        "quick-dispatch prompt should embed plan_and_attach_instruction verbatim"
    );
    assert!(
        !prompt.contains("vague or"),
        "quick-dispatch must not use the conditional plan_or_brainstorm wording"
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
        text.contains("attaching a plan")
            || text.contains("creating work packages")
            || text.contains("your work is done"),
        "wrap_up_instruction should describe the universal trigger (plan attach / work-packages / impl), got: {text}"
    );
}

#[test]
fn plan_and_attach_instruction_is_concise() {
    let instruction = plan_and_attach_instruction();
    assert!(
        instruction.len() < 200,
        "plan_and_attach_instruction should be concise (< 200 chars), got {} chars",
        instruction.len()
    );
    assert!(instruction.contains("/brainstorming"));
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
        section.contains("send_message"),
        "should guide agent to use send_message"
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

    let mock = MockProcessRunner::new(vec![
        // No detect_default_branch call — task.base_branch is used directly
        // git worktree add is skipped (dir exists)
        MockProcessRunner::ok(), // tmux new-window
        MockProcessRunner::ok(), // tmux set-option @dispatch_dir
        MockProcessRunner::ok(), // tmux set-hook (after-split-window)
        MockProcessRunner::ok(), // tmux send-keys -l
        MockProcessRunner::ok(), // tmux send-keys Enter
    ]);

    let task = make_task(&repo_path);
    dispatch_agent(&task, &mock, None, &LearningInjections::default(), None).unwrap();

    let calls = mock.recorded_calls();
    assert!(
        calls
            .iter()
            .all(|(prog, args)| !(prog == "git" && args.iter().any(|a| a == "worktree"))),
        "git worktree add should be skipped for existing worktree"
    );
    assert_eq!(calls[0].0, "tmux");
    assert_eq!(calls[0].1[0], "new-window");
    assert_eq!(calls[1].0, "tmux");
    assert_eq!(calls[1].1[0], "set-option");
    assert_eq!(calls[2].0, "tmux");
    assert_eq!(calls[2].1[0], "set-hook");
}

#[test]
fn dispatch_sends_claude_command() {
    let (_dir, repo_path, _worktree_dir) = make_test_repo_with_worktree("42-fix-bug");

    let mock = MockProcessRunner::new(vec![
        // No detect_default_branch call — task.base_branch is used directly
        MockProcessRunner::ok(), // tmux new-window
        MockProcessRunner::ok(), // tmux set-option @dispatch_dir
        MockProcessRunner::ok(), // tmux set-hook (after-split-window)
        MockProcessRunner::ok(), // tmux send-keys -l (the claude command)
        MockProcessRunner::ok(), // tmux send-keys Enter
    ]);

    let task = make_task(&repo_path);
    dispatch_agent(&task, &mock, None, &LearningInjections::default(), None).unwrap();

    let calls = mock.recorded_calls();
    // The literal send-keys call (index 3) carries the claude invocation
    assert!(
        calls[3].1.iter().any(|a| a.contains("claude")),
        "send-keys should include claude"
    );
    assert!(
        !calls[3].1.iter().any(|a| a.contains("--permission-mode")),
        "dispatch_agent send-keys should omit --permission-mode (auto/default)"
    );
}

#[test]
fn dispatch_agent_uses_default_permission_mode() {
    let (_dir, repo_path, _worktree_dir) = make_test_repo_with_worktree("42-fix-bug");

    let mock = MockProcessRunner::new(vec![
        // No detect_default_branch call — task.base_branch is used directly
        MockProcessRunner::ok(), // tmux new-window
        MockProcessRunner::ok(), // tmux set-option @dispatch_dir
        MockProcessRunner::ok(), // tmux set-hook
        MockProcessRunner::ok(), // tmux send-keys -l
        MockProcessRunner::ok(), // tmux send-keys Enter
    ]);

    let task = make_task(&repo_path);
    dispatch_agent(&task, &mock, None, &LearningInjections::default(), None).unwrap();

    let calls = mock.recorded_calls();
    let send_keys_arg = find_call_arg(&calls, 3, "claude");
    assert!(
        !send_keys_arg.contains("--permission-mode"),
        "dispatch_agent should use default (auto) mode — no --permission-mode flag, got: {send_keys_arg}"
    );
}

#[test]
fn research_agent_uses_plan_permission_mode() {
    let (_dir, repo_path, _worktree_dir) = make_test_repo_with_worktree("42-fix-bug");

    let mock = MockProcessRunner::new(vec![
        MockProcessRunner::ok(), // tmux new-window
        MockProcessRunner::ok(), // tmux set-option @dispatch_dir
        MockProcessRunner::ok(), // tmux set-hook
        MockProcessRunner::ok(), // tmux send-keys -l
        MockProcessRunner::ok(), // tmux send-keys Enter
    ]);

    let task = make_task(&repo_path);
    research_agent(&task, &mock, None, None).unwrap();

    let calls = mock.recorded_calls();
    let send_keys_arg = find_call_arg(&calls, 3, "claude");
    assert!(
        send_keys_arg.contains("--permission-mode plan"),
        "research_agent should use --permission-mode plan to keep investigation read-only, got: {send_keys_arg}"
    );
}

#[test]
fn quick_dispatch_agent_uses_default_permission_mode() {
    let (_dir, repo_path, _worktree_dir) = make_test_repo_with_worktree("42-fix-bug");

    let mock = MockProcessRunner::new(vec![
        MockProcessRunner::ok(), // tmux new-window
        MockProcessRunner::ok(), // tmux set-option @dispatch_dir
        MockProcessRunner::ok(), // tmux set-hook
        MockProcessRunner::ok(), // tmux send-keys -l
        MockProcessRunner::ok(), // tmux send-keys Enter
    ]);

    let task = make_task(&repo_path);
    quick_dispatch_agent(&task, &mock, None, &LearningInjections::default(), None).unwrap();

    let calls = mock.recorded_calls();
    let send_keys_arg = find_call_arg(&calls, 3, "claude");
    assert!(
        !send_keys_arg.contains("--permission-mode"),
        "quick_dispatch_agent should use default (auto) mode — no --permission-mode flag, got: {send_keys_arg}"
    );
}

// --- PR-based review worktree start point ---
//
// The mock runner doesn't actually create the worktree dir, so the post-provision
// `.claude-prompt` write fails for a fresh repo. Start-point tests use a fresh
// repo and inspect the recorded `git` calls (captured before the write fails) —
// they tolerate the resulting error. The prompt-content test pre-creates the
// worktree dir so the write succeeds.

fn pr_review_task(repo_path: &str) -> Task {
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
    ]);

    let task = pr_review_task(&repo_path);
    // Prompt write fails (mock didn't create the worktree dir) — that's fine, the
    // git calls we assert on were recorded during provisioning beforehand.
    let _ = dispatch_agent(&task, &mock, None, &LearningInjections::default(), None);

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
fn dispatch_pr_review_task_prompt_rebases_onto_pr_branch() {
    let (_dir, repo_path, _worktree_dir) = make_test_repo_with_worktree("42-fix-bug");

    let mock = MockProcessRunner::new(vec![
        MockProcessRunner::ok_with_stdout(b"feature-x\nfalse\n"), // gh pr view
        MockProcessRunner::ok(),                                  // tmux new-window
        MockProcessRunner::ok(),                                  // tmux set-option
        MockProcessRunner::ok(),                                  // tmux set-hook
        MockProcessRunner::ok(),                                  // tmux send-keys -l
        MockProcessRunner::ok(),                                  // tmux send-keys Enter
    ]);

    let task = pr_review_task(&repo_path);
    dispatch_agent(&task, &mock, None, &LearningInjections::default(), None).unwrap();

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
fn dispatch_non_review_task_skips_gh_and_keeps_base_rebase() {
    let (_dir, repo_path) = make_test_repo();

    let mock = MockProcessRunner::new(vec![
        MockProcessRunner::ok(), // git fetch origin main
        MockProcessRunner::ok(), // git worktree add origin/main
        MockProcessRunner::ok(), // tmux new-window
        MockProcessRunner::ok(), // tmux set-option
        MockProcessRunner::ok(), // tmux set-hook
    ]);

    // make_task has tag None / url None — a plain implementation task.
    let task = make_task(&repo_path);
    let _ = dispatch_agent(&task, &mock, None, &LearningInjections::default(), None);

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

    let mock = MockProcessRunner::new(vec![
        MockProcessRunner::fail("gh: not authenticated"), // gh pr view fails
        MockProcessRunner::ok(),                          // git fetch origin main
        MockProcessRunner::ok(),                          // git worktree add origin/main
        MockProcessRunner::ok(),                          // tmux new-window
        MockProcessRunner::ok(),                          // tmux set-option
        MockProcessRunner::ok(),                          // tmux set-hook
    ]);

    let task = pr_review_task(&repo_path);
    let _ = dispatch_agent(&task, &mock, None, &LearningInjections::default(), None);

    assert_eq!(
        worktree_add_start_point(&mock.recorded_calls()),
        "origin/main",
        "should fall back to the base branch when PR resolution fails"
    );
}

#[test]
fn dispatch_review_task_fork_pr_falls_back_to_base() {
    let (_dir, repo_path) = make_test_repo();

    let mock = MockProcessRunner::new(vec![
        MockProcessRunner::ok_with_stdout(b"patch-1\ntrue\n"), // gh pr view: fork PR
        MockProcessRunner::ok(),                               // git fetch origin main
        MockProcessRunner::ok(),                               // git worktree add origin/main
        MockProcessRunner::ok(),                               // tmux new-window
        MockProcessRunner::ok(),                               // tmux set-option
        MockProcessRunner::ok(),                               // tmux set-hook
    ]);

    let task = pr_review_task(&repo_path);
    let _ = dispatch_agent(&task, &mock, None, &LearningInjections::default(), None);

    assert_eq!(
        worktree_add_start_point(&mock.recorded_calls()),
        "origin/main",
        "fork PR should fall back to the base branch"
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
fn provision_worktree_with_base_branch_passes_start_point() {
    let (_dir, repo_path) = make_test_repo();

    let mock = MockProcessRunner::new(vec![
        MockProcessRunner::ok(), // git fetch origin 99-prev-task
        MockProcessRunner::ok(), // git worktree add (with origin/99-prev-task)
        MockProcessRunner::ok(), // tmux new-window
        MockProcessRunner::ok(), // tmux set-option @dispatch_dir
        MockProcessRunner::ok(), // tmux set-hook (after-split-window)
    ]);

    let task = make_task(&repo_path);
    let result =
        provision_worktree(&task, &mock, Some("99-prev-task"), SUBPROCESS_TIMEOUT).unwrap();

    let calls = mock.recorded_calls();
    // call[0] = fetch
    assert_eq!(calls[0].0, "git");
    assert!(calls[0].1.contains(&"fetch".to_string()));
    assert!(calls[0].1.contains(&"99-prev-task".to_string()));
    // call[1] = worktree add — start point is now origin/<base>
    assert_eq!(calls[1].0, "git");
    let git_args = &calls[1].1;
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

    let mock = MockProcessRunner::new(vec![
        MockProcessRunner::ok(), // git fetch origin main
        MockProcessRunner::ok(), // git worktree add
        MockProcessRunner::ok(), // tmux new-window
        MockProcessRunner::ok(), // tmux set-option @dispatch_dir
        MockProcessRunner::ok(), // tmux set-hook (after-split-window)
    ]);

    let task = make_task(&repo_path);
    provision_worktree(&task, &mock, Some("main"), SUBPROCESS_TIMEOUT).unwrap();

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
    // call[1] = git worktree add ... origin/main
    assert_eq!(calls[1].0, "git");
    assert!(calls[1].1.contains(&"worktree".to_string()));
    assert_eq!(
        calls[1].1.last().unwrap(),
        "origin/main",
        "worktree add should use origin/main as start point, got: {:?}",
        calls[1].1
    );
}

#[test]
fn provision_worktree_fetch_failure_falls_back_to_local() {
    // Fetch fails → worktree add should use local branch (no error propagated)
    let (_dir, repo_path) = make_test_repo();

    let mock = MockProcessRunner::new(vec![
        MockProcessRunner::fail("fatal: 'origin' does not appear to be a git repository"),
        MockProcessRunner::ok(), // git worktree add
        MockProcessRunner::ok(), // tmux new-window
        MockProcessRunner::ok(), // tmux set-option @dispatch_dir
        MockProcessRunner::ok(), // tmux set-hook (after-split-window)
    ]);

    let task = make_task(&repo_path);
    // Should NOT return an error — soft fail
    provision_worktree(&task, &mock, Some("main"), SUBPROCESS_TIMEOUT).unwrap();

    let calls = mock.recorded_calls();
    // call[0] = fetch (failed)
    assert_eq!(calls[0].0, "git");
    assert!(calls[0].1.contains(&"fetch".to_string()));
    // call[1] = worktree add using local "main" (not "origin/main")
    assert_eq!(calls[1].0, "git");
    assert!(calls[1].1.contains(&"worktree".to_string()));
    assert_eq!(
        calls[1].1.last().unwrap(),
        "main",
        "fallback should use local main, got: {:?}",
        calls[1].1
    );
}

#[test]
fn provision_worktree_fetch_uses_custom_base_branch() {
    // Custom base_branch is used in both fetch and worktree add
    let (_dir, repo_path) = make_test_repo();

    let mock = MockProcessRunner::new(vec![
        MockProcessRunner::ok(), // git fetch origin develop
        MockProcessRunner::ok(), // git worktree add
        MockProcessRunner::ok(), // tmux new-window
        MockProcessRunner::ok(), // tmux set-option @dispatch_dir
        MockProcessRunner::ok(), // tmux set-hook (after-split-window)
    ]);

    let task = make_task(&repo_path);
    provision_worktree(&task, &mock, Some("develop"), SUBPROCESS_TIMEOUT).unwrap();

    let calls = mock.recorded_calls();
    assert!(
        calls[0].1.contains(&"develop".to_string()),
        "fetch should use 'develop', got: {:?}",
        calls[0].1
    );
    assert_eq!(
        calls[1].1.last().unwrap(),
        "origin/develop",
        "worktree add should use origin/develop, got: {:?}",
        calls[1].1
    );
}

#[test]
fn provision_worktree_skips_fetch_when_dir_exists() {
    // Pre-existing worktree dir → no git calls at all (fetch + worktree add both skipped)
    let (_dir, repo_path, _worktree_dir) = make_test_repo_with_worktree("42-fix-bug");

    let mock = MockProcessRunner::new(vec![
        MockProcessRunner::ok(), // tmux new-window
        MockProcessRunner::ok(), // tmux set-option @dispatch_dir
        MockProcessRunner::ok(), // tmux set-hook (after-split-window)
    ]);

    let task = make_task(&repo_path);
    provision_worktree(&task, &mock, Some("main"), SUBPROCESS_TIMEOUT).unwrap();

    let calls = mock.recorded_calls();
    assert!(
        calls.iter().all(|(prog, _)| prog != "git"),
        "no git calls expected when worktree dir already exists, got: {calls:?}"
    );
}

#[test]
fn rebase_preamble_with_base_branch() {
    let preamble = rebase_preamble("99-prev-task");
    assert!(
        preamble.contains("99-prev-task"),
        "should reference the base branch"
    );
    assert!(
        !preamble.contains("origin/main"),
        "should not reference origin/main"
    );
}

#[test]
fn rebase_preamble_uses_given_target() {
    let preamble = rebase_preamble("origin/develop");
    assert!(
        preamble.contains("origin/develop"),
        "should use given target, got: {preamble}"
    );
    assert!(
        !preamble.contains("origin/main"),
        "should not contain origin/main"
    );
}

#[test]
fn resume_skips_git_issues_tmux_continue() {
    let (_dir, worktree_path) = make_test_repo();

    let mock = MockProcessRunner::new(vec![
        MockProcessRunner::ok(), // tmux new-window
        MockProcessRunner::ok(), // tmux set-option @dispatch_dir
        MockProcessRunner::ok(), // tmux set-hook (after-split-window)
        MockProcessRunner::ok(), // tmux send-keys -l
        MockProcessRunner::ok(), // tmux send-keys Enter
    ]);

    resume_agent(TaskId(42), &worktree_path, &mock).unwrap();

    let calls = mock.recorded_calls();
    assert_eq!(calls.len(), 5);
    assert_eq!(calls[0].0, "tmux");
    assert_eq!(calls[0].1[0], "new-window");
    assert_eq!(calls[1].1[0], "set-option");
    assert_eq!(calls[2].1[0], "set-hook");
    assert!(
        calls.iter().all(|(prog, _)| prog != "git"),
        "resume should make no git calls"
    );
    assert!(calls[3].1.iter().any(|a| a.contains("--continue")));
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

    cleanup_task(
        "/repo",
        "/repo/.worktrees/42-fix-bug",
        Some("task-42"),
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

    cleanup_task("/repo", "/repo/.worktrees/42-fix-bug", None, &mock).unwrap();
}

#[test]
fn dispatch_uses_task_base_branch_in_prompt() {
    let (_dir, repo_path, worktree_dir) = make_test_repo_with_worktree("42-fix-bug");

    let mock = MockProcessRunner::new(vec![
        // No detect_default_branch call — task.base_branch is used directly
        // git worktree add is skipped (dir exists)
        MockProcessRunner::ok(), // tmux new-window
        MockProcessRunner::ok(), // tmux set-option @dispatch_dir
        MockProcessRunner::ok(), // tmux set-hook
        MockProcessRunner::ok(), // tmux send-keys -l
        MockProcessRunner::ok(), // tmux send-keys Enter
    ]);

    let mut task = make_task(&repo_path);
    task.base_branch = "master".into();
    dispatch_agent(&task, &mock, None, &LearningInjections::default(), None).unwrap();

    // Verify the prompt uses task.base_branch directly — no symbolic-ref call needed
    let prompt_file = worktree_dir.join(".claude-prompt");
    let prompt = std::fs::read_to_string(prompt_file).unwrap();
    assert!(
        prompt.contains("rebase your branch from master"),
        "prompt should reference task.base_branch (master), got: {prompt}"
    );
    assert!(
        !prompt.contains("rebase your branch from main"),
        "prompt should not reference main when task.base_branch is master"
    );
}

#[test]
fn dispatch_fails_fast_if_git_fails() {
    let (_dir, repo_path) = make_test_repo();

    let mock = MockProcessRunner::new(vec![
        MockProcessRunner::ok(),                   // git fetch origin main (succeeds)
        MockProcessRunner::fail("not a git repo"), // git worktree add fails
    ]);

    let task = make_task(&repo_path);
    let result = dispatch_agent(&task, &mock, None, &LearningInjections::default(), None);
    assert!(result.is_err());
    let calls = mock.recorded_calls();
    assert_eq!(
        calls.len(),
        2,
        "only git fetch + git worktree add should have been called (no detect_default_branch)"
    );
}

#[test]
fn quick_dispatch_reuses_existing_worktree() {
    let (_dir, repo_path, _worktree_dir) = make_test_repo_with_worktree("42-fix-bug");

    let mock = MockProcessRunner::new(vec![
        // No detect_default_branch call — task.base_branch is used directly
        // git worktree add is skipped (dir exists)
        MockProcessRunner::ok(), // tmux new-window
        MockProcessRunner::ok(), // tmux set-option @dispatch_dir
        MockProcessRunner::ok(), // tmux set-hook (after-split-window)
        MockProcessRunner::ok(), // tmux send-keys -l
        MockProcessRunner::ok(), // tmux send-keys Enter
    ]);

    let task = make_task(&repo_path);
    quick_dispatch_agent(&task, &mock, None, &LearningInjections::default(), None).unwrap();

    let calls = mock.recorded_calls();
    assert!(
        calls
            .iter()
            .all(|(prog, args)| !(prog == "git" && args.iter().any(|a| a == "worktree"))),
        "git worktree add should be skipped for existing worktree"
    );
    assert_eq!(calls[0].0, "tmux");
    assert_eq!(calls[0].1[0], "new-window");
}

#[test]
fn quick_dispatch_sends_rename_prompt() {
    let (_dir, repo_path, worktree_dir) = make_test_repo_with_worktree("42-fix-bug");

    let mock = MockProcessRunner::new(vec![
        // No detect_default_branch call — task.base_branch is used directly
        MockProcessRunner::ok(), // tmux new-window
        MockProcessRunner::ok(), // tmux set-option @dispatch_dir
        MockProcessRunner::ok(), // tmux set-hook (after-split-window)
        MockProcessRunner::ok(), // tmux send-keys -l
        MockProcessRunner::ok(), // tmux send-keys Enter
    ]);

    let task = make_task(&repo_path);
    quick_dispatch_agent(&task, &mock, None, &LearningInjections::default(), None).unwrap();

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
    let mock = MockProcessRunner::new(vec![MockProcessRunner::ok_with_stdout(
        b"renovate/serde-1.x\nfalse\n",
    )]);
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
    let mock = MockProcessRunner::new(vec![MockProcessRunner::ok_with_stdout(b"patch-1\ntrue\n")]);
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

// --- finish_task tests ---

/// Build a `FinishContext` with the standard test repo/worktree/branch, varying
/// only the fields the individual tests care about.
fn fctx<'a>(base_branch: &'a str, tmux_window: Option<&'a str>) -> FinishContext<'a> {
    FinishContext {
        repo_path: "/repo",
        worktree: "/repo/.worktrees/42-fix-bug",
        branch: "42-fix-bug",
        base_branch,
        tmux_window,
    }
}

#[test]
fn finish_task_happy_path() {
    let mock = MockProcessRunner::new(vec![
        MockProcessRunner::ok_with_stdout(b"main\n"), // rev-parse HEAD
        MockProcessRunner::ok_with_stdout(b"git@github.com:org/repo.git\n"), // remote get-url origin
        MockProcessRunner::ok(),                                             // git pull origin main
        MockProcessRunner::ok(), // git rebase main (from worktree)
        MockProcessRunner::ok(), // git merge --ff-only (fast-forward main)
        // Only tmux kill (worktree preserved for archival):
        MockProcessRunner::ok_with_stdout(b"task-42\n"), // tmux list-windows (has_window)
        MockProcessRunner::ok(),                         // tmux kill-window
    ]);

    finish_task(&fctx("main", Some("task-42")), &mock).unwrap();

    let calls = mock.recorded_calls();
    assert!(calls.iter().any(|c| c.1.contains(&"rebase".to_string())));
    assert!(calls.iter().any(|c| c.1.contains(&"--ff-only".to_string())));
    // No worktree removal
    assert!(!calls.iter().any(|c| c.1.contains(&"remove".to_string())));
}

#[test]
fn finish_task_with_master_default_branch() {
    let mock = MockProcessRunner::new(vec![
        MockProcessRunner::ok_with_stdout(b"master\n"), // rev-parse HEAD
        MockProcessRunner::ok_with_stdout(b"git@github.com:org/repo.git\n"), // remote get-url origin
        MockProcessRunner::ok(), // git pull origin master
        MockProcessRunner::ok(), // git rebase master (from worktree)
        MockProcessRunner::ok(), // git merge --ff-only
    ]);

    finish_task(&fctx("master", None), &mock).unwrap();

    let calls = mock.recorded_calls();
    // pull should reference "master" not "main"
    let pull_call = calls
        .iter()
        .find(|c| c.1.contains(&"pull".to_string()))
        .unwrap();
    assert!(pull_call.1.contains(&"master".to_string()));
    // rebase should reference "master"
    let rebase_call = calls
        .iter()
        .find(|c| c.1.contains(&"rebase".to_string()))
        .unwrap();
    assert!(rebase_call.1.contains(&"master".to_string()));
}

#[test]
fn finish_task_not_on_default_branch() {
    let mock = MockProcessRunner::new(vec![
        MockProcessRunner::ok_with_stdout(b"feature-branch\n"), // rev-parse HEAD
    ]);

    let result = finish_task(&fctx("main", None), &mock);
    assert!(result.is_err());
    let err = result.unwrap_err();
    assert!(matches!(err, FinishError::NotOnDefaultBranch { .. }));
    assert!(err.to_string().contains("feature-branch"));
}

#[test]
fn finish_task_rebase_conflict() {
    let mock = MockProcessRunner::new(vec![
        MockProcessRunner::ok_with_stdout(b"main\n"),
        MockProcessRunner::fail(""),                         // remote get-url (no remote)
        Ok(Output {
            status: exit_fail(),
            stdout: b"".to_vec(),
            stderr: b"CONFLICT (content): Merge conflict in src/main.rs\nerror: could not apply abc1234\n".to_vec(),
        }),
        MockProcessRunner::ok(),                             // git rebase --abort
    ]);

    let result = finish_task(&fctx("main", None), &mock);
    assert!(matches!(
        result.unwrap_err(),
        FinishError::RebaseConflict(_)
    ));
    let calls = mock.recorded_calls();
    assert!(calls.last().unwrap().1.contains(&"--abort".to_string()));
}

#[test]
fn finish_task_pull_fails() {
    let mock = MockProcessRunner::new(vec![
        MockProcessRunner::ok_with_stdout(b"main\n"),
        MockProcessRunner::ok_with_stdout(b"git@github.com:org/repo.git\n"), // remote get-url origin
        MockProcessRunner::fail("fatal: unable to access remote"),           // git pull fails
    ]);

    let result = finish_task(&fctx("main", None), &mock);
    assert!(matches!(result.unwrap_err(), FinishError::Other(_)));
}

// --- repo_name_from_path tests ---

#[test]
fn repo_name_from_path_uses_basename() {
    assert_eq!(super::repo_name_from_path("/home/u/dispatch"), "dispatch");
    assert_eq!(super::repo_name_from_path("/home/u/dispatch/"), "dispatch");
    assert_eq!(super::repo_name_from_path(""), super::UNKNOWN_REPO_GROUP);
    assert_eq!(super::repo_name_from_path("/"), super::UNKNOWN_REPO_GROUP);
}

// --- repo_name_from_url tests ---

#[test]
fn repo_name_from_url_extracts_last_segment() {
    assert_eq!(
        super::repo_name_from_url("https://github.com/org/my-repo/pull/42"),
        "my-repo"
    );
    assert_eq!(
        super::repo_name_from_url("https://github.com/org/my-repo"),
        "my-repo"
    );
    assert_eq!(
        super::repo_name_from_url("https://github.com/org/another-repo/issues/1"),
        "another-repo"
    );
}

#[test]
fn repo_name_from_url_returns_other_for_non_github() {
    assert_eq!(super::repo_name_from_url(""), "other");
    assert_eq!(
        super::repo_name_from_url("https://example.com/not-github"),
        "other"
    );
    assert_eq!(
        super::repo_name_from_url("https://gitlab.com/org/repo"),
        "other"
    );
}

// --- extract_github_repo tests ---

#[test]
fn extract_github_repo_pr_url() {
    assert_eq!(
        extract_github_repo("https://github.com/org/repo/pull/42"),
        Some("org/repo"),
    );
}

#[test]
fn extract_github_repo_issue_url() {
    assert_eq!(
        extract_github_repo("https://github.com/org/repo/issues/5"),
        Some("org/repo"),
    );
}

#[test]
fn extract_github_repo_root_url() {
    assert_eq!(
        extract_github_repo("https://github.com/org/repo"),
        Some("org/repo"),
    );
}

#[test]
fn extract_github_repo_root_url_with_trailing_slash() {
    assert_eq!(
        extract_github_repo("https://github.com/org/repo/"),
        Some("org/repo"),
    );
}

#[test]
fn extract_github_repo_tree_url() {
    assert_eq!(
        extract_github_repo("https://github.com/org/repo/tree/main"),
        Some("org/repo"),
    );
}

#[test]
fn extract_github_repo_non_github_url() {
    assert_eq!(
        extract_github_repo("https://jira.company.com/browse/PROJ-123"),
        None
    );
}

#[test]
fn extract_github_repo_empty_string() {
    assert_eq!(extract_github_repo(""), None);
}

#[test]
fn extract_github_repo_only_one_segment() {
    assert_eq!(extract_github_repo("https://github.com/org"), None);
}

#[test]
fn extract_github_repo_malformed_url() {
    assert_eq!(extract_github_repo("not-a-url"), None);
}

// --- dispatch guard tests ---

#[test]
fn dispatch_agent_fails_fast_with_empty_repo_path() {
    let mock = MockProcessRunner::new(vec![]);
    let mut task = make_task("/some/repo");
    task.repo_path = "".to_string();
    let result = dispatch_agent(&task, &mock, None, &LearningInjections::default(), None);
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
    let mock = MockProcessRunner::new(vec![
        MockProcessRunner::ok_with_stdout(b"main\n"), // rev-parse HEAD
        MockProcessRunner::fail(""),                  // remote get-url (no remote)
        MockProcessRunner::ok(),                      // git rebase main (from worktree)
        MockProcessRunner::ok(),                      // git merge --ff-only (fast-forward)
                                                      // No tmux window, no cleanup
    ]);

    finish_task(&fctx("main", None), &mock).unwrap();
    let calls = mock.recorded_calls();
    // Should not have a "pull" call
    assert!(!calls.iter().any(|c| c.1.contains(&"pull".to_string())));
}

// --- new TDD tests for explicit base_branch ---

#[test]
fn finish_task_uses_explicit_base_branch_not_auto_detected() {
    // "develop" is passed explicitly; no symbolic-ref (detect_default_branch) call
    let mock = MockProcessRunner::new(vec![
        MockProcessRunner::ok_with_stdout(b"develop\n"), // rev-parse HEAD → on develop
        MockProcessRunner::fail(""),                     // remote get-url (no remote)
        MockProcessRunner::ok(),                         // git rebase develop
        MockProcessRunner::ok(),                         // git merge --ff-only develop
    ]);

    finish_task(&fctx("develop", None), &mock).unwrap();

    let calls = mock.recorded_calls();
    // No symbolic-ref call — branch was provided explicitly
    assert!(
        !calls
            .iter()
            .any(|c| c.0 == "git" && c.1.iter().any(|a| a == "symbolic-ref")),
        "symbolic-ref must not be called when base_branch is explicit"
    );
    // Rebase should target "develop"
    let rebase = calls
        .iter()
        .find(|c| c.1.contains(&"rebase".to_string()))
        .unwrap();
    assert!(rebase.1.contains(&"develop".to_string()));
}

#[test]
fn dispatch_agent_uses_task_base_branch_in_prompt() {
    let (_dir, repo_path, worktree_dir) = make_test_repo_with_worktree("42-fix-bug");

    // No detect_default_branch call expected — task.base_branch is used directly
    let mock = MockProcessRunner::new(vec![
        MockProcessRunner::ok(), // tmux new-window
        MockProcessRunner::ok(), // tmux set-option @dispatch_dir
        MockProcessRunner::ok(), // tmux set-hook
        MockProcessRunner::ok(), // tmux send-keys -l
        MockProcessRunner::ok(), // tmux send-keys Enter
    ]);

    let mut task = make_task(&repo_path);
    task.base_branch = "develop".into();
    dispatch_agent(&task, &mock, None, &LearningInjections::default(), None).unwrap();

    let prompt_file = worktree_dir.join(".claude-prompt");
    let prompt = std::fs::read_to_string(prompt_file).unwrap();
    assert!(
        prompt.contains("rebase your branch from develop"),
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

    let mock = MockProcessRunner::new(vec![
        // No detect_default_branch call — task.base_branch is used directly
        MockProcessRunner::ok(), // tmux new-window
        MockProcessRunner::ok(), // tmux set-option @dispatch_dir
        MockProcessRunner::ok(), // tmux set-hook
        MockProcessRunner::ok(), // tmux send-keys -l
        MockProcessRunner::ok(), // tmux send-keys Enter
    ]);

    let task = make_task(&repo_path);
    dispatch_agent(&task, &mock, None, &LearningInjections::default(), None).unwrap();

    let calls = mock.recorded_calls();
    let send_keys_arg = find_call_arg(&calls, 3, "claude");
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

    let mock = MockProcessRunner::new(vec![
        MockProcessRunner::ok(), // tmux new-window
        MockProcessRunner::ok(), // tmux set-option @dispatch_dir
        MockProcessRunner::ok(), // tmux set-hook
        MockProcessRunner::ok(), // tmux send-keys -l
        MockProcessRunner::ok(), // tmux send-keys Enter
    ]);

    resume_agent(TaskId(42), &worktree_path, &mock).unwrap();

    let calls = mock.recorded_calls();
    let send_keys_arg = find_call_arg(&calls, 3, "claude");
    assert!(
        send_keys_arg.contains("--plugin-dir"),
        "resume_agent should include --plugin-dir, got: {send_keys_arg}"
    );
    assert!(
        send_keys_arg.contains(".claude/plugins/local/dispatch"),
        "plugin-dir should point to local dispatch plugin, got: {send_keys_arg}"
    );
}

#[test]
fn provision_worktree_does_not_create_dispatch_dir() {
    // .dispatch/ should only be created on demand (e.g. when index_repo is called),
    // not automatically for every dispatched worktree.
    let (_dir, repo_path, worktree_dir) = make_test_repo_with_worktree("42-fix-bug");

    let mock = MockProcessRunner::new(vec![
        MockProcessRunner::ok(), // tmux new-window
        MockProcessRunner::ok(), // tmux set-option @dispatch_dir
        MockProcessRunner::ok(), // tmux set-hook (after-split-window)
    ]);

    let task = make_task(&repo_path);
    provision_worktree(&task, &mock, None, SUBPROCESS_TIMEOUT).unwrap();

    assert!(
        !worktree_dir.join(".dispatch").exists(),
        ".dispatch/ must not be created by provision_worktree"
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

// --- cleanup_task edge cases ---

#[test]
fn cleanup_skips_kill_when_window_not_found() {
    // tmux_window is Some but has_window returns false (window already gone).
    let mock = MockProcessRunner::new(vec![
        MockProcessRunner::ok_with_stdout(b"\n"), // has_window: empty → false
        MockProcessRunner::ok(),                  // git worktree remove
        MockProcessRunner::ok(),                  // git branch -D (best-effort)
    ]);

    cleanup_task(
        "/repo",
        "/repo/.worktrees/42-fix-bug",
        Some("task-42"),
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
fn finish_task_skips_kill_when_tmux_window_not_found() {
    // The tmux window has already disappeared before finish_task runs.
    let mock = MockProcessRunner::new(vec![
        MockProcessRunner::ok_with_stdout(b"main\n"), // rev-parse HEAD
        MockProcessRunner::fail(""),                  // remote get-url (no remote)
        MockProcessRunner::ok(),                      // git rebase main (from worktree)
        MockProcessRunner::ok(),                      // git merge --ff-only
        MockProcessRunner::ok_with_stdout(b"\n"),     // tmux list-windows — no match
    ]);

    finish_task(&fctx("main", Some("task-42")), &mock).unwrap();

    let calls = mock.recorded_calls();
    assert!(
        calls
            .iter()
            .all(|(prog, args)| !(prog == "tmux" && args.iter().any(|a| a == "kill-window"))),
        "kill-window should not be called when window not found, got: {calls:?}"
    );
}

// ---------------------------------------------------------------------------
// finish_task — additional branch coverage
// ---------------------------------------------------------------------------

#[test]
fn finish_task_rebase_other_failure_aborts_and_returns_other() {
    // Rebase fails with a non-conflict stderr → maps to FinishError::Other.
    // `git rebase --abort` is still issued for cleanup.
    let mock = MockProcessRunner::new(vec![
        MockProcessRunner::ok_with_stdout(b"main\n"), // rev-parse HEAD
        MockProcessRunner::fail(""),                  // remote get-url (no remote)
        MockProcessRunner::fail("fatal: unrelated histories"), // git rebase
        MockProcessRunner::ok(),                      // git rebase --abort
    ]);

    let err = finish_task(&fctx("main", None), &mock).unwrap_err();

    assert!(
        matches!(err, FinishError::Other(ref m) if m.contains("Rebase failed")),
        "non-conflict rebase failure should be FinishError::Other, got: {err}"
    );

    let calls = mock.recorded_calls();
    assert!(
        calls.last().unwrap().1.contains(&"--abort".to_string()),
        "rebase --abort must be invoked after a non-conflict rebase failure"
    );
}

#[test]
fn finish_task_ff_only_failure_returns_other() {
    let mock = MockProcessRunner::new(vec![
        MockProcessRunner::ok_with_stdout(b"main\n"), // rev-parse HEAD
        MockProcessRunner::fail(""),                  // remote get-url (no remote)
        MockProcessRunner::ok(),                      // git rebase main
        MockProcessRunner::fail("fatal: Not possible to fast-forward, aborting."),
    ]);

    let err = finish_task(&fctx("main", None), &mock).unwrap_err();

    assert!(
        matches!(err, FinishError::Other(ref m) if m.contains("Fast-forward failed")),
        "ff-only failure should map to FinishError::Other, got: {err}"
    );
}

#[test]
fn finish_task_rev_parse_runner_error_returns_other() {
    // The runner itself errors (e.g. git binary missing) on rev-parse.
    let mock = MockProcessRunner::new(vec![Err(anyhow::anyhow!("no such program"))]);

    let err = finish_task(&fctx("main", None), &mock).unwrap_err();

    assert!(
        matches!(err, FinishError::Other(ref m) if m.contains("Failed to check current branch")),
        "rev-parse runner error should map to FinishError::Other, got: {err}"
    );
}

#[test]
fn finish_task_no_tmux_window_skips_tmux_entirely() {
    // tmux_window=None → no list-windows or kill-window calls.
    let mock = MockProcessRunner::new(vec![
        MockProcessRunner::ok_with_stdout(b"main\n"), // rev-parse HEAD
        MockProcessRunner::fail(""),                  // remote get-url (no remote)
        MockProcessRunner::ok(),                      // git rebase
        MockProcessRunner::ok(),                      // git merge --ff-only
    ]);

    finish_task(&fctx("main", None), &mock).unwrap();

    let calls = mock.recorded_calls();
    assert!(
        !calls.iter().any(|c| c.0 == "tmux"),
        "no tmux calls expected when tmux_window is None, got: {calls:?}"
    );
}

// ---------------------------------------------------------------------------
// cleanup_task — additional branch coverage
// ---------------------------------------------------------------------------

#[test]
fn cleanup_task_no_tmux_window_arg_skips_tmux() {
    // tmux_window=None → cleanup goes straight to worktree remove + branch -D.
    let mock = MockProcessRunner::new(vec![
        MockProcessRunner::ok(), // git worktree remove
        MockProcessRunner::ok(), // git branch -D (best-effort)
    ]);

    cleanup_task("/repo", "/repo/.worktrees/42-fix-bug", None, &mock).unwrap();

    let calls = mock.recorded_calls();
    assert!(
        !calls.iter().any(|c| c.0 == "tmux"),
        "no tmux calls expected when tmux_window is None, got: {calls:?}"
    );
    assert!(calls[0].1.contains(&"remove".to_string()));
}

#[test]
fn cleanup_task_other_remove_failure_propagates() {
    // git worktree remove fails with stderr that is NOT "is not a working tree"
    // → cleanup_task surfaces an error to the caller.
    let mock = MockProcessRunner::new(vec![MockProcessRunner::fail(
        "fatal: some unexpected git failure",
    )]);

    let err = cleanup_task("/repo", "/repo/.worktrees/42-fix-bug", None, &mock).unwrap_err();

    let msg = format!("{err:#}");
    assert!(
        msg.contains("git worktree remove failed"),
        "expected 'git worktree remove failed' in error chain, got: {msg}"
    );
}

#[test]
fn cleanup_task_kill_window_failure_propagates() {
    // tmux kill-window fails → cleanup_task returns an error and does NOT
    // attempt the worktree remove.
    let mock = MockProcessRunner::new(vec![
        MockProcessRunner::ok_with_stdout(b"task-42\n"), // has_window → true
        MockProcessRunner::fail("can't find window"),    // kill-window fails
    ]);

    let err = cleanup_task(
        "/repo",
        "/repo/.worktrees/42-fix-bug",
        Some("task-42"),
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

// --- cleanup_task has_window runner error path ---

#[test]
fn cleanup_task_has_window_runner_error_warns_and_continues() {
    // When runner.run() itself returns Err (e.g. tmux not installed), has_window
    // propagates the error to cleanup_task, which logs a warning and continues
    // rather than aborting — worktree remove must still run.
    let mock = MockProcessRunner::new(vec![
        Err(anyhow::anyhow!("tmux not installed")), // has_window → runner error
        MockProcessRunner::ok(),                    // git worktree remove
        MockProcessRunner::ok(),                    // git branch -D (best-effort)
    ]);

    cleanup_task(
        "/repo",
        "/repo/.worktrees/42-fix-bug",
        Some("task-42"),
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
fn provision_worktree_kills_git_fetch_on_timeout_and_falls_back() {
    // git fetch times out → process is killed and treated as soft-fail (fallback to local branch).
    // Before fix (using run): mock sleeps 100ms then succeeds → start_point = "origin/main".
    // After fix (using run_with_timeout): timeout error caught → falls back to local "main".
    let (_dir, repo_path) = make_test_repo();
    let short_timeout = Duration::from_millis(10);

    let mock = MockProcessRunner::new_with_delays(vec![
        (Some(Duration::from_millis(100)), MockProcessRunner::ok()), // git fetch → timeout (killed)
        (None, MockProcessRunner::ok()), // git worktree add (local fallback)
        (None, MockProcessRunner::ok()), // tmux new-window
        (None, MockProcessRunner::ok()), // tmux set-option
        (None, MockProcessRunner::ok()), // tmux set-hook
    ]);

    let task = make_task(&repo_path);
    // Soft-fail: provision_worktree succeeds despite fetch timeout
    let result = provision_worktree(&task, &mock, Some("main"), short_timeout);
    assert!(
        result.is_ok(),
        "fetch timeout should soft-fail, got: {result:?}"
    );

    // After fetch timeout, worktree add must use local "main" (not "origin/main")
    let calls = mock.recorded_calls();
    let worktree_call = calls
        .iter()
        .find(|c| c.1.contains(&"worktree".to_string()))
        .expect("expected git worktree add call");
    assert_eq!(
        worktree_call.1.last().unwrap(),
        "main",
        "after fetch timeout, start point should be local 'main', got: {:?}",
        worktree_call.1
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
    let mock = MockProcessRunner::new(vec![
        MockProcessRunner::ok(), // tmux new-window
        MockProcessRunner::ok(), // tmux set-option @dispatch_dir
        MockProcessRunner::ok(), // tmux set-hook
        MockProcessRunner::ok(), // tmux send-keys -l
        MockProcessRunner::ok(), // tmux send-keys Enter
    ]);
    let task = make_task(&repo_path);
    dispatch_agent(&task, &mock, None, &LearningInjections::default(), None).unwrap();
    assert_worktree_confinement(&read_prompt(&worktree_dir));
}

#[test]
fn research_agent_prompt_is_correct() {
    let (_dir, repo_path, worktree_dir) = make_test_repo_with_worktree("42-fix-bug");
    let mock = MockProcessRunner::new(vec![
        MockProcessRunner::ok(), // tmux new-window
        MockProcessRunner::ok(), // tmux set-option @dispatch_dir
        MockProcessRunner::ok(), // tmux set-hook
        MockProcessRunner::ok(), // tmux send-keys -l
        MockProcessRunner::ok(), // tmux send-keys Enter
    ]);
    let task = make_task(&repo_path);
    research_agent(&task, &mock, None, None).unwrap();
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
    let mock = MockProcessRunner::new(vec![
        MockProcessRunner::ok(), // tmux new-window
        MockProcessRunner::ok(), // tmux set-option @dispatch_dir
        MockProcessRunner::ok(), // tmux set-hook
        MockProcessRunner::ok(), // tmux send-keys -l
        MockProcessRunner::ok(), // tmux send-keys Enter
    ]);
    let task = make_task(&repo_path);
    quick_dispatch_agent(&task, &mock, None, &LearningInjections::default(), None).unwrap();
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
    let mock = MockProcessRunner::new(vec![
        MockProcessRunner::ok(), // tmux new-window
        MockProcessRunner::ok(), // tmux set-option @dispatch_dir
        MockProcessRunner::ok(), // tmux set-hook
        MockProcessRunner::ok(), // tmux send-keys -l
        MockProcessRunner::ok(), // tmux send-keys Enter
    ]);
    let task = make_task(&repo_path);
    dispatch_agent(&task, &mock, None, &LearningInjections::default(), None).unwrap();

    let calls = mock.recorded_calls();
    // Call 0 is `tmux new-window …`; its `-c <dir>` argument sets the window cwd.
    assert_eq!(calls[0].0, "tmux");
    assert_eq!(calls[0].1[0], "new-window");
    let c_pos = calls[0]
        .1
        .iter()
        .position(|a| a == "-c")
        .expect("new-window should pass -c <working_dir>");
    let cwd = &calls[0].1[c_pos + 1];
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
    let mock = MockProcessRunner::new(vec![
        // git worktree add is skipped (dir exists); first tmux call fails.
        MockProcessRunner::fail("no server running on /tmp/tmux-1000/default"),
    ]);
    let task = make_task(&repo_path);
    let result = dispatch_agent(&task, &mock, None, &LearningInjections::default(), None);

    assert!(
        result.is_err(),
        "dispatch should propagate tmux new-window failure"
    );
    let msg = format!("{:#}", result.unwrap_err());
    assert!(
        msg.contains("failed to create tmux window"),
        "expected new-window context in error chain, got: {msg}"
    );
}

#[test]
fn dispatch_agent_propagates_send_keys_failure() {
    let (_dir, repo_path, _worktree_dir) = make_test_repo_with_worktree("42-fix-bug");
    let mock = MockProcessRunner::new(vec![
        MockProcessRunner::ok(),                    // tmux new-window
        MockProcessRunner::ok(),                    // tmux set-option @dispatch_dir
        MockProcessRunner::ok(),                    // tmux set-hook
        MockProcessRunner::fail("can't find pane"), // tmux send-keys -l fails
    ]);
    let task = make_task(&repo_path);
    let result = dispatch_agent(&task, &mock, None, &LearningInjections::default(), None);

    assert!(
        result.is_err(),
        "dispatch should propagate send-keys failure"
    );
    let msg = format!("{:#}", result.unwrap_err());
    assert!(
        msg.contains("failed to send keys to tmux window"),
        "expected send-keys context in error chain, got: {msg}"
    );
}

// ---------------------------------------------------------------------------
// resume_agent — failure path
// ---------------------------------------------------------------------------

#[test]
fn resume_agent_propagates_new_window_failure() {
    // First call (tmux new-window) fails — error should bubble up.
    let mock = MockProcessRunner::new(vec![MockProcessRunner::fail(
        "no server running on /tmp/tmux-1000/default",
    )]);

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
