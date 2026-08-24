# Sandbox blocks gradle/gcloud access to GCP Artifact Registry

## Investigation

Reported from task #4328 (WP4: Test mcp in user-scala): `./gradlew build`/`test`
resolves dependencies from GCP Artifact Registry
(`europe-west1-maven.pkg.dev`) via the `artifactregistry-gradle-plugin`,
which authenticates by shelling out to `gcloud auth print-access-token` as a
subprocess of the gradle JVM. Two sandbox layers block this: `*.pkg.dev` is
not in `sandbox.network.allowedDomains`, and `~/.config/gcloud` is in the
`sandbox.credentials.files` deny list.

Note the existing `GradleDaemonExcludedFromSandbox` guarantee (added a week
before this task was filed, see `docs/plans/2026-08-17-4256-gradle-sandbox-clone-newuser.md`)
already fully excludes literal `./gradlew *`/`gradlew *` invocations from the
sandbox — network and credentials included. Whether task #4328's session
predated that fix (stale `dispatch-statusline.json`, the same caveat CLAUDE.md
already documents for `GitSshFetchPushExcludedFromSandbox`) or the actual
build was invoked in a form that doesn't match that glob is unconfirmed. This
fix does not depend on which: it addresses network and credential access
directly, so it covers gradle/gcloud invocations regardless of whether the
top-level command happens to match the `excludedCommands` glob.

This also conflicts, on its face, with the existing
`GitHubPreAllowedNetworkOtherwiseUnrestricted` guarantee, which deliberately
declines to hardcode ecosystem package-registry domains (npm, PyPI, crates.io,
Maven, ...) because dispatch spans several ecosystems and such a list would be
incomplete and need constant upkeep. Presented this tradeoff to the user
directly:

1. **Domain allowlist** — presented as "widen the existing Gradle exclusion
   coverage" vs. "add `*.pkg.dev` as an explicit, documented exception".
   Chosen: **add `*.pkg.dev` as an explicit exception** — GCP Artifact
   Registry is Kognic-wide infrastructure many of dispatch's Scala/Gradle
   repos depend on, not a per-task ecosystem choice the way one repo's own
   npm registry would be, so it's closer to the GitHub precedent than to the
   generic-ecosystem-registry case `GitHubPreAllowedNetworkOtherwiseUnrestricted`
   already declines to enumerate.
2. **gcloud credential read access** — `sandbox.credentials.files` has no
   per-command scoping (unlike `excludedCommands`, which lifts every layer for
   a matching invocation): an entry there denies a path for *every* sandboxed
   command, or it doesn't. Presented as "scope via `excludedCommands` (add
   `gcloud *`, mirroring the `gh */git fetch *` pattern)" vs. "remove
   `~/.config/gcloud` from the global deny list entirely". The
   `excludedCommands` option doesn't actually help here — gradle's plugin
   invokes `gcloud auth print-access-token` as an internal subprocess of the
   JVM, not as a Bash-tool-visible top-level command dispatch could match —
   so a command-scoped exclusion on a literal `gcloud ...` invocation would
   never fire for this case. Chosen: **remove `~/.config/gcloud` from
   `credential_read_denied` entirely**, accepting that every sandboxed
   command in every dispatched task — not just gradle builds — gains
   unrestricted read access to the user's gcloud credentials. This is a
   strictly bigger exposure than the command-scoped exceptions
   (`GhCliExcludedFromSandboxKeyring`, `GitSshFetchPushExcludedFromSandbox`)
   and was an explicit, informed choice, not a default.

Per "Where sandbox config belongs" in `docs/reference.md`: this is genuinely
not "universal to dispatch" the way git/gh are — the tradeoff decisions above
reflect Kognic's own environment (this workspace) rather than a property of
dispatch itself. Baking it into the generated `dispatch-statusline.json`
regardless (rather than routing users to their own `~/.claude/settings.json`)
follows the same reasoning already applied to GitHub/Gradle/gh/git-SSH: those
are also single-organization-shaped defaults dispatch bakes in because they
apply to the overwhelming majority of dispatch's actual usage today.

## Action

1. **Tests first**: updated `src/setup/statusline.rs`'s test module —
   renamed `writes_sandbox_allowed_domains_for_github_only` to
   `writes_sandbox_allowed_domains_for_github_and_artifact_registry` with an
   exact-list assertion including `*.pkg.dev`; updated
   `writes_sandbox_credential_deny_list` to drop `~/.config/gcloud` from the
   expected-present list and added an explicit assertion that it is absent.
2. **Implement**: added `"*.pkg.dev"` to the `network.allowedDomains` array
   and removed the `~/.config/gcloud` entry from `credentials.files` in
   `write_settings_file` (`src/setup/statusline.rs`).
3. **Spec**: updated `allowed_domains`/`credential_read_denied` `let`s on
   `SandboxedAgentExecution` in `docs/specs/dispatch.allium`; amended
   `GitHubPreAllowedNetworkOtherwiseUnrestricted` and
   `CredentialsDeniedNotMasked` prose to acknowledge the new exceptions
   without contradicting their existing claims; added two new guarantees,
   `GcpArtifactRegistryPreAllowedForGradle` and
   `GcloudCredentialsUnrestrictedForArtifactRegistry`, documenting the
   mechanism and the tradeoffs above.
4. **Docs**: added a CLAUDE.md troubleshooting note (same style as the
   git-SSH one, including the stale-`dispatch-statusline.json` caveat), and
   extended the "Where sandbox config belongs" list in `docs/reference.md`.
5. Verified with the repo's verify command.
