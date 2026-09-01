#![allow(clippy::unwrap_used, clippy::expect_used)]
use super::*;
use crate::models::{test_tmux_window, SubStatus, TaskId, TaskStatus, TaskTag};
// Palette constants come from the palette, never retyped as literals here: a
// hand-copied RGB goes stale silently when the palette moves, which is the exact
// drift the derived header labels were introduced to stop.
use crate::tui::ui::palette::{BORDER, GREEN, MUTED, PURPLE, RED, YELLOW};
use crossterm::event::KeyCode;
use ratatui::buffer::Buffer;
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
    assert!(keys.contains(&"[Space]"), "should have dispatch hint");
    assert!(keys.contains(&"[e]"), "should have edit hint");
    assert!(keys.contains(&"[L]"), "should have move hint");
    assert!(!keys.contains(&"[H]"), "backlog has no back movement");
    assert!(keys.contains(&"[x]"), "should have archive hint");
    assert!(keys.contains(&"[n]"), "should have new hint");
    let text: String = hints.iter().map(|s| s.content.as_ref()).collect();
    // "dispatch" is the one name for starting a task, whether or not a plan is
    // attached. The label used to read "brainstorm" for a no-plan task, naming
    // a design step that no longer exists (see task #4366).
    assert!(
        text.contains("dispatch"),
        "starting a backlog task is always called dispatch, got: {text}"
    );
    assert!(
        !text.contains("brainstorm"),
        "the retired brainstorm label must not come back, got: {text}"
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
    task.tmux_window = Some(test_tmux_window("win-4"));
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
    task.tmux_window = Some(test_tmux_window("win-6"));
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
/// (docs/plans/archive/2026-07-31-3809-keybinding-pruning-implementation.md §3) — the footer must
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
    assert!(buffer_contains_ignore_case(&buf, "backlog"));
    assert!(buffer_contains_ignore_case(&buf, "running"));
    assert!(buffer_contains_ignore_case(&buf, "review"));
    assert!(buffer_contains_ignore_case(&buf, "done"));
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
    task.tmux_window = Some(test_tmux_window("win-1"));
    task.sub_status = SubStatus::Crashed;
    let mut app = App::new(vec![task]);
    let buf = render_to_buffer(&mut app, 120, 20);
    assert!(buffer_contains(&buf, "crashed"));
}

#[tokio::test]
async fn render_stale_task_shows_label() {
    let mut task = make_task(1, TaskStatus::Running);
    task.tmux_window = Some(test_tmux_window("win-1"));
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
    task.tmux_window = Some(test_tmux_window("1-fix"));
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
    task.tmux_window = Some(test_tmux_window("1-fix"));
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

/// Read back the foreground colour of the text INSIDE a `[badge]` on a card's
/// metadata line. Locates the literal `[text]` run in the buffer and returns
/// the colour of its first inner character — the brackets themselves are not
/// what carries the claim.
fn badge_fg(buf: &ratatui::buffer::Buffer, text: &str) -> Option<Color> {
    let needle = format!("[{text}]");
    let chars: Vec<char> = needle.chars().collect();
    for y in buf.area.top()..buf.area.bottom() {
        'x: for x in buf.area.left()..buf.area.right() {
            if x as usize + chars.len() > buf.area.right() as usize {
                continue;
            }
            for (i, c) in chars.iter().enumerate() {
                if buf[(x + i as u16, y)].symbol() != c.to_string() {
                    continue 'x;
                }
            }
            // First character past the opening bracket.
            return Some(buf[(x + 1, y)].fg);
        }
    }
    None
}

#[tokio::test]
async fn ci_status_badge_is_coloured_by_state() {
    // core.allium "Card label badges": labels are muted grey, and the three
    // CI-status texts are the one exception — they take the same state colours
    // the card indicator uses for the same three meanings. The exception is by
    // exact text match, so an unrecognised `ci:` value stays muted rather than
    // guessing a colour.
    let mut tasks = Vec::new();
    for (i, label) in [
        "ci:pass",
        "ci:fail",
        "ci:pending",
        "ci:flaky", // not in the vocabulary
    ]
    .iter()
    .enumerate()
    {
        let mut task = make_task(i as i64 + 1, TaskStatus::Backlog);
        // A plain label alongside the CI one, to prove ordinary badges are
        // unaffected on the very same card.
        task.labels = vec![label.to_string(), "dispatch".to_string()];
        tasks.push(task);
    }
    let mut app = App::new(tasks);
    let buf = render_to_buffer(&mut app, 160, 30);

    assert_eq!(badge_fg(&buf, "ci:pass"), Some(GREEN), "[ci:pass] is green");
    assert_eq!(badge_fg(&buf, "ci:fail"), Some(RED), "[ci:fail] is red");
    assert_eq!(
        badge_fg(&buf, "ci:pending"),
        Some(YELLOW),
        "[ci:pending] is yellow"
    );
    assert_eq!(
        badge_fg(&buf, "ci:flaky"),
        Some(MUTED),
        "an unrecognised ci: value renders as an ordinary muted badge"
    );
    assert_eq!(
        badge_fg(&buf, "dispatch"),
        Some(MUTED),
        "an ordinary label stays muted"
    );
}

#[tokio::test]
async fn render_v2_task_card_shows_stripe() {
    // core.allium "Card stripe": every card carries the quarter block ▎
    // (U+258E), the cursor card included. The superseded behaviour swapped in a
    // half block ▌ (U+258C) for the cursor — stripe weight no longer moves with
    // the cursor, because selection is carried by the hued frame and bold title.
    let mut app = App::new(vec![
        make_task(1, TaskStatus::Backlog),
        make_task(2, TaskStatus::Backlog),
    ]);
    let buf = render_to_buffer(&mut app, 120, 20);
    assert!(
        buffer_contains(&buf, "\u{258e}"),
        "task cards must carry the quarter-block stripe"
    );
    assert!(
        !buffer_contains(&buf, "\u{258c}"),
        "the half-block cursor stripe is superseded and must not be rendered"
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
    task.tmux_window = Some(test_tmux_window("win-1"));
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
    let headers = ["BACKLOG", "RUNNING", "REVIEW", "DONE"];
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
    let header = "DONE";
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
/// (docs/plans/archive/2026-07-31-3809-keybinding-pruning-implementation.md §6) — the help overlay
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
/// (docs/plans/archive/2026-07-31-3809-keybinding-pruning-implementation.md §7, hardened by
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
            phoenix: false,
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
    // Top-left corner should be a border character (╭ — rounded)
    assert_eq!(
        buf[(0, 0)].symbol(),
        "╭",
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
async fn focused_column_ground_is_distinct_from_unfocused() {
    let mut app = App::new(vec![
        make_task(1, TaskStatus::Backlog),
        make_task(2, TaskStatus::Running),
    ]);
    // Use wider terminal so 8 columns have enough room for content.
    // Columns use Ratio constraints (3/18, 2/18, ...) so they aren't equal width.
    let buf = render_to_buffer(&mut app, 240, 30);

    // core.allium "Focus is intensity, not colour-vs-absence": the focused
    // column's ground is one step lighter than an unfocused column's, and that
    // step is neutral. Check a row well below the cursor card so the assertion
    // reads column ground rather than card surface.
    let focused_bg = ui::column_bg_color(TaskStatus::Backlog, true);
    let cell = &buf[(1, 15)];
    // Backlog is 3/18 of 240 = 40px. Check well past that at x=120 (middle of board).
    let cell2 = &buf[(120, 15)];

    assert_eq!(
        cell.bg, focused_bg,
        "Focused column should carry the focused ground"
    );
    assert_ne!(
        cell2.bg, focused_bg,
        "An unfocused column's ground must differ from the focused one's"
    );
    assert_ne!(
        cell2.bg,
        Color::Rgb(26, 27, 38),
        "The board ground is painted, not left at the bare terminal background"
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
        buffer_contains_ignore_case(&buf, "backlog"),
        "parent header 'backlog' not found"
    );
    assert!(
        buffer_contains_ignore_case(&buf, "running"),
        "parent header 'running' not found"
    );
    assert!(
        buffer_contains_ignore_case(&buf, "review"),
        "parent header 'review' not found"
    );
    assert!(
        buffer_contains_ignore_case(&buf, "done"),
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
    task.tmux_window = Some(test_tmux_window("task-1"));
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
    task.tmux_window = Some(test_tmux_window("task-1"));
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
    task.tmux_window = Some(test_tmux_window("task-1"));
    let mut app = App::new(vec![task]);
    app.update(Message::NavigateColumn(1)); // Running column
    let buf = render_to_buffer(&mut app, 120, 30);
    assert!(
        buffer_contains_ignore_case(&buf, "running"),
        "Active running task should show 'running'"
    );
}

#[tokio::test]
async fn render_card_review_pr_shows_pr_number() {
    let mut task = make_task(1, TaskStatus::Review);
    task.worktree = Some("/repo/.worktrees/1-task-1".to_string());
    task.tmux_window = Some(test_tmux_window("task-1"));
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
    task.tmux_window = Some(test_tmux_window("task-1"));
    let mut app = App::new(vec![task]);
    app.agents.message_flash.insert(TaskId(1), Instant::now());
    app.update(Message::NavigateColumn(1)); // Running column
    let buf = render_to_buffer(&mut app, 120, 30);
    assert!(
        buffer_contains(&buf, "\u{2709}"),
        "Running task with message_flash set should show '\u{2709}' (envelope)"
    );
}

/// Build a Running task with a message flash stamped `age_secs` ago.
///
/// Backdates the `Instant` rather than sleeping: the flash TTL is a wall-clock
/// threshold, and `./scripts/check-no-test-sleep.sh` rejects sleeping to cross
/// one. See the "No `tokio::time::sleep` in tests" section of docs/conventions.md.
fn app_with_aged_message_flash(age_secs: u64) -> App {
    let mut task = make_task(1, TaskStatus::Running);
    task.sub_status = SubStatus::Active;
    task.worktree = Some("/repo/.worktrees/1-task-1".to_string());
    task.tmux_window = Some(test_tmux_window("task-1"));
    let mut app = App::new(vec![task]);
    let stamped = Instant::now()
        .checked_sub(Duration::from_secs(age_secs))
        .expect("monotonic clock must reach back far enough to age a flash");
    app.agents.message_flash.insert(TaskId(1), stamped);
    app.update(Message::NavigateColumn(1)); // Running column
    app
}

#[tokio::test]
async fn message_flash_envelope_outlives_the_old_three_second_window() {
    // core.allium "Message flash": the flash lasts MESSAGE_FLASH_TTL (30s), long
    // enough that a human whose attention is elsewhere still sees it. Ten seconds
    // in — well past the superseded 3s window — the envelope must still render.
    let mut app = app_with_aged_message_flash(10);
    let buf = render_to_buffer(&mut app, 120, 30);
    assert!(
        buffer_contains(&buf, "\u{2709}"),
        "a 10s-old flash must still show the envelope; the window is {:?}",
        crate::tui::MESSAGE_FLASH_TTL
    );
}

#[tokio::test]
async fn message_flash_expires_once_past_its_ttl() {
    // The other side of the threshold: a flash older than MESSAGE_FLASH_TTL is
    // swept by `tick_message_flash` and stops rendering. Without this the TTL
    // could be raised to infinity and nothing would notice.
    let ttl = crate::tui::MESSAGE_FLASH_TTL.as_secs();
    let mut app = app_with_aged_message_flash(ttl + 1);
    let _ = app.handle_tick();
    assert!(
        !app.agents.message_flash.contains_key(&TaskId(1)),
        "a flash older than {ttl}s must be swept from the tracking map"
    );
    let buf = render_to_buffer(&mut app, 120, 30);
    assert!(
        !buffer_contains(&buf, "\u{2709}"),
        "an expired flash must not render the envelope"
    );
}

/// Two Running tasks: the cursor sits on the first, the second carries a flash
/// stamped `age_secs` ago. Isolating the flash onto a non-cursor card is what
/// lets a test read the flash's own contribution to the frame colour — on the
/// cursor card the hue would be there either way.
fn app_with_flash_on_a_non_cursor_card(age_secs: u64) -> App {
    let mut tasks = Vec::new();
    for id in [1, 2] {
        let mut t = make_task(id, TaskStatus::Running);
        t.sub_status = SubStatus::Active;
        t.worktree = Some(format!("/repo/.worktrees/{id}-task"));
        t.tmux_window = Some(test_tmux_window(&format!("task-{id}")));
        // Seed recent activity. These tests drive `handle_tick`, which reclassifies
        // an inactive Running task as Stale — and Stale now claims an amber state
        // border, which would put a colour on a frame this fixture means to keep
        // healthy. Without this the tasks go stale mid-test and the assertions read
        // the wrong cause.
        t.last_pre_tool_use_at = Some(Utc::now());
        tasks.push(t);
    }
    let mut app = App::new(tasks);
    let stamped = Instant::now()
        .checked_sub(Duration::from_secs(age_secs))
        .expect("monotonic clock must reach back far enough to age a flash");
    app.agents.message_flash.insert(TaskId(2), stamped);
    app.update(Message::NavigateColumn(1)); // Running column; cursor on task 1
    app
}

#[tokio::test]
async fn message_flash_never_colours_the_card_frame() {
    // core.allium "Message flash": the flash is carried by its warm fill and its
    // envelope glyph, and it leaves the frame alone.
    //
    // It used to take the column hue, which was safe only because the envelope was
    // co-terminous with it — a whole exception resting on one timing coincidence.
    // Worse, once the frame began carrying state that hue collided head-on with
    // needs-input: in Running the column hue *is* amber. Giving the frame up
    // deleted the exception and the collision together, so what needs guarding now
    // is simply that the flash never touches the frame.
    let neutral = ui::card_border_color();
    let cursor = ui::cursor_border_color();
    let running_hue = ui::column_color(TaskStatus::Running);

    let mut app = app_with_flash_on_a_non_cursor_card(crate::tui::MESSAGE_FLASH_TTL.as_secs() - 1);
    let _ = app.handle_tick();
    let buf = render_to_buffer(&mut app, 120, 30);

    assert!(
        buffer_contains(&buf, "\u{2709}"),
        "inside the TTL the envelope must render — otherwise this test proves nothing"
    );

    let corners = cells_with_symbol(&buf, "\u{256d}");
    let frames: Vec<Color> = corners.iter().map(|c| c.fg).collect();
    assert!(
        !frames.contains(&running_hue),
        "a live flash must not put the column hue on any frame; in Running that hue \
         is the same amber needs-input claims, so it would read as a blocked agent"
    );
    // Both tasks are healthy, so the only non-neutral frame is the cursor's.
    let non_neutral: Vec<Color> = frames.iter().copied().filter(|c| *c != neutral).collect();
    assert_eq!(
        non_neutral,
        vec![cursor],
        "with a flash live on a healthy non-cursor card, the only non-neutral frame \
         on the board is the cursor white"
    );
}

#[tokio::test]
async fn message_flash_render_and_sweep_share_one_threshold() {
    // The duration used to be hardcoded in both `tick_message_flash` and the
    // card renderer with no shared constant, so the two could silently disagree —
    // the map holding an entry the card no longer draws, or the reverse. Just
    // inside the TTL, both must still agree the flash is live.
    let ttl = crate::tui::MESSAGE_FLASH_TTL.as_secs();
    let mut app = app_with_aged_message_flash(ttl - 1);
    let _ = app.handle_tick();
    assert!(
        app.agents.message_flash.contains_key(&TaskId(1)),
        "a flash one second inside the TTL must survive the sweep"
    );
    let buf = render_to_buffer(&mut app, 120, 30);
    assert!(
        buffer_contains(&buf, "\u{2709}"),
        "a flash the sweep kept must still render the envelope"
    );
}

#[tokio::test]
async fn render_card_message_flash_sent_shows_outgoing_glyph() {
    let mut task = make_task(1, TaskStatus::Running);
    task.sub_status = SubStatus::Active;
    task.worktree = Some("/repo/.worktrees/1-task-1".to_string());
    task.tmux_window = Some(test_tmux_window("task-1"));
    let mut app = App::new(vec![task]);
    app.agents
        .message_flash_sent
        .insert(TaskId(1), Instant::now());
    app.update(Message::NavigateColumn(1)); // Running column
    let buf = render_to_buffer(&mut app, 120, 30);
    assert!(
        buffer_contains(&buf, "\u{27a4}"),
        "Running task with message_flash_sent set should show '\u{27a4}' (outgoing arrow)"
    );
    assert!(
        !buffer_contains(&buf, "\u{2709}"),
        "a sent-only flash must not also show the received envelope"
    );
}

#[tokio::test]
async fn render_card_message_flash_sent_expires_once_past_its_ttl() {
    let mut task = make_task(1, TaskStatus::Running);
    task.sub_status = SubStatus::Active;
    task.worktree = Some("/repo/.worktrees/1-task-1".to_string());
    task.tmux_window = Some(test_tmux_window("task-1"));
    let mut app = App::new(vec![task]);
    let ttl = crate::tui::MESSAGE_FLASH_TTL.as_secs();
    let stamped = Instant::now()
        .checked_sub(Duration::from_secs(ttl + 1))
        .expect("monotonic clock must reach back far enough to age a flash");
    app.agents.message_flash_sent.insert(TaskId(1), stamped);
    app.update(Message::NavigateColumn(1));
    let _ = app.handle_tick();
    assert!(
        !app.agents.message_flash_sent.contains_key(&TaskId(1)),
        "a sent-flash older than {ttl}s must be swept from the tracking map"
    );
    let buf = render_to_buffer(&mut app, 120, 30);
    assert!(
        !buffer_contains(&buf, "\u{27a4}"),
        "an expired sent-flash must not render the outgoing glyph"
    );
}

#[tokio::test]
async fn a_sent_flash_expiring_marks_the_app_dirty() {
    // mark_tick_dirty's dirty check only diffed message_flash's length before
    // and after a tick — it never looked at message_flash_sent, so a sent-only
    // flash (no matching change to message_flash) expiring on its own would
    // never mark the app dirty, and the stale ➤ glyph would keep rendering
    // until an unrelated event happened to set app.dirty.
    let mut task = make_task(1, TaskStatus::Running);
    task.sub_status = SubStatus::Active;
    task.worktree = Some("/repo/.worktrees/1-task-1".to_string());
    task.tmux_window = Some(test_tmux_window("task-1"));
    // Seed recent activity so `tick_sub_status` reclassifies nothing this
    // tick — otherwise its own `self.dirty = true` on a sub_status change
    // would mask the bug this test targets. Same reasoning as
    // `app_with_flash_on_a_non_cursor_card` above.
    task.last_pre_tool_use_at = Some(Utc::now());
    let mut app = App::new(vec![task]);
    let ttl = crate::tui::MESSAGE_FLASH_TTL.as_secs();
    let stamped = Instant::now()
        .checked_sub(Duration::from_secs(ttl + 1))
        .expect("monotonic clock must reach back far enough to age a flash");
    app.agents.message_flash_sent.insert(TaskId(1), stamped);
    app.update(Message::NavigateColumn(1));
    app.dirty = false;

    let _ = app.handle_tick();

    assert!(
        !app.agents.message_flash_sent.contains_key(&TaskId(1)),
        "precondition: the sent-flash must actually have been swept"
    );
    assert!(
        app.dirty,
        "the tick that sweeps an expired sent-flash must mark the app dirty, \
         or the stale outgoing-arrow glyph keeps rendering"
    );
}

#[tokio::test]
async fn render_card_message_flash_shows_both_glyphs_when_sent_and_received() {
    let mut task = make_task(1, TaskStatus::Running);
    task.sub_status = SubStatus::Active;
    task.worktree = Some("/repo/.worktrees/1-task-1".to_string());
    task.tmux_window = Some(test_tmux_window("task-1"));
    let mut app = App::new(vec![task]);
    app.agents.message_flash.insert(TaskId(1), Instant::now());
    app.agents
        .message_flash_sent
        .insert(TaskId(1), Instant::now());
    app.update(Message::NavigateColumn(1));
    let buf = render_to_buffer(&mut app, 120, 30);
    assert!(
        buffer_contains(&buf, "\u{2709}"),
        "a task both sent and received must still show the envelope"
    );
    assert!(
        buffer_contains(&buf, "\u{27a4}"),
        "a task both sent and received must still show the outgoing arrow"
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
    // The focused header brightens toward the foreground rather than dropping
    // to grey; the hue stays Backlog's (core.allium: "Focus is intensity, not
    // colour-vs-absence").
    let expected_fg = ui::column_header_fg(TaskStatus::Backlog, true);
    let expected_bg = ui::column_header_bg(TaskStatus::Backlog, true);
    let target = "BACKLOG";
    let mut found = false;
    for y in area.top()..area.bottom() {
        for x in area.left()..area.right().saturating_sub(target.len() as u16 - 1) {
            let matches = target
                .bytes()
                .enumerate()
                .all(|(i, ch)| buf[(x + i as u16, y)].symbol().as_bytes().first() == Some(&ch));
            if matches {
                let cell = &buf[(x, y)];
                if cell.fg == expected_fg && cell.bg == expected_bg {
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
        "Focused Backlog header should render its label on the focused header bar"
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
        summary_row.contains("BACKLOG"),
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
        summary_row.contains("ARCHIVE"),
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

// ── Column identity and focus (core.allium: Column Identity and Focus) ──────

/// Every column that renders a ground, including the Archive edge column.
const GROUND_COLUMNS: [TaskStatus; 5] = [
    TaskStatus::Backlog,
    TaskStatus::Running,
    TaskStatus::Review,
    TaskStatus::Done,
    TaskStatus::Archived,
];

/// Signed lightness on the shared scale whose zero point is the bare terminal
/// background (Tokyo Night `#1a1b26`). Mirrors `BoardNeutralRamp` in
/// core.allium: only the ordering of these numbers is normative, and values
/// below the terminal background are negative.
fn lightness_vs_terminal_bg(c: Color) -> i32 {
    const BG: i32 = 26 + 27 + 38;
    match c {
        Color::Rgb(r, g, b) => i32::from(r) + i32::from(g) + i32::from(b) - BG,
        other => panic!("expected an Rgb surface, got {other:?}"),
    }
}

#[tokio::test]
async fn board_ground_is_uniform_across_columns() {
    // core.allium "Column ground and card surface": every column renders the
    // *same* ground at a given focus state — there is no per-column tint and the
    // ground carries no hue. The superseded design derived each column's ground
    // from its identity colour; that is the regression this guards.
    for is_focused in [false, true] {
        let expected = ui::column_bg_color(TaskStatus::Backlog, is_focused);
        for status in GROUND_COLUMNS {
            assert_eq!(
                ui::column_bg_color(status, is_focused),
                expected,
                "{status:?} (focused={is_focused}) must share the uniform board ground"
            );
        }
    }
}

#[tokio::test]
async fn neutral_ramp_is_strictly_ascending() {
    // core.allium invariant NeutralRampIsStrictlyAscending:
    //   column_ground_unfocused < column_ground_focused < card_surface
    let unfocused = lightness_vs_terminal_bg(ui::column_bg_color(TaskStatus::Backlog, false));
    let focused = lightness_vs_terminal_bg(ui::column_bg_color(TaskStatus::Backlog, true));
    let card = lightness_vs_terminal_bg(ui::card_surface_color());

    assert!(
        unfocused < focused,
        "focused ground ({focused}) must be lighter than unfocused ({unfocused})"
    );
    assert!(
        focused < card,
        "card surface ({card}) must be lighter than the focused ground ({focused})"
    );
}

#[tokio::test]
async fn board_ground_is_recessed_below_terminal_background() {
    // core.allium invariant GroundIsRecessedBelowTerminalBackground: the ground
    // sits *below* the bare terminal background so cards read as raised rather
    // than inset. Assumes the dark terminal the palette is built for.
    let unfocused = lightness_vs_terminal_bg(ui::column_bg_color(TaskStatus::Backlog, false));
    assert!(
        unfocused < 0,
        "unfocused ground must be recessed below the terminal background, got {unfocused}"
    );
}

#[tokio::test]
async fn selection_does_not_lift_the_fill() {
    // core.allium invariant SelectionDoesNotLiftTheFill: a selected card's
    // surface is exactly a resting card's. Its emphasis lives in frame hue and
    // title weight, neither of which is a tint. A test asserting "selected is
    // lighter than resting" would be asserting something this design
    // deliberately does not do.
    assert_eq!(
        ui::selected_card_surface_color(),
        ui::card_surface_color(),
        "selection must not change the card fill"
    );
}

#[tokio::test]
async fn resting_card_border_is_neutral() {
    // core.allium "Task card frame": the frame colour is neutral for a resting
    // card and the column's identity colour only for the selected card. A
    // resting border must therefore never equal any column's identity colour.
    let border = ui::card_border_color();
    // Archive needs no special case any more: `column_color(Archived)` is
    // ARCHIVE_STRIPE, the colour the archive renderer actually threads in, so one
    // loop covers every column.
    for status in GROUND_COLUMNS {
        assert_ne!(
            border,
            ui::column_color(status),
            "a resting card's border must not carry {status:?}'s identity colour"
        );
    }
}

/// A colour's channel signature: the RGB channels ranked brightest to dimmest.
///
/// This is a hue-family fingerprint that survives linear mixing toward any
/// neutral: `BLUE` is `b > g > r` and stays so whether dimmed toward the header
/// fill or brightened toward white, while `PURPLE` is `b > r > g` throughout.
/// Absolute distance does not work for this — a *dimmed* colour is by definition
/// far from its own undimmed source, so distance-to-own-hue is unsatisfiable.
fn channel_signature(c: Color) -> [usize; 3] {
    match c {
        Color::Rgb(r, g, b) => {
            let mut idx = [0usize, 1, 2];
            let v = [r, g, b];
            idx.sort_by_key(|&i| std::cmp::Reverse(v[i]));
            idx
        }
        other => panic!("expected an Rgb colour, got {other:?}"),
    }
}

#[tokio::test]
async fn each_header_label_keeps_its_column_hue_signature() {
    // The header labels are derived from `column_color` by a const mix, and this
    // asserts the property that derivation is *for*: whatever the mix does to
    // brightness, the label still reads as the same hue family as its source.
    //
    // The other three header tests — uniformity, per-column distinctness, and the
    // brightness ordering — would every one of them pass on a set of hand-picked
    // literals having nothing to do with the column hues. This is the one that
    // would not, because preserving all five channel signatures by accident is a
    // far stronger coincidence than being merely distinct and correctly ordered.
    for is_focused in [false, true] {
        for status in GROUND_COLUMNS {
            let hue = ui::column_color(status);
            let label = ui::column_header_fg(status, is_focused);
            assert_eq!(
                channel_signature(label),
                channel_signature(hue),
                "{status:?} (focused={is_focused}): label {label:?} does not share the \
                 channel signature of its hue {hue:?} — it is no longer derived from it"
            );
        }
    }
}

/// The position of the first cell whose symbol is exactly `sym`, scanning
/// top-to-bottom then left-to-right.
fn position_of_symbol(buf: &ratatui::buffer::Buffer, sym: &str) -> Option<(u16, u16)> {
    for y in buf.area.top()..buf.area.bottom() {
        for x in buf.area.left()..buf.area.right() {
            if buf[(x, y)].symbol() == sym {
                return Some((x, y));
            }
        }
    }
    None
}

#[tokio::test]
async fn cards_are_inset_by_one_cell_of_column_ground() {
    // core.allium "Task card frame": the card is inset from the column by one
    // cell on each side, and those margin cells are ground, not card surface.
    //
    // This exists because the inset was implemented before it was specified, and
    // nothing enforced it — it could have been widened or dropped with a green
    // suite. The assertions below pin the margin's width, its colour, and the
    // fact that the rail immediately inside it is lit, in one place.
    let mut app = App::new(vec![make_task(1, TaskStatus::Backlog)]);
    let buf = render_to_buffer(&mut app, 120, 30);

    let (cx, cy) = position_of_symbol(&buf, "\u{256d}").expect("expected a framed card");
    assert!(
        cx >= 1,
        "a card must not begin at the column's first cell; found the corner at x={cx}"
    );

    let ground = ui::column_bg_color(TaskStatus::Backlog, true);
    let surface = ui::card_surface_color();

    assert_eq!(
        buf[(cx - 1, cy)].bg,
        ground,
        "the cell left of a card's corner must be column ground, not card surface"
    );
    assert_eq!(
        buf[(cx, cy)].bg,
        surface,
        "the card's own corner must be lit by the card surface"
    );

    // The rail on the row below the top border: same column, and lit.
    assert_eq!(
        buf[(cx, cy + 1)].symbol(),
        "\u{2502}",
        "the row below a card's top border must open with its left rail"
    );
    assert_eq!(
        buf[(cx, cy + 1)].bg,
        surface,
        "a card's side rail must be lit by the card surface"
    );
    assert_eq!(
        buf[(cx - 1, cy + 1)].bg,
        ground,
        "the margin beside a card's rail must be column ground"
    );

    // Right margin: find the closing corner on the same row.
    let rx = (cx..buf.area.right())
        .find(|&x| buf[(x, cy)].symbol() == "\u{256e}")
        .expect("expected a closing top corner on the same row");
    assert_eq!(
        buf[(rx + 1, cy)].bg,
        ground,
        "the cell right of a card's closing corner must be column ground"
    );
}

/// Every position in `buf` whose symbol is exactly `sym`.
fn cells_with_symbol<'a>(
    buf: &'a ratatui::buffer::Buffer,
    sym: &'a str,
) -> Vec<&'a ratatui::buffer::Cell> {
    let mut out = Vec::new();
    for y in buf.area.top()..buf.area.bottom() {
        for x in buf.area.left()..buf.area.right() {
            let cell = &buf[(x, y)];
            if cell.symbol() == sym {
                out.push(cell);
            }
        }
    }
    out
}

#[tokio::test]
async fn every_card_frame_is_lit_by_the_card_surface() {
    // core.allium "Task card frame": the whole card is lit, frame included. The
    // border rows and side rails carry the *card surface* background, not the
    // column ground, so the card's boundary is the change of colour at its outer
    // edge. The rejected alternative painted the border on the ground.
    let mut app = App::new(vec![
        make_task(1, TaskStatus::Backlog),
        make_task(2, TaskStatus::Backlog),
    ]);
    let buf = render_to_buffer(&mut app, 120, 30);

    // All four corners, not just ╭ — the bottom border and the closing rails are
    // as much part of "the whole card is lit" as the top one. The side rails are
    // covered by `cards_are_inset_by_one_cell_of_column_ground`, which can tell a
    // card rail from the column separator by position; both use the │ glyph, so a
    // symbol scan alone cannot.
    let mut checked = 0usize;
    for glyph in ["\u{256d}", "\u{256e}", "\u{2570}", "\u{256f}"] {
        let corners = cells_with_symbol(&buf, glyph);
        assert!(
            !corners.is_empty(),
            "expected at least one card corner {glyph} to render"
        );
        for cell in corners {
            assert_eq!(
                cell.bg,
                ui::card_surface_color(),
                "card corner {glyph} must sit on the card surface, not the column ground"
            );
            checked += 1;
        }
    }
    assert!(
        checked >= 8,
        "expected both cards' four corners, saw {checked}"
    );
}

/// The cell holding an epic card's stripe, and the cell holding its title's
/// first character, on the row that starts with the given `#id` marker.
///
/// Located by scanning for the id rather than by fixed coordinates, so the test
/// does not silently start reading a different card when layout shifts.
fn epic_card_row(
    buf: &ratatui::buffer::Buffer,
    id: &str,
) -> Option<(ratatui::buffer::Cell, ratatui::buffer::Cell)> {
    for y in buf.area.top()..buf.area.bottom() {
        for x in buf.area.left()..buf.area.right() {
            if buf[(x, y)].symbol() != "\u{258e}" {
                continue;
            }
            // "▎ #10 Epic 10" — the id begins two cells right of the stripe.
            let after: String = (x + 1..(x + 8).min(buf.area.right()))
                .map(|xx| buf[(xx, y)].symbol())
                .collect();
            if after.trim_start().starts_with(id) {
                // First title character: past "▎ #10 ".
                let title_x = x + 2 + id.len() as u16 + 1;
                return Some((buf[(x, y)].clone(), buf[(title_x, y)].clone()));
            }
        }
    }
    None
}

#[tokio::test]
async fn epic_cards_carry_purple_identity_and_a_bold_title_at_rest() {
    // core.allium "Epic cards": an epic is its own identity object. Its stripe is
    // PURPLE in every column rather than the column's hue, and its title is bold
    // *unconditionally* — which is why bold cannot be a cursor signal on an epic,
    // and why the frame is the only cursor cue an epic card has.
    let mut app = App::new(vec![make_task(1, TaskStatus::Backlog)]);
    app.board.epics = vec![make_epic(10)];
    // Cursor on the task at row 0, so the epic below it is at rest.
    app.selection_mut().set_column(1);
    app.selection_mut().set_row(1, 0);
    let buf = render_to_buffer(&mut app, 120, 30);

    // The palette PURPLE, written out rather than taken from `column_color(Review)`.
    // Review happens to share the token, but an epic's purple is its *own*
    // identity — sourcing it from a column would encode exactly the conflation this
    // test exists to disprove, and would keep passing if epics started following
    // their column.

    let (stripe, title) = epic_card_row(&buf, "#10").expect("expected the epic card to render");
    assert_eq!(
        stripe.fg, PURPLE,
        "an epic's stripe is PURPLE — its own identity, never its column's"
    );
    assert_ne!(
        stripe.fg,
        ui::column_color(TaskStatus::Backlog),
        "an epic sitting in Backlog must not take Backlog's hue"
    );
    assert!(
        title.modifier.contains(Modifier::BOLD),
        "a resting epic's title is bold unconditionally, so bold cannot mark the cursor"
    );
}

#[tokio::test]
async fn epic_view_tints_the_enclosing_panel_but_not_the_column_grounds() {
    // core.allium "Column ground and card surface": inside an epic the *enclosing
    // panel* is faintly purple as a mode signal — it says "you are inside an epic",
    // not "this column is purple" — while the column grounds within it stay the
    // uniform neutral.
    //
    // This is the only place purple means *mode* rather than *epic identity*, so a
    // regression would not read as obviously wrong to whoever found it. Nothing
    // asserted it: there is one production site and the snapshots carry no style.
    const PANEL_TINT: Color = Color::Rgb(24, 20, 34);

    let mut app = App::new(vec![make_task(1, TaskStatus::Backlog)]);
    app.board.epics = vec![make_epic(10)];
    app.board.view_mode = crate::tui::types::ViewMode::Epic {
        epic_id: crate::models::EpicId(10),
        selection: crate::tui::types::BoardSelection::new_for_epic(),
        parent: Box::new(crate::tui::types::ViewMode::Board(
            crate::tui::types::BoardSelection::new(),
        )),
    };
    let buf = render_to_buffer(&mut app, 160, 30);

    let panel_cells = (buf.area.top()..buf.area.bottom())
        .flat_map(|y| (buf.area.left()..buf.area.right()).map(move |x| (x, y)))
        .filter(|&(x, y)| buf[(x, y)].bg == PANEL_TINT)
        .count();
    assert!(
        panel_cells > 0,
        "epic view must tint its enclosing panel as a mode signal"
    );

    // ...and the grounds inside it are untouched.
    let ground = ui::column_bg_color(TaskStatus::Backlog, true);
    let ground_cells = (buf.area.top()..buf.area.bottom())
        .flat_map(|y| (buf.area.left()..buf.area.right()).map(move |x| (x, y)))
        .filter(|&(x, y)| buf[(x, y)].bg == ground)
        .count();
    assert!(
        ground_cells > 0,
        "the column grounds inside an epic view must stay the uniform neutral, not \
         take the panel's tint"
    );
}

/// Whether row `y` is painted with column ground colour anywhere across its
/// width — the board fills a column's whole area with its ground colour
/// regardless of card content, so this is true for any row genuinely inside
/// the kanban board and false for a row that belongs to another band (the
/// idle input panel's gap, or its bordered box when a form is active).
fn row_is_board_ground(buf: &Buffer, y: u16) -> bool {
    let unfocused = ui::column_bg_color(TaskStatus::Review, false);
    let focused = ui::column_bg_color(TaskStatus::Backlog, true);
    let area = buf.area();
    (area.left()..area.right()).any(|x| {
        let bg = buf[(x, y)].bg;
        bg == unfocused || bg == focused
    })
}

#[tokio::test]
async fn idle_input_panel_lets_columns_reach_the_status_bar() {
    // core.allium "Board Vertical Layout": with no input mode active the input
    // panel is zero height, so the kanban board claims the full remaining
    // height and columns run uninterrupted down to the status bar — no empty
    // bordered box in between.
    let mut app = App::new(vec![make_task(1, TaskStatus::Backlog)]);
    let (width, height) = (160, 30);
    let buf = render_to_buffer(&mut app, width, height);

    let status_bar_row = height - 1;
    let last_board_row = status_bar_row - 1;

    assert!(
        row_is_board_ground(&buf, last_board_row),
        "the row directly above the status bar must be board ground when idle, \
         not a gap left by the empty input panel"
    );
}

#[tokio::test]
async fn a_status_bar_confirmation_does_not_reserve_the_input_panel() {
    // core.allium "Board Vertical Layout": a y/n confirmation is prompted in the
    // status bar, not the input panel, so it is idle from the panel's
    // perspective — the board must keep full height under it, the same as the
    // default Normal mode.
    let mut app = App::new(vec![make_task(1, TaskStatus::Backlog)]);
    app.input.mode = InputMode::ConfirmDelete;
    let (width, height) = (160, 30);
    let buf = render_to_buffer(&mut app, width, height);

    let status_bar_row = height - 1;
    let last_board_row = status_bar_row - 1;

    assert!(
        row_is_board_ground(&buf, last_board_row),
        "a status-bar-only confirmation must not leave an empty gap where the \
         input panel used to sit"
    );
}

#[tokio::test]
async fn active_input_mode_reserves_the_panel_and_shortens_the_board() {
    // core.allium "Board Vertical Layout": once an input mode is active, the
    // panel reserves its computed height and renders a bordered, titled box —
    // and the kanban board shrinks to make room for it, rather than the two
    // bands overlapping or the board staying full height underneath it.
    let mut app = App::new(vec![make_task(1, TaskStatus::Backlog)]);
    app.input.mode = InputMode::InputTitle;
    let (width, height) = (160, 30);
    let buf = render_to_buffer(&mut app, width, height);

    assert!(
        buffer_contains(&buf, "New Task"),
        "the input panel must render its bordered, titled box while a form is active"
    );

    let status_bar_row = height - 1;
    let last_board_row = status_bar_row - 1;

    assert!(
        !row_is_board_ground(&buf, last_board_row),
        "the row directly above the status bar must belong to the reserved input \
         panel while a form is active, not the kanban board"
    );
}

#[tokio::test]
async fn the_cursor_card_title_is_bold_and_a_resting_one_is_not() {
    // core.allium "Selection": the cursor is marked by two things — the white frame
    // and a bold title. Only the frame was asserted. The *epic* bold title is
    // covered, deliberately, because it is unconditional there; that is what
    // disguised this omission, since a grep for bold coverage finds a hit.
    let mut app = App::new(vec![
        make_task(1, TaskStatus::Backlog),
        make_task(2, TaskStatus::Backlog),
    ]);
    app.selection_mut().set_column(1);
    app.selection_mut().set_row(1, 0);
    let buf = render_to_buffer(&mut app, 120, 30);

    let bold_for = |id: &str| -> bool {
        for y in buf.area.top()..buf.area.bottom() {
            for x in buf.area.left()..buf.area.right().saturating_sub(6) {
                let run: String = (x..x + 6).map(|xx| buf[(xx, y)].symbol()).collect();
                if run.starts_with(id) {
                    // Title begins one cell past "#N ".
                    let tx = x + id.len() as u16 + 1;
                    return buf[(tx, y)].modifier.contains(Modifier::BOLD);
                }
            }
        }
        panic!("card {id} did not render");
    };

    assert!(
        bold_for("#1"),
        "the cursor card's title must be bold — it is one of selection's two markers"
    );
    assert!(
        !bold_for("#2"),
        "a resting card's title must not be bold, or bold says nothing about the cursor"
    );
}

#[tokio::test]
async fn select_all_checkbox_fill_is_neutral_in_every_column() {
    // core.allium "Column header bar": the fill behind the focused column's
    // select-all checkbox is a single neutral shared by every column.
    //
    // It replaced a per-column *hued* ramp, and a hued checkbox is exactly what the
    // neutral-fill treatment exists to rule out — so a regression here restores the
    // thing that was deliberately removed, and does it silently. The header
    // snapshots cannot catch it: they are text, not style.
    for (nav, &status) in TaskStatus::ALL.iter().enumerate() {
        let mut app = App::new(vec![make_task(1, status)]);
        app.selection_mut().set_column(nav + 1);
        // Move up off the first row to park the cursor on the column header.
        app.update(Message::NavigateRow(-1));
        assert!(
            app.on_select_all(),
            "{status:?}: expected the cursor to land on the select-all header"
        );

        let buf = render_to_buffer(&mut app, 160, 30);
        let checkbox = (buf.area.left()..buf.area.right())
            .map(|x| buf[(x, 1)].clone())
            .find(|c| c.symbol() == "[")
            .unwrap_or_else(|| panic!("{status:?}: expected a select-all checkbox on the header"));

        // Asserted against `card_border_color`, which is what the spec claims the
        // value *is* — not against `select_all_highlight_bg`, which is the function
        // the renderer already calls. Comparing a render to its own source restates
        // the implementation instead of checking it: change the function and both
        // sides move together, and the test passes on a value nobody chose.
        assert_eq!(
            checkbox.bg,
            ui::card_border_color(),
            "{status:?}: the checkbox fill must be the same neutral as a resting \
             card's border"
        );
        for &other in TaskStatus::ALL.iter() {
            assert_ne!(
                checkbox.bg,
                ui::column_color(other),
                "{status:?}: the checkbox fill must not be any column's identity hue"
            );
        }
    }
}

#[tokio::test]
async fn a_card_spends_four_cells_of_its_column_on_chrome() {
    // core.allium "Task card frame": two ground margins plus two frame rails, and
    // every one of those cells comes out of the title budget.
    //
    // `cards_are_inset_by_one_cell_of_column_ground` pins the margin's width; this
    // pins the *total*, which is the number that actually reaches truncation. They
    // are separate claims: narrowing the rails while widening the margin would keep
    // the inset test green and silently change what fits on a card.
    const CHROME: u16 = 4;

    let mut app = App::new(vec![make_task(1, TaskStatus::Backlog)]);
    let buf = render_to_buffer(&mut app, 160, 30);

    // The first column runs from the left edge to the first separator, so the
    // separator's x *is* that column's width.
    let probe_y = 15;
    let col_width = (buf.area.left()..buf.area.right())
        .find(|&x| {
            let c = &buf[(x, probe_y)];
            c.symbol() == "\u{2502}" && c.fg == BORDER
        })
        .expect("expected a column separator on an empty board row");

    let (cx, cy) = position_of_symbol(&buf, "\u{256d}").expect("expected a framed card");
    let rx = (cx..buf.area.right())
        .find(|&x| buf[(x, cy)].symbol() == "\u{256e}")
        .expect("expected the card's closing corner on the same row");

    let content = rx - cx - 1; // cells strictly between the two rails
    assert_eq!(
        content + CHROME,
        col_width,
        "a card must spend exactly {CHROME} cells of its {col_width}-cell column on \
         chrome; content measured {content}"
    );
}

#[tokio::test]
async fn flat_view_epic_breadcrumb_is_purple() {
    // core.allium "Epic cards": the breadcrumb row heading a group of epic-owned
    // tasks in flattened view carries epic purple on the same terms as the card
    // stripe — it is the second surface that claim covers.
    //
    // The flat-view snapshots render this row, but `.snap` files are text only and
    // carry no style, so a breadcrumb that lost its hue would leave every one of
    // them byte-identical. Nothing was checking the colour until this.

    let mut app = App::new(vec![]);
    app.board.epics = vec![make_epic(10)];
    let mut t1 = make_task(1, TaskStatus::Running);
    t1.epic_id = Some(crate::models::EpicId(10));
    t1.sort_order = Some(10);
    app.board.tasks = vec![t1];
    app.board.flattened = true;
    app.selection_mut().set_column(2); // Running
    let buf = render_to_buffer(&mut app, 120, 30);

    // Target the breadcrumb row itself. Counting purple anywhere in the buffer is
    // not enough: the epic *card* is purple too, so such a test passes even with
    // the breadcrumb drawn in grey — it measures the card and reports the
    // breadcrumb. Find the row that opens with the "── " rule and read the colour
    // of the title that follows it.
    let mut breadcrumb_title_colours: Vec<Color> = Vec::new();
    'rows: for y in buf.area.top()..buf.area.bottom() {
        for x in buf.area.left()..buf.area.right().saturating_sub(3) {
            let is_rule_prefix = buf[(x, y)].symbol() == "\u{2500}"
                && buf[(x + 1, y)].symbol() == "\u{2500}"
                && buf[(x + 2, y)].symbol() == " ";
            if !is_rule_prefix {
                continue;
            }
            let title: String = (x + 3..(x + 10).min(buf.area.right()))
                .map(|xx| buf[(xx, y)].symbol())
                .collect();
            if title.starts_with("Epic 10") {
                breadcrumb_title_colours = (x + 3..x + 10).map(|xx| buf[(xx, y)].fg).collect();
                break 'rows;
            }
        }
    }

    assert!(
        !breadcrumb_title_colours.is_empty(),
        "expected a \"── Epic 10\" breadcrumb row in the flattened view"
    );
    for c in &breadcrumb_title_colours {
        assert_eq!(
            *c, PURPLE,
            "the breadcrumb's title must be epic purple, not {c:?}"
        );
    }
}

#[tokio::test]
async fn scroll_indicators_follow_the_column_top_rule() {
    // core.allium's named exception covers the scroll indicators as well as the top
    // rule: hued while focused, neutral grey while not. They share one colour in
    // the renderer, but that is an implementation fact rather than an asserted one,
    // so splitting them would otherwise be caught by nothing.

    // Enough cards in both columns to overflow a short board.
    let mut tasks = Vec::new();
    for id in 1..=12 {
        tasks.push(make_task(id, TaskStatus::Backlog));
    }
    for id in 13..=24 {
        tasks.push(make_task(id, TaskStatus::Running));
    }
    let mut app = App::new(tasks);
    let buf = render_to_buffer(&mut app, 160, 24);

    let mut arrows: Vec<Color> = Vec::new();
    for y in buf.area.top()..buf.area.bottom() {
        for x in buf.area.left()..buf.area.right() {
            let sym = buf[(x, y)].symbol();
            if sym == "\u{25b2}" || sym == "\u{25bc}" {
                arrows.push(buf[(x, y)].fg);
            }
        }
    }
    assert!(
        arrows.len() >= 2,
        "expected an overflow indicator in both the focused and an unfocused column, \
         found {arrows:?}"
    );
    // Backlog is focused on a fresh board; Running is not.
    assert!(
        arrows.contains(&ui::column_color(TaskStatus::Backlog)),
        "the focused column's scroll indicator must carry its identity hue, got {arrows:?}"
    );
    assert!(
        arrows.contains(&MUTED),
        "an unfocused column's scroll indicator must be neutral grey, got {arrows:?}"
    );
}

#[tokio::test]
async fn selected_epic_frames_in_the_cursor_white_not_purple() {
    // core.allium "Epic cards": the cursor white applies to epics too, with no
    // exemption. A purple frame would put Review's own identity hue on a card
    // frame — the collision the white exists to prevent, surviving on the one card
    // type that had escaped it.
    let mut app = make_app_with_epic_selected();
    let buf = render_to_buffer(&mut app, 120, 30);

    let corners = cells_with_symbol(&buf, "\u{256d}");
    let purple = ui::column_color(TaskStatus::Review);
    let cursor = ui::cursor_border_color();
    let neutral = ui::card_border_color();

    assert!(
        corners.iter().any(|c| c.fg == cursor),
        "the selected epic's frame must be the cursor white"
    );
    assert!(
        !corners.iter().any(|c| c.fg == purple),
        "no card frame may be purple — an epic's identity stays on its stripe and title"
    );
    for c in &corners {
        assert!(
            c.fg == cursor || c.fg == neutral,
            "with one epic selected and one healthy task, every frame must be the \
             cursor white or the neutral; found {:?}",
            c.fg
        );
    }
}

#[tokio::test]
async fn card_frame_carries_state_and_the_cursor_outranks_it() {
    // core.allium "Card border: cursor and state". Three claims in one board,
    // because they only mean anything together:
    //   - a hard failure borders red,
    //   - an attention state borders amber,
    //   - and the cursor outranks both, so an unhealthy card that is also the
    //     cursor shows white and reports its state on the indicator line instead.

    let mut crashed = make_task(1, TaskStatus::Running);
    crashed.sub_status = SubStatus::Crashed;
    crashed.worktree = Some("/repo/.worktrees/1-task".to_string());
    let mut blocked = make_task(2, TaskStatus::Running);
    blocked.sub_status = SubStatus::NeedsInput;
    blocked.worktree = Some("/repo/.worktrees/2-task".to_string());
    blocked.tmux_window = Some(test_tmux_window("task-2"));
    let mut healthy = make_task(3, TaskStatus::Running);
    healthy.sub_status = SubStatus::Active;
    healthy.worktree = Some("/repo/.worktrees/3-task".to_string());
    healthy.tmux_window = Some(test_tmux_window("task-3"));
    healthy.last_pre_tool_use_at = Some(Utc::now());

    // Cursor on the *crashed* card: the case where the two rules collide.
    let mut app = App::new(vec![crashed, blocked, healthy]);
    app.update(Message::NavigateColumn(1)); // Running
    let buf = render_to_buffer(&mut app, 120, 30);

    let frames: Vec<Color> = cells_with_symbol(&buf, "\u{256d}")
        .iter()
        .map(|c| c.fg)
        .collect();
    assert_eq!(
        frames.len(),
        3,
        "expected three framed cards, got {frames:?}"
    );

    assert!(
        frames.contains(&YELLOW),
        "the blocked card must border amber; frames were {frames:?}"
    );
    assert!(
        frames.contains(&ui::cursor_border_color()),
        "the cursor card must border white; frames were {frames:?}"
    );
    assert!(
        frames.contains(&ui::card_border_color()),
        "the healthy card must border neutral; frames were {frames:?}"
    );
    assert!(
        !frames.contains(&RED),
        "the only crashed card here is the cursor, so the cursor white must win and \
         no red may appear; frames were {frames:?}"
    );

    // Move the cursor off it and the red it was suppressing appears.
    app.update(Message::NavigateRow(1));
    let buf = render_to_buffer(&mut app, 120, 30);
    let frames: Vec<Color> = cells_with_symbol(&buf, "\u{256d}")
        .iter()
        .map(|c| c.fg)
        .collect();
    assert!(
        frames.contains(&RED),
        "with the cursor moved away the crashed card must border red; frames were \
         {frames:?}"
    );
}

#[tokio::test]
async fn only_the_selected_card_has_the_cursor_white_frame() {
    // core.allium "Selection": the cursor's frame is a near-white owned by nothing
    // else on the board, and at most one card carries it. Healthy resting frames
    // are neutral.
    //
    // The card frame carries *state*, not identity — the cursor took a white of its
    // own precisely so it is not competing with the alarm colours. A test asserting
    // the cursor's frame is the *column hue* is asserting the superseded design.
    let mut app = App::new(vec![
        make_task(1, TaskStatus::Backlog),
        make_task(2, TaskStatus::Backlog),
        make_task(3, TaskStatus::Backlog),
    ]);
    let buf = render_to_buffer(&mut app, 120, 30);

    let cursor = ui::cursor_border_color();
    let neutral = ui::card_border_color();
    let hue = ui::column_color(TaskStatus::Backlog);
    let corners = cells_with_symbol(&buf, "\u{256d}"); // ╭
    let white = corners.iter().filter(|c| c.fg == cursor).count();
    let resting = corners.iter().filter(|c| c.fg == neutral).count();

    assert_eq!(
        white, 1,
        "exactly one card frame may carry the cursor white, found {white}"
    );
    assert!(
        resting >= 1,
        "healthy resting card frames must be neutral, found {resting} of {}",
        corners.len()
    );
    assert_eq!(
        white + resting,
        corners.len(),
        "with every task healthy, each frame must be the cursor white or the neutral"
    );
    assert!(
        !corners.iter().any(|c| c.fg == hue),
        "no card frame may carry the column's identity hue — the frame is a state \
         channel now, and identity lives on the stripe and the header label"
    );
}

#[tokio::test]
async fn column_top_rule_is_hued_only_while_focused() {
    // core.allium's named exception under "Focus is intensity, not
    // colour-vs-absence": the column's top rule and its scroll indicators take the
    // identity hue while focused and drop to a flat neutral grey while not. That is
    // the one place on the board where hue signals focus by presence rather than
    // intensity, and it was entirely unguarded — a change that flattened the
    // focused rule to grey, or gave the unfocused one a dimmed hue, passed either
    // way, in both cases silently erasing or contradicting the exception.
    let mut app = App::new(vec![
        make_task(1, TaskStatus::Backlog),
        make_task(2, TaskStatus::Running),
        make_task(3, TaskStatus::Review),
    ]);
    let buf = render_to_buffer(&mut app, 160, 30);

    // Row 0 is the indicator bar, row 1 the summary; the board's first row is the
    // columns' TOP borders.
    // Collect the *distinct* colours: a rule spans dozens of cells, so reporting
    // every one of them buries the answer in a wall of repeats.
    let mut rules: Vec<Color> = Vec::new();
    for x in buf.area.left()..buf.area.right() {
        let cell = &buf[(x, 2)];
        if cell.symbol() == "\u{2500}" && !rules.contains(&cell.fg) {
            rules.push(cell.fg);
        }
    }
    assert!(
        !rules.is_empty(),
        "expected column top rules on the board's first row"
    );

    // Backlog is the focused column on a fresh board.
    let focused_hue = ui::column_color(TaskStatus::Backlog);
    assert!(
        rules.contains(&focused_hue),
        "the focused column's top rule must carry its identity hue {focused_hue:?}; \
         the rules on this row are {rules:?}"
    );
    assert!(
        rules.contains(&MUTED),
        "an unfocused column's top rule must drop to neutral grey; \
         the rules on this row are {rules:?}"
    );
    for c in &rules {
        assert!(
            *c == focused_hue || *c == MUTED,
            "a top rule must be either the focused column's hue {focused_hue:?} or the \
             neutral grey; found {c:?}, so another column's hue has leaked into a rule"
        );
    }
}

#[tokio::test]
async fn no_column_leaks_an_identity_hue_onto_a_card_frame() {
    // The cross-column companion: `only_the_selected_card_has_the_cursor_white_frame`
    // renders one column and so structurally cannot see a colour appearing in
    // another. With every task healthy, the whole board should show exactly one
    // cursor white and neutrals everywhere else — and, crucially, no column's
    // identity hue anywhere, since the frame stopped being an identity channel.
    let mut app = App::new(vec![
        make_task(1, TaskStatus::Backlog),
        make_task(2, TaskStatus::Backlog),
        make_task(3, TaskStatus::Running),
        make_task(4, TaskStatus::Review),
        make_task(5, TaskStatus::Done),
    ]);
    let buf = render_to_buffer(&mut app, 160, 30);

    let neutral = ui::card_border_color();
    let cursor = ui::cursor_border_color();
    let corners = cells_with_symbol(&buf, "\u{256d}");
    assert!(corners.len() >= 5, "expected a card in every column");

    let non_neutral: Vec<Color> = corners
        .iter()
        .map(|c| c.fg)
        .filter(|fg| *fg != neutral)
        .collect();
    assert_eq!(
        non_neutral.len(),
        1,
        "with every task healthy exactly one frame may differ from the neutral; \
         found {non_neutral:?}"
    );
    assert_eq!(
        non_neutral[0], cursor,
        "the one non-neutral frame must be the cursor white"
    );
    for &status in TaskStatus::ALL.iter() {
        let hue = ui::column_color(status);
        assert!(
            !corners.iter().any(|c| c.fg == hue),
            "{status:?}'s identity hue appears on a card frame; the frame carries \
             state, not identity"
        );
    }
}

#[tokio::test]
async fn header_bar_stops_at_the_column_separators() {
    // The header bar must span exactly its own column. It used to be laid out by a
    // *different* constraint set than the board — the summary row divided the width
    // into N equal parts with no separator columns, while the board divided it into
    // N parts plus N-1 one-cell separators — so the two drifted apart and a header
    // fill bled across the separator into its neighbour.
    //
    // Checked at several widths because the drift depends on how the ratio rounding
    // falls, so a single width can happen to line up.
    let header_fills = [
        ui::column_header_bg(TaskStatus::Backlog, false),
        ui::column_header_bg(TaskStatus::Backlog, true),
    ];

    for width in [100u16, 120, 137, 200, 251] {
        let mut app = App::new(vec![
            make_task(1, TaskStatus::Backlog),
            make_task(2, TaskStatus::Running),
            make_task(3, TaskStatus::Review),
            make_task(4, TaskStatus::Done),
        ]);
        let buf = render_to_buffer(&mut app, width, 30);

        // Separator columns run the full height of the board in BORDER. Card rails
        // share the │ glyph but never that colour, and this row sits below the
        // cards, so anything matching here is a separator.
        let probe_y = 18;
        let sep_xs: Vec<u16> = (buf.area.left()..buf.area.right())
            .filter(|&x| {
                let c = &buf[(x, probe_y)];
                c.symbol() == "\u{2502}" && c.fg == BORDER
            })
            .collect();
        assert!(
            !sep_xs.is_empty(),
            "width {width}: expected column separators at y={probe_y}"
        );

        // The summary row sits directly under the top indicator row.
        for x in sep_xs {
            let cell = &buf[(x, 1)];
            assert!(
                !header_fills.contains(&cell.bg),
                "width {width}: a header fill covers the separator at x={x} — the bar \
                 is wider than its column"
            );
        }
    }
}

#[tokio::test]
async fn header_fill_is_uniform_across_columns() {
    // core.allium "Column header bar": the header fill carries no hue and is the
    // same in every column at a given focus state — identity moved to the label.
    // The superseded fill was a per-column darkened wash of the identity colour;
    // that is the regression this guards.
    for is_focused in [false, true] {
        let expected = ui::column_header_bg(TaskStatus::Backlog, is_focused);
        for status in GROUND_COLUMNS {
            assert_eq!(
                ui::column_header_bg(status, is_focused),
                expected,
                "{status:?} (focused={is_focused}) must share the uniform header fill"
            );
        }
    }
}

#[tokio::test]
async fn header_label_is_hued_in_every_column_at_both_focus_states() {
    // Identity rests on the label now, so it must be distinguishable per column at
    // both focus states. Two guards: no two columns may share a label colour, and
    // no label may collapse to a neutral.
    for is_focused in [false, true] {
        let mut seen: Vec<(TaskStatus, Color)> = Vec::new();
        for &status in TaskStatus::ALL.iter() {
            let fg = ui::column_header_fg(status, is_focused);
            assert_ne!(
                fg,
                ui::column_header_bg(status, is_focused),
                "{status:?} (focused={is_focused}) label must not vanish into the fill"
            );
            for (other, prev) in &seen {
                assert_ne!(
                    fg, *prev,
                    "{status:?} and {other:?} must not share a header label colour \
                     (focused={is_focused}) — the label is the only per-column signal left"
                );
            }
            seen.push((status, fg));
        }
    }
}

#[tokio::test]
async fn focused_header_label_is_brighter_than_unfocused() {
    // core.allium "Focus is intensity, not colour-vs-absence": the label keeps its
    // hue at both states and focus moves only its brightness. With the fill now
    // neutral, the label is the only place that intensity can be read.
    for &status in TaskStatus::ALL.iter() {
        let unfocused = lightness_vs_terminal_bg(ui::column_header_fg(status, false));
        let focused = lightness_vs_terminal_bg(ui::column_header_fg(status, true));
        assert!(
            unfocused < focused,
            "{status:?}: focused label ({focused}) must be brighter than unfocused ({unfocused})"
        );
    }
}

#[tokio::test]
async fn unfocused_column_header_keeps_its_identity_colour() {
    // core.allium: "the column's identity colour is always visible; focus
    // modulates emphasis only". The superseded behaviour flattened unfocused
    // headers to MUTED grey — that is the regression this guards.
    for &status in TaskStatus::ALL.iter() {
        let fg = ui::column_header_fg(status, false);
        assert_ne!(
            fg, MUTED,
            "{status:?} unfocused header must not collapse to MUTED grey"
        );
    }
}

#[tokio::test]
async fn focused_column_header_is_more_emphatic_than_unfocused() {
    // core.allium "Column header bar": the bar, not the ground, is where focus
    // is read as colour intensity. The header fill stays hued at both focus
    // states; the focused one is the brighter fill of the two.
    for &status in TaskStatus::ALL.iter() {
        let unfocused = lightness_vs_terminal_bg(ui::column_header_bg(status, false));
        let focused = lightness_vs_terminal_bg(ui::column_header_bg(status, true));
        assert!(
            unfocused < focused,
            "{status:?}: focused header fill ({focused}) must exceed unfocused ({unfocused})"
        );
    }
}

#[tokio::test]
async fn column_header_label_is_uppercased() {
    // core.allium: "It shows the column label, uppercased, followed by the
    // count of selectable items".
    let mut app = make_app();
    let buf = render_to_buffer(&mut app, 120, 40);
    assert!(
        buffer_contains(&buf, "BACKLOG"),
        "column header should render the label uppercased"
    );
}

#[tokio::test]
async fn task_cards_render_a_complete_frame() {
    // core.allium: "Every card draws its own complete frame — rounded top and
    // bottom borders plus left and right rails ... no two cards share a border."
    let mut app = App::new(vec![make_task(1, TaskStatus::Backlog)]);
    let buf = render_to_buffer(&mut app, 120, 40);

    for glyph in ["\u{256d}", "\u{256e}", "\u{2570}", "\u{256f}"] {
        assert!(
            buffer_contains(&buf, glyph),
            "card frame should draw the rounded corner {glyph:?}"
        );
    }
    assert!(
        buffer_contains(&buf, "\u{2502}"),
        "card frame should draw left/right rails"
    );
}

#[tokio::test]
async fn task_card_frame_spans_four_lines_top_to_bottom() {
    // The frame costs one line over the old shared-rule presentation: top
    // border, title, metadata, bottom border (core.allium: "Task card frame"),
    // so the closing corner sits exactly 3 rows below the opening one.
    let mut app = App::new(vec![make_task(1, TaskStatus::Backlog)]);
    let buf = render_to_buffer(&mut app, 120, 40);
    let area = buf.area();

    let row_has =
        |y: u16, glyph: &str| (area.left()..area.right()).any(|x| buf[(x, y)].symbol() == glyph);
    let top = (area.top()..area.bottom())
        .find(|&y| row_has(y, "\u{256d}"))
        .expect("a card top border");
    assert!(
        row_has(top + 3, "\u{2570}"),
        "the card's bottom border should sit 3 rows below its top border"
    );
}

/// The repo-filter overlay used to compute its height as
/// `.clamp(8, area.height - 4)`, which panics on `min > max` the moment the
/// board is shorter than 12 rows. Render code must never panic, so a short
/// board now shrinks the popup instead. Driven through the real render path
/// (not just the layout helper) so the whole overlay is exercised.
#[tokio::test]
async fn repo_filter_overlay_renders_on_a_board_too_short_for_its_minimum_height() {
    use crate::tui::messages::RepoFilterMessage;

    for height in [6_u16, 8, 10, 11, 12] {
        let mut app = App::new(vec![]);
        app.board.repo_paths = vec!["/repos/alpha".to_string(), "/repos/beta".to_string()];
        app.update(Message::RepoFilter(RepoFilterMessage::Start));
        // Panics here, not an assertion failure, are the regression.
        let _buf = render_to_buffer(&mut app, 60, height);
    }
}

/// The centred overlays are laid out from percentages of the board, so a
/// terminal far from the 120x40 the snapshots pin must still draw a complete,
/// on-screen frame. Substitutes for eyeballing the popup at an odd size.
#[tokio::test]
async fn repo_filter_overlay_stays_inside_a_narrow_board() {
    use crate::tui::messages::RepoFilterMessage;

    let mut app = App::new(vec![]);
    app.board.repo_paths = (0..30).map(|i| format!("/repos/r{i}")).collect();
    app.update(Message::RepoFilter(RepoFilterMessage::Start));
    let buf = render_to_buffer(&mut app, 46, 18);

    assert!(
        buffer_contains(&buf, "Repo Filter"),
        "the overlay title should be drawn"
    );
    // Double-line border corners: all four must be present, so the popup is
    // fully on-screen rather than clipped at an edge.
    for glyph in ["\u{2554}", "\u{2557}", "\u{255a}", "\u{255d}"] {
        assert!(
            buffer_contains(&buf, glyph),
            "the overlay border corner {glyph:?} should be on-screen"
        );
    }
}

/// Overlays size themselves as a clamped percentage of the board, and several
/// of those clamps have floors taller than a small terminal (the help overlay
/// floors at 25 rows, todos at 12). `Frame::render_widget` does no clipping and
/// `Clear` writes every cell it is handed, so before `centered_rect`/
/// `open_overlay` clamped, opening any of these on a short board panicked the
/// render thread with "index outside of buffer" rather than drawing something
/// small. One case per overlay, at a board shorter than every floor.
#[tokio::test]
async fn every_overlay_survives_a_board_shorter_than_its_own_minimum() {
    use crate::tui::messages::RepoFilterMessage;

    let short = (100_u16, 8_u16);

    // Help — floors at 25 rows.
    let mut app = App::new(vec![]);
    app.input.mode = crate::tui::InputMode::Help;
    let _ = render_to_buffer(&mut app, short.0, short.1);

    // Todos — floors at 12 rows.
    let mut app = App::new(vec![]);
    app.board.view_mode = crate::tui::ViewMode::Todos {
        todos: vec![],
        selected: 0,
        previous: Box::new(crate::tui::ViewMode::Board(Default::default())),
    };
    let _ = render_to_buffer(&mut app, short.0, short.1);

    // Repo filter — floors at 8 rows.
    let mut app = App::new(vec![]);
    app.board.repo_paths = vec!["/repos/alpha".to_string()];
    app.update(Message::RepoFilter(RepoFilterMessage::Start));
    let _ = render_to_buffer(&mut app, short.0, short.1);

    // Error popup — fixed 7 rows tall, top-pinned once the board is smaller.
    let mut app = App::new(vec![]);
    app.update(Message::System(crate::tui::messages::SystemMessage::Error(
        "boom".to_string(),
    )));
    let _ = render_to_buffer(&mut app, short.0, 4);
}

/// The repo-filter overlay's height budget and its visible-repo window are both
/// derived from `header.len() + footer.len()` — the rows the render body has
/// already built. This drives the real render path in every input mode the
/// overlay supports and asserts the footer's last row is on screen, which is
/// what a budget too small would clip. A layout tallied by hand (the `+7`/`+5`
/// literals this replaced) had nothing pinning it to the rows actually drawn.
///
/// The scrolling cases are the second half of the budget: the `↑ N more` /
/// `↓ N more` markers are content rows too, and the cursor positions below put
/// the window at the top (down-marker only), the middle (both) and the bottom
/// (up-marker only) of a list far longer than the popup.
#[tokio::test]
async fn repo_filter_renders_its_whole_footer_in_every_mode() {
    use crate::tui::messages::RepoFilterMessage;

    // (repo count, board height, repo cursor) — the first fits without
    // scrolling; the rest scroll, exercising each marker combination.
    let scenarios = [(12, 24, 0), (40, 20, 0), (40, 20, 20), (40, 20, 40)];

    for (repo_count, board_height, repo_cursor) in scenarios {
        let repos: Vec<String> = (0..repo_count).map(|i| format!("/repos/r{i}")).collect();

        // Match the overlay's own footer wording, not a bare verb — the board's
        // hint bar sits outside the popup and would satisfy a loose needle even
        // when the overlay's last row was clipped clean off.
        for (mode, expected_footer) in [
            (crate::tui::InputMode::RepoFilter, "[q/Esc] close"),
            (crate::tui::InputMode::InputPresetName, "[Enter] save"),
            (
                crate::tui::InputMode::ConfirmDeletePreset,
                "[A-Z] delete preset",
            ),
            (
                crate::tui::InputMode::ConfirmDeleteRepoPath,
                "n/Esc: cancel",
            ),
        ] {
            let mut app = App::new(vec![]);
            app.board.repo_paths = repos.clone();
            app.update(Message::RepoFilter(RepoFilterMessage::Start));
            app.input.mode = mode.clone();
            app.input.repo_cursor = repo_cursor;

            let case = format!(
                "{mode:?} with {repo_count} repos on a {board_height}-row board, \
                 cursor at {repo_cursor}"
            );
            let buf = render_to_buffer(&mut app, 100, board_height);
            assert!(
                buffer_contains(&buf, expected_footer),
                "footer text {expected_footer:?} was clipped in {case} — the layout \
                 budget disagrees with the rows the overlay renders"
            );
            // The bottom border must also survive: a footer that fits but a border
            // that doesn't means the popup is one row taller than it budgeted.
            assert!(
                buffer_contains(&buf, "\u{255a}"),
                "the overlay's bottom-left corner should be drawn in {case}"
            );
        }
    }
}
