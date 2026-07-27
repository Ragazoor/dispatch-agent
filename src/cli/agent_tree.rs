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
use crossterm::event::{self, Event, KeyCode, KeyEventKind, KeyModifiers};
use crossterm::execute;
use crossterm::terminal::{
    disable_raw_mode, enable_raw_mode, EnterAlternateScreen, LeaveAlternateScreen,
};
use ratatui::backend::{Backend, CrosstermBackend};
use ratatui::layout::Rect;
use ratatui::style::{Color, Modifier, Style};
use ratatui::text::{Line, Span};
use ratatui::widgets::{Block, Borders};
use ratatui::{Frame, Terminal};
use tui_tree_widget::{Tree, TreeItem, TreeState};

use crate::agent_tree::{build_tree, FileOperation, TreeNode, TreeNodeKind};
use crate::db::{Database, TaskRead};
use crate::file_events::FILE_EVENTS_SUBDIR;
use crate::models::TaskId;

/// Redraw cadence — see `docs/specs/agent-tree.allium`'s
/// `config.agent_tree_refresh_interval`. Doubles as the crossterm event
/// poll timeout, so a key press and a plain timer tick share one wait.
const REFRESH_INTERVAL: Duration = Duration::from_secs(1);

const MODIFIED_COLOR: Color = Color::Rgb(224, 175, 104);
const READ_COLOR: Color = Color::Rgb(122, 162, 247);
const DIR_COLOR: Color = Color::Rgb(192, 202, 245);

fn node_label(node: &TreeNode) -> Line<'static> {
    match node.badge {
        None => Line::from(Span::styled(
            node.name.clone(),
            Style::default().fg(DIR_COLOR),
        )),
        Some(FileOperation::Modified) => Line::from(vec![
            Span::raw(format!("{} ", node.name)),
            Span::styled(
                "[Modified]",
                Style::default()
                    .fg(MODIFIED_COLOR)
                    .add_modifier(Modifier::BOLD),
            ),
        ]),
        Some(FileOperation::Read) => Line::from(vec![
            Span::raw(format!("{} ", node.name)),
            Span::styled("[Read]", Style::default().fg(READ_COLOR)),
        ]),
    }
}

fn node_to_item(node: &TreeNode, prefix: &str) -> Option<TreeItem<'static, String>> {
    let id = if prefix.is_empty() {
        node.name.clone()
    } else {
        format!("{prefix}/{}", node.name)
    };
    let label = node_label(node);
    match node.kind {
        TreeNodeKind::File => Some(TreeItem::new_leaf(id, label)),
        TreeNodeKind::Directory => {
            let children = to_items(&node.children, &id);
            match TreeItem::new(id.clone(), label, children) {
                Ok(item) => Some(item),
                Err(e) => {
                    tracing::warn!(
                        error = ?e,
                        id,
                        "skipping agent-tree node with duplicate child identifiers"
                    );
                    None
                }
            }
        }
    }
}

fn to_items(children: &[TreeNode], prefix: &str) -> Vec<TreeItem<'static, String>> {
    children
        .iter()
        .filter_map(|node| node_to_item(node, prefix))
        .collect()
}

/// Convert subtask 3's touched-paths tree into `tui_tree_widget` items.
/// The root node itself is not rendered as a wrapping item — its children
/// become the top-level list, like a normal file browser.
pub fn build_tree_items(root: &TreeNode) -> Vec<TreeItem<'static, String>> {
    to_items(&root.children, "")
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
}

impl RenderState {
    pub fn new() -> Self {
        Self {
            tree_state: TreeState::default(),
            auto_expanded: HashSet::new(),
        }
    }

    /// Auto-open every directory with a touched descendant, exactly once
    /// per directory — see the struct doc comment on monotonicity.
    pub fn sync_expansion(&mut self, root: &TreeNode) {
        let mut path = Vec::new();
        self.sync_expansion_at(&root.children, &mut path);
    }

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
/// Pure — used by both the real polling loop and snapshot tests.
pub fn render(
    frame: &mut Frame,
    area: Rect,
    root: &TreeNode,
    state: &mut RenderState,
    title: &str,
) {
    let items = build_tree_items(root);
    let block = Block::default()
        .borders(Borders::ALL)
        .title(format!(" {title} "));

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

fn run_loop<B: Backend>(terminal: &mut Terminal<B>, root: &Path, events_path: &Path) -> Result<()> {
    let title = worktree_title(root);
    let mut state = RenderState::new();
    loop {
        let jsonl = read_events_file(events_path);
        let tree = build_tree(root, &jsonl);
        state.sync_expansion(&tree);
        terminal.draw(|frame| render(frame, frame.area(), &tree, &mut state, &title))?;

        if event::poll(REFRESH_INTERVAL)? {
            if let Event::Key(key) = event::read()? {
                if key.kind != KeyEventKind::Press {
                    continue;
                }
                match key.code {
                    KeyCode::Char('q') => return Ok(()),
                    KeyCode::Char('c') if key.modifiers.contains(KeyModifiers::CONTROL) => {
                        return Ok(())
                    }
                    KeyCode::Up => {
                        state.tree_state.key_up();
                    }
                    KeyCode::Down => {
                        state.tree_state.key_down();
                    }
                    KeyCode::Left => {
                        state.tree_state.key_left();
                    }
                    KeyCode::Right => {
                        state.tree_state.key_right();
                    }
                    KeyCode::Char(' ') | KeyCode::Enter => {
                        state.tree_state.toggle_selected();
                    }
                    _ => {}
                }
            }
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
    let events_path = data_dir
        .join(FILE_EVENTS_SUBDIR)
        .join(format!("{task_id}.jsonl"));

    enable_raw_mode()?;
    let mut stdout = io::stdout();
    execute!(stdout, EnterAlternateScreen)?;
    let backend = CrosstermBackend::new(stdout);
    let mut terminal = Terminal::new(backend)?;

    let result = run_loop(&mut terminal, &root, &events_path);

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

    #[test]
    fn empty_tree_produces_no_items() {
        let tree = build_tree(&root(), "");
        let items = build_tree_items(&tree);
        assert!(items.is_empty());
    }

    #[test]
    fn touched_file_becomes_a_leaf_item_named_by_relative_path() {
        let jsonl = r#"{"schema_version":"1.0.0","timestamp":"2026-07-27T12:00:00Z","task_id":"1","tool":"read","path":"/repo/a.rs","operation":"read"}"#;
        let tree = build_tree(&root(), jsonl);
        let items = build_tree_items(&tree);
        assert_eq!(items.len(), 1);
        assert_eq!(items[0].identifier(), "a.rs");
        assert!(items[0].children().is_empty());
    }

    #[test]
    fn touched_dir_becomes_a_non_leaf_item() {
        let jsonl = r#"{"schema_version":"1.0.0","timestamp":"2026-07-27T12:00:00Z","task_id":"1","tool":"read","path":"/repo/src/a.rs","operation":"read"}"#;
        let tree = build_tree(&root(), jsonl);
        let items = build_tree_items(&tree);
        assert_eq!(items.len(), 1);
        assert_eq!(items[0].identifier(), "src");
        assert_eq!(items[0].children().len(), 1);
    }

    #[test]
    fn nested_file_identifier_is_slash_joined_path() {
        let jsonl = r#"{"schema_version":"1.0.0","timestamp":"2026-07-27T12:00:00Z","task_id":"1","tool":"read","path":"/repo/a/b/c.rs","operation":"read"}"#;
        let tree = build_tree(&root(), jsonl);
        let items = build_tree_items(&tree);
        let a = &items[0];
        assert_eq!(a.identifier(), "a");
        let b = &a.children()[0];
        assert_eq!(b.identifier(), "a/b");
        let c = &b.children()[0];
        assert_eq!(c.identifier(), "a/b/c.rs");
    }

    #[test]
    fn two_touched_roots_produce_two_top_level_items_sorted_by_name() {
        let jsonl = format!(
            "{}\n{}",
            r#"{"schema_version":"1.0.0","timestamp":"2026-07-27T12:00:00Z","task_id":"1","tool":"read","path":"/repo/z.rs","operation":"read"}"#,
            r#"{"schema_version":"1.0.0","timestamp":"2026-07-27T12:00:00Z","task_id":"1","tool":"read","path":"/repo/a.rs","operation":"read"}"#
        );
        let tree = build_tree(&root(), &jsonl);
        let items = build_tree_items(&tree);
        assert_eq!(items.len(), 2);
        assert_eq!(items[0].identifier(), "a.rs");
        assert_eq!(items[1].identifier(), "z.rs");
    }

    #[test]
    fn sync_expansion_opens_newly_touched_directory() {
        let jsonl = r#"{"schema_version":"1.0.0","timestamp":"2026-07-27T12:00:00Z","task_id":"1","tool":"read","path":"/repo/src/a.rs","operation":"read"}"#;
        let tree = build_tree(&root(), jsonl);
        let mut state = RenderState::new();
        state.sync_expansion(&tree);
        assert!(state.tree_state.opened().contains(&vec!["src".to_string()]));
    }

    #[test]
    fn sync_expansion_does_not_reopen_a_manually_closed_directory() {
        let jsonl = r#"{"schema_version":"1.0.0","timestamp":"2026-07-27T12:00:00Z","task_id":"1","tool":"read","path":"/repo/src/a.rs","operation":"read"}"#;
        let tree = build_tree(&root(), jsonl);
        let mut state = RenderState::new();
        state.sync_expansion(&tree);
        assert!(state.tree_state.close(&["src".to_string()]));

        // Rebuild the same tree (as a fresh poll of an unchanged file would)
        // and sync again: "src" was already auto-expanded once, so the
        // manual close must survive.
        let tree_again = build_tree(&root(), jsonl);
        state.sync_expansion(&tree_again);
        assert!(!state.tree_state.opened().contains(&vec!["src".to_string()]));
    }

    #[test]
    fn sync_expansion_opens_a_newly_touched_sibling_without_reopening_a_closed_one() {
        let first_event = r#"{"schema_version":"1.0.0","timestamp":"2026-07-27T12:00:00Z","task_id":"1","tool":"read","path":"/repo/src/a.rs","operation":"read"}"#;
        let mut state = RenderState::new();
        state.sync_expansion(&build_tree(&root(), first_event));
        assert!(state.tree_state.close(&["src".to_string()]));

        // A second poll picks up a brand-new touch under a different directory.
        let jsonl = format!(
            "{}\n{}",
            first_event,
            r#"{"schema_version":"1.0.0","timestamp":"2026-07-27T12:00:01Z","task_id":"1","tool":"read","path":"/repo/docs/b.md","operation":"read"}"#
        );
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
        let jsonl = r#"{"schema_version":"1.0.0","timestamp":"2026-07-27T12:00:00Z","task_id":"1","tool":"read","path":"/repo/a/b/c.rs","operation":"read"}"#;
        let tree = build_tree(&root(), jsonl);
        let mut state = RenderState::new();
        state.sync_expansion(&tree);
        assert!(state.tree_state.opened().contains(&vec!["a".to_string()]));
        assert!(state
            .tree_state
            .opened()
            .contains(&vec!["a".to_string(), "b".to_string()]));
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

    #[test]
    fn snapshot_empty_tree_shows_bare_title() {
        let rendered = render_to_string("", "dispatch", 50, 10);
        insta::assert_snapshot!(rendered);
    }

    #[test]
    fn snapshot_modified_and_read_badges() {
        let jsonl = format!(
            "{}\n{}",
            r#"{"schema_version":"1.0.0","timestamp":"2026-07-27T12:00:00Z","task_id":"1","tool":"write","path":"/repo/src/lib.rs","operation":"modified"}"#,
            r#"{"schema_version":"1.0.0","timestamp":"2026-07-27T12:00:01Z","task_id":"1","tool":"read","path":"/repo/README.md","operation":"read"}"#
        );
        let rendered = render_to_string(&jsonl, "dispatch", 50, 12);
        insta::assert_snapshot!(rendered);
    }

    #[test]
    fn snapshot_nested_directories_auto_expanded() {
        let jsonl = r#"{"schema_version":"1.0.0","timestamp":"2026-07-27T12:00:00Z","task_id":"1","tool":"write","path":"/repo/a/b/c.rs","operation":"modified"}"#;
        let rendered = render_to_string(jsonl, "dispatch", 50, 12);
        insta::assert_snapshot!(rendered);
    }
}
