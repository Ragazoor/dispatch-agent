# WP-3 — Docs Accuracy

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:executing-plans to implement this plan task-by-task.

**Goal:** Fix the doc rot that neither automated checker can see, and add the two facts that cost agents real time this session.

## Context

From the follow-up codebase review at `4bf19b04` (`docs/plans/2026-08-16-4220-codebase-review-followup.md`, smell E and section 8).

Both doc checkers currently pass — `check-doc-paths.sh` confirms every cited path exists, `check-doc-symbols.sh` confirms every cited identifier resolves. Neither validates that a **prose description matches what the file actually does**. That is the gap this package closes, and it is worth understanding before starting: the failures here are invisible to the gate by construction, so they need human judgement, not a script.

## Findings

### 💡 The module map sends readers into a test file

**Issue:** `docs/module-map.md` lists `src/setup/{config,plugins,hooks}.rs` as "MCP config merging, plugin installation, **git hook installation**". `src/setup/hooks.rs` is **1,026 lines of pure `#[cfg(test)] mod tests`** — its own module doc comment says: *"Tests for the embedded hook scripts. Hook installation itself is part of `install_plugin_in`."* An agent asked to change hook installation opens a test file and finds no installation code.

**Fix:** Split the row so `hooks.rs` is described as the hook-script test suite, and point hook *installation* at `install_plugin_in` in `src/setup/plugins.rs`. Then consider whether the filename itself should change — `hooks.rs` misleads independently of the map, and a name like `hooks_tests.rs` would make the map redundant rather than load-bearing. **Renaming is a judgement call: raise it, don't assume it.** If you rename, `src/setup/mod.rs`'s `mod` declaration and any `super::` paths inside move with it.

### 💡 The `src/runtime/{…}` module-map row is incomplete

**Issue:** Line 20 lists `src/runtime/{editor,epics,learnings,pr,settings,split,todos}.rs`. The directory also contains `budget.rs` and `repo_sync.rs`, both real, and `agents.rs`, which is vestigial.

**Fix:** Add `budget` and `repo_sync` to the brace list. **WP-2 deletes `agents.rs` and removes its note from this same row** — check whether WP-2 has landed before editing, and if it has, rebase onto it rather than reintroducing the note.

### ✅ The sandbox tmux-socket failure is undocumented — ALREADY FIXED

**Status: done in task #4220's own commit. Nothing to do here; listed so the finding isn't re-discovered.**

CLAUDE.md warned that without `tmux` on `PATH` the `tmux_*` targets *skip* and pass. Under Claude Code's sandbox the failure mode is different and worse: `tmux` **is** on `PATH`, so the skip never triggers; the harness tries to start a server, the sandbox blocks the unix socket, and every test in the target panics with `error connecting to /tmp/tmux-…/… (Operation not permitted)`. `cargo test` then aborts at that target without running the six later ones.

Observed during the review session: `tests/tmux_editor_pane.rs` failed 9/9 sandboxed, passed 9/9 unsandboxed. The retro step of that session added the paragraph to CLAUDE.md's testing section. Also captured as knowledge-base learning #431.

### 💡 `docs/conventions.md` is 70KB and growing

**Issue:** At 70KB it is the largest doc in the repo and the correct destination for everything the previous review moved out of CLAUDE.md — but it has become the file nobody reads end to end, which quietly undoes the point of moving material there.

**Fix:** Add a table of contents at the top, linking each `##` section. That is the low-risk option and probably sufficient. A split along the seams the file already has (patch/DB conventions · TUI conventions · testing conventions) is the larger option — **propose it, don't execute it unilaterally**, since it moves every inbound pointer from CLAUDE.md and `docs/*.md`, all of which are under `check-doc-paths.sh`.

### 💡 Property testing is endorsed but essentially unused

**Issue:** `proptest!` appears in 7 files across 46k production LOC, despite the conventions endorsing property tests.

**Fix:** Either name where they are expected — `src/models` predicates, `src/tui/text_caret.rs` mechanics, and `fair_truncate_segments` are the natural fits — or drop the endorsement. An endorsement nobody acts on trains agents to skim the conventions.

## Changes

| File | Change |
|------|--------|
| `docs/module-map.md` | Split the `src/setup/{…}` row so `hooks.rs` reads as a test suite; point hook installation at `install_plugin_in` |
| `docs/module-map.md` | Add `budget` and `repo_sync` to the `src/runtime/{…}` brace list |
| ~~`CLAUDE.md`~~ | ~~Add the sandbox tmux-socket failure~~ — already landed in #4220 |
| `docs/conventions.md` | Add a table of contents; note the property-test expectation (or remove the endorsement) |
| `src/setup/hooks.rs` | *Optional, confirm first:* rename to `hooks_tests.rs` + update `src/setup/mod.rs` |

## Implementation notes

- **Prefer `path::symbol` over `file:NN` in anything you write.** The previous review found 7 of 8 line-number citations had rotted; they are all gone now and CLAUDE.md has zero. Do not reintroduce one — `check-doc-paths.sh` only bounds-checks a line number, while `check-doc-symbols.sh` actually resolves a symbol.
- CLAUDE.md is loaded into **every** dispatched agent's context. The sandbox note should be one or two sentences, not a paragraph — it competes for tokens with everything else in that file.
- Both checkers must pass after every edit, and the pre-push hook runs them plus their self-tests.

## Verification

- [ ] `./scripts/check-doc-paths.sh` passes
- [ ] `bash ./scripts/test-check-doc-paths.sh` passes
- [ ] `./scripts/check-doc-symbols.sh` passes
- [ ] `bash ./scripts/test-check-doc-symbols.sh` passes
- [ ] `cargo test` green (only relevant if the optional rename happens)
- [ ] Read the corrected `src/setup/{…}` row cold and confirm it answers "where does hook installation live?" correctly
- [ ] Every TOC anchor in `docs/conventions.md` resolves to a real heading
