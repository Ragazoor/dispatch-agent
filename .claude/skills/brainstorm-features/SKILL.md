---
name: brainstorm-features
description: Generate feature ideas for the Dispatch TUI from three angles — UX, DevX and UI — then turn the user's favourites into backlog tasks. Use when the user asks to "brainstorm features", "brainstorm new features", "generate feature ideas", "what features should we add", "suggest improvements", "what should we build next", "feature backlog ideas", "ideate on features", or wants to explore UX, DevX, or UI improvements for the Dispatch TUI.
---

# Brainstorm Features

Generate feature ideas for the Dispatch TUI by exploring the codebase from three perspectives — UX, DevX, and UI — then iterate on user-selected favorites and create backlog tasks.

## Workflow

### Step 1: Gather Baseline Context

Read these files to understand current project state:
- `CLAUDE.md` — architecture, conventions, key files
- `TODOS.md` — known improvement areas

For detailed file pointers and stable domain context, consult `references/project-context.md`.

### Step 2: Spawn 3 Parallel Agents

Use the Agent tool to spawn 3 agents in a single message (parallel execution). Each agent explores the codebase from its category's perspective and returns a list of feature ideas.

**Each agent receives:**
- Instruction to read `CLAUDE.md` and `TODOS.md` for project context
- Its category name and description
- The specific source files to explore (listed below)
- Instruction to return exactly 3 ideas, ranked by value (impact on daily usage x feasibility), with a one-sentence justification for each ranking
- Each idea has: **Title**, **Problem** (what's missing or painful today), **Idea** (what the feature would do)
- Instruction to self-filter: explore broadly but only surface the highest-value ideas — prefer small changes that solve real pain over ambitious features
- Instruction that this is a research-only task — do not write any code or modify any files

**Agent prompts:**

#### UX Agent
```
Brainstorm UX feature ideas for the Dispatch TUI.

Read CLAUDE.md and TODOS.md for project context, then explore these files:
- src/tui/input.rs (keyboard handling, keybindings)
- src/tui/mod.rs (app state, input modes, message handlers)
- src/tui/types.rs (Message, Command, InputMode enums)
- src/editor.rs (external editor integration)

Focus on: keybindings, navigation, input modes, task creation flow, confirmation dialogs, discoverability, keyboard shortcuts, modal interactions.

Explore broadly, then self-filter to exactly 3 ideas ranked by value (impact on daily usage x feasibility). Prefer small changes that solve real pain over ambitious features. For each idea provide:
- **Title** — short name
- **Problem** — what's missing or painful today
- **Idea** — what the feature would do
- **Why this ranks here** — one sentence on impact x feasibility

Research only — do not write code or modify files.
```

#### DevX Agent
```
Brainstorm DevX feature ideas for the Dispatch TUI.

Read CLAUDE.md and TODOS.md for project context, then explore these files:
- src/dispatch.rs (agent dispatch, worktree creation, tmux)
- src/mcp/handlers.rs (MCP tool implementations)
- src/mcp/mod.rs (MCP server setup)
- src/runtime.rs (TUI main loop, command execution)
- src/plan.rs (plan file parsing)
- src/editor.rs (external editor integration)

Focus on: agent dispatch workflow, MCP tools, CLI integration, plan-to-task pipeline, developer productivity, agent monitoring, task lifecycle.

Explore broadly, then self-filter to exactly 3 ideas ranked by value (impact on daily usage x feasibility). Prefer small changes that solve real pain over ambitious features. For each idea provide:
- **Title** — short name
- **Problem** — what's missing or painful today
- **Idea** — what the feature would do
- **Why this ranks here** — one sentence on impact x feasibility

Research only — do not write code or modify files.
```

#### UI Agent
```
Brainstorm UI feature ideas for the Dispatch TUI.

Read CLAUDE.md and TODOS.md for project context, then explore these files:
- src/tui/ui.rs (Ratatui rendering, columns, detail panel, status bar)
- src/models.rs (Task, TaskStatus, Note structs)
- src/tui/types.rs (enums for display state)

Focus on: layout improvements, information density, visual hierarchy, task card content, column rendering, detail panel, status bar, color usage, responsive layout, progress indicators.

Explore broadly, then self-filter to exactly 3 ideas ranked by value (impact on daily usage x feasibility). Prefer small changes that solve real pain over ambitious features. For each idea provide:
- **Title** — short name
- **Problem** — what's missing or painful today
- **Idea** — what the feature would do
- **Why this ranks here** — one sentence on impact x feasibility

Research only — do not write code or modify files.
```

### Step 3: Collect & Present Batch

Present the 9 ideas (3 per category) as a compact ranked list. Each entry: **Title** — one-line summary. Group by category.

Ask the user: "Which ideas would you like to turn into backlog tasks? Pick by number, name, or say 'all'."

### Step 4: Refine Selected Ideas

For each idea the user selected, write a concise task description (title + 2-3 sentence description) suitable for a backlog task. Ask the user to confirm or adjust before creating. Keep it brief — don't over-ask clarifying questions. If anything is ambiguous, pick a sensible default and note it.

### Step 5: Create Backlog Tasks

For each approved idea, create a task in the backlog:

**Primary — MCP `create_task`:**
Use the `dispatch` MCP server tool `create_task` with:
- `title`: the feature title
- `description`: the refined description from Step 4
- `repo_path`: the current working directory (resolve to the main repo root, not a worktree)

If MCP is unavailable, surface that to the user — task creation is
exclusively via the MCP `create_task` tool; there is no CLI fallback.

Report the created task IDs to the user.

## Key Constraints

- Each agent explores independently — do not share context between category agents
- Ideas should be concrete and actionable, not vague wishes
- Task descriptions should be detailed enough that a developer unfamiliar with the codebase can understand the intent
- Repo path for task creation should point to the main repo root, not a worktree path
