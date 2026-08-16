# Docs Accuracy & CLAUDE.md Slimming

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:executing-plans to implement this plan task-by-task.

**Goal:** Fix the rotted citations and content errors in `CLAUDE.md`, add the facts every agent currently discovers the hard way, and move ~40% of it behind pointers.

## Context

This work package addresses findings from the 2026-08-16 codebase review
(`docs/plans/2026-08-16-codebase-review.md`, commit `c05f512c`).

`CLAUDE.md` is loaded into **every** dispatched agent's context — 246 lines,
~26KB, ~6.5k tokens per dispatch. Errors in it are multiplied by every agent
run, and its size is a per-run tax.

## Findings

### ⚠️ 7 of 8 `file:NN` citations have rotted

**Issue:** Verified individually against source during the review:

| CLAUDE.md claims | Actual |
|---|---|
| `TaskTag` at `src/models/tasks.rs:438` | **558** |
| `DispatchMode::for_task()` at `src/models/tasks.rs:420` | **540** |
| `TaskTag::is_review()` at `src/models/tasks.rs:465` | **595** |
| `build_prompt` at `src/dispatch/prompts.rs:264` | **336** |
| `src/dispatch/agents.rs:50` (PR-head branch) | logic is at **253-280**; line 50 is an unrelated doc comment |
| `patch_struct!` at `src/db/mod.rs:30` | **38** |
| `mcp_tools!` at `src/mcp/handlers/dispatch.rs:39` | 38 (off by one, harmless) |
| `CallerIdentity::from_headers` at `src/mcp/identity.rs:21` | ✅ correct |

This is precisely the failure mode `CLAUDE.md` warns about on its own line 36.
`scripts/check-doc-paths.sh` only *bounds-checks* line numbers (confirms the line
exists, never that it says what the doc claims), so the hook can never catch this.

**Fix:** Rewrite all eight as `path::symbol` (e.g. `src/models/tasks.rs::TaskTag`,
`src/dispatch/prompts.rs::build_prompt`). That moves them under
`scripts/check-doc-symbols.sh`, which resolves symbols against the real file — so
the next drift fails the pre-push hook instead of silently misleading an agent.
Sweep `docs/*.md` for the same pattern while you are here.

### ⚠️ Content errors

**Issue (a):** `CLAUDE.md:240` describes `src/cli/` as "(`agent_tree`,
`caller_headers`)". The directory also contains **`statusline.rs`**, a real
subcommand documented in `docs/reference.md:127` and `docs/module-map.md:16` —
`CLAUDE.md` is the only one of the three that is wrong. *(Verified by `ls src/cli`.)*

**Issue (b):** The test-target list (`CLAUDE.md:41-55`) is presented as complete
but omits `tests/active_health.rs`, `caller_identity.rs`,
`dispatch_status_lifecycle.rs`, `feed_sync.rs`, `githooks.rs`,
`managed_feeds.rs`, `task_watchers.rs`, `tmux_send_message_pane_state.rs`,
`trajectory.rs`, `verify_command.rs`.

**Fix:** (a) add `statusline`. (b) either label it "selected targets" or drop the
enumeration — an incomplete list presented as complete is worse than a pointer.

### ⚠️ Missing context every agent pays for

**Issue:** Facts the codebase requires that no doc states:

- **`cargo fmt` in the pre-push hook has no `--check`** — pushing rewrites your working tree. The step is listed; the consequence is not.
- **This repo's own verify command is never stated.** A whole section explains the verify-command *mechanism* without saying that `dispatch`'s is `cargo test`.
- **`plugin/skills/` is unexplained** — it appears once, inside a table cell, with no statement that it is the source of truth for agent-facing skill copy or how it relates to `src/setup/plugins.rs::skill_body`.
- **`docs/plans/` commit policy is unstated** — the doc checker excludes it, some global rules forbid committing it, and repo history commits it. Per `CLAUDE.md`'s own "ambiguity is a stop condition" rule, this must be written down. **This repo commits it** (policy reversed 2026-07-26).
- **`cargo insta` must be installed** for the documented `cargo insta review` line.
- **tmux must already be running** before `cargo run -- tui` (currently stated only in a code comment, `docs/reference.md:116`).
- **The sandbox denies `unshare`**, so plain `ls`/`wc` can fail with `apply-seccomp: unshare(CLONE_NEWUSER): Invalid argument` under parallel load — hit twice during the review.

**Fix:** Add each as a single line in the appropriate section.

### 💡 Size and signal: move ~40% behind pointers

**Issue:** Roughly 100 of 246 lines are lookup material paid for on every dispatch.

**Fix:** In priority order:

1. **"Running tests" table + snapshot section + "Where new tests go"** (lines 38-113, ~76 lines, ~2.5k tokens) → new `docs/testing.md`. Only two sentences earn every-context placement: *"the full suite needs tmux on PATH"* and *"don't pipe `cargo test` into `tail`"*.
2. **"Tag System"** (195-204) → `docs/conventions.md`.
3. **The 15-item Allium spec list** (163-177) → "specs live in `docs/specs/`; each filename names its domain." The filenames are self-describing.
4. **"Running & Debugging Locally"** (115-126) → `docs/reference.md`, which already has a Configuration table.
5. **"External Dependencies"** (128-141) — keep the bubblewrap/socat sentence (a real silent-degradation trap), move the per-binary breakdown.

**Keep as-is:** the `main`-moves-under-you paragraph, First-time setup, Working
With the User, TDD, Mutation boundary, the doc index. Those change agent
*behaviour* rather than answer lookups. **Target ≈120 lines.**

### 💡 docs/ health

**Issue:**
- `docs/module-map.md:31` — the `src/tui/ui/shared.rs` row omits `staleness_color` and `feed_role_label` (both cited by `CLAUDE.md` as living there), and **`src/tui/ui/budget.rs` has no row at all**.
- `docs/reference.md` "CLI Usage" lists 7 invocations; `src/main.rs` defines 19 subcommands. Missing: `repo set-verify`/`clear-verify`/`list` (`CLAUDE.md:145` cites `repo set-verify`, so a reader following the pointer won't find it), the five `hook-*` commands, `agent-tree`, `caller-headers`, `pr-gate`, `uninstall`, `prune-repo-paths`, `toggle-agent-tree-pane`.
- `docs/architecture.md` is 51 lines, three of which are 300-word paragraphs (bullets 6, 27, 28) — promote each to an `##` section. Structure, not cuts.
- `docs/architecture.md:44` cites `Command::QuickDispatch { draft … }` in `src/tui/mod.rs`; it now lives at `src/tui/commands/task.rs::QuickDispatch` and is emitted from `src/tui/input/normal.rs:646`.
- `docs/how-to.md:122` documents only the guarded migration form, while `src/db/migrations.rs` has 16 sites using bare `let _ = conn.execute_batch(...)`. State explicitly that the guard form is **mandatory for new migrations** and that the `let _` sites are frozen history not to be copied.
- `docs/plans/` is 104 top-level entries / ~133 files / 1.3MB in two incompatible naming schemes; `docs/superpowers/` adds ~57 files / 1.3MB. Neither is checked by the doc hooks.

**Fix:** Correct each; for `docs/plans/`, archive entries below the current
milestone into `docs/plans/archive/` and standardise on the date prefix (it makes
staleness visible without a git query).

## Changes

| File | Change |
|------|--------|
| `CLAUDE.md` | Rewrite 8 `file:NN` → `path::symbol`; add `statusline` to `src/cli/`; fix the test-target list; add the 7 missing facts; move sections 1-5 out; target ≈120 lines |
| `docs/testing.md` | **New** — receives the test-command table, snapshot workflow, and "Where new tests go" table |
| `docs/conventions.md` | Receives the Tag System section |
| `docs/reference.md` | Receives "Running & Debugging Locally"; complete the CLI Usage list to all 19 subcommands |
| `docs/module-map.md` | Add `staleness_color`/`feed_role_label` to the `shared.rs` row; add a `budget.rs` row |
| `docs/architecture.md` | Promote the three long bullets to `##` sections; fix the `Command::QuickDispatch` citation |
| `docs/how-to.md` | State the guarded migration form is mandatory for new migrations |
| `docs/plans/` | Archive pre-milestone entries into `archive/`; standardise on the date prefix |

## Verification

- [ ] `./scripts/check-doc-paths.sh` and its self-test pass
- [ ] `./scripts/check-doc-symbols.sh` and its self-test pass — this is the check that must now be catching the citations
- [ ] Spot-check each rewritten `path::symbol` resolves: `grep -n "TaskTag\|for_task\|is_review" src/models/tasks.rs` etc.
- [ ] Deliberately break one `path::symbol` citation and confirm `check-doc-symbols.sh` **fails** — the whole point is that the hook now catches drift
- [ ] `wc -l CLAUDE.md` ≈ 120
- [ ] Every section moved out of `CLAUDE.md` is reachable from its doc index — no orphaned content
- [ ] `cargo test` — full suite green (skill-copy tests in `src/setup/plugins.rs` assert doc/skill text and may be affected)
- [ ] Pre-push hook passes end-to-end
