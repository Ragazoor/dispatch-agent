//! The `dispatch caller-headers` subcommand: Claude Code's `headersHelper` for
//! the dispatch MCP entry, installed by `dispatch setup`.
//!
//! It has one answer — the non-dispatched-session identity — and reads nothing
//! to arrive at it: no arguments, no working directory, no database, no
//! network. Runs on every MCP session start and reconnect.
//!
//! # Why there is no second answer
//!
//! This used to derive a task identity by inspecting its own working directory
//! for a `.worktrees/<id>-<slug>` segment. That could never work: Claude Code
//! runs a user-global helper from its OWN configuration directory, not the
//! session's, so the directory inspected was never the agent's. Every
//! dispatched agent got `session` regardless, silently, and its MCP calls
//! carried no task identity at all.
//!
//! A dispatched agent's identity comes from its launch instead — see
//! `dispatch::caller_identity` and `AgentCarriesItsOwnCallerIdentity` in
//! docs/specs/dispatch.allium. The derivation is gone rather than left
//! unreachable so that only one mechanism answers "which task is this agent";
//! see `TheHelperAnswersSessionUnconditionally` in
//! docs/specs/mcp-task-tools.allium.

/// The one payload this helper emits.
const SESSION_JSON: &str = r#"{"X-Caller-Kind":"session"}"#;

/// The JSON headers payload and its exit code.
///
/// Returns `(stdout, exit_code)`. Always exits 0 — there is no input to be
/// wrong about.
pub fn resolve_headers() -> (String, i32) {
    (SESSION_JSON.to_string(), 0)
}

#[cfg(test)]
#[allow(clippy::unwrap_used)]
mod tests {
    use super::*;

    #[test]
    fn emits_the_session_identity_and_exits_zero() {
        let (out, code) = resolve_headers();
        assert_eq!(code, 0);
        let v: serde_json::Value = serde_json::from_str(&out).unwrap();
        assert_eq!(v["X-Caller-Kind"], "session");
    }

    #[test]
    fn never_emits_a_task_identity() {
        // The transport rejects a request carrying both identity headers
        // (`CallerIdentity::from_headers` -> `Conflict`), and an agent's launch
        // supplies the task one. This must stay the only header here.
        let (out, _) = resolve_headers();
        let v: serde_json::Value = serde_json::from_str(&out).unwrap();
        assert!(v.get("X-Caller-Task-Id").is_none());
        assert_eq!(v.as_object().unwrap().len(), 1);
    }
}
