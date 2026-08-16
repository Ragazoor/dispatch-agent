# 3988 — Remove the `## Verification` section from the dispatch prompt

## Goal

Stop emitting the `## Verification` section in agent prompts. The verify command is
already surfaced by `wrap_up`, so carrying it in the initial prompt is duplicated
context. Delete the prompt-side rendering and all the plumbing that exists only to
feed it, leaving `wrap_up` as the single surface.

## Premise check (done — both halves confirmed)

- **Prompt side**: `render_verification` (`src/dispatch/prompts.rs::render_verification`)
  emits a 7-line `## Verification` block, wired into every prompt variant through the
  shared `render_task_prompt` skeleton.
- **Wrap-up side**: `wrap_up_verify_line` (`src/mcp/handlers/tasks/wrap_up.rs::wrap_up_verify_line`)
  emits `**Verify before exiting**: run \`<cmd>\`…` on all three wrap-up actions
  (`done`, `pr`, `rebase`) — `issue_wrap_up_token` is shared by both finish paths, so no
  action misses it.

Known and accepted trade-off (user decision, this task): `wrap_up` fires at Step 7C of
the `/wrap-up` skill, i.e. **after** the Step 6 commit and — on the rebase path — after
the base branch has been fast-forwarded. So post-change an agent may not see the verify
command until it is too late to act cheaply on it. We are doing the pure removal anyway;
no compensating change to `plugin/skills/wrap-up/SKILL.md` is in scope.

Incidental drift found while mapping: `docs/specs/dispatch.allium` claims the
verification section is **last** in the prompt and "intentionally" so, but the code puts
it between the validated-knowledge block and the mode addendum. The removal makes the
claim moot rather than requiring a separate fix.

## Scope

`fetch_verify_command` stays public — `wrap_up` and `tests/verify_command.rs` use it.
The `saved_repo_paths.verify_command` column, the `set_verify_command` MCP tool, and the
`dispatch repo set-verify` CLI are all untouched. Only the *prompt* consumer goes.

## Steps

### Step 1 — Spec first (`docs/specs/dispatch.allium`)

1. Delete the `{verification_section}` entry (and its four-line "Intentionally last"
   annotation) from the prompt-structure comment in the `DispatchTask` surface.
2. Replace the `-- When task.repo_path has a SavedRepoPath row with verify_command …`
   paragraph with an explicit negative statement, so a future agent does not re-add it:
   the dispatch prompt deliberately carries **no** verify command; it reaches the agent
   only through the `wrap_up` response (cross-reference `WrapUpRebase` /
   `WrapUpPr` in `pr-workflow.allium`).
3. Leave `core.allium`'s `SavedRepoPath.verify_command` field and its
   `VerifyCommandSingleLine` invariant alone — the field still exists and is still
   validated. `mcp-task-tools.allium`'s `set_verify_command` guidance already describes
   `wrap_up` as the only echo surface and needs no edit.
4. Run `allium check` on the edited spec.

### Step 2 — Tests first: prompt rendering (red)

In `mod tests` in `src/dispatch/prompts.rs`, rewrite the five verification tests
(`build_prompt_includes_verification_section_when_configured`,
`build_prompt_omits_verification_section_when_none`,
`build_prompt_verify_section_appears_after_task_block`,
`build_quick_dispatch_prompt_includes_verification_section_when_configured`,
`build_quick_dispatch_prompt_omits_verification_section_when_none`) into three
absence tests, one per prompt variant:

- `build_prompt_never_renders_verification_section`
- `build_quick_dispatch_prompt_never_renders_verification_section`
- `build_research_prompt_never_renders_verification_section`

Each asserts `!text.contains("## Verification")` **and**
`!text.contains("Before declaring work complete")`.

To make them genuinely red before the implementation lands, write them in this order:

1. First revision — still set `verify_command: Some("cargo test".to_string())` on the
   `PromptContext` (the field exists at this point). Run them: all three fail. This is
   the red state that proves the assertion has teeth.
2. After Step 3 removes the field, drop the `verify_command:` line from each test body
   (they become `PromptContext::default()` / `with_learnings`). They stay green and
   remain a guard against the section being re-introduced through any prompt-copy edit.

### Step 3 — Implementation: prompt layer (`src/dispatch/prompts.rs`)

1. Delete `render_verification` entirely.
2. In `render_task_prompt`, drop the `let verify = …` binding and change the format
   string from `{knowledge}{verify}{addendum}` to `{knowledge}{addendum}`. Update the
   doc comment's skeleton line (`{intro}{spacing}{block}\n\n{knowledge}{verify}{addendum}…`)
   and the "knowledge/verify plumbing" sentence to say knowledge only.
3. Remove `verify_command` from `PromptContext` and delete the `with_verify` builder
   method. `PromptContext::with_learnings` loses its `verify_command: None` initialiser.
4. Apply the Step 2.2 test simplification.

### Step 4 — Implementation: launcher plumbing (`src/dispatch/agents.rs`)

1. Drop the trailing `verify_command: Option<&str>` parameter from `dispatch_agent`,
   `research_agent`, and `quick_dispatch_agent`, and the `.with_verify(verify_command)`
   calls in their prompt closures.
2. Drop `DispatchInputs::verify_command` and the `fetch_verify_command` call in
   `prepare_inputs_with_epic_ctx`. Update the `DispatchInputs` doc comment: it now
   describes *two* per-task reads (epic banner, learnings), not three — fix the
   "three per-task reads" / "identical three-step prologue" wording.
3. Keep `fetch_verify_command` exactly as is, including its `pub` visibility and the
   `pub use` in `src/dispatch/mod.rs` — `wrap_up` imports it from there.

### Step 5 — Update call sites

Production:

- `src/mcp/handlers/tasks/dispatch.rs` — the `DispatchInputs` destructuring loses
  `verify_command`; drop the two `verify_command.as_deref()` arguments (lines ~25 and
  ~33).
- `src/runtime/tasks.rs` — same in `exec_quick_dispatch` (~66–74) and
  `exec_dispatch_agent` (~378–397): drop the `verify_command` binding and its three
  `.as_deref()` arguments.

Tests (mechanical, ~45 sites — use `Edit` with `replace_all` on the two uniform forms):

- `&LearningInjections::default(), None)` → `&LearningInjections::default())` — covers
  every `dispatch_agent` / `quick_dispatch_agent` call in `src/dispatch/tests.rs`.
- `research_agent(&task, &mock, None, None)` → `research_agent(&task, &mock, None)`.
- `src/dispatch/mock_sequence.rs` — the two `dispatch_agent(` calls (~693, ~838); these
  are multi-line, so edit them individually.
- `src/runtime/tests.rs` — `prepare_inputs_reads_epic_context_injections_and_verify_command`
  (~3684): rename to drop `_and_verify_command`, delete the `db.set_verify_command(…)`
  setup and the `assert_eq!(inputs.verify_command…)` (~3735), and delete the companion
  `assert!(inputs.verify_command.is_none())` (~3778).

`tests/verify_command.rs` needs no change — it exercises `fetch_verify_command` and the
DB lookup, both of which survive.

### Step 6 — Snapshots

- Delete the `snapshot_dispatch_prompt_with_verify` test from
  `src/dispatch/prompts_snapshots.rs`.
- Delete
  `src/dispatch/snapshots/dispatch_tui__dispatch__prompts_snapshots__snapshot_dispatch_prompt_with_verify.snap`.
- Every other prompt snapshot must stay **byte-identical**: `render_verification`
  returned `""` for `None` and no other snapshot sets a verify command. If any other
  `.snap` diffs, that is a bug in the edit, not an intentional change — do not accept it.
- `rm -f src/dispatch/snapshots/*.snap.new src/tui/tests/snapshots/*.snap.new` before
  finishing.

### Step 7 — Docs and agent-facing copy

- `CLAUDE.md`, the **Verify Command** section: replace "When set, `build_prompt` appends
  a `## Verification` section (`render_verification` in `src/dispatch/prompts.rs`); when
  null, nothing is emitted." with a statement that the command is surfaced **only** in
  the `wrap_up` response (`wrap_up_verify_line`), and that the dispatch prompt
  deliberately does not carry it. Keep the storage/CLI/newline-rejection sentences.
- `docs/module-map.md`, the `src/dispatch/prompts.rs` row: drop "and verification
  rendering" from the description.
- `plugin/skills/retro/SKILL.md`, the surfaces table: change the
  `the repo's verify command` row's "appended to every dispatch prompt" to reference the
  `wrap_up` response instead.
- `plugin/skills/allium-loop/SKILL.md`, step 3 priority 1: delete the
  `a "Verification" section in the current task's prompt` example — that source no longer
  exists. The remaining priorities (project docs, then ask) still resolve a command.
- Check `mod tests` in `src/setup/plugins.rs` for `contains` assertions over the two
  edited skill bodies and update any that reference the removed wording. (Current grep
  shows no verify-related assertion, but re-check after editing since the check is
  section-scoped.)

## Verification

```
cargo fmt --check && cargo test && ./scripts/check-doc-paths.sh
```

Plus, because the pre-push gate is stricter than a plain build:

```
cargo clippy --all-targets -- -D warnings
./scripts/check-doc-symbols.sh
```

`check-doc-symbols.sh` is the one most likely to bite: it rejects backticked
snake_case identifiers in agent-facing docs that occur nowhere in the code, so any
lingering `render_verification` / `with_verify` mention in `CLAUDE.md`, `docs/*.md`, or a
doc comment becomes a hard failure once the symbols are deleted. Grep for both names
across the tree before running the suite.

Targeted runs while iterating:

```
cargo test dispatch::prompts
cargo test dispatch::prompts_snapshots
cargo test --test verify_command
cargo test runtime::tests::prepare_inputs
```

## Out of scope

- Any change to `wrap_up`'s verify line, or to when `/wrap-up` fires it (the ordering
  weakness noted above). If it should be surfaced before the commit, that is a separate
  task.
- Removing the `verify_command` column, the `set_verify_command` MCP tool, or the
  `dispatch repo set-verify` / `clear-verify` CLI.
