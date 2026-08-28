#![allow(clippy::unwrap_used, clippy::expect_used)]
use super::super::shared::render_substatus_header;
use super::*;
use crate::models::TaskTag;
use crate::tui::types::TaskDraft;
use ratatui::buffer::Buffer;
use ratatui::widgets::ListItem;

fn make_test_app() -> App {
    App::new(vec![])
}

fn dummy_style() -> Style {
    Style::default()
}

fn dummy_styles() -> FormStyles {
    FormStyles {
        completed: dummy_style(),
        active: dummy_style(),
        hint: dummy_style(),
    }
}

#[test]
fn input_description_shows_tag_when_set() {
    let mut app = make_test_app();
    app.input.task_draft = Some(TaskDraft {
        title: "My task".into(),
        tag: Some(TaskTag::Bug),
        ..Default::default()
    });
    app.input.set_buffer("some desc".into());
    let lines = input_description_lines(&app, &dummy_styles());
    let text: String = lines
        .iter()
        .map(|l| l.to_string())
        .collect::<Vec<_>>()
        .join("\n");
    assert!(text.contains("Tag: bug"), "expected tag line, got:\n{text}");
    assert!(text.contains("Title: My task"));
    assert!(text.contains("Description: opening $EDITOR"));
}

#[test]
fn input_description_shows_none_when_no_tag() {
    let mut app = make_test_app();
    app.input.task_draft = Some(TaskDraft {
        title: "No tag task".into(),
        tag: None,
        ..Default::default()
    });
    let lines = input_description_lines(&app, &dummy_styles());
    let text: String = lines
        .iter()
        .map(|l| l.to_string())
        .collect::<Vec<_>>()
        .join("\n");
    assert!(
        text.contains("Tag: none"),
        "expected 'Tag: none', got:\n{text}"
    );
}

#[test]
fn input_repo_path_shows_tag_when_set() {
    let mut app = make_test_app();
    app.input.task_draft = Some(TaskDraft {
        title: "Feature task".into(),
        description: "A description".into(),
        tag: Some(TaskTag::Feature),
        ..Default::default()
    });
    app.input.set_buffer("/some/path".into());
    let area = Rect::new(0, 0, 80, 24);
    let lines = input_repo_path_lines(&app, area, &dummy_styles());
    let text: String = lines
        .iter()
        .map(|l| l.to_string())
        .collect::<Vec<_>>()
        .join("\n");
    assert!(
        text.contains("Tag: feature"),
        "expected tag line, got:\n{text}"
    );
    assert!(text.contains("Title: Feature task"));
    assert!(text.contains("Description: A description"));
    assert!(text.contains("Repo path: /some/path"));
}

#[test]
fn input_repo_path_shows_none_when_no_tag() {
    let mut app = make_test_app();
    app.input.task_draft = Some(TaskDraft {
        title: "Plain task".into(),
        description: "desc".into(),
        tag: None,
        ..Default::default()
    });
    app.input.clear_buffer();
    let area = Rect::new(0, 0, 80, 24);
    let lines = input_repo_path_lines(&app, area, &dummy_styles());
    let text: String = lines
        .iter()
        .map(|l| l.to_string())
        .collect::<Vec<_>>()
        .join("\n");
    assert!(
        text.contains("Tag: none"),
        "expected 'Tag: none', got:\n{text}"
    );
}

// ---------------------------------------------------------------------------
// InputPhoenix — creation-form panel (issue 4466: the step must render in
// this panel like every other creation-form step, not only in the status bar)
// ---------------------------------------------------------------------------

#[test]
fn input_panel_height_reserves_room_for_phoenix_step() {
    let mut app = make_test_app();
    app.input.mode = InputMode::InputPhoenix;
    assert!(
        input_panel_height(&app, 24) > 0,
        "InputPhoenix must reserve panel rows like the other creation-form steps"
    );
}

#[test]
fn render_input_form_draws_phoenix_step_in_the_panel() {
    use ratatui::{backend::TestBackend, Terminal};

    let mut app = make_test_app();
    app.input.mode = InputMode::InputPhoenix;
    app.input.task_draft = Some(TaskDraft {
        title: "My task".into(),
        tag: Some(TaskTag::Bug),
        repo_path: "/some/repo".into(),
        base_branch: "main".into(),
        wrap_up_mode: Some(crate::models::WrapUpMode::Rebase),
        ..Default::default()
    });

    let backend = TestBackend::new(60, 12);
    let mut terminal = Terminal::new(backend).unwrap();
    let mut drew_form = false;
    terminal
        .draw(|f| {
            let area = Rect::new(0, 0, 60, 12);
            drew_form = render_input_form(f, &app, area);
        })
        .unwrap();

    assert!(drew_form, "InputPhoenix must render inside the form panel");

    let buf = terminal.backend().buffer().clone();
    let text: String = (0..buf.area().height)
        .map(|y| buf_row(&buf, y))
        .collect::<Vec<_>>()
        .join("\n");
    assert!(text.contains("New Task"), "got:\n{text}");
    assert!(text.contains("My task"), "got:\n{text}");
    assert!(text.contains("Phoenix"), "got:\n{text}");
}

fn render_list_item_to_buf(item: ListItem<'static>, width: u16, height: u16) -> Buffer {
    use ratatui::{backend::TestBackend, widgets::List, Terminal};
    let backend = TestBackend::new(width, height);
    let mut terminal = Terminal::new(backend).unwrap();
    terminal
        .draw(|f| {
            let list = List::new(vec![item]);
            f.render_widget(list, f.area());
        })
        .unwrap();
    terminal.backend().buffer().clone()
}

fn buf_row(buf: &Buffer, y: u16) -> String {
    let area = buf.area();
    (area.left()..area.right())
        .map(|x| buf[(x, y)].symbol().to_owned())
        .collect()
}

// ---------------------------------------------------------------------------
// render_substatus_header
// ---------------------------------------------------------------------------

#[test]
fn substatus_header_has_two_lines() {
    let item = render_substatus_header("my-repo", false);
    let buf = render_list_item_to_buf(item, 40, 2);
    // Confirm both rows are allocated (height 2 means 2 rows rendered)
    assert_eq!(buf.area().height, 2);
}

#[test]
fn substatus_header_first_line_is_blank() {
    let item = render_substatus_header("my-repo", false);
    let buf = render_list_item_to_buf(item, 40, 2);
    let row0 = buf_row(&buf, 0);
    assert!(
        row0.trim().is_empty(),
        "first line should be blank spacer, got: {row0:?}"
    );
}

#[test]
fn substatus_header_second_line_contains_label() {
    let item = render_substatus_header("my-repo", false);
    let buf = render_list_item_to_buf(item, 40, 2);
    let row1 = buf_row(&buf, 1);
    assert!(
        row1.contains("my-repo"),
        "second line should contain label, got: {row1:?}"
    );
}

#[test]
fn substatus_header_second_line_is_bold_and_bright() {
    let item = render_substatus_header("my-repo", false);
    let buf = render_list_item_to_buf(item, 40, 2);
    let area = buf.area();
    let first_content_x = (area.left()..area.right())
        .find(|&x| !buf[(x, 1)].symbol().trim().is_empty())
        .expect("row 1 should have content");
    let style = buf[(first_content_x, 1)].style();
    assert!(
        style.add_modifier.contains(Modifier::BOLD),
        "header text should be BOLD"
    );
    assert_eq!(style.fg, Some(FG), "header text should use FG color");
}

#[test]
fn first_substatus_header_has_no_blank_line() {
    let item = render_substatus_header("awaiting review", true);
    assert_eq!(
        item.height(),
        1,
        "first header should have 1 line (no blank)"
    );
}

#[test]
fn subsequent_substatus_header_has_blank_line() {
    let item = render_substatus_header("in review", false);
    assert_eq!(
        item.height(),
        2,
        "subsequent header should have 2 lines (blank + label)"
    );
}

#[test]
fn wrapped_line_count_empty_string_returns_zero() {
    assert_eq!(wrapped_line_count("", 80), 0);
}

#[test]
fn wrapped_line_count_width_zero_returns_zero() {
    assert_eq!(wrapped_line_count("hello", 0), 0);
}

#[test]
fn wrapped_line_count_single_line_shorter_than_width() {
    assert_eq!(wrapped_line_count("hello", 80), 1);
}

#[test]
fn wrapped_line_count_single_line_exactly_width() {
    assert_eq!(wrapped_line_count("hello", 5), 1);
}

#[test]
fn wrapped_line_count_single_line_longer_than_width_wraps() {
    // 10 chars, width 5 -> ceil(10/5) = 2 lines
    assert_eq!(wrapped_line_count("helloworld", 5), 2);
}

#[test]
fn wrapped_line_count_single_newline_counts_as_one() {
    assert_eq!(wrapped_line_count("\n", 80), 1);
}

#[test]
fn wrapped_line_count_multiline_text() {
    // "hello\nworld" -> 2 lines each 5 chars, width 80 -> 2 lines total
    assert_eq!(wrapped_line_count("hello\nworld", 80), 2);
}

#[test]
fn wrapped_line_count_multiline_with_wrapping() {
    // "aaaaaaaaaa\nbb" -> ceil(10/5)=2 + ceil(2/5)=1 = 3
    assert_eq!(wrapped_line_count("aaaaaaaaaa\nbb", 5), 3);
}
