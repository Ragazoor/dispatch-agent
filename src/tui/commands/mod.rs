//! Per-domain command inner enums.
//!
//! Each module here owns one domain's side-effect commands. The outer
//! [`crate::tui::types::Command`] enum is a pure router over them — every one
//! of its variants wraps an inner enum from this module, with no inline
//! payload of its own. Keep it that way: a new side effect belongs on an
//! existing inner enum, or in a new module here, never inline in `types.rs`.

pub mod budget;
pub mod editor;
pub mod epic;
pub mod feed;
pub mod learnings;
pub mod main_session;
pub mod pr;
pub mod repo_filter;
pub mod repo_sync;
pub mod settings;
pub mod split;
pub mod system;
pub mod task;
pub mod todos;
pub mod usage;

pub use budget::BudgetCommand;
pub use editor::EditorCommand;
pub use epic::EpicCommand;
pub use feed::FeedCommand;
pub use learnings::LearningCommand;
pub use main_session::MainSessionCommand;
pub use pr::PrCommand;
pub use repo_filter::RepoFilterCommand;
pub use repo_sync::RepoSyncCommand;
pub use settings::SettingsCommand;
pub use split::SplitCommand;
pub use system::SystemCommand;
pub use task::{CleanupFollowUp, PersistFields, TaskCommand};
pub use todos::TodoCommand;
pub use usage::UsageCommand;
