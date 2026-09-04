//! Pure tree-building logic for the agent file tree companion pane (see
//! `docs/specs/agent-tree.allium`'s `AgentTreeNode` value type and the
//! badge/expansion rules in `RefreshAgentTree`).
//!
//! Git is the sole source of truth here — see the spec's `AgentTreeIsGitDerived`
//! guarantee. This module owns two halves of that: parsing what git printed
//! ([`parse_name_status`], [`parse_untracked`]) and folding the result into a
//! tree of only the changed paths and their ancestor directories
//! ([`build_tree`]). Running git is the renderer's job (`src/cli/agent_tree.rs`);
//! nothing in this file touches the filesystem or spawns a process, which is
//! what keeps every rule below testable from a string literal.

use std::collections::BTreeMap;
use std::ffi::OsString;
use std::path::{Path, PathBuf};

/// What git says happened to a file, relative to this worktree's fork point
/// from the task's base branch. Doubles as the badge vocabulary — see the
/// spec's `FileChange` enum, which is deliberately one enum for both so a
/// badge cannot claim something git did not say.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum FileChange {
    Added,
    Modified,
    Deleted,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum TreeNodeKind {
    File,
    Directory,
}

/// How many lines one path gained and lost against the baseline — the spec's
/// `LineCounts` value type.
///
/// One struct rather than two `Option<u32>` fields because git either counts a
/// path or it does not: there is no answer where an addition count arrives
/// without a removal count. Modelling it this way makes half a count
/// unrepresentable instead of merely forbidden.
///
/// Both numbers may legitimately be zero. A permission-only change is a real
/// change that moved no lines, which is also why this is not collapsed into a
/// single signed total.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub struct LineCounts {
    pub added: u32,
    pub removed: u32,
}

impl LineCounts {
    /// Fold another path's counts into these. Used to sum a directory over its
    /// descendants — see [`build_tree`].
    fn merge(self, other: LineCounts) -> LineCounts {
        LineCounts {
            added: self.added.saturating_add(other.added),
            removed: self.removed.saturating_add(other.removed),
        }
    }
}

/// One changed file as git reported it, with a path relative to the pane root.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct GitFileChange {
    pub path: PathBuf,
    pub change: FileChange,
    /// `None` when git reported no counts for this path, which happens in
    /// exactly two cases and both are facts about the file rather than
    /// decisions of ours: the path is untracked, so a diff against the
    /// baseline cannot see it; or git reports it binary. Rendering `+0 -0`
    /// instead would read as "nothing changed in there", which is the one
    /// thing that is certainly false of a file the agent just wrote. See the
    /// spec's `UntrackedFilesHaveNoLineCounts`.
    pub counts: Option<LineCounts>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct TreeNode {
    pub name: String,
    pub kind: TreeNodeKind,
    pub badge: Option<FileChange>,
    pub expanded: bool,
    pub children: Vec<TreeNode>,
    /// For a file, that file's own counts. For a directory, the sum over the
    /// changed files beneath it, so a collapsed directory still says how much
    /// is inside it.
    ///
    /// The sum SKIPS descendants that have none rather than reading them as
    /// zero, and is itself `None` when no descendant has any — see the spec's
    /// `DirectoryCountsSumDescendants`.
    pub counts: Option<LineCounts>,
}

impl TreeNode {
    /// Resolve a chain of name segments, relative to this node, to the node it
    /// names — `None` if any segment matches no child. Segment chains are how
    /// the companion pane's tree widget identifies a node (see
    /// `build_tree_items` in `src/cli/agent_tree.rs`), so this is what turns a
    /// widget selection back into a `TreeNode`.
    ///
    /// An empty path resolves to `self`. Callers that need to distinguish "the
    /// root" from "nothing selected" must check for that themselves — the
    /// synthetic root is a `Directory`, so it is otherwise indistinguishable
    /// from a real directory selection.
    pub fn node_at<S: AsRef<str>>(&self, path: &[S]) -> Option<&TreeNode> {
        let mut current = self;
        for segment in path {
            current = current
                .children
                .iter()
                .find(|c| c.name == segment.as_ref())?;
        }
        Some(current)
    }
}

/// Map one of git's `--name-status` status letters onto a [`FileChange`].
///
/// Only the leading letter is consulted, so the score suffix git appends to
/// similarity-scored letters (`R100`, `C75`) does not need stripping first.
/// See the spec's `CollapsedGitStatusLetters` note for why three values are
/// enough: `T` (type change) is a modification as far as a file tree is
/// concerned, and `R`/`C` never arrive because rename detection is off.
///
/// An unrecognised letter yields `None` and the line is skipped — this repo's
/// soft-fail-decoding convention. One unparseable line must not blank the tree.
fn change_from_status(status: &str) -> Option<FileChange> {
    match status.chars().next()? {
        'A' => Some(FileChange::Added),
        'D' => Some(FileChange::Deleted),
        'M' | 'T' => Some(FileChange::Modified),
        _ => None,
    }
}

/// Parse the output of `git diff --name-status --no-renames -z <base>`: a flat
/// NUL-separated stream alternating status and path, paths relative to the
/// repository root.
///
/// `-z` is what makes this a plain split rather than a parser. Without it git
/// separates the pair with a tab and, worse, C-quotes any path containing a
/// non-ASCII byte — `src/é.rs` arrives as the literal `"src/\303\251.rs"`,
/// quotes and octal escapes included, which would render as that string and
/// then open nothing. With `-z` there is no quoting and no escaping to undo, at
/// any byte value, and a path may contain spaces, tabs or newlines without
/// ambiguity. Nothing here trims, for the same reason: a leading or trailing
/// space is part of the filename.
///
/// A trailing status with no path, and any status letter this build does not
/// recognise, are skipped rather than erroring — this repo's
/// soft-fail-decoding convention. One unreadable record must not blank the tree.
pub fn parse_name_status(output: &str) -> Vec<GitFileChange> {
    let mut changes = Vec::new();
    // The stream ends with a NUL, so the final split segment is empty; taking
    // status and path strictly in pairs ignores it without a special case.
    let mut fields = output.split('\0');
    while let Some(status) = fields.next() {
        if status.is_empty() {
            continue;
        }
        let Some(path) = fields.next().filter(|p| !p.is_empty()) else {
            tracing::warn!(status, "skipping trailing git diff status with no path");
            break;
        };
        let Some(change) = change_from_status(status) else {
            tracing::warn!(status, path, "skipping git diff record with unknown status");
            continue;
        };
        changes.push(GitFileChange {
            path: PathBuf::from(path),
            change,
            // Filled in from the separate numstat query by
            // [`attach_line_counts`]; this parser is told only what happened.
            counts: None,
        });
    }
    changes
}

/// Parse the output of `git ls-files --others --exclude-standard -z`: a
/// NUL-separated list of paths, every one of them a file that exists but that
/// git is not tracking, and so [`FileChange::Added`].
///
/// `-z` for the same two reasons as [`parse_name_status`]: no quoting of
/// non-ASCII paths, and no ambiguity about a path containing whitespace.
pub fn parse_untracked(output: &str) -> Vec<GitFileChange> {
    output
        .split('\0')
        .filter(|path| !path.is_empty())
        .map(|path| GitFileChange {
            path: PathBuf::from(path),
            change: FileChange::Added,
            // Never filled in, and not an omission: an untracked path is not in
            // the index, so the numstat query cannot see it at all.
            counts: None,
        })
        .collect()
}

/// Parse the output of `git diff --numstat --no-renames -z <baseline>`: one
/// NUL-terminated record per path, holding `<added>\t<removed>\t<path>`.
///
/// A path git could not count carries `-` in both number fields — that is how
/// git spells a binary diff — and yields no entry rather than a zero. The
/// caller cannot tell that apart from "git said nothing about this path", and
/// does not need to: both mean the row shows no counts.
///
/// `-z` for the same two reasons as [`parse_name_status`]: git does not C-quote
/// non-ASCII paths, and a path containing whitespace stays unambiguous. That
/// second reason is why the path is taken as *everything after the second tab*
/// rather than as a third whitespace-delimited field — a filename may contain a
/// tab, and splitting on every tab would truncate it.
///
/// An unreadable record is skipped, not guessed at, in line with this repo's
/// soft-fail-decoding convention. One bad record must not cost the caller the
/// counts of every other path.
pub fn parse_numstat(output: &str) -> BTreeMap<PathBuf, LineCounts> {
    let mut counts = BTreeMap::new();

    for record in output.split('\0').filter(|r| !r.is_empty()) {
        let mut fields = record.splitn(3, '\t');
        let (Some(added), Some(removed), Some(path)) =
            (fields.next(), fields.next(), fields.next())
        else {
            tracing::warn!(record, "skipping malformed git numstat record");
            continue;
        };
        if path.is_empty() {
            tracing::warn!(record, "skipping git numstat record with no path");
            continue;
        }
        // A binary diff. Not an error and not a zero — see the doc comment.
        if added == "-" || removed == "-" {
            continue;
        }
        let (Ok(added), Ok(removed)) = (added.parse::<u32>(), removed.parse::<u32>()) else {
            tracing::warn!(
                record,
                "skipping git numstat record with unparseable counts"
            );
            continue;
        };
        counts.insert(PathBuf::from(path), LineCounts { added, removed });
    }

    counts
}

/// Fold a numstat map into the change set, matching on path.
///
/// A change with no entry in the map keeps `None`. That covers both the
/// untracked paths — which the numstat query cannot see at all — and any
/// tracked path whose record was skipped, and the two are deliberately not
/// distinguished: the row shows no counts either way.
pub fn attach_line_counts(changes: &mut [GitFileChange], counts: &BTreeMap<PathBuf, LineCounts>) {
    for change in changes {
        if let Some(found) = counts.get(&change.path) {
            change.counts = Some(*found);
        }
    }
}

/// Build the changed-paths tree rooted at `root` from git's answer.
///
/// `root` supplies only the tree's display name; `changes` carry paths already
/// relative to it, because that is how git reports them. A path that escapes
/// the root (`..`, or an absolute path) contributes no node — git does not
/// produce such paths, and rejecting them keeps a malformed one from rendering
/// above the worktree.
///
/// A path can appear twice, because the two git queries overlap: `git rm
/// --cached foo` leaves `foo` reported as `D` by the diff *and* listed as
/// untracked. [`change_precedence`] resolves it, so which query ran first
/// cannot change a badge.
pub fn build_tree(root: &Path, changes: &[GitFileChange]) -> TreeNode {
    let mut badges: BTreeMap<Vec<OsString>, (FileChange, Option<LineCounts>)> = BTreeMap::new();

    for change in changes {
        let Some(components) = relative_components(&change.path) else {
            continue;
        };
        badges
            .entry(components)
            .and_modify(|(existing, existing_counts)| {
                if change_precedence(change.change) > change_precedence(*existing) {
                    *existing = change.change;
                }
                // Counts are merged independently of the badge, because the two
                // queries that collide on a path do not both supply them: only
                // the diff counts, and the untracked listing never does. Taking
                // the counted answer whichever query produced it is what keeps
                // the result independent of which ran first — the same property
                // `change_precedence` gives the badge.
                if existing_counts.is_none() {
                    *existing_counts = change.counts;
                }
            })
            .or_insert((change.change, change.counts));
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
        counts: None,
    };

    for (components, (badge, counts)) in badges {
        insert_path(&mut root_node, &components, badge, counts);
    }

    sort_children(&mut root_node);
    compute_expansion(&mut root_node);
    compute_counts(&mut root_node);

    root_node
}

/// Rank for resolving a path reported by both git queries. Higher wins.
///
/// `Added` beats `Deleted` because the two only collide when the file is on
/// disk but out of the index (`git rm --cached`), and "it is there" is the more
/// useful of the two true statements. `Deleted` beats `Modified` because a
/// deletion is the more consequential fact and the one a file tree exists to
/// show.
fn change_precedence(change: FileChange) -> u8 {
    match change {
        FileChange::Modified => 0,
        FileChange::Deleted => 1,
        FileChange::Added => 2,
    }
}

/// Split a git-reported path into name segments, or `None` if it is not a plain
/// relative path below the root.
///
/// Every segment must be `Component::Normal`: `..`, a leading `/`, a `./` and a
/// Windows prefix all reject the whole path, as does an empty result. Git emits
/// none of these — its paths are normalised and repo-relative — so this is a
/// fail-closed backstop, not a normaliser. Rejecting outright rather than
/// sanitising is the point: a path this does not recognise is one whose meaning
/// we cannot vouch for, and the pane's rooting at the worktree
/// (`TaskPaneRootIsTaskWorktree`) is what depends on getting it right.
///
/// Shared with [`crate::agent_tree_editor::open_in_editor`], which applies it to
/// the selection path before handing it to an editor — one guard, so the two
/// cannot drift apart on which components they consider safe.
pub(crate) fn relative_components(path: &Path) -> Option<Vec<OsString>> {
    use std::path::Component;

    // `collect` into `Option<Vec<_>>` short-circuits on the first `None`, so one
    // non-Normal component rejects the path.
    let components: Option<Vec<OsString>> = path
        .components()
        .map(|component| match component {
            Component::Normal(segment) => Some(OsString::from(segment)),
            _ => None,
        })
        .collect();
    components.filter(|c| !c.is_empty())
}

fn insert_path(
    node: &mut TreeNode,
    components: &[OsString],
    badge: FileChange,
    counts: Option<LineCounts>,
) {
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
            counts,
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
                counts: None,
            });
            node.children.len() - 1
        }
    };
    insert_path(&mut node.children[child_index], rest, badge, counts);
}

/// Give every directory node the sum of the counts beneath it, and return what
/// this node contributes to its own parent.
///
/// The sum SKIPS descendants with no counts rather than folding them in as
/// zero, and a directory whose descendants all lack counts is left with `None`.
/// A directory holding one modified file and three brand-new ones must not
/// report the modified file's numbers as though they were the whole story of
/// the directory, and `+0 -0` on a directory full of unstaged files would read
/// as "nothing changed in there" — which is the one thing that is certainly
/// false. See the spec's `DirectoryCountsSumDescendants`.
///
/// A file node's own counts are already in place from [`insert_path`] and are
/// left untouched: this pass only ever writes to directories.
fn compute_counts(node: &mut TreeNode) -> Option<LineCounts> {
    if node.kind == TreeNodeKind::File {
        return node.counts;
    }

    let mut total: Option<LineCounts> = None;
    for child in &mut node.children {
        if let Some(child_counts) = compute_counts(child) {
            total = Some(match total {
                Some(running) => running.merge(child_counts),
                None => child_counts,
            });
        }
    }
    node.counts = total;
    total
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
    let mut has_changed_descendant = false;
    for child in &mut node.children {
        if compute_expansion(child) {
            has_changed_descendant = true;
        }
    }
    node.expanded = has_changed_descendant;
    has_changed_descendant
}

#[cfg(test)]
#[allow(clippy::unwrap_used, clippy::expect_used)]
mod tests {
    use super::*;

    fn root() -> PathBuf {
        PathBuf::from("/repo")
    }

    fn changed(path: &str, change: FileChange) -> GitFileChange {
        GitFileChange {
            path: PathBuf::from(path),
            change,
            counts: None,
        }
    }

    fn modified(path: &str) -> GitFileChange {
        changed(path, FileChange::Modified)
    }

    fn added(path: &str) -> GitFileChange {
        changed(path, FileChange::Added)
    }

    fn counted(path: &str, change: FileChange, added: u32, removed: u32) -> GitFileChange {
        GitFileChange {
            path: PathBuf::from(path),
            change,
            counts: Some(LineCounts { added, removed }),
        }
    }

    /// Build a `--numstat -z` stream. Git emits one NUL-terminated record per
    /// path, the three fields inside it separated by tabs.
    fn numstat_stream(records: &[&str]) -> String {
        records.iter().map(|r| format!("{r}\0")).collect()
    }

    fn counts_of(node: &TreeNode, path: &[&str]) -> Option<LineCounts> {
        let segments: Vec<String> = path.iter().map(|s| (*s).to_owned()).collect();
        node.node_at(&segments).expect("node should exist").counts
    }

    // -- parse_numstat ----------------------------------------------------

    #[test]
    fn numstat_reads_added_and_removed_for_each_path() {
        let out = numstat_stream(&["12\t3\tsrc/foo.rs", "0\t7\tsrc/bar.rs"]);
        let parsed = parse_numstat(&out);

        assert_eq!(
            parsed.get(Path::new("src/foo.rs")),
            Some(&LineCounts {
                added: 12,
                removed: 3
            })
        );
        assert_eq!(
            parsed.get(Path::new("src/bar.rs")),
            Some(&LineCounts {
                added: 0,
                removed: 7
            })
        );
    }

    /// Git spells a binary diff `-\t-\t<path>`. That is an absence of counts,
    /// not a zero, and it must not become one.
    #[test]
    fn numstat_reports_no_counts_for_a_binary_file() {
        let out = numstat_stream(&["-\t-\tassets/logo.png"]);
        assert_eq!(parse_numstat(&out).get(Path::new("assets/logo.png")), None);
    }

    /// One unreadable record must not cost the caller the rest of the answer —
    /// the same soft-fail-decoding rule `parse_name_status` follows.
    #[test]
    fn numstat_skips_a_malformed_record_and_keeps_the_others() {
        let out = numstat_stream(&["not a record", "4\t1\tsrc/ok.rs", "x\ty\tsrc/bad.rs"]);
        let parsed = parse_numstat(&out);

        assert_eq!(
            parsed.get(Path::new("src/ok.rs")),
            Some(&LineCounts {
                added: 4,
                removed: 1
            })
        );
        assert_eq!(parsed.get(Path::new("src/bad.rs")), None);
        assert_eq!(parsed.len(), 1);
    }

    /// A path is every byte after the second tab, so a filename containing a
    /// space or a tab survives. `-z` is what makes that unambiguous.
    #[test]
    fn numstat_keeps_a_path_containing_whitespace_intact() {
        let out = numstat_stream(&["1\t0\tdocs/my notes.md"]);
        assert_eq!(
            parse_numstat(&out).get(Path::new("docs/my notes.md")),
            Some(&LineCounts {
                added: 1,
                removed: 0
            })
        );
    }

    // -- attach_line_counts -----------------------------------------------

    #[test]
    fn attaching_counts_matches_on_path() {
        let mut changes = vec![modified("src/foo.rs"), modified("src/bar.rs")];
        let counts = parse_numstat(&numstat_stream(&["12\t3\tsrc/foo.rs"]));

        attach_line_counts(&mut changes, &counts);

        assert_eq!(
            changes[0].counts,
            Some(LineCounts {
                added: 12,
                removed: 3
            })
        );
        // In the diff but absent from the numstat: rendered with no counts
        // rather than with a guessed zero.
        assert_eq!(changes[1].counts, None);
    }

    #[test]
    fn attaching_counts_leaves_an_untracked_path_alone() {
        let mut changes = vec![added("new.rs")];
        attach_line_counts(&mut changes, &parse_numstat(""));
        assert_eq!(changes[0].counts, None);
    }

    // -- counts on the tree -----------------------------------------------

    #[test]
    fn a_file_node_carries_its_own_counts() {
        let tree = build_tree(
            &root(),
            &[counted("src/foo.rs", FileChange::Modified, 12, 3)],
        );
        assert_eq!(
            counts_of(&tree, &["src", "foo.rs"]),
            Some(LineCounts {
                added: 12,
                removed: 3
            })
        );
    }

    #[test]
    fn an_untracked_file_node_has_no_counts() {
        let tree = build_tree(&root(), &[added("new.rs")]);
        assert_eq!(counts_of(&tree, &["new.rs"]), None);
    }

    /// Zero is a real answer — a permission-only change moves no lines — so it
    /// must survive as `Some(0, 0)` and not collapse into "no counts".
    #[test]
    fn zero_counts_are_kept_rather_than_read_as_absent() {
        let tree = build_tree(&root(), &[counted("mode.sh", FileChange::Modified, 0, 0)]);
        assert_eq!(
            counts_of(&tree, &["mode.sh"]),
            Some(LineCounts {
                added: 0,
                removed: 0
            })
        );
    }

    #[test]
    fn a_directory_node_sums_its_descendants() {
        let tree = build_tree(
            &root(),
            &[
                counted("src/a.rs", FileChange::Modified, 12, 3),
                counted("src/inner/b.rs", FileChange::Added, 5, 1),
            ],
        );

        assert_eq!(
            counts_of(&tree, &["src"]),
            Some(LineCounts {
                added: 17,
                removed: 4
            })
        );
        assert_eq!(
            counts_of(&tree, &["src", "inner"]),
            Some(LineCounts {
                added: 5,
                removed: 1
            })
        );
    }

    /// The whole reason the sum is over `Option`: an unstaged file contributes
    /// nothing, rather than contributing a zero that would drag the directory's
    /// total down to a number smaller than what is really in there.
    #[test]
    fn a_directory_sum_skips_descendants_that_have_no_counts() {
        let tree = build_tree(
            &root(),
            &[
                counted("src/a.rs", FileChange::Modified, 12, 3),
                added("src/brand_new.rs"),
            ],
        );

        assert_eq!(
            counts_of(&tree, &["src"]),
            Some(LineCounts {
                added: 12,
                removed: 3
            })
        );
    }

    #[test]
    fn a_directory_with_no_counted_descendants_has_no_counts() {
        let tree = build_tree(&root(), &[added("src/one.rs"), added("src/two.rs")]);
        assert_eq!(counts_of(&tree, &["src"]), None);
    }

    /// The `git rm --cached` collision: the diff reports the path with counts
    /// and the untracked listing reports it without. The counted answer is the
    /// informative one, and which query ran first must not decide it.
    #[test]
    fn a_path_reported_by_both_queries_keeps_the_counts_it_has() {
        let orders: [Vec<GitFileChange>; 2] = [
            vec![
                counted("foo.rs", FileChange::Deleted, 0, 9),
                added("foo.rs"),
            ],
            vec![
                added("foo.rs"),
                counted("foo.rs", FileChange::Deleted, 0, 9),
            ],
        ];
        for changes in orders {
            let tree = build_tree(&root(), &changes);
            assert_eq!(
                counts_of(&tree, &["foo.rs"]),
                Some(LineCounts {
                    added: 0,
                    removed: 9
                })
            );
        }
    }

    // -- parse_name_status ------------------------------------------------

    /// Build a `-z` name-status stream: NUL after every field, including the
    /// last, exactly as git emits it.
    fn nul_stream(fields: &[&str]) -> String {
        fields.iter().map(|f| format!("{f}\0")).collect()
    }

    #[test]
    fn name_status_maps_the_three_letters_to_the_three_changes() {
        let out = nul_stream(&["A", "src/new.rs", "M", "src/foo.rs", "D", "src/old.rs"]);
        let out = out.as_str();
        assert_eq!(
            parse_name_status(out),
            vec![
                changed("src/new.rs", FileChange::Added),
                changed("src/foo.rs", FileChange::Modified),
                changed("src/old.rs", FileChange::Deleted),
            ]
        );
    }

    /// A type change (file becomes a symlink, or the reverse) is a
    /// modification as far as a file tree is concerned — see the spec's
    /// CollapsedGitStatusLetters note.
    #[test]
    fn type_change_is_reported_as_modified() {
        assert_eq!(
            parse_name_status(&nul_stream(&["T", "src/link.rs"])),
            vec![changed("src/link.rs", FileChange::Modified)]
        );
    }

    /// Soft-fail decoding: one line this build cannot read must not cost the
    /// lines around it.
    #[test]
    fn unrecognised_status_letter_is_skipped_not_guessed() {
        let out = nul_stream(&["M", "keep.rs", "U", "conflicted.rs", "A", "also-keep.rs"]);
        let out = out.as_str();
        assert_eq!(
            parse_name_status(out),
            vec![
                changed("keep.rs", FileChange::Modified),
                changed("also-keep.rs", FileChange::Added),
            ]
        );
    }

    /// A truncated stream — a status with no path after it — ends the parse
    /// rather than pairing the status with whatever follows.
    #[test]
    fn trailing_status_without_a_path_is_skipped() {
        assert_eq!(
            parse_name_status(&nul_stream(&["M", "keep.rs", "D"])),
            vec![changed("keep.rs", FileChange::Modified)]
        );
    }

    #[test]
    fn empty_output_parses_to_nothing() {
        assert!(parse_name_status("").is_empty());
        assert!(parse_name_status("\0").is_empty());
    }

    /// `-z` means whitespace in a filename is just bytes: no quoting to undo,
    /// and nothing may trim it away. A leading or trailing space is part of the
    /// name, and a path can even contain a newline.
    #[test]
    fn whitespace_in_a_path_survives_parsing_verbatim() {
        assert_eq!(
            parse_name_status(&nul_stream(&["M", "docs/my notes.md"])),
            vec![changed("docs/my notes.md", FileChange::Modified)]
        );
        assert_eq!(
            parse_name_status(&nul_stream(&["M", " leading.rs"])),
            vec![changed(" leading.rs", FileChange::Modified)]
        );
        assert_eq!(
            parse_untracked(&nul_stream(&["trailing.rs "])),
            vec![changed("trailing.rs ", FileChange::Added)]
        );
        assert_eq!(
            parse_name_status(&nul_stream(&["A", "weird\nname.rs"])),
            vec![changed("weird\nname.rs", FileChange::Added)]
        );
    }

    /// Git C-quotes non-ASCII paths unless `-z` is used. With it, the real
    /// bytes arrive and the parser needs no unescaping.
    #[test]
    fn non_ascii_paths_arrive_unquoted() {
        assert_eq!(
            parse_name_status(&nul_stream(&["M", "src/é.rs"])),
            vec![changed("src/é.rs", FileChange::Modified)]
        );
        assert_eq!(
            parse_untracked(&nul_stream(&["docs/naïve.md"])),
            vec![changed("docs/naïve.md", FileChange::Added)]
        );
    }

    // -- parse_untracked --------------------------------------------------

    #[test]
    fn every_untracked_path_is_added() {
        assert_eq!(
            parse_untracked(&nul_stream(&["src/new.rs", "docs/draft.md"])),
            vec![
                changed("src/new.rs", FileChange::Added),
                changed("docs/draft.md", FileChange::Added),
            ]
        );
    }

    #[test]
    fn empty_untracked_output_parses_to_nothing() {
        assert!(parse_untracked("").is_empty());
        assert!(parse_untracked("\0").is_empty());
    }

    // -- build_tree: badges ------------------------------------------------

    /// The headline fix (task #4408, part 1): a deleted file is badged deleted,
    /// not modified.
    #[test]
    fn deleted_file_is_badged_deleted() {
        let tree = build_tree(&root(), &[changed("src/old.rs", FileChange::Deleted)]);
        let node = tree.node_at(&["src", "old.rs"]).expect("node exists");
        assert_eq!(node.badge, Some(FileChange::Deleted));
    }

    #[test]
    fn added_file_is_badged_added() {
        let tree = build_tree(&root(), &[changed("src/new.rs", FileChange::Added)]);
        let node = tree.node_at(&["src", "new.rs"]).expect("node exists");
        assert_eq!(node.badge, Some(FileChange::Added));
    }

    #[test]
    fn modified_file_is_badged_modified() {
        let tree = build_tree(&root(), &[changed("src/foo.rs", FileChange::Modified)]);
        let node = tree.node_at(&["src", "foo.rs"]).expect("node exists");
        assert_eq!(node.badge, Some(FileChange::Modified));
    }

    /// The headline fix (task #4408, part 2): git reporting nothing means the
    /// tree shows nothing. A file the agent opened but did not change is not a
    /// node at all, so it cannot be badged modified.
    #[test]
    fn a_path_git_does_not_report_gets_no_node() {
        let tree = build_tree(&root(), &[changed("src/foo.rs", FileChange::Modified)]);
        assert!(tree.node_at(&["src", "untouched.rs"]).is_none());
        assert!(tree.node_at(&["README.md"]).is_none());
    }

    /// The spec's EveryFileNodeIsBadged invariant: an unbadged file node is
    /// exactly the "touched but unchanged" state this design exists to remove.
    #[test]
    fn every_file_node_carries_a_badge() {
        let tree = build_tree(
            &root(),
            &[
                changed("a/b/c.rs", FileChange::Added),
                changed("a/d.rs", FileChange::Deleted),
                changed("e.rs", FileChange::Modified),
            ],
        );
        fn assert_badged(node: &TreeNode) {
            match node.kind {
                TreeNodeKind::File => assert!(node.badge.is_some(), "{} unbadged", node.name),
                TreeNodeKind::Directory => assert_eq!(node.badge, None, "{} badged", node.name),
            }
            for child in &node.children {
                assert_badged(child);
            }
        }
        assert_badged(&tree);
    }

    /// A rename reaches us as two independent entries because rename detection
    /// is off — see the spec's rationale under RefreshAgentTree.
    #[test]
    fn a_rename_renders_as_a_delete_and_an_add() {
        let tree = build_tree(
            &root(),
            &[
                changed("src/old.rs", FileChange::Deleted),
                changed("src/new.rs", FileChange::Added),
            ],
        );
        assert_eq!(
            tree.node_at(&["src", "old.rs"]).expect("old").badge,
            Some(FileChange::Deleted)
        );
        assert_eq!(
            tree.node_at(&["src", "new.rs"]).expect("new").badge,
            Some(FileChange::Added)
        );
    }

    /// Both git queries can name the same path — `git rm --cached foo` leaves
    /// `foo` deleted in the diff and listed as untracked. Precedence resolves
    /// it, and crucially does so regardless of which query ran first: swapping
    /// the two commands must not flip a badge.
    #[test]
    fn a_path_reported_by_both_queries_resolves_by_precedence_not_order() {
        for pair in [
            [
                changed("a.rs", FileChange::Deleted),
                changed("a.rs", FileChange::Added),
            ],
            [
                changed("a.rs", FileChange::Added),
                changed("a.rs", FileChange::Deleted),
            ],
        ] {
            let tree = build_tree(&root(), &pair);
            assert_eq!(
                tree.node_at(&["a.rs"]).expect("a.rs").badge,
                Some(FileChange::Added),
                "on disk but out of the index reads as Added; got {pair:?}"
            );
        }
    }

    /// The rest of the precedence order, asserted both ways round for the same
    /// order-independence reason.
    #[test]
    fn deleted_beats_modified_in_either_order() {
        for pair in [
            [
                changed("a.rs", FileChange::Modified),
                changed("a.rs", FileChange::Deleted),
            ],
            [
                changed("a.rs", FileChange::Deleted),
                changed("a.rs", FileChange::Modified),
            ],
        ] {
            let tree = build_tree(&root(), &pair);
            assert_eq!(
                tree.node_at(&["a.rs"]).expect("a.rs").badge,
                Some(FileChange::Deleted),
                "got {pair:?}"
            );
        }
    }

    // -- build_tree: structure --------------------------------------------

    #[test]
    fn directory_containing_a_changed_file_is_expanded_and_unbadged() {
        let tree = build_tree(&root(), &[changed("src/lib.rs", FileChange::Modified)]);
        let dir = tree.node_at(&["src"]).expect("dir exists");
        assert!(dir.expanded);
        assert_eq!(dir.kind, TreeNodeKind::Directory);
        assert_eq!(dir.badge, None);
    }

    #[test]
    fn nested_ancestor_directories_are_all_expanded() {
        let tree = build_tree(&root(), &[changed("a/b/c/d.rs", FileChange::Deleted)]);
        assert!(tree.expanded);
        assert!(tree.node_at(&["a"]).expect("a exists").expanded);
        assert!(tree.node_at(&["a", "b"]).expect("b exists").expanded);
        assert!(tree.node_at(&["a", "b", "c"]).expect("c exists").expanded);
        let file = tree.node_at(&["a", "b", "c", "d.rs"]).expect("file exists");
        assert!(!file.expanded);
        assert_eq!(file.badge, Some(FileChange::Deleted));
    }

    #[test]
    fn unchanged_sibling_directory_does_not_appear() {
        let tree = build_tree(&root(), &[changed("a/b.rs", FileChange::Modified)]);
        assert!(tree.node_at(&["c"]).is_none());
        assert_eq!(tree.children.len(), 1);
    }

    #[test]
    fn no_changes_produce_root_only() {
        let tree = build_tree(&root(), &[]);
        assert_eq!(tree.kind, TreeNodeKind::Directory);
        assert!(tree.children.is_empty());
        assert!(!tree.expanded);
    }

    #[test]
    fn children_are_sorted_by_name() {
        let tree = build_tree(
            &root(),
            &[
                changed("zebra.rs", FileChange::Modified),
                changed("apple.rs", FileChange::Added),
                changed("mango.rs", FileChange::Deleted),
            ],
        );
        let names: Vec<&str> = tree.children.iter().map(|c| c.name.as_str()).collect();
        assert_eq!(names, vec!["apple.rs", "mango.rs", "zebra.rs"]);
    }

    #[test]
    fn root_node_name_is_last_path_component() {
        let tree = build_tree(&PathBuf::from("/home/user/my-worktree"), &[]);
        assert_eq!(tree.name, "my-worktree");
    }

    /// Git never emits any of these, but a malformed one must not render a node
    /// above the worktree root. `./` is rejected outright rather than
    /// normalised away — the guard vouches for paths it recognises, it does not
    /// repair ones it does not.
    #[test]
    fn path_not_strictly_below_the_root_is_dropped() {
        let tree = build_tree(
            &root(),
            &[
                changed("../outside.rs", FileChange::Modified),
                changed("/absolute.rs", FileChange::Modified),
                changed("./relative.rs", FileChange::Modified),
                changed("inside.rs", FileChange::Modified),
            ],
        );
        let names: Vec<&str> = tree.children.iter().map(|c| c.name.as_str()).collect();
        assert_eq!(names, vec!["inside.rs"]);
    }
}
