//! PR flow messages (creation is agent-driven via the `/wrap-up` skill;
//! these messages cover status polling).

use crate::models::{ReviewDecision, TaskId};

use crate::tui::types::Command;
use crate::tui::App;

/// Messages targeting the PR flow.
///
/// Wrapped by [`crate::tui::types::Message::Pr`] for dispatch.
#[derive(Debug, Clone)]
pub enum PrMessage {
    /// PR for a task has merged upstream.
    Merged(TaskId),
    /// PR for a task was closed without merging.
    Closed(TaskId),
    /// Review-state poll result for a task's PR.
    ReviewState {
        id: TaskId,
        review_decision: Option<ReviewDecision>,
    },
    /// A PR read failed. `permanent` is the whole reason this message exists:
    /// it says whether repeating the call could ever answer differently, and so
    /// whether this failure counts towards giving up on the task
    /// (`pr-workflow.allium`: `PollPrStatus`).
    CheckFailed {
        id: TaskId,
        permanent: bool,
        error: String,
    },
}

impl PrMessage {
    /// Route this message to its handler on [`App`]. See [`super::SplitMessage::route`].
    pub(in crate::tui) fn route(self, app: &mut App) -> Vec<Command> {
        match self {
            PrMessage::Merged(id) => app.handle_pr_merged(id),
            PrMessage::Closed(id) => app.handle_pr_closed(id),
            PrMessage::ReviewState {
                id,
                review_decision,
            } => app.handle_pr_review_state(id, review_decision),
            PrMessage::CheckFailed {
                id,
                permanent,
                error,
            } => app.handle_pr_check_failed(id, permanent, error),
        }
    }
}
