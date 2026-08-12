---
name: retro
description: Sub-step of the wrap-up skill, not a way to finish a task. The wrap-up skill invokes this automatically before its commit step — to complete, finish, wrap up, or end a task, always use the wrap-up skill, never this one. Only invoke retro directly when the user explicitly runs /retro or asks for a session retrospective. Reflects on where this session lost time, fixes small agent-context drift in place so the next agent does better, and opens follow-up tasks only for what it must not fix itself.
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
| the repo's verify command | echoed in the `wrap_up` response (not in the dispatch prompt) |

The test is **"would the next agent do better?"** — not "is this statement
inaccurate?" Every finding must trace back to a concrete moment from Step 1. If
nothing in this session was made harder by it, it is not a finding, however true
it is.

Concretely: "`CLAUDE.md` claims there is a single DB connection; I designed
against that and had to back the design out" is a finding — it cost real time.
"A timing constant isn't listed in the constants table" is not — nobody was
slowed by its absence, and filing it buys the next agent nothing.

## Step 3: Fix it here, or file it

Fix it yourself, in this session, when **all** of these hold:

- the surface is agent-facing prose — `CLAUDE.md`, a page under `docs/` other
  than `docs/specs/` (Allium specs have their own, narrower carve-out below),
  or a skill under `plugin/skills/`;
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
- it is not small;
- this session is wrapping up with `action="done"` (see below).

Also file a `bug` for a concrete defect you noticed but could not fix in scope —
one with an observable wrong behaviour, not a suspicion.

**On the `done` path, file instead of fixing.** A fix you make here is carried by
wrap-up's commit step, which reaches `{base_branch}` on the rebase and PR paths
but never on `done` — that path runs no rebase and no push, so the worktree and
its commits are simply left behind. File the finding as a task instead, even
where the rules above would otherwise say "fix it here": a task survives the
worktree, an edit does not.

Wrap-up settles the action before invoking you, so you can always know which
path you are on — it is in the invocation, or in `wrap_up_mode` from `get_task`.
If you somehow reach this step without knowing, ask rather than assuming.

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

## Step 4: Output

Print a structured summary:

```markdown
## Session Retrospective

**Went well:**
- {bullet}

**Lost time on:**
- {bullet}

**Context fixed in this session:** {what you corrected in place per Step 3, and where}{, or "none needed"}
**Follow-up tasks created:** #<id> (<tag>: <title>){, or "none needed"}
```

This is the last step of the retro skill itself — it is not the end of the session. Retro is almost always invoked as a sub-step of `wrap-up`, just before that skill's commit step. After printing this summary, immediately resume the calling skill's next instruction (wrap-up's commit step) in the same turn. Do not stop here.

## Relationship to other skills

- **`learnings`** still owns reusable pitfalls/conventions/preferences via
  `record_learning` — retro doesn't replace it. If Step 1 or Step 2 surfaces
  something learnings-shaped (a convention, a pitfall), record it there too.
- **`summarize`** still owns the behaviour-change recap for the user. Retro is
  about the session and the repo's docs, not what shipped.
- Retro is additive to both — run it in addition to, not instead of.
