//! `dispatch agent-tree <task_id>` — a small, standalone ratatui loop that
//! renders one task's file-touch tree (see `docs/specs/agent-tree.allium`'s
//! `AgentTreeCompanionPane` surface and `RefreshAgentTree` rule).
//!
//! Deliberately NOT part of the board TUI's `App`/message loop: this runs
//! as its own process in a tmux companion pane (subtask 5 wires up the
//! split; this subtask only builds the standalone renderer).
//!
//! Renders subtask 3's touched-paths-only tree (`agent_tree::build_tree`)
//! as-is. It does not merge in a full worktree filesystem scan — the
//! Allium spec's `TreeScanExclusions` open question (which subtrees a
//! fuller scan should skip) remains unresolved and out of scope here.

use std::collections::HashSet;
use std::io;
use std::path::{Path, PathBuf};
use std::time::Duration;

use anyhow::{Context, Result};
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

use crate::agent_tree::{build_tree, FileOperation, TreeNode, TreeNodeKind};
use crate::agent_tree_editor::{current_pane_from_env, open_in_editor};
use crate::db::{Database, TaskRead};
use crate::editor::editor_from_env;
use crate::file_events::file_events_path;
use crate::models::TaskId;
use crate::process::{ProcessRunner, RealProcessRunner};
use crate::tui::ui::palette::{BLUE, FG, YELLOW};

/// Redraw cadence — see `docs/specs/agent-tree.allium`'s
/// `config.agent_tree_refresh_interval`. Doubles as the crossterm event
/// poll timeout, so a key press and a plain timer tick share one wait.
const REFRESH_INTERVAL: Duration = Duration::from_secs(1);

fn node_label(node: &TreeNode) -> Line<'static> {
    let (badge, style) = match node.badge {
        None => return Line::from(Span::styled(node.name.clone(), Style::default().fg(FG))),
        Some(FileOperation::Modified) => (
            "[Modified]",
            Style::default().fg(YELLOW).add_modifier(Modifier::BOLD),
        ),
        Some(FileOperation::Read) => ("[Read]", Style::default().fg(BLUE)),
    };
    Line::from(vec![
        Span::raw(format!("{} ", node.name)),
        Span::styled(badge, style),
    ])
}

fn node_to_item(node: &TreeNode, path: &mut Vec<String>) -> Option<TreeItem<'static, String>> {
    path.push(node.name.clone());
    let label = node_label(node);
    let item = match node.kind {
        TreeNodeKind::File => Some(TreeItem::new_leaf(node.name.clone(), label)),
        TreeNodeKind::Directory => {
            let children = to_items(&node.children, path);
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

fn to_items(children: &[TreeNode], path: &mut Vec<String>) -> Vec<TreeItem<'static, String>> {
    children
        .iter()
        .filter_map(|node| node_to_item(node, path))
        .collect()
}

/// Convert subtask 3's touched-paths tree into `tui_tree_widget` items.
/// The root node itself is not rendered as a wrapping item — its children
/// become the top-level list, like a normal file browser.
///
/// A node is identified by its own name segment, which is all the widget
/// requires (identifiers must be unique among siblings only — it already
/// scopes lookups by the chain of ancestor identifiers). That makes a
/// node's widget key and its path segments the same `Vec<String>`, which is
/// exactly what `RenderState::sync_expansion` walks with. The `path`
/// accumulator survives only to give the duplicate-identifier warning
/// somewhere useful to point.
pub fn build_tree_items(root: &TreeNode) -> Vec<TreeItem<'static, String>> {
    to_items(&root.children, &mut Vec::new())
}

/// Tree-widget navigation/expansion state, plus tracking of which
/// directories have already been auto-expanded once.
///
/// A directory's `expanded` flag (set by `agent_tree::build_tree`) is
/// monotonic: the file-events log is append-only, so once a directory has
/// a touched descendant it always will. `sync_expansion` uses this to open
/// a directory automatically exactly once — the first rebuild where it's
/// touched — and never force it open again, so a user's manual collapse of
/// an already-touched directory survives later redraws. Only a directory
/// that becomes touched for the first time opens on its own.
pub struct RenderState {
    pub tree_state: TreeState<String>,
    auto_expanded: HashSet<Vec<String>>,
    /// A one-line failure notice, rendered in the pane's bottom border and
    /// cleared by the next key press. Opening a file is the only action in this
    /// pane that can fail visibly to the user — see
    /// `AgentTreeEditorOpenFailureIsVisible` in docs/specs/agent-tree.allium.
    pub notice: Option<String>,
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

    /// Auto-open every directory with a touched descendant, exactly once
    /// per directory — see the struct doc comment on monotonicity.
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

/// Render subtask 3's tree, with `[Modified]`/`[Read]` badges, into `area`.
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
    let items = build_tree_items(root);
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
    if let Some(notice) = &state.notice {
        block = block.title_bottom(Line::from(Span::styled(
            format!(" {notice} "),
            Style::default().fg(YELLOW).add_modifier(Modifier::BOLD),
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

fn read_events_file(path: &Path) -> String {
    match std::fs::read_to_string(path) {
        Ok(s) => s,
        Err(e) if e.kind() == io::ErrorKind::NotFound => String::new(),
        Err(e) => {
            tracing::warn!(
                error = ?e,
                path = %path.display(),
                "agent-tree: failed to read file-events log, showing empty tree"
            );
            String::new()
        }
    }
}

fn worktree_title(root: &Path) -> String {
    root.file_name()
        .map(|n| n.to_string_lossy().into_owned())
        .unwrap_or_else(|| root.to_string_lossy().into_owned())
}

/// `(len, modified)` for the events file, or `None` if it doesn't exist yet.
/// Used to skip re-reading and re-parsing the log on a poll tick where
/// nothing has actually landed — the log is append-only and only grows for
/// the life of the task, so a size/mtime match means "no new lines."
fn file_stamp(path: &Path) -> Option<(u64, std::time::SystemTime)> {
    let meta = std::fs::metadata(path).ok()?;
    let modified = meta.modified().ok()?;
    Some((meta.len(), modified))
}

fn rebuild(root: &Path, events_path: &Path, state: &mut RenderState) -> TreeNode {
    let tree = build_tree(root, &read_events_file(events_path));
    state.sync_expansion(&tree);
    tree
}

/// What the event loop should do after `handle_key` has processed a key.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum KeyAction {
    /// Stay in the loop and redraw.
    Continue,
    /// Leave the loop, which exits the process and so closes the tmux pane.
    Exit,
    /// Show this path — **relative to the pane root** — in the window's editor
    /// pane. `handle_key` stays pure: resolving the absolute path, the editor and
    /// the tmux calls all belong to the loop.
    OpenInEditor(PathBuf),
}

/// What kind of node the widget's current selection names, if any. A selection
/// path is exactly a node's chain of name segments below the root (see
/// `build_tree_items` and `sync_expansion_at`), so resolving it is
/// `TreeNode::node_at`.
///
/// Fails closed, and this is the one place that rule is stated: `None` for an
/// empty selection — which `node_at` would otherwise resolve to the root, itself
/// a directory — and `None` for a path that resolves to nothing, i.e. a stale
/// selection left over from before a rebuild.
fn selected_kind(root: &TreeNode, selected: &[String]) -> Option<TreeNodeKind> {
    if selected.is_empty() {
        return None;
    }
    root.node_at(selected).map(|node| node.kind)
}

/// Whether the selection is a directory, and so has something to open.
fn selected_is_directory(root: &TreeNode, selected: &[String]) -> bool {
    selected_kind(root, selected) == Some(TreeNodeKind::Directory)
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
    // Any key acknowledges a failure notice — docs/specs/agent-tree.allium's
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
        // Space/Enter dispatch on the selected node's kind — one resolution, both
        // arms — so an unselectable or stale selection reaches neither.
        KeyCode::Char(' ') | KeyCode::Enter => {
            let selected = state.tree_state.selected();
            match selected_kind(root, selected) {
                Some(TreeNodeKind::File) => {
                    return KeyAction::OpenInEditor(selected.iter().collect())
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

/// Resolve this pane and the user's editor, then show `relative` in the window's
/// editor pane. Split out of the loop so its arm stays one branch and the message
/// the notice shows is built in one place.
fn open_selected(root: &Path, relative: &Path, runner: &dyn ProcessRunner) -> Result<()> {
    let my_pane = current_pane_from_env()?;
    let editor = editor_from_env();
    open_in_editor(root, relative, &my_pane, &editor, runner)
}

fn run_loop<B: Backend>(
    terminal: &mut Terminal<B>,
    root: &Path,
    events_path: &Path,
    runner: &dyn ProcessRunner,
) -> Result<()> {
    let title = worktree_title(root);
    let mut state = RenderState::new();
    let mut tree = rebuild(root, events_path, &mut state);
    let mut last_stamp = file_stamp(events_path);

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
                KeyAction::OpenInEditor(relative) => {
                    // Every failure here is the user's to see: they pressed a key
                    // and expect a file. The renderer keeps running regardless —
                    // an editor that will not open must not take the tree with
                    // it (docs/specs/agent-tree.allium:
                    // AgentTreeEditorOpenFailureIsVisible).
                    if let Err(e) = open_selected(root, &relative, runner) {
                        tracing::warn!(
                            path = %relative.display(),
                            error = %e,
                            "failed to open the selected file in an editor"
                        );
                        state.notice = Some(e.to_string());
                    }
                }
            }
            continue;
        }

        // Poll timed out with no key event: the ~1s timer tick. Only
        // re-read and rebuild the tree if the file actually changed.
        let stamp = file_stamp(events_path);
        if stamp != last_stamp {
            last_stamp = stamp;
            tree = rebuild(root, events_path, &mut state);
        }
    }
}

/// Entry point for `dispatch agent-tree <task_id>`. Standalone ratatui loop
/// — not part of the board TUI's `App`/message loop (see the module-level
/// doc comment). Resolves the task's worktree from the DB once, then polls
/// `<data_dir>/file-events/<task_id>.jsonl` on a 1-second timer.
pub async fn run(db_path: &Path, task_id: i64) -> Result<()> {
    let database = Database::open(db_path).await?;
    let task = database
        .get_task(TaskId(task_id))
        .await?
        .with_context(|| format!("task {task_id} not found"))?;
    let worktree = task
        .worktree
        .with_context(|| format!("task {task_id} has no worktree"))?;
    let root = PathBuf::from(worktree);

    let data_dir = db_path.parent().unwrap_or_else(|| Path::new("."));
    let events_path = file_events_path(data_dir, task_id);

    enable_raw_mode()?;
    let mut stdout = io::stdout();
    execute!(stdout, EnterAlternateScreen)?;
    let backend = CrosstermBackend::new(stdout);
    let mut terminal = Terminal::new(backend)?;

    let result = run_loop(&mut terminal, &root, &events_path, &RealProcessRunner);

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
    use std::path::PathBuf;

    fn root() -> PathBuf {
        PathBuf::from("/repo")
    }

    /// One JSONL line for a file-event, varying only `path`/`operation`.
    /// `schema_version`/`timestamp`/`task_id`/`tool` don't affect tree-
    /// building (only `path` and `operation` do — see subtask 3's
    /// `agent_tree::build_tree`), so fixed placeholders are fine here.
    fn event(path: &str, operation: &str) -> String {
        format!(
            r#"{{"schema_version":"1.0.0","timestamp":"2026-07-27T12:00:00Z","task_id":"1","tool":"read","path":"{path}","operation":"{operation}"}}"#
        )
    }

    #[test]
    fn empty_tree_produces_no_items() {
        let tree = build_tree(&root(), "");
        let items = build_tree_items(&tree);
        assert!(items.is_empty());
    }

    #[test]
    fn touched_file_becomes_a_leaf_item_named_by_relative_path() {
        let tree = build_tree(&root(), &event("/repo/a.rs", "read"));
        let items = build_tree_items(&tree);
        assert_eq!(items.len(), 1);
        assert_eq!(items[0].identifier(), "a.rs");
        assert!(items[0].children().is_empty());
    }

    #[test]
    fn touched_dir_becomes_a_non_leaf_item() {
        let tree = build_tree(&root(), &event("/repo/src/a.rs", "read"));
        let items = build_tree_items(&tree);
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
        let tree = build_tree(&root(), &event("/repo/a/b/c.rs", "read"));
        let items = build_tree_items(&tree);
        let a = &items[0];
        assert_eq!(a.identifier(), "a");
        let b = &a.children()[0];
        assert_eq!(b.identifier(), "b");
        let c = &b.children()[0];
        assert_eq!(c.identifier(), "c.rs");
    }

    #[test]
    fn two_touched_roots_produce_two_top_level_items_sorted_by_name() {
        let jsonl = format!(
            "{}\n{}",
            event("/repo/z.rs", "read"),
            event("/repo/a.rs", "read")
        );
        let tree = build_tree(&root(), &jsonl);
        let items = build_tree_items(&tree);
        assert_eq!(items.len(), 2);
        assert_eq!(items[0].identifier(), "a.rs");
        assert_eq!(items[1].identifier(), "z.rs");
    }

    #[test]
    fn sync_expansion_opens_newly_touched_directory() {
        let tree = build_tree(&root(), &event("/repo/src/a.rs", "read"));
        let mut state = RenderState::new();
        state.sync_expansion(&tree);
        assert!(state.tree_state.opened().contains(&vec!["src".to_string()]));
    }

    #[test]
    fn sync_expansion_does_not_reopen_a_manually_closed_directory() {
        let jsonl = event("/repo/src/a.rs", "read");
        let tree = build_tree(&root(), &jsonl);
        let mut state = RenderState::new();
        state.sync_expansion(&tree);
        assert!(state.tree_state.close(&["src".to_string()]));

        // Rebuild the same tree (as a fresh poll of an unchanged file would)
        // and sync again: "src" was already auto-expanded once, so the
        // manual close must survive.
        let tree_again = build_tree(&root(), &jsonl);
        state.sync_expansion(&tree_again);
        assert!(!state.tree_state.opened().contains(&vec!["src".to_string()]));
    }

    #[test]
    fn sync_expansion_opens_a_newly_touched_sibling_without_reopening_a_closed_one() {
        let first_event = event("/repo/src/a.rs", "read");
        let mut state = RenderState::new();
        state.sync_expansion(&build_tree(&root(), &first_event));
        assert!(state.tree_state.close(&["src".to_string()]));

        // A second poll picks up a brand-new touch under a different directory.
        let jsonl = format!("{}\n{}", first_event, event("/repo/docs/b.md", "read"));
        state.sync_expansion(&build_tree(&root(), &jsonl));

        assert!(
            !state.tree_state.opened().contains(&vec!["src".to_string()]),
            "manually closed dir must stay closed"
        );
        assert!(
            state
                .tree_state
                .opened()
                .contains(&vec!["docs".to_string()]),
            "newly touched dir must auto-open"
        );
    }

    #[test]
    fn sync_expansion_opens_nested_ancestor_directories() {
        let tree = build_tree(&root(), &event("/repo/a/b/c.rs", "read"));
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

    fn render_to_string(jsonl: &str, title: &str, width: u16, height: u16) -> String {
        let tree = build_tree(&root(), jsonl);
        let mut state = RenderState::new();
        state.sync_expansion(&tree);
        let mut terminal = Terminal::new(TestBackend::new(width, height)).expect("terminal");
        terminal
            .draw(|frame| render(frame, frame.area(), &tree, &mut state, title))
            .expect("draw");
        buffer_to_string(terminal.backend().buffer())
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
        fn new(jsonl: &str) -> Self {
            Self::sized(jsonl, 12)
        }

        /// `height` is the whole pane, borders included, so the visible row
        /// count the half-page motions divide is `height - 2`.
        fn sized(jsonl: &str, height: u16) -> Self {
            let tree = build_tree(&root(), jsonl);
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

        /// The selected node's single name segment, for the flat-file logs the
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
    fn three_node_log() -> String {
        format!(
            "{}\n{}\n{}",
            event("/repo/a.rs", "read"),
            event("/repo/src/lib.rs", "modified"),
            event("/repo/z.rs", "read")
        )
    }

    #[test]
    fn q_exits_the_renderer() {
        let mut rig = KeyRig::new("");
        assert_eq!(rig.press(KeyCode::Char('q')), KeyAction::Exit);
    }

    #[test]
    fn ctrl_c_exits_the_renderer() {
        let mut state = RenderState::new();
        let tree = build_tree(&root(), "");
        let key = KeyEvent::new(KeyCode::Char('c'), KeyModifiers::CONTROL);
        assert_eq!(handle_key(&mut state, &tree, key), KeyAction::Exit);
    }

    #[test]
    fn a_bare_c_does_not_exit_the_renderer() {
        let mut rig = KeyRig::new("");
        assert_eq!(rig.press(KeyCode::Char('c')), KeyAction::Continue);
    }

    #[test]
    fn down_and_j_both_move_the_cursor_down() {
        for code in [KeyCode::Down, KeyCode::Char('j')] {
            let mut rig = KeyRig::new(&three_node_log());
            assert_eq!(rig.press(code), KeyAction::Continue);
            assert_eq!(rig.selected(), vec!["a.rs".to_string()], "{code:?}");
            rig.press(code);
            assert_eq!(rig.selected(), vec!["src".to_string()], "{code:?}");
        }
    }

    #[test]
    fn up_and_k_both_move_the_cursor_up() {
        for code in [KeyCode::Up, KeyCode::Char('k')] {
            let mut rig = KeyRig::new(&three_node_log());
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
            let mut rig = KeyRig::new(&three_node_log());
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
            let mut rig = KeyRig::new(&three_node_log());
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
        let mut rig = KeyRig::new(&three_node_log());
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
            let mut rig = KeyRig::new(&three_node_log());
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
            let mut rig = KeyRig::new(&three_node_log());
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
        let mut rig = KeyRig::new(&three_node_log());
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

    // ---- Jump motions: gg, G, Ctrl-D, Ctrl-U ------------------------------
    //
    // See the AgentTreeCompanionPane surface and the
    // AgentTreeGgChordNeverExpires guarantee in docs/specs/agent-tree.allium.

    /// `count` top-level files, `f01.rs`..`fNN.rs`. Flat and zero-padded, so
    /// the flattened view is exactly the files in that order and a landing row
    /// is nameable without counting directories.
    fn flat_files_log(count: usize) -> String {
        (1..=count)
            .map(|n| event(&format!("/repo/f{n:02}.rs"), "read"))
            .collect::<Vec<_>>()
            .join("\n")
    }

    #[test]
    fn gg_jumps_to_the_first_visible_node() {
        let mut rig = KeyRig::new(&flat_files_log(6));
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
        let mut rig = KeyRig::new(&flat_files_log(6));
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
        let mut rig = KeyRig::new(&flat_files_log(6));
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
        let mut rig = KeyRig::new(&flat_files_log(6));
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
        let mut rig = KeyRig::new(&flat_files_log(6));
        assert_eq!(rig.press(KeyCode::Char('G')), KeyAction::Continue);
        assert_eq!(rig.selected_name(), "f06.rs");
    }

    /// A jump lands on a *visible* row. Collapsing `src` hides `lib.rs`, so
    /// the last row becomes `src` itself — and the jump must not reopen it.
    #[test]
    fn shift_g_skips_rows_a_collapsed_directory_hides() {
        let jsonl = format!(
            "{}\n{}",
            event("/repo/a.rs", "read"),
            event("/repo/src/lib.rs", "modified")
        );
        let mut rig = KeyRig::new(&jsonl);
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
        let jsonl = format!(
            "{}\n{}",
            event("/repo/src/lib.rs", "modified"),
            event("/repo/z.rs", "read")
        );
        let mut rig = KeyRig::new(&jsonl);
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
        let mut rig = KeyRig::new(&flat_files_log(20));
        rig.press(KeyCode::Char('j'));
        assert_eq!(rig.press_ctrl(KeyCode::Char('d')), KeyAction::Continue);
        assert_eq!(rig.selected_name(), "f06.rs");
        rig.press_ctrl(KeyCode::Char('d'));
        assert_eq!(rig.selected_name(), "f11.rs");
    }

    #[test]
    fn ctrl_u_moves_the_cursor_half_a_page_up() {
        let mut rig = KeyRig::new(&flat_files_log(20));
        rig.press(KeyCode::Char('G'));
        assert_eq!(rig.selected_name(), "f20.rs");

        assert_eq!(rig.press_ctrl(KeyCode::Char('u')), KeyAction::Continue);
        assert_eq!(rig.selected_name(), "f15.rs");
    }

    #[test]
    fn ctrl_d_clamps_at_the_last_visible_node() {
        let mut rig = KeyRig::new(&flat_files_log(6));
        rig.press(KeyCode::Char('j'));
        rig.press_ctrl(KeyCode::Char('d'));
        rig.press_ctrl(KeyCode::Char('d'));
        assert_eq!(rig.selected_name(), "f06.rs");
    }

    #[test]
    fn ctrl_u_clamps_at_the_first_visible_node() {
        let mut rig = KeyRig::new(&flat_files_log(6));
        rig.press(KeyCode::Char('j'));
        rig.press_ctrl(KeyCode::Char('u'));
        assert_eq!(rig.selected_name(), "f01.rs");
    }

    /// The distance is half the pane's *current* height, not a constant: a
    /// 20-row pane (18 visible) jumps 9, where the default 12-row one jumps 5.
    #[test]
    fn half_a_page_scales_with_the_pane_height() {
        let mut tall = KeyRig::sized(&flat_files_log(20), 20);
        tall.press(KeyCode::Char('j'));
        tall.press_ctrl(KeyCode::Char('d'));
        assert_eq!(tall.selected_name(), "f10.rs");

        let mut short = KeyRig::sized(&flat_files_log(20), 8);
        short.press(KeyCode::Char('j'));
        short.press_ctrl(KeyCode::Char('d'));
        assert_eq!(short.selected_name(), "f04.rs");
    }

    /// A pane with a single visible row halves to zero. Zero is not a motion,
    /// so the floor is one row.
    #[test]
    fn a_pane_too_short_to_halve_still_moves_one_row() {
        let mut rig = KeyRig::sized(&flat_files_log(6), 3);
        rig.press(KeyCode::Char('j'));
        rig.press_ctrl(KeyCode::Char('d'));
        assert_eq!(rig.selected_name(), "f02.rs");
        rig.press_ctrl(KeyCode::Char('u'));
        assert_eq!(rig.selected_name(), "f01.rs");
    }

    #[test]
    fn jump_motions_are_no_ops_on_an_empty_tree() {
        for keys in [vec!['g', 'g'], vec!['G']] {
            let mut rig = KeyRig::new("");
            for key in keys {
                assert_eq!(rig.press(KeyCode::Char(key)), KeyAction::Continue);
            }
            assert!(rig.selected().is_empty());
        }

        let mut rig = KeyRig::new("");
        assert_eq!(rig.press_ctrl(KeyCode::Char('d')), KeyAction::Continue);
        assert_eq!(rig.press_ctrl(KeyCode::Char('u')), KeyAction::Continue);
        assert!(rig.selected().is_empty());
    }

    /// `q` still exits with a chord armed — disarming must not swallow the key
    /// that did the disarming.
    #[test]
    fn q_after_a_lone_g_still_exits() {
        let mut rig = KeyRig::new(&flat_files_log(6));
        rig.press(KeyCode::Char('g'));
        assert_eq!(rig.press(KeyCode::Char('q')), KeyAction::Exit);
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

    #[test]
    fn snapshot_empty_tree_shows_bare_title() {
        let rendered = render_to_string("", "dispatch", 50, 10);
        insta::assert_snapshot!(rendered);
    }

    #[test]
    fn snapshot_modified_and_read_badges() {
        let jsonl = format!(
            "{}\n{}",
            event("/repo/src/lib.rs", "modified"),
            event("/repo/README.md", "read")
        );
        let rendered = render_to_string(&jsonl, "dispatch", 50, 12);
        insta::assert_snapshot!(rendered);
    }

    #[test]
    fn snapshot_nested_directories_auto_expanded() {
        let jsonl = event("/repo/a/b/c.rs", "modified");
        let rendered = render_to_string(&jsonl, "dispatch", 50, 12);
        insta::assert_snapshot!(rendered);
    }

    /// The only form that exercises the widget's own open-set lookup, and
    /// so the only one that can catch a key-representation mismatch: an
    /// assertion over `opened()` can encode a key that matches no node and
    /// still pass, because `TreeState::open` reports success on it (#3811).
    /// Every directory on the way to a touched file is expanded, so the
    /// leaf is on screen with no keypresses.
    #[test]
    fn deeply_nested_touched_file_is_visible_without_manual_expansion() {
        let jsonl = event("/repo/a/b/c/d/leaf.rs", "modified");
        let rendered = render_to_string(&jsonl, "dispatch", 50, 12);
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
        let jsonl = event("/repo/a/b/c.rs", "read");
        let tree = build_tree(&root(), &jsonl);
        let mut state = RenderState::new();
        state.sync_expansion(&tree);

        let nested = vec!["a".to_string(), "b".to_string()];
        assert!(state.tree_state.close(&nested));

        state.sync_expansion(&build_tree(&root(), &jsonl));
        assert!(!state.tree_state.opened().contains(&nested));
    }
}
