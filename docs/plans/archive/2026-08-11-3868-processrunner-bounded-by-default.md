# ProcessRunner Bounded By Default — Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** Invert `ProcessRunner` so `run_with_timeout` is the one required method and `run` delegates to it at `SUBPROCESS_TIMEOUT`, leaving the trait with no unbounded path.

**Architecture:** The trait's defaulting direction flips from unsafe to safe. Today `run_with_timeout` has a default that ignores the timeout and calls `run()`, so a production impl that forgets the override silently un-bounds every call site. After this change `run_with_timeout` is required and `run` is the provided method, so a forgetful impl inherits a *bounded* `run` and no call site can be unbounded — there is no unbounded method to call. To keep the ten migrating call sites behaviour-identical apart from the bound, `run_bounded` also starts nulling child stdin when it has no payload, restoring what `Command::output()` always did.

**Tech Stack:** Rust 2021, `std::process`, `anyhow`, existing `MockProcessRunner` test harness, Allium specs.

**Design doc:** `docs/superpowers/specs/2026-08-11-processrunner-bounded-by-default-design.md`

## Global Constraints

- **TDD, always.** Every task writes the failing test first, runs it to see it fail, then implements. No exceptions.
- **Verify command:** `cargo fmt --check && cargo test && ./scripts/check-doc-paths.sh` must pass before the work is declared complete.
- **Clippy is `-D warnings` in the pre-push hook.** Inline test modules need `#![allow(clippy::unwrap_used, clippy::expect_used)]` at the top — the existing ones already have it.
- **No `tokio::time::sleep` under `src/` or `tests/`**, and no `std::thread::sleep` in test files. `./scripts/check-no-test-sleep.sh` enforces this. Nothing in this plan needs one.
- **Do not change** `SUBPROCESS_TIMEOUT` (60 s, `src/process.rs:9`) or `EXIT_POLL_STEP` (5 ms, `src/process.rs:101`) values. Task 5 only rewrites `EXIT_POLL_STEP`'s comment.
- **Stay in the worktree.** All paths below are relative to the worktree root.

---

## File Structure

| File | Responsibility | Tasks |
|---|---|---|
| `src/process.rs` | The trait, `run_bounded`, `RealProcessRunner`, `MockProcessRunner`, their tests | 1, 2, 3, 5 |
| `src/feed/exec.rs` | Three test-only runners (`AlwaysFailRunner`, `FixedBranchRunner`, `CountingRunner`) | 2 |
| `src/feed/mod.rs` | One test-only runner (`PerRepoBranchRunner`) | 2 |
| `tests/feed_sync.rs` | Test-only `AlwaysFailRunner` | 2 |
| `tests/managed_feeds.rs` | Test-only `AlwaysFailRunner` | 2 |
| `tests/tmux_harness/mod.rs` | `SocketRunner` — routes tmux to a private test server | 2 |
| `src/git.rs` | Four `*_bounds_its_subprocess` assertions | 3 |
| `src/dispatch/finish.rs` | Two `finish_task_bounds_*` assertions | 3 |
| `src/repo_sync.rs` | Four `*_bounds_*` assertions | 3 |
| `docs/specs/dispatch.allium`, `docs/specs/repo-sync.allium` | The bounding guarantee | 4 |
| `docs/conventions.md`, `CLAUDE.md` | Convention text and the recorded tradeoff | 5 |

No files are created. No call sites outside `src/process.rs` change behaviour by being edited — the ten production sites that gain a bound (`src/tmux.rs` ×4, `src/dispatch/mod.rs` ×2, `src/dispatch/worktree.rs` ×2, `src/runtime/settings.rs` ×2) keep calling `run()` verbatim and inherit the bound from the trait. That is the point of the design; **do not edit them.**

---

## Task 1: Null child stdin when `run_bounded` has no payload

`RealProcessRunner::run` uses `Command::output()`, which sets the child's stdin to `Stdio::null()`. `run_bounded` sets stdin only when given a payload, so with `stdin: None` the child inherits the parent's — the TUI's raw-mode terminal. Task 2 would spread that to ten more call sites, so it is fixed first. This also undoes an unintended change from #3757, which moved every git call from `run()` to `run_with_timeout` and silently switched them from null to inherited stdin.

**Files:**
- Modify: `src/process.rs:164-233` (`run_bounded` body and its doc comment)
- Test: `src/process.rs`, inline `mod tests`, in the `// --- run_bounded ---` section (after `run_bounded_ignores_a_child_that_never_reads_stdin`, around line 810)

**Interfaces:**
- Consumes: nothing from earlier tasks.
- Produces: `run_bounded(program: &str, args: &[&str], stdin: Option<&str>, timeout: Duration) -> Result<Output>` — signature unchanged; behaviour change is that `stdin: None` now yields a child with a closed stdin.

- [ ] **Step 1: Write the failing test**

Add to the `// --- run_bounded ---` section of `mod tests` in `src/process.rs`:

```rust
    /// With no payload the child must get a *closed* stdin, not the parent's.
    /// `RealProcessRunner::run` has always given children a null stdin
    /// (`Command::output()` does), and since #3868 every `run` goes through
    /// here — so inheriting would hand `git`/`gh` the TUI's raw-mode terminal
    /// and let a credential prompt eat the operator's keystrokes until the
    /// bound fires.
    ///
    /// `cat` is the probe: with a closed stdin it reads EOF and exits at once;
    /// with an inherited one it blocks until the test binary's own stdin ends,
    /// which the short bound turns into an `Err` rather than a hung suite.
    #[test]
    fn run_bounded_gives_a_child_with_no_payload_a_closed_stdin() {
        let out = run_bounded("cat", &[], None, Duration::from_millis(500))
            .expect("cat must see EOF immediately, not inherit the parent's stdin");
        assert!(
            out.status.success(),
            "cat should exit cleanly on an empty stdin, got {:?}",
            out.status
        );
        assert!(
            out.stdout.is_empty(),
            "a closed stdin yields no stdout, got {:?}",
            String::from_utf8_lossy(&out.stdout)
        );
    }
```

- [ ] **Step 2: Run the test to verify it fails**

Run: `cargo test --lib process::tests::run_bounded_gives_a_child_with_no_payload_a_closed_stdin -- --nocapture`

Expected: FAIL. `cat` inherits the test binary's stdin, does not see EOF within 500 ms, and `run_bounded` returns `Err("cat timed out after 500ms")`, so the `.expect(...)` panics.

Note: if the harness happens to run with an already-closed stdin the test may pass before the fix. That does not make the change unnecessary — the production parent is the TUI, whose stdin is a live terminal. If it passes at this step, record that in the commit message and continue; the assertion is still the right guard.

- [ ] **Step 3: Write the minimal implementation**

In `run_bounded` (`src/process.rs`), replace:

```rust
    if stdin.is_some() {
        command.stdin(std::process::Stdio::piped());
    }
```

with:

```rust
    // A child with no payload gets a *closed* stdin, never the parent's. See
    // the hazard note in this function's doc comment.
    match stdin {
        Some(_) => command.stdin(std::process::Stdio::piped()),
        None => command.stdin(std::process::Stdio::null()),
    };
```

- [ ] **Step 4: Correct the doc comment that claims the opposite**

In `run_bounded`'s doc comment (`src/process.rs`, the paragraph currently reading "With `stdin` absent the child's stdin is **inherited**, unchanged from what every git caller here has always had."), replace that paragraph with:

```rust
/// With `stdin` absent the child's stdin is **closed** (`Stdio::null()`), so a
/// child that reads it sees EOF at once. That is what `Command::output()` — and
/// therefore `ProcessRunner::run` — has always done. An earlier version of this
/// comment claimed inheritance was the status quo for git callers; it was not,
/// and #3757 changed it by accident when it routed them through here. Handing a
/// child the parent's stdin means handing it the TUI's raw-mode terminal, where
/// a `git` or `gh` credential prompt competes with the operator for keystrokes
/// until the deadline fires.
```

- [ ] **Step 5: Run the test to verify it passes**

Run: `cargo test --lib process::tests::run_bounded`

Expected: PASS — all six `run_bounded_*` tests, including the three existing stdin-payload ones (`run_bounded_writes_stdin_and_returns_stdout`, `run_bounded_does_not_deadlock_on_a_large_payload`, `run_bounded_ignores_a_child_that_never_reads_stdin`). Those three prove the payload path is untouched; if any of them fails, the `match` arm for `Some(_)` is wrong.

- [ ] **Step 6: Run the wider suite**

Run: `cargo test`

Expected: PASS. `src/cli/statusline.rs` is the only caller that passes a payload, and `tests/cli.rs` covers it.

- [ ] **Step 7: Commit**

```bash
git add src/process.rs
git commit -m "fix(3868): close child stdin when run_bounded has no payload

Command::output() has always nulled stdin, so ProcessRunner::run did too.
run_bounded inherited it instead, which #3757 spread to every git call and
the trait inversion would spread to ten more. Inheriting hands the child
the TUI's raw-mode terminal, where a credential prompt eats keystrokes
until the bound fires.

Co-Authored-By: Claude Opus 5 (1M context) <noreply@anthropic.com>"
```

---

## Task 2: Invert the trait and migrate every impl

The core change. `run_with_timeout` becomes the one required method; `run` becomes provided and delegates at `SUBPROCESS_TIMEOUT`. Deleting `RealProcessRunner::run` is load-bearing — keeping an `.output()`-based override would re-open the hole in the one impl where it matters.

**Files:**
- Modify: `src/process.rs:235-264` (trait), `src/process.rs:272-285` (`RealProcessRunner`), `src/process.rs:563-586` (`MockProcessRunner`), `src/process.rs:921-937` (one existing test)
- Modify: `src/feed/exec.rs:36-41` (`AlwaysFailRunner`), `src/feed/exec.rs:104-114` (`FixedBranchRunner`), `src/feed/exec.rs:118-123` (`CountingRunner`)
- Modify: `src/feed/mod.rs:1109-1126` (`PerRepoBranchRunner`)
- Modify: `tests/feed_sync.rs:21-25`, `tests/managed_feeds.rs:22-26` (`AlwaysFailRunner`)
- Modify: `tests/tmux_harness/mod.rs:269-288` (`SocketRunner`)
- Test: `src/process.rs`, inline `mod tests`, new section after the `// --- AgentBinaries ---` block

**Interfaces:**
- Consumes: `run_bounded(program, args, stdin, timeout)` from Task 1 (null stdin when `None`).
- Produces:
  - `ProcessRunner::run_with_timeout(&self, program: &str, args: &[&str], timeout: Duration) -> Result<Output>` — **required**, no default body.
  - `ProcessRunner::run(&self, program: &str, args: &[&str]) -> Result<Output>` — **provided**, delegates to `run_with_timeout` at `SUBPROCESS_TIMEOUT`.
  - `ProcessRunner::agent_binaries(&self) -> AgentBinaries` — unchanged, still defaulted.
  - `MockProcessRunner::recorded_timeouts(&self) -> Vec<Option<Duration>>` — type unchanged in this task; every entry becomes `Some(_)`. Task 3 narrows it.

- [ ] **Step 1: Write the failing test**

Add to `mod tests` in `src/process.rs`, immediately after the `// --- AgentBinaries ---` block ends (after `agent_binaries_stub_names_both_binaries_distinctly`):

```rust
    // --- The trait's defaulting direction ---

    /// An impl that writes only the required method. Before #3868 the defaulting
    /// ran the other way, so this shape silently un-bounded every call site with
    /// no compile error and no failing test.
    struct RequiredMethodOnlyRunner(Mutex<Vec<Duration>>);

    impl ProcessRunner for RequiredMethodOnlyRunner {
        fn run_with_timeout(&self, _: &str, _: &[&str], timeout: Duration) -> Result<Output> {
            self.0.lock().unwrap().push(timeout);
            MockProcessRunner::ok()
        }
    }

    /// The regression test for #3868. `run` must reach `run_with_timeout` with
    /// the canonical bound, so an impl cannot un-bound a call site by omission.
    #[test]
    fn run_bounds_an_impl_that_defines_only_run_with_timeout() {
        let runner = RequiredMethodOnlyRunner(Mutex::new(Vec::new()));
        runner.run("git", &["status"]).unwrap();
        assert_eq!(
            *runner.0.lock().unwrap(),
            vec![SUBPROCESS_TIMEOUT],
            "run must delegate to run_with_timeout at the canonical bound"
        );
    }
```

Only this one test. A matching test for `RealProcessRunner` is tempting but cannot be written usefully: proving its `run` is bounded means waiting out `SUBPROCESS_TIMEOUT`, and a short-bound version is just `real_run_with_timeout_kills_stuck_process_and_returns_error`, which already exists. What guards `RealProcessRunner` is Step 4's deletion of its `run` override plus the compiler.

- [ ] **Step 2: Run the test to verify it fails**

Run: `cargo test --lib process::tests::run_bounds_an_impl_that_defines_only_run_with_timeout`

Expected: FAIL — but as a **compile error**, not an assertion failure: `RequiredMethodOnlyRunner` does not implement `run`, which is still the required method today. The message is `error[E0046]: not all trait items implemented, missing: 'run'`. That compile error *is* the red state; it is the trait shape this task changes.

- [ ] **Step 3: Invert the trait**

In `src/process.rs`, replace the `run` / `run_with_timeout` pair in `pub trait ProcessRunner` with:

```rust
    /// Run `program` with `args` and kill it if it has not finished within
    /// `timeout`, returning its captured output.
    ///
    /// **The one required method.** [`Self::run`] is provided and delegates
    /// here at [`SUBPROCESS_TIMEOUT`], so an impl that writes only this one
    /// still bounds every call made through it.
    ///
    /// That direction is deliberate and is the whole point of the arrangement.
    /// Before #3868 the defaulting ran the other way — `run_with_timeout` had a
    /// default that ignored the timeout and delegated to `run` — so an impl
    /// that forgot the override silently un-bounded every call site, with no
    /// compile error and no failing test. There is now no unbounded method to
    /// inherit by accident or to reach for on purpose.
    fn run_with_timeout(&self, program: &str, args: &[&str], timeout: Duration) -> Result<Output>;

    /// Run `program` with `args`, bounded by the canonical
    /// [`SUBPROCESS_TIMEOUT`].
    ///
    /// Override only to change *how* the bound is applied, never to drop it.
    fn run(&self, program: &str, args: &[&str]) -> Result<Output> {
        self.run_with_timeout(program, args, SUBPROCESS_TIMEOUT)
    }
```

Leave `agent_binaries` and its doc comment exactly as they are.

- [ ] **Step 4: Delete the two overrides that must not survive**

In `impl ProcessRunner for RealProcessRunner`, delete the whole `fn run` and keep only:

```rust
impl ProcessRunner for RealProcessRunner {
    fn run_with_timeout(&self, program: &str, args: &[&str], timeout: Duration) -> Result<Output> {
        // No stdin payload: the child gets a closed stdin, as it always has.
        // See [`run_bounded`] for the hazards handled.
        run_bounded(program, args, None, timeout)
    }
}
```

The `use anyhow::Context` import may now be unused in that impl's vicinity — leave the top-of-file imports alone unless the compiler warns; `run_bounded` still uses `Context`.

In `impl ProcessRunner for MockProcessRunner`, delete the whole `fn run`, keeping `run_with_timeout` and `agent_binaries` unchanged:

```rust
impl ProcessRunner for MockProcessRunner {
    fn run_with_timeout(&self, program: &str, args: &[&str], timeout: Duration) -> Result<Output> {
        let (delay, response) = self.record_and_pop(program, args, Some(timeout));
        if let Some(d) = delay {
            if d >= timeout {
                anyhow::bail!("{program} timed out after {timeout:?}");
            }
            std::thread::sleep(d);
        }
        response
    }

    fn agent_binaries(&self) -> AgentBinaries {
        self.binaries.clone()
    }
}
```

- [ ] **Step 5: Migrate the three test runners in `src/feed/exec.rs`**

`AlwaysFailRunner`:

```rust
#[cfg(test)]
impl ProcessRunner for AlwaysFailRunner {
    fn run_with_timeout(
        &self,
        _: &str,
        _: &[&str],
        _: std::time::Duration,
    ) -> anyhow::Result<std::process::Output> {
        crate::process::MockProcessRunner::fail("not a git repo")
    }
}
```

`FixedBranchRunner`:

```rust
    impl ProcessRunner for FixedBranchRunner {
        fn run_with_timeout(
            &self,
            _program: &str,
            args: &[&str],
            _: std::time::Duration,
        ) -> anyhow::Result<std::process::Output> {
            let path = args.get(1).copied().unwrap_or("");
            match self.0.get(path) {
                Some(branch) => MockProcessRunner::ok_with_stdout(
                    format!("refs/remotes/origin/{branch}\n").as_bytes(),
                ),
                None => MockProcessRunner::fail("unknown repo"),
            }
        }
    }
```

`CountingRunner`:

```rust
    impl ProcessRunner for CountingRunner {
        fn run_with_timeout(
            &self,
            _: &str,
            _: &[&str],
            _: std::time::Duration,
        ) -> anyhow::Result<std::process::Output> {
            self.0.fetch_add(1, Ordering::SeqCst);
            MockProcessRunner::ok_with_stdout(b"refs/remotes/origin/main\n")
        }
    }
```

- [ ] **Step 6: Migrate `PerRepoBranchRunner` in `src/feed/mod.rs`**

Change only the signature; the body is unchanged:

```rust
    impl ProcessRunner for PerRepoBranchRunner {
        fn run_with_timeout(
            &self,
            program: &str,
            args: &[&str],
            _: std::time::Duration,
        ) -> anyhow::Result<std::process::Output> {
            assert_eq!(program, "git");
            // args = ["-C", <path>, "symbolic-ref", "refs/remotes/origin/HEAD"]
            let path = args.get(1).copied().unwrap_or("");
            *self
                .calls
                .lock()
                .unwrap()
                .entry(path.to_string())
                .or_insert(0) += 1;
            match self.branches.get(path) {
                Some(branch) => crate::process::MockProcessRunner::ok_with_stdout(
                    format!("refs/remotes/origin/{branch}\n").as_bytes(),
                ),
                None => crate::process::MockProcessRunner::fail("unknown repo"),
            }
        }
    }
```

- [ ] **Step 7: Migrate the two integration-test runners**

Identical edit in `tests/feed_sync.rs` and `tests/managed_feeds.rs` — the two `AlwaysFailRunner` impls are byte-identical:

```rust
impl ProcessRunner for AlwaysFailRunner {
    fn run_with_timeout(
        &self,
        _program: &str,
        _args: &[&str],
        _timeout: std::time::Duration,
    ) -> anyhow::Result<std::process::Output> {
        MockProcessRunner::fail("not a git repo")
    }
}
```

- [ ] **Step 8: Migrate `SocketRunner` in `tests/tmux_harness/mod.rs`**

The tmux-arg rewrite moves onto the required method and forwards the caller's timeout rather than swallowing it:

```rust
impl ProcessRunner for SocketRunner {
    /// The whole substitution seam — see the "Stub binaries" section below.
    fn agent_binaries(&self) -> AgentBinaries {
        self.binaries.clone()
    }

    fn run_with_timeout(
        &self,
        program: &str,
        args: &[&str],
        timeout: std::time::Duration,
    ) -> anyhow::Result<std::process::Output> {
        if program != "tmux" {
            return self.inner.run_with_timeout(program, args, timeout);
        }
        // `-f /dev/null` goes on *every* invocation, not on a one-off
        // `start-server`. tmux reads its config when the server starts, and the
        // server is started implicitly by whichever command happens to be first
        // — so the only way to be sure `-f` is in effect is to pass it always.
        // Verified: `-f` is silently ignored by an explicit `start-server`
        // (the user's `~/.tmux.conf` still loads), honoured on the implicit-start
        // `new-session`, and harmless on every later command.
        let mut full: Vec<&str> = vec!["-L", &self.socket, "-f", "/dev/null"];
        full.extend_from_slice(args);
        self.inner.run_with_timeout(program, &full, timeout)
    }
}
```

- [ ] **Step 9: Update the one existing mock test that asserted `None`**

`mock_records_the_timeout_each_call_was_made_with` in `src/process.rs` asserts that a plain `run` records no timeout. That is now false by construction. Replace the test's comment block and body with:

```rust
    // Whether a call was bounded is no longer a question — since #3868 the trait
    // has no unbounded method. What the mock still records is *which* bound each
    // call got, which is how the `*_bounds_every_subprocess_*` tests pin the
    // finish and sync paths.
    #[test]
    fn mock_records_the_timeout_each_call_was_made_with() {
        let mock = MockProcessRunner::new(vec![MockProcessRunner::ok(), MockProcessRunner::ok()]);
        mock.run("git", &["status"]).unwrap();
        mock.run_with_timeout("git", &["fetch"], Duration::from_secs(5))
            .unwrap();
        assert_eq!(
            mock.recorded_timeouts(),
            vec![Some(SUBPROCESS_TIMEOUT), Some(Duration::from_secs(5))],
            "a plain run records the canonical bound, a bounded one records its own"
        );
        assert_eq!(
            mock.recorded_timeouts().len(),
            mock.recorded_calls().len(),
            "timeouts must line up positionally with the calls they belong to"
        );
    }
```

- [ ] **Step 10: Run the new tests to verify they pass**

Run: `cargo test --lib process::tests`

Expected: PASS, including `run_bounds_an_impl_that_defines_only_run_with_timeout` and the pre-existing `real_run_with_timeout_*` tests.

- [ ] **Step 11: Run the full suite**

Run: `cargo test`

Expected: PASS. Two categories may fail and both are real regressions, not expected churn:
- A test asserting `recorded_timeouts()` contains `None` — only `mock_records_the_timeout_each_call_was_made_with` did, and Step 9 fixed it. Any other is a genuine surprise; read it before editing.
- A tmux test timing out — `SocketRunner` now bounds its passthrough. If one hangs, the cause is a stub binary that never exits, not the bound.

- [ ] **Step 12: Run clippy and fmt**

Run: `cargo fmt` then `cargo clippy --all-targets -- -D warnings`

Expected: clean. Watch for `clippy::needless_pass_by_value` or unused-import warnings from the deleted `run` bodies.

- [ ] **Step 13: Commit**

```bash
git add src/process.rs src/feed/exec.rs src/feed/mod.rs tests/feed_sync.rs tests/managed_feeds.rs tests/tmux_harness/mod.rs
git commit -m "fix(3868): make run_with_timeout the required ProcessRunner method

run is now provided and delegates at SUBPROCESS_TIMEOUT, so an impl that
writes only the required method still bounds every call and no call site
can be unbounded — the trait offers no unbounded method. Previously the
defaulting ran the other way and a forgetful impl silently un-bounded all
twelve call sites #3757 bounded.

Ten production sites gain a 60s bound by inheritance and are not edited:
tmux (x4), gh pr view (x2), archive cleanup (x2), notify-send/xdg-open.

Co-Authored-By: Claude Opus 5 (1M context) <noreply@anthropic.com>"
```

---

## Task 3: Narrow `recorded_timeouts` to `Vec<Duration>` and tighten the enumeration tests

With nothing unbounded, `None` cannot occur. Keeping the `Option` would leave four tests that read as guards but can never fail — the dead-weight pattern #3757's last commit cleaned up (`98e1a880`). The tests keep their **count** assertions, which is what still catches a thirteenth call appearing on a path, and swap `is_some()` for exact-duration equality.

**Files:**
- Modify: `src/process.rs` — `timeouts` field (~line 331), `recorded_timeouts` (~line 481), `record_and_pop` (~line 488), `run_with_timeout` (~line 572), two inline tests (~lines 921, 941)
- Modify: `src/git.rs:305-333` (four assertions)
- Modify: `src/dispatch/finish.rs:442-520` (two assertions)
- Modify: `src/repo_sync.rs:632-668` and `src/repo_sync.rs:1595-1655` (four assertions)

**Interfaces:**
- Consumes: the inverted trait from Task 2.
- Produces: `MockProcessRunner::recorded_timeouts(&self) -> Vec<Duration>` — every recorded call carries exactly one `Duration`, positionally aligned with `recorded_calls()`.

- [ ] **Step 1: Write the failing test**

Replace `mock_records_the_timeout_each_call_was_made_with` in `src/process.rs` (the version Step 9 of Task 2 left) with the `Vec<Duration>` form:

```rust
    // Whether a call was bounded is no longer a question — since #3868 the trait
    // has no unbounded method, so there is no `None` to record. What the mock
    // still answers is *which* bound each call got, which is how the
    // `*_bounds_every_subprocess_*` tests pin the finish and sync paths.
    #[test]
    fn mock_records_the_timeout_each_call_was_made_with() {
        let mock = MockProcessRunner::new(vec![MockProcessRunner::ok(), MockProcessRunner::ok()]);
        mock.run("git", &["status"]).unwrap();
        mock.run_with_timeout("git", &["fetch"], Duration::from_secs(5))
            .unwrap();
        assert_eq!(
            mock.recorded_timeouts(),
            vec![SUBPROCESS_TIMEOUT, Duration::from_secs(5)],
            "a plain run records the canonical bound, a bounded one records its own"
        );
        assert_eq!(
            mock.recorded_timeouts().len(),
            mock.recorded_calls().len(),
            "timeouts must line up positionally with the calls they belong to"
        );
    }
```

And `mock_timeouts_stay_aligned_across_an_out_of_band_window_lookup`, same file, change only its `assert_eq!`:

```rust
        assert_eq!(
            mock.recorded_timeouts(),
            vec![Duration::from_secs(5)],
            "the intercepted lookup records neither a call nor a timeout"
        );
```

- [ ] **Step 2: Run the tests to verify they fail**

Run: `cargo test --lib process::tests::mock_records_the_timeout_each_call_was_made_with`

Expected: FAIL to compile — `expected Vec<Option<Duration>>, found Vec<Duration>` (`error[E0308]: mismatched types`).

- [ ] **Step 3: Narrow the recording surface**

In `src/process.rs`, change the field:

```rust
    /// The timeout each recorded call was made with, positionally aligned with
    /// `calls`. Kept apart from `calls` so the `(program, args)` tuples every
    /// existing assertion destructures stay the shape they are.
    timeouts: Mutex<Vec<Duration>>,
```

The accessor:

```rust
    /// The bound each recorded call was made with, positionally aligned with
    /// [`Self::recorded_calls`].
    ///
    /// Every call carries one. Since #3868 the trait has no unbounded method, so
    /// a plain [`ProcessRunner::run`] records [`SUBPROCESS_TIMEOUT`] rather than
    /// nothing — "is this subprocess bounded?" is answered by the type, not by a
    /// test. What this still answers is *which* bound a call got, which is how
    /// the `*_bounds_every_subprocess_*` tests pin the finish and sync paths and
    /// how a thirteenth call appearing on either is caught.
    #[allow(clippy::unwrap_used)] // test helper — panics on poisoned mutex (programming error)
    pub fn recorded_timeouts(&self) -> Vec<Duration> {
        self.timeouts.lock().unwrap().clone()
    }
```

`record_and_pop`'s parameter — note the return type's `Option<Duration>` is the queued **delay**, unrelated, and stays:

```rust
    /// Record a call and pop the next queued (delay, response) pair.
    /// Panics if no response is queued — same contract as `run_with_timeout`.
    #[allow(clippy::unwrap_used)] // test helper — panics on poisoned mutex (programming error)
    fn record_and_pop(
        &self,
        program: &str,
        args: &[&str],
        timeout: Duration,
    ) -> (Option<Duration>, Result<Output>) {
```

Its body is unchanged except `self.timeouts.lock().unwrap().push(timeout);` (drop the `Some`).

And the caller in `impl ProcessRunner for MockProcessRunner`:

```rust
        let (delay, response) = self.record_and_pop(program, args, timeout);
```

- [ ] **Step 4: Run the process tests to verify they pass**

Run: `cargo test --lib process::tests`

Expected: PASS.

- [ ] **Step 5: Update the four `src/git.rs` assertions**

Four tests — `detect_default_branch_bounds_its_subprocess`, `has_origin_remote_bounds_its_subprocess`, `current_branch_bounds_its_subprocess`, `dirty_files_bounds_its_subprocess`. In each, change:

```rust
        assert_eq!(runner.recorded_timeouts(), vec![Some(SUBPROCESS_TIMEOUT)]);
```

to:

```rust
        assert_eq!(runner.recorded_timeouts(), vec![SUBPROCESS_TIMEOUT]);
```

- [ ] **Step 6: Update the two `src/dispatch/finish.rs` assertions**

First add the import to the test module (after `use crate::process::MockProcessRunner;` at `src/dispatch/finish.rs:200`):

```rust
use crate::process::SUBPROCESS_TIMEOUT;
```

In `finish_task_bounds_every_subprocess_it_runs`, replace the trailing `assert!` with:

```rust
        assert!(
            timeouts
                .iter()
                .all(|t| *t == TEST_TIMEOUT || *t == SUBPROCESS_TIMEOUT),
            "every subprocess on the finish path must carry one of this path's two \
             bounds — the injected {TEST_TIMEOUT:?} for the pull/rebase/merge calls \
             finish_task issues itself, the production {SUBPROCESS_TIMEOUT:?} for the \
             crate::git preflight reads — got: {timeouts:?}"
        );
```

Make the identical replacement in `finish_task_bounds_the_conflict_abort_path`, changing only the leading words to `"the conflict read and the abort must carry one of this path's two bounds — ..."`. Leave both `assert_eq!(timeouts.len(), 6)` / `(…, 6)` count assertions exactly as they are.

Then update the comment above `finish_task_bounds_every_subprocess_it_runs` — its last sentence currently reads "What matters here is that none of them is `None` — i.e. nothing on the path is unbounded." Replace that sentence with:

```rust
    // What matters here is that each is one of those two known bounds. Since
    // #3868 "bounded at all" is a property of the trait rather than of this
    // path, so this test's job narrowed to the count and the values.
```

- [ ] **Step 7: Update the four `src/repo_sync.rs` assertions**

`ahead_behind_bounds_the_rev_list_with_the_subprocess_timeout`:

```rust
        assert_eq!(
            mock.recorded_timeouts(),
            vec![SUBPROCESS_TIMEOUT],
            "rev-list must be bounded like every other subprocess here"
        );
```

`sync_repo_bounds_every_subprocess_that_can_block_on_the_network_or_a_lock` — the zip loop's assertion becomes an equality:

```rust
            assert_eq!(
                timeout, SUBPROCESS_TIMEOUT,
                "{program} {args:?} must carry the canonical bound"
            );
```

`sync_repo_bounds_every_subprocess_it_runs` and `sync_repo_bounds_the_conflict_abort_path` — keep both `assert_eq!(timeouts.len(), …)` count assertions and change the value check:

```rust
        assert!(
            timeouts.iter().all(|t| *t == SUBPROCESS_TIMEOUT),
            "every subprocess on the sync path must carry the canonical bound, got: {timeouts:?}"
        );
```

(and, in the conflict test, `"the conflict read and the abort must carry it too, got: {timeouts:?}"`).

- [ ] **Step 8: Run the full suite**

Run: `cargo test`

Expected: PASS.

- [ ] **Step 9: Run clippy and fmt**

Run: `cargo fmt` then `cargo clippy --all-targets -- -D warnings`

Expected: clean.

- [ ] **Step 10: Commit**

```bash
git add src/process.rs src/git.rs src/dispatch/finish.rs src/repo_sync.rs
git commit -m "refactor(3868): recorded_timeouts yields Duration, not Option

Nothing is unbounded any more, so None cannot occur and an is_some()
assertion cannot fail. The four *_bounds_every_subprocess_* tests keep
their count assertions — that is what catches a thirteenth call on a
path — and now assert exact bounds instead.

Co-Authored-By: Claude Opus 5 (1M context) <noreply@anthropic.com>"
```

---

## Task 4: Update the Allium specs

`docs/specs/repo-sync.allium` asserts that every subprocess the sync engine issues is bounded, enumerating which calls that covers. That enumeration is now redundant: the property holds for the whole system, not for one engine's call list.

Behaviour genuinely changed for two spec-visible surfaces — the PR poll (`gh pr view`) and archive cleanup now carry a deadline — so this is a spec change, not a comment tidy.

**Files:**
- Modify: `docs/specs/dispatch.allium` (add the guarantee)
- Modify: `docs/specs/repo-sync.allium:170-183` (shrink the `@guidance` paragraph)

**Interfaces:**
- Consumes: the inverted trait from Task 2.
- Produces: a named spec guarantee that Task 5's `docs/conventions.md` rewrite can reference.

- [ ] **Step 1: Invoke the `allium:tend` agent to place the guarantee**

Do **not** hand-write Allium syntax. Dispatch the `allium:tend` agent with this brief:

> In `docs/specs/dispatch.allium`, add a guarantee named `EverySubprocessIsBounded`. Choose the right construct and placement — it is a cross-cutting property of the system, not of one rule or surface; `ChainedCommandIsBounded` on the `StatusLineDecorator` surface (line 1393) is the closest existing neighbour, and there are top-level `invariant` declarations (e.g. `BranchHistoryCapped`, line 1495).
>
> The content: every subprocess the system issues carries a deadline. This is structural rather than per-call-site — the process-runner abstraction exposes no unbounded operation, so an unbounded subprocess cannot be written. A subprocess that overruns its deadline is killed and reaped and reports an error; it is never left running and never returns partial output. This covers the git and worktree work on the dispatch, finish and sync paths, every tmux command, the PR-status reads, archive cleanup, and the desktop notification and URL-open calls. `ChainedCommandIsBounded` is the one place a different, configurable deadline applies (`config.statusline_chain_timeout`) rather than the canonical one.
>
> Run `allium check` on the file before returning.

- [ ] **Step 2: Shrink the `repo-sync.allium` enumeration**

In `docs/specs/repo-sync.allium`, replace the `@guidance` paragraph beginning "Every subprocess the engine issues is bounded by a timeout — the fetch and the push because they touch the network…" and running to "…the merge can additionally hang the way the rebase path's conflict handling can." with:

```
        -- Every subprocess the engine issues is bounded, but not because this
        -- engine arranges it: see EverySubprocessIsBounded in dispatch.allium,
        -- which makes it structural. What matters here is the consequence. None
        -- of these calls may wedge a caller — the same counts back the TUI's
        -- drift poll and the dispatch path — and a subprocess that times out is
        -- simply one that answered nothing, which the surrounding logic already
        -- handles as unmeasurable.
```

- [ ] **Step 3: Verify the specs**

Run: `allium check docs/specs/dispatch.allium` and `allium check docs/specs/repo-sync.allium`

Expected: both pass.

- [ ] **Step 4: Check for drift**

Dispatch the `allium:weed` agent scoped to `docs/specs/dispatch.allium` and `docs/specs/repo-sync.allium`, asking it to confirm the new guarantee matches `src/process.rs` and that the shrunk `repo-sync` guidance still describes `src/repo_sync.rs`. Fix anything it flags as a real divergence; ignore findings about unrelated sections.

- [ ] **Step 5: Run the doc-path checker**

Run: `./scripts/check-doc-paths.sh` and `./scripts/check-doc-symbols.sh`

Expected: both pass. `check-doc-symbols.sh` rejects backticked snake_case identifiers that occur nowhere in the code — `run_with_timeout`, `run_bounded` and `EverySubprocessIsBounded` all exist, so this should be clean.

- [ ] **Step 6: Commit**

```bash
git add docs/specs/dispatch.allium docs/specs/repo-sync.allium
git commit -m "docs(3868): spec the system-wide subprocess bound

EverySubprocessIsBounded states the property once, structurally. repo-sync's
guidance drops its per-call enumeration and references it instead.

Co-Authored-By: Claude Opus 5 (1M context) <noreply@anthropic.com>"
```

---

## Task 5: Record the conventions and the accepted tradeoff

Two things need writing down. The convention text still warns about the delegating default that no longer exists and still asks the reader to judge which call sites deserve a bound — both moot. And the post-EOF exit poll now costs a 5 ms tail on every subprocess rather than only on git work; that is accepted, and an agent tempted to "tidy" the wait needs to find that on the way.

**Files:**
- Modify: `src/process.rs:97-101` (`EXIT_POLL_STEP` doc comment)
- Modify: `docs/conventions.md:368-380` (the "One bounded-child primitive" section)
- Modify: `CLAUDE.md` (the `docs/conventions.md` bullet, ~line 221)

**Interfaces:**
- Consumes: the guarantee name `EverySubprocessIsBounded` from Task 4.
- Produces: nothing code-facing.

- [ ] **Step 1: Rewrite `EXIT_POLL_STEP`'s doc comment**

In `src/process.rs`, replace the existing comment above `const EXIT_POLL_STEP` with:

```rust
/// Poll step for the bounded wait for *exit*, after the child has closed its
/// output.
///
/// Usually free: a child that closed stdout has usually exited, so the first
/// `try_wait` succeeds and this never sleeps. The exception is a real race —
/// the parent can observe stdout's EOF in the window between the child closing
/// the pipe and the kernel making it reapable — and losing it costs one sleep
/// of this length.
///
/// Since #3868 every subprocess reaches here, including the tmux calls that run
/// several times per tick per live agent, so that cost is paid more often than
/// it used to be. **Accepted deliberately**: it lands on `spawn_blocking`
/// threads, never the render path. If it ever shows in a profile the fix is a
/// sub-millisecond first backoff step before settling to this one — not a
/// redesign of the wait, whose blocking-recv shape is load-bearing (see the
/// `run_bounded` doc comment and docs/conventions.md).
const EXIT_POLL_STEP: Duration = Duration::from_millis(5);
```

- [ ] **Step 2: Rewrite the `docs/conventions.md` section**

In "One bounded-child primitive: `run_bounded`", replace the two paragraphs beginning "**Which call sites must be bounded**…" and "Note that `ProcessRunner::run_with_timeout` has a **delegating default**…" with:

```markdown
**Every call site is bounded, structurally.** `ProcessRunner` has exactly one required method — `run_with_timeout` — and `run` is a provided method that delegates to it at `SUBPROCESS_TIMEOUT`. There is no unbounded operation to inherit by accident or to reach for on purpose, so "should this call be bounded?" is not a judgement anyone has to make. Stated as a spec guarantee in `EverySubprocessIsBounded` (`docs/specs/dispatch.allium`).

Before #3868 the defaulting ran the other way: `run_with_timeout` had a default that ignored the timeout and called `run`, so an impl that forgot the override silently un-bounded every call site with no compile error and no failing test. If you are adding a `ProcessRunner` impl, write `run_with_timeout` and stop — overriding `run` is only ever to change *how* the bound is applied, never to drop it.

`MockProcessRunner::recorded_timeouts()` returns `Vec<Duration>`, not `Vec<Option<Duration>>`, for the same reason: there is no unbounded call to distinguish. It is still how `finish_task_bounds_every_subprocess_it_runs` (`src/dispatch/finish.rs`) and `sync_repo_bounds_every_subprocess_it_runs` (`src/repo_sync.rs`) pin *which* bound each call on those paths carries, and their call-count assertions are what catch a new call quietly appearing there.

Children get a **closed** stdin unless the caller passes a payload. `Command::output()` has always done that, and inheriting instead would hand a child the TUI's raw-mode terminal, where a `git` or `gh` credential prompt competes with the operator for keystrokes until the deadline fires.
```

Then append to the paragraph beginning "Two properties are load-bearing and easy to break while 'tidying' the wait":

```markdown
One cost is accepted rather than fixed. The wait for exit polls at `EXIT_POLL_STEP` (5 ms), which is normally free — a child that closed stdout has usually exited, so the first `try_wait` succeeds — but costs one 5 ms sleep when the parent observes EOF in the window before the kernel makes the child reapable. Since #3868 every subprocess pays that risk, tmux calls included, where before only git work did. It lands on `spawn_blocking` threads and never the render path, so it stays as it is; if it ever shows in a profile, add a sub-millisecond first backoff step rather than reshaping the wait. This is an instance of the wakeup-shape hazard above, recorded so it reads as expected rather than as a regression.
```

- [ ] **Step 3: Update the `CLAUDE.md` bullet**

In `CLAUDE.md`, in the `docs/conventions.md` bullet, replace:

```
the one bounded-child primitive (`run_bounded` — never hand-roll a second kill-on-timeout)
```

with:

```
the one bounded-child primitive (`run_bounded` — never hand-roll a second kill-on-timeout; `ProcessRunner` has no unbounded method, so every subprocess is bounded by construction)
```

- [ ] **Step 4: Run the doc checkers**

Run: `./scripts/check-doc-paths.sh` then `./scripts/check-doc-symbols.sh`

Expected: both pass.

- [ ] **Step 5: Record the tradeoff in the knowledge base**

Call the `record_learning` MCP tool, scope `repo`:

> `run_bounded`'s post-EOF exit poll (`EXIT_POLL_STEP`, 5 ms) costs one sleep when the parent observes stdout EOF before the kernel makes the child reapable. Since #3868 every subprocess goes through it — tmux calls included, several per tick per live agent — where before only git work did. This is expected and accepted, not a regression: it runs on `spawn_blocking` threads, never the render path. If it shows in a profile, add a sub-millisecond first backoff step; do not replace the blocking recv on the drain channel with a poll loop (see learning #360).

- [ ] **Step 6: Commit**

```bash
git add src/process.rs docs/conventions.md CLAUDE.md
git commit -m "docs(3868): record the bounding convention and the exit-poll tradeoff

The delegating-default warning and the which-sites-need-a-bound judgement
are both moot. Records the post-EOF 5ms poll as an accepted cost so a
future agent reads it as expected rather than as a regression.

Co-Authored-By: Claude Opus 5 (1M context) <noreply@anthropic.com>"
```

---

## Task 6: Full verification

**Files:** none modified unless a failure demands it.

- [ ] **Step 1: Run the task's verify command**

Run: `cargo fmt --check && cargo test && ./scripts/check-doc-paths.sh`

Expected: PASS. If anything fails, fix the underlying issue — do not skip it.

- [ ] **Step 2: Run the rest of the pre-push gate**

Run, one at a time:

```bash
cargo clippy --all-targets -- -D warnings
./scripts/check-doc-symbols.sh
./scripts/check-no-test-sleep.sh
./scripts/test-check-doc-paths.sh
./scripts/test-check-doc-symbols.sh
bash ./scripts/test-fetch-reviews.sh
```

Expected: all clean.

- [ ] **Step 3: Confirm the tmux integration targets actually ran**

Run: `cargo test --test tmux_lifecycle --test tmux_split_hook --test tmux_window_targets`

Expected: PASS, and **not** "skipping: tmux not available on PATH". `SocketRunner` changed in Task 2, so a skip here means this plan's riskiest edit went unverified. If tmux is missing, install it (`sudo dnf install tmux`) and rerun.

- [ ] **Step 4: Sanity-check the ten inherited call sites**

Confirm none of them was edited:

```bash
git diff main --stat -- src/tmux.rs src/dispatch/mod.rs src/dispatch/worktree.rs src/runtime/settings.rs
```

Expected: empty. Those sites gain their bound purely by inheriting the provided `run`. A diff there means someone changed a call site the design says not to touch.

---

## Self-Review Notes

**Spec coverage.** Every section of the design doc maps to a task: the trait inversion → Task 2; child stdin → Task 1; the nine impl migrations → Task 2 steps 4–8; the ten inherited call sites → Task 2 (no edit) plus Task 6 step 4 (verified untouched); the test surface and TDD order → Tasks 1–3; spec and documentation → Tasks 4 and 5; the accepted tradeoff and its three recording places → Task 5 steps 1, 2 and 5.

**Ordering rationale.** Task 1 precedes Task 2 so the inversion is behaviour-neutral for the migrating sites when it lands. Task 3 follows Task 2 rather than merging into it so each task ends green; the cost is that `mock_records_the_timeout_each_call_was_made_with` is touched twice, four lines each time.

**Known transitional state.** After Task 2 and before Task 3, `recorded_timeouts()` still returns `Vec<Option<Duration>>` with every entry `Some(_)`. That is intentional and the suite is green throughout.
