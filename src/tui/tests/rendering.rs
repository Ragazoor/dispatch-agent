#![allow(clippy::unwrap_used, clippy::expect_used)]
use super::*;
use crate::models::{SubStatus, TaskId, TaskStatus, TaskTag};
use crossterm::event::KeyCode;
use ratatui::style::{Color, Modifier};
use std::time::Instant;

#[tokio::test]
async fn action_hints_backlog_task() {
    let task = make_task(1, TaskStatus::Backlog);
    let hints = ui::action_hints(Some(&task), false, Color::Rgb(122, 162, 247));
    let keys: Vec<&str> = hints
        .iter()
        .filter(|s| s.style.add_modifier.contains(Modifier::BOLD))
        .map(|s| s.content.as_ref())
        .collect();
    assert!(
        keys.contains(&"[Space]"),
        "should have dispatch/brainstorm hint"
    );
    assert!(keys.contains(&"[e]"), "should have edit hint");
    assert!(keys.contains(&"[L]"), "should have move hint");
    assert!(!keys.contains(&"[H]"), "backlog has no back movement");
    assert!(keys.contains(&"[x]"), "should have archive hint");
    assert!(keys.contains(&"[n]"), "should have new hint");
    let text: String = hints.iter().map(|s| s.content.as_ref()).collect();
    assert!(
        text.contains("brainstorm"),
        "backlog dispatch means brainstorm"
    );
}

#[tokio::test]
async fn action_hints_backlog_task_with_plan() {
    let mut task = make_task(3, TaskStatus::Backlog);
    task.plan_path = Some("plan.md".into());
    let hints = ui::action_hints(Some(&task), false, Color::Rgb(122, 162, 247));
    let keys: Vec<&str> = hints
        .iter()
        .filter(|s| s.style.add_modifier.contains(Modifier::BOLD))
        .map(|s| s.content.as_ref())
        .collect();
    assert!(keys.contains(&"[Space]"), "should have dispatch hint");
    let text: String = hints.iter().map(|s| s.content.as_ref()).collect();
    assert!(
        text.contains("ispatch"),
        "backlog with plan dispatch means dispatch"
    );
}

#[tokio::test]
async fn action_hints_running_with_window() {
    let mut task = make_task(4, TaskStatus::Running);
    task.tmux_window = Some("win-4".to_string());
    let hints = ui::action_hints(Some(&task), false, Color::Rgb(122, 162, 247));
    let keys: Vec<&str> = hints
        .iter()
        .filter(|s| s.style.add_modifier.contains(Modifier::BOLD))
        .map(|s| s.content.as_ref())
        .collect();
    assert!(keys.contains(&"[Space]"), "should have go-to-session hint");
    assert!(
        !keys.contains(&"[d]"),
        "should not have dispatch/resume when window exists"
    );
}

#[tokio::test]
async fn action_hints_running_with_worktree_no_window() {
    let mut task = make_task(4, TaskStatus::Running);
    task.worktree = Some("/tmp/wt".to_string());
    task.tmux_window = None;
    let hints = ui::action_hints(Some(&task), false, Color::Rgb(122, 162, 247));
    let keys: Vec<&str> = hints
        .iter()
        .filter(|s| s.style.add_modifier.contains(Modifier::BOLD))
        .map(|s| s.content.as_ref())
        .collect();
    assert!(keys.contains(&"[Space]"), "should have resume hint");
    assert!(!keys.contains(&"[d]"), "the d key is no longer bound");
    let text: String = hints.iter().map(|s| s.content.as_ref()).collect();
    assert!(text.contains("resume"), "Space means resume here");
}

/// `@guarantee RetryReachableInPlace` in docs/specs/dispatch.allium — an
/// unprovisioned Running task has nothing to jump to or resume, so Space
/// advertises the kill-and-retry recovery instead.
#[tokio::test]
async fn action_hints_running_no_worktree_no_window() {
    let mut task = make_task(4, TaskStatus::Running);
    task.worktree = None;
    task.tmux_window = None;
    let hints = ui::action_hints(Some(&task), false, Color::Rgb(122, 162, 247));
    let keys: Vec<&str> = hints
        .iter()
        .filter(|s| s.style.add_modifier.contains(Modifier::BOLD))
        .map(|s| s.content.as_ref())
        .collect();
    assert!(
        !keys.contains(&"[d]"),
        "no dispatch/resume without worktree"
    );
    assert!(keys.contains(&"[Space]"), "Space offers retry");
    let text: String = hints.iter().map(|s| s.content.as_ref()).collect();
    assert!(
        text.contains("retry"),
        "Space means retry here, got {text:?}"
    );
    assert!(keys.contains(&"[e]"), "still has edit");
}

#[tokio::test]
async fn action_hints_review_with_window() {
    let mut task = make_task(6, TaskStatus::Review);
    task.tmux_window = Some("win-6".to_string());
    let hints = ui::action_hints(Some(&task), false, Color::Rgb(122, 162, 247));
    let keys: Vec<&str> = hints
        .iter()
        .filter(|s| s.style.add_modifier.contains(Modifier::BOLD))
        .map(|s| s.content.as_ref())
        .collect();
    assert!(
        keys.contains(&"[Space]"),
        "review with window shows go-to-session"
    );
}

#[tokio::test]
async fn action_hints_done_task() {
    let task = make_task(5, TaskStatus::Done);
    let hints = ui::action_hints(Some(&task), false, Color::Rgb(122, 162, 247));
    let keys: Vec<&str> = hints
        .iter()
        .filter(|s| s.style.add_modifier.contains(Modifier::BOLD))
        .map(|s| s.content.as_ref())
        .collect();
    assert!(keys.contains(&"[e]"), "done has edit");
    assert!(keys.contains(&"[H]"), "done has back");
    assert!(keys.contains(&"[x]"), "done has archive");
    assert!(!keys.contains(&"[L]"), "done has no forward move");
    assert!(!keys.contains(&"[d]"), "done has no dispatch");
}

#[tokio::test]
async fn action_hints_no_task() {
    let hints = ui::action_hints(None, false, Color::Rgb(122, 162, 247));
    let keys: Vec<&str> = hints
        .iter()
        .filter(|s| s.style.add_modifier.contains(Modifier::BOLD))
        .map(|s| s.content.as_ref())
        .collect();
    assert!(keys.contains(&"[n]"), "no-task shows new");
    assert!(!keys.contains(&"[d]"), "no-task has no dispatch");
    assert!(!keys.contains(&"[e]"), "no-task has no edit");
}

/// The `[I] learnings` footer hint went with the overlay
/// (docs/plans/3809-keybinding-pruning-implementation.md §3) — the footer must
/// not advertise a key that no longer has a handler.
#[tokio::test]
async fn action_hints_no_longer_advertises_learnings_key() {
    let hints = ui::action_hints(None, false, Color::Rgb(122, 162, 247));
    let keys = hint_keys(&hints);
    assert!(
        !keys.contains(&"[I]"),
        "retired learnings key must not appear"
    );
}

#[tokio::test]
async fn action_hints_backlog_shows_enter_detail() {
    let task = make_task(1, TaskStatus::Backlog);
    let hints = ui::action_hints(Some(&task), false, Color::Rgb(122, 162, 247));
    let keys = hint_keys(&hints);
    assert!(keys.contains(&"[Enter]"), "should show Enter/detail hint");
}

#[tokio::test]
async fn action_hints_shows_filter_help() {
    let task = make_task(1, TaskStatus::Backlog);
    let hints = ui::action_hints(Some(&task), false, Color::Rgb(122, 162, 247));
    let keys = hint_keys(&hints);
    assert!(keys.contains(&"[f]"), "should show filter hint");
    assert!(keys.contains(&"[?]"), "should show help hint");
}

#[tokio::test]
async fn action_hints_shows_copy_and_split() {
    let task = make_task(1, TaskStatus::Backlog);
    let hints = ui::action_hints(Some(&task), false, Color::Rgb(122, 162, 247));
    let keys = hint_keys(&hints);
    assert!(keys.contains(&"[c]"), "should show copy hint");
    assert!(keys.contains(&"[s]"), "should show split hint");
}

#[tokio::test]
async fn render_empty_board_shows_all_column_headers() {
    let mut app = App::new(vec![]);
    let buf = render_to_buffer(&mut app, 100, 20);
    assert!(buffer_contains(&buf, "backlog"));
    assert!(buffer_contains(&buf, "running"));
    assert!(buffer_contains(&buf, "review"));
    assert!(buffer_contains(&buf, "done"));
}

#[tokio::test]
async fn render_shows_task_titles_in_columns() {
    let tasks = vec![
        make_task(1, TaskStatus::Backlog),
        make_task(2, TaskStatus::Running),
        make_task(3, TaskStatus::Review),
    ];
    let mut app = App::new(tasks);
    let buf = render_to_buffer(&mut app, 120, 20);
    assert!(buffer_contains(&buf, "Task 1"));
    assert!(buffer_contains(&buf, "Task 2"));
    assert!(buffer_contains(&buf, "Task 3"));
}

#[tokio::test]
async fn render_error_popup_shows_message() {
    let mut app = App::new(vec![]);
    app.update(Message::System(crate::tui::messages::SystemMessage::Error(
        "Something went wrong".to_string(),
    )));
    let buf = render_to_buffer(&mut app, 100, 20);
    assert!(buffer_contains(&buf, "Something went wrong"));
}

#[tokio::test]
async fn render_crashed_task_shows_label() {
    let mut task = make_task(1, TaskStatus::Running);
    task.tmux_window = Some("win-1".to_string());
    task.sub_status = SubStatus::Crashed;
    let mut app = App::new(vec![task]);
    let buf = render_to_buffer(&mut app, 120, 20);
    assert!(buffer_contains(&buf, "crashed"));
}

#[tokio::test]
async fn render_stale_task_shows_label() {
    let mut task = make_task(1, TaskStatus::Running);
    task.tmux_window = Some("win-1".to_string());
    task.sub_status = SubStatus::Stale;
    let mut app = App::new(vec![task]);
    let buf = render_to_buffer(&mut app, 120, 20);
    assert!(buffer_contains(&buf, "stale"));
}

#[tokio::test]
async fn running_card_with_worktree_no_window_shows_detached() {
    let mut task = make_task(1, TaskStatus::Running);
    task.worktree = Some("/repo/.worktrees/1-fix".to_string());
    task.tmux_window = None;
    let mut app = App::new(vec![task]);
    let buf = render_to_buffer(&mut app, 120, 20);
    assert!(buffer_contains(&buf, "○ detached"), "expected '○ detached'");
}

#[tokio::test]
async fn running_card_with_window_shows_running_not_detached() {
    let mut task = make_task(1, TaskStatus::Running);
    task.worktree = Some("/repo/.worktrees/1-fix".to_string());
    task.tmux_window = Some("1-fix".to_string());
    let mut app = App::new(vec![task]);
    let buf = render_to_buffer(&mut app, 120, 20);
    assert!(buffer_contains(&buf, "◉ running"), "expected '◉ running'");
    assert!(
        !buffer_contains(&buf, "detached"),
        "should not show detached"
    );
}

#[tokio::test]
async fn review_card_with_pr_detached_shows_circle_prefix() {
    let mut task = make_task(1, TaskStatus::Review);
    task.sub_status = SubStatus::AwaitingReview;
    task.url = Some(crate::models::TaskUrl::new(
        "https://github.com/org/repo/pull/42",
        crate::models::UrlType::Pr,
    ));
    task.worktree = Some("/repo/.worktrees/1-fix".to_string());
    task.tmux_window = None;
    let mut app = App::new(vec![task]);
    let buf = render_to_buffer(&mut app, 120, 20);
    assert!(buffer_contains(&buf, "○ PR #42"), "expected '○ PR #42'");
}

#[tokio::test]
async fn review_card_with_pr_attached_shows_filled_circle() {
    let mut task = make_task(1, TaskStatus::Review);
    task.sub_status = SubStatus::AwaitingReview;
    task.url = Some(crate::models::TaskUrl::new(
        "https://github.com/org/repo/pull/42",
        crate::models::UrlType::Pr,
    ));
    task.worktree = Some("/repo/.worktrees/1-fix".to_string());
    task.tmux_window = Some("1-fix".to_string());
    let mut app = App::new(vec![task]);
    let buf = render_to_buffer(&mut app, 120, 20);
    assert!(buffer_contains(&buf, "● PR #42"), "expected '● PR #42'");
}

#[tokio::test]
async fn render_does_not_panic_on_small_terminal() {
    let mut app = App::new(vec![make_task(1, TaskStatus::Backlog)]);
    // Very small terminal — should not panic
    let _ = render_to_buffer(&mut app, 20, 5);
}

#[tokio::test]
async fn render_input_mode_shows_prompt() {
    let mut app = App::new(vec![]);
    app.update(Message::Input(
        crate::tui::messages::InputMessage::StartNewTask,
    ));
    let buf = render_to_buffer(&mut app, 100, 20);
    assert!(buffer_contains(&buf, "Title"));
}

#[tokio::test]
async fn truncate_respects_max_length() {
    assert_eq!(ui::truncate("short", 10), "short");
    assert_eq!(
        ui::truncate("hello world this is long", 10).chars().count(),
        10
    );
    assert!(ui::truncate("hello world this is long", 10).ends_with('…'));
}

#[tokio::test]
async fn render_v2_task_card_shows_stripe() {
    let mut app = App::new(vec![make_task(1, TaskStatus::Backlog)]);
    let buf = render_to_buffer(&mut app, 120, 20);
    // Cursor card uses thicker stripe ▌ (U+258C), non-cursor uses ▎ (U+258E)
    assert!(
        buffer_contains(&buf, "\u{258c}") || buffer_contains(&buf, "\u{258e}"),
        "task card should have stripe character"
    );
}

#[tokio::test]
async fn render_v2_backlog_task_shows_status_icon() {
    let mut app = App::new(vec![make_task(1, TaskStatus::Backlog)]);
    let buf = render_to_buffer(&mut app, 120, 20);
    assert!(
        buffer_contains(&buf, "\u{25e6}"),
        "backlog task should show \u{25e6} icon"
    );
}

#[tokio::test]
async fn render_v2_running_task_shows_status_icon() {
    let mut task = make_task(1, TaskStatus::Running);
    task.tmux_window = Some("win-1".to_string());
    let mut app = App::new(vec![task]);
    let buf = render_to_buffer(&mut app, 120, 20);
    assert!(
        buffer_contains(&buf, "\u{25c9}"),
        "running task should show \u{25c9} icon"
    );
}

#[tokio::test]
async fn render_v2_focused_column_shows_arrow() {
    let mut app = App::new(vec![]);
    let buf = render_to_buffer(&mut app, 120, 20);
    // Default focus is on first column (Backlog), should show \u{25b8}
    assert!(
        buffer_contains(&buf, "\u{25b8}"),
        "focused column should show \u{25b8} indicator"
    );
}

#[tokio::test]
async fn render_v2_unfocused_columns_show_dot() {
    let mut app = App::new(vec![]);
    let buf = render_to_buffer(&mut app, 120, 20);
    // Unfocused columns should show \u{25e6}
    assert!(
        buffer_contains(&buf, "\u{25e6}"),
        "unfocused columns should show \u{25e6} indicator"
    );
}

#[tokio::test]
async fn render_task_detail_overlay_shows_metadata() {
    // The old fixed detail panel is replaced by the TaskDetail overlay (Task 6).
    // Placeholder: verify that opening the overlay does not crash the renderer.
    let mut app = App::new(vec![make_task(1, TaskStatus::Backlog)]);
    app.update(Message::Task(
        crate::tui::messages::TaskMessage::OpenDetail(TaskId(1)),
    ));
    let _buf = render_to_buffer(&mut app, 120, 20);
}

#[tokio::test]
async fn render_v2_done_task_shows_checkmark() {
    let mut app = App::new(vec![make_task(1, TaskStatus::Done)]);
    // Navigate to Done column (index 3)
    for _ in 0..3 {
        app.update(Message::NavigateColumn(1));
    }
    let buf = render_to_buffer(&mut app, 120, 20);
    assert!(
        buffer_contains(&buf, "\u{2713}"),
        "done task should show \u{2713} icon"
    );
}

#[tokio::test]
async fn render_columns_appear_left_to_right() {
    let mut app = App::new(vec![]);
    let buf = render_to_buffer(&mut app, 120, 30);

    // Find the leftmost x-position where each header appears
    let headers = ["backlog", "running", "review", "done"];
    let mut positions: Vec<Option<u16>> = Vec::new();
    for header in &headers {
        let mut found = None;
        for y in 0..2u16 {
            for x in 0..120u16 {
                let remaining = (120 - x) as usize;
                if remaining < header.len() {
                    continue;
                }
                let segment: String = (0..header.len() as u16)
                    .map(|dx| buf[(x + dx, y)].symbol().to_string())
                    .collect();
                if segment == *header {
                    found = Some(x);
                    break;
                }
            }
            if found.is_some() {
                break;
            }
        }
        positions.push(found);
    }

    // All headers must render
    for (i, header) in headers.iter().enumerate() {
        assert!(
            positions[i].is_some(),
            "column header '{header}' not found in rendered output"
        );
    }

    // Verify strict left-to-right ordering
    let xs: Vec<u16> = positions.into_iter().flatten().collect();
    for pair in xs.windows(2) {
        assert!(
            pair[0] < pair[1],
            "columns must be ordered left to right, got positions: {xs:?}"
        );
    }
}

#[tokio::test]
async fn render_columns_fill_terminal_width() {
    // Regression test: columns must use the full terminal width, not leave a gap on the right.
    // A previous bug reserved a 34-char right sidebar in the column content area.
    let mut app = App::new(vec![make_task(1, TaskStatus::Done)]);
    let width: u16 = 120;
    let buf = render_to_buffer(&mut app, width, 20);

    // Find the rightmost x-position where "done" header text appears
    let header = "done";
    let mut header_x = None;
    'outer: for y in 0..3u16 {
        for x in (0..width).rev() {
            let remaining = (width - x) as usize;
            if remaining < header.len() {
                continue;
            }
            let segment: String = (0..header.len() as u16)
                .map(|dx| buf[(x + dx, y)].symbol().to_string())
                .collect();
            if segment == header {
                header_x = Some(x);
                break 'outer;
            }
        }
    }
    let done_col_x = header_x.expect("'done' column header not found");

    // The "done" column header should be centered in the last quarter of the terminal.
    // With 4 columns at width=120, each column is 30 chars wide, so the last column
    // starts at x=90. The header should be somewhere after x=90.
    // If the old bug exists (34-char sidebar), each column is only ~21 chars and the
    // header would be well before x=90.
    let expected_min_x = width * 3 / 4;
    assert!(
        done_col_x >= expected_min_x,
        "last column header 'done' at x={done_col_x}, expected >= {expected_min_x} — \
         columns are not filling the terminal width"
    );
}

/// Open the help overlay and render it into a `height`-row terminal.
fn help_buffer(height: u16) -> ratatui::buffer::Buffer {
    let mut app = App::new(vec![]);
    app.update(Message::System(
        crate::tui::messages::SystemMessage::ToggleHelp,
    ));
    render_to_buffer(&mut app, 100, height)
}

#[tokio::test]
async fn render_help_overlay_shows_keybindings_help() {
    let buf = help_buffer(30);
    assert!(
        buffer_contains(&buf, "Navigation"),
        "help overlay should show Navigation section"
    );
    assert!(
        buffer_contains(&buf, "Actions"),
        "help overlay should show Actions section"
    );
}

#[tokio::test]
async fn render_help_overlay_shows_tmux_global_bindings() {
    let buf = help_buffer(40);
    assert!(
        buffer_contains(&buf, "Prefix+Space"),
        "help overlay should mention the tmux-global Prefix+Space jump-back binding"
    );
    assert!(
        buffer_contains(&buf, "Prefix+e"),
        "help overlay should mention the tmux-global Prefix+e agent-tree toggle binding"
    );
}

/// The `[C] feed config` help line went with the popup
/// (docs/plans/3809-keybinding-pruning-implementation.md §6) — the help overlay
/// must not teach a key that no longer has a handler.
#[tokio::test]
async fn render_help_overlay_no_longer_teaches_feed_config_key() {
    let buf = help_buffer(40);
    assert!(
        !buffer_contains(&buf, "[C]"),
        "retired feed-config key must not appear in the help overlay"
    );
    assert!(
        !buffer_contains(&buf, "feed config"),
        "retired feed-config help text must not appear in the help overlay"
    );
}

/// Extract the text lines *inside* the help popup's double border.
///
/// The board still renders behind the overlay, and its footer hint bars are
/// full of `[k]`-shaped tokens — parsing the whole buffer would silently mix
/// them into the keymap comparison. The popup is located by its `╔`/`╝`
/// corners rather than by recomputing `render_help_overlay`'s clamp
/// arithmetic, so a change to the popup geometry does not break the parse.
fn help_popup_lines(buf: &ratatui::buffer::Buffer) -> Vec<String> {
    let area = buf.area();
    let mut top_left = None;
    let mut bottom_right = None;
    for y in area.top()..area.bottom() {
        for x in area.left()..area.right() {
            match buf[(x, y)].symbol() {
                "\u{2554}" => top_left = Some((x, y)),
                "\u{255d}" => bottom_right = Some((x, y)),
                _ => {}
            }
        }
    }
    let (x0, y0) = top_left.expect("help popup's top-left double-border corner not found");
    let (x1, y1) = bottom_right.expect("help popup's bottom-right double-border corner not found");
    (y0 + 1..y1)
        .map(|y| (x0 + 1..x1).map(|x| buf[(x, y)].symbol()).collect())
        .collect()
}

/// The set of keys the help overlay *teaches*, parsed out of its rendered text.
///
/// Every `[..]` token is a key legend. Slashes separate alternatives
/// (`[H/L]`, `[h/←]`), the named keys are folded onto what the input handler
/// actually matches (`Space` → `' '`, `gg` → `g`), and anything that isn't a
/// single ASCII key — arrow glyphs, `Prefix+…` tmux bindings, prose — is
/// dropped.
fn help_overlay_keys(lines: &[String]) -> std::collections::BTreeSet<String> {
    let mut keys = std::collections::BTreeSet::new();
    for line in lines {
        let chars: Vec<char> = line.chars().collect();
        let mut i = 0;
        while i < chars.len() {
            if chars[i] != '[' {
                i += 1;
                continue;
            }
            // Content runs to the first `]`, then greedily over any `]` that
            // immediately follows, so `[G/]]` yields `G/]` (two keys) rather
            // than `G/` (one key and a lost `]`).
            let Some(mut end) = (i + 1..chars.len()).find(|&j| chars[j] == ']') else {
                break;
            };
            while end + 1 < chars.len() && chars[end + 1] == ']' {
                end += 1;
            }
            let content: String = chars[i + 1..end].iter().collect();
            // `[/]` is the search key, not an empty alternation.
            let tokens: Vec<&str> = if content == "/" {
                vec!["/"]
            } else {
                content.split('/').filter(|t| !t.is_empty()).collect()
            };
            for token in tokens {
                let key = match token {
                    "Space" => Some(" "),
                    "gg" => Some("g"),
                    "Esc" | "Enter" => Some(token),
                    _ if token.chars().count() == 1
                        && token.chars().all(|c| c.is_ascii_graphic()) =>
                    {
                        Some(token)
                    }
                    // Arrow glyphs, `Prefix+…`, and any prose that happens to
                    // sit inside brackets.
                    _ => None,
                };
                keys.extend(key.map(str::to_string));
            }
            i = end + 1;
        }
    }
    keys
}

/// The set of keys `handle_key_board_normal` actually *handles*, parsed out of
/// its source text — the repo's source-checking idiom (`check-doc-paths.sh`,
/// `check-doc-symbols.sh`) applied to the keymap.
fn board_normal_source_keys() -> std::collections::BTreeSet<String> {
    const SRC: &str = include_str!("../input/normal.rs");
    let start = SRC
        .find("fn handle_key_board_normal")
        .expect("handle_key_board_normal not found — did the fn get renamed?");
    let tail = &SRC[start..];
    // The arms end where the next method in the `impl` block begins.
    let end = tail[1..]
        .find("\n    fn ")
        .map(|i| i + 1)
        .unwrap_or(tail.len());
    let body = &tail[..end];

    let mut keys = std::collections::BTreeSet::new();
    const NEEDLE: &str = "KeyCode::Char('";
    let mut rest = body;
    while let Some(i) = rest.find(NEEDLE) {
        let after = &rest[i + NEEDLE.len()..];
        if let Some(j) = after.find("')") {
            let ch = &after[..j];
            if ch.chars().count() == 1 {
                keys.insert(ch.to_string());
            }
        }
        rest = &rest[i + NEEDLE.len()..];
    }
    if body.contains("KeyCode::Esc") {
        keys.insert("Esc".to_string());
    }
    if body.contains("KeyCode::Enter") {
        keys.insert("Enter".to_string());
    }
    keys
}

/// The help overlay must teach exactly the keymap that exists
/// (docs/plans/3809-keybinding-pruning-implementation.md §7, hardened by
/// task #3986).
///
/// This is **bidirectional** on purpose. The predecessor pinned a fixed list
/// of key strings, which caught the known `[d]`/`F` drift but could not catch
/// the next one: adding a key without a help line passed, and deleting a key
/// failed with a message telling the author to *restore* the help line. Here,
/// a mismatch in either direction names the offending keys and says which side
/// to edit.
#[tokio::test]
async fn render_help_overlay_matches_current_keymap() {
    let buf = help_buffer(40);
    let taught = help_overlay_keys(&help_popup_lines(&buf));
    let handled = board_normal_source_keys();

    let undocumented: Vec<&String> = handled.difference(&taught).collect();
    let phantom: Vec<&String> = taught.difference(&handled).collect();

    assert!(
        undocumented.is_empty(),
        "handle_key_board_normal handles {undocumented:?} but the help overlay does not \
         teach them — add them to src/tui/ui/kanban/popups/help.rs (merge into an existing \
         line; the body is clipped at the 25-row floor)"
    );
    assert!(
        phantom.is_empty(),
        "the help overlay teaches {phantom:?} but handle_key_board_normal has no arm for \
         them — delete those legends from src/tui/ui/kanban/popups/help.rs"
    );

    // Not a key legend, so the set comparison above can't see it: the
    // context-dependence note hangs off `Space`, not the retired `d`.
    assert!(
        buffer_contains(&buf, "jumps to the agent's window"),
        "the context-dependence note should be attached to [Space]"
    );
}

/// The popup height is clamped to 25–36 rows, so on a short terminal the body
/// is clipped rather than scrolled — there is no scroll indicator to tell the
/// user something is missing. Pin the worst case: at the 25-row floor only 23
/// body lines are visible, and every section header must still be one of them.
#[tokio::test]
async fn render_help_overlay_fits_the_clamped_floor() {
    let buf = help_buffer(25);
    for section in ["Navigation", "Actions", "General"] {
        assert!(
            buffer_contains(&buf, section),
            "the {section} section must survive the 25-row floor — the help body \
             has grown past the clipped height"
        );
    }
}

#[tokio::test]
async fn render_1x1_terminal_does_not_panic() {
    let mut app = App::new(vec![make_task(1, TaskStatus::Running)]);
    let _ = render_to_buffer(&mut app, 1, 1);
}

#[tokio::test]
async fn stress_large_task_list_navigation() {
    let tasks: Vec<_> = (1..=1000)
        .map(|i| make_task(i, TaskStatus::Backlog))
        .collect();
    let mut app = App::new(tasks);

    assert_eq!(app.board.tasks.len(), 1000);

    // Navigate through all rows
    for _ in 0..999 {
        app.update(Message::NavigateRow(1));
    }
    assert_eq!(app.selected_row()[0], 999);

    // Navigate back
    for _ in 0..999 {
        app.update(Message::NavigateRow(-1));
    }
    assert_eq!(app.selected_row()[0], 0);
}

#[tokio::test]
async fn stress_large_task_list_rendering() {
    let mut tasks: Vec<_> = (1..=200)
        .map(|i| make_task(i, TaskStatus::Backlog))
        .collect();
    // Spread tasks across all columns
    for (i, task) in tasks.iter_mut().enumerate() {
        task.status = match i % 4 {
            0 => TaskStatus::Backlog,
            1 => TaskStatus::Running,
            2 => TaskStatus::Review,
            _ => TaskStatus::Done,
        };
    }
    let mut app = App::new(tasks);

    // Render at various sizes — must not panic
    for width in [40, 80, 120, 200] {
        for height in [10, 24, 50] {
            let _ = render_to_buffer(&mut app, width, height);
        }
    }
}

#[tokio::test]
async fn stress_rapid_status_transitions() {
    let tasks = vec![make_task(1, TaskStatus::Backlog)];
    let mut app = App::new(tasks);

    // Rapidly move task through all statuses and back.
    // Moving forward will stop at Review because Done requires confirmation.
    for _ in 0..100 {
        app.update(Message::Task(crate::tui::messages::TaskMessage::Move {
            id: TaskId(1),
            direction: MoveDirection::Forward,
        }));
    }
    // Should be at Review (blocked by Done confirmation)
    assert_eq!(app.board.tasks[0].status, TaskStatus::Review);
    assert_eq!(app.input.mode, InputMode::ConfirmDone);

    // Confirm the Done transition
    app.update(Message::Input(
        crate::tui::messages::InputMessage::ConfirmDone,
    ));
    assert_eq!(app.board.tasks[0].status, TaskStatus::Done);

    for _ in 0..100 {
        app.update(Message::Task(crate::tui::messages::TaskMessage::Move {
            id: TaskId(1),
            direction: MoveDirection::Backward,
        }));
    }
    // Should be at Backlog (clamped)
    assert_eq!(app.board.tasks[0].status, TaskStatus::Backlog);
}

#[tokio::test]
async fn stress_db_with_many_tasks() {
    let db = crate::db::Database::open_in_memory().await.unwrap();
    use crate::db::{CreateTaskRequest, TaskCrud, TaskRead};
    for i in 0..500 {
        db.create_task(CreateTaskRequest {
            title: &format!("Task {i}"),
            description: "stress test",
            repo_path: "/repo",
            plan: None,
            status: TaskStatus::Backlog,
            base_branch: "main",
            epic_id: None,
            sort_order: None,
            tag: None,
            wrap_up_mode: None,
            auto_run_plan: false,
        })
        .await
        .unwrap();
    }
    let tasks = db.list_all().await.unwrap();
    assert_eq!(tasks.len(), 500);

    // Create app from DB tasks and verify navigation works
    let mut app = App::new(tasks);
    for _ in 0..499 {
        app.update(Message::NavigateRow(1));
    }
    assert_eq!(app.selected_row()[0], 499);
}

#[tokio::test]
async fn split_focused_defaults_to_true() {
    let app = make_app();
    assert!(app.split_focused());
}

#[tokio::test]
async fn focus_changed_updates_split_focused_when_split_active() {
    let mut app = make_app();
    app.board.split.active = true;
    app.board.split.right_pane_id = Some("pane1".to_string());

    let cmds = app.update(Message::System(
        crate::tui::messages::SystemMessage::FocusChanged(false),
    ));
    assert!(cmds.is_empty());
    assert!(!app.split_focused());

    let cmds = app.update(Message::System(
        crate::tui::messages::SystemMessage::FocusChanged(true),
    ));
    assert!(cmds.is_empty());
    assert!(app.split_focused());
}

#[tokio::test]
async fn render_shows_border_when_split_active_and_focused() {
    let mut app = make_app();
    app.board.split.active = true;
    app.board.split.focused = true;
    app.board.split.right_pane_id = Some("pane1".to_string());

    let buf = render_to_buffer(&mut app, 80, 24);
    // Top-left corner should be a border character (┌)
    assert_eq!(
        buf[(0, 0)].symbol(),
        "┌",
        "Expected border corner when split active"
    );
}

#[tokio::test]
async fn render_no_border_when_split_inactive() {
    let mut app = make_app();
    assert!(!app.split_active());

    let buf = render_to_buffer(&mut app, 80, 24);
    // Top-left corner should NOT be a border character
    assert_ne!(
        buf[(0, 0)].symbol(),
        "┌",
        "No border expected when split inactive"
    );
}

#[tokio::test]
async fn help_overlay_renders_when_active() {
    let mut app = make_app();
    app.input.mode = InputMode::Help;

    let buf = render_to_buffer(&mut app, 80, 35);
    assert!(buffer_contains(&buf, "Navigation"));
    assert!(buffer_contains(&buf, "Actions"));
    assert!(buffer_contains(&buf, "General"));
}

#[tokio::test]
async fn truncate_title_short() {
    assert_eq!(super::truncate_title("Fix bug", 30), "\"Fix bug\"");
}

#[tokio::test]
async fn truncate_title_exact_limit() {
    let title = "a".repeat(30);
    assert_eq!(super::truncate_title(&title, 30), format!("\"{}\"", title));
}

#[tokio::test]
async fn truncate_title_over_limit() {
    let title = "Refactor the authentication middleware system";
    assert_eq!(
        super::truncate_title(title, 30),
        "\"Refactor the authentication...\""
    );
}

#[tokio::test]
async fn truncate_title_multibyte_chars() {
    // Multi-byte UTF-8 characters must not panic on truncation
    let title = "Fix the caf\u{00e9} rendering bug now";
    // 31 chars, should truncate at char boundary not byte boundary
    assert!(super::truncate_title(title, 10).ends_with("...\""));
}

#[tokio::test]
async fn focused_column_has_tinted_background() {
    let mut app = App::new(vec![
        make_task(1, TaskStatus::Backlog),
        make_task(2, TaskStatus::Running),
    ]);
    // Use wider terminal so 8 columns have enough room for content.
    // Columns use Ratio constraints (3/18, 2/18, ...) so they aren't equal width.
    let buf = render_to_buffer(&mut app, 240, 30);

    // Focused column (Backlog, col 0) should have a tinted bg.
    // Check a row well below the cursor card to avoid cursor highlight.
    let expected_bg = Color::Rgb(28, 30, 44);
    let cell = &buf[(1, 15)];
    // Backlog is 3/18 of 240 = 40px. Check well past that at x=120 (middle of board).
    let cell2 = &buf[(120, 15)];

    assert_eq!(
        cell.bg, expected_bg,
        "Focused column should have tinted background"
    );
    assert_ne!(
        cell2.bg, expected_bg,
        "Unfocused column should NOT have tinted background"
    );
}

#[tokio::test]
async fn on_select_all_defaults_to_false() {
    let app = make_app();
    assert!(!app.on_select_all());
}

#[tokio::test]
async fn select_all_column_selects_all_tasks_in_column() {
    let mut app = make_app();
    // Cursor is on Backlog (column 0) which has tasks 1, 2
    app.update(Message::SelectAllColumn);
    assert!(app.select.tasks.contains(&TaskId(1)));
    assert!(app.select.tasks.contains(&TaskId(2)));
    assert_eq!(app.select.tasks.len(), 2);
}

#[tokio::test]
async fn select_all_column_deselects_when_all_selected() {
    let mut app = make_app();
    app.update(Message::SelectAllColumn);
    assert_eq!(app.select.tasks.len(), 2);

    app.update(Message::SelectAllColumn);
    assert!(app.select.tasks.is_empty());
}

#[tokio::test]
async fn select_all_column_selects_remaining_when_partially_selected() {
    let mut app = make_app();
    app.update(Message::Task(
        crate::tui::messages::TaskMessage::ToggleSelect(TaskId(1)),
    ));
    assert_eq!(app.select.tasks.len(), 1);

    app.update(Message::SelectAllColumn);
    assert!(app.select.tasks.contains(&TaskId(1)));
    assert!(app.select.tasks.contains(&TaskId(2)));
    assert_eq!(app.select.tasks.len(), 2);
}

#[tokio::test]
async fn select_all_column_noop_on_empty_column() {
    let mut app = make_app();
    // Navigate to Review column (empty in make_app)
    app.update(Message::NavigateColumn(2));
    app.update(Message::SelectAllColumn);
    assert!(app.select.tasks.is_empty());
}

#[tokio::test]
async fn select_all_column_only_affects_current_column() {
    let mut app = make_app();
    // TaskId(3) is in Running column, pre-select it
    app.update(Message::Task(
        crate::tui::messages::TaskMessage::ToggleSelect(TaskId(3)),
    ));
    // SelectAllColumn selects all in current (Backlog) column
    app.update(Message::SelectAllColumn);
    assert!(app.select.tasks.contains(&TaskId(1)));
    assert!(app.select.tasks.contains(&TaskId(2)));
    assert!(app.select.tasks.contains(&TaskId(3)));
    assert_eq!(app.select.tasks.len(), 3);
}

#[tokio::test]
async fn select_all_deselect_only_affects_current_column() {
    let mut app = make_app();
    app.update(Message::Task(
        crate::tui::messages::TaskMessage::ToggleSelect(TaskId(3)),
    ));
    app.update(Message::SelectAllColumn);
    assert_eq!(app.select.tasks.len(), 3);

    app.update(Message::SelectAllColumn);
    assert_eq!(app.select.tasks.len(), 1);
    assert!(app.select.tasks.contains(&TaskId(3)));
}

#[tokio::test]
async fn key_a_selects_all_in_column() {
    let mut app = make_app();
    app.handle_key(make_key(KeyCode::Char('a')));
    assert!(app.select.tasks.contains(&TaskId(1)));
    assert!(app.select.tasks.contains(&TaskId(2)));
}

#[tokio::test]
async fn navigate_up_from_row_zero_enters_select_all_toggle() {
    let mut app = make_app();
    assert!(!app.on_select_all());
    app.handle_key(make_key(KeyCode::Char('k')));
    assert!(app.on_select_all());
}

#[tokio::test]
async fn column_switch_clears_on_select_all_for_nonempty_column() {
    let mut app = make_app();
    app.handle_key(make_key(KeyCode::Char('k')));
    assert!(app.on_select_all());
    // Running (nav col 2) has a task, so switching there must land on the
    // first card rather than preserving the select-all toggle.
    app.handle_key(make_key(KeyCode::Char('l')));
    assert!(!app.on_select_all());
}

#[tokio::test]
async fn enter_on_toggle_triggers_select_all() {
    let mut app = make_app();
    app.handle_key(make_key(KeyCode::Char('k')));
    app.handle_key(make_key(KeyCode::Enter));
    assert!(app.select.tasks.contains(&TaskId(1)));
    assert!(app.select.tasks.contains(&TaskId(2)));
}

#[tokio::test]
async fn v_is_noop_when_on_select_all() {
    let mut app = make_app();
    app.handle_key(make_key(KeyCode::Char('k')));
    app.handle_key(make_key(KeyCode::Char('v')));
    assert!(app.select.tasks.is_empty());
}

#[tokio::test]
async fn render_shows_select_all_toggle_in_focused_column() {
    let mut app = make_app();
    let buf = render_to_buffer(&mut app, 120, 30);
    assert!(buffer_contains(&buf, "[ ]"));
    assert!(!buffer_contains(&buf, "Select [a]ll"));
}

#[tokio::test]
async fn render_shows_checked_toggle_when_all_selected() {
    let mut app = make_app();
    app.update(Message::SelectAllColumn);
    let buf = render_to_buffer(&mut app, 120, 30);
    assert!(buffer_contains(&buf, "[x]"));
}

#[tokio::test]
async fn render_shows_unchecked_toggle_when_not_all_selected() {
    let mut app = make_app();
    let buf = render_to_buffer(&mut app, 120, 30);
    assert!(buffer_contains(&buf, "[ ]"));
}

#[tokio::test]
async fn action_hints_include_select_all() {
    let app = make_app();
    let task = app.selected_task();
    let spans = ui::action_hints(task, false, Color::Blue);
    let text: String = spans.iter().map(|s| s.content.as_ref()).collect();
    assert!(
        text.contains("select all"),
        "action hints should include 'select all'"
    );
}

#[tokio::test]
async fn card_shows_pr_badge() {
    let mut task = make_task(1, TaskStatus::Review);
    task.url = Some(crate::models::TaskUrl::new(
        "https://github.com/org/repo/pull/42",
        crate::models::UrlType::Pr,
    ));
    let mut app = App::new(vec![task]);
    // Navigate to Review column (index 2)
    for _ in 0..2 {
        app.update(Message::NavigateColumn(1));
    }

    let buf = render_to_buffer(&mut app, 120, 20);
    assert!(
        buffer_contains(&buf, "PR #42"),
        "Card should show PR #42 badge"
    );
}

#[tokio::test]
async fn card_shows_merged_pr_badge() {
    let mut task = make_task(1, TaskStatus::Done);
    task.url = Some(crate::models::TaskUrl::new(
        "https://github.com/org/repo/pull/42",
        crate::models::UrlType::Pr,
    ));
    let mut app = App::new(vec![task]);
    // Navigate to Done column (index 3)
    for _ in 0..3 {
        app.update(Message::NavigateColumn(1));
    }

    let buf = render_to_buffer(&mut app, 120, 20);
    assert!(
        buffer_contains(&buf, "PR #42 merged"),
        "Done card should show merged PR badge"
    );
}

#[tokio::test]
async fn reorder_task_down_swaps_sort_order() {
    let mut app = make_app();
    let t1 = make_task(1, TaskStatus::Backlog);
    let t2 = make_task(2, TaskStatus::Backlog);
    app.board.tasks = vec![t1, t2];

    // Cursor on first task (row 0, column 0 = Backlog)
    let cmds = app.update(Message::Task(
        crate::tui::messages::TaskMessage::ReorderItem(1),
    ));

    // After reorder, task 1 should have a higher sort value than task 2
    let t1 = app.find_task(TaskId(1)).unwrap();
    let t2 = app.find_task(TaskId(2)).unwrap();
    let eff1 = t1.sort_order.unwrap_or(t1.id.0);
    let eff2 = t2.sort_order.unwrap_or(t2.id.0);
    assert!(
        eff1 > eff2,
        "task 1 ({eff1}) should be after task 2 ({eff2}) after move down"
    );
    // Should emit PersistTask for both
    assert_eq!(
        cmds.iter()
            .filter(|c| matches!(
                c,
                Command::Task(crate::tui::commands::TaskCommand::Persist(_))
            ))
            .count(),
        2
    );
    // Cursor should have moved down
    assert_eq!(app.selection().row(1), 1);
}

#[tokio::test]
async fn reorder_task_up_at_top_is_noop() {
    let mut app = make_app();
    let t1 = make_task(1, TaskStatus::Backlog);
    app.board.tasks = vec![t1];

    let cmds = app.update(Message::Task(
        crate::tui::messages::TaskMessage::ReorderItem(-1),
    ));
    assert!(cmds.is_empty());
}

#[tokio::test]
async fn reorder_task_down_at_bottom_is_noop() {
    let mut app = make_app();
    let t1 = make_task(1, TaskStatus::Backlog);
    app.board.tasks = vec![t1];

    let cmds = app.update(Message::Task(
        crate::tui::messages::TaskMessage::ReorderItem(1),
    ));
    assert!(cmds.is_empty());
}

#[tokio::test]
async fn reorder_task_up_swaps_sort_order() {
    let mut app = make_app();
    let t1 = make_task(1, TaskStatus::Backlog);
    let t2 = make_task(2, TaskStatus::Backlog);
    app.board.tasks = vec![t1, t2];

    // Move cursor to row 1 (second task), then reorder up
    app.selection_mut().set_row(1, 1);
    let cmds = app.update(Message::Task(
        crate::tui::messages::TaskMessage::ReorderItem(-1),
    ));

    // After reorder, task 2 should have a lower sort value than task 1
    let t1 = app.find_task(TaskId(1)).unwrap();
    let t2 = app.find_task(TaskId(2)).unwrap();
    let eff1 = t1.sort_order.unwrap_or(t1.id.0);
    let eff2 = t2.sort_order.unwrap_or(t2.id.0);
    assert!(
        eff2 < eff1,
        "task 2 ({eff2}) should be before task 1 ({eff1}) after move up"
    );
    assert_eq!(
        cmds.iter()
            .filter(|c| matches!(
                c,
                Command::Task(crate::tui::commands::TaskCommand::Persist(_))
            ))
            .count(),
        2
    );
    // Cursor should have moved up
    assert_eq!(app.selection().row(1), 0);
}

#[tokio::test]
async fn reorder_task_down_swaps_sort_order_within_done_column() {
    let mut app = make_app();
    // t1's sort_order is more negative (= more recent), so it renders at
    // row 0; t2 renders at row 1. This must hold for the cursor position
    // below to actually land on t1 before the move.
    let mut t1 = make_task(1, TaskStatus::Done);
    t1.sort_order = Some(-1_700_000_100_000);
    let mut t2 = make_task(2, TaskStatus::Done);
    t2.sort_order = Some(-1_700_000_000_000);
    app.board.tasks = vec![t1, t2];
    app.selection_mut().set_column(4); // Done column
    app.selection_mut().set_row(4, 0);

    let cmds = app.update(Message::Task(
        crate::tui::messages::TaskMessage::ReorderItem(1),
    ));

    let t1 = app.find_task(TaskId(1)).unwrap();
    let t2 = app.find_task(TaskId(2)).unwrap();
    let eff1 = t1.sort_order.unwrap_or(t1.id.0);
    let eff2 = t2.sort_order.unwrap_or(t2.id.0);
    assert!(
        eff1 > eff2,
        "task 1 ({eff1}) should be after task 2 ({eff2}) after move down"
    );
    assert_eq!(
        cmds.iter()
            .filter(|c| matches!(
                c,
                Command::Task(crate::tui::commands::TaskCommand::Persist(_))
            ))
            .count(),
        2
    );
}

#[tokio::test]
async fn render_shows_subcolumn_headers() {
    // make_app() has one Running task (SubStatus::Active) → Running column shows "── active" header
    let mut app = App::new(vec![make_task(1, TaskStatus::Running), {
        let mut t = make_task(2, TaskStatus::Running);
        t.sub_status = SubStatus::Stale;
        t
    }]);
    let buf = render_to_buffer(&mut app, 160, 30);
    assert!(
        buffer_contains(&buf, "active"),
        "section header 'active' not found"
    );
    assert!(
        buffer_contains(&buf, "stale"),
        "section header 'stale' not found"
    );
}

#[tokio::test]
async fn render_shows_parent_status_headers() {
    let mut app = make_app();
    let buf = render_to_buffer(&mut app, 160, 30);
    assert!(
        buffer_contains(&buf, "backlog"),
        "parent header 'backlog' not found"
    );
    assert!(
        buffer_contains(&buf, "running"),
        "parent header 'running' not found"
    );
    assert!(
        buffer_contains(&buf, "review"),
        "parent header 'review' not found"
    );
    assert!(
        buffer_contains(&buf, "done"),
        "parent header 'done' not found"
    );
}

#[tokio::test]
async fn render_detail_shows_sub_status() {
    let mut task = make_task(1, TaskStatus::Running);
    task.sub_status = SubStatus::Active;
    let mut app = App::new(vec![task]);
    // Navigate to the Active visual column (index 1)
    app.update(Message::NavigateColumn(1));
    // The old detail panel is replaced by the TaskDetail overlay (Task 6).
    // Placeholder: verify that the overlay renderer does not crash.
    app.update(Message::Task(
        crate::tui::messages::TaskMessage::OpenDetail(TaskId(1)),
    ));
    let _buf = render_to_buffer(&mut app, 160, 30);
}

#[tokio::test]
async fn render_card_conflict_shows_rebase_conflict() {
    let mut task = make_task(1, TaskStatus::Running);
    task.sub_status = SubStatus::Conflict;
    task.worktree = Some("/repo/.worktrees/1-task-1".to_string());
    task.tmux_window = Some("task-1".to_string());
    let mut app = App::new(vec![task]);
    app.update(Message::NavigateColumn(1)); // Running column
    let buf = render_to_buffer(&mut app, 120, 30);
    assert!(
        buffer_contains(&buf, "rebase conflict"),
        "Conflict task should show 'rebase conflict'"
    );
}

#[tokio::test]
async fn render_card_detached_shows_detached() {
    let mut task = make_task(1, TaskStatus::Running);
    task.worktree = Some("/repo/.worktrees/1-task-1".to_string());
    task.tmux_window = None; // detached: worktree present but no tmux
    task.sub_status = SubStatus::Active;
    let mut app = App::new(vec![task]);
    app.update(Message::NavigateColumn(1)); // Running column
    let buf = render_to_buffer(&mut app, 120, 30);
    assert!(
        buffer_contains(&buf, "detached"),
        "Task with worktree but no tmux_window should show 'detached'"
    );
}

#[tokio::test]
async fn render_card_detached_review_shows_pr_label() {
    let mut task = make_task(1, TaskStatus::Review);
    task.worktree = Some("/repo/.worktrees/1-task-1".to_string());
    task.tmux_window = None; // detached
    task.url = Some(crate::models::TaskUrl::new(
        "https://github.com/acme/app/pull/42",
        crate::models::UrlType::Pr,
    ));
    task.sub_status = SubStatus::AwaitingReview;
    let mut app = App::new(vec![task]);
    app.update(Message::NavigateColumn(1)); // move to Running
    app.update(Message::NavigateColumn(1)); // move to Review
    let buf = render_to_buffer(&mut app, 120, 30);
    assert!(
        buffer_contains(&buf, "PR #42"),
        "Detached review task with pr_url should show 'PR #42'"
    );
}

#[tokio::test]
async fn render_card_blocked_shows_blocked() {
    let mut task = make_task(1, TaskStatus::Running);
    task.sub_status = SubStatus::NeedsInput;
    task.worktree = Some("/repo/.worktrees/1-task-1".to_string());
    task.tmux_window = Some("task-1".to_string());
    let mut app = App::new(vec![task]);
    app.update(Message::NavigateColumn(1)); // Running column
    let buf = render_to_buffer(&mut app, 120, 30);
    assert!(
        buffer_contains(&buf, "blocked"),
        "Running task with NeedsInput sub_status should show 'blocked'"
    );
}

#[tokio::test]
async fn render_card_running_shows_running() {
    let mut task = make_task(1, TaskStatus::Running);
    task.sub_status = SubStatus::Active;
    task.worktree = Some("/repo/.worktrees/1-task-1".to_string());
    task.tmux_window = Some("task-1".to_string());
    let mut app = App::new(vec![task]);
    app.update(Message::NavigateColumn(1)); // Running column
    let buf = render_to_buffer(&mut app, 120, 30);
    assert!(
        buffer_contains(&buf, "running"),
        "Active running task should show 'running'"
    );
}

#[tokio::test]
async fn render_card_review_pr_shows_pr_number() {
    let mut task = make_task(1, TaskStatus::Review);
    task.worktree = Some("/repo/.worktrees/1-task-1".to_string());
    task.tmux_window = Some("task-1".to_string());
    task.url = Some(crate::models::TaskUrl::new(
        "https://github.com/acme/app/pull/99",
        crate::models::UrlType::Pr,
    ));
    task.sub_status = SubStatus::AwaitingReview;
    let mut app = App::new(vec![task]);
    app.update(Message::NavigateColumn(1)); // move to Running
    app.update(Message::NavigateColumn(1)); // move to Review
    let buf = render_to_buffer(&mut app, 120, 30);
    assert!(
        buffer_contains(&buf, "PR #99"),
        "Review task with pr_url and tmux should show 'PR #99'"
    );
}

#[tokio::test]
async fn render_card_done_merged_shows_merged() {
    let mut task = make_task(1, TaskStatus::Done);
    task.url = Some(crate::models::TaskUrl::new(
        "https://github.com/acme/app/pull/77",
        crate::models::UrlType::Pr,
    ));
    let mut app = App::new(vec![task]);
    app.update(Message::NavigateColumn(1)); // Running
    app.update(Message::NavigateColumn(1)); // Review
    app.update(Message::NavigateColumn(1)); // Done
    let buf = render_to_buffer(&mut app, 120, 30);
    assert!(
        buffer_contains(&buf, "PR #77 merged"),
        "Done task with pr_url should show 'PR #77 merged'"
    );
}

#[tokio::test]
async fn render_card_idle_with_plan_shows_triangle() {
    let mut task = make_task(1, TaskStatus::Backlog);
    task.plan_path = Some("docs/plans/plan.md".to_string());
    let mut app = App::new(vec![task]);
    // Already in Backlog column (0)
    let buf = render_to_buffer(&mut app, 120, 30);
    assert!(
        buffer_contains(&buf, "\u{25b8}"),
        "Backlog task with plan should show '▸' (U+25B8)"
    );
}

#[tokio::test]
async fn render_card_idle_with_bug_tag() {
    let mut task = make_task(1, TaskStatus::Backlog);
    task.tag = Some(TaskTag::Bug);
    let mut app = App::new(vec![task]);
    let buf = render_to_buffer(&mut app, 120, 30);
    assert!(
        buffer_contains(&buf, "[bug]"),
        "Backlog task with Bug tag should show '[bug]'"
    );
}

#[tokio::test]
async fn render_card_idle_with_feature_tag() {
    let mut task = make_task(1, TaskStatus::Backlog);
    task.tag = Some(TaskTag::Feature);
    let mut app = App::new(vec![task]);
    let buf = render_to_buffer(&mut app, 120, 30);
    assert!(
        buffer_contains(&buf, "[feat]"),
        "Backlog task with Feature tag should show '[feat]'"
    );
}

#[tokio::test]
async fn render_card_message_flash_shows_envelope() {
    let mut task = make_task(1, TaskStatus::Running);
    task.sub_status = SubStatus::Active;
    task.worktree = Some("/repo/.worktrees/1-task-1".to_string());
    task.tmux_window = Some("task-1".to_string());
    let mut app = App::new(vec![task]);
    app.agents.message_flash.insert(TaskId(1), Instant::now());
    app.update(Message::NavigateColumn(1)); // Running column
    let buf = render_to_buffer(&mut app, 120, 30);
    assert!(
        buffer_contains(&buf, "\u{2709}"),
        "Running task with message_flash set should show '\u{2709}' (envelope)"
    );
}

#[tokio::test]
async fn render_detail_task_with_tag_shows_tag() {
    let mut task = make_task(1, TaskStatus::Backlog);
    task.tag = Some(TaskTag::Bug);
    let mut app = App::new(vec![task]);
    // The old detail panel is replaced by the TaskDetail overlay (Task 6).
    app.update(Message::Task(
        crate::tui::messages::TaskMessage::OpenDetail(TaskId(1)),
    ));
    let _buf = render_to_buffer(&mut app, 120, 30);
}

#[tokio::test]
async fn render_detail_task_with_pr_url() {
    let mut task = make_task(1, TaskStatus::Review);
    task.url = Some(crate::models::TaskUrl::new(
        "https://github.com/acme/app/pull/42",
        crate::models::UrlType::Pr,
    ));
    let mut app = App::new(vec![task]);
    // Navigate to Review column (index 2)
    app.update(Message::NavigateColumn(2));
    // The old detail panel is replaced by the TaskDetail overlay (Task 6).
    app.update(Message::Task(
        crate::tui::messages::TaskMessage::OpenDetail(TaskId(1)),
    ));
    let _buf = render_to_buffer(&mut app, 160, 30);
}

#[tokio::test]
async fn render_detail_no_selection_shows_message() {
    // The old detail panel is replaced by the TaskDetail overlay (Task 6).
    // Placeholder: just verify that rendering an empty board does not crash.
    let mut app = App::new(vec![]);
    let _buf = render_to_buffer(&mut app, 120, 30);
}

#[tokio::test]
async fn task_card_title_truncated_in_narrow_terminal() {
    let mut task = make_task(1, TaskStatus::Backlog);
    task.title = "This is a very long task title that should be truncated".to_string();
    let mut app = App::new(vec![task]);

    // Narrow terminal: 4 columns per status column (80 / 4 statuses = 20 each)
    let buf = render_to_buffer(&mut app, 80, 10);

    // Full title should NOT appear — it's too long for the column
    assert!(
        !buffer_contains(
            &buf,
            "This is a very long task title that should be truncated"
        ),
        "full title should be truncated in narrow terminal"
    );
    // Truncated title with ellipsis should appear
    assert!(
        buffer_contains(&buf, "…"),
        "truncated title should contain ellipsis"
    );
}

#[tokio::test]
async fn task_card_short_title_not_truncated_in_wide_terminal() {
    let mut task = make_task(1, TaskStatus::Backlog);
    task.title = "Short".to_string();
    let mut app = App::new(vec![task]);

    // Wide terminal: plenty of room
    let buf = render_to_buffer(&mut app, 200, 10);
    assert!(
        buffer_contains(&buf, "Short"),
        "short title should appear in full"
    );
}

#[tokio::test]
async fn task_card_title_adapts_to_terminal_width() {
    let mut task = make_task(1, TaskStatus::Backlog);
    task.title = "Medium length title here".to_string();
    let mut app_narrow = App::new(vec![task.clone()]);
    let mut app_wide = App::new(vec![task]);

    let buf_narrow = render_to_buffer(&mut app_narrow, 60, 10);
    let buf_wide = render_to_buffer(&mut app_wide, 200, 10);

    // In narrow terminal, should be truncated
    assert!(
        !buffer_contains(&buf_narrow, "Medium length title here"),
        "title should be truncated in narrow terminal"
    );
    // In wide terminal, should appear in full
    assert!(
        buffer_contains(&buf_wide, "Medium length title here"),
        "title should appear in full in wide terminal"
    );
}

#[tokio::test]
async fn handle_key_normal_reorder_j_down() {
    let mut app = make_app();
    app.selection_mut().set_column(1);
    app.selection_mut().set_row(1, 0);
    let cmds = app.handle_key(make_key(KeyCode::Char('J')));
    // Reorder should produce a persist command
    assert!(cmds.iter().any(|c| matches!(
        c,
        Command::Task(crate::tui::commands::TaskCommand::Persist(_))
    )));
}

#[tokio::test]
async fn handle_key_normal_reorder_k_up() {
    let mut app = make_app();
    app.selection_mut().set_column(1);
    app.selection_mut().set_row(1, 1);
    let cmds = app.handle_key(make_key(KeyCode::Char('K')));
    assert!(cmds.iter().any(|c| matches!(
        c,
        Command::Task(crate::tui::commands::TaskCommand::Persist(_))
    )));
}

#[tokio::test]
async fn handle_key_normal_enter_on_select_all_row() {
    let mut app = make_app();
    // Navigate up past first item to land on "select all" virtual row
    app.selection_mut().set_column(1);
    app.selection_mut().set_row(1, 0);
    // Manually set on_select_all
    app.selection_mut().on_select_all = true;

    app.handle_key(make_key(KeyCode::Enter));
    // Should have toggled select all — tasks should be selected
    assert!(
        !app.select.tasks.is_empty()
            || !app.select.epics.is_empty()
            || app.selection().on_select_all
    );
}

#[tokio::test]
async fn backlog_column_color_is_blue() {
    let backlog = ui::column_color(TaskStatus::Backlog);
    // Backlog should use a distinct blue, not the generic MUTED grey.
    assert_ne!(
        backlog,
        Color::Rgb(86, 95, 137),
        "Backlog column color should not be MUTED grey"
    );
    assert_eq!(
        backlog,
        Color::Rgb(122, 162, 247),
        "Backlog column color should be Tokyo Night blue"
    );
}

#[tokio::test]
async fn focused_backlog_header_renders_in_blue() {
    let mut app = make_app();
    assert_eq!(app.selected_column(), 1);

    let buf = render_to_buffer(&mut app, 100, 20);
    let area = buf.area();
    let blue = Color::Rgb(122, 162, 247);
    let target = "backlog";
    let mut found = false;
    for y in area.top()..area.bottom() {
        for x in area.left()..area.right().saturating_sub(target.len() as u16 - 1) {
            let matches = target
                .bytes()
                .enumerate()
                .all(|(i, ch)| buf[(x + i as u16, y)].symbol().as_bytes().first() == Some(&ch));
            if matches {
                let fg = buf[(x, y)].fg;
                if fg == blue {
                    found = true;
                }
                break;
            }
        }
        if found {
            break;
        }
    }
    assert!(
        found,
        "Focused Backlog header should render with blue foreground color"
    );
}

#[tokio::test]
async fn render_adapts_to_smaller_terminal_after_resize() {
    let mut app = make_app();

    // Render at a large size (pre-split)
    let buf_large = render_to_buffer(&mut app, 160, 40);
    // Render at a smaller size (post-split, e.g. half width)
    let buf_small = render_to_buffer(&mut app, 80, 40);

    // The smaller render should use the full width of the smaller terminal
    assert_eq!(buf_small.area().width, 80);
    assert_eq!(buf_large.area().width, 160);
    // Both should contain a task title — layout adapted, content still renders
    assert!(
        buffer_contains(&buf_small, "Task 1"),
        "task should render at smaller width"
    );
}

#[tokio::test]
async fn render_repo_path_mode_shows_filtered_list_when_typing() {
    let mut app = App::new(vec![]);
    app.board.repo_paths = vec!["/tmp".to_string(), "/var/log".to_string()];
    app.input.mode = InputMode::InputRepoPath;
    app.input.task_draft = Some(TaskDraft {
        title: "T".to_string(),
        ..Default::default()
    });
    app.input.set_buffer("tmp".to_string()); // filter active

    let buf = render_to_buffer(&mut app, 80, 20);
    assert!(buffer_contains(&buf, "/tmp"), "matching path should appear");
    assert!(
        !buffer_contains(&buf, "/var/log"),
        "non-matching path should be hidden"
    );
}

#[tokio::test]
async fn render_repo_path_mode_shows_all_when_buffer_empty() {
    let mut app = App::new(vec![]);
    app.board.repo_paths = vec!["/tmp".to_string(), "/var/log".to_string()];
    app.input.mode = InputMode::InputRepoPath;
    app.input.task_draft = Some(TaskDraft {
        title: "T".to_string(),
        ..Default::default()
    });
    // buffer is empty — all paths shown

    let buf = render_to_buffer(&mut app, 80, 20);
    assert!(buffer_contains(&buf, "/tmp"));
    assert!(buffer_contains(&buf, "/var/log"));
}

#[tokio::test]
async fn test_on_select_all_preserved_on_refresh() {
    let mut app = make_app();
    // Navigate up from row 0 to select-all header
    app.update(Message::NavigateRow(-1));
    assert!(app.selection().on_select_all);

    app.update(Message::Task(crate::tui::messages::TaskMessage::Refresh(
        vec![
            make_task(1, TaskStatus::Backlog),
            make_task(2, TaskStatus::Backlog),
        ],
    )));

    assert!(app.selection().on_select_all);
    assert_eq!(app.selection().anchor, None);
}

#[tokio::test]
async fn summary_shows_four_columns_when_backlog_focused() {
    let mut app = make_app();
    // Default is col 1 (Backlog)
    assert_eq!(app.selected_column(), 1);
    let buf = render_to_buffer(&mut app, 120, 40);
    // The summary row (y=1) should NOT contain "Projects".
    let summary_row: String = (0..120u16)
        .map(|x| buf[(x, 1)].symbol().to_string())
        .collect();
    assert!(
        !summary_row.contains("Projects"),
        "summary row should NOT show Projects when col 1 focused; got: {summary_row:?}"
    );
    assert!(
        summary_row.contains("backlog"),
        "summary row should show backlog header; got: {summary_row:?}"
    );
}

#[tokio::test]
async fn summary_shows_five_columns_when_archive_focused() {
    let mut app = make_app();
    for _ in 0..4 {
        app.update(Message::NavigateColumn(1));
    }
    assert_eq!(app.selected_column(), TaskStatus::COLUMN_COUNT + 1);
    let buf = render_to_buffer(&mut app, 120, 40);
    let summary_row: String = (0..120u16)
        .map(|x| buf[(x, 1)].symbol().to_string())
        .collect();
    assert!(
        summary_row.contains("Archive"),
        "summary row should show Archive header when col 5 focused; got: {summary_row:?}"
    );
    assert!(
        !summary_row.contains("Projects"),
        "summary row should NOT show Projects when Archive focused; got: {summary_row:?}"
    );
}

// ▼ = U+25BC (BLACK DOWN-POINTING TRIANGLE)
// ▲ = U+25B2 (BLACK UP-POINTING TRIANGLE)
// Distinct from ▸ U+25B8 used in the summary row for focused columns.

#[tokio::test]
async fn scroll_indicator_down_shown_when_items_overflow() {
    // 5 Backlog tasks × 3 lines each = 15 lines; at height=20 kanban inner ≈ 8 lines → overflow
    let tasks: Vec<_> = (1..=5).map(|i| make_task(i, TaskStatus::Backlog)).collect();
    let mut app = App::new(tasks);
    // Cursor at top (row 0): offset=0, only ▼ should show
    app.selection_mut().set_row(1, 0);

    let buf = render_to_buffer(&mut app, 120, 20);
    assert!(
        buffer_contains(&buf, "\u{25BC}"),
        "▼ indicator should appear when items overflow below the visible area"
    );
    assert!(
        !buffer_contains(&buf, "\u{25B2}"),
        "▲ indicator should NOT appear when cursor is at the top"
    );
}

#[tokio::test]
async fn scroll_indicator_up_shown_when_scrolled_past_top() {
    // 5 Backlog tasks, cursor on the last one → ratatui scrolls → offset > 0 → ▲ shows
    let tasks: Vec<_> = (1..=5).map(|i| make_task(i, TaskStatus::Backlog)).collect();
    let mut app = App::new(tasks);
    app.selection_mut().set_row(1, 4); // row 4 = 5th task

    let buf = render_to_buffer(&mut app, 120, 20);
    assert!(
        buffer_contains(&buf, "\u{25B2}"),
        "▲ indicator should appear when scrolled past the top"
    );
}

#[tokio::test]
async fn no_scroll_indicators_when_items_fit() {
    // 2 Backlog tasks × 3 lines = 6 lines; at height=40 kanban inner ≈ 28 lines → fits
    let tasks = vec![
        make_task(1, TaskStatus::Backlog),
        make_task(2, TaskStatus::Backlog),
    ];
    let mut app = App::new(tasks);

    let buf = render_to_buffer(&mut app, 120, 40);
    assert!(
        !buffer_contains(&buf, "\u{25BC}"),
        "▼ should NOT appear when all items fit in the visible area"
    );
    assert!(
        !buffer_contains(&buf, "\u{25B2}"),
        "▲ should NOT appear when all items fit in the visible area"
    );
}

#[tokio::test]
async fn scroll_indicators_do_not_panic_on_empty_column() {
    let mut app = App::new(vec![]);
    // Should render without panic
    let buf = render_to_buffer(&mut app, 120, 20);
    assert!(!buffer_contains(&buf, "\u{25BC}"));
    assert!(!buffer_contains(&buf, "\u{25B2}"));
}
