//! Opening the agent-tree companion pane's selected file in the user's editor
//! (see `docs/specs/agent-tree.allium`'s `OpenSelectedAgentTreeFile` surface
//! action and the `OpenAgentTreeFileInEditor` / `ReplaceAgentTreeEditorFile`
//! rules).
//!
//! Sibling of `src/agent_tree.rs`, which owns tree *building*. This module owns
//! the effect: which editor to run, and which tmux pane it runs in.

/// Editor of last resort when neither `$VISUAL` nor `$EDITOR` names one —
/// `config.agent_tree_editor_fallback` in docs/specs/agent-tree.allium.
pub const EDITOR_FALLBACK: &str = "vi";

/// Resolve the editor argv from environment *values*: `$VISUAL`, then
/// `$EDITOR`, then [`EDITOR_FALLBACK`]. Never returns an empty vector.
///
/// Takes the values as parameters rather than reading the process environment,
/// so the resolution order is testable without `std::env::set_var` — which is
/// `unsafe` in edition 2024 and races the test harness's threads either way.
/// [`editor_from_env`] is the one-line adapter that reads them.
///
/// A value is treated as unset when it is empty or all whitespace: `export
/// EDITOR=` is how a shell spells "no editor", and it would otherwise produce an
/// unrunnable empty argv. The value is split on whitespace into argv and
/// executed directly, never through a shell, so `EDITOR="nvim -p"` works and
/// nothing in it is expanded, globbed or word-split by anything but this
/// function.
pub fn resolve_editor(visual: Option<&str>, editor: Option<&str>) -> Vec<String> {
    for candidate in [visual, editor] {
        let Some(value) = candidate else { continue };
        let argv: Vec<String> = value.split_whitespace().map(str::to_string).collect();
        if !argv.is_empty() {
            return argv;
        }
    }
    vec![EDITOR_FALLBACK.to_string()]
}

/// [`resolve_editor`] against the real process environment.
pub fn editor_from_env() -> Vec<String> {
    let visual = std::env::var("VISUAL").ok();
    let editor = std::env::var("EDITOR").ok();
    resolve_editor(visual.as_deref(), editor.as_deref())
}

#[cfg(test)]
#[allow(clippy::unwrap_used, clippy::expect_used)]
mod tests {
    use super::*;

    #[test]
    fn visual_wins_over_editor() {
        assert_eq!(resolve_editor(Some("nvim"), Some("nano")), vec!["nvim"]);
    }

    #[test]
    fn editor_is_used_when_visual_is_unset() {
        assert_eq!(resolve_editor(None, Some("nano")), vec!["nano"]);
    }

    #[test]
    fn falls_back_to_vi_when_neither_is_set() {
        assert_eq!(resolve_editor(None, None), vec![EDITOR_FALLBACK]);
    }

    /// An exported-but-empty variable is how a shell spells "unset" in practice
    /// (`export EDITOR=`), and an empty argv would be unrunnable.
    #[test]
    fn an_empty_value_counts_as_unset() {
        assert_eq!(resolve_editor(Some(""), Some("nano")), vec!["nano"]);
        assert_eq!(resolve_editor(Some(""), Some("")), vec![EDITOR_FALLBACK]);
        assert_eq!(resolve_editor(Some("   "), None), vec![EDITOR_FALLBACK]);
    }

    /// The value is argv, not a shell command: it is split on whitespace and
    /// executed directly, so flags in `$EDITOR` work and nothing in it is
    /// shell-interpreted.
    #[test]
    fn a_multi_word_value_splits_into_argv() {
        assert_eq!(resolve_editor(Some("nvim -p"), None), vec!["nvim", "-p"]);
        assert_eq!(
            resolve_editor(Some("  code   -w  "), None),
            vec!["code", "-w"]
        );
    }
}
