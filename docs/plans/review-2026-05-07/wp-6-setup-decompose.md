# WP6: Decompose `src/setup.rs`

## Context

`src/setup.rs` (1,281 LOC) bundles three distinct concerns: MCP config merging, hook installation, and plugin (skills/commands) installation. Each concern has its own data, helpers, and tests. Splitting them clarifies ownership and reduces the file's churn surface.

## Findings

- **Severity:** Medium (code-organisation)
- **File:** `src/setup.rs` (1,281 LOC)
- **Issue:** Three concerns share one file; tests for one concern are interleaved with code for another.
- **Suggestion:** Promote to a `setup/` submodule.

## Plan (refactor)

Pure refactor — all 45+ existing tests must keep passing.

1. **Convert** `src/setup.rs` → `src/setup/mod.rs`.
2. **Extract** MCP config merging into `src/setup/config.rs` (Claude Code MCP config read/write/merge logic).
3. **Extract** hook installation into `src/setup/hooks.rs` (writing `.claude/hooks/*` and any wrapper scripts).
4. **Extract** plugin installation into `src/setup/plugins.rs` (skills, slash commands, embedded scripts like `fetch-dependabot.sh`).
5. **Move tests alongside** the code they exercise.
6. **Re-export** the public surface (most likely just `setup_mcp` / `run_setup`) from `src/setup/mod.rs`.

## Files to change

| File | Change |
|---|---|
| `src/setup.rs` | Delete. |
| `src/setup/mod.rs` | New. Public re-exports + entry point. |
| `src/setup/config.rs` | New. MCP config merge logic + tests. |
| `src/setup/hooks.rs` | New. Hook install logic + tests. |
| `src/setup/plugins.rs` | New. Plugin install logic + embedded scripts + tests. |
| `src/main.rs` | No changes if `pub use` is preserved; otherwise adjust `setup::*` import. |

## Verification

```bash
cargo test setup
cargo run -- setup --help    # smoke
cargo fmt --check
cargo clippy --all-targets -- -D warnings
```

Run `cargo run -- setup` against a throwaway HOME to confirm MCP config and hooks land in the right place. No behaviour change should be observable.
