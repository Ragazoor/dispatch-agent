# WP-1: Code Hygiene

## Context

A code quality review on 2026-05-05 identified five quick-win issues across editor, feed, tui tests, and documentation. None affect correctness today, but they suppress real warnings, make intent unclear, and leave gaps in the CLAUDE.md reference.

## Findings

### QW1 — Blanket `#![allow(unused_imports)]` in TUI test modules
- **Severity**: medium
- **Files**: `src/tui/tests/*.rs` (10+ files, each starting with `#![allow(unused_imports)]`)
- **Issue**: Blanket allows suppress real warnings. Unused imports should be removed, not silenced.
- **Fix**: Remove the blanket allow from each file; delete any imports that `rustc --warn unused_imports` flags.

### QW2 — `Mutex::lock().unwrap()` without context in feed infrastructure
- **Severity**: medium
- **Files**: `src/feed.rs`
- **Issue**: Bare `.unwrap()` on a Mutex lock panics with no context on a poisoned lock.
- **Fix**: Replace with `.expect("feed lock poisoned")` so a panic message is meaningful.

### QW3 — Magic feed-interval numbers in editor.rs
- **Severity**: low
- **Files**: `src/editor.rs:681,707,757,778`
- **Issue**: Literal integers 300, 120, 60 (seconds) are repeated without names.
- **Fix**: Introduce named constants (e.g. `FEED_INTERVAL_FAST_SECS`, `FEED_INTERVAL_SLOW_SECS`) and replace the literals.

### QW4 — `Option<Option<String>>` in editor.rs
- **Severity**: medium
- **Files**: `src/editor.rs:219`
- **Issue**: `pub detail: Option<Option<String>>` has unclear null semantics. The outer None vs inner None distinction isn't obvious to readers.
- **Fix**: Replace with `Option<FieldUpdate>` (from `src/service.rs`) or a small named enum if the "set" / "clear" / "absent" distinction is truly needed. If "clear" isn't needed, plain `Option<String>` suffices.

### QW5 — `dispatch/finish.rs` missing from CLAUDE.md module map
- **Severity**: low
- **Files**: `CLAUDE.md`
- **Issue**: The module map section lists most source files but omits `src/dispatch/finish.rs`, which contains the rebase + cleanup mechanic referenced in the conventions section.
- **Fix**: Add an entry for `src/dispatch/finish.rs` in the Module Map table.

## Changes Table

| File | What to change |
|---|---|
| `src/tui/tests/*.rs` (all files with blanket allow) | Remove `#![allow(unused_imports)]`; delete flagged imports |
| `src/feed.rs` | Replace `.lock().unwrap()` with `.lock().expect("feed lock poisoned")` |
| `src/editor.rs` | Extract 300/120/60 into named constants; replace field type |
| `CLAUDE.md` | Add `src/dispatch/finish.rs` row to Module Map table |

## Verification

```bash
cargo build
cargo clippy --all-targets -- -D warnings
cargo test
cargo fmt --check
```

No snapshot updates expected — these are internal changes only.
