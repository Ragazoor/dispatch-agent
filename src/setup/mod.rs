//! First-run setup: MCP config merging, plugin installation (hooks, skills, commands).
//!
//! Split into submodules:
//! - `config` — Claude Code MCP config read/write/merge
//! - `plugins` — embedded plugin install (skills, slash commands, hooks, example feed script)
//! - `hooks` — tests for the embedded hook scripts (the install path lives in `plugins`)

mod config;
mod hooks;
mod plugins;
pub(crate) mod statusline;

use anyhow::{Context, Result};
use serde_json::Value;
use std::fs;
use std::io::Write;
use std::path::{Path, PathBuf};

use crate::db::Database;
use crate::process::RealProcessRunner;
use crate::tmux;

pub(crate) use config::dispatch_entry_identifying;
pub use config::{merge_mcp_config, remove_mcp_config, MergeResult};
pub use plugins::{install_example_script, remove_plugin, seed_feed_epics};

// ---------------------------------------------------------------------------
// Shared helpers
// ---------------------------------------------------------------------------

/// Path to Claude Code's user-global configuration directory (`~/.claude`).
///
/// Every caller resolves this once and then passes the result around — see
/// [`SetupPaths::resolve`], [`UninstallPaths::resolve`] and
/// `runtime::StartupPaths::resolve`. Nothing re-derives it mid-flow, which is
/// what lets a test point a whole flow at a temp directory.
///
/// The directory's name comes from `crate::claude_paths`, the one place it is
/// written down — the spawn constant expands the same token. `$HOME` is the
/// other half of the agreement: the spawn constant names this directory as
/// `~/…`, which the launching shell resolves under the home directory, so the
/// two halves agree only while this lookup reads nothing but `$HOME`. Adding a
/// second input here (an operator-settable configuration directory, say) parts
/// them silently for whoever sets it, and the spawn constant would have to stop
/// naming the location outright in the same change. See
/// docs/specs/dispatch.allium:
/// `SpawnSitesAndStartupNameTheSameConfigurationDirectory`.
pub(super) fn claude_dir() -> Result<PathBuf> {
    let home = std::env::var("HOME").context("$HOME is not set")?;
    Ok(PathBuf::from(home).join(crate::claude_paths::claude_dir_name!()))
}

/// Path to Claude Code's user-global config file (`~/.claude.json`).
///
/// This is where Claude Code reads user-level MCP servers from — *not*
/// `~/.claude/.mcp.json`, which Claude Code does not consume.
pub(super) fn user_global_config_path() -> Result<PathBuf> {
    let home = std::env::var("HOME").context("$HOME is not set")?;
    Ok(PathBuf::from(home).join(".claude.json"))
}

pub(super) fn read_json_file(path: &std::path::Path) -> Result<Option<Value>> {
    match fs::read_to_string(path) {
        Ok(content) => {
            let value: Value = serde_json::from_str(&content)
                .with_context(|| format!("Invalid JSON in {}", path.display()))?;
            Ok(Some(value))
        }
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => Ok(None),
        Err(e) => Err(e).with_context(|| format!("Failed to read {}", path.display())),
    }
}

pub(super) fn write_json_file(path: &std::path::Path, value: &Value) -> Result<()> {
    let content = serde_json::to_string_pretty(value).context("Failed to serialize JSON")?;
    fs::write(path, content + "\n").with_context(|| format!("Failed to write {}", path.display()))
}

/// Whether `path` already holds exactly `content` — the single definition of "this
/// file is already up to date" for everything setup manages.
///
/// Two callers must agree on it or setup contradicts itself:
/// [`write_file_if_changed`], which decides whether to write and what to report,
/// and `plugins::plugin_needs_update_in`, which decides whether to *offer* an
/// update. A file the predicate calls stale and the writer calls unchanged would
/// be reported as needing an update forever.
///
/// **Exact bytes, deliberately.** The statusline settings file previously compared
/// `.trim()`-normalized, which is what made the two disagree. Exact equality also
/// rewrites a file something else edited back to the canonical content, and
/// converges rather than flip-flopping. An unreadable or absent file is not up to
/// date: the writer's own error is a better report than a read error here.
fn file_is_up_to_date(path: &std::path::Path, content: &str) -> bool {
    fs::read_to_string(path).is_ok_and(|existing| existing == content)
}

/// Write `content` at `path`, creating parent directories, and report whether the
/// on-disk bytes actually changed. Every setup-managed file goes through this, so
/// setup can be run repeatedly and only report what it really touched.
///
/// Shared by the plugin installer (`plugins::install_dir_recursive`) and the
/// statusline settings file (`statusline::write_settings_file`) — it lives here
/// rather than in either of them so there is one place to change what writing
/// means. Up-to-dateness is [`file_is_up_to_date`]'s to define.
///
/// Deliberately unmarked rather than `pub(…)`: a private item in this module is
/// already reachable from every child that needs it, and a filesystem write is
/// not something the rest of the crate should be able to name.
fn write_file_if_changed(path: &std::path::Path, content: &str, executable: bool) -> Result<bool> {
    if file_is_up_to_date(path, content) {
        return Ok(false);
    }
    if let Some(parent) = path.parent() {
        fs::create_dir_all(parent)
            .with_context(|| format!("Failed to create {}", parent.display()))?;
    }
    fs::write(path, content).with_context(|| format!("Failed to write {}", path.display()))?;
    if executable {
        use std::os::unix::fs::PermissionsExt;
        fs::set_permissions(path, fs::Permissions::from_mode(0o755))
            .with_context(|| format!("Failed to set permissions on {}", path.display()))?;
    }
    Ok(true)
}

// ---------------------------------------------------------------------------
// Confirmation seam (mirrors `ProcessRunner` in src/process.rs)
// ---------------------------------------------------------------------------

/// Seam over interactive yes/no prompts so the setup/uninstall orchestration
/// flows can be driven deterministically in tests. The real implementation
/// ([`StdinConfirmer`]) reads from stdin; tests inject a fake that returns
/// queued answers.
pub trait Confirmer {
    /// Prompt defaulting to **Yes** (empty input counts as yes).
    fn confirm(&self, prompt: &str) -> Result<bool>;

    /// Prompt defaulting to **No** — the user must explicitly type "y".
    fn confirm_dangerous(&self, prompt: &str) -> Result<bool>;
}

/// Real confirmer backed by stderr prompts and stdin input.
pub struct StdinConfirmer;

impl StdinConfirmer {
    /// Prompt on stderr and read a yes/no answer from stdin. `default_yes`
    /// selects both the displayed hint (`[Y/n]` vs `[y/N]`) and the meaning of
    /// empty input.
    fn prompt(&self, prompt: &str, default_yes: bool) -> Result<bool> {
        let hint = if default_yes { "[Y/n]" } else { "[y/N]" };
        eprint!("{prompt} {hint} ");
        std::io::stderr().flush()?;
        let mut input = String::new();
        std::io::stdin().read_line(&mut input)?;
        let trimmed = input.trim().to_lowercase();
        Ok(match trimmed.as_str() {
            "" => default_yes,
            "y" | "yes" => true,
            _ => false,
        })
    }
}

impl Confirmer for StdinConfirmer {
    fn confirm(&self, prompt: &str) -> Result<bool> {
        self.prompt(prompt, true)
    }

    fn confirm_dangerous(&self, prompt: &str) -> Result<bool> {
        self.prompt(prompt, false)
    }
}

fn count_tasks(db_path: &std::path::Path) -> Result<i64> {
    let conn =
        rusqlite::Connection::open_with_flags(db_path, rusqlite::OpenFlags::SQLITE_OPEN_READ_ONLY)?;
    let count: i64 = conn.query_row("SELECT COUNT(*) FROM tasks", [], |row| row.get(0))?;
    Ok(count)
}

pub fn remove_database(db_path: &std::path::Path) -> Result<bool> {
    if !db_path.exists() {
        return Ok(false);
    }

    let parent = db_path
        .parent()
        .context("database path has no parent directory")?;

    for name in ["tasks.db", "tasks.db-wal", "tasks.db-shm", "app.log"] {
        let path = parent.join(name);
        if path.exists() {
            fs::remove_file(&path)
                .with_context(|| format!("Failed to remove {}", path.display()))?;
        }
    }

    if parent.exists() && parent.read_dir()?.next().is_none() {
        fs::remove_dir(parent).with_context(|| format!("Failed to remove {}", parent.display()))?;
    }

    Ok(true)
}

/// Apply the dispatch MCP entry to `target` (Claude Code's user-global config,
/// `~/.claude.json`) and remove any stale entry from `legacy` (the old wrong
/// path, `~/.claude/.mcp.json`, that earlier dispatch versions wrote to and
/// that Claude Code never read).
///
/// Returns `true` if either file changed.
///
/// When `prompt_yes` is false the user is prompted before writing `target`;
/// the legacy cleanup is unconditional and cannot be suppressed by callers
/// (it only ever removes the `dispatch` entry from a file Claude Code does
/// not read, so it is always safe).
pub(super) fn apply_mcp_setup(
    target: &Path,
    legacy: &Path,
    port: u16,
    prompt_yes: bool,
    confirmer: &dyn Confirmer,
) -> Result<bool> {
    let mut changed = false;

    let existing = read_json_file(target)?;
    let merged = merge_mcp_config(existing, port);
    if merged.changed {
        let display = display_for(target);
        if prompt_yes
            || confirmer.confirm(&format!(
                "Add dispatch MCP server (localhost:{port}) to {display}?"
            ))?
        {
            write_json_file(target, &merged.value)?;
            println!("MCP config: added dispatch to {display} (port {port})");
            changed = true;
        } else {
            println!("MCP config: skipped");
        }
    } else {
        println!(
            "MCP config: dispatch already configured in {}",
            display_for(target)
        );
    }

    match remove_mcp_config(legacy) {
        Ok(true) => {
            println!(
                "MCP config: removed stale dispatch entry from {} (Claude Code did not read this file)",
                legacy.display()
            );
            changed = true;
        }
        Ok(false) => {}
        Err(e) => eprintln!("Warning: failed to clean up legacy MCP config: {e}"),
    }

    Ok(changed)
}

/// Best-effort tilde-shortened display for paths under `$HOME`.
fn display_for(path: &Path) -> String {
    if let Ok(home) = std::env::var("HOME") {
        let home_path = std::path::Path::new(&home);
        if let Ok(stripped) = path.strip_prefix(home_path) {
            return format!("~/{}", stripped.display());
        }
    }
    path.display().to_string()
}

// ---------------------------------------------------------------------------
// run_setup — top-level orchestrator
// ---------------------------------------------------------------------------

/// Filesystem locations the setup flow writes to. Grouped so tests can point
/// the whole flow at temp directories instead of the real `$HOME`.
pub(super) struct SetupPaths {
    pub claude_dir: PathBuf,
    pub mcp_path: PathBuf,
    pub legacy_mcp_path: PathBuf,
    pub tmux_conf_path: PathBuf,
    pub statusline_path: PathBuf,
    /// Where the statusLine decorator is told to publish the budget snapshot.
    /// Fixed per machine and independent of `--db` — see
    /// `docs/specs/dispatch.allium`:
    /// `SnapshotLocationIsFixedNotDerivedFromTheOpenDatabase`.
    pub budget_snapshot_path: PathBuf,
}

impl SetupPaths {
    /// Resolve the real `$HOME`-derived locations used in production.
    fn resolve() -> Result<Self> {
        let claude_dir = claude_dir()?;
        let legacy_mcp_path = claude_dir.join(".mcp.json");
        let statusline_path = statusline::settings_path(&claude_dir);
        Ok(Self {
            claude_dir,
            mcp_path: user_global_config_path()?,
            legacy_mcp_path,
            tmux_conf_path: tmux::tmux_conf_path()?,
            statusline_path,
            budget_snapshot_path: crate::budget_snapshot_path(),
        })
    }
}

pub async fn run_setup(port: u16, yes: bool, db_path: &Path) -> Result<()> {
    let db = Database::open(db_path).await?;
    let data_dir = db_path
        .parent()
        .context("database path has no parent directory")?;
    let paths = SetupPaths::resolve()?;
    run_setup_in(
        &db,
        data_dir,
        &paths,
        port,
        yes,
        &StdinConfirmer,
        &RealProcessRunner::default(),
    )
    .await
}

/// Injectable core of [`run_setup`]. Takes the target filesystem locations, a
/// confirmer, and a process runner (for tmux) so the orchestration can be
/// exercised deterministically in tests.
#[allow(clippy::too_many_arguments)]
pub(super) async fn run_setup_in(
    db: &Database,
    data_dir: &Path,
    paths: &SetupPaths,
    port: u16,
    yes: bool,
    confirmer: &dyn Confirmer,
    runner: &dyn crate::process::ProcessRunner,
) -> Result<()> {
    seed_feed_epics(db, data_dir).await?;
    fs::create_dir_all(&paths.claude_dir)
        .with_context(|| format!("Failed to create {}", paths.claude_dir.display()))?;

    let mut any_changes = false;

    // 1. MCP config — Claude Code reads user-level MCP servers from
    // `~/.claude.json`, NOT `~/.claude/.mcp.json`. Older dispatch setups
    // wrote to the latter; clean that up.
    if apply_mcp_setup(
        &paths.mcp_path,
        &paths.legacy_mcp_path,
        port,
        yes,
        confirmer,
    )? {
        any_changes = true;
    }

    // 2. Plugin (hooks, skills, commands)
    let plugin_base = plugins::plugin_dir_under(&paths.claude_dir);
    if plugins::plugin_needs_update_in(&plugin_base)? {
        // The prompt and the report name `plugin_base` itself rather than a
        // hand-written copy of the layout, which a rename would leave stale and
        // pointing the operator at a directory nothing writes to.
        if yes
            || confirmer.confirm(&format!(
                "Install dispatch plugin (skills, hooks, commands) to {}/?",
                plugin_base.display()
            ))?
        {
            plugins::install_plugin_in(&plugin_base)?;
            println!(
                "Plugin: installed dispatch plugin to {}/",
                plugin_base.display()
            );
            let skills: Vec<String> = plugins::PLUGIN_DIR
                .get_dir("skills")
                .map(|d| {
                    let mut names: Vec<String> = d
                        .dirs()
                        .filter_map(|sd| sd.path().file_name()?.to_str().map(|n| format!("/{n}")))
                        .collect();
                    names.sort();
                    names
                })
                .unwrap_or_default();
            println!("  → Skills: {}", skills.join(", "));
            let commands: Vec<String> = plugins::PLUGIN_DIR
                .get_dir("commands")
                .map(|d| {
                    let mut names: Vec<String> = d
                        .files()
                        .filter_map(|f| f.path().file_stem()?.to_str().map(|n| format!("/{n}")))
                        .collect();
                    names.sort();
                    names
                })
                .unwrap_or_default();
            println!("  → Commands: {}", commands.join(", "));
            println!("  → Hooks: task-status, task-usage");
            any_changes = true;
        } else {
            println!("Plugin: skipped");
        }
    } else {
        println!("Plugin: dispatch plugin already up to date");
    }

    // 2b. Status line — dispatch-owned settings file that chains to the
    // user's existing statusLine.command (see src/setup/statusline.rs).
    let chain = statusline::discover_chain(&paths.claude_dir);
    match statusline::write_settings_file(
        &paths.statusline_path,
        &paths.budget_snapshot_path,
        chain.as_deref(),
    ) {
        Ok(true) => println!(
            "Status line: wrote {} (budget indicator){}",
            display_for(&paths.statusline_path),
            match &chain {
                Some(c) => format!(", chaining to `{c}`"),
                None => String::new(),
            }
        ),
        Ok(false) => println!("Status line: already configured"),
        Err(e) => eprintln!("Warning: failed to write statusline settings: {e}"),
    }

    // 3. Tmux focus-events
    if !tmux::focus_events_enabled(runner) {
        if yes || confirmer.confirm("Enable tmux focus-events? (will run `tmux set-option -g focus-events on` and add `set -g focus-events on` to ~/.tmux.conf)")? {
            tmux::set_focus_events(runner)?;
            tmux::write_focus_events_to_tmux_conf_at(&paths.tmux_conf_path)?;
            println!("Tmux: enabled focus-events (set for current server and added to ~/.tmux.conf)");
            any_changes = true;
        } else {
            println!("Tmux: focus-events skipped");
        }
    } else {
        tmux::write_focus_events_to_tmux_conf_at(&paths.tmux_conf_path)?;
        println!("Tmux: focus-events already enabled (ensuring ~/.tmux.conf is up to date)");
    }

    if any_changes {
        println!("Setup complete.");
    } else {
        println!("Already configured, nothing to do.");
    }

    Ok(())
}

// ---------------------------------------------------------------------------
// run_uninstall — reverse of run_setup
// ---------------------------------------------------------------------------

/// Filesystem locations the uninstall flow removes. Grouped so tests can point
/// the whole flow at temp directories instead of the real `$HOME`.
pub(super) struct UninstallPaths {
    pub mcp_path: PathBuf,
    pub legacy_mcp_path: PathBuf,
    pub plugin_path: PathBuf,
    pub db_path: PathBuf,
    pub statusline_path: PathBuf,
}

impl UninstallPaths {
    /// Resolve the real `$HOME`-derived locations used in production.
    fn resolve() -> Result<Self> {
        let claude_dir = claude_dir()?;
        Ok(Self {
            mcp_path: user_global_config_path()?,
            legacy_mcp_path: claude_dir.join(".mcp.json"),
            plugin_path: plugins::plugin_dir()?,
            db_path: crate::default_db_path(),
            statusline_path: statusline::settings_path(&claude_dir),
        })
    }
}

pub fn run_uninstall(yes: bool, purge: bool) -> Result<()> {
    let paths = UninstallPaths::resolve()?;
    run_uninstall_in(&paths, &StdinConfirmer, yes, purge)
}

/// Injectable core of [`run_uninstall`]. Takes the target filesystem locations
/// and a confirmer so the removal decision matrix can be exercised
/// deterministically in tests.
pub(super) fn run_uninstall_in(
    paths: &UninstallPaths,
    confirmer: &dyn Confirmer,
    yes: bool,
    purge: bool,
) -> Result<()> {
    let UninstallPaths {
        mcp_path,
        legacy_mcp_path,
        plugin_path,
        db_path,
        statusline_path,
    } = paths;

    // Show what will be removed
    eprintln!("This will remove:");
    eprintln!("  Plugin:      {}", plugin_path.display());
    eprintln!(
        "  MCP config:  mcpServers.dispatch from {}",
        mcp_path.display()
    );
    eprintln!(
        "  Legacy MCP:  mcpServers.dispatch from {} (if present)",
        legacy_mcp_path.display()
    );
    eprintln!("  Status line: {} (if present)", statusline_path.display());
    if purge {
        eprintln!("  Database:    {}", db_path.display());
    }

    if !yes && !confirmer.confirm("\nContinue?")? {
        println!("Aborted.");
        return Ok(());
    }

    let mut any_removed = false;

    match remove_plugin(plugin_path) {
        Ok(true) => {
            println!("Removed plugin directory");
            any_removed = true;
        }
        Ok(false) => println!("Plugin directory not found, skipping"),
        Err(e) => eprintln!("Warning: failed to remove plugin: {e}"),
    }

    match remove_mcp_config(mcp_path) {
        Ok(true) => {
            println!("Removed dispatch from MCP config");
            any_removed = true;
        }
        Ok(false) => println!("No dispatch entry in MCP config, skipping"),
        Err(e) => eprintln!("Warning: failed to update MCP config: {e}"),
    }

    // Legacy cleanup: remove any stale entry from ~/.claude/.mcp.json that
    // earlier dispatch versions mistakenly wrote there.
    match remove_mcp_config(legacy_mcp_path) {
        Ok(true) => {
            println!(
                "Removed stale dispatch entry from {}",
                legacy_mcp_path.display()
            );
            any_removed = true;
        }
        Ok(false) => {}
        Err(e) => eprintln!("Warning: failed to clean up legacy MCP config: {e}"),
    }

    // Status line settings file — dispatch-owned, written by `dispatch setup`
    // (see src/setup/statusline.rs). Best-effort: a missing file is a no-op.
    match fs::remove_file(statusline_path) {
        Ok(()) => {
            println!("Removed status line settings file");
            any_removed = true;
        }
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => {
            println!("Status line settings file not found, skipping")
        }
        Err(e) => eprintln!("Warning: failed to remove status line settings file: {e}"),
    }

    // Note: ~/.claude/settings.json is intentionally not touched. Dispatch no
    // longer manages permissions in that file — it is user-owned config. Users
    // who ran an older `dispatch setup` may have stale mcp__dispatch__* entries
    // in settings.json; those are inert once the MCP server is removed and can
    // be cleaned up manually.

    if purge {
        if db_path.exists() {
            let task_count = count_tasks(db_path).unwrap_or(0);
            eprintln!("\n  Database contains {task_count} task(s). This cannot be undone.");
            if confirmer.confirm_dangerous("Delete database?")? {
                match remove_database(db_path) {
                    Ok(true) => {
                        println!("Removed database");
                        any_removed = true;
                    }
                    Ok(false) => println!("Database not found, skipping"),
                    Err(e) => eprintln!("Warning: failed to remove database: {e}"),
                }
            } else {
                println!("Kept database.");
            }
        } else {
            println!("Database not found, skipping");
        }
    }

    if any_removed {
        println!("Uninstall complete.");
    } else {
        println!("Nothing to remove.");
    }

    Ok(())
}

// ---------------------------------------------------------------------------
// Tests for shared helpers
// ---------------------------------------------------------------------------

#[cfg(test)]
mod tests {
    #![allow(clippy::unwrap_used, clippy::expect_used)]
    use super::*;
    use crate::db::EpicRead;
    use crate::process::MockProcessRunner;
    use serde_json::json;
    use std::collections::VecDeque;
    use std::sync::Mutex;

    /// A [`Confirmer`] that returns queued answers instead of reading stdin,
    /// mirroring `MockProcessRunner`. Separate queues for the default-yes and
    /// default-no (dangerous) prompts so tests assert which kind fired.
    /// Panics if a prompt is issued with no queued answer — the same
    /// fail-loud contract as `MockProcessRunner`.
    struct FakeConfirmer {
        confirm_answers: Mutex<VecDeque<bool>>,
        dangerous_answers: Mutex<VecDeque<bool>>,
        confirm_calls: Mutex<usize>,
        dangerous_calls: Mutex<usize>,
    }

    impl FakeConfirmer {
        fn new(confirm: Vec<bool>, dangerous: Vec<bool>) -> Self {
            Self {
                confirm_answers: Mutex::new(confirm.into()),
                dangerous_answers: Mutex::new(dangerous.into()),
                confirm_calls: Mutex::new(0),
                dangerous_calls: Mutex::new(0),
            }
        }

        /// Confirmer that must never be prompted (e.g. the `--yes` path).
        fn never() -> Self {
            Self::new(vec![], vec![])
        }

        fn confirm_call_count(&self) -> usize {
            *self.confirm_calls.lock().unwrap()
        }

        fn dangerous_call_count(&self) -> usize {
            *self.dangerous_calls.lock().unwrap()
        }
    }

    impl Confirmer for FakeConfirmer {
        fn confirm(&self, _prompt: &str) -> Result<bool> {
            *self.confirm_calls.lock().unwrap() += 1;
            Ok(self
                .confirm_answers
                .lock()
                .unwrap()
                .pop_front()
                .expect("FakeConfirmer: no confirm answer queued"))
        }

        fn confirm_dangerous(&self, _prompt: &str) -> Result<bool> {
            *self.dangerous_calls.lock().unwrap() += 1;
            Ok(self
                .dangerous_answers
                .lock()
                .unwrap()
                .pop_front()
                .expect("FakeConfirmer: no dangerous answer queued"))
        }
    }

    // -- File I/O --

    #[test]
    fn read_json_file_missing_returns_none() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("nonexistent.json");
        let result = read_json_file(&path).unwrap();
        assert!(result.is_none());
    }

    #[test]
    fn read_json_file_invalid_json_errors() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("bad.json");
        fs::write(&path, "not json").unwrap();
        let result = read_json_file(&path);
        assert!(result.is_err());
        assert!(result.unwrap_err().to_string().contains("Invalid JSON"),);
    }

    #[test]
    fn write_and_read_json_roundtrip() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("test.json");
        let value = json!({"key": "value"});
        write_json_file(&path, &value).unwrap();
        let read_back = read_json_file(&path).unwrap().unwrap();
        assert_eq!(read_back, value);
    }

    // -- write_file_if_changed --

    #[test]
    fn write_file_if_changed_creates_new() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("new.txt");
        let changed = write_file_if_changed(&path, "hello", false).unwrap();
        assert!(changed);
        assert_eq!(fs::read_to_string(&path).unwrap(), "hello");
    }

    #[test]
    fn write_file_if_changed_skips_identical() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("same.txt");
        fs::write(&path, "hello").unwrap();
        let changed = write_file_if_changed(&path, "hello", false).unwrap();
        assert!(!changed);
    }

    #[test]
    fn write_file_if_changed_updates_stale() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("stale.txt");
        fs::write(&path, "old").unwrap();
        let changed = write_file_if_changed(&path, "new", false).unwrap();
        assert!(changed);
        assert_eq!(fs::read_to_string(&path).unwrap(), "new");
    }

    #[test]
    fn write_file_if_changed_creates_missing_parent_directories() {
        // Both callers need this: the plugin path has nested skill/hook
        // directories, and the statusline settings file can land in a
        // `~/.claude` that does not exist yet.
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("nested").join("deeper").join("new.txt");
        assert!(write_file_if_changed(&path, "hello", false).unwrap());
        assert_eq!(fs::read_to_string(&path).unwrap(), "hello");
    }

    /// Exact bytes, not trim-normalized: `plugins::plugin_needs_update_in` asks
    /// the same question with exact equality, and the two must not disagree.
    #[test]
    fn write_file_if_changed_rewrites_when_only_whitespace_differs() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("padded.txt");
        fs::write(&path, "hello\n").unwrap();
        assert!(write_file_if_changed(&path, "hello", false).unwrap());
        assert_eq!(fs::read_to_string(&path).unwrap(), "hello");
    }

    #[test]
    fn write_file_if_changed_sets_executable_permission() {
        use std::os::unix::fs::PermissionsExt;
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("script.sh");
        write_file_if_changed(&path, "#!/bin/bash", true).unwrap();
        let mode = fs::metadata(&path).unwrap().permissions().mode();
        assert_eq!(mode & 0o755, 0o755, "should have executable permissions");
    }

    // -- Database removal --

    #[test]
    fn remove_database_deletes_db_and_related_files() {
        let dir = tempfile::tempdir().unwrap();
        let data_dir = dir.path().join("dispatch");
        fs::create_dir_all(&data_dir).unwrap();
        fs::write(data_dir.join("tasks.db"), "db").unwrap();
        fs::write(data_dir.join("tasks.db-wal"), "wal").unwrap();
        fs::write(data_dir.join("tasks.db-shm"), "shm").unwrap();
        fs::write(data_dir.join("app.log"), "log").unwrap();

        let db_path = data_dir.join("tasks.db");
        let removed = remove_database(&db_path).unwrap();
        assert!(removed);
        assert!(!data_dir.join("tasks.db").exists());
        assert!(!data_dir.join("tasks.db-wal").exists());
        assert!(!data_dir.join("tasks.db-shm").exists());
        assert!(!data_dir.join("app.log").exists());
        assert!(!data_dir.exists());
    }

    #[test]
    fn remove_database_keeps_parent_if_not_empty() {
        let dir = tempfile::tempdir().unwrap();
        let data_dir = dir.path().join("dispatch");
        fs::create_dir_all(&data_dir).unwrap();
        fs::write(data_dir.join("tasks.db"), "db").unwrap();
        fs::write(data_dir.join("other.txt"), "keep").unwrap();

        let db_path = data_dir.join("tasks.db");
        let removed = remove_database(&db_path).unwrap();
        assert!(removed);
        assert!(!data_dir.join("tasks.db").exists());
        assert!(data_dir.exists());
        assert!(data_dir.join("other.txt").exists());
    }

    // -- MCP setup application --

    #[test]
    fn apply_mcp_setup_writes_to_target_not_legacy() {
        // Guard against regression: dispatch must write to the user-global
        // file (~/.claude.json), not ~/.claude/.mcp.json which Claude Code
        // does not read.
        let dir = tempfile::tempdir().unwrap();
        let target = dir.path().join(".claude.json");
        let legacy = dir.path().join(".claude").join(".mcp.json");

        let changed = apply_mcp_setup(&target, &legacy, 3142, true, &StdinConfirmer).unwrap();
        assert!(changed);
        assert!(target.exists(), "target ~/.claude.json must be created");
        assert!(!legacy.exists(), "legacy file must not be created");

        let written = read_json_file(&target).unwrap().unwrap();
        assert_eq!(
            written["mcpServers"]["dispatch"]["url"],
            "http://localhost:3142/mcp"
        );
        assert!(written["mcpServers"]["dispatch"]["headersHelper"].is_string());
    }

    #[test]
    fn apply_mcp_setup_preserves_existing_target_fields() {
        // ~/.claude.json contains many fields (themes, tips, etc.). Setup
        // must merge into it, not overwrite it.
        let dir = tempfile::tempdir().unwrap();
        let target = dir.path().join(".claude.json");
        let legacy = dir.path().join(".claude").join(".mcp.json");
        write_json_file(
            &target,
            &json!({
                "theme": "dark",
                "mcpServers": {
                    "github": {"type": "http", "url": "http://localhost:9999/mcp"}
                }
            }),
        )
        .unwrap();

        apply_mcp_setup(&target, &legacy, 3142, true, &StdinConfirmer).unwrap();

        let written = read_json_file(&target).unwrap().unwrap();
        assert_eq!(written["theme"], "dark");
        assert!(written["mcpServers"]["github"].is_object());
        assert!(written["mcpServers"]["dispatch"].is_object());
    }

    #[test]
    fn apply_mcp_setup_migrates_legacy_dispatch_entry() {
        // Upgrade path: dispatch entry sits in the wrong legacy file.
        // After running setup it must be installed in the target and
        // removed from the legacy file.
        let dir = tempfile::tempdir().unwrap();
        let target = dir.path().join(".claude.json");
        let legacy_dir = dir.path().join(".claude");
        fs::create_dir_all(&legacy_dir).unwrap();
        let legacy = legacy_dir.join(".mcp.json");
        write_json_file(
            &legacy,
            &json!({
                "mcpServers": {
                    "dispatch": {"type": "http", "url": "http://localhost:3142/mcp"},
                    "github": {"type": "http", "url": "http://localhost:9999/mcp"}
                }
            }),
        )
        .unwrap();

        let changed = apply_mcp_setup(&target, &legacy, 3142, true, &StdinConfirmer).unwrap();
        assert!(changed);

        // Target got the dispatch entry (with headersHelper).
        let written = read_json_file(&target).unwrap().unwrap();
        assert!(written["mcpServers"]["dispatch"]["headersHelper"].is_string());

        // Legacy lost the dispatch entry but kept the other server.
        let legacy_after = read_json_file(&legacy).unwrap().unwrap();
        assert!(legacy_after["mcpServers"].get("dispatch").is_none());
        assert!(legacy_after["mcpServers"]["github"].is_object());
    }

    #[test]
    fn apply_mcp_setup_idempotent() {
        let dir = tempfile::tempdir().unwrap();
        let target = dir.path().join(".claude.json");
        let legacy = dir.path().join(".claude").join(".mcp.json");

        apply_mcp_setup(&target, &legacy, 3142, true, &StdinConfirmer).unwrap();
        let changed = apply_mcp_setup(&target, &legacy, 3142, true, &StdinConfirmer).unwrap();
        assert!(
            !changed,
            "second apply with no changes must report unchanged"
        );
    }

    #[test]
    fn remove_database_noop_when_missing() {
        let dir = tempfile::tempdir().unwrap();
        let db_path = dir.path().join("dispatch").join("tasks.db");

        let removed = remove_database(&db_path).unwrap();
        assert!(!removed);
    }

    #[test]
    fn setup_does_not_write_settings_json() {
        // Regression guard: the setup flow must not create or modify settings.json.
        // That file is user-owned config; dispatch must not add permissions to it.
        let dir = tempfile::tempdir().unwrap();
        let claude_json = dir.path().join(".claude.json");
        let legacy = dir.path().join(".mcp.json");
        let settings = dir.path().join("settings.json");

        apply_mcp_setup(&claude_json, &legacy, 3142, true, &StdinConfirmer).unwrap();

        assert!(
            !settings.exists(),
            "setup must not create settings.json; permissions are user-managed"
        );
    }

    // -- count_tasks --

    #[tokio::test]
    async fn count_tasks_reports_zero_for_fresh_db() {
        let dir = tempfile::tempdir().unwrap();
        let db_path = dir.path().join("dispatch").join("tasks.db");
        fs::create_dir_all(db_path.parent().unwrap()).unwrap();
        // Create the schema, then drop the handle so count_tasks can open it.
        let db = Database::open(&db_path).await.unwrap();
        drop(db);

        let count = count_tasks(&db_path).unwrap();
        assert_eq!(count, 0, "a freshly-created db has no tasks");
    }

    // -- display_for --

    #[test]
    fn display_for_shortens_home_prefixed_paths() {
        // Uses the real $HOME (present in every test env) without mutating it,
        // so it is safe under parallel execution.
        let home = std::env::var("HOME").unwrap();
        let path = std::path::Path::new(&home).join("some").join("file.json");
        assert_eq!(display_for(&path), "~/some/file.json");
    }

    #[test]
    fn display_for_leaves_non_home_paths_untouched() {
        // A path guaranteed not to sit under $HOME must be shown verbatim.
        let path = std::path::Path::new("/definitely/not/home/x.json");
        assert_eq!(display_for(path), "/definitely/not/home/x.json");
    }

    // -- run_uninstall_in: removal decision matrix --

    /// Build a fully-populated uninstall layout under a temp dir: a plugin
    /// directory with a file, a `~/.claude.json` carrying the dispatch MCP
    /// entry, an empty legacy file, a statusline settings file, and a
    /// `db_path` that does not yet exist.
    fn uninstall_layout(root: &Path) -> UninstallPaths {
        let plugin_path = root.join("plugins").join("local").join("dispatch");
        fs::create_dir_all(&plugin_path).unwrap();
        fs::write(plugin_path.join(".claude-plugin.json"), "{}").unwrap();

        let mcp_path = root.join(".claude.json");
        write_json_file(
            &mcp_path,
            &json!({
                "mcpServers": {
                    "dispatch": {"type": "http", "url": "http://localhost:3142/mcp"},
                    "github": {"type": "http", "url": "http://localhost:9999/mcp"}
                }
            }),
        )
        .unwrap();

        let statusline_path = root.join(".claude").join(statusline::SETTINGS_FILE_NAME);
        fs::create_dir_all(statusline_path.parent().unwrap()).unwrap();
        fs::write(&statusline_path, "{}").unwrap();

        UninstallPaths {
            mcp_path,
            legacy_mcp_path: root.join(".claude").join(".mcp.json"),
            plugin_path,
            db_path: root.join("dispatch").join("tasks.db"),
            statusline_path,
        }
    }

    #[test]
    fn run_uninstall_in_removes_plugin_and_mcp_when_confirmed() {
        let dir = tempfile::tempdir().unwrap();
        let paths = uninstall_layout(dir.path());
        let confirmer = FakeConfirmer::new(vec![true], vec![]);

        run_uninstall_in(&paths, &confirmer, false, false).unwrap();

        assert!(!paths.plugin_path.exists(), "plugin dir must be removed");
        let mcp = read_json_file(&paths.mcp_path).unwrap().unwrap();
        assert!(
            mcp["mcpServers"].get("dispatch").is_none(),
            "dispatch MCP entry must be removed"
        );
        assert!(
            mcp["mcpServers"]["github"].is_object(),
            "unrelated MCP servers must be preserved"
        );
        assert!(
            !paths.statusline_path.exists(),
            "statusline settings file must be removed"
        );
        assert_eq!(confirmer.confirm_call_count(), 1, "one 'Continue?' prompt");
    }

    #[test]
    fn run_uninstall_in_statusline_file_missing_is_a_noop() {
        let dir = tempfile::tempdir().unwrap();
        let paths = uninstall_layout(dir.path());
        fs::remove_file(&paths.statusline_path).unwrap();
        let confirmer = FakeConfirmer::new(vec![true], vec![]);

        // Must not error just because the statusline file was already absent.
        run_uninstall_in(&paths, &confirmer, false, false).unwrap();

        assert!(!paths.statusline_path.exists());
    }

    #[test]
    fn run_uninstall_in_aborts_when_declined() {
        let dir = tempfile::tempdir().unwrap();
        let paths = uninstall_layout(dir.path());
        let confirmer = FakeConfirmer::new(vec![false], vec![]);

        run_uninstall_in(&paths, &confirmer, false, false).unwrap();

        assert!(
            paths.plugin_path.exists(),
            "declining must leave the plugin dir untouched"
        );
        let mcp = read_json_file(&paths.mcp_path).unwrap().unwrap();
        assert!(
            mcp["mcpServers"]["dispatch"].is_object(),
            "declining must leave the MCP entry untouched"
        );
    }

    #[test]
    fn run_uninstall_in_yes_skips_continue_prompt() {
        let dir = tempfile::tempdir().unwrap();
        let paths = uninstall_layout(dir.path());
        // never() panics if any prompt fires — asserts --yes suppresses "Continue?".
        let confirmer = FakeConfirmer::never();

        run_uninstall_in(&paths, &confirmer, true, false).unwrap();

        assert!(!paths.plugin_path.exists(), "plugin dir must be removed");
        assert_eq!(confirmer.confirm_call_count(), 0);
    }

    #[tokio::test]
    async fn run_uninstall_in_purge_deletes_db_when_dangerous_confirmed() {
        let dir = tempfile::tempdir().unwrap();
        let paths = uninstall_layout(dir.path());
        let db = Database::open(&paths.db_path).await.unwrap();
        drop(db);
        assert!(paths.db_path.exists());

        // confirm "Continue?" -> yes; confirm_dangerous "Delete database?" -> yes.
        let confirmer = FakeConfirmer::new(vec![true], vec![true]);
        run_uninstall_in(&paths, &confirmer, false, true).unwrap();

        assert!(!paths.db_path.exists(), "purge must delete the database");
        assert_eq!(confirmer.dangerous_call_count(), 1);
    }

    #[tokio::test]
    async fn run_uninstall_in_purge_keeps_db_when_dangerous_declined() {
        let dir = tempfile::tempdir().unwrap();
        let paths = uninstall_layout(dir.path());
        let db = Database::open(&paths.db_path).await.unwrap();
        drop(db);

        let confirmer = FakeConfirmer::new(vec![true], vec![false]);
        run_uninstall_in(&paths, &confirmer, false, true).unwrap();

        assert!(
            paths.db_path.exists(),
            "declining the dangerous prompt must keep the database"
        );
    }

    #[tokio::test]
    async fn run_uninstall_in_yes_still_prompts_before_deleting_db() {
        // Regression guard: --yes suppresses "Continue?" but must NOT
        // auto-confirm the irreversible database deletion.
        let dir = tempfile::tempdir().unwrap();
        let paths = uninstall_layout(dir.path());
        let db = Database::open(&paths.db_path).await.unwrap();
        drop(db);

        // No confirm answers queued (would panic if consulted); dangerous -> no.
        let confirmer = FakeConfirmer::new(vec![], vec![false]);
        run_uninstall_in(&paths, &confirmer, true, true).unwrap();

        assert_eq!(confirmer.confirm_call_count(), 0, "--yes skips 'Continue?'");
        assert_eq!(
            confirmer.dangerous_call_count(),
            1,
            "--yes must still prompt before deleting the database"
        );
        assert!(paths.db_path.exists(), "db kept because dangerous declined");
    }

    #[test]
    fn run_uninstall_in_noop_when_nothing_present() {
        let dir = tempfile::tempdir().unwrap();
        // Bare paths: nothing exists on disk.
        let paths = UninstallPaths {
            mcp_path: dir.path().join(".claude.json"),
            legacy_mcp_path: dir.path().join(".mcp.json"),
            plugin_path: dir.path().join("plugin"),
            db_path: dir.path().join("dispatch").join("tasks.db"),
            statusline_path: dir.path().join(statusline::SETTINGS_FILE_NAME),
        };
        let confirmer = FakeConfirmer::new(vec![true], vec![]);

        // Must not error even though there is nothing to remove.
        run_uninstall_in(&paths, &confirmer, false, false).unwrap();
    }

    // -- run_setup_in: setup decision flow --

    /// Build empty setup paths under a temp root plus a fresh in-memory db and
    /// a temp data dir. Returns `(paths, data_dir)` — `data_dir` must be kept
    /// alive for its temp path to remain valid.
    fn setup_layout(root: &Path) -> SetupPaths {
        let claude_dir = root.join(".claude");
        SetupPaths {
            claude_dir: claude_dir.clone(),
            mcp_path: root.join(".claude.json"),
            legacy_mcp_path: claude_dir.join(".mcp.json"),
            tmux_conf_path: root.join(".tmux.conf"),
            statusline_path: statusline::settings_path(&claude_dir),
            budget_snapshot_path: root.join("data").join("rate-limits.json"),
        }
    }

    #[tokio::test]
    async fn run_setup_in_fresh_install_writes_everything() {
        let root = tempfile::tempdir().unwrap();
        let data_dir = tempfile::tempdir().unwrap();
        let paths = setup_layout(root.path());
        let db = Database::open_in_memory().await.unwrap();

        // focus-events currently OFF, then set-option succeeds.
        let runner = MockProcessRunner::new(vec![
            MockProcessRunner::ok_with_stdout(b"off\n"),
            MockProcessRunner::ok(),
        ]);
        // yes=true: no confirmer prompts should fire.
        let confirmer = FakeConfirmer::never();

        run_setup_in(
            &db,
            data_dir.path(),
            &paths,
            3142,
            true,
            &confirmer,
            &runner,
        )
        .await
        .unwrap();

        // MCP config written to the target with the dispatch entry.
        let mcp = read_json_file(&paths.mcp_path).unwrap().unwrap();
        assert_eq!(
            mcp["mcpServers"]["dispatch"]["url"], "http://localhost:3142/mcp",
            "dispatch MCP entry must be written"
        );
        // Plugin installed under the injected claude dir.
        let plugin_base = plugins::plugin_dir_under(&paths.claude_dir);
        assert!(
            plugin_base.join(".claude-plugin/plugin.json").exists(),
            "plugin must be installed under the injected claude dir"
        );
        // tmux.conf gained the focus-events line.
        let conf = fs::read_to_string(&paths.tmux_conf_path).unwrap();
        assert!(conf.contains("focus-events on"));
        // Example feed epic seeded.
        assert_eq!(db.list_epics().await.unwrap().len(), 1);
        assert_eq!(
            confirmer.confirm_call_count(),
            0,
            "--yes suppresses prompts"
        );

        // Status line: dispatch-owned settings file written, statusLine.command
        // matches the snapshot path derived from data_dir, and the invariant
        // that we never touch settings.json holds through the real code path
        // (not just via apply_mcp_setup, which the unit tests exercise).
        assert!(
            paths.statusline_path.exists(),
            "statusline settings file must be written"
        );
        let statusline_json: serde_json::Value =
            serde_json::from_str(&fs::read_to_string(&paths.statusline_path).unwrap()).unwrap();
        assert_eq!(statusline_json["statusLine"]["type"], "command");
        let expected_command = format!(
            "dispatch statusline --snapshot '{}'",
            paths.budget_snapshot_path.display()
        );
        assert_eq!(
            statusline_json["statusLine"]["command"], expected_command,
            "command must point at the machine-wide snapshot location, which is \
             independent of the database's own directory"
        );
        assert!(
            !statusline_json["statusLine"]["command"]
                .as_str()
                .unwrap()
                .contains(&data_dir.path().display().to_string()),
            "the open database's directory must not reach the settings file \
             (docs/specs/dispatch.allium: \
             SnapshotLocationIsFixedNotDerivedFromTheOpenDatabase)"
        );
        assert!(
            !paths.claude_dir.join("settings.json").exists(),
            "run_setup_in must never create settings.json"
        );
    }

    #[tokio::test]
    async fn run_setup_in_chains_to_existing_status_line_without_touching_settings_json() {
        let root = tempfile::tempdir().unwrap();
        let data_dir = tempfile::tempdir().unwrap();
        let paths = setup_layout(root.path());
        let db = Database::open_in_memory().await.unwrap();

        fs::create_dir_all(&paths.claude_dir).unwrap();
        let settings_path = paths.claude_dir.join("settings.json");
        let settings_before =
            r#"{"statusLine":{"type":"command","command":"my-prev-line"}}"#.to_string();
        fs::write(&settings_path, &settings_before).unwrap();

        let runner = MockProcessRunner::new(vec![
            MockProcessRunner::ok_with_stdout(b"off\n"),
            MockProcessRunner::ok(),
        ]);
        let confirmer = FakeConfirmer::never();

        run_setup_in(
            &db,
            data_dir.path(),
            &paths,
            3142,
            true,
            &confirmer,
            &runner,
        )
        .await
        .unwrap();

        let statusline_json: serde_json::Value =
            serde_json::from_str(&fs::read_to_string(&paths.statusline_path).unwrap()).unwrap();
        let command = statusline_json["statusLine"]["command"]
            .as_str()
            .unwrap()
            .to_string();
        assert!(
            command.contains("--chain 'my-prev-line'"),
            "must chain to the discovered statusLine command, got: {command}"
        );

        let settings_after = fs::read_to_string(&settings_path).unwrap();
        assert_eq!(
            settings_before, settings_after,
            "settings.json must be byte-identical after run_setup_in — we only read it"
        );
    }

    #[tokio::test]
    async fn run_setup_in_user_declines_all_prompts() {
        let root = tempfile::tempdir().unwrap();
        let data_dir = tempfile::tempdir().unwrap();
        let paths = setup_layout(root.path());
        let db = Database::open_in_memory().await.unwrap();

        // focus-events OFF; no set-option because the user declines.
        let runner = MockProcessRunner::new(vec![MockProcessRunner::ok_with_stdout(b"off\n")]);
        // Decline MCP, plugin, and tmux prompts in order.
        let confirmer = FakeConfirmer::new(vec![false, false, false], vec![]);

        run_setup_in(
            &db,
            data_dir.path(),
            &paths,
            3142,
            false,
            &confirmer,
            &runner,
        )
        .await
        .unwrap();

        assert!(
            !paths.mcp_path.exists(),
            "declining must not write the MCP config"
        );
        let plugin_base = plugins::plugin_dir_under(&paths.claude_dir);
        assert!(
            !plugin_base.join(".claude-plugin/plugin.json").exists(),
            "declining must not install the plugin"
        );
        assert!(
            !paths.tmux_conf_path.exists(),
            "declining must not write .tmux.conf"
        );
        assert_eq!(confirmer.confirm_call_count(), 3, "one prompt per section");
    }

    #[tokio::test]
    async fn run_setup_in_writes_tmux_conf_when_focus_events_already_enabled() {
        let root = tempfile::tempdir().unwrap();
        let data_dir = tempfile::tempdir().unwrap();
        let paths = setup_layout(root.path());
        let db = Database::open_in_memory().await.unwrap();

        // focus-events already ON: only the query runs, no set-option.
        let runner = MockProcessRunner::new(vec![MockProcessRunner::ok_with_stdout(b"on\n")]);
        let confirmer = FakeConfirmer::never();

        run_setup_in(
            &db,
            data_dir.path(),
            &paths,
            3142,
            true,
            &confirmer,
            &runner,
        )
        .await
        .unwrap();

        let conf = fs::read_to_string(&paths.tmux_conf_path).unwrap();
        assert!(
            conf.contains("focus-events on"),
            "the already-enabled branch must still persist to .tmux.conf"
        );
    }

    #[tokio::test]
    async fn run_setup_in_is_idempotent_on_second_run() {
        let root = tempfile::tempdir().unwrap();
        let data_dir = tempfile::tempdir().unwrap();
        let paths = setup_layout(root.path());
        let db = Database::open_in_memory().await.unwrap();

        let runner1 = MockProcessRunner::new(vec![
            MockProcessRunner::ok_with_stdout(b"off\n"),
            MockProcessRunner::ok(),
        ]);
        run_setup_in(
            &db,
            data_dir.path(),
            &paths,
            3142,
            true,
            &FakeConfirmer::never(),
            &runner1,
        )
        .await
        .unwrap();

        // Second run: MCP already configured, plugin up to date, focus-events on.
        let runner2 = MockProcessRunner::new(vec![MockProcessRunner::ok_with_stdout(b"on\n")]);
        run_setup_in(
            &db,
            data_dir.path(),
            &paths,
            3142,
            true,
            &FakeConfirmer::never(),
            &runner2,
        )
        .await
        .unwrap();

        // Still exactly one seeded epic (seeding stayed idempotent).
        assert_eq!(db.list_epics().await.unwrap().len(), 1);
    }

    /// docs/specs/dispatch.allium: StatusLineDecorator,
    /// `SpawnSitesAndStartupNameTheSameConfigurationDirectory`. The writing
    /// side's half of the layout, pinned to written-out expectations.
    ///
    /// The spawn constant's half is pinned the same way, against its own
    /// hand-written literal, by `spawn_constant_has_exactly_one_space_between_flags`
    /// in `src/dispatch/tests.rs`. Both halves expand one shared definition, so
    /// a rename cannot move only one of them — but it must fail *visibly on
    /// both sides*, which is what these written-out expectations are for.
    /// Deriving them from the shared tokens instead would make them agree with
    /// the code no matter what it said.
    ///
    /// The `$HOME` assertion is the other half of the agreement, and the one
    /// thing here that is not compile-time: the spawn constant names the
    /// directory as `~/…`, which the launching shell resolves under the home
    /// directory. It cannot see a lookup that gained a *second* input — that
    /// would match wherever the new input is unset, CI included, and part only
    /// for the operators who set it.
    #[test]
    fn the_configuration_layout_is_what_the_spawn_constant_names() {
        let home = std::env::var("HOME").expect("$HOME must be set");
        assert_eq!(
            claude_dir().expect("$HOME must be set"),
            std::path::Path::new(&home).join(".claude"),
            "the production lookup must be the home directory plus the name \
             the spawn constant expands, and nothing else"
        );

        let claude_dir = std::path::Path::new("/h/.claude");
        assert_eq!(
            statusline::settings_path(claude_dir),
            std::path::Path::new("/h/.claude/dispatch-statusline.json")
        );
        assert_eq!(
            plugins::plugin_dir_under(claude_dir),
            std::path::Path::new("/h/.claude/plugins/local/dispatch")
        );
    }

    /// docs/specs/dispatch.allium: StatusLineDecorator,
    /// `SpawnSitesAndStartupNameTheSameConfigurationDirectory`. The plugin
    /// directory's writer-side link.
    ///
    /// `dispatch setup` — not startup — is what installs the plugin, so the
    /// chain for that half runs through `SetupPaths`, not `StartupPaths`. This
    /// pins that the flow which actually writes there resolves the shared
    /// layout rather than one of its own.
    ///
    /// Every other test of this flow injects temp directories (that is what
    /// `SetupPaths` exists for), so without this the production resolution is
    /// exercised by nothing.
    #[test]
    fn setup_paths_resolve_to_the_shared_configuration_directory() {
        let paths = SetupPaths::resolve().expect("$HOME must be set");
        let expected = claude_dir().expect("$HOME must be set");

        assert_eq!(
            paths.claude_dir, expected,
            "setup must install under the same configuration directory the \
             spawn constant names"
        );
        assert_eq!(
            paths.statusline_path,
            statusline::settings_path(&expected),
            "the settings file setup writes and the one startup rewrites must \
             be the same file"
        );
    }
}
