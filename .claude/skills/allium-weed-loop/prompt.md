# Allium Weed Loop

You are in a ralph loop that iteratively aligns the allium specification with the implementation.

## Each Iteration

### 1. Rebase from main

```bash
git fetch origin
```
Then:
```bash
git rebase origin/main
```

### 2. Run allium weed

Use the Agent tool with `subagent_type: "allium:weed"` to compare `docs/specs/dispatch.allium` against the implementation code. Ask it to run in **check** mode and report all divergences, focusing on **undocumented behavior** — code that does things not captured in the spec.

Prompt the weed agent with:
> Weed the dispatch spec at docs/specs/dispatch.allium against the implementation in src/. Run in check mode. Focus on finding undocumented behavior — code paths, state transitions, validation rules, or edge cases that exist in the implementation but are missing from the spec. Classify each finding as: spec bug (spec wrong, code correct), code bug (code wrong, spec correct), or undocumented behavior (code does something useful not in spec). Report all findings with file locations.

### 3. Process findings

For each finding from the weed agent:

- **Undocumented behavior** (code does something useful not in spec): Use the Agent tool with `subagent_type: "allium:tend"` to add the behavior to the spec. Prompt it with the specific behavior to add and where it was found.

- **Code bugs** (code contradicts spec): Do NOT fix automatically. Use AskUserQuestion to describe the bug and ask the user whether it should be fixed. Only fix if the user confirms.

- **Spec bugs** (spec is wrong, code is correct): Treat these the same as undocumented behavior — update the spec to match the code.

### 4. Commit changes

After processing all findings, if any files were changed:

Stage only the changed files (spec files and any user-approved code fixes). Commit with a descriptive message like:

```
docs: align allium spec with implementation

- Added [specific behaviors] to dispatch.allium
- [Any code fixes if user-approved]
```

Do NOT commit files under `docs/plans/`.

### 5. Check completion

After the weed agent reports findings and you've processed them all:

- If there were findings and you made changes: try to exit (the loop will bring you back to check again)
- If the weed agent reports **no divergences found** (spec and code are fully aligned): output `<promise>SPEC ALIGNED</promise>`

## Important Rules

- Never skip the rebase step — you need the latest code each iteration.
- Never auto-fix code bugs — always ask the user first.
- Keep spec changes minimal and precise — only add what the code actually does.
- Each iteration should make incremental progress. Don't try to fix everything at once if there are many findings — pick the most important ones.
