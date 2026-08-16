# 3840 — Synchronous subcommands must not start a tokio runtime

**Goal:** `dispatch statusline` (and the other fully synchronous subcommands)
must run with no tokio runtime at all, so the hot, high-frequency invocations
stop paying for a worker-thread pool plus reactor they never use.

## Problem

`main` is `#[tokio::main]` (multi-thread flavour), so *every* invocation of the
binary builds a runtime: one worker thread per core plus the reactor, then tears
it down. Two subcommands run on Claude Code's hot paths and do no async work at
all:

- `statusline` — the `statusLine` command of every dispatch-spawned session, on
  a ~300 ms debounce, concurrently across every active session. `src/cli/statusline.rs`
  is entirely synchronous (`std::process::Command`, `std::thread`, `mpsc`) and
  deliberately avoids the database for exactly this reason.
- `caller-headers` — Claude Code's `headersHelper`, run on every MCP session
  start/reconnect. A pure path parser (`src/cli/caller_headers.rs`).

Three more arms are already backed by non-`async` handlers and get the same
treatment for free: `verify-feed`, `uninstall`, `toggle-agent-tree-pane`.

## Design

`main` becomes a plain `fn main()` that parses argv and matches once:

- The synchronous arms are handled inline, with no runtime in scope.
- Everything else falls through a single catch-all arm that builds the runtime
  (`Builder::new_multi_thread().enable_all().build()?`) and `block_on`s
  `run_async(&cli.db, command)`, which holds today's exhaustive match.

`main`'s match is the single classifier — there is no separate predicate to keep
in sync. `run_async` keeps an arm for the sync variants that is unreachable by
construction (main matched those patterns first); it carries `unreachable!()`
with a comment, which is the sanctioned guarded-panic case (see the
"Rendering purity" note in `docs/conventions.md` — the guard here is an upstream
pattern match in the only caller).

Error handling and exit codes are unchanged: each handler keeps its own
`std::process::exit` (statusline, caller-headers, verify-feed, pr-gate) or
returns `Result`, and `main` still returns `Result<()>` so anyhow's reporting
and the exit code for a returned `Err` are identical.

Runtime-freedom of the moved arms was checked: neither `src/setup/`,
`src/cli/caller_headers.rs`, nor `dispatch::toggle_agent_tree_pane`
(`src/dispatch/agents.rs:58`) calls `block_on`, `Handle::current`,
`tokio::spawn`, or `spawn_blocking`, so none of them can panic outside a
runtime.

## Steps (TDD — test first in each step)

### 1. Spec

Add to `surface StatusLineDecorator` in `docs/specs/dispatch.allium`:

```
@guarantee StartsNoAsyncRuntime
    -- The decorator does no asynchronous work, and starts no machinery for it:
    -- no worker-thread pool, no reactor. At several invocations a second in
    -- every session, setup it cannot use is the same waste as database work
    -- (see NeverReadsOrWritesTheDatabase).
```

Verify with `allium check`.

### 2. Failing test — the observable thread-count property

New test in `tests/cli.rs`, `#[cfg(target_os = "linux")]` (needs `/proc`):

`statusline_starts_no_worker_thread_pool` runs the binary with
`--chain 'ls /proc/$PPID/task | wc -l'`. The chain's parent is the `dispatch`
process itself, so its stdout is that process's live thread count at the moment
the decorator is waiting on the chain. `run_bounded` accounts for at most three
threads there (stdin writer + two drains) on top of `main`, so the assertion is
`count <= 4`.

Today the count is `1 + cores + 3`, so the test fails on any machine
(even single-core: 5 > 4). After the change it is 4 or fewer. The upper bound is
deliberately not exact — the stdin-writer thread may already have exited.

### 3. Failing test — `caller-headers` output contract

`tests/cli.rs` has no process-level test for it (only unit tests on
`resolve_headers_for_path`). Add two, so the refactor cannot silently change the
contract:

- `caller_headers_emits_session_header_outside_a_worktree` — run in a temp dir,
  assert exit 0 and stdout parses to `{"X-Caller-Kind":"session"}`.
- `caller_headers_emits_task_header_inside_a_worktree` — run with `current_dir`
  set to `<tmp>/.worktrees/3840-slug`, assert `X-Caller-Task-Id == "3840"`.

### 4. Implement

Restructure `src/main.rs`:

- `#[tokio::main] async fn main()` → `fn main() -> Result<()>`.
- Move the body's match into `async fn run_async(db: &Path, command: Commands) -> Result<()>`.
- Extract the inline `Statusline` arm into `fn cmd_statusline(snapshot: &str, chain: Option<&str>) -> !`-shaped
  handler (reads stdin, timestamps, calls `cli::statusline::run`, exits) so both
  the sync arm and the (unreachable) `run_async` arm name one function.
- `main` matches the five sync arms, catch-all builds the runtime and blocks on
  `run_async`.

### 5. Regression sweep

`tests/cli.rs` already covers `list`, `update`, `plan`, `verify-feed`,
`repo *`, `prune-repo-paths`, `hook*`, `pr-gate`, `statusline`. Run the whole
file and confirm every arm still routes; no arm's behaviour is being changed, so
no snapshot or expectation should move.

## Verification

```
cargo fmt --check && cargo test && ./scripts/check-doc-paths.sh
```

plus `cargo clippy --all-targets -- -D warnings` (pre-push gate) and
`allium check docs/specs/dispatch.allium`.

## Risks

- **Thread-count test portability.** Linux-gated; skipped elsewhere. The bound
  is an inequality, not an equality, so a future extra drain thread in
  `run_bounded` would need the bound revisited — the test comment says so.
- **A new subcommand added later** lands in the async catch-all by default,
  which is correct-but-unoptimised. That is the safe direction.
