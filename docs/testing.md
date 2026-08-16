# Testing

Everything about running, writing, and placing tests in this repo. `CLAUDE.md`
keeps only the two facts that change what you type at the prompt (the suite
needs `tmux`; never pipe `cargo test` into `tail`); the rest lives here.

## Running tests

```bash
cargo test                                # full suite
cargo test db::tests                      # database CRUD and migrations
cargo test service::                      # domain service layer
cargo test tui::tests                     # TUI input/message handling
cargo test mcp::handlers::tests           # MCP JSON-RPC handlers
cargo test --test lifecycle               # integration: full task lifecycle
cargo test --test epic_lifecycle          # integration: full epic lifecycle
cargo test --test cli                     # CLI subcommand smoke tests
cargo test tui::tests::scenarios          # key-sequence integration tests
cargo test tui::tests::snapshots          # ratatui buffer rendering tests
cargo test --test tmux_lifecycle          # real tmux: window/pane topology and cwd
cargo test --test tmux_split_hook         # real tmux: split-pane cwd, and that nothing is typed at a pane
cargo test --test tmux_window_targets     # real tmux: window-name resolution
cargo test --test tmux_editor_pane        # real tmux: agent-tree editor pane, toggle target
```

That is a **selection**, not the full inventory — `tests/` also holds
`active_health.rs`, `caller_identity.rs`, `ci_gates.rs`,
`dispatch_status_lifecycle.rs`, `feed_sync.rs`, `githooks.rs`,
`managed_feeds.rs`, `task_watchers.rs`,
`tmux_send_message_pane_state.rs`, `trajectory.rs`, and `verify_command.rs`.
`ls tests/` is the authority; this list is a shortcut for the targets you reach
for most.

**The full suite needs `tmux` on `PATH`.** The `--test tmux_*` targets drive a real tmux server (private `-L` socket, `-f /dev/null`, drop-guard teardown — see `tests/tmux_harness/mod.rs`). Without tmux they print `skipping: tmux not available on PATH` and pass, so a green local run isn't proof they ran; CI hard-fails instead of skipping (tmux is installed in both the `test` and `coverage` jobs).

**A sandboxed `cargo test` fails these targets for a reason that isn't your code.** The harness's tmux socket lives under `/tmp/tmux-$UID/`, outside the sandbox's write allowlist, so every `tmux_*` test panics with `error connecting to /tmp/tmux-1000/dispatch-test-… (Operation not permitted)` while the rest of the suite passes. That is the sandbox, not a regression — re-run just those targets with the sandbox disabled (or `/sandbox` to adjust the allowlist) before you go looking for what you broke. Note the skip above only covers a *missing* binary; this is the installed-but-unreachable case.

The sandbox has a second, less legible failure mode: a plain `grep`/`cargo` invocation dying with `apply-seccomp: unshare(CLONE_NEWUSER): Invalid argument` and **no** output. That string names neither the sandbox nor a path, but it is the same diagnosis — re-run the command with the sandbox disabled. It fires intermittently on commands that are otherwise perfectly sandbox-safe, so don't go hunting for which path you touched.

**Don't pipe `cargo test` into `tail`/`head`/`grep`.** A shell pipeline's exit code is the LAST command's, so `cargo test | tail -40` reports `tail`'s exit status (always 0) — combined with a truncation that happens to cut the summary lines, a failing suite reads as a clean pass. Redirect instead: `cargo test > /tmp/t.txt 2>&1; echo $?`, then `grep -E "^(test result|failures:)" /tmp/t.txt`.

Pick that redirect target with care under the sandbox: bare `/tmp` is **not** writable, and neither `$TMPDIR` nor the session scratchpad path is reliably expanded in a sandboxed shell (both fail as `Permission denied` / `No such file or directory` *before* the suite runs, which looks nothing like a sandbox problem). `mkdir -p /tmp/claude-1000/<something>` first and redirect there, or disable the sandbox for the run.

**The full suite takes ~20 seconds — run it in the foreground.** It used to take
5 minutes, which is why so much of the advice you may have seen elsewhere is
about managing a backgrounded run. Don't background it: `run_in_background`
buys nothing at this length and costs you the failure modes below.

It got there by not rebuilding the schema for every test — see "Schema template"
below. Incremental compilation after a one-file edit (~13 s) is now the larger
half of an edit→test cycle, so if a run feels slow, it is the compile.

If you do background a `cargo` command anyway, two things, both learned the hard
way. **Don't run any other `cargo` command while one is backgrounded** —
`cargo fmt`/`cargo clippy` contend for the same `target/` lock, and a run killed
mid-suite for "no reason" is usually another process holding it (`ps aux | grep
cargo`). And **judge a background run only by its completion notification**: its
redirected output file lags behind (a finished run can still look mid-suite),
and `pgrep` races the teardown, so neither "the tail stopped moving" nor "no
process found" is evidence it died.

Suite is green; if a runtime test fails locally, suspect timing — `spawn_blocking`-based tests are timing-sensitive.

## Schema template

`Database::open_in_memory()` — the constructor behind essentially every
DB-touching test — does **not** replay the migration chain. It clones a
process-wide, already-migrated template via SQLite's backup API
(`init_schema_from_template_sync` in `src/db/mod.rs`). Replaying ~88 migrations
costs ~87 ms per database; the clone costs ~0.05 ms. That single change took the
suite from ~260 s to ~20 s, `db::` alone from 103 s to 4 s.

`Database::open()` (file-backed) still migrates for real, and must: a file on
disk can be at any older `user_version`. The clone is only equivalent because an
in-memory database is always brand new.

**Adding a migration needs no action here** — the template is built by
`init_schema_sync`, the same function `Database::open` uses, so it picks up new
migrations automatically. `src/db/tests/schema_template.rs` fails loudly if that
ever stops being true. It pins four properties, each a way a clone could quietly
differ from a migrated database:

| Guard | Catches |
|---|---|
| identical `sqlite_master` | the template not picking up a new migration |
| identical `user_version` | a clone that looks unmigrated and re-runs the chain |
| identical per-table row counts | a migration-seeded row lost by the clone |
| identical connection PRAGMAs | a PRAGMA set on one init path but not the other |

That last one is the subtle one: PRAGMAs are properties of the *connection*, and
the backup API copies pages only, so the template path has to set them itself.
Both paths call `apply_writer_pragmas`, so they cannot drift by construction —
the test guards against someone re-inlining one of them.

Two traps if you ever think about replacing the mechanism. Capturing DDL from
`sqlite_master` and re-executing it looks equivalent but carries neither
`user_version` nor seeded rows, and is only ~28x faster rather than ~1700x. And
`foreign_keys` reads as the dangerous PRAGMA to drop but isn't: `libsqlite3-sys`
builds bundled SQLite with `-DSQLITE_DEFAULT_FOREIGN_KEYS=1`, so it is on
regardless — `synchronous`, `cache_size` and `temp_store` are the ones that
actually differ from their defaults.

## Snapshot tests

Snapshot review needs the `cargo insta` subcommand, which is **not** a workspace
dependency — install it once with `cargo install cargo-insta`, or use the
`INSTA_UPDATE=always` form below, which needs nothing extra.

Snapshots live in `src/tui/tests/snapshots/` and render to a 120×40 `TestBackend`. **Do not change the backend size** — it breaks all existing diffs.

Agent prompt snapshots live in `src/dispatch/snapshots/` and lock the rendered output of every `build_*_prompt` variant. `src/dispatch/prompts/` holds only the two review addenda as markdown (`pr-review.md`, `dependabot.md`, inlined via `include_str!`) — the dispatch, quick-dispatch, and research bodies are string-built in `src/dispatch/prompts.rs`.

To accept intentional UI changes:

```bash
cargo insta review                                  # interactive
INSTA_UPDATE=always cargo test tui::tests::snapshots # auto-accept
INSTA_UPDATE=always cargo test dispatch::prompts_snapshots # auto-accept prompt snapshots
rm src/tui/tests/snapshots/*.snap.new                # always clean up
rm src/dispatch/snapshots/*.snap.new                 # always clean up
```

**Don't skip the `rm *.snap.new` cleanup.** A stray `.snap.new` left in the tree is picked up by the next `cargo insta review` and silently mixed into an unrelated review pass, making it easy to accept the wrong diff. Always remove them once you've accepted (or rejected) a change.

## Where new tests go

| What you're testing | Where |
|---|---|
| TUI key handling / message flow | `src/tui/tests/` |
| DB schema, CRUD | `src/db/tests/` |
| A database migration | `src/db/tests/migrations.rs` — the migration fn must be `pub(super)` to be callable from there. See "Adding a Database Migration" in `docs/how-to.md` for the column-guard rule. |
| Service-layer business rules | inline in `src/service/<domain>/` |
| MCP JSON-RPC handler behaviour | `src/mcp/handlers/tests/` |
| Full task/epic lifecycle | `tests/` (integration tests) |
| Domain-type invariants | inline in the owning module |
| Agent prompt rendering (all variants) | `src/dispatch/prompts_snapshots.rs` |
| Agent-facing skill copy (`plugin/skills/*/SKILL.md`) | `mod tests` in `src/setup/plugins.rs` (via `skill_body`) |
| tmux semantics — which pane, which cwd, how many panes, which window a name resolves to | `tests/tmux_lifecycle.rs` (topology/cwd) / `tests/tmux_split_hook.rs` (split-pane cwd and keystroke absence) / `tests/tmux_window_targets.rs` (exact window-name resolution under prefix collisions) / `tests/tmux_editor_pane.rs` (agent-tree editor pane, and which pane the toggle kills), shared rig in `tests/tmux_harness/mod.rs` |
| tmux argv shape — that we sent the right command string | `MockProcessRunner` tests inline in `src/tmux.rs` |
| Anything that drives a dispatch/resume/provision/finish through a mock | wherever the behaviour lives, but script the runner with `DispatchScript` (`src/dispatch/mock_sequence.rs`) — never a hand-written `vec![ok(), ok(), …]`. `DispatchScript::finish()` covers the `finish_task` rebase path, including anything that reaches it through `wrap_up` |
| A `pub(in crate::tui::ui)`-or-narrower helper (unreachable from `src/tui/tests/`) | inline in the owning module, e.g. `staleness_color`/`feed_role_label` in `src/tui/ui/shared.rs`, `budget_spans` in `src/tui/ui/budget.rs` |

The two tmux rows are a real split, not two spellings of the same thing: a mock proves *which command we sent*, a real tmux server proves *what tmux did with it*. Read the "`MockProcessRunner` vs a real tmux server" section of `docs/conventions.md` before picking one — guessing wrong is how #3781 and #3782 stayed green while broken.

Property tests live alongside unit tests in a nested `mod property_tests` block.

Skill copy is asserted with targeted `contains` checks (not snapshots) so that deleting a specific instruction reads as a regression rather than an edit. Scope each assertion to the instruction's heading section — sibling sections repeat phrases, so a whole-document `contains` can still pass after the instruction is gone.

The same hazard has a rendering form: the buffer-search helpers in `src/tui/tests/helpers.rs` scan **every row of the whole terminal buffer**, not the overlay's rect. An overlay whose bottom rows are clipped off still satisfies an assertion for a bare `"close"` or `"cancel"`, because the board's own footer hint bar is drawn outside the popup and uses the same words. Assert the overlay's exact wording including its bracketed hints (`"[q/Esc] close"`), and run any new overlay assertion against the unfixed code first to confirm it really goes red.

Inline test modules (`mod tests`, `mod property_tests`) must have `#[allow(clippy::unwrap_used, clippy::expect_used)]` at the top — the workspace `-D warnings` policy otherwise rejects bare `unwrap()`/`expect()` calls. See `src/db/tests/mod.rs` for the canonical pattern.

## No wall-clock sleeps in tests

Tests must never sleep on the wall clock — not to "wait for" `spawn_blocking` or detached `tokio::spawn` work, and not to cross a duration threshold — and must never measure it either. Instead await a deterministic completion signal (oneshot / `Notify` / an `McpEvent`), inject a clock, or inject the threshold (`Database::set_slow_call_threshold`, used by `src/db/tests/async_handle.rs`). `./scripts/check-no-test-sleep.sh` (in the pre-push hook, with its own self-test) enforces this: no `tokio::time::sleep` anywhere under `src/`/`tests/`, and no `std::thread::sleep` or `.elapsed()` in test code — test files *and* top-level inline `#[cfg(test)] mod` blocks. Production use of both is unaffected, and a deadline-bounded poll may carry an `// allow-test-sleep: <why>` marker. When a test's subject *is* a deadline, bound it structurally (`tokio::time::timeout`, `Receiver::recv_timeout`) rather than asserting on measured elapsed time. See the "No `tokio::time::sleep` in tests" section of `docs/conventions.md` for the exact scoping rule, its remaining blind spot, and the canonical patterns.

## Coverage

CI's `coverage` job runs `cargo tarpaulin --engine llvm --out xml --out stdout --fail-under 88` (`--out Html` locally). It is **gated**: coverage below the floor fails the job. Each tarpaulin invocation re-runs the whole suite, so both output formats come from one run — don't add a second invocation to render another format.

**The engine is part of the measurement.** On the same tree, `--engine llvm` scored 90.28% (14846/16445 lines) and the default `Auto` engine 88.54% — a ~1.8-point instrumentation difference with no code change behind it. The floor is calibrated against llvm, which is why CI pins it; quote the engine whenever you quote a number, and don't compare a local default-engine run against the CI floor.

The floor is 88, deliberately ~2 points below the measured figure (90.28%, 2026-08-16). It is a regression tripwire, not a target: raise it by hand when a step-change in coverage makes the headroom pointless, never automatically to whatever the last run scored. Don't chase 100% on render-heavy code or `src/setup/`'s OS-interaction branches (hooks, filesystem writes) — a single file below the average is not by itself a problem.

Coverage is not in the pre-push hook; every *other* CI gate is, and `tests/ci_gates.rs` asserts the hook's script list and the workflow's stay in sync.
