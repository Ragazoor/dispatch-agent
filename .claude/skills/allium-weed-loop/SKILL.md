---
name: allium-weed-loop
description: >-
  Ralph loop that runs allium weed to find undocumented behaviour, updates the
  specs to match, and asks before touching code bugs. Use when the specs in
  docs/specs/ have drifted behind the implementation and the gap is too wide to
  close in one pass — after a large feature landed, or when a weed run returns
  more findings than one session can absorb. Requires the ralph-loop plugin.
allowed-tools: ["Read", "Write", "Bash"]
---

# Allium Weed Loop

This skill starts a ralph loop that iteratively aligns the Allium specs with the implementation.

**Requires the `ralph-loop` plugin.** The loop state file this skill writes
(`.claude/ralph-loop.local.md`) and the `<promise>` tag its prompt emits are
that plugin's mechanism. Without it installed, this skill writes a file and
nothing iterates. Check before starting; if it is missing, tell the user rather
than running a single pass and calling it a loop.

## Instructions

1. **Read the prompt file** at `.claude/skills/allium-weed-loop/prompt.md`.

2. **Create the ralph loop state file** directly at `.claude/ralph-loop.local.md` using the Write tool. Use this exact format, substituting the prompt content from step 1:

```markdown
---
active: true
iteration: 1
session_id: SESSION_ID
max_iterations: 10
completion_promise: "SPEC ALIGNED"
started_at: "TIMESTAMP"
---

[PROMPT CONTENT FROM prompt.md]
```

Get the session ID by running `echo $CLAUDE_CODE_SESSION_ID` and the timestamp with `date -u +%Y-%m-%dT%H:%M:%SZ`.

3. **Tell the user** the ralph loop is active, then start working on the prompt immediately.
