//! The standalone CLI renderers dispatch runs in tmux panes of its own, plus
//! the small non-rendering subcommands.
//!
//! The two pane renderers ([`agent_tree`] and [`agent_diff`]) share their
//! entry-point shape — resolve the task, take the terminal, run a loop, give
//! the terminal back — and that shape lives here rather than in either of them.

use std::io;
use std::path::{Path, PathBuf};

use anyhow::{Context, Result};
use crossterm::execute;
use crossterm::terminal::{
    disable_raw_mode, enable_raw_mode, EnterAlternateScreen, LeaveAlternateScreen,
};
use ratatui::backend::CrosstermBackend;
use ratatui::Terminal;

use crate::db::{Database, TaskRead};
use crate::models::TaskId;

pub mod agent_diff;
pub mod agent_tree;
pub mod caller_headers;
pub mod statusline;

/// The worktree and base branch a pane renderer works from.
///
/// Both panes take a task id rather than a path, so they cannot disagree about
/// which worktree they are looking at and both resolve their baseline from the
/// same base branch. This is that lookup, once.
pub(crate) async fn pane_task_context(db_path: &Path, task_id: i64) -> Result<(PathBuf, String)> {
    let database = Database::open(db_path).await?;
    let task = database
        .get_task(TaskId(task_id))
        .await?
        .with_context(|| format!("task {task_id} not found"))?;
    let worktree = task
        .worktree
        .clone()
        .with_context(|| format!("task {task_id} has no worktree"))?;
    Ok((PathBuf::from(worktree), task.base_branch))
}

/// Take the terminal, run `body`, and give the terminal back — whatever `body`
/// did.
///
/// The restore is deliberately NOT behind `?`. A renderer that returns an error
/// still has to leave raw mode and the alternate screen, or the user is dropped
/// back into a shell that echoes nothing and shows no cursor, with no
/// indication why. Both panes get that property from here rather than each
/// re-deriving it, because the failure is silent and identical in both.
pub(crate) fn with_pane_terminal<T>(
    body: impl FnOnce(&mut Terminal<CrosstermBackend<io::Stdout>>) -> Result<T>,
) -> Result<T> {
    enable_raw_mode()?;
    let mut stdout = io::stdout();
    execute!(stdout, EnterAlternateScreen)?;
    let mut terminal = Terminal::new(CrosstermBackend::new(stdout))?;

    let result = body(&mut terminal);

    disable_raw_mode()?;
    execute!(terminal.backend_mut(), LeaveAlternateScreen)?;
    terminal.show_cursor()?;

    result
}
