---
name: grill
description: >-
  Interview the user relentlessly about a plan or design, then capture decisions
  in Allium specs and ADRs. Use when the user wants to stress-test an idea before
  building — "grill me on this", "poke holes in this", "interrogate this design",
  "challenge my assumptions", "what am I missing".
---

# Grill

**Announce at start:** "I'm using the grill skill to interview you about this."

## Part 1: Interview

Interview me relentlessly about every aspect of this plan until we reach a shared understanding. Walk down each branch of the design tree, resolving dependencies between decisions one-by-one. For each question, provide your recommended answer.

Ask the questions one at a time, waiting for feedback on each question before continuing. Asking multiple questions at once is bewildering.

If a question can be answered by exploring the codebase, explore the codebase instead.

While interviewing:

- **Challenge fuzzy language.** If a term is ambiguous or used inconsistently, surface it — ask whether it maps to an existing concept in the Allium specs or is genuinely new.
- **Cross-reference with specs.** Check `docs/specs/*.allium` when a decision touches existing domain concepts. If the decision contradicts or extends a spec, flag it.
- **Stress-test with scenarios.** After a decision is made, probe edge cases: "what happens when X is null?", "what if two agents do this concurrently?", etc.

## Part 2: Capture decisions

After the interview reaches a shared understanding, capture what was decided.

### New or changed domain concepts → update Allium specs

If the interview introduced a new entity, enum, rule, or canonical term — or changed the meaning of an existing one — propose an update to the relevant `docs/specs/*.allium` file using the `allium:tend` skill.

Do this for concepts that will be part of the implementation. Don't speculate about things not yet decided.

### Hard architectural decisions → write an ADR

Write an ADR to `docs/adr/NNNN-slug.md` when a decision meets **all three**:

1. Hard to reverse (would require significant rework to undo)
2. Surprising without context (a future reader would not obvious understand why this choice was made)
3. Had genuine tradeoffs (alternatives were seriously considered)

Do **not** write an ADR for: obvious choices, decisions fully captured by the Allium spec, or implementation details.

**ADR format** (keep it short — one tight paragraph per section is enough):

```markdown
# NNNN: <Decision title>

## Status
Accepted

## Context
<Why this decision was needed. What constraint or requirement forced it.>

## Decision
<What was decided, in one sentence.>

## Consequences
<What this makes easier. What it makes harder or impossible.>
```

Number ADRs sequentially (`0001`, `0002`, …). Check existing files in `docs/adr/` for the next number. Create the directory with `mkdir -p docs/adr/` if it doesn't exist — do this silently without asking.
