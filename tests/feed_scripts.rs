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
