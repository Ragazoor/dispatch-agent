# WP2 — Routing function (pure `route(&[Signal]) -> FeedRole`)

> **For agentic workers:** REQUIRED SUB-SKILL: superpowers:executing-plans. TDD: test first.

**Goal:** A pure, exhaustively-tested function mapping a PR's signals to its target role. No I/O, no DB.

**Spec:** `docs/superpowers/specs/2026-06-15-pr-review-feed-routing-design.md` §3.
**Depends on:** WP1 (`Signal`, `FeedRole`).

**Interface this WP exposes:** `pub fn route(signals: &[Signal]) -> FeedRole` (in `src/feed/routing.rs`, re-exported from `src/feed/mod.rs`). Returns one of `MyReviews | TeamReviews | Bots` (never `None`/`ReviewsParent`/`Cve`).

**Precedence (in order):**
1. engaged (`Reviewed` OR `Commented`) AND NOT `AuthorMe` → `MyReviews`
2. `AuthorBot` → `Bots`
3. `DirectRequest` → `MyReviews`
4. `TeamRequest` → `TeamReviews`
5. fallback → `MyReviews` (caller logs a warning; see WP3)

---

### Task 1: `route` with the full precedence table

**Files:**
- Create: `src/feed/routing.rs`
- Modify: `src/feed/mod.rs` (add `mod routing; pub use routing::route;`)

- [ ] **Step 1 — failing tests.** In `src/feed/routing.rs`:
```rust
#[cfg(test)]
mod tests {
    #![allow(clippy::unwrap_used, clippy::expect_used)]
    use super::*;
    use crate::models::Signal::*;
    use crate::models::FeedRole;

    #[test] fn direct_request_to_my() { assert_eq!(route(&[DirectRequest]), FeedRole::MyReviews); }
    #[test] fn team_request_to_team() { assert_eq!(route(&[TeamRequest]), FeedRole::TeamReviews); }
    #[test] fn reviewed_to_my() { assert_eq!(route(&[Reviewed]), FeedRole::MyReviews); }
    #[test] fn commented_to_my() { assert_eq!(route(&[Commented]), FeedRole::MyReviews); }
    #[test] fn bot_to_bots() { assert_eq!(route(&[AuthorBot]), FeedRole::Bots); }

    // engaged wins over bot (resolved decision #1)
    #[test] fn reviewed_bot_to_my() { assert_eq!(route(&[Reviewed, AuthorBot]), FeedRole::MyReviews); }
    // but my own commented PR is not "engagement" -> bot/author rules apply
    #[test] fn own_comment_on_bot_is_bots() { assert_eq!(route(&[Commented, AuthorMe, AuthorBot]), FeedRole::Bots); }
    // team-requested PR I reviewed -> My (engagement wins, no leak)
    #[test] fn reviewed_team_to_my() { assert_eq!(route(&[TeamRequest, Reviewed]), FeedRole::MyReviews); }
    // empty -> fallback My
    #[test] fn empty_to_my() { assert_eq!(route(&[]), FeedRole::MyReviews); }
}
```
- [ ] **Step 2 — run, expect fail** (`route` undefined): `cargo test -p dispatch feed::routing`
- [ ] **Step 3 — implement:**
```rust
use crate::models::{FeedRole, Signal};

/// Map a PR's signals to its target role sub-epic. Pure. Precedence is
/// documented in the design doc §3 (engagement wins over bot).
pub fn route(signals: &[Signal]) -> FeedRole {
    let has = |s: Signal| signals.contains(&s);
    let engaged = (has(Signal::Reviewed) || has(Signal::Commented)) && !has(Signal::AuthorMe);
    if engaged {
        FeedRole::MyReviews
    } else if has(Signal::AuthorBot) {
        FeedRole::Bots
    } else if has(Signal::DirectRequest) {
        FeedRole::MyReviews
    } else if has(Signal::TeamRequest) {
        FeedRole::TeamReviews
    } else {
        FeedRole::MyReviews
    }
}
```
- [ ] **Step 4 — run, expect pass.**
- [ ] **Step 5 — commit:** `feat(feed): pure signal->role routing function`

---

## Done when
- All routing tests pass; `cargo test -p dispatch feed::routing` green.
- `route` is pure (no `async`, no DB, no I/O) and total over the `Signal` set.
