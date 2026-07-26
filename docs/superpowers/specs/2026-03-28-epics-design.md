# Epics — Design Spec

Epics group related tasks under a high-level plan. An epic describes an entire body of work and loosely enumerates the parts that need doing. A decomposition skill walks through the epic plan and interactively creates a task (with its own detailed plan) for each part.

## Data Model

### Epic struct (`src/models.rs`)

```rust
pub struct Epic {
    pub id: EpicId,
    pub title: String,
    pub description: String,
    pub plan: String,          // high-level markdown plan
    pub repo_path: String,
    pub done: bool,            // only stored status — set explicitly by user
    pub created_at: DateTime<Utc>,
    pub updated_at: DateTime<Utc>,
}
```

`EpicId` is a newtype `EpicId(i64)`, same pattern as `TaskId`.

### Task extension

Add `epic_id: Option<EpicId>` to the existing `Task` struct. `None` means standalone task. Tasks with an `epic_id` are subtasks of that epic and are hidden from the top-level board view.

## Database Schema

Migration v3 (current is v2):

```sql
CREATE TABLE epics (
    id          INTEGER PRIMARY KEY,
    title       TEXT NOT NULL,
    description TEXT NOT NULL,
    plan        TEXT NOT NULL DEFAULT '',
    repo_path   TEXT NOT NULL,
    done        INTEGER NOT NULL DEFAULT 0,
    created_at  TEXT NOT NULL DEFAULT (datetime('now')),
    updated_at  TEXT NOT NULL DEFAULT (datetime('now'))
);

ALTER TABLE tasks ADD COLUMN epic_id INTEGER REFERENCES epics(id);
```

### TaskStore trait additions

- `create_epic(title, description, plan, repo_path) -> Epic`
- `get_epic(id) -> Option<Epic>`
- `list_epics() -> Vec<Epic>`
- `update_epic(id, title?, description?, plan?, done?) -> Epic`
- `delete_epic(id)` — cascades: deletes all subtasks belonging to the epic
- `list_tasks_for_epic(epic_id) -> Vec<Task>`

## Derived Epic Status

Epic status is computed from subtasks, never stored (except `done`):

```
epic_status(epic, subtasks) -> TaskStatus:
    if epic.done          → Done
    if subtasks is empty  → Backlog

    let statuses = subtasks.map(|t| t.status)

    if all(Done)                → Review     // ready for user to verify
    if any(Running|Review|Done) → Running    // work in progress
    if any(Ready)               → Ready      // decomposed, waiting to start
    else                        → Backlog    // all subtasks still in backlog
```

- `Done` is the only explicitly set state (via the `done` flag on Epic).
- Computed each render cycle. Subtask counts are small, so cost is negligible.
- `m/M` (move status) is a no-op on epic cards — their position is derived.
- Archiving an epic also archives all its subtasks.

## TUI State — ViewMode Enum

### New types (`src/tui/types.rs`)

```rust
pub(in crate::tui) struct BoardSelection {
    pub selected_column: usize,
    pub selected_row: [usize; COLUMN_COUNT],
}

pub(in crate::tui) enum ViewMode {
    Board(BoardSelection),
    Epic {
        epic_id: EpicId,
        selection: BoardSelection,
        saved_board: BoardSelection,
    },
}
```

### App state changes (`src/tui/mod.rs`)

- Remove `selected_column` and `selected_row` from App — they move into `ViewMode`.
- Add `view_mode: ViewMode`.
- Add `epics: Vec<Epic>`.
- Accessor methods `selection()` / `selection_mut()` return `&BoardSelection` / `&mut BoardSelection` from the active variant. All navigation code uses these.

### View transitions

- **Enter epic view:** press Enter on an epic card → `ViewMode::Epic { epic_id, selection: fresh, saved_board: current_board_selection }`.
- **Exit epic view:** press Esc → `ViewMode::Board(saved_board)`. Cursor restored.

### Task filtering

```rust
fn tasks_for_current_view(&self) -> Vec<&Task> {
    match &self.view_mode {
        ViewMode::Board { .. } => {
            self.tasks.iter().filter(|t| t.epic_id.is_none()).collect()
        }
        ViewMode::Epic { epic_id, .. } => {
            self.tasks.iter().filter(|t| t.epic_id == Some(*epic_id)).collect()
        }
    }
}
```

Board view shows only standalone tasks (plus epics as cards). Epic view shows only that epic's subtasks.

### ColumnItem enum

Columns contain a mix of epics and tasks. A `ColumnItem` enum resolves what the cursor is on:

```rust
enum ColumnItem<'a> {
    Task(&'a Task),
    Epic(&'a Epic),
}
```

Each column builds a `Vec<ColumnItem>` sorted by `created_at`. The selected row index maps into this vec. Input handling and rendering match on the variant to determine behavior and card style.

### New Message variants

- `EnterEpic(EpicId)`, `ExitEpic`
- `CreateEpic`, `EpicCreated(Epic)`
- `EditEpic(EpicId)`, `EpicEdited(Epic)`
- `DeleteEpic(EpicId)`, `ConfirmDeleteEpic`
- `MarkEpicDone(EpicId)`
- `AddTaskToEpic { task_id, epic_id }`

### New Command variants

- `InsertEpic(Epic)`, `PersistEpic(Epic)`
- `DeleteEpic(EpicId)` — cascades to subtasks
- `RefreshEpicsFromDb`

## UI Rendering

### Board view

- `render_columns()` calls `tasks_for_current_view()` (excludes tasks with `epic_id`).
- Epics render as cards mixed into columns, positioned by derived status.
- Epic card style: purple border, `EPIC` label, title, colored status dots showing subtask breakdown (e.g., `●1 ●1 ●1` for done/running/ready).
- Column headers unchanged (Backlog/Ready/Running/Review/Done).

### Epic view

- `render_epic_banner()` — pinned banner at top showing: epic title, description snippet, subtask progress summary, "Esc to return".
- `render_columns()` — same function, operates on filtered subtasks.
- No epic cards in this view — only subtasks.

### Detail panel

- Board view, cursor on task → task detail (as today).
- Board view, cursor on epic → enters epic view (Enter triggers `EnterEpic`, not `ToggleDetail`).
- Epic view, cursor on subtask → task detail (as today).
- Epic banner always shows epic info, so no separate epic detail panel needed.

### Status bar keybind hints

- Board view: adds `E: new epic`.
- Epic view: `n: add subtask | d: dispatch | e: edit epic | Esc: back`.

## Input Handling

### Board view key additions

| Key | Action |
|-----|--------|
| `E` (shift) | Create new epic (epic creation input mode) |
| `Enter` | On task → toggle detail. On epic → `EnterEpic` |
| `x` | On task → archive. On epic → archive epic |
| `D` | On epic → delete epic (with confirmation) |

### Epic view keys

| Key | Action |
|-----|--------|
| `h/j/k/l` | Navigate columns/rows (operates on subtasks) |
| `n` | Create new subtask (auto-sets `epic_id`) |
| `d` | Dispatch selected subtask |
| `e` | Edit the epic (title, description, plan) |
| `Enter` | Toggle detail panel for selected subtask |
| `m/M` | Move subtask status forward/backward |
| `x` | Archive subtask |
| `Esc` | Exit epic view, return to board with saved selection |

### Epic creation input mode

Same flow as task creation (title → description → repo path) but uses a separate `EpicDraft` struct and produces an `InsertEpic` command.

## Decomposition Skill

A Claude Code skill that reads an epic's high-level plan and interactively creates subtasks with detailed plans.

### Invocation

- **Terminal:** `/decompose-epic <epic_id>` — reads epic from DB via MCP, walks through plan items in the conversation.
- **TUI:** keybind (e.g., `s` on an epic in board view) dispatches a Claude agent into a tmux window that runs the skill.

### Flow

1. Fetch epic via MCP `get_epic(id)`.
2. Parse the high-level plan into sections/items.
3. For each item:
   - Present proposed task: title, description, detailed plan.
   - Wait for user to approve, edit, or skip.
   - On approve: call MCP `create_task(title, description, repo_path, plan, epic_id)`.
4. After all items processed, summarize what was created.

### MCP changes

- Add `epic_id` optional parameter to `create_task` tool.
- Add new tools: `create_epic`, `get_epic`, `update_epic`, `list_epics`.

### Skill location

`.claude/skills/decompose-epic/SKILL.md`

## Worktree & Merge Strategy

- Each subtask gets its own independent worktree (same as standalone tasks today).
- Subtasks merge to main individually — no epic branch.
- The epic's derived status tracks overall progress.
- When all subtasks reach Done, the epic moves to Review automatically.
- User marks epic Done after verifying the complete body of work.
