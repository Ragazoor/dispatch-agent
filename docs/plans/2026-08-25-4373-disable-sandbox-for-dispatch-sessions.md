# Disable the sandbox for dispatch task sessions (Docker/Testcontainers blocked)

## Investigation

Reported from task #4367 (annotell, adding PKs for pglogical/CDC): annotell's
own CLAUDE.md says "Repository tests use Testcontainers Postgres (Docker
required)." Under the sandbox, that test path cannot run at all — the agent
either skips tests or repeatedly requests `dangerouslyDisableSandbox`. The
same session also needed `dangerouslyDisableSandbox` for `docker run/exec/cp`
calls while regenerating a schema dump via dbmate against a real Postgres
container, and hit an unrelated `git checkout -- <file>` failure
(`Unable to create '<main-repo>/.git/worktrees/<name>/index.lock': Read-only
file system`) that couldn't be conclusively diagnosed from ~84 truncated
`denyWithinAllow` entries.

All sandbox config for dispatch-spawned sessions lives in exactly one place:
`write_settings_file` (`src/setup/statusline.rs`), which generates
`~/.claude/dispatch-statusline.json`. This single global file is applied
identically to every dispatch-spawned Claude session — `dispatch_agent`,
`quick_dispatch_agent`, `research_agent`, and `resume` — via the fixed
`--settings` flag baked into `DISPATCH_PLUGIN_DIR` (`src/dispatch/prompts.rs`).
There is no per-repo or per-task sandbox setting today (unlike `verify_command`,
which lives per-repo in the `repo_paths` table) — regenerating the file
(`dispatch setup`, or the `runtime::bootstrap` fallback) always rebuilds the
whole `sandbox` object from a fixed literal.

Considered a narrower fix first: `sandbox.excludedCommands` already carries
Gradle/`gh`/git-SSH exceptions for the same class of problem (a command the
sandbox blocks with no narrower schema knob). But Testcontainers-style tests
don't shell out to a `docker` CLI command visible to `excludedCommands` — the
JVM/Go/Python test process talks to the Docker daemon directly over its Unix
socket (`/var/run/docker.sock`), from *inside* an arbitrary per-ecosystem
test-runner invocation (`mvn test`, `./gradlew test`, `go test`, `pytest`, ...).
That's the same class of gap as the `gh`/D-Bus keyring problem (AF_UNIX
sockets aren't domain traffic and aren't matched by `excludedCommands` unless
the top-level command itself is enumerable) — except here the invoking
command *isn't* enumerable across ecosystems, so `excludedCommands` doesn't
generalize the way it did for `gh`. The only schema knob that reaches this on
Linux is `sandbox.network.allowAllUnixSockets`, an all-or-nothing switch for
local socket access (same limitation already documented in
`GhCliExcludedFromSandboxKeyring`).

Presented the tradeoff to the user: (a) `allowAllUnixSockets` — narrower,
keeps filesystem confinement/credential deny-list/worktree-boundary
enforcement active, but is still a global loosening of all local IPC, not
just Docker; or (b) disable the sandbox outright for dispatch-spawned
sessions — simplest, matches the task title, but drops every protection the
sandbox added (filesystem confinement, credential denial, and the
"agents work from their worktree" OS-level enforcement `SandboxedAgentExecution`
was originally added to give real teeth to). **Chosen: (b), full disable.**
This also resolves the `git checkout`/`index.lock` failure and the
`apply-seccomp: unshare(CLONE_NEWUSER)` flaky-startup class of failure as a
side effect, since no sandbox means no filesystem/seccomp restriction to hit
in the first place. Per-command permission prompts (Claude Code's own
approval flow, separate from the OS-level sandbox) are unaffected and remain
the sole review layer for dispatched agents going forward.

Per-repo/per-task opt-out (raised as an open question in the task) was not
pursued: the settings file has no per-repo mechanism today, and building one
(a new DB column plus per-repo settings-file generation and a change to the
fixed `DISPATCH_PLUGIN_DIR` spawn constant) is a materially bigger change than
this task calls for. With the sandbox off entirely, there is also nothing
left to opt in or out of per repo.

## Action

1. **Tests first**: update `src/setup/statusline.rs`'s test module —
   `writes_sandbox_config_enabled_with_no_filesystem_key` now asserts
   `enabled == false`; remove the tests asserting the now-deleted
   `excludedCommands`/`network.allowedDomains`/`credentials.files` arrays and
   replace them with one assertion that `sandbox` is exactly `{"enabled":
   false}`.
2. **Implement**: replace the `sandbox` object in `write_settings_file`
   (`src/setup/statusline.rs`) with `{"enabled": false}`, deleting the
   `excludedCommands`/`network`/`credentials` sub-keys entirely (they're
   inert once the sandbox is off, and dead config invites drift).
3. **Spec**: rewrite `SandboxedAgentExecution` in
   `docs/specs/dispatch.allium` — `sandbox_enabled = false`, drop the
   `credential_read_denied`/`excluded_commands`/`allowed_domains` lets, retire
   the guarantees that described sandbox-on behaviour
   (`FilesystemLeftAtDefaults`, `GitHubPreAllowedNetworkOtherwiseUnrestricted`,
   `CredentialsDeniedNotMasked`, `GradleDaemonExcludedFromSandbox`,
   `GhCliExcludedFromSandboxKeyring`, `GitSshFetchPushExcludedFromSandbox`,
   `GcpArtifactRegistryPreAllowedForGradle`,
   `GcloudCredentialsUnrestrictedForArtifactRegistry` — all moot once
   `enabled = false`), and add one new guarantee explaining why full disable
   was chosen over `allowAllUnixSockets` or continuing to special-case
   ecosystems one at a time. Use `allium:tend` to make the edit and
   `allium:weed` to check alignment.
4. **CLAUDE.md**: add a note that dispatch-spawned sessions no longer run
   under the sandbox, with a pointer to `SandboxedAgentExecution` for why —
   leave the existing tmux/apply-seccomp/git-ssh/gcp-artifact-registry
   troubleshooting notes in place, since they still apply to anyone running
   `cargo test` in this repo under a sandbox they enabled themselves outside
   of dispatch.
5. **docs/reference.md**: update "Where sandbox config belongs" to reflect
   that `dispatch-statusline.json`'s `sandbox` key is now just `{"enabled":
   false}`.
6. Verify with `cargo test` (this repo's verify command). Note: per learning
   #441, a `dispatch-statusline.json` edit doesn't take effect in the session
   that made it — this session's own Bash tool will keep running sandboxed
   until a fresh `dispatch setup`/session picks up the regenerated file, so
   there's nothing to self-verify live from inside this task.
