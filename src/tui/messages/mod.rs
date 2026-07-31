//! Per-domain message inner enums.
//!
//! Variants of the outer [`crate::tui::types::Message`] enum are progressively
//! migrated into per-domain inner enums to keep the dispatcher manageable as
//! the TUI grows. Each module here owns one domain's messages.

pub mod editor;
pub mod epic;
pub mod feed;
pub mod input;
pub mod learnings;
pub mod main_session;
pub mod managed_feeds;
pub mod pr;
pub mod repo_filter;
pub mod repo_sync;
pub mod split;
pub mod system;
pub mod task;
pub mod tips;
pub mod todos;
pub mod wrap_up;

pub use editor::EditorMessage;
pub use epic::EpicMessage;
pub use feed::FeedMessage;
pub use input::InputMessage;
pub use learnings::LearningMessage;
pub use main_session::MainSessionMessage;
pub use managed_feeds::{ManagedFeedConfigMessage, ManagedFeedField};
pub use pr::PrMessage;
pub use repo_filter::RepoFilterMessage;
pub use repo_sync::RepoSyncMessage;
pub use split::SplitMessage;
pub use system::SystemMessage;
pub use task::TaskMessage;
pub use tips::TipsMessage;
pub use todos::TodoMessage;
pub use wrap_up::WrapUpMessage;
