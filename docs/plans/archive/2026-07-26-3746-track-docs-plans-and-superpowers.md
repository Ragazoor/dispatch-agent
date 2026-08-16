# Plan: track docs/plans/ and docs/superpowers/ in git

## Goal

`docs/plans/**` and `docs/superpowers/**` are currently listed in `.gitignore`,
even though 46 files under those paths are already tracked (added before, or
despite, the ignore rule — `.gitignore` doesn't untrack committed files). Every
doc/skill that describes this policy calls these "working artifacts, never
committed." The user wants that reversed: all `docs/` content should be
tracked, and every doc that states the old policy updated to match.

Discovery (via grep sweep, semantic search returned no relevant hits for this
literal-text policy question):

- `.gitignore:3-4` — the two ignore lines to remove.
- `CLAUDE.md:168` — doc-index entry: "working artifacts, never committed".
- `.claude/skills/allium-weed-loop/prompt.md:52` — "Do NOT commit files under `docs/plans/`."
- `plugin/skills/allium-loop/prompt.md:93` — "Never commit files under `docs/plans/`."
- `docs/research/329-self-learning-frameworks.md:110` — "working artifacts, never committed" (stale factual claim once policy changes).

Not in scope (checked, no policy statement present — just path examples):
`docs/specs/dispatch.allium`, `docs/specs/feeds.allium`, `docs/specs/tasks.allium`,
`plugin/skills/allium-loop/SKILL.md`, `plugin/skills/decompose-review/SKILL.md`,
various `src/**` test fixtures/doc-comments using `docs/plans/...` as a sample path.

Out of scope: the user's global `~/.claude/rules/git.md` ("Never git add or
commit files under docs/plans/") — that's a personal cross-repo preference
file outside this worktree, not a repo doc. Flagging it to the user instead of
editing it.

## Steps

1. Remove the `docs/superpowers/**` and `docs/plans/**` lines from `.gitignore`.
2. Update `CLAUDE.md:168` to drop the "never committed" characterization.
3. Update `.claude/skills/allium-weed-loop/prompt.md:52` to drop the "Do NOT commit" instruction.
4. Update `plugin/skills/allium-loop/prompt.md:93` to drop the "Never commit" guardrail.
5. Update `docs/research/329-self-learning-frameworks.md:110` to drop the stale claim.
6. Verify: `git check-ignore` reports nothing under `docs/plans/`/`docs/superpowers/` anymore; `./scripts/check-doc-paths.sh` and `cargo test` still pass.

No production code changes, so no TDD red/green cycle applies — verification
is the doc-path checker plus a manual `git check-ignore` sanity check.
