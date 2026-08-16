# 3769 — Document `exit_session`'s failed-close path in `docs/mcp.md`

Doc-only change. No behaviour, no code, no new tests: the behaviour already exists
(introduced in #3744) and is already covered by
`src/mcp/handlers/tests/tasks/dispatch.rs` (the `close_persisted = false` branch tests).
The specs are already correct and are the normative source to mirror.

## Sources of truth (mirror, do not reinvent)

- `ExitSession` in `docs/specs/pr-workflow.allium` — `close_persisted` gates the
  terminal mutation, the tmux teardown and `SessionClosed`; only token consumption
  is unconditional.
- `ExitSessionViaMcp` in `docs/specs/mcp-task-tools.allium` — owns the response
  wording on a failed close (successful tools/call response, not a JSON-RPC error).
- `AutoDispatchNextSubtask` in `docs/specs/epics.allium` — no `SessionClosed` ⇒ no
  chain; plus the fail-closed epic read in `auto_dispatch_next`
  (`src/mcp/handlers/tasks/dispatch.rs`).

## Changes to `docs/mcp.md` (the `exit_session` walkthrough, steps 4–5)

1. **Step 4** — say the terminal mutation is one patch (status, sub_status, url,
   cleared `tmux_window`) that can fail, and that the tmux kill happens **only** when
   that write persisted. On failure the window is deliberately left alive with
   `tmux_window` still set, so the task can never satisfy `is_detached` and drift into
   the awaiting-merge rendering.
2. **New step** (between 4 and 5) — the failed-close response: a *successful*
   JSON-RPC response whose text says the close did not take effect and the task needs
   closing by hand. Deliberately not an error, because the exit token is consumed
   earlier and an error would strand the agent with no retry path.
3. **Step 5** — the chain is gated on the close having persisted first, then on
   `epic_id` + `auto_dispatch`; and the `auto_dispatch` read fails closed (a DB error
   reading the epic stops the chain too).

## Constraints

- `./scripts/check-doc-paths.sh` validates every `src/…`/`docs/…` path and `file:NN`
  citation in this file — keep existing citations valid and verify any new one.

## Verification

```
cargo test && ./scripts/check-doc-paths.sh
```
