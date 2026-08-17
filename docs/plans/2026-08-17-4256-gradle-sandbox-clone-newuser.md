# Bash sandbox blocks Gradle daemon startup (CLONE_NEWUSER)

## Investigation

Initial read: the settings file dispatch writes for every spawned session
(`src/setup/statusline.rs::write_settings_file`) only sets `sandbox.enabled`
and `sandbox.credentials.files` (see `SandboxedAgentExecution` in
`docs/specs/dispatch.allium`) — no `filesystem`/`network`/`failIfUnavailable`
keys. That looked like it ruled out a settings-level fix.

Verified against Claude Code's current sandboxing docs
(https://code.claude.com/docs/en/sandboxing) that this was wrong: there is a
documented `sandbox.excludedCommands` setting — an array of command-pattern
strings (e.g. `"docker *"`) that run entirely outside the sandbox, no
`dangerouslyDisableSandbox` retry needed. The docs' own troubleshooting
section uses the identical shape of problem as a worked example: *"`docker`
commands fail: docker is incompatible with the sandbox. Add `docker *` to
`excludedCommands` to run it outside the sandbox."* Gradle's fresh-daemon
`unshare(CLONE_NEWUSER)` is the same class of problem — a tool whose normal
operation needs something the sandbox's seccomp policy blocks, with no
narrower knob available (there is no syscall/capability allowlist, only
filesystem/network/credentials/excludedCommands).

Confirmed the string exists and is wired up in the installed Claude Code
binary (`sandbox?.excludedCommands`, read via a `DZ_()` helper and merged via
a `sandbox_exclude_command` action), not just in the docs.

## Action

1. **Spec**: add `excluded_commands` to `SandboxedAgentExecution` in
   `docs/specs/dispatch.allium` alongside the existing `sandbox_enabled` /
   `credential_read_denied` lets, with a new `@guarantee` explaining why
   Gradle wrapper invocations are pre-excluded (the sandbox has no
   syscall-level knob for `CLONE_NEWUSER`; the documented fix for a
   sandbox-incompatible tool is `excludedCommands`, the same mechanism
   Anthropic's own docs use for `docker`).
2. **Tests first**: extend `src/setup/statusline.rs`'s test module —
   a new test asserting `sandbox.excludedCommands` contains the Gradle
   wrapper patterns, and a tweak to the existing
   `writes_sandbox_config_enabled_with_no_filesystem_or_network_keys` test
   (which currently doesn't assert anything about `excludedCommands`) to
   confirm it stays scoped (only Gradle patterns, not a blanket exclusion).
3. **Implement**: add `"excludedCommands": ["./gradlew *", "gradlew *"]` to
   the JSON `write_settings_file` produces. This is dispatch's one shared
   settings file, used by every dispatch-spawned session in every repo
   (Rust, Python, Scala, frontend, Terraform, dbt — see
   `NetworkLeftUnrestricted`'s guarantee for why this file must generalize
   across ecosystems), so the fix reaches every future Gradle-repo task
   automatically — no per-repo CLAUDE.md note, no reliance on the agent
   recognizing the error and retrying.
4. Keep the `scope=user` knowledge-base learning already recorded (#440) as
   a fallback for the rare case a Gradle invocation doesn't match either
   pattern (e.g. invoked via `sh gradlew` or a custom wrapper script name).

## Follow-on: GitHub pre-allowed in the sandbox's network policy

While discussing where sandbox config belongs (this dispatch-owned generated
file vs. the user's own `~/.claude/settings.json`), it came up that the
user's global settings already carries `sandbox.network.allowedDomains:
["github.com", "api.github.com"]` — added because otherwise a sandboxed
agent's first git/gh network call either blocks on an interactive
host-approval prompt with no human present to answer it, or depends on the
auto-mode classifier, which can stall an unattended session on its very
first `git push`/`git fetch`/`gh` call.

Unlike a general ecosystem-registry domain list (npm, PyPI, crates.io, ... —
which `GitHubPreAllowedNetworkOtherwiseUnrestricted`'s predecessor guarantee
correctly kept out of this file, since it's incomplete and needs upkeep
across dispatch's many ecosystems), GitHub isn't ecosystem-specific: every
dispatched task's worktree is a git repo, and git/gh are hard runtime
dependencies for every single task, not just some. So this belongs in
dispatch's generated file too, not only the user's personal settings.

Action taken:
1. Revised the `NetworkLeftUnrestricted` guarantee in
   `SandboxedAgentExecution` (`docs/specs/dispatch.allium`) into
   `GitHubPreAllowedNetworkOtherwiseUnrestricted`, keeping the
   "no strictAllowlist, ecosystem registries stay unenumerated" reasoning
   but carving out `github.com`/`api.github.com` as pre-allowed.
2. Added a test (`writes_sandbox_allowed_domains_for_github_only`) before
   implementing, asserting exactly those two domains and that
   `strictAllowlist` stays absent.
3. Added `"network": {"allowedDomains": ["github.com", "api.github.com"]}`
   to `write_settings_file`.
4. Documented, separately, the actual reason two files exist at all
   (`dispatch-statusline.json` is regenerated from code on every
   `dispatch setup` run and silently drops hand-added keys;
   `~/.claude/settings.json` is durable and untouched by dispatch) as a new
   "Where sandbox config belongs" subsection in `docs/reference.md`'s Setup
   section, so this doesn't need re-deriving next time.
