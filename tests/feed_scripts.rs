//! Regression guard for #4283: every reference feed script that emits GitHub
//! Dependabot vulnerability alerts (CVEs) must default `wrap_up_mode` to
//! `"pr"`. `fetch-cve.sh` got this in `8d9942f4 feat(feed): let feed items
//! declare wrap_up_mode; CVE feed defaults to pr`, but the commit only
//! touched that one script — `fetch-security.sh`, which hits the same `gh
//! api .../dependabot/alerts` endpoint, was left emitting items with no
//! `wrap_up_mode` at all. This test makes that drift structural: editing one
//! script's `wrap_up_mode` handling without the other now fails the suite.

#![allow(clippy::unwrap_used, clippy::expect_used)]

use std::path::PathBuf;

fn repo_file(rel: &str) -> String {
    let path = PathBuf::from(env!("CARGO_MANIFEST_DIR")).join(rel);
    std::fs::read_to_string(&path).unwrap_or_else(|e| panic!("cannot read {}: {e}", path.display()))
}

/// The Dependabot review prompt and the feed that creates its tasks must not
/// both check the PR author. `fetch-dependabot.sh` lists PRs with
/// `--author app/kognic-renovate`, so a dependabot task exists only for a PR
/// that already passed that filter — an agent re-deriving it spends a `gh` call
/// on a check that can only ever agree.
///
/// Pinned from both sides: if the feed ever drops its author filter, this fails
/// and the prompt's "do not re-check" instruction stops being true.
#[test]
fn dependabot_prompt_does_not_re_check_the_author_the_feed_already_filtered() {
    let feed = repo_file("scripts/fetch-dependabot.sh");
    assert!(
        feed.contains("--author app/kognic-renovate"),
        "the feed must filter by bot author for the prompt's skipped check to be safe"
    );

    let prompt = repo_file("src/dispatch/prompts/dependabot.md");
    assert!(
        !prompt.contains("commits[].authors[].login"),
        "the prompt must not re-derive the author the feed already filtered on"
    );
    assert!(
        prompt.contains("Do not re-check the PR author"),
        "the prompt must say why the author check is absent, or the next author will re-add it"
    );
}

#[test]
fn dependabot_alert_feed_scripts_default_wrap_up_mode_to_pr() {
    for script in ["scripts/fetch-cve.sh", "scripts/fetch-security.sh"] {
        let body = repo_file(script);
        assert!(
            body.contains(r#"wrap_up_mode: "pr""#),
            "{script} emits Dependabot vulnerability alerts (CVEs) and must set \
             wrap_up_mode: \"pr\" on every item so wrap-up always goes straight to a PR"
        );
    }
}
