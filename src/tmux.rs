use anyhow::{bail, Context, Result};
use std::process::Output;

use crate::models::TmuxWindow;
use crate::process::{stderr_str, stdout_str, ProcessRunner, SUBPROCESS_TIMEOUT};

// ---------------------------------------------------------------------------
// Shared checked-run helper
// ---------------------------------------------------------------------------

/// Build the consistent `"tmux {context} failed with status {status}[: {stderr}]"`
/// error for a failed [`Output`].
fn checked_error(context: &str, output: &Output) -> anyhow::Error {
    let stderr = stderr_str(output);
    if stderr.is_empty() {
        anyhow::anyhow!("tmux {context} failed with status {}", output.status)
    } else {
        anyhow::anyhow!(
            "tmux {context} failed with status {}: {}",
            output.status,
            stderr
        )
    }
}

/// Turn a raw [`Output`] into a [`Result`], via [`checked_error`] on failure.
/// Shared by [`run_checked`] and [`run_checked_timeout`], which differ only in
/// which `ProcessRunner` method produced the `Output`.
fn check_output(output: Output, context: &str) -> Result<Output> {
    if !output.status.success() {
        return Err(checked_error(context, &output));
    }
    Ok(output)
}

/// Run `tmux` with `args`, returning the raw [`Output`] on success and a
/// consistent checked-run error (see [`checked_error`]) otherwise.
fn run_checked(runner: &dyn ProcessRunner, args: &[&str], context: &str) -> Result<Output> {
    check_output(runner.run("tmux", args)?, context)
}

/// Like [`run_checked`], but returns trimmed stdout as a `String` instead of
/// the raw `Output` — for calls whose stdout is the actual result.
fn run_checked_stdout(runner: &dyn ProcessRunner, args: &[&str], context: &str) -> Result<String> {
    let output = run_checked(runner, args, context)?;
    Ok(stdout_str(&output))
}

/// Like [`run_checked`], but bounds the subprocess with [`SUBPROCESS_TIMEOUT`]
/// so a hung tmux server cannot park the calling thread forever.
///
/// Used by [`new_window`], [`set_window_dispatch_dir`] and
/// [`ensure_split_hook`] — and so by every one of their callers, including
/// `provision_worktree`'s `post_add` step and `resume_agent`
/// (`src/dispatch/agents.rs`) — which is what closes
/// #4202. Every other tmux call in this module still goes through the
/// unbounded [`run_checked`]; if one of those turns out to need the same
/// treatment it should get its own pass rather than folding in here.
fn run_checked_timeout(runner: &dyn ProcessRunner, args: &[&str], context: &str) -> Result<Output> {
    check_output(
        runner.run_with_timeout("tmux", args, SUBPROCESS_TIMEOUT)?,
        context,
    )
}

// ---------------------------------------------------------------------------
// Exact window-name targeting
// ---------------------------------------------------------------------------

/// `list-panes` output format for [`window_target`]: fixed-width fields first so
/// the window name — which may contain spaces — is the parseable remainder.
pub(crate) const WINDOW_PANE_FORMAT: &str = "#{pane_active} #{pane_id} #{window_name}";

/// Opening of the `-f` filter [`window_filter`] builds. Split out so
/// [`window_name_in_lookup`] can invert it.
const WINDOW_FILTER_PREFIX: &str = "#{==:#{window_name},";

/// A `list-panes -f` filter selecting panes whose window name equals `window`.
/// `#{==:…}` compares in tmux, so no prefix matching is involved.
fn window_filter(window: &str) -> String {
    format!("{WINDOW_FILTER_PREFIX}{window}}}")
}

/// The window name a [`window_target`] lookup is asking about, given its argv —
/// the inverse of [`window_filter`]. `None` when `args` is not such a lookup.
///
/// Exists for `MockProcessRunner`, which answers the lookup without a tmux
/// server and so needs to know which window is being asked for. Keeping the
/// construction and the inversion adjacent is what stops them drifting apart.
#[cfg(any(test, feature = "test-support"))]
pub(crate) fn window_name_in_lookup<'a>(args: &[&'a str]) -> Option<&'a str> {
    match args {
        ["list-panes", "-a", "-f", filter, "-F", format] if *format == WINDOW_PANE_FORMAT => {
            filter.strip_prefix(WINDOW_FILTER_PREFIX)?.strip_suffix('}')
        }
        _ => None,
    }
}

/// Whether `target` is already unambiguous and must reach tmux untouched:
/// a pane ID (`%N`), or the empty string, which is tmux's "current window" and
/// is part of [`rename_window`]'s documented contract.
///
/// These are exactly the two strings [`TmuxWindow::parse`] rejects, and the
/// pane-ID half is [`crate::models::is_pane_id`] so the two cannot drift: a
/// string is either a window name or a resolved target, never both.
fn is_resolved_target(target: &str) -> bool {
    target.is_empty() || crate::models::is_pane_id(target)
}

/// Resolve a tmux window *name* to the pane ID of that window's active pane,
/// matching the name **exactly**. Pane IDs pass through unchanged.
///
/// # Why every window target goes through here
///
/// tmux resolves a `-t <window-name>` target by exact match and then by
/// **prefix**. Dispatch names windows `task-<id>`, so once ids reach the
/// thousands one task's name is a prefix of another's — `task-378` and
/// `task-3782`. When the intended window is absent (killed, crashed, cleaned
/// up) and a longer-named sibling is alive, tmux silently redirects the
/// operation to the sibling: `send-keys` types the agent command into another
/// task's live Claude session, and `kill-window` destroys another task's agent.
/// Both are the same class of defect as issue #3781. See the
/// `TmuxWindowTargetedExactly` invariant in docs/specs/dispatch.allium.
///
/// # Why a pane ID rather than tmux's `=` sigil
///
/// tmux's documented exact-match sigil (`-t '=task-4'`) is not a general
/// answer. Verified against tmux 3.5a: `send-keys` rejects it outright
/// (`can't find pane: =task-42`, even when that window exists), `set-option -w`
/// rejects it (`no such window`), and `display-message -p` accepts it while
/// printing nothing and **exiting zero** — the worst outcome, a silently empty
/// pane ID. It only works for the target-*window* commands. A pane ID, by
/// contrast, is accepted by every command this module issues and cannot be
/// prefix-matched, so one mechanism covers all of them.
///
/// # Errors
///
/// Absent name, or two windows sharing it. Ambiguity is refused rather than
/// resolved arbitrarily: tmux already refuses it for `kill-window` and
/// `select-window`, but silently picks one for `set-option -w`. Refusing
/// uniformly is what [`set_window_dispatch_dir`]'s stderr sniff for
/// "ambiguous" used to approximate for that one call.
///
/// `pub(crate)` rather than private: [`crate::notify::notify_tmux`] resolves
/// a window once and reuses the resolved pane id for both its `capture-pane`
/// and `send-keys` calls, rather than letting each of those helpers
/// independently re-run this lookup's `list-panes` subprocess for the exact
/// same window on every notification.
///
/// # Why this one still takes `&str`
///
/// It accepts *either* a window name *or* an already-resolved target
/// ([`is_resolved_target`] — a pane ID, or `""` for the current window), and
/// those two are precisely what [`TmuxWindow::parse`] rejects. Typing the
/// parameter would either swallow that distinction or force callers to fake a
/// name for a pane they already hold. The same reasoning keeps
/// [`rename_window`]'s `target`, [`pane_ids_with_option`],
/// [`pane_ids_with_option_value`] and the `split_window_*_running` pair on
/// `&str` — each is documented as taking a name *or* a pane id. Every helper
/// whose parameter can only ever be a name takes a [`TmuxWindow`].
pub(crate) fn window_target(window: &str, runner: &dyn ProcessRunner) -> Result<String> {
    if is_resolved_target(window) {
        return Ok(window.to_string());
    }
    // A failed query means there are no windows to match — no server running,
    // or tmux unreachable. Same soft-fail as `list_all_window_names`, whose
    // `-a` rationale this shares: works inside or outside tmux, and finds
    // windows living in a session other than the current one.
    let filter = window_filter(window);
    let output = runner.run(
        "tmux",
        &["list-panes", "-a", "-f", &filter, "-F", WINDOW_PANE_FORMAT],
    )?;
    let listing = if output.status.success() {
        String::from_utf8_lossy(&output.stdout).into_owned()
    } else {
        String::new()
    };

    // The `name == window` check is not redundant with the `-f` filter: the
    // filter interpolates the name into a tmux format string, so a name
    // containing `,` or `}` could confuse `#{==:…}`. Re-comparing here makes
    // correctness independent of that — a crafted name can at worst produce a
    // miss (which fails safe), never a match on the wrong window.
    //
    // Filtering on the *active* pane yields exactly one row per window, so a
    // second match means two windows share the name — not two panes in one.
    let mut matches = listing.lines().filter_map(|line| {
        let mut parts = line.splitn(3, ' ');
        let active = parts.next()?;
        let pane_id = parts.next()?;
        let name = parts.next()?.trim_end();
        (active == "1" && name == window).then(|| pane_id.to_string())
    });

    let Some(pane_id) = matches.next() else {
        bail!("no tmux window named '{window}'");
    };
    if matches.next().is_some() {
        bail!(
            "multiple tmux windows named '{}' exist — close the duplicate windows before dispatching",
            window
        );
    }
    Ok(pane_id)
}

// ---------------------------------------------------------------------------
// Public API
// ---------------------------------------------------------------------------

/// Create a new tmux window with the given name, starting in `working_dir`.
pub fn new_window(name: &TmuxWindow, working_dir: &str, runner: &dyn ProcessRunner) -> Result<()> {
    run_checked_timeout(
        runner,
        &["new-window", "-d", "-n", name.as_str(), "-c", working_dir],
        "new-window",
    )?;
    Ok(())
}

/// Create a new tmux window running the given command as separate argv
/// elements (no shell wrapping). When the command exits, the window closes.
///
/// `-d` keeps current focus; callers use [`select_window`] afterwards to
/// switch to the new window if desired.
pub fn new_window_running(
    name: &TmuxWindow,
    working_dir: &str,
    command: &[&str],
    runner: &dyn ProcessRunner,
) -> Result<()> {
    if command.is_empty() {
        bail!("new_window_running: command must not be empty");
    }
    let mut args: Vec<&str> = vec![
        "new-window",
        "-d",
        "-n",
        name.as_str(),
        "-c",
        working_dir,
        "--",
    ];
    args.extend(command.iter().copied());
    run_checked(runner, &args, "new-window")?;
    Ok(())
}

/// Send literal text to a tmux window, then press Enter.
///
/// Uses `-l` to prevent tmux from interpreting escape sequences in the text.
/// Enter is sent as a separate `send-keys` call without `-l`.
///
/// The window name is resolved by [`window_target`] first — an absent window
/// must fail here, never fall through to a prefix-matched sibling, because the
/// payload is typed into whatever Claude session receives it.
pub fn send_keys(window: &TmuxWindow, keys: &str, runner: &dyn ProcessRunner) -> Result<()> {
    let target = window_target(window.as_str(), runner)?;
    send_keys_at(&target, keys, runner)
}

/// [`send_keys`] against a target [`window_target`] has already resolved.
///
/// `pub(crate)` for [`crate::notify::notify_tmux`] alone, which resolves one
/// window and drives both this and [`capture_pane`] from that single pane
/// id rather than paying for the `list-panes` lookup twice per notification.
/// Everything else goes through the [`TmuxWindow`]-typed wrappers, so the
/// resolution step cannot be skipped by accident.
pub(crate) fn send_keys_at(target: &str, keys: &str, runner: &dyn ProcessRunner) -> Result<()> {
    run_checked(
        runner,
        &["send-keys", "-t", target, "-l", keys],
        "send-keys -l",
    )?;
    run_checked(
        runner,
        &["send-keys", "-t", target, "Enter"],
        "send-keys Enter",
    )?;
    Ok(())
}

/// Return true if a tmux window with the given name currently exists,
/// searching across all sessions. Built on [`list_all_window_names`], which
/// issues the identical `list-windows -a` query — same `-a` rationale: works
/// whether the caller is inside or outside tmux, and finds windows living in
/// a session other than the current/attached one.
pub fn has_window(window: &TmuxWindow, runner: &dyn ProcessRunner) -> Result<bool> {
    Ok(list_all_window_names(runner)?
        .iter()
        .any(|n| n == window.as_str()))
}

/// Whether `window` should be treated as alive: `true` when [`has_window`]
/// finds it, but also `true` when the query itself fails.
///
/// A query failure (tmux not reachable, transient error) is deliberately
/// mapped to "present" rather than "absent" — the callers of this helper use
/// the result to decide whether to treat a task's agent as crashed or the
/// main session as gone, and a false "absent" would trigger a spurious
/// re-dispatch or crash notification from a hiccup that has nothing to do
/// with the window's actual state. See `has_window`'s other callers
/// (`kill_window_if_present`) for the opposite default, which applies where
/// the gated action is itself destructive.
pub fn has_window_or_assume_present(window: &TmuxWindow, runner: &dyn ProcessRunner) -> bool {
    has_window(window, runner).unwrap_or(true)
}

/// Kill `window` if a live check finds it present.
///
/// Unlike [`has_window_or_assume_present`], a query failure here is logged
/// and treated as "nothing to kill" rather than propagated or assumed
/// present — attempting a `kill-window` against a query we couldn't
/// validate risks a hard failure that would abort the rest of the
/// caller's cleanup (e.g. removing the git worktree). Skipping the kill is
/// the safe choice when we can't tell whether the window still exists.
pub fn kill_window_if_present(window: &TmuxWindow, runner: &dyn ProcessRunner) -> Result<()> {
    match has_window(window, runner) {
        Ok(true) => kill_window(window, runner),
        Ok(false) => Ok(()),
        Err(e) => {
            tracing::warn!("could not check tmux window '{window}' before kill: {e}");
            Ok(())
        }
    }
}

/// Run a server-wide `list-*` query and collect its non-empty output lines.
///
/// Shared by [`list_all_window_names`] and [`list_all_pane_ids`] so the
/// non-obvious half of the contract — a failed call means "no server running",
/// not an error — lives in one place.
fn list_all(args: &[&str], runner: &dyn ProcessRunner) -> Result<Vec<String>> {
    let output = runner.run("tmux", args)?;
    if !output.status.success() {
        return Ok(vec![]);
    }
    let lines = String::from_utf8_lossy(&output.stdout)
        .lines()
        .map(|l| l.trim().to_string())
        .filter(|l| !l.is_empty())
        .collect();
    Ok(lines)
}

/// List the names of all tmux windows across all sessions.
///
/// Uses `-a` so the query works whether the caller is inside or outside tmux.
/// Returns an empty vec (not an error) when no tmux server is running.
pub fn list_all_window_names(runner: &dyn ProcessRunner) -> Result<Vec<String>> {
    list_all(&["list-windows", "-a", "-F", "#{window_name}"], runner)
}

/// Kill the tmux window with the given name.
///
/// The name is resolved by [`window_target`] first, so an absent window fails
/// instead of destroying a prefix-matched sibling's agent. That makes this safe
/// on its own; [`kill_window_if_present`] remains the wrapper for callers whose
/// cleanup must not abort when the window is simply already gone.
pub fn kill_window(window: &TmuxWindow, runner: &dyn ProcessRunner) -> Result<()> {
    let target = window_target(window.as_str(), runner)?;
    run_checked(runner, &["kill-window", "-t", &target], "kill-window")?;
    Ok(())
}

/// Switch the active tmux window to the one with the given name.
pub fn select_window(window: &TmuxWindow, runner: &dyn ProcessRunner) -> Result<()> {
    let target = window_target(window.as_str(), runner)?;
    run_checked(runner, &["select-window", "-t", &target], "select-window")?;
    Ok(())
}

/// Store the worktree path as a per-window user option so the session-level
/// `after-split-window` hook (installed by [`ensure_split_hook`]) can look it
/// up when a split happens in this window.
///
/// A prefix-matched target here would leak one task's worktree path onto
/// another task's window, sending that window's future splits into the wrong
/// worktree — so the name is resolved by [`window_target`] first. That resolver
/// also owns the duplicate-name refusal this function used to approximate by
/// sniffing tmux's stderr for "ambiguous"; `set-option -w` does not actually
/// report ambiguity, it silently picks one of the duplicates.
pub fn set_window_dispatch_dir(
    window: &TmuxWindow,
    working_dir: &str,
    runner: &dyn ProcessRunner,
) -> Result<()> {
    let target = window_target(window.as_str(), runner)?;
    run_checked_timeout(
        runner,
        &[
            "set-option",
            "-w",
            "-t",
            &target,
            "@dispatch_dir",
            working_dir,
        ],
        "set-option",
    )?;
    Ok(())
}

/// Install a single session-level `after-split-window` hook that reads the
/// `@dispatch_dir` window option. If the option is set on the window being split
/// *and* the new pane did not already start in that directory, the pane is
/// respawned there; otherwise nothing happens.
///
/// This is idempotent — calling it multiple times replaces the same hook.
///
/// # Why this hook exists (do not delete it)
///
/// It is load-bearing, not a convenience. tmux never inherits the split pane's
/// directory: a `split-window` invoked by an *external CLI client* — which is how
/// dispatch, or any script, shells out to tmux — starts the new pane in the
/// **invoking process's** cwd, and one the user triggers from an attached client
/// starts it in the **session's** cwd. Neither is the worktree. Without this hook
/// every user split inside an agent window lands outside it. Any refactor that
/// removes the hook must first replace that guarantee by other means. Full
/// history (issue #231, commit 8bf36803) and the behavioural contract live in the
/// `AgentWindowSplitStartsInTaskWorktree` rule in docs/specs/split-pane.allium.
///
/// # Why the hook is a *fallback*
///
/// Dispatch's own splits name their start directory
/// ([`split_window_horizontal_running`]'s `start_dir` → `split-window -c`), so
/// they are correct at creation. The `#{pane_start_path}` half of the guard makes
/// the hook skip them, which matters: `respawn-pane` would restart the companion
/// process those panes were created to run.
///
/// # Why the correction is not `send-keys`
///
/// It used to be — `cd <dir>` + Enter typed at the new pane — and that is only
/// correct if a shell happens to be reading. The agent-tree companion pane exits
/// on `q`, so any worktree path containing that letter closed the pane the moment
/// it opened. See `SplitDirectoryIsNeverKeystrokes` in the same spec.
///
/// # Why `respawn-pane` must carry `-t #{pane_id}`
///
/// The target is mandatory, not decorative. `run-shell -bC` loses the enclosing
/// command's target context, so an untargeted command falls back to the session's
/// **active** pane. Because dispatch opens the agent-tree companion pane by
/// splitting the agent window in the background (`spawn_agent_tree_pane`) while
/// the board is still focused, the untargeted form of the old hook typed
/// `cd <worktree>` into the board TUI, where `c` fired the Copy-Task keybinding
/// (#3781). `#{pane_id}` is expanded in the hook's own context — the newly
/// created pane. Pane routing is only observable against a real tmux server, so
/// it is covered by tests/tmux_split_hook.rs rather than by this file's
/// mock-level test.
/// The tmux format [`ensure_split_hook`] tests before correcting a pane: true
/// when the window carries `@dispatch_dir` *and* the new pane did not already
/// start there.
///
/// Named rather than inlined so that a test can evaluate the real guard against
/// a real pane (`display-message -p`) instead of restating it — whether a pane is
/// selected for correction is the whole point of the hook, and a restated copy
/// could agree with itself while disagreeing with what ships.
pub const SPLIT_NEEDS_CORRECTION: &str =
    "#{&&:#{@dispatch_dir},#{!=:#{pane_start_path},#{@dispatch_dir}}}";

pub fn ensure_split_hook(runner: &dyn ProcessRunner) -> Result<()> {
    // if-shell -F only format-expands its test argument, NOT the branch command.
    // respawn-pane doesn't expand formats either, so we wrap it in run-shell -C
    // which does expand #{…} before executing the tmux command.
    //
    // The innermost quotes around #{@dispatch_dir} are load-bearing: without
    // them a worktree path containing a space silently resolves to $HOME.
    let hook_cmd = format!(
        "if-shell -F '{SPLIT_NEEDS_CORRECTION}' \
         'run-shell -bC \"respawn-pane -k -t #{{pane_id}} -c \\\"#{{@dispatch_dir}}\\\"\"'"
    );
    run_checked_timeout(
        runner,
        &["set-hook", "after-split-window", &hook_cmd],
        "set-hook",
    )?;
    Ok(())
}

/// Read back a window's `@dispatch_dir` — the worktree path
/// [`set_window_dispatch_dir`] stored on it. `None` when the option is unset,
/// which is the normal answer for the board and the main session.
///
/// Lets the callers that hold only a window name (the agent-tree toggle and
/// resync paths) name a start directory for a split, without threading the
/// worktree through tmux and back out of the database. The window option is
/// already the single source of truth the correction hook reads.
///
/// `window` goes through [`window_target`] like every other name-taking helper
/// here, so a prefix-matched sibling window cannot answer for it.
pub fn window_dispatch_dir(
    window: &TmuxWindow,
    runner: &dyn ProcessRunner,
) -> Result<Option<String>> {
    let target = window_target(window.as_str(), runner)?;
    // -q so an unset option is an empty answer rather than an error.
    let dir = run_checked_stdout(
        runner,
        &["show-options", "-wqv", "-t", &target, "@dispatch_dir"],
        "show-options",
    )?;
    Ok((!dir.is_empty()).then_some(dir))
}

/// Check whether tmux has `focus-events` enabled globally.
///
/// Returns `false` if the option is off or if the query fails (e.g. not
/// running inside tmux).
pub fn focus_events_enabled(runner: &dyn ProcessRunner) -> bool {
    let Ok(output) = runner.run("tmux", &["show-options", "-gv", "focus-events"]) else {
        return false;
    };
    if !output.status.success() {
        return false;
    }
    let stdout = String::from_utf8_lossy(&output.stdout);
    stdout.trim() == "on"
}

/// Enable tmux `focus-events` globally.
///
/// This is idempotent — calling it when already enabled is a no-op.
pub fn set_focus_events(runner: &dyn ProcessRunner) -> Result<()> {
    run_checked(
        runner,
        &["set-option", "-g", "focus-events", "on"],
        "set-option focus-events",
    )?;
    Ok(())
}

/// Path to the user's `~/.tmux.conf` under a given home directory.
///
/// Owned here so callers don't re-derive the `$HOME`-relative location, and
/// takes the home directory rather than reading it so this is not a second
/// reader with its own idea of what an unavailable `$HOME` means — see
/// `crate::setup::home_dir` and docs/specs/dispatch.allium:
/// `AnUnavailableHomeDirectoryIsAFailureNotAPath`. This one *writes* the file
/// it names, so an empty `$HOME` resolving here would drop a `.tmux.conf` into
/// whatever directory the process happened to be started from.
pub(crate) fn tmux_conf_path_in(home: &std::path::Path) -> std::path::PathBuf {
    home.join(".tmux.conf")
}

/// [`tmux_conf_path_in`] against the operator's own home directory.
pub(crate) fn tmux_conf_path() -> Result<std::path::PathBuf> {
    Ok(tmux_conf_path_in(&crate::setup::home_dir()?))
}

pub(crate) fn write_focus_events_to_tmux_conf_at(path: &std::path::Path) -> Result<()> {
    let existing = if path.exists() {
        std::fs::read_to_string(path).context("failed to read .tmux.conf")?
    } else {
        String::new()
    };
    if existing.contains("focus-events on") {
        return Ok(());
    }
    let addition = if existing.ends_with('\n') || existing.is_empty() {
        "set -g focus-events on\n".to_string()
    } else {
        "\nset -g focus-events on\n".to_string()
    };
    std::fs::write(path, existing + &addition).context("failed to write .tmux.conf")?;
    Ok(())
}

/// Return the name of the currently active tmux window.
pub fn current_window_name(runner: &dyn ProcessRunner) -> Result<String> {
    run_checked_stdout(runner, &["display-message", "-p", "#W"], "display-message")
}

/// Rename a tmux window. `target` may be a window name, a pane ID, or `""` to
/// rename the current window.
///
/// Only `target` is resolved by [`window_target`] — never `new_name`, which is
/// a name being assigned and by definition need not exist yet.
pub fn rename_window(
    target: &str,
    new_name: &TmuxWindow,
    runner: &dyn ProcessRunner,
) -> Result<()> {
    let target = window_target(target, runner)?;
    run_checked(
        runner,
        &["rename-window", "-t", &target, new_name.as_str()],
        "rename-window",
    )?;
    Ok(())
}

/// Bind a tmux key (requires the tmux prefix first) to a command string.
pub fn bind_key(key: &str, command: &str, runner: &dyn ProcessRunner) -> Result<()> {
    run_checked(runner, &["bind-key", key, command], "bind-key")?;
    Ok(())
}

/// Remove a tmux key binding (previously registered with `bind-key`).
pub fn unbind_key(key: &str, runner: &dyn ProcessRunner) -> Result<()> {
    run_checked(runner, &["unbind-key", key], "unbind-key")?;
    Ok(())
}

// ---------------------------------------------------------------------------
// Split mode operations
// ---------------------------------------------------------------------------

/// Return the tmux pane ID of the current pane (e.g. "%42").
pub fn current_pane_id(runner: &dyn ProcessRunner) -> Result<String> {
    run_checked_stdout(
        runner,
        &["display-message", "-p", "#{pane_id}"],
        "display-message",
    )
}

/// Create a horizontal split (right pane) at 40% width, keeping focus on the
/// left pane. Returns the new pane's ID.
pub fn split_window_horizontal(target_pane: &str, runner: &dyn ProcessRunner) -> Result<String> {
    run_checked_stdout(
        runner,
        &[
            "split-window",
            "-h",
            "-d",
            "-l",
            "40%",
            "-t",
            target_pane,
            "-P",
            "-F",
            "#{pane_id}",
        ],
        "split-window",
    )
}

/// Create a horizontal split (left pane) at `size_pct`% width, running the
/// given command as separate argv elements (no shell wrapping) in the new
/// pane, following the same "create + immediately run a command" shape
/// [`new_window_running`] establishes for window creation. Keeps focus on
/// the target pane. Returns the new pane's ID.
///
/// Sibling of [`split_window_horizontal`] (hardcoded 40%, no command, used
/// by the board's own split-pane feature) — this one is for spawning a
/// companion process (e.g. `dispatch agent-tree <task_id>`) narrower than
/// that split, since the target pane's own output still needs the room.
///
/// `target` may be a pane ID or a window *name*: `spawn_agent_tree_pane`
/// (src/dispatch/agents.rs) passes the agent's `task-<id>` window. Names go
/// through [`window_target`], so the companion pane cannot be opened inside a
/// prefix-matched sibling's window; pane IDs pass through untouched.
///
/// `start_dir` becomes `split-window -c`, the new pane's working directory.
/// Naming it is what makes the pane correct at creation instead of leaving it to
/// the `after-split-window` correction hook, which would restart `command` —
/// see [`ensure_split_hook`]. `None` leaves the directory to tmux, which picks
/// the invoking process's cwd for an external CLI client like this one.
pub fn split_window_horizontal_running(
    target: &str,
    size_pct: u8,
    command: &[&str],
    start_dir: Option<&str>,
    runner: &dyn ProcessRunner,
) -> Result<String> {
    if command.is_empty() {
        bail!("split_window_horizontal_running: command must not be empty");
    }
    let target_pane = window_target(target, runner)?;
    let size_arg = format!("{size_pct}%");
    let mut args: Vec<&str> = vec![
        "split-window",
        "-h",
        "-b",
        "-d",
        "-l",
        &size_arg,
        "-t",
        &target_pane,
        "-P",
        "-F",
        "#{pane_id}",
    ];
    if let Some(dir) = start_dir {
        args.extend(["-c", dir]);
    }
    // `--` last: everything after it is the pane's command, not an option.
    args.push("--");
    args.extend(command.iter().copied());
    run_checked_stdout(runner, &args, "split-window")
}

/// Create a pane spanning the **full window width** below `target`, taking
/// `size_pct`% of the window's height, running `command` as separate argv
/// elements (no shell) with `cwd` as its start directory. Keeps focus where it
/// is. Returns the new pane's ID.
///
/// The third split helper in this module, and each difference is load-bearing.
/// [`split_window_horizontal`] (40%, right, no command) serves the board's
/// split-pane feature; [`split_window_horizontal_running`] (left, `size_pct`,
/// command) opens the agent-tree companion pane. This one opens the editor pane
/// *from* that companion pane, where:
///
/// * `-f` makes the new pane span the window rather than subdividing the pane it
///   was split from — the companion pane is the natural target and it is the
///   narrow one, so without `-f` the editor would inherit its 30% column.
/// * `-c` is passed explicitly rather than relying on [`ensure_split_hook`]'s
///   `@dispatch_dir` `cd`: that hook *types* `cd <dir>` into the new pane, which
///   works for a shell and would land in the editor's own input here.
/// * Focus stays put (`-d`) so the user can keep browsing the tree — see
///   `OpenAgentTreeFileInEditor` in docs/specs/agent-tree.allium.
pub fn split_window_full_below_running(
    target: &str,
    size_pct: u8,
    cwd: &str,
    command: &[&str],
    runner: &dyn ProcessRunner,
) -> Result<String> {
    if command.is_empty() {
        bail!("split_window_full_below_running: command must not be empty");
    }
    let target_pane = window_target(target, runner)?;
    let size_arg = format!("{size_pct}%");
    let mut args: Vec<&str> = vec![
        "split-window",
        "-v",
        "-f",
        "-d",
        "-l",
        &size_arg,
        "-t",
        &target_pane,
        "-c",
        cwd,
        "-P",
        "-F",
        "#{pane_id}",
        "--",
    ];
    args.extend(command.iter().copied());
    run_checked_stdout(runner, &args, "split-window")
}

/// Replace what is running in `pane_id` with `command` (argv, no shell), started
/// in `cwd`. `-k` kills the pane's current process first.
///
/// The pane object itself survives, which is what makes this the way the editor
/// pane shows a second file: it keeps its geometry and its pane options, so
/// nothing has to be re-marked, and focus is untouched. Sibling of
/// [`respawn_pane`], which respawns a plain shell in place.
pub fn respawn_pane_running(
    pane_id: &str,
    cwd: &str,
    command: &[&str],
    runner: &dyn ProcessRunner,
) -> Result<()> {
    if command.is_empty() {
        bail!("respawn_pane_running: command must not be empty");
    }
    let mut args: Vec<&str> = vec!["respawn-pane", "-k", "-c", cwd, "-t", pane_id, "--"];
    args.extend(command.iter().copied());
    run_checked(runner, &args, "respawn-pane")?;
    Ok(())
}

/// Pane option marking a pane **dispatch itself created** in an agent window,
/// valued with what that pane is for: [`PANE_ROLE_AGENT_TREE`] or
/// [`PANE_ROLE_EDITOR`].
///
/// Lives here rather than with either creator because unrelated modules must
/// agree on it forever: `dispatch::agents` writes the tree role and reads it back
/// to toggle and resync that pane, `agent_tree_editor` writes and reads the
/// editor role to reuse its pane, and `dispatch::companion_pane_ids` reads the
/// option's mere *presence* to drain every dispatch-created pane when an agent
/// window is pinned into the board. The *policy* (when to split, when to replace)
/// stays with each creator; the vocabulary is shared infrastructure.
///
/// One option with a role value, rather than one option per pane kind, is what
/// makes that third question answerable in a single lookup — and answerable for
/// a future role without teaching the drain about it. See
/// docs/specs/agent-tree.allium's `HideAgentTreePane` ("How the companion pane
/// is identified") and `OneEditorPanePerAgentWindow`.
pub const PANE_ROLE_OPTION: &str = "@dispatch_pane_role";

/// [`PANE_ROLE_OPTION`] value for the `dispatch agent-tree` companion pane.
pub const PANE_ROLE_AGENT_TREE: &str = "agent_tree";

/// [`PANE_ROLE_OPTION`] value for the editor pane opened from that companion pane.
pub const PANE_ROLE_EDITOR: &str = "editor";

/// Set a pane-scoped tmux user option (`@name`). The pane-level sibling of
/// [`set_window_dispatch_dir`]'s `set-option -w`.
///
/// Takes a pane **id** only, never a window name: a pane option is how dispatch
/// marks a pane it created, and a marker written to the wrong pane is worse than
/// no marker at all.
pub fn set_pane_option(
    pane_id: &str,
    option: &str,
    value: &str,
    runner: &dyn ProcessRunner,
) -> Result<()> {
    run_checked(
        runner,
        &["set-option", "-p", "-t", pane_id, option, value],
        "set-option",
    )?;
    Ok(())
}

/// Move a tmux window into the current window as a right pane (40% width).
/// Returns the new pane's ID.
pub fn join_pane(
    source_window: &TmuxWindow,
    target_pane: &str,
    runner: &dyn ProcessRunner,
) -> Result<String> {
    // Resolving the source window by exact name *is* the pane-ID lookup this
    // used to do with a separate `display-message`: it returns the window's
    // active pane, which is the pane join-pane moves, and pane IDs are
    // preserved across the move (join-pane has no -P/-F to print the result).
    // Passing the ID rather than the name also keeps a prefix-matched sibling
    // from being torn out of its own window and into the board.
    let pane_id = window_target(source_window.as_str(), runner)?;

    run_checked(
        runner,
        &[
            "join-pane",
            "-h",
            "-d",
            "-s",
            &pane_id,
            "-t",
            target_pane,
            "-l",
            "40%",
        ],
        "join-pane",
    )?;
    Ok(pane_id)
}

/// Break a pane out into its own tmux window with the given name.
pub fn break_pane_to_window(
    pane_id: &str,
    window_name: &TmuxWindow,
    runner: &dyn ProcessRunner,
) -> Result<()> {
    run_checked(
        runner,
        &[
            "break-pane",
            "-d",
            "-s",
            pane_id,
            "-n",
            window_name.as_str(),
        ],
        "break-pane",
    )?;
    Ok(())
}

/// Kill a specific tmux pane by ID.
pub fn kill_pane(pane_id: &str, runner: &dyn ProcessRunner) -> Result<()> {
    run_checked(runner, &["kill-pane", "-t", pane_id], "kill-pane")?;
    Ok(())
}

/// Replace the content of a pane with a fresh shell, preserving the pane itself.
pub fn respawn_pane(pane_id: &str, runner: &dyn ProcessRunner) -> Result<()> {
    run_checked(
        runner,
        &["respawn-pane", "-k", "-t", pane_id],
        "respawn-pane",
    )?;
    Ok(())
}

/// Get the pane ID of a window's active pane, matching the window name exactly.
/// Errors when `window` does not exist.
///
/// This is the public face of [`window_target`]. It replaced a
/// `display-message -p -t <window> '#{pane_id}'` call, which was wrong in two
/// compounding ways. It prefix-matched the window name, so it could hand back a
/// *different* task's pane ID — one that then propagated into swaps, splits and
/// the split-pane's tracked pane. And for a window that genuinely did not exist
/// it exited 0 printing an empty string rather than failing, so the miss
/// resolved to `Ok("")` — and `swap-pane -s ''` also exits 0, so the empty id
/// propagated silently until some later command failed with a misleading
/// message. Resolving through a `list-panes` row removes both: there is no row
/// to misattribute, and no row at all means no window. Verified against tmux
/// 3.5a.
pub fn pane_id_for_window(window: &TmuxWindow, runner: &dyn ProcessRunner) -> Result<String> {
    window_target(window.as_str(), runner)
}

/// Atomically swap the contents of two panes without changing the layout.
/// `-d` keeps focus on the current pane.
///
/// Pass pane **ids**, not `<window>.<index>` targets: such a target is wrong in
/// both halves. The index shifts with the user's `pane-base-index` and is
/// renumbered by a `-b` split, so it can miss or hit the wrong pane; the window
/// name prefix-matches (see [`window_target`]), so it can address the wrong
/// window entirely. Use [`pane_id_for_window`], [`pane_ids_with_option`] or
/// [`pane_ids_with_option_value`] to resolve one.
pub fn swap_pane(source: &str, target: &str, runner: &dyn ProcessRunner) -> Result<()> {
    run_checked(
        runner,
        &["swap-pane", "-d", "-s", source, "-t", target],
        "swap-pane",
    )?;
    Ok(())
}

/// Move tmux focus to the specified pane.
pub fn select_pane(pane_id: &str, runner: &dyn ProcessRunner) -> Result<()> {
    run_checked(runner, &["select-pane", "-t", pane_id], "select-pane")?;
    Ok(())
}

// An `inactive_pane_id` used to live here: "the window's single inactive pane",
// which every caller used to mean "the agent-tree companion pane". It was removed
// with its last caller (#3856). The premise held only for a two-pane window whose
// focus had not moved, so with focus in the companion pane it named the *agent's*
// pane and the toggle killed a live claude session. Identify a pane by what it is
// rather than by whether it happens to be focused: [`pane_ids_with_option`] for
// "any pane dispatch created", [`pane_ids_with_option_value`] for one role. See
// docs/specs/agent-tree.allium's HideAgentTreePane.

/// Split one `list-panes` row of the form `<pane_id> <rest…>` into its two
/// halves. `rest` is empty when the field it carries is unset — tmux prints the
/// separator either way — and may itself contain spaces, so only the first field
/// is consumed.
fn split_pane_row(line: &str) -> Option<(&str, &str)> {
    // A row with no separator at all is not a shape tmux produces for these
    // formats, but reading it as "id, no value" is strictly better than dropping
    // the pane from the listing — and it is the shape the *last* row takes, since
    // `run_checked_stdout` trims the trailing whitespace an unset field leaves.
    (!line.is_empty()).then(|| line.split_once(' ').unwrap_or((line, "")))
}

/// Pane ids in `target`'s window whose `field` (a tmux format expression)
/// satisfies `matches`. The predicate receives the field's whole value, which may
/// contain spaces and is empty when the field is unset.
///
/// `target` may be a window name or a pane id. A pane id resolves to *its own*
/// window's panes, which is what lets a process inside a pane look up its
/// siblings knowing only `$TMUX_PANE`.
///
/// Private: the two wrappers below are the vocabulary callers should reach for,
/// so that "which field identifies a pane" stays a decision made here rather than
/// at every call site.
fn pane_ids_matching(
    target: &str,
    field: &str,
    matches: impl Fn(&str) -> bool,
    runner: &dyn ProcessRunner,
) -> Result<Vec<String>> {
    let resolved = window_target(target, runner)?;
    let format = format!("#{{pane_id}} {field}");
    let out = run_checked_stdout(
        runner,
        &["list-panes", "-t", &resolved, "-F", &format],
        "list-panes",
    )?;
    Ok(out
        .lines()
        .filter_map(split_pane_row)
        .filter(|(_, value)| matches(value))
        .map(|(id, _)| id.to_string())
        .collect())
}

/// Pane ids in `target`'s window whose pane-scoped user option `option` is set to
/// a non-empty value — *any* value.
///
/// This is how dispatch asks "which panes in this window did I create?", over
/// [`PANE_ROLE_OPTION`]: the marker is written at creation ([`set_pane_option`])
/// and survives [`respawn_pane_running`], so a pane is identified by what it is
/// rather than by whether it happens to be the focused one — which is the
/// heuristic this replaced, true only for an untouched two-pane window. Use
/// [`pane_ids_with_option_value`] to ask for one specific role.
pub fn pane_ids_with_option(
    target: &str,
    option: &str,
    runner: &dyn ProcessRunner,
) -> Result<Vec<String>> {
    pane_ids_matching(
        target,
        &option_format(option),
        |value| !value.is_empty(),
        runner,
    )
}

/// Pane ids in `target`'s window whose pane-scoped user option `option` equals
/// `value` exactly.
///
/// The role-specific half of [`pane_ids_with_option`]: exact equality, never a
/// prefix or substring, because the roles of two panes in one window must not be
/// able to stand in for each other.
pub fn pane_ids_with_option_value(
    target: &str,
    option: &str,
    value: &str,
    runner: &dyn ProcessRunner,
) -> Result<Vec<String>> {
    pane_ids_matching(
        target,
        &option_format(option),
        |found| found == value,
        runner,
    )
}

/// The tmux format expression that expands to user option `option`'s value.
/// Stated once so the two lookups above cannot spell it differently.
fn option_format(option: &str) -> String {
    format!("#{{{option}}}")
}

/// List the ids of all tmux panes across all sessions.
///
/// The pane-level sibling of [`list_all_window_names`], with the same `-a`
/// rationale and the same "no server running" handling. Private: only
/// [`pane_exists`] needs it.
fn list_all_pane_ids(runner: &dyn ProcessRunner) -> Result<Vec<String>> {
    list_all(&["list-panes", "-a", "-F", "#{pane_id}"], runner)
}

/// Check whether a tmux pane with the given ID still exists.
///
/// Implemented as a membership test over [`list_all_pane_ids`], mirroring
/// [`has_window`], because the obvious-looking alternative does not work:
/// `display-message -t <pane> -p ''` **succeeds for a pane that has never
/// existed**. tmux resolves an unknown target by falling back to the current
/// pane rather than failing, and with an empty format string there is no output
/// to betray the substitution — so an exit-status check reports every pane as
/// alive, always. Verified against tmux 3.5a with `-t %999`.
///
/// That made this function's only caller — `exec_check_split_pane`, which polls
/// whether the user has closed the pinned split pane — permanently blind, so a
/// closed pane left the board in split mode with a dead pane. Found by the
/// real-tmux harness in tests/tmux_lifecycle.rs; the mock tests could not see it,
/// and in fact pinned the broken behaviour by asserting a non-zero exit that
/// real tmux never returns (the same trap as task #3781).
///
/// A query failure maps to "gone", which is the pre-existing behaviour and the
/// conservative choice for the polling caller: it exits split mode rather than
/// leaving a pane pinned that may no longer be there. Contrast
/// [`has_window_or_assume_present`], where the gated action is destructive and
/// the default therefore goes the other way.
pub fn pane_exists(pane_id: &str, runner: &dyn ProcessRunner) -> bool {
    list_all_pane_ids(runner)
        .map(|ids| ids.iter().any(|id| id == pane_id))
        .unwrap_or(false)
}

/// Capture the current on-screen content of `target`'s pane — exactly what a
/// human attached to it would see right now, not scrollback history.
///
/// Takes a target [`window_target`] has already resolved, not a window name:
/// its one caller, [`crate::notify::notify_tmux`], resolves the window once and
/// drives both this and [`send_keys_at`] from that single pane id. A
/// name-taking wrapper would have no callers — see [`send_keys`] for the shape
/// used where both kinds of caller exist.
///
/// Used by [`crate::notify`] to decide whether a pane is showing its normal
/// input surface before injecting keystrokes into it — see
/// `docs/superpowers/specs/2026-08-15-send-message-delivery-hardening-design.md`
/// for the reproduction this exists to guard against.
pub(crate) fn capture_pane(target: &str, runner: &dyn ProcessRunner) -> Result<String> {
    run_checked_stdout(
        runner,
        &["capture-pane", "-p", "-t", target],
        "capture-pane",
    )
}

// ---------------------------------------------------------------------------
// Tests
// ---------------------------------------------------------------------------

#[cfg(test)]
mod tests {
    #![allow(clippy::unwrap_used, clippy::expect_used)]
    use super::*;
    use crate::models::test_tmux_window;

    // --- window_target scaffolding ---
    //
    // Every helper that takes a window *name* resolves it to a pane ID first, so
    // these tests declare the fake server's windows with `with_windows` and
    // assert the resolved `-t %N` target via `pane_id_of`. `with_windows` answers
    // the lookup out of band, so response queues and `calls[N]` indices stay
    // about the operation — see `MockProcessRunner::with_windows`.
    //
    // What these tests can and cannot do: they assert the argv we hand tmux, not
    // what tmux does with it. The pre-fix versions pinned the vulnerable
    // `-t task-42` argv and stayed green throughout. The behavioural coverage —
    // that an absent name cannot reach a prefix-matched sibling — is in
    // tests/tmux_window_targets.rs, against a real server.

    /// The three-window topology that exposes the bug: `task-4`'s name is a
    /// prefix of `task-42`'s.
    const COLLIDING: [&str; 3] = ["dispatch", "task-4", "task-42"];

    #[test]
    fn has_window_finds_match_in_output() {
        let mock = MockProcessRunner::new(vec![MockProcessRunner::ok_with_stdout(
            b"main\ntask-42\nother-window\n",
        )]);
        let result = has_window(&test_tmux_window("task-42"), &mock).unwrap();
        assert!(result);
    }

    #[test]
    fn has_window_no_match() {
        let mock = MockProcessRunner::new(vec![MockProcessRunner::ok_with_stdout(
            b"main\nother-window\n",
        )]);
        let result = has_window(&test_tmux_window("task-42"), &mock).unwrap();
        assert!(!result);
    }

    #[test]
    fn has_window_exact_match_not_prefix() {
        let mock = MockProcessRunner::new(vec![MockProcessRunner::ok_with_stdout(b"task-42\n")]);
        let result = has_window(&test_tmux_window("task-4"), &mock).unwrap();
        assert!(!result);
    }

    // --- ProcessRunner-based tests ---

    use crate::process::MockProcessRunner;

    #[test]
    fn new_window_issues_correct_tmux_args() {
        let mock = MockProcessRunner::new(vec![MockProcessRunner::ok()]);
        new_window(&test_tmux_window("task-42"), "/some/path", &mock).unwrap();
        let calls = mock.recorded_calls();
        assert_eq!(calls.len(), 1);
        assert_eq!(calls[0].0, "tmux");
        assert_eq!(
            calls[0].1,
            vec!["new-window", "-d", "-n", "task-42", "-c", "/some/path"]
        );
    }

    // A hung tmux server must not park the calling thread forever (#4202):
    // this is one of `provision_worktree`'s `post_add` calls.
    #[test]
    fn new_window_is_bounded_by_subprocess_timeout() {
        let mock = MockProcessRunner::new(vec![MockProcessRunner::ok()]);
        new_window(&test_tmux_window("task-42"), "/some/path", &mock).unwrap();
        assert_eq!(
            mock.recorded_timeouts(),
            vec![Some(crate::process::SUBPROCESS_TIMEOUT)]
        );
    }

    #[test]
    fn new_window_running_issues_correct_tmux_args() {
        let mock = MockProcessRunner::new(vec![MockProcessRunner::ok()]);
        new_window_running(
            &test_tmux_window("dispatch-edit-1"),
            "/home/u",
            &["vim", "/tmp/foo.md"],
            &mock,
        )
        .unwrap();
        let calls = mock.recorded_calls();
        assert_eq!(calls.len(), 1);
        assert_eq!(calls[0].0, "tmux");
        assert_eq!(
            calls[0].1,
            vec![
                "new-window",
                "-d",
                "-n",
                "dispatch-edit-1",
                "-c",
                "/home/u",
                "--",
                "vim",
                "/tmp/foo.md"
            ]
        );
    }

    #[test]
    fn new_window_running_keeps_argv_elements_separate() {
        // A path with spaces must be passed as its own argv element, not
        // joined into a single shell string. This is why we use the `--`
        // exec form rather than `send-keys` with a concatenated command.
        let mock = MockProcessRunner::new(vec![MockProcessRunner::ok()]);
        new_window_running(
            &test_tmux_window("edit-1"),
            "/tmp",
            &["vim", "/tmp/dir with spaces/file.md"],
            &mock,
        )
        .unwrap();
        let calls = mock.recorded_calls();
        assert_eq!(calls[0].1.last().unwrap(), "/tmp/dir with spaces/file.md");
        // and the preceding element is the exec separator + program
        assert_eq!(calls[0].1[calls[0].1.len() - 3], "--");
        assert_eq!(calls[0].1[calls[0].1.len() - 2], "vim");
    }

    #[test]
    fn new_window_running_rejects_empty_command() {
        let mock = MockProcessRunner::new(vec![]);
        let err = new_window_running(&test_tmux_window("n"), "/tmp", &[], &mock).unwrap_err();
        assert!(err.to_string().contains("command must not be empty"));
    }

    #[test]
    fn new_window_running_fails_on_nonzero_exit() {
        let mock = MockProcessRunner::new(vec![MockProcessRunner::fail("bad")]);
        let err =
            new_window_running(&test_tmux_window("n"), "/tmp", &["vim", "f"], &mock).unwrap_err();
        assert!(err.to_string().contains("new-window failed"));
    }

    #[test]
    fn has_window_returns_false_on_nonzero_exit() {
        let mock = MockProcessRunner::new(vec![MockProcessRunner::fail("no sessions")]);
        let result = has_window(&test_tmux_window("task-42"), &mock).unwrap();
        assert!(!result);
    }

    // --- has_window_or_assume_present ---

    #[test]
    fn has_window_or_assume_present_true_when_present() {
        let mock = MockProcessRunner::new(vec![MockProcessRunner::ok_with_stdout(b"task-42\n")]);
        assert!(has_window_or_assume_present(
            &test_tmux_window("task-42"),
            &mock
        ));
    }

    #[test]
    fn has_window_or_assume_present_false_when_absent() {
        let mock = MockProcessRunner::new(vec![MockProcessRunner::ok_with_stdout(b"other\n")]);
        assert!(!has_window_or_assume_present(
            &test_tmux_window("task-42"),
            &mock
        ));
    }

    #[test]
    fn has_window_or_assume_present_true_when_query_fails() {
        let mock = MockProcessRunner::new(vec![Err(anyhow::anyhow!("tmux: command not found"))]);
        assert!(has_window_or_assume_present(
            &test_tmux_window("task-42"),
            &mock
        ));
    }

    // --- kill_window_if_present ---

    #[test]
    fn kill_window_if_present_kills_when_present() {
        let mock = MockProcessRunner::new(vec![
            MockProcessRunner::ok_with_stdout(b"task-42\n"), // has_window
            MockProcessRunner::ok(),                         // kill-window
        ])
        .with_windows(&["task-42"]);
        kill_window_if_present(&test_tmux_window("task-42"), &mock).unwrap();
        let calls = mock.recorded_calls();
        assert_eq!(calls.len(), 2);
        assert_eq!(calls[1].1[0], "kill-window");
    }

    #[test]
    fn kill_window_if_present_skips_when_absent() {
        let mock = MockProcessRunner::new(vec![MockProcessRunner::ok_with_stdout(b"other\n")]);
        kill_window_if_present(&test_tmux_window("task-42"), &mock).unwrap();
        let calls = mock.recorded_calls();
        assert_eq!(calls.len(), 1, "kill-window should not be called");
    }

    #[test]
    fn kill_window_if_present_skips_and_succeeds_when_query_fails() {
        let mock = MockProcessRunner::new(vec![Err(anyhow::anyhow!("tmux: command not found"))]);
        kill_window_if_present(&test_tmux_window("task-42"), &mock).unwrap();
        let calls = mock.recorded_calls();
        assert_eq!(calls.len(), 1, "kill-window should not be attempted");
    }

    #[test]
    fn kill_window_if_present_propagates_kill_failure() {
        let mock = MockProcessRunner::new(vec![
            MockProcessRunner::ok_with_stdout(b"task-42\n"), // has_window
            MockProcessRunner::fail("no such window"),       // kill-window fails
        ])
        .with_windows(&["task-42"]);
        let err = kill_window_if_present(&test_tmux_window("task-42"), &mock).unwrap_err();
        assert!(err.to_string().contains("kill-window failed"), "got: {err}");
    }

    #[test]
    fn has_window_queries_across_all_sessions() {
        // has_window is used for cross-session liveness checks (main session,
        // cleanup, finish, editor, staleness) — without -a, list-windows scopes
        // to the current/attached session only and misses windows living in
        // another session, producing a false "not found".
        let mock = MockProcessRunner::new(vec![MockProcessRunner::ok_with_stdout(b"task-42\n")]);
        let _ = has_window(&test_tmux_window("task-42"), &mock).unwrap();
        let calls = mock.recorded_calls();
        assert_eq!(calls.len(), 1);
        assert_eq!(calls[0].0, "tmux");
        assert_eq!(
            calls[0].1,
            vec!["list-windows", "-a", "-F", "#{window_name}"]
        );
    }

    #[test]
    fn set_window_dispatch_dir_issues_correct_tmux_args() {
        let mock = MockProcessRunner::new(vec![MockProcessRunner::ok()]).with_windows(&COLLIDING);
        set_window_dispatch_dir(&test_tmux_window("task-42"), "/some/path", &mock).unwrap();
        let calls = mock.recorded_calls();
        assert_eq!(calls.len(), 1);
        assert_eq!(calls[0].0, "tmux");
        assert_eq!(
            calls[0].1,
            vec![
                "set-option",
                "-w",
                "-t",
                &mock.pane_id_of("task-42"),
                "@dispatch_dir",
                "/some/path",
            ]
        );
    }

    // A hung tmux server must not park the calling thread forever (#4202):
    // this is one of `provision_worktree`'s `post_add` calls.
    #[test]
    fn set_window_dispatch_dir_is_bounded_by_subprocess_timeout() {
        let mock = MockProcessRunner::new(vec![MockProcessRunner::ok()]).with_windows(&COLLIDING);
        set_window_dispatch_dir(&test_tmux_window("task-42"), "/some/path", &mock).unwrap();
        assert_eq!(
            mock.recorded_timeouts(),
            vec![Some(crate::process::SUBPROCESS_TIMEOUT)]
        );
    }

    #[test]
    fn set_window_dispatch_dir_detects_ambiguous_windows() {
        let mock = MockProcessRunner::new(vec![]).with_windows(&["task-42", "task-42"]);
        let err =
            set_window_dispatch_dir(&test_tmux_window("task-42"), "/some/path", &mock).unwrap_err();
        assert!(err.to_string().contains("multiple tmux windows"));
        assert!(
            mock.recorded_calls().is_empty(),
            "set-option must not be attempted for an ambiguous name"
        );
    }

    /// Pins the hook string, including its `-t #{pane_id}` target, its
    /// already-in-the-worktree guard, and the quotes around the interpolated
    /// directory. Note what this test can and cannot do: it proves the argv we
    /// hand tmux, not what tmux then does with it. The *behaviour* — which pane
    /// is corrected, which is left alone, and that nothing is typed anywhere —
    /// is only observable against a real server, so it is asserted in
    /// tests/tmux_split_hook.rs. A mock-level test of this hook once asserted
    /// the untargeted string verbatim and stayed green while the board was being
    /// typed into.
    #[test]
    fn ensure_split_hook_issues_correct_tmux_args() {
        let mock = MockProcessRunner::new(vec![MockProcessRunner::ok()]);
        ensure_split_hook(&mock).unwrap();
        let calls = mock.recorded_calls();
        assert_eq!(calls.len(), 1);
        assert_eq!(calls[0].0, "tmux");
        assert_eq!(
            calls[0].1,
            vec![
                "set-hook",
                "after-split-window",
                "if-shell -F '#{&&:#{@dispatch_dir},#{!=:#{pane_start_path},#{@dispatch_dir}}}' \
                 'run-shell -bC \"respawn-pane -k -t #{pane_id} -c \\\"#{@dispatch_dir}\\\"\"'",
            ]
        );
    }

    // A hung tmux server must not park the calling thread forever (#4202):
    // this is one of `provision_worktree`'s `post_add` calls.
    #[test]
    fn ensure_split_hook_is_bounded_by_subprocess_timeout() {
        let mock = MockProcessRunner::new(vec![MockProcessRunner::ok()]);
        ensure_split_hook(&mock).unwrap();
        assert_eq!(
            mock.recorded_timeouts(),
            vec![Some(crate::process::SUBPROCESS_TIMEOUT)]
        );
    }

    /// The correction must never be delivered as synthesised input — see
    /// split-pane.allium's `SplitDirectoryIsNeverKeystrokes`. Asserted here as
    /// well as behaviourally, so that reintroducing `send-keys` fails at the
    /// cheapest possible layer.
    #[test]
    fn ensure_split_hook_never_sends_keys() {
        let mock = MockProcessRunner::new(vec![MockProcessRunner::ok()]);
        ensure_split_hook(&mock).unwrap();
        let hook = mock.recorded_calls()[0].1[2].clone();
        assert!(
            !hook.contains("send-keys"),
            "the split correction must not type at a pane: {hook}"
        );
    }

    #[test]
    fn current_window_name_returns_trimmed_stdout() {
        let mock = MockProcessRunner::new(vec![MockProcessRunner::ok_with_stdout(b"dispatch\n")]);
        let result = current_window_name(&mock).unwrap();
        assert_eq!(result, "dispatch");
    }

    #[test]
    fn current_window_name_issues_correct_tmux_args() {
        let mock = MockProcessRunner::new(vec![MockProcessRunner::ok_with_stdout(b"dispatch\n")]);
        current_window_name(&mock).unwrap();
        let calls = mock.recorded_calls();
        assert_eq!(calls.len(), 1);
        assert_eq!(calls[0].0, "tmux");
        assert_eq!(calls[0].1, vec!["display-message", "-p", "#W"]);
    }

    #[test]
    fn current_window_name_fails_on_nonzero_exit() {
        let mock = MockProcessRunner::new(vec![MockProcessRunner::fail("no session")]);
        assert!(current_window_name(&mock).is_err());
    }

    #[test]
    fn rename_window_issues_correct_tmux_args() {
        let mock = MockProcessRunner::new(vec![MockProcessRunner::ok()])
            .with_windows(&["dispatch", "task-42"]);
        rename_window("dispatch", &test_tmux_window("my-old-name"), &mock).unwrap();
        let calls = mock.recorded_calls();
        assert_eq!(calls.len(), 1);
        assert_eq!(calls[0].0, "tmux");
        assert_eq!(
            calls[0].1,
            vec![
                "rename-window",
                "-t",
                &mock.pane_id_of("dispatch"),
                "my-old-name",
            ]
        );
    }

    /// `-t` and the new name are adjacent arguments; only the target is resolved.
    /// A resolver applied to the new name would reject every rename, since the
    /// name being assigned does not exist yet — here `brand-new-name` is not a
    /// declared window, so resolving it would fail.
    #[test]
    fn rename_window_does_not_resolve_the_new_name() {
        let mock =
            MockProcessRunner::new(vec![MockProcessRunner::ok()]).with_windows(&["dispatch"]);
        rename_window("dispatch", &test_tmux_window("brand-new-name"), &mock).unwrap();
        let calls = mock.recorded_calls();
        assert_eq!(calls[0].1.last().unwrap(), "brand-new-name");
    }

    /// `setup_tmux_for_tui` (src/runtime/mod.rs) renames by pane ID, and falls
    /// back to `""` (tmux's "current window") when that lookup fails. Both are
    /// already unambiguous and must reach tmux untouched.
    #[test]
    fn rename_window_passes_through_already_exact_targets() {
        for target in ["%7", ""] {
            // `with_queued_window_lookup` makes a resolution attempt observable:
            // it would consume the queued Ok and then panic for want of a second
            // response. Passing means no lookup happened at all.
            let mock =
                MockProcessRunner::new(vec![MockProcessRunner::ok()]).with_queued_window_lookup();
            rename_window(target, &test_tmux_window("dispatch"), &mock).unwrap();
            let calls = mock.recorded_calls();
            assert_eq!(calls.len(), 1, "no resolution for target {target:?}");
            assert_eq!(
                calls[0].1,
                vec!["rename-window", "-t", target, "dispatch"],
                "target {target:?} should pass through unchanged"
            );
        }
    }

    #[test]
    fn rename_window_fails_on_nonzero_exit() {
        let mock = MockProcessRunner::new(vec![MockProcessRunner::fail("no window")])
            .with_windows(&["dispatch"]);
        assert!(rename_window("dispatch", &test_tmux_window("other"), &mock).is_err());
    }

    #[test]
    fn rename_window_fails_when_target_window_is_absent() {
        let mock = MockProcessRunner::new(vec![]).with_windows(&["dispatch", "task-42"]);
        let err = rename_window("task-4", &test_tmux_window("renamed"), &mock).unwrap_err();
        assert!(
            err.to_string().contains("no tmux window named 'task-4'"),
            "got: {err}"
        );
    }

    #[test]
    fn bind_key_issues_correct_tmux_args() {
        let mock = MockProcessRunner::new(vec![MockProcessRunner::ok()]);
        bind_key("space", "select-window -t dispatch", &mock).unwrap();
        let calls = mock.recorded_calls();
        assert_eq!(calls.len(), 1);
        assert_eq!(calls[0].0, "tmux");
        assert_eq!(
            calls[0].1,
            vec!["bind-key", "space", "select-window -t dispatch"]
        );
    }

    #[test]
    fn unbind_key_issues_correct_tmux_args() {
        let mock = MockProcessRunner::new(vec![MockProcessRunner::ok()]);
        unbind_key("space", &mock).unwrap();
        let calls = mock.recorded_calls();
        assert_eq!(calls.len(), 1);
        assert_eq!(calls[0].0, "tmux");
        assert_eq!(calls[0].1, vec!["unbind-key", "space"]);
    }

    #[test]
    fn join_pane_issues_correct_tmux_args() {
        // Resolution *is* the pane lookup, so the separate display-message call
        // this used to make is gone: one recorded call, not two.
        let mock = MockProcessRunner::new(vec![MockProcessRunner::ok()]).with_windows(&COLLIDING);
        let pane_id = join_pane(&test_tmux_window("task-42"), "%1", &mock).unwrap();
        let expected = mock.pane_id_of("task-42");
        assert_eq!(pane_id, expected);
        let calls = mock.recorded_calls();
        assert_eq!(calls.len(), 1);
        // join-pane takes the resolved pane ID as its source, not the name.
        assert_eq!(
            calls[0].1,
            vec![
                "join-pane",
                "-h",
                "-d",
                "-s",
                &expected,
                "-t",
                "%1",
                "-l",
                "40%"
            ]
        );
    }

    #[test]
    fn join_pane_returns_source_pane_id() {
        let mock =
            MockProcessRunner::new(vec![MockProcessRunner::ok()]).with_windows(&["my-window"]);
        let result = join_pane(&test_tmux_window("my-window"), "%0", &mock).unwrap();
        assert_eq!(result, mock.pane_id_of("my-window"));
    }

    #[test]
    fn join_pane_fails_when_source_window_is_absent() {
        // Otherwise a prefix-matched sibling's pane is torn out of its own window
        // and pulled into the board's split.
        let mock = MockProcessRunner::new(vec![]).with_windows(&["dispatch", "task-42"]);
        let err = join_pane(&test_tmux_window("task-4"), "%1", &mock).unwrap_err();
        assert!(
            err.to_string().contains("no tmux window named 'task-4'"),
            "got: {err}"
        );
        assert!(mock.recorded_calls().is_empty(), "join-pane must not run");
    }

    #[test]
    fn select_pane_issues_correct_tmux_args() {
        let mock = MockProcessRunner::new(vec![MockProcessRunner::ok()]);
        select_pane("%42", &mock).unwrap();
        let calls = mock.recorded_calls();
        assert_eq!(calls.len(), 1);
        assert_eq!(calls[0].0, "tmux");
        assert_eq!(calls[0].1, vec!["select-pane", "-t", "%42"]);
    }

    #[test]
    fn focus_events_enabled_returns_true_when_on() {
        let mock = MockProcessRunner::new(vec![MockProcessRunner::ok_with_stdout(b"on\n")]);
        assert!(focus_events_enabled(&mock));
    }

    #[test]
    fn focus_events_enabled_returns_false_when_off() {
        let mock = MockProcessRunner::new(vec![MockProcessRunner::ok_with_stdout(b"off\n")]);
        assert!(!focus_events_enabled(&mock));
    }

    #[test]
    fn set_focus_events_issues_correct_tmux_args() {
        let mock = MockProcessRunner::new(vec![MockProcessRunner::ok()]);
        set_focus_events(&mock).unwrap();
        let calls = mock.recorded_calls();
        assert_eq!(calls.len(), 1);
        assert_eq!(calls[0].0, "tmux");
        assert_eq!(calls[0].1, vec!["set-option", "-g", "focus-events", "on"]);
    }

    #[test]
    fn write_focus_events_creates_file_if_absent() {
        let dir = tempfile::tempdir().unwrap();
        let conf = dir.path().join(".tmux.conf");
        write_focus_events_to_tmux_conf_at(&conf).unwrap();
        let content = std::fs::read_to_string(&conf).unwrap();
        assert!(content.contains("set -g focus-events on"));
    }

    #[test]
    fn write_focus_events_appends_to_existing_file() {
        let dir = tempfile::tempdir().unwrap();
        let conf = dir.path().join(".tmux.conf");
        std::fs::write(&conf, "set -g mouse on\n").unwrap();
        write_focus_events_to_tmux_conf_at(&conf).unwrap();
        let content = std::fs::read_to_string(&conf).unwrap();
        assert!(content.contains("set -g mouse on"));
        assert!(content.contains("set -g focus-events on"));
    }

    #[test]
    fn write_focus_events_is_idempotent() {
        let dir = tempfile::tempdir().unwrap();
        let conf = dir.path().join(".tmux.conf");
        std::fs::write(&conf, "set -g focus-events on\n").unwrap();
        write_focus_events_to_tmux_conf_at(&conf).unwrap();
        let content = std::fs::read_to_string(&conf).unwrap();
        assert_eq!(
            content.matches("focus-events on").count(),
            1,
            "should not duplicate the line"
        );
    }

    #[test]
    fn list_all_window_names_parses_output() {
        let mock = MockProcessRunner::new(vec![MockProcessRunner::ok_with_stdout(
            b"dispatch\ntask-42\ntask-99\n",
        )]);
        let names = list_all_window_names(&mock).unwrap();
        assert_eq!(names, vec!["dispatch", "task-42", "task-99"]);
    }

    #[test]
    fn list_all_window_names_empty_when_no_sessions() {
        let mock = MockProcessRunner::new(vec![MockProcessRunner::fail("no server running")]);
        let names = list_all_window_names(&mock).unwrap();
        assert!(
            names.is_empty(),
            "expected empty vec when tmux not running, got: {names:?}"
        );
    }

    #[test]
    fn list_all_window_names_issues_correct_args() {
        let mock = MockProcessRunner::new(vec![MockProcessRunner::ok_with_stdout(b"dispatch\n")]);
        let _ = list_all_window_names(&mock).unwrap();
        let calls = mock.recorded_calls();
        assert_eq!(calls.len(), 1);
        assert_eq!(calls[0].0, "tmux");
        assert_eq!(
            calls[0].1,
            vec!["list-windows", "-a", "-F", "#{window_name}"]
        );
    }

    // --- new_window failure path ---

    #[test]
    fn new_window_fails_on_nonzero_exit() {
        let mock = MockProcessRunner::new(vec![MockProcessRunner::fail("no server running")]);
        let err = new_window(&test_tmux_window("task-1"), "/tmp", &mock).unwrap_err();
        assert!(
            err.to_string().contains("new-window failed"),
            "expected 'new-window failed', got: {err}"
        );
    }

    // --- send_keys ---

    #[test]
    fn send_keys_issues_correct_tmux_args() {
        let mock = MockProcessRunner::new(vec![
            MockProcessRunner::ok(), // send-keys -l
            MockProcessRunner::ok(), // send-keys Enter
        ])
        .with_windows(&["dispatch", "task-1"]);
        send_keys(&test_tmux_window("task-1"), "hello world", &mock).unwrap();
        let calls = mock.recorded_calls();
        assert_eq!(calls.len(), 2);
        let pane = mock.pane_id_of("task-1");
        assert_eq!(calls[0].0, "tmux");
        assert_eq!(
            calls[0].1,
            vec!["send-keys", "-t", &pane, "-l", "hello world"]
        );
        assert_eq!(calls[1].0, "tmux");
        assert_eq!(calls[1].1, vec!["send-keys", "-t", &pane, "Enter"]);
    }

    /// Both `send-keys` calls must name the *same* resolved pane, so the payload
    /// and the Enter that submits it cannot land in different places.
    #[test]
    fn send_keys_targets_one_pane_for_both_calls() {
        let mock = MockProcessRunner::new(vec![MockProcessRunner::ok(), MockProcessRunner::ok()])
            .with_windows(&["task-1"]);
        send_keys(&test_tmux_window("task-1"), "hello", &mock).unwrap();
        let calls = mock.recorded_calls();
        let target = |c: &(String, Vec<String>)| c.1[2].clone();
        assert_eq!(target(&calls[0]), target(&calls[1]));
        assert_eq!(target(&calls[0]), mock.pane_id_of("task-1"));
    }

    #[test]
    fn send_keys_fails_when_window_is_absent() {
        // The worst consequence of prefix matching: `task-1`'s payload typed into
        // `task-12`'s live Claude session as user input.
        let mock = MockProcessRunner::new(vec![]).with_windows(&["dispatch", "task-12"]);
        let err = send_keys(&test_tmux_window("task-1"), "hello", &mock).unwrap_err();
        assert!(
            err.to_string().contains("no tmux window named 'task-1'"),
            "got: {err}"
        );
        assert!(
            mock.recorded_calls().is_empty(),
            "no send-keys may be attempted"
        );
    }

    #[test]
    fn send_keys_fails_on_first_send_error() {
        let mock = MockProcessRunner::new(vec![MockProcessRunner::fail("no pane")])
            .with_windows(&["task-1"]);
        let err = send_keys(&test_tmux_window("task-1"), "hello", &mock).unwrap_err();
        assert!(
            err.to_string().contains("send-keys -l failed"),
            "got: {err}"
        );
    }

    #[test]
    fn send_keys_fails_on_enter_send_error() {
        let mock = MockProcessRunner::new(vec![
            MockProcessRunner::ok(),              // send-keys -l succeeds
            MockProcessRunner::fail("pane gone"), // send-keys Enter fails
        ])
        .with_windows(&["task-1"]);
        let err = send_keys(&test_tmux_window("task-1"), "hello", &mock).unwrap_err();
        assert!(
            err.to_string().contains("send-keys Enter failed"),
            "got: {err}"
        );
    }

    // --- kill_window ---

    #[test]
    fn kill_window_issues_correct_tmux_args() {
        let mock = MockProcessRunner::new(vec![MockProcessRunner::ok()]).with_windows(&COLLIDING);
        kill_window(&test_tmux_window("task-42"), &mock).unwrap();
        let calls = mock.recorded_calls();
        assert_eq!(calls.len(), 1);
        assert_eq!(calls[0].0, "tmux");
        assert_eq!(
            calls[0].1,
            vec!["kill-window", "-t", &mock.pane_id_of("task-42")]
        );
    }

    #[test]
    fn kill_window_fails_when_window_is_absent() {
        // The other worst consequence: killing a live sibling's agent because the
        // intended window had already died.
        let mock = MockProcessRunner::new(vec![]).with_windows(&["dispatch", "keep-99"]);
        let err = kill_window(&test_tmux_window("keep-9"), &mock).unwrap_err();
        assert!(
            err.to_string().contains("no tmux window named 'keep-9'"),
            "got: {err}"
        );
        assert!(
            mock.recorded_calls().is_empty(),
            "no kill-window may be attempted"
        );
    }

    #[test]
    fn kill_window_fails_on_nonzero_exit() {
        let mock = MockProcessRunner::new(vec![MockProcessRunner::fail("no window")])
            .with_windows(&["task-42"]);
        let err = kill_window(&test_tmux_window("task-42"), &mock).unwrap_err();
        assert!(err.to_string().contains("kill-window failed"), "got: {err}");
    }

    // --- select_window ---

    #[test]
    fn select_window_issues_correct_tmux_args() {
        let mock = MockProcessRunner::new(vec![MockProcessRunner::ok()]).with_windows(&COLLIDING);
        select_window(&test_tmux_window("task-4"), &mock).unwrap();
        let calls = mock.recorded_calls();
        assert_eq!(
            calls[0].1,
            vec!["select-window", "-t", &mock.pane_id_of("task-4")]
        );
    }

    #[test]
    fn select_window_fails_when_window_is_absent() {
        let mock = MockProcessRunner::new(vec![]).with_windows(&["dispatch", "task-42"]);
        let err = select_window(&test_tmux_window("task-4"), &mock).unwrap_err();
        assert!(
            err.to_string().contains("no tmux window named 'task-4'"),
            "got: {err}"
        );
    }

    #[test]
    fn select_window_fails_on_nonzero_exit() {
        let mock = MockProcessRunner::new(vec![MockProcessRunner::fail("no window")])
            .with_windows(&["task-42"]);
        let err = select_window(&test_tmux_window("task-42"), &mock).unwrap_err();
        assert!(
            err.to_string().contains("select-window failed"),
            "got: {err}"
        );
    }

    // --- ensure_split_hook failure ---

    #[test]
    fn ensure_split_hook_fails_on_nonzero_exit() {
        let mock = MockProcessRunner::new(vec![MockProcessRunner::fail("no session")]);
        let err = ensure_split_hook(&mock).unwrap_err();
        assert!(err.to_string().contains("set-hook failed"), "got: {err}");
    }

    // --- set_window_dispatch_dir generic failure ---

    #[test]
    fn set_window_dispatch_dir_fails_on_generic_nonzero_exit() {
        // The window resolves, but `set-option` itself fails.
        let mock = MockProcessRunner::new(vec![MockProcessRunner::fail("no session running")])
            .with_windows(&["task-42"]);
        let err =
            set_window_dispatch_dir(&test_tmux_window("task-42"), "/some/path", &mock).unwrap_err();
        let msg = err.to_string();
        assert!(
            msg.contains("set-option failed"),
            "expected 'set-option failed', got: {msg}"
        );
        assert!(
            !msg.contains("multiple tmux windows"),
            "should not be the ambiguous-window error, got: {msg}"
        );
    }

    #[test]
    fn set_window_dispatch_dir_fails_when_window_is_absent() {
        // The bug: with only `task-42` alive, a request for `task-4` used to set
        // @dispatch_dir on task-42, sending that task's future splits into a
        // different task's worktree.
        let mock = MockProcessRunner::new(vec![]).with_windows(&["dispatch", "task-42"]);
        let err =
            set_window_dispatch_dir(&test_tmux_window("task-4"), "/some/path", &mock).unwrap_err();
        assert!(
            err.to_string().contains("no tmux window named 'task-4'"),
            "got: {err}"
        );
        assert!(mock.recorded_calls().is_empty(), "set-option must not run");
    }

    // --- split_window_horizontal ---

    #[test]
    fn split_window_horizontal_issues_correct_args() {
        let mock = MockProcessRunner::new(vec![MockProcessRunner::ok_with_stdout(b"%5\n")]);
        let pane_id = split_window_horizontal("%1", &mock).unwrap();
        assert_eq!(pane_id, "%5");
        let calls = mock.recorded_calls();
        assert_eq!(
            calls[0].1,
            vec![
                "split-window",
                "-h",
                "-d",
                "-l",
                "40%",
                "-t",
                "%1",
                "-P",
                "-F",
                "#{pane_id}",
            ]
        );
    }

    #[test]
    fn split_window_horizontal_fails_on_nonzero_exit() {
        let mock = MockProcessRunner::new(vec![MockProcessRunner::fail("no target pane")]);
        let err = split_window_horizontal("%1", &mock).unwrap_err();
        assert!(
            err.to_string().contains("split-window failed"),
            "got: {err}"
        );
    }

    // --- split_window_horizontal_running ---

    #[test]
    fn split_window_horizontal_running_issues_correct_args() {
        let mock = MockProcessRunner::new(vec![MockProcessRunner::ok_with_stdout(b"%9\n")]);
        let pane_id = split_window_horizontal_running(
            "%1",
            30,
            &["dispatch", "agent-tree", "42"],
            None,
            &mock,
        )
        .unwrap();
        assert_eq!(pane_id, "%9");
        let calls = mock.recorded_calls();
        assert_eq!(calls.len(), 1);
        assert_eq!(calls[0].0, "tmux");
        assert_eq!(
            calls[0].1,
            vec![
                "split-window",
                "-h",
                "-b",
                "-d",
                "-l",
                "30%",
                "-t",
                "%1",
                "-P",
                "-F",
                "#{pane_id}",
                "--",
                "dispatch",
                "agent-tree",
                "42",
            ]
        );
    }

    /// The start directory is what keeps a dispatch-created pane out of the
    /// correction hook — see `ensure_split_hook`. Whether tmux honours it is a
    /// real-server question (tests/tmux_split_hook.rs); this pins that we send it.
    #[test]
    fn split_window_horizontal_running_passes_the_start_directory() {
        let mock = MockProcessRunner::new(vec![MockProcessRunner::ok_with_stdout(b"%9\n")]);
        split_window_horizontal_running(
            "%1",
            30,
            &["dispatch", "agent-tree", "42"],
            Some("/home/u/wt"),
            &mock,
        )
        .unwrap();
        let calls = mock.recorded_calls();
        let args = &calls[0].1;
        let at = args
            .iter()
            .position(|a| a == "-c")
            .expect("expected -c in argv");
        assert_eq!(args[at + 1], "/home/u/wt");
        // Before `--`, or tmux would read it as part of the pane's command.
        let sep = args.iter().position(|a| a == "--").expect("expected --");
        assert!(at < sep, "-c must precede the command separator: {args:?}");
    }

    #[test]
    fn split_window_horizontal_running_keeps_argv_elements_separate() {
        // A path with spaces must stay one argv element via the `--` exec
        // form, not get joined into a single shell string.
        let mock = MockProcessRunner::new(vec![MockProcessRunner::ok_with_stdout(b"%9\n")]);
        split_window_horizontal_running(
            "%1",
            30,
            &["vim", "/tmp/dir with spaces/file.md"],
            None,
            &mock,
        )
        .unwrap();
        let calls = mock.recorded_calls();
        assert_eq!(calls[0].1.last().unwrap(), "/tmp/dir with spaces/file.md");
        assert_eq!(calls[0].1[calls[0].1.len() - 3], "--");
        assert_eq!(calls[0].1[calls[0].1.len() - 2], "vim");
    }

    #[test]
    fn split_window_horizontal_running_rejects_empty_command() {
        let mock = MockProcessRunner::new(vec![]);
        let err = split_window_horizontal_running("%1", 30, &[], None, &mock).unwrap_err();
        assert!(err.to_string().contains("command must not be empty"));
    }

    #[test]
    fn split_window_horizontal_running_fails_on_nonzero_exit() {
        let mock = MockProcessRunner::new(vec![MockProcessRunner::fail("no target pane")]);
        let err = split_window_horizontal_running(
            "%1",
            30,
            &["dispatch", "agent-tree", "42"],
            None,
            &mock,
        )
        .unwrap_err();
        assert!(
            err.to_string().contains("split-window failed"),
            "got: {err}"
        );
    }

    // --- pane_ids_with_option / pane_ids_with_option_value ---

    #[test]
    fn pane_ids_with_option_returns_only_marked_panes() {
        // `list-panes` rows are "<pane_id> <value>"; an unset option renders as
        // the empty string, with the separator still there.
        let mock = MockProcessRunner::new(vec![MockProcessRunner::ok_with_stdout(
            b"%1 \n%2 agent_tree\n%3 \n",
        )]);
        let found = pane_ids_with_option("%1", PANE_ROLE_OPTION, &mock).unwrap();
        assert_eq!(found, vec!["%2".to_string()]);
        assert_eq!(
            mock.recorded_calls()[0].1,
            vec![
                "list-panes",
                "-t",
                "%1",
                "-F",
                "#{pane_id} #{@dispatch_pane_role}",
            ]
        );
    }

    /// Any role at all: this is how the pin drain asks for "every pane dispatch
    /// put in this window", without being taught the roles.
    #[test]
    fn pane_ids_with_option_returns_every_role() {
        let mock = MockProcessRunner::new(vec![MockProcessRunner::ok_with_stdout(
            b"%1 \n%2 agent_tree\n%3 editor\n",
        )]);
        let found = pane_ids_with_option("%1", PANE_ROLE_OPTION, &mock).unwrap();
        assert_eq!(found, vec!["%2".to_string(), "%3".to_string()]);
    }

    #[test]
    fn pane_ids_with_option_is_empty_when_nothing_is_marked() {
        let mock = MockProcessRunner::new(vec![MockProcessRunner::ok_with_stdout(b"%1 \n%2 \n")]);
        assert!(pane_ids_with_option("%1", PANE_ROLE_OPTION, &mock)
            .unwrap()
            .is_empty());
    }

    #[test]
    fn pane_ids_with_option_fails_on_nonzero_exit() {
        let mock = MockProcessRunner::new(vec![MockProcessRunner::fail("no window")]);
        assert!(pane_ids_with_option("%1", "@x", &mock).is_err());
    }

    /// Value-matched, not presence-matched: the two roles must never stand in for
    /// each other, or a file open would respawn the tree pane with an editor.
    #[test]
    fn pane_ids_with_option_value_returns_only_that_role() {
        let mock = MockProcessRunner::new(vec![MockProcessRunner::ok_with_stdout(
            b"%1 \n%2 agent_tree\n%3 editor\n",
        )]);
        let found =
            pane_ids_with_option_value("%2", PANE_ROLE_OPTION, PANE_ROLE_EDITOR, &mock).unwrap();
        assert_eq!(found, vec!["%3".to_string()]);
        assert_eq!(
            mock.recorded_calls()[0].1,
            vec![
                "list-panes",
                "-t",
                "%2",
                "-F",
                "#{pane_id} #{@dispatch_pane_role}",
            ]
        );
    }

    /// The whole value, not a prefix of it: a future role named `editor_split`
    /// must not answer a lookup for `editor`.
    #[test]
    fn pane_ids_with_option_value_matches_the_whole_value() {
        let mock = MockProcessRunner::new(vec![MockProcessRunner::ok_with_stdout(
            b"%1 editor_split\n%2 editor\n",
        )]);
        let found =
            pane_ids_with_option_value("%1", PANE_ROLE_OPTION, PANE_ROLE_EDITOR, &mock).unwrap();
        assert_eq!(found, vec!["%2".to_string()]);
    }

    #[test]
    fn pane_ids_with_option_value_is_empty_when_no_pane_has_that_role() {
        let mock = MockProcessRunner::new(vec![MockProcessRunner::ok_with_stdout(
            b"%1 \n%2 agent_tree\n",
        )]);
        assert!(
            pane_ids_with_option_value("%1", PANE_ROLE_OPTION, PANE_ROLE_EDITOR, &mock)
                .unwrap()
                .is_empty()
        );
    }

    #[test]
    fn pane_ids_with_option_value_fails_on_nonzero_exit() {
        let mock = MockProcessRunner::new(vec![MockProcessRunner::fail("no window")]);
        assert!(
            pane_ids_with_option_value("%1", PANE_ROLE_OPTION, PANE_ROLE_AGENT_TREE, &mock)
                .is_err()
        );
    }

    // --- split_window_full_below_running ---

    #[test]
    fn split_window_full_below_running_issues_correct_args() {
        let mock = MockProcessRunner::new(vec![MockProcessRunner::ok_with_stdout(b"%7\n")]);
        let pane_id = split_window_full_below_running(
            "%3",
            60,
            "/work/wt",
            &["vim", "/work/wt/src/lib.rs"],
            &mock,
        )
        .unwrap();
        assert_eq!(pane_id, "%7");
        let calls = mock.recorded_calls();
        assert_eq!(calls.len(), 1);
        assert_eq!(calls[0].0, "tmux");
        assert_eq!(
            calls[0].1,
            vec![
                "split-window",
                "-v",
                "-f",
                "-d",
                "-l",
                "60%",
                "-t",
                "%3",
                "-c",
                "/work/wt",
                "-P",
                "-F",
                "#{pane_id}",
                "--",
                "vim",
                "/work/wt/src/lib.rs",
            ]
        );
    }

    /// `-f` (span the window) is what makes the geometry independent of which
    /// pane is targeted, and `-d` is what keeps focus in the tree pane. Both are
    /// single-character flags, easy to drop in a refactor and invisible in the
    /// result, so they are asserted by name as well as by the argv above.
    #[test]
    fn split_window_full_below_running_spans_the_window_and_keeps_focus() {
        let mock = MockProcessRunner::new(vec![MockProcessRunner::ok_with_stdout(b"%7\n")]);
        split_window_full_below_running("%3", 60, "/work/wt", &["vi", "a"], &mock).unwrap();
        let args = &mock.recorded_calls()[0].1;
        assert!(args.contains(&"-f".to_string()), "args: {args:?}");
        assert!(args.contains(&"-d".to_string()), "args: {args:?}");
    }

    #[test]
    fn split_window_full_below_running_keeps_argv_elements_separate() {
        let mock = MockProcessRunner::new(vec![MockProcessRunner::ok_with_stdout(b"%7\n")]);
        split_window_full_below_running(
            "%3",
            60,
            "/work/wt",
            &["vim", "-p", "/work/wt/dir with spaces/a.rs"],
            &mock,
        )
        .unwrap();
        let args = &mock.recorded_calls()[0].1;
        assert_eq!(args.last().unwrap(), "/work/wt/dir with spaces/a.rs");
        assert_eq!(args[args.len() - 4], "--");
        assert_eq!(args[args.len() - 3], "vim");
        assert_eq!(args[args.len() - 2], "-p");
    }

    #[test]
    fn split_window_full_below_running_rejects_empty_command() {
        let mock = MockProcessRunner::new(vec![]);
        let err = split_window_full_below_running("%3", 60, "/w", &[], &mock).unwrap_err();
        assert!(err.to_string().contains("command must not be empty"));
    }

    #[test]
    fn split_window_full_below_running_fails_on_nonzero_exit() {
        let mock = MockProcessRunner::new(vec![MockProcessRunner::fail("no space")]);
        let err = split_window_full_below_running("%3", 60, "/w", &["vi", "a"], &mock).unwrap_err();
        assert!(
            err.to_string().contains("split-window failed"),
            "got: {err}"
        );
    }

    /// A window *name* target must be resolved rather than handed to tmux, which
    /// prefix-matches names (see [`window_target`]).
    #[test]
    fn split_window_full_below_running_resolves_a_window_name() {
        let mock = MockProcessRunner::new(vec![MockProcessRunner::ok_with_stdout(b"%7\n")])
            .with_windows(&["task-42"]);
        split_window_full_below_running("task-42", 60, "/w", &["vi", "a"], &mock).unwrap();
        let args = &mock.recorded_calls()[0].1;
        let target = args.iter().position(|a| a == "-t").unwrap() + 1;
        assert_eq!(args[target], mock.pane_id_of("task-42"));
    }

    // --- respawn_pane_running ---

    #[test]
    fn respawn_pane_running_issues_correct_args() {
        let mock = MockProcessRunner::new(vec![MockProcessRunner::ok()]);
        respawn_pane_running("%7", "/work/wt", &["vim", "-p", "/work/wt/a.rs"], &mock).unwrap();
        let calls = mock.recorded_calls();
        assert_eq!(calls.len(), 1);
        assert_eq!(
            calls[0].1,
            vec![
                "respawn-pane",
                "-k",
                "-c",
                "/work/wt",
                "-t",
                "%7",
                "--",
                "vim",
                "-p",
                "/work/wt/a.rs",
            ]
        );
    }

    #[test]
    fn respawn_pane_running_rejects_empty_command() {
        let mock = MockProcessRunner::new(vec![]);
        let err = respawn_pane_running("%7", "/w", &[], &mock).unwrap_err();
        assert!(err.to_string().contains("command must not be empty"));
    }

    #[test]
    fn respawn_pane_running_fails_on_nonzero_exit() {
        let mock = MockProcessRunner::new(vec![MockProcessRunner::fail("boom")]);
        let err = respawn_pane_running("%7", "/w", &["vi", "a"], &mock).unwrap_err();
        assert!(
            err.to_string().contains("respawn-pane failed"),
            "got: {err}"
        );
    }

    // --- set_pane_option ---

    #[test]
    fn set_pane_option_issues_correct_args() {
        let mock = MockProcessRunner::new(vec![MockProcessRunner::ok()]);
        set_pane_option("%7", PANE_ROLE_OPTION, PANE_ROLE_EDITOR, &mock).unwrap();
        assert_eq!(
            mock.recorded_calls()[0].1,
            vec![
                "set-option",
                "-p",
                "-t",
                "%7",
                "@dispatch_pane_role",
                "editor"
            ]
        );
    }

    #[test]
    fn set_pane_option_fails_on_nonzero_exit() {
        let mock = MockProcessRunner::new(vec![MockProcessRunner::fail("bad option")]);
        let err = set_pane_option("%7", "@x", "1", &mock).unwrap_err();
        assert!(err.to_string().contains("set-option failed"), "got: {err}");
    }

    // --- join_pane failure paths ---

    #[test]
    fn join_pane_fails_when_the_window_lookup_fails() {
        let mock = MockProcessRunner::new(vec![MockProcessRunner::fail("no server running")])
            .with_queued_window_lookup();
        let err = join_pane(&test_tmux_window("task-42"), "%1", &mock).unwrap_err();
        assert!(
            err.to_string().contains("no tmux window named 'task-42'"),
            "got: {err}"
        );
    }

    #[test]
    fn join_pane_fails_when_join_pane_command_fails() {
        let mock = MockProcessRunner::new(vec![MockProcessRunner::fail("invalid target")])
            .with_windows(&["task-42"]);
        let err = join_pane(&test_tmux_window("task-42"), "%1", &mock).unwrap_err();
        assert!(err.to_string().contains("join-pane failed"), "got: {err}");
    }

    // --- break_pane_to_window ---

    #[test]
    fn break_pane_to_window_issues_correct_args() {
        let mock = MockProcessRunner::new(vec![MockProcessRunner::ok()]);
        break_pane_to_window("%5", &test_tmux_window("new-win"), &mock).unwrap();
        let calls = mock.recorded_calls();
        assert_eq!(
            calls[0].1,
            vec!["break-pane", "-d", "-s", "%5", "-n", "new-win"]
        );
    }

    #[test]
    fn break_pane_to_window_fails_on_nonzero_exit() {
        let mock = MockProcessRunner::new(vec![MockProcessRunner::fail("no such pane")]);
        let err = break_pane_to_window("%5", &test_tmux_window("new-win"), &mock).unwrap_err();
        assert!(err.to_string().contains("break-pane failed"), "got: {err}");
    }

    // --- kill_pane ---

    #[test]
    fn kill_pane_issues_correct_tmux_args() {
        let mock = MockProcessRunner::new(vec![MockProcessRunner::ok()]);
        kill_pane("%42", &mock).unwrap();
        let calls = mock.recorded_calls();
        assert_eq!(calls[0].1, vec!["kill-pane", "-t", "%42"]);
    }

    #[test]
    fn kill_pane_fails_on_nonzero_exit() {
        let mock = MockProcessRunner::new(vec![MockProcessRunner::fail("no pane")]);
        let err = kill_pane("%42", &mock).unwrap_err();
        assert!(err.to_string().contains("kill-pane failed"), "got: {err}");
    }

    // --- respawn_pane ---

    #[test]
    fn respawn_pane_issues_correct_tmux_args() {
        let mock = MockProcessRunner::new(vec![MockProcessRunner::ok()]);
        respawn_pane("%42", &mock).unwrap();
        let calls = mock.recorded_calls();
        assert_eq!(calls[0].1, vec!["respawn-pane", "-k", "-t", "%42"]);
    }

    #[test]
    fn respawn_pane_fails_on_nonzero_exit() {
        let mock = MockProcessRunner::new(vec![MockProcessRunner::fail("no such pane")]);
        let err = respawn_pane("%42", &mock).unwrap_err();
        assert!(
            err.to_string().contains("respawn-pane failed"),
            "got: {err}"
        );
    }

    // --- pane_id_for_window ---

    #[test]
    fn pane_id_for_window_issues_correct_args() {
        let mock = MockProcessRunner::new(vec![]).with_windows(&COLLIDING);
        let short = pane_id_for_window(&test_tmux_window("task-4"), &mock).unwrap();
        let long = pane_id_for_window(&test_tmux_window("task-42"), &mock).unwrap();
        assert_eq!(short, mock.pane_id_of("task-4"));
        assert_eq!(long, mock.pane_id_of("task-42"));
        assert_ne!(short, long, "colliding names are different windows");
    }

    #[test]
    fn pane_id_for_window_fails_on_empty_output() {
        // The case real tmux produces for a missing window: exit 0 with no
        // output. Under the old `display-message` implementation that returned
        // Ok(""), and `swap-pane -s ''` exits 0 too, so the bad id propagated
        // silently. A no-row listing must stay a hard miss.
        let mock = MockProcessRunner::new(vec![MockProcessRunner::ok_with_stdout(b"\n")])
            .with_queued_window_lookup();
        let err = pane_id_for_window(&test_tmux_window("task-999"), &mock).unwrap_err();
        assert!(
            err.to_string().contains("no tmux window named 'task-999'"),
            "got: {err}"
        );
    }

    #[test]
    fn pane_id_for_window_fails_on_nonzero_exit() {
        let mock = MockProcessRunner::new(vec![MockProcessRunner::fail("no such window")])
            .with_queued_window_lookup();
        let err = pane_id_for_window(&test_tmux_window("task-42"), &mock).unwrap_err();
        assert!(
            err.to_string().contains("no tmux window named 'task-42'"),
            "got: {err}"
        );
    }

    /// The prefix case, which used to return the sibling's pane ID — a wrong ID
    /// that then propagated into swaps, splits and the split-pane's tracked pane.
    #[test]
    fn pane_id_for_window_fails_rather_than_returning_a_siblings_pane() {
        let mock = MockProcessRunner::new(vec![]).with_windows(&["dispatch", "task-42"]);
        let err = pane_id_for_window(&test_tmux_window("task-4"), &mock).unwrap_err();
        assert!(
            err.to_string().contains("no tmux window named 'task-4'"),
            "got: {err}"
        );
    }

    // --- swap_pane ---

    #[test]
    fn swap_pane_issues_correct_tmux_args() {
        let mock = MockProcessRunner::new(vec![MockProcessRunner::ok()]);
        swap_pane("%1", "%2", &mock).unwrap();
        let calls = mock.recorded_calls();
        assert_eq!(calls[0].1, vec!["swap-pane", "-d", "-s", "%1", "-t", "%2"]);
    }

    #[test]
    fn swap_pane_fails_on_nonzero_exit() {
        let mock = MockProcessRunner::new(vec![MockProcessRunner::fail("no pane")]);
        let err = swap_pane("%1", "%2", &mock).unwrap_err();
        assert!(err.to_string().contains("swap-pane failed"), "got: {err}");
    }

    // --- current_pane_id ---

    #[test]
    fn current_pane_id_issues_correct_args() {
        let mock = MockProcessRunner::new(vec![MockProcessRunner::ok_with_stdout(b"%42\n")]);
        let result = current_pane_id(&mock).unwrap();
        assert_eq!(result, "%42");
        let calls = mock.recorded_calls();
        assert_eq!(calls[0].1, vec!["display-message", "-p", "#{pane_id}"]);
    }

    #[test]
    fn current_pane_id_fails_on_nonzero_exit() {
        let mock = MockProcessRunner::new(vec![MockProcessRunner::fail("no session")]);
        let err = current_pane_id(&mock).unwrap_err();
        assert!(
            err.to_string().contains("display-message failed"),
            "got: {err}"
        );
    }

    // --- pane_exists ---

    #[test]
    fn pane_exists_finds_the_pane_in_the_listing() {
        let mock =
            MockProcessRunner::new(vec![MockProcessRunner::ok_with_stdout(b"%1\n%42\n%7\n")]);
        assert!(pane_exists("%42", &mock));
        assert_eq!(
            mock.recorded_calls()[0].1,
            vec!["list-panes", "-a", "-F", "#{pane_id}"],
            "must query the pane listing, not display-message — see pane_exists' docs"
        );
    }

    #[test]
    fn pane_exists_is_false_when_the_pane_is_absent_from_the_listing() {
        // The case the old implementation could never detect: tmux exits 0 for an
        // unknown pane target, so only a membership test sees a closed pane.
        let mock = MockProcessRunner::new(vec![MockProcessRunner::ok_with_stdout(b"%1\n%7\n")]);
        assert!(!pane_exists("%42", &mock));
    }

    #[test]
    fn pane_exists_does_not_match_a_pane_id_prefix() {
        // `%4` must not satisfy a query for `%42`, nor the reverse.
        let mock = MockProcessRunner::new(vec![MockProcessRunner::ok_with_stdout(b"%4\n")]);
        assert!(!pane_exists("%42", &mock));
        let mock = MockProcessRunner::new(vec![MockProcessRunner::ok_with_stdout(b"%42\n")]);
        assert!(!pane_exists("%4", &mock));
    }

    #[test]
    fn pane_exists_returns_false_when_no_server_is_running() {
        let mock = MockProcessRunner::new(vec![MockProcessRunner::fail("no server running")]);
        assert!(!pane_exists("%42", &mock));
    }

    #[test]
    fn pane_exists_returns_false_on_runner_error() {
        let mock = MockProcessRunner::new(vec![Err(anyhow::anyhow!("binary not found"))]);
        assert!(!pane_exists("%42", &mock));
    }

    // --- capture_pane ---

    #[test]
    fn capture_pane_issues_correct_args_against_the_resolved_target() {
        let mock = MockProcessRunner::new(vec![MockProcessRunner::ok_with_stdout(b"hello\n")])
            .with_windows(&["task-42"]);
        let result = capture_pane(&mock.pane_id_of("task-42"), &mock).unwrap();
        assert_eq!(result, "hello");
        let calls = mock.recorded_calls();
        assert_eq!(calls.len(), 1);
        assert_eq!(
            calls[0].1,
            vec!["capture-pane", "-p", "-t", &mock.pane_id_of("task-42")]
        );
    }

    // --- set_focus_events failure ---

    #[test]
    fn set_focus_events_fails_on_nonzero_exit() {
        let mock = MockProcessRunner::new(vec![MockProcessRunner::fail("no server running")]);
        let err = set_focus_events(&mock).unwrap_err();
        assert!(
            err.to_string().contains("set-option focus-events failed"),
            "got: {err}"
        );
    }

    // --- focus_events_enabled runner error ---

    #[test]
    fn focus_events_enabled_returns_false_on_runner_error() {
        let mock = MockProcessRunner::new(vec![Err(anyhow::anyhow!("tmux not found"))]);
        assert!(!focus_events_enabled(&mock));
    }

    // --- bind_key / unbind_key failure paths ---

    #[test]
    fn bind_key_fails_on_nonzero_exit() {
        let mock = MockProcessRunner::new(vec![MockProcessRunner::fail("invalid key")]);
        let err = bind_key("space", "select-window -t dispatch", &mock).unwrap_err();
        assert!(err.to_string().contains("bind-key failed"), "got: {err}");
    }

    #[test]
    fn unbind_key_fails_on_nonzero_exit() {
        let mock = MockProcessRunner::new(vec![MockProcessRunner::fail("no key bound")]);
        let err = unbind_key("space", &mock).unwrap_err();
        assert!(err.to_string().contains("unbind-key failed"), "got: {err}");
    }

    // --- select_pane failure ---

    #[test]
    fn select_pane_fails_on_nonzero_exit() {
        let mock = MockProcessRunner::new(vec![MockProcessRunner::fail("no such pane")]);
        let err = select_pane("%42", &mock).unwrap_err();
        assert!(err.to_string().contains("select-pane failed"), "got: {err}");
    }

    // --- pane lookups by window NAME ---
    //
    // The `%N`-target cases live with the other pane-lookup tests above; these two
    // cover what only a *name* target exercises, and are what the deleted
    // `inactive_pane_id` tests used to guarantee.

    /// The window is named to `list-panes` by its resolved pane, so the panes
    /// returned cannot be a prefix-matched sibling's.
    #[test]
    fn pane_ids_with_option_resolves_a_window_name_to_its_own_panes() {
        let mock = MockProcessRunner::new(vec![MockProcessRunner::ok_with_stdout(b"%3 \n%7 1\n")])
            .with_windows(&COLLIDING);
        let found = pane_ids_with_option("task-42", PANE_ROLE_OPTION, &mock).unwrap();
        assert_eq!(found, vec!["%7".to_string()]);
        assert_eq!(
            mock.recorded_calls()[0].1,
            vec![
                "list-panes",
                "-t",
                &mock.pane_id_of("task-42"),
                "-F",
                "#{pane_id} #{@dispatch_pane_role}",
            ]
        );
    }

    /// An absent window must error rather than reporting "no marked pane" after
    /// inspecting a prefix-matched sibling's panes.
    #[test]
    fn pane_ids_with_option_fails_when_window_is_absent() {
        let mock = MockProcessRunner::new(vec![]).with_windows(&["dispatch", "task-42"]);
        let err = pane_ids_with_option("task-4", "@x", &mock).unwrap_err();
        assert!(
            err.to_string().contains("no tmux window named 'task-4'"),
            "got: {err}"
        );
    }

    // --- window_target ---
    //
    // These assert the lookup itself, so they queue its response positionally
    // (`with_queued_window_lookup`) rather than letting the mock resolve out of
    // band — the listing bytes *are* the fixture here.

    /// A runner whose window lookup is answered from `responses`, for the tests
    /// whose subject is resolution.
    fn queued(responses: Vec<Result<Output>>) -> MockProcessRunner {
        MockProcessRunner::new(responses).with_queued_window_lookup()
    }

    #[test]
    fn window_target_asks_tmux_for_an_exact_name_match() {
        let mock = queued(vec![MockProcessRunner::ok_with_stdout(b"1 %1 task-4\n")]);
        window_target("task-4", &mock).unwrap();
        let calls = mock.recorded_calls();
        assert_eq!(calls.len(), 1);
        assert_eq!(calls[0].0, "tmux");
        assert_eq!(
            calls[0].1,
            vec![
                "list-panes",
                "-a",
                "-f",
                "#{==:#{window_name},task-4}",
                "-F",
                WINDOW_PANE_FORMAT,
            ],
            "the filter must compare window_name for equality, not prefix"
        );
    }

    #[test]
    fn window_target_resolves_an_exact_name_to_its_active_pane() {
        // Two panes in task-42, only one active: the active pane is the one every
        // command this module issues means by "the window".
        let mock = queued(vec![MockProcessRunner::ok_with_stdout(
            b"0 %5 task-42\n1 %6 task-42\n",
        )]);
        assert_eq!(window_target("task-42", &mock).unwrap(), "%6");
    }

    /// The local name comparison is not redundant with tmux's `-f` filter: a name
    /// carrying `,` or `}` could confuse `#{==:…}` into returning a row for a
    /// different window. Re-checking here turns that into a miss, never a wrong
    /// hit — so a hostile listing cannot make resolution target the wrong pane.
    #[test]
    fn window_target_rechecks_the_name_the_filter_returned() {
        let mock = queued(vec![MockProcessRunner::ok_with_stdout(b"1 %9 task-3782\n")]);
        let err = window_target("task-378", &mock).unwrap_err();
        assert!(
            err.to_string().contains("no tmux window named 'task-378'"),
            "a row for the wrong window must not resolve, got: {err}"
        );
    }

    #[test]
    fn window_target_rejects_duplicate_names() {
        let mock = queued(vec![MockProcessRunner::ok_with_stdout(
            b"1 %1 task-42\n1 %2 task-42\n",
        )]);
        let err = window_target("task-42", &mock).unwrap_err();
        assert!(
            err.to_string().contains("multiple tmux windows"),
            "got: {err}"
        );
    }

    /// Window names may contain spaces (the board window inherits whatever the
    /// user had), so the name must be the parsed remainder, not one field.
    #[test]
    fn window_target_handles_names_containing_spaces() {
        let mock = queued(vec![MockProcessRunner::ok_with_stdout(
            b"1 %3 my project shell\n",
        )]);
        assert_eq!(window_target("my project shell", &mock).unwrap(), "%3");
    }

    #[test]
    fn window_target_passes_through_pane_ids_and_empty_targets() {
        for target in ["%42", "%0", ""] {
            let mock = queued(vec![]);
            assert_eq!(window_target(target, &mock).unwrap(), target);
            assert!(
                mock.recorded_calls().is_empty(),
                "an already-exact target must not be looked up: {target:?}"
            );
        }
    }

    /// A window can genuinely be named `%foo`, so only `%`-plus-digits is a pane
    /// ID. Anything else takes the lookup path and gets this module's clear
    /// "no tmux window named" error rather than tmux's "can't find pane".
    #[test]
    fn window_target_looks_up_names_that_merely_start_with_percent() {
        for target in ["%foo", "%", "%1a"] {
            let mock = queued(vec![MockProcessRunner::ok_with_stdout(b"")]);
            let err = window_target(target, &mock).unwrap_err();
            assert!(
                err.to_string()
                    .contains(&format!("no tmux window named '{target}'")),
                "{target:?} should be looked up as a name, got: {err}"
            );
            assert_eq!(
                mock.recorded_calls().len(),
                1,
                "{target:?} should have been looked up"
            );
        }
    }

    #[test]
    fn window_target_treats_a_failed_lookup_as_not_found() {
        // No server running: there is genuinely no such window. Mirrors
        // `list_all_window_names`, which maps the same failure to an empty list.
        let mock = queued(vec![MockProcessRunner::fail("no server running")]);
        let err = window_target("task-42", &mock).unwrap_err();
        assert!(
            err.to_string().contains("no tmux window named 'task-42'"),
            "got: {err}"
        );
    }

    #[test]
    fn window_target_propagates_a_runner_error() {
        let mock = queued(vec![Err(anyhow::anyhow!("tmux: command not found"))]);
        let err = window_target("task-42", &mock).unwrap_err();
        assert!(err.to_string().contains("command not found"), "got: {err}");
    }

    /// `window_name_in_lookup` must invert `window_filter` exactly, or
    /// `MockProcessRunner` silently stops recognising the lookup and every mock
    /// test that relies on out-of-band resolution starts failing obscurely.
    #[test]
    fn window_name_in_lookup_inverts_the_filter_this_module_builds() {
        let filter = window_filter("task-42");
        let args = ["list-panes", "-a", "-f", &filter, "-F", WINDOW_PANE_FORMAT];
        assert_eq!(window_name_in_lookup(&args), Some("task-42"));
    }

    #[test]
    fn window_name_in_lookup_ignores_other_calls() {
        assert_eq!(
            window_name_in_lookup(&["list-windows", "-a", "-F", "#{window_name}"]),
            None
        );
        assert_eq!(
            window_name_in_lookup(&["list-panes", "-t", "%1", "-F", "#{pane_id}"]),
            None
        );
    }
}
