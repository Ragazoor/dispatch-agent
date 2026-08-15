# Knowledge-base citation gate — design

Task #4152.

## Problem

Knowledge-base entries (`core/Learning`, SQLite) are injected verbatim into
future dispatch prompts, but no gate ever reads them — unlike `CLAUDE.md`,
`docs/`, `docs/specs/*.allium`, and `src/**/*.rs` doc comments, which
`check-doc-symbols.sh` re-validates on every push. A KB entry that cites a
specific file, symbol, or line number rots exactly like doc prose does, but
silently: nothing catches it, and it reaches every future agent whether or
not they go looking.

Learning #401 is the concrete case: it cites `src/feed/cycle.rs::run_feed_cycle`,
which was renamed to `FeedCycle::run` by #4091. The underlying advice ("a
step that must behave identically on both feed paths goes in one shared
place") is already correctly captured in `feeds.allium` (citing the current
name). The learning is pure duplication with a dead citation, not a unique
fact — it should be deleted, not corrected.

While investigating, two more instances of the same rot class turned up,
neither covered by any existing gate:

- `record_learning`'s JSON-schema (`src/mcp/handlers/dispatch.rs`, a plain
  string literal — `check-doc-symbols.sh` only scans `///`/`//!` doc
  comments) still advertises a `project` scope value. `LearningScope` in
  code has only `user | repo | epic | task`; a call with `scope: "project"`
  fails deserialization. The same stale value appears in
  `plugin/skills/learnings/SKILL.md`'s "Picking a scope" table, which is
  also outside every scanned surface (`docs/*.md`, `docs/specs/*.allium`,
  `CLAUDE.md`).
- Both the live `handle_rate_learning` response text and the same SKILL.md
  describe a `wrong` verdict as routing an entry to human review. Per
  `learnings.allium`, there is no human review, no `needs_review` state —
  neither verdict changes status. This is simply wrong, not stale-but-once-true.

## Goals

1. Make it structurally hard to add a new phantom-prone citation to the KB.
2. Fix the two drift bugs found above, and add a real test so this specific
   class (hand-written JSON schema vs. the Rust enum it describes) can't
   silently diverge again.
3. Extend `check-doc-symbols.sh` to the one surface it's safe to add
   (`plugin/skills/*/SKILL.md` — prose, like the docs it already scans).
4. Clean up existing debt: not just #401, but every currently-reachable
   learning that carries the same citation shape, since a citation that
   happens to still resolve today is exactly as exposed to future rot as
   #401 was.
5. Make the authoring instructions state the rule explicitly, with a
   worked bad/good example — today's guidance doesn't name this failure
   mode at all.

## Non-goals

- No periodic re-validation job for KB citations (unlike `ArchiveStaleLearning`,
  there is no proposal here to re-check citations against the current
  repo on a schedule). The mitigation is structural: forbid the citation
  shape at write time, so there's nothing left to re-check.
- No cross-repo sweep. This task's MCP access (`query_learnings`) only
  reaches learnings scoped to `user` and to this task's own `repo_path`.
  Entries scoped to other repos this dispatch instance tracks are out of
  reach from here and out of scope for this task.
- No change to `LearningKind`/scope semantics beyond the `project` fix.

## Design

### A. Reject code-shaped citations at write time

`LearningService::create_learning` (`src/service/learnings.rs`) gains a
check against `summary` and `detail`: if either contains a
`path.rs::symbol` citation, a `Type::method` citation, or a bare (i.e. not
backticked) snake_case identifier with four or more underscores, the call
fails with `ServiceError::Validation`, naming the offending token and
explaining the alternative: describe the durable behavior in prose, or —
if the specific citation matters — add it to the relevant `docs/specs/*.allium`
file or a Rust doc comment, both of which `check-doc-symbols.sh` keeps
accurate on every push.

These three shapes mirror three of `check-doc-symbols.sh`'s four candidate
shapes (`pathsym`, `typesym`, `bare`) — see that script's header comment for
their rationale and false-positive tuning. The fourth shape (`span`: any
backticked snake_case identifier with at least one underscore) is
deliberately **not** rejected here, even though the script treats it as a
candidate. That shape is dominated in KB text by references to this
project's own MCP tool names — `` `query_learnings` ``, `` `wrap_up` ``,
`` `exit_session` `` — which are a stable, intentionally-versioned public
interface, not an internal implementation detail prone to silent rename.
Blocking it would break the exact usage the `learnings` skill already
teaches ("call `query_learnings` directly ... before guessing"). The three
shapes that are rejected (a `.rs` file path, a `Type::` qualifier, or a
long multi-word snake_case name) are unambiguous markers of an internal
code reference; none of this project's MCP tool names match any of them.

No escape hatch (unlike `check-doc-symbols.sh`'s `allow-phantom-symbol:`
marker): the KB has no human review step (`learnings.allium`: "no human
gate, and no human action of any kind is available or required"), so a
free-text override marker embedded in agent-authored content would be
unenforceable. An agent that needs to reference specific code should put
it in the spec/doc-comment surface that has the escape hatch and the
gate, not in the KB.

This is new domain behavior on `RecordLearningViaMcp`
(`docs/specs/learnings.allium`), so the spec gets a new `requires` clause
and `@guidance` note before tests/code, per this repo's spec-first
convention.

### B. Fix the two drift bugs, and test the class

- `src/mcp/handlers/dispatch.rs`: drop `"project"` from `record_learning`'s
  `scope` enum array and its description string.
- `plugin/skills/learnings/SKILL.md`: drop the `project` row from "Picking
  a scope"; correct the `verdict="wrong"` bullet to say what actually
  happens (downvotes the entry; no review, no status change; it may
  become eligible for the stale-cleanup sweep if it goes net-negative and
  untouched — see `ArchiveStaleLearning`).
- `src/mcp/handlers/learnings.rs::handle_rate_learning`: replace the
  `LearningVerdict::Wrong` response note ("flagged for review if it was
  approved") with accurate text ("recorded as wrong (downvoted)").
- New test in `src/mcp/handlers/tests/learnings.rs`: assert that
  `record_learning`'s advertised `scope` and `kind` JSON-schema enum
  arrays exactly match `LearningScope`'s and `LearningKind`'s variant
  lists. This is a targeted equality check, not a `check-doc-symbols.sh`
  extension — pointing that script's phantom-shape regexes at arbitrary
  Rust source would false-positive on every ordinary `Type::method` call
  in the file.

### C. Extend `check-doc-symbols.sh`

Add `plugin/skills/*/SKILL.md` to the default `TARGETS` list. This is
prose markdown, scanned exactly like `docs/*.md` already is — no new
shape handling needed. Covered by the script's own self-test
(`scripts/test-check-doc-symbols.sh`) with one new case.

### D. Authoring guidance

`plugin/skills/learnings/SKILL.md`'s "Do NOT record" list gains an
explicit entry:

> A citation to a specific file, symbol, or line number (`path.rs::symbol`,
> `Type::method`, a long snake_case function name). These rot silently —
> nothing re-checks the knowledge base the way `check-doc-symbols.sh`
> re-checks docs on every push. If the fact is worth citing precisely, put
> it in the Allium spec or a Rust doc comment (both gated) and describe
> the *behavior* here in prose instead.
>
> Bad: "A step that must behave identically on both feed paths goes in
> `src/feed/cycle.rs::run_feed_cycle`."
> Good: "Feed-cycle logic shared by the auto-poll and manual-refresh paths
> must live in one place, not be duplicated per caller — see feeds.allium."

`record_learning`'s tool description (`dispatch.rs`) gains a short clause
to the same effect, since that's the text an agent sees at the point of
calling the tool, independent of whether it loaded the skill.

### E. Data cleanup

Broaden from "delete #401" to a sweep of every learning reachable from
this task's MCP context (`user`-scoped and `repo`-scoped to this repo) for
the same three citation shapes (A above), deleting any match — not only
ones that are currently phantom. A citation that still happens to resolve
today is exactly as exposed to future rot as #401 was; the point of this
task is to stop treating "still correct" as "safe to keep." Done via
`query_learnings`/inspection plus `delete_learning`, no code change.
Cross-repo entries are out of reach (see Non-goals).

## Testing

TDD, spec first:

1. `docs/specs/learnings.allium`: add the `requires` clause + `@guidance`
   to `RecordLearningViaMcp` before any test/code.
2. `src/service/learnings.rs`: new inline `mod tests` — the citation-shape
   detector accepts tool-name backticks (`` `query_learnings` ``,
   `` `wrap_up` ``) and prose, and rejects each of the three shapes;
   `create_learning` wired to return `ServiceError::Validation` on a hit.
3. `src/mcp/handlers/tests/learnings.rs`: end-to-end
   `record_learning` rejection test; the scope/kind enum-parity test.
4. `scripts/test-check-doc-symbols.sh`: one new case covering
   `plugin/skills/*/SKILL.md`.
