//! The outer `Command` enum is a pure router over `src/tui/commands/`.
//!
//! Guards the invariant the WP-8 migration established (see the doc comment on
//! `Command` in `src/tui/types.rs`). Prose alone is what let the previous
//! migration stall half-done for 15 variants — the next agent could not tell
//! which convention was current, because both were present and neither failed.

#![allow(clippy::unwrap_used, clippy::expect_used)]

/// Every payload-carrying line of the `Command` enum body, as written in the
/// source — the repo's source-checking idiom (`check-doc-paths.sh`,
/// `check-doc-symbols.sh`, `board_normal_source_keys` in `rendering.rs`)
/// applied to an enum shape a type can't express.
fn command_variant_lines() -> Vec<String> {
    const SRC: &str = include_str!("../types.rs");
    let start = SRC
        .find("pub enum Command {")
        .expect("`pub enum Command {` not found — did the enum get renamed or moved?");
    let body = &SRC[start..];
    let end = body
        .find("\n}")
        .expect("unterminated `Command` enum body — expected a closing brace at column 0");

    body[..end]
        .lines()
        .skip(1) // the `pub enum Command {` line itself
        .map(str::trim)
        .filter(|l| !l.is_empty() && !l.starts_with("//"))
        .map(str::to_string)
        .collect()
}

/// Every variant wraps exactly one per-domain inner enum from
/// `crate::tui::commands` — no variant carries an inline payload of its own.
///
/// A new side effect belongs on an existing inner enum, or in a new module
/// under `src/tui/commands/`. If this test fails, the fix is to move the
/// variant's payload there, not to relax the assertion.
#[test]
fn every_command_variant_wraps_a_per_domain_inner_enum() {
    let lines = command_variant_lines();
    assert!(
        lines.len() >= 10,
        "parsed only {} variant lines out of `Command` — the parser is probably \
         mis-slicing the enum body rather than the enum having shrunk: {lines:?}",
        lines.len()
    );

    let offenders: Vec<&String> = lines
        .iter()
        .filter(|l| !l.contains("crate::tui::commands::"))
        .collect();

    assert!(
        offenders.is_empty(),
        "`Command` must stay a pure router: every variant wraps one inner enum from \
         `crate::tui::commands`. These variants carry an inline payload instead — move \
         each onto an existing inner enum, or into a new module under `src/tui/commands/`:\n  {}",
        offenders
            .iter()
            .map(|l| l.as_str())
            .collect::<Vec<_>>()
            .join("\n  ")
    );
}

/// The parser above only proves what it can see. If the slice ever stops
/// covering the real enum body, `every_command_variant_wraps_a_per_domain_inner_enum`
/// passes vacuously — so pin a variant that is known to exist.
#[test]
fn command_variant_parser_sees_the_real_enum_body() {
    let lines = command_variant_lines();
    assert!(
        lines.iter().any(|l| l.starts_with("Task(")),
        "expected to find the `Task(..)` variant in the parsed body, got: {lines:?}"
    );
}
