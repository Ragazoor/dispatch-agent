# Keybinding inventory & usage-driven pruning (task #3809)

Date: 2026-07-31. Metrics source: `query_usage(category="keybinding")` against the live
`~/.local/share/dispatch/tasks.db`, plus `category="mcp_tool"` for the agent-side comparison.

## 1. Reading the metrics correctly

Three caveats decide everything below:

1. **The keymap changed on 2026-07-25.** Commit `9010006d` ("unify dispatch and jump-to-agent
   onto Space") retired `d` (dispatch) and `g` (jump-to-tmux). Their all-time counts — `g` 3558,
   `d` 581 — are the *largest numbers in the table* and are entirely legacy. Any pruning decision
   must be read from the post-2026-07-26 window, which reflects the current keymap.
2. **Only ~46 actions are instrumented.** `key_event()` is pushed by the action arms only. All
   navigation (`h j k l` / arrows / `[` `]` / `gg` / `G`), `q`, `Esc`, every confirm dialog
   (`y`/`n`), the search-mode keys, the tag picker, the quick-dispatch picker, the repo-filter
   picker, the task-detail overlay (`z` zoom, `j`/`k` scroll) and **all** todo-overlay internals
   (`a e Space J K c d L U Tab BackTab Enter`) emit no telemetry. Absence of data ≠ absence of use
   for those.
3. **Single-user data.** n=1 human. "Zero in 5 days" is weak evidence on its own; combined with
   "last used 2 months ago" it is strong.

## 2. Full keybinding inventory

### 2.1 Board / epic view — normal mode (`src/tui/input/normal.rs:318`)

| Key | Action | Instrumented as | All-time | Since 07-26 | Last used |
|-----|--------|-----------------|---------:|------------:|-----------|
| `Space` | activate: focus pane → jump to window → dispatch/resume/retry | `jump_to_tmux` / `dispatch_task` / `resume_task` / `open_retry_dialog` | 770 / 42 / 4 / 2 | 446 / 42 / 4 / 2 | 07-31 |
| `Enter` | task detail (or cancel select-all) | `open_task_detail` | 342 | 71 | 07-31 |
| `n` | new task | `create_task` | 208 | 16 | 07-31 |
| `F` | toggle flattened | `toggle_flattened` | 1060 | 28 | 07-29 |
| `c` | copy task | `copy_task` | 27 | 19 | 07-29 |
| `:` | main session (jump / pick dir) | `open_main_session` | 111 | 18 | 07-31 |
| `K` / `J` | reorder item up / down | `reorder_task_up` / `reorder_task_down` | 40 / 19 | 15 / **0** | 07-29 / 06-23 |
| `D` | quick dispatch | `quick_dispatch` | 169 | 10 | 07-31 |
| `L` / `H` | move task/epic forward / backward | `move_task_forward` / `move_task_backward` | 122 / 136 | 7 / 3 | 07-29 |
| `/` | search titles | `search_tasks` | 23 | 7 | 07-29 |
| `U` | toggle epic auto-dispatch | `toggle_auto_dispatch` | 43 | 4 | 07-29 |
| `e` | edit task/epic in editor | `edit_task` | 118 | 1 | 07-31 |
| `s` | toggle split view | `toggle_split_mode` | 12 (+12 legacy `S`) | 2 | 07-29 |
| `N` | toggle notification panel | `toggle_notifications` | 6 | 2 | 07-29 |
| `m` | reparent epic / move task to epic | `reparent_epic` / `move_task_to_epic` | 6 / 11 | 0 / 1 | 06-12 / 07-26 |
| `p` | open PR/URL in browser | `open_pr_url` | 468 | **0** | 07-20 |
| `f` | filter by repo | `filter_repos` | 91 | **0** | 07-17 |
| `R` | toggle group-by-repo (in epic) | `toggle_group_by_repo` | 61 | **0** | 07-17 |
| `A` | filter: only active tmux | `filter_active` | 52 | **0** | 07-11 |
| `P` | open TODO overlay | `open_todos` | 47 | **0** | 07-16 |
| `v` | toggle select | `toggle_select` | 43 | **0** | 07-09 |
| `E` | new epic | `create_epic` | 24 | **0** | 07-09 |
| `r` | refresh feed epic | `refresh_feed` | 18 | **0** | 07-16 |
| `x` | move to Done / archive | `archive_task` | 16 | **0** | **05-28** |
| `a` | select all in column | `select_all` | 14 | **0** | 07-10 |
| `t` | quick-add selected item as todo | `todo_quick_add` | 11 | **0** | 07-16 |
| `S` | swap task into split pane | `swap_split_pane` | 10 | **0** | **05-31** |
| `?` | help overlay | `toggle_help` | 7 | **0** | 07-07 |
| `I` | learnings overlay | `open_learnings` | 5 | **0** | 07-09 |
| `T` | detach tmux pane | `detach_tmux` | 4 | **0** | **05-28** |
| `C` | managed feed config | `open_managed_feed_config` | 1 | **0** | 06-17 |
| `W` | wrap up (rebase / PR picker) | `wrap_up` | **1** | **0** | **05-22** |
| `h`/`l`/`←`/`→` | prev/next column | — | uninstrumented | | |
| `j`/`k`/`↓`/`↑` | prev/next task | — | uninstrumented | | |
| `[` / `gg` | jump to top of column | — | uninstrumented | | |
| `]` / `G` | jump to bottom of column | — | uninstrumented | | |
| `q` | quit (or exit epic view) | — | uninstrumented | | |
| `Esc` | clear selection / cancel | — | uninstrumented | | |

### 2.2 Overlays and modes

| Surface | Keys | Instrumented | Data |
|---|---|---|---|
| Task detail (`src/tui/input.rs:191`) | `j`/`k`/`↓`/`↑` scroll, `z` zoom, `q`/`Esc`/`Enter` close | none | — |
| Archive column (`src/tui/input.rs:222`) | `j`/`k` nav, `h`/`←`/`Esc` leave, `x` delete, `e` edit, `q` quit, `[`/`]` | none | — |
| Learnings (`src/tui/input/normal.rs:33`) | `Tab` list/tree, `e` edit, `x` reject, `A` archive, `j k l h` nav, `q`/`Esc` close | `toggle_learnings_view` 2 (last 07-09), `edit_learning` **0**, `reject_learning` **0**, `archive_learning` **0** | dead |
| TODO overlay (`src/tui/input/normal.rs:125`) | `a` add, `e` edit, `Space` done, `J`/`K` reorder, `c` clear done, `d` delete, `L` link, `U` unlink, `Tab`/`BackTab` nest, `Enter`/`g` jump to link, `q`/`Esc` | none (entry key `P` = 47, last 07-16) | — |
| Tips popup (`src/tui/input.rs:120`) | `l`/`→` next, `h`/`←` prev, `n` mode toggle, `x` disable, `q`/`Esc` close | `close_tips` 81, `browse_tips_next` **1**, `browse_tips_prev` **0**, `set_tips_mode` **0**, `disable_tips` **0** | dismissed, never read |
| Confirm dialogs (`src/tui/input/confirm.rs`) | 14 modes, `y`/`n`/`Esc` + `r`/`p`/`d` in wrap-up pickers | none | — |
| Repo filter (`src/tui/input/repo_filter.rs`) | `1-9` toggle, `a` all, `q`/`Esc`, preset save/delete | via `f` = 91 | — |
| Quick dispatch / tag / text fields | picker + caret keys (`Ctrl/Alt+←→`, `Home`/`End`, …) | via `D` = 169 | — |
| tmux global | `Prefix+Space` back to board, `Prefix+e` toggle agent tree | none | — |

### 2.3 Documentation drift found while inventorying

The help overlay (`src/tui/ui/kanban/popups/help.rs`) is **stale and wrong**:

- It still documents `[d] dispatch*` with a four-line explainer, plus `[Space] session/board`.
  `d` has not existed since 2026-07-25.
- It omits `F` (flatten — the *most-pressed* action key), `t`, `I`, `U`, `R`, `r`, `v`.
- `docs/reference.md:28` correctly describes the unified `Space`; the help popup contradicts it.

## 3. The agent-side comparison

Human keybinding presses since 07-26 total ~700, of which 446 are `Space`→jump-to-agent. Agent MCP
calls over the same DB: `exit_session` 823, `rate_learning` 796, `get_task` 757, `wrap_up` 614,
`update_task` 389, `record_learning` 388, `create_task` 272, `dispatch_next` 242,
`query_learnings` 155.

This is the central finding: **the board is a launcher and a window switcher; the workflow itself
lives in the agents.** Every human-facing feature that duplicates an agent-driven MCP path is
where the dead weight sits — wrap-up, learnings curation, task creation via epics.

## 4. Pruning recommendations

### Tier 1 — remove; the data is unambiguous

| # | Feature | Evidence | Blast radius |
|---|---|---|---|
| 1 | **`W` wrap-up from the board** — and its `WrapUpMode` picker, `ConfirmWrapUp`, `ConfirmEpicWrapUp` modes | 1 press ever, last 2026-05-22. Agents call the `wrap_up` MCP tool 614×. | `input.rs` handler, 3 `InputMode` variants, 3 confirm handlers, 2 footer hints, help line, `pr-workflow.allium` |
| 2 | **Tips popup** (whole feature) | Opened at startup, dismissed 81×, browsed 1×, mode never changed, never disabled. It is pure startup friction. | `handle_key_tips`, `TipsShowMode`, tips table + migration, `tips.allium`, popup renderer |
| 3 | **Learnings overlay** (`I`, `Tab`, and the `e`/`x`/`A` curation actions) | Overlay opened 5× ever; every *mutating* action inside it has **zero** recorded uses. Agents curate via MCP (`record_learning` 388, `rate_learning` 796). | `ViewMode::Learnings`, tree view + `tui_tree_widget` use, `handle_key_learnings`, learnings popup renderer, footer hint, `learnings.allium` (TUI surface only) |
| 4 | **`T` detach tmux pane** + `ConfirmDetachTmux` | 4 presses ever, last 2026-05-28. | handler, confirm mode, footer hint, help line, `split-pane.allium` |
| 5 | **`S` swap task into split pane** | 10 presses ever, last 2026-05-31. `s` (open split) survives with 2 presses in the last 5 days. | handler, `SplitMessage::Swap`, pin logic, footer badge, help line, `split-pane.allium` |

### Tier 2 — remove if you want the bigger simplification, but each has a real cost

| # | Feature | Argument for | Argument against |
|---|---|---|---|
| 6 | **TODO overlay** (`P`, `t`, and 12 in-overlay keys) | 47 opens over 2.5 months, none in the last 2 weeks; a whole subsystem (`todo.allium`, `TodoCommand`, link/nest, 3 input modes, delete confirm) for a checklist. | Its internals are uninstrumented, so we only know it stopped being *opened*. Deleting drops user data. |
| 7 | **Multi-select** (`v`, `a`, batch hints, batch archive/move) | `v` 43 / `a` 14, both dead since early July. Batch paths complicate archive, move, wrap-up and detach. | Removing it simplifies four handlers at once; but batch move (`L`/`H` over a selection) is genuinely handy on a busy board. |
| 8 | **`x` (move-to-Done / archive)** | Last pressed **2026-05-28** despite being in every footer. Tasks reach Done via `L` and agents. | It is the *only* keyboard path to archive a task. Removing it makes the Archive column write-only. Prefer: keep `x`, drop it from the footer. |
| 9 | **`m` reparent-epic half** (tree picker) | 6 presses, last 06-12. The task→epic half (11, last 07-26) is the useful one. | Shares the picker with move-to-epic, so the saving is one branch + `ConfirmReparentEpic`. |
| 10 | **`C` managed feed config** | 1 press ever. | Config surfaces are pressed once by design; MCP `set_managed_feed_config` exists as the alternative. Low cost to keep. |

### Tier 3 — do not remove despite zero presses in the last 5 days

`p` (468 all-time), `f` (91), `R` (61), `A` (52), `E` (24), `r` (18), `J` (pair with `K`), and all
uninstrumented navigation. The 5-day window is too short; these show healthy multi-month use.

### Tier 4 — fix, don't remove

11. **Rewrite the help overlay** to match the current keymap (drop `d`, add `F`/`t`/`I`/`U`/`R`/`r`),
    or delete it outright (`?` = 7 presses) and let the footer hint bar be the only help surface.
    Leaving it as-is is the worst option: it actively teaches a key that no longer exists.
12. **Instrument the gaps** — navigation, task-detail, todo-overlay internals, confirm dialogs. Without
    this, tier-2 decisions stay guesswork. One `key_event()` push per arm; the `dispatch_keyed`
    helper already makes it a one-liner.

## 5. Recommended scope for the follow-up work

Do tier 1 (items 1–5) plus item 11 as one epic: it deletes five features, three input modes, two
confirm handlers and a whole overlay, and fixes the one piece of documentation that is actively
misleading. Hold tier 2 until item 12 lands and gives a month of honest data on the
uninstrumented surfaces.

Per learning **#88**, each keybinding removal touches ~6 surfaces: input handler, Allium spec,
both footer hint builders (`action_hints` / `epic_action_hints` in
`src/tui/ui/kanban/mod.rs:452`), the help popup, `docs/reference.md`, plus rendering-assertion
tests and many footer-bar snapshots. Budget for the snapshot churn, and follow spec → tests → code.
