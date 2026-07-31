---
name: retro
description: Sub-step of the wrap-up skill, not a way to finish a task. The wrap-up skill invokes this automatically between wrap_up and exit_session — to complete, finish, wrap up, or end a task, always use the wrap-up skill, never this one. Only invoke retro directly when the user explicitly runs /retro or asks for a session retrospective. Captures what went well/could improve, checks whether this repo's CLAUDE.md or Allium specs are now stale, and opens follow-up tasks for anything actionable.
---

# Retro

A dispatched-task-scoped retrospective. This is dispatch's own version of a
session retro — narrower than a human's long-lived Claude Code session retro,
because a dispatched agent works in one isolated worktree and can only act on
what's in front of it: this task's session, this repo's docs, and the shared
knowledge base.

**Announce at start:** "I'm using the retro skill to run a session
retrospective."

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

## Step 3: Turn findings into follow-up tasks, not edits

For each **concrete, actionable** finding from Step 2 (or a bug noticed but
out of scope, or a worthwhile enhancement surfaced along the way), call
`create_task`:

- `title` — specific and actionable (e.g. "Update tasks.allium: X rule now
  says Y").
- `description` — reference this task's ID for traceability (e.g. "Found
  during task #123 — CLAUDE.md's module-map entry for `src/foo/` no longer
  matches after this session's refactor.").
- `tag` — `chore` for doc/spec drift, `bug` for a noticed-but-unfixed bug,
  `feature` for an enhancement idea.

`repo_path` and `epic_id` are inherited automatically from the caller — no
need to pass them explicitly unless overriding.

**Do not edit files yourself.** The follow-up task is what gets dispatched
later to make the actual change — this skill only identifies and records.

**Anti-patterns — do not create a task for:**
- A vague idea with no concrete next step.
- A one-off nit not worth a dedicated task.
- Something already tracked elsewhere (check before creating a duplicate).

Cap it to what's genuinely worth a task — most sessions will produce zero or
one, not several.

## Step 4: Output

Print a structured summary:

```markdown
## Session Retrospective

**Went well:**
- {bullet}

**Could improve:**
- {bullet}

**Docs/specs checked:** CLAUDE.md{, docs/specs/<relevant>.allium if applicable}
**Follow-up tasks created:** #<id> (<tag>: <title>){, or "none needed"}
```

This is the last step of the retro skill itself — it is not the end of the session. Retro is almost always invoked as a sub-step of `wrap-up`, between `wrap_up` and `exit_session`. After printing this summary, immediately resume the calling skill's next instruction (the closing `exit_session` call) in the same turn. Do not stop here.

## Relationship to other skills

- **`learnings`** still owns reusable pitfalls/conventions/preferences via
  `record_learning` — retro doesn't replace it. If Step 1 or Step 2 surfaces
  something learnings-shaped (a convention, a pitfall), record it there too.
- **`summarize`** still owns the behaviour-change recap for the user. Retro is
  about the session and the repo's docs, not what shipped.
- Retro is additive to both — run it in addition to, not instead of.
