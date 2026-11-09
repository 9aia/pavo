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
//! | `render`  | `kind`, `node`, `data`, `surface`,  | [`RenderModel`]   |
//! |           | `format?`                           |                   |
//! | `submit`  | `view`, `form?`, `values`           | [`SubmitResult`]  |
//! | `jobs`    | —                                   | `Vec<JobItem>`    |
//! | `notifications` | —                          | `Vec<NotificationItem>` |
//!
//! `jobs` and `notifications` are the **platform-feed ops**: drivers
//! whose manifest declares the matching `contributes` slot
//! (`jobs`/`notifications`) answer them with item lists — see
//! [`crate::platform`]. Both are optional capabilities: drivers
//! without the feed answer `ok:false` (or `[]`) per the usual
//! convention, and hosts treat either as "no feed".
//!
//! `submit` is the form-interaction verb: a renderer collects the
//! editable [`crate::view_node::FormField`] values of a `form` node
//! and posts them here. `view` names the view the form lives in;
//! `form` optionally names the form node's `id` when a view carries
//! several forms (absent = the driver's default/first form);
//! `values` maps field `key` → submitted JSON value. The driver
//! dispatches on the form's `submit_op`; the result may carry a
//! message, per-field validation `errors`, driver `data`, and an
//! updated `view` for re-render.
//!
//! `render` is the view-kind renderer-plugin verb: a driver whose
//! manifest declares `renderers` answers it with a canonical
//! [`crate::render_model::RenderModel`] for one node on one surface.
//! Drivers that declare no renderers may ignore it (hosts only call
//! it for kinds the manifest claims).
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
    /// `{"op":"render","kind":"graph","node":{…},"data":{…},
    /// "surface":"cli","format":"svg"}` → [`RenderModel`] — the
    /// renderer-plugin verb. `node` is the raw ViewNode JSON (the
    /// host passes it untyped so unknown/future fields survive);
    /// `data` is the view's query-binding map verbatim; `surface` is
    /// `"cli"`, `"tui"`, or `"web"` (open string — hosts may add
    /// surfaces); `format` is the surface's format hint (`"svg"`,
    /// `"ascii"`, …) when one applies.
    Render {
        /// The node's `kind` — also the renderer claim key.
        kind: String,
        /// The node as raw JSON (a ViewNode object, `{"kind":…}`).
        #[serde(default)]
        node: Value,
        /// The view's bound query data — the same map `view` results
        /// carry, so the plugin can resolve `node.query` itself.
        #[serde(default)]
        data: Map<String, Value>,
        /// Target surface — `"cli"`, `"tui"`, `"web"`.
        surface: String,
        /// Optional format hint (CLI `--format` value, etc.).
        #[serde(default, skip_serializing_if = "Option::is_none")]
        format: Option<String>,
    },
    /// `{"op":"submit","view":"settings","form":"prefs",
    /// "values":{"theme":"dark"}}` → [`SubmitResult`] — the
    /// form-submission verb. `view` is the view id the form node
    /// belongs to; `form` optionally names the form node's `id`
    /// (its `NodeBase::id`) when the view has several forms —
    /// drivers resolve the node to read its `submit_op` and field
    /// declarations. `values` maps each submitted field's `key` to
    /// its JSON value (`string`/`number`/`bool` per field `kind`).
    /// Drivers that don't implement forms answer `ok:false`.
    Submit {
        /// View id from the driver's manifest — same namespace as
        /// the `view` op.
        view: String,
        /// Optional form-node id (`NodeBase::id` of the `form`
        /// node); absent submits the view's default form.
        #[serde(default, skip_serializing_if = "Option::is_none")]
        form: Option<String>,
        /// Submitted field values keyed by `FormField::key`. Keys
        /// the driver doesn't know are ignored; missing `required`
        /// fields come back in [`SubmitResult::errors`].
        #[serde(default, skip_serializing_if = "Map::is_empty")]
        values: Map<String, Value>,
    },
    /// `{"op":"jobs"}` → `Vec<JobItem>` — the job/queue feed op.
    /// Drivers that declare `contributes.jobs` answer with their
    /// current job list; drivers without a job feed answer `ok:false`
    /// (or an empty list) — both mean "no feed" to hosts.
    Jobs,
    /// `{"op":"notifications"}` → `Vec<NotificationItem>` — the
    /// notification feed op. Same optional-capability convention as
    /// `jobs`: `contributes.notifications` marks the provider, the
    /// op carries the items.
    Notifications,
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

/// Result of the `submit` op — what a form submission produced.
///
/// Every field is optional so drivers answer with only what they
/// have: a bare `{"message":"saved"}` is a complete result. Hosts
/// treat `errors` non-empty as a validation failure (the form should
/// re-render with the messages, not navigate away); `view`, when
/// present, is a fresh [`ViewResult`] to swap in.
#[derive(Clone, Debug, Default, PartialEq, Serialize, Deserialize)]
pub struct SubmitResult {
    /// Human/machine-facing outcome (`"saved myco.theme"`,
    /// `"applied 2 keys"`). Renderers show it as a status line.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub message: Option<String>,
    /// Per-field validation errors keyed by `FormField::key` —
    /// non-empty means the submit was rejected and the form should
    /// re-render with these messages. Values are driver-defined
    /// (usually strings).
    #[serde(default, skip_serializing_if = "Map::is_empty")]
    pub errors: Map<String, Value>,
    /// Driver-defined result payload (what changed, new ids, …) —
    /// shape is the driver's, hosts pass it through verbatim.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub data: Option<Value>,
    /// An updated [`ViewResult`] to re-render in place of the
    /// submitted view (e.g. the form now shows the saved values).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub view: Option<Box<ViewResult>>,
    /// Forward-compat catch-all — unknown result fields a future
    /// driver emits survive a typed round-trip.
    #[serde(flatten)]
    pub extra: Map<String, Value>,
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
        "manifest" | "views" | "view" | "query" | "render" | "submit" | "jobs"
        | "notifications" => Ok(serde_json::from_value(value)?),
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

#[cfg(test)]
#[cfg_attr(coverage, coverage(off))]
mod tests {
    #![allow(clippy::unwrap_used)]

    use super::*;
    use serde_json::json;

    /// `submit` parses from its wire shape and round-trips; absent
    /// `form`/`values` default cleanly (old hosts may omit them).
    #[test]
    fn submit_request_roundtrip() {
        let req = parse_request(
            r#"{"op":"submit","view":"settings","form":"prefs","values":{"theme":"dark","zoom":2}}"#,
        )
        .unwrap();
        match req {
            Request::Submit { view, form, values } => {
                assert_eq!(view, "settings");
                assert_eq!(form.as_deref(), Some("prefs"));
                assert_eq!(values["theme"], json!("dark"));
                assert_eq!(values["zoom"], json!(2));
            }
            other => panic!("expected Submit, got {other:?}"),
        }
        // Minimal form: only `view` required.
        let req = parse_request(r#"{"op":"submit","view":"v"}"#).unwrap();
        match req {
            Request::Submit { view, form, values } => {
                assert_eq!(view, "v");
                assert!(form.is_none());
                assert!(values.is_empty());
            }
            other => panic!("expected Submit, got {other:?}"),
        }
        // Serialized form keeps the same shape.
        let line = serde_json::to_string(&Request::Submit {
            view: "v".into(),
            form: None,
            values: Map::new(),
        })
        .unwrap();
        assert_eq!(line, r#"{"op":"submit","view":"v"}"#);
    }

    /// `SubmitResult` parses sparse driver answers, carries
    /// per-field errors, and preserves unknown fields via `extra`.
    #[test]
    fn submit_result_parses_and_preserves_extras() {
        let r: SubmitResult =
            serde_json::from_value(json!({"message":"saved","future_field":{"x":1}})).unwrap();
        assert_eq!(r.message.as_deref(), Some("saved"));
        assert!(r.errors.is_empty());
        assert_eq!(r.extra["future_field"], json!({"x": 1}));

        let r: SubmitResult = serde_json::from_value(json!({
            "errors": {"theme": "unknown value"},
            "data": {"applied": 2},
            "view": {"view": {"id":"v","title":"V","route":"/v","root":{"kind":"kv","pairs":[]}},"data":{}}
        }))
        .unwrap();
        assert_eq!(r.errors["theme"], json!("unknown value"));
        assert!(r.view.is_some());
    }

    /// `jobs`/`notifications` joined the v1 op set.
    #[test]
    fn platform_feed_ops_parse() {
        assert!(matches!(
            parse_request(r#"{"op":"jobs"}"#).unwrap(),
            Request::Jobs
        ));
        assert!(matches!(
            parse_request(r#"{"op":"notifications"}"#).unwrap(),
            Request::Notifications
        ));
        assert_eq!(
            serde_json::to_string(&Request::Jobs).unwrap(),
            r#"{"op":"jobs"}"#
        );
        assert_eq!(
            serde_json::to_string(&Request::Notifications).unwrap(),
            r#"{"op":"notifications"}"#
        );
    }

    /// Unknown ops still fail; `submit` joined the v1 op set.
    #[test]
    fn parse_request_rejects_unknown_ops() {
        assert!(matches!(
            parse_request(r#"{"op":"explode"}"#),
            Err(WireError::UnknownOp { .. })
        ));
        assert!(parse_request(r#"{"op":"submit"}"#).is_err()); // missing view
    }
}
