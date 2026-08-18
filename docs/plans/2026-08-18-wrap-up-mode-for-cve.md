# Wrap-up mode for CVE tasks should always be PR

## Problem

`scripts/fetch-cve.sh` (the reference script for the managed CVE feed) sets
`wrap_up_mode: "pr"` on every item it emits, added in `8d9942f4 feat(feed):
let feed items declare wrap_up_mode; CVE feed defaults to pr`. That commit
only touched `fetch-cve.sh`.

`scripts/fetch-security.sh` emits the exact same kind of item — open GitHub
Dependabot vulnerability alerts (CVEs) — via the same `gh api
.../dependabot/alerts` endpoint, but was never updated: it does not set
`wrap_up_mode` at all, so a task created through it defaults to "decide at
wrap-up time" instead of "always PR". This is the drift task #4283 reports.

## Fix

1. Add a regression test asserting every reference feed script that emits
   Dependabot-vulnerability-alert items sets `wrap_up_mode: "pr"` in its jq
   mapping, so this can't silently drift again when one script is edited and
   its sibling isn't.
2. Add `wrap_up_mode: "pr"` to `scripts/fetch-security.sh`'s jq object,
   mirroring `fetch-cve.sh`.
3. Update `docs/specs/feeds.allium`'s reference-templates prose: the current
   list (around line 216) omits `fetch-cve.sh` entirely and doesn't mention
   that `fetch-security.sh` also defaults to `pr`. Fix both via `allium:tend`.
4. Run `allium:weed` to confirm spec/code alignment.

## Test plan

- New test: `tests/feed_scripts.rs` — reads `scripts/fetch-cve.sh` and
  `scripts/fetch-security.sh` from disk and asserts both contain
  `wrap_up_mode: "pr"` in their jq mapping.
- `cargo test`, `cargo fmt --check`, `./scripts/check-doc-paths.sh`.
