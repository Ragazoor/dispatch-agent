# Agent Tree: Open Selected File in Editor — Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** Pressing Space or Enter on a *file* in the agent-tree companion pane opens that file in the user's editor, in a full-width pane at the bottom of the agent's tmux window, without moving focus out of the tree.

**Approach:** The renderer's `handle_key` stays pure and gains one new action carrying the selected path; `run_loop` performs the effect through an injected `ProcessRunner`. Two new tmux verbs (a full-width split, and `respawn-pane` with a command) do the work. Because a third pane breaks `tmux::inactive_pane_id`'s "the window's single inactive pane = the tree pane" heuristic — which is already wrong the moment the user focuses the tree pane — pane identity becomes explicit: the tree pane is found by its `#{pane_start_command}`, the editor pane by a `@dispatch_editor_pane` tmux pane option.

**Tech stack:** Rust 2021, ratatui 0.29, tui-tree-widget 0.23, crossterm, tmux (3.x), anyhow, insta snapshots.

**Design doc:** `docs/superpowers/specs/2026-08-11-agent-tree-open-in-editor-design.md`
**Spec:** `docs/specs/agent-tree.allium`

## Global Constraints

- **Spec first.** `docs/specs/agent-tree.allium` is the source of truth. Task 1 updates it before any code is written (see "Working With the User" in `CLAUDE.md`).
- **TDD, always.** Every task writes the failing test first, runs it to see it fail, then implements.
- Inline test modules need `#[allow(clippy::unwrap_used, clippy::expect_used)]` at the top — the workspace `-D warnings` policy rejects bare `unwrap()`/`expect()` outside tests.
- No `unwrap()`/`expect()` in production code. Clippy is only a hard error via the pre-push hook (`cargo clippy --all-targets -- -D warnings`), so a green `cargo build` proves nothing.
- **Never sleep on the wall clock in tests.** `./scripts/check-no-test-sleep.sh` rejects `tokio::time::sleep` anywhere under `src/`/`tests/` and `std::thread::sleep` in test files. Use the harness's `DELIVERY_DEADLINE`/`POLL_STEP` polling helpers.
- Renderer snapshot tests use a **50×12** `TestBackend` (the companion pane's own size). Do **not** use the board's 120×40.
- Always `rm src/tui/tests/snapshots/*.snap.new` and `rm src/dispatch/snapshots/*.snap.new` after accepting snapshots.
- tmux pane targets are **pane ids or resolved targets, never indices** — `pane-base-index` shifts indices and a `-b` split renumbers them.
- Every tmux window *name* target goes through `tmux::window_target`; `%N` pane ids pass through untouched (`is_resolved_target`).
- Editor pane geometry: `-l 60%` (`agent_tree_editor_pane_percent`). Editor fallback: `vi` (`agent_tree_editor_fallback`). Pane option name: `@dispatch_editor_pane`, value `1`.
- Verify command for this repo: `cargo fmt --check && cargo test && ./scripts/check-doc-paths.sh`.

---

## File Structure

| File | Change | Responsibility |
|---|---|---|
| `docs/specs/agent-tree.allium` | Modify | Source of truth: new surface action, editor-pane rules, config, invariant, corrected pane-identity guidance |
| `src/tmux.rs` | Modify | Four new verbs: full-width split with a command, `respawn-pane` with a command, `set-option -p`, and two pane lookups |
| `src/agent_tree_editor.rs` | Create | Editor resolution from env values + the open/replace orchestration. Sibling of `src/agent_tree.rs` (which owns tree building) |
| `src/lib.rs` | Modify | `pub mod agent_tree_editor;` |
| `src/cli/agent_tree.rs` | Modify | `KeyAction::OpenInEditor`, the error notice in `RenderState`, bottom-border rendering, `run_loop` effect plumbing |
| `src/main.rs` | Modify | `cmd_agent_tree` installs the app-log subscriber |
| `src/dispatch/agents.rs` | Modify | `toggle_agent_tree_pane` / `resync_agent_tree_pane` use the tree-pane lookup |
| `src/dispatch/split_panes.rs` | Modify | `join_task_window_into_pane` drains *all* companion panes |
| `tests/tmux_harness/mod.rs` | Modify | `pane_option` oracle, plus `extra_stub` for a stand-in `$EDITOR` |
| `tests/tmux_editor_pane.rs` | Create | Real-tmux coverage: pane count, focus, cwd, toggle target |
| `docs/reference.md` | Modify | Companion-pane key table |

---

## Task 1: Spec the behaviour in Allium

**Files:**
- Modify: `docs/specs/agent-tree.allium`

**Interfaces:**
- Produces: the names later tasks implement against — `OpenSelectedAgentTreeFile`, `config.agent_tree_editor_pane_percent`, `config.agent_tree_editor_fallback`, `AgentTreePane.editor_pane`, `AgentTreePane.error_notice`.

- [ ] **Step 1: Read the spec end to end**

Read `docs/specs/agent-tree.allium`. The parts this task changes: the `config` block, the `AgentTreePane` entity, `HideAgentTreePane`/`ShowAgentTreePane` guidance, the `AgentTreeCompanionPane` surface (its `provides` list and `@guarantee ReadOnlyObservation`), and the invariants block.

- [ ] **Step 2: Invoke the `allium:tend` skill and make these changes**

Use the `allium:tend` skill (it owns Allium syntax and will validate with `allium check`). The changes:

1. **Config** — add next to `agent_tree_toggle_key`:

```
    -- Height of the editor pane as a percentage of the agent window. It spans
    -- the full window width (tmux split-window's -f), so the tree and the
    -- agent's own pane share the remaining 40% above it. Deliberately the
    -- larger share: reading code is the point of opening it.
    agent_tree_editor_pane_percent: Integer = 60

    -- Editor of last resort, when neither VISUAL nor EDITOR is set to a
    -- non-empty value. Not a user-facing setting — the editor is chosen by the
    -- environment; this constant only gives the fallback a name.
    agent_tree_editor_fallback: String = "vi"
```

2. **`AgentTreePane`** — add two fields:

```
    -- The editor pane opened from this tree, if any. At most one per window
    -- (see OneEditorPanePerAgentWindow); opening a second file replaces this
    -- pane's contents rather than adding another.
    editor_pane: core/TmuxPane?

    -- Set when opening a file fails, shown in the pane's own border, and
    -- cleared by the next keypress. The only thing in this pane that reports
    -- an error to the user — see AgentTreeEditorOpenFailureIsVisible.
    error_notice: String?
```

3. **Two new rules** — `OpenAgentTreeFileInEditor` (no editor pane yet: split) and `ReplaceAgentTreeEditorFile` (editor pane exists: respawn it). Both are triggered by `OpenSelectedAgentTreeFile(user, pane, node)`, require `node.kind = file` and that the file exists under `pane.root`, and ensure `pane.error_notice = null`. Their guidance must record: the pane spans the full window width and takes `config.agent_tree_editor_pane_percent` of its height; **focus stays in the tree pane** (`-d`), because the intended interaction is browsing file after file; the editor is argv, executed with no shell; and that replacing kills a running editor, which is accepted so the layout does not degrade as the user browses.

4. **A failure rule** — `AgentTreeEditorOpenFailureIsVisible`: when the open fails (the file no longer exists, the renderer cannot identify its own pane, or the tmux call fails), ensure `pane.error_notice` is set and the renderer keeps running. Guidance: deliberately the *opposite* of `FileEventWriteFailureIsSilent` — this failure is a direct response to a keypress, so a silent no-op reads as a broken key.

5. **`ClearAgentTreeErrorNotice`** — any key handled by the renderer clears `error_notice`.

6. **Surface `AgentTreeCompanionPane`** — add to `provides`:

```
        -- Space and Enter on a FILE node. On a directory the same two keys
        -- toggle expansion (see ToggleSelectedAgentTreeNode) — the key is
        -- dispatched on the selected node's kind, so the two never contend.
        OpenSelectedAgentTreeFile(user, pane, node)
```

   and narrow `ToggleSelectedAgentTreeNode`'s comment: Space/Enter on a file is no longer a no-op, it opens the file. `ExpandSelectedAgentTreeNode` (`l`/`Right`) is unchanged and *is* still a no-op on a file.

7. **`@guarantee ReadOnlyObservation`** — amend, do not delete. The pane still never writes to the worktree, never mutates a task and never issues an MCP call. It now launches an editor process, at the user's explicit keypress, which can itself write. Say exactly that.

8. **New invariant**:

```
-- At most one editor pane per agent window. A second would divide the window
-- further on every open, and nothing would own the older one.
invariant OneEditorPanePerAgentWindow {
    for a in AgentTreePanes:
        for b in AgentTreePanes:
            a != b implies (a.editor_pane = null or a.editor_pane != b.editor_pane)
}
```

9. **`HideAgentTreePane` / `ShowAgentTreePane` guidance** — replace the active/inactive derivation. It claimed the companion pane is "always the *inactive* one" because every split passes `-d`. That is a property of the split, not of the window: with focus in the tree pane the single inactive pane is the *agent's*, so the toggle killed the agent's claude session. Record that the tree pane is now identified by `#{pane_start_command}` — argv0's basename equal to the dispatch binary and argv1 equal to `agent-tree` — that "hidden" means no such pane in the window, and that a substring match would be wrong because an editor pane opened on `docs/specs/agent-tree.allium` contains that substring. Note the marker is retroactive: tmux reports `pane_start_command` for panes running right now, so windows open at upgrade time need no migration.

10. **The `ToggleVsSplitPaneInteraction` resolved-question note** under `HideAgentTreePane` concluded the state-reading "was already correct; the gap was that the other feature's mutations could leave tmux state it couldn't distinguish from 'hidden'." Correct it: the state-reading was *also* wrong. And record that `join_task_window_into_pane` now drains every dispatch-created companion pane, not just a single inactive one.

- [ ] **Step 3: Validate the spec**

Run: `allium check docs/specs/agent-tree.allium`
Expected: no errors.

- [ ] **Step 4: Commit**

```bash
git add docs/specs/agent-tree.allium
git commit -m "spec(agent-tree): Space/Enter on a file opens it in an editor"
```

---

## Task 2: tmux verbs — full-width split, respawn with a command, pane option

**Files:**
- Modify: `src/tmux.rs` (new functions after `split_window_horizontal_running`, tests in the existing `mod tests`)

**Interfaces:**
- Consumes: existing `run_checked`, `run_checked_stdout`, `window_target`, `bail!`.
- Produces:
  - `pub fn split_window_full_below_running(target: &str, size_pct: u8, cwd: &str, command: &[&str], runner: &dyn ProcessRunner) -> Result<String>` — returns the new pane id.
  - `pub fn respawn_pane_running(pane_id: &str, cwd: &str, command: &[&str], runner: &dyn ProcessRunner) -> Result<()>`
  - `pub fn set_pane_option(pane_id: &str, option: &str, value: &str, runner: &dyn ProcessRunner) -> Result<()>`

- [ ] **Step 1: Write the failing tests**

Add to `src/tmux.rs`'s `mod tests`. Follow the existing style there (`MockProcessRunner::new(vec![...])`, `recorded_calls()`), e.g. `split_window_horizontal_running_issues_correct_args` at `src/tmux.rs:1501`.

```rust
    #[test]
    fn split_window_full_below_running_issues_correct_args() {
        let runner = MockProcessRunner::new(vec![MockProcessRunner::ok_with_stdout(b"%7\n")])
            .with_queued_window_lookup();

        let pane = split_window_full_below_running(
            "%3",
            60,
            "/work/wt",
            &["vim", "/work/wt/src/lib.rs"],
            &runner,
        )
        .expect("split");

        assert_eq!(pane, "%7");
        let calls = runner.recorded_calls();
        assert_eq!(calls.len(), 1);
        assert_eq!(calls[0].0, "tmux");
        assert_eq!(
            calls[0].1,
            vec![
                "split-window",
                "-v",
                "-f",
                "-d",
                "-l",
                "60%",
                "-t",
                "%3",
                "-c",
                "/work/wt",
                "-P",
                "-F",
                "#{pane_id}",
                "--",
                "vim",
                "/work/wt/src/lib.rs",
            ]
        );
    }

    /// `-f` (full window width) is what makes the geometry independent of which
    /// pane the split targets, and `-d` is what keeps focus in the tree.
    /// Asserted by name because both are single-character flags that are easy
    /// to drop in a refactor and invisible in the result.
    #[test]
    fn split_window_full_below_running_spans_the_window_and_keeps_focus() {
        let runner = MockProcessRunner::new(vec![MockProcessRunner::ok_with_stdout(b"%7\n")])
            .with_queued_window_lookup();
        split_window_full_below_running("%3", 60, "/work/wt", &["vi", "a"], &runner)
            .expect("split");
        let args = &runner.recorded_calls()[0].1;
        assert!(args.contains(&"-f".to_string()), "args: {args:?}");
        assert!(args.contains(&"-d".to_string()), "args: {args:?}");
    }

    #[test]
    fn split_window_full_below_running_rejects_empty_command() {
        let runner = MockProcessRunner::new(vec![]).with_queued_window_lookup();
        assert!(split_window_full_below_running("%3", 60, "/w", &[], &runner).is_err());
    }

    #[test]
    fn split_window_full_below_running_fails_on_nonzero_exit() {
        let runner = MockProcessRunner::new(vec![MockProcessRunner::fail("no space")])
            .with_queued_window_lookup();
        assert!(
            split_window_full_below_running("%3", 60, "/w", &["vi", "a"], &runner).is_err()
        );
    }

    /// A window *name* target must be resolved rather than handed to tmux, which
    /// prefix-matches names (see `window_target`).
    #[test]
    fn split_window_full_below_running_resolves_a_window_name() {
        let runner = MockProcessRunner::new(vec![MockProcessRunner::ok_with_stdout(b"%7\n")])
            .with_windows(&["task-42"]);
        split_window_full_below_running("task-42", 60, "/w", &["vi", "a"], &runner)
            .expect("split");
        let args = &runner.recorded_calls()[0].1;
        let target = args.iter().position(|a| a == "-t").expect("-t") + 1;
        assert_eq!(args[target], runner.pane_id_of("task-42"));
    }

    #[test]
    fn respawn_pane_running_issues_correct_args() {
        let runner = MockProcessRunner::new(vec![MockProcessRunner::ok()])
            .with_queued_window_lookup();

        respawn_pane_running("%7", "/work/wt", &["vim", "-p", "/work/wt/a.rs"], &runner)
            .expect("respawn");

        let calls = runner.recorded_calls();
        assert_eq!(calls.len(), 1);
        assert_eq!(
            calls[0].1,
            vec![
                "respawn-pane",
                "-k",
                "-c",
                "/work/wt",
                "-t",
                "%7",
                "--",
                "vim",
                "-p",
                "/work/wt/a.rs",
            ]
        );
    }

    #[test]
    fn respawn_pane_running_rejects_empty_command() {
        let runner = MockProcessRunner::new(vec![]).with_queued_window_lookup();
        assert!(respawn_pane_running("%7", "/w", &[], &runner).is_err());
    }

    #[test]
    fn respawn_pane_running_fails_on_nonzero_exit() {
        let runner = MockProcessRunner::new(vec![MockProcessRunner::fail("boom")])
            .with_queued_window_lookup();
        assert!(respawn_pane_running("%7", "/w", &["vi", "a"], &runner).is_err());
    }

    #[test]
    fn set_pane_option_issues_correct_args() {
        let runner = MockProcessRunner::new(vec![MockProcessRunner::ok()])
            .with_queued_window_lookup();

        set_pane_option("%7", "@dispatch_editor_pane", "1", &runner).expect("set-option");

        assert_eq!(
            runner.recorded_calls()[0].1,
            vec!["set-option", "-p", "-t", "%7", "@dispatch_editor_pane", "1"]
        );
    }

    #[test]
    fn set_pane_option_fails_on_nonzero_exit() {
        let runner = MockProcessRunner::new(vec![MockProcessRunner::fail("bad option")])
            .with_queued_window_lookup();
        assert!(set_pane_option("%7", "@x", "1", &runner).is_err());
    }
```

`with_queued_window_lookup()` is deliberate throughout: a `%N` target short-circuits in `window_target` (`is_resolved_target`), so no lookup is issued at all, and the queued policy makes an unexpected one panic loudly instead of being answered out of band. `with_windows(&[...])` is for the one test that passes a window *name*.

- [ ] **Step 2: Run the tests to verify they fail**

Run: `cargo test tmux::tests::split_window_full_below 2>&1 | tail -20`
Expected: FAIL — `cannot find function split_window_full_below_running in this scope`.

- [ ] **Step 3: Implement the three functions**

Insert after `split_window_horizontal_running` in `src/tmux.rs`:

```rust
/// Create a pane spanning the **full window width** below `target`, taking
/// `size_pct`% of the window's height, running `command` as separate argv
/// elements (no shell) with `cwd` as its start directory. Keeps focus where it
/// is. Returns the new pane's ID.
///
/// The third split helper in this module, and the differences are all
/// load-bearing. [`split_window_horizontal`] (40%, right, no command) serves the
/// board's split-pane feature; [`split_window_horizontal_running`] (left,
/// `size_pct`, command) opens the agent-tree companion pane. This one is for the
/// editor pane opened *from* that companion pane, where:
///
/// * `-f` makes the new pane span the window rather than subdividing the tree
///   pane's own column, so the geometry does not depend on which pane the
///   split is targeted at — the tree pane is the natural target, and it is the
///   narrow one.
/// * `-c` is passed explicitly rather than relying on `ensure_split_hook`'s
///   `@dispatch_dir` `cd`: that hook types `cd <dir>` into the new pane, which
///   works for a shell and would be typed straight into the editor here.
/// * Focus stays put (`-d`) so the user can keep browsing the tree.
pub fn split_window_full_below_running(
    target: &str,
    size_pct: u8,
    cwd: &str,
    command: &[&str],
    runner: &dyn ProcessRunner,
) -> Result<String> {
    if command.is_empty() {
        bail!("split_window_full_below_running: command must not be empty");
    }
    let target_pane = window_target(target, runner)?;
    let size_arg = format!("{size_pct}%");
    let mut args: Vec<&str> = vec![
        "split-window",
        "-v",
        "-f",
        "-d",
        "-l",
        &size_arg,
        "-t",
        &target_pane,
        "-c",
        cwd,
        "-P",
        "-F",
        "#{pane_id}",
        "--",
    ];
    args.extend(command.iter().copied());
    run_checked_stdout(runner, &args, "split-window")
}

/// Replace what is running in `pane_id` with `command` (argv, no shell),
/// started in `cwd`. `-k` kills the pane's current process first.
///
/// The pane object itself survives, which is why this is how the editor pane
/// shows a second file: the pane keeps its geometry and its
/// `@dispatch_editor_pane` option, so nothing has to be re-marked, and focus is
/// untouched. Sibling of [`respawn_pane`], which respawns a plain shell.
pub fn respawn_pane_running(
    pane_id: &str,
    cwd: &str,
    command: &[&str],
    runner: &dyn ProcessRunner,
) -> Result<()> {
    if command.is_empty() {
        bail!("respawn_pane_running: command must not be empty");
    }
    let mut args: Vec<&str> = vec!["respawn-pane", "-k", "-c", cwd, "-t", pane_id, "--"];
    args.extend(command.iter().copied());
    run_checked(runner, &args, "respawn-pane")?;
    Ok(())
}

/// Set a pane-scoped tmux user option (`@name`). The pane-level sibling of
/// [`set_window_dispatch_dir`]'s `set-option -w`.
///
/// Takes a pane **id** only: pane options are how dispatch marks a pane it
/// created, and a marker written to the wrong pane is worse than no marker.
pub fn set_pane_option(
    pane_id: &str,
    option: &str,
    value: &str,
    runner: &dyn ProcessRunner,
) -> Result<()> {
    run_checked(
        runner,
        &["set-option", "-p", "-t", pane_id, option, value],
        "set-option",
    )?;
    Ok(())
}
```

- [ ] **Step 4: Run the tests to verify they pass**

Run: `cargo test tmux::tests 2>&1 | tail -5`
Expected: PASS, no regressions in the existing tmux tests.

- [ ] **Step 5: Commit**

```bash
git add src/tmux.rs
git commit -m "feat(tmux): full-width split, respawn-with-command, pane options"
```

---

## Task 3: tmux verbs — pane lookups by marker

**Files:**
- Modify: `src/tmux.rs` (new functions after `inactive_pane_id`, tests in the existing `mod tests`)

**Interfaces:**
- Produces:
  - `pub fn pane_ids_with_option(target: &str, option: &str, runner: &dyn ProcessRunner) -> Result<Vec<String>>` — every pane in `target`'s window whose `option` is a non-empty value.
  - `pub fn pane_ids_with_start_command<F>(target: &str, matches: F, runner: &dyn ProcessRunner) -> Result<Vec<String>>` where `F: Fn(&str) -> bool` — every pane in `target`'s window whose `#{pane_start_command}` satisfies `matches`.

`target` may be a window name or a `%N` pane id; a pane id resolves to *its own window's* panes, which is what lets the renderer look up siblings knowing only `$TMUX_PANE`.

- [ ] **Step 1: Write the failing tests**

```rust
    #[test]
    fn pane_ids_with_option_returns_only_marked_panes() {
        // `list-panes` rows: "<pane_id> <option value>", unset renders empty.
        let runner = MockProcessRunner::new(vec![MockProcessRunner::ok_with_stdout(
            b"%1 \n%2 1\n%3 \n",
        )])
        .with_queued_window_lookup();

        let found = pane_ids_with_option("%1", "@dispatch_editor_pane", &runner).expect("lookup");

        assert_eq!(found, vec!["%2".to_string()]);
        assert_eq!(
            runner.recorded_calls()[0].1,
            vec![
                "list-panes",
                "-t",
                "%1",
                "-F",
                "#{pane_id} #{@dispatch_editor_pane}",
            ]
        );
    }

    #[test]
    fn pane_ids_with_option_is_empty_when_nothing_is_marked() {
        let runner = MockProcessRunner::new(vec![MockProcessRunner::ok_with_stdout(b"%1 \n%2 \n")])
            .with_queued_window_lookup();
        assert!(pane_ids_with_option("%1", "@dispatch_editor_pane", &runner)
            .expect("lookup")
            .is_empty());
    }

    #[test]
    fn pane_ids_with_option_fails_on_nonzero_exit() {
        let runner = MockProcessRunner::new(vec![MockProcessRunner::fail("no window")])
            .with_queued_window_lookup();
        assert!(pane_ids_with_option("%1", "@x", &runner).is_err());
    }

    #[test]
    fn pane_ids_with_start_command_matches_on_the_command() {
        let runner = MockProcessRunner::new(vec![MockProcessRunner::ok_with_stdout(
            b"%1 dispatch agent-tree 42\n%2 \n%3 vim /w/a.rs\n",
        )])
        .with_queued_window_lookup();

        let found = pane_ids_with_start_command("%2", |cmd| cmd.starts_with("dispatch "), &runner)
            .expect("lookup");

        assert_eq!(found, vec!["%1".to_string()]);
        assert_eq!(
            runner.recorded_calls()[0].1,
            vec!["list-panes", "-t", "%2", "-F", "#{pane_id} #{pane_start_command}"]
        );
    }

    /// A start command can contain spaces, so only the *first* field is the pane
    /// id — the rest is handed to the predicate whole.
    #[test]
    fn pane_ids_with_start_command_passes_the_whole_command_to_the_predicate() {
        let runner = MockProcessRunner::new(vec![MockProcessRunner::ok_with_stdout(
            b"%4 vim /w/some file.rs\n",
        )])
        .with_queued_window_lookup();

        let found = pane_ids_with_start_command("%4", |cmd| cmd == "vim /w/some file.rs", &runner)
            .expect("lookup");

        assert_eq!(found, vec!["%4".to_string()]);
    }

    /// A pane running a plain shell reports an empty start command. It must
    /// reach the predicate as an empty string rather than being dropped, so a
    /// predicate can deliberately match it.
    #[test]
    fn pane_ids_with_start_command_yields_panes_with_no_start_command() {
        let runner = MockProcessRunner::new(vec![MockProcessRunner::ok_with_stdout(b"%9 \n")])
            .with_queued_window_lookup();

        let found = pane_ids_with_start_command("%9", str::is_empty, &runner).expect("lookup");

        assert_eq!(found, vec!["%9".to_string()]);
    }

    #[test]
    fn pane_ids_with_start_command_fails_on_nonzero_exit() {
        let runner = MockProcessRunner::new(vec![MockProcessRunner::fail("no window")])
            .with_queued_window_lookup();
        assert!(pane_ids_with_start_command("%1", |_| true, &runner).is_err());
    }
```

- [ ] **Step 2: Run the tests to verify they fail**

Run: `cargo test tmux::tests::pane_ids_with 2>&1 | tail -20`
Expected: FAIL — `cannot find function pane_ids_with_option in this scope`.

- [ ] **Step 3: Implement both lookups**

Insert after `inactive_pane_id` in `src/tmux.rs`:

```rust
/// Split one `list-panes` row of the form `<pane_id> <rest...>` into its two
/// halves. `rest` is empty when the field it carries is unset — tmux prints the
/// separator either way — and may itself contain spaces, so only the first
/// field is consumed.
fn split_pane_row(line: &str) -> Option<(&str, &str)> {
    match line.split_once(' ') {
        Some((id, rest)) => Some((id, rest)),
        // Defensive: a row with no separator at all is not a shape tmux
        // produces for these formats, but treating it as "id, no value" is
        // strictly better than dropping the pane.
        None if !line.is_empty() => Some((line, "")),
        None => None,
    }
}

/// Pane ids in `target`'s window whose pane-scoped user option `option` is set
/// to a non-empty value.
///
/// This is how dispatch finds a pane it created: the marker is written at
/// creation ([`set_pane_option`]) and survives [`respawn_pane_running`], so it
/// identifies the pane by *what it is* rather than by whether it happens to be
/// the focused one. The active/inactive heuristic it replaces held only for a
/// two-pane window whose focus had not moved.
pub fn pane_ids_with_option(
    target: &str,
    option: &str,
    runner: &dyn ProcessRunner,
) -> Result<Vec<String>> {
    let resolved = window_target(target, runner)?;
    let format = format!("#{{pane_id}} #{{{option}}}");
    let out = run_checked_stdout(
        runner,
        &["list-panes", "-t", &resolved, "-F", &format],
        "list-panes",
    )?;
    Ok(out
        .lines()
        .filter_map(split_pane_row)
        .filter(|(_, value)| !value.is_empty())
        .map(|(id, _)| id.to_string())
        .collect())
}

/// Pane ids in `target`'s window whose `#{pane_start_command}` satisfies
/// `matches`. The predicate receives the whole command line, which may contain
/// spaces and is empty for a pane running a plain shell.
///
/// Used where no marker can be written after the fact: the agent-tree companion
/// pane is identified this way so that panes already running at upgrade time are
/// covered without a migration — tmux has always reported the command a pane was
/// started with.
pub fn pane_ids_with_start_command<F>(
    target: &str,
    matches: F,
    runner: &dyn ProcessRunner,
) -> Result<Vec<String>>
where
    F: Fn(&str) -> bool,
{
    let resolved = window_target(target, runner)?;
    let out = run_checked_stdout(
        runner,
        &[
            "list-panes",
            "-t",
            &resolved,
            "-F",
            "#{pane_id} #{pane_start_command}",
        ],
        "list-panes",
    )?;
    Ok(out
        .lines()
        .filter_map(split_pane_row)
        .filter(|(_, cmd)| matches(cmd))
        .map(|(id, _)| id.to_string())
        .collect())
}
```

- [ ] **Step 4: Run the tests to verify they pass**

Run: `cargo test tmux::tests 2>&1 | tail -5`
Expected: PASS.

- [ ] **Step 5: Commit**

```bash
git add src/tmux.rs
git commit -m "feat(tmux): look panes up by pane option and start command"
```

---

## Task 4: Editor resolution

**Files:**
- Create: `src/agent_tree_editor.rs`
- Modify: `src/lib.rs` (add `pub mod agent_tree_editor;` in alphabetical position among the existing `pub mod` lines)

**Interfaces:**
- Produces: `pub fn resolve_editor(visual: Option<&str>, editor: Option<&str>) -> Vec<String>` — never empty; `pub fn editor_from_env() -> Vec<String>`; `pub const EDITOR_FALLBACK: &str = "vi";`

- [ ] **Step 1: Write the failing tests**

Create `src/agent_tree_editor.rs` containing only the test module and the `use super::*;` — the functions do not exist yet, so this is the failing state.

```rust
#[cfg(test)]
#[allow(clippy::unwrap_used, clippy::expect_used)]
mod tests {
    use super::*;

    #[test]
    fn visual_wins_over_editor() {
        assert_eq!(resolve_editor(Some("nvim"), Some("nano")), vec!["nvim"]);
    }

    #[test]
    fn editor_is_used_when_visual_is_unset() {
        assert_eq!(resolve_editor(None, Some("nano")), vec!["nano"]);
    }

    #[test]
    fn falls_back_to_vi_when_neither_is_set() {
        assert_eq!(resolve_editor(None, None), vec![EDITOR_FALLBACK]);
    }

    /// An exported-but-empty variable is how a shell spells "unset" in
    /// practice (`export EDITOR=`), and an empty argv would be unrunnable.
    #[test]
    fn an_empty_value_counts_as_unset() {
        assert_eq!(resolve_editor(Some(""), Some("nano")), vec!["nano"]);
        assert_eq!(resolve_editor(Some(""), Some("")), vec![EDITOR_FALLBACK]);
        assert_eq!(resolve_editor(Some("   "), None), vec![EDITOR_FALLBACK]);
    }

    /// The value is argv, not a shell command: it is split on whitespace and
    /// executed directly, so flags in $EDITOR work and nothing is
    /// shell-interpreted.
    #[test]
    fn a_multi_word_value_splits_into_argv() {
        assert_eq!(resolve_editor(Some("nvim -p"), None), vec!["nvim", "-p"]);
        assert_eq!(
            resolve_editor(Some("  code   -w  "), None),
            vec!["code", "-w"]
        );
    }
}
```

- [ ] **Step 2: Run the test to verify it fails**

Run: `cargo test agent_tree_editor 2>&1 | tail -20`
Expected: FAIL — `cannot find function resolve_editor` (after adding the module to `src/lib.rs`, which Step 3 does; if the module is not yet declared the test simply does not run, which is also a fail).

- [ ] **Step 3: Implement**

Add to the top of `src/agent_tree_editor.rs`:

```rust
//! Opening the agent-tree companion pane's selected file in the user's editor
//! (see `docs/specs/agent-tree.allium`'s `OpenSelectedAgentTreeFile` surface
//! action and the `OpenAgentTreeFileInEditor` / `ReplaceAgentTreeEditorFile`
//! rules).
//!
//! Sibling of `src/agent_tree.rs`, which owns tree *building*. This module owns
//! the effect: which editor to run, and the tmux pane it runs in.

/// Editor of last resort when neither `$VISUAL` nor `$EDITOR` names one —
/// `config.agent_tree_editor_fallback` in docs/specs/agent-tree.allium.
pub const EDITOR_FALLBACK: &str = "vi";

/// Resolve the editor argv from environment *values*: `$VISUAL`, then
/// `$EDITOR`, then [`EDITOR_FALLBACK`]. Never returns an empty vector.
///
/// Takes the values as parameters rather than reading the process environment,
/// so the resolution order is testable without `std::env::set_var` — which is
/// `unsafe` in edition 2024 and racy across the test harness's threads either
/// way. [`editor_from_env`] is the one-line adapter that reads them.
///
/// A value is treated as unset when it is empty or all whitespace: `export
/// EDITOR=` is how a shell spells "no editor", and it would otherwise produce
/// an unrunnable empty argv. The value is split on whitespace into argv and
/// executed directly, never through a shell, so `EDITOR="nvim -p"` works and
/// nothing in it is expanded, globbed or word-split by anything but this
/// function.
pub fn resolve_editor(visual: Option<&str>, editor: Option<&str>) -> Vec<String> {
    for candidate in [visual, editor] {
        let Some(value) = candidate else { continue };
        let argv: Vec<String> = value.split_whitespace().map(str::to_string).collect();
        if !argv.is_empty() {
            return argv;
        }
    }
    vec![EDITOR_FALLBACK.to_string()]
}

/// [`resolve_editor`] against the real process environment.
pub fn editor_from_env() -> Vec<String> {
    let visual = std::env::var("VISUAL").ok();
    let editor = std::env::var("EDITOR").ok();
    resolve_editor(visual.as_deref(), editor.as_deref())
}
```

And in `src/lib.rs`, next to the existing `pub mod agent_tree;`:

```rust
pub mod agent_tree_editor;
```

- [ ] **Step 4: Run the test to verify it passes**

Run: `cargo test agent_tree_editor 2>&1 | tail -5`
Expected: PASS (6 tests).

- [ ] **Step 5: Commit**

```bash
git add src/agent_tree_editor.rs src/lib.rs
git commit -m "feat(agent-tree): resolve the editor from \$VISUAL/\$EDITOR/vi"
```

---

## Task 5: Open or replace the editor pane

**Files:**
- Modify: `src/agent_tree_editor.rs`

**Interfaces:**
- Consumes: `tmux::{split_window_full_below_running, respawn_pane_running, set_pane_option, pane_ids_with_option}` (Tasks 2–3), `resolve_editor` (Task 4).
- Produces:
  - `pub const EDITOR_PANE_OPTION: &str = "@dispatch_editor_pane";`
  - `pub const EDITOR_PANE_PERCENT: u8 = 60;`
  - `pub fn open_in_editor(root: &Path, relative: &Path, my_pane: &str, editor: &[String], runner: &dyn ProcessRunner) -> Result<()>`
  - `pub fn current_pane_from_env() -> Result<String>` — reads `$TMUX_PANE`.

- [ ] **Step 1: Write the failing tests**

Add to `src/agent_tree_editor.rs`'s `mod tests`:

```rust
    use crate::process::MockProcessRunner;
    use std::path::Path;

    /// A real worktree with one real file in it: `open_in_editor` checks the
    /// file exists before splitting, so a `tempfile` root is not optional.
    struct Fixture {
        dir: tempfile::TempDir,
    }

    impl Fixture {
        fn new() -> Self {
            let dir = tempfile::tempdir().expect("tempdir");
            std::fs::create_dir_all(dir.path().join("src")).expect("mkdir");
            std::fs::write(dir.path().join("src/lib.rs"), "fn main() {}").expect("write");
            Self { dir }
        }

        fn root(&self) -> &Path {
            self.dir.path()
        }

        fn abs(&self, relative: &str) -> String {
            self.root().join(relative).to_string_lossy().into_owned()
        }
    }

    fn editor() -> Vec<String> {
        vec!["vim".to_string()]
    }

    #[test]
    fn first_open_splits_a_pane_and_marks_it() {
        let fx = Fixture::new();
        let runner = MockProcessRunner::new(vec![
            // pane_ids_with_option: no editor pane yet
            MockProcessRunner::ok_with_stdout(b"%1 \n%2 \n"),
            // split-window
            MockProcessRunner::ok_with_stdout(b"%7\n"),
            // set-option
            MockProcessRunner::ok(),
        ])
        .with_queued_window_lookup();

        open_in_editor(
            fx.root(),
            Path::new("src/lib.rs"),
            "%1",
            &editor(),
            &runner,
        )
        .expect("open");

        let calls = runner.recorded_calls();
        assert_eq!(calls.len(), 3, "calls: {calls:?}");
        assert_eq!(calls[1].1[0], "split-window");
        assert_eq!(
            calls[1].1.last().expect("file arg"),
            &fx.abs("src/lib.rs"),
            "the editor must be given the absolute path"
        );
        assert_eq!(
            calls[2].1,
            vec!["set-option", "-p", "-t", "%7", EDITOR_PANE_OPTION, "1"]
        );
    }

    #[test]
    fn the_editor_pane_starts_in_the_worktree() {
        let fx = Fixture::new();
        let runner = MockProcessRunner::new(vec![
            MockProcessRunner::ok_with_stdout(b"%1 \n"),
            MockProcessRunner::ok_with_stdout(b"%7\n"),
            MockProcessRunner::ok(),
        ])
        .with_queued_window_lookup();

        open_in_editor(fx.root(), Path::new("src/lib.rs"), "%1", &editor(), &runner)
            .expect("open");

        let args = &runner.recorded_calls()[1].1;
        let cwd = args.iter().position(|a| a == "-c").expect("-c") + 1;
        assert_eq!(args[cwd], fx.root().to_string_lossy());
    }

    #[test]
    fn a_second_open_respawns_the_existing_pane_instead_of_splitting() {
        let fx = Fixture::new();
        let runner = MockProcessRunner::new(vec![
            // pane_ids_with_option: %7 is already the editor pane
            MockProcessRunner::ok_with_stdout(b"%1 \n%7 1\n"),
            // respawn-pane
            MockProcessRunner::ok(),
        ])
        .with_queued_window_lookup();

        open_in_editor(fx.root(), Path::new("src/lib.rs"), "%1", &editor(), &runner)
            .expect("open");

        let calls = runner.recorded_calls();
        assert_eq!(calls.len(), 2, "calls: {calls:?}");
        assert_eq!(calls[1].1[0], "respawn-pane");
        assert!(
            calls[1].1.contains(&"%7".to_string()),
            "must target the marked pane; calls: {calls:?}"
        );
        assert!(
            !calls.iter().any(|(_, args)| args[0] == "split-window"),
            "a second open must not add a pane; calls: {calls:?}"
        );
    }

    /// Multi-word editors reach tmux as separate argv elements — a single
    /// "nvim -p /path" string would be looked up as a binary of that name.
    #[test]
    fn a_multi_word_editor_stays_separate_argv_elements() {
        let fx = Fixture::new();
        let runner = MockProcessRunner::new(vec![
            MockProcessRunner::ok_with_stdout(b"%1 \n"),
            MockProcessRunner::ok_with_stdout(b"%7\n"),
            MockProcessRunner::ok(),
        ])
        .with_queued_window_lookup();

        open_in_editor(
            fx.root(),
            Path::new("src/lib.rs"),
            "%1",
            &["nvim".to_string(), "-p".to_string()],
            &runner,
        )
        .expect("open");

        let args = &runner.recorded_calls()[1].1;
        let sep = args.iter().position(|a| a == "--").expect("--");
        assert_eq!(args[sep + 1], "nvim");
        assert_eq!(args[sep + 2], "-p");
        assert_eq!(args[sep + 3], fx.abs("src/lib.rs"));
    }

    #[test]
    fn a_missing_file_is_an_error_and_runs_no_tmux_command() {
        let fx = Fixture::new();
        let runner = MockProcessRunner::new(vec![]).with_queued_window_lookup();

        let err = open_in_editor(fx.root(), Path::new("src/gone.rs"), "%1", &editor(), &runner)
            .expect_err("must fail");

        assert!(
            err.to_string().contains("src/gone.rs"),
            "the message must name the file: {err}"
        );
        assert!(runner.recorded_calls().is_empty());
    }

    /// A selection path must not be able to address anything outside the
    /// worktree. Tree nodes are built from path segments so this is not
    /// reachable today, but the check is one line and the alternative is
    /// trusting that forever.
    #[test]
    fn a_path_escaping_the_worktree_is_rejected() {
        let fx = Fixture::new();
        let runner = MockProcessRunner::new(vec![]).with_queued_window_lookup();

        assert!(open_in_editor(
            fx.root(),
            Path::new("../outside.rs"),
            "%1",
            &editor(),
            &runner
        )
        .is_err());
        assert!(runner.recorded_calls().is_empty());
    }

    #[test]
    fn a_failing_split_is_an_error() {
        let fx = Fixture::new();
        let runner = MockProcessRunner::new(vec![
            MockProcessRunner::ok_with_stdout(b"%1 \n"),
            MockProcessRunner::fail("no space for a new pane"),
        ])
        .with_queued_window_lookup();

        assert!(
            open_in_editor(fx.root(), Path::new("src/lib.rs"), "%1", &editor(), &runner)
                .is_err()
        );
    }

    /// The marker is a convenience for the *next* open, not the point of this
    /// one: the pane is already open and showing the file, so failing the
    /// operation would misreport what happened.
    #[test]
    fn a_failing_marker_write_does_not_fail_the_open() {
        let fx = Fixture::new();
        let runner = MockProcessRunner::new(vec![
            MockProcessRunner::ok_with_stdout(b"%1 \n"),
            MockProcessRunner::ok_with_stdout(b"%7\n"),
            MockProcessRunner::fail("bad option"),
        ])
        .with_queued_window_lookup();

        assert!(
            open_in_editor(fx.root(), Path::new("src/lib.rs"), "%1", &editor(), &runner).is_ok()
        );
    }

    /// A failed lookup must not be read as "no editor pane" — that would split a
    /// second pane on every press.
    #[test]
    fn a_failing_pane_lookup_is_an_error() {
        let fx = Fixture::new();
        let runner = MockProcessRunner::new(vec![MockProcessRunner::fail("no window")])
            .with_queued_window_lookup();

        assert!(
            open_in_editor(fx.root(), Path::new("src/lib.rs"), "%1", &editor(), &runner)
                .is_err()
        );
    }
```

- [ ] **Step 2: Run the tests to verify they fail**

Run: `cargo test agent_tree_editor 2>&1 | tail -20`
Expected: FAIL — `cannot find function open_in_editor in this scope`.

- [ ] **Step 3: Implement**

Add to `src/agent_tree_editor.rs` (and the imports it needs at the top of the file):

```rust
use std::path::{Component, Path};

use anyhow::{bail, Context, Result};

use crate::process::ProcessRunner;
use crate::tmux;

/// tmux pane option marking the pane this module opened, so the next open finds
/// it instead of splitting another. See
/// `docs/specs/agent-tree.allium`'s `OneEditorPanePerAgentWindow`.
pub const EDITOR_PANE_OPTION: &str = "@dispatch_editor_pane";

/// Height of the editor pane as a percentage of the agent window — matches
/// `agent_tree_editor_pane_percent` in docs/specs/agent-tree.allium.
pub const EDITOR_PANE_PERCENT: u8 = 60;

/// The pane this process is running in, from `$TMUX_PANE`.
///
/// tmux exports it into every pane it creates, so it is present whenever the
/// renderer runs where it is meant to. Its absence means the renderer was
/// started outside tmux, which is a real (if unusual) way to run it — hence an
/// error the caller can show, not a panic.
pub fn current_pane_from_env() -> Result<String> {
    let pane = std::env::var("TMUX_PANE")
        .ok()
        .filter(|p| !p.is_empty())
        .context("not running inside tmux ($TMUX_PANE is unset)")?;
    Ok(pane)
}

/// Show `relative` (a path below `root`) in this agent window's editor pane:
/// split one below the tree at [`EDITOR_PANE_PERCENT`] if there is none yet,
/// otherwise replace what the existing one is running.
///
/// `my_pane` is the calling renderer's own pane id, used both as the split
/// target and to identify the window to look in — every pane involved is in the
/// agent's own window.
///
/// Focus does not move (see [`tmux::split_window_full_below_running`]), so the
/// user can keep browsing; each subsequent call swaps the file shown below.
/// Replacing kills whatever the pane was running, editor included: accepted at
/// design time, in exchange for a layout that does not subdivide on every open.
pub fn open_in_editor(
    root: &Path,
    relative: &Path,
    my_pane: &str,
    editor: &[String],
    runner: &dyn ProcessRunner,
) -> Result<()> {
    if editor.is_empty() {
        bail!("no editor to run");
    }
    // `..` in a selection path would address a file outside the worktree, which
    // no rendered node can name — the tree is built from path segments — but the
    // pane is deliberately rooted at the worktree and this keeps it that way.
    if relative
        .components()
        .any(|c| matches!(c, Component::ParentDir | Component::RootDir))
    {
        bail!("{}: not a path inside the worktree", relative.display());
    }

    let absolute = root.join(relative);
    if !absolute.is_file() {
        bail!("{}: no longer exists", relative.display());
    }
    let absolute = absolute.to_string_lossy().into_owned();
    let root = root.to_string_lossy().into_owned();

    let mut command: Vec<&str> = editor.iter().map(String::as_str).collect();
    command.push(&absolute);

    // A failed lookup must not degrade to "no editor pane": that would split a
    // fresh pane on every press until the window ran out of room.
    let existing = tmux::pane_ids_with_option(my_pane, EDITOR_PANE_OPTION, runner)
        .context("failed to look for an existing editor pane")?;

    if let Some(pane) = existing.first() {
        return tmux::respawn_pane_running(pane, &root, &command, runner);
    }

    let pane = tmux::split_window_full_below_running(
        my_pane,
        EDITOR_PANE_PERCENT,
        &root,
        &command,
        runner,
    )?;
    // The pane is open and showing the file; the marker only matters to the
    // *next* open, so a failure here is logged rather than reported as a failed
    // open. Worst case the next press splits a second pane.
    if let Err(e) = tmux::set_pane_option(&pane, EDITOR_PANE_OPTION, "1", runner) {
        tracing::warn!(%pane, error = %e, "failed to mark the editor pane");
    }
    Ok(())
}
```

- [ ] **Step 4: Run the tests to verify they pass**

Run: `cargo test agent_tree_editor 2>&1 | tail -5`
Expected: PASS.

- [ ] **Step 5: Check `tempfile` is a dev-dependency**

Run: `rtk proxy grep -n "tempfile" Cargo.toml`
Expected: present (the harness already uses it). If it is only under `[dev-dependencies]` that is correct — these are `#[cfg(test)]` tests.

- [ ] **Step 6: Commit**

```bash
git add src/agent_tree_editor.rs
git commit -m "feat(agent-tree): open or replace the editor pane for a selected file"
```

---

## Task 6: Wire Space/Enter in the renderer

**Files:**
- Modify: `src/cli/agent_tree.rs`
- Modify: `src/main.rs` (`cmd_agent_tree`)

**Interfaces:**
- Consumes: `agent_tree_editor::{open_in_editor, editor_from_env, current_pane_from_env}` (Tasks 4–5).
- Produces: `KeyAction::OpenInEditor(PathBuf)` (path **relative** to the pane root); `RenderState::notice: Option<String>`.

- [ ] **Step 1: Write the failing tests**

Add to `src/cli/agent_tree.rs`'s `mod tests`. `three_node_log()` and `KeyRig` already exist there.

```rust
    /// Space and Enter on a *file* ask the loop to open it. The action carries
    /// the path relative to the pane root — `handle_key` stays pure, so joining
    /// it to the worktree is the loop's job.
    #[test]
    fn space_and_enter_on_a_file_ask_to_open_it_in_an_editor() {
        for code in [KeyCode::Char(' '), KeyCode::Enter] {
            let mut rig = KeyRig::new(&three_node_log());
            rig.press(KeyCode::Char('j'));
            assert_eq!(rig.selected(), vec!["a.rs".to_string()], "{code:?}");

            assert_eq!(
                rig.press(code),
                KeyAction::OpenInEditor(PathBuf::from("a.rs")),
                "{code:?}"
            );
        }
    }

    #[test]
    fn opening_a_nested_file_carries_its_whole_relative_path() {
        let mut rig = KeyRig::new(&three_node_log());
        rig.press(KeyCode::Char('j'));
        rig.press(KeyCode::Char('j'));
        rig.press(KeyCode::Char('j'));
        assert_eq!(
            rig.selected(),
            vec!["src".to_string(), "lib.rs".to_string()]
        );

        assert_eq!(
            rig.press(KeyCode::Enter),
            KeyAction::OpenInEditor(PathBuf::from("src/lib.rs"))
        );
    }

    /// The directory behaviour is unchanged: Space/Enter still toggles, and must
    /// not ask to open anything.
    #[test]
    fn space_on_a_directory_still_toggles_and_does_not_open() {
        let mut rig = KeyRig::new(&three_node_log());
        rig.press(KeyCode::Char('j'));
        rig.press(KeyCode::Char('j'));
        assert_eq!(rig.selected(), vec!["src".to_string()]);

        assert_eq!(rig.press(KeyCode::Char(' ')), KeyAction::Continue);
        assert!(!rig.is_open(&["src"]));
    }

    #[test]
    fn space_with_nothing_selected_does_nothing() {
        let mut rig = KeyRig::new(&three_node_log());
        assert!(rig.selected().is_empty());
        assert_eq!(rig.press(KeyCode::Char(' ')), KeyAction::Continue);
    }

    /// `l`/`Right` are expansion keys only — they must NOT have picked up the
    /// open behaviour along with Space/Enter (#3834's guard stays a guard).
    #[test]
    fn l_and_right_on_a_file_still_do_nothing() {
        for code in [KeyCode::Char('l'), KeyCode::Right] {
            let mut rig = KeyRig::new(&three_node_log());
            rig.press(KeyCode::Char('j'));
            assert_eq!(rig.press(code), KeyAction::Continue, "{code:?}");
        }
    }

    #[test]
    fn any_key_clears_a_pending_notice() {
        let mut rig = KeyRig::new(&three_node_log());
        rig.state.notice = Some("src/gone.rs: no longer exists".to_string());

        rig.press(KeyCode::Char('j'));

        assert!(rig.state.notice.is_none());
    }

    #[test]
    fn snapshot_notice_is_shown_in_the_bottom_border() {
        let tree = build_tree(&root(), &three_node_log());
        let mut state = RenderState::new();
        state.sync_expansion(&tree);
        state.notice = Some("src/gone.rs: no longer exists".to_string());
        let mut terminal = Terminal::new(TestBackend::new(50, 12)).expect("terminal");
        terminal
            .draw(|frame| render(frame, frame.area(), &tree, &mut state, "dispatch"))
            .expect("draw");
        let rendered = buffer_to_string(terminal.backend().buffer());

        assert!(
            rendered.contains("no longer exists"),
            "the notice must be visible; rendered:\n{rendered}"
        );
        insta::assert_snapshot!(rendered);
    }
```

- [ ] **Step 2: Run the tests to verify they fail**

Run: `cargo test cli::agent_tree 2>&1 | tail -20`
Expected: FAIL — no variant `OpenInEditor`, no field `notice`.

- [ ] **Step 3: Implement `KeyAction::OpenInEditor` and the notice**

In `src/cli/agent_tree.rs`:

1. Extend the action enum (it is no longer `Copy`, so remove that derive; the loop compares it by value):

```rust
/// What the event loop should do after `handle_key` has processed a key.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum KeyAction {
    /// Stay in the loop and redraw.
    Continue,
    /// Leave the loop, which exits the process and so closes the tmux pane.
    Exit,
    /// Show this path — relative to the pane root — in the window's editor
    /// pane. `handle_key` stays pure: resolving the absolute path, the editor
    /// and the tmux calls all belong to the loop.
    OpenInEditor(PathBuf),
}
```

2. Add `notice` to `RenderState`, alongside `tree_state` and `auto_expanded`:

```rust
    /// A one-line failure notice, rendered in the pane's bottom border and
    /// cleared by the next key press. Opening a file is the only action here
    /// that can fail visibly to the user (see the spec's
    /// `AgentTreeEditorOpenFailureIsVisible`).
    pub notice: Option<String>,
```

Initialise it to `None` in `RenderState::new`.

3. Add the file-selection resolver next to `selected_is_directory`:

```rust
/// The selected node's path relative to the pane root, when the selection is a
/// **file**. `None` for a directory, for an empty selection, and for a stale
/// selection left over from before a rebuild — the same fail-closed shape as
/// `selected_is_directory`.
fn selected_file_path(root: &TreeNode, selected: &[String]) -> Option<PathBuf> {
    if selected.is_empty() {
        return None;
    }
    let node = root.node_at(selected)?;
    if node.kind != TreeNodeKind::File {
        return None;
    }
    Some(selected.iter().collect())
}
```

4. In `handle_key`, clear the notice first and add the file arm *before* the directory arm:

```rust
pub fn handle_key(state: &mut RenderState, root: &TreeNode, key: KeyEvent) -> KeyAction {
    // Any key acknowledges a failure notice (spec: ClearAgentTreeErrorNotice).
    state.notice = None;
    // `TreeState`'s navigation methods return whether anything changed; the
    // loop redraws unconditionally, so the answer is discarded.
    match key.code {
        // ... q, Ctrl-C, k/Up, j/Down, h/Left, l/Right unchanged ...

        // Space/Enter dispatch on the selected node's kind: open a file, toggle
        // a directory. The file arm is first so the directory guard below only
        // ever sees non-files.
        KeyCode::Char(' ') | KeyCode::Enter => {
            if let Some(path) = selected_file_path(root, state.tree_state.selected()) {
                return KeyAction::OpenInEditor(path);
            }
            if selected_is_directory(root, state.tree_state.selected()) {
                state.tree_state.toggle_selected();
            }
        }
        _ => {}
    }
    KeyAction::Continue
}
```

Note the `l`/`Right` arm keeps its `if selected_is_directory(...)` guard exactly as it is — expansion on a file is still a no-op (#3834).

5. In `render`, hang the notice off the block:

```rust
    let mut block = Block::default()
        .borders(Borders::ALL)
        .title(format!(" {title} "));
    if let Some(notice) = &state.notice {
        block = block.title_bottom(Line::from(Span::styled(
            format!(" {notice} "),
            Style::default().fg(YELLOW).add_modifier(Modifier::BOLD),
        )));
    }
```

`block` is moved into both match arms below, so bind it before the `match Tree::new(&items)` exactly where the current `let block = ...` sits.

- [ ] **Step 4: Run the tests to verify they pass**

Run: `cargo test cli::agent_tree 2>&1 | tail -20`
Expected: PASS, except the new snapshot, which needs accepting:

```bash
INSTA_UPDATE=always cargo test cli::agent_tree
rm -f src/cli/snapshots/*.snap.new
```

Inspect the accepted `.snap` file and confirm the notice really is on the bottom border line before continuing. Snapshot files for this module live next to it — check `ls src/cli/snapshots/` for where the existing `snapshot_empty_tree_shows_bare_title` snapshot landed and use that directory.

- [ ] **Step 5: Perform the effect in `run_loop`**

Change `run_loop`'s signature to take the runner, and handle the new action:

```rust
fn run_loop<B: Backend>(
    terminal: &mut Terminal<B>,
    root: &Path,
    events_path: &Path,
    runner: &dyn ProcessRunner,
) -> Result<()> {
```

and replace the key-handling block's body:

```rust
            match handle_key(&mut state, &tree, key) {
                KeyAction::Exit => return Ok(()),
                KeyAction::Continue => {}
                KeyAction::OpenInEditor(relative) => {
                    // Every failure here is the user's to see: they pressed a
                    // key and expect a file. The renderer keeps running
                    // regardless — a broken editor must not close the tree.
                    if let Err(e) = open_selected(root, &relative, runner) {
                        tracing::warn!(
                            path = %relative.display(),
                            error = %e,
                            "failed to open the selected file in an editor"
                        );
                        state.notice = Some(format!("{e}"));
                    }
                }
            }
            continue;
```

and add the helper above `run_loop`:

```rust
/// Resolve this pane and the user's editor, then show `relative` in the
/// window's editor pane. Split out of the loop so the loop's arm stays one
/// line and the error message the notice shows is built in one place.
fn open_selected(root: &Path, relative: &Path, runner: &dyn ProcessRunner) -> Result<()> {
    let my_pane = current_pane_from_env()?;
    let editor = editor_from_env();
    open_in_editor(root, relative, &my_pane, &editor, runner)
}
```

with the imports:

```rust
use crate::agent_tree_editor::{current_pane_from_env, editor_from_env, open_in_editor};
use crate::process::{ProcessRunner, RealProcessRunner};
```

In `run`, pass the runner:

```rust
    let result = run_loop(&mut terminal, &root, &events_path, &RealProcessRunner);
```

- [ ] **Step 6: Install the log subscriber for this subcommand**

In `src/main.rs`, `cmd_agent_tree` currently opens the renderer with no `tracing` subscriber, so every `tracing::warn!` the renderer already contains goes nowhere. Add it:

```rust
async fn cmd_agent_tree(db: &std::path::Path, task_id: i64) -> Result<()> {
    // The renderer owns the alternate screen, so its warnings cannot go to
    // stderr — they go to `app.log` next to the database, like the board's.
    // Best-effort: a renderer that cannot open the log still renders.
    let data_dir = db.parent().unwrap_or(std::path::Path::new("."));
    let _ = init_app_log_subscriber(data_dir);
    dispatch_tui::cli::agent_tree::run(db, task_id).await
}
```

- [ ] **Step 7: Run the whole suite**

Run: `cargo test 2>&1 | tail -20`
Expected: PASS. If a `KeyAction` match elsewhere now fails to compile (it was `Copy` and compared with `==`), make it exhaustive rather than adding a catch-all.

- [ ] **Step 8: Commit**

```bash
git add src/cli/agent_tree.rs src/main.rs src/cli/snapshots
git commit -m "feat(agent-tree): Space/Enter on a file opens it in the editor pane"
```

---

## Task 7: Identify the tree pane by what it is, not by focus

**Files:**
- Modify: `src/dispatch/agents.rs` (`toggle_agent_tree_pane`, `resync_agent_tree_pane`, and a new pane-matching helper)
- Modify: `src/dispatch/split_panes.rs` (`join_task_window_into_pane`)
- Modify: `src/runtime/tests.rs` (the existing mock scripts whose queued `list-panes` output changes shape)

**Interfaces:**
- Consumes: `tmux::{pane_ids_with_start_command, pane_ids_with_option}` (Task 3), `agent_tree_editor::EDITOR_PANE_OPTION` (Task 5).
- Produces:
  - `pub fn agent_tree_pane_id(window: &str, runner: &dyn ProcessRunner) -> Result<Option<String>>` in `src/dispatch/agents.rs`
  - `pub fn companion_pane_ids(window: &str, runner: &dyn ProcessRunner) -> Result<Vec<String>>` in `src/dispatch/agents.rs`

- [ ] **Step 1: Write the failing tests**

Add to `src/dispatch/tests.rs` (where `parse_tmux_window_task_id_*` and the other agents tests live):

```rust
/// The regression this replaces the active/inactive heuristic for: with focus
/// in the companion pane, "the window's single inactive pane" is the *agent's*,
/// so the toggle killed the user's claude session (#3856).
#[test]
fn toggle_kills_the_tree_pane_even_when_the_tree_pane_is_active() {
    let runner = MockProcessRunner::new(vec![
        // pane_ids_with_start_command: %2 is the tree pane and it is active
        MockProcessRunner::ok_with_stdout(b"%1 \n%2 dispatch agent-tree 42\n"),
        // kill-pane
        MockProcessRunner::ok(),
    ])
    .with_windows(&["task-42"]);

    toggle_agent_tree_pane("task-42", &runner).expect("toggle");

    let calls = runner.recorded_calls();
    assert_eq!(calls[1].1, vec!["kill-pane", "-t", "%2"]);
}

/// An editor pane makes two panes inactive, which the old heuristic read as
/// "hidden" — so the toggle split a *second* tree pane.
#[test]
fn toggle_with_an_editor_pane_open_still_kills_the_tree_pane() {
    let runner = MockProcessRunner::new(vec![
        MockProcessRunner::ok_with_stdout(
            b"%1 \n%2 dispatch agent-tree 42\n%3 vim /w/src/lib.rs\n",
        ),
        MockProcessRunner::ok(),
    ])
    .with_windows(&["task-42"]);

    toggle_agent_tree_pane("task-42", &runner).expect("toggle");

    let calls = runner.recorded_calls();
    assert_eq!(
        calls.len(),
        2,
        "expected exactly a lookup and a kill; calls: {calls:?}"
    );
    assert_eq!(calls[1].1, vec!["kill-pane", "-t", "%2"]);
}

/// An editor pane showing this very spec file contains the string
/// "agent-tree", so the match cannot be a substring test.
#[test]
fn toggle_is_not_fooled_by_an_editor_pane_showing_an_agent_tree_file() {
    let runner = MockProcessRunner::new(vec![
        MockProcessRunner::ok_with_stdout(
            b"%1 \n%3 vim /w/docs/specs/agent-tree.allium\n",
        ),
        // No tree pane found, so the toggle SPLITS one: split-window.
        MockProcessRunner::ok_with_stdout(b"%4\n"),
    ])
    .with_windows(&["task-42"]);

    toggle_agent_tree_pane("task-42", &runner).expect("toggle");

    let calls = runner.recorded_calls();
    assert_eq!(calls[1].1[0], "split-window", "calls: {calls:?}");
    assert!(
        !calls.iter().any(|(_, args)| args[0] == "kill-pane"),
        "must not kill a pane it did not create; calls: {calls:?}"
    );
}

/// The dispatch binary is named through `ProcessRunner::agent_binaries`, and it
/// may be an absolute path — so the match is on argv0's *basename*.
#[test]
fn the_tree_pane_is_matched_by_an_absolute_dispatch_path() {
    let runner = MockProcessRunner::new(vec![
        MockProcessRunner::ok_with_stdout(b"%2 /opt/bin/dispatch agent-tree 42\n"),
        MockProcessRunner::ok(),
    ])
    .with_windows(&["task-42"]);

    toggle_agent_tree_pane("task-42", &runner).expect("toggle");

    assert_eq!(runner.recorded_calls()[1].1, vec!["kill-pane", "-t", "%2"]);
}

#[test]
fn toggle_splits_a_tree_pane_when_the_window_has_none() {
    let runner = MockProcessRunner::new(vec![
        MockProcessRunner::ok_with_stdout(b"%1 \n"),
        MockProcessRunner::ok_with_stdout(b"%2\n"),
    ])
    .with_windows(&["task-42"]);

    toggle_agent_tree_pane("task-42", &runner).expect("toggle");

    assert_eq!(runner.recorded_calls()[1].1[0], "split-window");
}

#[test]
fn toggle_is_a_no_op_for_a_window_that_is_not_a_task_window() {
    let runner = MockProcessRunner::new(vec![]).with_queued_window_lookup();
    toggle_agent_tree_pane("TUI", &runner).expect("toggle");
    assert!(runner.recorded_calls().is_empty());
}

/// Pinning moves only the agent's own pane out, so *every* companion pane left
/// behind has to go — with an editor pane open the old single-inactive-pane
/// lookup was ambiguous and orphaned both.
#[test]
fn companion_pane_ids_returns_both_the_tree_and_the_editor_pane() {
    let runner = MockProcessRunner::new(vec![
        // pane_ids_with_start_command (tree)
        MockProcessRunner::ok_with_stdout(
            b"%1 \n%2 dispatch agent-tree 42\n%3 vim /w/a.rs\n",
        ),
        // pane_ids_with_option (editor)
        MockProcessRunner::ok_with_stdout(b"%1 \n%2 \n%3 1\n"),
    ])
    .with_windows(&["task-42"]);

    let found = companion_pane_ids("task-42", &runner).expect("lookup");

    assert_eq!(found, vec!["%2".to_string(), "%3".to_string()]);
}
```

Match the existing imports at the top of `src/dispatch/tests.rs`; add `toggle_agent_tree_pane` / `companion_pane_ids` to the `use super::agents::...` (or `use super::*`) line as that file's style requires.

- [ ] **Step 2: Run the tests to verify they fail**

Run: `cargo test dispatch::tests::toggle 2>&1 | tail -20`
Expected: FAIL — the current implementation queues an `inactive_pane_id`-shaped lookup, so the argv assertions and the response ordering do not match.

- [ ] **Step 3: Implement the lookups and rewire the three call sites**

In `src/dispatch/agents.rs`:

```rust
/// Whether `start_command` is a `dispatch agent-tree <id>` invocation — the
/// companion pane this module spawns.
///
/// Matched as argv0's basename plus argv1, not as a substring: an editor pane
/// opened on `docs/specs/agent-tree.allium` contains the string "agent-tree",
/// and killing the user's editor instead of the tree would be exactly the class
/// of bug this lookup exists to fix. argv0 may be an absolute path because the
/// binary is named through `ProcessRunner::agent_binaries` (which is how the
/// real-tmux harness points it at a stub), hence the basename comparison.
fn is_agent_tree_command(start_command: &str, dispatch_bin: &str) -> bool {
    let mut argv = start_command.split_whitespace();
    let Some(argv0) = argv.next() else {
        return false;
    };
    let basename = |s: &str| {
        std::path::Path::new(s)
            .file_name()
            .map(|n| n.to_string_lossy().into_owned())
            .unwrap_or_else(|| s.to_string())
    };
    basename(argv0) == basename(dispatch_bin) && argv.next() == Some(AGENT_TREE_SUBCOMMAND)
}

/// The `dispatch` subcommand the companion pane runs. Shared between the
/// spawn side and the lookup side so the two cannot drift.
const AGENT_TREE_SUBCOMMAND: &str = "agent-tree";

/// The companion agent-tree pane in `window`, if it has one.
///
/// Replaces a `tmux::inactive_pane_id` call, which asked "which pane is not
/// focused?" and answered "the companion" only for a two-pane window whose
/// focus had not moved. Neither holds: the user can focus the tree pane (and
/// must, to press keys in it), and an editor pane opened from the tree makes a
/// third. Identifying the pane by the command it was started with is true
/// whatever the focus and however many panes there are — and it is retroactive,
/// since tmux reports `#{pane_start_command}` for panes already running.
pub fn agent_tree_pane_id(window: &str, runner: &dyn ProcessRunner) -> Result<Option<String>> {
    let dispatch_bin = runner.agent_binaries().dispatch;
    let panes = tmux::pane_ids_with_start_command(
        window,
        |cmd| is_agent_tree_command(cmd, &dispatch_bin),
        runner,
    )?;
    Ok(panes.into_iter().next())
}

/// Every pane in `window` that dispatch put there beside the agent's own: the
/// companion tree pane and the editor pane opened from it.
///
/// Used by the split-pane pin path, which moves only the agent's own pane out
/// and must not leave the rest behind in a window nothing owns.
pub fn companion_pane_ids(window: &str, runner: &dyn ProcessRunner) -> Result<Vec<String>> {
    let mut panes = Vec::new();
    if let Some(tree) = agent_tree_pane_id(window, runner)? {
        panes.push(tree);
    }
    panes.extend(tmux::pane_ids_with_option(
        window,
        crate::agent_tree_editor::EDITOR_PANE_OPTION,
        runner,
    )?);
    Ok(panes)
}
```

Use `AGENT_TREE_SUBCOMMAND` in `spawn_agent_tree_pane` in place of its literal `"agent-tree"`.

Then rewire:

```rust
pub fn toggle_agent_tree_pane(window: &str, runner: &dyn ProcessRunner) -> Result<()> {
    let Some(task_id) = parse_tmux_window_task_id(window) else {
        return Ok(());
    };
    match agent_tree_pane_id(window, runner)? {
        Some(pane_id) => tmux::kill_pane(&pane_id, runner),
        None => {
            spawn_agent_tree_pane(window, task_id, runner);
            Ok(())
        }
    }
}
```

`resync_agent_tree_pane`: replace its `tmux::inactive_pane_id(window, runner)` with `agent_tree_pane_id(window, runner)`, keeping the three-arm `match` (`Ok(Some)` → kill + respawn, `Ok(None)` → nothing, `Err` → warn) and its warning messages as they are.

In `src/dispatch/split_panes.rs`, `join_task_window_into_pane`: replace the single `companion_pane_id` with the plural lookup, keeping the "capture before the join" ordering and the best-effort degradation:

```rust
    let companion_panes = match crate::dispatch::agents::companion_pane_ids(window, runner) {
        Ok(ids) => ids,
        Err(e) => {
            tracing::warn!(
                %window,
                error = %e,
                "failed to check for companion panes before join-pane"
            );
            Vec::new()
        }
    };

    let pane_id = tmux::join_pane(window, target_pane, runner)?;

    for companion_id in companion_panes {
        if let Err(e) = tmux::kill_pane(&companion_id, runner) {
            tracing::warn!(
                %window,
                %companion_id,
                error = %e,
                "failed to kill leftover companion pane after join-pane"
            );
        }
    }
```

Adjust the module-level `use` for whatever path `companion_pane_ids` needs (`super::agents::companion_pane_ids`).

- [ ] **Step 4: Run the tests to verify they pass**

Run: `cargo test dispatch:: 2>&1 | tail -20`
Expected: PASS.

- [ ] **Step 5: Fix the mock scripts in `src/runtime/tests.rs`**

Five scripts there queue an `inactive_pane_id`-shaped `list-panes` reply (`b"1 %1\n"` / `b"1 %1\n0 %2\n"`), commented `inactive_pane_id check`. The new lookups ask for `#{pane_id} #{pane_start_command}` — and the pin path now issues *two* lookups, so a second reply is needed.

Run: `cargo test runtime:: 2>&1 | tail -30` to see which fail, then update each script:
- "no companion" (`src/runtime/tests.rs:2396`, `:2835`) → `b"%1 \n"`, comment `// agent_tree_pane_id: no companion`.
- "companion is %2" (`:2440`, `:2515`) → `b"%1 \n%2 dispatch agent-tree 42\n"` — use whatever task id that test dispatches, and the stub dispatch binary name if the test sets one via `with_agent_binaries`.
- the error case (`:2479`) keeps `MockProcessRunner::fail(...)`.
- For the pin path, add the editor-pane lookup reply (`b"%1 \n%2 \n"`) after the tree lookup.

- [ ] **Step 6: Run the whole suite**

Run: `cargo test 2>&1 | tail -20`
Expected: PASS.

- [ ] **Step 7: Commit**

```bash
git add src/dispatch/agents.rs src/dispatch/split_panes.rs src/dispatch/tests.rs src/runtime/tests.rs
git commit -m "fix(agent-tree): resolve the toggle target by pane identity, not focus"
```

---

## Task 8: Real-tmux coverage

**Files:**
- Create: `tests/tmux_editor_pane.rs`
- Modify: `tests/tmux_harness/mod.rs` (two oracles)

**Interfaces:**
- Consumes: `TmuxServer`, `tmux_available_or_skip`, `SocketRunner` (existing harness); `agent_tree_editor::{open_in_editor, EDITOR_PANE_OPTION}`; `dispatch::{toggle_agent_tree_pane, join_task_window_into_pane}`.

A mock proves which command string we sent; only a real server proves what tmux did with it. See the harness's module docs and learning #327.

- [ ] **Step 1: Add the harness oracle and a third stub**

In `tests/tmux_harness/mod.rs`, next to `window_option`:

```rust
    /// A pane-scoped user option (e.g. `@dispatch_editor_pane`). Empty when unset.
    pub fn pane_option(&self, pane_id: &str, option: &str) -> String {
        self.tmux_stdout(&["show-options", "-pqv", "-t", pane_id, option])
    }

    /// Write an additional stub binary into this server's stub dir and return
    /// its absolute path. Same shape as the `claude` / `dispatch` stubs: it
    /// records one line to [`Self::stub_log`] and then holds its pane open with
    /// `exec cat`.
    ///
    /// Exists because the editor pane's command is not a dispatch binary — it
    /// comes from `$EDITOR` — so it cannot be one of the two stubs written at
    /// server start. Holding the pane open is the load-bearing part: a stub that
    /// exited would close its pane and make every pane-count assertion racy.
    pub fn extra_stub(&self, name: &str) -> String {
        write_stub(self.stubs.path(), name, &self.stub_log());
        self.stubs.path().join(name).to_string_lossy().into_owned()
    }
```

`write_stub` is a private module-level function in the same file, so no visibility change is needed.

- [ ] **Step 2: Write the failing test file**

Create `tests/tmux_editor_pane.rs`. Build the agent window with the harness directly (two panes: a "claude" pane and a tree pane started with the stub dispatch binary) rather than through `dispatch_agent`, so the test is about pane mechanics only. Read `tests/tmux_lifecycle.rs`'s `Fixture` first and follow its shape — in particular the `TmuxServer::start()` + `new-session` setup and the drop ordering comment on its fields.

```rust
#![allow(clippy::unwrap_used, clippy::expect_used)]
//! Real-tmux integration tests for the editor pane the agent-tree companion
//! pane opens (docs/specs/agent-tree.allium: `OpenAgentTreeFileInEditor`,
//! `ReplaceAgentTreeEditorFile`).
//!
//! What only a real server can show: how many panes a window ends up with after
//! two opens, which pane keeps focus, which cwd the editor resolved, and which
//! pane `prefix+e` kills. `MockProcessRunner` can only pin the command strings —
//! see tests/tmux_harness/mod.rs.

mod tmux_harness;

use std::path::PathBuf;

use dispatch_tui::agent_tree_editor::{open_in_editor, EDITOR_PANE_OPTION};
use dispatch_tui::dispatch;
use dispatch_tui::process::ProcessRunner;

use tmux_harness::{await_stub_line, stub_lines, tmux_available_or_skip, StubLine, TmuxServer};

const WINDOW: &str = "task-42";
/// The stand-in for `$EDITOR`. A harness stub, so it records its own cwd and
/// argv and then holds its pane open — a real editor would need a tty, and a
/// command that exits would close the pane mid-assertion.
const EDITOR_STUB: &str = "fake-editor";

/// An agent window shaped like a live one: the agent's own pane (active) plus a
/// companion tree pane started with the stub `dispatch agent-tree 42`, and a
/// worktree on disk holding one file to open.
struct Fixture {
    server: TmuxServer,
    dir: tempfile::TempDir,
    editor_bin: String,
    tree_pane: String,
    agent_pane: String,
}

fn setup_or_skip() -> Option<Fixture> {
    if !tmux_available_or_skip() {
        return None;
    }
    let server = TmuxServer::start();
    let dir = tempfile::tempdir().expect("tempdir");
    std::fs::create_dir_all(dir.path().join("src")).expect("mkdir");
    std::fs::write(dir.path().join("src/lib.rs"), "fn main() {}").expect("write a.rs");
    std::fs::write(dir.path().join("README.md"), "# hi").expect("write README");

    let root = dir.path().to_string_lossy().into_owned();
    server.tmux_ok(&["new-session", "-d", "-s", "t", "-n", WINDOW, "-c", &root]);
    let agent_pane = server.active_pane_id(WINDOW).expect("agent pane");

    // The companion pane exactly as production spawns it: the stub dispatch
    // binary, `agent-tree`, the task id — so the start-command lookup under
    // test sees a real production-shaped command line.
    let dispatch_bin = server.runner().agent_binaries().dispatch;
    let tree_pane = dispatch_tui::tmux::split_window_horizontal_running(
        WINDOW,
        30,
        &[&dispatch_bin, "agent-tree", "42"],
        &server.runner(),
    )
    .expect("split companion pane");

    let editor_bin = server.extra_stub(EDITOR_STUB);

    Some(Fixture {
        server,
        dir,
        editor_bin,
        tree_pane,
        agent_pane,
    })
}

impl Fixture {
    fn open(&self, relative: &str) {
        open_in_editor(
            self.dir.path(),
            &PathBuf::from(relative),
            &self.tree_pane,
            &[self.editor_bin.clone()],
            &self.server.runner(),
        )
        .expect("open in editor");
    }

    fn editor_pane(&self) -> Option<String> {
        self.server
            .pane_ids(WINDOW)
            .into_iter()
            .find(|id| self.server.pane_option(id, EDITOR_PANE_OPTION) == "1")
    }

    /// Wait for the editor stub to report having been handed `relative`. The
    /// pane starts asynchronously relative to `open_in_editor` returning, so
    /// this polls (deadline-bounded, never a fixed sleep).
    fn await_opened(&self, relative: &str) -> StubLine {
        await_stub_line(&self.server, |line| {
            line.name == EDITOR_STUB && line.args.contains(relative)
        })
        .unwrap_or_else(|| {
            panic!(
                "editor stub was never handed {relative}; recorded: {:?}",
                stub_lines(&self.server)
            )
        })
    }
}

#[test]
fn opening_a_file_adds_one_marked_pane() {
    let Some(fx) = setup_or_skip() else { return };

    fx.open("src/lib.rs");

    assert_eq!(
        fx.server.pane_count(WINDOW),
        3,
        "agent + tree + editor; panes: {:?}",
        fx.server.pane_ids(WINDOW)
    );
    assert!(
        fx.editor_pane().is_some(),
        "the new pane must carry {EDITOR_PANE_OPTION}"
    );
}

/// The point of `-d`: the user keeps browsing the tree after opening a file.
#[test]
fn opening_a_file_does_not_move_focus() {
    let Some(fx) = setup_or_skip() else { return };
    fx.server
        .tmux_ok(&["select-pane", "-t", &fx.tree_pane]);

    fx.open("src/lib.rs");

    assert_eq!(
        fx.server.active_pane_id(WINDOW).as_deref(),
        Some(fx.tree_pane.as_str()),
        "focus must stay in the tree pane"
    );
}

/// `-f`: the editor pane spans the window rather than subdividing the narrow
/// tree pane it was split from.
#[test]
fn the_editor_pane_spans_the_window_width() {
    let Some(fx) = setup_or_skip() else { return };

    fx.open("src/lib.rs");

    let editor = fx.editor_pane().expect("editor pane");
    let width = |pane: &str| {
        fx.server
            .tmux_stdout(&["display-message", "-p", "-t", pane, "#{pane_width}"])
            .parse::<u32>()
            .expect("width")
    };
    let window_width = fx
        .server
        .tmux_stdout(&["display-message", "-p", "-t", WINDOW, "#{window_width}"])
        .parse::<u32>()
        .expect("window width");
    assert_eq!(width(&editor), window_width);
    assert!(
        width(&editor) > width(&fx.tree_pane),
        "the editor pane must be wider than the tree it was split from"
    );
}

/// #231's failure mode, for this pane: `-c` is passed explicitly because the
/// `@dispatch_dir` split hook would type `cd` into the editor.
#[test]
fn the_editor_runs_in_the_worktree_with_the_absolute_path() {
    let Some(fx) = setup_or_skip() else { return };

    fx.open("src/lib.rs");

    // The stub reports its own `$PWD` and argv, so this observes what the
    // editor process actually got — not merely what tmux was asked for.
    let line = fx.await_opened("src/lib.rs");
    let want = std::fs::canonicalize(fx.dir.path()).expect("canonicalize");
    let got = std::fs::canonicalize(&line.cwd).expect("canonicalize");
    assert_eq!(got, want, "line: {line:?}");
    assert!(
        line.args.starts_with('/'),
        "the editor must be handed an absolute path; line: {line:?}"
    );
}

#[test]
fn a_second_open_reuses_the_same_pane() {
    let Some(fx) = setup_or_skip() else { return };

    fx.open("src/lib.rs");
    fx.await_opened("src/lib.rs");
    let first = fx.editor_pane().expect("editor pane");

    fx.open("README.md");

    // The second file reaching the stub is what proves the respawn ran the
    // editor again rather than leaving the first file on screen.
    fx.await_opened("README.md");
    assert_eq!(
        fx.server.pane_count(WINDOW),
        3,
        "a second open must not add a pane; panes: {:?}",
        fx.server.pane_ids(WINDOW)
    );
    assert_eq!(
        fx.editor_pane().as_deref(),
        Some(first.as_str()),
        "respawn preserves the pane and its marker"
    );
}

/// The regression: with focus in the tree pane, the old
/// single-inactive-pane lookup identified the *agent's* pane as the companion
/// and killed the user's claude session.
#[test]
fn the_toggle_kills_the_tree_pane_not_the_agents_when_the_tree_has_focus() {
    let Some(fx) = setup_or_skip() else { return };
    fx.server
        .tmux_ok(&["select-pane", "-t", &fx.tree_pane]);

    dispatch::toggle_agent_tree_pane(WINDOW, &fx.server.runner()).expect("toggle");

    assert!(
        !fx.server.pane_exists(&fx.tree_pane),
        "the tree pane must be gone"
    );
    assert!(
        fx.server.pane_exists(&fx.agent_pane),
        "the agent's own pane must survive"
    );
}

#[test]
fn the_toggle_kills_the_tree_pane_with_an_editor_pane_open() {
    let Some(fx) = setup_or_skip() else { return };
    fx.open("src/lib.rs");
    let editor = fx.editor_pane().expect("editor pane");

    dispatch::toggle_agent_tree_pane(WINDOW, &fx.server.runner()).expect("toggle");

    assert!(!fx.server.pane_exists(&fx.tree_pane), "tree pane must go");
    assert!(fx.server.pane_exists(&editor), "editor pane must stay");
    assert!(fx.server.pane_exists(&fx.agent_pane), "agent pane must stay");
}

/// Pinning moves only the agent's own pane into the board window; every pane
/// dispatch added must be cleaned up, or they are orphaned in a window nothing
/// owns.
#[test]
fn pinning_drains_both_the_tree_and_the_editor_pane() {
    let Some(fx) = setup_or_skip() else { return };
    fx.open("src/lib.rs");
    let editor = fx.editor_pane().expect("editor pane");
    fx.server
        .tmux_ok(&["new-window", "-d", "-n", "board"]);
    let board_pane = fx.server.active_pane_id("board").expect("board pane");

    dispatch::join_task_window_into_pane(WINDOW, &board_pane, &fx.server.runner())
        .expect("pin");

    assert!(!fx.server.pane_exists(&fx.tree_pane), "tree pane orphaned");
    assert!(!fx.server.pane_exists(&editor), "editor pane orphaned");
}
```

- [ ] **Step 3: Run the tests to verify they fail, then pass**

Run: `cargo test --test tmux_editor_pane 2>&1 | tail -30`

They should fail first only if something is genuinely missing (the harness oracles, an export). If a test fails on a *harness* detail rather than production behaviour — the fixture's window setup, `active_pane_id` on a fresh session, `join_task_window_into_pane`'s signature — fix the test, and read the failing assertion carefully before concluding production is wrong. Confirm the file is not silently skipping: `cargo test --test tmux_editor_pane -- --nocapture 2>&1 | rtk proxy grep -c "skipping"` must print `0`.

- [ ] **Step 4: Commit**

```bash
git add tests/tmux_editor_pane.rs tests/tmux_harness/mod.rs
git commit -m "test(agent-tree): real-tmux coverage for the editor pane"
```

---

## Task 9: Documentation and spec alignment

**Files:**
- Modify: `docs/reference.md` (the "Agent-tree companion pane" section)
- Modify: `CLAUDE.md` (only if a test-location row is now wrong)

- [ ] **Step 1: Update the companion-pane key table**

In `docs/reference.md`, the sentence above the table says "all of them act on the pane's own view only" — no longer true. Replace that paragraph and the `Space` / `Enter` row:

```markdown
Pressed inside the pane itself (no tmux prefix) while it has tmux focus. The pane is
its own process — these keys never reach the board TUI, and all of them act on the
pane's own view only, except `Space`/`Enter` on a file, which opens an editor.

| Key | Action |
|-----|--------|
| `j` / `↓` | Move the cursor down |
| `k` / `↑` | Move the cursor up |
| `h` / `←` | Collapse the selected directory, or move to its parent |
| `l` / `→` | Expand the selected directory (a no-op on a file) |
| `Space` / `Enter` | On a directory: toggle it open/closed. On a file: open it in `$VISUAL`, else `$EDITOR`, else `vi`, in a full-width pane below (60% of the window height). Focus stays in the tree, so you can keep browsing; the next file you open **replaces** that pane, killing whatever was running in it |
| `q` / `Ctrl+C` | Close the pane |
```

Add after the paragraph that follows the table:

```markdown
The editor pane starts in the task's worktree and is given the file's absolute path.
`$VISUAL`/`$EDITOR` are split on whitespace and run directly, with no shell — so
`EDITOR="nvim -p"` works, and nothing in the value is shell-expanded. A GUI editor
that forks (`gvim`) returns immediately, so its pane closes while the window stays
open. When opening fails — the agent deleted the file after touching it, or tmux
refused the split — the reason appears in the pane's bottom border until the next
keypress, and is logged to `app.log`.
```

- [ ] **Step 2: Verify the doc-path and doc-symbol checkers pass**

Run: `./scripts/check-doc-paths.sh && ./scripts/check-doc-symbols.sh`
Expected: both pass. `check-doc-symbols.sh` rejects backticked snake_case identifiers in agent-facing docs that appear nowhere in the code — so any identifier named in prose must exist.

- [ ] **Step 3: Check whether `CLAUDE.md` needs a row**

`CLAUDE.md`'s "Running tests" list names each `--test tmux_*` target, and its "Where new tests go" table has a row for tmux semantics. Add `cargo test --test tmux_editor_pane` to the first list and name the new file in the tmux row of the second. Keep both edits to one line each — that file is loaded into every agent's context.

- [ ] **Step 4: Run the Allium weeder**

Use the `allium:weed` skill against `docs/specs/agent-tree.allium` and resolve any divergence it reports between Task 1's spec text and the shipped code. Where the code is right, correct the spec; where the spec is right, fix the code.

- [ ] **Step 5: Run the full verify command**

Run: `cargo fmt --check && cargo test && ./scripts/check-doc-paths.sh`
Expected: PASS. Then `cargo clippy --all-targets -- -D warnings`, which the pre-push hook enforces and a plain build does not.

- [ ] **Step 6: Commit**

```bash
git add docs/reference.md CLAUDE.md docs/specs/agent-tree.allium
git commit -m "docs(agent-tree): document opening the selected file in an editor"
```

---

## Task 10: Verify by hand

Automated tests cannot see that this *feels* right. One manual pass, in a scratch database so nothing touches the real board.

- [ ] **Step 1: Run the board against a throwaway DB**

```bash
cargo run -- --db /tmp/scratch-3856.db tui
```

- [ ] **Step 2: Exercise the flow**

Dispatch any task, switch to its agent window, focus the tree pane, and once the agent has touched some files:

1. `j`/`k` to a file, press Enter → the editor opens full width below, focus stays in the tree.
2. `j` to another file, Enter → the same pane now shows the second file; still three panes.
3. `prefix+e` → the tree pane closes, the editor and the agent survive.
4. `prefix+e` again → the tree pane comes back.
5. Delete a touched file from another terminal, then press Enter on it in the tree → the border shows `<path>: no longer exists`; the next keypress clears it.

- [ ] **Step 3: Confirm the log is now populated**

Run: `rtk proxy grep -c "agent-tree\|editor" /tmp/app.log` against the scratch DB's directory (`/tmp/app.log` for the `--db /tmp/...` run above).
Expected: warnings from step 5 are present — proof the subscriber added in Task 6 works.

- [ ] **Step 4: Report anything the tests missed**

If the layout, focus behaviour or geometry is wrong in practice, that is a spec-level finding: fix the spec and the test that should have caught it, not just the code.

---

## Self-Review Notes

**Spec coverage** — every design-doc decision maps to a task: placement and focus → Tasks 2, 6, 8; one-pane-replace → Tasks 5, 8; `$VISUAL`/`$EDITOR`/`vi` → Task 4; visible failures → Tasks 6, 9; explicit pane identity → Tasks 3, 7, 8; the `split_panes.rs` pin drain → Tasks 7, 8; the missing log subscriber → Task 6; spec text → Tasks 1 and 9.

**Naming consistency** — `EDITOR_PANE_OPTION` / `EDITOR_PANE_PERCENT` / `EDITOR_FALLBACK` live in `agent_tree_editor` and are referenced by those names in Tasks 5, 7 and 8. `agent_tree_pane_id` and `companion_pane_ids` (Task 7) are the only two names `split_panes.rs` and the tests reach for. `KeyAction::OpenInEditor(PathBuf)` carries a **relative** path in every task that mentions it.

**Deliberate coverage gap** — `current_pane_from_env`'s failure path (`$TMUX_PANE` unset) has no test. `std::env::set_var` is `unsafe` in edition 2024 and races the test harness's threads regardless, and the whole point of `resolve_editor`/`open_in_editor` taking values as parameters is that everything worth testing is reachable without touching the process environment. The one line that reads the env is left uncovered rather than made testable at the cost of a racy test.

**Known risk** — Task 7 Step 5 touches mock scripts written against the old lookup shape. The exact stdout each needs depends on that test's task id and binary stubs, so the step says to run the suite and read the failures rather than guessing the bytes.
