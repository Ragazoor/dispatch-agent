# 3739 — Tree-building logic: JSONL events → tree with Read/Modified badges

Subtask 3 of epic #272. Pure data-structure/algorithm module: given a task's
raw `file-events/<task_id>.jsonl` content (as a string) and the worktree root
path, build an in-memory tree whose touched nodes carry a badge and whose
ancestor directories are flagged for auto-expansion. No rendering, no real
filesystem I/O — everything is fed in as strings, matching the task's stated
scope and `docs/specs/agent-tree.allium`'s `AgentTreeNode` value type /
`RefreshAgentTree` rule (badge precedence, expansion-follows-touched-
descendants).

## Source of truth for the JSONL shape

Read directly from subtask 2's in-progress worktree
(`.worktrees/3738-.../src/file_events.rs`, not yet merged to `main`) since
that's the actual schema being produced, not just the design doc's summary.
One JSON object per line:

```json
{"schema_version":"1.0.0","timestamp":"2026-07-27T12:00:00Z","task_id":"42","tool":"read","path":"/abs/path","operation":"read"}
```

`tool` is snake_case (`read`/`write`/`edit`/`notebook_edit`); `operation` is
`read` or `modified`. Only `path` and `operation` matter for tree-building —
`schema_version`/`timestamp`/`task_id`/`tool` are ignored (serde skips unknown
fields by default, and an event's badge is fully determined by `operation`).

## Module

New top-level module `src/agent_tree.rs` (single file, following the
`tips.rs`/`notify.rs`/`plan.rs` pattern for small standalone modules),
registered in `src/lib.rs`.

```rust
pub enum FileOperation { Read, Modified }        // mirrors allium's FileOperation
pub enum TreeNodeKind { File, Directory }        // mirrors allium's TreeNodeKind

pub struct TreeNode {
    pub name: String,               // path segment (file/dir name), not full path
    pub kind: TreeNodeKind,
    pub badge: Option<FileOperation>,  // Some only for touched files; always None for directories
    pub expanded: bool,              // directories only; true iff a touched descendant exists
    pub children: Vec<TreeNode>,     // sorted by name; empty for files
}

pub fn build_tree(root: &Path, jsonl: &str) -> TreeNode
```

`build_tree` returns the root node (`kind: Directory`) representing `root`
itself. Only touched paths (and their ancestor directories) become part of
the tree — this subtask does not scan the real filesystem, so untouched
siblings never appear. (Subtask 4's renderer is expected to merge this with
an actual directory listing; out of scope here, matches "no I/O mocking
needed beyond feeding in strings.")

## Algorithm

1. **Parse.** Split `jsonl` on `\n`. Skip blank lines. For each remaining
   line, `serde_json::from_str::<RawFileEvent>(line)` where
   `RawFileEvent { path: String, operation: RawOperation }` and
   `RawOperation` is a `#[serde(rename_all = "snake_case")]` enum of
   `Read | Modified`. A parse error (invalid JSON, missing `path`, unknown
   `operation` value) is skipped, not propagated — logged at `tracing::warn!`
   and the loop continues. No panics, no `unwrap`/`expect` outside tests.

2. **Relativize + merge badges.** For each successfully parsed event:
   - `Path::new(&event.path).strip_prefix(root)` — component-wise, so
     `/repo2/x` does not falsely match root `/repo`. If it fails (event path
     is not under root) or yields zero components, the event contributes no
     node (dropped, matching the allium spec's `OutOfWorktreeTouches` open
     question's stated behavior: out-of-root touches are captured in the log
     but produce nothing in a worktree-rooted tree).
   - Otherwise, accumulate into `BTreeMap<Vec<OsString>, FileOperation>`
     keyed by the path's components, merging via:
     `merged = match (existing, new) { (Some(Modified), _) | (_, Modified) => Modified, _ => Read }`
     — i.e. Modified is sticky/dominant regardless of arrival order, which is
     exactly "Modified wins if both occurred" and subsumes latest-wins for
     same-operation duplicates (idempotent: Read+Read stays Read,
     Modified+Modified stays Modified).

3. **Build the trie.** Walk the merged map's keys (a `BTreeMap` iterates in
   sorted key order, which gives deterministic, alphabetically-grouped
   construction) and insert each component chain into a nested `TreeNode`
   structure rooted at `root`, creating intermediate `Directory` nodes
   (`badge: None`) as needed and a final `File` node carrying the resolved
   badge. Children of each directory are sorted by name for deterministic
   output/tests.

4. **Expansion pass.** After the trie is built, a bottom-up pass sets
   `expanded = true` on every directory that has at least one descendant
   file with `badge.is_some()` (always true here, since only touched files
   are in the tree at all — but computed generically via a recursive
   "does this subtree contain any file node" check, so the rule reads the
   same way the allium spec states it: `directory.expanded = has_touched_descendant`).
   Files always have `expanded: false`.

## Tests (write first)

In an inline `#[cfg(test)] mod tests` block in `src/agent_tree.rs`
(`#[allow(clippy::unwrap_used, clippy::expect_used)]` at the top per
convention):

- `modified_wins_when_read_then_modified` — same path, Read then Modified →
  final badge Modified.
- `modified_wins_when_modified_then_read` — same path, Modified then Read →
  final badge Modified (order-independence).
- `read_only_path_gets_read_badge`
- `repeated_reads_stay_read` — duplicate Read events collapse, no panic, no
  spurious state change.
- `repeated_modifieds_stay_modified`
- `malformed_json_line_is_skipped_not_panicking` — a line that isn't valid
  JSON at all, interleaved with valid lines before/after; valid lines still
  produce correct nodes.
- `missing_path_field_is_skipped`
- `unknown_operation_value_is_skipped`
- `blank_lines_are_skipped`
- `path_outside_root_is_dropped`
- `interleaved_events_for_different_paths_resolve_independently` — events for
  path A and B interleaved in the stream, each resolves to its own correct
  final badge, no cross-contamination.
- `directory_containing_touched_file_is_expanded`
- `nested_ancestor_directories_are_all_expanded` — a deeply nested touched
  file expands every ancestor directory, not just the immediate parent.
- `untouched_sibling_directory_does_not_appear` — building from an event
  under `root/a/b.rs` produces no node for a sibling `root/c/` that was never
  in an event.
- `empty_event_stream_produces_root_only`
- `children_are_sorted_by_name` — deterministic ordering check.

## Verification

`cargo test agent_tree` plus the full `cargo test && ./scripts/check-doc-paths.sh`
gate before wrap-up. No Allium spec changes expected — `agent-tree.allium`
already documents this behavior (`AgentTreeNode`, `RefreshAgentTree`'s badge
and expansion rules) from subtask 1; this subtask implements the pure
tree-building half of that rule, not the full `worktree_tree` filesystem
merge (subtask 4).
