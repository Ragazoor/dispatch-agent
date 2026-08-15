//! Tests for targeted single-entity refresh: `TaskMessage::Updated` and
//! `EpicMessage::Updated`. These splice one row into the in-memory list
//! instead of rebuilding the whole vector.

#![allow(clippy::unwrap_used, clippy::expect_used)]

use super::helpers::{make_app, make_epic, make_task};
use crate::models::{EpicId, SubStatus, TaskId, TaskStatus};
use crate::tui::Message;

#[test]
fn task_updated_replaces_existing_row() {
    let mut app = make_app();
    let original = app
        .board
        .tasks
        .iter()
        .find(|t| t.id == TaskId(1))
        .cloned()
        .expect("task 1 in fixture");
    assert_eq!(original.status, TaskStatus::Backlog);

    let mut updated = original.clone();
    updated.status = TaskStatus::Running;
    updated.sub_status = SubStatus::default_for(TaskStatus::Running);
    let other_count_before = app.board.tasks.len();

    app.update(Message::Task(crate::tui::messages::TaskMessage::Updated(
        updated,
    )));

    let now = app
        .board
        .tasks
        .iter()
        .find(|t| t.id == TaskId(1))
        .expect("task 1 still present");
    assert_eq!(now.status, TaskStatus::Running);
    assert_eq!(
        app.board.tasks.len(),
        other_count_before,
        "TaskUpdated must not append when the id matches"
    );
}

#[test]
fn task_updated_appends_new_task() {
    let mut app = make_app();
    let before = app.board.tasks.len();
    let new_task = make_task(99, TaskStatus::Backlog);

    app.update(Message::Task(crate::tui::messages::TaskMessage::Updated(
        new_task,
    )));

    assert_eq!(app.board.tasks.len(), before + 1);
    assert!(app.board.tasks.iter().any(|t| t.id == TaskId(99)));
}

#[test]
fn epic_updated_replaces_existing_row() {
    let mut app = make_app();
    let epic = make_epic(7);
    app.board.epics.push(epic.clone());

    let mut renamed = epic.clone();
    renamed.title = "renamed".to_string();
    let count_before = app.board.epics.len();

    app.update(Message::Epic(crate::tui::messages::EpicMessage::Updated(
        renamed,
    )));

    let now = app
        .board
        .epics
        .iter()
        .find(|e| e.id == EpicId(7))
        .expect("epic 7 still present");
    assert_eq!(now.title, "renamed");
    assert_eq!(app.board.epics.len(), count_before);
}

#[test]
fn epic_updated_appends_new_epic() {
    let mut app = make_app();
    let before = app.board.epics.len();

    app.update(Message::Epic(crate::tui::messages::EpicMessage::Updated(
        make_epic(42),
    )));

    assert_eq!(app.board.epics.len(), before + 1);
    assert!(app.board.epics.iter().any(|e| e.id == EpicId(42)));
}

#[test]
fn task_updated_fires_review_notification_on_transition() {
    let mut app = make_app();
    app.set_notifications_enabled(true);
    let mut updated = app
        .board
        .tasks
        .iter()
        .find(|t| t.id == TaskId(1))
        .cloned()
        .expect("task 1 fixture");
    updated.status = TaskStatus::Review;
    updated.sub_status = SubStatus::default_for(TaskStatus::Review);

    let cmds = app.update(Message::Task(crate::tui::messages::TaskMessage::Updated(
        updated,
    )));

    assert!(
        cmds.iter().any(|c| matches!(
            c,
            crate::tui::Command::System(
                crate::tui::commands::SystemCommand::SendNotification { .. }
            )
        )),
        "expected a SendNotification command on review transition"
    );
}

#[test]
fn task_updated_flashes_sent_on_new_peer_message_sent_timestamp() {
    let mut app = make_app();
    let mut updated = app
        .board
        .tasks
        .iter()
        .find(|t| t.id == TaskId(1))
        .cloned()
        .expect("task 1 fixture");
    assert!(!app.agents.message_flash_sent.contains_key(&TaskId(1)));

    updated.last_peer_message_sent_at = Some(chrono::Utc::now());
    app.update(Message::Task(crate::tui::messages::TaskMessage::Updated(
        updated,
    )));

    assert!(
        app.agents.message_flash_sent.contains_key(&TaskId(1)),
        "a freshly-stamped last_peer_message_sent_at must flash the sent indicator"
    );
    assert!(
        !app.agents.message_flash.contains_key(&TaskId(1)),
        "a sent stamp must not also flash the received indicator"
    );
}

#[test]
fn task_updated_flashes_received_on_new_peer_message_received_timestamp() {
    let mut app = make_app();
    let mut updated = app
        .board
        .tasks
        .iter()
        .find(|t| t.id == TaskId(1))
        .cloned()
        .expect("task 1 fixture");
    assert!(!app.agents.message_flash.contains_key(&TaskId(1)));

    updated.last_peer_message_received_at = Some(chrono::Utc::now());
    app.update(Message::Task(crate::tui::messages::TaskMessage::Updated(
        updated,
    )));

    assert!(
        app.agents.message_flash.contains_key(&TaskId(1)),
        "a freshly-stamped last_peer_message_received_at must flash the received indicator"
    );
    assert!(!app.agents.message_flash_sent.contains_key(&TaskId(1)));
}

#[test]
fn task_updated_does_not_reflash_an_unchanged_peer_message_timestamp() {
    let mut app = make_app();
    let stamp = chrono::Utc::now();
    let mut updated = app
        .board
        .tasks
        .iter()
        .find(|t| t.id == TaskId(1))
        .cloned()
        .expect("task 1 fixture");
    updated.last_peer_message_sent_at = Some(stamp);
    app.update(Message::Task(crate::tui::messages::TaskMessage::Updated(
        updated.clone(),
    )));
    assert!(app.agents.message_flash_sent.contains_key(&TaskId(1)));

    // Manually clear the tracked flash to prove the second update, carrying
    // the *same* timestamp, does not re-insert it.
    app.agents.message_flash_sent.remove(&TaskId(1));
    app.update(Message::Task(crate::tui::messages::TaskMessage::Updated(
        updated,
    )));

    assert!(
        !app.agents.message_flash_sent.contains_key(&TaskId(1)),
        "an unchanged last_peer_message_sent_at must not re-trigger the flash"
    );
}
