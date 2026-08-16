# Plan: worktree start point — local `<base>` vs `origin/<base>` (task #3810)

> **For agentic workers:** REQUIRED SUB-SKILL: Use `superpowers:subagent-driven-development` (recommended) or `superpowers:executing-plans` to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

Design: `docs/superpowers/specs/2026-08-01-worktree-start-point-design.md`

**Goal:** Stop branching every worktree off `origin/<base>` when local `<base>`
is ahead of it, abort the dispatch on an unreachable origin instead of silently
using a stale local ref, and make the agent's rebase preamble target whichever
ref was actually chosen.

**Architecture:** Three changes inside `src/dispatch/`. Provisioning gains a
classified fetch (404-class → local, infra-class → `Err`) and a start-point
selection that prefers local `<base>` only on a positive `ahead > 0` reading
from `repo_sync::ahead_behind`. `dispatch_with_prompt` is reordered so the
preamble is chosen *after* provisioning, from the resolved ref.

**Tech Stack:** Rust 2021, `anyhow`, `MockProcessRunner` for argv-shape tests,
real tmux + real git for `tests/tmux_lifecycle.rs`.

## Global Constraints

- TDD throughout: write the test, watch it fail for the right reason, then implement.
- Repo convention is **spec → tests → code**. Task 1 updates the Allium spec first.
- Inline test modules need `#![allow(clippy::unwrap_used, clippy::expect_used)]` at the top — see `src/db/tests/mod.rs`.
- No `tokio::time::sleep` anywhere under `src/`/`tests/`; no `std::thread::sleep` in test files (`./scripts/check-no-test-sleep.sh`, pre-push hook).
- Bare `unwrap()`/`expect()` outside tests are a hard error under the pre-push `cargo clippy --all-targets -- -D warnings`.
- Prefer `Read`/`Edit`/`Write` over shell file operations.
- Task verify command: `cargo fmt --check && cargo test && ./scripts/check-doc-paths.sh`
- Snapshot backend is 120×40 — **do not** change it. Zero snapshot churn is expected in this work; any `.snap.new` signals something unintended.

---

## Task 1: Spec first

**Files:**
- Modify: `docs/specs/dispatch.allium:196-237`, and the `{rebase_preamble}` line in the prompt skeleton at `:250-252`

**Interfaces:**
- Consumes: nothing
- Produces: the written rule that Tasks 3–8 implement

- [ ] **Step 1: Rewrite the provisioning block**

Use the `allium:tend` skill. The block currently says the fetch always yields
`origin/<base>` with a blanket fallback to local on failure. Replace with:

- The fetch is classified. **404-class** (no `origin` remote, or origin has no
  such branch) is not an error: local `<base>` is the only ref that exists, so
  it is used and a warning is surfaced to the agent as a `Note:` line. It is
  **not retried** — retrying a branch that does not exist cannot succeed.
  **infra-class** (origin reachable-or-unknown but the fetch failed) is retried
  up to 3 times and then **aborts the dispatch**; no worktree, no tmux window,
  no agent.
- Classification is by exit code, never by matching git's stderr text:
  `git ls-remote --exit-code origin refs/heads/<base>` returns `2` for "no
  matching ref" and `128` for "could not reach the remote".
- On a successful fetch the start point is chosen by comparing local `<base>`
  with `origin/<base>`: local wins only when it holds commits origin lacks
  (`ahead > 0`); otherwise `origin/<base>`. Local is *never* chosen on an
  unmeasurable comparison, because that is what "local `<base>` does not exist"
  looks like.
- Divergence (`ahead > 0 && behind > 0`) takes local silently; the repo-sync
  drift indicator is the human-facing signal.
- PR-based review worktrees (tag `pr-review`/`dependabot` with a PR URL) skip
  the comparison entirely and always use `origin/<headRefName>`.
- The prompt preamble is conditional and targets the resolved ref: emitted only
  for a reused worktree (or a PR worktree), absent on a fresh dispatch because
  the branch already *is* the start point.

Record the invariant: **the preamble target is always the resolved base branch**
(`task.base_branch`, else the detected default), never a literal `main`.

- [ ] **Step 2: Note pre-existing staleness without fixing it**

`dispatch.allium:214` lists `build_epic_planning_prompt` as a prompt builder;
no such function exists anywhere in `src/`. Leave it — it is a separate
cleanup — but note it so `allium:weed` in Task 9 is not confused about what
this change touched.

- [ ] **Step 3: Verify**

Run: `./scripts/check-doc-paths.sh`
Expected: `check-doc-paths: all references resolve`

- [ ] **Step 4: Commit**

```bash
git add docs/specs/dispatch.allium
git commit -m "docs(allium): specify classified fetch and start-point selection"
```

---

## Task 2: Mock helper for exit-code classification

`MockProcessRunner` can only produce exit `0` and exit `1`. Classification reads
exit `2` vs `128`, so the helper comes first.

**Files:**
- Modify: `src/process.rs:430-437` (add next to `fail`), `src/process.rs:476-480` (add next to `exit_fail`)

**Interfaces:**
- Produces: `MockProcessRunner::fail_with_code(code: i32, stderr: &str) -> Result<Output>`, `process::exit_code(code: i32) -> ExitStatus`

- [ ] **Step 1: Write the failing test**

In `src/process.rs`'s existing `mod tests`:

```rust
#[test]
fn fail_with_code_reports_the_requested_exit_code() {
    let out = MockProcessRunner::fail_with_code(2, "no matching ref").unwrap();
    assert_eq!(out.status.code(), Some(2));
    assert!(!out.status.success());
    assert_eq!(String::from_utf8_lossy(&out.stderr), "no matching ref");
}
```

- [ ] **Step 2: Run test to verify it fails**

Run: `cargo test process::tests::fail_with_code_reports_the_requested_exit_code`
Expected: FAIL — `no function or associated item named 'fail_with_code'`

- [ ] **Step 3: Implement**

Next to `exit_fail` (`src/process.rs:476`):

```rust
/// An `ExitStatus` carrying a specific exit code, for callers that classify on
/// the code rather than the message (e.g. `git ls-remote --exit-code`).
#[cfg(unix)]
pub fn exit_code(code: i32) -> std::process::ExitStatus {
    use std::os::unix::process::ExitStatusExt;
    // Raw status word: the exit code lives in the high byte.
    std::process::ExitStatus::from_raw(code << 8)
}
```

Next to `fail` (`src/process.rs:431`), inside `impl MockProcessRunner`:

```rust
/// Failed Output with a specific exit code. `fail` hardcodes 1, which cannot
/// express the codes `git ls-remote --exit-code` uses to distinguish "no
/// matching ref" (2) from "could not reach the remote" (128).
pub fn fail_with_code(code: i32, stderr: &str) -> Result<Output> {
    Ok(Output {
        status: exit_code(code),
        stdout: vec![],
        stderr: stderr.as_bytes().to_vec(),
    })
}
```

- [ ] **Step 4: Run test to verify it passes**

Run: `cargo test process::tests::fail_with_code_reports_the_requested_exit_code`
Expected: PASS

- [ ] **Step 5: Commit**

```bash
git add src/process.rs
git commit -m "test(process): let MockProcessRunner produce a specific exit code"
```

---

## Task 3: The `StartPoint` type

**Files:**
- Modify: `src/dispatch/worktree.rs` (add after the constants, before `ensure_dispatch_dir_and_gitignore`)

**Interfaces:**
- Produces: `pub(super) enum StartPoint { Remote { base: String }, Local { base: String } }` with `fn git_ref(&self) -> String` and `fn base(&self) -> &str`

- [ ] **Step 1: Write the failing test**

Add a new `mod start_point_tests` at the bottom of `src/dispatch/worktree.rs`:

```rust
#[cfg(test)]
#[allow(clippy::unwrap_used, clippy::expect_used)]
mod start_point_tests {
    use super::*;

    #[test]
    fn remote_start_point_refs_origin_and_keeps_the_bare_base() {
        let sp = StartPoint::Remote { base: "develop".to_string() };
        assert_eq!(sp.git_ref(), "origin/develop");
        assert_eq!(sp.base(), "develop");
    }

    #[test]
    fn local_start_point_refs_the_bare_branch() {
        let sp = StartPoint::Local { base: "develop".to_string() };
        assert_eq!(sp.git_ref(), "develop");
        assert_eq!(sp.base(), "develop");
    }
}
```

- [ ] **Step 2: Run test to verify it fails**

Run: `cargo test dispatch::worktree::start_point_tests`
Expected: FAIL — `cannot find type 'StartPoint' in this scope`

- [ ] **Step 3: Implement**

```rust
/// Which ref a worktree branch was created from — and therefore what the agent
/// should rebase onto if it ever needs to.
///
/// Both arms carry the bare branch name because the `git fetch origin <base>`
/// line is identical either way; only the rebase target differs.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(super) enum StartPoint {
    /// `origin/<base>`: origin is at least as new as local `<base>`.
    Remote { base: String },
    /// Bare local `<base>`: it carries commits `origin/<base>` does not, or
    /// origin has no such branch at all.
    Local { base: String },
}

impl StartPoint {
    /// The ref to hand `git worktree add`, and to rebase onto.
    pub(super) fn git_ref(&self) -> String {
        match self {
            StartPoint::Remote { base } => format!("origin/{base}"),
            StartPoint::Local { base } => base.clone(),
        }
    }

    /// The bare branch name, for the `git fetch origin <base>` line.
    pub(super) fn base(&self) -> &str {
        match self {
            StartPoint::Remote { base } | StartPoint::Local { base } => base,
        }
    }
}
```

- [ ] **Step 4: Run test to verify it passes**

Run: `cargo test dispatch::worktree::start_point_tests`
Expected: PASS (2 tests)

- [ ] **Step 5: Commit**

```bash
git add src/dispatch/worktree.rs
git commit -m "feat(dispatch): add StartPoint, the ref a worktree branched from"
```

---

## Task 4: Classified fetch

**Files:**
- Modify: `src/dispatch/worktree.rs:76-98` — `fetch_origin_with_retry` becomes `fetch_origin`
- Test: `src/dispatch/worktree.rs`, new `mod fetch_tests`

**Interfaces:**
- Consumes: `crate::git::has_origin_remote` (`src/git.rs:33`), `MockProcessRunner::fail_with_code` (Task 2)
- Produces: `enum FetchOutcome { Fetched, NoOriginRef(String) }`, `fn fetch_origin(runner, repo_path, base, timeout) -> Result<FetchOutcome>`

- [ ] **Step 1: Write the failing tests**

```rust
#[cfg(test)]
#[allow(clippy::unwrap_used, clippy::expect_used)]
mod fetch_tests {
    use super::*;
    use crate::process::MockProcessRunner;
    use std::time::Duration;

    const T: Duration = Duration::from_secs(5);

    #[test]
    fn successful_fetch_reports_fetched() {
        let mock = MockProcessRunner::new(vec![MockProcessRunner::ok()]);
        assert!(matches!(
            fetch_origin(&mock, "/repo", "main", T).unwrap(),
            FetchOutcome::Fetched
        ));
        assert_eq!(mock.recorded_calls().len(), 1, "no classification needed");
    }

    #[test]
    fn missing_origin_remote_is_not_retried() {
        let mock = MockProcessRunner::new(vec![
            MockProcessRunner::fail("no such remote"), // git fetch
            MockProcessRunner::fail("no origin"),      // git remote get-url origin
        ]);
        let outcome = fetch_origin(&mock, "/repo", "main", T).unwrap();
        let FetchOutcome::NoOriginRef(warning) = outcome else {
            panic!("expected NoOriginRef");
        };
        assert!(warning.contains("origin remote"), "got: {warning}");
        assert_eq!(
            mock.recorded_calls().len(),
            2,
            "one fetch, one classification — no retries: {:?}",
            mock.recorded_calls()
        );
    }

    #[test]
    fn branch_absent_from_origin_is_not_retried() {
        let mock = MockProcessRunner::new(vec![
            MockProcessRunner::fail("couldn't find remote ref"), // git fetch
            MockProcessRunner::ok(),                             // git remote get-url origin
            MockProcessRunner::fail_with_code(2, ""),            // git ls-remote --exit-code
        ]);
        let outcome = fetch_origin(&mock, "/repo", "nosuch", T).unwrap();
        let FetchOutcome::NoOriginRef(warning) = outcome else {
            panic!("expected NoOriginRef");
        };
        assert!(warning.contains("nosuch"), "got: {warning}");
        assert_eq!(mock.recorded_calls().len(), 3, "no retries after a 404");
    }

    #[test]
    fn unreachable_origin_is_retried_then_aborts() {
        let mock = MockProcessRunner::new(vec![
            MockProcessRunner::fail("Could not resolve host"), // fetch 1
            MockProcessRunner::ok(),                           // remote get-url origin
            MockProcessRunner::fail_with_code(128, ""),        // ls-remote: unreachable
            MockProcessRunner::fail("Could not resolve host"), // fetch 2
            MockProcessRunner::fail("Could not resolve host"), // fetch 3
        ]);
        let err = fetch_origin(&mock, "/repo", "main", T).unwrap_err();
        assert!(
            err.to_string().contains("Could not reach origin"),
            "got: {err}"
        );
        assert_eq!(
            mock.recorded_calls().len(),
            5,
            "classify once, then retry the fetch: {:?}",
            mock.recorded_calls()
        );
    }

    #[test]
    fn existing_ref_that_fails_to_fetch_aborts_rather_than_using_local() {
        // ls-remote finds the ref, so origin is reachable and the branch is
        // there — a fetch that still fails is infrastructure, never a 404.
        let mock = MockProcessRunner::new(vec![
            MockProcessRunner::fail("early EOF"),
            MockProcessRunner::ok(),  // remote get-url origin
            MockProcessRunner::ok(),  // ls-remote: exit 0, ref exists
            MockProcessRunner::fail("early EOF"),
            MockProcessRunner::fail("early EOF"),
        ]);
        assert!(fetch_origin(&mock, "/repo", "main", T).is_err());
    }

    #[test]
    fn fetch_succeeding_on_retry_reports_fetched() {
        let mock = MockProcessRunner::new(vec![
            MockProcessRunner::fail("early EOF"),
            MockProcessRunner::ok(),                    // remote get-url origin
            MockProcessRunner::fail_with_code(128, ""), // ls-remote: unreachable
            MockProcessRunner::ok(),                    // fetch 2 succeeds
        ]);
        assert!(matches!(
            fetch_origin(&mock, "/repo", "main", T).unwrap(),
            FetchOutcome::Fetched
        ));
    }
}
```

- [ ] **Step 2: Run tests to verify they fail**

Run: `cargo test dispatch::worktree::fetch_tests`
Expected: FAIL — `cannot find function 'fetch_origin'`

- [ ] **Step 3: Implement**

Replace `fetch_origin_with_retry` (`src/dispatch/worktree.rs:76-98`) with:

```rust
/// `git ls-remote --exit-code` returns this when no ref matched — as opposed to
/// 128, which means it could not reach the remote at all.
const LS_REMOTE_NO_MATCHING_REF: i32 = 2;

/// The outcome of making `origin/<base>` current before provisioning.
#[derive(Debug)]
enum FetchOutcome {
    /// `origin/<base>` is up to date locally.
    Fetched,
    /// There is no `origin/<base>` to fetch. Carries the message shown to the
    /// agent as a `Note:` line.
    NoOriginRef(String),
}

/// Why a `git fetch origin <base>` failed.
enum FetchFailure {
    /// Nothing to fetch: no `origin` remote, or origin has no such branch.
    NoOriginRef(String),
    /// Origin has the branch, or we could not even determine that. Either way
    /// this is infrastructure, not a missing ref.
    Unreachable,
}

/// Classify a fetch failure without pattern-matching git's stderr text.
///
/// `git fetch` exits 128 for a missing ref, an unresolvable host and an
/// unreadable remote alike, so its own status cannot classify.
/// `git ls-remote --exit-code` can: 2 means "no matching ref", 128 means "could
/// not reach the remote". Anything we cannot positively identify as a missing
/// ref is treated as unreachable — the safe polarity, since only a recognised
/// 404 earns the local-branch fallback.
fn classify_fetch_failure(
    runner: &dyn ProcessRunner,
    repo_path: &str,
    base: &str,
    timeout: Duration,
) -> FetchFailure {
    if !crate::git::has_origin_remote(repo_path, runner) {
        return FetchFailure::NoOriginRef("no origin remote is configured".to_string());
    }
    let refspec = format!("refs/heads/{base}");
    let probe = runner.run_with_timeout(
        "git",
        &[
            "-C",
            repo_path,
            "ls-remote",
            "--exit-code",
            "origin",
            &refspec,
        ],
        timeout,
    );
    match probe {
        Ok(output) if output.status.code() == Some(LS_REMOTE_NO_MATCHING_REF) => {
            FetchFailure::NoOriginRef(format!("origin has no branch {base}"))
        }
        _ => FetchFailure::Unreachable,
    }
}

/// Make `origin/<base>` current, or establish that there is no such ref.
///
/// An infrastructure failure is retried up to `FETCH_MAX_ATTEMPTS` and then
/// aborts the dispatch: a worktree silently branched off a stale local ref is
/// worse than a dispatch that refuses to start. A missing ref is not retried —
/// retrying a branch that does not exist cannot succeed — and is not an error,
/// because local `<base>` is then the only ref there is.
fn fetch_origin(
    runner: &dyn ProcessRunner,
    repo_path: &str,
    base: &str,
    timeout: Duration,
) -> Result<FetchOutcome> {
    let mut last_err = String::new();
    for attempt in 1..=FETCH_MAX_ATTEMPTS {
        match runner.run_with_timeout("git", &["-C", repo_path, "fetch", "origin", base], timeout) {
            Ok(output) if output.status.success() => return Ok(FetchOutcome::Fetched),
            Ok(output) => last_err = stderr_str(&output),
            Err(e) => last_err = e.to_string(),
        }
        // Classify once, on the first failure: the answer cannot change between
        // attempts, and a 404 must not burn the retry budget.
        if attempt == 1 {
            if let FetchFailure::NoOriginRef(reason) =
                classify_fetch_failure(runner, repo_path, base, timeout)
            {
                tracing::info!(base, %reason, "no origin ref to fetch; using the local branch");
                return Ok(FetchOutcome::NoOriginRef(format!(
                    "Could not fetch origin/{base} ({reason}); this worktree is based on \
                     the local {base} branch."
                )));
            }
        }
        if attempt < FETCH_MAX_ATTEMPTS {
            std::thread::sleep(FETCH_RETRY_DELAY);
        }
    }
    tracing::warn!(base, error = %last_err, "could not reach origin; aborting dispatch");
    anyhow::bail!(
        "Could not reach origin to fetch {base} after {FETCH_MAX_ATTEMPTS} attempts: {last_err}"
    )
}
```

- [ ] **Step 4: Run tests to verify they pass**

Run: `cargo test dispatch::worktree::fetch_tests`
Expected: PASS (6 tests). `cargo build` will still fail — `resolve_start_point`
calls the removed `fetch_origin_with_retry`. Task 5 fixes that.

- [ ] **Step 5: Commit**

Deferred to Task 5 — the tree does not compile between these two tasks.

---

## Task 5: Start-point selection and provisioning

**Files:**
- Modify: `src/dispatch/worktree.rs:100-203` — delete `resolve_start_point`, add `BaseRef` + `select_start_point`, rewrite `provision_worktree`'s signature and body, extend `ProvisionResult`
- Test: `src/dispatch/worktree.rs`, new `mod selection_tests`

**Interfaces:**
- Consumes: `StartPoint` (Task 3), `fetch_origin` (Task 4), `crate::repo_sync::ahead_behind` (`src/repo_sync.rs:125`)
- Produces: `pub(super) enum BaseRef<'a> { Branch(&'a str), PrHead(&'a str) }`; `provision_worktree(task, runner, base: Option<BaseRef<'_>>, timeout)`; `ProvisionResult { worktree_path, tmux_window, fetch_warning, start_point: Option<StartPoint>, reused_worktree: bool }`

- [ ] **Step 1: Write the failing tests**

```rust
#[cfg(test)]
#[allow(clippy::unwrap_used, clippy::expect_used)]
mod selection_tests {
    use super::*;
    use crate::process::MockProcessRunner;

    // `git rev-list --count --left-right <base>...origin/<base>` prints
    // "<ahead>\t<behind>".
    fn counts(ahead: u32, behind: u32) -> anyhow::Result<std::process::Output> {
        MockProcessRunner::ok_with_stdout(format!("{ahead}\t{behind}\n").as_bytes())
    }

    #[test]
    fn local_wins_when_it_holds_commits_origin_lacks() {
        let mock = MockProcessRunner::new(vec![counts(3, 0)]);
        assert_eq!(
            select_start_point(&mock, "/repo", "main"),
            StartPoint::Local { base: "main".to_string() }
        );
    }

    #[test]
    fn origin_wins_when_local_is_behind() {
        let mock = MockProcessRunner::new(vec![counts(0, 2)]);
        assert_eq!(
            select_start_point(&mock, "/repo", "main"),
            StartPoint::Remote { base: "main".to_string() }
        );
    }

    #[test]
    fn origin_wins_when_the_two_are_level() {
        let mock = MockProcessRunner::new(vec![counts(0, 0)]);
        assert_eq!(
            select_start_point(&mock, "/repo", "main"),
            StartPoint::Remote { base: "main".to_string() }
        );
    }

    #[test]
    fn diverged_takes_local_silently() {
        let mock = MockProcessRunner::new(vec![counts(3, 2)]);
        assert_eq!(
            select_start_point(&mock, "/repo", "main"),
            StartPoint::Local { base: "main".to_string() }
        );
    }

    #[test]
    fn unmeasurable_falls_to_origin_not_local() {
        // This is what "local <base> does not exist" looks like — a base branch
        // the human never checked out. Preferring local here would hand
        // `git worktree add` a ref that is not there.
        let mock = MockProcessRunner::new(vec![MockProcessRunner::fail("unknown revision")]);
        assert_eq!(
            select_start_point(&mock, "/repo", "develop"),
            StartPoint::Remote { base: "develop".to_string() }
        );
    }
}
```

And, in `src/dispatch/tests.rs`, the PR-worktree rule:

```rust
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
        !calls.iter().any(|(_, args)| args.contains(&"rev-list".to_string())),
        "a PR head branch must never be compared against a local ref: {calls:?}"
    );
    assert_eq!(
        result.start_point,
        Some(StartPoint::Remote { base: "feature-x".to_string() })
    );
}
```

- [ ] **Step 2: Run tests to verify they fail**

Run: `cargo test dispatch::worktree::selection_tests`
Expected: FAIL — `cannot find function 'select_start_point'`

- [ ] **Step 3: Implement**

Delete `resolve_start_point` (`worktree.rs:100-126`) entirely. Add:

```rust
/// What a worktree is being based on, and whether local history may be
/// preferred over origin's.
#[derive(Debug, Clone, Copy)]
pub(super) enum BaseRef<'a> {
    /// The repo's base branch. Local `<base>` may legitimately hold commits
    /// origin lacks, so the two are compared and the one with unique commits
    /// wins.
    Branch(&'a str),
    /// A PR's head branch. Always `origin/<branch>`, never compared: a review
    /// must see exactly the PR's code, and a stale local branch of the same
    /// name would silently poison it.
    PrHead(&'a str),
}

impl BaseRef<'_> {
    fn name(&self) -> &str {
        match self {
            BaseRef::Branch(b) | BaseRef::PrHead(b) => b,
        }
    }
}

/// Choose the ref a new worktree branch starts from, given a fetch that just
/// succeeded so both refs are current.
///
/// Local `<base>` wins only on a positive `ahead > 0` reading. That polarity is
/// load-bearing: `ahead_behind` yields `None` whenever local `<base>` does not
/// resolve, which is the normal case for a base branch the human never checked
/// out, and preferring local there would fail `git worktree add`.
fn select_start_point(runner: &dyn ProcessRunner, repo_path: &str, base: &str) -> StartPoint {
    let base = base.to_string();
    match crate::repo_sync::ahead_behind(repo_path, &base, runner) {
        Some(counts) if counts.ahead > 0 => StartPoint::Local { base },
        _ => StartPoint::Remote { base },
    }
}
```

Extend `ProvisionResult` (`worktree.rs:67-74`):

```rust
#[derive(Debug)]
pub(super) struct ProvisionResult {
    pub(super) worktree_path: String,
    pub(super) tmux_window: String,
    /// `Some(...)` when there is no `origin/<base>` to base on and the local
    /// branch was used instead. Injected into the agent's prompt as a `Note:`
    /// line by `dispatch_with_prompt`.
    pub(super) fetch_warning: Option<String>,
    /// The ref the branch was created from. `None` when no base was given.
    /// The caller needs it to point the rebase preamble at the same ref.
    pub(super) start_point: Option<StartPoint>,
    /// True when the worktree directory already existed and `git worktree add`
    /// was skipped, so the branch may still hold a previous attempt's state.
    pub(super) reused_worktree: bool,
}
```

Rewrite the middle of `provision_worktree`:

```rust
pub(super) fn provision_worktree(
    task: &Task,
    runner: &dyn ProcessRunner,
    base: Option<BaseRef<'_>>,
    timeout: Duration,
) -> Result<ProvisionResult> {
    // ... unchanged through `fs::create_dir_all` ...

    // The fetch runs unconditionally — even when reusing an existing worktree
    // directory — so `origin/<base>` stays fresh for whatever rebases onto it
    // later. An unreachable origin aborts here rather than quietly producing a
    // worktree based on a stale local ref.
    let (start_point, fetch_warning): (Option<StartPoint>, Option<String>) = match base {
        Some(base_ref) => match fetch_origin(runner, &repo_path, base_ref.name(), timeout)? {
            FetchOutcome::Fetched => {
                let sp = match base_ref {
                    BaseRef::PrHead(b) => StartPoint::Remote { base: b.to_string() },
                    BaseRef::Branch(b) => select_start_point(runner, &repo_path, b),
                };
                (Some(sp), None)
            }
            FetchOutcome::NoOriginRef(warning) => (
                Some(StartPoint::Local {
                    base: base_ref.name().to_string(),
                }),
                Some(warning),
            ),
        },
        None => (None, None),
    };

    let start_ref = start_point.as_ref().map(StartPoint::git_ref);

    let reused_worktree = std::path::Path::new(&worktree_path).exists();
    if reused_worktree {
        tracing::info!(task_id = task.id.0, %worktree_path, "worktree already exists, reusing");
    } else {
        let mut args = vec![
            "-C",
            &repo_path,
            "worktree",
            "add",
            &worktree_path,
            "-B",
            &worktree_name,
        ];
        if let Some(sp) = start_ref.as_deref() {
            args.push(sp);
        }
        let output = runner
            .run_with_timeout("git", &args, timeout)
            .context("failed to run git worktree add")?;
        anyhow::ensure!(
            output.status.success(),
            "git worktree add failed: {}",
            stderr_str(&output)
        );
    }

    // ... unchanged tmux calls ...

    Ok(ProvisionResult {
        worktree_path,
        tmux_window,
        fetch_warning,
        start_point,
        reused_worktree,
    })
}
```

Update the sole production caller (`src/dispatch/agents.rs:163`) to
`Some(BaseRef::Branch(&effective_base))` for now — Task 8 refines it to pick
`PrHead` for review tasks.

- [ ] **Step 4: Run tests**

Run: `cargo test dispatch::worktree`
Expected: PASS. `cargo test dispatch::` still fails — existing `tests.rs` call
sites pass `Some("main")` rather than `Some(BaseRef::Branch("main"))`, and mocks
lack the new `rev-list` response. Task 7 fixes those.

- [ ] **Step 5: Commit**

```bash
git add src/dispatch/worktree.rs src/dispatch/agents.rs
git commit -m "feat(dispatch): classify fetch failures and prefer a local base that is ahead"
```

---

## Task 6: Preamble builders and the decision table

**Files:**
- Modify: `src/dispatch/prompts.rs:52-78` — delete `rebase_preamble`, add `reused_rebase_preamble` and `select_preamble`, keep `pr_rebase_preamble`
- Test: `src/dispatch/prompts.rs`, inline `mod tests`

**Interfaces:**
- Consumes: `StartPoint` (Task 3) via `use super::worktree::StartPoint;`
- Produces: `reused_rebase_preamble(&StartPoint) -> String`; `select_preamble(pr_branch: Option<&str>, start_point: Option<&StartPoint>, reused: bool) -> String`; `compose_prompt_head(preamble: &str, fetch_warning: Option<&str>) -> String`

- [ ] **Step 1: Write the failing tests**

```rust
#[test]
fn reused_preamble_targets_a_local_start_point() {
    let sp = StartPoint::Local { base: "develop".to_string() };
    let text = reused_rebase_preamble(&sp);
    assert!(text.contains("git fetch origin develop"), "got: {text}");
    assert!(text.contains("git rebase develop"), "got: {text}");
    assert!(
        !text.contains("git rebase origin/develop"),
        "must not drag a local-based branch back onto origin: {text}"
    );
    assert!(text.contains("git status"), "tells the agent to inspect first");
    assert!(!text.contains("main"), "no literal main: {text}");
}

#[test]
fn reused_preamble_targets_a_remote_start_point() {
    let sp = StartPoint::Remote { base: "develop".to_string() };
    let text = reused_rebase_preamble(&sp);
    assert!(text.contains("git fetch origin develop"), "got: {text}");
    assert!(text.contains("git rebase origin/develop"), "got: {text}");
}

#[test]
fn select_preamble_is_empty_for_a_fresh_worktree() {
    let sp = StartPoint::Remote { base: "main".to_string() };
    assert_eq!(select_preamble(None, Some(&sp), false), "");
}

#[test]
fn select_preamble_uses_reuse_wording_for_a_reused_worktree() {
    let sp = StartPoint::Local { base: "main".to_string() };
    let text = select_preamble(None, Some(&sp), true);
    assert!(text.contains("reused from a previous attempt"), "got: {text}");
    assert!(text.contains("git rebase main"), "mirrors the start point: {text}");
}

#[test]
fn select_preamble_prefers_the_pr_branch_regardless_of_reuse() {
    let sp = StartPoint::Remote { base: "renovate/serde-1.x".to_string() };
    for reused in [true, false] {
        let text = select_preamble(Some("renovate/serde-1.x"), Some(&sp), reused);
        assert!(
            text.contains("git rebase origin/renovate/serde-1.x"),
            "reused={reused}, got: {text}"
        );
        assert!(!text.contains("reused from a previous attempt"), "reused={reused}");
    }
}

#[test]
fn prompt_head_carries_the_warning_even_with_no_preamble() {
    // The fresh + no-origin-ref case: nothing to rebase onto, but the agent
    // must still be told its base is local-only.
    let head = compose_prompt_head("", Some("origin has no branch main"));
    assert!(head.contains("Note: origin has no branch main"), "got: {head}");
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
```

- [ ] **Step 2: Run tests to verify they fail**

Run: `cargo test dispatch::prompts`
Expected: FAIL — `cannot find function 'reused_rebase_preamble'`

- [ ] **Step 3: Implement**

Delete `rebase_preamble` (`prompts.rs:52-60`). Add:

```rust
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
```

Add `use super::worktree::StartPoint;` to the imports at the top of `prompts.rs`.

- [ ] **Step 4: Run tests to verify they pass**

Run: `cargo test dispatch::prompts`
Expected: PASS for the new tests. `agents.rs` still calls the deleted
`rebase_preamble`; Task 8 rewires it.

- [ ] **Step 5: Commit**

Deferred to Task 8 — the tree does not compile until `agents.rs` is rewired.

---

## Task 7: Update existing provisioning tests

These assert the behaviour being replaced. They are rewritten, not relaxed.

**Files:**
- Modify: `src/dispatch/tests.rs` — all `provision_worktree` call sites and their mocks

**Interfaces:**
- Consumes: `BaseRef` (Task 5), `StartPoint` (Task 3)

- [ ] **Step 1: Convert the call sites**

Every `provision_worktree(&task, &mock, Some("X"), ...)` becomes
`Some(BaseRef::Branch("X"))`. The six `None` call sites (`:1161`, `:1185`,
`:2330`, `:2348`, `:2367`, `:2674`) are unchanged — `None` still means "no
explicit start point".

- [ ] **Step 2: Add the `rev-list` response to every mock that now needs one**

Any mock whose fetch succeeds and whose base is a `BaseRef::Branch` now issues a
`git rev-list --count --left-right` call between the fetch and the
`git worktree add`. Insert
`MockProcessRunner::ok_with_stdout(b"0\t0\n")` there and update the sequence
comments. This keeps those tests asserting `origin/<base>`, which stays correct:
no local-only commits means origin wins.

Affected: `:1099`, `:1120`, `:1143`, `:1223`, `:1262`, `:1343`, `:1376`, and the
`dispatch_agent` fixtures at `:1082-1088`, `:1106-1113`, `:1129-1136`.

- [ ] **Step 3: Re-point the two fetch-failure tests**

`:1286` (`provision_worktree_falls_back_to_local_on_fetch_failure`) and `:2639`
(the fetch-timeout test) assert the blanket fallback that no longer exists.

- The fallback test becomes the **404-class** path: mock the fetch failure, then
  `MockProcessRunner::ok()` for `git remote get-url origin`, then
  `MockProcessRunner::fail_with_code(2, "")` for `ls-remote`. Assert the start
  point is bare `main` and `fetch_warning.is_some()`.
- The timeout test becomes the **infra-class** path: assert
  `provision_worktree(...).is_err()` and that no `tmux` call was issued —
  aborting must happen before any window is created.

- [ ] **Step 4: Run the suite**

Run: `cargo test dispatch::`
Expected: PASS

- [ ] **Step 5: Commit**

```bash
git add src/dispatch/tests.rs
git commit -m "test(dispatch): cover classified fetch and start-point selection"
```

---

## Task 8: Wire `dispatch_with_prompt`

**Files:**
- Modify: `src/dispatch/agents.rs:140-180`
- Test: `src/dispatch/tests.rs`

**Interfaces:**
- Consumes: `select_preamble`, `compose_prompt_head` (Task 6); `BaseRef`, `ProvisionResult` (Task 5)

- [ ] **Step 1: Write the failing test**

```rust
#[test]
fn dispatch_reused_worktree_prompt_carries_the_reuse_preamble() {
    let (_dir, repo_path, worktree_dir) = make_test_repo_with_worktree("42-fix-bug");

    let mock = MockProcessRunner::new(vec![
        MockProcessRunner::ok(),                          // git fetch origin main
        MockProcessRunner::ok_with_stdout(b"0\t0\n"),     // git rev-list (level)
        MockProcessRunner::ok(),                          // tmux new-window
        MockProcessRunner::ok(),                          // tmux set-option
        MockProcessRunner::ok(),                          // tmux set-hook
        MockProcessRunner::ok(),                          // tmux send-keys
        MockProcessRunner::ok(),                          // tmux split-window (agent tree)
    ]);

    let task = make_task(&repo_path);
    dispatch_agent(&task, &mock, None, &LearningInjections::default(), None).unwrap();

    let prompt = std::fs::read_to_string(worktree_dir.join(".claude-prompt")).unwrap();
    assert!(
        prompt.contains("reused from a previous attempt"),
        "got: {prompt}"
    );
    assert!(prompt.contains("git rebase origin/main"), "got: {prompt}");
    assert!(prompt.contains("Always work from this worktree folder"));
}
```

- [ ] **Step 2: Run test to verify it fails**

Run: `cargo test dispatch::tests::dispatch_reused_worktree_prompt_carries_the_reuse_preamble`
Expected: FAIL — the preamble is still built before provisioning and uses the deleted `rebase_preamble`

- [ ] **Step 3: Implement**

Replace `src/dispatch/agents.rs:150-176` with:

```rust
    // Match on a reference: `select_preamble` needs `pr_branch` after
    // provisioning, so the `Some` arm clones rather than moving. The `None` arm
    // may move `resolved`, which is unused thereafter.
    let effective_base: String = match &pr_branch {
        Some(branch) => branch.clone(),
        None => resolved,
    };
    // A PR head branch is never compared against a local ref — a review must
    // see exactly the PR's code.
    let base_ref = match &pr_branch {
        Some(_) => BaseRef::PrHead(&effective_base),
        None => BaseRef::Branch(&effective_base),
    };

    let provision = provision_worktree(task, runner, Some(base_ref), SUBPROCESS_TIMEOUT)?;

    let preamble = select_preamble(
        pr_branch.as_deref(),
        provision.start_point.as_ref(),
        provision.reused_worktree,
    );
    let head = compose_prompt_head(&preamble, provision.fetch_warning.as_deref());

    let prompt = make_prompt();
    let full_prompt = format!(
        "{head}Always work from this worktree folder — do not `cd` to the parent repo \
         or other directories.\n\n\
         {prompt}"
    );
```

Delete the now-redundant `match &provision.fetch_warning` block — the warning is
`compose_prompt_head`'s job. Update the `use` list at `agents.rs:11` to drop
`rebase_preamble` and add `compose_prompt_head`, `select_preamble`.

- [ ] **Step 4: Run the suite**

Run: `cargo test`
Expected: PASS. Every existing `dispatch_agent` fixture pre-creates the worktree
dir, so they all land on the *reuse* row and receive the reuse wording; they
survive because every prompt assertion uses `.contains(...)`. The sole
`.starts_with("Before starting work")` is at `tests.rs:559`, inside
`rebase_preamble_prepended_to_all_prompts`.

- [ ] **Step 5: Delete the misleading test**

Delete `rebase_preamble_prepended_to_all_prompts` (`src/dispatch/tests.rs:541-561`).
It hand-assembles the preamble and body instead of calling `dispatch_with_prompt`,
so it asserts nothing about dispatch and would pass unchanged after this work.

Re-point the two direct `rebase_preamble` tests at `:1424` (`"99-prev-task"`) and
`:1441` (`"develop"`) to `reused_rebase_preamble(&StartPoint::Remote { .. })`,
preserving what they lock: the target is the resolved base branch, never a
literal `main`.

- [ ] **Step 6: Run the suite and commit**

Run: `cargo test`
Expected: PASS

```bash
git add src/dispatch/agents.rs src/dispatch/prompts.rs src/dispatch/tests.rs
git commit -m "feat(dispatch): point the rebase preamble at the resolved start point"
```

---

## Task 9: Real-git regression coverage

A mock never runs `git worktree add` for real, so it cannot answer which commit
the branch actually lands on. `tests/tmux_lifecycle.rs` is the one place with a
real repo, a real `origin`, and a real dispatch through the production entry
point.

**Files:**
- Modify: `tests/tmux_lifecycle.rs` — add a stdout-capturing git helper and two tests

**Interfaces:**
- Consumes: `seed_repo` (`:104-118`), `git` (`:125-143`), `Fixture::dispatch` (`:178-187`)

- [ ] **Step 1: Add a stdout-capturing git helper**

The existing `git()` asserts success and discards stdout; `rev-parse` needs the
value.

```rust
/// Like `git`, but returns trimmed stdout — for queries such as `rev-parse`.
fn git_stdout(cwd: &Path, args: &[&str]) -> String {
    let out = std::process::Command::new("git")
        .args(args)
        .current_dir(cwd)
        .env("GIT_CONFIG_GLOBAL", "/dev/null")
        .env("GIT_CONFIG_SYSTEM", "/dev/null")
        .output()
        .expect("run git");
    assert!(
        out.status.success(),
        "git {:?} failed: {}",
        args,
        String::from_utf8_lossy(&out.stderr)
    );
    String::from_utf8_lossy(&out.stdout).trim().to_string()
}
```

- [ ] **Step 2: Write the failing regression test**

```rust
/// This asks a *git* question, not a tmux one. It lives in this file only
/// because this is the one harness with a real repo, a real `origin` and a real
/// dispatch — a mock cannot answer it, because a mock never runs
/// `git worktree add` for real. Do not "simplify" it onto `MockProcessRunner`.
#[test]
fn fresh_dispatch_prefers_local_base_when_it_is_ahead_of_origin() {
    // `setup_or_skip` wraps `tmux_available_or_skip`, which skips locally when
    // tmux is missing but hard-fails under `CI` — so this cannot quietly stop
    // running. This is the file's established guard pattern.
    let Some(fx) = setup_or_skip() else { return };

    // Land a commit on local main WITHOUT pushing — exactly what the rebase
    // wrap-up path produces, and the drift this task exists to respect.
    std::fs::write(fx.repo.join("landed.txt"), "from a finished task\n").unwrap();
    git(&fx.repo, &["add", "landed.txt"]);
    git(&fx.repo, &["commit", "-qm", "landed but unpushed"]);

    let local_main = git_stdout(&fx.repo, &["rev-parse", "main"]);
    let origin_main = git_stdout(&fx.repo, &["rev-parse", "origin/main"]);
    assert_ne!(local_main, origin_main, "fixture must actually be ahead");

    let result = fx.dispatch(4242);

    let branch = git_stdout(&fx.repo, &["rev-parse", "4242-some-task"]);
    assert_eq!(
        branch, local_main,
        "worktree must start from local main, which holds the landed work"
    );
    assert_ne!(branch, origin_main, "must not start from the stale origin ref");
    assert!(std::path::Path::new(&result.worktree_path).exists());
}
```

Note the fixture's own doc comment at `tests/tmux_lifecycle.rs:94-103`: the local
`origin` exists precisely so provisioning takes the normal path rather than the
fetch-failure one. That comment names `resolve_start_point` and
`fetch_origin_with_retry`, both of which this work deletes — update it to name
`fetch_origin` and `select_start_point`.

`./scripts/check-doc-symbols.sh` will **not** catch this: it scans `CLAUDE.md`,
`docs/*.md`, `docs/specs/*.allium` and `src/**/*.rs` doc comments as targets,
with `src` *and* `tests` as the code corpus — so a stale name in a `tests/` file
is unchecked. It must be fixed by hand. The same checker *does* cover
`src/dispatch/worktree.rs:101`, whose doc comment names `fetch_origin_with_retry`;
Task 4 rewrites it, so that one is already handled.

- [ ] **Step 3: Run test to verify it fails**

Run: `cargo test --test tmux_lifecycle fresh_dispatch_prefers_local_base`
Expected: FAIL — branch equals `origin_main` (the pre-fix behaviour). If it
passes before the implementation is wired in, the fixture is not actually ahead;
fix the fixture, not the assertion.

- [ ] **Step 4: Add the level-refs companion test**

```rust
/// #3804's premise, preserved: with local and origin level, the branch is the
/// start point, which is what makes a fresh dispatch's rebase a no-op.
#[test]
fn fresh_dispatch_with_level_base_starts_from_origin() {
    let Some(fx) = setup_or_skip() else { return };
    let result = fx.dispatch(4243);

    let branch = git_stdout(&fx.repo, &["rev-parse", "4243-some-task"]);
    assert_eq!(branch, git_stdout(&fx.repo, &["rev-parse", "origin/main"]));
    assert!(std::path::Path::new(&result.worktree_path).exists());
}
```

- [ ] **Step 5: Run both and commit**

Run: `cargo test --test tmux_lifecycle`
Expected: PASS (needs a running tmux server)

```bash
git add tests/tmux_lifecycle.rs
git commit -m "test(dispatch): assert worktrees start from local base when it is ahead"
```

---

## Task 10: Verify and align

- [ ] **Step 1: Format**

Run: `cargo fmt`
Then diff-check — a scoped `cargo fmt -- <files>` can still touch unrelated
files, so review what changed before staging.

- [ ] **Step 2: Clippy (the pre-push gate)**

Run: `cargo clippy --all-targets -- -D warnings`
Expected: clean. A plain `cargo build` will not catch these.

- [ ] **Step 3: Full verify command**

Run: `cargo fmt --check && cargo test && ./scripts/check-doc-paths.sh`
Expected: PASS

- [ ] **Step 4: Confirm spec and code agree**

Run the `allium:weed` skill against `docs/specs/dispatch.allium`. Expect it to
flag only the pre-existing `build_epic_planning_prompt` staleness noted in
Task 1 — anything else means the spec and implementation drifted during the work.

- [ ] **Step 5: Confirm no snapshot churn**

Run: `ls src/dispatch/snapshots/*.snap.new src/tui/tests/snapshots/*.snap.new`
Expected: no such files. The snapshots cover `build_*_prompt`, which is upstream
of the preamble, so any `.snap.new` here signals something unintended changed.

- [ ] **Step 6: Commit any formatting fallout**

```bash
git add -A
git commit -m "chore: formatting and spec alignment"
```

---

## Risks

- **Aborting on an unreachable origin blocks offline dispatch.** Intended: today
  an offline dispatch silently yields a stale worktree. The error names the
  cause and the attempt count.
- **A stale local base that is nonetheless ahead now wins.** Only reachable when
  the human has committed to local `<base>` without pushing — which is precisely
  the case this task exists to respect. The repo-sync drift indicator is the
  signal for the converse.
- **One extra `git rev-list` per dispatch.** Local-only, no network.
- **Ordering regression.** A future edit could reintroduce a pre-provision
  preamble that ignores the resolution. The `select_preamble` table tests make
  the rule the single source of truth, so such an edit fails tests rather than
  regressing silently.
