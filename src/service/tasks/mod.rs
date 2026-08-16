//! Task service: CRUD, validation, and parameter shapes split across
//! submodules to keep navigation tractable. The public surface is unchanged
//! — call sites continue to import from `crate::service::tasks`.

mod crud;
mod dispatch;
mod params;
mod validators;
mod watchers;
mod wrap_up;

pub use crud::{CloseSessionOutcome, ClosedSession, TaskService, UpdateTaskResult};
pub use dispatch::{DispatchClaim, DispatchOutcome, DispatchRequest};
pub use params::{CreateTaskParams, ListTasksFilter, UpdateTaskParams};
pub use watchers::SubscribeOutcome;
pub use wrap_up::WrapUpRebaseOutcome;

#[cfg(test)]
mod tests;
