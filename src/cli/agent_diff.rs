//! `dispatch agent-diff <task_id>` — the pane beneath the agent-tree companion
//! pane, showing the contents of whatever the tree has open (see
//! `docs/specs/agent-tree.allium`'s `AgentTreeDiffPane` surface and
//! `RefreshAgentTreeDiff` rule).
//!
//! A separate process from the tree for the same reason the tree is a separate
//! process from the board: it is a separate tmux pane, and tmux moves the
//! cursor between panes itself. That is the whole reason the diff is a pane and
//! not a second region inside the tree's pane — a region would have needed a
//! focus model, a focus indicator, a key to move between the two, and a rule
//! for which region each existing motion key acted on. See the spec's
//! `DiffPaneNavigationIsTmuxNavigation`.
//!
//! **This process never writes the open set.** It reads it, and the tree owns
//! it — see [`crate::agent_tree_open_set`] and the spec's
//! `DiffPaneHasNoToggleOfItsOwn`. Everything else it shows comes from git,
//! exactly as the tree's badges do: the paths come from the tree, the contents
//! never do.

use std::collections::BTreeSet;
use std::io;
use std::path::{Path, PathBuf};

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
use ratatui::widgets::{Block, Borders, Paragraph};
use ratatui::{Frame, Terminal};

use crate::cli::agent_tree::{fork_point, run_git, REFRESH_INTERVAL};
use crate::db::{Database, TaskRead};
use crate::models::TaskId;
use crate::process::{ProcessRunner, RealProcessRunner};
use crate::tui::ui::palette::{FG, GREEN, RED, YELLOW};

/// How much diff text this pane will render for ONE file before refusing it.
///
/// Per file, not per pane: one enormous generated file must not cost the user
/// the diffs of the files either side of it, which is what a whole-pane budget
/// would do depending on sort order. Matches `config.agent_tree_diff_max_bytes`
/// in `docs/specs/agent-tree.allium`.
///
/// Bounded for the same reason [`GIT_TIMEOUT`] is: rendering runs inline in a
/// single-threaded loop that also has to answer keypresses, so an unbounded
/// diff is an unbounded freeze.
pub const DIFF_MAX_BYTES: usize = 1_048_576;

/// Why a file is shown as a placeholder rather than as contents.
///
/// Each value is a fact about the file, not a failure of this pane: it rendered
/// exactly what it could, and says which of these it hit. See the spec's
/// `DiffRefusal`.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum DiffRefusal {
    /// Not in the index, so a diff against the baseline cannot see it. Git's
    /// own advice is to stage it, and that is what the placeholder repeats.
    Untracked,
    /// Git reports the change without line contents.
    Binary,
    /// The diff exceeds [`DIFF_MAX_BYTES`].
    TooLarge,
}

impl DiffRefusal {
    /// The one line shown in place of the file's contents.
    pub fn message(self) -> &'static str {
        match self {
            Self::Untracked => "new file, not yet staged — `git add` it to see its diff",
            Self::Binary => "binary file",
            Self::TooLarge => "diff too large to display",
        }
    }
}

/// One file's diff, or the reason there is none.
///
/// Exactly one of `body` and `refusal` is set — the pane always has something
/// to draw for an open file, and the user is never left looking at a blank
/// region wondering whether it is still loading. See the spec's
/// `DiffBodyExcludesRefusal`.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct FileDiff {
    pub path: PathBuf,
    pub body: Option<String>,
    pub refusal: Option<DiffRefusal>,
}

impl FileDiff {
    fn shown(path: &Path, body: String) -> Self {
        Self {
            path: path.to_path_buf(),
            body: Some(body),
            refusal: None,
        }
    }

    fn refused(path: &Path, refusal: DiffRefusal) -> Self {
        Self {
            path: path.to_path_buf(),
            body: None,
            refusal: Some(refusal),
        }
    }
}

/// Whether git printed a binary-file notice rather than a patch.
///
/// Git says `Binary files a/x.png and b/x.png differ` in place of hunks. The
/// check is anchored to the start of a line so a patch that merely *contains*
/// that sentence — a diff of this very file, for instance — is not mistaken for
/// one.
fn is_binary_notice(diff: &str) -> bool {
    diff.lines().any(|line| line.starts_with("Binary files "))
}

/// One open file's diff against `baseline`, or `None` when there is nothing to
/// show for it.
///
/// `None` is not an error and not a placeholder. The open set holds paths, and
/// a path can stop being reported by git between the press that opened it and
/// this call — the agent reverted the file, or committed and reset. The path
/// stays open, because the agent may change the file again and the user did not
/// ask for it to be closed; it simply renders nothing meanwhile. See the spec's
/// `OpenDiffPathsMaySurviveTheirFiles`.
///
/// `untracked` is the tick's whole untracked listing, taken once by the caller
/// rather than probed per path: an untracked file is invisible to a diff
/// against the index, so asking git for its diff would return empty and be
/// indistinguishable from the reverted case above.
///
/// The baseline is the caller's, and it must be the same one the tree resolved
/// — a diff taken against a different baseline than the badge beside it would
/// show the user two answers to one question.
pub fn file_diff(
    root: &Path,
    baseline: &str,
    path: &Path,
    untracked: &BTreeSet<PathBuf>,
    runner: &dyn ProcessRunner,
) -> Result<Option<FileDiff>> {
    if untracked.contains(path) {
        return Ok(Some(FileDiff::refused(path, DiffRefusal::Untracked)));
    }

    let root = root.to_string_lossy().into_owned();
    let path_arg = path.to_string_lossy().into_owned();
    // `--` separates the revision from the pathspec, so a path that looks like a
    // ref ("main", "HEAD") is still read as a path.
    let diff = run_git(
        runner,
        &[
            "-C",
            &root,
            "diff",
            "--no-renames",
            baseline,
            "--",
            &path_arg,
        ],
    )?;

    if diff.trim().is_empty() {
        return Ok(None);
    }
    if is_binary_notice(&diff) {
        return Ok(Some(FileDiff::refused(path, DiffRefusal::Binary)));
    }
    if diff.len() > DIFF_MAX_BYTES {
        return Ok(Some(FileDiff::refused(path, DiffRefusal::TooLarge)));
    }
    Ok(Some(FileDiff::shown(path, diff)))
}

/// The tick's untracked listing, as a set of paths relative to `root`.
///
/// Taken once per refresh and shared across every open path — see
/// [`file_diff`]'s `untracked` argument.
pub fn untracked_paths(root: &Path, runner: &dyn ProcessRunner) -> Result<BTreeSet<PathBuf>> {
    let root = root.to_string_lossy().into_owned();
    let listing = run_git(
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
    Ok(listing
        .split('\0')
        .filter(|p| !p.is_empty())
        .map(PathBuf::from)
        .collect())
}

/// The whole document the pane shows: every open file's diff, in tree order,
/// under its own path heading.
///
/// ONE document, not one region per file. Scrolling past the end of one file
/// reaches the top of the next without a keystroke in between, and there is no
/// per-file cursor to keep track of.
#[derive(Debug, Default, Clone, PartialEq, Eq)]
pub struct DiffDocument {
    pub files: Vec<FileDiff>,
}

impl DiffDocument {
    /// The document as rendered lines, each already tagged with how it should
    /// be coloured. Built once per change rather than per frame, because
    /// scrolling redraws far more often than git answers differently.
    pub fn lines(&self) -> Vec<DiffLine> {
        let mut lines = Vec::new();
        for file in &self.files {
            lines.push(DiffLine {
                kind: DiffLineKind::Heading,
                text: file.path.to_string_lossy().into_owned(),
            });
            match (&file.body, file.refusal) {
                (Some(body), _) => {
                    for line in body.lines() {
                        lines.push(DiffLine {
                            kind: DiffLineKind::of(line),
                            text: line.to_owned(),
                        });
                    }
                }
                (None, Some(refusal)) => lines.push(DiffLine {
                    kind: DiffLineKind::Refusal,
                    text: refusal.message().to_owned(),
                }),
                // Unreachable through `file_diff`, which never builds a
                // `FileDiff` with neither — see DiffBodyExcludesRefusal. Left
                // as an empty heading rather than a panic: this runs in a
                // render path, and a blank section is a better failure than a
                // dead pane.
                (None, None) => {}
            }
        }
        lines
    }
}

/// How one rendered line should be coloured.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum DiffLineKind {
    /// The path heading that opens each file's section.
    Heading,
    Added,
    Removed,
    /// A hunk header (`@@ ... @@`) or git's own file headers.
    Meta,
    Context,
    /// The one-line stand-in for a file whose contents are not shown.
    Refusal,
}

impl DiffLineKind {
    fn of(line: &str) -> Self {
        // Order matters: `+++`/`---` are file headers, not content, and both
        // start with a character that would otherwise read as content, so they
        // have to be recognised BEFORE the single-character arms below.
        const META_PREFIXES: [&str; 4] = ["+++", "---", "@@", "diff --git "];
        if META_PREFIXES.iter().any(|p| line.starts_with(p)) {
            Self::Meta
        } else if line.starts_with('+') {
            Self::Added
        } else if line.starts_with('-') {
            Self::Removed
        } else {
            Self::Context
        }
    }
}

/// One rendered line of the document.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct DiffLine {
    pub kind: DiffLineKind,
    pub text: String,
}

/// Build the document for `open` against `baseline`.
///
/// Order is the order of `open`, which is a `BTreeSet` of relative paths and so
/// is TREE order — the tree is the index the user reads this pane through, so
/// the two must scroll the same way.
///
/// A path git no longer reports contributes nothing and is not an error; a
/// git failure fails the whole build, which the caller turns into a notice
/// while keeping the last good document on screen.
pub fn build_document(
    root: &Path,
    baseline: &str,
    open: &BTreeSet<PathBuf>,
    untracked: &BTreeSet<PathBuf>,
    runner: &dyn ProcessRunner,
) -> Result<DiffDocument> {
    let mut files = Vec::new();
    for path in open {
        if let Some(diff) = file_diff(root, baseline, path, untracked, runner)? {
            files.push(diff);
        }
    }
    Ok(DiffDocument { files })
}

/// The pane's view state: where the document is scrolled to, and the notice in
/// its border.
///
/// Deliberately small. Everything the pane SHOWS is re-derived from git every
/// tick; this is only where the user has scrolled to, which no git query can
/// answer. It is not modelled in the spec for the same reason the tree's cursor
/// is not — see the AgentTreeViewState open question, which covers both.
pub struct DiffState {
    /// Index of the first visible line.
    offset: usize,
    /// Rows the document had to draw into at the last render — the pane's
    /// height less its two borders. Recorded by [`render`], because the
    /// half-page motions are defined against the VISIBLE height and
    /// [`handle_key`] never sees a `Rect`.
    viewport_rows: usize,
    /// Whether a lone `g` is waiting for the second half of the `gg` chord.
    /// No deadline, exactly as in the tree pane: `g` is bound to nothing else
    /// here, so nothing is waiting for the chord to expire.
    pending_g: bool,
    /// A one-line failure notice from the last git query, rendered in the
    /// bottom border. While it is set the border is drawn in the error colour,
    /// for the same reason the tree's is: a document kept on screen after a
    /// failed query is indistinguishable from a correct one at a glance.
    pub notice: Option<String>,
}

impl Default for DiffState {
    fn default() -> Self {
        Self::new()
    }
}

impl DiffState {
    pub fn new() -> Self {
        Self {
            offset: 0,
            viewport_rows: 0,
            pending_g: false,
            notice: None,
        }
    }

    pub fn offset(&self) -> usize {
        self.offset
    }

    /// How far `Ctrl-D`/`Ctrl-U` move: half the last-rendered visible height,
    /// floored at one row. A pane too short to show two rows would otherwise
    /// halve to zero and turn both motions into no-ops, which reads as a broken
    /// key rather than as a small pane. Same rule as the tree pane's.
    fn half_page(&self) -> usize {
        (self.viewport_rows / 2).max(1)
    }

    /// The furthest the document can scroll: far enough to put its last line on
    /// screen, and no further. Scrolling past the end into blank rows would let
    /// the user lose the document entirely and have to guess their way back.
    fn max_offset(&self, line_count: usize) -> usize {
        line_count.saturating_sub(self.viewport_rows)
    }

    fn scroll_to(&mut self, target: usize, line_count: usize) {
        self.offset = target.min(self.max_offset(line_count));
    }
}

/// What the event loop should do after [`handle_key`] has processed a key.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum DiffKeyAction {
    /// Stay in the loop and redraw.
    Continue,
    /// Leave the loop, which exits the process and so closes the tmux pane.
    ///
    /// The open set is deliberately NOT cleared on the way out: the user closed
    /// this pane, not their selection, and the next toggle in the tree brings
    /// it back with everything still open. See SplitAgentTreeDiffPane in
    /// docs/specs/agent-tree.allium.
    Exit,
}

/// Handle one key press. Pure: it moves the offset and nothing else.
///
/// The same vocabulary the tree pane uses for the same motions, so moving
/// between the two panes does not mean changing keyboards. What it does NOT
/// have is any way to open or close a file — Space, Enter and the all-files key
/// do nothing here, because the open set is decided in the tree and only in the
/// tree (`DiffPaneHasNoToggleOfItsOwn`).
pub fn handle_key(state: &mut DiffState, line_count: usize, key: KeyEvent) -> DiffKeyAction {
    // Any key acknowledges a notice, exactly as in the tree pane: the next
    // keypress is the earliest moment the user has demonstrably seen it.
    state.notice = None;

    // `gg`: a lone `g` arms the chord and does nothing else. Any other key
    // disarms it and is then handled normally, so a lone `g` is never
    // observable as anything but a swallowed keystroke.
    if key.code == KeyCode::Char('g') && !key.modifiers.contains(KeyModifiers::CONTROL) {
        if state.pending_g {
            state.pending_g = false;
            state.offset = 0;
        } else {
            state.pending_g = true;
        }
        return DiffKeyAction::Continue;
    }
    state.pending_g = false;

    let half_page = state.half_page();
    match key.code {
        KeyCode::Char('q') => return DiffKeyAction::Exit,
        KeyCode::Char('c') if key.modifiers.contains(KeyModifiers::CONTROL) => {
            return DiffKeyAction::Exit
        }
        KeyCode::Char('j') | KeyCode::Down => {
            state.scroll_to(state.offset.saturating_add(1), line_count);
        }
        KeyCode::Char('k') | KeyCode::Up => {
            state.offset = state.offset.saturating_sub(1);
        }
        KeyCode::Char('d') if key.modifiers.contains(KeyModifiers::CONTROL) => {
            state.scroll_to(state.offset.saturating_add(half_page), line_count);
        }
        KeyCode::Char('u') if key.modifiers.contains(KeyModifiers::CONTROL) => {
            state.offset = state.offset.saturating_sub(half_page);
        }
        KeyCode::Char('G') => {
            state.offset = state.max_offset(line_count);
        }
        _ => {}
    }
    DiffKeyAction::Continue
}

/// Draw the document into `area`.
///
/// Lines wider than the pane are TRUNCATED, not wrapped. The pane is narrow by
/// design — it inherits the tree's column — so one long line would wrap into
/// many rows and push several files' diffs off screen, making the cost of a
/// single minified or generated line fall on everything the user opened
/// alongside it. Truncation costs only the line it happens to. See
/// `DiffLinesTruncateRatherThanWrap` in docs/specs/agent-tree.allium.
pub fn render(frame: &mut Frame, area: Rect, lines: &[DiffLine], state: &mut DiffState) {
    // Two borders, so the drawable height is the pane's less two. Recorded for
    // the half-page motions, which only the renderer knows the height for.
    state.viewport_rows = usize::from(area.height).saturating_sub(2);
    // A pane that shrank under the user can leave the offset past the end.
    state.offset = state.offset.min(state.max_offset(lines.len()));

    let border_style = if state.notice.is_some() {
        Style::default().fg(RED)
    } else {
        Style::default().fg(FG)
    };
    let mut block = Block::default()
        .borders(Borders::ALL)
        .border_style(border_style)
        .title("diff");
    if let Some(notice) = &state.notice {
        block = block.title_bottom(notice.as_str());
    }

    let visible: Vec<Line> = lines
        .iter()
        .skip(state.offset)
        .take(state.viewport_rows)
        .map(|line| {
            let style = match line.kind {
                DiffLineKind::Heading => Style::default().fg(FG).add_modifier(Modifier::BOLD),
                DiffLineKind::Added => Style::default().fg(GREEN),
                DiffLineKind::Removed => Style::default().fg(RED),
                DiffLineKind::Meta => Style::default().fg(YELLOW),
                DiffLineKind::Context => Style::default().fg(FG),
                DiffLineKind::Refusal => Style::default().fg(YELLOW).add_modifier(Modifier::ITALIC),
            };
            Line::from(Span::styled(line.text.clone(), style))
        })
        .collect();

    // No `.wrap(..)`: absence is the truncation, and it is load-bearing — see
    // the doc comment above.
    frame.render_widget(Paragraph::new(visible).block(block), area);
}

/// A cheap fingerprint of what the open files currently look like to git.
///
/// One `--numstat` over every open path, which git answers from the index
/// without materialising a patch. Comparing it tick to tick is what lets the
/// pane skip the per-file diffs — the expensive part — when nothing has moved,
/// so the steady-state cost of a one-second poll is two short git processes
/// however many files are open. The same "only rebuild when the answer
/// changes" short-circuit the tree already applies to its own query.
///
/// Untracked paths never appear here and do not need to: their placeholder is
/// the same whatever the file says. A path that becomes tracked DOES change
/// this output, which is the transition that has to be caught.
fn open_files_fingerprint(
    root: &Path,
    baseline: &str,
    open: &BTreeSet<PathBuf>,
    runner: &dyn ProcessRunner,
) -> Result<String> {
    let root = root.to_string_lossy().into_owned();
    let paths: Vec<String> = open
        .iter()
        .map(|p| p.to_string_lossy().into_owned())
        .collect();
    let mut args: Vec<&str> = vec![
        "-C",
        &root,
        "diff",
        "--numstat",
        "--no-renames",
        "-z",
        baseline,
        "--",
    ];
    args.extend(paths.iter().map(String::as_str));
    run_git(runner, &args)
}

/// What the last successful refresh saw, so the next one can tell whether
/// anything moved.
#[derive(Default)]
struct LastSeen {
    open: BTreeSet<PathBuf>,
    fingerprint: String,
}

/// One refresh pass: re-read the open set, and rebuild the document only if it
/// or the files in it have moved.
///
/// A failed git query leaves the document untouched and sets a notice, the same
/// way the tree keeps its last good tree: the commonest failure is a transient
/// index lock taken by the agent's own git, and blanking the pane on that would
/// make it flicker empty exactly when the user most wants to read it.
fn refresh(
    root: &Path,
    base_branch: &str,
    runner: &dyn ProcessRunner,
    last: &mut LastSeen,
    lines: &mut Vec<DiffLine>,
    state: &mut DiffState,
) {
    let open = crate::agent_tree_open_set::read_open_set(&root.to_string_lossy());

    if open.is_empty() {
        state.notice = None;
        last.open.clear();
        last.fingerprint.clear();
        lines.clear();
        return;
    }

    let rebuilt = (|| -> Result<Option<Vec<DiffLine>>> {
        let baseline = fork_point(&root.to_string_lossy(), base_branch, runner)?;
        let fingerprint = open_files_fingerprint(root, &baseline, &open, runner)?;
        if open == last.open && fingerprint == last.fingerprint {
            return Ok(None);
        }
        let untracked = untracked_paths(root, runner)?;
        let document = build_document(root, &baseline, &open, &untracked, runner)?;
        last.open = open.clone();
        last.fingerprint = fingerprint;
        Ok(Some(document.lines()))
    })();

    match rebuilt {
        Ok(Some(fresh)) => {
            state.notice = None;
            *lines = fresh;
        }
        Ok(None) => state.notice = None,
        Err(e) => {
            tracing::warn!(
                root = %root.display(),
                base_branch,
                error = %e,
                "agent-diff: git query failed, keeping the last good document"
            );
            // `{:#}`, not `{}`: anyhow's plain Display prints only the outermost
            // context, so a git that could not be spawned — or that overran
            // GIT_TIMEOUT — would put the bare word "git" in the border.
            state.notice = Some(format!("{e:#}"));
        }
    }
}

fn run_loop<B: Backend>(
    terminal: &mut Terminal<B>,
    root: &Path,
    base_branch: &str,
    runner: &dyn ProcessRunner,
) -> Result<()> {
    let mut state = DiffState::new();
    let mut lines: Vec<DiffLine> = Vec::new();
    let mut last = LastSeen::default();

    // Draw the empty pane BEFORE the first query, for the same reason the tree
    // does: git runs inline in this single-threaded loop, and one frame of an
    // empty bordered pane is a better answer than whatever tmux left in the
    // cell.
    terminal.draw(|frame| render(frame, frame.area(), &lines, &mut state))?;
    refresh(root, base_branch, runner, &mut last, &mut lines, &mut state);

    loop {
        terminal.draw(|frame| render(frame, frame.area(), &lines, &mut state))?;

        if event::poll(REFRESH_INTERVAL)? {
            let Event::Key(key) = event::read()? else {
                continue;
            };
            if key.kind != KeyEventKind::Press {
                continue;
            }
            match handle_key(&mut state, lines.len(), key) {
                DiffKeyAction::Exit => return Ok(()),
                DiffKeyAction::Continue => {}
            }
            continue;
        }

        refresh(root, base_branch, runner, &mut last, &mut lines, &mut state);
    }
}

/// `dispatch agent-diff <task_id>`: render the diffs of whatever the task's
/// agent-tree pane currently has open.
///
/// Takes the task id rather than a worktree path so the two panes cannot
/// disagree about which worktree they are looking at, and so this pane resolves
/// the baseline from the same `base_branch` the tree does.
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
    use crate::cli::agent_tree::GIT_TIMEOUT;
    use crate::process::MockProcessRunner;

    const BASELINE: &str = "1111111111111111111111111111111111111111";

    fn no_untracked() -> BTreeSet<PathBuf> {
        BTreeSet::new()
    }

    fn untracked_set(paths: &[&str]) -> BTreeSet<PathBuf> {
        paths.iter().map(PathBuf::from).collect()
    }

    fn diff_rig(stdout: &str) -> MockProcessRunner {
        MockProcessRunner::new(vec![MockProcessRunner::ok_with_stdout(stdout.as_bytes())])
    }

    const A_PATCH: &str = "diff --git a/a.rs b/a.rs\n@@ -1 +1 @@\n-old\n+new\n";

    #[test]
    fn a_tracked_files_diff_comes_back_as_its_body() {
        let runner = diff_rig(A_PATCH);

        let diff = file_diff(
            Path::new("/wt"),
            BASELINE,
            Path::new("a.rs"),
            &no_untracked(),
            &runner,
        )
        .unwrap()
        .unwrap();

        assert_eq!(diff.body.as_deref(), Some(A_PATCH));
        assert_eq!(diff.refusal, None);
    }

    /// The same baseline the tree resolved, and `--` so a path that looks like
    /// a ref is still read as a path.
    #[test]
    fn the_diff_is_taken_against_the_callers_baseline() {
        let runner = diff_rig(A_PATCH);

        file_diff(
            Path::new("/wt"),
            BASELINE,
            Path::new("main"),
            &no_untracked(),
            &runner,
        )
        .unwrap();

        assert_eq!(
            runner.flattened_calls(),
            vec![format!("git -C /wt diff --no-renames {BASELINE} -- main")]
        );
    }

    /// An untracked file is invisible to a diff against the index, so it is not
    /// even asked about — a diff would come back empty and be indistinguishable
    /// from a file the agent reverted.
    #[test]
    fn an_untracked_file_is_refused_without_running_git() {
        let runner = MockProcessRunner::new(vec![]);

        let diff = file_diff(
            Path::new("/wt"),
            BASELINE,
            Path::new("new.rs"),
            &untracked_set(&["new.rs"]),
            &runner,
        )
        .unwrap()
        .unwrap();

        assert_eq!(diff.refusal, Some(DiffRefusal::Untracked));
        assert_eq!(diff.body, None);
        assert!(runner.flattened_calls().is_empty());
    }

    #[test]
    fn a_binary_file_is_refused_with_its_own_reason() {
        let runner = diff_rig(
            "diff --git a/logo.png b/logo.png\nBinary files a/logo.png and b/logo.png differ\n",
        );

        let diff = file_diff(
            Path::new("/wt"),
            BASELINE,
            Path::new("logo.png"),
            &no_untracked(),
            &runner,
        )
        .unwrap()
        .unwrap();

        assert_eq!(diff.refusal, Some(DiffRefusal::Binary));
    }

    /// The notice is matched at the START of a line, so a patch that merely
    /// contains the sentence — a diff of this very file, say — is still shown
    /// as a patch.
    #[test]
    fn a_patch_mentioning_the_binary_notice_is_not_mistaken_for_one() {
        let body = "diff --git a/x.rs b/x.rs\n@@ -1 +1 @@\n+// Binary files a and b differ\n";
        let runner = diff_rig(body);

        let diff = file_diff(
            Path::new("/wt"),
            BASELINE,
            Path::new("x.rs"),
            &no_untracked(),
            &runner,
        )
        .unwrap()
        .unwrap();

        assert_eq!(diff.refusal, None);
        assert_eq!(diff.body.as_deref(), Some(body));
    }

    #[test]
    fn a_diff_over_the_cap_is_refused_rather_than_rendered() {
        let huge = format!(
            "diff --git a/big.rs b/big.rs\n{}",
            "+x\n".repeat(DIFF_MAX_BYTES)
        );
        let runner = diff_rig(&huge);

        let diff = file_diff(
            Path::new("/wt"),
            BASELINE,
            Path::new("big.rs"),
            &no_untracked(),
            &runner,
        )
        .unwrap()
        .unwrap();

        assert_eq!(diff.refusal, Some(DiffRefusal::TooLarge));
        assert_eq!(diff.body, None);
    }

    /// A path the user opened and the agent then reverted. Not an error, not a
    /// placeholder, and NOT a reason to close it — see
    /// OpenDiffPathsMaySurviveTheirFiles in docs/specs/agent-tree.allium.
    #[test]
    fn a_path_git_no_longer_reports_shows_nothing_at_all() {
        let runner = diff_rig("");

        let diff = file_diff(
            Path::new("/wt"),
            BASELINE,
            Path::new("reverted.rs"),
            &no_untracked(),
            &runner,
        )
        .unwrap();

        assert_eq!(diff, None);
    }

    #[test]
    fn a_git_failure_is_an_error_not_a_silently_empty_diff() {
        let runner = MockProcessRunner::new(vec![MockProcessRunner::fail("fatal: bad object")]);

        let err = file_diff(
            Path::new("/wt"),
            BASELINE,
            Path::new("a.rs"),
            &no_untracked(),
            &runner,
        )
        .expect_err("a failing git must not read as an empty diff");

        assert!(format!("{err:#}").contains("bad object"), "got {err:#}");
    }

    #[test]
    fn every_diff_query_is_bounded_by_the_shared_timeout() {
        let runner = diff_rig(A_PATCH);
        file_diff(
            Path::new("/wt"),
            BASELINE,
            Path::new("a.rs"),
            &no_untracked(),
            &runner,
        )
        .unwrap();
        assert_eq!(runner.recorded_timeouts(), vec![Some(GIT_TIMEOUT)]);
    }

    // -- untracked_paths ---------------------------------------------------

    #[test]
    fn the_untracked_listing_is_taken_once_for_the_whole_tick() {
        let runner = diff_rig("new.rs\0docs/my notes.md\0");

        let paths = untracked_paths(Path::new("/wt"), &runner).unwrap();

        assert_eq!(paths, untracked_set(&["new.rs", "docs/my notes.md"]));
        assert_eq!(
            runner.flattened_calls(),
            vec!["git -C /wt ls-files --others --exclude-standard -z".to_string()]
        );
    }
}

#[cfg(test)]
#[allow(clippy::unwrap_used, clippy::expect_used)]
mod document_tests {
    use super::*;
    use crate::process::MockProcessRunner;

    const BASELINE: &str = "1111111111111111111111111111111111111111";

    fn open_set(paths: &[&str]) -> BTreeSet<PathBuf> {
        paths.iter().map(PathBuf::from).collect()
    }

    fn patch(path: &str) -> String {
        format!("diff --git a/{path} b/{path}\n@@ -1 +1 @@\n-old\n+new\n")
    }

    fn rig(outputs: &[&str]) -> MockProcessRunner {
        MockProcessRunner::new(
            outputs
                .iter()
                .map(|o| MockProcessRunner::ok_with_stdout(o.as_bytes()))
                .collect(),
        )
    }

    fn texts(doc: &DiffDocument) -> Vec<String> {
        doc.lines().into_iter().map(|l| l.text).collect()
    }

    /// Tree order, not the order the user opened them in: the tree is the index
    /// this pane is read through, so the two must scroll the same way.
    #[test]
    fn files_render_in_tree_order_under_their_own_headings() {
        let runner = rig(&[&patch("a.rs"), &patch("src/lib.rs")]);

        let doc = build_document(
            Path::new("/wt"),
            BASELINE,
            &open_set(&["src/lib.rs", "a.rs"]),
            &BTreeSet::new(),
            &runner,
        )
        .unwrap();

        let headings: Vec<_> = doc.files.iter().map(|f| f.path.clone()).collect();
        assert_eq!(
            headings,
            vec![PathBuf::from("a.rs"), PathBuf::from("src/lib.rs")]
        );
        assert_eq!(texts(&doc)[0], "a.rs");
    }

    #[test]
    fn a_refused_file_renders_its_reason_in_place_of_contents() {
        let runner = rig(&[]);

        let doc = build_document(
            Path::new("/wt"),
            BASELINE,
            &open_set(&["new.rs"]),
            &open_set(&["new.rs"]),
            &runner,
        )
        .unwrap();

        let lines = texts(&doc);
        assert_eq!(lines[0], "new.rs");
        assert!(lines[1].contains("not yet staged"), "got {lines:?}");
    }

    /// One refused file must not cost the user the diffs either side of it —
    /// which is why a refusal rides on the file rather than on the pane.
    #[test]
    fn a_refusal_does_not_stop_the_files_around_it_rendering() {
        let runner = rig(&[&patch("a.rs"), &patch("z.rs")]);

        let doc = build_document(
            Path::new("/wt"),
            BASELINE,
            &open_set(&["a.rs", "new.rs", "z.rs"]),
            &open_set(&["new.rs"]),
            &runner,
        )
        .unwrap();

        assert_eq!(doc.files.len(), 3);
        assert!(doc.files[0].body.is_some());
        assert_eq!(doc.files[1].refusal, Some(DiffRefusal::Untracked));
        assert!(doc.files[2].body.is_some());
    }

    /// The agent reverted a file the user had open. It renders nothing and
    /// stays open — see OpenDiffPathsMaySurviveTheirFiles in the spec.
    #[test]
    fn a_path_with_nothing_to_show_contributes_no_section() {
        let runner = rig(&["", &patch("b.rs")]);

        let doc = build_document(
            Path::new("/wt"),
            BASELINE,
            &open_set(&["a.rs", "b.rs"]),
            &BTreeSet::new(),
            &runner,
        )
        .unwrap();

        assert_eq!(doc.files.len(), 1);
        assert_eq!(doc.files[0].path, PathBuf::from("b.rs"));
    }

    #[test]
    fn an_empty_open_set_builds_an_empty_document() {
        let runner = rig(&[]);
        let doc = build_document(
            Path::new("/wt"),
            BASELINE,
            &BTreeSet::new(),
            &BTreeSet::new(),
            &runner,
        )
        .unwrap();
        assert!(doc.lines().is_empty());
    }

    #[test]
    fn added_and_removed_lines_are_classified_apart_from_the_file_headers() {
        let runner = rig(&[&patch("a.rs")]);
        let doc = build_document(
            Path::new("/wt"),
            BASELINE,
            &open_set(&["a.rs"]),
            &BTreeSet::new(),
            &runner,
        )
        .unwrap();

        let kinds: Vec<_> = doc.lines().into_iter().map(|l| l.kind).collect();
        assert_eq!(
            kinds,
            vec![
                DiffLineKind::Heading,
                DiffLineKind::Meta,
                DiffLineKind::Meta,
                DiffLineKind::Removed,
                DiffLineKind::Added,
            ]
        );
    }

    /// `+++` and `---` open git's file headers. Reading them as content would
    /// paint two lines of every single diff the wrong colour.
    #[test]
    fn the_triple_dash_file_headers_are_not_read_as_content() {
        assert_eq!(DiffLineKind::of("--- a/x.rs"), DiffLineKind::Meta);
        assert_eq!(DiffLineKind::of("+++ b/x.rs"), DiffLineKind::Meta);
        assert_eq!(DiffLineKind::of("-old"), DiffLineKind::Removed);
        assert_eq!(DiffLineKind::of("+new"), DiffLineKind::Added);
        assert_eq!(DiffLineKind::of(" same"), DiffLineKind::Context);
    }

    #[test]
    fn a_git_failure_fails_the_whole_build() {
        let runner = MockProcessRunner::new(vec![MockProcessRunner::fail("fatal: bad object")]);
        assert!(build_document(
            Path::new("/wt"),
            BASELINE,
            &open_set(&["a.rs"]),
            &BTreeSet::new(),
            &runner,
        )
        .is_err());
    }
}

#[cfg(test)]
#[allow(clippy::unwrap_used, clippy::expect_used)]
mod view_tests {
    use super::*;
    use ratatui::backend::TestBackend;
    use ratatui::Terminal;

    /// A pane with `rows` of height, already drawn once — the half-page motions
    /// resolve against the LAST render's height, so a key test has to draw
    /// before pressing anything.
    struct Rig {
        lines: Vec<DiffLine>,
        state: DiffState,
        terminal: Terminal<TestBackend>,
    }

    impl Rig {
        fn new(line_count: usize, rows: u16) -> Self {
            let lines = (0..line_count)
                .map(|n| DiffLine {
                    kind: DiffLineKind::Context,
                    text: format!("line{n:02}"),
                })
                .collect();
            let mut rig = Self {
                lines,
                state: DiffState::new(),
                terminal: Terminal::new(TestBackend::new(40, rows)).unwrap(),
            };
            rig.draw();
            rig
        }

        fn draw(&mut self) {
            let lines = &self.lines;
            let state = &mut self.state;
            self.terminal
                .draw(|frame| render(frame, frame.area(), lines, state))
                .unwrap();
        }

        fn press(&mut self, code: KeyCode) -> DiffKeyAction {
            self.press_with(code, KeyModifiers::NONE)
        }

        fn press_ctrl(&mut self, code: KeyCode) -> DiffKeyAction {
            self.press_with(code, KeyModifiers::CONTROL)
        }

        fn press_with(&mut self, code: KeyCode, modifiers: KeyModifiers) -> DiffKeyAction {
            let action = handle_key(
                &mut self.state,
                self.lines.len(),
                KeyEvent::new(code, modifiers),
            );
            self.draw();
            action
        }

        fn rendered(&self) -> String {
            let buffer = self.terminal.backend().buffer();
            let area = buffer.area();
            (0..area.height)
                .map(|y| {
                    (0..area.width)
                        .map(|x| buffer[(x, y)].symbol())
                        .collect::<String>()
                })
                .collect::<Vec<_>>()
                .join("\n")
        }
    }

    // -- exit ---------------------------------------------------------------

    #[test]
    fn q_and_ctrl_c_leave_the_renderer() {
        let mut rig = Rig::new(50, 10);
        assert_eq!(rig.press(KeyCode::Char('q')), DiffKeyAction::Exit);
        assert_eq!(rig.press_ctrl(KeyCode::Char('c')), DiffKeyAction::Exit);
    }

    #[test]
    fn a_bare_c_does_not_leave_the_renderer() {
        let mut rig = Rig::new(50, 10);
        assert_eq!(rig.press(KeyCode::Char('c')), DiffKeyAction::Continue);
    }

    // -- scrolling ----------------------------------------------------------

    #[test]
    fn j_and_down_both_scroll_one_line() {
        for code in [KeyCode::Char('j'), KeyCode::Down] {
            let mut rig = Rig::new(50, 10);
            rig.press(code);
            assert_eq!(rig.state.offset(), 1, "{code:?}");
        }
    }

    #[test]
    fn k_and_up_both_scroll_back_one_line() {
        for code in [KeyCode::Char('k'), KeyCode::Up] {
            let mut rig = Rig::new(50, 10);
            rig.press(KeyCode::Char('j'));
            rig.press(KeyCode::Char('j'));
            rig.press(code);
            assert_eq!(rig.state.offset(), 1, "{code:?}");
        }
    }

    /// Half of the VISIBLE height, which is the pane less its two borders — so
    /// a 10-row pane moves by four, and dragging the pane taller moves further.
    #[test]
    fn ctrl_d_and_ctrl_u_move_half_the_visible_height() {
        let mut rig = Rig::new(50, 10);
        rig.press_ctrl(KeyCode::Char('d'));
        assert_eq!(rig.state.offset(), 4);
        rig.press_ctrl(KeyCode::Char('u'));
        assert_eq!(rig.state.offset(), 0);
    }

    /// A pane too short to show two rows still moves by one. Halving to zero
    /// would read as a broken key rather than as a small pane.
    #[test]
    fn a_pane_too_short_to_halve_still_moves_by_one() {
        let mut rig = Rig::new(50, 3);
        rig.press_ctrl(KeyCode::Char('d'));
        assert_eq!(rig.state.offset(), 1);
    }

    // -- clamping -----------------------------------------------------------

    /// Scrolling past the end into blank rows would let the user lose the
    /// document and have to guess their way back.
    #[test]
    fn scrolling_down_stops_with_the_last_line_on_screen() {
        let mut rig = Rig::new(12, 10);
        for _ in 0..50 {
            rig.press(KeyCode::Char('j'));
        }
        // 12 lines, 8 visible rows: the furthest useful offset is 4.
        assert_eq!(rig.state.offset(), 4);
        assert!(rig.rendered().contains("line11"), "{}", rig.rendered());
    }

    #[test]
    fn scrolling_up_stops_at_the_top() {
        let mut rig = Rig::new(50, 10);
        for _ in 0..50 {
            rig.press(KeyCode::Char('k'));
        }
        assert_eq!(rig.state.offset(), 0);
    }

    /// A document shorter than the pane cannot scroll at all.
    #[test]
    fn a_document_that_fits_does_not_scroll() {
        let mut rig = Rig::new(3, 20);
        rig.press(KeyCode::Char('j'));
        rig.press_ctrl(KeyCode::Char('d'));
        rig.press(KeyCode::Char('G'));
        assert_eq!(rig.state.offset(), 0);
    }

    #[test]
    fn an_empty_document_cannot_scroll() {
        let mut rig = Rig::new(0, 10);
        rig.press(KeyCode::Char('G'));
        assert_eq!(rig.state.offset(), 0);
    }

    // -- jumps --------------------------------------------------------------

    #[test]
    fn capital_g_jumps_to_the_end() {
        let mut rig = Rig::new(12, 10);
        rig.press(KeyCode::Char('G'));
        assert_eq!(rig.state.offset(), 4);
    }

    #[test]
    fn gg_jumps_to_the_top() {
        let mut rig = Rig::new(50, 10);
        rig.press(KeyCode::Char('G'));
        assert!(rig.state.offset() > 0);

        rig.press(KeyCode::Char('g'));
        assert_eq!(rig.press(KeyCode::Char('g')), DiffKeyAction::Continue);
        assert_eq!(rig.state.offset(), 0);
    }

    /// A lone `g` arms the chord and moves nothing, and it never expires —
    /// exactly as in the tree pane (AgentTreeGgChordNeverExpires).
    #[test]
    fn a_lone_g_moves_nothing() {
        let mut rig = Rig::new(50, 10);
        rig.press(KeyCode::Char('j'));
        rig.press(KeyCode::Char('g'));
        assert_eq!(rig.state.offset(), 1);
    }

    /// Any other key disarms the chord and is then handled normally, so the
    /// swallowed `g` is the only trace a lone press leaves.
    #[test]
    fn a_key_between_the_two_gs_disarms_the_chord() {
        let mut rig = Rig::new(50, 10);
        rig.press(KeyCode::Char('G'));
        let before = rig.state.offset();

        rig.press(KeyCode::Char('g'));
        rig.press(KeyCode::Char('k'));
        rig.press(KeyCode::Char('g'));

        assert_eq!(rig.state.offset(), before - 1);
    }

    // -- the pane has no toggle of its own ----------------------------------

    /// Space, Enter and the all-files key do nothing here. The open set is
    /// decided in the tree and only in the tree, which is what makes one file
    /// have one open state — see DiffPaneHasNoToggleOfItsOwn in the spec.
    #[test]
    fn the_trees_toggle_keys_do_nothing_in_this_pane() {
        for code in [KeyCode::Char(' '), KeyCode::Enter, KeyCode::Char('a')] {
            let mut rig = Rig::new(50, 10);
            rig.press(KeyCode::Char('j'));
            assert_eq!(rig.press(code), DiffKeyAction::Continue, "{code:?}");
            assert_eq!(rig.state.offset(), 1, "{code:?}");
        }
    }

    // -- notices ------------------------------------------------------------

    #[test]
    fn a_notice_is_shown_and_cleared_by_the_next_key() {
        let mut rig = Rig::new(50, 10);
        rig.state.notice = Some("git: index.lock".to_string());
        rig.draw();
        assert!(rig.rendered().contains("index.lock"), "{}", rig.rendered());

        rig.press(KeyCode::Char('j'));
        assert!(rig.state.notice.is_none());
    }

    // -- truncation ---------------------------------------------------------

    /// A long line is cut at the pane's edge, not wrapped onto a second row.
    /// Wrapping would let one minified line push several files off screen — see
    /// DiffLinesTruncateRatherThanWrap in docs/specs/agent-tree.allium.
    #[test]
    fn a_long_line_is_truncated_rather_than_wrapped() {
        let mut rig = Rig::new(0, 6);
        rig.lines = vec![
            DiffLine {
                kind: DiffLineKind::Added,
                text: format!("+{}", "x".repeat(200)),
            },
            DiffLine {
                kind: DiffLineKind::Context,
                text: "second".to_string(),
            },
        ];
        rig.draw();

        let rendered = rig.rendered();
        assert!(
            rendered.contains("second"),
            "the next line must still be on screen; got:\n{rendered}"
        );
    }
}
