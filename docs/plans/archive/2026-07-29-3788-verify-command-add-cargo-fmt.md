# 3788 — Add `cargo fmt` to the dispatch verify command

## Goal

Dispatched agents in this repo should catch formatting drift before declaring work
complete, instead of discovering it when the pre-push hook rewrites their files.

## What the verify command is

A per-repo, single-line shell command stored on the `repo_paths` row for the task's
`repo_path` (see the "Verify Command" section of `CLAUDE.md`). It is **data, not code** —
there is nothing to change in `src/`, and no Allium spec covers the *value* of the
command (only the mechanism, in `docs/specs/dispatch.allium`). Set via the
`set_verify_command` MCP tool or `cargo run -- repo set-verify <path> <command>`.

## Change

For `/home/ragge/Code/work/experiments/dispatch`:

| | |
|---|---|
| Before | `cargo test && ./scripts/check-doc-paths.sh` |
| After | `cargo fmt --check && cargo test && ./scripts/check-doc-paths.sh` |

`--check` rather than a bare `cargo fmt`: verification should report, not mutate. A bare
`cargo fmt` reformats pre-existing drift across files the task never touched, which the
knowledge base flags repeatedly (#131, #265, #322) as a source of noisy diffs and failed
`wrap_up` rebases. With `--check` the agent sees the failure and fixes only its own files.

Placed first in the chain so the cheapest check fails fastest.

## Steps

1. `cargo fmt --check` on a clean checkout of `main` — one pre-existing drift hunk in
   `src/service/epics.rs` (a `let child = db.create_epic(...)` chain left unformatted by
   commit `8586b4c5`). Fix it, otherwise the new verify command fails on arrival for
   every future task in this repo.
2. `cargo run -- repo set-verify /home/ragge/Code/work/experiments/dispatch 'cargo fmt --check && cargo test && ./scripts/check-doc-paths.sh'`
3. Confirm with `cargo run -- repo list`.
4. Run the new command end to end.

## Tests

No test-first step applies: the only code change is a formatting fix to an existing test
body, and the behaviour change lives in a DB row rather than in this crate. The new verify
command running green **is** the verification.

## Out of scope

- `cargo clippy --all-targets -- -D warnings`, which the pre-push hook also runs. Adding it
  would roughly double verify runtime; not requested here.
- The stale `/home/ragge/Code/experiments/dispatch` repo-path row (no verify command, wrong
  path). Left alone — `dispatch prune-repo-paths` is the tool for that.
