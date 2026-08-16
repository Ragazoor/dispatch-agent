# 3801 — Add a real-tmux row to CLAUDE.md's "Where new tests go" table

Follow-up to #3782. Docs-only.

## Why

The "Where new tests go" table is the routing decision an agent makes *before*
writing a test. It has no row for tmux semantics, so the nearest-looking option
is a `MockProcessRunner` test in `src/tmux.rs` — precisely the trap that produced
#3781 (a mock test pinned a broken `send-keys` string and stayed green) and then
#3782 (three more bugs of the same family: `pane_exists` blind,
`pane_id_for_window` blind, swap by pane index).

The real-tmux harness now exists specifically so that class is catchable. A
missing table row is how the next agent fails to find it.

## Changes

1. **`CLAUDE.md`** — one new row in the "Where new tests go" table pointing at
   `tests/tmux_lifecycle.rs` (topology/cwd), `tests/tmux_split_hook.rs`
   (keystroke routing), and the shared rig in `tests/tmux_harness/mod.rs`, plus
   a one-line note stating the boundary and linking to the longer explanation.

2. **`docs/conventions.md`** — a new "`MockProcessRunner` vs a real tmux server"
   section, placed next to the existing "No `tokio::time::sleep` in tests"
   section (which already references `tests/tmux_harness/mod.rs`'s
   `allow-test-sleep` escape hatch). It states:

   - `MockProcessRunner` proves *which command we sent* — right for argv shape,
     and the existing `src/tmux.rs` tests should stay.
   - A real tmux server proves *what tmux did with it* — the only thing that
     catches wrong-pane, wrong-cwd, or wrong-pane-count bugs, because tmux
     resolves loose targets by falling back rather than failing.
   - Which file to reach for, and the CI hard-fail-vs-local-skip guard
     (`tmux_available_or_skip`).

Not in scope: `docs/module-map.md`, which already gained its
`src/dispatch/split_panes.rs` entry in #3782.

## Verification

`cargo fmt --check && cargo test && ./scripts/check-doc-paths.sh` — the last of
these is the one that actually exercises this change: it validates every
`src/…`/`tests/…` path cited in `CLAUDE.md` and the linked topic docs, so a
mistyped harness path fails the build rather than misrouting a future agent.
