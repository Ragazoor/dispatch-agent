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
//! # Finish is the same problem, one operation over
//!
//! [`DispatchScript::finish`] declares `finish_task`'s sequence (three preflight
//! reads → pull? → rebase → mid-rebase status? → abort? → fast-forward?) the
//! same way, for the same reason: four of its eight calls are conditional and
//! each preflight read gates everything after it, and the dirty-worktree
//! preflight landing is exactly the splice-into-every-vector episode described
//! above.
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

use anyhow::{anyhow, Result};

use super::worktree::FETCH_MAX_ATTEMPTS;
use crate::process::MockProcessRunner;
use crate::tmux;

/// One call `MockProcessRunner` recorded: the program, then its argv.
pub(crate) type RecordedCall = (String, Vec<String>);

/// What driving a scripted `finish_task` produced — the calls it issued and the
/// outcome it returned. Named because the three test modules that drive a finish
/// shape all hand both back from their own helper.
pub(crate) type FinishRun = (
    Vec<RecordedCall>,
    std::result::Result<(), super::FinishError>,
);

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

/// The stderr a `tmux` call stands in with when no server is running — shared
/// by [`Step::HasWindowQuery`] and [`Step::NewWindow`], since both fail the
/// same realistic way when there is nothing to connect to.
const NO_TMUX_SERVER: &str = "no server running on /tmp/tmux-1000/default";

/// What the runner itself reports when a git call cannot be spawned at all —
/// the `Err` arm, as opposed to a git that ran and exited non-zero.
const GIT_NOT_ON_PATH: &str = "git: command not found";

/// `git remote get-url origin`'s reply for a repo that has one.
const ORIGIN_URL: &[u8] = b"git@github.com:org/repo.git\n";

/// The repo root, worktree and branch a finish shape runs against by default.
/// Only their argv matters — `finish_task` runs subprocesses and touches no
/// filesystem of its own, so nothing here has to exist on disk.
const FINISH_REPO: &str = "/repo";
const FINISH_WORKTREE: &str = "/repo/.worktrees/42-fix-bug";
const FINISH_BRANCH: &str = "42-fix-bug";

/// The bound every scripted finish runs under. Short enough that a timing-out
/// shape resolves instantly: `MockProcessRunner::run_with_timeout` bails
/// *without* sleeping once a scripted delay reaches the timeout.
pub(crate) const FINISH_TIMEOUT: Duration = Duration::from_millis(50);

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
    /// `tmux list-windows -a -F #{window_name}` — `resume_agent`'s live
    /// `has_window` check for a stray already-running window under the
    /// deterministic name, issued before anything else. Resume-only: a fresh
    /// [`DispatchScript::dispatch`] never issues this.
    HasWindowQuery,
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

    // --- finish (`DispatchScript::finish`) ---
    //
    // A finish is a different operation with a disjoint sequence; only
    // [`Step::OriginProbe`] is shared, because it is literally the same
    // `git remote get-url origin` call reached from the other side.
    /// `git rev-parse --abbrev-ref HEAD` in the repo root — `git::current_branch`,
    /// the first thing a finish reads.
    CurrentBranch,
    /// `git status --porcelain` in the repo root — `git::dirty_files`, the
    /// preflight that refuses to rebase into an uncommitted mess.
    DirtyCheck,
    /// `git pull --no-rebase origin <base>` in the repo root — only when the
    /// origin probe found a remote.
    Pull,
    /// `git rebase <base>` in the worktree.
    Rebase,
    /// `git status --porcelain` in the *worktree*, read mid-rebase to name the
    /// conflicted files before the abort below clears that state. Only for a
    /// rebase failure that looks like a conflict.
    ConflictStatus,
    /// `git rebase --abort` in the worktree — after any rebase that ran and
    /// failed.
    RebaseAbort,
    /// `git merge --ff-only <branch>` in the repo root, the last step of a
    /// successful finish.
    FastForward,
}

impl Step {
    /// Whether a recorded call is this step, by program plus the argv token that
    /// distinguishes it from its siblings.
    fn matches(self, program: &str, args: &[String]) -> bool {
        let has = |needle: &str| args.iter().any(|a| a == needle);
        let command_is = |name: &str| args.first().is_some_and(|a| a == name);
        match self {
            Step::HasWindowQuery => program == "tmux" && command_is("list-windows"),
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

            Step::CurrentBranch => program == "git" && has("rev-parse"),
            // The two porcelain reads differ only in their `-C` path — the repo
            // root for the preflight, the worktree for the mid-rebase read —
            // which this matcher, being argv-token based, cannot see. Sharing a
            // predicate costs nothing: the rebase sits between them, so
            // `assert_matches` still rejects a reordering, and the two tests
            // that care about the scope assert it through the error's own
            // `path` field.
            Step::DirtyCheck | Step::ConflictStatus => {
                program == "git" && has("status") && has("--porcelain")
            }
            Step::Pull => program == "git" && has("pull"),
            // The abort is a `rebase` too, and `--abort` is the whole difference.
            Step::Rebase => program == "git" && has("rebase") && !has("--abort"),
            Step::RebaseAbort => program == "git" && has("rebase") && has("--abort"),
            Step::FastForward => program == "git" && has("merge") && has("--ff-only"),
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

/// How one of the finish path's fallible git calls answers. Only [`Self::Ok`]
/// lets the sequence continue; the other three end it where they sit.
#[derive(Clone, Copy)]
enum CallOutcome {
    /// Exits zero.
    Ok,
    /// Exits non-zero, with the step's own realistic stderr (see
    /// [`failure_stderr`]).
    Fails,
    /// The process could not be spawned at all, i.e. the runner itself returns
    /// `Err`. Distinct from [`Self::Fails`] because `finish_task` reaches it
    /// through a different arm — `map_err` rather than a `status.success()`
    /// check — and names it differently.
    CannotRun,
    /// Answers *successfully*, but only after `delay`, so a caller bounding the
    /// call kills it first. Deliberately not a plain failure: a success held
    /// back past the deadline still fails if the call ever regresses to the
    /// unbounded `run`, which is exactly what the bounding tests defend.
    TimesOut(Duration),
}

impl CallOutcome {
    fn is_ok(self) -> bool {
        matches!(self, CallOutcome::Ok)
    }

    /// This outcome's response, with `stderr` used only by [`Self::Fails`].
    fn response(self, stderr: &str) -> (Option<Duration>, Result<Output>) {
        match self {
            CallOutcome::Ok => (None, MockProcessRunner::ok()),
            CallOutcome::Fails => (None, MockProcessRunner::fail(stderr)),
            CallOutcome::CannotRun => (None, Err(anyhow!("{GIT_NOT_ON_PATH}"))),
            CallOutcome::TimesOut(delay) => (Some(delay), MockProcessRunner::ok()),
        }
    }
}

/// How `git rebase <base>` goes. A rebase has every [`CallOutcome`] the other
/// calls do, plus one that is its alone: a *conflict*, which is what decides
/// whether the mid-rebase porcelain read happens at all. Only that arm is
/// modelled here; the rest delegate, so a new outcome shape is added once.
#[derive(Clone, Copy)]
enum RebaseOutcome {
    /// Goes the way any other call can go. [`CallOutcome::Fails`] is the
    /// non-conflict failure — an invalid upstream, a dirty worktree — which is
    /// aborted and reported as `FinishError::Other` with no mid-rebase read,
    /// because there is no conflict to name.
    Plain(CallOutcome),
    /// Exits non-zero carrying git's `CONFLICT (content): …` marker. Which
    /// stream carries it is a real axis, not a detail: `is_rebase_conflict`
    /// reads both, and a regression to reading only one would pass the other's
    /// tests. `files` appear both in the marker and in the mid-rebase porcelain
    /// read that follows.
    Conflicts {
        files: &'static [&'static str],
        on_stderr: bool,
    },
}

/// What `git rev-parse --abbrev-ref HEAD` reports for the repo root. One enum
/// rather than a branch plus a can-be-read flag, so "HEAD is on `x`" and "HEAD
/// could not be read" cannot both be declared at once.
#[derive(Clone, Copy)]
enum HeadOutcome {
    /// HEAD is on the base branch, so the finish proceeds.
    OnBase,
    /// HEAD is on some other branch, so the finish refuses before touching
    /// anything.
    On(&'static str),
    /// The read's subprocess could not be spawned.
    CannotRun,
}

/// Which checkout a finish's call runs in, i.e. what its `-C` names.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
enum Scope {
    /// The repo root, where the base branch is checked out.
    Repo,
    /// The task's worktree, where the task branch is.
    Worktree,
}

/// What `git remote get-url origin` reports, i.e. whether the pull happens.
#[derive(Clone, Copy)]
enum RemoteOutcome {
    /// A remote is configured, so the base branch is pulled first.
    Present,
    /// No remote: the probe exits non-zero and the pull is skipped.
    Absent,
    /// The probe could not be *run*, which is not the same as finding no
    /// remote — `finish_task` stops rather than rebasing onto a base it never
    /// refreshed.
    CannotRun,
}

/// The shape of one `finish_task` call sequence: the preflight reads, the
/// optional pull, the rebase and whatever that rebase's outcome pulls in.
///
/// Its own struct behind one `Option` on [`DispatchScript`] rather than more
/// flat fields, because a finish's axes and a dispatch's are disjoint — folding
/// them together would put `fails_at(Step::NewWindow)` in reach of a shape that
/// launches no tmux window at all.
#[derive(Clone, Copy)]
struct Finish {
    /// The repo root and the task worktree the finish is driven against. Held
    /// here so [`DispatchScript::drive_finish`] can build the `FinishContext`
    /// from the same declaration the scope assertion reads.
    repo_path: &'static str,
    worktree: &'static str,
    /// The task branch, i.e. the fast-forward target.
    branch: &'static str,
    /// The branch the repo root must be on, and the rebase/pull target.
    base_branch: &'static str,
    head: HeadOutcome,
    /// Paths the repo root's working tree reports as dirty. Empty is clean.
    dirty: &'static [&'static str],
    remote: RemoteOutcome,
    pull: CallOutcome,
    rebase: RebaseOutcome,
    fast_forward: CallOutcome,
}

impl Finish {
    /// The branch HEAD reports, which is the base branch unless a test moved it.
    fn head_branch(&self) -> &'static str {
        match self.head {
            HeadOutcome::On(branch) => branch,
            HeadOutcome::OnBase | HeadOutcome::CannotRun => self.base_branch,
        }
    }

    /// Which checkout `step` runs in — the repo root or the task worktree.
    ///
    /// `finish_task` splits its calls across both, and getting that wrong is not
    /// cosmetic: a mid-rebase porcelain read against the repo root names the
    /// wrong files in `RebaseConflict`, and a preflight dirty check against the
    /// worktree clears the wrong tree. Argv alone cannot tell the two porcelain
    /// reads apart, so [`DispatchScript::assert_matches`] checks this separately.
    fn scope_of(step: Step) -> Scope {
        match step {
            Step::CurrentBranch | Step::DirtyCheck | Step::OriginProbe | Step::Pull => Scope::Repo,
            Step::Rebase | Step::ConflictStatus | Step::RebaseAbort => Scope::Worktree,
            // The fast-forward is the repo root's own merge, not the worktree's.
            Step::FastForward => Scope::Repo,
            other => unreachable!("{other:?} is not part of a finish sequence"),
        }
    }

    /// Assert call `i`'s `-C` path is the checkout [`Self::scope_of`] expects.
    fn assert_scope(&self, i: usize, step: Step, args: &[String]) {
        let (scope, expected) = match Self::scope_of(step) {
            Scope::Repo => (Scope::Repo, self.repo_path),
            Scope::Worktree => (Scope::Worktree, self.worktree),
        };
        // `-C <path>` is how every one of these calls names its checkout, so the
        // token after `-C` is the whole claim.
        let got = args
            .iter()
            .position(|a| a == "-C")
            .and_then(|at| args.get(at + 1));
        assert_eq!(
            got.map(String::as_str),
            Some(expected),
            "call {i} ({step:?}) must run in the {scope:?} ({expected}), got argv {args:?}",
        );
    }

    /// The steps a finish of this shape issues, one entry per recorded call.
    /// Mirrors `finish_task`'s own control flow, which is what the self-tests in
    /// this module hold it to.
    fn steps(&self) -> Vec<Step> {
        let mut steps = vec![Step::CurrentBranch];
        // A HEAD that could not be read, or that is not on the base branch, is
        // refused before anything is touched.
        if !matches!(self.head, HeadOutcome::OnBase) {
            return steps;
        }
        steps.push(Step::DirtyCheck);
        if !self.dirty.is_empty() {
            return steps;
        }
        steps.push(Step::OriginProbe);
        match self.remote {
            RemoteOutcome::CannotRun => return steps,
            RemoteOutcome::Present => {
                steps.push(Step::Pull);
                if !self.pull.is_ok() {
                    return steps;
                }
            }
            RemoteOutcome::Absent => {}
        }
        steps.push(Step::Rebase);
        match self.rebase {
            RebaseOutcome::Plain(CallOutcome::Ok) => steps.push(Step::FastForward),
            // The conflicted files are read out of the worktree's own status
            // while the rebase is still mid-flight, because the abort clears it.
            RebaseOutcome::Conflicts { .. } => {
                steps.push(Step::ConflictStatus);
                steps.push(Step::RebaseAbort);
            }
            // A rebase that ran and exited non-zero is aborted; one that never
            // ran — could not be spawned, or was killed by the bound — leaves
            // nothing to abort.
            RebaseOutcome::Plain(CallOutcome::Fails) => steps.push(Step::RebaseAbort),
            RebaseOutcome::Plain(CallOutcome::CannotRun | CallOutcome::TimesOut(_)) => {}
        }
        steps
    }

    /// How `step` answers under this shape.
    fn response(&self, step: Step) -> (Option<Duration>, Result<Output>) {
        match step {
            Step::CurrentBranch => match self.head {
                HeadOutcome::CannotRun => (None, Err(anyhow!("{GIT_NOT_ON_PATH}"))),
                HeadOutcome::OnBase | HeadOutcome::On(_) => {
                    let head = format!("{}\n", self.head_branch());
                    (None, MockProcessRunner::ok_with_stdout(head.as_bytes()))
                }
            },
            Step::DirtyCheck => (
                None,
                MockProcessRunner::ok_with_stdout(&porcelain_lines(" M", self.dirty)),
            ),
            Step::OriginProbe => match self.remote {
                RemoteOutcome::Present => (None, MockProcessRunner::ok_with_stdout(ORIGIN_URL)),
                // "No remote" is a non-zero exit, not an error: `has_origin_remote`
                // reads the status, and only an `Err` means it could not look.
                RemoteOutcome::Absent => (None, MockProcessRunner::fail("")),
                RemoteOutcome::CannotRun => (None, Err(anyhow!("{GIT_NOT_ON_PATH}"))),
            },
            Step::Pull => self.pull.response(failure_stderr(Step::Pull)),
            Step::Rebase => rebase_response(self.rebase),
            Step::ConflictStatus => match self.rebase {
                RebaseOutcome::Conflicts { files, .. } => (
                    None,
                    MockProcessRunner::ok_with_stdout(&porcelain_lines("UU", files)),
                ),
                _ => unreachable!("ConflictStatus is only emitted for a conflicting rebase"),
            },
            // Best-effort cleanup whose result `finish_task` discards.
            Step::RebaseAbort => (None, MockProcessRunner::ok()),
            Step::FastForward => self
                .fast_forward
                .response(failure_stderr(Step::FastForward)),
            other => unreachable!("{other:?} is not part of a finish sequence"),
        }
    }
}

/// `git rebase`'s reply for each [`RebaseOutcome`]. Only the conflict arm is
/// built here — the rest are ordinary [`CallOutcome`]s — and it is built by hand
/// because it is a *failure carrying stdout*, which no `MockProcessRunner`
/// helper produces.
fn rebase_response(outcome: RebaseOutcome) -> (Option<Duration>, Result<Output>) {
    match outcome {
        RebaseOutcome::Plain(plain) => plain.response(failure_stderr(Step::Rebase)),
        RebaseOutcome::Conflicts { files, on_stderr } => {
            let marker = conflict_markers(files);
            let response = if on_stderr {
                MockProcessRunner::fail(&marker)
            } else {
                Ok(Output {
                    status: crate::process::exit_fail(),
                    stdout: marker.into_bytes(),
                    stderr: Vec::new(),
                })
            };
            (None, response)
        }
    }
}

/// `git status --porcelain`'s reply listing `paths` under status `code`.
///
/// The code is what each reader selects on: `parse_porcelain_files` keeps every
/// entry regardless (so ` M` stands in for any dirty path), while
/// `parse_unmerged_files` keeps only the conflict codes (so `UU`). That both
/// parse the column correctly — including the leading-space and `??` forms — is
/// covered directly in `src/git.rs`, so this need only pick one form of each.
fn porcelain_lines(code: &str, paths: &[&str]) -> Vec<u8> {
    paths
        .iter()
        .map(|path| format!("{code} {path}\n"))
        .collect::<String>()
        .into_bytes()
}

/// `git rebase`'s conflict report for `files`, one marker line each — the form
/// `is_rebase_conflict` matches on.
fn conflict_markers(files: &[&str]) -> String {
    files
        .iter()
        .map(|file| format!("CONFLICT (content): Merge conflict in {file}\n"))
        .collect()
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
    /// Whether this shape is a resume (as opposed to a fresh dispatch) — the
    /// only shape that issues [`Step::HasWindowQuery`], since the live
    /// `has_window` check lives in `resume_agent`, not `dispatch_agent`.
    is_resume: bool,
    /// Only meaningful when `is_resume`: the window `has_window` looks for is
    /// already alive, so `resume_agent` short-circuits after the check —
    /// see [`Self::window_already_alive`].
    window_already_alive: bool,
    ending: Ending,
    /// Present only for a [`Self::finish`] shape. `Some` makes every field
    /// above irrelevant: a finish issues none of a dispatch's calls.
    finish: Option<Finish>,
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
            is_resume: false,
            window_already_alive: false,
            ending: Ending::Complete,
            finish: None,
        }
    }

    /// `finish_task`'s sequence: the three preflight reads, a pull, the rebase
    /// and the fast-forward. The default is the fullest successful path — a
    /// remote is configured, so the pull happens, and every call succeeds.
    ///
    /// Shares nothing with a dispatch but [`Step::OriginProbe`], so the
    /// dispatch modifiers do not apply to it and panic if used — as the finish
    /// modifiers do on a dispatch shape.
    pub(crate) fn finish() -> Self {
        let mut script = Self::dispatch();
        script.finish = Some(Finish {
            repo_path: FINISH_REPO,
            worktree: FINISH_WORKTREE,
            branch: FINISH_BRANCH,
            base_branch: "main",
            head: HeadOutcome::OnBase,
            dirty: &[],
            remote: RemoteOutcome::Present,
            pull: CallOutcome::Ok,
            rebase: RebaseOutcome::Plain(CallOutcome::Ok),
            fast_forward: CallOutcome::Ok,
        });
        script
    }

    /// The finish configuration, for the modifiers below.
    ///
    /// # Panics
    ///
    /// If this is not a [`Self::finish`] shape — a finish modifier on a
    /// dispatch shape would otherwise be a silent no-op.
    fn finish_mut(&mut self) -> &mut Finish {
        match self.finish.as_mut() {
            Some(finish) => finish,
            None => panic!("this modifier only applies to a finish() shape"),
        }
    }

    /// Guards a *dispatch* modifier, the mirror of [`Self::finish_mut`].
    ///
    /// Without it, `finish().fetch_times_out(d)` would set a field `steps()`
    /// early-returns past: it compiles, passes, and asserts nothing — the exact
    /// silent no-op the finish-side guard exists to prevent.
    ///
    /// # Panics
    ///
    /// If this is a [`Self::finish`] shape.
    fn assert_is_dispatch(&self) {
        assert!(
            self.finish.is_none(),
            "this modifier only applies to a dispatch/resume/provision shape"
        );
    }

    /// The branch the repo root is on and the rebase targets. Defaults to
    /// `main`.
    pub(crate) fn base_branch(mut self, branch: &'static str) -> Self {
        self.finish_mut().base_branch = branch;
        self
    }

    /// The repo root is on `branch` rather than the base branch, so the finish
    /// refuses before touching anything — its sequence is the HEAD read alone.
    pub(crate) fn head_branch(mut self, branch: &'static str) -> Self {
        self.finish_mut().head = HeadOutcome::On(branch);
        self
    }

    /// `git::current_branch`'s subprocess cannot be spawned, so the finish stops
    /// at its very first call.
    pub(crate) fn current_branch_cannot_run(mut self) -> Self {
        self.finish_mut().head = HeadOutcome::CannotRun;
        self
    }

    /// The repo root's working tree reports `paths` as dirty, so the finish
    /// stops after the preflight read — before any pull, rebase or merge.
    pub(crate) fn dirty_primary(mut self, paths: &'static [&'static str]) -> Self {
        self.finish_mut().dirty = paths;
        self
    }

    /// No origin remote is configured, so the pull is skipped entirely.
    pub(crate) fn no_remote(mut self) -> Self {
        self.finish_mut().remote = RemoteOutcome::Absent;
        self
    }

    /// The origin probe could not be *run*. Not the same as [`Self::no_remote`]:
    /// a probe that identified nothing is a failure worth naming, not a licence
    /// to rebase onto a base that was never refreshed.
    pub(crate) fn remote_probe_cannot_run(mut self) -> Self {
        self.finish_mut().remote = RemoteOutcome::CannotRun;
        self
    }

    /// The pull exits non-zero.
    pub(crate) fn pull_fails(mut self) -> Self {
        self.finish_mut().pull = CallOutcome::Fails;
        self
    }

    /// The pull's subprocess cannot be spawned.
    pub(crate) fn pull_cannot_run(mut self) -> Self {
        self.finish_mut().pull = CallOutcome::CannotRun;
        self
    }

    /// The pull succeeds but only after `delay`, so a bounded caller kills it.
    /// See [`CallOutcome::TimesOut`] for why this is not just a failure.
    pub(crate) fn pull_times_out(mut self, delay: Duration) -> Self {
        self.finish_mut().pull = CallOutcome::TimesOut(delay);
        self
    }

    /// The rebase conflicts, with git's marker on **stdout** — one of the two
    /// streams `is_rebase_conflict` reads. `files` come back both in the marker
    /// and in the mid-rebase porcelain read.
    pub(crate) fn rebase_conflicts_in_stdout(mut self, files: &'static [&'static str]) -> Self {
        self.finish_mut().rebase = RebaseOutcome::Conflicts {
            files,
            on_stderr: false,
        };
        self
    }

    /// The rebase conflicts with the marker on **stderr** — the other stream.
    pub(crate) fn rebase_conflicts_in_stderr(mut self, files: &'static [&'static str]) -> Self {
        self.finish_mut().rebase = RebaseOutcome::Conflicts {
            files,
            on_stderr: true,
        };
        self
    }

    /// The rebase exits non-zero with no conflict marker, so it is aborted and
    /// reported as `Other` — with no mid-rebase porcelain read.
    pub(crate) fn rebase_fails(mut self) -> Self {
        self.finish_mut().rebase = RebaseOutcome::Plain(CallOutcome::Fails);
        self
    }

    /// The rebase's subprocess cannot be spawned, so there is nothing to abort.
    pub(crate) fn rebase_cannot_run(mut self) -> Self {
        self.finish_mut().rebase = RebaseOutcome::Plain(CallOutcome::CannotRun);
        self
    }

    /// The rebase answers past the caller's bound and is killed.
    pub(crate) fn rebase_times_out(mut self, delay: Duration) -> Self {
        self.finish_mut().rebase = RebaseOutcome::Plain(CallOutcome::TimesOut(delay));
        self
    }

    /// Drive a real `finish_task` under this shape, against the paths and base
    /// branch the shape itself declares, and return the calls it recorded with
    /// its outcome — after asserting the calls are exactly the declared steps.
    ///
    /// Every test module that drives a finish goes through here, so the context
    /// can never contradict the script: before this existed, three separate
    /// helpers each built their own `FinishContext` and two of them hardcoded a
    /// base branch the script was free to change underneath them.
    ///
    /// # Panics
    ///
    /// If this is not a [`Self::finish`] shape, or if the recorded calls are not
    /// the declared sequence.
    pub(crate) fn drive_finish(&self) -> FinishRun {
        let mock = self.runner();
        let result = super::finish_task(&self.finish_context(), &mock);
        let calls = mock.recorded_calls();
        self.assert_matches(&calls);
        (calls, result)
    }

    /// The `FinishContext` this shape declares — the paths, the branch, the base
    /// branch and the bound, all from one place.
    ///
    /// [`Self::drive_finish`] is the usual way in; reach for this directly only
    /// when a test needs the `MockProcessRunner` itself afterwards (to read
    /// `recorded_timeouts`, say), which `drive_finish` does not hand back.
    ///
    /// # Panics
    ///
    /// If this is not a [`Self::finish`] shape.
    pub(crate) fn finish_context(&self) -> super::FinishContext<'static> {
        let finish = match self.finish.as_ref() {
            Some(finish) => finish,
            None => panic!("finish_context only applies to a finish() shape"),
        };
        super::FinishContext {
            repo_path: finish.repo_path,
            worktree: finish.worktree,
            branch: finish.branch,
            base_branch: finish.base_branch,
            timeout: FINISH_TIMEOUT,
        }
    }

    /// The fast-forward exits non-zero.
    pub(crate) fn fast_forward_fails(mut self) -> Self {
        self.finish_mut().fast_forward = CallOutcome::Fails;
        self
    }

    /// The fast-forward's subprocess cannot be spawned.
    pub(crate) fn fast_forward_cannot_run(mut self) -> Self {
        self.finish_mut().fast_forward = CallOutcome::CannotRun;
        self
    }

    /// The fast-forward answers past the caller's bound and is killed.
    pub(crate) fn fast_forward_times_out(mut self, delay: Duration) -> Self {
        self.finish_mut().fast_forward = CallOutcome::TimesOut(delay);
        self
    }

    /// `resume_agent`'s sequence: the tmux tail only, since resume reuses the
    /// worktree that already exists and touches git not at all.
    pub(crate) fn resume() -> Self {
        let mut script = Self::dispatch().no_fetch();
        script.is_resume = true;
        script
    }

    /// The window `resume_agent`'s `has_window` check looks for (the
    /// deterministic `task-<id>`, matching every resume test's `TaskId(42)` —
    /// see `resume_script_matches_a_real_resume`) is already alive. Resume
    /// then reports success without creating a window, sending keys, or
    /// spawning a companion pane — so this shape's sequence ends right after
    /// [`Step::HasWindowQuery`].
    ///
    /// # Panics
    ///
    /// If called on a non-resume shape — the check this models only exists in
    /// `resume_agent`.
    pub(crate) fn window_already_alive(mut self) -> Self {
        assert!(
            self.is_resume,
            "window_already_alive only applies to a resume() shape"
        );
        self.window_already_alive = true;
        self
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
        self.assert_is_dispatch();
        self.fresh_worktree = true;
        self
    }

    /// No base ref was resolved, so no fetch is attempted.
    pub(crate) fn no_fetch(mut self) -> Self {
        self.assert_is_dispatch();
        self.fetch = FetchOutcome::Absent;
        self
    }

    /// The fetch fails until attempt `n`, which succeeds. Needs a fresh worktree
    /// for `n > 1`: the reuse path only ever makes one attempt.
    pub(crate) fn fetch_succeeds_on_attempt(mut self, n: u32) -> Self {
        self.assert_is_dispatch();
        assert!(n >= 1, "fetch attempts are 1-based");
        self.fetch = FetchOutcome::SucceedsOnAttempt(n);
        self
    }

    /// The fetch fails and the probes identify a missing origin ref, so the local
    /// branch is used with a `Note:` and no retry happens.
    pub(crate) fn fetch_finds_no_origin_ref(mut self) -> Self {
        self.assert_is_dispatch();
        self.fetch = FetchOutcome::NoOriginRef;
        self
    }

    /// The fetch fails with the remote unreachable. On a fresh worktree that
    /// aborts the dispatch after the probes and the full budget; on the reuse
    /// path it is one attempt, no probes, and a warning.
    pub(crate) fn fetch_is_unreachable(mut self) -> Self {
        self.assert_is_dispatch();
        self.fetch = FetchOutcome::Unreachable;
        self
    }

    /// Every fetch attempt succeeds but takes `delay` to answer, so a caller
    /// passing a shorter timeout kills it instead. See
    /// [`FetchOutcome::TimesOut`] for why this is not the same as
    /// [`Self::fetch_is_unreachable`].
    pub(crate) fn fetch_times_out(mut self, delay: Duration) -> Self {
        self.assert_is_dispatch();
        self.fetch = FetchOutcome::TimesOut(delay);
        self
    }

    /// Local `<base>` holds `n` commits origin lacks, so `select_start_point`
    /// prefers the local ref. The default is `0` — no drift, origin wins.
    pub(crate) fn local_ahead(mut self, n: u32) -> Self {
        self.assert_is_dispatch();
        self.local_ahead = n;
        self
    }

    /// The caller passed no base branch, so the dispatch detects the repo's
    /// default branch first and then fetches `branch`.
    pub(crate) fn detecting_default_branch(mut self, branch: &'static str) -> Self {
        self.assert_is_dispatch();
        self.default_branch = Some(branch);
        self
    }

    /// This is a review task carrying a PR url, so `gh pr view` runs first.
    pub(crate) fn pr_head(mut self, head: PrHead) -> Self {
        self.assert_is_dispatch();
        self.pr_head = Some(head);
        self
    }

    /// `step` succeeds and nothing after it is queued. See [`Ending::StopsAfter`].
    ///
    /// Private because [`Self::provision`] is the only shape that needs it; widen
    /// it if a test ever needs a different stopping point.
    fn stops_after(mut self, step: Step) -> Self {
        self.assert_is_dispatch();
        self.ending = Ending::StopsAfter(step);
        self
    }

    /// `step` fails and nothing after it is queued. See [`Ending::FailsAt`].
    pub(crate) fn fails_at(mut self, step: Step) -> Self {
        // Not the generic guard: a finish shape *can* state a failure, just not
        // this way, so the message points at the modifier that does it.
        assert!(
            self.finish.is_none(),
            "a finish() shape states its failures directly — use one of the \
             *_fails / *_cannot_run modifiers instead"
        );
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
        if let Some(finish) = self.finish.as_ref() {
            return finish.steps();
        }
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
        if self.is_resume {
            steps.push(Step::HasWindowQuery);
            if self.window_already_alive {
                // Found alive: resume_agent returns immediately, without
                // creating a window, sending keys, or spawning a companion
                // pane.
                return steps;
            }
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
    /// `pub(crate)` so a test can append its own trailing responses after an
    /// `Ending::FailsAt` shape — e.g. the rollback calls a provisioning
    /// failure now triggers, which this script has no vocabulary to model
    /// itself. See `dispatch_agent_propagates_tmux_new_window_failure` for
    /// the pattern.
    pub(crate) fn responses(&self) -> Vec<(Option<Duration>, Result<Output>)> {
        let steps = self.steps();
        if let Some(finish) = self.finish.as_ref() {
            return steps
                .into_iter()
                .map(|step| finish.response(step))
                .collect();
        }
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
                Step::HasWindowQuery => (
                    None,
                    if self.window_already_alive {
                        // Matches TaskId(42) -> "task-42", the id every resume
                        // test drives (see resume_script_matches_a_real_resume).
                        MockProcessRunner::ok_with_stdout(b"task-42\n")
                    } else {
                        MockProcessRunner::ok()
                    },
                ),
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
    pub(crate) fn assert_matches(&self, calls: &[RecordedCall]) {
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
            // A finish splits its calls across two checkouts, and argv alone
            // cannot tell its two `git status --porcelain` reads apart. Checking
            // the `-C` path here is what stops a swapped scope — a mid-rebase
            // read against the repo root, which would name the wrong files in
            // `RebaseConflict` — from passing as the right sequence.
            if let Some(finish) = self.finish.as_ref() {
                finish.assert_scope(i, *step, args);
            }
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
        Step::HasWindowQuery => NO_TMUX_SERVER,
        Step::DetectDefaultBranch => "fatal: ref refs/remotes/origin/HEAD is not a symbolic ref",
        Step::PrHeadLookup => "gh: not authenticated",
        Step::Fetch | Step::OriginProbe | Step::LsRemote => FETCH_FAILURE,
        Step::AheadBehind => "fatal: ambiguous argument: unknown revision",
        Step::WorktreeAdd => "fatal: not a git repository",
        Step::NewWindow => NO_TMUX_SERVER,
        Step::SetDispatchDir => "can't find window",
        Step::SetSplitHook => "unknown hook",
        Step::SendKeysLiteral | Step::SendKeysEnter => "can't find pane",
        Step::CompanionSplit => "no target pane",
        Step::CompanionRoleMark => "unknown option",

        // The finish steps with no non-zero-exit mode of their own: the HEAD
        // read and the porcelain reads only ever succeed or fail to spawn, and
        // the abort's result `finish_task` discards outright. Left unreachable
        // rather than given invented prose that would read as configuration.
        Step::CurrentBranch | Step::DirtyCheck | Step::ConflictStatus | Step::RebaseAbort => {
            unreachable!("{step:?} has no failing-exit shape")
        }
        Step::Pull => "fatal: unable to access remote",
        // Deliberately free of every marker `is_rebase_conflict` reads —
        // "CONFLICT", "could not apply", "Merge conflict" — since this is the
        // stderr that must classify as a *non*-conflict failure.
        Step::Rebase => "fatal: refusing to rebase onto unrelated histories",
        Step::FastForward => "fatal: Not possible to fast-forward, aborting.",
    }
}

#[cfg(test)]
#[allow(clippy::unwrap_used, clippy::expect_used)]
mod tests {
    use super::*;
    use crate::dispatch::tests::{make_task, make_test_repo_with_worktree, pr_review_task};
    use crate::dispatch::worktree::BaseRef;
    use crate::dispatch::{dispatch_agent, resume_agent, FinishError};
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

    /// The reattach path: a live tmux window already answers to the
    /// deterministic name, so resume_agent must not create a duplicate — it
    /// should stop right after the has_window check with no new-window,
    /// send-keys, or companion-pane calls at all.
    #[test]
    fn resume_script_matches_a_real_resume_when_window_already_alive() {
        let (dir, _repo_path) = bare_repo();
        let script = DispatchScript::resume().window_already_alive();
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

    // --- the finish shape describes what finish_task really does ---

    /// Drive `script` and hand back only the outcome — `drive_finish` has
    /// already asserted the calls are exactly its declared steps.
    fn assert_finish_matches(script: &DispatchScript) -> std::result::Result<(), FinishError> {
        script.drive_finish().1
    }

    /// The finish counterpart of `dispatch_script_matches_a_real_dispatch`, and
    /// load-bearing in the same way: a preflight call added to `finish_task`
    /// fails here until `Finish::steps` names it, rather than shifting an
    /// unrelated test's queue by one.
    #[test]
    fn finish_script_matches_a_real_finish() {
        assert_finish_matches(&DispatchScript::finish()).expect("the happy path succeeds");
    }

    #[test]
    fn finish_script_matches_a_finish_with_no_remote() {
        let (calls, result) = DispatchScript::finish().no_remote().drive_finish();
        result.expect("no remote is not an error, it just skips the pull");
        // The absence is the claim: assert_matches already rejects an extra
        // call, and this names why that one in particular must not appear.
        assert!(
            !calls.iter().any(|(_, args)| args.contains(&"pull".into())),
            "no remote means no pull: {calls:?}"
        );
    }

    // That a finish issues no tmux call at all is asserted where the behaviour
    // lives, by `finish_task_issues_no_tmux_command` in `src/dispatch/finish.rs`.

    #[test]
    fn finish_script_stops_when_head_is_not_on_the_base_branch() {
        let err =
            assert_finish_matches(&DispatchScript::finish().head_branch("feature-x")).unwrap_err();
        assert!(matches!(err, FinishError::NotOnDefaultBranch { .. }));
    }

    #[test]
    fn finish_script_stops_on_a_dirty_primary_worktree() {
        let script = DispatchScript::finish().dirty_primary(&["src/unrelated.rs"]);
        let err = assert_finish_matches(&script).unwrap_err();
        assert!(
            matches!(err, FinishError::DirtyPrimaryWorktree { ref files, .. }
                if files == &["src/unrelated.rs".to_string()]),
            "the declared dirty paths must reach the error, got: {err}"
        );
    }

    /// Both conflict shapes must classify as conflicts and name the same files,
    /// because `is_rebase_conflict` reads both streams — a regression to reading
    /// only one would still pass the other's test.
    #[test]
    fn finish_script_matches_a_conflicting_rebase_on_either_stream() {
        for script in [
            DispatchScript::finish().rebase_conflicts_in_stdout(&["lib.rs"]),
            DispatchScript::finish().rebase_conflicts_in_stderr(&["lib.rs"]),
        ] {
            let err = assert_finish_matches(&script).unwrap_err();
            assert!(
                matches!(err, FinishError::RebaseConflict { ref files, .. }
                    if files == &["lib.rs".to_string()]),
                "a conflict shape must name its files, got: {err}"
            );
        }
    }

    /// A non-conflict failure is aborted too, but reads no porcelain: there is
    /// no conflict to name. Declared as the absence of `ConflictStatus`, so
    /// `assert_matches` rejects one appearing.
    #[test]
    fn finish_script_matches_a_non_conflict_rebase_failure() {
        let script = DispatchScript::finish().rebase_fails();
        let err = assert_finish_matches(&script).unwrap_err();
        assert!(
            matches!(err, FinishError::Other(ref m) if m.contains("Rebase failed")),
            "a non-conflict rebase failure is Other, got: {err}"
        );
        assert_eq!(
            script.index_of(Step::RebaseAbort),
            script.index_of(Step::Rebase) + 1,
            "the abort follows the rebase directly, with no porcelain read between"
        );
    }

    /// A rebase that never ran leaves nothing to abort, so the sequence ends at
    /// the rebase itself.
    #[test]
    fn finish_script_matches_a_rebase_that_could_not_be_spawned() {
        let script = DispatchScript::finish().rebase_cannot_run();
        let err = assert_finish_matches(&script).unwrap_err();
        assert!(
            matches!(err, FinishError::Other(ref m) if m.contains("Failed to run git rebase")),
            "a rebase that could not run must say so, got: {err}"
        );
        assert_eq!(*script.steps().last().unwrap(), Step::Rebase);
    }

    #[test]
    fn finish_script_matches_a_remote_probe_that_could_not_be_run() {
        let script = DispatchScript::finish().remote_probe_cannot_run();
        let err = assert_finish_matches(&script).unwrap_err();
        assert!(
            matches!(err, FinishError::Other(ref m) if m.contains(GIT_NOT_ON_PATH)),
            "a probe that cannot run must carry why, got: {err}"
        );
        assert_eq!(*script.steps().last().unwrap(), Step::OriginProbe);
    }

    #[test]
    fn finish_script_matches_a_failing_pull() {
        let err = assert_finish_matches(&DispatchScript::finish().pull_fails()).unwrap_err();
        assert!(
            matches!(err, FinishError::Other(ref m) if m.contains("Failed to pull")),
            "a failing pull is Other, got: {err}"
        );
    }

    #[test]
    fn finish_script_matches_a_failing_fast_forward() {
        let err =
            assert_finish_matches(&DispatchScript::finish().fast_forward_fails()).unwrap_err();
        assert!(
            matches!(err, FinishError::Other(ref m) if m.contains("Fast-forward failed")),
            "a failing fast-forward is Other, got: {err}"
        );
    }

    #[test]
    fn finish_script_matches_a_head_read_that_could_not_be_run() {
        let script = DispatchScript::finish().current_branch_cannot_run();
        let err = assert_finish_matches(&script).unwrap_err();
        assert!(
            matches!(err, FinishError::Other(ref m) if m.contains("Failed to check current branch")),
            "a HEAD read that cannot run must say so, got: {err}"
        );
        assert_eq!(script.steps(), vec![Step::CurrentBranch]);
    }

    /// Every timing-out shape must surface as a *timeout*, which only holds
    /// because the queued response is a success the bound never waits for.
    #[test]
    fn finish_script_times_out_each_bounded_call() {
        for (script, expected) in [
            (
                DispatchScript::finish().pull_times_out(FINISH_TIMEOUT),
                "Failed to pull",
            ),
            (
                DispatchScript::finish().rebase_times_out(FINISH_TIMEOUT),
                "Failed to run git rebase",
            ),
            (
                DispatchScript::finish().fast_forward_times_out(FINISH_TIMEOUT),
                "Failed to fast-forward",
            ),
        ] {
            let err = assert_finish_matches(&script).unwrap_err();
            assert!(
                matches!(err, FinishError::Other(ref m) if m.contains(expected) && m.contains("timed out")),
                "expected a timed-out {expected}, got: {err}"
            );
        }
    }

    /// The base branch reaches both the HEAD reply and the calls' argv, so a
    /// non-default one needs no hand-built response.
    #[test]
    fn finish_script_drives_a_non_default_base_branch() {
        let script = DispatchScript::finish().base_branch("develop").no_remote();
        let (calls, result) = script.drive_finish();
        result.expect("develop is as good a base as main");
        assert!(
            calls[script.index_of(Step::Rebase)]
                .1
                .contains(&"develop".to_string()),
            "the rebase must target the declared base branch: {calls:?}"
        );
        assert!(
            !calls
                .iter()
                .any(|(_, args)| args.iter().any(|a| a == "symbolic-ref")),
            "an explicit base branch must not be detected: {calls:?}"
        );
    }

    /// The finish counterpart of `index_of_shifts_with_the_optional_leading_steps`:
    /// the pull is conditional, so everything after it moves by one and no call
    /// site has to know that.
    #[test]
    fn finish_index_of_shifts_with_the_optional_pull() {
        let with_pull = DispatchScript::finish();
        assert_eq!(with_pull.index_of(Step::CurrentBranch), 0);
        assert_eq!(with_pull.index_of(Step::DirtyCheck), 1);
        assert_eq!(with_pull.index_of(Step::OriginProbe), 2);
        assert_eq!(with_pull.index_of(Step::Pull), 3);
        assert_eq!(with_pull.index_of(Step::Rebase), 4);
        assert_eq!(with_pull.index_of(Step::FastForward), 5);

        let without = DispatchScript::finish().no_remote();
        assert_eq!(without.index_of(Step::Rebase), 3);
        assert_eq!(without.index_of(Step::FastForward), 4);
    }

    #[test]
    #[should_panic(expected = "Pull is not part of this script's sequence")]
    fn finish_index_of_panics_for_a_step_this_shape_never_issues() {
        DispatchScript::finish().no_remote().index_of(Step::Pull);
    }

    #[test]
    #[should_panic(expected = "only applies to a finish() shape")]
    fn a_finish_modifier_on_a_dispatch_shape_panics() {
        let _ = DispatchScript::dispatch().no_remote();
    }

    /// The mirror guard. Without it a dispatch modifier on a finish shape sets a
    /// field `steps()` skips past — it compiles, passes, and asserts nothing.
    #[test]
    #[should_panic(expected = "only applies to a dispatch/resume/provision shape")]
    fn a_dispatch_modifier_on_a_finish_shape_panics() {
        let _ = DispatchScript::finish().fetch_times_out(Duration::from_millis(1));
    }

    /// The scope check earns its keep on `ConflictStatus` above all: a
    /// mid-rebase porcelain read issued against the repo root instead of the
    /// worktree returns the *repo's* status, so `RebaseConflict` would name the
    /// wrong files — and argv alone cannot tell the two reads apart.
    #[test]
    #[should_panic(expected = "must run in the Worktree")]
    fn assert_matches_rejects_a_mid_rebase_read_against_the_repo_root() {
        let script = DispatchScript::finish()
            .no_remote()
            .rebase_conflicts_in_stdout(&["lib.rs"]);
        let (mut calls, _) = script.drive_finish();
        // Re-point the mid-rebase read at the repo root, leaving argv otherwise
        // identical — exactly the drift a program+argv matcher cannot see.
        let at = script.index_of(Step::ConflictStatus);
        let dash_c = calls[at].1.iter().position(|a| a == "-C").unwrap();
        calls[at].1[dash_c + 1] = FINISH_REPO.to_string();
        script.assert_matches(&calls);
    }

    /// And the other direction: the preflight dirty check belongs to the repo
    /// root, not the worktree.
    #[test]
    #[should_panic(expected = "must run in the Repo")]
    fn assert_matches_rejects_a_dirty_check_against_the_worktree() {
        let script = DispatchScript::finish().no_remote();
        let (mut calls, _) = script.drive_finish();
        let at = script.index_of(Step::DirtyCheck);
        let dash_c = calls[at].1.iter().position(|a| a == "-C").unwrap();
        calls[at].1[dash_c + 1] = FINISH_WORKTREE.to_string();
        script.assert_matches(&calls);
    }

    #[test]
    #[should_panic(expected = "a finish() shape states its failures directly")]
    fn fails_at_rejects_a_finish_shape() {
        let _ = DispatchScript::finish().fails_at(Step::Rebase);
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
    #[should_panic(expected = "should be HasWindowQuery")]
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
