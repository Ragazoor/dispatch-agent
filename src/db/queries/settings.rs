use anyhow::{Context, Result};
use rusqlite::{params, OptionalExtension};

use super::super::{Database, SettingsStore};

#[async_trait::async_trait]
impl super::super::SettingsStore for Database {
    async fn list_repo_paths(&self) -> Result<Vec<String>> {
        self.db_call_read(move |conn| {
            let mut stmt = conn
                .prepare("SELECT path FROM repo_paths ORDER BY last_used DESC")
                .context("Failed to prepare list_repo_paths")?;
            let paths = stmt
                .query_map([], |row| row.get(0))
                .context("Failed to query repo_paths")?
                .collect::<rusqlite::Result<Vec<String>>>()
                .context("Failed to collect repo_paths")?;
            Ok(paths)
        })
        .await
    }

    async fn save_repo_path(&self, path: &str) -> Result<()> {
        let path = path.to_string();
        self.db_call(move |conn| {
            conn.execute(
                "INSERT INTO repo_paths (path) VALUES (?1)
                 ON CONFLICT(path) DO UPDATE SET last_used = datetime('now')",
                params![path],
            )
            .context("Failed to save repo_path")?;
            Ok(())
        })
        .await
    }

    async fn delete_repo_path(&self, path: &str) -> Result<()> {
        let path = path.to_string();
        self.db_call(move |conn| {
            conn.execute("DELETE FROM repo_paths WHERE path = ?1", params![path])
                .context("Failed to delete repo_path")?;
            // Clean up filter presets that reference this path
            let presets: Vec<(String, String)> = {
                let mut stmt = conn
                    .prepare("SELECT name, repo_paths FROM filter_presets")
                    .context("Failed to prepare preset query")?;
                let rows = stmt
                    .query_map([], |row| {
                        Ok((row.get::<_, String>(0)?, row.get::<_, String>(1)?))
                    })?
                    .collect::<rusqlite::Result<Vec<_>>>()
                    .context("Failed to list presets for cleanup")?;
                rows
            };
            for (name, json) in presets {
                let paths: Vec<String> = serde_json::from_str(&json)
                    .with_context(|| format!("corrupt filter_preset JSON for preset {name:?}"))?;
                let filtered: Vec<String> = paths.into_iter().filter(|p| p != &path).collect();
                if filtered.is_empty() {
                    conn.execute("DELETE FROM filter_presets WHERE name = ?1", params![name])?;
                } else {
                    let updated = serde_json::to_string(&filtered)
                        .context("Failed to serialize filtered repo_paths")?;
                    conn.execute(
                        "UPDATE filter_presets SET repo_paths = ?1 WHERE name = ?2",
                        params![updated, name],
                    )?;
                }
            }
            Ok(())
        })
        .await
    }

    async fn get_setting_bool(&self, key: &str) -> Result<Option<bool>> {
        let key = key.to_string();
        self.db_call_read(move |conn| {
            conn.query_row(
                "SELECT value FROM settings WHERE key = ?1",
                params![key],
                |row| {
                    let v: String = row.get(0)?;
                    Ok(v == "1")
                },
            )
            .optional()
            .context("Failed to get setting")
        })
        .await
    }

    async fn set_setting_bool(&self, key: &str, value: bool) -> Result<()> {
        let key = key.to_string();
        self.db_call(move |conn| {
            conn.execute(
                "INSERT INTO settings (key, value) VALUES (?1, ?2)
                 ON CONFLICT(key) DO UPDATE SET value = ?2",
                params![key, if value { "1" } else { "0" }],
            )?;
            Ok(())
        })
        .await
    }

    async fn get_setting_string(&self, key: &str) -> Result<Option<String>> {
        let key = key.to_string();
        self.db_call_read(move |conn| {
            conn.query_row(
                "SELECT value FROM settings WHERE key = ?1",
                params![key],
                |row| row.get(0),
            )
            .optional()
            .context("Failed to get setting")
        })
        .await
    }

    async fn set_setting_string(&self, key: &str, value: &str) -> Result<()> {
        let key = key.to_string();
        let value = value.to_string();
        self.db_call(move |conn| {
            conn.execute(
                "INSERT INTO settings (key, value) VALUES (?1, ?2)
                 ON CONFLICT(key) DO UPDATE SET value = ?2",
                params![key, value],
            )?;
            Ok(())
        })
        .await
    }

    async fn save_filter_preset(
        &self,
        name: &str,
        repo_paths: &[String],
        mode: &str,
    ) -> Result<()> {
        let name = name.to_string();
        let mode = mode.to_string();
        let json = serde_json::to_string(repo_paths).context("Failed to serialize repo_paths")?;
        self.db_call(move |conn| {
            conn.execute(
                "INSERT INTO filter_presets (name, repo_paths, mode) VALUES (?1, ?2, ?3)
                 ON CONFLICT(name) DO UPDATE SET repo_paths = ?2, mode = ?3",
                params![name, json, mode],
            )?;
            Ok(())
        })
        .await
    }

    async fn delete_filter_preset(&self, name: &str) -> Result<()> {
        let name = name.to_string();
        self.db_call(move |conn| {
            conn.execute("DELETE FROM filter_presets WHERE name = ?1", params![name])?;
            Ok(())
        })
        .await
    }

    async fn list_filter_presets(&self) -> Result<Vec<(String, Vec<String>, String)>> {
        self.db_call_read(move |conn| {
            let mut stmt =
                conn.prepare("SELECT name, repo_paths, mode FROM filter_presets ORDER BY name")?;
            let rows = stmt.query_map([], |row| {
                Ok((
                    row.get::<_, String>(0)?,
                    row.get::<_, String>(1)?,
                    row.get::<_, String>(2)?,
                ))
            })?;
            let raw: Vec<(String, String, String)> = rows
                .collect::<Result<Vec<_>, _>>()
                .context("Failed to list filter presets")?;
            raw.into_iter()
                .map(|(name, json, mode)| {
                    let paths: Vec<String> = serde_json::from_str(&json).with_context(|| {
                        format!("corrupt filter_preset JSON for preset {name:?}")
                    })?;
                    Ok((name, paths, mode))
                })
                .collect()
        })
        .await
    }

    async fn get_verify_command(&self, path: &str) -> Result<Option<String>> {
        let path = path.to_string();
        self.db_call_read(move |conn| {
            let result: Option<Option<String>> = conn
                .query_row(
                    "SELECT verify_command FROM repo_paths WHERE path = ?1",
                    params![path],
                    |row| row.get::<_, Option<String>>(0),
                )
                .optional()
                .context("Failed to get verify_command")?;
            Ok(result.flatten())
        })
        .await
    }

    async fn set_verify_command(&self, path: &str, command: Option<&str>) -> Result<()> {
        let path = path.to_string();
        let resolved: Option<String> = match command {
            Some(raw) => {
                if raw.contains('\n') || raw.contains('\r') {
                    anyhow::bail!(
                        "verify_command must not contain a newline or carriage return (use && or ; to chain steps)"
                    );
                }
                let trimmed = raw.trim();
                if trimmed.is_empty() {
                    None
                } else {
                    Some(trimmed.to_string())
                }
            }
            None => None,
        };
        self.db_call(move |conn| {
            match resolved {
                Some(cmd) => {
                    conn.execute(
                        "INSERT INTO repo_paths(path, verify_command) VALUES(?1, ?2)
                         ON CONFLICT(path) DO UPDATE SET verify_command = excluded.verify_command",
                        params![path, cmd],
                    )
                    .context("Failed to upsert verify_command")?;
                }
                None => {
                    conn.execute(
                        "UPDATE repo_paths SET verify_command = NULL WHERE path = ?1",
                        params![path],
                    )
                    .context("Failed to clear verify_command")?;
                }
            }
            Ok(())
        })
        .await
    }

    // -- Managed-feed config (WP5) --

    async fn get_reviews_feed_command(&self) -> Result<Option<String>> {
        self.get_setting_string(REVIEWS_FEED_COMMAND_KEY).await
    }

    async fn set_reviews_feed_command(&self, value: Option<&str>) -> Result<()> {
        self.set_managed_feed_setting(REVIEWS_FEED_COMMAND_KEY, value)
            .await
    }

    async fn get_reviews_feed_interval_secs(&self) -> Result<Option<i64>> {
        parse_setting_i64(
            REVIEWS_FEED_INTERVAL_SECS_KEY,
            self.get_setting_string(REVIEWS_FEED_INTERVAL_SECS_KEY)
                .await?,
        )
    }

    async fn set_reviews_feed_interval_secs(&self, value: Option<i64>) -> Result<()> {
        self.set_managed_feed_setting(
            REVIEWS_FEED_INTERVAL_SECS_KEY,
            value.map(|n| n.to_string()).as_deref(),
        )
        .await
    }

    async fn get_cve_feed_command(&self) -> Result<Option<String>> {
        self.get_setting_string(CVE_FEED_COMMAND_KEY).await
    }

    async fn set_cve_feed_command(&self, value: Option<&str>) -> Result<()> {
        self.set_managed_feed_setting(CVE_FEED_COMMAND_KEY, value)
            .await
    }

    async fn get_cve_feed_interval_secs(&self) -> Result<Option<i64>> {
        parse_setting_i64(
            CVE_FEED_INTERVAL_SECS_KEY,
            self.get_setting_string(CVE_FEED_INTERVAL_SECS_KEY).await?,
        )
    }

    async fn set_cve_feed_interval_secs(&self, value: Option<i64>) -> Result<()> {
        self.set_managed_feed_setting(
            CVE_FEED_INTERVAL_SECS_KEY,
            value.map(|n| n.to_string()).as_deref(),
        )
        .await
    }

    async fn record_base_branch(&self, repo_path: &str, branch: &str) -> Result<()> {
        let repo_path = repo_path.to_string();
        let branch = branch.to_string();
        self.db_call(move |conn| {
            conn.execute(
                "INSERT INTO repo_base_branches (repo_path, branch) VALUES (?1, ?2)
                 ON CONFLICT(repo_path, branch) DO UPDATE SET last_used = datetime('now')",
                params![repo_path, branch],
            )
            .context("Failed to record base branch")?;
            // Prune this repo's history down to the most-recently-used cap
            // (config.max_base_branches_per_repo; see dispatch.allium:
            // BranchHistoryCapped). Pruning is part of the write so callers
            // never have to manage the cap themselves.
            conn.execute(
                "DELETE FROM repo_base_branches
                 WHERE repo_path = ?1
                   AND id NOT IN (
                       SELECT id FROM repo_base_branches
                       WHERE repo_path = ?1
                       ORDER BY last_used DESC, id DESC
                       LIMIT ?2
                   )",
                params![repo_path, MAX_BASE_BRANCHES_PER_REPO],
            )
            .context("Failed to prune base branch history")?;
            Ok(())
        })
        .await
    }

    async fn list_all_base_branches(&self) -> Result<Vec<(String, String)>> {
        self.db_call_read(move |conn| {
            let mut stmt = conn
                .prepare(
                    "SELECT repo_path, branch FROM repo_base_branches ORDER BY last_used DESC, id DESC",
                )
                .context("Failed to prepare list_all_base_branches")?;
            let pairs = stmt
                .query_map([], |row| Ok((row.get(0)?, row.get(1)?)))
                .context("Failed to query repo_base_branches")?
                .collect::<rusqlite::Result<Vec<(String, String)>>>()
                .context("Failed to collect repo_base_branches")?;
            Ok(pairs)
        })
        .await
    }
}

/// Per-repo cap on remembered base branches (config.max_base_branches_per_repo
/// in docs/specs/dispatch.allium). `record_base_branch` prunes beyond this on
/// every write; see the `BranchHistoryCapped` invariant.
const MAX_BASE_BRANCHES_PER_REPO: i64 = 10;

// ---------------------------------------------------------------------------
// Managed-feed config keys (WP5)
// ---------------------------------------------------------------------------

const REVIEWS_FEED_COMMAND_KEY: &str = "reviews_feed_command";
const REVIEWS_FEED_INTERVAL_SECS_KEY: &str = "reviews_feed_interval_secs";
const CVE_FEED_COMMAND_KEY: &str = "cve_feed_command";
const CVE_FEED_INTERVAL_SECS_KEY: &str = "cve_feed_interval_secs";

/// Parse a settings value stored as a decimal string into an `i64`. A stored
/// non-integer is a corruption we surface rather than silently treat as unset.
fn parse_setting_i64(key: &str, raw: Option<String>) -> Result<Option<i64>> {
    match raw {
        Some(s) => s
            .parse::<i64>()
            .map(Some)
            .with_context(|| format!("setting {key:?} is not a valid integer: {s:?}")),
        None => Ok(None),
    }
}

impl Database {
    /// Upsert a managed-feed settings key when `value` is `Some`, or delete the
    /// row when `None` so a subsequent get returns `None`.
    async fn set_managed_feed_setting(&self, key: &'static str, value: Option<&str>) -> Result<()> {
        match value {
            Some(v) => self.set_setting_string(key, v).await,
            None => {
                self.db_call(move |conn| {
                    conn.execute("DELETE FROM settings WHERE key = ?1", params![key])
                        .context("Failed to delete setting")?;
                    Ok(())
                })
                .await
            }
        }
    }
}
