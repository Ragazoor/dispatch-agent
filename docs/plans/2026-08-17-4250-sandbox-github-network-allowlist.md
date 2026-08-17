# 4250: Allow github.com/api.github.com through the command sandbox

## Problem

The command sandbox's network allowlist (`sandbox.network.allowedDomains`)
is empty, so every `gh` invocation inside a sandboxed Bash call fails with
network-egress errors. Those errors surface as "invalid token" from
`gh auth status`, which reads as an auth problem when it's actually network
policy. The only current workaround is `dangerouslyDisableSandbox: true` on
every `gh` call.

## Fix

Add `github.com` and `api.github.com` to `sandbox.network.allowedDomains`
in the user's **global** `~/.claude/settings.json`, not this repo's
`.claude/settings.json` — the user asked for it scoped to their own machine
across all repos, not committed as a team-wide policy for this one.
Consequently there is no change to this repo's tracked files other than
this plan doc.

This is a Claude Code harness config change, not a change to the dispatch
Rust codebase — there's no code path to unit test.

**Known limitation**: `sandbox.network.allowedDomains` did not take effect
in the session that made the edit — the sandbox's network policy appears to
be loaded once at session startup, not hot-reloaded. `gh auth status` under
the sandbox still failed with the network-block symptom in the same
session after the edit. Verifying the fix requires a fresh Claude Code
session. Recorded as knowledge-base entry #441.

## Steps

1. Add `sandbox.network.allowedDomains: ["github.com", "api.github.com"]`
   to `~/.claude/settings.json` (global, user-scoped), merging with
   existing keys.
2. Verify in a **new** Claude Code session: `gh auth status` and
   `gh pr list --limit 1` succeed under the sandbox (no
   `dangerouslyDisableSandbox`).
3. No Rust code touched — `cargo test` is not affected by this change.
