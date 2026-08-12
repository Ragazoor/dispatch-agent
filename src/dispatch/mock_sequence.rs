//! One declaration of the subprocess call sequence a dispatch issues, for
//! [`MockProcessRunner`]-based tests.
//!
//! # Why this exists
//!
//! `dispatch_with_prompt` → `provision_worktree` → tmux launch issues a fixed
//! sequence of `git` / `gh` / `tmux` calls, and `MockProcessRunner` answers from
//! a positional queue. Before this module, every test that drove a dispatch
//! hand-wrote that queue as a `vec![ok(), ok(), …]` with a trailing comment per
//! entry, then asserted against hard-coded `calls[N]` indices. Three costs:
//!
//! - Six of the twelve steps are conditional, so the index of every later step
//!   depends on choices the test made earlier — which each site re-derived.
//! - Adding one preflight call meant splicing a response into ~45 vectors at the
//!   right offset and renumbering by hand. #3810 did exactly that and still left
//!   a stale entry behind, caught only because a reviewer counted calls manually.
//! - A stale entry is invisible: a spare `ok_with_stdout` is silently consumed by
//!   whatever call comes next, because most callers ignore stdout.
//!
//! A [`DispatchScript`] declares the *shape* once — which optional steps are
//! present, how the fetch goes, where the sequence stops — and derives both the
//! response queue and the call indices from that one declaration, so the two
//! cannot disagree.
//!
//! # Assertions get stronger, not weaker
//!
//! [`DispatchScript::index_of`] replaces a literal index with a named step, and
//! [`DispatchScript::assert_matches`] turns "this queue is the right sequence"
//! from a trailing comment into a checked claim: an extra, missing, reordered, or
//! stale call fails. Tests that pin exact argv keep doing so — this module only
//! decides *which* call to look at, never what to assert about it.
//!
//! # Response index == recorded-call index
//!
//! `MockProcessRunner` answers `tmux::window_target`'s name lookup out of band
//! (see `WindowLookup::AnyName`): it is neither taken from the queue nor
//! recorded. This module relies on that, so a script's Nth step really is
//! `recorded_calls()[N]`. A test using `with_queued_window_lookup` cannot use a
//! script.

use std::process::Output;
use std::time::Duration;

use anyhow::Result;

use super::worktree::FETCH_MAX_ATTEMPTS;
use crate::process::MockProcessRunner;
use crate::tmux;

/// tmux's reply to the companion pane's `split-window -P`: the new pane's id.
/// Arbitrary but non-positional on purpose — a pane id that does not look like
/// an index cannot be mistaken for one.
pub(crate) const COMPANION_PANE_ID: &[u8] = b"%9\n";

/// `git ls-remote --exit-code`'s "no matching ref" status, i.e. the only failure
/// `classify_fetch_failure` reads as a positive 404.
const LS_REMOTE_NO_MATCHING_REF: i32 = 2;
/// Any other `ls-remote` failure: could not reach the remote at all.
const LS_REMOTE_UNREACHABLE: i32 = 128;

/// The stderr a failing `git fetch origin <base>` stands in with.
const FETCH_FAILURE: &str = "fatal: unable to access 'origin': transient network error";

/// `git symbolic-ref refs/remotes/origin/HEAD`'s reply for a repo whose default
/// branch is `branch` — the form `crate::git::detect_default_branch` parses.
fn default_branch_ref(branch: &str) -> Vec<u8> {
    format!("refs/remotes/origin/{branch}\n").into_bytes()
}

/// `gh pr view --json headRefName,isCrossRepository`'s reply: the head branch,
/// then whether the PR comes from a fork, one per line. Stated here once so no
/// call site has to re-derive that the second line drives the fork fallback.
pub(crate) fn pr_view_reply(head: &str, cross_repository: bool) -> Vec<u8> {
    format!("{head}\n{cross_repository}\n").into_bytes()
}

/// `git rev-list --count --left-right <base>...origin/<base>`'s reply: commits
/// local-only, then commits origin-only, tab separated. `level()` is the common
/// case and states once that a zero pair means "no drift, so prefer origin".
fn rev_list_counts(ahead: u32, behind: u32) -> Vec<u8> {
    format!("{ahead}\t{behind}\n").into_bytes()
}

/// One recorded subprocess call in a dispatch's sequence.
///
/// Ordered as issued. [`Step::Fetch`] is the only one that can repeat (see
/// `fetch_origin`); [`DispatchScript::index_of`] reports its first attempt.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub(crate) enum Step {
    /// `git symbolic-ref refs/remotes/origin/HEAD` — only when the caller passed
    /// no base branch, i.e. quick dispatch.
    DetectDefaultBranch,
    /// `gh pr view` — only for a review-tagged task carrying a PR url.
    PrHeadLookup,
    /// `git fetch origin <base>`, retried under `FetchPolicy::Required`.
    Fetch,
    /// `git remote get-url origin` — the first half of `classify_fetch_failure`,
    /// issued once after the first failed fetch under `FetchPolicy::Required`.
    OriginProbe,
    /// `git ls-remote --exit-code origin refs/heads/<base>` — the second half,
    /// which tells a 404 (fall back to local) from an unreachable remote.
    LsRemote,
    /// `git rev-list --count --left-right <base>...origin/<base>` — `select_start_point`'s
    /// measurement. Only for `BaseRef::Branch`, and only after a fetch succeeded:
    /// a PR head branch is never compared against a local ref.
    AheadBehind,
    /// `git worktree add` — only when the worktree directory does not exist yet.
    WorktreeAdd,
    /// `tmux new-window`
    NewWindow,
    /// `tmux set-option -w … @dispatch_dir`
    SetDispatchDir,
    /// `tmux set-hook after-split-window`
    SetSplitHook,
    /// `tmux send-keys -l <the claude command>`
    SendKeysLiteral,
    /// `tmux send-keys … Enter`
    SendKeysEnter,
    /// `tmux split-window` for the `dispatch agent-tree` companion pane.
    CompanionSplit,
    /// `tmux set-option -p … @dispatch_pane_role agent_tree` on the pane that
    /// split just returned — how dispatch will recognise that pane again.
    CompanionRoleMark,
}

impl Step {
    /// Whether a recorded call is this step, by program plus the argv token that
    /// distinguishes it from its siblings.
    fn matches(self, program: &str, args: &[String]) -> bool {
        let has = |needle: &str| args.iter().any(|a| a == needle);
        let command_is = |name: &str| args.first().is_some_and(|a| a == name);
        match self {
            Step::DetectDefaultBranch => program == "git" && has("symbolic-ref"),
            Step::PrHeadLookup => program == "gh" && has("view"),
            Step::Fetch => program == "git" && has("fetch"),
            Step::OriginProbe => program == "git" && has("remote") && has("get-url"),
            Step::LsRemote => program == "git" && has("ls-remote"),
            Step::AheadBehind => program == "git" && has("rev-list"),
            Step::WorktreeAdd => program == "git" && has("worktree"),
            Step::NewWindow => program == "tmux" && command_is("new-window"),
            // The two `set-option` calls differ in scope and in which option they
            // name: the window's dispatch dir, and the new pane's role.
            Step::SetDispatchDir => {
                program == "tmux" && command_is("set-option") && has("@dispatch_dir")
            }
            Step::CompanionRoleMark => {
                program == "tmux" && command_is("set-option") && has(tmux::PANE_ROLE_OPTION)
            }
            Step::SetSplitHook => program == "tmux" && command_is("set-hook"),
            // The two send-keys calls differ only by `-l`, which is exactly what
            // separates the payload from the Enter that submits it.
            Step::SendKeysLiteral => program == "tmux" && command_is("send-keys") && has("-l"),
            Step::SendKeysEnter => program == "tmux" && command_is("send-keys") && !has("-l"),
            Step::CompanionSplit => program == "tmux" && command_is("split-window"),
        }
    }
}

/// What `gh pr view` reports for a review task's PR.
#[derive(Clone, Copy)]
pub(crate) enum PrHead {
    /// A same-repo PR. `provision_worktree` receives `BaseRef::PrHead`, so no
    /// [`Step::AheadBehind`] is issued.
    Branch(&'static str),
    /// A fork PR: resolvable, but soft-falls back to the base branch — which
    /// means the dispatch proceeds as `BaseRef::Branch` and *does* measure.
    Fork(&'static str),
    /// `gh` itself failed, so nothing is resolved and the base branch is used —
    /// likewise a measuring path.
    Unresolvable,
}

impl PrHead {
    /// Whether this outcome leaves the dispatch on the PR head branch, i.e. the
    /// one case that skips the ahead/behind measurement.
    fn resolves_to_pr_head(self) -> bool {
        matches!(self, PrHead::Branch(_))
    }
}

/// How `git fetch origin <base>` goes. Which of these are even reachable depends
/// on the fetch policy `provision_worktree` picks, which is itself decided by
/// whether the worktree directory already exists — see [`DispatchScript::attempts`].
#[derive(Clone, Copy)]
enum FetchOutcome {
    /// No base ref was resolved, so no fetch is attempted at all.
    Absent,
    /// Attempts `1..n` fail, attempt `n` succeeds. `1` is the plain happy path;
    /// anything higher needs `FetchPolicy::Required`, i.e. a fresh worktree.
    SucceedsOnAttempt(u32),
    /// Every attempt fails and the classification probe positively identifies a
    /// missing ref. Not retried — retrying a branch that does not exist cannot
    /// succeed — so exactly one attempt plus the two probes.
    NoOriginRef,
    /// Every attempt fails and the remote is unreachable. Under `Required` this
    /// costs the probes plus the full retry budget and then aborts the dispatch;
    /// under `BestEffort` (reuse path) it is one attempt, no probes, a warning.
    Unreachable,
    /// Every attempt answers *successfully* but takes longer than the caller's
    /// timeout, so `run_with_timeout` kills it and reports an error instead.
    ///
    /// Deliberately distinct from [`Self::Unreachable`]: a plain failure would
    /// pass even if `provision_worktree` regressed to the unbounded `run`, so the
    /// watchdog tests need the response itself to be a success.
    TimesOut(Duration),
}

/// Where the sequence ends.
#[derive(Clone, Copy)]
enum Ending {
    /// Runs through [`Step::CompanionRoleMark`], i.e. the whole companion-pane
    /// tail.
    Complete,
    /// This step succeeds and nothing beyond it is queued — for a dispatch that
    /// stops for a reason other than a subprocess failure (e.g. the
    /// `.claude-prompt` write failing because the mock never created the
    /// worktree directory).
    StopsAfter(Step),
    /// This step returns a failure and nothing beyond it is queued, so a call
    /// the code should not have made panics the mock instead of passing.
    FailsAt(Step),
}

/// The shape of one dispatch's call sequence.
///
/// Holds configuration only — no responses — so it stays usable for
/// [`Self::index_of`] and [`Self::assert_matches`] after [`Self::runner`] has
/// built the queue.
#[derive(Clone, Copy)]
pub(crate) struct DispatchScript {
    default_branch: Option<&'static str>,
    pr_head: Option<PrHead>,
    fetch: FetchOutcome,
    /// Commits local `<base>` holds that origin lacks, as reported by
    /// [`Step::AheadBehind`]. `0` means "no drift", so origin wins.
    local_ahead: u32,
    fresh_worktree: bool,
    ending: Ending,
}

impl DispatchScript {
    /// One successful dispatch: an existing worktree directory, a fetch that
    /// succeeds first try, a level ahead/behind reading, and a launch that runs
    /// through the companion pane.
    ///
    /// The default because it is what most dispatch tests want: they pre-create
    /// the worktree directory so the `.claude-prompt` write succeeds, which is
    /// itself what puts provisioning on the reuse branch.
    pub(crate) fn dispatch() -> Self {
        Self {
            default_branch: None,
            pr_head: None,
            fetch: FetchOutcome::SucceedsOnAttempt(1),
            local_ahead: 0,
            fresh_worktree: false,
            ending: Ending::Complete,
        }
    }

    /// `resume_agent`'s sequence: the tmux tail only, since resume reuses the
    /// worktree that already exists and touches git not at all.
    pub(crate) fn resume() -> Self {
        Self::dispatch().no_fetch()
    }

    /// `provision_worktree` called directly: everything up to and including the
    /// split hook, with no prompt write and no agent launch.
    pub(crate) fn provision() -> Self {
        Self::dispatch().stops_after(Step::SetSplitHook)
    }

    /// The worktree directory does not exist yet, so `git worktree add` runs —
    /// and the fetch becomes `FetchPolicy::Required`, which is what makes the
    /// retry budget and the classification probes reachable.
    pub(crate) fn fresh_worktree(mut self) -> Self {
        self.fresh_worktree = true;
        self
    }

    /// No base ref was resolved, so no fetch is attempted.
    pub(crate) fn no_fetch(mut self) -> Self {
        self.fetch = FetchOutcome::Absent;
        self
    }

    /// The fetch fails until attempt `n`, which succeeds. Needs a fresh worktree
    /// for `n > 1`: the reuse path only ever makes one attempt.
    pub(crate) fn fetch_succeeds_on_attempt(mut self, n: u32) -> Self {
        assert!(n >= 1, "fetch attempts are 1-based");
        self.fetch = FetchOutcome::SucceedsOnAttempt(n);
        self
    }

    /// The fetch fails and the probes identify a missing origin ref, so the local
    /// branch is used with a `Note:` and no retry happens.
    pub(crate) fn fetch_finds_no_origin_ref(mut self) -> Self {
        self.fetch = FetchOutcome::NoOriginRef;
        self
    }

    /// The fetch fails with the remote unreachable. On a fresh worktree that
    /// aborts the dispatch after the probes and the full budget; on the reuse
    /// path it is one attempt, no probes, and a warning.
    pub(crate) fn fetch_is_unreachable(mut self) -> Self {
        self.fetch = FetchOutcome::Unreachable;
        self
    }

    /// Every fetch attempt succeeds but takes `delay` to answer, so a caller
    /// passing a shorter timeout kills it instead. See
    /// [`FetchOutcome::TimesOut`] for why this is not the same as
    /// [`Self::fetch_is_unreachable`].
    pub(crate) fn fetch_times_out(mut self, delay: Duration) -> Self {
        self.fetch = FetchOutcome::TimesOut(delay);
        self
    }

    /// Local `<base>` holds `n` commits origin lacks, so `select_start_point`
    /// prefers the local ref. The default is `0` — no drift, origin wins.
    pub(crate) fn local_ahead(mut self, n: u32) -> Self {
        self.local_ahead = n;
        self
    }

    /// The caller passed no base branch, so the dispatch detects the repo's
    /// default branch first and then fetches `branch`.
    pub(crate) fn detecting_default_branch(mut self, branch: &'static str) -> Self {
        self.default_branch = Some(branch);
        self
    }

    /// This is a review task carrying a PR url, so `gh pr view` runs first.
    pub(crate) fn pr_head(mut self, head: PrHead) -> Self {
        self.pr_head = Some(head);
        self
    }

    /// `step` succeeds and nothing after it is queued. See [`Ending::StopsAfter`].
    ///
    /// Private because [`Self::provision`] is the only shape that needs it; widen
    /// it if a test ever needs a different stopping point.
    fn stops_after(mut self, step: Step) -> Self {
        self.ending = Ending::StopsAfter(step);
        self
    }

    /// `step` fails and nothing after it is queued. See [`Ending::FailsAt`].
    pub(crate) fn fails_at(mut self, step: Step) -> Self {
        assert!(
            !matches!(step, Step::Fetch | Step::OriginProbe | Step::LsRemote),
            "a failing fetch is retried and classified, not terminal — use one of \
             the fetch_* modifiers instead"
        );
        self.ending = Ending::FailsAt(step);
        self
    }

    /// Whether the fetch runs under `FetchPolicy::Required`, which is what makes
    /// retries and the classification probes reachable. Mirrors
    /// `provision_worktree`: the policy follows the worktree directory's
    /// existence, nothing else.
    fn fetch_is_required(&self) -> bool {
        self.fresh_worktree
    }

    /// How many `git fetch` calls this shape issues.
    fn attempts(&self) -> u32 {
        match self.fetch {
            FetchOutcome::Absent => 0,
            FetchOutcome::SucceedsOnAttempt(n) => n,
            // A positively-identified 404 is never retried.
            FetchOutcome::NoOriginRef => 1,
            FetchOutcome::Unreachable | FetchOutcome::TimesOut(_) => {
                if self.fetch_is_required() {
                    FETCH_MAX_ATTEMPTS
                } else {
                    1
                }
            }
        }
    }

    /// Whether `classify_fetch_failure`'s two probes run: only when a fetch
    /// actually failed *and* the policy is `Required`, since `BestEffort` never
    /// needs to tell a 404 from an outage.
    fn classifies(&self) -> bool {
        if !self.fetch_is_required() {
            return false;
        }
        match self.fetch {
            FetchOutcome::Absent => false,
            FetchOutcome::SucceedsOnAttempt(n) => n > 1,
            FetchOutcome::NoOriginRef | FetchOutcome::Unreachable | FetchOutcome::TimesOut(_) => {
                true
            }
        }
    }

    /// Whether `select_start_point` measures local against origin. Requires a
    /// fetch that ultimately succeeded, and a base that is not a PR head.
    fn measures(&self) -> bool {
        let fetched = matches!(self.fetch, FetchOutcome::SucceedsOnAttempt(_));
        let on_pr_head = self.pr_head.is_some_and(PrHead::resolves_to_pr_head);
        fetched && !on_pr_head
    }

    /// Whether the fetch itself ends the dispatch. Under `FetchPolicy::Required`
    /// an unreachable origin is a hard error — a worktree silently branched off a
    /// stale local ref is worse than a dispatch that refuses to start — so
    /// `provision_worktree` propagates it and nothing after the last attempt runs.
    ///
    /// A 404 is *not* an abort for a branch base: local `<base>` is then the only
    /// ref there is, so provisioning continues on it.
    fn fetch_aborts(&self) -> bool {
        self.fetch_is_required()
            && matches!(
                self.fetch,
                FetchOutcome::Unreachable | FetchOutcome::TimesOut(_)
            )
    }

    /// The steps this shape issues, one entry per recorded call, in order.
    fn steps(&self) -> Vec<Step> {
        let mut steps = Vec::new();
        if self.default_branch.is_some() {
            steps.push(Step::DetectDefaultBranch);
        }
        if self.pr_head.is_some() {
            steps.push(Step::PrHeadLookup);
        }
        // The classification probes sit *between* attempt 1 and the rest:
        // `fetch_origin` classifies once, on the first failure.
        let attempts = self.attempts();
        for attempt in 1..=attempts {
            steps.push(Step::Fetch);
            if attempt == 1 && self.classifies() {
                steps.push(Step::OriginProbe);
                steps.push(Step::LsRemote);
            }
        }
        // An aborting fetch is itself the end of the sequence: `provision_worktree`
        // propagates the error, so no start point is measured and no worktree or
        // tmux window is created.
        if self.fetch_aborts() {
            return steps;
        }
        if self.measures() {
            steps.push(Step::AheadBehind);
        }
        if self.fresh_worktree {
            steps.push(Step::WorktreeAdd);
        }
        steps.extend([
            Step::NewWindow,
            Step::SetDispatchDir,
            Step::SetSplitHook,
            Step::SendKeysLiteral,
            Step::SendKeysEnter,
            Step::CompanionSplit,
            Step::CompanionRoleMark,
        ]);

        let last = match self.ending {
            Ending::Complete => return steps,
            Ending::StopsAfter(step) | Ending::FailsAt(step) => step,
        };
        let cut = steps
            .iter()
            .position(|s| *s == last)
            .unwrap_or_else(|| panic!("{last:?} is not part of this script's sequence"));
        steps.truncate(cut + 1);
        steps
    }

    /// The recorded-call index of `step` — its *first* call, for the repeatable
    /// [`Step::Fetch`].
    ///
    /// # Panics
    ///
    /// If `step` is not part of this shape. Asserting against a call the script
    /// never queued is a test bug, and a loud one beats an index that silently
    /// points at a neighbouring call.
    pub(crate) fn index_of(&self, step: Step) -> usize {
        self.steps()
            .iter()
            .position(|s| *s == step)
            .unwrap_or_else(|| panic!("{step:?} is not part of this script's sequence"))
    }

    /// The response queue for this shape, ready to inject as a `ProcessRunner`.
    pub(crate) fn runner(&self) -> MockProcessRunner {
        MockProcessRunner::new_with_delays(self.responses())
    }

    /// [`Self::runner`] behind an `Arc`, for the async fixtures that hold their
    /// runner as one.
    pub(crate) fn shared_runner(&self) -> std::sync::Arc<MockProcessRunner> {
        std::sync::Arc::new(self.runner())
    }

    /// One `(delay, response)` pair per step, in order. Only `Fetch` ever carries
    /// a delay (see [`Self::fetch_times_out`]).
    fn responses(&self) -> Vec<(Option<Duration>, Result<Output>)> {
        let steps = self.steps();
        let failing_last = matches!(self.ending, Ending::FailsAt(_));
        let mut fetch_attempt = 0;
        let mut out = Vec::with_capacity(steps.len());
        for (i, step) in steps.iter().copied().enumerate() {
            // The terminal step of a `fails_at` shape reports failure; every
            // other step answers as its own kind dictates.
            if failing_last && i + 1 == steps.len() {
                out.push((None, MockProcessRunner::fail(failure_stderr(step))));
                continue;
            }
            let (delay, response) = match step {
                Step::Fetch => {
                    fetch_attempt += 1;
                    self.fetch_response(fetch_attempt)
                }
                // The probe answers "yes, there is an origin"; it is `ls-remote`
                // that then distinguishes the two failure classes.
                Step::OriginProbe => (None, MockProcessRunner::ok()),
                Step::LsRemote => (None, self.ls_remote_response()),
                Step::AheadBehind => (
                    None,
                    MockProcessRunner::ok_with_stdout(&rev_list_counts(self.local_ahead, 0)),
                ),
                // Both lookups answer from the very option that put them in the
                // sequence, so `steps()` has already ruled out the `None` arm and
                // no fallback value has to be invented for it.
                Step::DetectDefaultBranch => match self.default_branch {
                    Some(branch) => (
                        None,
                        MockProcessRunner::ok_with_stdout(&default_branch_ref(branch)),
                    ),
                    None => unreachable!("DetectDefaultBranch is only emitted for Some(branch)"),
                },
                Step::PrHeadLookup => match self.pr_head {
                    Some(head) => (None, pr_head_response(head)),
                    None => unreachable!("PrHeadLookup is only emitted for Some(head)"),
                },
                Step::CompanionSplit => {
                    (None, MockProcessRunner::ok_with_stdout(COMPANION_PANE_ID))
                }
                _ => (None, MockProcessRunner::ok()),
            };
            out.push((delay, response));
        }
        out
    }

    /// How fetch attempt `n` (1-based) answers under this shape's outcome, with
    /// the delay that makes a timeout shape time out.
    fn fetch_response(&self, n: u32) -> (Option<Duration>, Result<Output>) {
        match self.fetch {
            // A success the caller's timeout never waits long enough to see.
            FetchOutcome::TimesOut(delay) => (Some(delay), MockProcessRunner::ok()),
            FetchOutcome::SucceedsOnAttempt(target) if n >= target => {
                (None, MockProcessRunner::ok())
            }
            _ => (None, MockProcessRunner::fail(FETCH_FAILURE)),
        }
    }

    /// `ls-remote --exit-code`'s status is the whole classification: 2 is a
    /// positively-identified missing ref, anything else is "could not reach".
    fn ls_remote_response(&self) -> Result<Output> {
        let code = match self.fetch {
            FetchOutcome::NoOriginRef => LS_REMOTE_NO_MATCHING_REF,
            _ => LS_REMOTE_UNREACHABLE,
        };
        MockProcessRunner::fail_with_code(code, "")
    }

    /// Assert `calls` is exactly the sequence this shape declares.
    ///
    /// The point of the whole module: a queue that has drifted from the code no
    /// longer sits there as a stale comment, it fails here.
    pub(crate) fn assert_matches(&self, calls: &[(String, Vec<String>)]) {
        let steps = self.steps();
        for (i, step) in steps.iter().enumerate() {
            let Some((program, args)) = calls.get(i) else {
                panic!(
                    "expected {} calls, got {}; missing {step:?} at index {i}. Recorded: {calls:#?}",
                    steps.len(),
                    calls.len(),
                );
            };
            assert!(
                step.matches(program, args),
                "call {i} should be {step:?}, got {program} {args:?}. \
                 Full sequence expected: {steps:?}",
            );
        }
        // A short `calls` already panicked in the loop above, so anything left is
        // a call the code should not have made.
        assert_eq!(
            calls.len(),
            steps.len(),
            "unexpected extra call(s) past {:?}: {:#?}",
            steps.last(),
            &calls[steps.len()..],
        );
    }
}

/// What `gh pr view` answers for each [`PrHead`]. `Unresolvable` is a *failure*
/// response — the lookup soft-fails and the dispatch continues on the base
/// branch — which is why this is not folded in with the successes.
fn pr_head_response(head: PrHead) -> Result<Output> {
    match head {
        PrHead::Branch(branch) => MockProcessRunner::ok_with_stdout(&pr_view_reply(branch, false)),
        PrHead::Fork(branch) => MockProcessRunner::ok_with_stdout(&pr_view_reply(branch, true)),
        PrHead::Unresolvable => MockProcessRunner::fail("gh: not authenticated"),
    }
}

/// The stderr a deliberately-failing `step` reports. Wording matches what the
/// real tool says, so an error message a test asserts on stays realistic.
fn failure_stderr(step: Step) -> &'static str {
    match step {
        Step::DetectDefaultBranch => "fatal: ref refs/remotes/origin/HEAD is not a symbolic ref",
        Step::PrHeadLookup => "gh: not authenticated",
        Step::Fetch | Step::OriginProbe | Step::LsRemote => FETCH_FAILURE,
        Step::AheadBehind => "fatal: ambiguous argument: unknown revision",
        Step::WorktreeAdd => "fatal: not a git repository",
        Step::NewWindow => "no server running on /tmp/tmux-1000/default",
        Step::SetDispatchDir => "can't find window",
        Step::SetSplitHook => "unknown hook",
        Step::SendKeysLiteral | Step::SendKeysEnter => "can't find pane",
        Step::CompanionSplit => "no target pane",
        Step::CompanionRoleMark => "unknown option",
    }
}

#[cfg(test)]
#[allow(clippy::unwrap_used, clippy::expect_used)]
mod tests {
    use super::*;
    use crate::dispatch::tests::{make_task, make_test_repo_with_worktree, pr_review_task};
    use crate::dispatch::worktree::BaseRef;
    use crate::dispatch::{dispatch_agent, resume_agent};
    use crate::models::TaskId;
    use crate::process::SUBPROCESS_TIMEOUT;

    /// The worktree directory name `make_task`'s id and title slugify to. Every
    /// reused-worktree shape needs it pre-created, which is also what puts
    /// provisioning on the reuse branch.
    const WORKTREE_SLUG: &str = "42-fix-bug";

    /// A repo whose task worktree already exists, so provisioning reuses it and
    /// the `.claude-prompt` write succeeds.
    fn repo_with_worktree() -> (tempfile::TempDir, String) {
        let (dir, repo_path, _) = make_test_repo_with_worktree(WORKTREE_SLUG);
        (dir, repo_path)
    }

    /// A repo with no task worktree, so provisioning takes the fresh path.
    fn bare_repo() -> (tempfile::TempDir, String) {
        let dir = tempfile::TempDir::new().unwrap();
        let path = dir.path().to_str().unwrap().to_string();
        (dir, path)
    }

    /// Drive `provision_worktree` for `base` and return what it recorded.
    fn provision(
        repo_path: &str,
        base: Option<BaseRef<'_>>,
        script: &DispatchScript,
    ) -> Vec<(String, Vec<String>)> {
        let mock = script.runner();
        let _ = crate::dispatch::worktree::provision_worktree(
            &make_task(repo_path),
            &mock,
            base,
            SUBPROCESS_TIMEOUT,
        );
        mock.recorded_calls()
    }

    // --- the scripts describe what the code really does ---

    /// The load-bearing self-test: the happy-path shape drives a real dispatch
    /// end to end and the calls it recorded are exactly the declared steps. If a
    /// preflight call is ever added to `provision_worktree`, this fails until the
    /// step list here is updated — one place, not ~45.
    #[test]
    fn dispatch_script_matches_a_real_dispatch() {
        let (_dir, repo_path) = repo_with_worktree();
        let script = DispatchScript::dispatch();
        let mock = script.runner();

        dispatch_agent(&make_task(&repo_path), &mock, None, &Default::default()).unwrap();

        script.assert_matches(&mock.recorded_calls());
    }

    #[test]
    fn resume_script_matches_a_real_resume() {
        let (dir, _repo_path) = bare_repo();
        let script = DispatchScript::resume();
        let mock = script.runner();

        resume_agent(TaskId(42), dir.path().to_str().unwrap(), &mock).unwrap();

        script.assert_matches(&mock.recorded_calls());
    }

    #[test]
    fn provision_script_matches_provision_worktree_alone() {
        let (_dir, repo_path) = repo_with_worktree();
        let script = DispatchScript::provision();
        script.assert_matches(&provision(
            &repo_path,
            Some(BaseRef::Branch("main")),
            &script,
        ));
    }

    #[test]
    fn fresh_worktree_script_matches_a_real_provision() {
        let (_dir, repo_path) = bare_repo();
        let script = DispatchScript::provision().fresh_worktree();
        script.assert_matches(&provision(
            &repo_path,
            Some(BaseRef::Branch("main")),
            &script,
        ));
    }

    #[test]
    fn no_fetch_script_matches_a_provision_with_no_base_ref() {
        let (_dir, repo_path) = repo_with_worktree();
        let script = DispatchScript::provision().no_fetch();
        script.assert_matches(&provision(&repo_path, None, &script));
    }

    /// The reuse path is best-effort: one attempt, and crucially *no*
    /// classification probes. That budget is what
    /// `provision_worktree_reuse_does_not_retry_or_probe_an_unreachable_origin`
    /// exists to defend, so the script must model it exactly.
    #[test]
    fn unreachable_on_the_reuse_path_is_one_attempt_and_no_probes() {
        let (_dir, repo_path) = repo_with_worktree();
        let script = DispatchScript::provision().fetch_is_unreachable();
        assert_eq!(script.attempts(), 1);
        script.assert_matches(&provision(
            &repo_path,
            Some(BaseRef::Branch("main")),
            &script,
        ));
    }

    /// A fresh worktree fetches under `Required`, so the probes run and the full
    /// retry budget is spent before the dispatch aborts.
    #[test]
    fn unreachable_on_a_fresh_worktree_probes_then_spends_the_whole_budget() {
        let (_dir, repo_path) = bare_repo();
        let script = DispatchScript::provision()
            .fresh_worktree()
            .fetch_is_unreachable();
        assert_eq!(script.attempts(), FETCH_MAX_ATTEMPTS);
        let calls = provision(&repo_path, Some(BaseRef::Branch("main")), &script);
        // The script declares the abort, so this is what pins "fetch ×3 plus the
        // two probes and nothing whatsoever after".
        script.assert_matches(&calls);
        assert!(
            !calls.iter().any(|(prog, _)| prog == "tmux"),
            "an aborted dispatch must issue no tmux call at all, got: {calls:?}"
        );
    }

    /// A positively-identified 404 is not retried, so it costs one attempt plus
    /// the probes — and then provisioning continues on the local branch.
    #[test]
    fn no_origin_ref_is_classified_once_and_never_retried() {
        let (_dir, repo_path) = bare_repo();
        let script = DispatchScript::provision()
            .fresh_worktree()
            .fetch_finds_no_origin_ref();
        assert_eq!(script.attempts(), 1);
        script.assert_matches(&provision(
            &repo_path,
            Some(BaseRef::Branch("main")),
            &script,
        ));
    }

    #[test]
    fn a_retried_fetch_classifies_once_then_retries_to_success() {
        let (_dir, repo_path) = bare_repo();
        let script = DispatchScript::provision()
            .fresh_worktree()
            .fetch_succeeds_on_attempt(3);
        script.assert_matches(&provision(
            &repo_path,
            Some(BaseRef::Branch("main")),
            &script,
        ));
    }

    /// `BaseRef::PrHead` must never be measured against a local ref — the
    /// guarantee `dispatch_pr_review_task_never_measures_the_pr_head_branch`
    /// asserts. Declared as the *absence* of a step, so `assert_matches` rejects
    /// a `rev-list` appearing.
    #[test]
    fn a_pr_head_base_declares_no_ahead_behind_step() {
        let (_dir, repo_path) = bare_repo();
        let script = DispatchScript::provision()
            .fresh_worktree()
            .pr_head(PrHead::Branch("feature-x"));
        let calls = provision(&repo_path, Some(BaseRef::PrHead("feature-x")), &script);
        assert!(
            !calls
                .iter()
                .any(|(_, args)| args.contains(&"rev-list".to_string())),
            "a PR head branch must never be compared against a local ref: {calls:?}"
        );
    }

    /// End-to-end: a review task carrying a PR url resolves the head branch and
    /// then skips the measurement, so the `gh` step is present and the `rev-list`
    /// one is not. Drives the real `dispatch_agent` so the `BaseRef::PrHead`
    /// construction is exercised, not just `provision_worktree`'s half of it.
    #[test]
    fn pr_head_script_matches_a_real_review_dispatch() {
        let (_dir, repo_path) = repo_with_worktree();
        let script = DispatchScript::dispatch().pr_head(PrHead::Branch("feature-x"));
        let mock = script.runner();

        dispatch_agent(
            &pr_review_task(&repo_path),
            &mock,
            None,
            &Default::default(),
        )
        .unwrap();

        script.assert_matches(&mock.recorded_calls());
    }

    /// A branch base *is* measured, and the reading decides the start point.
    #[test]
    fn a_branch_base_measures_and_prefers_local_when_ahead() {
        let (_dir, repo_path) = bare_repo();
        let script = DispatchScript::provision().fresh_worktree().local_ahead(3);
        let calls = provision(&repo_path, Some(BaseRef::Branch("main")), &script);
        script.assert_matches(&calls);
        assert_eq!(
            calls[script.index_of(Step::WorktreeAdd)].1.last().unwrap(),
            "main",
            "a local branch that is ahead wins the start point: {calls:?}"
        );
    }

    #[test]
    fn a_level_branch_base_prefers_origin() {
        let (_dir, repo_path) = bare_repo();
        let script = DispatchScript::provision().fresh_worktree();
        let calls = provision(&repo_path, Some(BaseRef::Branch("main")), &script);
        assert_eq!(
            calls[script.index_of(Step::WorktreeAdd)].1.last().unwrap(),
            "origin/main",
            "no drift means origin wins: {calls:?}"
        );
    }

    // --- step bookkeeping ---

    #[test]
    fn index_of_reports_the_recorded_call_position() {
        let script = DispatchScript::dispatch();
        assert_eq!(script.index_of(Step::Fetch), 0);
        assert_eq!(script.index_of(Step::AheadBehind), 1);
        assert_eq!(script.index_of(Step::NewWindow), 2);
        assert_eq!(script.index_of(Step::SendKeysLiteral), 5);
        assert_eq!(script.index_of(Step::CompanionSplit), 7);
    }

    /// The whole point of deriving indices: optional steps ahead of the one being
    /// asserted on shift it, and no call site has to know by how much.
    #[test]
    fn index_of_shifts_with_the_optional_leading_steps() {
        let script = DispatchScript::dispatch()
            .detecting_default_branch("main")
            .fresh_worktree();
        assert_eq!(script.index_of(Step::DetectDefaultBranch), 0);
        assert_eq!(script.index_of(Step::Fetch), 1);
        assert_eq!(script.index_of(Step::AheadBehind), 2);
        assert_eq!(script.index_of(Step::WorktreeAdd), 3);
        assert_eq!(script.index_of(Step::SendKeysLiteral), 7);
    }

    /// The retried-fetch case is exactly what a hand-written queue got wrong: the
    /// probes land between attempt 1 and attempt 2, so everything after moves by
    /// four rather than by the two the retries alone suggest.
    #[test]
    fn index_of_accounts_for_every_attempt_and_both_probes() {
        let script = DispatchScript::provision()
            .fresh_worktree()
            .fetch_succeeds_on_attempt(3);
        assert_eq!(script.index_of(Step::Fetch), 0, "the first attempt");
        assert_eq!(script.index_of(Step::OriginProbe), 1);
        assert_eq!(script.index_of(Step::LsRemote), 2);
        // attempts 2 and 3 occupy 3 and 4
        assert_eq!(script.index_of(Step::AheadBehind), 5);
        assert_eq!(script.index_of(Step::WorktreeAdd), 6);
    }

    #[test]
    #[should_panic(expected = "WorktreeAdd is not part of this script's sequence")]
    fn index_of_panics_for_a_step_this_shape_never_issues() {
        DispatchScript::dispatch().index_of(Step::WorktreeAdd);
    }

    #[test]
    fn stops_after_queues_nothing_beyond_the_named_step() {
        let script = DispatchScript::dispatch().stops_after(Step::SetSplitHook);
        assert_eq!(
            script.responses().len(),
            script.index_of(Step::SetSplitHook) + 1
        );
    }

    #[test]
    fn fails_at_queues_a_failure_as_the_last_response() {
        let script = DispatchScript::dispatch().fails_at(Step::CompanionSplit);
        let responses = script.responses();
        assert_eq!(responses.len(), script.index_of(Step::CompanionSplit) + 1);
        let (_delay, last) = responses.last().unwrap();
        assert!(
            !last.as_ref().unwrap().status.success(),
            "the named step must fail"
        );
    }

    #[test]
    #[should_panic(expected = "a failing fetch is retried and classified")]
    fn fails_at_rejects_the_retried_fetch() {
        let _ = DispatchScript::dispatch().fails_at(Step::Fetch);
    }

    /// The timeout shape is a *success* held back past the deadline, not a
    /// failure — the distinction the watchdog test depends on.
    #[test]
    fn fetch_times_out_queues_delayed_successes() {
        let delay = Duration::from_millis(100);
        let script = DispatchScript::provision()
            .fresh_worktree()
            .fetch_times_out(delay);
        let (got_delay, response) = script.responses().into_iter().next().unwrap();
        assert_eq!(got_delay, Some(delay));
        assert!(
            response.unwrap().status.success(),
            "a timeout shape must queue successes; only the delay defeats them"
        );
    }

    // --- assert_matches really discriminates ---

    #[test]
    #[should_panic(expected = "unexpected extra call")]
    fn assert_matches_rejects_an_extra_call() {
        let mut calls = recorded_resume_calls();
        calls.push(("git".to_string(), vec!["rev-list".to_string()]));
        DispatchScript::resume().assert_matches(&calls);
    }

    #[test]
    #[should_panic(expected = "missing CompanionRoleMark")]
    fn assert_matches_rejects_a_missing_call() {
        let mut calls = recorded_resume_calls();
        calls.pop();
        DispatchScript::resume().assert_matches(&calls);
    }

    #[test]
    #[should_panic(expected = "should be NewWindow")]
    fn assert_matches_rejects_a_reordered_call() {
        let mut calls = recorded_resume_calls();
        calls.swap(0, 1);
        DispatchScript::resume().assert_matches(&calls);
    }

    /// A real resume's calls, as raw material for the rejection tests above.
    fn recorded_resume_calls() -> Vec<(String, Vec<String>)> {
        let (dir, _repo_path) = bare_repo();
        let mock = DispatchScript::resume().runner();
        resume_agent(TaskId(42), dir.path().to_str().unwrap(), &mock).unwrap();
        mock.recorded_calls()
    }

    // The response formatters need no tests of their own: a malformed
    // `default_branch_ref` makes `exec_quick_dispatch_sets_base_branch_to_repo_default`
    // (src/runtime/tests.rs) resolve the wrong branch, a malformed `pr_view_reply`
    // makes `dispatch_pr_review_task_bases_worktree_on_pr_head_branch` fall back to
    // the base branch, and a malformed `rev_list_counts` is caught by the two
    // start-point tests above. All three already fail on the real parse.
}
