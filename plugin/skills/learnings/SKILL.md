---
name: learnings
description: Manage the knowledge base lifecycle — query, rate, and record entries. Use at wrap-up or whenever you want to contribute to the shared knowledge base.
---

# Knowledge Base

Use this skill to interact with the shared knowledge base — recording new entries and rating entries that were surfaced to you.

To *query* the knowledge base mid-task, call `query_learnings` directly (with `task_id` and an optional `tag_filter` as an array of tags, e.g. `["conventions", "rust"]`). Do that when anything is unclear — before guessing or asking.

**Announce at start:** "I'm using the learnings skill to interact with the knowledge base."

## Rating entries you acted on

When you act on a knowledge base entry that was surfaced to you (injected into your prompt or returned by `query_learnings`), give feedback right away:
```
rate_learning(learning_id=<id>, task_id=<your task id>, verdict="helped")
```

- `verdict="helped"` — the entry applied and was useful (upvotes it).
- `verdict="wrong"` — the entry misled you or is inaccurate (downvotes it; may go negative). Neither verdict changes the entry's status — there is no human review step. If it's clearly wrong rather than just unhelpful, delete it instead (see below).

Do this at the moment you act on it, not deferred to wrap-up. You can only rate entries that were surfaced to you this task.

**Rate `helped` when:** an entry saved you from a pitfall, matched a convention you applied, or guided a decision you made.

**Rate `wrong` when:** an entry was misleading or no longer accurate.

**Don't rate:** entries you read but didn't act on.

## Recording new entries

Before finishing a task, ask: *Did I discover anything non-obvious that a future agent would benefit from knowing?*

### Record if:

- The user expressed a **preference** explicitly that isn't already in CLAUDE.md
- You built a **landscape understanding** of a codebase area worth sharing
- You found a **convention** that applies broadly but isn't visible from reading the code
- A specific **workflow pattern** solved a cross-repo or cross-task problem elegantly
- This epic or project has a **procedural step** every agent working here should follow

### Do NOT record:

- Code patterns readable from source code — the code is self-documenting
- Things already in CLAUDE.md, README, or existing docs
- Git history — visible via `git log` / `git blame`
- Debugging solutions where the fix is in the commit
- Things too specific to generalise — if it won't apply to other tasks, skip it
- How you fixed a specific problem — that's in the code and commit message
- **A citation to a specific file, symbol, or line number** (`path.rs::symbol`, `Type::method`, a long snake_case function name). These rot silently — nothing re-checks the knowledge base the way `check-doc-symbols.sh` re-checks docs on every push, so a correct citation today can go stale forever without anyone noticing. `record_learning` rejects these outright. If the fact is worth citing precisely, put it in the Allium spec or a Rust doc comment (both gated, both re-checked on every push) and describe the *behavior* here in prose instead. A short reference to a stable MCP tool name (`query_learnings`, `wrap_up`, ...) is fine — that's a public interface, not the internal detail this rule exists to keep out.

  Bad: "A step that must behave identically on both feed paths goes in `src/feed/cycle.rs::run_feed_cycle`."
  Good: "Feed-cycle logic shared by the auto-poll and manual-refresh paths must live in one place, not be duplicated per caller — see feeds.allium."
- Generic language/library idioms that would apply to any codebase (e.g. "use an enum instead of a string sentinel," "clone the Arc once, not per branch") — if it's not tied to a specific type, module, or convention in *this* repo, it's not repo-scoped knowledge

### Picking a kind

| Kind | Use for |
|------|---------|
| `pitfall` | Silent failures, API traps, behaviour surprises — warn future agents |
| `convention` | Preferred patterns or style for this codebase |
| `preference` | Explicit user preference expressed during the task |
| `tool_recommendation` | Specific tool or library for a problem type |
| `procedural` | Step-by-step instructions to prefix dispatch prompts (epic-level) |
| `landscape` | Codebase/system overviews — service maps, module responsibilities |

### Picking a scope

| Scope | Use when | `scope_ref` |
|-------|----------|-------------|
| `user` | Personal workflow preference, applies to all work | omit |
| `repo` | Codebase-wide convention or landscape entry | omit (auto-derived) |
| `epic` | Shared design decision for this epic only | omit (auto-derived) |
| `task` | One-off note; not auto-injected into future prompts | omit (auto-derived) |

**Default to `repo` for code conventions and `user` for workflow preferences.**

### Writing a good summary

- **One sentence only.** If you need two, the entry is too broad — split or drop it.
- **Name the specific thing.** Not "be careful with DB queries" but "TaskPatch double-Option means `Some(None)` clears a field, `None` leaves it unchanged."
- **Lead with the actionable insight.** What should a future agent do differently?
- **No file/symbol/line citations** — see "Do NOT record" above.

## Deleting stale entries

If a knowledge base entry is incorrect, outdated, or should be removed entirely, delete it:

```
delete_learning(learning_id=<id>)
```

This permanently removes the entry. Use `query_learnings` first to find the entry's ID if you only know its content.
