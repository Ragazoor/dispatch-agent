# Consolidate the two duplicated setup/subprocess helpers (#3841)

Two independently-written implementations of the same primitive, both found by
the `/simplify` reuse pass while wrapping up #3821. Pure quality work: **no
user-visible behaviour change**, so the existing tests passing unchanged is the
primary signal. No Allium spec changes — see "Spec impact" below.

---

## Part 1 — one write-if-changed, one equality rule

### Current state

| | `src/setup/plugins.rs:117` `write_file_if_changed` | `src/setup/statusline.rs:81` `write_settings_file` |
|---|---|---|
| compares | exact equality | `.trim()`-normalized |
| creates parent dir | no — caller (`install_dir_recursive`) does it | yes, inline |
| permissions | `0o755` when `executable` | none |

### Decision: exact equality wins; trim goes away

Not a coin flip. `plugins.rs` has a *second* reader of the same predicate:
`needs_update_recursive` (`src/setup/plugins.rs:140`) decides "does the plugin
need updating?" with exact equality. If the writer became trim-normalized, a
plugin file differing from the embedded copy only in trailing whitespace would
be reported as needing an update forever while the write reported "no change" —
a permanent, silent inconsistency between the two.

For the statusline call site, trim buys nothing: the content it compares against
is the string it is about to write (`serde_json::to_string_pretty`, no trailing
newline), so for any file *we* wrote the two rules agree. They differ only when
something else edited the file, and there exact equality is the better answer —
it restores the canonical bytes. It converges (the rewrite removes the stray
whitespace), so there is no flip-flop across the repeated
`runtime::bootstrap` writes.

### Home: `src/setup/mod.rs`, not `plugins.rs`

The task text says "widen `write_file_if_changed` to `pub(in crate::setup)`".
Deviating slightly: it *moves* to `src/setup/mod.rs` instead of being widened in
place. `mod.rs` already hosts the module's shared file helpers (`read_json_file`,
`write_json_file`) at the same visibility, and a helper shared by `plugins` and
`statusline` living inside `plugins` is exactly the kind of thing the next reuse
pass flags. Same visibility outcome, honest location.

### Steps

1. **Test first** — in `src/setup/mod.rs`'s test module, a new
   `write_file_if_changed_creates_missing_parent_directories`. Fails today
   (no `create_dir_all` in the plugin version).
2. **Test first** — in `src/setup/statusline.rs`,
   `write_reports_change_when_only_trailing_whitespace_differs`: write the file,
   append `"\n\n"` to it, assert the next `write_settings_file` returns `true`
   and the file is back to the canonical bytes. Fails today (trim says
   unchanged). This is the one deliberate behaviour change, so it gets a test
   that names it.
3. Move `write_file_if_changed` to `src/setup/mod.rs` as
   `pub(super) fn write_file_if_changed(path, content, executable) -> Result<bool>`,
   adding the `create_dir_all(parent)` step at the top. Move its four existing
   tests (`..._creates_new`, `..._skips_identical`, `..._updates_stale`,
   `..._sets_executable_permission`) across with it, unchanged.
4. Drop the now-redundant `create_dir_all` from `install_dir_recursive`
   (`src/setup/plugins.rs:46`) — one place does it now.
5. Rewrite `write_settings_file`'s body to build the JSON and then
   `super::write_file_if_changed(path, &content, false)`.
6. `cargo test setup::` — every pre-existing test in both files must pass
   untouched, including `write_is_idempotent`.

---

## Part 2 — one bounded-subprocess primitive

### Current state

`RealProcessRunner::run_with_timeout` (`src/process.rs:138`) and
`run_chain` + `reap_before` (`src/cli/statusline.rs:102`) are two hand-written
kill-on-timeout implementations with different mechanics (polling `try_wait`
vs `mpsc::recv_timeout`). Correctness-sensitive against the same OS hazards.

`run_chain` additionally needs what `run_with_timeout` has no notion of: a
payload written to the child's stdin, concurrently with draining its stdout, or
a >64 KiB payload deadlocks against the pipe buffer (covered by
`chain_that_echoes_a_large_payload_does_not_deadlock`).

### Decision: extract a free function, don't route statusline through the trait

The task text says "extending `ProcessRunner::run_with_timeout` with an optional
stdin payload, then reusing it". Deviating: the shared primitive becomes a plain
`pub(crate) fn run_bounded` in `src/process.rs`, and `run_with_timeout` becomes a
one-line delegation to it. Reasoning:

- The duplication being removed is the *primitive*, not the trait method. One
  `run_bounded` is one implementation of kill-on-timeout — the stated goal.
- Routing `run_chain` through `ProcessRunner` would mean threading a runner into
  the statusline CLI, which today has no injected dependencies by design (it
  must never open the database and never fail). The seam buys nothing: the
  hazards it guards against — real pipe buffers, a child that closes stdout and
  keeps running — are only observable against a real OS, so the tests must spawn
  real processes either way. A mock cannot test any of it.
- Adding a stdin parameter to the trait method would touch ~15 `run_with_timeout`
  call sites plus `MockProcessRunner` for a capability exactly one caller wants;
  adding a *second* trait method would leave it with one implementor and no mock
  coverage. Both are worse than a free function.

### Shape

```rust
/// Run `program` with `args`, writing `stdin` to it when present, and kill it if
/// it has not exited within `timeout`.
pub(crate) fn run_bounded(
    program: &str,
    args: &[&str],
    stdin: Option<&str>,
    timeout: Duration,
) -> Result<Output>
```

- `stdout`/`stderr` piped and drained on background threads (as today) so the
  pipe buffer cannot fill while we poll.
- `stdin`: `Stdio::piped()` + a writer thread when `Some` — the thread owns the
  payload and the pipe, so the pipe drops (and the child sees EOF) when it
  finishes, and a child that never reads stdin just gives the writer an ignored
  `EPIPE`. When `None`, stdin stays **inherited**, exactly as
  `run_with_timeout` leaves it today; switching git's stdin to `null` would be a
  behaviour change (credential prompts) and is out of scope.
- One deadline covering exit; on expiry `kill` + `wait` + `bail!("{program} timed
  out after {timeout:?}")`, preserving the current error wording that
  `real_run_with_timeout_kills_stuck_process_and_returns_error` and the mock's
  message assert on.

### The poll cadence has to change

`run_with_timeout` polls every fixed 50 ms. The statusline path runs on Claude
Code's sub-second debounce and today returns the instant the child closes stdout
(`recv_timeout`, no polling), so adopting a flat 50 ms poll would add up to 50 ms
of latency to every status line redraw. So `run_bounded` backs off instead:

```rust
const POLL_MIN: Duration = Duration::from_millis(1);
const POLL_MAX: Duration = Duration::from_millis(50);
fn next_poll_step(prev: Duration) -> Duration       // doubles, capped at POLL_MAX
fn poll_sleep(step: Duration, remaining: Duration) -> Duration  // min of the two
```

A command finishing in a few ms costs a few 1–2 ms polls; a 60 s `git fetch`
settles at the same 50 ms cadence as today. `poll_sleep` clamping to the
remaining budget is what keeps a 100 ms deadline from overshooting by 50 ms —
`statusline.rs`'s 5 ms `WAIT_POLL_STEP` exists for that reason and this replaces
it. Both are pure functions, so they get deterministic unit tests rather than
timing assertions.

### Steps

1. **Test first** — in `src/process.rs`'s test module, four new tests, all
   failing to compile/pass before the function exists:
   - `run_bounded_writes_stdin_and_returns_stdout` — `cat` echoes a payload back.
   - `run_bounded_does_not_deadlock_on_a_large_payload` — 200 KB through `cat`,
     comfortably past the ~64 KiB pipe buffer.
   - `run_bounded_kills_a_child_that_closed_stdout_but_keeps_running` —
     `sh -c "exec 1>&- ; sleep 30"` with a 100 ms budget returns `Err` and
     returns fast. This hazard's coverage currently lives only in the statusline
     module; it belongs with the primitive that now owns it.
   - `run_bounded_ignores_a_child_that_never_reads_stdin` — `echo hi` with a
     payload present must not hang or panic on the writer thread's `EPIPE`.
   - plus `next_poll_step_backs_off_and_caps` and
     `poll_sleep_never_overshoots_the_deadline` as pure-function tests.
2. Implement `run_bounded` and `next_poll_step` / `poll_sleep`; reduce
   `RealProcessRunner::run_with_timeout` to
   `run_bounded(program, args, None, timeout)`.
3. Rewrite `run_chain` in `src/cli/statusline.rs` as
   `run_bounded("sh", &["-c", chain], Some(stdin), timeout)` mapping `Ok` to
   `String::from_utf8_lossy(&out.stdout)` and any `Err` to `String::new()`.
   Delete `reap_before`, `WAIT_POLL_STEP`, and the now-unused `mpsc` / `Read` /
   `Instant` imports. Keep the module comment's two-hazard explanation, updated
   to point at `run_bounded` instead of restating the mechanics, and drop the
   "cannot be reused here" paragraph that this change falsifies.
4. `cargo test cli::statusline` — all six chain tests must pass **unchanged**.
   Two behaviour deltas to be explicit about, neither observable through the
   spec's guarantees:
   - The chain's stderr is now piped and discarded instead of inherited by the
     decorator's stderr. Strictly quieter; stdout — the only thing the status
     line shows — is unaffected.
   - A non-zero-exit chain still has its stdout returned (`run_bounded` does not
     inspect `status`), matching today.
5. Add one line to the subprocess section of `docs/conventions.md` naming
   `run_bounded` as the single place a bounded child is spawned, so a third
   implementation gets caught in review rather than by the next `/simplify`.

---

## Spec impact

None. Checked `docs/specs/dispatch.allium`'s `StatusLineDecorator` (lines
1323–1400): every guarantee it makes — `AlwaysSucceeds`,
`ChainedOutputReproducedVerbatim`, `ChainedCommandIsBounded` ("one budget covers
output AND exit"), `NeverReadsOrWritesTheDatabase` — is preserved verbatim by
Part 2, and `run_bounded` lives in `src/process.rs`, which has no database
dependency. The spec says nothing about how the settings file's
already-up-to-date check compares content, so Part 1's equality decision is
below the spec's altitude. Nothing to `tend`; `weed` should stay quiet.

## What changed during implementation

Two deviations from the plan above, both from the `/simplify` review pass:

1. **The poll cadence was the wrong fix.** The plan's 1 ms→50 ms backoff
   (`POLL_MIN`/`next_poll_step`/`poll_sleep`) treated the statusline's latency as
   a poll-interval problem. It wasn't: the old `run_chain` blocked on the stdout
   drain channel, so it paid *zero* timer wakeups and returned at EOF. Any poll
   interval is a regression against that. `run_bounded` now waits on stdout's EOF
   (`recv_timeout`, bounded by the deadline) and only then polls for exit, paced
   by a single `EXIT_POLL_STEP` — which also removes an unbounded final `recv()`
   the old `run_with_timeout` had, and cuts a 60 s `git fetch` from ~1 200
   `try_wait` calls to one. The backoff helpers and their two tests are gone.
2. **The spec did need a clause after all.** Bounding the wait on *exit* rather
   than on output means a chain that prints, closes stdout, then hangs now
   contributes nothing, where the old code returned what it had printed. The
   spec's `ChainedCommandIsBounded` was silent on an abandoned command's output —
   a gap, not agreement — so it now says so explicitly, with
   `run_bounded_discards_output_from_a_child_that_then_overruns` covering it. The
   pre-existing test missed this because its chain prints nothing.

Also from that pass: `file_is_up_to_date` was extracted so
`plugin_needs_update_in` and the writer share the equality rule structurally
rather than by prose agreement; the shared helper is plain-private rather than
`pub(super)` (which in `src/setup/mod.rs` resolves to crate-wide); the redundant
`create_dir_all` in `runtime::ensure_statusline_settings_file_in` is gone; and
the tests that duplicated primitive mechanics at the `run_chain` level were
removed in favour of the `run_bounded` ones.

## Verification

```
cargo fmt --check && cargo test && ./scripts/check-doc-paths.sh
```

Plus the pre-push gate (clippy `-D warnings`, `check-doc-symbols.sh`,
`check-no-test-sleep.sh` — no test sleeps are added; the new tests spawn `sleep`
as a *subprocess*, which the checker does not and should not flag).
