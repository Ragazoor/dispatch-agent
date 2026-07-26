# Agent File Tree Panel — Design Spec

**Date:** 2026-07-26
**Status:** Proposed
**Task:** #3722 "Improved agent window"

## Context

Today, following what a dispatched agent is doing means watching raw Claude Code terminal output scroll by — either by jumping to the agent's tmux window (Space) or joining it into a split pane next to the board. There is no structured view of *which files* the agent has touched, so keeping track of the scope of an agent's changes requires reading the whole transcript.

This feature adds a file-tree view, showing which files an agent has read/written and how (badge: Read vs. Modified), so a user can follow along at a glance instead of parsing terminal scrollback.

**Key framing decision, arrived at during brainstorming:** the "agent/task window" the user means is each dispatched task's own tmux window — where the actual `claude` CLI process runs — not dispatch's kanban-board TUI. The board process never renders this view; it lives entirely inside each agent's own tmux window, as a second pane the user can toggle.

---

## Architecture Overview

Two independent pieces, deliberately decoupled from the board TUI's own process/state model:

1. **Capture**: Claude Code's `PostToolUse` hook, for `Read`/`Write`/`Edit`/`NotebookEdit` tool calls, appends an event (task id, tool, file path, timestamp) to a new per-task JSONL file. This is additive to the existing hook wiring — no change to existing task-activity-classification behavior.
2. **Render**: each agent's own tmux window is split at creation time into two panes — pane 1 unchanged (the real `claude` CLI), pane 2 a new `dispatch agent-tree <task_id>` subcommand: a small standalone ratatui loop (not part of the board's `App`) that reads that task's JSONL file and renders a tree with Read/Modified badges. A global tmux keybinding toggles pane 2's visibility.

Neither piece touches the board TUI's state, DB refresh cycle, or `App` — the whole feature is invisible to the board process. This significantly simplifies the design relative to a board-rendered panel: no `dirty_since_refresh` plumbing, no new `InputMode`, no board-side rendering code at all.

---

## Capture Pipeline

### Storage: per-task JSONL, not a DB table

Decision, informed by a dedicated comparison pass (see the session's analysis — headline finding: short-lived `dispatch` CLI processes *already* open their own writer connection to the primary DB on every tool call today, via the existing `pre_tool_use` hook path, so "can a short-lived process write to the DB" was never the open question). The real considerations:

- SQLite does not offer true parallel writes at any architecture — WAL mode gives one writer at a time, database-wide. A "rearchitecture to allow parallel writes" was evaluated and **rejected**: it wouldn't add concurrency the engine can honor, it would add per-event connection-open + migration-check overhead, and it would jeopardize the existing single-writer's migration-safety guarantee and the `get_total_changes` watermark the board's refresh-skip logic depends on.
- The per-task JSONL file's safety rests on a structural fact, not just precedent-matching with `src/mcp/trajectory.rs`: **the file shard coincides exactly with the natural serialization boundary** — one task, one agent session, one strictly serial tool-call stream, so at most one process ever appends to a given task's file at a time.
- This data is purely observational (no cross-entity invariant, same category as `UsageStore::record_usage_event`), has no relational structure worth a schema, and sits on a path that synchronously blocks the agent's tool call — the append-only file is both the architecturally cleanest and the lowest-latency option.
- A DB table remains viable later with zero DB-layer changes (piggyback on the existing per-invocation connection) if cross-task queryability becomes an actual requirement — not needed now.

Location: `<data_dir>/file-events/<task_id>.jsonl`, one line per event: `{task_id, tool, path, operation, timestamp}` where `operation` is `read` for `Read` and `modified` for `Write`/`Edit`/`NotebookEdit`.

**Open item, not decided in this doc**: no retention/truncation policy exists yet for this file (nor does one exist for `trajectory.jsonl` today). A long-running agent session could generate a large number of events. Left as a follow-up rather than blocking this design.

### Hook changes — additive, not a replacement

`plugin/hooks/scripts/task-status-hook`'s existing `PreToolUse|PostToolUse` case arm calls `dispatch hook "$ID" pre_tool_use` on *both* events today — this is deliberate, documented behavior (`HookEventKind::PreToolUse`'s doc comment in `src/models/tasks.rs:642-647`) that collapses both hook firings into one task-activity signal. **That call is untouched.** A second, independent call is added, firing only on the `PostToolUse` half, only for tracked tools:

```bash
PreToolUse|PostToolUse)
    TOOL=$(echo "$INPUT" | jq -r '.tool_name // empty')
    [[ "$TOOL" == mcp__dispatch__* ]] && exit 0
    dispatch hook "$ID" pre_tool_use          # unchanged

    if [[ "$EVENT" == "PostToolUse" ]]; then
        case "$TOOL" in
            Read|Write|Edit)
                FILE_PATH=$(echo "$INPUT" | jq -r '.tool_input.file_path // empty') ;;
            NotebookEdit)
                FILE_PATH=$(echo "$INPUT" | jq -r '.tool_input.notebook_path // empty') ;;
            *) FILE_PATH="" ;;
        esac
        [[ -n "$FILE_PATH" ]] && dispatch hook-file-event "$ID" --tool "$TOOL" --path "$FILE_PATH"
    fi
    ;;
```

Note `NotebookEdit`'s path argument is named `notebook_path`, not `file_path` — a per-tool field mapping that must stay correct if Claude Code changes tool schemas.

### New Rust CLI surface — dedicated command, not an extension of `Hook`

A new `Commands::HookFileEvent { id: i64, tool: String, path: String }`, invoked as `dispatch hook-file-event <id> --tool <name> --path <path>`, deliberately **not** added to the existing `Commands::Hook`/`HookEventKind` — that enum and `record_hook_event` are narrowly scoped to task-activity/sub-status classification, and are extensively doc-commented specifically because their scope is deliberate. The new command's handler does not touch `TaskService`/`record_hook_event` at all; it only appends one line to the task's file-events JSONL. Zero risk to the existing activity-classification path.

**Known limitation**: whether Claude Code's `PostToolUse` payload reliably signals tool failure (e.g. an `Edit` whose `old_string` wasn't found) is unconfirmed as of this design. If it can't be generalized across tools cheaply, every captured `PostToolUse` is treated as a successful write — an accepted inaccuracy, not a blocker.

### The Bash gap

Tool calls that touch files only via `Bash` (`mv`, `sed -i`, `grep`, etc.) have no structured file path in `tool_input` and are **not** captured by this design. Per direction from this session: address via a new global rule file, not a hook block (yet). A new `~/.claude/rules/agent-file-tool-preference.md` (auto-discovered per Claude Code's official `.claude/rules/` convention — confirmed against current docs, not just this user's existing personal convention) nudges agents toward `Read`/`Write`/`Edit`/`Grep`/`Glob` over raw shell file operations. This follows the same pattern as this user's existing `worktree-scope.md`/`cross-repo-scope.md` — global, manually maintained, applies to every dispatched agent regardless of which repo it's working in. This is an edit outside the dispatch repo, explicitly authorized for this task. If this proves insufficient in practice, a blocking `PreToolUse` hook is the documented fallback — explicitly out of scope for this iteration.

---

## Companion Pane Mechanics

- **Where**: `src/dispatch/agents.rs`, at each place an agent's tmux window is created (`tmux::new_window` + `send_keys`, covering fresh dispatch, resume, and main-session paths — all three, for consistency). Right after the window is created, split it and start `dispatch agent-tree <task_id>` in the new pane.
- **Split sizing**: `split_window_horizontal` (`src/tmux.rs:278`) exists but is hardcoded to a 40% split (used today for the board's own split-pane feature) and doesn't run a command in the new pane. A new sibling function is needed: narrower (~25-30%, since Claude's own CLI output needs the room) and takes a command to run in the new pane — following the same "create + immediately run a command" shape `new_window_running` (`src/tmux.rs:61`) already establishes for window creation.
- **Toggle keybinding**: a new global tmux key binding (`bind_key`/`unbind_key`, `src/tmux.rs:252-261`), bound/unbound alongside the board TUI process's own lifecycle — matching the existing Space "jump to agent" binding exactly (`src/runtime/mod.rs:66-85`). Toggling is **kill-pane + re-split**, matching the board's own split-pane enter/exit mechanism (`kill_pane` + `split_window_horizontal` in `src/runtime/split.rs`) rather than a new `resize-pane -Z` zoom wrapper — proven pattern, at the cost of the companion process restarting (cheap — it just re-reads the JSONL file) on every toggle.
- **Scope boundary**: only newly-created agent windows get the companion pane; already-running tasks at the time this ships are not retrofitted.

## Render Content

- Tree rooted at the task's worktree.
- Each touched file gets a badge: `[Modified]` (Write/Edit/NotebookEdit) or `[Read]` (read-only) — Modified wins if both occurred.
- Directories containing touched files auto-expand; untouched parts of the tree stay collapsed, so the view doesn't become a full repo browser.
- Implementation reuses `tui-tree-widget` (already a dependency, used today by `src/tui/ui/learnings.rs`), redrawing on a timer as new lines land in the JSONL file.

---

## Allium Spec

New dedicated spec file: `docs/specs/agent-tree.allium`, covering the capture pipeline, the companion-pane lifecycle (creation, toggle, scope boundary), and the tree/badge rendering rules as a first-class domain surface — not folded into `observability.allium` (which explicitly scopes out hook events today) or `agent-health.allium` (which owns task-activity classification, a different concern).

---

## Testing Strategy

- **Hook script**: extend the existing inline bash-script tests in `src/setup/hooks.rs` (e.g. `hook_script_handles_all_events`) to assert `PostToolUse` for tracked tools forwards `--tool`/`--path` via the new command, and that the existing `pre_tool_use` activity-signal call is unchanged for both events and all tools.
- **New CLI command**: unit tests for JSONL append correctness; malformed/missing-path input is skipped, not a panic (matching the codebase's soft-fail-decoding convention).
- **Tree-building logic**: pure unit tests feeding synthetic event streams — badge precedence (Modified over Read), malformed-line skipping, directory auto-expand — no I/O mocking needed.
- **`dispatch agent-tree` rendering**: snapshot tests (`TestBackend`, same convention as the board's snapshot suite), fed synthetic events directly, bypassing tmux entirely.
- **tmux orchestration**: the new split call in `agents.rs`, and the toggle keybinding's bind/unbind lifecycle + kill/re-split sequence, tested via the existing `ProcessRunner`/mock-runner pattern already used to verify tmux command sequences without a real tmux.
- Per this repo's standing convention: Allium spec first, then tests, then code.

---

## Known Limitations / Open Items

1. Bash-driven file changes remain invisible unless the new global rule succeeds in practice; a blocking hook is the documented fallback, out of scope here.
2. No confirmed way (as of this design) to detect a failed tool call from `PostToolUse`'s payload — accepted inaccuracy.
3. No retention/truncation policy for the new JSONL file.
4. Toggle keybinding only works while the board TUI process is running (matches existing Space precedent).
5. Already-running agent windows at ship time do not get the companion pane retroactively.

## Out of Scope / Follow-ups Filed

- Task #3724: non-atomic `migrate_fn`/`user_version` bump race in `Database::init_schema_sync` (`src/db/mod.rs:954-959`), discovered as a side effect of the storage-comparison research. Unrelated to this feature; filed separately.

## Suggested Epic Breakdown

For decomposing into an epic with subtasks, roughly in dependency order:

1. **Capture pipeline** — hook script changes, new `HookFileEvent`-style CLI command, JSONL append/read helpers, tests. Spec: `agent-tree.allium` capture rules.
2. **Tree-building logic** — pure JSONL → tree-with-badges data structure, unit tested independent of rendering.
3. **`dispatch agent-tree` rendering subcommand** — ratatui loop + `tui-tree-widget`, snapshot tests.
4. **tmux orchestration** — window-split at agent creation (`agents.rs`, all three paths), new `split_window` sibling function with configurable size + command.
5. **Toggle keybinding** — global bind/unbound tied to board TUI lifecycle, kill+resplit toggle logic.
6. **Bash-avoidance rule** — author `~/.claude/rules/agent-file-tool-preference.md`.
7. **Allium spec authoring** — `docs/specs/agent-tree.allium`, via `allium:tend`, verified with `allium:weed` once 1-5 land.
