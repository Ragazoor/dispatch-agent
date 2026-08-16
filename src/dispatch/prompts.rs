use std::sync::Arc;

use crate::db;
use crate::models::{EpicId, Learning, RetrievalSource, Task, TaskId, TaskTag};
use crate::service::embeddings::{
    deserialize_candidate_rows, embed_text_for_query, rag_rank_learnings, EmbeddingService,
    RagRankParams,
};

use super::worktree::StartPoint;

/// Flags added to all Claude agent invocations. `--plugin-dir` so dispatched
/// agents discover the dispatch plugin's skills and commands (e.g. /wrap-up);
/// `--settings` so every session reports its subscription budget windows via
/// the `dispatch statusline` decorator (see docs/specs/dispatch.allium:
/// TokenBudgetIndicator).
///
/// Both paths are fixed literals on purpose. This string is interpolated into
/// shell command lines sent through `tmux send-keys`, so a runtime path could
/// break argument splitting on any `$HOME` containing a space; and a `const`
/// cannot hold a runtime value anyway. Runtime paths live inside the settings
/// file, written by `src/setup/statusline.rs`.
///
/// Built with `concat!` rather than a backslash line-continuation inside the
/// literal: a `\` at end-of-line inside a Rust string swallows the newline
/// *and* all leading whitespace on the next line, which can silently collapse
/// or duplicate the space between the two flags.
pub(super) const DISPATCH_PLUGIN_DIR: &str = concat!(
    "--plugin-dir ~/.claude/plugins/local/dispatch",
    " --settings ~/.claude/dispatch-statusline.json"
);

/// Epic context passed to prompt builders so agents know about their epic.
pub struct EpicContext {
    pub epic_id: EpicId,
    pub epic_title: String,
}

impl EpicContext {
    /// Build epic context from the database for a task that belongs to an epic.
    pub async fn from_db(task: &Task, db: &dyn db::TaskReadStore) -> Option<Self> {
        let epic_id = task.epic_id?;
        let epic = db.get_epic(epic_id).await.ok()??;
        Some(EpicContext {
            epic_id,
            epic_title: epic.title,
        })
    }

    pub(super) fn prompt_section(&self) -> String {
        format!(
            "\n\nThis task is part of epic #{}: {}\n\
            To find other tasks in this epic, call list_tasks with epic_id={}.\n\
            To ask questions or send updates to a sibling agent, use ListAgents to find its \
            session (named task-<id>, matching that task's own id) and message it directly \
            with SendMessage.",
            self.epic_id, self.epic_title, self.epic_id
        )
    }
}

/// Preamble for a worktree reused from a previous attempt.
///
/// Reuse is the only non-PR case where a rebase does real work: a fresh
/// worktree's branch *is* its start point, so rebasing onto that ref can only
/// report "up to date". The rebase targets whichever ref provisioning chose —
/// pointing a local-based branch at `origin/<base>` would replay local `<base>`'s
/// unpushed commits under new SHAs, which then collide with the wrap-up rebase
/// onto local `<base>`.
pub(super) fn reused_rebase_preamble(start_point: &StartPoint) -> String {
    format!(
        "This worktree was reused from a previous attempt and may contain \
         uncommitted changes or commits from that run. Check `git status` and \
         `git log` first, then bring the branch up to date:\n\
         ```\n\
         git fetch origin {base}\n\
         git rebase {target}\n\
         ```\n\
         If the rebase reports unstaged changes, commit or stash them first.",
        base = start_point.base(),
        target = start_point.git_ref(),
    )
}

/// Which rebase preamble — if any — a dispatch gets. The whole rule, in one
/// pure function, evaluated *after* provisioning so it can see the resolved ref.
///
/// Takes no fetch warning: the `Note:` is composed separately by
/// [`compose_prompt_head`], which is what keeps this a three-row table.
pub(super) fn select_preamble(
    pr_branch: Option<&str>,
    start_point: Option<&StartPoint>,
    reused: bool,
) -> String {
    if let Some(branch) = pr_branch {
        return pr_rebase_preamble(branch);
    }
    match start_point {
        Some(sp) if reused => reused_rebase_preamble(sp),
        _ => String::new(),
    }
}

/// Everything that precedes the "Always work from this worktree folder" line:
/// the preamble (possibly empty) and the fetch `Note:` (possibly absent), each
/// separated from what follows by a blank line.
///
/// The two are independent — a fresh worktree based on a local-only branch has
/// a warning worth surfacing and no preamble to attach it to.
pub(super) fn compose_prompt_head(preamble: &str, fetch_warning: Option<&str>) -> String {
    let mut parts: Vec<String> = Vec::new();
    if !preamble.is_empty() {
        parts.push(preamble.to_string());
    }
    if let Some(warning) = fetch_warning {
        parts.push(format!("Note: {warning}"));
    }
    if parts.is_empty() {
        return String::new();
    }
    format!("{}\n\n", parts.join("\n\n"))
}

/// Preamble for review tasks whose worktree is based on a PR branch.
///
/// The worktree already starts from the PR's code, so this is an on-demand
/// refresh: run it whenever you (or the user) want to pull in commits pushed to
/// the PR after dispatch. It rebases the worktree branch onto the latest
/// `origin/<branch>` rather than onto the repo's base branch.
pub(super) fn pr_rebase_preamble(branch: &str) -> String {
    format!(
        "This worktree is based on the PR branch `{branch}`. To pull in the \
         latest commits pushed to the PR, rebase onto it (do this whenever you \
         want to refresh the PR's code):\n\
         ```\n\
         git fetch origin {branch}\n\
         git rebase origin/{branch}\n\
         ```"
    )
}

/// Returns `(epic_id_line, epic_section)` for embedding in agent prompts.
pub(super) fn epic_preamble(epic: Option<&EpicContext>) -> (String, String) {
    let id_line = epic.map_or(String::new(), |e| format!("\n  EpicId: {}", e.epic_id));
    let section = epic.map_or(String::new(), |e| e.prompt_section());
    (id_line, section)
}

/// Standard task identification block shared by all task agent prompts.
pub(super) fn task_block(
    task_id: TaskId,
    title: &str,
    description: &str,
    epic: Option<&EpicContext>,
) -> String {
    let (epic_id_line, epic_section) = epic_preamble(epic);
    format!(
        "Task:\n  ID: {task_id}\n  Title: {title}\n  Description: {description}\
         {epic_id_line}{epic_section}"
    )
}

/// TDD instruction line, shared across all agents.
pub(super) fn tdd_instruction() -> &'static str {
    "Always use TDD: express intended behaviour as tests first, then implement the minimum code to make them pass."
}

/// MCP tools availability notice, shared across all task agents.
pub(super) fn mcp_tools_instruction() -> &'static str {
    "The dispatch MCP tools are available — use them to query and update this task (get_task, update_task)."
}

/// One-line knowledge-base nudge for dispatched agents. The earlier
/// seven-skill checkpoint list saw <2 invocations each across hundreds
/// of dispatches — replaced with a direct prompt to query the KB
/// whenever anything is unclear.
pub(super) fn learning_tools_instruction() -> &'static str {
    "Knowledge base: when anything is unclear, call `query_learnings` to check \
the knowledge base before guessing or asking. When you act on a surfaced learning, \
call `rate_learning` (`helped` or `wrong`); use `/learnings` to record useful findings. \
Use `delete_learning` to remove stale or incorrect entries by ID."
}

/// Instructions for writing a plan and attaching it to the task via MCP.
pub(super) fn plan_and_attach_instruction() -> &'static str {
    "Use /brainstorming to design the solution, then save the plan to docs/plans/ \
and call update_task to attach it."
}

/// Dispatch instruction for no-plan tasks: conditionally suggests brainstorming
/// based on agent judgment of task description clarity. Framed as an
/// intermediate step, not a stopping point; `Research`/`Dependabot`/`PrReview`
/// never reach this addendum (see `DispatchMode::for_task` and
/// `TaskTag::is_review`), so no per-tag branch is needed here. Carries the
/// same epic-decomposition carve-out as `wrap_up_instruction` so the two
/// stay consistent about what counts as done.
pub(super) fn plan_or_brainstorm_instruction() -> &'static str {
    "Use /brainstorming to design the solution if the task description is vague or \
underspecified. Otherwise write an implementation plan directly, save it to docs/plans/ \
and call update_task to attach it. Attaching the plan is not the end of the task — \
implement it in this same session (or, for an epic-decomposition task, create work \
packages for its subtasks instead), following the TDD instruction below, and verify \
your work before wrapping up."
}

/// Wrap-up instruction shared by every dispatched task agent. Wording is
/// intentionally universal, but no longer treats attaching a plan as an
/// independently sufficient stopping point: it must be followed by
/// implementation in the same session. Creating work packages on an epic
/// remains a legitimate terminal state for a decomposition task, since that
/// task's job is delegation, not implementation.
pub(super) fn wrap_up_instruction() -> &'static str {
    "Writing or attaching a plan for your own task is not a stopping point on \
its own — implement it in the same session first. When your work is done — \
finishing implementation, or (for an epic-decomposition task) creating work \
packages for its subtasks — use the /wrap-up skill to commit any remaining \
changes and finalise the task."
}

/// Allium spec instruction — shared across all agents that may touch domain behaviour.
pub(super) fn allium_instruction() -> &'static str {
    "The Allium specs in `docs/specs/` are the source of truth for domain logic. \
Consult them before changing core behaviour. If your implementation changes domain behaviour, \
update the spec using the `allium:tend` skill and verify alignment with `allium:weed`."
}

/// Trailing metadata shared by every dispatched task agent prompt:
/// `tdd + allium + mcp + learning + wrap_up`, separated by blank lines.
/// Each `format!` in a builder ends with `{trailing}` where this helper plugs in.
pub(super) fn trailing_block() -> String {
    format!(
        "{tdd}\n\
\n\
{allium}\n\
\n\
{mcp}\n\
\n\
{learning}\n\
\n\
{wrap_up}",
        tdd = tdd_instruction(),
        allium = allium_instruction(),
        mcp = mcp_tools_instruction(),
        learning = learning_tools_instruction(),
        wrap_up = wrap_up_instruction(),
    )
}

/// Render the tiered-knowledge block placed between the task block and the
/// addendum in a dispatch prompt. Returns an empty string when `picked` is
/// empty so existing prompts are byte-identical when no learnings are injected.
pub(super) fn render_validated_knowledge_block(picked: &[&Learning]) -> String {
    if picked.is_empty() {
        return String::new();
    }
    let mut out = String::from(
        "## Validated knowledge for this task\n\n\
The following knowledge has been validated by previous agents. Apply it where relevant. \
When you act on an entry, call `rate_learning(learning_id, task_id, verdict)` — `helped` if it \
applied, `wrong` if it misled you.\n\n",
    );
    for l in picked {
        out.push_str(&format!(
            "- [#{} {}, \u{2191}{}] {}\n",
            l.id.0,
            l.scope.as_str(),
            l.upvote_count,
            l.summary
        ));
    }
    out.push('\n');
    out
}

/// Whether a blank line separates the intro line from the task block.
/// `build_prompt` uses a single newline; the other two builders use a blank
/// line — a pre-existing inconsistency this enum makes explicit instead of
/// encoding it as raw `"\n"` vs `"\n\n"` bytes in the `intro` string.
enum IntroSpacing {
    SingleNewline,
    BlankLine,
}

impl IntroSpacing {
    fn as_separator(&self) -> &'static str {
        match self {
            IntroSpacing::SingleNewline => "\n",
            IntroSpacing::BlankLine => "\n\n",
        }
    }
}

/// Shared skeleton for every `build_*_prompt` variant:
/// `{intro}{spacing}{block}\n\n{knowledge}{addendum}\n\n{trailing}`.
///
/// Callers build `block` themselves (via `task_block`) since its inputs
/// aren't otherwise needed here — keeps this under clippy's argument-count
/// limit.
///
/// Each builder computes its own `intro`/`block`/`addendum`/`trailing` and
/// passes them here, so the knowledge plumbing stays in one place — a variant
/// can no longer silently drop the knowledge block (the research-prompt drift
/// this fixed) by forgetting to wire it in.
fn render_task_prompt(
    intro: &str,
    spacing: IntroSpacing,
    block: &str,
    ctx: &PromptContext<'_>,
    addendum: &str,
    trailing: &str,
) -> String {
    let knowledge = render_validated_knowledge_block(&ctx.learnings.ranked);
    let sep = spacing.as_separator();
    format!("{intro}{sep}{block}\n\n{knowledge}{addendum}\n\n{trailing}")
}

pub(super) fn build_prompt(
    task_id: TaskId,
    title: &str,
    description: &str,
    plan: Option<&str>,
    epic: Option<&EpicContext>,
    ctx: &PromptContext<'_>,
) -> String {
    // Dependabot and PR-review tasks are review-only: they skip the plan /
    // implementation flow and use a trimmed trailing block.
    let is_review = ctx.tag.is_some_and(|t| t.is_review());
    let addendum = match (ctx.tag, plan) {
        (Some(TaskTag::Dependabot), _) => dependabot_review_addendum(task_id),
        (Some(TaskTag::PrReview), _) => pr_review_addendum().to_string(),
        (_, None) => plan_or_brainstorm_instruction().to_string(),
        (_, Some(path)) => {
            let tail = if ctx.auto_run_plan {
                " and begin implementing it right away — the plan has already \
been reviewed and confirmed, so no summary or confirmation step is needed."
            } else {
                ".\n\
\n\
Review the plan carefully. Summarise your intended approach in 3–5 bullet points, \
then ask: 'Shall I proceed with implementation?' Wait for confirmation before \
making any changes."
            };
            format!("Plan: {path}\nRead this file for the full implementation plan{tail}")
        }
    };
    let trailing = if is_review {
        format!(
            "{mcp}\n\
\n\
{learning}",
            mcp = mcp_tools_instruction(),
            learning = learning_tools_instruction(),
        )
    } else {
        trailing_block()
    };

    let block = task_block(task_id, title, description, epic);
    render_task_prompt(
        "Your task is:",
        IntroSpacing::SingleNewline,
        &block,
        ctx,
        &addendum,
        &trailing,
    )
}

/// Substitute a `{{KEY}}` placeholder in a prompt template loaded via
/// `include_str!`. Trims the trailing newline added by editors so the
/// inlined block composes cleanly with surrounding `format!` blocks.
fn render_template(template: &str, key: &str, value: &str) -> String {
    template
        .trim_end_matches('\n')
        .replace(&format!("{{{{{key}}}}}"), value)
}

/// PR review guidance, loaded from `prompts/pr-review.md`.
/// The agent checks the diff size and runs either /review (small) or
/// /review-pr (large). It does NOT write a plan, implement code,
/// or call /wrap-up.
fn pr_review_addendum() -> &'static str {
    include_str!("prompts/pr-review.md").trim_end_matches('\n')
}

/// Dependabot PR review guidance, loaded from `prompts/dependabot.md`.
/// The agent vets a dependency-bump PR and auto-approves/merges if clearly
/// safe, otherwise asks the user. It does NOT call /wrap-up — the task is
/// auto-cleaned when the PR merges.
fn dependabot_review_addendum(task_id: TaskId) -> String {
    render_template(
        include_str!("prompts/dependabot.md"),
        "TASK_ID",
        &task_id.0.to_string(),
    )
}

pub(super) fn build_quick_dispatch_prompt(
    task_id: TaskId,
    title: &str,
    description: &str,
    epic: Option<&EpicContext>,
    ctx: &PromptContext<'_>,
) -> String {
    let addendum = format!(
        "This is a quick-dispatched task with a placeholder title. Start by asking the user \
what they want to achieve. Once you understand the goal, call `update_task` with a \
descriptive `title` (and optionally `description`) to rename the task on the kanban board.\n\
\n\
Then write a focused plan before making any changes:\n\
\n\
{attach}",
        attach = plan_and_attach_instruction(),
    );

    let block = task_block(task_id, title, description, epic);
    render_task_prompt(
        "You are working interactively with the user.",
        IntroSpacing::BlankLine,
        &block,
        ctx,
        &addendum,
        &trailing_block(),
    )
}

pub(super) fn build_research_prompt(
    task_id: TaskId,
    title: &str,
    description: &str,
    epic: Option<&EpicContext>,
    ctx: &PromptContext<'_>,
) -> String {
    let addendum = "Investigate the topic described above. You may read the codebase, \
documentation, and external resources.\n\
\n\
When you have gathered sufficient information, present your findings clearly to the user \
and wait for further instructions. Do NOT call /wrap-up — that is for the user to \
decide.\n\
\n\
Do NOT make code changes.";

    let block = task_block(task_id, title, description, epic);
    render_task_prompt(
        "You are a research agent.",
        IntroSpacing::BlankLine,
        &block,
        ctx,
        addendum,
        mcp_tools_instruction(),
    )
}

/// Prompt for a scheduled pipeline tick (`pipeline_agent`).
///
/// Deliberately spare: a tick has no plan and no epic addendum to summarise —
/// the work is entirely "the branch moved, make it green again". The
/// description is left empty for the same reason; the branch name and the
/// merge target are the whole brief.
///
/// **This prompt is not final.** Subtask #4205 lands `wrap_up(action="merge")`;
/// until then the closing instruction names an action the tools do not yet
/// accept, and the merge sentence should be revisited once it exists. (Verify
/// tiering was dropped — see the design doc's Part B — so the single
/// `verify_command` is correct here and needs no follow-up.)
pub(super) fn build_pipeline_prompt(
    task_id: TaskId,
    title: &str,
    pinned_branch: Option<&str>,
    base_branch: &str,
    epic: Option<&EpicContext>,
    ctx: &PromptContext<'_>,
) -> String {
    let branch = pinned_branch.unwrap_or("this task's own branch");
    let addendum = format!(
        "This is a recurring pipeline task tracking `{branch}`. New commits have landed on \
         it since the last successful run.\n\
         \n\
         Run the verify command (`get_task` reports it on its \"Verify command\" line) and \
         fix whatever fails, with ordinary commits on `{branch}`. Then promote the branch by \
         merging it into `{base_branch}` and close the session.\n\
         \n\
         If the branch is already green, say so and close the session without inventing work."
    );

    let block = task_block(task_id, title, "", epic);
    render_task_prompt(
        "You are a pipeline agent.",
        IntroSpacing::BlankLine,
        &block,
        ctx,
        &addendum,
        mcp_tools_instruction(),
    )
}

/// Maximum total learnings injected into a dispatch prompt via RAG.
pub const DISPATCH_INJECTION_CAP: usize = 5;

/// Push-injection groups for a dispatch prompt.
#[derive(Default, Clone)]
pub struct LearningInjections<'a> {
    pub ranked: Vec<&'a Learning>,
}

impl<'a> From<&'a [Learning]> for LearningInjections<'a> {
    fn from(v: &'a [Learning]) -> Self {
        Self {
            ranked: v.iter().collect(),
        }
    }
}

/// Bundle of all push-injected context for a dispatch prompt. Threaded through
/// every `build_*_prompt` so individual builders never grow more positional
/// parameters when a new context source lands.
#[derive(Default)]
pub struct PromptContext<'a> {
    pub learnings: LearningInjections<'a>,
    pub tag: Option<TaskTag>,
    pub auto_run_plan: bool,
}

pub use crate::service::embeddings::RAG_SIMILARITY_THRESHOLD as DISPATCH_RAG_THRESHOLD;

/// Build the learning injections for a dispatch prompt using the RAG pipeline.
///
/// Steps:
/// 1. Embeds the task title + description to form a query vector.
/// 2. Fetches all approved non-task-scoped learnings with embeddings from the DB.
/// 3. Ranks them by cosine similarity + scope/upvote boost (via `rag_rank_learnings`).
/// 4. Returns at most `DISPATCH_INJECTION_CAP` results; all go into the
///    validated-knowledge block regardless of `LearningKind`.
///
/// On embedding failure the function falls back to an empty list so a single
/// model error never blocks dispatch.
pub async fn list_learnings_for_dispatch_rag(
    db: &dyn crate::db::TaskReadStore,
    task: &Task,
    emb_svc: &Arc<EmbeddingService>,
    threshold: f32,
) -> Vec<Learning> {
    let query_text = embed_text_for_query(&task.title, &task.description);
    let query_vec = match emb_svc.embed(query_text).await {
        Ok(v) => v,
        Err(e) => {
            tracing::warn!(
                task_id = task.id.0,
                error = ?e,
                "dispatch RAG: embedding query failed, skipping injection"
            );
            return vec![];
        }
    };

    let rows = match db.list_all_approved_non_task_learnings().await {
        Ok(r) => r,
        Err(e) => {
            tracing::warn!(
                task_id = task.id.0,
                error = ?e,
                "dispatch RAG: failed to fetch learnings, skipping injection"
            );
            return vec![];
        }
    };

    let candidates = deserialize_candidate_rows(rows);

    let epic_id_str = task.epic_id.map(|e| e.0.to_string());
    let all_ranked = rag_rank_learnings(
        &candidates,
        &RagRankParams {
            query_vec: &query_vec,
            task_epic_id: epic_id_str.as_deref(),
            task_repo: Some(task.repo_path.as_str()),
            threshold,
            tag_filter: &[],
            limit: DISPATCH_INJECTION_CAP,
        },
    );

    all_ranked.into_iter().cloned().collect()
}

pub async fn build_and_record_injections(
    db: &dyn crate::db::TaskReadStore,
    task: &crate::models::Task,
    emb_svc: &Arc<EmbeddingService>,
) -> Vec<Learning> {
    let all = list_learnings_for_dispatch_rag(db, task, emb_svc, DISPATCH_RAG_THRESHOLD).await;
    for l in &all {
        if let Err(e) = db
            .record_retrieval(task.id, l.id, RetrievalSource::PromptInjection)
            .await
        {
            tracing::warn!(
                task_id = task.id.0,
                learning_id = l.id.0,
                error = ?e,
                "failed to record learning retrieval"
            );
        }
    }
    all
}

#[cfg(test)]
#[allow(clippy::unwrap_used, clippy::expect_used)]
mod tests {
    use super::*;
    use crate::models::{LearningKind, LearningScope};

    #[test]
    fn pr_rebase_preamble_targets_pr_branch_on_demand() {
        let text = pr_rebase_preamble("renovate/serde-1.x");
        assert!(
            text.contains("git fetch origin renovate/serde-1.x"),
            "should fetch the PR branch, got: {text}"
        );
        assert!(
            text.contains("git rebase origin/renovate/serde-1.x"),
            "should rebase onto origin/<pr-branch>, got: {text}"
        );
        assert!(
            !text.contains("rebase main"),
            "must not rebase onto the base branch, got: {text}"
        );
    }

    #[test]
    fn reused_preamble_targets_a_local_start_point() {
        let sp = StartPoint::Local {
            base: "develop".to_string(),
        };
        let text = reused_rebase_preamble(&sp);
        assert!(text.contains("git fetch origin develop"), "got: {text}");
        assert!(text.contains("git rebase develop"), "got: {text}");
        assert!(
            !text.contains("git rebase origin/develop"),
            "must not drag a local-based branch back onto origin: {text}"
        );
        assert!(
            text.contains("git status"),
            "tells the agent to inspect first"
        );
        assert!(!text.contains("main"), "no literal main: {text}");
    }

    #[test]
    fn reused_preamble_targets_a_remote_start_point() {
        let sp = StartPoint::Remote {
            base: "develop".to_string(),
        };
        let text = reused_rebase_preamble(&sp);
        assert!(text.contains("git fetch origin develop"), "got: {text}");
        assert!(text.contains("git rebase origin/develop"), "got: {text}");
    }

    #[test]
    fn select_preamble_is_empty_for_a_fresh_worktree() {
        let sp = StartPoint::Remote {
            base: "main".to_string(),
        };
        assert_eq!(select_preamble(None, Some(&sp), false), "");
    }

    #[test]
    fn select_preamble_uses_reuse_wording_for_a_reused_worktree() {
        let sp = StartPoint::Local {
            base: "main".to_string(),
        };
        let text = select_preamble(None, Some(&sp), true);
        assert!(
            text.contains("reused from a previous attempt"),
            "got: {text}"
        );
        assert!(
            text.contains("git rebase main"),
            "mirrors the start point: {text}"
        );
    }

    #[test]
    fn select_preamble_prefers_the_pr_branch_regardless_of_reuse() {
        let sp = StartPoint::Remote {
            base: "renovate/serde-1.x".to_string(),
        };
        for reused in [true, false] {
            let text = select_preamble(Some("renovate/serde-1.x"), Some(&sp), reused);
            assert!(
                text.contains("git rebase origin/renovate/serde-1.x"),
                "reused={reused}, got: {text}"
            );
            assert!(
                !text.contains("reused from a previous attempt"),
                "reused={reused}"
            );
        }
    }

    #[test]
    fn prompt_head_carries_the_warning_even_with_no_preamble() {
        // The fresh + no-origin-ref case: nothing to rebase onto, but the agent
        // must still be told its base is local-only.
        let head = compose_prompt_head("", Some("origin has no branch main"));
        assert!(
            head.contains("Note: origin has no branch main"),
            "got: {head}"
        );
        assert!(!head.starts_with('\n'), "no leading blank line: {head:?}");
    }

    #[test]
    fn prompt_head_is_empty_when_there_is_nothing_to_say() {
        assert_eq!(compose_prompt_head("", None), "");
    }

    #[test]
    fn prompt_head_combines_preamble_and_warning() {
        let head = compose_prompt_head("REBASE", Some("stale"));
        assert!(head.starts_with("REBASE"), "got: {head}");
        assert!(head.contains("Note: stale"), "got: {head}");
        assert!(head.ends_with("\n\n"), "separates from the body: {head:?}");
    }

    #[test]
    fn fresh_worktree_with_no_origin_ref_gets_the_note_and_no_preamble() {
        // The decoupled case: nothing to rebase onto, but the agent must still be
        // told its base is local-only. An earlier design attached the warning to
        // the preamble, which silently dropped it on exactly this row.
        let sp = StartPoint::Local {
            base: "main".to_string(),
        };
        let preamble = select_preamble(None, Some(&sp), false);
        let head = compose_prompt_head(&preamble, Some("origin has no branch main"));

        assert!(
            preamble.is_empty(),
            "fresh worktree emits no preamble: {preamble:?}"
        );
        assert!(
            head.contains("Note: origin has no branch main"),
            "got: {head}"
        );
        assert!(
            !head.contains("git rebase"),
            "nothing to rebase onto: {head}"
        );
        assert!(
            !head.contains("reused from a previous attempt"),
            "got: {head}"
        );
    }

    #[test]
    fn learning_instruction_references_learnings_skill() {
        let text = learning_tools_instruction();
        assert!(
            text.contains("/learnings"),
            "learning instruction should reference the /learnings skill, got: {text}"
        );
    }

    #[test]
    fn learning_instruction_references_rate_learning_not_upvote() {
        let text = learning_tools_instruction();
        assert!(
            text.contains("rate_learning"),
            "learning instruction should point agents at rate_learning, got: {text}"
        );
        assert!(
            !text.contains("upvote entries"),
            "learning instruction should no longer mention upvoting entries, got: {text}"
        );
    }

    #[test]
    fn learning_instruction_nudges_query_before_guessing() {
        let text = learning_tools_instruction();
        assert!(
            text.contains("query_learnings"),
            "learning instruction should point at the query_learnings tool, got: {text}"
        );
        assert!(
            text.contains("before guessing or asking"),
            "learning instruction should nudge agents to check the KB before guessing or asking, got: {text}"
        );
    }

    #[test]
    fn learning_instruction_covers_all_unclear_situations() {
        let text = learning_tools_instruction();
        assert!(
            text.contains("anything is unclear"),
            "learning instruction should say 'anything is unclear' rather than enumerating specific domains, got: {text}"
        );
    }

    #[test]
    fn research_prompt_names_forbidden_wrap_up_tool() {
        let text = build_research_prompt(
            TaskId(7),
            "Research async runtimes",
            "Compare tokio vs async-std",
            None,
            &PromptContext::default(),
        );
        assert!(
            text.contains("/wrap-up"),
            "research prompt should explicitly forbid /wrap-up by name, got: {text}"
        );
    }

    #[test]
    fn learning_instruction_omits_deleted_action_skills() {
        let text = learning_tools_instruction();
        for skill in [
            "/codebase-knowledge",
            "/code-conventions",
            "/test-conventions",
            "/pr-workflow",
            "/dispatch-workflow",
            "/troubleshoot",
            "/improvement",
        ] {
            assert!(
                !text.contains(skill),
                "learning instruction should no longer reference deleted skill {skill}, got: {text}"
            );
        }
    }

    #[test]
    fn learning_instruction_in_task_prompts_with_plan() {
        let text = build_prompt(
            TaskId(1),
            "title",
            "desc",
            Some("/path/to/plan.md"),
            None,
            &PromptContext::default(),
        );
        assert!(
            text.contains("/learnings"),
            "build_prompt (with plan) should reference /learnings skill"
        );
    }

    #[test]
    fn build_prompt_with_plan_and_auto_run_skips_confirmation() {
        let ctx = PromptContext {
            auto_run_plan: true,
            ..PromptContext::default()
        };
        let text = build_prompt(
            TaskId(1),
            "title",
            "desc",
            Some("/path/to/plan.md"),
            None,
            &ctx,
        );
        assert!(
            !text.contains("Shall I proceed with implementation?"),
            "auto_run_plan should skip the ask-permission addendum, got: {text}"
        );
        assert!(
            text.contains("/path/to/plan.md"),
            "the plan path should still be referenced, got: {text}"
        );
    }

    #[test]
    fn build_prompt_with_plan_default_still_asks_permission() {
        let text = build_prompt(
            TaskId(1),
            "title",
            "desc",
            Some("/path/to/plan.md"),
            None,
            &PromptContext::default(),
        );
        assert!(
            text.contains("Shall I proceed with implementation?"),
            "default (auto_run_plan: false) must keep asking, got: {text}"
        );
    }

    #[test]
    fn plan_or_brainstorm_instruction_frames_plan_as_intermediate_step() {
        let text = plan_or_brainstorm_instruction();
        assert!(
            text.contains("not the end of the task"),
            "plan_or_brainstorm_instruction should make clear attaching a plan \
doesn't finish the task, got: {text}"
        );
        assert!(
            text.contains("implement it"),
            "plan_or_brainstorm_instruction should instruct the agent to implement \
after attaching the plan, got: {text}"
        );
    }

    #[test]
    fn no_plan_addendum_instructs_implementation_for_every_working_tag() {
        // plan_or_brainstorm_instruction is reused verbatim for every tag that
        // reaches it with no plan — Bug, Feature, Chore, Fix, and no tag.
        // Research never reaches this addendum with no plan (DispatchMode
        // diverts it to build_research_prompt instead), so no per-tag branch
        // is needed here.
        for tag in [
            None,
            Some(TaskTag::Bug),
            Some(TaskTag::Feature),
            Some(TaskTag::Chore),
            Some(TaskTag::Fix),
        ] {
            let ctx = PromptContext {
                tag,
                ..PromptContext::default()
            };
            let text = build_prompt(TaskId(1), "Task", "Desc", None, None, &ctx);
            assert!(
                text.contains("not the end of the task"),
                "tag {tag:?}: no-plan prompt should instruct implementation to \
follow plan-attach, got: {text}"
            );
        }
    }

    #[test]
    fn wrap_up_instruction_no_longer_treats_plan_attach_as_sufficient() {
        // The bug this guards: prose that lists "attaching a plan" alongside
        // "finishing implementation" as equally valid stopping points reads
        // as permission to stop at a plan for bug/feature/chore/fix tasks
        // (see task #4188). The instruction may still mention plan-attach,
        // but only while explicitly saying it isn't sufficient on its own.
        let text = wrap_up_instruction();
        assert!(
            text.contains("not a stopping point"),
            "wrap_up_instruction should say attaching a plan alone is not a \
stopping point, got: {text}"
        );
        assert!(
            text.contains("finishing implementation"),
            "wrap_up_instruction should still name finishing implementation as \
a valid stopping point, got: {text}"
        );
        assert!(
            text.contains("creating work packages"),
            "wrap_up_instruction should still allow work-package creation as a \
stopping point for epic-decomposition tasks, got: {text}"
        );
    }

    #[test]
    fn learning_instruction_in_task_prompts_no_plan() {
        let text = build_prompt(
            TaskId(1),
            "title",
            "desc",
            None,
            None,
            &PromptContext::default(),
        );
        assert!(
            text.contains("/learnings"),
            "build_prompt (no plan) should reference /learnings skill"
        );
    }

    #[test]
    fn learning_instruction_in_quick_dispatch_prompt() {
        let text = build_quick_dispatch_prompt(
            TaskId(1),
            "title",
            "desc",
            None,
            &PromptContext::default(),
        );
        assert!(
            text.contains("/learnings"),
            "quick dispatch prompt should reference /learnings skill"
        );
    }

    #[test]
    fn trailing_block_includes_knowledge_base_nudge() {
        let text = trailing_block();
        assert!(
            text.contains("query_learnings"),
            "trailing block should reference query_learnings tool, got: {text}"
        );
        assert!(
            text.contains("before guessing or asking"),
            "trailing block should include the 'before guessing or asking' nudge, got: {text}"
        );
    }

    #[test]
    fn research_prompt_includes_knowledge_block_when_learnings_injected() {
        // Regression: build_research_prompt used to silently omit the
        // validated-knowledge block that build_prompt/build_quick_dispatch_prompt
        // both include — research tasks get RAG-injected learnings too, so the
        // block must appear here as well.
        let l = seed(20, LearningScope::Repo, 1);
        let ctx = PromptContext {
            learnings: LearningInjections { ranked: vec![&l] },
            tag: None,
            auto_run_plan: false,
        };
        let text = build_research_prompt(
            TaskId(7),
            "Research async runtimes",
            "Compare tokio vs async-std",
            None,
            &ctx,
        );
        assert!(
            text.contains("## Validated knowledge for this task"),
            "research prompt should include the knowledge block when learnings are injected, got: {text}"
        );
        assert!(text.contains("[#20 repo, \u{2191}1]"));
    }

    #[test]
    fn research_prompt_content() {
        let text = build_research_prompt(
            TaskId(7),
            "Research async runtimes",
            "Compare tokio vs async-std",
            None,
            &PromptContext::default(),
        );
        assert!(
            text.contains("research agent"),
            "research prompt should identify the agent role"
        );
        assert!(
            text.contains("present") || text.contains("findings"),
            "research prompt should instruct presenting findings"
        );
        assert!(
            text.contains("Do NOT make code changes")
                || text.contains("do not make code changes")
                || text.contains("no code changes"),
            "research prompt should prohibit code changes"
        );
    }

    fn seed(id: i64, scope: LearningScope, count: i64) -> Learning {
        use crate::models::{LearningId, LearningStatus};
        use chrono::{TimeZone, Utc};
        Learning {
            id: LearningId(id),
            kind: LearningKind::Pitfall,
            summary: format!("learning {id}"),
            detail: None,
            scope,
            scope_ref: match scope {
                LearningScope::User => None,
                _ => Some("ref".into()),
            },
            tags: vec![],
            status: LearningStatus::Approved,
            source_task_id: None,
            upvote_count: count,
            last_upvoted_at: None,
            created_at: Utc.with_ymd_and_hms(2026, 1, 1, 0, 0, 0).unwrap(),
            updated_at: Utc.with_ymd_and_hms(2026, 1, 1, 0, 0, 0).unwrap(),
        }
    }

    #[test]
    fn render_validated_knowledge_block_omits_when_empty() {
        assert_eq!(render_validated_knowledge_block(&[]), String::new());
    }

    #[test]
    fn render_validated_knowledge_block_formats_entries() {
        let l = seed(7, LearningScope::Epic, 3);
        let out = render_validated_knowledge_block(&[&l]);
        assert!(out.contains("## Validated knowledge for this task"));
        assert!(out.contains("[#7 epic, \u{2191}3]"));
        assert!(out.contains("learning 7"));
        assert!(
            out.contains("rate_learning"),
            "validated-knowledge block should instruct rate_learning, got: {out}"
        );
        assert!(
            !out.contains("learning_verdicts"),
            "validated-knowledge block should no longer reference wrap_up verdicts, got: {out}"
        );
    }

    #[test]
    fn build_prompt_default_injections_unchanged() {
        // Regression: when no learnings are injected the prompt must not gain
        // any leading whitespace or knowledge-block headers.
        let text = build_prompt(
            TaskId(1),
            "title",
            "desc",
            None,
            None,
            &PromptContext::default(),
        );
        assert!(text.starts_with("Your task is:"));
        assert!(!text.contains("Validated knowledge for this task"));
    }

    #[test]
    fn build_prompt_with_injections_includes_knowledge_block() {
        let procedural_l = {
            let mut l = seed(10, LearningScope::User, 0);
            l.kind = LearningKind::Procedural;
            l.detail = Some("Always run tests before committing.".into());
            l
        };
        let convention_l = seed(11, LearningScope::Repo, 2);
        let injections = LearningInjections {
            ranked: vec![&procedural_l, &convention_l],
        };
        let ctx = PromptContext {
            learnings: injections,
            tag: None,
            auto_run_plan: false,
        };
        let text = build_prompt(TaskId(1), "title", "desc", None, None, &ctx);
        // Procedural learnings no longer appear as a verbatim prefix — prompt
        // always starts with the task block.
        assert!(text.starts_with("Your task is:"));
        assert!(text.contains("## Validated knowledge for this task"));
        // Both learnings appear in the validated-knowledge block.
        assert!(text.contains("[#10 user, \u{2191}0]"));
        assert!(text.contains("[#11 repo, \u{2191}2]"));
    }

    #[test]
    fn build_quick_dispatch_prompt_default_injections_unchanged() {
        let text = build_quick_dispatch_prompt(
            TaskId(1),
            "title",
            "desc",
            None,
            &PromptContext::default(),
        );
        assert!(text.starts_with("You are working interactively with the user."));
        assert!(!text.contains("Validated knowledge for this task"));
    }

    #[test]
    fn build_prompt_with_dependabot_tag_includes_review_section() {
        let ctx = PromptContext {
            tag: Some(TaskTag::Dependabot),
            ..PromptContext::default()
        };
        let text = build_prompt(
            TaskId(42),
            "Bump serde from 1.0.0 to 1.0.1",
            "https://github.com/example/repo/pull/7",
            None,
            None,
            &ctx,
        );

        assert!(text.contains("Dependabot PR review"), "missing role line");
        assert!(text.contains("gh pr view"));
        assert!(text.contains("gh pr diff"));
        assert!(text.contains("gh pr checks"));
        assert!(text.contains("gh pr review"));
        assert!(text.contains("--approve"));
        assert!(text.contains("gh pr merge"));
        assert!(text.contains("--squash --auto"));
        assert!(text.contains("patch"));
        assert!(text.contains("minor"));
        assert!(text.contains("major"));
        assert!(text.contains("CHANGELOG"));
        assert!(text.contains("BREAKING"));
        assert!(text.contains("update_task(task_id=42, url="));
        assert!(text.contains("url_type=\"pr\""));
        assert!(text.contains("needs_input"));
        // Must NOT call /wrap-up — task auto-cleans on PR merge.
        assert!(
            text.contains("Do NOT call /wrap-up"),
            "dependabot prompt must explicitly forbid /wrap-up"
        );
        // The standard trailing wrap-up instruction must not be present.
        assert!(
            !text.contains("use the /wrap-up skill"),
            "dependabot prompt must omit the standard wrap-up instruction"
        );
        // No TDD / allium — this agent doesn't edit code.
        assert!(
            !text.contains("Always use TDD"),
            "dependabot prompt must omit the TDD instruction"
        );
        // The standard plan-or-brainstorm addendum must be replaced.
        assert!(
            !text.contains("/brainstorming"),
            "dependabot prompt must omit the brainstorming addendum"
        );
    }

    #[test]
    fn build_prompt_without_dependabot_tag_omits_review_section() {
        let text = build_prompt(
            TaskId(1),
            "title",
            "desc",
            None,
            None,
            &PromptContext::default(),
        );
        assert!(!text.contains("Dependabot PR review"));
        assert!(!text.contains("gh pr merge"));
    }

    #[test]
    fn build_prompt_with_pr_review_tag_includes_review_commands() {
        let ctx = PromptContext {
            tag: Some(TaskTag::PrReview),
            ..PromptContext::default()
        };
        let text = build_prompt(
            TaskId(42),
            "Review PR: Add new login flow",
            "https://github.com/example/repo/pull/99",
            None,
            None,
            &ctx,
        );

        assert!(
            text.contains("/review"),
            "pr-review prompt must reference /review skill"
        );
        assert!(
            text.contains("/review-pr"),
            "pr-review prompt must reference /review-pr skill"
        );
        assert!(
            text.contains("diff"),
            "pr-review prompt must instruct checking the diff"
        );
    }

    #[test]
    fn build_prompt_with_pr_review_tag_omits_plan_and_brainstorm_instructions() {
        let ctx = PromptContext {
            tag: Some(TaskTag::PrReview),
            ..PromptContext::default()
        };
        let text = build_prompt(
            TaskId(42),
            "Review PR: Add new login flow",
            "https://github.com/example/repo/pull/99",
            None,
            None,
            &ctx,
        );

        assert!(
            !text.contains("/brainstorming"),
            "pr-review prompt must NOT contain /brainstorming"
        );
        assert!(
            !text.contains("implementation plan"),
            "pr-review prompt must NOT mention implementation plan"
        );
        assert!(
            !text.contains("docs/plans/"),
            "pr-review prompt must NOT reference docs/plans/"
        );
    }

    #[test]
    fn build_prompt_with_pr_review_tag_omits_tdd_and_allium_instructions() {
        let ctx = PromptContext {
            tag: Some(TaskTag::PrReview),
            ..PromptContext::default()
        };
        let text = build_prompt(
            TaskId(42),
            "Review PR: Add new login flow",
            "https://github.com/example/repo/pull/99",
            None,
            None,
            &ctx,
        );

        assert!(
            !text.contains("Always use TDD"),
            "pr-review prompt must NOT contain TDD instruction"
        );
        assert!(
            !text.contains("Allium specs"),
            "pr-review prompt must NOT contain allium instruction"
        );
    }

    #[test]
    fn build_prompt_with_pr_review_tag_omits_wrap_up() {
        let ctx = PromptContext {
            tag: Some(TaskTag::PrReview),
            ..PromptContext::default()
        };
        let text = build_prompt(
            TaskId(42),
            "Review PR: Add new login flow",
            "https://github.com/example/repo/pull/99",
            None,
            None,
            &ctx,
        );

        assert!(
            text.contains("Do NOT call /wrap-up"),
            "pr-review prompt must explicitly forbid /wrap-up by name"
        );
        assert!(
            !text.contains("use the /wrap-up skill"),
            "pr-review prompt must omit the standard wrap-up instruction"
        );
    }

    #[test]
    fn build_prompt_with_pr_review_tag_includes_mcp_and_learning_instructions() {
        let ctx = PromptContext {
            tag: Some(TaskTag::PrReview),
            ..PromptContext::default()
        };
        let text = build_prompt(
            TaskId(42),
            "Review PR: Add new login flow",
            "https://github.com/example/repo/pull/99",
            None,
            None,
            &ctx,
        );

        assert!(
            text.contains("dispatch MCP tools"),
            "pr-review prompt must include MCP tools instruction"
        );
        assert!(
            text.contains("query_learnings"),
            "pr-review prompt must include learning tools instruction"
        );
    }

    /// No prompt variant may render a "## Verification" section — see the
    /// unified prompt skeleton in `docs/specs/dispatch.allium`.
    ///
    /// The builders take no verify input, so this can only regress through
    /// hardcoded prompt copy. That is exactly the case the snapshots don't
    /// catch: `INSTA_UPDATE=always` silently accepts a reintroduced section,
    /// whereas a named test cannot be blanket-accepted.
    #[test]
    fn no_prompt_variant_renders_a_verification_section() {
        let ctx = PromptContext::default();
        let variants = [
            (
                "dispatch",
                build_prompt(TaskId(1), "t", "d", None, None, &ctx),
            ),
            (
                "quick dispatch",
                build_quick_dispatch_prompt(TaskId(1), "t", "d", None, &ctx),
            ),
            (
                "research",
                build_research_prompt(TaskId(1), "t", "d", None, &ctx),
            ),
        ];
        for (variant, text) in variants {
            assert!(
                !text.contains("## Verification"),
                "{variant} prompt must not render a verification section"
            );
            assert!(
                !text.contains("Before declaring work complete"),
                "{variant} prompt must not carry the verification instruction"
            );
        }
    }
}

#[cfg(test)]
#[allow(clippy::unwrap_used, clippy::expect_used)]
mod rag_dispatch_tests {
    use std::sync::Arc;

    use crate::db::{
        CreateLearningRow, CreateTaskRequest, Database, LearningRetrievalStore, LearningStore,
        TaskCrud, TaskRead,
    };
    use crate::models::{LearningKind, LearningScope, TaskStatus};
    use crate::service::embeddings::{serialize_embedding, EmbeddingService};

    use super::{
        build_and_record_injections, list_learnings_for_dispatch_rag, DISPATCH_INJECTION_CAP,
    };

    // The test EmbeddingService returns vec![0.1f32; 384]. Use the same dimensionality
    // for stored embeddings so cosine similarity is computed correctly.
    fn fake_emb_bytes() -> Vec<u8> {
        serialize_embedding(&vec![0.1f32; 384])
    }

    async fn seed_db() -> Arc<Database> {
        Arc::new(Database::open_in_memory().await.unwrap())
    }

    async fn make_task(db: &Arc<Database>) -> crate::models::Task {
        let id = db
            .create_task(CreateTaskRequest {
                title: "test task",
                description: "test description",
                repo_path: "/repo/test",
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
            .unwrap();
        db.get_task(id).await.unwrap().unwrap()
    }

    #[tokio::test]
    async fn dispatch_injection_includes_procedural_learnings_without_prioritizing_them() {
        let db = seed_db().await;
        let task = make_task(&db).await;
        let emb = fake_emb_bytes();

        let proc_id = db
            .create_learning(CreateLearningRow {
                kind: LearningKind::Procedural,
                summary: "always run clippy",
                detail: None,
                scope: LearningScope::User,
                scope_ref: None,
                tags: &[],
                source_task_id: None,
                embedding: Some(&emb),
            })
            .await
            .unwrap();

        for i in 0..2 {
            db.create_learning(CreateLearningRow {
                kind: LearningKind::Convention,
                summary: &format!("convention {i}"),
                detail: None,
                scope: LearningScope::User,
                scope_ref: None,
                tags: &[],
                source_task_id: None,
                embedding: Some(&emb),
            })
            .await
            .unwrap();
        }

        let emb_svc = EmbeddingService::new_test();
        // threshold=0.0 so all candidates pass the cosine filter
        let results = list_learnings_for_dispatch_rag(&*db, &task, &emb_svc, 0.0).await;

        assert!(!results.is_empty(), "should return at least one learning");
        // Procedural learnings are still included — just not artificially first.
        let ids: Vec<_> = results.iter().map(|l| l.id).collect();
        assert!(
            ids.contains(&proc_id),
            "procedural learning must be in results"
        );
    }

    #[tokio::test]
    async fn dispatch_injection_excludes_task_scoped_learnings() {
        let db = seed_db().await;
        let task = make_task(&db).await;
        let emb = fake_emb_bytes();

        // Task-scoped learning — should be excluded by list_all_approved_non_task_learnings
        db.create_learning(CreateLearningRow {
            kind: LearningKind::Convention,
            summary: "task-scoped learning",
            detail: None,
            scope: LearningScope::Task,
            scope_ref: Some(&task.id.0.to_string()),
            tags: &[],
            source_task_id: Some(task.id),
            embedding: Some(&emb),
        })
        .await
        .unwrap();

        let emb_svc = EmbeddingService::new_test();
        let results = list_learnings_for_dispatch_rag(&*db, &task, &emb_svc, 0.0).await;

        assert!(
            results.iter().all(|l| l.scope != LearningScope::Task),
            "task-scoped learnings must not appear in dispatch injection"
        );
    }

    #[tokio::test]
    async fn dispatch_injection_respects_cap_of_5() {
        let db = seed_db().await;
        let task = make_task(&db).await;
        let emb = fake_emb_bytes();

        // Seed 8 approved non-task learnings with embeddings
        for i in 0..8 {
            db.create_learning(CreateLearningRow {
                kind: LearningKind::Convention,
                summary: &format!("convention {i}"),
                detail: None,
                scope: LearningScope::User,
                scope_ref: None,
                tags: &[],
                source_task_id: None,
                embedding: Some(&emb),
            })
            .await
            .unwrap();
        }

        let emb_svc = EmbeddingService::new_test();
        let results = list_learnings_for_dispatch_rag(&*db, &task, &emb_svc, 0.0).await;

        assert_eq!(
            results.len(),
            DISPATCH_INJECTION_CAP,
            "should return at most DISPATCH_INJECTION_CAP ({DISPATCH_INJECTION_CAP}) learnings"
        );
    }

    #[tokio::test]
    async fn dispatch_injection_excludes_learnings_without_embeddings() {
        let db = seed_db().await;
        let task = make_task(&db).await;
        let emb = fake_emb_bytes();

        // One learning with embedding, one without
        let with_emb_id = db
            .create_learning(CreateLearningRow {
                kind: LearningKind::Convention,
                summary: "has embedding",
                detail: None,
                scope: LearningScope::User,
                scope_ref: None,
                tags: &[],
                source_task_id: None,
                embedding: Some(&emb),
            })
            .await
            .unwrap();

        let no_emb_id = db
            .create_learning(CreateLearningRow {
                kind: LearningKind::Convention,
                summary: "no embedding",
                detail: None,
                scope: LearningScope::User,
                scope_ref: None,
                tags: &[],
                source_task_id: None,
                embedding: None,
            })
            .await
            .unwrap();

        let emb_svc = EmbeddingService::new_test();
        let results = list_learnings_for_dispatch_rag(&*db, &task, &emb_svc, 0.0).await;

        assert!(
            results.iter().any(|l| l.id == with_emb_id),
            "learning with embedding should be included"
        );
        assert!(
            results.iter().all(|l| l.id != no_emb_id),
            "learning without embedding should be excluded"
        );
    }

    #[tokio::test]
    async fn build_and_record_injections_records_all_as_prompt_injection() {
        let db = seed_db().await;
        let task = make_task(&db).await;
        let emb = fake_emb_bytes();

        let proc_id = db
            .create_learning(CreateLearningRow {
                kind: LearningKind::Procedural,
                summary: "always run tests",
                detail: None,
                scope: LearningScope::User,
                scope_ref: None,
                tags: &[],
                source_task_id: None,
                embedding: Some(&emb),
            })
            .await
            .unwrap();

        let conv_id = db
            .create_learning(CreateLearningRow {
                kind: LearningKind::Convention,
                summary: "use Arc for shared state",
                detail: None,
                scope: LearningScope::Repo,
                scope_ref: Some("/repo/test"),
                tags: &[],
                source_task_id: None,
                embedding: Some(&emb),
            })
            .await
            .unwrap();

        let emb_svc = EmbeddingService::new_test();
        let injected = build_and_record_injections(&*db, &task, &emb_svc).await;

        assert_eq!(injected.len(), 2);
        let ids: Vec<_> = injected.iter().map(|l| l.id).collect();
        assert!(ids.contains(&proc_id));
        assert!(ids.contains(&conv_id));

        // All retrievals recorded as PromptInjection regardless of kind.
        let rows = db.list_retrievals_for_task(task.id).await.unwrap();
        assert_eq!(rows.len(), 2);
        assert!(rows
            .iter()
            .all(|r| matches!(r.source, crate::models::RetrievalSource::PromptInjection)));
    }
}
