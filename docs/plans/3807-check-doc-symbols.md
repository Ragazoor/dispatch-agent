# 3807 — Validate backticked symbol names in agent-facing docs

## Problem

`scripts/check-doc-paths.sh` validates paths and `file:NN` citations but never
*symbol* names. Task #3806 removed two phantom function names that had survived
indefinitely because nothing mechanical could catch them:

- `build_epic_planning_prompt` — `docs/specs/dispatch.allium:214`
- `brainstorm_agent` — `src/dispatch/worktree.rs:129` doc comment

Both are still live in this worktree (#3806 landed on a separate branch), so they
serve as real end-to-end test cases.

## Approach: a "phantom identifier" check, not a definition check

The naive design — match backticked tokens against `fn <name>` / `struct <name>`
definitions in `src/` — drowns in false positives (struct fields, enum variants,
config keys, MCP tool names, test-helper names in `tests/`).

Instead: build a **word index of all identifiers occurring in code**, and flag a
backticked token only when it appears *nowhere* in that index. This is a phantom
check, not a "is it a function" check. It accepts a real false-negative — a
renamed symbol whose old name still occurs somewhere in code passes — in exchange
for a false-positive rate low enough that the checker stays trustworthy.

### Two index sources, both comment-stripped

| Source | Extraction |
|---|---|
| Rust code | `src/` + `tests/` `*.rs`, line comments stripped |
| Allium specs | `docs/specs/*.allium`, `--` comments stripped |

**Stripping comments from the index is load-bearest.** A first prototype built the
index from raw file text; every phantom then self-validated, because the phantom's
own doc comment put the token into the index. `brainstorm_agent` was invisible
until comments were stripped.

`tests/` must be in the index: `poll_for` and `tmux_available_or_skip` are cited by
`docs/conventions.md` and are defined only in `tests/tmux_harness/mod.rs`.

The Allium spec bodies must be an index source because specs declare their own
namespace — `repo_group` is an `EpicOrigin` enum variant, `current_tmux_window()`
is spec-level pseudocode, `close_persisted` is a spec concept cited from a Rust
comment. All three are correct references and all three resolve only via this index.

### Candidate token shape

Inside backticks, the **entire** backtick content must match:

```
^[a-z][a-z0-9]*(_[a-z0-9]+)+(\(\))?$
```

snake_case with at least one underscore, optional trailing `()`. Requiring an
underscore drops single prosey words (`main`, `token`). Requiring the whole
backtick span to match drops CLI flags, `cargo test`, paths, and `Type::method`.

Matching is **whole-word and strict** — no substring fallback (decision below).

## Measured false-positive rate

Prototype run over the whole repo. This is the evidence the design rests on.

| Surface | Rule | Hits | Real | FP |
|---|---|---|---|---|
| `CLAUDE.md` + `docs/*.md`, backticked | index | 3 | 1 | 2 |
| `docs/specs/*.allium`, backticked | index | **0** | – | – |
| `src/**/*.rs` doc comments, backticked | index | 13 | 9 | 4 |
| `docs/specs/*.allium`, **bare tokens in comments** | index | 37 | 1 | **36** |

The last row is the rejected option. Catching the `dispatch.allium` phantom
requires scanning bare (un-backticked) tokens inside Allium comments, which yields
a **97% false-positive rate** (`status_text`, `mode_addendum`, `word_boundary_left`,
… — all legitimate prose). The task warns that a checker which cries wolf is worse
than none, so this surface is **out of scope**, and that limitation is documented.

Consequence, stated plainly: **this checker catches the `worktree.rs` phantom from
#3806 but not the `dispatch.allium` one.** Un-backticked identifiers in Allium
comments remain unguarded.

## Scope

In scope: `CLAUDE.md`, `docs/*.md` (the 6 files `check-doc-paths.sh` already
scans), `docs/specs/*.allium`, and `src/**/*.rs` doc comments (`///`, `//!`).

Out of scope: `docs/plans/`, `docs/superpowers/`, `docs/research/` — dated working
artifacts that legitimately describe code as it stood then. #3806 deliberately left
phantom names there.

## Escape hatch (decision)

An inline `allow-phantom-symbol: <why>` marker on the offending line or the line
directly above it, mirroring the existing `allow-test-sleep: <why>` convention in
`scripts/check-no-test-sleep.sh:58`. The reason sits next to the reference, and no
central file drifts.

This is load-bearing, not decoration: 8 of the 16 findings are *deliberate*
historical references — `/// former `pending_todo_edit` / `pending_todo_delete``
and `/// Migrated from `dispatch_next_respects_sort_order``.

## Shorthand policy (decision)

Strict whole-word matching, no substring fallback; the three shorthand references
get their docs corrected rather than suppressed. A substring escape would let a
genuine phantom resolve against an unrelated longer identifier.

- `install_plugin` → `install_plugin_in`
- `pending_g` → `clear_pending_g_chord`
- `current_thread` → tokio's `new_current_thread`

## Findings to resolve (16)

Fix the docs:

| Token | Site | Action |
|---|---|---|
| `render_tab_bar` | `docs/module-map.md:31` | not in `shared.rs`; verify the whole row (`push_hint_spans`, `caret_line` too) |
| `brainstorm_agent` | `src/dispatch/worktree.rs:129` | the #3806 phantom — name the real callers |
| `opt_value` | `src/db/queries/mod.rs:11` | renamed parameter; name the real one |
| `install_plugin`, `pending_g`, `current_thread` | doc comments | expand to the real identifier |
| `compile_fail` ×2 | `docs/conventions.md`, `docs/how-to.md` | rustdoc attribute, not our symbol — marker |

Marker (deliberate history):

| `dispatch_next_*` ×5 | `src/mcp/handlers/tests/tasks/dispatch.rs:903` | renamed tests, cited for provenance |
| `pending_todo_delete/edit/link` | `src/tui/mod.rs:279` | removed fields, cited as history |

## Deliverables

- `scripts/check-doc-symbols.sh` — sibling to `check-doc-paths.sh` (separate
  concern: index building vs. path resolution; keeps each self-test focused).
  Accepts explicit paths to scan a single file, as `check-doc-paths.sh` does.
- `scripts/test-check-doc-symbols.sh` — hermetic self-test over a temp fixture
  repo, mirroring `test-check-doc-paths.sh`.
- `.githooks/pre-push` — two new steps after the existing doc-paths pair.
- `CLAUDE.md` + `docs/conventions.md` — document the checker and the marker.

## TDD sequence

Tests first at every step; the self-test is the specification.

1. **Write `test-check-doc-symbols.sh` against a temp fixture repo.** Red — the
   checker does not exist. Assertions:
   - green: token defined in fixture `src/`; token defined only in fixture
     `tests/`; token that is an Allium enum variant in the fixture spec; token
     with `()` suffix that resolves; non-candidate shapes (`cargo test`,
     `--force`, `Type::method`, single word `main`, a `src/…` path)
   - red: phantom in a fixture `.md`; phantom in a fixture `.allium`; phantom in a
     fixture Rust doc comment; **phantom whose only occurrence is another
     comment** (the index-must-strip-comments regression)
   - green via marker: phantom with `allow-phantom-symbol:` on its line, and on
     the line above; red when the marker is two lines above
   - a `docs/plans/` fixture file with a phantom is not scanned by default
2. **Implement `check-doc-symbols.sh`** to green, minimum needed.
3. **Run it against this repo.** Expect the 16 findings above.
4. **Resolve all 16** per the table — doc fixes and markers.
5. **Wire into `.githooks/pre-push`** and document in `CLAUDE.md` /
   `docs/conventions.md`.
6. **Verify**: `cargo fmt --check && cargo test && ./scripts/check-doc-paths.sh`,
   plus both self-tests and the new checker green.

## Spec impact

None. This is repo tooling, not domain logic — no Allium spec describes the
pre-push checkers, so nothing in `docs/specs/` changes.
