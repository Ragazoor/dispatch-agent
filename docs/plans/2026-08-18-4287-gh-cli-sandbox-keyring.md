# gh CLI fails in the Bash sandbox (reported as unshare(CLONE_NEWUSER))

## Investigation

Reproduced directly in this session (which runs under the dispatch-generated
sandbox, `~/.claude/dispatch-statusline.json`):

- `gh auth status` inside the sandbox: "The token in default is invalid."
- `gh auth status` with `dangerouslyDisableSandbox: true`: `✓ Logged in ...
  (keyring)` — the token is fine.
- Root cause: `gh` stores its token in the OS keyring (GNOME Keyring, over
  D-Bus). D-Bus is an AF_UNIX socket
  (`$DBUS_SESSION_BUS_ADDRESS=unix:path=/run/user/1000/bus`). Reproduced the
  exact failure directly: `secret-tool search --all service gh:github.com`
  inside the sandbox gives `secret-tool: Unable to create socket: Operation
  not permitted`. The sandbox's Linux seccomp policy blocks AF_UNIX socket
  creation outright — `sandbox.network.allowUnixSockets` is macOS-only ("Linux
  seccomp cannot filter by path" per the settings schema); the only Linux
  knob is the all-or-nothing `allowAllUnixSockets`. Without it, `gh` can't
  reach the keyring and falls back to reporting the token invalid — this is
  unrelated to `allowedDomains`, since the failure is at the local IPC layer,
  before any network egress.
- Plain `curl` to `api.github.com` worked fine (200) every time — no
  CLONE_NEWUSER, no auth issue. Could not reproduce the literal
  `apply-seccomp: unshare(CLONE_NEWUSER): Invalid argument` crash from the
  task description with either tool today. That message is the same
  pre-existing, undirected sandbox limitation already documented in
  `CLAUDE.md` and in `docs/plans/2026-08-17-4256-gradle-sandbox-clone-newuser.md`
  — a command that itself needs to create a Linux user namespace (e.g. a
  fresh build-daemon startup) gets blocked by the sandbox's seccomp policy,
  intermittently and without naming the offending command. It is not
  specific to `gh`/`curl`, and per that prior investigation there is no
  narrower config fix for it (`excludedCommands` is the documented escape
  hatch, already used there). Distinct mechanism from the AF_UNIX block
  above (denied `socket()` vs. denied `unshare()`), and out of scope here.

Presented the fixable AF_UNIX/keyring issue to the user as three options
(loosen the socket policy globally, move `gh` off keyring auth, or exclude
`gh` from the sandbox). Chosen: **exclude `gh` from the sandbox**, via
`sandbox.excludedCommands` — the same mechanism already used for the Gradle
wrapper (`docs/plans/2026-08-17-4256-gradle-sandbox-clone-newuser.md`), and
the one Claude Code's own sandboxing docs recommend for a tool the sandbox
can't accommodate (their worked example is `docker *`).

## Why this belongs in `dispatch-statusline.json`, not personal settings

Per "Where sandbox config belongs" in `docs/reference.md`: settings universal
to every dispatched task belong in the generated file
(`src/setup/statusline.rs::write_settings_file`), not the user's personal
`~/.claude/settings.json`. `gh` is a hard runtime dependency of every
dispatched task (same reasoning as the existing `github.com`/`api.github.com`
allowlist entry — see `GitHubPreAllowedNetworkOtherwiseUnrestricted` in
`docs/specs/dispatch.allium`), so excluding it belongs in code, reaching every
future dispatched session automatically.

## Action

1. **Spec**: add a `GhCliExcludedFromSandboxKeyring` guarantee to
   `SandboxedAgentExecution` in `docs/specs/dispatch.allium`, alongside the
   existing Gradle one — noting the different underlying mechanism (blocked
   AF_UNIX socket to the credential keyring, not `CLONE_NEWUSER`) so the two
   guarantees aren't read as the same bug.
2. **Tests first**: extend `src/setup/statusline.rs`'s test module with an
   assertion that `sandbox.excludedCommands` contains `"gh *"`, and adjust
   the existing exact-list assertions (`writes_sandbox_excluded_commands_for_gradle_wrapper`)
   so they still pass with the added entry.
3. **Implement**: add `"gh *"` to the `excludedCommands` array in
   `write_settings_file`.
4. Verify with `cargo test`.
