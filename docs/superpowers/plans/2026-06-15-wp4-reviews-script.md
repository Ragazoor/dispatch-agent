# WP4 — Reviews script rewrite (single emission with signals)

> **For agentic workers:** REQUIRED SUB-SKILL: superpowers:executing-plans.
> This is mostly bash + jq. Validate with `cargo run -- verify-feed`.

**Goal:** Rewrite `scripts/fetch-reviews.sh` to emit ONE deduped FeedItem array covering every relevant PR, each carrying the `signals` that matched. Drop the `my`/`team`/`all` scope arg; stop excluding Renovate; fold in `fetch-dependabot.sh`.

**Spec:** `docs/superpowers/specs/2026-06-15-pr-review-feed-routing-design.md` §1, §2.
**Depends on:** WP1 (the `Signal` wire vocabulary: `direct-request | team-request | reviewed | commented | author-bot | author-me`).

**Important wiring facts:**
- Feed scripts run from the data dir (`~/.local/share/dispatch/scripts/`), not the tracked `scripts/` — see learning #123. Validate with `cargo run -- verify-feed '<cmd>'`.
- Renovate bot author login is `kognic-renovate[bot]`; Dependabot is `dependabot[bot]` (learning #122). Bot PRs get `tag: dependabot` (the existing tag intended for feed scripts) and signal `author-bot`.
- Human-review PRs keep `tag: pr-review`.

---

### Task 1: Per-query emission carrying one signal each

**Files:** `scripts/fetch-reviews.sh`

- [ ] **Step 1 — define the queries.** Run these `gh search prs --state=open` queries (reuse the existing `search_reviews` helper; add a `--json author` field for bot detection), each tagging every returned PR with the signal that query represents:
  - `review-requested:@me` → signal `team-request` *(see note below on direct vs team)*
  - `user-review-requested:@me` → signal `direct-request`
  - `reviewed-by:@me` → signal `reviewed`
  - `commenter:@me -author:@me` → signal `commented`
  - Within each result, additionally derive per-PR signals from the author: `author-bot` if author login matches `*[bot]` (renovate/dependabot), `author-me` if author login == the gh user (`gh api user -q .login`).
  > Note: `review-requested:@me` is direct+team; `user-review-requested:@me` is direct-only. Emit `direct-request` from the user-scoped query and `team-request` from the broad query; the signal-merge in Task 2 lets a PR carry both, and `route` (WP2) treats direct as My — correct.
- [ ] **Step 2 — set tag + url_type per PR.** `tag = "dependabot"` when `author-bot` else `"pr-review"`; `url`=PR url, `url_type`="pr". Keep draft exclusion. Build the FeedItem with a `signals` array.
- [ ] **Step 3 — commit:** `feat(scripts): per-query signal tagging in fetch-reviews`

### Task 2: Signal-MERGING dedup by URL (H4)

**Files:** `scripts/fetch-reviews.sh`

- [ ] **Step 1 — implement merge.** Concatenate all query outputs and merge by URL, **unioning the signals** (NOT `unique_by`, which drops objects and loses signals):
```bash
jq -s 'add
  | group_by(.url)
  | map(.[0] + {signals: (map(.signals[]) | unique)})'
```
- [ ] **Step 2 — validate.** Run `cargo run -- verify-feed 'bash scripts/fetch-reviews.sh'` against a logged-in `gh`; confirm valid FeedItem JSON, each item has `signals`, bot PRs carry `author-bot`+`tag:dependabot`.
- [ ] **Step 3 — commit:** `feat(scripts): signal-merging dedup by URL`

### Task 3: Shell test with a stub `gh`

**Files:** Create `scripts/test-fetch-reviews.sh` (plain bash; mirror the style of `scripts/check-no-test-sleep.sh`)

- [ ] **Step 1 — write the test.** Put a fake `gh` first on `PATH` that returns canned JSON per qualifier (read `$@` to decide which fixture). Cover:
  - a PR matched by both `review-requested` and `reviewed-by` → ONE output item carrying both `team-request` and `reviewed` (proves signal-merge).
  - a `dependabot[bot]`/`kognic-renovate[bot]`-authored PR → `author-bot` + `tag:dependabot`, not excluded.
  - a PR authored by me and only matched by `commenter` → excluded (the `-author:@me` guard), or if it appears, no `commented` mis-route.
  - assert output parses as a JSON array (pipe to `jq -e 'type=="array"'`).
- [ ] **Step 2 — run:** `bash scripts/test-fetch-reviews.sh` → exits 0.
- [ ] **Step 3 — wire into pre-push (optional).** If trivial, add it to `.githooks/pre-push` next to `check-no-test-sleep.sh`; otherwise document running it manually in the script header.
- [ ] **Step 4 — commit:** `test(scripts): stub-gh shell test for fetch-reviews`

### Task 4: Remove `fetch-dependabot.sh`, update header docs

**Files:** Delete `scripts/fetch-dependabot.sh`; update the header comment block in `scripts/fetch-reviews.sh` (no scope arg; routing handled by dispatch; bots included).

- [ ] **Step 1 — delete** `scripts/fetch-dependabot.sh` and grep for references (`grep -rn fetch-dependabot`); update/remove any.
- [ ] **Step 2 — rewrite the header** of `fetch-reviews.sh` documenting the single-emission contract and the signal vocabulary.
- [ ] **Step 3 — run** `./scripts/check-doc-paths.sh` (catches broken doc links).
- [ ] **Step 4 — commit:** `chore(scripts): fold dependabot into reviews emission`

---

## Done when
- `verify-feed 'bash scripts/fetch-reviews.sh'` returns valid FeedItems with merged signals.
- `bash scripts/test-fetch-reviews.sh` passes.
- No scope arg; Renovate/Dependabot included with `author-bot`+`tag:dependabot`; `fetch-dependabot.sh` gone.
- `cargo test && ./scripts/check-doc-paths.sh` passes.
