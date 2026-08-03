//! Per-domain `Message` handlers, organised by area of concern.
//!
//! `App::update()` (in `crate::tui`) is the single entry point for all
//! `Message` dispatch. The handler bodies live in this module split by
//! domain (PR flow, epics, repo filters, etc.) so each file stays small
//! enough to navigate quickly.

mod agent;
mod budget;
mod epics;
mod feeds;
mod forms;
mod learnings;
mod lifecycle;
mod main_session;
mod managed_feeds;
mod move_task;
mod navigation;
mod pr;
mod repo_filter;
mod repo_sync;
mod retry;
mod selection;
mod split_pane;
mod system;
mod todos;
