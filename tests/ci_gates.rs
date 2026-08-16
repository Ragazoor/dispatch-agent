//! Parity guard: every gate script the pre-push hook runs must also run in CI.
//!
//! The pre-push hook only fires for a clone that has opted into
//! `core.hooksPath` (see CLAUDE.md's "First-time setup"), so a check that lives
//! *only* there is a check nothing enforces on a fresh clone or on a push that
//! used `--no-verify`. Three of the five gate scripts — the doc-symbol,
//! no-test-sleep and fetch-reviews checkers — sat in exactly that state until
//! #4221. This test makes the drift structural: add a script to the hook and CI
//! is required to run it too, without anyone remembering to say so.
//!
//! It also pins the two coverage-job properties #4221 fixed: tarpaulin runs
//! once (it used to run the whole suite twice, once for XML and once for the
//! stdout summary), and it enforces a floor rather than reporting a number
//! nobody reads.

#![allow(clippy::unwrap_used, clippy::expect_used)]

use std::path::PathBuf;

fn repo_file(rel: &str) -> String {
    let path = PathBuf::from(env!("CARGO_MANIFEST_DIR")).join(rel);
    std::fs::read_to_string(&path).unwrap_or_else(|e| panic!("cannot read {}: {e}", path.display()))
}

/// Every `scripts/<name>.sh` path mentioned in `body`, deduped, in first-seen
/// order. Deliberately textual: it reads the hook the same way a human does, so
/// a script added to the hook in any invocation style is picked up.
fn gate_scripts(body: &str) -> Vec<String> {
    let mut found: Vec<String> = Vec::new();
    for (idx, _) in body.match_indices("scripts/") {
        let token: String = body[idx..]
            .chars()
            .take_while(|c| c.is_ascii_alphanumeric() || matches!(c, '-' | '_' | '.' | '/'))
            .collect();
        if token.ends_with(".sh") && !found.contains(&token) {
            found.push(token);
        }
    }
    found
}

#[test]
fn ci_workflow_mirrors_every_pre_push_gate_script() {
    let hook = repo_file(".githooks/pre-push");
    let ci = repo_file(".github/workflows/ci.yml");

    let scripts = gate_scripts(&hook);
    assert!(
        scripts.len() >= 5,
        "expected the pre-push hook to run at least the five known gate scripts, found {scripts:?}"
    );

    for script in &scripts {
        assert!(
            ci.contains(script.as_str()),
            "`{script}` runs in .githooks/pre-push but not in .github/workflows/ci.yml — \
             a hook-only gate is inert for any clone that has not set core.hooksPath"
        );
    }
}

#[test]
fn ci_coverage_job_runs_tarpaulin_once() {
    let ci = repo_file(".github/workflows/ci.yml");
    // `cargo install cargo-tarpaulin` has a hyphen, so it does not match.
    let runs = ci.matches("cargo tarpaulin").count();
    assert_eq!(
        runs, 1,
        "expected exactly one `cargo tarpaulin` invocation in CI, found {runs} — \
         tarpaulin re-runs the whole suite per invocation, so emit every output \
         format from a single run instead"
    );
}

#[test]
fn ci_coverage_job_enforces_a_coverage_floor() {
    let ci = repo_file(".github/workflows/ci.yml");
    // Scan every occurrence, not just the first: the flag is also named in the
    // surrounding comment, where it is not followed by a number.
    let floor = ci
        .match_indices("--fail-under")
        .find_map(|(idx, m)| {
            let digits: String = ci[idx + m.len()..]
                .trim_start()
                .chars()
                .take_while(|c| c.is_ascii_digit())
                .collect();
            digits.parse::<u32>().ok()
        })
        .expect(
            "the coverage job must pass `--fail-under <percent>` to tarpaulin; \
             an ungated coverage number cannot catch a regression",
        );
    assert!(
        (50..=100).contains(&floor),
        "coverage floor {floor} is outside the plausible range 50..=100"
    );
}
