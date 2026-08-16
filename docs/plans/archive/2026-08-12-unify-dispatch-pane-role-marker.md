# Unify dispatch-created pane identity on one role marker (#3902)

**Goal.** One mechanism answers "which panes in this agent window did dispatch
create, and what is each one for": a pane-scoped tmux option
`@dispatch_pane_role`, written at creation, valued `agent_tree` or `editor`.
The start-command parsing half (`is_agent_tree_command`,
`tmux::pane_ids_with_start_command` and its public predicate parameter) is
deleted.

**Blocker resolved (decided with the user, 2026-08-12): accept the one-off
regression.** No transitional fallback, no startup backfill. Agent windows
already open when this lands carry unmarked panes, so for the remaining life of
each such window the toggle reads the tree as hidden and splits a second tree
pane, and pin/resync stop draining those panes. It self-clears as agent windows
turn over. The rejected alternatives both keep the string matcher alive — a
fallback that nothing forces anyone to delete, or a backfill that moves the mess
into new startup code — which is precisely what this task exists to remove.

## Why the start-command form is the weaker half

It re-derives identity from a string tmux was never asked to keep stable: it
whitespace-splits the command line (a dispatch binary at a path containing a
space mis-parses), compares argv0 by basename, and has to actively defend
against substring matching, because an editor pane opened on
`docs/specs/agent-tree.allium` contains "agent-tree". A marker written by the
code that created the pane needs none of those defences.

## Shape after the change

| Pane | Identified by |
|---|---|
| Companion tree pane | `@dispatch_pane_role` = `agent_tree` |
| Editor pane | `@dispatch_pane_role` = `editor` |
| Any pane dispatch created | `@dispatch_pane_role` set at all |

`companion_pane_ids` becomes the third row: **one** tmux lookup over the option's
presence, down from three calls (a window pre-resolution plus two listings), and
it automatically covers any future dispatch-created pane without being taught
about it.

## Steps

Each step is test-first. Spec before tests, tests before code
(`CLAUDE.md`: "Working With the User").

### 1. Spec — `docs/specs/agent-tree.allium` (`allium:tend`)

`HideAgentTreePane`'s guidance documents the start-command mechanism and its
retroactivity as a *resolved decision*, so this is a spec change, not a comment
change.

- Rewrite the `== How the companion pane is identified ==` section: a
  pane-scoped tmux user option written at creation, matched on its **exact
  value** (`agent_tree`), with `editor` as the sibling value on the pane
  `OpenAgentTreeFileInEditor` opens. "Hidden" is still "no such pane in the
  window". The two load-bearing details of the old form (token-not-substring
  matching, basename comparison of argv0) go away with it — a marker cannot be
  confused by the *contents* of a file an editor is showing.
- Replace the "retroactive at no cost" paragraph with an `ACCEPTED COST`
  paragraph stating the regression above, that it self-clears, and why a
  fallback or a backfill was rejected.
- Add the reason the two panes now share one vocabulary: any pane dispatch put
  in the window is exactly a pane carrying this option, which is what the pin
  drain reads (cross-reference `ToggleVsSplitPaneInteraction` and
  split-pane.allium's `PinTaskInSplitPane`).
- `ShowAgentTreePane`: "no pane in the window was started with the agent-tree
  command" → "no pane in the window carries the marker". Its
  `ToggleTargetResolution` resolution still holds and now covers one more case —
  a window whose companion pane *predates the marker* also gets a fresh, marked
  pane on the first manual toggle, which is the regression's own escape hatch.
- `OneEditorPanePerAgentWindow`'s note ("a mark written immediately after the
  split", and the accepted gap when that write fails) stays true verbatim; only
  the mark's name changes, so touch it only if it names the option.

Then `allium check` on the file, and `allium:weed` on it at the end of the task
to confirm spec and code agree.

Note for the implementer: `./scripts/check-doc-symbols.sh` rejects backticked
snake_case identifiers that occur nowhere in the code, so the spec must not keep
naming `is_agent_tree_command` once it is deleted.

### 2. `src/tmux.rs` — the vocabulary

**Tests first** (inline `mod tests`, `MockProcessRunner`):

- `pane_ids_with_option_value` returns only panes whose value equals the wanted
  one — a listing of `%1 \n%2 agent_tree\n%3 editor\n` yields `%3` for `editor`.
- …and matches exactly, not by prefix or substring.
- …asks tmux for `#{pane_id} #{@dispatch_pane_role}` (argv assertion, so the
  format string cannot silently drift).
- …fails on a non-zero exit.
- Delete the four `pane_ids_with_start_command_*` tests: the behaviour is gone,
  not moved.

**Code:**

- `pub const PANE_ROLE_OPTION: &str = "@dispatch_pane_role";` plus
  `PANE_ROLE_AGENT_TREE: &str = "agent_tree"` and `PANE_ROLE_EDITOR: &str =
  "editor"`, replacing `EDITOR_PANE_OPTION`. Keep them here, with the existing
  "two unrelated modules must agree on this forever" rationale extended: now
  *three* call sites share it (spawn, toggle/resync, editor) plus the pin drain.
- `pub fn pane_ids_with_option_value(target, option, value, runner)` — a third
  wrapper over the private `pane_ids_matching`. The private closure parameter
  stays (the two wrappers differ in predicate: non-empty vs equality); only the
  *public* predicate parameter disappears.
- Delete `pane_ids_with_start_command`.
- Fix the doc comments that name it: `swap_pane`'s "use
  `pane_ids_with_option`/`pane_ids_with_start_command` to resolve one", the
  `inactive_pane_id` gravestone comment, and `pane_ids_with_option`'s own
  paragraph.

### 3. `src/dispatch/agents.rs` — write the marker, read the marker

**Tests first** (`src/dispatch/tests.rs`, rewriting the "Companion pane
identity" block):

- Toggle kills the tree pane even when the tree pane is active — the regression
  the whole lookup exists for — over a role listing.
- Toggle with an editor pane open kills the tree pane only: listing
  `%1 \n%2 agent_tree\n%3 editor\n` → `kill-pane -t %2`, and exactly two calls.
  This replaces the "not fooled by an editor showing agent-tree.allium" test:
  the confusion it guarded against is structurally impossible now, and the risk
  that *is* live is the two roles being conflated.
- Toggle splits when no pane carries the marker.
- **New**: the spawn side marks the pane — split returns `%2`, so the toggle's
  next call is `set-option -p -t %2 @dispatch_pane_role agent_tree`.
- **New**: a failed marker write does not fail the toggle (best-effort, warn;
  same accepted gap as the editor pane's own marker).
- `companion_pane_ids` returns both panes **in one `list-panes` call** — assert
  `calls.len() == 1`, which is the regression guard for the three-call version
  creeping back.
- `companion_pane_ids` is empty for an agent-only window.
- Delete the absolute-path-basename and other-`dispatch`-subcommand tests: both
  assert properties of the deleted matcher.

**Code:**

- `spawn_agent_tree_pane` uses the pane id `split_window_horizontal_running`
  already returns and calls `tmux::set_pane_option(&pane, PANE_ROLE_OPTION,
  PANE_ROLE_AGENT_TREE, runner)`, warning on failure — the pane is open and
  useful either way; only the *next* toggle suffers.
- `agent_tree_pane_id` → first pane from `tmux::pane_ids_with_option_value(window,
  PANE_ROLE_OPTION, PANE_ROLE_AGENT_TREE, runner)`.
- `companion_pane_ids` → `tmux::pane_ids_with_option(window, PANE_ROLE_OPTION,
  runner)`, dropping the `pane_id_for_window` pre-resolution whose whole purpose
  was to avoid re-resolving the window name across two lookups.
- Delete `is_agent_tree_command`. `AGENT_TREE_SUBCOMMAND` survives as spawn-side
  only, so its "shared between the spawn side and the lookup side" doc comment
  must be corrected rather than left to mislead.

### 4. `src/agent_tree_editor.rs` — the editor half

**Tests first:** update the three mock listings and the `set-option` argv
assertion to the role option/value, and add one new test: a pane marked
`agent_tree` is **not** taken as the editor pane (i.e. the lookup is
value-matched, not presence-matched — get this wrong and an open would respawn
the tree pane with an editor).

**Code:** `pane_ids_with_option_value(my_pane, PANE_ROLE_OPTION,
PANE_ROLE_EDITOR, …)` for the lookup, `PANE_ROLE_EDITOR` for the mark. The
`respawn_pane_running` reuse path is unchanged and still relies on pane options
surviving a respawn — now doubly worth stating, since the option also carries
the role.

### 5. `src/runtime/tests.rs` — mock scripts

Four `exec_enter_split_mode_with_task*` scripts queue two `list-panes` replies
for `companion_pane_ids`; each drops to one, and the two that spell a
start-command listing (`%2 dispatch agent-tree 1`) become role listings
(`%2 agent_tree`, `%5 editor`). The fifth (`…_companion_check_fails`) already
queues a single failure and needs no change. Window lookups resolve out of band
under `with_windows`, so nothing else in these scripts shifts.

### 6. Real tmux — `tests/tmux_editor_pane.rs`, `tests/tmux_lifecycle.rs`

A mock proves which command we sent; only a real server proves what tmux did
with it (`docs/conventions.md`). The marker is a tmux-semantics change — a
written option, read back by a different process — so it needs both.

- `tests/tmux_editor_pane.rs`: the fixture currently hand-builds the tree pane
  with `tmux::split_window_horizontal_running`, which after this change leaves
  it unmarked and would break the three toggle/pin tests. Build it through the
  **production** path instead — `dispatch::toggle_agent_tree_pane(WINDOW, …)` on
  the `task-42` window — so the marker under test is written by production code
  and not by the test's own setup. Resolve the tree pane as the window's pane
  that is not the agent's. `editor_pane()` reads role `editor`.
- Add one assertion there: the tree pane carries `@dispatch_pane_role =
  agent_tree` on a real server after the split.
- `tests/tmux_lifecycle.rs`: add the same assertion to the dispatch-time
  companion test, so the *launch* path's marker is covered on a real server too.
  Leave `pin_joins_the_agent_pane_and_kills_the_leftover_companion`'s oracle
  matching on `pane_start_command`: it is deliberately independent of
  production's lookup, and an oracle that read the marker would keep passing if
  production marked the wrong pane. `tmux_harness::pane_start_command` therefore
  stays.

### 7. Verify

```
cargo fmt --check && cargo test && ./scripts/check-doc-paths.sh
```

Plus `cargo clippy --all-targets -- -D warnings` (the pre-push gate; a plain
build will not fail on a bare `unwrap`), and `allium:weed` on
`docs/specs/agent-tree.allium`.

## Out of scope

- `docs/superpowers/specs/2026-08-11-agent-tree-open-in-editor-design.md` and
  `docs/plans/2026-08-11-agent-tree-open-in-editor.md` describe the design as it
  was decided then. They are dated artifacts, excluded from the doc-path checker,
  and are not rewritten to match today.
- No migration, backfill, or fallback (see the blocker resolution above).
