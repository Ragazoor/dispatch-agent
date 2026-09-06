# 4706 — Prompt audit: dated prompting patterns

Run of `/claude-api prompt-audit`. Findings below; everything actionable was applied — see "Applied" at the end.

## Step 0: assumptions

**Scope** (named in the request):

| Surface | Files | Lines |
|---|---|---|
| Dispatch prompt assembly | `src/dispatch/prompts.rs` | 2020 (≈420 are prompt text) |
| Review-mode prompt templates | `src/dispatch/prompts/pr-review.md`, `dependabot.md` | 45 |
| Agent skills | `plugin/skills/*/SKILL.md` + `allium-loop/prompt.md` + `decompose-review/references/plan-template.md` | 1082 |
| MCP tool + parameter descriptions | `src/mcp/handlers/dispatch.rs`, `src/mcp/handlers/tasks/mod.rs` | 21 tools, ~60 parameters |

Also in the surface but not separately audited: `src/dispatch/agents.rs:422` (the
worktree-scope line), which is one sentence and carries its reason.

**Target model: Claude Opus 5.** Resolved by the guide's fallback chain. The repo
names no model ID anywhere — it dispatches `claude` with no `--model` flag, so
agents run on whatever Claude Code defaults to, which is currently Opus 5. The
only model references in the whole surface are the tier aliases `sonnet` and
`opus` in `allium-loop`, which resolve to the current generation by design.

**Not applicable to this repo.** Dispatch calls no Anthropic API — no SDK, no
HTTP, no key. So Group 4's API-fossil checks (`budget_tokens`, prefill,
sampling parameters, beta headers, stop sequences, forced `tool_choice`) have no
code to examine. Confirmed by grep: zero hits.

---

## Summary

**The surface is in good shape.** The patterns this audit usually finds in
quantity are absent: no `think step by step`, no `<scratchpad>`/`<thinking>` tag
instructions, no prefill, no sampling-parameter fossils, no retired model names,
no grader vocabulary, no anti-formatting rules, no update suppressors, no
`do not hallucinate`. Pressure-language density is 18 capitalised
`MUST`/`NEVER`/`Do NOT` across ~4,500 lines, and every one of them carries its
reason in the same sentence.

**13 findings. One high confidence, six medium, six flag-only.**

The three that matter:

1. **Two skills give contradictory instructions on when to rate a learning.**
   `learnings` says explicitly "not deferred to wrap-up"; `wrap-up` permits
   exactly that deferral. This is the one duplication in the surface where the
   copies actually disagree.
2. **Six MCP tool descriptions are under-described** — one sentence or less,
   against the guide's 3–4 sentence floor. Under-description is the failure mode
   the audit says people most often get backwards, and the fix is *more* text,
   not less.
3. **The no-plan dispatch prompt states TDD twice and the Allium tend/weed
   cycle twice**, once inside the spec-first sequence and again in the trailing
   block. One conditional fixes both.

---

## Findings

### HIGH

#### H1 — `wrap-up` permits the deferral `learnings` forbids

| | |
|---|---|
| **Location** | `plugin/skills/wrap-up/SKILL.md:193` (vs `plugin/skills/learnings/SKILL.md:24`) |
| **Evidence** | wrap-up: "Rate via the `rate_learning` MCP tool — **ideally** the moment you act on an entry, **or at the latest before you wrap up**." — learnings: "Do this at the moment you act on it, **not deferred to wrap-up**." |
| **Pattern** | Group 2, duplicated info across skill files that has drifted apart; plus Group 1a, a hedge (`ideally`) attached to an actual requirement |
| **Why obsolete** | Keep-list item 8 allows working redundancy, but explicitly makes disagreeing duplicates a finding. Current models follow instructions literally, so an agent that loads wrap-up reads "at the latest before you wrap up" as permission for the batch-at-the-end behaviour the learnings skill was written to stop. `ideally` compounds it: attached to a requirement, it reads as optional. |
| **Confidence** | **High** |
| **Action** | `rewrite` — see hunk 1 |

### MEDIUM

#### M1 — TDD is stated twice in the same no-plan prompt

| | |
|---|---|
| **Location** | `src/dispatch/prompts.rs:176` (`tdd_instruction`) against `:212` (`spec_first_instruction`, steps 3–4) |
| **Evidence** | spec-first: "3. Generate tests from the spec with `allium:propagate` and confirm they fail before you write any code. 4. Implement the minimum code that makes them pass." — trailing block, six lines later: "Always use TDD: express intended behaviour as tests first, then implement the minimum code to make them pass." |
| **Pattern** | Group 1c — "repetition as reinforcement … duplicated rules make the model spend effort reconciling wordings" |
| **Why obsolete** | The two say the same thing, and "implement the minimum code" is near-verbatim in both. Written when they lived on different paths; the spec-first step now subsumes it. On the brainstorm and with-plan paths `tdd_instruction` is the only statement of TDD and must stay. |
| **Confidence** | **Medium** |
| **Action** | `remove`, conditionally — see hunk 2 |

#### M2 — the Allium tend/weed cycle is stated twice in the same no-plan prompt

| | |
|---|---|
| **Location** | `src/dispatch/prompts.rs:291` (`allium_instruction`) against `:212` (`spec_first_instruction`, steps 2 and 5) |
| **Evidence** | spec-first: "2. Capture what you agreed in the relevant `docs/specs/*.allium` file, via `allium:tend`. … 5. Confirm spec and code agree with `allium:weed`." — trailing block: "If your implementation changes domain behaviour, update the spec using the `allium:tend` skill and verify alignment with `allium:weed`." |
| **Pattern** | Group 1c, repetition |
| **Why obsolete** | Same two skills, same two actions, restated as a conditional after being given as unconditional steps. The restatement is weaker than the step it repeats ("if your implementation changes domain behaviour"), which invites the agent to reconcile them and conclude step 2 is conditional. Keep it on the with-plan path, where spec-first is absent. |
| **Confidence** | **Medium** |
| **Action** | `remove`, conditionally — same hunk as M1 |

#### M3 — the plan-confirmation ritual is scripted three ways

| | |
|---|---|
| **Location** | `src/dispatch/prompts.rs:409-412` |
| **Evidence** | "Review the plan **carefully**. Summarise your intended approach in **3–5 bullet points**, then ask: **'Shall I proceed with implementation?'** Wait for confirmation before making any changes." |
| **Pattern** | Three rows at once — Group 1a (bare intensifier with no adjacent reason), Group 1f (numeric output ceiling), Group 1c (a scripted exact utterance for a judgment moment) |
| **Why obsolete** | `carefully` adds no information a current model acts on. `3–5 bullet points` is a clamp tuned against an older model's verbosity and, per Group 1f, an operational reason would not rescue it — re-express as outcome. The quoted question freezes one phrasing into every plan dispatch regardless of what the plan is. The final sentence is the actual gate and stays. |
| **Confidence** | **Medium** |
| **Action** | `rewrite` — see hunk 3 |

#### M4 — six MCP tool descriptions are under the length floor

| | |
|---|---|
| **Location** | `src/mcp/handlers/dispatch.rs:255` `list_epics` (1 sentence, 35 chars) · `:245` `get_epic` (1, 56) · `:259` `update_epic` (1, 98) · `:232` `create_epic` (2, 105) · `:335` `unsubscribe_from_task` (2, 114) · `:394` `query_learnings` (2, 134) |
| **Evidence** | e.g. `list_epics`: "List all epics on the kanban board." — no mention of what a row contains, whether sub-epics are nested or flat, or that it takes no filters. |
| **Pattern** | Group 3, row 1 — "Vague one-liners; parameters without descriptions; no when-not-to-use → **Under-described – add**" |
| **Why obsolete** | Not a dated pattern but the direction the audit says is most often got backwards: description detail is the single largest factor in tool performance, and the floor is 3–4 sentences covering what the tool does, when to use it, when not to, and what it does not return. `query_learnings` is the costliest of the six — it never says the results are ranked by semantic similarity with a threshold, so a caller cannot tell an empty result from a below-threshold one. (Parameter descriptions are complete across all 21 tools; this finding is about tool-level text only.) |
| **Confidence** | **Medium** |
| **Action** | `add` — see hunk 4 |

#### M5 — numeric word ceiling in the PR-body template

| | |
|---|---|
| **Location** | `plugin/skills/wrap-up/SKILL.md:242` |
| **Evidence** | "{user-visible change and why it matters, plain language, **under 20 words where possible**}" |
| **Pattern** | Group 1f, numeric output ceiling; plus the `where possible` hedge from Group 1a |
| **Why obsolete** | A word cap tuned against an older model's padding. The hedge already concedes the number is not real, which leaves an instruction that costs tokens and steers nothing. The desired property — a bullet a reviewer can scan — survives without the count. |
| **Confidence** | **Medium** |
| **Action** | `rewrite` — see hunk 5 |

#### M6 — `mcp_tools_instruction` names tools the schema already carries

| | |
|---|---|
| **Location** | `src/dispatch/prompts.rs:181` |
| **Evidence** | "The dispatch MCP tools are available — use them to query and update this task (get_task, update_task)." |
| **Pattern** | Group 3 — "Tool names in the system prompt; prose lists that shadow the real tool list → **Duplicated – delete**" |
| **Why obsolete** | Both tools are in the model's tool list with full schemas, and `get_task`'s description now states what it returns and which lines matter. The prose adds no fact the schema lacks. |
| **Confidence** | **Medium** |
| **Action** | `remove` — see hunk 6. **Counter-argument, stated so you can decline:** a dispatched agent sees a large tool list and `mcp__dispatch__*` competes with Claude Code's own tools; this line is a cheap pointer at the two that matter. The documented pattern matches, so it is proposed — but this is a reasonable hunk to reject. |

### FLAG — reported, no edit proposed

#### F1 — the Dependabot agent executes a largely deterministic plan

`src/dispatch/prompts/dependabot.md:1-35`. Group 4, "An LLM executor for a
deterministic plan". Counting the work: step 3 matches commit authors against a
two-item allowlist and file paths against a fixed glob list; step 4 parses
`Bump <pkg> from <X> to <Y>` with a regex and classifies semver; step 5 reads
`gh pr checks` exit state; step 6's minor branch greps release notes for seven
fixed tokens. All five are functions of their inputs. The genuinely adaptive
work is one step — reading a major bump's changelog and writing the breaking-change
summary in step 6.

The fix is to move steps 3–5 and the token scan into the feed script that
creates these tasks, so the agent receives a pre-classified task and keeps only
the judgment call. That is a change to the feed script, which is outside the
scope you named, so no hunk is proposed. Worth its own task.

#### F2 — `pr-review.md`'s diff-size branch is a deterministic route

`src/dispatch/prompts/pr-review.md:2-6`. Same pattern as F1, much smaller: the
agent shells out to `wc -l` and branches on `< ~300`. Cheap enough that moving
it may not pay for itself. Flagged for completeness.

#### F3 — "Do NOT call /wrap-up" appears three times in one 35-line prompt

`src/dispatch/prompts/dependabot.md:1,31,35`; twice in the 9-line
`pr-review.md:1,9`. Group 1c repetition. Defensible as placement rather than
repetition — lines 31 and 35 sit at the two terminal branches where an agent
would reach for wrap-up. Not proposed.

#### F4 — the epic-decomposition carve-out is stated twice

`src/dispatch/prompts.rs:212` and `:282`. Both say implementation (or, for a
decomposition task, creating work packages) is what ends the task. Keep-list
item 10 covers this: a single end-of-prompt restatement of key constraints is a
known reasonable pattern, and `wrap_up_instruction` is that position. The source
comments show the duplication is deliberate and kept in sync. Not proposed.

#### F5 — "has happened twice" incident narrative

`plugin/skills/wrap-up/SKILL.md:49`. Group 2 flags history narratives, but
keep-list item 5 keeps prohibitions against demonstrated failures, and here the
incident *is* the stated reason. Low-confidence idiom match only. Not proposed.

#### F6 — stable instructions sit last in the assembled prompt

`src/dispatch/prompts.rs:386`. The renderer puts the volatile parts (task
block, injected knowledge) before the invariant trailing block, which is the
reverse of cache-friendly ordering. Not actionable here — dispatch hands a
single message to the `claude` CLI and does not control its caching. Noted only
because Group 4 asks for it.

---

## Proposed diff

Six hunks, one per finding. Take them independently.

### Hunk 1 — H1: make wrap-up defer to the learnings rule

```diff
--- a/plugin/skills/wrap-up/SKILL.md
+++ b/plugin/skills/wrap-up/SKILL.md
@@
-Rate via the `rate_learning` MCP tool — ideally the moment you act on an entry, or at the latest before you wrap up. For every learning you acted on that was surfaced to you this task:
+Rate via the `rate_learning` MCP tool at the moment you act on an entry, not here — see the `learnings` skill. This step is the backstop: for anything you acted on this task and have not yet rated, rate it now.
```

### Hunk 2 — M1 + M2: drop the two restatements when spec-first is already in the prompt

Requires threading one flag. `trailing_block` currently takes only
`has_allium_specs`; the caller knows which addendum it chose.

```diff
--- a/src/dispatch/prompts.rs
+++ b/src/dispatch/prompts.rs
@@ pub(super) fn trailing_block(
-pub(super) fn trailing_block(has_allium_specs: bool) -> String {
-    let mut lines = vec![tdd_instruction()];
-    if has_allium_specs {
-        lines.push(allium_instruction());
-    }
+/// `design_is_spec_first` says the prompt already carries
+/// `spec_first_instruction`. That sequence states TDD (steps 3-4) and the
+/// tend/weed cycle (steps 2 and 5) as unconditional steps, so repeating both
+/// here restates them in weaker, conditional wording a few lines later.
+pub(super) fn trailing_block(has_allium_specs: bool, design_is_spec_first: bool) -> String {
+    let mut lines = Vec::new();
+    if !design_is_spec_first {
+        lines.push(tdd_instruction());
+        if has_allium_specs {
+            lines.push(allium_instruction());
+        }
+    }
     lines.extend([
         mcp_tools_instruction(),
         learning_tools_instruction(),
         wrap_up_instruction(),
     ]);
     lines.join("\n\n")
 }
```

Both call sites pass `ctx.has_allium_specs && plan.is_none()` (in
`build_prompt`) and `ctx.has_allium_specs` (in `build_quick_dispatch_prompt`,
which always uses the design step).

**Completing this hunk** — per the guide, a removal is done only when everything
referencing it goes too:
- `src/dispatch/prompts.rs:1146` `trailing_block_omits_the_allium_instruction_when_the_repo_has_no_specs` — update for the new signature and add a case for the spec-first branch.
- `src/dispatch/prompts.rs:1158,1162,1175` — the `contains` assertions on `tdd_instruction()` / `allium_instruction()`.
- Six snapshots under `src/dispatch/snapshots/` contain "Always use TDD"; `no_plan`, `with_epic` and `quick_dispatch` will change, the three `with_plan*` ones will not.

### Hunk 3 — M3: state the gate, drop the script

```diff
--- a/src/dispatch/prompts.rs
+++ b/src/dispatch/prompts.rs
@@
-                " .\n\
-\n\
-Review the plan carefully. Summarise your intended approach in 3–5 bullet points, \
-then ask: 'Shall I proceed with implementation?' Wait for confirmation before \
-making any changes."
+                ".\n\
+\n\
+Read the plan, then summarise the approach you intend to take and ask the user to \
+confirm it. Make no changes until they do."
```

`src/dispatch/prompts.rs:1573` and the two `with_plan` snapshots assert on this
text.

### Hunk 4 — M4: bring six descriptions up to the floor

```diff
--- a/src/mcp/handlers/dispatch.rs
+++ b/src/mcp/handlers/dispatch.rs
@@ create_epic
-        "Create a new epic on the kanban board. Pass parent_epic_id to create it as a sub-epic of an existing one.",
+        "Create a new epic on the kanban board. An epic groups related tasks and derives its \
+status from theirs, so you do not set an epic's status directly. Pass parent_epic_id to create \
+it as a sub-epic of an existing one. Epics carry no repo_path — that lives on each subtask, and \
+passing it here is rejected.",
@@ get_epic
-        "Get details about an epic including its subtask summary.",
+        "Get one epic by ID: its title, description, status, plan and a summary of its subtasks \
+by status. Read-only. Use it to check an epic's progress before adding work to it or closing it \
+out. It does not return the subtasks themselves — call list_tasks with epic_id for those.",
@@ list_epics
-        "List all epics on the kanban board.",
+        "List every epic on the kanban board. Takes no arguments and applies no filtering or \
+auto-scoping — sub-epics come back as ordinary rows alongside their parents rather than nested \
+under them. Use it to find an epic's ID when you only know its title. For one epic's detail or \
+its subtask counts, call get_epic instead.",
@@ update_epic
-        "Update an epic's title, description, status, plan, sort order, feed configuration, or parent epic.",
+        "Update an epic in place: title, description, status, plan_path, sort_order, the feed \
+configuration, or parent_epic_id. Every field left out is untouched; the nullable fields take an \
+explicit null to clear. Setting status by hand is rarely right — an epic's status is recalculated \
+from its subtasks whenever they change. Re-parenting is cycle-checked and rejected if it would \
+create a loop.",
@@ unsubscribe_from_task
-        "Cancel a previously registered subscribe_to_task watch. Idempotent — succeeds even if no such subscription exists.",
+        "Cancel a watch previously registered with subscribe_to_task. Idempotent: it succeeds \
+whether or not such a subscription exists, so it is safe to call speculatively. You do not need \
+it after a notification has fired — delivery is one-shot and spends the subscription. Use it when \
+you stop caring about the target before it finishes.",
@@ query_learnings
-        "Query the knowledge base for entries relevant to the current task's context using semantic search (RAG). Excludes task-scoped entries.",
+        "Query the knowledge base by semantic similarity to your task's context. Results are \
+ranked by embedding similarity plus a scope and upvote boost, and entries below the similarity \
+threshold are dropped — so an empty result means nothing relevant was found, not that the store \
+is empty. Task-scoped entries are excluded; the entries already injected into your prompt are \
+not, so expect overlap with those. Call it when something is unclear, before guessing or asking.",
```

The `query_learnings` rewrite is the one hunk here asserting runtime behaviour
rather than restating a schema, so it was checked against the code rather than
inferred: the ranker scores `cosine * (1.0 + scope_mul) + upvote_boost + tag_boost`
and returns `None` for any candidate below the threshold, and the handler
applies no dedup against the entries already injected at dispatch — so both the
"empty result means nothing relevant" and the "expect overlap" claims hold.

### Hunk 5 — M5: drop the word cap

```diff
--- a/plugin/skills/wrap-up/SKILL.md
+++ b/plugin/skills/wrap-up/SKILL.md
@@
-  - {user-visible change and why it matters, plain language, under 20 words where possible}
+  - {user-visible change and why it matters, in plain language}
```

### Hunk 6 — M6: drop the tool-name pointer

Optional; see the counter-argument in M6.

```diff
--- a/src/dispatch/prompts.rs
+++ b/src/dispatch/prompts.rs
@@ fn trailing_block
     lines.extend([
-        mcp_tools_instruction(),
         learning_tools_instruction(),
         wrap_up_instruction(),
     ]);
```

`mcp_tools_instruction` stays in use by the research and review trailing blocks
(`prompts.rs:422`, `:526`), so the function itself is not dead. Tests at
`src/dispatch/tests.rs:52` and the six snapshots would need updating.

---

## What was checked and found clean

Recorded so the next audit does not re-derive it:

- **Group 1b, scaffolds replaced by API features** — zero hits for `think step by step`, `<scratchpad>`, `<thinking>` instructions, "show your thinking", `stop_sequences`, `budget_tokens`, `temperature`, `top_p`, prefill, forced `tool_choice`. The repo calls no API, so there is no request builder to audit.
- **Group 1d, fossils** — no retired model names in prompts or comments, no date-conditional guidance, no update suppressors ("hold all findings", "don't narrate"), no anti-formatting rules, no turn-cadence reminder re-insertion.
- **Group 1e, prohibition clusters** — 18 capitalised prohibitions across the surface, each carrying its reason in the same sentence. No banned-phrase or tic lists.
- **Group 1c, grader vocabulary** — none.
- **Group 3, parameter descriptions** — complete across all 21 tools; none missing.
- **Group 2, volatile specifics** — the skills pin no version numbers and no absolute paths beyond `~/.claude/plugins/local/dispatch/`, which is the real install location.
- **Group 4, token accounting** — present, via `query_usage` and the statusline budget decorator.
- **The "Announce at start" line in six skills** — a scripted exact utterance, but it serves user-visible observability of which skill is running, and the harness's own skill conventions require it. Not a model-era workaround.
- **Numbered step sequences** in `wrap-up` (8), `allium-loop` (9) and `decompose-review` (7) — checked against Group 1c and kept. Each governs a fragile ordering where exactly one sequence is safe (the `wrap_up` → `exit_session` token handshake, the loop's state-file format, the epic-then-subtasks creation order). Keep-list item 3.

---

## Applied

All six hunks are in, plus the actionable half of F1. Full suite green (23
targets, no skips), clippy clean, all gate scripts pass, `allium check` reports
no errors.

Two things the tests found that the audit's greps had missed:

- **`delete_learning` and `get_managed_feed_config` were also under the
  three-sentence floor** — M4 named six tools; there were eight. The audit
  measured descriptions with a regex over the source, which silently skipped
  every entry using `\`-continuation. The floor test reads the *generated*
  definitions, so it saw all 21. Both were rewritten.
- **The Dependabot agent re-checks the PR author the feed already filtered on.**
  `fetch-dependabot.sh` lists PRs with `--author app/kognic-renovate`, so a
  dependabot task exists only for a PR that passed that filter — the agent's
  step 3 spent a `gh` call re-deriving a check that could only ever agree. The
  audit had this inside F1's general shape without naming it as its own defect.

### Stopped short of, deliberately

The rest of F1. Moving semver classification into the feed script needs a parser
for the PR title format, and the prompt asserts Dependabot's shape
(`Bump <pkg> from <X.Y.Z> to <A.B.C>`) while the feed fetches **Renovate** PRs,
whose default titles do not carry a `from`. Writing a parser against a format
neither the script nor the prompt can be checked against would be guessing at
production data. **This mismatch is worth its own investigation** — if Renovate
here is not configured to emit Dependabot-style titles, step 4 of the runbook
never matches and every bump falls through to "ask the user".

Left with the agent on purpose: CI status (it changes between the feed's poll
and dispatch) and changelog fetching (network work that does not belong on a
60-second timer).

### Recurrence guards added

| Guard | Lives in | Derived from |
|---|---|---|
| every tool description meets a 3-sentence floor | `src/mcp/handlers/tests/mod.rs` | the generated tool definitions |
| wrap-up and learnings agree that rating is not deferred | `src/setup/plugins.rs` | both skill bodies |
| no prompt shadows the tool list with a prose tool notice | `src/dispatch/tests.rs` | all three assembled prompts |
| every implementation prompt states test-first | `src/dispatch/tests.rs` | both wordings, per path |
| the trailing block drops tdd/allium only when spec-first states them | `src/dispatch/prompts.rs` | all four paths asserted |
| the Dependabot prompt does not re-check the author the feed filtered | `tests/feed_scripts.rs` | the feed script's own `--author` flag |

`SHARED_TRAILING_LINES` shrank from four needles to three: `TDD` and the
`allium_instruction` sentence are no longer universal, and what they stood in
for is pinned by the test above instead.

### Spec

`dispatch.allium` gained two guarantees under `DesignStepMatchesTheReposSpecs`
— "No trailing line restates the design step" and "The prompt names no tool
merely to say it exists" — and its trailing-block skeleton, research-mode note,
and the Dependabot/PrReview trailing descriptions were corrected to match.
