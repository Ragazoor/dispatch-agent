//! `dispatch agent-tree <task_id>` — a small, standalone ratatui loop that
//! renders one task's changed-file tree (see `docs/specs/agent-tree.allium`'s
//! `AgentTreeCompanionPane` surface and `RefreshAgentTree` rule).
//!
//! Deliberately NOT part of the board TUI's `App`/message loop: this runs as its
//! own process in a tmux companion pane.
//!
//! Git is the sole source of truth for what the tree shows — see the spec's
//! `AgentTreeIsGitDerived` guarantee. This module owns the running of git
//! ([`git_changes`]) and the polling loop around it; parsing its output and
//! folding the result into a tree belong to `crate::agent_tree`. It does not
//! merge in a full worktree filesystem scan and never will: git already answers
//! the question a scan was meant to approximate, which is what resolved the
//! spec's old `TreeScanExclusions` question.

use std::collections::{BTreeSet, HashSet};
use std::io;
use std::path::{Path, PathBuf};
use std::time::Duration;

use anyhow::{anyhow, Context, Result};
use crossterm::event::{self, Event, KeyCode, KeyEvent, KeyEventKind, KeyModifiers};
use crossterm::execute;
use crossterm::terminal::{
    disable_raw_mode, enable_raw_mode, EnterAlternateScreen, LeaveAlternateScreen,
};
use ratatui::backend::{Backend, CrosstermBackend};
use ratatui::layout::Rect;
use ratatui::style::{Modifier, Style};
use ratatui::text::{Line, Span};
use ratatui::widgets::{Block, Borders};
use ratatui::{Frame, Terminal};
use tui_tree_widget::{Tree, TreeItem, TreeState};

use crate::agent_tree::{
    attach_line_counts, build_tree, parse_name_status, parse_numstat, parse_untracked, FileChange,
    GitFileChange, TreeNode, TreeNodeKind,
};
use crate::db::{Database, TaskRead};
use crate::models::TaskId;
use crate::process::{stderr_str, ProcessRunner, RealProcessRunner};
use crate::tui::ui::palette::{FG, GREEN, RED, YELLOW};

/// Redraw cadence — see `docs/specs/agent-tree.allium`'s
/// `config.agent_tree_refresh_interval`. Doubles as the crossterm event
/// poll timeout, so a key press and a plain timer tick share one wait.
pub(crate) const REFRESH_INTERVAL: Duration = Duration::from_secs(1);

/// How long ONE git command may run before it is killed and treated as a
/// failure — `config.agent_tree_git_timeout` in the spec.
///
/// Deliberately far below [`crate::process::SUBPROCESS_TIMEOUT`], which the
/// board's other git calls use. Those run on a worker while the TUI stays live;
/// these run inline in this loop, so the timeout bounds how long this pane can
/// ignore a keypress.
///
/// The bound is PER COMMAND, and a tick runs up to six of them
/// ([`git_changes`]), so the arithmetic worst case is six times this. Only
/// three of the six can realistically reach it: the two `diff`s and `ls-files`
/// touch the index, and a lock the agent's own git holds is by far the
/// commonest cause of a slow query. The three `merge-base` probes walk refs and
/// objects only and take no lock, so the practical ceiling is unchanged by the
/// baseline resolution.
pub(crate) const GIT_TIMEOUT: Duration = Duration::from_secs(5);

/// A one-line failure notice, tagged with which of the two writers set it.
/// Rendered in the pane's bottom border, and while one is set the whole border
/// is drawn red — see `AgentTreeNoticeRedensBorder`.
///
/// The tag is what lets a recovering git query clear its own stale notice
/// without also wiping the answer to a keypress the user made half a second ago
/// (see [`RenderState::clear_git_notice`] and the spec's `NoticeSource`).
/// Modelled as a variant rather than a field beside the text because the two
/// are only ever meaningful together — which is exactly what the spec's
/// `notice_source: NoticeSource when error_notice != null` says.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Notice {
    Git(String),
    /// A diff pane that could not be opened. The direct answer to a keypress
    /// the user made moments ago, which is why a recovering git query must not
    /// clear it — see [`RenderState::clear_git_notice`].
    Diff(String),
}

impl Notice {
    pub fn git(text: impl Into<String>) -> Self {
        Self::Git(text.into())
    }

    pub fn diff(text: impl Into<String>) -> Self {
        Self::Diff(text.into())
    }

    pub fn text(&self) -> &str {
        match self {
            Self::Git(text) | Self::Diff(text) => text,
        }
    }
}

/// The `+N -M` half of a row, or nothing when the node has no counts.
///
/// Absence is rendered as absence, never as `+0 -0`: a zero is a real answer
/// for a tracked file that moved no lines (a permission change), and printing
/// one for an untracked file — which git could not count at all — would read as
/// "nothing changed in there" about a file the agent has just written. See the
/// spec's `UntrackedFilesHaveNoLineCounts`.
fn count_spans(node: &TreeNode) -> Vec<Span<'static>> {
    let Some(counts) = node.counts else {
        return Vec::new();
    };
    vec![
        Span::raw(" "),
        Span::styled(format!("+{}", counts.added), Style::default().fg(GREEN)),
        Span::raw(" "),
        Span::styled(format!("-{}", counts.removed), Style::default().fg(RED)),
    ]
}

/// One rendered row: the name, then the badge if the node has one, then the
/// line counts if it has any.
///
/// A directory reaches the counts too. It carries no badge — git says nothing
/// about directories, and `OnlyFilesCarryBadges` holds that — but it does carry
/// the sum over everything beneath it, which is what lets a collapsed directory
/// say how much is inside without being opened.
/// Marks a file whose diff is open in the pane below. Rendered for every FILE
/// row, as the marker or as a blank of the same width, so the names stay in one
/// column whatever is open — a marker that shifted its neighbours would make
/// the set harder to read, not easier.
///
/// Directories never carry it: only files can be open (`OnlyFilesOpenDiffs`),
/// and giving them the blank as well would indent them out of line with the
/// files beneath them.
const DIFF_OPEN_MARKER: &str = "\u{25cf} ";
const DIFF_CLOSED_MARKER: &str = "  ";

fn node_label(node: &TreeNode, diff_open: bool) -> Line<'static> {
    let mut spans = Vec::new();
    if node.kind == TreeNodeKind::File {
        spans.push(Span::styled(
            if diff_open {
                DIFF_OPEN_MARKER
            } else {
                DIFF_CLOSED_MARKER
            },
            Style::default().fg(YELLOW),
        ));
    }
    spans.push(Span::styled(node.name.clone(), Style::default().fg(FG)));

    if let Some(change) = node.badge {
        let (badge, style) = match change {
            FileChange::Added => ("[Added]", Style::default().fg(GREEN)),
            FileChange::Modified => (
                "[Modified]",
                Style::default().fg(YELLOW).add_modifier(Modifier::BOLD),
            ),
            FileChange::Deleted => (
                "[Deleted]",
                Style::default().fg(RED).add_modifier(Modifier::BOLD),
            ),
        };
        spans.push(Span::raw(" "));
        spans.push(Span::styled(badge, style));
    }

    spans.extend(count_spans(node));
    Line::from(spans)
}

fn node_to_item(
    node: &TreeNode,
    path: &mut Vec<String>,
    open_diffs: &BTreeSet<PathBuf>,
) -> Option<TreeItem<'static, String>> {
    path.push(node.name.clone());
    // `path` now names this node relative to the root, which is exactly the
    // shape the open set holds — see `RenderState::open_diffs`.
    let diff_open = open_diffs.contains(&path.iter().collect::<PathBuf>());
    let label = node_label(node, diff_open);
    let item = match node.kind {
        TreeNodeKind::File => Some(TreeItem::new_leaf(node.name.clone(), label)),
        TreeNodeKind::Directory => {
            let children = to_items(&node.children, path, open_diffs);
            match TreeItem::new(node.name.clone(), label, children) {
                Ok(item) => Some(item),
                Err(e) => {
                    tracing::warn!(
                        error = ?e,
                        path = path.join("/"),
                        "skipping agent-tree node with duplicate child identifiers"
                    );
                    None
                }
            }
        }
    };
    path.pop();
    item
}

fn to_items(
    children: &[TreeNode],
    path: &mut Vec<String>,
    open_diffs: &BTreeSet<PathBuf>,
) -> Vec<TreeItem<'static, String>> {
    children
        .iter()
        .filter_map(|node| node_to_item(node, path, open_diffs))
        .collect()
}

/// Convert the changed-paths tree into `tui_tree_widget` items. The root node
/// itself is not rendered as a wrapping item — its children become the
/// top-level list, like a normal file browser.
///
/// A node is identified by its own name segment, which is all the widget
/// requires (identifiers must be unique among siblings only — it already
/// scopes lookups by the chain of ancestor identifiers). That makes a
/// node's widget key and its path segments the same `Vec<String>`, which is
/// exactly what `RenderState::sync_expansion` walks with. The `path`
/// accumulator survives only to give the duplicate-identifier warning
/// somewhere useful to point.
pub fn build_tree_items(
    root: &TreeNode,
    open_diffs: &BTreeSet<PathBuf>,
) -> Vec<TreeItem<'static, String>> {
    to_items(&root.children, &mut Vec::new(), open_diffs)
}

/// The commit where this worktree forked from `git_ref`, or git's own error if
/// the ref does not resolve.
fn merge_base(root: &str, git_ref: &str, runner: &dyn ProcessRunner) -> Result<String> {
    let sha = run_git(runner, &["-C", root, "merge-base", "HEAD", git_ref])?
        .trim()
        .to_string();
    // Git prints a commit id whenever it exits zero, so this is defensive
    // rather than reachable — but an empty string would be handed to `git diff`
    // as its baseline, where it means something else entirely. Soft-fail into
    // "this ref is not a candidate" instead.
    if sha.is_empty() {
        return Err(anyhow!("git: no common ancestor of HEAD and {git_ref}"));
    }
    Ok(sha)
}

/// Whether `ancestor` is an ancestor of `descendant`.
///
/// `merge-base --is-ancestor` answers with an exit code and no output: 0 for
/// yes, 1 for no. Any other code — or a git that could not be spawned, or one
/// that overran [`GIT_TIMEOUT`] — is not an answer, and is reported as a
/// failure rather than folded into "no".
///
/// That distinction is load-bearing. "No" keeps the LOCAL fork point, so
/// reading an unanswered probe as "no" would silently reinstate exactly the
/// mis-attribution `AgentTreeBaselineIsTaskBaseBranch` exists to forbid — and
/// present it as a correct tree, with no notice and no red border. A failure
/// instead reaches `AgentTreeGitFailureKeepsLastGoodTree`, which keeps the last
/// good tree and says so.
fn is_ancestor(
    root: &str,
    ancestor: &str,
    descendant: &str,
    runner: &dyn ProcessRunner,
) -> Result<bool> {
    let output = runner
        .run_with_timeout(
            "git",
            &[
                "-C",
                root,
                "merge-base",
                "--is-ancestor",
                ancestor,
                descendant,
            ],
            GIT_TIMEOUT,
        )
        .context("could not run git")?;
    match output.status.code() {
        Some(0) => Ok(true),
        Some(1) => Ok(false),
        _ => Err(git_error(&output)),
    }
}

/// Resolve the baseline the tree measures against: this worktree's fork point
/// from `base_branch` — step 1 of the spec's `AgentTreeGitQuery`.
///
/// A base branch is a NAME, and a repo can hold two refs under it: the local
/// branch and its remote-tracking counterpart. Dispatch branches a worktree
/// from whichever of the two is ahead at provision time
/// (`crate::dispatch::worktree`'s `select_start_point`), and the two drift in
/// BOTH directions during normal operation — a base branch the human has not
/// pulled leaves local behind, while a wrap-up that fast-forwards local without
/// pushing leaves it ahead. So each ref is probed and the fork point nearer
/// HEAD wins; see the spec's `AgentTreeBaselineIsTaskBaseBranch` for why
/// picking either ref unconditionally mis-attributes other people's commits to
/// the agent.
///
/// The remote ref comes from [`crate::git::origin_ref`], the crate's one
/// definition of it — deliberately the same one `select_start_point` reaches
/// through, so the two cannot disagree about which ref a worktree branched
/// from. In a repo whose remote is named anything else that ref never
/// resolves, and the baseline falls back to the local branch alone, with the
/// stale-branch mis-attribution that implies.
///
/// A ref that does not resolve is simply not a candidate: a base branch never
/// checked out locally is ordinary and must leave the pane working. Only when
/// NEITHER resolves is there no baseline, and then the LOCAL branch's error is
/// the one returned — that is the name the user put on the task, so it is the
/// one they can act on. A ranking probe that could not answer is a different
/// thing and fails the whole query; see [`is_ancestor`].
pub(crate) fn fork_point(
    root: &str,
    base_branch: &str,
    runner: &dyn ProcessRunner,
) -> Result<String> {
    let local = merge_base(root, base_branch, runner);
    let remote = merge_base(root, &crate::git::origin_ref(base_branch), runner);

    match (local, remote) {
        // Nearness is ancestry, not commit count. Equal fork points need no
        // ranking, so the probe is skipped — that is the ordinary case, where
        // the two refs agree. Where neither is an ancestor of the other the
        // refs have truly diverged, no choice is right, and the spec settles it
        // by fixed rule: the local ref wins.
        (Ok(local), Ok(remote)) => {
            if local != remote && is_ancestor(root, &local, &remote, runner)? {
                Ok(remote)
            } else {
                Ok(local)
            }
        }
        (Ok(local), Err(_)) => Ok(local),
        (Err(_), Ok(remote)) => Ok(remote),
        (Err(local_err), Err(_)) => Err(local_err),
    }
}

/// Run the git queries behind the tree and return everything they reported,
/// with paths relative to `root`.
///
/// The sequence is the spec's `AgentTreeGitQuery`:
///
///   1. [`fork_point`] — resolve the baseline from the task's base branch.
///   2. `git diff --name-status --no-renames -z <fork point>` — every tracked
///      change against that baseline, committed or not, because the diff is
///      taken against the WORKING TREE. An agent that commits mid-session does
///      not watch its work vanish.
///   3. `git diff --numstat --no-renames -z <fork point>` — how many lines
///      each of those paths gained and lost. A separate ask against the same
///      baseline and the same rename setting, not a richer form of step 2:
///      drift between the two would leave a row's badge and its numbers
///      answering different questions.
///   4. `git ls-files --others --exclude-standard -z` — files the agent created
///      and has not staged, which a diff cannot see. All of them are Added,
///      and none of them carries counts — step 3 cannot see a path that is not
///      in the index.
///
/// Rename detection is off (`--no-renames`): with it on a rename is one entry
/// naming two paths, which the three-value [`FileChange`] vocabulary cannot
/// express. Off, git reports the same rename as a delete plus an add — which is
/// both true and what a file tree should show.
///
/// `-z` on both path-emitting queries is load-bearing, not a style choice:
/// git's default output C-quotes any path containing a non-ASCII byte and
/// separates fields with a tab, so `src/é.rs` would arrive as
/// `"src/\303\251.rs"` and render as that literal string. See
/// [`parse_name_status`] for the full reasoning.
///
/// A path both path-listing queries name (`git rm --cached foo`) is resolved by
/// `build_tree` on precedence, not on the order the two run in — and so are its
/// counts, which the diff supplies and the untracked listing never does.
///
/// Every command is read-only: nothing here fetches, commits, stages or writes
/// to the index, which is what keeps the pane's `ReadOnlyObservation` guarantee
/// true while it runs git against a worktree an agent is actively using.
pub fn git_changes(
    root: &Path,
    base_branch: &str,
    runner: &dyn ProcessRunner,
) -> Result<Vec<GitFileChange>> {
    let root = root.to_string_lossy().into_owned();
    let baseline = fork_point(&root, base_branch, runner)?;

    let diff = run_git(
        runner,
        &[
            "-C",
            &root,
            "diff",
            "--name-status",
            "--no-renames",
            "-z",
            &baseline,
        ],
    )?;
    // The SAME baseline and the same rename setting as the diff above. The two
    // are one comparison asked twice, and if they ever drifted apart a row's
    // badge and its numbers would be answering different questions.
    let numstat = run_git(
        runner,
        &[
            "-C",
            &root,
            "diff",
            "--numstat",
            "--no-renames",
            "-z",
            &baseline,
        ],
    )?;
    let untracked = run_git(
        runner,
        &[
            "-C",
            &root,
            "ls-files",
            "--others",
            "--exclude-standard",
            "-z",
        ],
    )?;

    let mut changes = parse_name_status(&diff);
    attach_line_counts(&mut changes, &parse_numstat(&numstat));
    // Appended after the counts are attached, and deliberately: an untracked
    // path is not in the index, so the numstat query never saw it and there is
    // nothing to attach. See the spec's UntrackedFilesHaveNoLineCounts.
    changes.extend(parse_untracked(&untracked));
    Ok(changes)
}

/// Run one git command, returning its stdout or an error carrying git's own
/// first line of stderr — that line is what reaches the user's border, so it
/// has to say something they can act on ("unknown revision", "index.lock").
///
/// Stdout is returned untrimmed. [`crate::process::stdout_str`] trims the whole
/// buffer, which would eat a leading space off the first `-z` path; these two
/// commands emit NUL-delimited records where every byte between delimiters
/// belongs to the filename.
pub(crate) fn run_git(runner: &dyn ProcessRunner, args: &[&str]) -> Result<String> {
    let output = runner
        .run_with_timeout("git", args, GIT_TIMEOUT)
        .context("could not run git")?;
    if !output.status.success() {
        return Err(git_error(&output));
    }
    Ok(String::from_utf8_lossy(&output.stdout).into_owned())
}

/// Git's own first line of stderr, as an error. Shared by every caller here
/// because that line is what reaches the user's border and all of them need it
/// to say the same kind of thing.
fn git_error(output: &std::process::Output) -> anyhow::Error {
    let stderr = stderr_str(output);
    let detail = stderr
        .lines()
        .map(str::trim)
        .find(|line| !line.is_empty())
        .unwrap_or("git failed");
    anyhow!("git: {detail}")
}

/// Tree-widget navigation/expansion state, plus tracking of which
/// directories have already been auto-expanded once.
///
/// A directory's `expanded` flag (set by `agent_tree::build_tree`) is treated as
/// monotonic for auto-expansion purposes: `sync_expansion` opens a directory
/// automatically exactly once — the first rebuild where it holds a change — and
/// never forces it open again, so a user's manual collapse survives later
/// redraws. Unlike the event-log design this replaced, a directory CAN stop
/// being changed (the agent reverts its last edit there); if it later changes
/// again it is treated as newly changed and opens again, which is the right
/// behaviour and not worth a second set to prevent.
pub struct RenderState {
    pub tree_state: TreeState<String>,
    auto_expanded: HashSet<Vec<String>>,
    /// A one-line failure notice, rendered in the pane's bottom border and
    /// cleared by the next key press. Two things set it: a failed git query
    /// and a failed diff-pane split. While it is set the border is drawn red —
    /// see `AgentTreeNoticeRedensBorder` in docs/specs/agent-tree.allium.
    pub notice: Option<Notice>,
    /// Which files' diffs the user has opened, as paths relative to the pane
    /// root.
    ///
    /// The only state here the user builds up rather than git supplying:
    /// everything else the pane shows is re-derived from a git query every
    /// tick. A set rather than a cursor, because the diff pane shows every open
    /// file at once — which is what makes the all-files key a bulk version of
    /// the single-file toggle rather than a second mode.
    ///
    /// It holds PATHS, not nodes, so it survives a refresh that rebuilds every
    /// node. A path whose file git stops reporting simply stops matching
    /// anything; it is not pruned, because the agent may change the file again
    /// and the user did not ask for it to be closed. See
    /// `OpenDiffPathsMaySurviveTheirFiles` in docs/specs/agent-tree.allium.
    open_diffs: BTreeSet<PathBuf>,
    /// Whether a lone `g` is waiting for the second half of the `gg` chord.
    /// Unlike the board's chord this one carries no deadline, so there is no
    /// timestamp beside it — see `AgentTreeGgChordNeverExpires` in
    /// docs/specs/agent-tree.allium for why a clock would buy nothing here.
    pending_g: bool,
    /// Rows the tree had to draw into at the last render — the pane's height
    /// less its two borders. Recorded by [`render`] because the half-page
    /// motions are defined against the *visible* height, which only the
    /// renderer knows, and `handle_key` never sees a `Rect`.
    viewport_rows: usize,
}

impl RenderState {
    pub fn new() -> Self {
        Self {
            tree_state: TreeState::default(),
            auto_expanded: HashSet::new(),
            notice: None,
            open_diffs: BTreeSet::new(),
            pending_g: false,
            viewport_rows: 0,
        }
    }

    /// How far `Ctrl-D`/`Ctrl-U` move: half the last-rendered visible height,
    /// floored at one row. A pane too short to show two rows would otherwise
    /// halve to zero and turn both motions into no-ops, which reads as a
    /// broken key rather than a small pane.
    fn half_page(&self) -> usize {
        (self.viewport_rows / 2).max(1)
    }

    /// Clear a notice left by a failed git query, leaving an editor-open notice
    /// alone. Called after every SUCCESSFUL query: a working git retracts its
    /// own complaint, but must not swallow the answer to a keypress the user
    /// made moments ago (`RefreshAgentTree`'s clearing expression).
    /// The paths whose diffs are open, in tree order — which is `BTreeSet`'s
    /// own order, since a path sorts by its segments.
    pub fn open_diffs(&self) -> &BTreeSet<PathBuf> {
        &self.open_diffs
    }

    pub fn is_diff_open(&self, path: &Path) -> bool {
        self.open_diffs.contains(path)
    }

    /// Open `path`'s diff if it is closed, close it if it is open.
    fn toggle_diff(&mut self, path: PathBuf) {
        if !self.open_diffs.remove(&path) {
            self.open_diffs.insert(path);
        }
    }

    /// Open every changed file's diff, or close everything if anything is
    /// already open.
    ///
    /// Direction is decided by the set rather than by a remembered mode, so one
    /// press always has one meaning for the state the user can see. The paths
    /// come from the last rendered tree — the one the user is looking at —
    /// which is also why the key still works while a notice is showing and the
    /// pane is holding its last good answer.
    fn toggle_all_diffs(&mut self, root: &TreeNode) {
        if !self.open_diffs.is_empty() {
            self.open_diffs.clear();
            return;
        }
        self.open_diffs = collect_file_paths(root);
    }

    fn clear_git_notice(&mut self) {
        if matches!(self.notice, Some(Notice::Git(_))) {
            self.notice = None;
        }
    }

    /// Auto-open every directory holding a change, exactly once per directory —
    /// see the struct doc comment.
    pub fn sync_expansion(&mut self, root: &TreeNode) {
        self.sync_expansion_at(&root.children, &mut Vec::new());
    }

    /// `path` doubles as the widget's open-set key: it looks a node up by
    /// the chain of its ancestors' identifiers, and `node_to_item`
    /// identifies each node by its own name segment, so the two coincide.
    fn sync_expansion_at(&mut self, children: &[TreeNode], path: &mut Vec<String>) {
        for child in children {
            if child.kind != TreeNodeKind::Directory {
                continue;
            }
            path.push(child.name.clone());
            if child.expanded && !self.auto_expanded.contains(path) {
                self.tree_state.open(path.clone());
                self.auto_expanded.insert(path.clone());
            }
            self.sync_expansion_at(&child.children, path);
            path.pop();
        }
    }
}

impl Default for RenderState {
    fn default() -> Self {
        Self::new()
    }
}

/// Render the tree, with `[Added]`/`[Modified]`/`[Deleted]` badges, into `area`.
/// Does no I/O of its own — used by both the real polling loop and snapshot
/// tests. It is not, however, read-only in `state`: besides the widget's own
/// cursor bookkeeping it records `viewport_rows`, which the half-page motions
/// then read. A `handle_key` that has never been preceded by a `render` sees
/// the fallback height, so tests must draw before they press.
pub fn render(
    frame: &mut Frame,
    area: Rect,
    root: &TreeNode,
    state: &mut RenderState,
    title: &str,
) {
    let items = build_tree_items(root, &state.open_diffs);
    // The half-page motions are defined against the rows the user can actually
    // see, and this is the only place that number exists. Recorded on every
    // draw, so resizing the pane resizes the jump with no further plumbing.
    state.viewport_rows = usize::from(area.height.saturating_sub(2));
    let mut block = Block::default()
        .borders(Borders::ALL)
        .title(format!(" {title} "));
    // The bottom border is the pane's only place to say anything: the tree fills
    // the rest, and stealing a row for a status line would move every node the
    // moment a notice appeared.
    //
    // The whole border reddens with it. A single line of border text is easy to
    // miss, and a tree left on screen after a failed git query
    // (AgentTreeGitFailureKeepsLastGoodTree) is indistinguishable from a correct
    // one at a glance — the red frame is the part that carries across the room.
    if let Some(notice) = &state.notice {
        block = block
            .border_style(Style::default().fg(RED))
            .title_bottom(Line::from(Span::styled(
                format!(" {} ", notice.text()),
                Style::default().fg(RED).add_modifier(Modifier::BOLD),
            )));
    }

    match Tree::new(&items) {
        Ok(tree) => {
            let tree = tree
                .block(block)
                .highlight_style(Style::default().add_modifier(Modifier::REVERSED));
            frame.render_stateful_widget(tree, area, &mut state.tree_state);
        }
        Err(e) => {
            tracing::warn!(
                error = ?e,
                "agent-tree: duplicate identifiers building tree, rendering title only"
            );
            frame.render_widget(block, area);
        }
    }
}

/// What the event loop should do after `handle_key` has processed a key.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum KeyAction {
    /// Stay in the loop and redraw.
    Continue,
    /// Leave the loop, which exits the process and so closes the tmux pane.
    Exit,
    /// The set of open diffs moved. The loop reconciles the diff pane with it —
    /// splitting one when the set became non-empty and no pane exists, killing
    /// it when the set emptied. `handle_key` stays pure: every tmux call
    /// belongs to the loop.
    DiffSetChanged,
}

/// Every FILE path in the tree, relative to the root, in tree order.
///
/// Directories contribute their descendants but never themselves: a directory
/// has no contents of its own to diff, which is what `OnlyFilesOpenDiffs` in
/// docs/specs/agent-tree.allium says.
fn collect_file_paths(root: &TreeNode) -> BTreeSet<PathBuf> {
    fn walk(node: &TreeNode, prefix: &mut PathBuf, out: &mut BTreeSet<PathBuf>) {
        for child in &node.children {
            prefix.push(&child.name);
            match child.kind {
                TreeNodeKind::File => {
                    out.insert(prefix.clone());
                }
                TreeNodeKind::Directory => walk(child, prefix, out),
            }
            prefix.pop();
        }
    }

    let mut out = BTreeSet::new();
    walk(root, &mut PathBuf::new(), &mut out);
    out
}

/// The node the widget's current selection names, if any. A selection path is
/// exactly a node's chain of name segments below the root (see
/// `build_tree_items` and `sync_expansion_at`), so resolving it is
/// `TreeNode::node_at`.
///
/// Fails closed, and this is the one place that rule is stated — every key that
/// acts on the selection goes through here: `None` for an empty selection —
/// which `node_at` would otherwise resolve to the root, itself a directory —
/// and `None` for a path that resolves to nothing, i.e. a stale selection left
/// over from before a rebuild.
fn selected_node<'a>(root: &'a TreeNode, selected: &[String]) -> Option<&'a TreeNode> {
    if selected.is_empty() {
        return None;
    }
    root.node_at(selected)
}

/// Whether the selection is a directory, and so has something to open.
fn selected_is_directory(root: &TreeNode, selected: &[String]) -> bool {
    selected_node(root, selected).map(|node| node.kind) == Some(TreeNodeKind::Directory)
}

/// Apply one key press to the view state — see `docs/specs/agent-tree.allium`'s
/// `AgentTreeCompanionPane` surface for the bindings. One-step cursor and
/// expansion keys each have a vim motion and an arrow key bound to the same
/// action; the four jump motions (`gg`, `G`, `Ctrl-D`, `Ctrl-U`) are vim-only.
///
/// Space and Enter dispatch on the selected node's kind — a directory toggles, a
/// file opens — and expand is directory-only, which is why `root` is a
/// parameter: `tui_tree_widget`'s `open()` has no leaf guard, so without the tree
/// to consult, `l` on a *file* would record a phantom open that the next `h`
/// silently consumes instead of stepping out to the parent (#3834). Collapse
/// needs no guard — on a file it always falls through to stepping out.
///
/// Pure with respect to everything but `state`, so the loop's key handling is
/// testable without a terminal, an event source, or a tmux server.
pub fn handle_key(state: &mut RenderState, root: &TreeNode, key: KeyEvent) -> KeyAction {
    // Any key acknowledges a notice — docs/specs/agent-tree.allium's
    // ClearAgentTreeErrorNotice. Cleared before dispatching, so a key that sets
    // a fresh one wins.
    state.notice = None;

    // The `gg` chord, resolved before anything else so every other arm below
    // can assume no chord is in flight. Taking the flag disarms it
    // unconditionally: a second `g` completes the chord, and any other key
    // falls through to its own arm having quietly cancelled it. See
    // AgentTreeGgChordNeverExpires in docs/specs/agent-tree.allium — there is
    // no deadline, so the only thing that can end a pending chord is the next
    // key, whenever it comes.
    let was_pending_g = std::mem::take(&mut state.pending_g);
    if key.code == KeyCode::Char('g') && !key.modifiers.contains(KeyModifiers::CONTROL) {
        if was_pending_g {
            state.tree_state.select_first();
        } else {
            state.pending_g = true;
        }
        return KeyAction::Continue;
    }

    let half_page = state.half_page();
    // `TreeState`'s navigation methods return whether anything changed; the
    // loop redraws unconditionally, so the answer is discarded.
    match key.code {
        KeyCode::Char('q') => return KeyAction::Exit,
        KeyCode::Char('c') if key.modifiers.contains(KeyModifiers::CONTROL) => {
            return KeyAction::Exit
        }
        KeyCode::Char('k') | KeyCode::Up => {
            state.tree_state.key_up();
        }
        KeyCode::Char('j') | KeyCode::Down => {
            state.tree_state.key_down();
        }
        // Jump motions. All four resolve against the identifiers of the last
        // render — the visible rows — so a collapsed directory's children are
        // skipped and nothing is expanded to reach a target. `select_relative`
        // clamps its result to the last visible row for us; `saturating_sub`
        // clamps the other end. With nothing selected yet they all land on the
        // first row, matching what `j`/`k` already do from that state.
        KeyCode::Char('G') => {
            state.tree_state.select_last();
        }
        KeyCode::Char('d') if key.modifiers.contains(KeyModifiers::CONTROL) => {
            state
                .tree_state
                .select_relative(|current| current.map_or(0, |c| c.saturating_add(half_page)));
        }
        KeyCode::Char('u') if key.modifiers.contains(KeyModifiers::CONTROL) => {
            state
                .tree_state
                .select_relative(|current| current.map_or(0, |c| c.saturating_sub(half_page)));
        }
        KeyCode::Char('h') | KeyCode::Left => {
            state.tree_state.key_left();
        }
        // Expand carries a directory guard, so on a file it falls through to the
        // catch-all arm and does nothing at all.
        KeyCode::Char('l') | KeyCode::Right
            if selected_is_directory(root, state.tree_state.selected()) =>
        {
            state.tree_state.key_right();
        }
        // The all-files key. Unlike Space/Enter it does NOT dispatch on the
        // selection — it acts on the whole tree, whatever the cursor is on,
        // including a directory or nothing at all.
        KeyCode::Char('a') => {
            state.toggle_all_diffs(root);
            return KeyAction::DiffSetChanged;
        }
        // Space/Enter dispatch on the selected node's kind — one resolution, both
        // arms — so an unselectable or stale selection reaches neither.
        KeyCode::Char(' ') | KeyCode::Enter => {
            let selected = state.tree_state.selected();
            match selected_node(root, selected).map(|n| n.kind) {
                // No badge guard, and deliberately none. The editor this
                // replaced refused a node badged Deleted, because an editor
                // given a missing path opens a misleading empty buffer. A diff
                // has the opposite property: a deleted file's diff is exactly
                // its former contents, so deleted is the case where opening it
                // is most useful. See OpenAgentTreeFileDiff in
                // docs/specs/agent-tree.allium.
                Some(TreeNodeKind::File) => {
                    state.toggle_diff(selected.iter().collect());
                    return KeyAction::DiffSetChanged;
                }
                // `toggle_selected` reports whether anything changed; the loop
                // redraws unconditionally, so the answer is discarded.
                Some(TreeNodeKind::Directory) => {
                    state.tree_state.toggle_selected();
                }
                None => {}
            }
        }
        _ => {}
    }
    KeyAction::Continue
}

fn run_loop<B: Backend>(
    terminal: &mut Terminal<B>,
    root: &Path,
    base_branch: &str,
    runner: &dyn ProcessRunner,
) -> Result<()> {
    let mut state = RenderState::new();
    let mut tree = build_tree(root, &[]);
    // `build_tree` names the root node after the worktree directory, which is
    // exactly the pane title — so there is one basename computation, not two.
    let title = tree.name.clone();

    // Draw the empty tree BEFORE the first query. git runs inline in this
    // single-threaded loop, so a slow first query — a cold index, a large repo
    // — is time the pane has painted nothing and still shows whatever tmux left
    // in that cell. One frame of an empty bordered pane is a better answer than
    // a stale one; the query below fills it in immediately after.
    terminal.draw(|frame| render(frame, frame.area(), &tree, &mut state, &title))?;
    refresh(root, base_branch, runner, &mut tree, &mut state);

    loop {
        terminal.draw(|frame| render(frame, frame.area(), &tree, &mut state, &title))?;

        if event::poll(REFRESH_INTERVAL)? {
            let Event::Key(key) = event::read()? else {
                continue;
            };
            if key.kind != KeyEventKind::Press {
                continue;
            }
            match handle_key(&mut state, &tree, key) {
                KeyAction::Exit => return Ok(()),
                KeyAction::Continue => {}
                KeyAction::DiffSetChanged => {}
            }
            continue;
        }

        // Poll timed out with no key event: the ~1s timer tick.
        refresh(root, base_branch, runner, &mut tree, &mut state);
    }
}

/// One refresh pass: ask git, and rebuild only if the answer moved.
///
/// A failed query leaves `changes` and `tree` untouched and sets a notice — the
/// spec's `AgentTreeGitFailureKeepsLastGoodTree`. The commonest failure is a
/// transient index lock taken by the agent's own git commands, and blanking the
/// tree on that would make the pane flicker empty exactly when the user most
/// wants to watch it.
///
/// The unchanged-result short-circuit is a performance optimisation with one
/// behavioural consequence worth stating: the user's manual expansion state
/// survives a tick precisely because nothing is rebuilt on it.
fn refresh(
    root: &Path,
    base_branch: &str,
    runner: &dyn ProcessRunner,
    tree: &mut TreeNode,
    state: &mut RenderState,
) {
    match git_changes(root, base_branch, runner) {
        Ok(fresh) => {
            state.clear_git_notice();
            // Compared as TREES, not as change lists. The tree is what the user
            // sees, so it is the thing whose sameness matters — and two change
            // lists that differ only in a duplicate entry render identically,
            // which a list comparison would mistake for news.
            let rebuilt = build_tree(root, &fresh);
            if rebuilt != *tree {
                *tree = rebuilt;
                state.sync_expansion(tree);
            }
        }
        Err(e) => {
            tracing::warn!(
                root = %root.display(),
                base_branch,
                error = %e,
                "agent-tree: git query failed, keeping the last good tree"
            );
            // `{:#}`, not `{}`: anyhow's plain Display prints only the outermost
            // context, so a git that could not be spawned at all — or that
            // overran GIT_TIMEOUT — would put the bare word "git" in the
            // border and nothing else. Those are the two failures the user can
            // least afford to have unexplained.
            state.notice = Some(Notice::git(format!("{e:#}")));
        }
    }
}

/// Entry point for `dispatch agent-tree <task_id>`. Standalone ratatui loop
/// — not part of the board TUI's `App`/message loop (see the module-level
/// doc comment). Resolves the task's worktree and base branch from the DB once,
/// then re-queries git on a 1-second timer.
pub async fn run(db_path: &Path, task_id: i64) -> Result<()> {
    let database = Database::open(db_path).await?;
    let task = database
        .get_task(TaskId(task_id))
        .await?
        .with_context(|| format!("task {task_id} not found"))?;
    let base_branch = task.base_branch.clone();
    let worktree = task
        .worktree
        .with_context(|| format!("task {task_id} has no worktree"))?;
    let root = PathBuf::from(worktree);

    enable_raw_mode()?;
    let mut stdout = io::stdout();
    execute!(stdout, EnterAlternateScreen)?;
    let backend = CrosstermBackend::new(stdout);
    let mut terminal = Terminal::new(backend)?;

    let result = run_loop(
        &mut terminal,
        &root,
        &base_branch,
        &RealProcessRunner::default(),
    );

    disable_raw_mode()?;
    execute!(terminal.backend_mut(), LeaveAlternateScreen)?;
    terminal.show_cursor()?;

    result
}

#[cfg(test)]
#[allow(clippy::unwrap_used, clippy::expect_used)]
mod tests {
    use super::*;
    use crate::agent_tree::build_tree;
    use crate::agent_tree::LineCounts;
    use std::path::PathBuf;

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

    fn deleted(path: &str) -> GitFileChange {
        changed(path, FileChange::Deleted)
    }

    fn added(path: &str) -> GitFileChange {
        changed(path, FileChange::Added)
    }

    #[test]
    fn empty_tree_produces_no_items() {
        let tree = build_tree(&root(), &[]);
        let items = build_tree_items(&tree, &BTreeSet::new());
        assert!(items.is_empty());
    }

    #[test]
    fn changed_file_becomes_a_leaf_item_named_by_relative_path() {
        let tree = build_tree(&root(), &[modified("a.rs")]);
        let items = build_tree_items(&tree, &BTreeSet::new());
        assert_eq!(items.len(), 1);
        assert_eq!(items[0].identifier(), "a.rs");
        assert!(items[0].children().is_empty());
    }

    #[test]
    fn changed_dir_becomes_a_non_leaf_item() {
        let tree = build_tree(&root(), &[modified("src/a.rs")]);
        let items = build_tree_items(&tree, &BTreeSet::new());
        assert_eq!(items.len(), 1);
        assert_eq!(items[0].identifier(), "src");
        assert_eq!(items[0].children().len(), 1);
    }

    /// Each node is identified by its own name segment — the widget scopes
    /// lookups by ancestor chain, so sibling-uniqueness is all it needs.
    /// Keeping it a bare segment is what makes a node's widget key and its
    /// path segments the same vector (see `sync_expansion_at`).
    #[test]
    fn node_identifier_is_its_own_name_segment() {
        let tree = build_tree(&root(), &[modified("a/b/c.rs")]);
        let items = build_tree_items(&tree, &BTreeSet::new());
        let a = &items[0];
        assert_eq!(a.identifier(), "a");
        let b = &a.children()[0];
        assert_eq!(b.identifier(), "b");
        let c = &b.children()[0];
        assert_eq!(c.identifier(), "c.rs");
    }

    #[test]
    fn two_changed_roots_produce_two_top_level_items_sorted_by_name() {
        let tree = build_tree(&root(), &[modified("z.rs"), modified("a.rs")]);
        let items = build_tree_items(&tree, &BTreeSet::new());
        assert_eq!(items.len(), 2);
        assert_eq!(items[0].identifier(), "a.rs");
        assert_eq!(items[1].identifier(), "z.rs");
    }

    #[test]
    fn sync_expansion_opens_newly_changed_directory() {
        let tree = build_tree(&root(), &[modified("src/a.rs")]);
        let mut state = RenderState::new();
        state.sync_expansion(&tree);
        assert!(state.tree_state.opened().contains(&vec!["src".to_string()]));
    }

    #[test]
    fn sync_expansion_does_not_reopen_a_manually_closed_directory() {
        let changes = [modified("src/a.rs")];
        let tree = build_tree(&root(), &changes);
        let mut state = RenderState::new();
        state.sync_expansion(&tree);
        assert!(state.tree_state.close(&["src".to_string()]));

        // Rebuild the same tree (as a fresh poll with an unchanged answer
        // would) and sync again: "src" was already auto-expanded once, so the
        // manual close must survive.
        let tree_again = build_tree(&root(), &changes);
        state.sync_expansion(&tree_again);
        assert!(!state.tree_state.opened().contains(&vec!["src".to_string()]));
    }

    #[test]
    fn sync_expansion_opens_a_newly_changed_sibling_without_reopening_a_closed_one() {
        let mut state = RenderState::new();
        state.sync_expansion(&build_tree(&root(), &[modified("src/a.rs")]));
        assert!(state.tree_state.close(&["src".to_string()]));

        // A second poll picks up a brand-new change under a different directory.
        state.sync_expansion(&build_tree(
            &root(),
            &[modified("src/a.rs"), added("docs/b.md")],
        ));

        assert!(
            !state.tree_state.opened().contains(&vec!["src".to_string()]),
            "manually closed dir must stay closed"
        );
        assert!(
            state
                .tree_state
                .opened()
                .contains(&vec!["docs".to_string()]),
            "newly changed dir must auto-open"
        );
    }

    #[test]
    fn sync_expansion_opens_nested_ancestor_directories() {
        let tree = build_tree(&root(), &[modified("a/b/c.rs")]);
        let mut state = RenderState::new();
        state.sync_expansion(&tree);
        let opened = state.tree_state.opened();
        assert!(opened.contains(&vec!["a".to_string()]));
        assert!(opened.contains(&vec!["a".to_string(), "b".to_string()]));
    }

    use ratatui::backend::TestBackend;
    use ratatui::buffer::Buffer;
    use ratatui::Terminal;

    fn buffer_to_string(buf: &Buffer) -> String {
        let area = buf.area();
        let mut lines = Vec::with_capacity(area.height as usize);
        for y in area.top()..area.bottom() {
            let mut line = String::with_capacity(area.width as usize);
            for x in area.left()..area.right() {
                line.push_str(buf[(x, y)].symbol());
            }
            line.truncate(line.trim_end().len());
            lines.push(line);
        }
        lines.join("\n")
    }

    fn render_to_string(changes: &[GitFileChange], title: &str, width: u16, height: u16) -> String {
        let tree = build_tree(&root(), changes);
        let mut state = RenderState::new();
        state.sync_expansion(&tree);
        let mut terminal = Terminal::new(TestBackend::new(width, height)).expect("terminal");
        terminal
            .draw(|frame| render(frame, frame.area(), &tree, &mut state, title))
            .expect("draw");
        buffer_to_string(terminal.backend().buffer())
    }

    fn counted(path: &str, change: FileChange, added: u32, removed: u32) -> GitFileChange {
        GitFileChange {
            path: PathBuf::from(path),
            change,
            counts: Some(LineCounts { added, removed }),
        }
    }

    // ---- line counts on the rendered rows ---------------------------------

    #[test]
    fn a_file_row_shows_its_added_and_removed_line_counts() {
        let out = render_to_string(
            &[counted("a.rs", FileChange::Modified, 12, 3)],
            "task",
            60,
            8,
        );
        assert!(out.contains("+12"), "expected +12 in:\n{out}");
        assert!(out.contains("-3"), "expected -3 in:\n{out}");
    }

    /// A collapsed directory has to say how much is inside it, or the counts
    /// are only useful once the user has already opened everything.
    #[test]
    fn a_directory_row_shows_the_sum_of_its_descendants() {
        let out = render_to_string(
            &[
                counted("src/a.rs", FileChange::Modified, 12, 3),
                counted("src/b.rs", FileChange::Added, 5, 1),
            ],
            "task",
            60,
            10,
        );
        assert!(out.contains("+17"), "expected summed +17 in:\n{out}");
        assert!(out.contains("-4"), "expected summed -4 in:\n{out}");
    }

    /// An untracked file has no counts and must show none — not "+0 -0", which
    /// would read as "nothing changed in there" about a file the agent just
    /// wrote. See the spec's UntrackedFilesHaveNoLineCounts.
    #[test]
    fn an_untracked_file_row_shows_no_counts_at_all() {
        let out = render_to_string(&[added("brand_new.rs")], "task", 60, 8);
        assert!(out.contains("brand_new.rs"), "expected the row in:\n{out}");
        assert!(!out.contains("+0"), "expected no +0 in:\n{out}");
        assert!(!out.contains("-0"), "expected no -0 in:\n{out}");
    }

    /// Zero is a real answer for a TRACKED file — a permission-only change
    /// moves no lines — and is shown, unlike the absent counts above.
    #[test]
    fn a_tracked_file_with_no_moved_lines_still_shows_zeroes() {
        let out = render_to_string(
            &[counted("mode.sh", FileChange::Modified, 0, 0)],
            "task",
            60,
            8,
        );
        assert!(out.contains("+0"), "expected +0 in:\n{out}");
    }

    /// The badge still renders beside the counts. The two answer different
    /// questions — what happened, and how much of it — and a row needs both.
    #[test]
    fn counts_render_alongside_the_badge_not_instead_of_it() {
        let out = render_to_string(
            &[counted("a.rs", FileChange::Modified, 2, 1)],
            "task",
            60,
            8,
        );
        assert!(out.contains("[Modified]"), "expected the badge in:\n{out}");
        assert!(out.contains("+2"), "expected the counts in:\n{out}");
    }

    /// A rendered companion pane: `TreeState`'s cursor movement resolves
    /// against the identifiers captured by the *last render*, so a key test
    /// has to draw at least once before pressing anything.
    struct KeyRig {
        tree: TreeNode,
        state: RenderState,
        terminal: Terminal<TestBackend>,
    }

    impl KeyRig {
        fn new(changes: &[GitFileChange]) -> Self {
            Self::sized(changes, 12)
        }

        /// `height` is the whole pane, borders included, so the visible row
        /// count the half-page motions divide is `height - 2`.
        fn sized(changes: &[GitFileChange], height: u16) -> Self {
            let tree = build_tree(&root(), changes);
            let mut state = RenderState::new();
            state.sync_expansion(&tree);
            let terminal = Terminal::new(TestBackend::new(50, height)).expect("terminal");
            let mut rig = Self {
                tree,
                state,
                terminal,
            };
            rig.draw();
            rig
        }

        fn draw(&mut self) {
            let tree = &self.tree;
            let state = &mut self.state;
            self.terminal
                .draw(|frame| render(frame, frame.area(), tree, state, "dispatch"))
                .expect("draw");
        }

        /// Press a key, then redraw as the real loop does — so a following
        /// press sees the identifiers the new view actually rendered.
        fn press(&mut self, code: KeyCode) -> KeyAction {
            self.press_with(code, KeyModifiers::NONE)
        }

        /// Press a key with Ctrl held.
        fn press_ctrl(&mut self, code: KeyCode) -> KeyAction {
            self.press_with(code, KeyModifiers::CONTROL)
        }

        fn press_with(&mut self, code: KeyCode, modifiers: KeyModifiers) -> KeyAction {
            let action = handle_key(&mut self.state, &self.tree, KeyEvent::new(code, modifiers));
            self.draw();
            action
        }

        fn selected(&self) -> Vec<String> {
            self.state.tree_state.selected().to_vec()
        }

        /// The selected node's single name segment, for the flat-file trees the
        /// jump-motion tests use.
        fn selected_name(&self) -> String {
            self.selected().join("/")
        }

        fn is_open(&self, path: &[&str]) -> bool {
            let path: Vec<String> = path.iter().map(|s| (*s).to_string()).collect();
            self.state.tree_state.opened().contains(&path)
        }
    }

    /// Two top-level files plus a directory holding one file. Sorted by name,
    /// so the flattened view is: a.rs, src, src/lib.rs, z.rs.
    fn three_node_changes() -> Vec<GitFileChange> {
        vec![added("a.rs"), modified("src/lib.rs"), modified("z.rs")]
    }

    #[test]
    fn q_exits_the_renderer() {
        let mut rig = KeyRig::new(&[]);
        assert_eq!(rig.press(KeyCode::Char('q')), KeyAction::Exit);
    }

    #[test]
    fn ctrl_c_exits_the_renderer() {
        let mut state = RenderState::new();
        let tree = build_tree(&root(), &[]);
        let key = KeyEvent::new(KeyCode::Char('c'), KeyModifiers::CONTROL);
        assert_eq!(handle_key(&mut state, &tree, key), KeyAction::Exit);
    }

    #[test]
    fn a_bare_c_does_not_exit_the_renderer() {
        let mut rig = KeyRig::new(&[]);
        assert_eq!(rig.press(KeyCode::Char('c')), KeyAction::Continue);
    }

    #[test]
    fn down_and_j_both_move_the_cursor_down() {
        for code in [KeyCode::Down, KeyCode::Char('j')] {
            let mut rig = KeyRig::new(&three_node_changes());
            assert_eq!(rig.press(code), KeyAction::Continue);
            assert_eq!(rig.selected(), vec!["a.rs".to_string()], "{code:?}");
            rig.press(code);
            assert_eq!(rig.selected(), vec!["src".to_string()], "{code:?}");
        }
    }

    #[test]
    fn up_and_k_both_move_the_cursor_up() {
        for code in [KeyCode::Up, KeyCode::Char('k')] {
            let mut rig = KeyRig::new(&three_node_changes());
            rig.press(KeyCode::Down);
            rig.press(KeyCode::Down);
            assert_eq!(rig.selected(), vec!["src".to_string()], "{code:?}");
            assert_eq!(rig.press(code), KeyAction::Continue);
            assert_eq!(rig.selected(), vec!["a.rs".to_string()], "{code:?}");
        }
    }

    #[test]
    fn right_and_l_both_expand_the_selected_directory() {
        for code in [KeyCode::Right, KeyCode::Char('l')] {
            let mut rig = KeyRig::new(&three_node_changes());
            // "src" auto-expanded on first sync; collapse it so expanding is
            // an observable change.
            rig.press(KeyCode::Down);
            rig.press(KeyCode::Down);
            assert_eq!(rig.selected(), vec!["src".to_string()], "{code:?}");
            assert!(rig.state.tree_state.close(&["src".to_string()]));
            rig.draw();
            assert!(!rig.is_open(&["src"]), "{code:?}");

            assert_eq!(rig.press(code), KeyAction::Continue);
            assert!(rig.is_open(&["src"]), "{code:?}");
        }
    }

    #[test]
    fn left_and_h_both_collapse_the_selected_directory() {
        for code in [KeyCode::Left, KeyCode::Char('h')] {
            let mut rig = KeyRig::new(&three_node_changes());
            rig.press(KeyCode::Down);
            rig.press(KeyCode::Down);
            assert_eq!(rig.selected(), vec!["src".to_string()], "{code:?}");
            assert!(rig.is_open(&["src"]), "{code:?}");

            assert_eq!(rig.press(code), KeyAction::Continue);
            assert!(!rig.is_open(&["src"]), "{code:?}");
        }
    }

    #[test]
    fn h_on_a_child_moves_the_cursor_to_its_parent() {
        let mut rig = KeyRig::new(&three_node_changes());
        rig.press(KeyCode::Char('j'));
        rig.press(KeyCode::Char('j'));
        rig.press(KeyCode::Char('j'));
        // A node is identified by its own name segment, so a child's
        // selection path is [parent, child] — see `build_tree_items`.
        assert_eq!(
            rig.selected(),
            vec!["src".to_string(), "lib.rs".to_string()]
        );

        rig.press(KeyCode::Char('h'));
        assert_eq!(rig.selected(), vec!["src".to_string()]);
    }

    #[test]
    fn space_and_enter_both_toggle_the_selected_directory() {
        for code in [KeyCode::Char(' '), KeyCode::Enter] {
            let mut rig = KeyRig::new(&three_node_changes());
            rig.press(KeyCode::Char('j'));
            rig.press(KeyCode::Char('j'));
            assert_eq!(rig.selected(), vec!["src".to_string()], "{code:?}");

            assert_eq!(rig.press(code), KeyAction::Continue);
            assert!(!rig.is_open(&["src"]), "{code:?}");
            rig.press(code);
            assert!(rig.is_open(&["src"]), "{code:?}");
        }
    }

    /// No key expands a file — `tui_tree_widget`'s `open()` has no leaf guard, so
    /// an unguarded press on a file inserts that file's path into `opened()`
    /// (#3834). Nothing renders differently, which is exactly why this has to be
    /// asserted on the open set.
    ///
    /// Space/Enter now *open* a file rather than doing nothing, but that is an
    /// action for the loop to perform; the pane's own expansion state must still
    /// come out untouched, which is what this covers for all four keys. The
    /// returned action differs per key and is asserted by the tests above.
    ///
    /// Asserted over the *whole* set, not just the file's own path, so no
    /// sibling directory's expansion is disturbed either. One press per rig:
    /// `l` followed by Space on the same file would cancel out (open, then
    /// toggle closed) and pass even unguarded.
    #[test]
    fn no_key_expands_a_file_in_the_open_set() {
        for code in [
            KeyCode::Char(' '),
            KeyCode::Enter,
            KeyCode::Char('l'),
            KeyCode::Right,
        ] {
            let mut rig = KeyRig::new(&three_node_changes());
            rig.press(KeyCode::Char('j'));
            assert_eq!(rig.selected(), vec!["a.rs".to_string()], "{code:?}");
            let before = rig.state.tree_state.opened().clone();

            rig.press(code);

            assert_eq!(rig.state.tree_state.opened(), &before, "{code:?}");
        }
    }

    /// The user-visible regression: a phantom open on a file is consumed by the
    /// next `h`, so step-out silently needed two presses (#3834).
    #[test]
    fn h_after_space_on_a_file_steps_out_to_the_parent_in_one_press() {
        let mut rig = KeyRig::new(&three_node_changes());
        rig.press(KeyCode::Char('j'));
        rig.press(KeyCode::Char('j'));
        rig.press(KeyCode::Char('j'));
        assert_eq!(
            rig.selected(),
            vec!["src".to_string(), "lib.rs".to_string()]
        );
        rig.press(KeyCode::Char(' '));
        rig.press(KeyCode::Char('h'));
        assert_eq!(rig.selected(), vec!["src".to_string()]);
    }

    /// Space and Enter on a *file* open its diff. The action tells the loop
    /// the set moved; `handle_key` stays pure, so splitting a pane and asking
    /// git are the loop's job.
    #[test]
    fn space_and_enter_on_a_file_open_its_diff() {
        for code in [KeyCode::Char(' '), KeyCode::Enter] {
            let mut rig = KeyRig::new(&three_node_changes());
            rig.press(KeyCode::Char('j'));
            assert_eq!(rig.selected(), vec!["a.rs".to_string()], "{code:?}");

            assert_eq!(rig.press(code), KeyAction::DiffSetChanged, "{code:?}");
            assert!(rig.state.is_diff_open(Path::new("a.rs")), "{code:?}");
        }
    }

    /// The same key both ways — which is what makes it a toggle rather than an
    /// open with a separate close to remember.
    #[test]
    fn space_and_enter_on_an_open_file_close_its_diff() {
        for code in [KeyCode::Char(' '), KeyCode::Enter] {
            let mut rig = KeyRig::new(&three_node_changes());
            rig.press(KeyCode::Char('j'));
            rig.press(code);
            assert!(rig.state.is_diff_open(Path::new("a.rs")), "{code:?}");

            assert_eq!(rig.press(code), KeyAction::DiffSetChanged, "{code:?}");
            assert!(!rig.state.is_diff_open(Path::new("a.rs")), "{code:?}");
        }
    }

    #[test]
    fn opening_a_nested_file_records_its_whole_relative_path() {
        let mut rig = KeyRig::new(&three_node_changes());
        rig.press(KeyCode::Char('j'));
        rig.press(KeyCode::Char('j'));
        rig.press(KeyCode::Char('j'));
        assert_eq!(
            rig.selected(),
            vec!["src".to_string(), "lib.rs".to_string()]
        );

        assert_eq!(rig.press(KeyCode::Enter), KeyAction::DiffSetChanged);
        assert!(rig.state.is_diff_open(Path::new("src/lib.rs")));
    }

    /// The reversal from the editor this replaced, and the resolution of the
    /// spec's old ShowDeletedFileContent question. An editor given a deleted
    /// path opens a misleading empty buffer; a DIFF of a deleted file is
    /// exactly its former contents, so the deleted case is the one where
    /// opening it is most useful.
    #[test]
    fn space_and_enter_on_a_deleted_file_open_its_diff_too() {
        for code in [KeyCode::Char(' '), KeyCode::Enter] {
            let mut rig = KeyRig::new(&[deleted("gone.rs")]);
            rig.press(KeyCode::Char('j'));
            assert_eq!(rig.selected(), vec!["gone.rs".to_string()], "{code:?}");

            assert_eq!(rig.press(code), KeyAction::DiffSetChanged, "{code:?}");
            assert!(rig.state.is_diff_open(Path::new("gone.rs")), "{code:?}");
            assert!(rig.state.notice.is_none(), "{code:?}: no refusal expected");
        }
    }

    /// Every badge opens. There is no per-badge guard left anywhere in this
    /// path — see OpenAgentTreeFileDiff in docs/specs/agent-tree.allium.
    #[test]
    fn every_badge_opens_a_diff() {
        for change in [FileChange::Added, FileChange::Modified, FileChange::Deleted] {
            let mut rig = KeyRig::new(&[changed("a.rs", change)]);
            rig.press(KeyCode::Char('j'));
            assert_eq!(
                rig.press(KeyCode::Enter),
                KeyAction::DiffSetChanged,
                "{change:?}"
            );
            assert!(rig.state.is_diff_open(Path::new("a.rs")), "{change:?}");
        }
    }

    /// The directory behaviour is unchanged: Space/Enter still toggles
    /// expansion, and must not put a directory in the open set. See
    /// OnlyFilesOpenDiffs in docs/specs/agent-tree.allium.
    #[test]
    fn space_on_a_directory_toggles_expansion_and_opens_no_diff() {
        let mut rig = KeyRig::new(&three_node_changes());
        rig.press(KeyCode::Char('j'));
        rig.press(KeyCode::Char('j'));
        assert_eq!(rig.selected(), vec!["src".to_string()]);

        assert_eq!(rig.press(KeyCode::Char(' ')), KeyAction::Continue);
        assert!(!rig.is_open(&["src"]));
        assert!(rig.state.open_diffs().is_empty());
    }

    #[test]
    fn space_with_nothing_selected_does_nothing() {
        let mut rig = KeyRig::new(&three_node_changes());
        assert!(rig.selected().is_empty());
        assert_eq!(rig.press(KeyCode::Char(' ')), KeyAction::Continue);
        assert!(rig.state.open_diffs().is_empty());
    }

    // ---- the open marker on tree rows -------------------------------------

    /// Render the tree with `state` as it stands, rather than fresh — the
    /// marker is a function of the open set, which only a pressed key fills.
    fn render_rig(rig: &mut KeyRig) -> String {
        rig.draw();
        buffer_to_string(rig.terminal.backend().buffer())
    }

    #[test]
    fn an_open_files_row_carries_the_open_marker() {
        let mut rig = KeyRig::new(&three_node_changes());
        rig.press(KeyCode::Char('j'));
        rig.press(KeyCode::Char(' '));

        let out = render_rig(&mut rig);
        assert!(
            out.contains("\u{25cf} a.rs"),
            "expected the marker on a.rs in:\n{out}"
        );
    }

    #[test]
    fn a_closed_files_row_carries_no_marker() {
        let mut rig = KeyRig::new(&three_node_changes());
        let out = render_rig(&mut rig);
        assert!(
            !out.contains("\u{25cf}"),
            "expected no marker anywhere in:\n{out}"
        );
    }

    /// The marker follows the SET, so closing a file takes it away again —
    /// which is what NodeDiffOpenMatchesOpenSet asks of the rendered row.
    #[test]
    fn closing_a_diff_removes_its_marker() {
        let mut rig = KeyRig::new(&three_node_changes());
        rig.press(KeyCode::Char('j'));
        rig.press(KeyCode::Char(' '));
        rig.press(KeyCode::Char(' '));

        let out = render_rig(&mut rig);
        assert!(!out.contains("\u{25cf}"), "expected no marker in:\n{out}");
    }

    /// A directory can never be open, so it never carries the marker OR the
    /// blank that keeps file names in one column.
    #[test]
    fn a_directory_row_carries_no_marker_and_no_blank() {
        let mut rig = KeyRig::new(&three_node_changes());
        rig.press(KeyCode::Char('a'));

        let out = render_rig(&mut rig);
        assert!(
            !out.contains("\u{25cf} src"),
            "a directory must not be marked; got:\n{out}"
        );
        assert!(
            out.contains("\u{25cf} lib.rs"),
            "its file child must be; got:\n{out}"
        );
    }

    // ---- the all-files key ------------------------------------------------

    #[test]
    fn a_opens_every_changed_files_diff() {
        let mut rig = KeyRig::new(&three_node_changes());

        assert_eq!(rig.press(KeyCode::Char('a')), KeyAction::DiffSetChanged);

        assert!(rig.state.is_diff_open(Path::new("a.rs")));
        assert!(rig.state.is_diff_open(Path::new("src/lib.rs")));
    }

    /// Directories are routes to files, not things with contents, so `a` must
    /// not put one in the set — see OnlyFilesOpenDiffs in the spec.
    #[test]
    fn a_opens_no_directories() {
        let mut rig = KeyRig::new(&three_node_changes());
        rig.press(KeyCode::Char('a'));
        assert!(!rig.state.is_diff_open(Path::new("src")));
    }

    /// Direction is decided by the SET, not by a remembered mode: one press
    /// always empties a non-empty set, however it came to be non-empty.
    #[test]
    fn a_closes_everything_when_anything_is_open() {
        let mut rig = KeyRig::new(&three_node_changes());
        rig.press(KeyCode::Char('a'));
        assert!(!rig.state.open_diffs().is_empty());

        assert_eq!(rig.press(KeyCode::Char('a')), KeyAction::DiffSetChanged);
        assert!(rig.state.open_diffs().is_empty());
    }

    /// Even one file opened with Space is enough to make `a` mean "close",
    /// because the set is what decides and the set is what the user can see.
    #[test]
    fn a_closes_a_set_that_space_filled() {
        let mut rig = KeyRig::new(&three_node_changes());
        rig.press(KeyCode::Char('j'));
        rig.press(KeyCode::Char(' '));
        assert_eq!(rig.state.open_diffs().len(), 1);

        rig.press(KeyCode::Char('a'));
        assert!(rig.state.open_diffs().is_empty());
    }

    /// `a` does not dispatch on the selection at all, so it works with the
    /// cursor parked on a directory or on nothing.
    #[test]
    fn a_acts_on_the_whole_tree_whatever_the_cursor_is_on() {
        let mut rig = KeyRig::new(&three_node_changes());
        assert!(rig.selected().is_empty());
        rig.press(KeyCode::Char('a'));
        assert!(rig.state.is_diff_open(Path::new("src/lib.rs")));

        let mut rig = KeyRig::new(&three_node_changes());
        rig.press(KeyCode::Char('j'));
        rig.press(KeyCode::Char('j'));
        assert_eq!(rig.selected(), vec!["src".to_string()]);
        rig.press(KeyCode::Char('a'));
        assert!(rig.state.is_diff_open(Path::new("a.rs")));
    }

    /// On an empty tree there is nothing to open, and the press is harmless.
    #[test]
    fn a_on_an_empty_tree_opens_nothing() {
        let mut rig = KeyRig::new(&[]);
        assert_eq!(rig.press(KeyCode::Char('a')), KeyAction::DiffSetChanged);
        assert!(rig.state.open_diffs().is_empty());
    }

    /// `l`/`Right` are expansion keys only — they must NOT have picked up the
    /// open behaviour along with Space/Enter (#3834's guard stays a guard).
    #[test]
    fn l_and_right_on_a_file_still_do_nothing() {
        for code in [KeyCode::Char('l'), KeyCode::Right] {
            let mut rig = KeyRig::new(&three_node_changes());
            rig.press(KeyCode::Char('j'));
            assert_eq!(rig.press(code), KeyAction::Continue, "{code:?}");
        }
    }

    #[test]
    fn any_key_clears_a_pending_notice() {
        let mut rig = KeyRig::new(&three_node_changes());
        rig.state.notice = Some(Notice::diff("could not split the diff pane"));

        rig.press(KeyCode::Char('j'));

        assert!(rig.state.notice.is_none());
    }

    // ---- Jump motions: gg, G, Ctrl-D, Ctrl-U ------------------------------
    //
    // See the AgentTreeCompanionPane surface and the
    // AgentTreeGgChordNeverExpires guarantee in docs/specs/agent-tree.allium.

    /// `count` top-level files, `f01.rs`..`fNN.rs`. Flat and zero-padded, so
    /// the flattened view is exactly the files in that order and a landing row
    /// is nameable without counting directories.
    fn flat_file_changes(count: usize) -> Vec<GitFileChange> {
        (1..=count)
            .map(|n| modified(&format!("f{n:02}.rs")))
            .collect()
    }

    #[test]
    fn gg_jumps_to_the_first_visible_node() {
        let mut rig = KeyRig::new(&flat_file_changes(6));
        for _ in 0..4 {
            rig.press(KeyCode::Char('j'));
        }
        assert_ne!(
            rig.selected_name(),
            "f01.rs",
            "precondition: moved off row 0"
        );

        rig.press(KeyCode::Char('g'));
        assert_eq!(rig.press(KeyCode::Char('g')), KeyAction::Continue);
        assert_eq!(rig.selected_name(), "f01.rs");
    }

    #[test]
    fn a_lone_g_moves_nothing() {
        let mut rig = KeyRig::new(&flat_file_changes(6));
        rig.press(KeyCode::Char('j'));
        rig.press(KeyCode::Char('j'));
        let before = rig.selected();

        assert_eq!(rig.press(KeyCode::Char('g')), KeyAction::Continue);
        assert_eq!(rig.selected(), before, "a lone g must be swallowed");
    }

    /// The chord has no clock, so the only thing that can end it is another
    /// key — and that key must still do its own job.
    #[test]
    fn a_key_between_the_two_gs_disarms_the_chord_and_still_acts() {
        let mut rig = KeyRig::new(&flat_file_changes(6));
        rig.press(KeyCode::Char('j'));
        rig.press(KeyCode::Char('j'));

        rig.press(KeyCode::Char('g'));
        rig.press(KeyCode::Char('j'));
        assert_eq!(rig.selected_name(), "f03.rs", "the j must still move down");

        rig.press(KeyCode::Char('g'));
        assert_eq!(
            rig.selected_name(),
            "f03.rs",
            "the disarmed chord must not complete on the next lone g"
        );
    }

    #[test]
    fn g_then_a_second_g_after_many_other_keys_needs_a_fresh_pair() {
        let mut rig = KeyRig::new(&flat_file_changes(6));
        rig.press(KeyCode::Char('j'));
        rig.press(KeyCode::Char('g'));
        rig.press(KeyCode::Char('j'));
        rig.press(KeyCode::Char('j'));

        rig.press(KeyCode::Char('g'));
        rig.press(KeyCode::Char('g'));
        assert_eq!(rig.selected_name(), "f01.rs");
    }

    #[test]
    fn shift_g_jumps_to_the_last_visible_node() {
        let mut rig = KeyRig::new(&flat_file_changes(6));
        assert_eq!(rig.press(KeyCode::Char('G')), KeyAction::Continue);
        assert_eq!(rig.selected_name(), "f06.rs");
    }

    /// A jump lands on a *visible* row. Collapsing `src` hides `lib.rs`, so
    /// the last row becomes `src` itself — and the jump must not reopen it.
    #[test]
    fn shift_g_skips_rows_a_collapsed_directory_hides() {
        let mut rig = KeyRig::new(&[modified("a.rs"), modified("src/lib.rs")]);
        rig.press(KeyCode::Char('j'));
        rig.press(KeyCode::Char('j'));
        rig.press(KeyCode::Char('h'));
        assert!(!rig.is_open(&["src"]), "precondition: src is collapsed");

        rig.press(KeyCode::Char('G'));
        assert_eq!(rig.selected_name(), "src");
        assert!(!rig.is_open(&["src"]), "a jump must not expand anything");
    }

    #[test]
    fn gg_lands_on_the_first_row_without_expanding_it() {
        let mut rig = KeyRig::new(&[modified("src/lib.rs"), modified("z.rs")]);
        rig.press(KeyCode::Char('j'));
        rig.press(KeyCode::Char('h'));
        assert!(!rig.is_open(&["src"]), "precondition: src is collapsed");

        rig.press(KeyCode::Char('G'));
        rig.press(KeyCode::Char('g'));
        rig.press(KeyCode::Char('g'));
        assert_eq!(rig.selected_name(), "src");
        assert!(!rig.is_open(&["src"]), "a jump must not expand anything");
    }

    /// A 12-row pane has 10 visible rows, so half a page is 5. The leading `j`
    /// establishes a selection: with nothing selected a half-page motion just
    /// selects the first row, exactly as `j`/`k` do.
    #[test]
    fn ctrl_d_moves_the_cursor_half_a_page_down() {
        let mut rig = KeyRig::new(&flat_file_changes(20));
        rig.press(KeyCode::Char('j'));
        assert_eq!(rig.press_ctrl(KeyCode::Char('d')), KeyAction::Continue);
        assert_eq!(rig.selected_name(), "f06.rs");
        rig.press_ctrl(KeyCode::Char('d'));
        assert_eq!(rig.selected_name(), "f11.rs");
    }

    #[test]
    fn ctrl_u_moves_the_cursor_half_a_page_up() {
        let mut rig = KeyRig::new(&flat_file_changes(20));
        rig.press(KeyCode::Char('G'));
        assert_eq!(rig.selected_name(), "f20.rs");

        assert_eq!(rig.press_ctrl(KeyCode::Char('u')), KeyAction::Continue);
        assert_eq!(rig.selected_name(), "f15.rs");
    }

    #[test]
    fn ctrl_d_clamps_at_the_last_visible_node() {
        let mut rig = KeyRig::new(&flat_file_changes(6));
        rig.press(KeyCode::Char('j'));
        rig.press_ctrl(KeyCode::Char('d'));
        rig.press_ctrl(KeyCode::Char('d'));
        assert_eq!(rig.selected_name(), "f06.rs");
    }

    #[test]
    fn ctrl_u_clamps_at_the_first_visible_node() {
        let mut rig = KeyRig::new(&flat_file_changes(6));
        rig.press(KeyCode::Char('j'));
        rig.press_ctrl(KeyCode::Char('u'));
        assert_eq!(rig.selected_name(), "f01.rs");
    }

    /// The distance is half the pane's *current* height, not a constant: a
    /// 20-row pane (18 visible) jumps 9, where the default 12-row one jumps 5.
    #[test]
    fn half_a_page_scales_with_the_pane_height() {
        let mut tall = KeyRig::sized(&flat_file_changes(20), 20);
        tall.press(KeyCode::Char('j'));
        tall.press_ctrl(KeyCode::Char('d'));
        assert_eq!(tall.selected_name(), "f10.rs");

        let mut short = KeyRig::sized(&flat_file_changes(20), 8);
        short.press(KeyCode::Char('j'));
        short.press_ctrl(KeyCode::Char('d'));
        assert_eq!(short.selected_name(), "f04.rs");
    }

    /// A pane with a single visible row halves to zero. Zero is not a motion,
    /// so the floor is one row.
    #[test]
    fn a_pane_too_short_to_halve_still_moves_one_row() {
        let mut rig = KeyRig::sized(&flat_file_changes(6), 3);
        rig.press(KeyCode::Char('j'));
        rig.press_ctrl(KeyCode::Char('d'));
        assert_eq!(rig.selected_name(), "f02.rs");
        rig.press_ctrl(KeyCode::Char('u'));
        assert_eq!(rig.selected_name(), "f01.rs");
    }

    #[test]
    fn jump_motions_are_no_ops_on_an_empty_tree() {
        for keys in [vec!['g', 'g'], vec!['G']] {
            let mut rig = KeyRig::new(&[]);
            for key in keys {
                assert_eq!(rig.press(KeyCode::Char(key)), KeyAction::Continue);
            }
            assert!(rig.selected().is_empty());
        }

        let mut rig = KeyRig::new(&[]);
        assert_eq!(rig.press_ctrl(KeyCode::Char('d')), KeyAction::Continue);
        assert_eq!(rig.press_ctrl(KeyCode::Char('u')), KeyAction::Continue);
        assert!(rig.selected().is_empty());
    }

    /// `q` still exits with a chord armed — disarming must not swallow the key
    /// that did the disarming.
    #[test]
    fn q_after_a_lone_g_still_exits() {
        let mut rig = KeyRig::new(&flat_file_changes(6));
        rig.press(KeyCode::Char('g'));
        assert_eq!(rig.press(KeyCode::Char('q')), KeyAction::Exit);
    }

    // ---- git_changes: baseline resolution, diff, untracked listing --------

    use crate::process::MockProcessRunner;

    /// Fork point of HEAD with the LOCAL base branch, in every rig below.
    const LOCAL_FORK: &str = "1111111111111111111111111111111111111111";
    /// Fork point of HEAD with the REMOTE-TRACKING base ref.
    const REMOTE_FORK: &str = "2222222222222222222222222222222222222222";

    /// A `-z` stream: NUL after every field, exactly as git emits it.
    fn nul(fields: &[&str]) -> String {
        fields.iter().map(|f| format!("{f}\0")).collect()
    }

    /// One `git merge-base` answer, newline-terminated as git writes it.
    fn sha(commit: &str) -> Result<std::process::Output> {
        MockProcessRunner::ok_with_stdout(format!("{commit}\n").as_bytes())
    }

    /// The diff, its line counts, and the untracked listing, in call order.
    /// `diff` alternates status and path; `numstat` holds whole
    /// `added\tremoved\tpath` records; `untracked` is bare paths.
    fn changes_out_counted(
        diff: &[&str],
        numstat: &[&str],
        untracked: &[&str],
    ) -> Vec<Result<std::process::Output>> {
        vec![
            MockProcessRunner::ok_with_stdout(nul(diff).as_bytes()),
            MockProcessRunner::ok_with_stdout(nul(numstat).as_bytes()),
            MockProcessRunner::ok_with_stdout(nul(untracked).as_bytes()),
        ]
    }

    /// The common case: no line counts queued, so every path renders without
    /// them. Rigs that care about counts use [`changes_out_counted`].
    fn changes_out(diff: &[&str], untracked: &[&str]) -> Vec<Result<std::process::Output>> {
        changes_out_counted(diff, &[], untracked)
    }

    /// The two fork-point probes answering `local` and `remote`, then the diff
    /// and the untracked listing. Covers every rig whose probes need no
    /// ranking — either they agree, or one of them failed.
    fn probe_rig(
        local: Result<std::process::Output>,
        remote: Result<std::process::Output>,
        diff: &[&str],
        untracked: &[&str],
    ) -> MockProcessRunner {
        let mut queued = vec![local, remote];
        queued.extend(changes_out(diff, untracked));
        MockProcessRunner::new(queued)
    }

    /// Every git command succeeds, with both base refs agreeing on the fork
    /// point — the ordinary case, where no ancestry probe is needed.
    fn git_rig(diff: &[&str], untracked: &[&str]) -> MockProcessRunner {
        probe_rig(sha(LOCAL_FORK), sha(LOCAL_FORK), diff, untracked)
    }

    /// Neither base ref resolves, so the baseline cannot be found and the query
    /// fails before the diff. Nothing is queued past the two probes, so a third
    /// call would panic — which is what
    /// `a_failed_baseline_resolution_runs_no_further_commands` relies on.
    fn failing_git_rig(stderr: &str) -> MockProcessRunner {
        MockProcessRunner::new(vec![
            MockProcessRunner::fail(stderr),
            MockProcessRunner::fail(stderr),
        ])
    }

    /// The two probes disagree, and `local_is_ancestor` says which way. Git
    /// answers `merge-base --is-ancestor` with an exit code, not stdout: 0 for
    /// yes, 1 for no.
    fn diverged_rig(local_is_ancestor: bool, diff: &[&str]) -> MockProcessRunner {
        let verdict = if local_is_ancestor {
            MockProcessRunner::ok()
        } else {
            MockProcessRunner::fail_with_code(1, "")
        };
        let mut queued = vec![sha(LOCAL_FORK), sha(REMOTE_FORK), verdict];
        queued.extend(changes_out(diff, &[]));
        MockProcessRunner::new(queued)
    }

    /// The spec's AgentTreeGitQuery, in full: probe both refs the base branch
    /// name can denote, then diff the working tree against the fork point they
    /// agree on. Agreement is the ordinary case, and it costs no ancestry
    /// probe — there is nothing to rank.
    #[test]
    fn git_changes_probes_both_base_refs_then_diffs_from_the_fork_point() {
        let runner = git_rig(&["M", "src/a.rs"], &[]);
        let changes = git_changes(Path::new("/wt"), "main", &runner).expect("ok");

        assert_eq!(changes, vec![modified("src/a.rs")]);
        assert_eq!(
            runner.flattened_calls(),
            vec![
                "git -C /wt merge-base HEAD main".to_string(),
                "git -C /wt merge-base HEAD origin/main".to_string(),
                format!("git -C /wt diff --name-status --no-renames -z {LOCAL_FORK}"),
                format!("git -C /wt diff --numstat --no-renames -z {LOCAL_FORK}"),
                "git -C /wt ls-files --others --exclude-standard -z".to_string(),
            ]
        );
    }

    /// The counts query is a SEPARATE ask against the SAME baseline and the
    /// same rename setting. If the two ever drifted apart, a row's badge and
    /// its numbers would be answering different questions.
    #[test]
    fn git_changes_counts_lines_against_the_same_baseline_as_the_badges() {
        let mut queued = vec![sha(LOCAL_FORK), sha(LOCAL_FORK)];
        queued.extend(changes_out_counted(
            &["M", "src/a.rs"],
            &["12\t3\tsrc/a.rs"],
            &[],
        ));
        let runner = MockProcessRunner::new(queued);

        let changes = git_changes(Path::new("/wt"), "main", &runner).expect("ok");

        assert_eq!(
            changes[0].counts,
            Some(LineCounts {
                added: 12,
                removed: 3
            })
        );
    }

    /// An untracked file is invisible to a diff against the index, so it comes
    /// back with no counts however the query went. The pane must not fill that
    /// hole with a zero — see the spec's UntrackedFilesHaveNoLineCounts.
    #[test]
    fn an_untracked_path_comes_back_with_no_line_counts() {
        let mut queued = vec![sha(LOCAL_FORK), sha(LOCAL_FORK)];
        queued.extend(changes_out_counted(&[], &[], &["brand_new.rs"]));
        let runner = MockProcessRunner::new(queued);

        let changes = git_changes(Path::new("/wt"), "main", &runner).expect("ok");

        assert_eq!(changes, vec![added("brand_new.rs")]);
        assert_eq!(changes[0].counts, None);
    }

    /// The bug this resolution exists for. A base branch the human has not
    /// pulled in weeks leaves the LOCAL ref behind its remote, while the
    /// worktree was branched from the remote one. Measuring from the local
    /// fork point would badge every upstream commit since as the agent's work.
    ///
    /// Local fork point is an ancestor of the remote one, so the remote wins.
    #[test]
    fn a_local_base_behind_its_remote_diffs_from_the_remote_fork_point() {
        let runner = diverged_rig(true, &["M", "src/a.rs"]);
        git_changes(Path::new("/wt"), "main", &runner).expect("ok");

        let calls = runner.flattened_calls();
        assert_eq!(
            calls[2],
            format!("git -C /wt merge-base --is-ancestor {LOCAL_FORK} {REMOTE_FORK}")
        );
        assert_eq!(
            calls[3],
            format!("git -C /wt diff --name-status --no-renames -z {REMOTE_FORK}")
        );
    }

    /// The mirror image, and dispatch's own default: wrap-up fast-forwards the
    /// local base branch without pushing, so the local ref is AHEAD and the
    /// worktree was branched from it. Preferring the remote ref unconditionally
    /// would mis-attribute in exactly the same way.
    ///
    /// The same exit code covers the case where the two refs have truly
    /// diverged and neither fork point is an ancestor of the other: the spec
    /// settles that one by fixed rule, and the rule is that the local ref wins.
    #[test]
    fn a_local_base_ahead_of_its_remote_diffs_from_the_local_fork_point() {
        let runner = diverged_rig(false, &["M", "src/a.rs"]);
        git_changes(Path::new("/wt"), "main", &runner).expect("ok");

        assert_eq!(
            runner.flattened_calls()[3],
            format!("git -C /wt diff --name-status --no-renames -z {LOCAL_FORK}")
        );
    }

    /// A base branch the human never checked out locally is ordinary — it is
    /// the case dispatch's own start-point selection calls normal. The pane
    /// must keep working on the remote ref alone, not fail.
    #[test]
    fn a_missing_local_base_branch_still_resolves_from_the_remote_ref() {
        let runner = probe_rig(
            MockProcessRunner::fail("fatal: Not a valid object name main\n"),
            sha(REMOTE_FORK),
            &["M", "src/a.rs"],
            &[],
        );

        let changes = git_changes(Path::new("/wt"), "main", &runner).expect("ok");
        assert_eq!(changes, vec![modified("src/a.rs")]);
        assert_eq!(
            runner.flattened_calls()[2],
            format!("git -C /wt diff --name-status --no-renames -z {REMOTE_FORK}"),
            "one candidate needs no ranking, so no ancestry probe runs"
        );
    }

    /// The other half: a repo with no remote-tracking ref for the base branch
    /// — a purely local base, or a remote never fetched — resolves from the
    /// local branch alone.
    #[test]
    fn a_missing_remote_base_ref_still_resolves_from_the_local_branch() {
        let runner = probe_rig(
            sha(LOCAL_FORK),
            MockProcessRunner::fail("fatal: Not a valid object name origin/main\n"),
            &["M", "src/a.rs"],
            &[],
        );

        let changes = git_changes(Path::new("/wt"), "main", &runner).expect("ok");
        assert_eq!(changes, vec![modified("src/a.rs")]);
        assert_eq!(
            runner.flattened_calls()[2],
            format!("git -C /wt diff --name-status --no-renames -z {LOCAL_FORK}")
        );
    }

    /// Git answers `--is-ancestor` with exit 0 or 1 and nothing else. Any
    /// other exit means it did not answer at all, and a probe that did not
    /// answer must not be read as "no" — "no" keeps the LOCAL fork point, which
    /// is exactly the mis-attribution AgentTreeBaselineIsTaskBaseBranch exists
    /// to forbid, and it would be shown as a correct tree with no red border.
    /// Fail the query instead, so the failure rule fires.
    #[test]
    fn an_ancestry_probe_that_cannot_answer_fails_the_query() {
        let runner = MockProcessRunner::new(vec![
            sha(LOCAL_FORK),
            sha(REMOTE_FORK),
            MockProcessRunner::fail_with_code(128, "fatal: unable to read index.lock\n"),
        ]);
        let err = git_changes(Path::new("/wt"), "main", &runner)
            .expect_err("must fail")
            .to_string();
        assert!(err.contains("index.lock"), "got {err}");
        assert_eq!(
            runner.recorded_calls().len(),
            3,
            "nothing may run after a baseline we could not rank"
        );
    }

    /// A probe that exits zero but says nothing is not a baseline. An empty
    /// string handed to `git diff` means something else entirely, so the ref is
    /// soft-failed out of the running instead — here leaving the remote one to
    /// answer alone.
    #[test]
    fn a_probe_that_returns_no_commit_is_not_a_candidate() {
        let runner = probe_rig(
            MockProcessRunner::ok_with_stdout(b"\n"),
            sha(REMOTE_FORK),
            &["M", "src/a.rs"],
            &[],
        );

        let changes = git_changes(Path::new("/wt"), "main", &runner).expect("ok");
        assert_eq!(changes, vec![modified("src/a.rs")]);
        assert_eq!(
            runner.flattened_calls()[2],
            format!("git -C /wt diff --name-status --no-renames -z {REMOTE_FORK}")
        );
    }

    /// Only when NEITHER ref resolves is there no baseline, and only then does
    /// the query fail.
    #[test]
    fn neither_base_ref_resolving_fails_the_query() {
        let runner = failing_git_rig("fatal: Not a valid object name nosuchbranch\n");
        let err = git_changes(Path::new("/wt"), "nosuchbranch", &runner)
            .expect_err("must fail")
            .to_string();
        assert!(err.contains("nosuchbranch"), "got {err}");
    }

    /// Git C-quotes any path with a non-ASCII byte, and separates the status
    /// from the path with a tab, unless `-z` is passed — so `src/é.rs` would
    /// arrive as the literal `"src/\303\251.rs"`, a name that renders wrong and
    /// opens nothing. Both path-emitting queries must pass it; the fork-point
    /// probes emit commit ids, which have no such problem.
    #[test]
    fn both_path_emitting_queries_ask_for_nul_delimited_output() {
        let runner = git_rig(&[], &[]);
        git_changes(Path::new("/wt"), "main", &runner).expect("ok");
        for call in &runner.flattened_calls()[2..] {
            assert!(
                call.split(' ').any(|arg| arg == "-z"),
                "without -z, quoting breaks non-ASCII names; got {call}"
            );
        }
    }

    /// The payoff of the flag above: a non-ASCII path survives end to end, from
    /// git's stdout to a node the tree can name.
    #[test]
    fn a_non_ascii_path_survives_parsing_and_tree_building() {
        let runner = git_rig(&["M", "src/é.rs"], &["docs/naïve.md"]);
        let changes = git_changes(Path::new("/wt"), "main", &runner).expect("ok");
        assert_eq!(changes, vec![modified("src/é.rs"), added("docs/naïve.md")]);

        let tree = build_tree(&root(), &changes);
        assert_eq!(
            tree.node_at(&["src", "é.rs"]).expect("é.rs").badge,
            Some(FileChange::Modified)
        );
        assert_eq!(
            tree.node_at(&["docs", "naïve.md"]).expect("naïve.md").badge,
            Some(FileChange::Added)
        );
    }

    /// Every query is bounded, so a git blocked on an index lock the agent
    /// itself holds cannot wedge the renderer's single-threaded loop. The
    /// fork-point probes are queries like any other and are bounded too — the
    /// baseline resolution must not become an unbounded hole in that promise.
    #[test]
    fn every_git_query_is_bounded_by_a_timeout() {
        let runner = diverged_rig(true, &[]);
        git_changes(Path::new("/wt"), "main", &runner).expect("ok");
        assert_eq!(runner.recorded_timeouts(), vec![Some(GIT_TIMEOUT); 6]);
    }

    /// The diff is taken against the working tree, so a committed change is
    /// still reported. That is the whole reason the baseline is the fork point
    /// rather than HEAD — see AgentTreeBaselineIsTaskBaseBranch.
    ///
    /// Both probes are built from the task's own base branch name, so a task
    /// based on anything but `main` is measured against what it actually
    /// branched from.
    #[test]
    fn git_changes_uses_the_tasks_own_base_branch() {
        let runner = git_rig(&[], &[]);
        git_changes(Path::new("/wt"), "develop", &runner).expect("ok");
        assert_eq!(
            &runner.flattened_calls()[..2],
            [
                "git -C /wt merge-base HEAD develop".to_string(),
                "git -C /wt merge-base HEAD origin/develop".to_string(),
            ]
        );
    }

    #[test]
    fn git_changes_reports_untracked_files_as_added() {
        let runner = git_rig(&["M", "a.rs"], &["new.rs", "docs/draft.md"]);
        let changes = git_changes(Path::new("/wt"), "main", &runner).expect("ok");
        assert_eq!(
            changes,
            vec![modified("a.rs"), added("new.rs"), added("docs/draft.md")]
        );
    }

    #[test]
    fn git_changes_reports_deletions() {
        let runner = git_rig(&["D", "src/old.rs"], &[]);
        let changes = git_changes(Path::new("/wt"), "main", &runner).expect("ok");
        assert_eq!(changes, vec![deleted("src/old.rs")]);
    }

    #[test]
    fn git_changes_on_a_clean_worktree_reports_nothing() {
        let runner = git_rig(&[], &[]);
        assert!(git_changes(Path::new("/wt"), "main", &runner)
            .expect("ok")
            .is_empty());
    }

    /// A failing git surfaces its own first stderr line, because that line is
    /// what reaches the user's border and has to say something actionable.
    /// With two probes to fail, the line the user sees is the LOCAL branch's —
    /// that is the name they typed on the task.
    #[test]
    fn git_changes_fails_with_gits_own_message() {
        let runner = MockProcessRunner::new(vec![
            MockProcessRunner::fail("fatal: Not a valid object name nosuchbranch\n"),
            MockProcessRunner::fail("fatal: Not a valid object name origin/nosuchbranch\n"),
        ]);
        let err = git_changes(Path::new("/wt"), "nosuchbranch", &runner)
            .expect_err("must fail")
            .to_string();
        assert!(err.contains("nosuchbranch"), "got {err}");
        assert!(!err.contains("origin/"), "got {err}");
    }

    /// A baseline we could not resolve short-circuits — neither the diff nor
    /// the listing may run against a repo we already know we cannot read.
    #[test]
    fn a_failed_baseline_resolution_runs_no_further_commands() {
        let runner = failing_git_rig("fatal: not a git repository\n");
        let _ = git_changes(Path::new("/wt"), "main", &runner);
        assert_eq!(runner.recorded_calls().len(), 2);
    }

    /// A failing diff short-circuits too: the listing must not run against a
    /// repo that just refused to diff.
    #[test]
    fn a_failing_diff_does_not_run_the_untracked_listing() {
        let runner = MockProcessRunner::new(vec![
            sha(LOCAL_FORK),
            sha(LOCAL_FORK),
            MockProcessRunner::fail("fatal: unable to read index.lock\n"),
        ]);
        let _ = git_changes(Path::new("/wt"), "main", &runner);
        assert_eq!(runner.recorded_calls().len(), 3);
    }

    // ---- refresh: failure keeps the last good tree ------------------------

    /// The spec's AgentTreeGitFailureKeepsLastGoodTree: a failed query leaves
    /// the tree exactly as it was and says so. Blanking on a transient index
    /// lock — the commonest failure, taken by the agent's own git — would make
    /// the pane flicker empty.
    #[test]
    fn a_failed_git_query_keeps_the_last_good_tree_and_sets_a_notice() {
        let mut state = RenderState::new();
        let mut tree = build_tree(&root(), &[]);

        let good = git_rig(&["M", "src/a.rs"], &[]);
        refresh(&root(), "main", &good, &mut tree, &mut state);
        assert!(tree.node_at(&["src", "a.rs"]).is_some());
        assert!(state.notice.is_none());

        let bad = failing_git_rig("fatal: unable to read index.lock\n");
        refresh(&root(), "main", &bad, &mut tree, &mut state);

        assert!(
            tree.node_at(&["src", "a.rs"]).is_some(),
            "the last good tree must survive"
        );
        let notice = state.notice.as_ref().expect("notice set");
        assert!(matches!(notice, Notice::Git(_)), "got {notice:?}");
        assert!(notice.text().contains("index.lock"), "got {notice:?}");
    }

    /// A working git retracts its own complaint on the next tick.
    #[test]
    fn a_recovering_git_query_clears_its_own_notice() {
        let mut state = RenderState::new();
        let mut tree = build_tree(&root(), &[]);

        let bad = failing_git_rig("fatal: unable to read index.lock\n");
        refresh(&root(), "main", &bad, &mut tree, &mut state);
        assert!(state.notice.is_some());

        let good = git_rig(&["M", "a.rs"], &[]);
        refresh(&root(), "main", &good, &mut tree, &mut state);
        assert!(state.notice.is_none());
    }

    /// ...but it must not swallow the answer to a keypress the user made half a
    /// second ago. The two notices share one field and one line of border, so
    /// the source is what keeps them apart — see NoticeSource in the spec.
    #[test]
    fn a_successful_git_query_leaves_a_diff_notice_alone() {
        let mut state = RenderState::new();
        let mut tree = build_tree(&root(), &[]);
        state.notice = Some(Notice::diff("could not split the diff pane"));

        let good = git_rig(&["M", "a.rs"], &[]);
        refresh(&root(), "main", &good, &mut tree, &mut state);

        let notice = state.notice.as_ref().expect("diff notice must survive");
        assert!(matches!(notice, Notice::Diff(_)), "got {notice:?}");
    }

    /// A revert un-badges the file with no bookkeeping: git stops reporting it,
    /// so the node goes. This is the second half of task #4408 — a file that is
    /// not modified must not show as modified.
    #[test]
    fn a_reverted_file_disappears_from_the_tree() {
        let mut state = RenderState::new();
        let mut tree = build_tree(&root(), &[]);

        let dirty = git_rig(&["M", "a.rs"], &[]);
        refresh(&root(), "main", &dirty, &mut tree, &mut state);
        assert!(tree.node_at(&["a.rs"]).is_some());

        let clean = git_rig(&[], &[]);
        refresh(&root(), "main", &clean, &mut tree, &mut state);
        assert!(
            tree.node_at(&["a.rs"]).is_none(),
            "a reverted file must leave the tree"
        );
    }

    // ---- Snapshots ---------------------------------------------------------

    #[test]
    fn snapshot_notice_is_shown_in_the_bottom_border() {
        let tree = build_tree(&root(), &three_node_changes());
        let mut state = RenderState::new();
        state.sync_expansion(&tree);
        state.notice = Some(Notice::diff("could not split the diff pane"));
        let mut terminal = Terminal::new(TestBackend::new(50, 12)).expect("terminal");
        terminal
            .draw(|frame| render(frame, frame.area(), &tree, &mut state, "dispatch"))
            .expect("draw");
        let rendered = buffer_to_string(terminal.backend().buffer());

        assert!(
            rendered.contains("could not split"),
            "the notice must be visible; rendered:\n{rendered}"
        );
        insta::assert_snapshot!(rendered);
    }

    /// The border reddens with the notice — see AgentTreeNoticeRedensBorder.
    /// Asserted on the styled buffer, not the plain text, because the whole
    /// point is that the frame carries where a line of text does not.
    #[test]
    fn a_notice_reddens_the_whole_border() {
        let tree = build_tree(&root(), &three_node_changes());
        let mut state = RenderState::new();
        state.sync_expansion(&tree);
        let mut terminal = Terminal::new(TestBackend::new(50, 12)).expect("terminal");

        terminal
            .draw(|frame| render(frame, frame.area(), &tree, &mut state, "dispatch"))
            .expect("draw");
        let corner = terminal.backend().buffer()[(0, 0)].clone();
        assert_ne!(
            corner.style().fg,
            Some(RED),
            "no notice: the border must not be red"
        );

        state.notice = Some(Notice::git("git: fatal: unable to read index.lock"));
        terminal
            .draw(|frame| render(frame, frame.area(), &tree, &mut state, "dispatch"))
            .expect("draw");
        let buf = terminal.backend().buffer();
        for (x, y) in [(0u16, 0u16), (49, 0), (0, 11), (49, 11)] {
            assert_eq!(
                buf[(x, y)].style().fg,
                Some(RED),
                "corner ({x},{y}) must be red while a notice shows"
            );
        }
    }

    #[test]
    fn snapshot_empty_tree_shows_bare_title() {
        let rendered = render_to_string(&[], "dispatch", 50, 10);
        insta::assert_snapshot!(rendered);
    }

    #[test]
    fn snapshot_added_modified_and_deleted_badges() {
        let changes = vec![
            added("src/new.rs"),
            modified("src/lib.rs"),
            deleted("README.md"),
        ];
        let rendered = render_to_string(&changes, "dispatch", 50, 12);
        insta::assert_snapshot!(rendered);
    }

    #[test]
    fn snapshot_nested_directories_auto_expanded() {
        let rendered = render_to_string(&[modified("a/b/c.rs")], "dispatch", 50, 12);
        insta::assert_snapshot!(rendered);
    }

    /// The only form that exercises the widget's own open-set lookup, and
    /// so the only one that can catch a key-representation mismatch: an
    /// assertion over `opened()` can encode a key that matches no node and
    /// still pass, because `TreeState::open` reports success on it (#3811).
    /// Every directory on the way to a changed file is expanded, so the
    /// leaf is on screen with no keypresses.
    #[test]
    fn deeply_nested_changed_file_is_visible_without_manual_expansion() {
        let rendered = render_to_string(&[modified("a/b/c/d/leaf.rs")], "dispatch", 50, 12);
        assert!(
            rendered.contains("leaf.rs"),
            "the leaf must be visible unaided; rendered:\n{rendered}"
        );
        assert!(
            !rendered.contains('▶'),
            "no directory may render collapsed; rendered:\n{rendered}"
        );
    }

    #[test]
    fn manually_collapsed_nested_directory_stays_collapsed_on_refresh() {
        let changes = [modified("a/b/c.rs")];
        let tree = build_tree(&root(), &changes);
        let mut state = RenderState::new();
        state.sync_expansion(&tree);

        let nested = vec!["a".to_string(), "b".to_string()];
        assert!(state.tree_state.close(&nested));

        state.sync_expansion(&build_tree(&root(), &changes));
        assert!(!state.tree_state.opened().contains(&nested));
    }
}
