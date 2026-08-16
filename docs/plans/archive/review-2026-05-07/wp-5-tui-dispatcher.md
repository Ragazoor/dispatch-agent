# WP5: Extract `App::update()` dispatcher

## Context

`src/tui/mod.rs` (1,268 LOC) holds the central `App::update(Message) -> Vec<Command>` match — roughly 95 arms, each delegating to a handler in `src/tui/update/` (3,002 LOC). The match itself is simple but adding a new `Message` variant currently requires editing three files (`types.rs`, `mod.rs`, and the handler module) when it could be two.

## Findings

- **Severity:** Medium
- **Files:** `src/tui/mod.rs`, `src/tui/update/*`
- **Issue:** Routing table and state container live in the same file. New `Message` variants take an unnecessary file edit.
- **Suggestion:** Move the match into `src/tui/dispatcher.rs`, leaving `App` and lifecycle methods in `mod.rs`.

## Plan

1. **Create** `src/tui/dispatcher.rs` containing a single function:
   ```rust
   pub(in crate::tui) fn dispatch(app: &mut App, msg: Message) -> Vec<Command> { ... }
   ```
   Move the entire match body verbatim.
2. **Reduce** `App::update()` in `src/tui/mod.rs` to a one-liner: `dispatcher::dispatch(self, msg)`.
3. **Visibility** — handlers in `src/tui/update/*` currently have `pub(in crate::tui)` visibility, which keeps working. Verify by compilation.
4. **Tests** — `src/tui/tests/*` call `app.update(...)`. No test changes required.
5. **Run** `cargo test tui` and `cargo clippy --all-targets -- -D warnings`. All snapshots must remain unchanged.

## Files to change

| File | Change |
|---|---|
| `src/tui/dispatcher.rs` | New. Holds the routing match. |
| `src/tui/mod.rs` | Shrink `App::update()` to delegate. Add `mod dispatcher;`. |
| `src/tui/update/*.rs` | No changes (visibility already permits cross-module calls within `crate::tui`). |

## Verification

```bash
cargo test tui::
cargo test --test lifecycle
cargo fmt --check
cargo clippy --all-targets -- -D warnings
```

Snapshot tests (`cargo test tui::tests::snapshots`) must pass with **zero** `.snap.new` files generated — this is a pure refactor and rendering must be byte-identical.
