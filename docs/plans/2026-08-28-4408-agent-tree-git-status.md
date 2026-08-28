# 4408 — agent-tree: git as the single source of file status

Date: 2026-08-28
Epic: #311 (Agent Tree)

## Problem

The companion pane's tree is a projection of a hook-driven event log
(`<data_dir>/file-events/<task_id>.jsonl`). That log records *tool calls*, not
*file changes*, so:

1. A deleted file shows as `[Modified]` — the log has no delete concept.
2. A file the agent opened with Edit but did not actually change (or changed and
   reverted) shows `[Modified]` forever. The log is append-only; nothing ever
   retracts a badge.
3. A file changed by a shell command (`cargo fmt`, `sed`, a script) shows
   nothing at all — the "Bash gap" already documented in the spec.

## Decision

**Git is the sole source of truth for the tree.** The event-log capture pipeline
is removed entirely.

| Question | Decision |
|---|---|
| Baseline | merge-base of `Task.base_branch` and `HEAD`, compared against the **working tree** — so committed and uncommitted changes both count |
| Badges | `Added` / `Modified` / `Deleted`. No `Read` badge |
| Nodes | exactly the paths git reports as changed, plus their ancestor directories |
| Renames | `--no-renames`: the old path is `Deleted`, the new path is `Added` |
| Refresh | poll git on the existing ~1s timer; rebuild only when the result changes |
| Git failure | keep the last good tree, red border + notice |
| Notices | *any* notice reddens the pane border (including the existing editor-open failure) |
| Enter on a `[Deleted]` file | refuse, with a notice — do not open an editor |

### Git commands

Two, both run with `-C <worktree>` and bounded by `SUBPROCESS_TIMEOUT`:

```
git diff --name-status --no-renames --merge-base <base_branch>
git ls-files --others --exclude-standard
```

The first covers tracked changes against the base, committed or not
(`A`/`M`/`D`/`T`/`C` status letters). The second lists untracked, non-ignored
files, which become `Added`. Ignored files never appear — that is the intended
consequence of git being the authority.

`--merge-base` requires git ≥ 2.30; the runtime already requires git on PATH.

### Why not keep Read

Read badges cannot come from git, so keeping them means keeping the whole
capture pipeline for the least valuable half of the feature. One source of
truth is worth more than the Read badge.

## Work

### Spec (`docs/specs/agent-tree.allium`)

- Delete the CAPTURE half: `TrackedFileTool`, `FileEvent`, `CaptureFileEvent`,
  the `BashGap` note, `file_events_subdir` config, and every invariant scoped to
  the log.
- Replace `FileOperation { read | modified }` with
  `FileChange { added | modified | deleted }`.
- Rewrite `RefreshAgentTree` to derive nodes from a `git_changes(root, base)`
  black box instead of `file_events_for`.
- Add rules/guarantees for: last-good-tree on git failure, red border while a
  notice shows, refusing to open a deleted file.
- Resolve the `TreeScanExclusions` open question — a git-derived tree needs no
  worktree traversal and no exclusion list.

### Tests first (`allium:propagate`)

- `src/agent_tree.rs` unit tests: parse `--name-status` output, badge mapping,
  rename → D+A, untracked → Added, ancestor directories, sorting, expansion.
- Soft-fail: malformed porcelain lines are skipped, not fatal.
- `src/cli/agent_tree.rs`: git failure keeps the previous tree and sets a
  notice; notice implies red border; Enter on a Deleted node sets a notice and
  returns `Continue`, not `OpenInEditor`.
- Snapshot test covering all three badges.

### Code

1. `src/agent_tree.rs` — replace `build_tree(root, jsonl)` with a builder over a
   parsed `Vec<(PathBuf, FileChange)>`; add the porcelain parser.
2. `src/cli/agent_tree.rs` — new `GitStatusSource` that runs the two commands
   via `ProcessRunner`; loop polls it, diffs the result, rebuilds on change;
   red border; deleted-file guard in `handle_key`.
3. `run()` reads `Task.base_branch` alongside `Task.worktree`.
4. Delete `src/file_events.rs`, `pub mod file_events` in `src/lib.rs`,
   `cmd_hook_file_event` + the `HookFileEvent` command variant in `src/main.rs`,
   the file-event block in `plugin/hooks/scripts/task-status-hook`, the
   `hook-file-event` assertions in `src/setup/hooks.rs`, and the
   `hook-file-event` line in `docs/reference.md`.

### Verify

`cargo test`, then `cargo clippy --all-targets -- -D warnings` and
`cargo fmt`.
