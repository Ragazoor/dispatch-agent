# 3845 — Tidy three provisioning nits found reviewing #3810

Three independent minor findings deferred from #3810's reviews. Unrelated to one
another; grouped only because each is a few lines.

## Context check

`#3810` is **not merged into main** — its branch
(`3810-worktrees-start-from-origin-lt-base-gt-not-local-base-branch`) is a
sibling worktree. So `classify_fetch_failure`, and the spec sentence about "no
worktree is created", do not exist on this branch. All three fixes are still
present and fixable in `main`; the #3810 references in the task description are
motivation, not code this branch can touch.

---

## 1. `has_origin_remote` polarity (`src/git.rs:33`)

**Problem.** `.unwrap_or(false)` collapses "the probe could not be run" into
"there is no origin remote". Those are different facts, and one caller (`#3810`'s
`classify_fetch_failure`) will read the collapsed value as a positive
identification it was never entitled to make.

**Constraint — `sync_repo`'s behaviour must not change.** `repo-sync.allium`'s
`PreconditionsPrecedeEveryWrite` invariant carves this out explicitly:

> One carve-out, deliberate: a remote probe that cannot be run at all and one
> that reports no origin are the same fact for this operation — there is nothing
> to sync against — so both report `no_remote` rather than splitting the second
> into `other`.

So the fix is not "change what `sync_repo` does"; it is "stop `git.rs` from
making that decision on every caller's behalf".

**Change.** `has_origin_remote` returns `Result<bool, String>`:

| result | meaning |
|---|---|
| `Ok(true)` | probe ran, `origin` is configured |
| `Ok(false)` | probe ran, exited non-zero — no `origin` |
| `Err(msg)` | probe could not be run at all |

Callers:

- `repo_sync::sync_repo` — `.unwrap_or(false)`, with a comment naming the spec
  carve-out. Behaviour identical; the collapse is now the *caller's* stated
  choice rather than a hidden one.
- `dispatch::finish::finish_task` — `.map_err(FinishError::Other)?`. A git that
  cannot be spawned is a real failure with a real message, not a reason to
  silently skip the pull and rebase anyway. `FinishError::Other` already means
  "a git command could not be run".

**Testability.** The task description says neither polarity is testable through
`MockProcessRunner` because `fail` returns `Ok(non-zero)`. That is true of the
`fail` *helper*, but the mock's response queue takes `Result<Output>` directly —
`MockProcessRunner::new(vec![Err(anyhow!("git not on PATH"))])` already appears
in `src/git.rs` and `src/repo_sync.rs` tests. No new mock capability needed here.

### Tests first

`src/git.rs`:
- `has_origin_remote_reports_a_configured_remote` → `Ok(true)`.
- `has_origin_remote_reports_a_repo_without_one` → `Ok(false)` on non-zero exit.
- `has_origin_remote_distinguishes_a_probe_that_could_not_be_run` → `Err`
  carrying the spawn message. **This is the regression test for the nit.**
- `has_origin_remote_invokes_remote_get_url_origin` — exact argv.

`src/repo_sync.rs`:
- `sync_repo_reports_no_remote_when_the_probe_cannot_be_run` — spawn `Err` still
  yields `SyncError::NoRemote` and stops before any write (pins the spec
  carve-out so a later "tidy" cannot silently undo it).

`src/dispatch/finish.rs`:
- `finish_task_reports_a_remote_probe_that_could_not_be_run` — spawn `Err` yields
  `FinishError::Other` carrying the message, and no rebase is attempted.

### Docs

- `docs/module-map.md:85` — the `src/git.rs` row describes the three preflight
  reads; update the sentence if it asserts the boolean shape.
- `docs/specs/pr-workflow.allium:223` — step 3 currently reads "If origin remote
  exists, `git pull` … (skipped when no remote is configured)". Add that a probe
  that cannot be run aborts rather than skipping.

---

## 2. `ahead_behind` has no timeout (`src/repo_sync.rs:125`)

**Problem.** It uses `runner.run`; `fetch_base` and the push both use
`run_with_timeout(…, SUBPROCESS_TIMEOUT)`. A hung `rev-list` blocks the caller
indefinitely — including the TUI's drift poll today, and `provision_worktree`'s
hot path once #3810 lands.

**Change.** `runner.run` → `runner.run_with_timeout(…, SUBPROCESS_TIMEOUT)`.
One line.

**Testability — needs a small mock addition.** `recorded_calls()` records only
`(program, args)`, so it cannot distinguish `run` from `run_with_timeout`.
Testing via `new_with_delays` alone is possible (a delay ≥ the timeout makes
`run_with_timeout` bail without sleeping) but a *regression* would hang the suite
for 60 s rather than fail — an unacceptable failure mode.

So: add `MockProcessRunner::recorded_timeouts() -> Vec<Option<Duration>>`,
positionally aligned with `recorded_calls()`, `None` for a plain `run`. Small,
and it makes every other `run_with_timeout` site assertable too.

### Tests first

`src/process.rs`:
- `mock_records_the_timeout_each_call_was_made_with` — one `run` and one
  `run_with_timeout`, asserting `[None, Some(d)]` and alignment with
  `recorded_calls()`.

`src/repo_sync.rs`:
- `ahead_behind_bounds_the_rev_list_with_the_subprocess_timeout` — asserts
  `recorded_timeouts() == [Some(SUBPROCESS_TIMEOUT)]`. Fails fast, does not hang.
- `sync_repo_bounds_every_subprocess_it_can` (optional, if cheap) — no
  network/ref-walking call is made with `None`.

### Docs

- `docs/specs/repo-sync.allium` `RepoSyncEngine` `@guidance` — note that every
  subprocess the engine issues is bounded by `SUBPROCESS_TIMEOUT`, so an
  unresponsive git cannot wedge a caller.

---

## 3. `.worktrees` created before the fetch (`src/dispatch/worktree.rs:149`)

**Problem.** `fs::create_dir_all("{repo}/.worktrees")` runs before
`resolve_start_point`, so a dispatch that aborts on an unreachable origin leaves
an empty directory behind.

**Change.** Move the `create_dir_all` call below the fetch block, immediately
above the `git worktree add` it exists to serve.

**Honest scoping.** On `main` nothing between the two positions can abort — the
fetch soft-fails to the local branch and dispatch continues — so this ordering
change has **no observable behaviour** here and is not directly testable. It is
preparatory for #3810, where the fetch becomes fatal. The change carries a
comment saying why the call sits where it does, so a future reader does not
"tidy" it back up. Existing provisioning tests continue to cover that the
directory does get created on the success path.

---

## Order of work

1. Mock capability (`recorded_timeouts`) + its test — unblocks step 3.
2. `has_origin_remote` tests → signature change → both callers → docs/spec.
3. `ahead_behind` test → one-line timeout change → spec guidance.
4. Move `create_dir_all` below the fetch.
5. `cargo fmt --check && cargo test && ./scripts/check-doc-paths.sh`, plus
   `cargo clippy --all-targets -- -D warnings` (pre-push gate).
