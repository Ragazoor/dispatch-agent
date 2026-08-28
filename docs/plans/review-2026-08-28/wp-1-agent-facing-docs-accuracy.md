# Agent-Facing Docs Accuracy

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:executing-plans to implement this plan task-by-task.

**Goal:** Make `CLAUDE.md` tell agents the truth about the verify command, reclaim 18% of the always-loaded context budget, and add the three pieces of context agents currently have to rediscover.

## Context

This work package addresses findings from the whole-repository codebase review of 2026-08-28 (`docs/plans/2026-08-28-codebase-review.md`, section 7).

`CLAUDE.md` is 159 lines / 20,384 bytes and is loaded into **every** agent's context. It is genuinely good — dense, opinionated, specific about hazards, and it links out rather than duplicating. These are gaps, not a rewrite. Do not restructure the file; make targeted edits.

## Findings

### ⚠️ Verify command drift (`CLAUDE.md`, "Build & Test" section)

**Issue:** `CLAUDE.md` states:

> **This repo's verify command is `cargo test`** — the thing every dispatched agent must run green before declaring work complete.

The command actually stored on the `repo_paths` row, as returned by `get_task`, is:

```
cargo fmt --check && cargo test && ./scripts/check-doc-paths.sh
```

An agent that trusts `CLAUDE.md` runs a **weaker gate than CI**. This is the highest-value fix in this work package.

**Fix:** The file's own "Verify Command" section already explains that the value reaches agents through `get_task`'s "Verify command" line and the `wrap_up` response. Change the "Build & Test" claim to cite that as the source of truth instead of restating a copy that can drift. Something in the shape of: "This repo has a verify command that every dispatched agent must run green before declaring work complete — read it from `get_task`'s *Verify command* line, never from this file."

Do **not** simply paste today's stored value in — that recreates the same drift.

### ⚠️ 18% of the always-loaded file is moot sandbox history (`CLAUDE.md`, "Build & Test" section)

**Issue:** Six paragraphs totalling 3,758 bytes — **18% of the file** — describe sandbox-specific failures. The file itself opens that discussion by saying they no longer apply:

> **Dispatch-spawned sessions no longer run under Claude Code's sandbox** (as of task #4373)

Each paragraph then repeats a variant of "this exception is now moot … still relevant if you enable the sandbox yourself." That is history worth keeping, but not in the file loaded into every agent's context.

The six paragraphs are the ones whose bold lead-in mentions the sandbox, covering: the `SandboxDisabledForDockerAndUnixSockets` preamble, tmux targets failing rather than skipping, `apply-seccomp: unshare(CLONE_NEWUSER)`, `git fetch`/`git push` over SSH remotes, and Gradle resolving from GCP Artifact Registry.

**Fix:** Move them to `docs/reference.md` under a new "Sandbox (historical)" heading, preserving the content verbatim — the specific error strings are the valuable part. Leave a two-line pointer in `CLAUDE.md`: dispatch-spawned sessions do not run under the sandbox (cite `SandboxDisabledForDockerAndUnixSockets` in `docs/specs/dispatch.allium`), and if you enabled it yourself, see the historical section in `docs/reference.md`.

Keep the two paragraphs that are **not** sandbox history and must stay: "The full suite needs `tmux` on `PATH`" and "Don't pipe `cargo test` into `tail`/`head`/`grep`".

### 💡 Missing context agents rediscover every session (`CLAUDE.md`)

**Issue:** Three things an agent needs are absent, so each new agent works them out or guesses.

1. **The `service_api!` macro family.** The "Workhorse macros" note names `patch_struct!` and `mcp_tools!` but not the family in `src/service/api.rs`: `task_service_api!` / `epic_service_api!` / `todo_service_api!` / `learning_service_api!` (spec macros) replaying into `service_api_trait!` / `service_api_delegate!` / `service_api_stub_trait!` / `service_api_stub_bridge!` (emitters). Adding a method to a service seam is a common task, the mechanism is two macro layers deep, and types are `$crate::`-qualified because `macro_rules!` resolves type paths at the call site.

2. **Suite timing.** Measured on this tree: 9.5s for the 4328-test lib target, ~80s wall from cold including compile. `CLAUDE.md` says nothing, so agents guess and some background the run unnecessarily. This already exists as knowledge-base entry #428; it belongs in the always-loaded file.

3. **How to run coverage locally.** `docs/testing.md` gives the CI invocation and is emphatic that the engine is part of the measurement, but neither file gives a copy-pasteable local command.

**Fix:** Add one line each, matching the existing terse style.

- Macro note: extend the existing "Workhorse macros" blockquote to name the `src/service/api.rs` family and say "read the module doc comment before adding a service-seam method".
- Timing: one line in "Build & Test" — the lib target runs in ~10s, a cold full run ~80s including compile; run it in the foreground.
- Coverage: one line — `cargo tarpaulin --engine llvm --out stdout`, with the warning that the default `Auto` engine reads ~1.8 points lower and must not be compared against the CI floor.

### 💡 Undocumented invariants that are currently perfect (`CLAUDE.md`)

**Issue:** Three rules the codebase follows rigorously are written down nowhere, so nothing stops the next agent breaking them.

1. **Read-side layering.** `CLAUDE.md` documents the *mutation* boundary (`state.db` typed as `Arc<dyn db::TaskReadStore>`) because the compiler enforces it. But the read-side layering is enforced only by habit, and measured at zero: `tui → db` = 0, `tui → tmux` = 0, `mcp → tui` = 0, `service → tui` = 0, `models → db` = 0. `models` is a true leaf.

2. **The `#[cfg(test)]` gating rule.** Applied rigorously — `src/dispatch/mock_sequence.rs` (1,993 lines), `MockLearningService`, and the whole `service_api_stub_trait!` family are all gated. There is exactly one deliberate exception: `MockProcessRunner` in `src/process.rs`, which 9 files under `tests/` depend on and which therefore **cannot** be `cfg(test)` (integration-test targets cannot see `cfg(test)` items). Without a note, a well-meaning agent will "fix" it and break those 9 files.

3. **Stale coverage headline.** `docs/testing.md` cites 90.28% as of 2026-08-16; today's run on the same llvm engine reads **91.56%** (15134/16529). Not urgent — the floor is the contract and it correctly has not moved — but a stale headline invites someone to "restore" coverage that never dropped.

**Fix:** Add the layering rule and the `cfg(test)` rule (with its one named exception) as short bullets. Update the figure in `docs/testing.md` to 91.56% (llvm, 2026-08-28) and leave the "the floor is a tripwire, not a target" wording exactly as it is.

## Changes

| File | Change |
|------|--------|
| `CLAUDE.md` | Replace the `cargo test` verify-command claim with a pointer to `get_task`'s "Verify command" line |
| `CLAUDE.md` | Remove the six sandbox-history paragraphs; leave a two-line pointer to `docs/reference.md` |
| `CLAUDE.md` | Keep the `tmux`-on-`PATH` and no-piping-`cargo test` paragraphs in place |
| `CLAUDE.md` | Extend the "Workhorse macros" blockquote with the `src/service/api.rs` spec/emitter family |
| `CLAUDE.md` | Add suite timing (~10s lib, ~80s cold) and the local `cargo tarpaulin --engine llvm --out stdout` command |
| `CLAUDE.md` | Add the read-side layering rule and the `#[cfg(test)]` gating rule with its `MockProcessRunner` exception |
| `docs/reference.md` | New "Sandbox (historical)" section holding the six moved paragraphs verbatim |
| `docs/testing.md` | Update the coverage figure to 91.56% (llvm engine, 2026-08-28); leave the floor wording untouched |

## Verification

- [ ] `cargo test` — all pass (the `contains` assertions in `src/setup/plugins.rs` and `tests/ci_gates.rs` are the ones most likely to notice a doc edit)
- [ ] `./scripts/check-doc-paths.sh` and its self-test pass — every path and `file:NN` citation in the edited files still resolves
- [ ] `./scripts/check-doc-symbols.sh` and its self-test pass — every backticked identifier and `path::symbol` citation added still resolves. `SandboxDisabledForDockerAndUnixSockets` and `TaskReadStore` are cited; confirm both still exist
- [ ] `CLAUDE.md` is meaningfully smaller — target under 17,000 bytes (`wc -c CLAUDE.md`), down from 20,384
- [ ] Re-read the finished `CLAUDE.md` end to end and confirm no paragraph now contradicts another (the sandbox removal touches text in three places)
- [ ] Confirm `AGENTS.md` still resolves — it is a symlink to `CLAUDE.md`, so do not replace the file, edit it in place
