# Dispatch

Terminal kanban board for dispatching Claude Code agents into isolated git worktrees via tmux.

**Stack**: Rust (2021 edition), ratatui TUI, SQLite (rusqlite), Axum HTTP/MCP server, tokio async runtime.

## Build & Test

```bash
cargo build
cargo test
cargo run -- tui
```

**This repo has a verify command every dispatched agent must run green before declaring work complete — read it from `get_task`'s *Verify command* line, never from this file.** See "Verify Command" below.

**Dispatch-spawned sessions do not run under Claude Code's sandbox** — see `SandboxDisabledForDockerAndUnixSockets` in `docs/specs/dispatch.allium`. If you've enabled the sandbox yourself outside of dispatch, see "Sandbox (historical)" in [docs/reference.md](docs/reference.md).

**The full suite needs `tmux` on `PATH`.** Without it the `tmux_*` targets print `skipping: tmux not available on PATH` and pass, so a green local run isn't proof they ran.

**Don't pipe `cargo test` into `tail`/`head`/`grep`.** A pipeline's exit code is the last command's, so a failing suite reads as a clean pass. Redirect instead: `cargo test > /tmp/t.txt 2>&1; echo $?`.

**The lib target runs in ~10s; a cold full run (including compile) is ~80s.** Run it in the foreground — don't background it.

**Local coverage**: `cargo tarpaulin --engine llvm --out stdout`. The default `Auto` engine reads ~1.8 points lower than `llvm`, so don't compare an `Auto` run against the CI floor.

Everything else about tests — the per-target command list, snapshot workflow, where a new test belongs, the no-wall-clock-sleep rule, coverage — is in [docs/testing.md](docs/testing.md).

**`main` moves while you work.** Other agents land on it during your session, so
the snapshot you read at startup goes stale. Before wrapping up, run `git log
--oneline main..HEAD` **and** `git log --oneline HEAD..main` — the second is the
one that catches a base that moved under you. If it is non-empty, merge `main`
into your branch and re-run the suite before reporting completion; a green run
against a stale base proves nothing. Assume a function you did not write may have
been rewritten since you read it.

**Compare against `main`, not `origin/main`.** A sibling agent wrapping up with
the rebase path fast-forwards the *local* `main` in the parent checkout and does
not push, so `HEAD..origin/main` can be empty while `HEAD..main` holds the very
commit that moved your base. Fetching first does not help — there is nothing on
the remote to fetch. Substituting `origin/main` here reads as "main has not
moved" and is wrong in exactly the case this check exists for.

**A clean merge doesn't mean no conflict.** `HEAD..main` isn't only a code-conflict
check. A sibling task's commit can record a *design decision* — in a
`docs/plans/` doc or an Allium guarantee — that directly contradicts what you're
mid-implementing, in files yours never touches, so `git merge` succeeds with
nothing to resolve. Skim new commits' content, not just their file list, before
wrapping up; if one conflicts with a decision you're making this session, surface
it to the user rather than silently proceeding either way.

### First-time setup

A fresh clone must point git at the tracked hooks once: `git config core.hooksPath .githooks`. Nothing does this for you, and until it is run the whole gate below is silently inert locally — CI runs the same checks (see "CI" below), so skipping the setup costs you the fast local feedback, not the enforcement. Don't add hooks to `.git/hooks/` directly — that directory is untracked and shared across all worktrees, so changes there aren't version-controlled or reviewed.

The pre-push hook (`.githooks/pre-push`) runs, in order: `cargo fmt`, `cargo clippy --all-targets -- -D warnings` (no `--fix`), `./scripts/check-doc-paths.sh` + its self-test (validates every path and `file:NN` citation in `README.md`/`CLAUDE.md`/`docs/*.md`/`docs/specs/*.allium`, excluding the dated `docs/plans/`, `docs/superpowers/`, `docs/research/`), `./scripts/check-doc-symbols.sh` + its self-test (rejects citations — backticked identifiers, `path.rs::symbol`, `Type::method`, bare snake_case names of five-plus words, and `<spec>.allium's Block` cross-spec names — that no longer resolve; annotate a deliberate exception with `allow-phantom-symbol: <why>` on the citing line or the one directly above), `./scripts/check-no-test-sleep.sh` + its self-test, and `bash ./scripts/test-fetch-reviews.sh`. Run `cargo test` separately before pushing — it is not part of the hook.

**That `cargo fmt` step has no `--check`.** Pushing reformats your working tree in place, so a push can leave you with unstaged changes you did not make. Run `cargo fmt` yourself before committing and the step becomes a no-op.

Prefer `path::symbol` citations (`src/feed/exec.rs::exec_feed_command`) over `file:NN` line numbers in docs: a line number is only bounds-checked (confirmed to exist, never that it still says what the doc claims), while a symbol is checked against the real file. See "`file:NN` vs `path::symbol` citations" in `docs/conventions.md` for what each checker does and doesn't catch.

### CI

`.github/workflows/ci.yml` runs five jobs: Test, Clippy, Format, Coverage, and **Gate scripts** — the last mirrors every `scripts/*.sh` check the pre-push hook runs, in the same order. The two lists are kept in sync by `tests/ci_gates.rs`, which fails if the hook gains a script CI does not run. Coverage is a gate, not a report: tarpaulin runs once, emits both the XML artifact and the stdout summary, and fails the job under the floor (see [docs/testing.md](docs/testing.md)).

## Running & Debugging Locally

`cargo run -- tui` needs a tmux server **already running** — start one first, or launch the TUI from inside a tmux session. Point dev runs at a throwaway database (`cargo run -- --db /tmp/scratch.db tui`), never your real one.

Logs do not go to stderr — stderr belongs to the TUI. They append to `app.log` next to the database file; `tail -f ~/.local/share/dispatch/app.log`. Database location, port, environment variables, the full CLI subcommand list, and troubleshooting are in [docs/reference.md](docs/reference.md); driving MCP by hand is in [docs/mcp.md](docs/mcp.md).

## External Dependencies

**tmux**, **git**, **gh**, and **claude** must be on `PATH` at runtime. There is **no startup preflight** — nothing checks binary availability, so a missing binary surfaces as a failed shell command mid-operation. Per-binary detail (which module calls what, and the two load-bearing `claude` flags) is under "External Dependencies" in [docs/reference.md](docs/reference.md).

Dispatched agents do not run under Claude Code's sandbox mode (see "Build & Test" above). `bubblewrap` and `socat` on `PATH` (`sudo dnf install bubblewrap socat` on Fedora) only matter if you re-enable the sandbox yourself — see `SandboxedAgentExecution` in `docs/specs/dispatch.allium`; if either is missing, Claude Code warns and silently falls back to running unsandboxed rather than failing to start.

POSIX-only. Embeddings/RAG (`src/service/embeddings.rs`) run **locally** — `fastembed` does inference in-process, no API key, no per-call network I/O. The only network activity is a one-time model download on first init.

## Verify Command

A per-repo, single-line shell command that dispatched agents must run before declaring work complete. Stored on the `repo_paths` row for the task's `repo_path`; set via the `set_verify_command` MCP tool or `cargo run -- repo set-verify <path> <command>`. Newlines and carriage returns are rejected — chain steps with `&&` or `;`. It never appears in the dispatch prompt. Instead it reaches the agent through two surfaces: `get_task`'s "Verify command" line (read by the `/wrap-up` skill in its Step 2, and acted on in its Step 7, before the closing sequence ever calls `wrap_up`), and, as a secondary reminder, the `wrap_up` response's action-specific "Verify before exiting" line (`src/mcp/handlers/tasks/wrap_up.rs::wrap_up_verify_line`). See `docs/specs/dispatch.allium` for why both exist.

## Working With the User

The most important thing is to stay aligned with the user. The Allium specs in `docs/specs/` are the shared source of truth that alignment is expressed in — when the spec and your intent agree, you are aligned; when they don't, one of them is wrong and it must be resolved before code is written.

- **Ambiguity is a stop condition, not a judgement call.** If the spec is silent, contradictory, or open to more than one reading, ask. Do not pick the plausible interpretation and proceed.
- **Behaviour changes start in the spec.** Spec first, then tests, then code (see the two sections below). This applies to UI and interaction behaviour too — that is a first-class Allium surface, not a prose note.
- **Agreement gets recorded, in one of two places.** A decision about *what the system does* goes into the relevant `docs/specs/*.allium` file. A decision about *how to work in this repo* — a convention, a pitfall, a gotcha that would trip the next agent — goes into the knowledge base via `record_learning`. A decision that lives only in the conversation is lost the moment the session ends.

## Test-Driven Development

Always use TDD. Express intended behaviour as tests before writing the code that satisfies them — for new features, bug fixes, and refactors alike.

## Allium Specification

The Allium specs in `docs/specs/` are the **source of truth** for domain and interaction behaviour. Each filename names its domain — `tasks.allium`, `dispatch.allium`, `epics.allium`, `feeds.allium`, and so on; `core.allium` holds the shared domain model. Consult the relevant spec before changing core behaviour, and use the `allium:tend` and `allium:weed` skills to keep spec and code aligned.

## MCP Tools for Agents

The `dispatch` MCP server exposes more than task creation. Worth knowing by name:

- **Knowledge base** — relevant learnings are already injected into your prompt at dispatch time ("Validated knowledge for this task" above); rate each one you act on with `rate_learning` (`helped`/`wrong`). Call `query_learnings` yourself for anything not already surfaced, and `record_learning` to capture a new pitfall/convention/tip. See `docs/specs/learnings.allium`.
- **Your own task** — `get_task` / `update_task` to read or mutate the task you're running as (title, description, status, plan, tag).
- **Finishing** — `wrap_up` + `exit_session` to close out a session (see the `/wrap-up` skill). `exit_session` chains the epic's next backlog subtask automatically when `auto_dispatch` is on; there is no tool for you to call.

`create_task`/`create_epic` matter mainly to orchestrating agents decomposing work, not to an agent executing a single dispatched task. Full tool list and schemas: call `tools/list`, or see `docs/specs/mcp-task-tools.allium`.

## Agent-Facing Skill Copy

`plugin/skills/*/SKILL.md` is the **source of truth** for the skills agents run (`/wrap-up`, `/retro`, `/learnings`, `/grill`, `/summarize`, `/decompose-review`, `/allium-loop`). The directory is embedded in the binary via `include_dir!` and only reaches `~/.claude/plugins/local/dispatch/` when someone runs `cargo run -- setup` — editing the installed copy is editing a build artifact. Changes there are asserted by the `contains` tests in `src/setup/plugins.rs` (via its `skill_body` helper); see [docs/testing.md](docs/testing.md).

`.claude/skills/` is a separate, unrelated path: a plain tracked directory Claude Code auto-discovers for any session inside this repo, with no build or install step.

## Agent Working Directory

Dispatched agents always work from their worktree folder. Every prompt includes an instruction to stay in the worktree and not `cd` to the parent repo. The tmux window's *starting* cwd is test-covered: `dispatch_agent_opens_tmux_window_in_worktree_not_parent_repo` in `src/dispatch/tests.rs` asserts the window opens inside the task worktree, never the bare parent repo. Runtime `cd`-escape prevention — an agent later `cd`ing out of the worktree — remains prompt-instruction only, with no test asserting against it.

<!-- allow-phantom-symbol: file_path names a Claude Code tool parameter, not a repo symbol -->
**A second, easier way to leave the worktree: an absolute Read/Edit/Write `file_path` missing the `.worktrees/<id>-<slug>/` segment.** The parent repo's path and the worktree's path both look like valid absolute paths and differ only by that one segment, but using the former silently edits the parent checkout instead of the worktree — the tool reports success and even Read echoes the change back, so nothing looks wrong until a shell command (`git status`, `cargo build`) run against the *worktree* path shows no change. If the parent checkout has any auto-commit/snapshot tooling watching it, a stray edit like this can land on shared `main` before anyone notices. Always build `file_path` from the worktree's own absolute path (e.g. from `pwd`), never assume the parent repo's path with the task ID spliced in.

## Documentation

This file is intentionally slim — it is loaded into every agent's context. Read these on demand:

> **`FieldUpdate`/`TaskPatch`** (nullable field mutations) is the most-touched pattern here — read [docs/conventions.md](docs/conventions.md) before writing an update handler; it also covers the `OwnedTaskPatch` parity hazard, now compiler-enforced via exhaustive destructuring.

> Bare `unwrap()`/`expect()` outside tests are clippy-warned but only hard-fail via `-D warnings` (the pre-push hook's `cargo clippy --all-targets -- -D warnings`) — a plain local `cargo build`/`cargo clippy` won't catch it. See the soft-fail-decoding section of `docs/conventions.md` for the fallback pattern.

> **Mutation boundary** (compiler-enforced): reads via `state.db` are fine, but task/epic mutations go through `TaskServiceApi`/`EpicServiceApi`, never the DB directly — `state.db` is typed `Arc<dyn db::TaskReadStore>`, so `state.db.patch_task(...)` from a handler is a compile error. `TaskReadStore` only seals task/epic writes; settings/learning/usage writes stay reachable through it. Sanctioned direct-mutation exceptions (own write handle, own invariants): `FeedRunner`, `TuiRuntime::feed_db`, and startup/CLI paths (`runtime::bootstrap`, `src/setup/`, `src/main.rs`) — but CLI handlers still route through `TaskService` (e.g. `cmd_plan` → `TaskService::attach_plan`). See "Sanctioned direct-mutation consumers" and `recalculate_epic_status` in `docs/conventions.md`.

> **Dispatch seam**: launching an agent is `TaskService::dispatch` (`src/service/tasks/dispatch.rs`) — claim → prologue → `DispatchMode` match → blocking provision → record worktree/tmux → release the claim on failure. A new entry point extends it (a `DispatchClaim` variant, a `DispatchMode` variant) rather than re-deriving the sequence: `DispatchClaimExclusive` is the most safety-critical rule in the system, and a second copy is one that can drift. Skipping it also silently skips the claim's stale-counter clear and epic recalculation. See `docs/module-map.md` and the module's own doc comment.

> **Layout-cache coherence** (self-healing, not compiler-enforced): `App.layout` (`LayoutCache`) derives from `board.tasks`/`board.epics`. Call `invalidate_layout_cache()` after a mutation as a perf optimization, but `cached_epic_stats()` fingerprints the board on every call and self-heals on mismatch — a handler that forgets to invalidate cannot serve stale data. See `docs/architecture.md`.

> **DB connection model**: one writer `tokio_rusqlite::Connection` (`src/db/mod.rs`) — mutations serialize through it via `db_call` — plus a read-only pool via `db_call_read`, so concurrent reads don't queue behind the writer. See "DB access" in `docs/conventions.md`.

> **Render-panic policy**: a guarded `unreachable!()` in a render match arm is fine when an upstream filter/type already rules that arm out (e.g. `ColumnItem` variants stripped before the match in `src/tui/ui/kanban/columns.rs`) — but MCP handlers and `src/tui/input.rs` must never panic, guarded or not. See "Rendering purity" in `docs/conventions.md`.

> **Workhorse macros**: `patch_struct!` (`src/db/mod.rs::patch_struct`) generates `TaskPatch`/`EpicPatch`; `mcp_tools!` (`src/mcp/handlers/dispatch.rs::mcp_tools`) generates the MCP tool registry; the `service_api!` family in `src/service/api.rs` (`task_service_api!`/`epic_service_api!`/`todo_service_api!`/`learning_service_api!`) generates each `*ServiceApi` trait, impl, and test stub. Read the module doc comment before adding a patch field, an MCP tool, or a service-seam method by hand.

> **Unsafe policy**: any `unsafe` block requires a `// SAFETY:` comment justifying why the invariant holds, plus reviewer sign-off. Full policy in `docs/conventions.md`.

> **Tag system**: `TaskTag` is a kanban label with exactly two behavioural readers (`DispatchMode::for_task` and `TaskTag::is_review`). See "Tag system" in `docs/conventions.md` before assuming a tag does anything.

> **Read-side layering** (convention, not compiler-enforced): zero `tui → db`, `tui → tmux`, `mcp → tui`, or `service → tui` references; `models` is a true leaf. Keep new read paths on the same seam the rest of the layer uses.

> **`#[cfg(test)]` gating**: test-only scaffolding is gated behind `#[cfg(test)]`, except `MockProcessRunner` in `src/process.rs` (plus `window_name_in_lookup` in `src/tmux.rs`, which exists solely to serve it) and `test_tmux_window` in `src/models/tmux_window.rs` — `tests/` targets depend on both and can't see `cfg(test)` items. Gated instead behind `#[cfg(any(test, feature = "test-support"))]`: the `test-support` cargo feature, off by default, turned on for `tests/` targets via a self dev-dependency in `Cargo.toml` (`dispatch-tui = { path = ".", features = ["test-support"] }`) so it stays out of the release binary.

> **Timing constants**: tick interval, DB refresh, status TTL, PR poll, message flash, main-session poll, the gg-chord timeout, and the dispatch watchdog are documented in "Timing Constants" in `docs/reference.md`.

- [docs/testing.md](docs/testing.md) — running tests, snapshot workflow, where a new test goes, the no-sleep rule, coverage
- [docs/architecture.md](docs/architecture.md) — Message→Command, ProcessRunner, command queue draining, editor session invariant, layout-cache coherence, render dirty flag, error handling, quick dispatch
- [docs/conventions.md](docs/conventions.md) — the full convention set: `FieldUpdate`/`TaskPatch` double-Option, DB/service trait narrowing, the `run_bounded` primitive, keybinding telemetry, Clippy/visibility rules, tag system, and more
- [docs/module-map.md](docs/module-map.md) — module and subsystem responsibilities
- [docs/how-to.md](docs/how-to.md) — adding an MCP tool, TUI view, entity, database migration; knowledge base MCP tools
- [docs/mcp.md](docs/mcp.md) — MCP notification flow, error codes, debugging handlers, feed epics, knowledge base flow
- [docs/reference.md](docs/reference.md) — key bindings, CLI subcommands, configuration, environment variables, troubleshooting, learning store
- [docs/specs/](docs/specs/) — Allium specifications for domain logic
- [docs/plans/](docs/plans/) — implementation plans and one-off analysis/review docs. **This repo commits them** (the policy was reversed on 2026-07-26); older entries are filed under `docs/plans/archive/`. Neither doc checker scans them, so treat their contents as dated snapshots.

Subsystem entry points (no dedicated doc page — read the source):

- `src/feed/mod.rs` — feed system: `FeedRunner` poll loop, exec/parse/ingest pipeline that upserts tasks from external commands
- `src/cli/` — CLI subcommand implementations (`agent_tree`, `caller_headers`, `statusline`)
- `src/mcp/trajectory.rs` — agent trajectory capture (records the agent's tool-call history for a task)
- `src/repo_sync.rs` — local-first repo sync: `ahead_behind` drift measurement and `sync_repo` (fetch, merge `origin/<base>`, push). See `docs/specs/repo-sync.allium`
