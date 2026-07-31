# Retro Skill Pickiness Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** Rework the `/retro` skill so it fixes small agent-context drift in place and files a follow-up task only when the next dispatched agent would genuinely do better.

**Architecture:** Three coordinated prose changes, each locked by targeted `contains` assertions on the embedded skill body. `plugin/skills/retro/SKILL.md` gets a new admission test (next-agent benefit, traced to a concrete moment this session lost time on), a bounded in-session edit licence, and a two-tag filing bar. `plugin/skills/wrap-up/SKILL.md` moves the `/retro` invocation from the closing sequence (Step 5D) to pre-commit (Step 2.6) so retro's edits are committed with the session's work. Three `@guidance` blocks in `docs/specs/` that assert the old ordering are corrected.

**Tech Stack:** Rust 2021, `include_dir!` (skills embedded into the binary at compile time), Markdown skill prose, Allium specs.

**Design doc:** `docs/superpowers/specs/2026-07-31-retro-skill-pickiness-design.md`

## Global Constraints

- **TDD, always.** Every task writes its failing assertion first, watches it fail, then edits the prose. This is the repo's rule in `CLAUDE.md` and it applies to prose changes too — the test is what makes a deleted instruction read as a regression.
- **Skill copy is tested in `mod tests` in `src/setup/plugins.rs`**, via the existing `skill_body(name)` helper (`src/setup/plugins.rs:599`), per `CLAUDE.md`'s "Where new tests go" table. Not in `tests/`, not as a snapshot.
- **Targeted `contains` checks, never snapshots.** Deleting one instruction must fail a named test rather than show up as a snapshot diff to rubber-stamp.
- **Scope every assertion to a heading section.** Retro repeats words like "task", "spec", and "fix" across its steps, so a whole-document `contains` can still pass after the instruction under test is gone. Task 1 adds the `retro_section` helper for this; it mirrors `failed_close_guidance()` at `src/setup/plugins.rs:585`.
- **`mod tests` already carries** `#![allow(clippy::unwrap_used, clippy::expect_used)]` (`src/setup/plugins.rs:282`) — no need to add it.
- **No new Allium spec.** Retro has none today and gains none. Only the three stale `@guidance` lines in Task 5 change.
- **Assertions compare lowercased text** wherever the helper lowercases (it does). Write the expected substrings in lowercase.
- **Verification command** (run before declaring done): `cargo fmt --check && cargo test && ./scripts/check-doc-paths.sh`

## File Structure

| File | Responsibility | Change |
|---|---|---|
| `plugin/skills/retro/SKILL.md` | the retro skill's agent-facing prose | rewritten Steps 1–4 + frontmatter description |
| `plugin/skills/wrap-up/SKILL.md` | the wrap-up skill's agent-facing prose | `/retro` moves to Step 2.6; closing sequence loses Step D and renumbers E→D |
| `src/setup/plugins.rs` | plugin install + skill-copy tests | `retro_section` helper + 7 new tests; 1 existing test's comment updated |
| `docs/specs/mcp-task-tools.allium` | wrap_up/exit_session domain rules | 1 `@guidance` line corrected |
| `docs/specs/pr-workflow.allium` | PR + finish-path domain rules | 3 `@guidance` lines corrected |

---

### Task 1: Reframe the admission test from doc accuracy to next-agent benefit

Retro's current Step 2 asks whether `CLAUDE.md` or a spec is "stale or wrong" — a correctness question every trivial nit passes. This task replaces Steps 1 and 2 with a single evidence-based question and adds the section-scoping test helper the later tasks reuse.

**Files:**
- Modify: `src/setup/plugins.rs` (add `retro_section` helper + 2 tests, after `skill_body` at `src/setup/plugins.rs:606`)
- Modify: `plugin/skills/retro/SKILL.md:17-45` (Steps 1 and 2)

**Interfaces:**
- Consumes: `skill_body(skill: &str) -> &'static str` (`src/setup/plugins.rs:599`)
- Produces: `fn retro_section(anchor: &str) -> String` — lowercased retro skill body from the first occurrence of `anchor` up to the next Markdown heading of any depth. Tasks 2, 3, and 4 call it with anchors `"## step 3:"`, `"### what you may file"`, and `"### before you file"`.

- [ ] **Step 1: Write the failing tests**

Add to `mod tests` in `src/setup/plugins.rs`, immediately after the `skill_body` function (`src/setup/plugins.rs:606`):

```rust
    /// A lowercased section of the retro skill body: from the first occurrence
    /// of `anchor` up to the next Markdown heading of any depth.
    ///
    /// Scoped per-section deliberately. Retro repeats words like "task",
    /// "spec" and "fix" across its steps, so a whole-document `contains` can
    /// still pass after the instruction under test has been deleted. Ending at
    /// `\n#` rather than at a fixed depth means promoting or demoting a heading
    /// cannot silently widen a section to the rest of the file. If you reword
    /// an anchor heading, re-anchor it here.
    fn retro_section(anchor: &str) -> String {
        let content = skill_body("retro").to_lowercase();
        let (_, section) = content.split_once(anchor).unwrap_or_else(|| {
            panic!("retro skill must contain the section anchored on {anchor:?}")
        });
        section
            .split_once("\n#")
            .map_or(section, |(block, _)| block)
            .to_string()
    }

    #[test]
    fn retro_admission_test_is_next_agent_benefit_not_doc_accuracy() {
        // The old Step 2 asked whether CLAUDE.md or a spec was "stale or
        // wrong" — a correctness question every trivial nit passes, which is
        // how retro came to file 38 one-line doc chores. The bar is now
        // whether the *next* agent would do better, and each finding must
        // trace to a concrete moment this session actually lost time on.
        let section = retro_section("## step 2:");
        assert!(
            section.contains("would the next agent do better"),
            "retro's admission test must be whether the next agent benefits, \
             not whether a sentence is inaccurate"
        );
        assert!(
            section.contains("concrete moment"),
            "retro must require every finding to trace to a concrete moment \
             from Step 1 rather than to a hypothetical"
        );
        assert!(
            !section.contains("stale or wrong"),
            "retro must not frame its check as a documentation-accuracy audit"
        );
    }

    #[test]
    fn retro_reflection_feeds_the_context_check() {
        // Step 1's reflection used to be decorative: printed in Step 4 and
        // discarded, while Step 2 ran an audit that ignored it. The friction
        // the agent actually hit is now the input to what gets fixed.
        let section = retro_section("## step 1:");
        assert!(
            section.contains("lost time") || section.contains("lose time"),
            "retro's first step must ask where the session lost time"
        );
        assert!(
            section.contains("nothing notable"),
            "retro must state that an empty reflection is a real answer, so a \
             smooth session is not pressured into inventing findings"
        );
    }
```

- [ ] **Step 2: Run the tests to verify they fail**

Run: `cargo test --lib setup::plugins::tests::retro_ -- --nocapture`

Expected: both new tests FAIL. `retro_admission_test_is_next_agent_benefit_not_doc_accuracy` panics on `retro skill must contain the section anchored on "## step 2:"`— the current heading is `## Step 2: Check for drift`, which lowercases to `## step 2: check for drift` and *does* contain the anchor, so it instead fails the first assertion: `retro's admission test must be whether the next agent benefits`. `retro_reflection_feeds_the_context_check` fails on `retro must ask where the session lost time` (the current Step 1 says "Went well"/"Could improve").

- [ ] **Step 3: Rewrite Steps 1 and 2 of the skill**

In `plugin/skills/retro/SKILL.md`, replace everything from `## Step 1: Reflect` through the end of Step 2 (the line `direct edit from this skill.`) with:

```markdown
## Step 1: Where did you lose time?

List the concrete moments this session where you were slowed down or misled. Be
specific — a moment, not a category:

- An assumption you took from `CLAUDE.md` that turned out to be wrong.
- A convention you had to discover by reading source, because nothing told you.
- A spec that described behaviour the code no longer had.
- A rule you had to guess at — where a test goes, which helper to use.
- A command that failed until you found the right invocation.

Also note what went well and is worth repeating.

Keep both grounded in what actually happened this session. **"Nothing notable"
is a real and common answer** — a session that ran smoothly has no findings, and
padding this list is how retro turns into busywork.

## Step 2: Would a context change have prevented it?

For each moment in Step 1, ask: **is there a change to the agent-facing context
that would have prevented it?** These are the surfaces that reach the next agent:

| Surface | Reaches the next agent via |
|---|---|
| root `CLAUDE.md` | loaded into every dispatch prompt |
| `docs/specs/*.allium` | the source of truth agents consult |
| `docs/*.md` | linked from `CLAUDE.md`, read on demand |
| `plugin/skills/*/SKILL.md` | the skill the next agent invokes |
| the knowledge base | injected into the prompt (see the `learnings` skill) |
| the repo's verify command | appended to every dispatch prompt |

The test is **"would the next agent do better?"** — not "is this statement
inaccurate?" Every finding must trace back to a concrete moment from Step 1. If
nothing in this session was made harder by it, it is not a finding, however true
it is.

Concretely: "`CLAUDE.md` claims there is a single DB connection; I designed
against that and had to back the design out" is a finding — it cost real time.
"A timing constant isn't listed in the constants table" is not — nobody was
slowed by its absence, and filing it buys the next agent nothing.
```

- [ ] **Step 4: Run the tests to verify they pass**

Run: `cargo test --lib setup::plugins::tests::retro_`

Expected: `retro_admission_test_is_next_agent_benefit_not_doc_accuracy` and `retro_reflection_feeds_the_context_check` PASS. `retro_skill_tells_agent_to_resume_the_caller` still passes (Step 4's "Do not stop here" is untouched by this task).

- [ ] **Step 5: Commit**

```bash
git add plugin/skills/retro/SKILL.md src/setup/plugins.rs
```

```bash
git commit -m "$(cat <<'EOF'
feat(retro): judge findings by next-agent benefit, not doc accuracy

Steps 1 and 2 merge into one evidence-based question: where did this
session lose time, and would a context change have prevented it. The old
"is CLAUDE.md stale or wrong?" audit passed every trivial nit and never
returned no, which is how retro filed 38 one-line doc chores.

Co-Authored-By: Claude Opus 5 (1M context) <noreply@anthropic.com>
EOF
)"
```

---

### Task 2: Replace the blanket no-edit ban with a bounded edit licence

Retro currently forbids editing anything, so the one agent with full context must hand a one-line correction to a future agent who rebuilds that context from scratch. This task grants a bounded licence and states exactly what must still be filed.

**Files:**
- Modify: `src/setup/plugins.rs` (2 tests, after Task 1's tests)
- Modify: `plugin/skills/retro/SKILL.md` (Step 3's opening — replaces the current `## Step 3: Turn findings into follow-up tasks, not edits` heading and its first two paragraphs)

**Interfaces:**
- Consumes: `retro_section(anchor: &str) -> String` from Task 1
- Produces: the `## Step 3: Fix it here, or file it` heading, which Task 3 appends its `###` subsections to

- [ ] **Step 1: Write the failing tests**

Add to `mod tests` in `src/setup/plugins.rs`, after Task 1's tests:

```rust
    #[test]
    fn retro_skill_permits_fixing_small_context_drift_in_session() {
        // The old Step 3 said "Do not edit files yourself", which turned every
        // one-line doc correction into a task + worktree + agent dispatch. The
        // agent that just did the work has the context and is already in a
        // worktree whose next step is a commit; it should make the fix.
        let content = skill_body("retro").to_lowercase();
        assert!(
            !content.contains("do not edit files yourself"),
            "retro must no longer ban editing outright — fixing small context \
             drift in place is now its job"
        );
        let section = retro_section("## step 3:");
        assert!(
            section.contains("fix it yourself"),
            "retro must tell the agent to fix small context drift in this session"
        );
        assert!(
            section.contains("small and self-evident"),
            "retro's edit licence must be bounded to small, self-evident \
             corrections that need no design judgement"
        );
    }

    #[test]
    fn retro_skill_forbids_speccing_unimplemented_behaviour() {
        // A spec edit describing behaviour the session already implemented is
        // documentation catching up. One describing behaviour the code lacks is
        // a design change, and this repo runs those spec -> tests -> code with
        // their own dispatch — so retro must file it, not write it.
        let section = retro_section("## step 3:");
        assert!(
            section.contains("already implemented"),
            "retro may only edit a spec to describe behaviour this session \
             already implemented"
        );
        assert!(
            section.contains("spec → tests → code"),
            "retro must route a spec change for not-yet-implemented behaviour \
             to a task, naming the spec -> tests -> code loop as the reason"
        );
    }
```

- [ ] **Step 2: Run the tests to verify they fail**

Run: `cargo test --lib setup::plugins::tests::retro_skill_permits setup::plugins::tests::retro_skill_forbids`

Expected: both FAIL. `retro_skill_permits_fixing_small_context_drift_in_session` fails its first assertion — `plugin/skills/retro/SKILL.md` still contains "**Do not edit files yourself.**". `retro_skill_forbids_speccing_unimplemented_behaviour` fails on `already implemented`.

- [ ] **Step 3: Rewrite Step 3's opening**

In `plugin/skills/retro/SKILL.md`, replace the heading `## Step 3: Turn findings into follow-up tasks, not edits` and everything up to (but not including) the current `**Anti-patterns — do not create a task for:**` line with:

```markdown
## Step 3: Fix it here, or file it

Fix it yourself, in this session, when **all** of these hold:

- the surface is agent-facing prose — `CLAUDE.md`, a page under `docs/`, or a
  skill under `plugin/skills/`;
- your own work this session made it wrong, or this session proved it wrong;
- the correction is small and self-evident, needing no judgement about intended
  design.

One Allium case is also yours to fix: making a spec describe behaviour **this
session already implemented** is documentation catching up, not a design change.

You are the best-placed agent to make these fixes. You have the context, and
you are already in a worktree whose very next step is a commit — the calling
skill picks your edits up. Handing a one-line correction to a future agent who
must rebuild everything you already know is the expensive way to do nothing.

File a task with `create_task` instead — and do **not** edit — when:

- the change would make a spec describe behaviour the code does not have yet.
  That is a `spec → tests → code` loop and needs its own dispatch;
- the fix needs a judgement call about intended design;
- it is a pre-existing inaccuracy whose history you do not know;
- it is not small.

Also file a `bug` for a concrete defect you noticed but could not fix in scope —
one with an observable wrong behaviour, not a suspicion.
```

- [ ] **Step 4: Run the tests to verify they pass**

Run: `cargo test --lib setup::plugins::tests::retro_`

Expected: all four retro tests so far PASS.

- [ ] **Step 5: Commit**

```bash
git add plugin/skills/retro/SKILL.md src/setup/plugins.rs
```

```bash
git commit -m "$(cat <<'EOF'
feat(retro): let retro fix small context drift instead of filing it

The blanket "do not edit files yourself" ban forced one-line corrections
through a whole task + worktree + agent dispatch, handing them to an
agent with none of the context. Retro now fixes small, self-evident
agent-context drift in place and files only what it must not touch —
notably a spec change describing behaviour the code does not have yet,
which owes the spec -> tests -> code loop.

Co-Authored-By: Claude Opus 5 (1M context) <noreply@anthropic.com>
EOF
)"
```

---

### Task 3: Narrow the filing bar to two tags, with a duplicate check

Retro's archived tasks are almost entirely speculative refactors filed as enhancements, and two findings were each filed twice. This task drops `feature` from retro's vocabulary and requires a duplicate check.

**Files:**
- Modify: `src/setup/plugins.rs` (3 tests, after Task 2's tests)
- Modify: `plugin/skills/retro/SKILL.md` (replaces the current `**Anti-patterns — do not create a task for:**` block, the `repo_path` note, and the `Cap it to what's genuinely worth a task` line — i.e. the remainder of the old Step 3)

**Interfaces:**
- Consumes: `retro_section(anchor: &str) -> String` from Task 1; the `## Step 3:` heading from Task 2
- Produces: the `### What you may file` and `### Before you file` subsections, which Task 3's tests anchor on

- [ ] **Step 1: Write the failing tests**

Add to `mod tests` in `src/setup/plugins.rs`, after Task 2's tests:

```rust
    #[test]
    fn retro_skill_does_not_file_feature_tasks() {
        // Every archived retro-created task was a speculative refactor dressed
        // as an enhancement — "this invariant is enforced by convention, so the
        // same omission could recur", "this could be a single atomic insert".
        // feature leaves retro's vocabulary entirely.
        let section = retro_section("### what you may file");
        assert!(
            section.contains("never file a `feature`"),
            "retro must explicitly refuse to file feature tasks"
        );
        assert!(
            section.contains("speculative refactor"),
            "retro must name speculative refactors as a non-finding, since that \
             is the shape of every retro task that got archived"
        );
        assert!(
            !section.contains("`feature` for"),
            "retro must not still describe when to use the feature tag"
        );
    }

    #[test]
    fn retro_skill_requires_a_duplicate_check_before_filing() {
        // Two findings were each filed twice: one stale sentence that appeared
        // in two documents, and one recurring shape nobody recognised. Nothing
        // in the skill told the agent to look first.
        let section = retro_section("### before you file");
        assert!(
            section.contains("list_tasks"),
            "retro must check for an existing task with list_tasks before filing"
        );
        assert!(
            section.contains("one task per finding"),
            "retro must collapse a finding that spans several files into one task"
        );
    }

    #[test]
    fn retro_skill_states_zero_findings_is_the_normal_outcome() {
        // The old skill buried this under three steps of checklist-shaped
        // instructions, which read as a quota to fill rather than a bar to
        // clear.
        let section = retro_section("### before you file");
        assert!(
            section.contains("zero tasks is the normal outcome"),
            "retro must state outright that filing nothing is the expected result"
        );
    }
```

- [ ] **Step 2: Run the tests to verify they fail**

Run: `cargo test --lib setup::plugins::tests::retro_skill_does_not_file setup::plugins::tests::retro_skill_requires setup::plugins::tests::retro_skill_states`

Expected: all three FAIL by panicking in `retro_section` — the `### what you may file` and `### before you file` headings do not exist yet.

- [ ] **Step 3: Rewrite the rest of Step 3**

In `plugin/skills/retro/SKILL.md`, replace everything from `**Anti-patterns — do not create a task for:**` through the line `one, not several.` (the end of the old Step 3, just before `## Step 4: Output`) with:

```markdown
### What you may file

Only two tags:

- `bug` — a concrete defect with observable wrong behaviour.
- `chore` — a context improvement that passed Step 2 but that you must not fix
  yourself under the rules above.

**Never file a `feature`.** Speculative refactors and enhancement ideas are not
retro findings. "This invariant is enforced by convention, so the same omission
could recur" is a hypothetical, not a defect — and it is the shape of every
retro-filed task that later got archived unread. If it matters, it will come
back as a real bug with a real incident behind it.

### Before you file

- **Check for a duplicate.** Call `list_tasks` and look for an existing task
  covering the finding. Do not file a second one.
- **One task per finding, not per file.** The same wrong statement repeated
  across three documents is one task that lists all three, not three tasks.
- **Write it so a cold agent can act.** `title` names the specific change.
  `description` references this task's ID and says what the next agent will hit
  if it stays unfixed — e.g. "Found during task #123 — `CLAUDE.md` claims the DB
  has a single connection; I designed against that and had to back it out."

`repo_path` and `epic_id` are inherited from the caller — no need to pass them
explicitly unless overriding.

**Zero tasks is the normal outcome.** Most sessions should file none. Filing one
is unremarkable. Filing several means you are recording nits, not findings — go
back to Step 2 and drop the ones that cost this session nothing.
```

- [ ] **Step 4: Run the tests to verify they pass**

Run: `cargo test --lib setup::plugins::tests::retro_`

Expected: all seven retro tests PASS.

- [ ] **Step 5: Commit**

```bash
git add plugin/skills/retro/SKILL.md src/setup/plugins.rs
```

```bash
git commit -m "$(cat <<'EOF'
feat(retro): drop the feature tag and require a duplicate check

Retro may file only bug and chore. Speculative refactors — the shape of
every retro-filed task that later got archived — are named as
non-findings. A list_tasks duplicate check and a one-task-per-finding
rule address the two findings that were each filed twice, and "zero tasks
is the normal outcome" is now stated where the agent will read it.

Co-Authored-By: Claude Opus 5 (1M context) <noreply@anthropic.com>
EOF
)"
```

---

### Task 4: Move `/retro` to wrap-up's pre-commit step

Retro's edit licence only works if its edits get committed. Today retro runs at wrap-up Step 5D, *after* `wrap_up` — by which point the rebase path has already fast-forwarded `base_branch` and the PR path has already pushed. This task moves the invocation to Step 2.6, beside `simplify`.

**Files:**
- Modify: `src/setup/plugins.rs` (1 new test; update the comment and message of `retro_skill_tells_agent_to_resume_the_caller` at `src/setup/plugins.rs:640`)
- Modify: `plugin/skills/wrap-up/SKILL.md` (frontmatter description; line 10's flow summary; new Step 2.6 after line 66; Step 5 renumbering at lines 105–125; the Step E references at lines 237 and 243)
- Modify: `plugin/skills/retro/SKILL.md` (frontmatter description; Step 4's closing paragraph)

**Interfaces:**
- Consumes: `skill_body(skill: &str) -> &'static str` (`src/setup/plugins.rs:599`)
- Produces: nothing later tasks depend on

- [ ] **Step 1: Write the failing test**

Add to `mod tests` in `src/setup/plugins.rs`, after Task 3's tests:

```rust
    #[test]
    fn wrap_up_skill_runs_retro_before_the_commit_step() {
        // Retro fixes small agent-context drift in place, so it has to run
        // before the commit that carries those edits. Invoked after wrap_up
        // instead, its edits are stranded: the rebase path has already
        // fast-forwarded base_branch, so a later commit sits on a branch
        // nobody merges, and the PR path has already pushed.
        let content = skill_body("wrap-up");
        let retro_at = content
            .find("Skill({ skill: \"retro\" })")
            .expect("wrap-up skill must invoke the retro skill");
        let commit_at = content
            .find("## Step 3: Commit uncommitted changes")
            .expect("wrap-up skill must have a commit step to anchor retro before");
        assert!(
            retro_at < commit_at,
            "wrap-up must invoke retro before its commit step, so retro's \
             context fixes are committed with the session's work"
        );
        assert_eq!(
            content.matches("Skill({ skill: \"retro\" })").count(),
            1,
            "wrap-up must invoke retro exactly once — a leftover call in the \
             closing sequence would run it twice and re-file its findings"
        );
        assert!(
            !content.to_lowercase().contains("run `/retro`"),
            "wrap-up must not still invoke retro from the closing sequence \
             between wrap_up and exit_session"
        );
    }
```

- [ ] **Step 2: Update the existing resume test's rationale**

The assertion in `retro_skill_tells_agent_to_resume_the_caller` (`src/setup/plugins.rs:640`) still holds — retro must resume its caller — but it now resumes into wrap-up's commit step, not `exit_session`. Replace that test's comment and failure message:

```rust
    #[test]
    fn retro_skill_tells_agent_to_resume_the_caller() {
        // wrap-up invokes retro pre-commit, before its commit step. Without an
        // explicit instruction to resume the caller's remaining steps, an
        // agent that just finished following retro's own steps has nothing
        // telling it to continue — that's how wrap-up gets stuck after retro
        // and never reaches the commit, the user's action choice, or
        // exit_session. Retro's own edits are among what that commit carries,
        // so stopping here loses them.
        let content = skill_body("retro");
        assert!(
            content.contains("do not stop here") || content.contains("Do not stop here"),
            "retro skill must explicitly instruct the agent to resume the \
             calling skill's next step instead of stopping"
        );
    }
```

- [ ] **Step 3: Run the test to verify it fails**

Run: `cargo test --lib setup::plugins::tests::wrap_up_skill_runs_retro`

Expected: FAIL on `wrap-up must invoke retro before its commit step` — the current `Skill({ skill: "retro" })` sits in Step 5D, after `## Step 3: Commit uncommitted changes`.

- [ ] **Step 4: Add wrap-up's Step 2.6**

In `plugin/skills/wrap-up/SKILL.md`, insert after line 66 (`If there are no code file changes, skip this step entirely.`) and before `## Step 3: Commit uncommitted changes`:

```markdown
## Step 2.6: Run the retro

Invoke the retro skill:

```
Skill({ skill: "retro" })
```

Wait for it to complete before proceeding.

Retro reflects on where this session lost time and may fix small inaccuracies in
`CLAUDE.md`, a page under `docs/`, or a skill so the next agent dispatched here
does better. It runs **here, before the commit**, so anything it fixes is
committed by Step 3 and travels with the rebase or the PR.

Do not defer it to the closing sequence. After `wrap_up` the rebase path has
already fast-forwarded `{base_branch}`, so a later commit strands those fixes on
a branch nobody merges, and the PR path has already pushed.

Retro may also file follow-up tasks. That is expected — leave them alone.
```

- [ ] **Step 5: Remove the closing sequence's retro step and renumber**

Five edits in `plugin/skills/wrap-up/SKILL.md`:

1. Line 10 — replace the flow summary:

```markdown
`/retro` (pre-commit) → commit → `wrap_up(action)` → a single `exit_session(token, action, ...)` call that applies the terminal state change and closes the session.
```

2. Line 105 — `Every path ends with the same five steps.` becomes `Every path ends with the same four steps.`

3. Line 111 — in step C, `that all waits for Step E` becomes `that all waits for Step D`.

4. Delete the whole of the old **D. Run `/retro`.** paragraph (line 121), then relabel the old **E. Call `exit_session`** as **D. Call `exit_session`**, and change `token` (from Step C) to stay as-is. In the same block, line 125's `Do not stop between C and E.` becomes `Do not stop between C and D.`

5. Line 237 — `it is the pr_url you pass to exit_session in Step E` becomes `... in Step D`.

- [ ] **Step 6: Fix wrap-up's two remaining stale ordering claims**

1. Frontmatter (line 3) — replace `the two calls have an ordering and a retro step between them that are easy to get wrong` with:

```
the retro step must run before the commit and the two closing calls have an ordering that is easy to get wrong
```

2. Line 243 — the PR path's closing note. Replace `so a merge can't tear the session down while you're still in retro. Don't reorder to "close first, retro after".` with:

```
so a merge can't tear the session down between the two calls. Don't reorder to "close first, then finish up".
```

This sentence's reason is strengthened by the move, not weakened: at Step 2.6 the PR does not exist yet, so nothing can be merged out from under retro.

- [ ] **Step 7: Update retro's own frontmatter and closing paragraph**

1. In `plugin/skills/retro/SKILL.md`, the frontmatter `description` (line 3) — replace `The wrap-up skill invokes this automatically between wrap_up and exit_session` with `The wrap-up skill invokes this automatically before its commit step`, and replace the trailing `Captures what went well/could improve, checks whether this repo's CLAUDE.md or Allium specs are now stale, and opens follow-up tasks for anything actionable.` with:

```
Reflects on where this session lost time, fixes small agent-context drift in place so the next agent does better, and opens follow-up tasks only for what it must not fix itself.
```

2. Step 4's closing paragraph — replace the sentence `Retro is almost always invoked as a sub-step of \`wrap-up\`, between \`wrap_up\` and \`exit_session\`.` with:

```
Retro is almost always invoked as a sub-step of `wrap-up`, just before that skill's commit step — which is what carries any edit you just made.
```

Leave `Do not stop here.` in place; `retro_skill_tells_agent_to_resume_the_caller` asserts on it.

- [ ] **Step 8: Run the tests to verify they pass**

Run: `cargo test --lib setup::plugins::tests`

Expected: all `setup::plugins::tests` tests PASS, including `wrap_up_skill_runs_retro_before_the_commit_step`, `retro_skill_tells_agent_to_resume_the_caller`, and the pre-existing `wrap_up_skill_uses_simplify_not_code_simplifier` (`src/setup/plugins.rs:498`).

- [ ] **Step 9: Commit**

```bash
git add plugin/skills/wrap-up/SKILL.md plugin/skills/retro/SKILL.md src/setup/plugins.rs
```

```bash
git commit -m "$(cat <<'EOF'
fix(wrap-up): run retro pre-commit so its context fixes land

Retro sat at Step 5D, after wrap_up — by then the rebase path has
fast-forwarded base_branch and the PR path has pushed, so any edit retro
made was stranded. It moves to Step 2.6 beside simplify, whose changes
Step 3 already picks up the same way. The closing sequence loses its
retro step and renumbers E to D.

Co-Authored-By: Claude Opus 5 (1M context) <noreply@anthropic.com>
EOF
)"
```

---

### Task 5: Correct the specs that assert the old ordering

Three `@guidance` blocks state that wrap-up runs `/retro` between `wrap_up` and `exit_session`. That is now false. Their neighbouring justification — that the old in-handler `record_learning` nudge is redundant because `/retro` runs *before* `exit_session` — stays true and must not be touched.

**Files:**
- Modify: `docs/specs/mcp-task-tools.allium:623`
- Modify: `docs/specs/pr-workflow.allium:296`, `docs/specs/pr-workflow.allium:352`, `docs/specs/pr-workflow.allium:366`

**Interfaces:**
- Consumes: the wrap-up ordering established in Task 4
- Produces: nothing

- [ ] **Step 1: Confirm the four sites and read their context**

Run:

```bash
grep -n "retro" docs/specs/mcp-task-tools.allium docs/specs/pr-workflow.allium
```

Expected: eight hits — three in `mcp-task-tools.allium` (`:623`, `:682`, `:683`), five in `pr-workflow.allium` (`:296`, `:352`, `:366`, `:425`, `:427`).

Four need correcting: `mcp-task-tools.allium:623`, `pr-workflow.allium:296`, `:352`, `:366`.

Four must be left alone. `mcp-task-tools.allium:682-683` and `pr-workflow.allium:425,427` justify removing the old in-handler `record_learning` nudge on the grounds that `/retro` runs "BEFORE exit_session is ever called" and is "the forcing function" for reflection. Both claims survive the move — retro still runs before `exit_session`, just earlier — so the reasoning stays as written.

- [ ] **Step 2: Correct `mcp-task-tools.allium`**

At `docs/specs/mcp-task-tools.allium:623`, replace:

```
        -- The /wrap-up skill runs the /retro skill between wrap_up and
        -- exit_session.
```

with:

```
        -- The /wrap-up skill runs the /retro skill before its commit step,
        -- ahead of wrap_up, so any agent-context fix retro makes is committed
        -- with the session's work rather than stranded after the rebase.
```

Keep the following sentence (`For the pr path the agent has already run git push and gh pr create by this point…`) unchanged — it describes wrap_up's own preconditions, not retro's placement.

- [ ] **Step 3: Correct `pr-workflow.allium`'s three sites**

At `docs/specs/pr-workflow.allium:296`, replace `-- then call wrap_up(action="pr") (no pr_url), run the /retro skill,` with:

```
        -- then call wrap_up(action="pr") (no pr_url),
```

At `docs/specs/pr-workflow.allium:352`, replace `-- runs the /retro skill between wrap_up and exit_session. This removes` with:

```
        -- runs the /retro skill before its commit step, ahead of wrap_up. This removes
```

At `docs/specs/pr-workflow.allium:366`, replace `-- wrap_up(action="done"), runs /retro, then exit_session(token,` with:

```
        -- wrap_up(action="done"), then exit_session(token,
```

- [ ] **Step 4: Verify spec alignment**

Run: `allium check docs/specs/pr-workflow.allium docs/specs/mcp-task-tools.allium`

Expected: no errors. `@guidance` blocks are prose, so this confirms the edits did not break surrounding syntax.

Then run the `allium:weed` skill scoped to these two specs to confirm no remaining divergence between the ordering they describe and `plugin/skills/wrap-up/SKILL.md`.

- [ ] **Step 5: Run full verification**

Run: `cargo fmt --check && cargo test && ./scripts/check-doc-paths.sh`

Expected: all PASS. Note that `scripts/check-doc-paths.sh` does not scan `plugin/skills/` or `docs/superpowers/` (its list is hardcoded at `scripts/check-doc-paths.sh:23-28`), so it does not validate the new prose — that is pre-existing and already tracked as its own task.

- [ ] **Step 6: Commit**

```bash
git add docs/specs/mcp-task-tools.allium docs/specs/pr-workflow.allium
```

```bash
git commit -m "$(cat <<'EOF'
docs(specs): correct the retro ordering in wrap-up guidance

Four @guidance lines said /wrap-up runs /retro between wrap_up and
exit_session; it now runs before the commit step. The neighbouring
justification for dropping the in-handler record_learning nudge — that
/retro runs before exit_session — survives the move and is unchanged.

Co-Authored-By: Claude Opus 5 (1M context) <noreply@anthropic.com>
EOF
)"
```

---

## Self-Review

**Spec coverage** — every design section maps to a task:

| Design section | Task |
|---|---|
| Reframe: harness improvement, not documentation audit | 1 |
| Zero findings is the expected outcome | 3 |
| Fix in-session vs. file a task | 2 |
| What may still be filed (two tags, dup check, one-per-finding) | 3 |
| Ordering: retro moves pre-commit | 4 |
| Accepted consequence (cancelled wrap-up) | 4 — Step 2.6's prose does not promise the edits are committed on a cancelled wrap-up; no test needed since the design accepts the behaviour |
| Testing (7 new tests + 1 comment update) | 1 (2), 2 (2), 3 (3), 4 (1 + the comment update) |
| Allium specs (4 guidance lines; 2 left alone) | 5 |

**Type consistency** — `retro_section(anchor: &str) -> String` is defined once in Task 1 and called in Tasks 2 and 3 with lowercase anchors matching the headings those tasks write: `"## step 2:"` and `"## step 1:"` (Task 1), `"## step 3:"` (Task 2), `"### what you may file"` and `"### before you file"` (Task 3). `skill_body` is pre-existing and unchanged.

**Anchor/prose cross-check** — every asserted substring appears verbatim (case-insensitively) in the prose the same or an earlier task writes: `would the next agent do better` and `concrete moment` (Task 1 Step 3), `nothing notable` and `lose time` (Task 1 Step 3), `fix it yourself` / `small and self-evident` / `already implemented` / `spec → tests → code` (Task 2 Step 3), `never file a \`feature\`` / `speculative refactor` / `list_tasks` / `one task per finding` / `zero tasks is the normal outcome` (Task 3 Step 3).

**Negative-assertion cross-check** — the strings asserted absent are all removed by the task that asserts them: `stale or wrong` (Task 1 Step 3 deletes the old Step 2), `do not edit files yourself` (Task 2 Step 3), `` `feature` for `` (Task 3 Step 3 deletes the old tag list), `run \`/retro\`` (Task 4 Step 5).
