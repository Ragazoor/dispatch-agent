# 4097 — Catch stale symbol citations in docs and Allium specs

## Problem

`scripts/check-doc-symbols.sh` only examines **whole backtick spans that are snake_case
with at least one underscore**. Three rot shapes slip past it, all of them confirmed to
have shipped green:

1. **Bare test-name citations in `.allium` prose.** `DegradedEmptyEmission`'s `@guidance`
   in `docs/specs/feeds.allium` cited
   `exec_trigger_epic_feed_quiet_command_reports_no_stderr` after the test was deleted.
   Allium specs use no backticks, so nothing looked at it. Four tasks, four reviews, all
   green (found by hand during #3989).
2. **Deleted PascalCase symbols.** #4091 removed the `FeedJob` struct; eight `src/` doc
   comments and two `feeds.allium` lines still named `FeedJob::run`. Not snake_case, so
   not a candidate.
3. **Unbackticked `path.rs::symbol` citations.** #4091 specified
   `src/feed/cycle.rs::run_feed_cycle`; the code shipped as `FeedCycle::run`. Twelve
   citations to a function that never existed. Spec-first work makes this systematic: the
   citation is authored before the symbol exists, so it is never checked against anything.

`CLAUDE.md` currently documents shapes 2 and 3 as a *manual* grep obligation. This task is
the mechanical version.

## Why a naive widening does not work

The existing checker's own header records that scanning bare tokens in Allium comments
measured **37 hits for 1 real finding**. That result still holds, and I re-measured it
before designing:

| Capture shape | Distinct unresolved tokens today |
|---|---|
| Bare snake_case, ≥1 underscore, in `docs/specs/*.allium` | **34** (all false positives) |
| Bare snake_case, ≥3 underscores | 1 (`repo_group_epic_id`, a cache-key name in prose) |
| Bare snake_case, ≥4 underscores | **0** |
| Bare PascalCase (`FeedSync`, `EpicStatusRecalculation`, …) | **~60** (all Allium block names) |
| `Type::member` with both halves checked | **1** — and it is a *genuine* rot |
| `path.rs::symbol` resolved *within the cited file* | **1** — the already-annotated `CLAUDE.md` example |

So: bare PascalCase and short bare snake_case are unusable, and the three shapes below are
essentially noise-free. The underscore threshold is a *calibration*, not a guess — 4 is the
lowest value with zero false positives across 12,757 spec lines, and it still catches the
motivating citation (`exec_trigger_epic_feed_quiet_command_reports_no_stderr`, 8 underscores).

## Design

Extend `scripts/check-doc-symbols.sh` rather than adding a sibling: the identifier index is
the expensive part and is shared, and one hook step with one self-test is easier to keep
honest than two.

Scanned surfaces are unchanged (`CLAUDE.md`, `docs/*.md`, `docs/specs/*.allium`,
`src/**/*.rs` doc-comment lines only). `allow-phantom-symbol: <why>` on the offending line
or the line directly above suppresses **every** kind, unchanged.

Three new candidate kinds are added alongside the existing backticked-span kind.

### Kind `pathsym` — `<path>.rs::Seg(::Seg)*`

Captured **whether or not it is backticked**. Verified more strictly than a phantom check:

- the cited path must exist, else `cited file does not exist`;
- **every** `::` segment must occur as a whole word in **that file**, with `//` comments
  stripped.

Per-file resolution is what makes this catch #4091: `run_feed_cycle` occurred nowhere, but
even a symbol that exists *elsewhere* is a wrong citation. Word sets are cached per file
(a few dozen distinct files).

### Kind `typesym` — `[A-Z][A-Za-z0-9]*(::seg)+`

Captured whether or not backticked, and only outside an already-matched `pathsym`. Every
segment must occur in the global identifier index. This is the `FeedJob::run` shape.

### Kind `bare` — snake_case with **≥4 underscores**, unbackticked

Captured outside `pathsym`/`typesym` matches and outside backtick spans (backticked spans
are already handled by the existing kind at its ≥1-underscore threshold). Checked against
the global index. This is the stale-test-name shape.

### Unchanged

The existing backticked-span kind, its `TOKEN_RE`, the index construction (code only,
comments stripped — load-bearing, see #340), and the `docs/plans/` exclusion.

## Existing findings this turns up

Running the widened checker over the repo as it stands produces exactly three hits — a
useful sanity check on the noise estimate:

1. `docs/mcp.md:15` and `docs/mcp.md:23` — `Message::RefreshTasks`. **Genuine rot.** The
   variant does not exist in `src/tui/types.rs`; the real path is
   `Message::Task(TaskMessage::Refresh(..))` (`src/runtime/tasks.rs`). Fix the doc.
2. `docs/how-to.md` — `my_tool_returns_expected_data`, an illustrative placeholder test
   name in a how-to example. **Legitimate false positive.** Annotate with
   `allow-phantom-symbol`.
3. `CLAUDE.md` — `src/feed/cycle.rs::run_feed_cycle`, already annotated by #4091. No action.

## Plan (TDD — assertions first, then the checker)

### Step 1 — extend the fixture repo in `scripts/test-check-doc-symbols.sh`

Add to the temp fixture:
- `src/feed/cycle.rs` containing `struct FeedCycle` with an `impl` method `run`, so
  `FeedCycle::run` resolves and `FeedJob::run` does not;
- keep `src/db/mod.rs::real_function` as the single-segment resolvable citation;
- a long real test name in `tests/harness.rs`
  (e.g. `fn feed_cycle_reports_removed_task_with_worktree`) so a ≥4-underscore token has a
  green counterpart.

### Step 2 — write the failing assertions

`pathsym`:
- green: `src/db/mod.rs::real_function` unbackticked in an `.allium` comment
- green: `src/feed/cycle.rs::FeedCycle::run` (multi-segment)
- green: the same citation backticked
- red: `src/feed/cycle.rs::run_feed_cycle` — the exact #4091 rot
- red: `src/db/mod.rs::ghost_function` — symbol absent from an existing file
- red: `src/feed/cycle.rs::real_function` — **symbol real but in the wrong file**, the case
  a global phantom check cannot see
- red: `src/nowhere.rs::real_function` — cited file does not exist
- red: backticked `src/db/mod.rs::ghost_function` — backticking must not launder it

`typesym`:
- green: `FeedCycle::run` unbackticked in `.allium`
- red: `FeedJob::run` unbackticked in `.allium`
- red: `FeedJob::run` in a Rust `///` doc comment
- green: `FeedJob::run` on a Rust **code** line (not a doc comment — must not be scanned)
- green: `Database::ghost_method` is no longer silently ignored… — **note**: the existing
  suite asserts this passes as "a qualified path is not a candidate token". That assertion
  encodes the old behaviour and must be *changed*, not kept: under `typesym` it should now
  be red. Update the case and its label.

`bare`:
- red: a ≥4-underscore phantom in `docs/specs/scratch.allium`
- red: the same in a markdown doc
- green: the real long test name from the fixture
- green: a 3-underscore token that resolves nowhere (`repo_group_epic_id` shape) — pins the
  threshold calibration, so lowering it silently is a test failure
- green: a ≥4-underscore token inside a backtick span still routes through the existing
  kind (no double-report)

Escape hatch: one marker assertion per new kind (on-line and line-above), reusing the
existing `expect` helper.

Run the suite; confirm it is red for the right reasons before touching the checker.

### Step 3 — implement the checker changes

Rework `extract_spans` into a single awk pass emitting
`lineno<TAB>flag<TAB>kind<TAB>token`, masking each shape out of the line as it is consumed
(pathsym → typesym → backtick spans → bare). Keep the `.rs` doc-comment gate and the
marker-tracking-before-the-gate behaviour intact.

Add the bash-side verification per kind, with a per-file word-set cache for `pathsym` and
distinct diagnostics:
- `references <path>::<sym>, but <path> does not exist`
- `references <path>::<sym>, which does not occur in that file`
- `references <tok>, which occurs nowhere in the code` (existing wording, for the other kinds)

Update the script header comment: the "deliberately NOT scanned" note about bare Allium
tokens now needs the ≥4-underscore carve-out and the measured numbers behind it.

### Step 4 — make the repo green

- `docs/mcp.md`: replace both `Message::RefreshTasks` citations with the real chain.
- `docs/how-to.md`: annotate `my_tool_returns_expected_data`.

### Step 5 — update `CLAUDE.md`

The `path::symbol` paragraph ("only partly verified", naming both holes) is now wrong and
must be rewritten to state what is mechanically checked and what is still not:
- still unchecked: `file:NN` citations remain bounds-only; a bare PascalCase type name with
  no `::` is still unguarded; bare snake_case below 4 underscores is unguarded.
- now checked: `path.rs::symbol` resolves inside the cited file; `Type::method` segments
  exist; long bare snake_case names in specs and docs exist.

Also refresh the pre-push-hook step description of `check-doc-symbols.sh`.

### Step 6 — verify

- `bash scripts/test-check-doc-symbols.sh`
- `bash scripts/check-doc-symbols.sh`
- `bash scripts/check-doc-paths.sh` and `bash scripts/test-check-doc-paths.sh` (the
  `docs/mcp.md` edit touches a doc it validates)
- `cargo test`
- `git log --oneline HEAD..main`; merge `main` and re-run if non-empty.

## Spec impact

None expected: `docs/specs/*.allium` describe the application's domain and interaction
behaviour, not the repo's pre-push tooling, and no spec block covers `scripts/`. Confirm
with a grep for `check-doc` across `docs/specs/` during Step 1; if a spec does describe the
gate, tend it before writing code.

## Risks

- **Awk masking order.** Getting `pathsym` masked before `typesym` wrong would double-report
  `src/feed/cycle.rs::FeedCycle::run` as both kinds. Covered by the multi-segment green case.
- **Fenced code blocks in markdown are scanned.** `docs/mcp.md:15` sits inside a ``` fence
  and is a real citation, so this is wanted — but a future doc pasting foreign code into a
  fence would need the marker. Acceptable; noted in the script header.
- **Threshold drift.** The ≥4 calibration holds for today's corpus. The 3-underscore green
  assertion makes a silent lowering fail the self-test.
