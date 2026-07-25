//! Task service: CRUD, validation, and parameter shapes split across
//! submodules to keep navigation tractable. The public surface is unchanged
//! — call sites continue to import from `crate::service::tasks`.

mod crud;
mod params;
mod validators;
mod watchers;

pub use crud::{TaskService, UpdateTaskResult};
pub use params::{ClaimTaskParams, CreateTaskParams, ListTasksFilter, UpdateTaskParams};
pub use watchers::SubscribeOutcome;

#[cfg(test)]
mod tests;
