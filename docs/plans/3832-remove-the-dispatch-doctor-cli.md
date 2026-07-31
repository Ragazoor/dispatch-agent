# 3832 — Remove the `dispatch doctor` CLI

## Goal

Retire the `dispatch doctor` self-diagnosis CLI surface entirely: implementation,
spec, tests, and every documentation reference. Decided 2026-07-31 while
designing the token budget indicator (#3821), where two proposed doctor checks
were dropped on the grounds that doctor is going away.

## Survey (verified 2026-07-31)

`doctor` is a manual-only CLI surface with exactly one consumer:

- `src/cli/doctor.rs` — 895 lines (checks + repairs + inline tests).
- `src/cli/mod.rs:3` — `pub mod doctor;`.
- `src/main.rs` — the `Commands::Doctor` variant (143-152), the `DoctorCheck`
  subcommand enum (189-227), `cmd_doctor` (638-805), and the dispatch arm
  (857-861).
- `tests/cli.rs:965-1176` — eight integration tests.
- `docs/specs/doctor.allium` — 154 lines, whole file.
- Cross-references in `CLAUDE.md`, `docs/conventions.md`, `docs/module-map.md`,
  `docs/specs/repo-sync.allium`, `docs/specs/observability.allium`,
  `docs/specs/mcp-task-tools.allium`.

Confirmed **not** referenced from: `src/runtime/`, `src/tui/`, `src/service/`,
`plugin/` skills, `scripts/`, `.githooks/`, CI workflows, `docs/reference.md`.
`grep -rn "cli::doctor\|doctor::"` outside the file itself returns only the one
`use` in `cmd_doctor`. Nothing else imports any of its 17 public items, so
deletion cannot orphan a caller.

## Decision: the hook-repair capability is replaced by the documented one-liner

`doctor hooks --repair` is the only automated way anything sets
`core.hooksPath = .githooks` — `grep -rn "hooksPath"` finds no other writer. The
task flags this as the real risk: removing it without a replacement leaves fresh
clones with no pre-push hook, silently disabling the fmt/clippy/doc-path/no-sleep
gates.

**Replacement: keep only `git config core.hooksPath .githooks`, already documented
alongside it in `CLAUDE.md`'s "First-time setup".** The capability does not move
into `dispatch setup`.

Why not move it into `setup`:

- `check_hooks` (`src/cli/doctor.rs:227`) iterates the *union of every known repo
  path* and warns on any whose `core.hooksPath` isn't `.githooks`. That applies a
  dispatch-repo-specific convention to every repository dispatch manages — most
  of which have no `.githooks/` directory at all. The check was wrong in the
  general case; preserving it would preserve that bug.
- `dispatch setup` configures the user's Claude Code MCP integration globally. It
  is not scoped to a repo checkout and has no business writing per-repo git
  config for arbitrary managed repos.
- The remediation is a single, memorable git command that the doc already gives.
  There is nothing for a tool to add beyond typing it.

## Plan

Removal work, so the TDD step is expressing the *absence* as a test before
deleting the code: the eight existing doctor integration tests are replaced by
one asserting the subcommand is gone.

### Step 1 — Test first: assert the subcommand no longer exists

In `tests/cli.rs`, delete the `dispatch doctor` block (965-1176) and put in its
place a single test asserting clap rejects the argument:

```rust
#[test]
fn doctor_subcommand_no_longer_exists() {
    let db = NamedTempFile::new().unwrap();
    let out = binary()
        .args(["--db", db.path().to_str().unwrap(), "doctor"])
        .output()
        .unwrap();
    assert!(!out.status.success(), "`dispatch doctor` was removed ...");
    let stderr = String::from_utf8_lossy(&out.stderr);
    assert!(stderr.contains("unrecognized subcommand") || stderr.contains("unexpected argument"));
}
```

Run `cargo test --test cli doctor` and confirm it **fails** while the subcommand
still exists. That failure is the proof the test is wired to the behaviour.

### Step 2 — Delete the implementation

- `rm src/cli/doctor.rs`.
- Drop `pub mod doctor;` from `src/cli/mod.rs`.
- In `src/main.rs`: drop the `Commands::Doctor` variant, the whole `DoctorCheck`
  enum, `cmd_doctor`, and the `Commands::Doctor => ...` match arm. Watch for
  imports that become unused once `cmd_doctor` is gone.

Run `cargo test --test cli doctor` — now green — then `cargo build`.

### Step 3 — Delete the spec

`rm docs/specs/doctor.allium`.

### Step 4 — Reconcile the cross-references

The specs do not merely mention doctor in passing; three of them use it as
load-bearing rationale. Each is rewritten to keep the *reasoning* while dropping
the dangling reference — the arguments for why drift is not a doctor check
survive as arguments about why drift needs a fetch-backed surface.

- `docs/specs/repo-sync.allium`
  - Header `Excludes:` (13-15) — drop the doctor clause.
  - "Not a surface" note (24-30) — this whole note exists to argue against a
    doctor check. With doctor gone the note has no subject; delete it. The
    honesty argument it makes is already carried by
    `UnmeasuredIsNeverPresentedAsClean`.
  - `UnmeasuredIsNeverPresentedAsClean` (649-653) — drop the trailing doctor
    sentences; keep the invariant itself intact.
  - `OneRepoSetForDriftMeasurement` (665-676) — rewrite so the rule states the
    repo set positively (saved repo paths, on all four surfaces) without
    reconciling against doctor's now-nonexistent "known repo" union.
- `docs/specs/observability.allium` — remove `doctor` from the two one-shot
  subcommand lists (24, 255) and from "No new metric, counter, doctor check, or
  queryable surface" (258).
- `docs/specs/mcp-task-tools.allium:554` — delete the `dispatch doctor ...` bullet.
- `CLAUDE.md`
  - Line 26 (First-time setup) — the decision above: `git config core.hooksPath
    .githooks` becomes the only instruction.
  - Line 109 (External Dependencies) — the "no startup preflight" sentence
    explains itself via doctor; reword to state the fact directly.
  - Line 147 — drop the `doctor.allium` spec-list entry.
  - Line 196 and `docs/conventions.md:241` — drop `src/cli/doctor.rs` from the
    sanctioned direct-mutation consumer lists. These are checked paths; leaving
    them fails `check-doc-paths.sh`.
  - Line 219 — drop `doctor` from the `src/cli/` subsystem entry-point line.
- `docs/module-map.md` — drop `doctor` from the `src/main.rs` subcommand list
  (11) and the `src/cli/mod.rs` submodule list (13); delete the
  `src/cli/doctor.rs` row (14).

### Step 5 — Verify

```
cargo fmt --check && cargo test && ./scripts/check-doc-paths.sh
```

Plus the rest of the pre-push gate, since docs changed substantially:
`cargo clippy --all-targets -- -D warnings`, `./scripts/check-doc-symbols.sh`.

Then `allium:weed` over the touched specs to confirm no spec now claims
behaviour the code no longer has.

## Risks

- **Silent loss of the hook gate for fresh clones.** Mitigated by Step 4's
  `CLAUDE.md` rewrite making the one-liner the single, unambiguous instruction
  rather than the second half of an "or".
- **Stale prose the path checker cannot see.** `check-doc-paths.sh` validates
  only that referenced paths exist (learning #281) — it will catch
  `src/cli/doctor.rs` but not the word "doctor" in prose. Step 4 is a manual
  sweep; re-grep for `doctor`/`Doctor` at the end and confirm the only survivors
  are under `docs/plans/`, `docs/superpowers/`, `docs/research/` (dated
  artifacts, deliberately excluded).
- **Unused imports in `main.rs`** after `cmd_doctor` goes. `cargo clippy -D
  warnings` catches these; a plain `cargo build` may not.
