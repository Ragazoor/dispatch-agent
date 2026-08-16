# 3855 — Fix the flaky statusline concurrency test

## Problem

`cli::statusline::tests::concurrent_writers_never_publish_foreign_or_torn_bytes`
(`src/cli/statusline.rs`) fails under machine load with
`reader never observed a published snapshot` (~3 in 60 runs with the CPU
saturated; also seen twice in full-suite runs at `--test-threads=4`).

The reader loop is bounded by iteration count only. A read that finds no file
yet is `continue`d. Under load the main thread can exhaust all `ROUNDS * 4`
attempts before either writer thread is scheduled past the barrier and completes
its first atomic rename, leaving `observed == 0` even though the production code
behaved correctly. The test's own comment claims it cannot flake, which misleads
the next reader.

No production behaviour is wrong; this is a test-synchronisation defect. No
Allium spec change — the guarantee under test
(`StatusLineDecorator @guarantee PublishedSnapshotIsAlwaysWholeAndFromOneWriter`)
is unchanged.

## Fix

Replace the iteration budget with a deterministic writer-progress signal. Sleeping
is ruled out by `./scripts/check-no-test-sleep.sh`.

1. Add an `std::sync::mpsc` channel. Each writer sends `()` once, immediately
   after its **first** successful `record_snapshot`, then continues its remaining
   rounds.
2. The reader receives one signal per writer before it starts counting. `recv()`
   erroring means a writer thread died before publishing — surface that as an
   explicit failure rather than a silent zero count.
3. Because the snapshot path is published by atomic rename and never removed,
   every read after the last signal must succeed. So tighten the final assertion
   from `observed > 0` to `observed == ROUNDS * 4`: with the synchronisation
   removed the tightened assertion fails immediately and deterministically, which
   is what makes the fix testable rather than merely plausible.
4. Rewrite the "nothing to flake on" comment to state the actual invariant: the
   reader is gated on writer progress, not on an iteration budget.

The torn/blended assertions inside the loop — the real subject of the test — are
untouched.

## Steps (TDD)

1. Tighten the assertion to `observed == ROUNDS * 4` first and confirm it fails
   (writers have not necessarily published before the reader starts).
2. Add the channel + gate; confirm the tightened assertion passes.
3. Re-run the single test repeatedly under CPU saturation to confirm stability.
4. `cargo fmt --check && cargo test && ./scripts/check-doc-paths.sh`.
