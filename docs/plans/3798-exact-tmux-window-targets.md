# 3798 — tmux prefix-matches window-name targets

## The bug

tmux resolves `-t <window-name>` by **prefix match** when no exact match exists.
Dispatch names windows `task-<id>`, so with ids in the 3000s a `task-378` whose
window has died resolves to a live `task-3782`:

- `send_keys` — the agent command or a `--continue` typed into a *different
  task's* live Claude session.
- `kill_window` — destroys the wrong task's window.

Confirmed against tmux 3.5a.

## What the empirical probe changed about the suggested fix

The task suggested the `=` exact-match sigil (`-t '=task-4'`). Probing tmux 3.5a
shows **the sigil is not universally supported**, so a blanket `=` prefix would
have fixed some call sites and silently broken others:

| command                    | `-t '=task-42'` (window exists) | verdict |
|----------------------------|---------------------------------|---------|
| `kill-window`              | ok                              | ✅ works |
| `select-window`            | ok                              | ✅ works |
| `rename-window -t`         | ok                              | ✅ works |
| `list-panes -t`            | ok                              | ✅ works |
| `send-keys`                | `can't find pane: =task-42` (exit 1) | ❌ breaks |
| `display-message -p -t`    | empty output, **exit 0**        | ❌ breaks *silently* |
| `set-option -w -t`         | `no such window: =task-42` (exit 1) | ❌ breaks |

The pane-target commands only accept the sigil in the `=name.<index>` form —
which reintroduces the hardcoded-pane-index hazard of learning #324 / #3782.

A **pane ID (`%N`) is accepted by every one of these commands** (probed:
`send-keys`, `display-message -t`, `set-option -w -t`, `select-window`,
`list-panes -t`, `rename-window -t`, `join-pane -s`, `kill-window`) and is
unambiguous by construction. That is the mechanism to use.

## Design

One private resolver in `src/tmux.rs`, and every window-*name* target routed
through it:

```rust
/// Resolve a window NAME to the pane ID of its active pane, matching the name
/// exactly. Errors when absent; errors when two windows share the name.
fn window_target(window: &str, runner: &dyn ProcessRunner) -> Result<String>
```

- Query: `tmux list-panes -a -f '#{==:#{window_name},<name>}' -F '#{pane_active}
  #{pane_id} #{window_name}'`. The `-f` filter makes **tmux** compare names for
  equality, so no prefix matching is involved and names containing spaces need
  no special handling. `-a` for the same reason `list_all_window_names` uses it:
  works inside or outside tmux, finds windows in other sessions. A failed query
  (no server) is treated as "no windows", i.e. not-found, matching
  `list_all_window_names`' precedent.
- The returned name is then **re-compared locally**. That is not redundant: the
  filter interpolates the name into a tmux format string, so a name carrying
  `,` or `}` could confuse `#{==:…}`. Re-checking makes correctness independent
  of the filter — a crafted name can at worst produce a miss, never a match on
  the wrong window.
- Absent → `no tmux window named '<w>'`.
- Two exact matches → the existing `multiple tmux windows named '<w>' exist …`
  message, generalised out of `set_window_dispatch_dir`. This matches what
  tmux already does for `select-window`/`kill-window` on duplicate names
  (`can't find window`); it newly rejects the case for `set-option -w`, which
  today silently picks one.
- **Pass-through** for targets that are already unambiguous: a `%`-prefixed
  pane ID, and `""` (tmux's "current window", which `rename_window` documents
  and `setup_tmux_for_tui` relies on as a fallback).

### Functions changed

| function | change |
|---|---|
| `send_keys` | resolve window → send both `send-keys` to the pane ID |
| `kill_window` | resolve → `kill-window -t %id` |
| `select_window` | resolve → `select-window -t %id` |
| `set_window_dispatch_dir` | resolve → `set-option -w -t %id`; drop the now-dead stderr `"ambiguous"` sniff (the resolver owns it) |
| `rename_window` | resolve the *target* only — never `new_name` |
| `join_pane` | resolve `source_window` → that *is* the source pane ID, so the separate `display-message` call disappears; `join-pane -s %id` |
| `pane_id_for_window` | becomes a thin wrapper over the resolver |
| `inactive_pane_id` | resolve → `list-panes -t %id` |
| `split_window_horizontal_running` | resolve — `spawn_agent_tree_pane` passes the agent's `task-<id>` **window name** into what the signature called a pane target, so the companion pane could open inside a sibling's window |

Unchanged: `has_window` / `list_all_window_names` (already exact);
`new_window*` (`-n` is a new name, not a target); `break_pane_to_window`
(`-s` is a pane ID, `-n` a new name); `kill_pane` / `respawn_pane` /
`select_pane` / `swap_pane` / `split_window_horizontal` (pane IDs).

### The one target that cannot be resolved

`setup_tmux_for_tui` (`src/runtime/mod.rs`) registers
`bind-key space "select-window -t TUI"`. That string is executed by tmux later,
so a pane ID captured at registration time would be stale. It is the single
place where tmux's `=` sigil is the right tool — and `select-window` is one of
the commands the sigil actually works for. Fixed to `-t =TUI`.

### One call-site fix

`src/runtime/split.rs` `exec_swap_split_pane` builds `format!("{new_window}.0")`
as the swap source — a window-name-prefixed target, so prefix-vulnerable, and
index-`0`-vulnerable under `pane-base-index 1`. It already holds
`new_pane_id` from `pane_id_for_window(&new_window)` two lines above; use that.
Removes both hazards and makes the reported pane and the swapped pane the same
pane.

## Tests first

New `tests/tmux_window_targets.rs` — real tmux, private `-L` socket, drop-guard
teardown, `cat > file` capture panes, modelled on `tests/tmux_split_hook.rs`.
(The task named `tests/tmux_lifecycle.rs`; #3782 has not landed, so that file
does not exist. Its rig is reproduced from `tmux_split_hook.rs`.)

No existing test anywhere has two windows whose names are prefixes of each
other — that absence is the reason this shipped.

Topology: `board`, `task-42`, `keep-99`, each running a capture command.

1. `send_keys_to_absent_prefix_window_errors_and_types_nothing`
   `send_keys("task-4")` → `Err`, and `task-42`'s capture file stays empty.
2. `kill_window_on_absent_prefix_window_errors_and_spares_sibling`
   `kill_window("keep-9")` → `Err`, `keep-99` still listed.
3. `send_keys_reaches_the_exactly_named_window`
   with both `task-4` and `task-42` alive, `send_keys("task-4")` lands in
   `task-4` only — `task-42`'s file stays empty.
4. `kill_window_kills_the_exactly_named_window`
   both alive → `kill_window("task-4")` leaves `task-42`.
5. `select_window_on_absent_prefix_window_errors` — active window unchanged.
6. `pane_id_for_window_on_absent_prefix_window_errors` — guards the silent
   exit-0 empty-output failure mode the `=` sigil would have introduced.
7. `set_window_dispatch_dir_on_absent_prefix_window_errors` — the option must
   not land on `task-42`.
8. `resolvers_reject_duplicate_window_names` — two windows both named `dup`
   → error mentions "multiple tmux windows".

Plus mock-level updates in `src/tmux.rs`: every touched function's argv test
declares its windows and asserts the resolved `%id` target; new tests for
pane-ID and empty-string pass-through, for the local name re-check, and for
`window_name_in_lookup` inverting `window_filter`.

### Test-infrastructure change this forced

Adding a lookup to nine helpers broke **78** existing mock tests: the queue in
`MockProcessRunner` is positional, and many tests assert `calls[N]` indices.
Interleaving a listing response into all 78 queues and renumbering every index
would have obscured tests whose subject is the *operation*, not the resolution —
and a mock cannot verify target resolution anyway (it records argv, not tmux's
interpretation of it).

So `MockProcessRunner` grew an explicit policy for the lookup, `WindowLookup`:

- **`AnyName`** (default) — resolve whatever name is asked for, out of band: not
  from the positional queue, not recorded. This is why the `-f` filter matters
  beyond production: it puts the requested name in the argv, so the mock can
  answer without a server. 66 of the 78 needed no change at all.
- **`OnlyNames`** via `with_windows(&[…])` — only these windows exist, so a
  prefix collision can be set up. Paired with `pane_id_of(name)` for asserting
  the resolved target. Used by the 12 tests that assert a `-t` target.
- **`Queued`** via `with_queued_window_lookup()` — no interception; the lookup
  is answered from the queue and recorded. Used by the `window_target` tests
  themselves, and by `unused()`, whose job is to panic on any shell-out.

## Spec

`docs/specs/dispatch.allium` owns the `task-<id>` naming that creates the
exposure, so the invariant goes there: `TmuxWindowTargetedExactly` — every tmux
operation aimed at a named window acts on that window or fails, never on a
different one. `split-pane.allium`'s `JumpBackToTuiWindow` guidance and
`agent-tree.allium`'s reference to it pick up the `=TUI` binding.

## Order of work

1. Spec rule (`allium:tend`).
2. `tests/tmux_window_targets.rs` — red.
3. Resolver + the eight helpers; `split.rs` swap source.
4. Update `src/tmux.rs` mock tests.
5. `cargo fmt --check && cargo test && ./scripts/check-doc-paths.sh`; `allium:weed`.
