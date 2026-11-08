//! Wire ops — the ndjson JSON-RPC convention every sibling `serve`
//! speaks, applied to pavo drivers.
//!
//! Transport: one JSON object per line on stdin/stdout (CLI/TUI) or
//! bridged to ws/http by `pavo serve` (web). Requests are
//! `{"op":"…", …}`; responses are `{"ok":true,"result":…}` or
//! `{"ok":false,"error":"…"}`. This is the same shape as
//! `tamias-serve`, `myco serve`, `cuora serve`, `sepia serve`.
//!
//! Ops (v1):
//!
//! | op        | params                              | result            |
//! |-----------|-------------------------------------|-------------------|
//! | `manifest`| —                                   | [`DriverManifest`]|
//! | `views`   | —                                   | `Vec<ViewSummary>`|
//! | `view`    | `view`, `params?`                   | [`ViewResult`]    |
//! | `query`   | `query`, `params?`                  | [`QueryResult`]   |
//!
//! Thin drivers may proxy a product's existing `serve` for `query`
//! payloads; `manifest`/`views`/`view` are always answered by the
//! driver itself.

use serde::{Deserialize, Serialize};
use serde_json::{Map, Value};

use crate::manifest::{DriverManifest, ViewSpec};

/// A wire request — one JSON object per line, discriminated by `op`.
///
/// Unknown ops fail deserialization; hosts should treat that as a
/// protocol error and surface the driver's raw error text.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(tag = "op", rename_all = "lowercase")]
pub enum Request {
    /// `{"op":"manifest"}` → [`DriverManifest`]. Discovery verb —
    /// cheap, must not require product state.
    Manifest,
    /// `{"op":"views"}` → `Vec<ViewSummary>` — view ids/titles/routes
    /// without the full trees (lightweight listing).
    Views,
    /// `{"op":"view","view":"board","params":{…}}` → [`ViewResult`] —
    /// the resolved tree plus query-fed data for one view.
    View {
        /// View id from the driver's manifest (`board`, `item`).
        view: String,
        /// Caller-supplied params merged into each query binding.
        #[serde(default, skip_serializing_if = "Map::is_empty")]
        params: Map<String, Value>,
    },
    /// `{"op":"query","query":"items","params":{…}}` → [`QueryResult`] —
    /// execute a single named query binding (or raw product op, for
    /// thin drivers) and return its data payload.
    Query {
        /// Query name from the view's `queries` bindings, or a raw
        /// product op when the driver proxies `serve`.
        query: String,
        /// Params merged into the binding's static params.
        #[serde(default, skip_serializing_if = "Map::is_empty")]
        params: Map<String, Value>,
    },
}

/// A wire response — one JSON object per line.
///
/// `ok:true` carries `result`; `ok:false` carries `error`. Fields
/// for the other arm are absent (not null) so consumers can match on
/// `ok` alone.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct Response {
    /// Success flag — the only field guaranteed present.
    pub ok: bool,
    /// Result payload when `ok` is true.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub result: Option<Value>,
    /// Human/machine-readable error string when `ok` is false.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub error: Option<String>,
}

impl Response {
    /// Build a success response around an arbitrary JSON result.
    #[must_use]
    pub fn ok(result: Value) -> Self {
        Self {
            ok: true,
            result: Some(result),
            error: None,
        }
    }

    /// Build a success response from any serializable payload.
    ///
    /// Serialization failure is reported as an error response rather
    /// than panicking — a driver must never crash the wire.
    #[must_use]
    pub fn ok_typed<T: Serialize>(value: &T) -> Self {
        match serde_json::to_value(value) {
            Ok(v) => Self::ok(v),
            Err(e) => Self::err(format!("result serialization failed: {e}")),
        }
    }

    /// Build an error response.
    #[must_use]
    pub fn err(message: impl Into<String>) -> Self {
        Self {
            ok: false,
            result: None,
            error: Some(message.into()),
        }
    }

    /// Typed accessor — the result payload deserialized as `T`.
    ///
    /// # Errors
    /// Returns the serde error when `result` is absent or doesn't
    /// match `T`.
    pub fn result_as<T: for<'de> Deserialize<'de>>(&self) -> Result<T, serde_json::Error> {
        match &self.result {
            Some(v) => serde_json::from_value(v.clone()),
            None => serde_json::from_value(Value::Null),
        }
    }
}

/// Result of the `views` op — a [`ViewSpec`] minus the `root` tree,
/// for cheap listing. Renderers fetch full trees via `view`.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct ViewSummary {
    /// View id.
    pub id: String,
    /// Human-facing title.
    pub title: String,
    /// Route the view answers.
    pub route: String,
    /// Optional description.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub description: Option<String>,
    /// Names of the query bindings this view uses.
    #[serde(default)]
    pub queries: Vec<String>,
}

impl From<&ViewSpec> for ViewSummary {
    fn from(spec: &ViewSpec) -> Self {
        Self {
            id: spec.id.clone(),
            title: spec.title.clone(),
            route: spec.route.clone(),
            description: spec.description.clone(),
            queries: spec.queries.iter().map(|q| q.name.clone()).collect(),
        }
    }
}

/// Result of the `view` op — the resolved view: the declarative tree
/// plus every query binding's data, keyed by binding name.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct ViewResult {
    /// The view's manifest spec (tree included).
    pub view: ViewSpec,
    /// Query results keyed by binding name — `data["items"]` feeds
    /// every node whose `base.query == "items"`. Bindings that failed
    /// appear with an `{"error": "…"}` payload rather than failing the
    /// whole view (partial degradation).
    #[serde(default)]
    pub data: Map<String, Value>,
}

/// Result of the `query` op — one binding's data payload.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct QueryResult {
    /// The query/binding name that produced this payload.
    pub query: String,
    /// The data — shape is defined by the driver/product; hosts pass
    /// it through to renderers verbatim.
    pub data: Value,
}

/// Re-export for convenience — the `manifest` op's result type.
pub type ManifestResult = DriverManifest;

/// Errors pavo-protocol itself can produce (parsing helpers, not the
/// wire — wire errors travel as `Response::err` strings).
#[derive(Debug, thiserror::Error)]
pub enum WireError {
    /// The line wasn't valid JSON for the expected type.
    #[error("invalid wire JSON: {0}")]
    Json(#[from] serde_json::Error),
    /// The request `op` isn't in the v1 op set.
    #[error("unknown op: {op}")]
    UnknownOp {
        /// The unrecognized op string.
        op: String,
    },
}

/// Parse one ndjson line into a [`Request`].
///
/// # Errors
/// [`WireError::Json`] on malformed JSON; [`WireError::UnknownOp`]
/// when `op` is missing or not a v1 op.
pub fn parse_request(line: &str) -> Result<Request, WireError> {
    let value: Value = serde_json::from_str(line)?;
    let op = value
        .get("op")
        .and_then(Value::as_str)
        .ok_or_else(|| WireError::UnknownOp {
            op: "<missing>".to_string(),
        })?;
    match op {
        "manifest" | "views" | "view" | "query" => Ok(serde_json::from_value(value)?),
        other => Err(WireError::UnknownOp {
            op: other.to_string(),
        }),
    }
}

/// Serialize a [`Response`] to its single-line ndjson form.
///
/// # Errors
/// [`WireError::Json`] if serialization fails (should not happen for
/// well-formed responses).
pub fn response_line(response: &Response) -> Result<String, WireError> {
    Ok(serde_json::to_string(response)?)
}
