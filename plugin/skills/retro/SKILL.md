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

## Step 1: Reflect

Write two short bullet lists grounded in what actually happened this session —
not a fixed questionnaire:

- **Went well** — approaches that saved time, tools/commands that worked
  cleanly, patterns worth repeating.
- **Could improve** — places you wasted time exploring, information you
  should have asked for earlier, anything you'd do differently next time.

Keep both lists to what's actually true of this session. An empty list
("nothing notable") is a fine answer — don't pad it.

## Step 2: Check for drift

Read the repo's root `CLAUDE.md`. If `docs/specs/*.allium` exists, read the
spec(s) relevant to the files this session touched (not the whole spec
directory).

Ask: does anything this session built make `CLAUDE.md` or a spec now stale or
wrong? Look for:

- A convention or file-path reference in `CLAUDE.md` that this session's
  changes made inaccurate.
- A behavior this session changed that a spec still describes the old way.

This is a check, not an edit — do not modify `CLAUDE.md` or any spec file
yourself here. Anything found becomes a follow-up task (Step 3), never a
direct edit from this skill.

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
