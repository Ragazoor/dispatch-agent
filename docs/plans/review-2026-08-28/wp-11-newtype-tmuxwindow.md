# Newtype TmuxWindow

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:executing-plans to implement this plan task-by-task.

**Goal:** Make "acted on the wrong task's tmux window" unrepresentable by promoting the window name from `String` to a `TmuxWindow` newtype — the first staged step of the larger path-newtype effort.

## Context

This work package addresses findings from the whole-repository codebase review of 2026-08-28 (`docs/plans/2026-08-28-codebase-review.md`, sections 5.3 and 6.3, where it is the #3 magic-wand change and rated highest for bug reduction).

`Task` carries five stringly-typed identifiers (`repo_path`, `worktree`, `tmux_window`, `plan_path`, `base_branch`). The domain vocabulary exists — `src/models/tmux_window.rs` and `src/models/paths.rs` — but as free functions over `&str`, not types.

**Start with `tmux_window`**, because it has the highest risk and the smallest surface. The other four follow in later work; do **not** attempt them here.

### Why this is a real hazard, not a style preference

The codebase already documents the exact bug. From `MockProcessRunner`'s own docs in `src/process.rs`:

> Every `tmux::` helper that takes a window *name* resolves it to a pane ID first, because tmux resolves a bare `-t <name>` by **prefix** and would otherwise act on a different task's window (see `tmux::window_target`). That makes the lookup a precondition of nearly every tmux call in the codebase — so how the mock answers it is a decision worth naming rather than burying.

So today the defence is three things, none of them a type: a convention ("always call `window_target` first"), a doc paragraph, and a mock policy enum (`WindowLookup`, with an `OnlyNames` variant that exists specifically to test the `task-4` / `task-42` prefix collision).

Meanwhile `Command::Task(TaskCommand::KillTmuxWindow { window: String })` (`src/tui/commands/task.rs:155`) accepts any string — a repo path, a branch name, a partial window name.

Contrast with `TaskId` / `EpicId`, which **are** newtyped, and where no corresponding class of confusion bug exists. The pattern is proven in this codebase; it just has not reached these fields.

## Findings

### 💡 Window names are bare `String` across ~12 tmux helpers (`src/tmux.rs`, `src/models/tmux_window.rs`)

**Issue:** `src/models/tmux_window.rs` has exactly two functions and no type:

```rust
pub fn build_tmux_window_name(task_id: TaskId) -> String
pub fn parse_tmux_window_task_id(window: &str) -> Option<TaskId>
```

The `tmux::` public surface takes `window: &str` in about a dozen places: `window_target` (`:154`), `new_window` (`:206`), `new_window_running` (`:220`), `send_keys` (`:243`), `has_window` (`:263`), `has_window_or_assume_present` (`:278`), `kill_window_if_present` (`:290`), `kill_window` (`:333`), `select_window` (`:340`), `set_window_dispatch_dir` (`:356`), `window_dispatch_dir` (`:464`), `rename_window` (`:537`).

Plus `Task.tmux_window: Option<String>`, `ProvisionResult.tmux_window: String` (`src/dispatch/worktree.rs:89`), `TaskCommand::KillTmuxWindow { window: String }`, and `notify::notify_tmux(…, tmux_window: &str, …)` (`src/notify.rs:89`).

**Fix:** Add to `src/models/tmux_window.rs`:

```rust
/// A tmux window name owned by a dispatch task (`task-<id>`).
///
/// Exists as a type rather than a `String` because tmux resolves a bare
/// `-t <name>` by **prefix**, so `task-4` will act on `task-42`'s window.
/// Constructing one is therefore a claim that the string is a whole, valid
/// window name — see `tmux::window_target`, which every helper calls to
/// resolve it to a pane ID before use.
#[derive(Debug, Clone, PartialEq, Eq, Hash)]
pub struct TmuxWindow(String);

impl TmuxWindow {
    /// The canonical constructor: derives the name from the task id.
    pub fn for_task(task_id: TaskId) -> Self { … }

    /// Accept a name read back from the database or from tmux itself.
    /// Returns `None` for an empty or malformed name.
    pub fn parse(s: &str) -> Option<Self> { … }

    pub fn as_str(&self) -> &str { &self.0 }

    /// The task this window belongs to, if the name encodes one.
    pub fn task_id(&self) -> Option<TaskId> { … }
}
```

Keep `build_tmux_window_name` and `parse_tmux_window_task_id` as thin wrappers during the migration, or delete them once nothing calls them — but if you delete them, note that `src/dispatch/agents.rs:25` has a doc comment citing `build_tmux_token_name` by name and `./scripts/check-doc-symbols.sh` will fire.

### Two things that make this harder than it looks

**1. `window_target` accepts two different kinds of string.** Read `src/tmux.rs:154` before designing the type:

```rust
pub(crate) fn window_target(window: &str, runner: &dyn ProcessRunner) -> Result<String> {
    if is_resolved_target(window) {
        return Ok(window.to_string());
    }
    …
}
```

It takes *either* a window name *or* an already-resolved pane target (`%3`) and passes the latter straight through. So `TmuxWindow` must not swallow that distinction. Either keep `window_target` on `&str` at the boundary, or model the two cases explicitly (a `WindowOrPane` enum) — but do not make `TmuxWindow::parse("%3")` succeed, because a pane ID is not a window name and the prefix hazard does not apply to it.

**2. The pane-oriented helpers are out of scope.** `split_window_horizontal`, `join_pane`, `kill_pane`, `set_pane_option`, `respawn_pane_running`, `break_pane_to_window` take pane IDs (`%3`), not window names. A `PaneId` newtype is a reasonable follow-up but is **not** part of this work package. Leave them as `&str`.

Also leave `MAIN_SESSION_WINDOW` / the fixed `dispatch-main` window alone unless it falls out naturally — it is not a `task-<id>` window and forcing it through `for_task` would be wrong.

### Scope discipline

The compiler will drive this end to end, which makes it tempting to keep going. Resist:

- **Do** convert `Task.tmux_window`, `ProvisionResult.tmux_window`, `TaskCommand::KillTmuxWindow`, `notify_tmux`, and the ~12 window-taking `tmux::` helpers.
- **Do not** touch `repo_path`, `worktree`, `plan_path` or `base_branch` in this work package. Each deserves its own pass, and mixing them makes the diff unreviewable.
- **Do not** change any behaviour. Every tmux command issued must be byte-identical.

At the DB boundary the column stays `TEXT`. Serialise via `as_str()` and deserialise via `parse()`; decide explicitly what a malformed stored value does. **Soft-fail is almost certainly right** — see the soft-fail-decoding section of `docs/conventions.md`, and note that a `parse()` returning `None` must not become an `unwrap()`. `Cargo.toml` sets `unwrap_used = "warn"` and the pre-push hook escalates to `-D warnings`.

## Changes

| File | Change |
|------|--------|
| `src/models/tmux_window.rs` | Add `TmuxWindow` with `for_task`, `parse`, `as_str`, `task_id`; keep or retire the two free functions |
| `src/models/tasks.rs` | `Task.tmux_window: Option<TmuxWindow>` |
| `src/db/queries/tasks.rs` | Serialise via `as_str()`, deserialise via `parse()` with a soft-fail on malformed input (`:78`, `:138`, `:164`, `:188`, `:355`) |
| `src/db/queries/mod.rs` | `row_to_task`'s `tmux_window` decode |
| `src/tmux.rs` | Convert the ~12 window-taking helpers to `&TmuxWindow`; leave `window_target`'s dual-input boundary and every pane-ID helper on `&str` |
| `src/dispatch/worktree.rs` | `ProvisionResult.tmux_window: TmuxWindow` (`:89`); `build` call site (`:335`); `teardown_task` (`:511`, `:616`) |
| `src/tui/commands/task.rs` | `KillTmuxWindow { window: TmuxWindow }` (`:155`) |
| `src/runtime/commands.rs` | The `KillTmuxWindow` handler (`:322`) |
| `src/tui/mod.rs`, `src/tui/update/{epics,retry,pr}.rs` | The `KillTmuxWindow` construction sites |
| `src/notify.rs` | `notify_tmux(…, tmux_window: &TmuxWindow, …)` (`:89`, `:148`, `:156`) |
| `src/dispatch/agents.rs` | `build_tmux_window_name` / `parse_tmux_window_task_id` call sites (`:6`, `:25`) |

## Verification

- [ ] `cargo test` — all pass. Behaviour-preserving: any test needing a semantic change means you altered what tmux is told to do
- [ ] `cargo test --no-fail-fast` — the seven `tmux_*` targets are the ones that matter here, and without this flag one blocked target hides the rest
- [ ] Confirm `tmux` is on `PATH` first: without it those targets print `skipping: tmux not available on PATH` **and pass**, so a green run would prove nothing about the change most likely to break them
- [ ] `tests/tmux_window_targets.rs` passes — it drives a real tmux server and is the direct test of the prefix-collision hazard this newtype defends
- [ ] Confirm the `task-4` / `task-42` prefix-collision test still exercises `WindowLookup::OnlyNames`. If the newtype made that test unreachable or trivially true, the type is wrong, not the test
- [ ] `cargo clippy --all-targets -- -D warnings` — clean, and **no new `unwrap()` / `expect()`** at the DB decode boundary
- [ ] `cargo fmt` before committing
- [ ] `./scripts/check-doc-symbols.sh` and its self-test pass — `build_tmux_window_name` and `parse_tmux_window_task_id` are cited from `src/dispatch/agents.rs:25` and `CLAUDE.md`; if either is retired, update the citations rather than adding `allow-phantom-symbol`
- [ ] `./scripts/check-doc-paths.sh` passes
- [ ] Run `allium:weed` over `docs/specs/dispatch.allium` and `docs/specs/split-pane.allium` — confirm no guarantee drifted
- [ ] Confirm the diff touches **only** `tmux_window`. Any `repo_path`, `worktree`, `plan_path` or `base_branch` change is out of scope and should be reverted
- [ ] Verify the win: attempting to pass a repo path or a bare `String` to `KillTmuxWindow` should now be a compile error. Try it, confirm it fails, revert
