# Task: Remove Dedicated Review/Security Boards

## Context

Fifth and final phase. Removes ~1,700 lines of specialised board code now that the
feed epic mechanism (phases 1–4) covers the same functionality.

**Do not start this task until phases 3 and 4 are verified working end-to-end**, i.e.,
the "Reviews" and "Security" feed epics are populating correctly and accessible via the
tab bar.

## What Gets Removed

### ViewMode variants (src/tui/types.rs)

```rust
// Remove:
ViewMode::ReviewBoard { mode, selection, saved_board }
ViewMode::SecurityBoard { mode, selection, dependabot_selection, saved_board }
```

And all associated types:
- `ReviewBoardMode`, `ReviewBoardSelection`, `SecurityBoardMode`, `SecurityBoardSelection`
- `ReviewBoardState`, `SecurityBoardState`, `DependabotBoardState`
- `PrListState`, `ReviewAgentHandle`, `FixAgentHandle` (if only used by board views)
- `ReviewWorkflowState`, `ReviewWorkflowSubState`, `SecurityWorkflowState`,
  `SecurityWorkflowSubState` (confirm not used elsewhere before deleting)

### Message variants (src/tui/types.rs)

Remove all `Message` variants related to review/security boards:
- `SwitchToReviewBoard`, `SwitchToSecurityBoard`, `SwitchToTaskBoard`
- `SwitchReviewBoardMode`, `SwitchSecurityBoardMode`
- `RefreshReviewPrs`, `RefreshBotPrs`, `PrsLoaded`, `PrsFetchFailed`
- `RefreshSecurityAlerts`, `SecurityAlertsLoaded`, `SecurityAlertsFetchFailed`,
  `SecurityAlertsUnconfigured`
- `ToggleReviewDetail`, `ToggleSecurityDetail`, `ToggleSecurityKindFilter`
- `DispatchReviewAgent`, `ReviewAgentDispatched`, `ReviewAgentFailed`
- `DispatchFixAgent`, `FixAgentDispatched`, `FixAgentFailed`
- `DetachReviewAgent`, `DetachFixAgent`
- `MoveReviewItemForward`, `MoveReviewItemBack`, `ReviewWorkflowUpdated`
- `MoveSecurityItemForward`, `MoveSecurityItemBack`, `SecurityWorkflowUpdated`

### Command variants (src/tui/types.rs)

Remove `Command` variants:
- `FetchPrs`, `FetchSecurityAlerts`
- `DispatchReviewAgent`, `DispatchFixAgent`
- `DetachReviewAgent`, `DetachFixAgent`
- `MoveReviewItem`, `MoveSecurityItem`
- `MergeReviewPr`, `MergeSecurityPr`

### App struct fields (src/tui/mod.rs)

Remove from `App`:
- `review: ReviewBoardState`
- `security: SecurityBoardState`

### Files to delete entirely

- `src/tui/ui/review.rs` (~719 lines)
- `src/tui/ui/security.rs` (~704 lines)

Update `src/tui/ui/mod.rs` to remove the `review` and `security` module declarations
and any re-exports.

### Input handlers (src/tui/input.rs)

Remove the `ViewMode::ReviewBoard` and `ViewMode::SecurityBoard` match arms (~335 lines).

### Message handling (src/tui/mod.rs)

Remove all review/security message handlers in `update()` and any helper functions
they call (`handle_switch_to_review_board`, `handle_prs_loaded`, etc.).

### Runtime (src/runtime.rs)

Remove `exec_fetch_prs`, `exec_fetch_security_alerts`, `exec_dispatch_review_agent`,
`exec_dispatch_fix_agent`, and related command handlers.

Remove polling for review/security in `handle_tick()`:
- `REVIEW_REFRESH_INTERVAL` and `SECURITY_POLL_INTERVAL` constants

### Constants (src/tui/mod.rs)

Remove:
- `REVIEW_REFRESH_INTERVAL`
- `SECURITY_POLL_INTERVAL`

### Allium specs

- Delete `docs/specs/review-board.allium`
- Delete `docs/specs/security.allium`
- Delete `docs/specs/dependabot.allium` (if review-board-only)
- Update `docs/specs/core.allium` config section:
  - Remove `review_refresh_interval`, `security_poll_interval`
  - Add `default_feed_interval: Duration = 30.seconds`
- Write `docs/specs/feeds.allium` covering the feed epic concept, `FeedItem` schema,
  upsert semantics, status preservation

## Approach

Work in two sub-steps to keep diffs reviewable:

1. **Delete files and strip imports** — remove the two UI files, strip module
   declarations, fix compilation errors. At this point the app should compile but Tab
   switching to review/security will be broken.

2. **Remove types and handlers** — delete ViewMode variants, Message/Command variants,
   App fields, input handlers, runtime handlers. Compiler errors guide the cleanup
   systematically.

Run `cargo clippy --all-targets -- -D warnings` after each sub-step to catch dead code.

## TDD Checklist

- [ ] Confirm all existing tests still pass after removal (`cargo test`)
- [ ] Confirm `cargo clippy` clean
- [ ] Update any snapshot tests that showed the old tab bar
- [ ] Verify feed epic tabs appear for Reviews and Security in a fresh run
- [ ] Smoke test: PRs and security alerts load correctly via feed commands

## Files

All files listed in "What Gets Removed" above, plus:
- `docs/specs/feeds.allium` — new
- `docs/specs/core.allium` — updated config
