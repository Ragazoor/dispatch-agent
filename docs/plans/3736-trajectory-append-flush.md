# Fix silent trajectory data loss: `append_entry` never flushes

## Root cause

`append_entry` (`src/mcp/trajectory.rs:22-62`) writes a line via
`file.write_all(...).await` and then lets `file` drop. `tokio::fs::File`
buffers small writes in-process and only issues the real OS-level write
when the buffer fills, or when `flush()`/`shutdown()` is called, or
(best-effort, un-awaited) on `Drop`. Since a JSON trajectory line is far
smaller than the internal buffer, `write_all().await` returning `Ok(())`
does **not** mean the bytes reached the file — they can still be sitting
in the dropped `File`'s in-process buffer, racing against whatever reads
the file next (another `append_entry` call, or a test). Under load, the
background flush spawned by `Drop` frequently loses that race, and the
line is gone with no error anywhere (every failure branch only
`tracing::warn!`s).

## Test plan (red first)

The existing `append_adds_second_line` test only fails under real
full-suite parallel load — not a usable red test on its own. Replace it
with a test that forces the race deterministically: spawn N (30)
concurrent `append_entry` calls against the *same* trajectory file, each
with a distinct marker in `method`, then read the file back and assert
all N distinct markers are present. With concurrent open+write+drop on
the same path, the buffered-write-vs-drop race reproduces reliably
without depending on other tests running in parallel.

1. Add `append_entry_survives_concurrent_writes` to
   `src/mcp/trajectory.rs` tests. Confirm it fails repeatedly
   (run several times) against current code.
2. Implement the fix.
3. Confirm the new test — and the full suite, run at least twice — pass.

## Fix

- Add `file.flush().await` after the successful `write_all`, before
  `append_entry` returns. `flush()` (not `sync_all()`) is enough: it
  forces the buffered write through to the OS so a subsequent
  open/write or read sees it, without paying an fsync per MCP call on
  this hot path. Durability across a hard crash is not a stated
  requirement here (this is a best-effort audit log, per
  `TrajectoryWriteFailureIsSilent` in `docs/specs/observability.allium`)
  — decided in favor of `flush()`.
- On flush error, `tracing::warn!` — consistent with every other error
  branch in this function and with the spec's declared "best-effort, no
  retry, no dead-letter" contract. Not propagated to the caller.

## Neighbour audit

`grep -rn 'write_all' src/` (excluding tests): the only other tokio
*async* write-then-drop site is this one. `src/notify.rs`
`write_message_file`, `src/setup/hooks.rs`, `src/setup/plugins.rs`, and
`src/runtime/editor.rs` all use synchronous `std::fs`/`std::io::Write`,
which perform the real syscall inline — no buffering-vs-drop race is
possible there. No other fix needed for the flush bug itself.

The related same-millisecond filename collision in
`write_message_file` (`src/notify.rs:16-23`) is a genuinely separate bug
(the path format is spec'd in `docs/specs/mcp-task-tools.allium:376`),
not cheap to fix in this task's scope — noting it in the PR rather than
fixing it here.

## Spec

Update `docs/specs/observability.allium`'s `TrajectoryAppend` guidance
to state the file is flushed before close (not just "written and
closed"), so the durability guarantee is spec'd, not just implicit in
the code.

## Verification

- `cargo test mcp::trajectory` (new test green, deterministically)
- Full `cargo test` run at least twice
- `cargo clippy --all-targets -- -D warnings`
