//! Pure tree-building logic for the agent file tree companion pane (see
//! `docs/specs/agent-tree.allium`'s `AgentTreeNode` value type and the
//! badge/expansion rules in `RefreshAgentTree`).
//!
//! Given a task's raw `file-events/<task_id>.jsonl` content and the
//! worktree root it is rooted at, [`build_tree`] produces an in-memory tree
//! of only the touched paths (and their ancestor directories) — no real
//! filesystem access. Untouched siblings never appear; merging this with an
//! actual directory listing is the rendering subcommand's job (subtask 4),
//! not this module's.

use std::collections::BTreeMap;
use std::ffi::OsString;
use std::path::Path;

use serde::Deserialize;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum FileOperation {
    Read,
    Modified,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum TreeNodeKind {
    File,
    Directory,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct TreeNode {
    pub name: String,
    pub kind: TreeNodeKind,
    pub badge: Option<FileOperation>,
    pub expanded: bool,
    pub children: Vec<TreeNode>,
}

#[derive(Debug, Deserialize)]
struct RawFileEvent {
    path: String,
    operation: RawOperation,
}

#[derive(Debug, Deserialize, Clone, Copy)]
#[serde(rename_all = "snake_case")]
enum RawOperation {
    Read,
    Modified,
}

impl From<RawOperation> for FileOperation {
    fn from(op: RawOperation) -> Self {
        match op {
            RawOperation::Read => FileOperation::Read,
            RawOperation::Modified => FileOperation::Modified,
        }
    }
}

/// Build the touched-paths tree rooted at `root` from `jsonl` (one JSON
/// object per line, matching `<data_dir>/file-events/<task_id>.jsonl`).
///
/// Lines that fail to parse (invalid JSON, missing `path`, an unrecognised
/// `operation`) are skipped rather than causing a panic — this repo's
/// soft-fail-decoding convention. Events whose `path` does not lie under
/// `root` contribute no node (see the allium spec's `OutOfWorktreeTouches`
/// open question).
pub fn build_tree(root: &Path, jsonl: &str) -> TreeNode {
    let mut badges: BTreeMap<Vec<OsString>, FileOperation> = BTreeMap::new();

    for line in jsonl.lines() {
        let line = line.trim();
        if line.is_empty() {
            continue;
        }
        let event: RawFileEvent = match serde_json::from_str(line) {
            Ok(event) => event,
            Err(e) => {
                tracing::warn!(error = ?e, line, "skipping malformed file-event line");
                continue;
            }
        };

        let Ok(relative) = Path::new(&event.path).strip_prefix(root) else {
            continue;
        };
        let components: Vec<OsString> = relative.iter().map(OsString::from).collect();
        if components.is_empty() {
            continue;
        }

        let new_op: FileOperation = event.operation.into();
        badges
            .entry(components)
            .and_modify(|existing| {
                if new_op == FileOperation::Modified {
                    *existing = FileOperation::Modified;
                }
            })
            .or_insert(new_op);
    }

    let root_name = root
        .file_name()
        .map(|n| n.to_string_lossy().into_owned())
        .unwrap_or_else(|| root.to_string_lossy().into_owned());
    let mut root_node = TreeNode {
        name: root_name,
        kind: TreeNodeKind::Directory,
        badge: None,
        expanded: false,
        children: Vec::new(),
    };

    for (components, badge) in badges {
        insert_path(&mut root_node, &components, badge);
    }

    sort_children(&mut root_node);
    compute_expansion(&mut root_node);

    root_node
}

fn insert_path(node: &mut TreeNode, components: &[OsString], badge: FileOperation) {
    let Some((head, rest)) = components.split_first() else {
        return;
    };
    let name = head.to_string_lossy().into_owned();

    if rest.is_empty() {
        node.children.push(TreeNode {
            name,
            kind: TreeNodeKind::File,
            badge: Some(badge),
            expanded: false,
            children: Vec::new(),
        });
        return;
    }

    let child_index = match node.children.iter().position(|c| c.name == name) {
        Some(index) => index,
        None => {
            node.children.push(TreeNode {
                name,
                kind: TreeNodeKind::Directory,
                badge: None,
                expanded: false,
                children: Vec::new(),
            });
            node.children.len() - 1
        }
    };
    insert_path(&mut node.children[child_index], rest, badge);
}

fn sort_children(node: &mut TreeNode) {
    node.children.sort_by(|a, b| a.name.cmp(&b.name));
    for child in &mut node.children {
        sort_children(child);
    }
}

fn compute_expansion(node: &mut TreeNode) -> bool {
    if node.kind == TreeNodeKind::File {
        return node.badge.is_some();
    }
    let mut has_touched_descendant = false;
    for child in &mut node.children {
        if compute_expansion(child) {
            has_touched_descendant = true;
        }
    }
    node.expanded = has_touched_descendant;
    has_touched_descendant
}

#[cfg(test)]
#[allow(clippy::unwrap_used, clippy::expect_used)]
mod tests {
    use super::*;
    use std::path::PathBuf;

    fn root() -> PathBuf {
        PathBuf::from("/repo")
    }

    fn event(path: &str, operation: &str) -> String {
        format!(
            r#"{{"schema_version":"1.0.0","timestamp":"2026-07-27T12:00:00Z","task_id":"1","tool":"read","path":"{path}","operation":"{operation}"}}"#
        )
    }

    fn find<'a>(node: &'a TreeNode, path: &[&str]) -> Option<&'a TreeNode> {
        let mut current = node;
        for segment in path {
            current = current.children.iter().find(|c| c.name == *segment)?;
        }
        Some(current)
    }

    #[test]
    fn modified_wins_when_read_then_modified() {
        let jsonl = format!(
            "{}\n{}",
            event("/repo/src/lib.rs", "read"),
            event("/repo/src/lib.rs", "modified")
        );
        let tree = build_tree(&root(), &jsonl);
        let node = find(&tree, &["src", "lib.rs"]).expect("node exists");
        assert_eq!(node.badge, Some(FileOperation::Modified));
    }

    #[test]
    fn modified_wins_when_modified_then_read() {
        let jsonl = format!(
            "{}\n{}",
            event("/repo/src/lib.rs", "modified"),
            event("/repo/src/lib.rs", "read")
        );
        let tree = build_tree(&root(), &jsonl);
        let node = find(&tree, &["src", "lib.rs"]).expect("node exists");
        assert_eq!(node.badge, Some(FileOperation::Modified));
    }

    #[test]
    fn read_only_path_gets_read_badge() {
        let jsonl = event("/repo/README.md", "read");
        let tree = build_tree(&root(), &jsonl);
        let node = find(&tree, &["README.md"]).expect("node exists");
        assert_eq!(node.badge, Some(FileOperation::Read));
    }

    #[test]
    fn repeated_reads_stay_read() {
        let jsonl = format!(
            "{}\n{}\n{}",
            event("/repo/a.rs", "read"),
            event("/repo/a.rs", "read"),
            event("/repo/a.rs", "read")
        );
        let tree = build_tree(&root(), &jsonl);
        let node = find(&tree, &["a.rs"]).expect("node exists");
        assert_eq!(node.badge, Some(FileOperation::Read));
    }

    #[test]
    fn repeated_modifieds_stay_modified() {
        let jsonl = format!(
            "{}\n{}",
            event("/repo/a.rs", "modified"),
            event("/repo/a.rs", "modified")
        );
        let tree = build_tree(&root(), &jsonl);
        let node = find(&tree, &["a.rs"]).expect("node exists");
        assert_eq!(node.badge, Some(FileOperation::Modified));
    }

    #[test]
    fn malformed_json_line_is_skipped_not_panicking() {
        let jsonl = format!(
            "{}\nnot valid json at all\n{}",
            event("/repo/a.rs", "read"),
            event("/repo/b.rs", "modified")
        );
        let tree = build_tree(&root(), &jsonl);
        assert_eq!(
            find(&tree, &["a.rs"]).expect("a.rs exists").badge,
            Some(FileOperation::Read)
        );
        assert_eq!(
            find(&tree, &["b.rs"]).expect("b.rs exists").badge,
            Some(FileOperation::Modified)
        );
    }

    #[test]
    fn missing_path_field_is_skipped() {
        let jsonl = format!(
            r#"{{"operation":"read"}}
{}"#,
            event("/repo/a.rs", "read")
        );
        let tree = build_tree(&root(), &jsonl);
        assert_eq!(tree.children.len(), 1);
        assert_eq!(find(&tree, &["a.rs"]).expect("a.rs exists").name, "a.rs");
    }

    #[test]
    fn unknown_operation_value_is_skipped() {
        let jsonl = format!(
            "{}\n{}",
            event("/repo/a.rs", "deleted"),
            event("/repo/b.rs", "read")
        );
        let tree = build_tree(&root(), &jsonl);
        assert!(find(&tree, &["a.rs"]).is_none());
        assert!(find(&tree, &["b.rs"]).is_some());
    }

    #[test]
    fn blank_lines_are_skipped() {
        let jsonl = format!("\n{}\n\n", event("/repo/a.rs", "read"));
        let tree = build_tree(&root(), &jsonl);
        assert_eq!(tree.children.len(), 1);
    }

    #[test]
    fn path_outside_root_is_dropped() {
        let jsonl = event("/elsewhere/a.rs", "read");
        let tree = build_tree(&root(), &jsonl);
        assert!(tree.children.is_empty());
    }

    #[test]
    fn interleaved_events_for_different_paths_resolve_independently() {
        let jsonl = format!(
            "{}\n{}\n{}\n{}",
            event("/repo/a.rs", "read"),
            event("/repo/b.rs", "modified"),
            event("/repo/a.rs", "modified"),
            event("/repo/b.rs", "read")
        );
        let tree = build_tree(&root(), &jsonl);
        assert_eq!(
            find(&tree, &["a.rs"]).expect("a.rs exists").badge,
            Some(FileOperation::Modified)
        );
        assert_eq!(
            find(&tree, &["b.rs"]).expect("b.rs exists").badge,
            Some(FileOperation::Modified)
        );
    }

    #[test]
    fn directory_containing_touched_file_is_expanded() {
        let jsonl = event("/repo/src/lib.rs", "read");
        let tree = build_tree(&root(), &jsonl);
        let dir = find(&tree, &["src"]).expect("dir exists");
        assert!(dir.expanded);
        assert_eq!(dir.kind, TreeNodeKind::Directory);
        assert_eq!(dir.badge, None);
    }

    #[test]
    fn nested_ancestor_directories_are_all_expanded() {
        let jsonl = event("/repo/a/b/c/d.rs", "modified");
        let tree = build_tree(&root(), &jsonl);
        assert!(tree.expanded);
        assert!(find(&tree, &["a"]).expect("a exists").expanded);
        assert!(find(&tree, &["a", "b"]).expect("b exists").expanded);
        assert!(find(&tree, &["a", "b", "c"]).expect("c exists").expanded);
        let file = find(&tree, &["a", "b", "c", "d.rs"]).expect("file exists");
        assert!(!file.expanded);
        assert_eq!(file.badge, Some(FileOperation::Modified));
    }

    #[test]
    fn untouched_sibling_directory_does_not_appear() {
        let jsonl = event("/repo/a/b.rs", "read");
        let tree = build_tree(&root(), &jsonl);
        assert!(find(&tree, &["c"]).is_none());
        assert_eq!(tree.children.len(), 1);
    }

    #[test]
    fn empty_event_stream_produces_root_only() {
        let tree = build_tree(&root(), "");
        assert_eq!(tree.kind, TreeNodeKind::Directory);
        assert!(tree.children.is_empty());
        assert!(!tree.expanded);
    }

    #[test]
    fn children_are_sorted_by_name() {
        let jsonl = format!(
            "{}\n{}\n{}",
            event("/repo/zebra.rs", "read"),
            event("/repo/apple.rs", "read"),
            event("/repo/mango.rs", "read")
        );
        let tree = build_tree(&root(), &jsonl);
        let names: Vec<&str> = tree.children.iter().map(|c| c.name.as_str()).collect();
        assert_eq!(names, vec!["apple.rs", "mango.rs", "zebra.rs"]);
    }

    #[test]
    fn root_node_name_is_last_path_component() {
        let tree = build_tree(&PathBuf::from("/home/user/my-worktree"), "");
        assert_eq!(tree.name, "my-worktree");
    }
}
