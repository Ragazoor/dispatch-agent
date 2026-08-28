pub mod api;
pub mod clock;
pub mod embeddings;
pub mod epics;
mod grouping;
pub mod learnings;
pub mod managed_feeds;
pub mod tasks;
pub mod todos;
pub mod usage;

pub use api::{EpicServiceApi, LearningServiceApi, TaskServiceApi, TodoServiceApi};
#[cfg(test)]
pub use api::{LearningServiceApiStub, MockLearningService, TaskServiceApiStub};
pub use clock::{Clock, FixedClock, SystemClock};
pub use epics::{CreateEpicParams, EpicService, UpdateEpicParams, UpdateEpicResult};
pub use grouping::{flatten_epic, regroup_epic, reroute_on_repo_change, route_target};
pub use learnings::{CreateLearningParams, LearningService, QueryLearningsParams};
pub use managed_feeds::{
    ensure_managed_epics, provision_managed_feeds_from_settings, read_managed_feed_settings,
    write_managed_feed_settings, ManagedFeedSettings, ManagedFeedSettingsPatch,
};
pub use tasks::{
    CloseSessionOutcome, ClosedSession, CreateTaskParams, DispatchClaim, DispatchOutcome,
    DispatchRequest, ListTasksFilter, SubscribeOutcome, TaskService, UpdateTaskParams,
    UpdateTaskResult, WrapUpRebaseOutcome,
};
pub use todos::{TodoService, TodoUpdate};
pub use usage::record_usage_event_logged;

// ---------------------------------------------------------------------------
// Service error
// ---------------------------------------------------------------------------

#[derive(Debug)]
pub enum ServiceError {
    /// Client-provided data is invalid (bad status, missing fields, etc.)
    Validation(String),
    /// Entity not found
    NotFound(String),
    /// Database or internal error; preserves the underlying anyhow source chain.
    Internal(anyhow::Error),
}

impl std::fmt::Display for ServiceError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            ServiceError::Validation(msg) => write!(f, "{msg}"),
            ServiceError::NotFound(msg) => write!(f, "{msg}"),
            ServiceError::Internal(e) => write!(f, "{e}"),
        }
    }
}

impl std::error::Error for ServiceError {
    fn source(&self) -> Option<&(dyn std::error::Error + 'static)> {
        match self {
            ServiceError::Internal(e) => Some(e.as_ref()),
            _ => None,
        }
    }
}

impl From<anyhow::Error> for ServiceError {
    fn from(e: anyhow::Error) -> Self {
        ServiceError::Internal(e)
    }
}

// ---------------------------------------------------------------------------
// The feed-cadence floor
// ---------------------------------------------------------------------------

/// Reject a feed cadence below [`crate::models::MIN_FEED_INTERVAL_SECS`].
///
/// The single enforcement point for the floor, called by every service write
/// path that can set one: `EpicService::create_epic`, `EpicService::update_epic`
/// and `write_managed_feed_settings`. `None` means "unset", which resolves to
/// `config.default_feed_interval` and is always legal.
///
/// This lives in the service layer, not in the interval grammar, because the
/// floor is a domain invariant of the field rather than a spelling rule — see
/// "Interval literals" in `docs/specs/core.allium`, which states the two claims
/// separately and says why. A grammar that also enforced the floor would be a
/// second place it could drift out of step, and it would not bind the MCP
/// integer paths at all.
///
/// `field` names the caller's own parameter, because the same invariant is
/// spelled three ways across the surfaces (`feed_interval_secs`,
/// `reviews_interval_secs`, `cve_interval_secs`) and an error naming the wrong
/// one sends the reader looking in the wrong place.
pub fn validate_feed_interval(field: &str, secs: Option<i64>) -> Result<(), ServiceError> {
    match secs {
        Some(s) if s < crate::models::MIN_FEED_INTERVAL_SECS => {
            Err(ServiceError::Validation(format!(
                "{field} must be at least {min} seconds (got {s})",
                min = crate::models::MIN_FEED_INTERVAL_SECS
            )))
        }
        _ => Ok(()),
    }
}

// ---------------------------------------------------------------------------
// FieldUpdate — explicit set-or-clear for nullable string fields
// ---------------------------------------------------------------------------

/// Replaces the `Option<String>` + empty-string sentinel pattern.
/// `Set(value)` sets the field, `Clear` sets it to NULL.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum FieldUpdate {
    Set(String),
    Clear,
}

impl FieldUpdate {
    /// Translate the legacy MCP convention where `Some("")` means "clear to
    /// NULL", `Some(v)` means "set to v", and `None` means "do not touch" into
    /// an `Option<FieldUpdate>` patch value.
    pub fn from_optional_string(s: Option<String>) -> Option<FieldUpdate> {
        match s {
            None => None,
            Some(v) if v.is_empty() => Some(FieldUpdate::Clear),
            Some(v) => Some(FieldUpdate::Set(v)),
        }
    }

    /// Build a `FieldUpdate` from an editor section string, where an empty
    /// string clears the field and any other value sets it. This is the
    /// editor's "the section is always present, so absent means clear"
    /// convention (`apply_task_editor_fields`, `apply_epic_editor_fields`).
    pub fn from_string(s: String) -> FieldUpdate {
        if s.is_empty() {
            FieldUpdate::Clear
        } else {
            FieldUpdate::Set(s)
        }
    }

    /// Convert to `Option<&str>` for use with DB patch builders.
    /// `Set(s)` → `Some(s)`, `Clear` → `None`.
    pub fn as_option(&self) -> Option<&str> {
        match self {
            FieldUpdate::Set(s) => Some(s.as_str()),
            FieldUpdate::Clear => None,
        }
    }
}

/// Set-or-clear for the typed URL field. Mirrors [`FieldUpdate`] but carries a
/// whole [`TaskUrl`](crate::models::TaskUrl) so the url and its type are always
/// updated together.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum UrlUpdate {
    Set(crate::models::TaskUrl),
    Clear,
}

impl UrlUpdate {
    /// `Set(u)` → `Some(&u)`, `Clear` → `None`, for the DB patch builder.
    pub fn as_option(&self) -> Option<&crate::models::TaskUrl> {
        match self {
            UrlUpdate::Set(u) => Some(u),
            UrlUpdate::Clear => None,
        }
    }
}

/// Set-or-clear for the typed tmux-window field. Mirrors [`UrlUpdate`] but
/// carries a [`TmuxWindow`](crate::models::TmuxWindow), so a caller cannot
/// point a task at a repo path, a branch name, or a half-typed window name —
/// see that type for why a bare `String` there is a live hazard.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum TmuxWindowUpdate {
    Set(crate::models::TmuxWindow),
    Clear,
}

impl TmuxWindowUpdate {
    /// `Set(w)` → `Some(&w)`, `Clear` → `None`, for the DB patch builder.
    pub fn as_option(&self) -> Option<&crate::models::TmuxWindow> {
        match self {
            TmuxWindowUpdate::Set(w) => Some(w),
            TmuxWindowUpdate::Clear => None,
        }
    }
}

#[cfg(test)]
#[allow(clippy::unwrap_used, clippy::expect_used)]
mod error_tests {
    use super::ServiceError;
    use std::error::Error;

    #[test]
    fn internal_error_preserves_source_chain() {
        let anyhow_err = anyhow::anyhow!("db connection failed");
        let service_err = ServiceError::from(anyhow_err);
        assert!(
            service_err.source().is_some(),
            "ServiceError::Internal should expose its anyhow error as source()"
        );
    }
}

#[cfg(test)]
mod field_update_tests {
    use super::FieldUpdate;

    #[test]
    fn from_optional_string_none_is_noop() {
        assert_eq!(FieldUpdate::from_optional_string(None), None);
    }

    #[test]
    fn as_option_set_returns_some() {
        let fu = FieldUpdate::Set("https://example.com".to_string());
        assert_eq!(fu.as_option(), Some("https://example.com"));
    }

    #[test]
    fn as_option_clear_returns_none() {
        assert_eq!(FieldUpdate::Clear.as_option(), None);
    }

    #[test]
    fn from_optional_string_empty_clears() {
        assert_eq!(
            FieldUpdate::from_optional_string(Some(String::new())),
            Some(FieldUpdate::Clear)
        );
    }

    #[test]
    fn from_optional_string_value_sets() {
        assert_eq!(
            FieldUpdate::from_optional_string(Some("https://x".to_string())),
            Some(FieldUpdate::Set("https://x".to_string()))
        );
    }

    #[test]
    fn from_string_empty_clears() {
        assert_eq!(FieldUpdate::from_string(String::new()), FieldUpdate::Clear);
    }

    #[test]
    fn from_string_value_sets() {
        assert_eq!(
            FieldUpdate::from_string("docs/plan.md".to_string()),
            FieldUpdate::Set("docs/plan.md".to_string())
        );
    }
}
