# TUI Scheduled-Task Create/Edit — Implementation Plan

> **For agentic workers:** Use TDD throughout — write the failing test before the code that makes it pass. Update `docs/specs/dispatch.allium` and `docs/specs/tasks.allium` via `allium:tend` as part of this work, then verify with `allium:weed`.

**Goal:** Let a user set `schedule_interval_secs` and `pinned_branch` on a task from the TUI — both at creation and via the existing task editor — and see a visual indicator on a scheduled task's card.

**Architecture:** Reuses two existing, already-tested mechanisms rather than inventing new ones: the manual task-creation `InputMode` chain (`src/tui/update/forms.rs`) gets two new optional steps, and the `$EDITOR`-based task editor (`src/editor.rs`) gets two new `--- SECTION ---` blocks — this is the **exact same mechanism** `Epic.feed_interval_secs` already uses (`editor.rs:143-153`/`:183-200`), so this plan is closely mirroring existing, tested code rather than designing from scratch.

**Tech Stack:** Rust, ratatui TUI, the existing `TextInputField`/single-key-picker input mechanics, the tmux-external-editor task-edit flow.

**Spec:** `docs/superpowers/specs/2026-08-16-staging-pipeline-scheduled-agents-design.md` (Part A's config-surface section — this plan is the "add TUI forms" alternative the design doc deferred; it depends on the sibling "generic scheduling primitive" task having landed `Task.schedule_interval_secs`/`Task.pinned_branch`/`TaskPatch` first).

## Global Constraints

- No printable character may become a navigation/selection shortcut for the new input steps (mirrors `RepoPathPicker`/`BaseBranchPicker`'s `NoPrintableShortcut` guarantee already in `dispatch.allium`) — free text goes through `TypeChar`, not letter shortcuts.
- Both new fields are **optional** — a task with neither set must behave exactly as every task does today. Skipping both steps (Enter on an empty field) must produce `None`, not an error.
- `schedule_interval_secs` must parse as a positive integer; a non-numeric or zero/negative entry must be rejected with an inline error, not silently coerced or accepted.
- Depends on: `Task.schedule_interval_secs: Option<i64>`, `Task.pinned_branch: Option<String>`, and the corresponding `TaskPatch`/`CreateTaskRequest` fields — from the sibling "generic scheduling primitive" plan/task. Do not start Task 1 below until those exist (`cargo build` will fail otherwise on the `TaskDraft`/patch-diff code).

---

## File Structure

- Modify `src/tui/types.rs` — `TaskDraft` struct (currently `title, description, repo_path, tag, base_branch, wrap_up_mode`) gains `schedule_interval_secs: Option<i64>` and `pinned_branch: Option<String>`; `InputMode` enum gains two new variants, e.g. `InputScheduleInterval`, `InputPinnedBranch`.
- Modify `src/tui/update/forms.rs` — two new `handle_submit_*` functions inserted between `handle_submit_wrap_up_mode` (line 142) and `finish_task_creation`'s call site (line 159).
- Modify `src/tui/input.rs` — key handling for the two new `InputMode` variants (plain `TextInputField` mechanics, no picker).
- Modify `src/tui/ui/input_form.rs` — rendering for the two new steps (mirror `input_wrap_up_mode_lines` at line 299).
- Modify `src/editor.rs` — `format_editor_content`/`EditorFields`/`parse_editor_content`/`apply_task_editor_fields` (lines 202, 65-88, 379, 304, 248-266) gain two new sections, directly mirroring `format_epic_for_editor`/`parse_epic_editor_output`'s existing `--- FEED_INTERVAL_SECS ---` handling (lines 143-153/183-200).
- Modify `src/tui/ui/kanban/cards.rs` — `render_card_indicator` (line 275/535) gains a small badge for a scheduled task (e.g. `[⏱ 10m]` or similar — keep it terse, one badge, no new `CardIndicator` variant needed if a synthetic label works; check `CardIndicator`'s definition first to decide which is less invasive).
- Test: `src/tui/tests/input_handlers.rs` (new tests alongside the existing `wrap_up_mode_*` tests at lines 2540-2679), `src/tui/tests/scenarios` (a full create-flow scenario test), `src/editor.rs`'s inline `mod tests` (new round-trip tests mirroring the epic feed-interval ones), `src/tui/tests/snapshots` (only if the new form steps change a rendered snapshot — run `cargo insta review` if so, per CLAUDE.md's snapshot workflow).

---

## Task 1: `TaskDraft` + `InputMode` fields

**Files:**
- Modify: `src/tui/types.rs` (`TaskDraft`, `InputMode`).
- Test: unit test on `TaskDraft::default()` (or however it's constructed) asserting the two new fields default to `None`.

- [ ] **Step 1: Write the failing test**

```rust
#[test]
fn task_draft_defaults_have_no_schedule_or_pinned_branch() {
    let draft = TaskDraft::default(); // or whatever constructor is used today
    assert_eq!(draft.schedule_interval_secs, None);
    assert_eq!(draft.pinned_branch, None);
}
```

- [ ] **Step 2: Run test, confirm it fails to compile.**

- [ ] **Step 3: Add the fields**

```rust
schedule_interval_secs: Option<i64>,
pinned_branch: Option<String>,
```

to `TaskDraft` (`types.rs:329-336`), and two variants to `InputMode` (`types.rs:230`):

```rust
InputScheduleInterval,
InputPinnedBranch,
```

- [ ] **Step 4: Run test, confirm it passes; `cargo build` to catch any other exhaustive-match compile errors on `InputMode` and fix them with a temporary `todo!("wired in Task 2/3")` ONLY if the compiler forces it — replace before this plan's final commit.**

- [ ] **Step 5: Commit**

```bash
git add src/tui/types.rs
git commit -m "feat(tui): add schedule_interval_secs/pinned_branch to TaskDraft"
```

---

## Task 2: Creation-flow steps

**Files:**
- Modify: `src/tui/update/forms.rs` (insert two new steps between `handle_submit_wrap_up_mode`, line 142, and `finish_task_creation`, line 159).
- Modify: `src/tui/input.rs` (key handling for the two new modes — plain text input, reusing `TextInputField` mechanics, no candidate-list picker).
- Modify: `src/tui/ui/input_form.rs` (render the two new steps — mirror `input_wrap_up_mode_lines` for layout, but these are free-text fields like `InputBaseBranch`, not a single-key picker).
- Test: `src/tui/tests/input_handlers.rs`, `src/tui/tests/scenarios`.

**Interfaces:**
- Consumes: `TaskDraft.schedule_interval_secs`/`pinned_branch` (Task 1); `handle_submit_wrap_up_mode`'s exact call-chain shape (read `forms.rs:142-159` before writing this, to match argument/return types precisely).
- Produces: `handle_submit_schedule_interval`, `handle_submit_pinned_branch` functions with the same signature shape as their neighbors.

- [ ] **Step 1: Write the failing scenario test — both fields skipped**

```rust
#[test]
fn creating_a_task_with_both_schedule_fields_skipped_leaves_them_none() {
    let mut app = test_app(); // existing harness
    // ... drive through InputTitle -> ... -> InputWrapUpMode -> InputScheduleInterval -> InputPinnedBranch
    app.handle_key(key_enter()); // skip schedule interval (empty buffer)
    app.handle_key(key_enter()); // skip pinned branch (empty buffer)
    // finish_task_creation should now have fired
    let created = app.last_created_task(); // however the harness inspects this
    assert_eq!(created.schedule_interval_secs, None);
    assert_eq!(created.pinned_branch, None);
}
```

- [ ] **Step 2: Run test, confirm it fails** (new InputMode steps don't exist in the chain yet — `handle_submit_wrap_up_mode` still calls `finish_task_creation` directly).

- [ ] **Step 3: Implement.** Change `handle_submit_wrap_up_mode`'s terminal call from `self.finish_task_creation(repo_path)` to entering `InputMode::InputScheduleInterval`. Add:

```rust
fn handle_submit_schedule_interval(&mut self) {
    let raw = self.input_buffer.trim();
    if raw.is_empty() {
        self.draft.schedule_interval_secs = None;
    } else {
        match raw.parse::<i64>() {
            Ok(n) if n > 0 => self.draft.schedule_interval_secs = Some(n),
            _ => {
                self.set_status_error("Schedule interval must be a positive number of seconds");
                return; // stay on this step
            }
        }
    }
    self.input_buffer.clear();
    self.mode = InputMode::InputPinnedBranch;
}

fn handle_submit_pinned_branch(&mut self) {
    let raw = self.input_buffer.trim();
    self.draft.pinned_branch = if raw.is_empty() { None } else { Some(raw.to_string()) };
    self.input_buffer.clear();
    self.finish_task_creation(repo_path); // same terminal call wrap_up_mode used to make
}
```

(Match the actual existing `self.input_buffer`/`self.draft`/`self.mode` field names and `set_status_error`-equivalent helper by reading `forms.rs` directly — the above is the shape, not verbatim code to paste blindly.)

Wire key handling in `input.rs` for both new modes to plain `TextInputField` char/backspace/submit mechanics (reuse whatever function `InputBaseBranch` uses for its free-text half, minus its picker-candidate resolution — these two fields have no MRU history to fuzzy-match against, so it's the plainer of the two patterns already in the file).

Wire rendering in `input_form.rs`: a one-line prompt each — "Schedule interval (seconds, blank = not scheduled):" and "Pinned branch (blank = normal per-task branch):".

- [ ] **Step 4: Run test, confirm it passes.**

- [ ] **Step 5: Write and pass the "both fields set" test** — typing `"600"` then `"staging"` should produce `schedule_interval_secs: Some(600), pinned_branch: Some("staging".to_string())`.

- [ ] **Step 6: Write and pass the "invalid interval rejected" test** — typing `"abc"` or `"0"` should show an error and NOT advance past `InputScheduleInterval`.

- [ ] **Step 7: Run the full existing `cargo test tui::tests` suite** to catch any snapshot/scenario regressions the new step insertion causes (anything that drove through the old `InputWrapUpMode -> finish_task_creation` transition directly now needs the two new steps in between — fix every broken scenario test's key sequence, don't skip them).

- [ ] **Step 8: Commit**

```bash
git add src/tui/update/forms.rs src/tui/input.rs src/tui/ui/input_form.rs src/tui/tests/
git commit -m "feat(tui): add schedule-interval/pinned-branch steps to task creation"
```

---

## Task 3: Task-editor sections (existing task edit)

**Files:**
- Modify: `src/editor.rs` — `format_editor_content` (line 202), `EditorFields` struct (65-88), `parse_editor_content` (379), `apply_task_editor_fields` (304), `TaskEditApplied` (248-266).
- Test: `src/editor.rs`'s inline `mod tests`, mirroring whatever tests exist for `format_epic_for_editor`/`parse_epic_editor_output`'s `FEED_INTERVAL_SECS` section.

**Interfaces:**
- Consumes: `format_epic_for_editor`/`parse_epic_editor_output`'s exact section-delimiter and `parse_section` helper shape (`editor.rs:143-153`, `:183-200`) — read this code directly before writing, to match it precisely rather than reinvent parsing.
- Produces: two new `EditorFields` members (`schedule_interval_secs: Option<i64>`, `pinned_branch: Option<String>`), threaded into `TaskEditApplied`'s diff-against-prior-task logic, which the runtime turns into a `TaskPatch` (from the sibling scheduling-primitive plan).

- [ ] **Step 1: Write the failing round-trip test**

```rust
#[test]
fn task_editor_round_trips_schedule_interval_and_pinned_branch() {
    let task = test_task_with(|t| {
        t.schedule_interval_secs = Some(600);
        t.pinned_branch = Some("staging".to_string());
    });
    let content = format_editor_content(&task);
    assert!(content.contains("--- SCHEDULE_INTERVAL_SECS ---"));
    assert!(content.contains("600"));
    assert!(content.contains("--- PINNED_BRANCH ---"));
    assert!(content.contains("staging"));

    let fields = parse_editor_content(&content).expect("should parse");
    assert_eq!(fields.schedule_interval_secs, Some(600));
    assert_eq!(fields.pinned_branch, Some("staging".to_string()));
}

#[test]
fn task_editor_clears_schedule_interval_when_section_emptied() {
    let task = test_task_with(|t| t.schedule_interval_secs = Some(600));
    let content = format_editor_content(&task);
    let cleared = content.replace("600", ""); // simulate user deleting the value
    let fields = parse_editor_content(&cleared).expect("should parse");
    assert_eq!(fields.schedule_interval_secs, None);
}
```

- [ ] **Step 2: Run tests, confirm they fail** (sections don't exist yet).

- [ ] **Step 3: Implement**, mirroring the epic editor's exact pattern:

```rust
// In format_editor_content, alongside the existing sections:
writeln!(out, "--- SCHEDULE_INTERVAL_SECS ---").unwrap();
writeln!(out, "{}", task.schedule_interval_secs.map(|n| n.to_string()).unwrap_or_default()).unwrap();
writeln!(out, "--- PINNED_BRANCH ---").unwrap();
writeln!(out, "{}", task.pinned_branch.as_deref().unwrap_or("")).unwrap();
```

```rust
// In parse_editor_content, alongside the existing parse_section calls:
schedule_interval_secs: parse_section(&sections, "SCHEDULE_INTERVAL_SECS", |raw| raw.parse::<i64>().ok(), None),
pinned_branch: parse_section(&sections, "PINNED_BRANCH", |raw| if raw.is_empty() { None } else { Some(raw.to_string()) }, None),
```

(Match `parse_section`'s actual signature from `editor.rs:186-192` exactly — the above is illustrative shape, confirm generic parameters/error handling against the real function.)

Add both fields to `EditorFields` and to `TaskEditApplied`'s diff (comparing old vs new task, producing the patch entries only for changed fields — same discipline the existing fields already follow).

- [ ] **Step 4: Run tests, confirm they pass.**

- [ ] **Step 5: Write and pass an invalid-input test** — non-numeric content in the `SCHEDULE_INTERVAL_SECS` section should parse to `None` (silently, matching `parse_section`'s existing `.ok()`-based leniency for other numeric fields — confirm this matches the file's actual convention, since a hard-fail might be more appropriate depending on how `feed_interval_secs` handles it; follow whatever the existing epic pattern actually does).

- [ ] **Step 6: Commit**

```bash
git add src/editor.rs
git commit -m "feat(editor): add schedule_interval_secs/pinned_branch sections to task editor"
```

---

## Task 4: Card indicator

**Files:**
- Modify: `src/tui/ui/kanban/cards.rs` (`render_card_indicator`, line 275/535).
- Test: `src/tui/tests/snapshots` (a new snapshot fixture with a scheduled task) or an inline unit test on `render_card_indicator`'s output, whichever the file's existing tests use for other badges.

- [ ] **Step 1: Write the failing test** — a task with `schedule_interval_secs = Some(600)` should render a badge (e.g. `[⏱ 10m]`) somewhere in the card's line-2 output; a task without it should not.

- [ ] **Step 2: Run test, confirm it fails.**

- [ ] **Step 3: Implement** — read `CardIndicator`'s definition first (not traced by the research pass) to decide: add a synthetic labels-style entry, or a new `CardIndicator` arm. Prefer whichever requires touching fewer exhaustive-match call sites — check with `cargo build` after a trial change before committing to one approach.

- [ ] **Step 4: Run test, confirm it passes. If a rendering snapshot changed, run `INSTA_UPDATE=always cargo test tui::tests::snapshots`, review the diff, then `rm src/tui/tests/snapshots/*.snap.new`.**

- [ ] **Step 5: Commit**

```bash
git add src/tui/ui/kanban/cards.rs src/tui/tests/snapshots
git commit -m "feat(tui): show a badge on scheduled-task cards"
```

---

## Task 5: `docs/specs` alignment

- [ ] Use `allium:tend` to document the new TUI surfaces (creation-flow steps, editor sections, card badge) in `dispatch.allium`/`tasks.allium` — likely as additions to the existing `TextInputField`/task-editor surface descriptions, not new top-level surfaces.
- [ ] Run `allium:weed` to confirm alignment; fix any drift found.
- [ ] Commit spec changes separately (`docs: document TUI scheduled-task create/edit in dispatch.allium`).

---

## Self-Review Notes

- This plan hard-depends on the sibling "generic scheduling primitive" plan's Task 1/2 (`Task.schedule_interval_secs`/`pinned_branch` + `TaskPatch` fields) having landed. Do not start Task 1 above until `cargo build` succeeds with those fields present.
- Every new step must remain skippable (Enter on empty input) with no behavior change for a task that never touches these fields — re-run the full existing `cargo test tui::tests::scenarios` suite at the end, not just the new tests, since the creation-flow chain is a single ordered sequence and inserting a step is exactly the kind of change that silently breaks an unrelated scenario test's hardcoded key sequence.
