# Keybinding pruning — implementation plan (task #3809)

Decisions taken by the user on 2026-07-31, from the analysis in
[3809-keybinding-inventory-and-pruning.md](3809-keybinding-inventory-and-pruning.md):

- Remove tier 1: `W` (board wrap-up), the tips popup, the learnings overlay, `T` (detach tmux),
  `S` (swap into split pane).
- Also remove `C` — both the keybinding **and** the managed-feed-config popup overlay.
- Rewrite the help overlay to match the current keymap.
- Instrument the uninstrumented key surfaces in this task.

Eight work packages, §1–§8 below. Each is a subtask of the tracking epic and cites this file.

## Ground rules for every package

1. **Order: spec → tests → code.** Update the Allium spec with `allium:tend` first, then express the
   removal as failing/updated tests, then delete the code, then `allium:weed` to confirm alignment.
2. **Learning #88 — a keybinding touches ~6 surfaces.** For each key removed, sweep *all* of:
   - the input handler (`src/tui/input.rs`, `src/tui/input/normal.rs`, `src/tui/input/confirm.rs`)
   - the owning Allium spec in `docs/specs/`
   - **both** footer hint builders: `action_hints` (`src/tui/ui/kanban/mod.rs:452`) and
     `epic_action_hints` (`src/tui/ui/kanban/mod.rs:537`), plus `status_bar.rs` mode hints
   - the help popup (`src/tui/ui/kanban/popups/help.rs`) — §7 rewrites it, but earlier packages must
     still not leave it referencing a key they deleted
   - `docs/reference.md` (the key-binding tables at lines 5–63)
   - rendering-assertion tests and footer-bar snapshots (`src/tui/tests/snapshots/`)
3. **Snapshot churn is expected.** Accept with `INSTA_UPDATE=always cargo test tui::tests::snapshots`,
   then **`rm src/tui/tests/snapshots/*.snap.new`** — a stray `.snap.new` silently contaminates the
   next review pass. Do not change the 120×40 `TestBackend` size.
4. **Add a "key does nothing" test per removed key.** Per learning #114 this needs only `make_app()` —
   no column navigation or task setup, because the key has no match arm at all. Assert
   `app.handle_key(key) == vec![]` and that `app.input.mode` is unchanged.
5. **Do not remove the MCP-side capability.** Agents drive wrap-up, learnings and feed config through
   MCP; only the *human TUI surface* is being deleted. Any package that breaks an MCP handler or a
   service method has overreached.
6. Verify with `cargo fmt --check && cargo test && ./scripts/check-doc-paths.sh`. Inline test modules
   need `#[allow(clippy::unwrap_used, clippy::expect_used)]`, and the pre-push hook applies
   `-D warnings`, so run `cargo clippy --all-targets -- -D warnings` before declaring done.

Sequence the packages §1 → §8 rather than in parallel: every one of §1–§6 edits the same two footer
hint builders and the same footer snapshots, so concurrent worktrees would conflict on every file.
§7 must land after §1–§6 (it rewrites the help text they each prune), and §8 last (it touches the
handlers the others delete).

## §1 — Remove `W`, the board wrap-up entry point

**Keep**: the `wrap_up` MCP tool (`src/mcp/handlers/tasks/wrap_up.rs`, 614 agent calls), the
`wrap_up_mode` column and `WrapUpMode` model, `src/dispatch/finish.rs`, the `/wrap-up` skill, and the
`InputWrapUpMode` picker **if** it is reachable from the agent-driven path — check
`src/tui/update/wrap_up.rs` before deleting, and keep whatever the MCP flow drives.

**Delete**: the `KeyCode::Char('W')` arm (`src/tui/input/normal.rs:374`); the `wrap_up` `key_event`;
`InputMode::ConfirmWrapUp(TaskId)` (`src/tui/types.rs:288`) and `ConfirmEpicWrapUp(EpicId)`
(`src/tui/types.rs:295`) with their `handle_key_confirm_wrap_up` / `handle_key_confirm_epic_wrap_up`
handlers (`src/tui/input/confirm.rs`); the `[W] wrap up` hints in `action_hints` (Review arm) and
`epic_action_hints`; the `status_bar.rs` "Wrap up: [r] rebase…" and "Epic wrap up:…" hint lines
(only those unreachable once `W` is gone); the help line; `docs/reference.md:27`.

**Spec**: `docs/specs/pr-workflow.allium` — remove the board-initiated wrap-up trigger, leave the
agent-driven `wrap_up` contract intact. Touch `epics.allium` for the epic variant.

**Tests**: `W` is inert; the MCP `wrap_up` handler still works end-to-end (`tests/lifecycle.rs`);
footer no longer advertises `W` for a Review task with a worktree.

## §2 — Remove the tips popup

The largest deletion. Opened at startup, dismissed 81×, browsed once, mode never changed.

**Delete**: `src/tips.rs` and the whole `src/tips/` markdown directory (17 files);
`src/tui/ui/kanban/popups/tips.rs`; `src/tui/messages/tips.rs`; `src/tui/commands/tips.rs`;
`src/tui/update/tips_projects.rs` (**check first** — if it also owns project/repo state, split it
rather than delete it); `handle_key_tips` (`src/tui/input.rs:120`) and its five `key_event` actions;
`TipsOverlayState` (`src/tui/types.rs:19`), `Message::Tips`, `Command::Tips`, `InputMode` entries and
the `interaction.tips` field; `TipsShowMode` (`src/models/tasks.rs`, `src/models/string_enum.rs`);
`exec_save_tips_state` (`src/runtime/settings.rs:72`) and `save_tips_state` / the load path in
`src/db/queries/settings.rs`; the startup show decision in `src/runtime/mod.rs` and `src/setup/mod.rs`;
`docs/specs/tips.allium` (delete the file) and the tips config in `docs/specs/core.allium`.

**Migration**: add a new forward migration that drops `tips_state`. Do **not** edit
`migrate_v36_tips_state` (`src/db/migrations.rs:975`) — historical migrations must keep replaying
for existing DBs. Confirm the migration-count test and any `MIGRATIONS` length assertion are updated.

**Tests**: startup emits no tips overlay and no `save_tips_state` call; `q`/`Esc`/`h`/`l`/`n`/`x` at
startup reach the board handler instead of a popup; a DB opened at v36 still migrates forward.

## §3 — Remove the learnings TUI overlay

Overlay opened 5× ever; `edit_learning`, `reject_learning`, `archive_learning` have **zero** recorded
uses. Agents curate via MCP (`record_learning` 388, `rate_learning` 796, `query_learnings` 155).

**Keep**: `src/service/learnings.rs`, `src/mcp/handlers/learnings.rs`, `src/models/learnings.rs`,
`src/runtime/learnings.rs` (the parts the MCP handlers and dispatch-time injection need), and the
learning-injection path in `src/dispatch/prompts.rs`.

**Delete**: the `KeyCode::Char('I')` arm (`src/tui/input/normal.rs:514`); `handle_key_learnings`
(`src/tui/input/normal.rs:33`) and `selected_learning_id_from_tree`; `ViewMode::Learnings`;
`LearningsView` (`src/tui/types.rs:115`); `src/tui/ui/learnings.rs`; `src/tui/messages/learnings.rs`
and `LearningMessage`; `src/tui/update/learnings.rs`; the `[I] learnings` footer hint; the help line;
`docs/reference.md` entry. Check whether `TreeNav` / `apply_tree_nav` (`src/tui/types.rs:133`) is
still used by the move-to-epic tree picker — it is shared, so keep it if so.

**Spec**: `docs/specs/learnings.allium` — remove the TUI browsing/curation surface, keep every MCP
rule and the dispatch-time retrieval/ranking behaviour.

**Tests**: `I` is inert; `Tab` on the board no longer toggles a learnings view; MCP
record/rate/query/delete still pass; dispatch-time injection snapshot unchanged
(`src/dispatch/snapshots/`).

## §4 — Remove `T`, detach tmux pane

4 presses ever, last 2026-05-28.

**Delete**: the `KeyCode::Char('T')` arm (`src/tui/input/normal.rs:567`) and `handle_key_detach`;
`InputMode::ConfirmDetachTmux(Vec<TaskId>)` (`src/tui/types.rs:289`) and
`handle_key_confirm_detach_tmux` (`src/tui/input/confirm.rs`); the detach `Message`/`Command` pair
(`src/tui/messages/task.rs`, `src/tui/commands/task.rs`) and its runtime executor — but only if
nothing else drives detach; the `[T] detach` hint in the `action_hints` Review arm; the
`status_bar.rs` confirm hint; the help line (help.rs:122) and `docs/reference.md`.

**Spec**: `docs/specs/split-pane.allium` — remove the tmux-detach surface.

**Tests**: `T` inert; a Review task with a live window shows no `[T]` hint.

## §5 — Remove `S`, swap task into split pane

10 presses ever, last 2026-05-31. `s` (toggle split) stays.

**Delete**: the `KeyCode::Char('S')` arm (`src/tui/input/normal.rs:563`) and `handle_key_swap_split`
(`src/tui/input/normal.rs:607`); `SplitMessage::Swap` (`src/tui/messages/split.rs`) and its
`src/tui/update/split_pane.rs` handler; the `SplitCommand`/`src/runtime/split.rs` swap executor;
pinned-task swap bookkeeping *only where it is swap-specific* — `split.pinned_task_id` is still read
by `handle_key_activate` (`src/tui/input.rs:294`) for the focus-the-pane priority, so **keep the pin
field and that priority branch**; the help line (help.rs:129).

Also fix the split-active badge: `status_bar.rs:246` renders `[S]plit`, which will advertise the
removed key. Change it to `[s]plit`.

**Spec**: `docs/specs/split-pane.allium` — remove the swap/pin-in-place surface, keep pane focus,
jump-to-agent and the focus border.

**Tests**: `S` inert both in and out of split mode (it previously emitted a "Split view not active"
hint outside split mode — assert that hint is gone); `s` still toggles; `Space` on a pinned task
still focuses the pane.

## §6 — Remove `C` and the managed-feed-config popup

1 press ever. **Keep** `get_managed_feed_config` / `set_managed_feed_config` MCP tools
(`src/mcp/handlers/managed_feeds.rs`) — they become the only configuration path, so verify they
cover every field the popup exposed and note any gap on the task before deleting the popup.

**Delete**: the `KeyCode::Char('C')` arm (`src/tui/input/normal.rs:545`);
`src/tui/input/managed_feeds.rs`; `src/tui/ui/kanban/popups/managed_feeds.rs`;
`src/tui/messages/managed_feeds.rs` and `ManagedFeedConfigMessage`;
`src/tui/update/managed_feeds.rs`; `InputMode::ManagedFeedConfig` (`src/tui/types.rs:331`) and
`ManagedFeedConfigState` (`src/tui/types.rs:527`); the `status_bar.rs` hint; the help line
(help.rs:141); `docs/reference.md`.

**Spec**: `docs/specs/dispatch.allium` / `docs/specs/epics.allium` — remove the TUI config surface,
keep the MCP config contract and the feed pipeline in `feeds.allium`.

**Tests**: `C` inert; MCP get/set round-trip still passes; the managed-feed epics still upsert.

## §7 — Rewrite the help overlay

`src/tui/ui/kanban/popups/help.rs` currently teaches `[d] dispatch` with a four-line explainer — a key
retired on 2026-07-25 — and omits `F`, the single most-pressed action key.

**Do**: delete the `[d]` block and the `* [d] is context-dependent:` explainer, folding the
context-dependence into the `Space` line; remove every line for `W`/`T`/`S`/`I`/`C` (deleted in
§1–§6); add `F` flat, `t` add-todo, `U` epic auto-dispatch, `R` group-by-repo, `r` refresh feed,
`v` select; keep the `Prefix+Space` / `Prefix+e` tmux lines. Re-check the popup fits the clamped
25–36 row height after the edits.

**Tests**: a rendering assertion that the help text contains no `[d]`, `[W]`, `[T]`, `[S]`, `[I]` or
`[C]`, and does contain `[F]` and `[Space]` — targeted `contains` checks, not a snapshot, so that a
future deletion reads as a regression. Update `docs/reference.md` to match in the same commit.

## §8 — Instrument the uninstrumented key surfaces

Today only ~46 action arms push `key_event`. Add one push per arm so the next pruning pass has data:

- **Board navigation** (`src/tui/input/normal.rs:327-332`, `430-436`): `navigate_column`,
  `navigate_row`, `navigate_row_first`, `navigate_row_last`, plus the completed `gg` chord and `q`
  (`quit` / `exit_epic`) and `Esc` (`clear_selection`).
- **Task detail** (`src/tui/input.rs:191`): `scroll_detail`, `zoom_detail`, `close_detail`.
- **Archive column** (`src/tui/input.rs:222`): `delete_archived`, `edit_archived`, `leave_archive`.
- **TODO overlay** (`src/tui/input/normal.rs:125`): all 12 in-overlay actions — this is the data that
  decides the deferred tier-2 call on the whole subsystem.
- **Confirm dialogs** (`src/tui/input/confirm.rs`): a `confirm_<mode>_yes` / `_no` pair each.
- **Pickers**: search mode, tag picker, quick-dispatch picker, repo-filter toggles/presets.

Use `dispatch_keyed` / `dispatch_handler_keyed` (`src/tui/input/normal.rs:278`, `:289`) where the arm
already dispatches a `Message` — they exist precisely so an arm cannot forget the telemetry push.
Navigation arms fire on every keypress, so check `record_usage_event_with_cap` /
`UsageCap::default()` (`src/db/mod.rs:583`) actually bounds the row growth before adding the
highest-frequency keys; if it does not, cap or sample them.

**Tests**: for each newly instrumented arm, assert the returned `Vec<Command>` contains the expected
`Command::RecordUsageEvent`. A no-op arm must **not** emit one (the `dispatch_handler_keyed`
empty-commands rule).

## Deferred — not in scope

Tier 2 from the analysis stays open pending a month of data from §8: the TODO overlay (`P`/`t`),
multi-select (`v`/`a`), `x` (move-to-Done/archive, last pressed 2026-05-28), and the `m`
reparent-epic half. Tier 3 (`p`, `f`, `R`, `A`, `E`, `r`, `J`) is explicitly **kept** — those show
healthy multi-month use despite a quiet last week.
