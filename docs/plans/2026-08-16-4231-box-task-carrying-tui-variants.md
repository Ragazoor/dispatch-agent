# 4231 — Box the `Task`-carrying TUI message/command variants

**Date:** 2026-08-16
**Task:** #4231 (deferred from #4203, sequenced after #4204)

## Premise check

Verified against current `main` (`b18a17ab`):

- `src/tui/types.rs::Message` and `src/tui/types.rs::Command` both carry
  `#[allow(clippy::large_enum_variant)]` with the deferral comment from #4203.
- Measured: both enums are **480 bytes**. Runner-up variants are ~208 bytes, so
  the spread is ~272 bytes against clippy's 200-byte default.
- The `Task`-carrying variants are exactly the six named in the task:
  - `TaskMessage::Created { task: Task }`, `TaskMessage::Updated(Task)`
  - `TaskCommand::Persist(Task)`, `TaskCommand::DispatchAgent { task, .. }`,
    `TaskCommand::TrustAndDispatch { task, .. }`, `TaskCommand::Resume { task }`
- #4204 has landed; nothing else is in flight on this plumbing.

Three further `#[allow(clippy::large_enum_variant)]` sites exist and are **out of
scope** except where boxing makes them dead:

| Site | Cause | Disposition |
| --- | --- | --- |
| `src/tui/messages/task.rs::TaskMessage` | the two `Task` variants | remove — boxing kills it |
| `src/tui/commands/task.rs::TaskCommand` | the four `Task` variants | remove — boxing kills it |
| `src/runtime/mod.rs::LoopEvent` | wraps `Message`, deliberate (documented) | re-check; keep if still needed |
| `src/tui/commands/epic.rs::EpicCommand` | `EpicDraft` vs unit variants | leave — unrelated cause |
| `src/tui/messages/editor.rs::EditorMessage` | `EditKind` (already-boxed `Task` + `Epic`) | leave — unrelated cause |

**Outcome:** all five came out. The three predicted "leave" cases were each
already sitting just inside the lint's threshold and only tipped over it because
of the same #4203 growth — once `Task` was boxed, none of them fired. Verified by
removing each and running `cargo clippy --all-targets -- -D warnings` on a clean
`cargo clean -p dispatch-tui`, which is clean at exit 0.

## Approach

Mechanical: change the payload type to `Box<Task>` and let the compiler enumerate
the call sites. Construction sites gain `Box::new(...)`; consumption sites either
work unchanged (auto-deref through `&Task`) or need a `*task` / `.as_ref()`.

Preference at each site: **construct the `Box` as early as possible and pass it
through**, rather than unboxing and re-boxing between the message and the command
that follows it — several of these hand the same `Task` from a `TaskMessage` to a
`TaskCommand` and would otherwise allocate twice.

## Steps

1. **(test, red)** Add `message_enum_stays_small_enough_to_move_by_value` and
   `command_enum_stays_small_enough_to_move_by_value` to `src/tui/types.rs`'s
   test module: a 256-byte guard-rail on `size_of::<Message>()` /
   `size_of::<Command>()`. Confirm both fail at 480 bytes.
   *(A size assertion, not a behaviour assertion, because the change is
   size-only: no observable behaviour may change. The behavioural guarantee is
   that the existing suite stays green untouched.)*
2. **(green)** Box `TaskMessage::Created`/`Updated`; fix call sites.
3. **(green)** Box `TaskCommand::Persist`/`DispatchAgent`/`TrustAndDispatch`/
   `Resume`; fix call sites.
4. Remove the two `#[allow]`s in `src/tui/types.rs` plus the now-stale comment,
   and the two in `messages/task.rs` / `commands/task.rs`. Re-check
   `runtime/mod.rs::LoopEvent` — keep its `#[allow]` only if clippy still fires.
5. **(verify)** `cargo clippy --all-targets -- -D warnings` clean; `cargo test`
   green (sandbox off, `--no-fail-fast`, per CLAUDE.md).

## Spec impact

None. `Box<T>` is a representation change with no domain-visible behaviour, so no
`docs/specs/*.allium` surface moves. The stale deferral comment in `types.rs` is
the only doc text that must go.

## Risks

- **Double allocation** on the message→command hand-off paths if boxes are
  unwrapped and re-wrapped. Mitigated by step 2/3 ordering: box at the message,
  move the box into the command.
- **Snapshot tests** should be untouched — no rendering path reads a `Task` by
  value out of these enums.

## Result

`Message` and `Command` both went **480 → 208 bytes**. The four `exec_*` runtime
entry points (`exec_persist_task`, `exec_dispatch_agent`, `exec_resume`) kept
their `Task`-by-value signatures and unbox at the `runtime/commands.rs`
boundary — the alternative, threading `Box<Task>` down into them, would have
churned ~12 test call sites to save one stack move on a non-hot path.

`cargo clippy --all-targets -- -D warnings`: clean.
`cargo test --no-fail-fast` (unsandboxed): 20/20 targets green, 4308 tests, no
tmux skips.
