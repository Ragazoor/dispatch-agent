# 3986 — Bind the help overlay to the real keymap with a drift-detecting test

## Problem

`render_help_overlay_matches_current_keymap` (added by §7 of
`docs/plans/3809-keybinding-pruning-implementation.md`) pinned a *fixed list*
of key strings: retired keys that must be absent (`[d]`, `[W]`, `[I]`, `[S]`)
and live keys that must be present (`[F]`, `[t]`, `[U]`, …).

That shape catches only the drift already known at the time it was written:

- **Adding** a key arm without a help line passes silently — the new key is on
  neither list.
- **Deleting** a key arm makes the test fail with a message telling the author
  to *restore* the help line, which is exactly backwards. `[T]`/`[S]` were
  called out in the original comment as future landmines for this reason.

## Approach

Make the assertion a **set equality** between two parsed sets, following the
repo's existing source-checking idiom (`scripts/check-doc-paths.sh`,
`check-doc-symbols.sh`) — no production refactor.

1. **Handled set** — `include_str!("../input/normal.rs")`, slice from
   `fn handle_key_board_normal` to the next `\n    fn `, collect every
   `KeyCode::Char('X')` in that slice, plus `Esc`/`Enter` for the two bare
   `KeyCode::` arms the overlay also teaches.
2. **Taught set** — parse the `[..]` legends out of the rendered help buffer,
   split on `/`, fold the named keys (`Space` → `' '`, `gg` → `g`), drop
   anything that isn't a single ASCII key (arrow glyphs, prose, `Prefix+…`).
3. Assert both differences are empty, with a direction-specific message that
   names the offending keys and says which file to edit.

### Parsing details worth knowing

- The board renders *behind* the overlay and its footer hint bars are full of
  `[k]`-shaped tokens. `help_popup_lines` locates the popup by its `╔`/`╝`
  double-border corners and reads only the inner rect, so footer hints cannot
  leak into the comparison — and the popup's clamp arithmetic is not
  duplicated in the test.
- Legend content runs to the first `]`, then greedily over any immediately
  following `]`, so `[G/]]` yields two keys (`G`, `]`) rather than one key and
  a lost `]`. `[gg/[]` falls out of the same rule.
- `[/]` is special-cased: it is the search key, not an empty alternation.

## TDD evidence

The test was written first, then each drift direction was induced and the
failure message checked:

| Perturbation | Failure |
|---|---|
| Delete the `[F] flat view` legend from `help.rs` | `handles ["F"] but the help overlay does not teach them — add them to …help.rs` |
| Add a `KeyCode::Char('Z')` arm to `normal.rs` | `handles ["Z"] but the help overlay does not teach them — …` |
| Delete the `KeyCode::Char('T')` arm from `normal.rs` | `the help overlay teaches ["T"] but handle_key_board_normal has no arm for them — delete those legends from …help.rs` |

All three were reverted; the only file changed is
`src/tui/tests/rendering.rs`.

## Scope decisions

- **No spec change.** No `docs/specs/*.allium` surface covers help-overlay
  copy, and this is a test-only change with no behaviour change.
- **The retired-key list is dropped, not kept.** `[d]`, `[W]`, `[I]`, `[S]` are
  subsumed by the phantom-key half of the set comparison — if any of them
  reappeared in the overlay without an arm, the test fails and names it.
- **`render_help_overlay_no_longer_teaches_feed_config_key` is kept.** Its
  `[C]` half is subsumed, but its `"feed config"` prose check is not.
- **The `KEYBINDINGS` table is not built.** The task explicitly ranks the
  parsing test as the cheap version to try first; it proved sufficient.
