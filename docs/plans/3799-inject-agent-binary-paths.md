# 3799 — Inject `claude` / `dispatch` binary paths instead of stubbing them via `PATH`

Follow-up to #3782. The real-tmux harness (`tests/tmux_harness/mod.rs`) stops the
tests launching the real `claude` and `dispatch` binaries by name-shadowing on
`PATH`. That works, but needs four cooperating mechanisms, one of which
(`std::env::set_var`) is unsound under libtest's parallel threads. Production
already threads `&dyn ProcessRunner` through every agent-launch path for exactly
this kind of substitutability; the binary identities should ride alongside it.

## Design decision

**Chosen: a default method on `ProcessRunner`.**

```rust
pub trait ProcessRunner: Send + Sync {
    fn run(&self, program: &str, args: &[&str]) -> Result<Output>;
    fn run_with_timeout(&self, ..) -> Result<Output> { self.run(program, args) }

    /// Which `claude` / `dispatch` binaries the agent launchers invoke.
    /// Bare names by default — resolved on `PATH` at launch, exactly as today.
    fn agent_binaries(&self) -> AgentBinaries { AgentBinaries::default() }
}
```

Rationale: the binaries are part of the *process-execution environment* the
runner already abstracts, so the seam lands where the substitution boundary
already is. Every one of the ~12 production call sites and ~40 mock-test call
sites compiles unchanged; the harness's `SocketRunner` overrides one method.

Alternatives considered and rejected:

- **Explicit param on each launcher** (`dispatch_agent(&task, runner, &bins, …)`).
  Most explicit, but grows six public signatures and edits ~50 call sites for a
  value that is `default()` in every one of them but the harness.
- **`AgentLaunch { runner, binaries }` context struct.** Groups the two properly
  but introduces a second way to pass a runner, and still edits every call site.

`DISPATCH_PLUGIN_DIR` (`src/dispatch/prompts.rs:12`) is *not* the right home: it
is a flag fragment appended after the binary name, and it is not something a test
substitutes. It stays as-is.

## Spec impact

None. Production behaviour is unchanged: the defaults are the bare names
`claude` / `dispatch`, shell quoting is a no-op for them, and the one textual
change to an emitted command string (the `$0` indirection, above) resolves the
same binary the same way. No Allium spec describes *how* the binaries are named,
so there is nothing to tend. Verified by grepping `docs/specs/` for binary/path
language.

## Work

### Step 1 — `AgentBinaries` (TDD)

Tests first, in `src/process.rs`'s inline `mod tests`:

- `agent_binaries_defaults_to_bare_names` — `AgentBinaries::default()` yields
  `claude` / `dispatch`.
- `real_process_runner_uses_default_agent_binaries` — `RealProcessRunner`
  inherits the trait default.
- `shell_quote_leaves_plain_paths_untouched` — `claude`,
  `/tmp/x/claude`, `./claude` come back unchanged (this is what keeps production
  output byte-identical).
- `shell_quote_wraps_paths_needing_it` — a path with a space or a quote comes
  back single-quoted with embedded quotes escaped.

Then implement:

```rust
/// The `claude` / `dispatch` binaries the agent launchers invoke.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct AgentBinaries { pub claude: String, pub dispatch: String }

impl Default for AgentBinaries { /* bare names */ }

impl AgentBinaries {
    /// The binary as one shell word. One quoting layer, everywhere.
    pub fn claude_quoted(&self) -> String { .. }
}
```

**One accessor, because every call site is arranged to need exactly one quoting
layer.** The first cut had two — `dispatch_with_prompt` interpolates *inside* an
already single-quoted `bash -c '…'` body, which imposes two layers (the pane's
shell strips the outer quotes, then the inner `bash` parses what is left), so it
needed the value quoted for the inner shell and that quoting escaped (`'` →
`'\''`) for the outer one. Getting only one of the two right yields a string that
looks escaped and splits at the first space.

The simplify pass (two independent reviewers, converging) called that the wrong
altitude: it encapsulates a subtle rule instead of removing it. The fix is to pass
the binary as bash's `$0`, *after* the script body:

```rust
let claude_cmd = format!(
    "bash -c 'prompt=$(cat .claude-prompt) && rm -f .claude-prompt \
     && \"$0\" {DISPATCH_PLUGIN_DIR}{permission_flag} \"$prompt\"' {claude}"
);
```

Now the binary is an ordinary shell word outside the quotes, one layer like every
other launcher, and `claude_for_bash_c` / `escape_within_single_quotes` are
deleted. `claude_quoted_survives_the_launcher_command_shape` composes the real
shape and runs it through `sh` for a path with a space and one with an embedded
quote — that test is what pins the `$0` arrangement, and it fails for
`"claude bin"` if anyone moves the binary back inside the quotes.

`shell_quote` returns its input unchanged when it matches
`[A-Za-z0-9_@%+=:,./-]+` (the standard "safe" set), else single-quotes it. Keeping
the common case unquoted means the only change to the production command string
is the `$0` indirection itself, with no quoting noise on top. Behaviour is
identical — `bash` resolves a bare `claude` from `PATH` exactly as before.

Add the trait default method plus a `MockProcessRunner::with_agent_binaries`
builder so mock tests can pin a distinctive path.

### Step 2 — thread the binaries through `src/dispatch/agents.rs` (TDD)

Tests first, in `src/dispatch/tests.rs` (the bonus the task calls out — these are
assertions the mock tests currently *cannot* make):

- `dispatch_agent_launches_the_runners_claude_binary` — a mock with
  `AgentBinaries { claude: "/stub/bin/claude", .. }` produces a `send-keys`
  payload containing `/stub/bin/claude --plugin-dir`, and **not** a bare
  ` claude ` token.
- `resume_agent_launches_the_runners_claude_binary` — same for the
  `claude … --continue` resume string.
- `create_main_session_launches_the_runners_claude_binary` — same for the main
  session (this test lives in `agents.rs`'s own `mod tests`, beside its siblings).
- `spawn_agent_tree_pane_launches_the_runners_dispatch_binary` — driven through
  `resync_agent_tree_pane`, asserts argv[0] of the `split-window` command is
  `/stub/bin/dispatch`, not `dispatch`.
- `agent_launchers_default_to_bare_binary_names` — a default mock still emits
  bare `claude` / `dispatch`, locking the no-behaviour-change guarantee.

Then implement: read `runner.agent_binaries()` once at the top of
`dispatch_with_prompt`, `resume_agent`, `create_main_session` and
`spawn_agent_tree_pane`, and interpolate instead of hardcoding. Four call sites
total (`agents.rs:35`, `:184`, `:300`, `:344`).

### Step 3 — harness: own the stubs per server, drop all four mechanisms

`TmuxServer::start()` gains its own stub directory: a `tempfile::TempDir` field
holding a `claude` and a `dispatch` stub plus one log file. Because each server
owns its stubs, the stub script no longer has to sniff `$TMUX` to pick a
per-test log path — it hardcodes the one belonging to its server. `SocketRunner`
gains the `agent_binaries()` override returning those two absolute paths.

Field order matters: the `TempDir` is declared *after* `socket`, so `Drop for
TmuxServer` (which runs `kill-server`, killing the stub processes holding the log
open) completes before the directory is unlinked.

Deleted outright:

| Mechanism | Why it can go |
|---|---|
| `std::env::set_var("PATH", …)` | Binaries are named explicitly; nothing resolves on `PATH`. Removes the unsoundness. |
| `std::env::set_var("DISPATCH_DB", …)` | Was a blast-radius limiter for a defeated guard. There is no guard to defeat. |
| `isolate_pane_shell()` / `default-command` pinning | Existed only because a login shell re-resolves `PATH`. An absolute path is immune to `PATH` order. |
| Both stub-resolution guards (`install_stubs`' process check, `verify_pane_resolution_once`) | They asserted that name-shadowing worked. Nothing to assert once the name is not the mechanism. |
| `resolve_on_path` / `is_executable` / `PANE_SHELL` | Only used by the deleted guards. |
| `install_stubs()` (public API) | Replaced by `TmuxServer::start()`; `stub_log_path(server)` becomes `server.stub_log()`. |

Retained, as the task specifies: `-f /dev/null` on every call, and the per-test
`-L` socket. Those isolate *tmux*, not the binaries.

One residual exposure is accepted and documented in the harness: panes now run
the developer's login shell. Nothing about *which* binary runs depends on that
any more, but an rc file that `cd`s would move a pane's cwd out from under the
`pane_cwd` / `StubLine::cwd` assertions. If it ever bites, `default-command`
comes back as pane-determinism config — the same category as `-f /dev/null` —
not as stub protection.

`tests/tmux_lifecycle.rs` loses its `install_stubs()` calls (both the `setup()`
one and the one in `harness_ignores_the_developers_tmux_config`, whose only
purpose was to reach the `PATH` mutation early) and its
`server.isolate_pane_shell()` call. The module-level comment blocks in both files
that document the three/four-piece mechanism get rewritten to describe the seam.

A new harness self-test replaces the retired guards with one that is actually
about the new seam:

- `harness_runner_names_the_stub_binaries` — `server.runner().agent_binaries()`
  points at files inside the server's own stub dir.

### Step 4 — docs

`CLAUDE.md`'s External Dependencies section notes `claude` is spawned by
`src/dispatch/agents.rs`; add that the binary names come from
`ProcessRunner::agent_binaries()` so tests can substitute them. Nothing in
`docs/conventions.md` or `docs/architecture.md` currently describes the stub rig,
so no edit is needed there.

## Verification

```
cargo fmt --check && cargo test && ./scripts/check-doc-paths.sh
```

Plus, because the point of the change is that the tests no longer depend on
process-global state:

- `cargo test --test tmux_lifecycle` passes with the developer's real `claude`
  and `dispatch` first on `PATH` (which is the normal state — the guards are gone,
  so a regression here is a real launch of the real binary, and
  `dispatch_launches_claude_in_the_worktree` asserting the *stub path* is what
  catches it).
- `cargo clippy --all-targets -- -D warnings` (the pre-push gate).
