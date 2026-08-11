//! Background learning-maintenance side-effect commands.

/// Side-effect commands for background learning maintenance. The TUI no
/// longer has an interactive learnings surface — learnings are curated
/// exclusively via MCP — so this seam now carries only the background sweep.
///
/// Wrapped by [`crate::tui::types::Command::Learning`] for runtime dispatch.
#[derive(Debug, Clone)]
pub enum LearningCommand {
    /// Background stale-learning sweep: archive approved entries with a
    /// non-positive score that have gone untouched past the configured
    /// threshold. Emitted from the tick loop, gated by
    /// [`crate::tui::STALE_LEARNING_CLEANUP_ENABLED`] and
    /// [`crate::tui::STALE_CLEANUP_INTERVAL`]. See
    /// docs/specs/learnings.allium: ArchiveStaleLearning.
    ArchiveStale,
}
