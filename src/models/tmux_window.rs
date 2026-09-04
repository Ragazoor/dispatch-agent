//! The `task-<id>` window-name convention, and the [`TmuxWindow`] type that
//! carries it.
//!
//! The same `task-<id>` string names two things: the tmux window a dispatched
//! agent runs in, and that agent's native cross-session-messaging session
//! (`dispatch::agents::session_name_flag`). Both the dispatch adapter and
//! `TaskService::record_peer_message_sent` need to build and parse it, so the
//! convention lives here in the domain model rather than in either consumer —
//! a service reaching into the adapter for a pure predicate inverts the
//! layering, and a second copy of `strip_prefix("task-")` could drift from
//! this one.

use std::borrow::Cow;
use std::fmt;

use super::TaskId;

/// The `task-` prefix every dispatched agent's window name carries.
const TASK_WINDOW_PREFIX: &str = "task-";

/// The prefix the board's own popped-out editor windows carry.
const EDITOR_WINDOW_PREFIX: &str = "dispatch-edit-";

/// Whether `s` is a tmux **pane** ID (`%N`) rather than a window name.
///
/// A pane ID is `%` followed by digits, and nothing else counts: a window
/// *can* be named `%foo`, and such a name must take the normal name-resolution
/// path rather than be passed through to tmux as if it were an ID. Lives here
/// rather than in `crate::tmux` so [`TmuxWindow::parse`] and
/// `tmux::is_resolved_target` cannot disagree about what a pane ID looks like.
///
/// `const` so [`TmuxWindow::from_static`] can reject one at compile time; the
/// byte loop is the const-compatible spelling of `strip_prefix` + `all`.
pub(crate) const fn is_pane_id(s: &str) -> bool {
    let bytes = s.as_bytes();
    if bytes.len() < 2 || bytes[0] != b'%' {
        return false;
    }
    let mut i = 1;
    while i < bytes.len() {
        if !bytes[i].is_ascii_digit() {
            return false;
        }
        i += 1;
    }
    true
}

/// A tmux window **name** — the board's own window, an editor window, or a
/// dispatched agent's `task-<id>`.
///
/// Exists as a type rather than a `String` because tmux resolves a bare
/// `-t <name>` target by **prefix**, so `task-4` will act on `task-42`'s
/// window once the intended one is gone. Constructing one is therefore a claim
/// that the string is a whole, valid window name — see `tmux::window_target`,
/// which every name-taking helper calls to resolve it to a pane ID before use.
///
/// It is deliberately *not* limited to `task-<id>`: the TUI's own window and
/// the editor windows are window names too, and the prefix
/// hazard applies to them identically. What it does exclude is the two things
/// that are not window names — the empty string (tmux's "current window") and a
/// pane ID (`%3`) — both of which bypass name resolution and so must never be
/// mistaken for one. [`crate::tmux::window_target`] keeps taking `&str`
/// precisely because it accepts those two as well.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct TmuxWindow(Cow<'static, str>);

impl TmuxWindow {
    /// The canonical constructor: the window name dispatch gives task `task_id`.
    pub fn for_task(task_id: TaskId) -> Self {
        Self(Cow::Owned(format!("{TASK_WINDOW_PREFIX}{task_id}")))
    }

    /// Wrap a window name that is a literal in this crate's own source — the
    /// board's `TUI` window. `'static` proves the argument
    /// cannot come from the database, from tmux, or from an MCP request;
    /// anything derived from data goes through [`parse`] instead.
    ///
    /// `'static` proves provenance, not validity, so this still checks the two
    /// exclusions — as a `const fn`, which is the point: the call site is
    /// a `const` item, so an empty or pane-ID literal is a compile error rather
    /// than a panic. Nothing calls this outside a `const` context, so the
    /// `assert!`s have no runtime path to reach.
    ///
    /// [`parse`]: TmuxWindow::parse
    pub const fn from_static(name: &'static str) -> Self {
        assert!(!name.is_empty(), "a tmux window name must not be empty");
        assert!(
            !is_pane_id(name),
            "a tmux pane ID is not a window name — see TmuxWindow::parse"
        );
        Self(Cow::Borrowed(name))
    }

    /// The window name for a popped-out editor, disambiguated by `nanos`.
    ///
    /// Lives beside [`for_task`] rather than in `crate::runtime::editor` for
    /// the same reason that one does: this module owns every window-naming
    /// convention, so no consumer has to spell a prefix itself. Infallible by
    /// construction — the literal prefix is a window name and appending digits
    /// cannot empty it or turn it into `%<digits>`.
    ///
    /// [`for_task`]: TmuxWindow::for_task
    pub fn for_editor(nanos: u128) -> Self {
        Self(Cow::Owned(format!("{EDITOR_WINDOW_PREFIX}{nanos}")))
    }

    /// Accept a name read back from the database or from tmux itself.
    ///
    /// `None` for the two strings that are not window names: the empty string
    /// and a pane ID (see [`is_pane_id`]). A `None` here is a soft failure at
    /// the DB boundary — the task reads back as owning no window — never an
    /// `unwrap()`.
    pub fn parse(s: &str) -> Option<Self> {
        Self::from_owned(s.to_string()).ok()
    }

    /// [`parse`] for a name the caller already owns — the DB read path, which
    /// gets a `String` out of rusqlite and would otherwise re-allocate it.
    /// Hands the string back on rejection so the caller can log the value.
    ///
    /// [`parse`]: TmuxWindow::parse
    pub fn from_owned(name: String) -> Result<Self, String> {
        if name.is_empty() || is_pane_id(&name) {
            return Err(name);
        }
        Ok(Self(Cow::Owned(name)))
    }

    /// The name as tmux sees it.
    pub fn as_str(&self) -> &str {
        &self.0
    }

    /// The name as an owned `String`, for the DB write boundary. Consumes the
    /// `Cow` rather than copying out of it.
    pub fn into_string(self) -> String {
        self.0.into_owned()
    }

    /// The task this window belongs to, or `None` for any window that isn't a
    /// task-agent window (the board's own TUI window, an editor window,
    /// anything else).
    pub fn task_id(&self) -> Option<TaskId> {
        self.0.strip_prefix(TASK_WINDOW_PREFIX)?.parse().ok()
    }
}

/// Compare a window against a literal name without unwrapping it first.
///
/// Comparison-only sugar, mirroring `String: PartialEq<str>`: it can read a
/// name but never construct one, so it does not weaken the guarantee that every
/// `TmuxWindow` in existence went through [`TmuxWindow::for_task`],
/// [`TmuxWindow::parse`] or a `'static` constructor.
impl PartialEq<str> for TmuxWindow {
    fn eq(&self, other: &str) -> bool {
        self.0 == other
    }
}

impl fmt::Display for TmuxWindow {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(&self.0)
    }
}

/// Test-only shorthand for [`TmuxWindow::parse`] on a name the test itself
/// knows is valid.
///
/// Panicking is the right failure mode here: an invalid literal in a test is a
/// bug in the test, not a runtime condition. Gated behind `test-support` rather
/// than plain `cfg(test)` for the same reason `MockProcessRunner` is — the
/// `tests/` integration targets cannot see `cfg(test)` items.
#[cfg(any(test, feature = "test-support"))]
#[allow(clippy::expect_used)]
pub fn test_tmux_window(name: &str) -> TmuxWindow {
    TmuxWindow::parse(name).expect("test tmux window name must be a valid window name")
}

#[cfg(test)]
#[allow(clippy::unwrap_used, clippy::expect_used)]
mod tests {
    use super::*;

    #[test]
    fn for_task_uses_task_prefix() {
        assert_eq!(TmuxWindow::for_task(TaskId(42)).as_str(), "task-42");
    }

    #[test]
    fn task_id_roundtrips_with_for_task() {
        let window = TmuxWindow::for_task(TaskId(42));
        assert_eq!(window.task_id(), Some(TaskId(42)));
    }

    #[test]
    fn task_id_is_none_for_non_task_windows() {
        for name in ["TUI", "edit-window", "task-", "task-abc"] {
            let window = TmuxWindow::parse(name).expect("valid window name");
            assert_eq!(window.task_id(), None, "{name}");
        }
    }

    #[test]
    fn parse_accepts_ordinary_window_names() {
        for name in ["task-42", "TUI", "edit-window", "%foo", "edit-1"] {
            assert_eq!(
                TmuxWindow::parse(name).map(|w| w.as_str().to_string()),
                Some(name.to_string()),
                "{name}"
            );
        }
    }

    #[test]
    fn parse_rejects_the_empty_name() {
        assert_eq!(TmuxWindow::parse(""), None);
    }

    #[test]
    fn parse_rejects_pane_ids() {
        // A pane ID bypasses name resolution in tmux, so it is not a window
        // name and the prefix hazard this type guards does not apply to it.
        assert_eq!(TmuxWindow::parse("%3"), None);
        assert_eq!(TmuxWindow::parse("%0"), None);
    }

    #[test]
    fn from_owned_hands_a_rejected_name_back() {
        assert_eq!(
            TmuxWindow::from_owned("%3".to_string()),
            Err("%3".to_string())
        );
        assert_eq!(TmuxWindow::from_owned(String::new()), Err(String::new()));
        assert_eq!(
            TmuxWindow::from_owned("task-9".to_string()).map(|w| w.into_string()),
            Ok("task-9".to_string())
        );
    }

    #[test]
    fn into_string_returns_the_bare_name() {
        assert_eq!(TmuxWindow::for_task(TaskId(9)).into_string(), "task-9");
        assert_eq!(TmuxWindow::from_static("TUI").into_string(), "TUI");
    }

    #[test]
    fn is_pane_id_matches_only_percent_digits() {
        assert!(is_pane_id("%0"));
        assert!(is_pane_id("%12"));
        assert!(!is_pane_id("%"));
        assert!(!is_pane_id("%foo"));
        assert!(!is_pane_id("task-4"));
        assert!(!is_pane_id(""));
    }

    #[test]
    fn from_static_keeps_the_literal_verbatim() {
        const TUI: TmuxWindow = TmuxWindow::from_static("TUI");
        assert_eq!(TUI.as_str(), "TUI");
    }

    #[test]
    fn for_editor_is_a_valid_non_task_window() {
        let window = TmuxWindow::for_editor(17);
        assert_eq!(window.as_str(), "dispatch-edit-17");
        assert_eq!(window.task_id(), None);
        assert_eq!(TmuxWindow::parse(window.as_str()), Some(window));
    }

    #[test]
    fn compares_equal_to_its_own_name() {
        let window = TmuxWindow::for_task(TaskId(42));
        assert!(window == *"task-42");
        assert!(window != *"task-4");
    }

    #[test]
    fn display_renders_the_bare_name() {
        assert_eq!(format!("{}", TmuxWindow::for_task(TaskId(7))), "task-7");
    }
}
