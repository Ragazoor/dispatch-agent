---
description: "Ralph loop that runs allium weed to find undocumented behavior, updates the spec, and asks about code bugs"
allowed-tools: ["Read", "Skill"]
---

# Allium Weed Loop

This skill starts a ralph loop that iteratively aligns the allium spec with the implementation.

## Instructions

1. **Read the prompt file** at `.claude/skills/allium-weed-loop/prompt.md` to get the full loop prompt.

2. **Start the ralph loop** by invoking the `ralph-loop:ralph-loop` skill with the prompt content from the file, plus these options:
   - `--completion-promise 'SPEC ALIGNED'`
   - `--max-iterations 10`

   Pass the full prompt text as the first argument, followed by the options.
