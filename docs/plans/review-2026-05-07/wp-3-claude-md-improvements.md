# WP3: CLAUDE.md improvements

## Context

The project's `CLAUDE.md` is in the top tier of project memory files but undocuments a handful of implicit conventions and workflows that new contributors (human or agent) currently have to infer from the code.

## Findings

- **Severity:** Medium
- **File:** `CLAUDE.md`
- **Issue:** Several implicit conventions are unstated. New contributors hit the same paper cuts.
- **Suggestion:** Add the items below.

### Items to add or expand

1. **Hook installation step** — pull `git config core.hooksPath .githooks` into a top-level "First-time setup" subsection. Note that pre-push runs fmt + clippy + test (slow on first run).
2. **Command-queue draining** — show the 4-line `VecDeque::extend` pattern from `src/runtime/mod.rs:395` as a code snippet so the cascading-effects mechanism is concrete.
3. **`FieldUpdate` decision rule** — the syntax is documented; add the *when* rule: "Nullable string the user can clear → `FieldUpdate`. Non-nullable update → plain `String` in the patch."
4. **Trait-narrowing concrete example** — add `let d: Arc<dyn EpicCrud> = task_store_arc.clone();` so the upcasting mechanism is searchable.
5. **MCP debugging** — port 3142 is mentioned; add: how to tail logs, how to send a manual `curl` JSON-RPC call to reproduce a handler bug.
6. **Snapshot-test cleanup** — the `INSTA_UPDATE=always` step is documented but the `rm src/tui/tests/snapshots/*.snap.new` cleanup is easy to miss; pull it into the main numbered list.

## Files to change

| File | Change |
|---|---|
| `CLAUDE.md` | Add or expand the 6 items above. Keep total length growth modest — concise additions, not paragraphs. |

## Verification

- Read the diff in plain prose.
- Confirm no existing sections were accidentally removed.
- No code or tests change; pre-push hooks pass trivially.
