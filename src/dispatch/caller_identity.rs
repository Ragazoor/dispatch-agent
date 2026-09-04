//! Where an agent's per-task MCP configuration is put, and how a launch names
//! it (`AgentCarriesItsOwnCallerIdentity`, docs/specs/dispatch.allium).
//!
//! # Why the launcher has to say who the agent is
//!
//! `dispatch setup` installs a `headersHelper` in Claude Code's user-global
//! config, and `dispatch caller-headers` answers it by reading its own working
//! directory for a `.worktrees/<id>-<slug>` segment. Claude Code runs a helper
//! declared there **from its own config directory**, not from the session's, so
//! that reading is never of the agent's worktree. The helper answers
//! `X-Caller-Kind: session` for every dispatched agent — correctly for the
//! directory it was handed, and uselessly for the question being asked.
//!
//! Nothing in the environment the helper receives names the session's
//! workspace, so this is not repairable at the agent's end. The launcher, which
//! already knows the task id, states it instead.
//!
//! # What lives here, and what does not
//!
//! This module owns *placement*: which directory the configuration goes in, and
//! how a launch command names it. Deriving the entry's content is knowledge
//! about the file `dispatch setup` wrote, so it lives next to that writing in
//! `setup::config` — read-back beside write, so the two cannot drift.
//!
//! The file goes in the worktree's git administrative directory
//! (`<repo>/.git/worktrees/<name>/`), so it never appears in `git status` for
//! an agent to commit, and `git worktree remove` deletes it as part of the
//! teardown that already runs (`CallerIdentityConfigGoesWithTheWorktree` in
//! docs/specs/tasks.allium).
//!
//! # When a failure is worth a log line
//!
//! Silent where the inputs say this launch was never meant to carry identity: a
//! path that is not a linked worktree, or a runner told about no config file,
//! has nothing to derive from and nothing to report. Warned where it should
//! have carried one and could not — because that outcome is indistinguishable,
//! at the MCP end, from a session that is not an agent at all
//! (`CallerIdentityDependsOnTheLaunch` in docs/specs/mcp-task-tools.allium).

use std::path::PathBuf;

use crate::models::TaskId;
use crate::process::ProcessRunner;
use crate::worktree_admin::worktree_admin_dir;

/// Basename of the per-task config inside the worktree's git admin directory.
const CONFIG_FILE: &str = "dispatch-mcp.json";

/// Write the per-task MCP config for an agent about to be launched in
/// `worktree_path`, returning the path a launch command should name.
///
/// `None` on every failure, and the caller must then launch WITHOUT the flag:
/// naming a file that does not exist would trade a silent loss of caller
/// identity — the state the system was already in — for a broken launch.
fn write_agent_mcp_config(
    runner: &dyn ProcessRunner,
    worktree_path: &str,
    task_id: TaskId,
) -> Option<PathBuf> {
    // Cheapest guard first, and the only one whose failure means "this is not
    // an agent worktree" rather than "this should have worked".
    let admin_dir = worktree_admin_dir(worktree_path)?;
    let claude_json = runner.claude_json_path()?;
    let Some(config) = crate::setup::dispatch_entry_identifying(&claude_json, task_id) else {
        tracing::warn!(
            claude_json = %claude_json.display(),
            "no dispatch MCP entry to derive a caller-identity config from; \
             the agent's MCP calls will carry no caller identity"
        );
        return None;
    };
    let path = admin_dir.join(CONFIG_FILE);
    match crate::setup::write_json_file(&path, &config) {
        Ok(()) => Some(path),
        Err(error) => {
            tracing::warn!(
                error = %format!("{error:#}"),
                path = %path.display(),
                "failed to write the agent's caller-identity MCP config; \
                 launching without it"
            );
            None
        }
    }
}

/// The ` --mcp-config <path>` fragment naming this task's configuration, or an
/// empty string when none could be written.
///
/// Deliberately no `--strict-mcp-config`: the per-task entry overrides the
/// same-named user entry, and leaves every other MCP server the operator
/// configured available to the agent.
pub(super) fn mcp_config_flag(
    runner: &dyn ProcessRunner,
    worktree_path: &str,
    task_id: TaskId,
) -> String {
    match write_agent_mcp_config(runner, worktree_path, task_id) {
        Some(path) => format!(
            " --mcp-config {}",
            crate::process::shell_quote(&path.to_string_lossy())
        ),
        None => String::new(),
    }
}

#[cfg(test)]
#[allow(clippy::unwrap_used, clippy::expect_used)]
mod tests {
    use super::*;
    use crate::dispatch::tests::claude_json_with_dispatch_entry;
    use crate::process::MockProcessRunner;
    use crate::worktree_admin::tests::make_linked_worktree;
    use serde_json::Value;
    use std::path::Path;

    #[test]
    fn the_config_is_written_into_the_worktrees_git_admin_dir() {
        let dir = tempfile::tempdir().unwrap();
        let (worktree, admin) = make_linked_worktree(dir.path(), "42-fix-bug");
        let mock = MockProcessRunner::new(vec![])
            .with_claude_json(claude_json_with_dispatch_entry(dir.path()));

        let path = write_agent_mcp_config(&mock, &worktree, TaskId(4486)).unwrap();

        assert_eq!(path, admin.join("dispatch-mcp.json"));
        let written: Value =
            serde_json::from_str(&std::fs::read_to_string(&path).unwrap()).unwrap();
        assert_eq!(
            written["mcpServers"]["dispatch"]["headers"]["X-Caller-Task-Id"],
            "4486"
        );
    }

    #[test]
    fn the_config_never_lands_inside_the_worktree_itself() {
        // Anything written in the worktree shows up in `git status`, where an
        // agent running `git add -A` can commit it into the user's repo.
        let dir = tempfile::tempdir().unwrap();
        let (worktree, _admin) = make_linked_worktree(dir.path(), "42-fix-bug");
        let mock = MockProcessRunner::new(vec![])
            .with_claude_json(claude_json_with_dispatch_entry(dir.path()));

        write_agent_mcp_config(&mock, &worktree, TaskId(4486)).unwrap();

        assert!(!Path::new(&worktree).join("dispatch-mcp.json").exists());
    }

    #[test]
    fn no_claude_json_means_no_config() {
        let dir = tempfile::tempdir().unwrap();
        let (worktree, _admin) = make_linked_worktree(dir.path(), "42-fix-bug");
        assert!(
            write_agent_mcp_config(&MockProcessRunner::new(vec![]), &worktree, TaskId(1)).is_none()
        );
    }

    #[test]
    fn a_claude_json_without_a_dispatch_entry_means_no_config() {
        let dir = tempfile::tempdir().unwrap();
        let (worktree, _admin) = make_linked_worktree(dir.path(), "42-fix-bug");
        let claude_json = dir.path().join("claude.json");
        std::fs::write(&claude_json, r#"{"mcpServers":{"other":{}}}"#).unwrap();
        let mock = MockProcessRunner::new(vec![]).with_claude_json(claude_json);
        assert!(write_agent_mcp_config(&mock, &worktree, TaskId(1)).is_none());
    }

    #[test]
    fn a_worktree_with_no_admin_dir_means_no_config() {
        let dir = tempfile::tempdir().unwrap();
        let plain = dir.path().join("plain");
        std::fs::create_dir_all(&plain).unwrap();
        let mock = MockProcessRunner::new(vec![])
            .with_claude_json(claude_json_with_dispatch_entry(dir.path()));
        assert!(
            write_agent_mcp_config(&mock, plain.to_str().unwrap(), TaskId(1)).is_none(),
            "no path is better than a path that does not exist"
        );
    }

    // -- mcp_config_flag --

    #[test]
    fn no_config_contributes_no_flag() {
        let dir = tempfile::tempdir().unwrap();
        assert_eq!(
            mcp_config_flag(
                &MockProcessRunner::new(vec![]),
                dir.path().to_str().unwrap(),
                TaskId(1)
            ),
            ""
        );
    }

    #[test]
    fn a_config_is_named_but_never_made_exclusive() {
        let dir = tempfile::tempdir().unwrap();
        let (worktree, admin) = make_linked_worktree(dir.path(), "42-fix-bug");
        let mock = MockProcessRunner::new(vec![])
            .with_claude_json(claude_json_with_dispatch_entry(dir.path()));

        let flag = mcp_config_flag(&mock, &worktree, TaskId(1));

        assert_eq!(
            flag,
            format!(
                " --mcp-config {}",
                admin.join("dispatch-mcp.json").display()
            )
        );
        assert!(
            !flag.contains("--strict-mcp-config"),
            "strict mode would strip every other MCP server the operator configured"
        );
    }

    #[test]
    fn a_config_path_with_a_space_is_shell_quoted() {
        // The flag is interpolated into a command string sent through tmux
        // send-keys, so an unquoted space would split it into two arguments.
        let dir = tempfile::tempdir().unwrap();
        let spaced = dir.path().join("a b");
        std::fs::create_dir_all(&spaced).unwrap();
        let (worktree, _admin) = make_linked_worktree(&spaced, "42-fix-bug");
        let mock = MockProcessRunner::new(vec![])
            .with_claude_json(claude_json_with_dispatch_entry(dir.path()));

        let flag = mcp_config_flag(&mock, &worktree, TaskId(1));

        assert!(
            flag.starts_with(" --mcp-config '") && flag.ends_with('\''),
            "a path with a space must survive as one shell word, got: {flag}"
        );
    }
}
