# 3829 — Glob the check-doc-paths.sh scan list

## Problem

`scripts/check-doc-paths.sh` hardcodes its default scan list as seven named files
(`CLAUDE.md` plus six `docs/*.md`). The list happens to be complete today, so
there is no coverage gap right now — but the next doc added under `docs/` has its
`src/…`/`docs/…` paths and `file:NN` citations silently unvalidated until someone
remembers to edit the script. Same failure mode as #3807: a guard that quietly
stops covering new surface.

`docs/specs/*.allium` is also not scanned at all today, despite the specs citing
plenty of `src/…` paths.

## Findings from investigation

- `docs/*.md` currently expands to exactly the six files already listed
  (`architecture`, `conventions`, `how-to`, `mcp`, `module-map`, `reference`), so
  globbing `docs/*.md` is behaviour-preserving today.
- `bash scripts/check-doc-paths.sh docs/specs/*.allium` reports
  `all references resolve` — **zero** existing findings. Globbing the specs in is
  free; no pre-existing staleness has to be resolved first.
- `scripts/check-doc-symbols.sh` (the prior art the task description points at)
  **is** on `main` — #3807 landed as `935cc44d`, which this branch was initially
  rebased past because the first `git rebase origin/main` targeted a stale
  `origin/main` that was 13 commits behind the local `main`. Its default list is
  `CLAUDE.md` + `docs/*.md` + `docs/specs/*.allium` under `nullglob` (plus
  `src/**/*.rs` for doc comments, which is specific to symbol checking), and it
  documents the same dated-artifact exclusion. This plan's glob matches it.
- No Allium spec covers `scripts/`, so no spec change is needed.

## Plan

### 1. Test first — `scripts/test-check-doc-paths.sh`

Replace the `grep -q 'docs/reference.md' "$CHECKER"` assertion (line 82), which
greps the script's source for a literal and cannot survive a glob, with
behavioural assertions that exercise the **default (no-argument)** invocation
against a purpose-built fixture repo:

- New fixture repo (separate temp dir from the existing one, so the existing
  single-file assertions stay untouched) containing:
  - `CLAUDE.md` — clean, so the hardcoded first entry resolves
  - `docs/newdoc.md` — a doc *not* in the old hardcoded list, carrying a broken
    `src/…` reference
  - `docs/specs/newspec.allium` — carrying a broken `src/…` reference
  - `docs/plans/dated.md`, `docs/superpowers/dated.md`, `docs/research/dated.md`
    — each carrying a broken reference
- Assertions:
  1. Default run exits 1 and its output names `docs/newdoc.md` → a new `docs/*.md`
     is covered without editing the script.
  2. Default run output names `docs/specs/newspec.allium` → specs are scanned.
  3. After fixing those two docs, the default run exits 0 → `docs/plans/`,
     `docs/superpowers/`, and `docs/research/` stay excluded — dated working
     artifacts that legitimately describe code as it stood then, per the
     exclusion rationale in `docs/plans/3807-check-doc-symbols.md`.
  4. Default run in a repo with no `docs/` at all does not crash on an unexpanded
     glob (pins `nullglob`).

Run the test → expect failures on 1–3 against the current script.

### 2. Implement — `scripts/check-doc-paths.sh`

```bash
DOCS=(CLAUDE.md)
shopt -s nullglob
DOCS+=(docs/*.md docs/specs/*.allium)
shopt -u nullglob
```

Update the header comment: the default scan is now `CLAUDE.md` plus every
`docs/*.md` and `docs/specs/*.allium`, with the dated-artifact subdirectories
(`docs/plans/`, `docs/superpowers/`, `docs/research/`) deliberately excluded
because the glob is non-recursive.

### 3. Docs

Update the `.githooks/pre-push` description in `CLAUDE.md` so the parenthetical
for `check-doc-paths.sh` says it covers the Allium specs too, not just "the
agent-facing docs".

### 4. Verify

```
cargo fmt --check && cargo test && ./scripts/check-doc-paths.sh
```

plus `bash scripts/test-check-doc-paths.sh` (the self-test the hook runs).
