use anyhow::{Context, Result};
use rusqlite::params;

use crate::models::{UsageEvent, UsageSummary};

use super::super::{Database, UsageCap, UsageQuery};
use super::{format_datetime, parse_datetime};

#[async_trait::async_trait]
impl crate::db::UsageStore for Database {
    async fn record_usage_event_with_cap(&self, event: &UsageEvent, cap: UsageCap) -> Result<()> {
        let category: &'static str = event.category.as_str();
        let action = event.action.clone();
        let detail = event.detail.clone();
        let actor: &'static str = event.actor.as_str();
        self.db_call(move |conn| {
            conn.execute(
                "INSERT INTO usage_events (category, action, detail, actor)
                 VALUES (?1, ?2, ?3, ?4)",
                params![category, action, detail, actor],
            )
            .context("Failed to insert usage_event")?;

            // Cap enforcement: MAX(id) on the autoincrement PK is O(1) (last
            // B-tree page), unlike OFFSET which would walk `cap` index entries
            // on every insert. Below cap, MAX(id) - cap is non-positive and
            // the DELETE is a no-op.
            conn.execute(
                "DELETE FROM usage_events
                 WHERE id <= (SELECT MAX(id) FROM usage_events) - ?1",
                params![cap.value() as i64],
            )
            .context("Failed to enforce usage_events cap")?;

            Ok(())
        })
        .await
    }

    async fn query_usage(&self, q: &UsageQuery) -> Result<Vec<UsageSummary>> {
        let category = q.category.clone();
        let actor = q.actor.clone();
        let since = q.since;
        let limit = q.limit.unwrap_or(50).clamp(1, 500) as i64;

        self.db_call_read(move |conn| {
            let mut conditions: Vec<String> = Vec::new();
            let mut bind: Vec<Box<dyn rusqlite::ToSql>> = Vec::new();

            if let Some(cat) = category {
                conditions.push(format!("category = ?{}", bind.len() + 1));
                bind.push(Box::new(cat));
            }
            if let Some(act) = actor {
                conditions.push(format!("actor = ?{}", bind.len() + 1));
                bind.push(Box::new(act));
            }
            if let Some(since_dt) = since {
                conditions.push(format!("recorded_at >= ?{}", bind.len() + 1));
                bind.push(Box::new(format_datetime(since_dt)));
            }

            let where_clause = if conditions.is_empty() {
                String::new()
            } else {
                format!("WHERE {}", conditions.join(" AND "))
            };

            bind.push(Box::new(limit));
            let sql = format!(
                "SELECT category, action, detail, actor,
                        COUNT(*) AS count,
                        MAX(recorded_at) AS last_used
                 FROM usage_events
                 {where_clause}
                 GROUP BY category, action, detail, actor
                 ORDER BY count ASC
                 LIMIT ?{}",
                bind.len()
            );

            let mut stmt = conn
                .prepare(&sql)
                .context("Failed to prepare query_usage")?;

            let params_refs: Vec<&dyn rusqlite::ToSql> = bind.iter().map(|b| b.as_ref()).collect();

            let rows = stmt
                .query_map(params_refs.as_slice(), |row| {
                    let category: String = row.get(0)?;
                    let action: String = row.get(1)?;
                    let detail: Option<String> = row.get(2)?;
                    let actor: String = row.get(3)?;
                    let count: i64 = row.get(4)?;
                    let last_used_str: String = row.get(5)?;
                    let last_used = parse_datetime(&last_used_str)?;
                    Ok(UsageSummary {
                        category,
                        action,
                        detail,
                        actor,
                        count,
                        last_used,
                    })
                })
                .context("Failed to execute query_usage")?
                .collect::<rusqlite::Result<Vec<_>>>()
                .context("Failed to collect usage_summary rows")?;

            Ok(rows)
        })
        .await
    }
}
