//! Per-epic single-flight claim for feed cycles.
//!
//! One [`FeedSyncGuard`] instance is shared by every surface that can start a
//! feed cycle — the auto-poll [`crate::feed::FeedRunner`] and the manual "r"
//! refresh in `src/runtime/epics.rs::exec_trigger_epic_feed`. Each cycle claims
//! its epic for the whole exec → parse → sync → teardown sequence; a request
//! that finds the epic already claimed is dropped. See feeds.allium
//! `SerialisedFeedCycle` for why the claim spans the exec and why the loser is
//! dropped rather than queued.
//!
//! **Both surfaces must hold the same `Arc`.** Two instances type-check and
//! silently serialise nothing.

use std::collections::HashSet;
use std::sync::{Arc, Mutex};

use crate::models::EpicId;

/// Registry of epics with a feed cycle in flight.
///
/// A `std::sync::Mutex`, deliberately, not a `tokio::sync::Mutex`: the critical
/// section is a single `HashSet` insert or remove and is never held across an
/// `.await`. Mirrors `TuiRuntime::editor_session`.
#[derive(Debug, Default)]
pub(crate) struct FeedSyncGuard {
    in_flight: Mutex<HashSet<EpicId>>,
}

impl FeedSyncGuard {
    /// Claim `epic_id` for a feed cycle, or return `None` if a cycle for it is
    /// already in flight.
    ///
    /// The returned [`FeedClaim`] releases on drop, which is what makes the
    /// release hold across the cycle's many early returns and across a panic.
    pub(crate) fn try_claim(self: &Arc<Self>, epic_id: EpicId) -> Option<FeedClaim> {
        let mut in_flight = self.lock_in_flight();
        if !in_flight.insert(epic_id) {
            return None;
        }
        drop(in_flight);
        Some(FeedClaim {
            guard: Arc::clone(self),
            epic_id,
        })
    }

    /// Lock the registry, recovering from a poisoned mutex rather than
    /// propagating.
    ///
    /// A panic while the set was borrowed would otherwise make every epic in it
    /// permanently unclaimable — the feed would be dead until restart, which is
    /// a far worse outcome than proceeding with a set whose contents are still
    /// perfectly well-formed (the critical sections are a single insert or
    /// remove, so there is no torn intermediate state to recover from).
    fn lock_in_flight(&self) -> std::sync::MutexGuard<'_, HashSet<EpicId>> {
        match self.in_flight.lock() {
            Ok(guard) => guard,
            Err(poisoned) => poisoned.into_inner(),
        }
    }
}

/// Proof that the holder owns the in-flight feed cycle for one epic. Releases
/// the claim on drop.
#[derive(Debug)]
pub(crate) struct FeedClaim {
    guard: Arc<FeedSyncGuard>,
    epic_id: EpicId,
}

impl Drop for FeedClaim {
    fn drop(&mut self) {
        self.guard.lock_in_flight().remove(&self.epic_id);
    }
}

#[cfg(test)]
mod tests {
    #![allow(clippy::unwrap_used, clippy::expect_used)]
    use super::*;

    fn guard() -> Arc<FeedSyncGuard> {
        Arc::new(FeedSyncGuard::default())
    }

    #[test]
    fn claim_is_exclusive_per_epic() {
        let guard = guard();
        let _first = guard
            .try_claim(EpicId(1))
            .expect("first claim on a free epic must succeed");
        assert!(
            guard.try_claim(EpicId(1)).is_none(),
            "a second claim while the first is alive must be refused"
        );
    }

    #[test]
    fn claim_is_released_on_drop() {
        let guard = guard();
        drop(
            guard
                .try_claim(EpicId(1))
                .expect("first claim must succeed"),
        );
        assert!(
            guard.try_claim(EpicId(1)).is_some(),
            "dropping the claim must free the epic for the next cycle"
        );
    }

    #[test]
    fn different_epics_claim_independently() {
        let guard = guard();
        let _one = guard.try_claim(EpicId(1)).expect("epic 1 is free");
        assert!(
            guard.try_claim(EpicId(2)).is_some(),
            "a claim on one epic must not block a different epic"
        );
    }

    /// The reason the release is a `Drop` impl rather than an explicit call: a
    /// feed cycle that panics partway through must not leave its epic
    /// permanently unclaimable. Awaiting the `JoinHandle` is the deterministic
    /// completion signal — no sleep.
    #[tokio::test]
    async fn claim_is_released_when_the_holder_panics() {
        let guard = guard();
        let held = Arc::clone(&guard);

        let handle = tokio::spawn(async move {
            let _claim = held.try_claim(EpicId(1)).expect("epic 1 is free");
            panic!("cycle blew up mid-flight");
        });

        assert!(
            handle.await.is_err(),
            "the spawned task must have panicked for this test to mean anything"
        );
        assert!(
            guard.try_claim(EpicId(1)).is_some(),
            "a panicking cycle must still release its claim"
        );
    }

    /// A panic taken *while the registry mutex was held* must not wedge the
    /// feed. `lock_in_flight` recovers the poisoned guard, so the epic set
    /// remains usable.
    #[test]
    fn a_poisoned_registry_still_claims() {
        let guard = guard();
        let poisoner = Arc::clone(&guard);

        let result = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
            let _held = poisoner.lock_in_flight();
            panic!("panic with the registry locked");
        }));
        assert!(result.is_err(), "the closure must have panicked");

        assert!(
            guard.try_claim(EpicId(1)).is_some(),
            "a poisoned registry must still hand out claims, not deadlock the feed"
        );
    }
}
