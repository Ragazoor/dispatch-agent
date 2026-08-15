//! Tests for the embedded hook scripts.
//!
//! Hook installation itself is part of `install_plugin_in` (see [`super::plugins`]) — the
//! hook bytes live in the plugin's `hooks/` directory and are embedded via
//! `PLUGIN_DIR`. This module owns the suite that asserts hook script behaviour
//! and the `hooks.json` metadata so the hook contract is in one obvious place.

#[cfg(test)]
mod tests {
    #![allow(clippy::unwrap_used, clippy::expect_used)]
    use super::super::plugins::PLUGIN_DIR;
    use serde_json::Value;

    fn hook_script() -> &'static str {
        PLUGIN_DIR
            .get_file("hooks/scripts/task-status-hook")
            .expect("task-status-hook must be embedded")
            .contents_utf8()
            .expect("task-status-hook must be UTF-8")
    }

    #[test]
    fn hook_script_is_valid_bash() {
        assert!(hook_script().starts_with("#!/usr/bin/env bash"));
    }

    #[test]
    fn hook_script_handles_all_events() {
        let s = hook_script();
        // PreToolUse and PostToolUse share a case arm (PreToolUse|PostToolUse)
        assert!(s.contains("PreToolUse"), "hook must handle PreToolUse");
        assert!(s.contains("PostToolUse"), "hook must handle PostToolUse");
        assert!(s.contains("Stop)"), "hook must handle Stop");
        assert!(s.contains("Notification)"), "hook must handle Notification");
        assert!(
            s.contains("UserPromptSubmit)"),
            "hook must handle UserPromptSubmit"
        );
        assert!(
            s.contains("SubagentStart") && s.contains("SubagentStop"),
            "hook must handle the subagent lifecycle events"
        );
        assert!(s.contains("SessionStart)"), "hook must handle SessionStart");
    }

    #[test]
    fn hook_script_skips_dispatch_mcp_in_pretooluse() {
        // The PreToolUse handler must read tool_name from the JSON input
        // and skip dispatch MCP tool calls to avoid clobbering review status
        // during wrap-up (get_task and wrap_up would otherwise set running).
        let s = hook_script();
        assert!(
            s.contains("tool_name"),
            "hook must extract tool_name from PreToolUse input"
        );
        assert!(
            s.contains("mcp__dispatch__"),
            "hook must skip mcp__dispatch__ tools in PreToolUse"
        );
    }

    #[test]
    fn hook_script_uses_dispatch_hook_subcommand() {
        let s = hook_script();
        assert!(
            s.contains("dispatch hook"),
            "must use new `dispatch hook` subcommand"
        );
        assert!(s.contains("pre_tool_use"));
        assert!(s.contains("notification"));
        assert!(s.contains("stop"));
        assert!(s.contains("user_prompt_submit"));
        assert!(
            !s.contains("--sub-status"),
            "old --sub-status flag must not appear"
        );
        assert!(
            !s.contains("--needs-input"),
            "deprecated --needs-input flag must not appear"
        );
    }

    #[test]
    fn hook_script_forwards_notification_type_as_kind() {
        // The Notification handler must read `notification_type` from the hook
        // JSON and forward it as `--kind` so the Rust side can classify the
        // notification (raise / clear / ignore) instead of always needs_input.
        let s = hook_script();
        assert!(
            s.contains("notification_type"),
            "hook must extract notification_type from the Notification payload"
        );
        assert!(
            s.contains("--kind"),
            "hook must forward notification_type as the --kind argument"
        );
    }

    #[test]
    fn hook_script_forwards_post_tool_use_file_events() {
        // Additive to the PreToolUse|PostToolUse arm: on PostToolUse only, for
        // tracked tools, the script must forward to the new hook-file-event
        // subcommand, extracting the path from the per-tool field.
        let s = hook_script();
        assert!(
            s.contains("hook-file-event"),
            "hook must forward tracked-tool PostToolUse calls to `dispatch hook-file-event`"
        );
        assert!(
            s.contains("tool_input.file_path"),
            "Read/Write/Edit must extract tool_input.file_path"
        );
        assert!(
            s.contains("tool_input.notebook_path"),
            "NotebookEdit must extract tool_input.notebook_path, not file_path"
        );
    }

    #[test]
    fn hook_script_extracts_task_id_from_git_branch() {
        // Agents commonly cd into subdirectories of the worktree. The hook
        // must still resolve the task ID via `git branch --show-current` so
        // PreToolUse/Stop/Notification keep firing when the agent's cwd is
        // below the worktree root.
        let s = hook_script();
        assert!(
            s.contains("git") && s.contains("branch"),
            "task-status-hook must derive the task ID from the git branch \
             so it works from subdirectories of the worktree"
        );
    }

    /// Shared scaffolding for tests that run the real `task-status-hook`
    /// script under bash: a git repo checked out on `branch`, the embedded
    /// script dropped to a real executable file, and a `dispatch` shim on
    /// `PATH` that logs its args instead of touching a live database.
    /// Returns `(tempdir, repo_dir, script_path, observed_log, path_env)` —
    /// the `TempDir` must be kept alive for the paths within it to remain
    /// valid.
    #[cfg(unix)]
    fn spawn_hook_harness(
        branch: &str,
    ) -> (
        tempfile::TempDir,
        std::path::PathBuf,
        std::path::PathBuf,
        std::path::PathBuf,
        String,
    ) {
        use std::os::unix::fs::PermissionsExt;

        let tmp = tempfile::tempdir().expect("tempdir");
        let repo = tmp.path().join("repo");
        std::fs::create_dir_all(&repo).unwrap();
        run(&["git", "init", "-q", "-b", branch], &repo);
        run(&["git", "config", "user.email", "t@e.st"], &repo);
        run(&["git", "config", "user.name", "T"], &repo);
        std::fs::write(repo.join("README"), "x").unwrap();
        run(&["git", "add", "."], &repo);
        run(&["git", "commit", "-q", "-m", "init"], &repo);

        // Drop the embedded script to a real file so bash can execute it.
        let script_path = tmp.path().join("task-status-hook");
        std::fs::write(&script_path, hook_script()).unwrap();
        let mut perm = std::fs::metadata(&script_path).unwrap().permissions();
        perm.set_mode(0o755);
        std::fs::set_permissions(&script_path, perm).unwrap();

        // Shim `dispatch` on PATH so we can observe the call without invoking
        // the real binary or touching the live database.
        let bin = tmp.path().join("bin");
        std::fs::create_dir_all(&bin).unwrap();
        let observed = tmp.path().join("dispatch.log");
        let shim = format!(
            "#!/usr/bin/env bash\necho \"$@\" >> {}\n",
            observed.display()
        );
        let dispatch_shim = bin.join("dispatch");
        std::fs::write(&dispatch_shim, shim).unwrap();
        let mut p = std::fs::metadata(&dispatch_shim).unwrap().permissions();
        p.set_mode(0o755);
        std::fs::set_permissions(&dispatch_shim, p).unwrap();

        let path = format!(
            "{}:{}",
            bin.display(),
            std::env::var("PATH").unwrap_or_default()
        );
        (tmp, repo, script_path, observed, path)
    }

    /// Run `script_path` under bash with `PATH` set to `path_env`, feeding it
    /// `payload` on stdin. Panics if the hook exits non-zero.
    #[cfg(unix)]
    fn invoke_hook(
        script_path: &std::path::Path,
        cwd: &std::path::Path,
        path_env: &str,
        payload: &str,
    ) {
        use std::io::Write;
        use std::process::{Command, Stdio};

        let mut child = Command::new("bash")
            .arg(script_path)
            .env("PATH", path_env)
            .current_dir(cwd)
            .stdin(Stdio::piped())
            .stdout(Stdio::piped())
            .stderr(Stdio::piped())
            .spawn()
            .expect("spawn hook");
        child
            .stdin
            .as_mut()
            .unwrap()
            .write_all(payload.as_bytes())
            .unwrap();
        assert!(
            child.wait().expect("wait").success(),
            "hook script exited non-zero"
        );
    }

    #[cfg(unix)]
    #[test]
    fn hook_resolves_task_id_from_worktree_subdirectory() {
        let (_tmp, repo, script_path, observed, path) = spawn_hook_harness("567-foo");
        let sub = repo.join("sub").join("deep");
        std::fs::create_dir_all(&sub).unwrap();

        let payload = format!(
            r#"{{"cwd":"{}","hook_event_name":"PreToolUse","tool_name":"Read"}}"#,
            sub.display()
        );
        invoke_hook(&script_path, &sub, &path, &payload);

        let log = std::fs::read_to_string(&observed).unwrap_or_default();
        assert!(
            log.contains("hook 567 pre_tool_use"),
            "expected `dispatch hook 567 pre_tool_use` to be invoked from a \
             subdirectory of the worktree; got: {log:?}"
        );
    }

    #[cfg(unix)]
    #[test]
    fn hook_dispatches_user_prompt_submit_event() {
        use std::io::Write;
        use std::os::unix::fs::PermissionsExt;
        use std::process::{Command, Stdio};

        let tmp = tempfile::tempdir().expect("tempdir");
        let repo = tmp.path().join("repo");
        std::fs::create_dir_all(&repo).unwrap();
        run(&["git", "init", "-q", "-b", "789-bar"], &repo);
        run(&["git", "config", "user.email", "t@e.st"], &repo);
        run(&["git", "config", "user.name", "T"], &repo);
        std::fs::write(repo.join("README"), "x").unwrap();
        run(&["git", "add", "."], &repo);
        run(&["git", "commit", "-q", "-m", "init"], &repo);

        let script_path = tmp.path().join("task-status-hook");
        std::fs::write(&script_path, hook_script()).unwrap();
        let mut perm = std::fs::metadata(&script_path).unwrap().permissions();
        perm.set_mode(0o755);
        std::fs::set_permissions(&script_path, perm).unwrap();

        let bin = tmp.path().join("bin");
        std::fs::create_dir_all(&bin).unwrap();
        let observed = tmp.path().join("dispatch.log");
        let shim = format!(
            "#!/usr/bin/env bash\necho \"$@\" >> {}\n",
            observed.display()
        );
        let dispatch_shim = bin.join("dispatch");
        std::fs::write(&dispatch_shim, shim).unwrap();
        let mut p = std::fs::metadata(&dispatch_shim).unwrap().permissions();
        p.set_mode(0o755);
        std::fs::set_permissions(&dispatch_shim, p).unwrap();

        let path = format!(
            "{}:{}",
            bin.display(),
            std::env::var("PATH").unwrap_or_default()
        );
        let payload = format!(
            r#"{{"cwd":"{}","hook_event_name":"UserPromptSubmit","prompt":"hi"}}"#,
            repo.display()
        );
        let mut child = Command::new("bash")
            .arg(&script_path)
            .env("PATH", &path)
            .current_dir(&repo)
            .stdin(Stdio::piped())
            .stdout(Stdio::piped())
            .stderr(Stdio::piped())
            .spawn()
            .expect("spawn hook");
        child
            .stdin
            .as_mut()
            .unwrap()
            .write_all(payload.as_bytes())
            .unwrap();
        let status = child.wait().expect("wait");
        assert!(status.success(), "hook script exited non-zero");

        let log = std::fs::read_to_string(&observed).unwrap_or_default();
        assert!(
            log.contains("hook 789 user_prompt_submit"),
            "expected `dispatch hook 789 user_prompt_submit` to be invoked; got: {log:?}"
        );
    }

    #[cfg(unix)]
    #[test]
    fn hook_forwards_notification_kind_from_payload() {
        let (_tmp, repo, script_path, observed, path) = spawn_hook_harness("789-notif");

        // With notification_type present -> forwarded as --kind.
        invoke_hook(
            &script_path,
            &repo,
            &path,
            r#"{"cwd":".","hook_event_name":"Notification","notification_type":"auth_success"}"#,
        );
        // Without notification_type -> plain notification, no --kind.
        invoke_hook(
            &script_path,
            &repo,
            &path,
            r#"{"cwd":".","hook_event_name":"Notification"}"#,
        );

        let log = std::fs::read_to_string(&observed).unwrap_or_default();
        assert!(
            log.contains("hook 789 notification --kind auth_success"),
            "expected notification_type forwarded as --kind; got: {log:?}"
        );
        assert!(
            log.lines().any(|l| l.trim() == "hook 789 notification"),
            "expected a plain `hook 789 notification` when notification_type absent; got: {log:?}"
        );
    }

    #[cfg(unix)]
    #[test]
    fn hook_forwards_file_event_on_post_tool_use_for_each_tracked_tool() {
        // Read/Write/Edit extract tool_input.file_path; NotebookEdit extracts
        // tool_input.notebook_path instead (and must ignore a stray file_path
        // in the same payload).
        for (id, tool, path, tool_input) in [
            (
                111,
                "Read",
                "/tmp/foo.rs",
                r#"{"file_path":"/tmp/foo.rs"}"#.to_string(),
            ),
            (
                112,
                "Write",
                "/tmp/bar.rs",
                r#"{"file_path":"/tmp/bar.rs"}"#.to_string(),
            ),
            (
                113,
                "Edit",
                "/tmp/baz.rs",
                r#"{"file_path":"/tmp/baz.rs"}"#.to_string(),
            ),
            (
                114,
                "NotebookEdit",
                "/tmp/nb.ipynb",
                r#"{"notebook_path":"/tmp/nb.ipynb","file_path":"/tmp/wrong.rs"}"#.to_string(),
            ),
        ] {
            let (_tmp, repo, script_path, observed, path_env) =
                spawn_hook_harness(&format!("{id}-tree"));
            let payload = format!(
                r#"{{"cwd":"{}","hook_event_name":"PostToolUse","tool_name":"{tool}","tool_input":{tool_input}}}"#,
                repo.display()
            );
            invoke_hook(&script_path, &repo, &path_env, &payload);

            let log = std::fs::read_to_string(&observed).unwrap_or_default();
            assert!(
                log.contains(&format!("hook-file-event {id} --tool {tool} --path {path}")),
                "expected hook-file-event forwarded for {tool}; got: {log:?}"
            );
            assert!(
                log.contains(&format!("hook {id} pre_tool_use")),
                "pre_tool_use must still fire on PostToolUse for {tool}; got: {log:?}"
            );
        }
    }

    #[cfg(unix)]
    #[test]
    fn hook_does_not_forward_file_event_on_pre_tool_use() {
        let (_tmp, repo, script_path, observed, path) = spawn_hook_harness("115-tree");
        let payload = format!(
            r#"{{"cwd":"{}","hook_event_name":"PreToolUse","tool_name":"Read","tool_input":{{"file_path":"/tmp/foo.rs"}}}}"#,
            repo.display()
        );
        invoke_hook(&script_path, &repo, &path, &payload);

        let log = std::fs::read_to_string(&observed).unwrap_or_default();
        assert!(
            log.contains("hook 115 pre_tool_use"),
            "pre_tool_use must still fire on PreToolUse; got: {log:?}"
        );
        assert!(
            !log.contains("hook-file-event"),
            "hook-file-event must not fire on PreToolUse; got: {log:?}"
        );
    }

    #[cfg(unix)]
    #[test]
    fn hook_does_not_forward_file_event_for_untracked_tool() {
        let (_tmp, repo, script_path, observed, path) = spawn_hook_harness("116-tree");
        let payload = format!(
            r#"{{"cwd":"{}","hook_event_name":"PostToolUse","tool_name":"Bash","tool_input":{{"command":"ls"}}}}"#,
            repo.display()
        );
        invoke_hook(&script_path, &repo, &path, &payload);

        let log = std::fs::read_to_string(&observed).unwrap_or_default();
        assert!(log.contains("hook 116 pre_tool_use"));
        assert!(
            !log.contains("hook-file-event"),
            "hook-file-event must not fire for an untracked tool like Bash; got: {log:?}"
        );
    }

    #[cfg(unix)]
    #[test]
    fn hook_does_not_forward_file_event_when_path_missing() {
        let (_tmp, repo, script_path, observed, path) = spawn_hook_harness("117-tree");
        let payload = format!(
            r#"{{"cwd":"{}","hook_event_name":"PostToolUse","tool_name":"Edit","tool_input":{{}}}}"#,
            repo.display()
        );
        invoke_hook(&script_path, &repo, &path, &payload);

        let log = std::fs::read_to_string(&observed).unwrap_or_default();
        assert!(log.contains("hook 117 pre_tool_use"));
        assert!(
            !log.contains("hook-file-event"),
            "malformed payload (missing path) must be skipped, not forwarded; got: {log:?}"
        );
    }

    #[cfg(unix)]
    #[test]
    fn hook_skips_dispatch_mcp_tools_on_post_tool_use() {
        let (_tmp, repo, script_path, observed, path) = spawn_hook_harness("118-tree");
        let payload = format!(
            r#"{{"cwd":"{}","hook_event_name":"PostToolUse","tool_name":"mcp__dispatch__get_task","tool_input":{{}}}}"#,
            repo.display()
        );
        invoke_hook(&script_path, &repo, &path, &payload);

        let log = std::fs::read_to_string(&observed).unwrap_or_default();
        assert!(
            !log.contains("hook 118 pre_tool_use"),
            "mcp__dispatch__ tools must still be skipped entirely; got: {log:?}"
        );
        assert!(!log.contains("hook-file-event"));
    }

    #[cfg(unix)]
    #[test]
    fn hook_forwards_subagent_start_and_stop() {
        for (event, verb) in [("SubagentStart", "start"), ("SubagentStop", "stop")] {
            let (_tmp, repo, script_path, observed, path_env) =
                spawn_hook_harness(&format!("221-sub-{verb}"));
            let payload = format!(
                r#"{{"cwd":"{}","hook_event_name":"{event}","agent_id":"sub_01ABC","session_id":"sess_9"}}"#,
                repo.display()
            );
            invoke_hook(&script_path, &repo, &path_env, &payload);

            let log = std::fs::read_to_string(&observed).unwrap_or_default();
            assert!(
                log.contains("hook-subagent 221 ")
                    && log.contains(verb)
                    && log.contains("--agent-id sub_01ABC")
                    && log.contains("--session-id sess_9"),
                "expected {event} forwarded as {verb}; got: {log:?}"
            );
        }
    }

    #[cfg(unix)]
    #[test]
    fn hook_forwards_session_start_as_clear() {
        let (_tmp, repo, script_path, observed, path_env) = spawn_hook_harness("222-sess");
        let payload = format!(
            r#"{{"cwd":"{}","hook_event_name":"SessionStart","session_id":"sess_9"}}"#,
            repo.display()
        );
        invoke_hook(&script_path, &repo, &path_env, &payload);

        let log = std::fs::read_to_string(&observed).unwrap_or_default();
        assert!(
            log.contains("hook-subagent 222 clear"),
            "SessionStart must clear the task's subagent entries; got: {log:?}"
        );
    }

    #[cfg(unix)]
    #[test]
    fn hook_forwards_backgrounded_bash_as_shell_start() {
        let (_tmp, repo, script_path, observed, path) = spawn_hook_harness("231-bash-bg");
        let payload = format!(
            r#"{{"cwd":"{}","hook_event_name":"PostToolUse","tool_name":"Bash","session_id":"sess_9","tool_input":{{"command":"npm run dev","run_in_background":true}},"tool_response":{{"shell_id":"bash_1"}}}}"#,
            repo.display()
        );
        invoke_hook(&script_path, &repo, &path, &payload);

        let log = std::fs::read_to_string(&observed).unwrap_or_default();
        assert!(
            log.contains("hook-shell 231 start")
                && log.contains("--shell-id bash_1")
                && log.contains("--session-id sess_9"),
            "expected a backgrounded Bash call to forward as hook-shell start; got: {log:?}"
        );
    }

    #[cfg(unix)]
    #[test]
    fn hook_does_not_forward_shell_start_for_a_foreground_bash_call() {
        let (_tmp, repo, script_path, observed, path) = spawn_hook_harness("232-bash-fg");
        let payload = format!(
            r#"{{"cwd":"{}","hook_event_name":"PostToolUse","tool_name":"Bash","session_id":"sess_9","tool_input":{{"command":"cargo test"}},"tool_response":{{}}}}"#,
            repo.display()
        );
        invoke_hook(&script_path, &repo, &path, &payload);

        let log = std::fs::read_to_string(&observed).unwrap_or_default();
        assert!(
            !log.contains("hook-shell"),
            "a plain (non-backgrounded) Bash call must not forward as a shell event; got: {log:?}"
        );
    }

    #[cfg(unix)]
    #[test]
    fn hook_forwards_kill_bash_as_shell_stop() {
        let (_tmp, repo, script_path, observed, path) = spawn_hook_harness("233-killbash");
        let payload = format!(
            r#"{{"cwd":"{}","hook_event_name":"PostToolUse","tool_name":"KillBash","session_id":"sess_9","tool_input":{{"shell_id":"bash_1"}}}}"#,
            repo.display()
        );
        invoke_hook(&script_path, &repo, &path, &payload);

        let log = std::fs::read_to_string(&observed).unwrap_or_default();
        assert!(
            log.contains("hook-shell 233 stop")
                && log.contains("--shell-id bash_1")
                && log.contains("--session-id sess_9"),
            "expected KillBash to forward as hook-shell stop; got: {log:?}"
        );
    }

    #[cfg(unix)]
    #[test]
    fn hook_forwards_bash_output_as_shell_stop_only_when_no_longer_running() {
        let (_tmp, repo, script_path, observed, path) = spawn_hook_harness("234-bashoutput");
        let still_running = format!(
            r#"{{"cwd":"{}","hook_event_name":"PostToolUse","tool_name":"BashOutput","session_id":"sess_9","tool_input":{{"shell_id":"bash_1"}},"tool_response":{{"status":"running"}}}}"#,
            repo.display()
        );
        invoke_hook(&script_path, &repo, &path, &still_running);
        let log = std::fs::read_to_string(&observed).unwrap_or_default();
        assert!(
            !log.contains("hook-shell"),
            "a BashOutput poll reporting status=running must not forward a stop; got: {log:?}"
        );

        let completed = format!(
            r#"{{"cwd":"{}","hook_event_name":"PostToolUse","tool_name":"BashOutput","session_id":"sess_9","tool_input":{{"shell_id":"bash_1"}},"tool_response":{{"status":"completed"}}}}"#,
            repo.display()
        );
        invoke_hook(&script_path, &repo, &path, &completed);
        let log = std::fs::read_to_string(&observed).unwrap_or_default();
        assert!(
            log.contains("hook-shell 234 stop") && log.contains("--shell-id bash_1"),
            "a BashOutput poll reporting status=completed must forward a stop; got: {log:?}"
        );
    }

    #[cfg(unix)]
    #[test]
    fn hook_ignores_subagent_event_without_agent_id() {
        let (_tmp, repo, script_path, observed, path_env) = spawn_hook_harness("223-noid");
        let payload = format!(
            r#"{{"cwd":"{}","hook_event_name":"SubagentStart","session_id":"sess_9"}}"#,
            repo.display()
        );
        invoke_hook(&script_path, &repo, &path_env, &payload);

        let log = std::fs::read_to_string(&observed).unwrap_or_default();
        assert!(
            !log.contains("hook-subagent"),
            "a payload with no agent_id must be a silent no-op; got: {log:?}"
        );
    }

    #[cfg(unix)]
    #[test]
    fn subagent_events_do_not_stamp_activity() {
        // SubagentStart/Stop are not tool calls. Stamping last_pre_tool_use_at
        // from them would mask a genuinely idle agent.
        let (_tmp, repo, script_path, observed, path_env) = spawn_hook_harness("224-noact");
        let payload = format!(
            r#"{{"cwd":"{}","hook_event_name":"SubagentStart","agent_id":"a1","session_id":"s1"}}"#,
            repo.display()
        );
        invoke_hook(&script_path, &repo, &path_env, &payload);

        let log = std::fs::read_to_string(&observed).unwrap_or_default();
        assert!(
            !log.contains("hook 224 pre_tool_use"),
            "subagent lifecycle events must not fire the activity signal; got: {log:?}"
        );
    }

    #[cfg(unix)]
    fn run(args: &[&str], cwd: &std::path::Path) {
        let status = std::process::Command::new(args[0])
            .args(&args[1..])
            .current_dir(cwd)
            .status()
            .expect("spawn");
        assert!(status.success(), "command failed: {args:?}");
    }

    fn pr_learnings_hook_script() -> &'static str {
        PLUGIN_DIR
            .get_file("hooks/scripts/pr-learnings-hook")
            .expect("pr-learnings-hook must be embedded")
            .contents_utf8()
            .expect("pr-learnings-hook must be UTF-8")
    }

    fn hooks_json_value() -> Value {
        let content = PLUGIN_DIR
            .get_file("hooks/hooks.json")
            .expect("hooks.json must be embedded")
            .contents_utf8()
            .expect("hooks.json must be UTF-8");
        serde_json::from_str(content).expect("hooks.json is invalid JSON")
    }

    fn hook_commands_for_event<'a>(value: &'a Value, event: &str) -> Vec<&'a str> {
        value["hooks"][event][0]["hooks"]
            .as_array()
            .expect("hooks array")
            .iter()
            .filter_map(|h| h["command"].as_str())
            .collect()
    }

    #[test]
    fn pr_learnings_hook_is_valid_bash() {
        assert!(pr_learnings_hook_script().starts_with("#!/usr/bin/env bash"));
    }

    #[test]
    fn pr_learnings_hook_matches_gh_pr_create_and_calls_gate() {
        let s = pr_learnings_hook_script();
        assert!(
            s.contains("gh pr create"),
            "must match gh pr create commands"
        );
        assert!(s.contains("pr-gate"), "must call `dispatch pr-gate`");
        assert!(
            s.contains("tool_input") || s.contains(".command"),
            "must read the Bash command from the hook JSON"
        );
    }

    #[test]
    fn hooks_json_registers_pr_learnings_hook() {
        let value = hooks_json_value();
        let commands = hook_commands_for_event(&value, "PreToolUse");
        assert!(
            commands.iter().any(|c| c.contains("task-status-hook")),
            "existing task-status-hook must remain registered"
        );
        assert!(
            commands.iter().any(|c| c.contains("pr-learnings-hook")),
            "pr-learnings-hook must be registered under PreToolUse"
        );
    }

    #[cfg(unix)]
    #[test]
    fn pr_learnings_hook_invokes_gate_only_for_gh_pr_create() {
        use std::io::Write;
        use std::os::unix::fs::PermissionsExt;
        use std::process::{Command, Stdio};

        let tmp = tempfile::tempdir().expect("tempdir");
        let repo = tmp.path().join("repo");
        std::fs::create_dir_all(&repo).unwrap();
        run(&["git", "init", "-q", "-b", "321-pr"], &repo);
        run(&["git", "config", "user.email", "t@e.st"], &repo);
        run(&["git", "config", "user.name", "T"], &repo);
        std::fs::write(repo.join("README"), "x").unwrap();
        run(&["git", "add", "."], &repo);
        run(&["git", "commit", "-q", "-m", "init"], &repo);

        // Drop the embedded script to a real executable file.
        let script_path = tmp.path().join("pr-learnings-hook");
        std::fs::write(&script_path, pr_learnings_hook_script()).unwrap();
        let mut perm = std::fs::metadata(&script_path).unwrap().permissions();
        perm.set_mode(0o755);
        std::fs::set_permissions(&script_path, perm).unwrap();

        // Shim `dispatch` on PATH (exit 0 so the script doesn't abort on block).
        let bin = tmp.path().join("bin");
        std::fs::create_dir_all(&bin).unwrap();
        let observed = tmp.path().join("dispatch.log");
        let shim = format!(
            "#!/usr/bin/env bash\necho \"$@\" >> {}\n",
            observed.display()
        );
        let dispatch_shim = bin.join("dispatch");
        std::fs::write(&dispatch_shim, shim).unwrap();
        let mut p = std::fs::metadata(&dispatch_shim).unwrap().permissions();
        p.set_mode(0o755);
        std::fs::set_permissions(&dispatch_shim, p).unwrap();
        let path = format!(
            "{}:{}",
            bin.display(),
            std::env::var("PATH").unwrap_or_default()
        );

        let invoke = |payload: &str| {
            let mut child = Command::new("bash")
                .arg(&script_path)
                .env("PATH", &path)
                .current_dir(&repo)
                .stdin(Stdio::piped())
                .stdout(Stdio::piped())
                .stderr(Stdio::piped())
                .spawn()
                .expect("spawn hook");
            child
                .stdin
                .as_mut()
                .unwrap()
                .write_all(payload.as_bytes())
                .unwrap();
            let _ = child.wait().expect("wait");
        };

        // Matching command -> gate invoked.
        invoke(&format!(
            r#"{{"cwd":"{}","hook_event_name":"PreToolUse","tool_name":"Bash","tool_input":{{"command":"gh pr create --draft"}}}}"#,
            repo.display()
        ));
        // Non-matching command -> gate NOT invoked.
        invoke(&format!(
            r#"{{"cwd":"{}","hook_event_name":"PreToolUse","tool_name":"Bash","tool_input":{{"command":"gh pr view"}}}}"#,
            repo.display()
        ));

        let log = std::fs::read_to_string(&observed).unwrap_or_default();
        assert!(
            log.contains("pr-gate 321"),
            "expected `dispatch pr-gate 321` for gh pr create; got: {log:?}"
        );
        assert_eq!(
            log.matches("pr-gate").count(),
            1,
            "gate must fire only for gh pr create, not gh pr view; got: {log:?}"
        );
    }

    #[test]
    fn hooks_json_is_valid() {
        let value = hooks_json_value();
        assert!(
            value["hooks"].is_object(),
            "missing top-level hooks wrapper"
        );
        assert!(
            value["hooks"]["PreToolUse"].is_array(),
            "missing PreToolUse"
        );
        assert!(
            value["hooks"]["PostToolUse"].is_array(),
            "missing PostToolUse"
        );
        assert!(value["hooks"]["Stop"].is_array(), "missing Stop");
        assert!(
            value["hooks"]["Notification"].is_array(),
            "missing Notification"
        );
        assert!(
            value["hooks"]["UserPromptSubmit"].is_array(),
            "missing UserPromptSubmit"
        );
        assert!(
            value["hooks"]["SubagentStart"].is_array(),
            "missing SubagentStart"
        );
        assert!(
            value["hooks"]["SubagentStop"].is_array(),
            "missing SubagentStop"
        );
        assert!(
            value["hooks"]["SessionStart"].is_array(),
            "missing SessionStart"
        );
    }

    #[test]
    fn session_start_hook_excludes_compact_and_fork_sources() {
        // Claude Code's SessionStart `source` values are startup, resume,
        // clear, compact and fork. Only the first three mean "the previous
        // turn is over and any subagent rows left behind are dead".
        //
        //   compact — fires on *auto* compaction too, in the middle of a live
        //             fan-out. Clearing there wipes a genuine live count that
        //             session fencing cannot restore: the still-running
        //             subagents share this session id, so their eventual
        //             SubagentStops are no-op deletes.
        //   fork    — a fork is a different session; wiping the original's
        //             count from it is simply wrong.
        //
        // Matching on the three safe sources is what keeps that from
        // regressing, so pin the matcher rather than merely the registration.
        let value = hooks_json_value();
        let matcher = value["hooks"]["SessionStart"][0]["matcher"]
            .as_str()
            .expect("SessionStart entry must carry a matcher");
        assert_eq!(
            matcher, "startup|resume|clear",
            "SessionStart must not fire on compact or fork"
        );
    }

    #[test]
    fn hooks_json_registers_user_prompt_submit_hook() {
        let value = hooks_json_value();
        let commands = hook_commands_for_event(&value, "UserPromptSubmit");
        assert!(
            commands.iter().any(|c| c.contains("task-status-hook")),
            "task-status-hook must be registered under UserPromptSubmit"
        );
    }

    #[test]
    fn hooks_json_registers_post_tool_use_hook() {
        // PostToolUse must register task-status-hook so activity timestamps
        // are refreshed after every tool call — catching activity between
        // chained sub-agent invocations that would otherwise expire the
        // 10-minute active threshold.
        let value = hooks_json_value();
        let commands = hook_commands_for_event(&value, "PostToolUse");
        assert!(
            commands.iter().any(|c| c.contains("task-status-hook")),
            "task-status-hook must be registered under PostToolUse"
        );
    }
}
