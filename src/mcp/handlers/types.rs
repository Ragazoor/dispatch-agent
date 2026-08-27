use serde::{Deserialize, Serialize};
use serde_json::Value;

use crate::models::{TaskStatus, WrapUpMode};

// ---------------------------------------------------------------------------
// JSON-RPC error codes
//
// The four pre-defined codes from the JSON-RPC 2.0 spec (§5.1), plus this
// server's one application-defined code. Named so a handler reads as "this is
// an invalid-params failure" rather than as a bare magic number, and so the
// wire values live in exactly one place.
// ---------------------------------------------------------------------------

/// JSON-RPC 2.0 §5.1 — the request is not a valid Request object.
pub(super) const INVALID_REQUEST: i32 = -32600;

/// JSON-RPC 2.0 §5.1 — the method does not exist.
pub(super) const METHOD_NOT_FOUND: i32 = -32601;

/// JSON-RPC 2.0 §5.1 — invalid method parameters. The workhorse code: every
/// caller-fixable argument failure at this boundary uses it.
pub(super) const INVALID_PARAMS: i32 = -32602;

/// JSON-RPC 2.0 §5.1 — internal server error. Reserved for failures the caller
/// cannot fix by changing the request.
pub(super) const INTERNAL_ERROR: i32 = -32603;

/// Application-defined JSON-RPC error code for "entity not found", distinct
/// from [`INVALID_PARAMS`]. Reserved server-error range per the JSON-RPC 2.0
/// spec (`-32000` to `-32099`). Without this split, a caller cannot distinguish
/// "you sent garbage" from "the entity is gone" without string-matching the
/// message.
pub(super) const NOT_FOUND_CODE: i32 = -32001;

// ---------------------------------------------------------------------------
// JSON-RPC request / response types
// ---------------------------------------------------------------------------

#[derive(Debug, Deserialize)]
pub struct JsonRpcRequest {
    pub jsonrpc: String,
    pub id: Option<Value>,
    pub method: String,
    pub params: Option<Value>,
}

#[derive(Debug, Serialize)]
#[cfg_attr(test, derive(Deserialize))]
pub struct JsonRpcResponse {
    pub jsonrpc: String,
    pub id: Option<Value>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub result: Option<Value>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub error: Option<JsonRpcError>,
}

#[derive(Debug, Serialize)]
#[cfg_attr(test, derive(Deserialize))]
pub struct JsonRpcError {
    pub code: i32,
    pub message: String,
}

impl JsonRpcResponse {
    pub(super) fn ok(id: Option<Value>, result: Value) -> Self {
        JsonRpcResponse {
            jsonrpc: "2.0".to_string(),
            id,
            result: Some(result),
            error: None,
        }
    }

    pub(super) fn err(id: Option<Value>, code: i32, message: impl Into<String>) -> Self {
        JsonRpcResponse {
            jsonrpc: "2.0".to_string(),
            id,
            result: None,
            error: Some(JsonRpcError {
                code,
                message: message.into(),
            }),
        }
    }
}

// ---------------------------------------------------------------------------
// Tool-error result helper
// ---------------------------------------------------------------------------

/// Build an MCP tool-execution error result.
///
/// Per the MCP spec, tool failures are reported as `result` with
/// `isError: true` and a text content block — *not* as a JSON-RPC protocol
/// `error`. Protocol errors are reserved for malformed-request failures
/// (parse errors, unknown method, invalid request shape).
pub(super) fn tool_error(id: Option<Value>, message: impl Into<String>) -> JsonRpcResponse {
    JsonRpcResponse::ok(
        id,
        serde_json::json!({
            "isError": true,
            "content": [{"type": "text", "text": message.into()}]
        }),
    )
}

// ---------------------------------------------------------------------------
// Flexible i64 deserializer (accepts both 4 and "4")
// ---------------------------------------------------------------------------

/// Claude Code sometimes sends integer MCP arguments as strings.
/// Shared visitor that accepts both native integers and string-encoded integers.
pub(super) struct FlexibleI64Visitor;

impl<'de> serde::de::Visitor<'de> for FlexibleI64Visitor {
    type Value = i64;

    fn expecting(&self, formatter: &mut std::fmt::Formatter) -> std::fmt::Result {
        formatter.write_str("an integer or a string containing an integer")
    }

    fn visit_i64<E: serde::de::Error>(self, v: i64) -> Result<i64, E> {
        Ok(v)
    }

    fn visit_u64<E: serde::de::Error>(self, v: u64) -> Result<i64, E> {
        i64::try_from(v).map_err(|_| E::custom(format!("u64 out of i64 range: {v}")))
    }

    fn visit_str<E: serde::de::Error>(self, v: &str) -> Result<i64, E> {
        v.parse::<i64>()
            .map_err(|_| E::custom(format!("invalid integer string: {v}")))
    }
}

pub(super) fn deserialize_flexible_i64<'de, D>(deserializer: D) -> Result<i64, D::Error>
where
    D: serde::Deserializer<'de>,
{
    deserializer.deserialize_any(FlexibleI64Visitor)
}

pub(super) fn deserialize_optional_flexible_i64<'de, D>(
    deserializer: D,
) -> Result<Option<i64>, D::Error>
where
    D: serde::Deserializer<'de>,
{
    use serde::de;

    struct OptFlexI64;
    impl<'de> de::Visitor<'de> for OptFlexI64 {
        type Value = Option<i64>;
        fn expecting(&self, f: &mut std::fmt::Formatter) -> std::fmt::Result {
            f.write_str("null, an integer, or a string integer")
        }
        fn visit_none<E: de::Error>(self) -> Result<Option<i64>, E> {
            Ok(None)
        }
        fn visit_unit<E: de::Error>(self) -> Result<Option<i64>, E> {
            Ok(None)
        }
        fn visit_some<D2: serde::Deserializer<'de>>(self, d: D2) -> Result<Option<i64>, D2::Error> {
            d.deserialize_any(FlexibleI64Visitor).map(Some)
        }
    }
    deserializer.deserialize_option(OptFlexI64)
}

/// Generic nullable deserializer for `Option<Option<T>>` fields where T: Deserialize.
/// Distinguishes absent (→ outer None), JSON null (→ Some(None) = clear), value (→ Some(Some(v))).
fn deserialize_nullable<'de, D, T>(deserializer: D) -> Result<Option<Option<T>>, D::Error>
where
    D: serde::Deserializer<'de>,
    T: serde::Deserialize<'de>,
{
    use serde::de;
    use std::marker::PhantomData;

    struct Nullable<T>(PhantomData<T>);
    impl<'de, T: serde::Deserialize<'de>> de::Visitor<'de> for Nullable<T> {
        type Value = Option<Option<T>>;
        fn expecting(&self, f: &mut std::fmt::Formatter) -> std::fmt::Result {
            f.write_str("null or a value")
        }
        fn visit_none<E: de::Error>(self) -> Result<Self::Value, E> {
            Ok(Some(None))
        }
        fn visit_unit<E: de::Error>(self) -> Result<Self::Value, E> {
            Ok(Some(None))
        }
        fn visit_some<D2: serde::Deserializer<'de>>(self, d: D2) -> Result<Self::Value, D2::Error> {
            T::deserialize(d).map(|v| Some(Some(v)))
        }
    }
    deserializer.deserialize_option(Nullable(PhantomData))
}

/// Nullable string deserializer for `Option<Option<String>>` fields.
/// Used with `#[serde(default)]` to distinguish absent (→ outer None),
/// JSON null (→ Some(None) = clear), and a value (→ Some(Some(v)) = set).
pub(super) fn deserialize_nullable_string<'de, D>(
    deserializer: D,
) -> Result<Option<Option<String>>, D::Error>
where
    D: serde::Deserializer<'de>,
{
    deserialize_nullable(deserializer)
}

/// Nullable flexible i64 deserializer for `Option<Option<i64>>` fields.
/// Used with `#[serde(default)]` to distinguish absent (→ outer None),
/// JSON null (→ Some(None) = clear), and a value (→ Some(Some(v)) = set).
pub(super) fn deserialize_nullable_flexible_i64<'de, D>(
    deserializer: D,
) -> Result<Option<Option<i64>>, D::Error>
where
    D: serde::Deserializer<'de>,
{
    use serde::de;

    struct NullableFlexI64;
    impl<'de> de::Visitor<'de> for NullableFlexI64 {
        type Value = Option<Option<i64>>;
        fn expecting(&self, f: &mut std::fmt::Formatter) -> std::fmt::Result {
            f.write_str("null, an integer, or a string integer")
        }
        fn visit_none<E: de::Error>(self) -> Result<Option<Option<i64>>, E> {
            Ok(Some(None))
        }
        fn visit_unit<E: de::Error>(self) -> Result<Option<Option<i64>>, E> {
            Ok(Some(None))
        }
        fn visit_some<D2: serde::Deserializer<'de>>(
            self,
            d: D2,
        ) -> Result<Option<Option<i64>>, D2::Error> {
            d.deserialize_any(FlexibleI64Visitor).map(|v| Some(Some(v)))
        }
    }
    deserializer.deserialize_option(NullableFlexI64)
}

// ---------------------------------------------------------------------------
// Typed-id deserializers
//
// The three `*_flexible_i64` deserializers above, lifted to any `From<i64>`
// newtype (`TaskId`, `EpicId`, `LearningId`). Args structs name the newtype
// directly so the compiler — not review — catches a swapped pair of adjacent
// ids. The wire format is unchanged: these newtypes are transparent over
// `i64`, and the same flexible int-or-string acceptance applies.
// ---------------------------------------------------------------------------

pub(super) fn deserialize_flexible_id<'de, D, T>(deserializer: D) -> Result<T, D::Error>
where
    D: serde::Deserializer<'de>,
    T: From<i64>,
{
    deserialize_flexible_i64(deserializer).map(T::from)
}

pub(super) fn deserialize_optional_flexible_id<'de, D, T>(
    deserializer: D,
) -> Result<Option<T>, D::Error>
where
    D: serde::Deserializer<'de>,
    T: From<i64>,
{
    deserialize_optional_flexible_i64(deserializer).map(|v| v.map(T::from))
}

pub(super) fn deserialize_nullable_flexible_id<'de, D, T>(
    deserializer: D,
) -> Result<Option<Option<T>>, D::Error>
where
    D: serde::Deserializer<'de>,
    T: From<i64>,
{
    deserialize_nullable_flexible_i64(deserializer).map(|v| v.map(|inner| inner.map(T::from)))
}

// ---------------------------------------------------------------------------
// UrlType boundary parser
// ---------------------------------------------------------------------------

/// Parse `url_type` into [`crate::models::UrlType`] at the JSON-RPC boundary,
/// like `status`/`tag`/`sub_status` already are, instead of carrying a
/// `String` to be re-parsed at the point of use.
///
/// A hand-written deserializer rather than the derived one so the rejection
/// message still names the field and lists the accepted values — serde's
/// `unknown variant` text carries neither.
pub(super) fn deserialize_optional_url_type<'de, D>(
    deserializer: D,
) -> Result<Option<crate::models::UrlType>, D::Error>
where
    D: serde::Deserializer<'de>,
{
    use serde::de::Error as _;
    let raw = Option::<String>::deserialize(deserializer)?;
    match raw {
        None => Ok(None),
        Some(s) => crate::models::UrlType::parse(&s).map(Some).ok_or_else(|| {
            // Accepted values come from UrlType::ALL, the same source the
            // schema's `"enum"` folds, so a new variant can't leave this
            // message lying.
            let accepted = crate::models::UrlType::ALL
                .iter()
                .map(|u| u.as_str())
                .collect::<Vec<_>>()
                .join(", ");
            D::Error::custom(format!(
                "unknown url_type '{s}' (expected one of: {accepted})"
            ))
        }),
    }
}

/// Nullable WrapUpMode deserializer for `Option<Option<WrapUpMode>>` fields.
/// Used with `#[serde(default)]` to distinguish absent (→ outer None),
/// JSON null (→ Some(None) = clear), and a value (→ Some(Some(m)) = set).
pub(super) fn deserialize_nullable_wrap_up_mode<'de, D>(
    deserializer: D,
) -> Result<Option<Option<WrapUpMode>>, D::Error>
where
    D: serde::Deserializer<'de>,
{
    deserialize_nullable(deserializer)
}

// ---------------------------------------------------------------------------
// StatusFilter — accepts a single string or array of strings at the JSON-RPC
// boundary and parses each into a TaskStatus.
// ---------------------------------------------------------------------------

/// Filter for `list_tasks.status`. Deserialises from either a JSON string
/// (`"backlog"`) or a JSON array of strings (`["backlog", "ready"]`).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct StatusFilter(pub Vec<TaskStatus>);

impl StatusFilter {
    pub fn into_vec(self) -> Vec<TaskStatus> {
        self.0
    }
}

impl<'de> Deserialize<'de> for StatusFilter {
    fn deserialize<D>(deserializer: D) -> Result<StatusFilter, D::Error>
    where
        D: serde::Deserializer<'de>,
    {
        use serde::de;

        struct StatusFilterVisitor;

        fn parse_one<E: de::Error>(s: &str) -> Result<TaskStatus, E> {
            TaskStatus::parse(s).ok_or_else(|| {
                E::custom(format!(
                    "Unknown status: {s}. Valid values: backlog, running, review, done"
                ))
            })
        }

        impl<'de> de::Visitor<'de> for StatusFilterVisitor {
            type Value = StatusFilter;

            fn expecting(&self, f: &mut std::fmt::Formatter) -> std::fmt::Result {
                f.write_str("a status string or an array of status strings")
            }

            fn visit_str<E: de::Error>(self, v: &str) -> Result<StatusFilter, E> {
                Ok(StatusFilter(vec![parse_one(v)?]))
            }

            fn visit_string<E: de::Error>(self, v: String) -> Result<StatusFilter, E> {
                self.visit_str(&v)
            }

            fn visit_seq<A: de::SeqAccess<'de>>(
                self,
                mut seq: A,
            ) -> Result<StatusFilter, A::Error> {
                let mut out = Vec::new();
                while let Some(s) = seq.next_element::<String>()? {
                    out.push(parse_one::<A::Error>(&s)?);
                }
                Ok(StatusFilter(out))
            }
        }

        deserializer.deserialize_any(StatusFilterVisitor)
    }
}

// ---------------------------------------------------------------------------
// Argument parsing helper
// ---------------------------------------------------------------------------

pub(super) fn parse_args<T: serde::de::DeserializeOwned>(
    id: &Option<Value>,
    args: Value,
) -> Result<T, JsonRpcResponse> {
    serde_json::from_value(args).map_err(|e| {
        JsonRpcResponse::err(
            id.clone(),
            INVALID_PARAMS,
            format!("Invalid arguments: {e}"),
        )
    })
}

// ---------------------------------------------------------------------------
// Caller task lookup helper
// ---------------------------------------------------------------------------

/// Fetch the task identified by `caller_id` from the DB, mapping errors to
/// JSON-RPC responses. Used by handlers that resolve the caller task for
/// context inheritance (project_id, epic_id, etc.).
pub(super) async fn fetch_caller_task(
    db: &dyn crate::db::TaskReadStore,
    id: &Option<serde_json::Value>,
    caller_id: crate::models::TaskId,
) -> Result<crate::models::Task, JsonRpcResponse> {
    match db.get_task(caller_id).await {
        Ok(Some(task)) => Ok(task),
        Ok(None) => Err(JsonRpcResponse::err(
            id.clone(),
            NOT_FOUND_CODE,
            format!("Unknown caller task {}", caller_id.0),
        )),
        Err(e) => Err(JsonRpcResponse::err(
            id.clone(),
            INTERNAL_ERROR,
            format!("Database error: {e}"),
        )),
    }
}

// ---------------------------------------------------------------------------
// ServiceError → JsonRpcResponse conversion
// ---------------------------------------------------------------------------

pub(super) fn service_err_to_response(
    id: Option<Value>,
    err: crate::service::ServiceError,
) -> JsonRpcResponse {
    use crate::service::ServiceError;
    match err {
        ServiceError::Validation(msg) => JsonRpcResponse::err(id, INVALID_PARAMS, msg),
        ServiceError::NotFound(msg) => JsonRpcResponse::err(id, NOT_FOUND_CODE, msg),
        ServiceError::Internal(e) => JsonRpcResponse::err(id, INTERNAL_ERROR, e.to_string()),
    }
}

#[cfg(test)]
#[allow(clippy::unwrap_used)]
mod status_filter_tests {
    use super::StatusFilter;
    use crate::models::TaskStatus;
    use serde_json::json;

    #[test]
    fn parses_single_string() {
        let f: StatusFilter = serde_json::from_value(json!("backlog")).unwrap();
        assert_eq!(f.into_vec(), vec![TaskStatus::Backlog]);
    }

    #[test]
    fn parses_array_of_strings() {
        let f: StatusFilter = serde_json::from_value(json!(["backlog", "running"])).unwrap();
        assert_eq!(f.into_vec(), vec![TaskStatus::Backlog, TaskStatus::Running]);
    }

    #[test]
    fn parses_ready_alias() {
        let f: StatusFilter = serde_json::from_value(json!("ready")).unwrap();
        assert_eq!(f.into_vec(), vec![TaskStatus::Backlog]);
    }

    #[test]
    fn empty_array_yields_empty_vec() {
        let f: StatusFilter = serde_json::from_value(json!([])).unwrap();
        assert!(f.into_vec().is_empty());
    }

    #[test]
    fn invalid_string_errors() {
        let err = serde_json::from_value::<StatusFilter>(json!("bogus")).unwrap_err();
        assert!(err.to_string().contains("Unknown status"));
    }

    #[test]
    fn invalid_status_in_array_errors() {
        let err = serde_json::from_value::<StatusFilter>(json!(["backlog", "bogus"])).unwrap_err();
        assert!(err.to_string().contains("Unknown status"));
    }

    #[test]
    fn number_errors() {
        let err = serde_json::from_value::<StatusFilter>(json!(42)).unwrap_err();
        assert!(
            err.to_string().contains("status string") || err.to_string().contains("invalid type"),
            "got: {err}"
        );
    }
}

#[cfg(test)]
mod flexible_i64_tests {
    #![allow(clippy::unwrap_used, clippy::expect_used)]
    use super::{deserialize_flexible_i64, deserialize_optional_flexible_i64};
    use proptest::prelude::*;
    use serde::Deserialize;

    #[derive(Debug, Deserialize, PartialEq)]
    struct Wrap {
        #[serde(deserialize_with = "deserialize_flexible_i64")]
        v: i64,
    }

    #[derive(Debug, Deserialize, PartialEq)]
    struct OptWrap {
        #[serde(default, deserialize_with = "deserialize_optional_flexible_i64")]
        v: Option<i64>,
    }

    #[test]
    fn flexible_i64_rejects_non_numeric_string() {
        let json = r#"{"v":"not-a-number"}"#;
        let err = serde_json::from_str::<Wrap>(json).unwrap_err();
        assert!(err.to_string().contains("invalid integer string"));
    }

    #[test]
    fn flexible_i64_rejects_float() {
        let json = r#"{"v":1.5}"#;
        assert!(serde_json::from_str::<Wrap>(json).is_err());
    }

    #[test]
    fn flexible_i64_accepts_negative_string() {
        let parsed: Wrap = serde_json::from_str(r#"{"v":"-42"}"#).unwrap();
        assert_eq!(parsed.v, -42);
    }

    proptest! {
        /// Native integer JSON decodes losslessly via the flexible deserializer.
        #[test]
        fn flexible_i64_roundtrip_integer(v: i64) {
            let json = format!(r#"{{"v":{v}}}"#);
            let parsed: Wrap = serde_json::from_str(&json).unwrap();
            prop_assert_eq!(parsed.v, v);
        }

        /// String-encoded integer JSON decodes to the same i64 — this is the
        /// path Claude Code occasionally takes for MCP integer arguments.
        #[test]
        fn flexible_i64_roundtrip_string(v: i64) {
            let json = format!(r#"{{"v":"{v}"}}"#);
            let parsed: Wrap = serde_json::from_str(&json).unwrap();
            prop_assert_eq!(parsed.v, v);
        }

        /// Both encodings yield identical results.
        #[test]
        fn flexible_i64_int_and_string_agree(v: i64) {
            let from_int: Wrap = serde_json::from_str(&format!(r#"{{"v":{v}}}"#)).unwrap();
            let from_str: Wrap = serde_json::from_str(&format!(r#"{{"v":"{v}"}}"#)).unwrap();
            prop_assert_eq!(from_int.v, from_str.v);
        }

        /// The optional variant carries the same flexibility through `Some(v)`,
        /// while still treating `null` and absent as `None`.
        #[test]
        fn optional_flexible_i64_roundtrip(v: i64) {
            let from_int: OptWrap = serde_json::from_str(&format!(r#"{{"v":{v}}}"#)).unwrap();
            let from_str: OptWrap = serde_json::from_str(&format!(r#"{{"v":"{v}"}}"#)).unwrap();
            prop_assert_eq!(from_int.v, Some(v));
            prop_assert_eq!(from_str.v, Some(v));
        }
    }

    #[test]
    fn optional_flexible_i64_handles_null_and_absent() {
        let null_case: OptWrap = serde_json::from_str(r#"{"v":null}"#).unwrap();
        assert_eq!(null_case.v, None);
        let absent_case: OptWrap = serde_json::from_str(r#"{}"#).unwrap();
        assert_eq!(absent_case.v, None);
    }
}

#[cfg(test)]
#[allow(clippy::unwrap_used)]
mod fetch_caller_task_tests {
    use super::fetch_caller_task;
    use crate::db::{CreateTaskRequest, Database, TaskCrud};
    use crate::models::{TaskId, TaskStatus};
    use serde_json::json;

    #[tokio::test]
    async fn returns_task_when_found() {
        let db = Database::open_in_memory().await.unwrap();
        let task_id = db
            .create_task(CreateTaskRequest {
                title: "caller",
                description: "",
                repo_path: "/repo",
                plan: None,
                status: TaskStatus::Running,
                base_branch: "main",
                epic_id: None,
                sort_order: None,
                tag: None,
                wrap_up_mode: None,
                auto_run_plan: false,
            })
            .await
            .unwrap();

        let result = fetch_caller_task(&db, &Some(json!(1)), task_id).await;
        let task = result.unwrap();
        assert_eq!(task.id, task_id);
        assert_eq!(task.title, "caller");
    }

    #[tokio::test]
    async fn returns_not_found_error_when_not_found() {
        let db = Database::open_in_memory().await.unwrap();

        let result = fetch_caller_task(&db, &Some(json!(1)), TaskId(99999)).await;
        let err_resp = result.unwrap_err();
        let err = err_resp.error.unwrap();
        assert_eq!(err.code, super::NOT_FOUND_CODE);
        assert!(err.message.contains("99999"), "got: {}", err.message);
    }
}

#[cfg(test)]
#[allow(clippy::unwrap_used, clippy::expect_used)]
mod error_code_tests {
    /// The named constants are a readability change only — the wire values are
    /// the JSON-RPC 2.0 §5.1 numbers and clients match on them, so pinning them
    /// here turns a rename-and-retype slip into a test failure rather than a
    /// silently different error code.
    #[test]
    fn named_codes_match_the_json_rpc_spec_values() {
        assert_eq!(super::INVALID_REQUEST, -32600);
        assert_eq!(super::METHOD_NOT_FOUND, -32601);
        assert_eq!(super::INVALID_PARAMS, -32602);
        assert_eq!(super::INTERNAL_ERROR, -32603);
    }
}

#[cfg(test)]
#[allow(clippy::unwrap_used, clippy::expect_used)]
mod typed_id_tests {
    use super::{
        deserialize_flexible_id, deserialize_nullable_flexible_id,
        deserialize_optional_flexible_id, deserialize_optional_url_type,
    };
    use crate::models::{EpicId, TaskId, UrlType};
    use serde::Deserialize;
    use serde_json::json;

    #[derive(Debug, Deserialize, PartialEq)]
    struct Ids {
        #[serde(deserialize_with = "deserialize_flexible_id")]
        task_id: TaskId,
        #[serde(default, deserialize_with = "deserialize_optional_flexible_id")]
        epic_id: Option<EpicId>,
        #[serde(default, deserialize_with = "deserialize_nullable_flexible_id")]
        parent_epic_id: Option<Option<EpicId>>,
    }

    /// Typed ids keep the flexible int-or-string acceptance the raw `i64`
    /// deserializers had — Claude Code sends either encoding.
    #[test]
    fn typed_ids_accept_both_integer_and_string_encodings() {
        let from_int: Ids =
            serde_json::from_value(json!({"task_id": 7, "epic_id": 3, "parent_epic_id": 9}))
                .unwrap();
        let from_str: Ids =
            serde_json::from_value(json!({"task_id": "7", "epic_id": "3", "parent_epic_id": "9"}))
                .unwrap();
        assert_eq!(from_int, from_str);
        assert_eq!(from_int.task_id, TaskId(7));
        assert_eq!(from_int.epic_id, Some(EpicId(3)));
        assert_eq!(from_int.parent_epic_id, Some(Some(EpicId(9))));
    }

    /// The nullable form keeps its three-way distinction after the newtype
    /// lift: absent is not the same as an explicit `null`.
    #[test]
    fn nullable_typed_id_distinguishes_absent_from_null() {
        let absent: Ids = serde_json::from_value(json!({"task_id": 1})).unwrap();
        assert_eq!(absent.parent_epic_id, None);
        assert_eq!(absent.epic_id, None);

        let null: Ids =
            serde_json::from_value(json!({"task_id": 1, "parent_epic_id": null})).unwrap();
        assert_eq!(null.parent_epic_id, Some(None));
    }

    /// Wire format is unchanged: these are transparent newtypes, so a typed id
    /// serialises to the same bare integer a raw `i64` did.
    #[test]
    fn typed_ids_serialise_as_bare_integers() {
        assert_eq!(serde_json::to_value(TaskId(42)).unwrap(), json!(42));
        assert_eq!(serde_json::to_value(EpicId(-1)).unwrap(), json!(-1));
    }

    #[derive(Debug, Deserialize, PartialEq)]
    struct WithUrlType {
        #[serde(default, deserialize_with = "deserialize_optional_url_type")]
        url_type: Option<UrlType>,
    }

    #[test]
    fn url_type_parses_at_the_boundary() {
        let parsed: WithUrlType = serde_json::from_value(json!({"url_type": "pr"})).unwrap();
        assert_eq!(parsed.url_type, Some(UrlType::Pr));
        let absent: WithUrlType = serde_json::from_value(json!({})).unwrap();
        assert_eq!(absent.url_type, None);
    }

    /// The rejection message must still name the field and the accepted set —
    /// serde's derived `unknown variant` text carries neither.
    #[test]
    fn unknown_url_type_names_the_field_and_accepted_values() {
        let err = serde_json::from_value::<WithUrlType>(json!({"url_type": "bogus"})).unwrap_err();
        let msg = err.to_string();
        assert!(msg.contains("url_type"), "got: {msg}");
        assert!(msg.contains("security_alert"), "got: {msg}");
    }
}

#[cfg(test)]
#[allow(clippy::unwrap_used)]
mod service_err_to_response_tests {
    use super::service_err_to_response;
    use crate::service::ServiceError;

    // `NotFound` ("the entity is gone") and `Validation` ("you sent garbage")
    // are semantically distinct failures; callers need different codes to
    // react differently (e.g. retry-with-different-id vs fix-the-request).
    #[tokio::test]
    async fn not_found_and_validation_have_distinct_codes() {
        let not_found = service_err_to_response(None, ServiceError::NotFound("x".into()));
        let validation = service_err_to_response(None, ServiceError::Validation("y".into()));
        let nf_code = not_found.error.unwrap().code;
        let val_code = validation.error.unwrap().code;
        assert_ne!(nf_code, val_code);
        assert_eq!(nf_code, super::NOT_FOUND_CODE);
        assert_eq!(val_code, -32602);
    }
}
