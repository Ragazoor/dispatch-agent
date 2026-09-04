use super::*;

impl TuiRuntime {
    pub(super) fn exec_check_pr_status(
        &self,
        id: TaskId,
        url: String,
    ) -> tokio::task::JoinHandle<()> {
        let tx = self.msg_tx.clone();
        let runner = self.runner.clone();

        tokio::task::spawn_blocking(move || match dispatch::check_pr_status(&url, &*runner) {
            Ok(status) => match status.state {
                dispatch::PrState::Merged => {
                    let _ = tx.send(Message::Pr(crate::tui::messages::PrMessage::Merged(id)));
                }
                dispatch::PrState::Closed => {
                    let _ = tx.send(Message::Pr(crate::tui::messages::PrMessage::Closed(id)));
                }
                dispatch::PrState::Open => {
                    let _ = tx.send(Message::Pr(crate::tui::messages::PrMessage::ReviewState {
                        id,
                        review_decision: status.review_decision,
                    }));
                }
            },
            // Deliberately NOT logged here. This ran once per task per
            // PR_POLL_INTERVAL, so a permanently unreadable PR warned every 30
            // seconds — 63,000 identical lines from five tasks over five
            // months. The failure now travels to the update loop, which counts
            // it and warns once, on the transition into giving up
            // (pr-workflow.allium: PrPollGaveUp).
            Err(failure) => {
                let _ = tx.send(Message::Pr(crate::tui::messages::PrMessage::CheckFailed {
                    id,
                    permanent: failure.is_permanent(),
                    error: failure.to_string(),
                }));
            }
        })
    }
}
