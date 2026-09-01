use crate::models::{FeedRole, Signal};

/// Map a PR's signals to its target role sub-epic. Pure: no async, no DB, no
/// I/O. Precedence is documented in the PR-review-feed-routing design doc §3 —
/// engagement (a review or comment on a PR that is not my own) wins over the
/// bot-author rule, so a bot PR I have reviewed still routes to my reviews.
///
/// Total over the `Signal` set: every input (including the empty slice) maps to
/// exactly one of `MyReviews | TeamReviews | Bots`. Never returns
/// `None`/`ReviewsParent`/`Cve`.
/// `OrgReview` (an org-scoped review-related match, not limited to the
/// repos.conf repo list) is treated the same as `DirectRequest`: it still
/// loses to the bot rule, so an org-scoped bot PR routes to `Bots` unless it
/// is also reviewed/commented (engagement wins).
/// The `!has(Signal::AuthorMe)` guard below is UNREACHABLE from the role-routed
/// path — [`excluded_from_reviews`] drops every `AuthorMe` item before routing.
/// It is retained rather than removed because `route` is total over the whole
/// signal set and must stay correct for an input this caller happens never to
/// hand it.
/// Pure exclusion predicate (`ExcludeOwnAuthored` in `docs/specs/feeds.allium`):
/// true when a feed item must not reach the review subtree at all. Evaluated
/// BEFORE [`route`], so an excluded item is never routed, never inserted, and
/// never enters the reconcile's keep-set — an existing task for one is deleted
/// by the same stale pass that removes a merged PR.
///
/// Exactly one signal excludes: `AuthorMe`. A PR the user wrote is not review
/// work for the user. The exclusion is unconditional — `TeamRequest` or
/// `AuthorBot` alongside it does not rescue the item — because authorship is
/// what makes it not review work, regardless of who else was asked.
///
/// Only as good as the signal: the feed script derives `AuthorMe` from the
/// authenticated user's login, and that lookup soft-fails to empty rather than
/// failing the feed. A cycle where it failed emits no `AuthorMe` at all, so own
/// PRs surface until the next successful poll. Self-healing, and deliberately
/// preferred to dropping the emission.
///
/// This lives in the runtime rather than in `scripts/fetch-reviews.sh` on
/// purpose: feed scripts are user-owned copies, so a fix to the shipped
/// template never reaches the copy that actually runs. See the contract's
/// "WHY IN THE RUNTIME AND NOT IN THE SCRIPT" note.
pub fn excluded_from_reviews(signals: &[Signal]) -> bool {
    signals.contains(&Signal::AuthorMe)
}

pub fn route(signals: &[Signal]) -> FeedRole {
    let has = |s: Signal| signals.contains(&s);
    let engaged = (has(Signal::Reviewed) || has(Signal::Commented)) && !has(Signal::AuthorMe);
    if engaged {
        FeedRole::MyReviews
    } else if has(Signal::AuthorBot) {
        FeedRole::Bots
    } else if has(Signal::DirectRequest) || has(Signal::OrgReview) {
        FeedRole::MyReviews
    } else if has(Signal::TeamRequest) {
        FeedRole::TeamReviews
    } else {
        FeedRole::MyReviews
    }
}

#[cfg(test)]
mod tests {
    #![allow(clippy::unwrap_used, clippy::expect_used)]
    use super::*;
    use crate::models::FeedRole;
    use crate::models::Signal::*;

    #[test]
    fn direct_request_to_my() {
        assert_eq!(route(&[DirectRequest]), FeedRole::MyReviews);
    }
    #[test]
    fn team_request_to_team() {
        assert_eq!(route(&[TeamRequest]), FeedRole::TeamReviews);
    }
    #[test]
    fn reviewed_to_my() {
        assert_eq!(route(&[Reviewed]), FeedRole::MyReviews);
    }
    #[test]
    fn commented_to_my() {
        assert_eq!(route(&[Commented]), FeedRole::MyReviews);
    }
    #[test]
    fn bot_to_bots() {
        assert_eq!(route(&[AuthorBot]), FeedRole::Bots);
    }

    // engaged wins over bot (resolved decision #1)
    #[test]
    fn reviewed_bot_to_my() {
        assert_eq!(route(&[Reviewed, AuthorBot]), FeedRole::MyReviews);
    }
    // but my own commented PR is not "engagement" -> bot/author rules apply
    #[test]
    fn own_comment_on_bot_is_bots() {
        assert_eq!(route(&[Commented, AuthorMe, AuthorBot]), FeedRole::Bots);
    }
    // team-requested PR I reviewed -> My (engagement wins, no leak)
    #[test]
    fn reviewed_team_to_my() {
        assert_eq!(route(&[TeamRequest, Reviewed]), FeedRole::MyReviews);
    }
    // empty -> fallback My
    #[test]
    fn empty_to_my() {
        assert_eq!(route(&[]), FeedRole::MyReviews);
    }

    #[test]
    fn org_review_to_my() {
        assert_eq!(route(&[OrgReview]), FeedRole::MyReviews);
    }

    // an org-scoped match doesn't override the bot rule, matching the
    // existing direct_request-vs-bot precedent (author_bot is checked first).
    #[test]
    fn org_review_bot_to_bots() {
        assert_eq!(route(&[OrgReview, AuthorBot]), FeedRole::Bots);
    }

    // but engagement still wins over an org-scoped bot PR.
    #[test]
    fn org_review_bot_reviewed_to_my() {
        assert_eq!(
            route(&[OrgReview, AuthorBot, Reviewed]),
            FeedRole::MyReviews
        );
    }

    // --- excluded_from_reviews (ExcludeOwnAuthored) ---

    #[test]
    fn author_me_is_excluded() {
        assert!(excluded_from_reviews(&[AuthorMe]));
    }

    #[test]
    fn empty_is_not_excluded() {
        assert!(!excluded_from_reviews(&[]));
    }

    // OnlyAuthorMe: no other signal excludes.
    #[test]
    fn no_other_signal_excludes() {
        for s in [
            DirectRequest,
            TeamRequest,
            Reviewed,
            Commented,
            AuthorBot,
            OrgReview,
        ] {
            assert!(
                !excluded_from_reviews(&[s]),
                "{s:?} must not exclude an item"
            );
        }
    }

    // OnlyAuthorMe: and no combination of other signals rescues an author_me
    // item. The exclusion is unconditional, NOT "drop from my_reviews only".
    #[test]
    fn author_me_excluded_alongside_every_other_signal() {
        for s in [
            DirectRequest,
            TeamRequest,
            Reviewed,
            Commented,
            AuthorBot,
            OrgReview,
        ] {
            assert!(
                excluded_from_reviews(&[AuthorMe, s]),
                "author_me + {s:?} must still be excluded"
            );
        }
        assert!(excluded_from_reviews(&[
            DirectRequest,
            TeamRequest,
            Reviewed,
            Commented,
            AuthorBot,
            AuthorMe,
            OrgReview,
        ]));
    }
}
