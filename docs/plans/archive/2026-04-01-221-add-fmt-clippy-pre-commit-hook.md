# Plan: Add fmt/clippy as pre-commit hook

## Context

CI already enforces `cargo fmt --check` and `cargo clippy -- -D warnings` (.github/workflows/ci.yml), but there's no local pre-commit enforcement. Contributors discover formatting/linting issues only after pushing.

Using `cargo-husky`: a dev-dependency that auto-installs git hooks to `.git/hooks/` whenever `cargo test` runs. No manual `git config` step needed — hooks are installed automatically for any contributor who runs tests.

## Changes

- **`Cargo.toml`** — added `cargo-husky = "1"` to `[dev-dependencies]`
- **`.cargo-husky/hooks/pre-commit`** — hook script running fmt + clippy

## Notes

- `cargo test` installs the hook to `.git/hooks/pre-commit` on first run
- CI is unaffected: pre-commit hooks don't run during CI (no `git commit` there)
- Known limitation: cargo-husky v1 does not follow `.git` files used by git worktrees, so hook auto-install is skipped when developing in a worktree. Normal clones (with a `.git` directory) work correctly.
