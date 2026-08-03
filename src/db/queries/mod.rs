mod epics;
mod learnings;
mod settings;
pub(super) mod subagents;
mod tasks;
mod todos;
mod usage;

/// Push a conditional `SET col = ?` clause for patch builders.
///
/// Usage: `set_field!(sets, values, opt_value, "col_name")`
// allow-phantom-symbol: opt_value is an illustrative argument name in the usage example
/// If `opt_value` is `Some(v)`, appends the SQL fragment and boxes `v`.
/// Handles both plain `Option<T>` (plain field) and `Option<Option<T>>`
/// (nullable field — the inner `Option` maps to SQL NULL vs value).
#[macro_export]
macro_rules! set_field {
    ($sets:ident, $values:ident, $opt:expr, $col:literal) => {
        if let Some(v) = $opt {
            $sets.push(concat!($col, " = ?"));
            $values.push(Box::new(v));
        }
    };
}

use anyhow::{Context, Result};
use chrono::{DateTime, NaiveDateTime, TimeZone, Utc};

use crate::models::{
    Epic, EpicId, FeedRole, SubStatus, Task, TaskId, TaskStatus, TaskTag, WrapUpMode,
};

/// Build a `FromSqlConversionFailure` error for an unrecognised enum string.
pub(super) fn unknown_enum(field: &'static str, raw: &str) -> rusqlite::Error {
    rusqlite::Error::FromSqlConversionFailure(
        0,
        rusqlite::types::Type::Text,
        format!("unrecognised {field} value: {raw:?}").into(),
    )
}

/// Process-wide count of decode soft-fails: defaulted enum values plus rows
/// skipped by [`collect_decodable`]. See [`decode_fallback_count`].
static DECODE_FALLBACKS: std::sync::atomic::AtomicU64 = std::sync::atomic::AtomicU64::new(0);

/// Bump the decode-fallback counter and return the new total, so callers can
/// include `count=N` in their `tracing::warn!`. Call this from every soft-fail
/// branch — see the soft-fail-decoding section of `docs/conventions.md`.
///
/// Call it on its own line, **not** inline as a `tracing::warn!` field value:
/// the macro skips evaluating its field expressions when no subscriber has the
/// event enabled, so an inline bump would silently stop counting in every
/// process without a subscriber (most one-shot CLI subcommands, and the test
/// suite).
pub(super) fn bump_decode_fallback() -> u64 {
    DECODE_FALLBACKS.fetch_add(1, std::sync::atomic::Ordering::Relaxed) + 1
}

/// Total decode soft-fails since process start. Monotonic and never reset, so
/// tests assert on *deltas* rather than absolute values (the suite shares one
/// process).
pub(super) fn decode_fallback_count() -> u64 {
    DECODE_FALLBACKS.load(std::sync::atomic::Ordering::Relaxed)
}

/// True for errors meaning "this row's *contents* could not be decoded", as
/// opposed to "the query itself failed". Only the former are skippable by
/// [`collect_decodable`]: skipping a `SqliteFailure` (I/O error, interrupt)
/// would silently truncate an otherwise-healthy result set, and skipping an
/// `InvalidColumnName`/`InvalidColumnIndex` would hide a programmer error in
/// the SELECT column list.
fn is_row_decode_error(e: &rusqlite::Error) -> bool {
    matches!(
        e,
        rusqlite::Error::FromSqlConversionFailure(..)
            | rusqlite::Error::InvalidColumnType(..)
            | rusqlite::Error::IntegralValueOutOfRange(..)
            | rusqlite::Error::Utf8Error(..)
    )
}

/// Collect a `query_map` iterator under the **bulk-read** half of the
/// decode-failure policy: a row that fails to decode is skipped with a
/// `tracing::warn!` (and counted via [`bump_decode_fallback`]) so one corrupt
/// row degrades the board instead of blanking it. Errors that are not row
/// decode failures still propagate.
///
/// Single-entity reads (`get_task`, `get_epic`, `find_task_by_plan`) must
/// **not** use this — the caller asked for that specific row, so a decode
/// failure there is reported, not swallowed. See the decode-failure-policy
/// section of `docs/conventions.md`.
pub(super) fn collect_decodable<T>(
    rows: impl Iterator<Item = rusqlite::Result<T>>,
    what: &str,
) -> rusqlite::Result<Vec<T>> {
    let mut out = Vec::new();
    for row in rows {
        match row {
            Ok(value) => out.push(value),
            Err(e) if is_row_decode_error(&e) => {
                let count = bump_decode_fallback();
                tracing::warn!(count, error = %e, "skipping undecodable {what} row");
            }
            Err(e) => return Err(e),
        }
    }
    Ok(out)
}

/// Column list shared by all task SELECT queries. Pair with `row_to_task`.
pub(super) const TASK_COLUMNS: &str =
    "id, title, description, repo_path, status, worktree, tmux_window, \
     plan_path, epic_id, sub_status, url, url_type, tag, sort_order, base_branch, external_id, \
     created_at, updated_at, labels, last_pre_tool_use_at, last_notification_at, \
     wrap_up_mode, auto_run_plan, live_subagents, stop_pending";

/// The `SET` list that applies a `Stop` — the one definition of what "the task
/// finished its turn" writes. Shared by the two statements that can apply it:
/// the immediate flip in `try_record_stop` and the drain in
/// `apply_pending_stop_if_drained`. Binds `?1` = status, `?2` = sub_status; the
/// caller supplies `?3` onward in its own `WHERE`, which is deliberately *not*
/// shared — each predicate is a different concurrency argument and belongs
/// inline where it can be read.
///
/// `migrate_v82_resolve_stranded_pending_stops` deliberately keeps its own copy
/// instead of using this. See the note there.
pub(super) const STOP_FLIP_SET: &str = "SET status = ?1, sub_status = ?2, \
     last_pre_tool_use_at = NULL, last_notification_at = NULL, \
     stop_pending = 0, updated_at = datetime('now')";

/// Column list shared by all epic SELECT queries. Pair with `row_to_epic`.
/// Order must match the field reads in `row_to_epic`.
pub(super) const EPIC_COLUMNS: &str =
    "id, title, description, status, plan_path, sort_order, auto_dispatch, \
     parent_epic_id, feed_command, feed_interval_secs, created_at, updated_at, group_by_repo, \
     feed_role, origin";

/// Reconstruct `Option<TaskUrl>` from the `url` + `url_type` columns. Both null
/// → None; both set → Some. A url present without a type (shouldn't happen)
/// surfaces as a decode error.
fn read_task_url(row: &rusqlite::Row<'_>) -> rusqlite::Result<Option<crate::models::TaskUrl>> {
    let url: Option<String> = row.get("url")?;
    let url_type: Option<crate::models::UrlType> = row.get("url_type")?;
    match (url, url_type) {
        (Some(u), Some(t)) => Ok(Some(crate::models::TaskUrl::new(u, t))),
        (None, None) => Ok(None),
        // A url without a url_type (or vice versa) is a corrupted row the
        // application can never produce, so fail the row rather than silently
        // coercing to None. Single-entity reads report that error; bulk reads
        // skip the row via `collect_decodable`.
        (u, t) => Err(unknown_enum(
            "url/url_type",
            &format!("inconsistent url={u:?} url_type={t:?}"),
        )),
    }
}

pub(super) fn row_to_task(row: &rusqlite::Row<'_>) -> rusqlite::Result<Task> {
    let status_str: String = row.get("status")?;
    let status =
        TaskStatus::parse(&status_str).ok_or_else(|| unknown_enum("task_status", &status_str))?;

    let created_str: String = row.get("created_at")?;
    let updated_str: String = row.get("updated_at")?;

    Ok(Task {
        id: TaskId(row.get("id")?),
        title: row.get("title")?,
        description: row.get("description")?,
        repo_path: row.get("repo_path")?,
        status,
        worktree: row.get("worktree")?,
        tmux_window: row.get("tmux_window")?,
        plan_path: row.get("plan_path")?,
        epic_id: row.get::<_, Option<i64>>("epic_id")?.map(EpicId),
        sub_status: parse_sub_status(&row.get::<_, String>("sub_status")?)?,
        url: read_task_url(row)?,
        tag: parse_tag(row.get("tag")?)?,
        sort_order: row.get("sort_order")?,
        base_branch: row.get("base_branch")?,
        external_id: row.get("external_id")?,
        labels: read_json_string_vec(row, "labels")?,
        created_at: parse_datetime(&created_str)?,
        updated_at: parse_datetime(&updated_str)?,
        last_pre_tool_use_at: read_optional_datetime(row, "last_pre_tool_use_at")?,
        last_notification_at: read_optional_datetime(row, "last_notification_at")?,
        wrap_up_mode: parse_wrap_up_mode(row.get("wrap_up_mode")?)?,
        auto_run_plan: row.get("auto_run_plan")?,
        live_subagents: row.get("live_subagents")?,
        stop_pending: row.get("stop_pending")?,
    })
}

pub(super) fn row_to_epic(row: &rusqlite::Row<'_>) -> rusqlite::Result<Epic> {
    let created_str: String = row.get("created_at")?;
    let updated_str: String = row.get("updated_at")?;
    let status_str: String = row.get("status")?;

    Ok(Epic {
        id: EpicId(row.get("id")?),
        title: row.get("title")?,
        description: row.get("description")?,
        status: TaskStatus::parse(&status_str)
            .ok_or_else(|| unknown_enum("epic_status", &status_str))?,
        plan_path: row.get("plan_path")?,
        sort_order: row.get("sort_order")?,
        auto_dispatch: row.get("auto_dispatch")?,
        parent_epic_id: row.get::<_, Option<i64>>("parent_epic_id")?.map(EpicId),
        feed_command: row.get("feed_command")?,
        feed_interval_secs: row.get("feed_interval_secs")?,
        group_by_repo: row.get::<_, bool>("group_by_repo")?,
        feed_role: parse_feed_role(&row.get::<_, String>("feed_role")?),
        origin: parse_epic_origin(&row.get::<_, String>("origin")?),
        created_at: parse_datetime(&created_str)?,
        updated_at: parse_datetime(&updated_str)?,
    })
}

/// Decode a JSON-encoded `Vec<String>` column. Returns an error for malformed
/// JSON so corrupt cells surface immediately rather than silently becoming empty.
pub(super) fn read_json_string_vec(
    row: &rusqlite::Row<'_>,
    column: &str,
) -> rusqlite::Result<Vec<String>> {
    let raw: Option<String> = row.get::<_, Option<String>>(column)?;
    match raw {
        None => Ok(Vec::new()),
        Some(s) => serde_json::from_str::<Vec<String>>(&s).map_err(|e| {
            rusqlite::Error::FromSqlConversionFailure(
                0,
                rusqlite::types::Type::Text,
                format!("invalid JSON in column {column:?}: {e}").into(),
            )
        }),
    }
}

fn parse_sub_status(raw: &str) -> rusqlite::Result<SubStatus> {
    SubStatus::parse(raw).ok_or_else(|| unknown_enum("sub_status", raw))
}

/// Soft-fail decode of `epics.feed_role`: an unknown role (e.g. a variant
/// written by a newer binary) defaults to `None` rather than poisoning the
/// row. See the soft-fail-decoding section of docs/conventions.md.
fn parse_feed_role(raw: &str) -> FeedRole {
    FeedRole::parse(raw).unwrap_or_else(|| {
        let count = bump_decode_fallback();
        tracing::warn!(
            count,
            value = %raw,
            "unknown epics.feed_role value; defaulting to none"
        );
        FeedRole::None
    })
}

/// Soft-fail decode of `epics.origin`: an unknown origin (e.g. a variant
/// written by a newer binary) defaults to `Manual` rather than poisoning the
/// row. See the soft-fail-decoding section of docs/conventions.md.
fn parse_epic_origin(raw: &str) -> crate::models::EpicOrigin {
    crate::models::EpicOrigin::parse(raw).unwrap_or_else(|| {
        let count = bump_decode_fallback();
        tracing::warn!(
            count,
            value = %raw,
            "unknown epics.origin value; defaulting to manual"
        );
        crate::models::EpicOrigin::Manual
    })
}

fn parse_wrap_up_mode(raw: Option<String>) -> rusqlite::Result<Option<WrapUpMode>> {
    match raw {
        None => Ok(None),
        Some(s) => WrapUpMode::parse(&s)
            .map(Some)
            .ok_or_else(|| unknown_enum("wrap_up_mode", &s)),
    }
}

fn parse_tag(raw: Option<String>) -> rusqlite::Result<Option<TaskTag>> {
    match raw {
        None => Ok(None),
        Some(s) => TaskTag::parse(&s)
            .map(Some)
            .ok_or_else(|| unknown_enum("task_tag", &s)),
    }
}

/// Serialize a `Vec<String>` for storage in a JSON-encoded column.
pub(super) fn write_json_string_vec(values: &[String]) -> Result<String> {
    serde_json::to_string(values).context("Failed to serialize string list to JSON")
}

/// Parse SQLite `datetime('now')` output: "YYYY-MM-DD HH:MM:SS"
pub(super) fn parse_datetime(s: &str) -> rusqlite::Result<DateTime<Utc>> {
    NaiveDateTime::parse_from_str(s, "%Y-%m-%d %H:%M:%S")
        .map(|ndt| Utc.from_utc_datetime(&ndt))
        .map_err(|e| {
            rusqlite::Error::FromSqlConversionFailure(
                0,
                rusqlite::types::Type::Text,
                format!("invalid datetime {s:?}: {e}").into(),
            )
        })
}

/// Format a `DateTime<Utc>` for storage in TEXT timestamp columns.
/// Pairs with [`parse_datetime`] — both use "YYYY-MM-DD HH:MM:SS".
pub(super) fn format_datetime(dt: DateTime<Utc>) -> String {
    dt.format("%Y-%m-%d %H:%M:%S").to_string()
}

/// Read a nullable TEXT timestamp column.
pub(super) fn read_optional_datetime(
    row: &rusqlite::Row<'_>,
    col: &str,
) -> rusqlite::Result<Option<DateTime<Utc>>> {
    let s: Option<String> = row.get::<_, Option<String>>(col)?;
    match s {
        None => Ok(None),
        Some(s) => parse_datetime(&s).map(Some),
    }
}

pub(super) fn get_tips_state(
    conn: &rusqlite::Connection,
) -> Result<(u32, crate::models::TipsShowMode)> {
    use crate::models::TipsShowMode;
    let result = conn.query_row(
        "SELECT seen_up_to, show_mode FROM tips_state WHERE id = 1",
        [],
        |row| {
            let seen_up_to: u32 = row.get(0)?;
            let show_mode_str: String = row.get(1)?;
            Ok((seen_up_to, show_mode_str))
        },
    );

    match result {
        Ok((seen_up_to, show_mode_str)) => {
            let show_mode = show_mode_str.parse::<TipsShowMode>().map_err(|e| {
                anyhow::anyhow!("unrecognised tips show_mode {:?}: {}", show_mode_str, e)
            })?;
            Ok((seen_up_to, show_mode))
        }
        Err(rusqlite::Error::QueryReturnedNoRows) => Ok((0, TipsShowMode::Always)),
        Err(e) => Err(e).context("Failed to read tips_state"),
    }
}

pub(super) fn save_tips_state(
    conn: &rusqlite::Connection,
    seen_up_to: u32,
    show_mode: crate::models::TipsShowMode,
) -> Result<()> {
    let rows = conn
        .execute(
            "UPDATE tips_state SET seen_up_to = ?1, show_mode = ?2 WHERE id = 1",
            rusqlite::params![seen_up_to, show_mode.as_str()],
        )
        .context("Failed to save tips_state")?;
    if rows != 1 {
        anyhow::bail!("save_tips_state: expected 1 row updated, got {rows}");
    }
    Ok(())
}

#[cfg(test)]
#[allow(clippy::unwrap_used, clippy::expect_used)]
mod tests {
    use super::*;

    fn sqlite_failure() -> rusqlite::Error {
        rusqlite::Error::SqliteFailure(
            rusqlite::ffi::Error::new(rusqlite::ffi::SQLITE_IOERR),
            Some("disk I/O error".to_string()),
        )
    }

    #[test]
    fn collect_decodable_skips_decode_failures() {
        let rows = vec![Ok(1), Err(unknown_enum("task_status", "bogus")), Ok(3)];
        let before = decode_fallback_count();
        let collected = collect_decodable(rows.into_iter(), "tasks").unwrap();
        assert_eq!(collected, vec![1, 3]);
        // `>=`, not `==`: the counter is process-wide and tests run in
        // parallel, so a concurrent test's skip can land between the two reads.
        assert!(
            decode_fallback_count() > before,
            "a skipped row must bump the decode-fallback counter"
        );
    }

    #[test]
    fn collect_decodable_propagates_non_decode_errors() {
        // A failing `step()` (I/O error, interrupt) must not be mistaken for a
        // corrupt row — skipping it would silently truncate the result set.
        let rows: Vec<rusqlite::Result<i32>> = vec![Ok(1), Err(sqlite_failure())];
        let err = collect_decodable(rows.into_iter(), "tasks")
            .expect_err("a SqliteFailure must propagate, not be skipped");
        assert!(
            matches!(err, rusqlite::Error::SqliteFailure(..)),
            "got {err:?}"
        );
    }

    #[test]
    fn collect_decodable_propagates_column_mistakes() {
        // An `InvalidColumnName` means the SELECT list and the decoder
        // disagree — a programmer error, not row corruption.
        let rows: Vec<rusqlite::Result<i32>> =
            vec![Err(rusqlite::Error::InvalidColumnName("nope".to_string()))];
        assert!(collect_decodable(rows.into_iter(), "tasks").is_err());
    }
}
