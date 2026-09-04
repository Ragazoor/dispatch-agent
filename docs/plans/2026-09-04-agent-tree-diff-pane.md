# Agent tree: line counts and a diff pane

Task #4645. Spec: `docs/specs/agent-tree.allium` (already updated — this plan
implements what the spec now says, it does not re-decide it).

## What changes, in one paragraph

Every row of the agent-tree companion pane gains `+N -M` line counts, summed
over descendants for a directory. Space/Enter on a file no longer opens an
editor — it toggles that file's diff into a **second tmux pane** beneath the
tree, and `a` toggles every file at once. The tree writes the set of open paths
to a file in the worktree's git administrative directory; the diff pane reads
it and renders the diffs from git. Editor-open is removed outright, along with
`src/agent_tree_editor.rs`.

## Origin

Claude Code 2.1.260 shipped a similar diff panel. Its implementation cannot be
copied — it ships as a compiled bun binary with minified JS inside — so this is
a re-implementation of the behaviour, not a port. Two related findings are
recorded as learnings #610 and #611 rather than here, because they are about
Claude Code and tmux rather than about this code.

## Non-goals

- Horizontal scrolling in the diff pane. Long lines truncate; the open question
  `DiffPaneWidth` records that this may want revisiting.
- Any use of `CLAUDE_CODE_BASE_REF`. Investigated and dropped — it does not
  reach Claude Code's own panel, and routing our baseline through it would
  need four plumbing points and delete a tested fallback. See learning #611.
- Changing the baseline resolution. `AgentTreeBaselineIsTaskBaseBranch` stands
  exactly as it was.

## Phases

Each phase is test-first: write the listed tests, watch them fail for the right
reason, then write the minimum code that makes them pass. Each phase ends with
the suite green, so the branch is never mid-refactor.

### Phase 1 — Line counts

Tests (`src/agent_tree.rs`):
- `parse_numstat` reads added/removed pairs for a NUL-delimited numstat.
- A binary file's `-\t-\t<path>` numstat row parses to `None`/`None`.
- A tracked file's node carries its own counts.
- A directory node's counts sum its descendants.
- A directory whose only descendants lack counts has `None`, not `Some(0)`.
- An untracked file's node has no counts.
- A path in the name-status diff but absent from the numstat renders with no
  counts rather than zeros.

Code: add `lines_added` / `lines_removed` to `GitFileChange` and `TreeNode`;
add `parse_numstat`; add a fourth git call to `git_changes`
(`diff --numstat --no-renames -z <baseline>`); fold counts in `build_tree`.

Render tests (`src/cli/agent_tree.rs`): a snapshot showing `+12 -3` on a file
row and a summed count on a directory row.

### Phase 2 — Remove editor-open, add the in-memory open set

Tests: rewrite the editor-open tests in `src/cli/agent_tree.rs`.
- Space and Enter on a file add its path to the open set.
- Space and Enter on an already-open file remove it.
- Space and Enter on a directory still toggle expansion and touch no open set.
- Space/Enter on a **deleted** file now opens it (the old refusal test inverts).
- A file row renders an open marker when its path is in the set.

Code: `RenderState` gains `open_diffs: BTreeSet<String>`; the `Space | Enter`
arm dispatches on node kind as before but calls the toggle; delete
`open_selected`, the `Notice::editor` variant's editor usages, and the
`agent_tree_editor` import.

### Phase 3 — The all-files key

Tests: `a` on an empty set opens every changed file; `a` on a non-empty set
empties it; `a` reads the last good tree, so it still works while a notice is
showing; `a` with the cursor on a directory still acts on the whole tree.

### Phase 4 — Persisting the open set

Tests (new `src/agent_tree_open_set.rs`): round-trips a set through a temp
directory; a missing file reads as empty; a corrupt file reads as empty rather
than erroring (soft-fail decoding); the path is built under the admin dir.

Code: move `worktree_admin_dir` out of `src/dispatch/caller_identity.rs` into a
shared home and make it `pub(crate)` — it is now wanted by two subsystems.
`caller_identity` keeps its own `CONFIG_FILE` constant and its own placement
rule; only the directory resolver is shared.

### Phase 5 — The diff query

Tests: `git_file_diff` returns a body for a tracked file; returns
`DiffRefusal::Untracked` for a path in the untracked listing; returns
`Binary` when git says `Binary files ... differ`; returns `TooLarge` above
`DIFF_MAX_BYTES`; a git failure is an error, not a silent empty body.

### Phase 6 — The diff renderer

New `src/cli/agent_diff.rs` and a `dispatch agent-diff <task_id>` subcommand,
modelled on `src/cli/agent_tree.rs`: same 1-second tick, same `GIT_TIMEOUT`,
same notice/red-border treatment. It re-reads the open-set file every tick,
renders each open path's diff under a path heading in tree order, and truncates
lines at the pane width.

Tests: renders two files' diffs in tree order; renders each refusal's
placeholder; an open path git no longer reports contributes nothing; keys
`j`/`k`/`Ctrl-D`/`Ctrl-U`/`gg`/`G`/`q` behave as the tree's do.

### Phase 7 — tmux plumbing

Tests (`src/tmux.rs`): the diff split issues `split-window -v -d -l 66%` against
the **tree pane id** with no `-f`; the role marker written is the diff value.

Code: rename `PANE_ROLE_EDITOR` to `PANE_ROLE_DIFF`; drop `-f` from
`split_window_full_below_running` and rename it to say it subdivides its
target. Wire the tree's toggles to split when the set becomes non-empty and no
diff pane exists, and to kill the pane when the set empties.

### Phase 8 — Cleanup

Delete `src/agent_tree_editor.rs` and its `pub mod` line. Re-run
`./scripts/check-doc-symbols.sh` — it already flags
`agent_tree_editor_pane_percent` in that file, and deleting the file is the
fix. Update `docs/module-map.md` and `docs/architecture.md` where they describe
the editor pane.

## Verification

`cargo test`, `cargo clippy --all-targets -- -D warnings`, `cargo fmt`, and both
doc gate scripts. The real verify command comes from `get_task`, not from here.
