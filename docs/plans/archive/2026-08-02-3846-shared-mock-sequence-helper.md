# 3846 — Shared mock-sequence helper for dispatch tests

## A note on the base branch

The dispatch prompt says to rebase onto `origin/main`, and at the time of this
task `origin/main` was **18 commits behind local `main`** — the gap being exactly
#3810's start-point work. Working from `origin/main` therefore made the task
description look stale (no `select_start_point`, no `rev-list`, no `b"0\t0\n"`),
when in fact it described local `main` accurately.

`wrap_up(action="rebase")` targets local `main`, so that is the base that
matters. Everything below is written against it. **Check both refs before
concluding a task description has drifted.**

The description's two costs both hold on `main`, and the second is worse than
stated: the sequence has *six* conditional steps whose conditions interact
(fetch policy by fresh-vs-reused, classification probes between attempts 1 and 2,
`rev-list` only for a non-PR base after a successful fetch), so the index of
every later step depends on choices each site re-derived by hand.

## The sequence

`dispatch_with_prompt` → `provision_worktree` → tmux launch, in order:

| # | Step | Present when |
|---|---|---|
| 1 | `git symbolic-ref` (detect default branch) | caller passes `base_branch: None` (quick dispatch) |
| 2 | `gh pr view --json headRefName,isCrossRepository` | review tag + PR url |
| 3 | `git fetch origin <base>` ×1 or ×3 | a base branch is resolved |
| 4 | `git worktree add` | worktree dir does **not** exist |
| 5 | `tmux new-window` | always |
| 6 | `tmux set-option @dispatch_dir` | always |
| 7 | `tmux set-hook` (`ensure_split_hook`) | always |
| 8 | `tmux send-keys -l` | provisioning + prompt write succeeded |
| 9 | `tmux send-keys Enter` | as 8 |
| 10 | `tmux split-window` → pane id | as 8 |

`resume_agent` issues 5–10 only. `provision_worktree` on its own stops after 7.

Window-name lookups are answered out of band by `MockProcessRunner`
(`WindowLookup::AnyName`) and are neither queued nor recorded, so response index
== recorded-call index throughout. The helper depends on that.

## Design

New `#[cfg(test)] pub(crate) mod mock_sequence;` under `src/dispatch/`, owned by
the module that produces the sequence.

```rust
/// One subprocess call a dispatch issues.
pub(crate) enum Step {
    DetectDefaultBranch, PrHeadLookup, Fetch, WorktreeAdd,
    NewWindow, SetDispatchDir, SetSplitHook,
    SendKeysLiteral, SendKeysEnter, CompanionSplit,
}

/// The *shape* of one dispatch's call sequence — config only, no responses, so
/// it stays usable after the runner is built.
pub(crate) struct DispatchScript { /* lead, fetch, worktree, ending */ }
```

Constructors name the three families:

- `DispatchScript::dispatch()` — reused worktree, fetch succeeds, full launch.
  The dominant shape (~20 sites).
- `DispatchScript::resume()` — steps 5–10.
- `DispatchScript::provision()` — `dispatch()` stopped after `SetSplitHook`.

Modifiers, each naming one axis the tests actually vary:

- `.fresh_worktree()` — insert `WorktreeAdd`
- `.no_fetch()` — caller passed no base branch
- `.detecting_default_branch(&str)` — prepend `git symbolic-ref`
- `.pr_head(PrHead::Branch("feature-x") | PrHead::Fork("patch-1") | PrHead::Unresolvable)`
- `.fetch_fails()` — three failing attempts, local-branch fallback
- `.fetch_succeeds_on_attempt(n)`
- `.fails_at(Step)` — that step returns failure, nothing queued after
- `.stops_after(Step)` — that step succeeds, nothing queued after

Accessors:

- `.runner() -> MockProcessRunner` — builds the response vector
- `.index_of(Step) -> usize` — recorded-call index (first, for the repeated
  `Fetch`); panics if the step is not in this shape, so a stale index is a loud
  test bug rather than an off-by-one assertion
- `.assert_matches(&calls)` — the recorded calls are **exactly** the declared
  steps, checked by program + distinguishing argv token

`assert_matches` is the part that pays for itself: it converts "this vector is
the right shape" from a comment into a checked claim. #3810 left a stale mock
entry that only a reviewer counting calls by hand caught; that would now fail.

Named response constants so each convention is stated once:

- `COMPANION_PANE_ID: &[u8] = b"%9\n"` — tmux's `split-window -P` reply
- `fn default_branch_ref(branch)` — `refs/remotes/origin/<branch>\n`
- `fn pr_view_reply(head, cross_repo)` — `gh pr view`'s two-line reply

## Scope

In scope — the provisioning/launch sequence in:

- `src/dispatch/tests.rs` (~27 dispatch/resume sites + 9 `provision_worktree` sites)
- `src/runtime/tests.rs` (4 quick-dispatch/dispatch sites + 1 resume)
- `src/mcp/handlers/tests/tasks/dispatch.rs` (`dispatch_runner_script`)
- `src/mcp/handlers/tests/tasks/wrap_up.rs` (1 site)

Out of scope — different sequence families that a `provision_worktree` preflight
cannot disturb: `finish_task`, `cleanup_task`, `check_pr_status`,
`pr_head_branch`, `toggle_agent_tree_pane`, `create_main_session`, and the two
`new_with_delays` timeout tests (they script per-response delays, not a shape).

No Allium spec change: this is test infrastructure, no domain behaviour moves.

## Assertion preservation

Non-negotiable. Every `calls[N]` becomes `calls[script.index_of(Step::X)]` —
strictly stronger, since the index is now derived from the same declaration the
responses come from. Exact-argv assertions are untouched. The two assertions
#3810 relied on stay verbatim:

- "no rev-list is issued for a PR head branch" → the PR-head shapes declare no
  extra git step, and `assert_matches` now rejects one appearing.
- "no tmux call happens when the dispatch aborts" → `.fails_at()` shapes queue
  nothing past the failure, so an extra tmux call panics the mock.

## Steps (TDD)

1. **Tests for the helper first** — `mod tests` in `mock_sequence.rs`:
   `index_of` agrees with a real `dispatch_agent`'s recorded calls for each
   shape; `assert_matches` accepts the true sequence and rejects an extra /
   missing / reordered call; `index_of` panics for an absent step.
2. Implement `mock_sequence.rs` to green.
3. Convert `src/dispatch/tests.rs`, adding `assert_matches` where the sequence is
   the subject. Run `cargo test dispatch::`.
4. Convert `src/runtime/tests.rs`, then the two MCP test files.
5. Document the helper in the "Where new tests go" area of `docs/conventions.md`
   (the `MockProcessRunner` vs real-tmux section) so the next preflight-adding
   agent finds it instead of hand-editing 45 vectors.
6. `cargo fmt --check && cargo test && ./scripts/check-doc-paths.sh`, plus
   `cargo clippy --all-targets -- -D warnings`.
