# Fix same-millisecond filename collision in `write_message_file`

Task #3760.

## Problem

`write_message_file` (`src/notify.rs:12-24`) names message files
`{file_prefix}-{timestamp_millis}.md`. Two calls landing in the same
millisecond (concurrent `send_message` calls, or a watcher completion racing
a `send_message`) produce the same filename — the second `std::fs::write`
silently overwrites the first with no error. This is silent data loss in a
notification path, same family as #3736.

## Fix

Add a process-wide monotonic `AtomicU64` counter (same pattern as
`NEXT_MEMDB_ID` in `src/db/mod.rs:675`) and append it to the filename, so
uniqueness no longer depends on clock resolution:

```
{file_prefix}-{timestamp_millis}-{counter}.md
```

No caller parses the filename structure (verified: `dispatch.rs` and
`watchers.rs` only ever pass the filename back opaquely in tmux nudge text),
so appending a segment is safe — existing tests asserting
`starts_with(prefix)` / `ends_with(".md")` remain valid.

## Steps

1. **Test first**: add a test in `src/notify.rs` that calls
   `write_message_file` twice in immediate succession with the same prefix
   and asserts the two returned filenames differ (regression test for the
   collision — without the fix this is flaky/fails when both calls land in
   the same millisecond, so drive it directly by calling twice back-to-back
   in a tight loop or by asserting inequality across many iterations).
2. Implement: add `static NEXT_MESSAGE_ID: AtomicU64` at module scope in
   `src/notify.rs`, `fetch_add(1, Ordering::Relaxed)` inside
   `write_message_file`, append to the filename.
3. Run `cargo test notify::` to confirm green, plus full `cargo test`.
4. Update `docs/specs/mcp-task-tools.allium:376` — the `path:` formula gains
   the counter segment — and adjust the `@guidance` prose below it
   (`from_task.id>-<unix_epoch_millis>.md` description) to mention the
   counter suffix.
5. Update `docs/specs/task-watchers.allium` — the reference at line ~247-250
   describes the delivery mechanism reusing `write_message_file`; no formula
   is spelled out there, so just confirm the prose ("a timestamped file")
   still reads accurately (may want "timestamped, uniquely-suffixed file").
6. Run `allium:weed` (or manual review) to confirm spec/code alignment.
7. Full verification: `cargo test && ./scripts/check-doc-paths.sh`.
