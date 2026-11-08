//! Driver conformance — [`check_driver`] runs the pavo wire contract
//! suite against a `tamias-driver-*` executable and returns structured
//! [`Violation`]s rather than panicking.
//!
//! The suite (all over `<driver> serve`, one persistent session):
//!
//! 1. `{"op":"manifest"}` → parses as [`DriverManifest`] and
//!    advertises pavo (`"serve"` in `verbs` or `"pavo"` in
//!    `capabilities`).
//! 2. `{"op":"views"}` → parses as `Vec<ViewSummary>` whose id set
//!    matches the manifest's declared views.
//! 3. `{"op":"view","view":<id>"}` for every declared view → parses
//!    as [`ViewResult`], echoes the requested view id, and its `data`
//!    map carries every declared query binding.
//! 4. `{"op":"query","query":<name>"}` for every declared binding →
//!    parses as [`QueryResult`].
//! 5. Malformed requests — an unknown op and a non-JSON line — must
//!    both get an `ok:false` [`Response`].
//! 6. The process must still answer after all of the above (session
//!    liveness across multiple requests).
//!
//! Transport failures (timeout, EOF, unparseable response lines)
//! become violations on the check that hit them; the suite keeps
//! going so one broken op doesn't hide the rest.

use std::collections::BTreeSet;
use std::path::Path;
use std::time::Duration;

use pavo_protocol::wire::{QueryResult, Response, ViewResult, ViewSummary};
use pavo_protocol::DriverManifest;
use serde::Serialize;
use serde_json::{json, Value};

use crate::wire::{Session, WireError};

/// Which contract check produced a [`Violation`].
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash, PartialOrd, Ord, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum Check {
    /// The `serve` process couldn't be spawned.
    Spawn,
    /// `manifest` op: transport failure, `ok:false`, or a result that
    /// doesn't parse as [`DriverManifest`].
    Manifest,
    /// The manifest parses but doesn't advertise pavo service.
    AdvertisesPavo,
    /// `views` op: transport failure, `ok:false`, or a result that
    /// doesn't parse as `Vec<ViewSummary>`.
    Views,
    /// `views` result ids don't match the manifest's view ids.
    ViewsConsistent,
    /// `view` op for a declared view: transport failure, `ok:false`,
    /// or a result that doesn't parse as [`ViewResult`].
    View,
    /// `view` result shape: echoed id mismatch or `data` missing a
    /// declared query binding.
    ViewShape,
    /// `query` op for a declared binding: transport failure,
    /// `ok:false`, or a result that doesn't parse as [`QueryResult`].
    Query,
    /// Malformed requests (unknown op, non-JSON) didn't get `ok:false`.
    MalformedRejected,
    /// The session died or stopped answering partway through.
    SessionAlive,
}

impl std::fmt::Display for Check {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        let s = match self {
            Self::Spawn => "spawn",
            Self::Manifest => "manifest",
            Self::AdvertisesPavo => "advertises_pavo",
            Self::Views => "views",
            Self::ViewsConsistent => "views_consistent",
            Self::View => "view",
            Self::ViewShape => "view_shape",
            Self::Query => "query",
            Self::MalformedRejected => "malformed_rejected",
            Self::SessionAlive => "session_alive",
        };
        f.write_str(s)
    }
}

/// One contract violation — which check failed and why.
#[derive(Clone, Debug, PartialEq, Eq, Serialize)]
pub struct Violation {
    /// The check that failed.
    pub check: Check,
    /// Human/machine-readable detail (driver errors, parse errors,
    /// mismatched ids, …).
    pub detail: String,
}

impl std::fmt::Display for Violation {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "[{}] {}", self.check, self.detail)
    }
}

impl std::error::Error for Violation {}

fn violation(check: Check, detail: impl Into<String>) -> Violation {
    Violation {
        check,
        detail: detail.into(),
    }
}

/// Knobs for [`check_driver_with`].
#[derive(Clone, Debug)]
pub struct CheckOptions {
    /// Per-request response timeout. Default 10s.
    pub timeout: Duration,
}

impl Default for CheckOptions {
    fn default() -> Self {
        Self {
            timeout: Duration::from_secs(10),
        }
    }
}

/// Run the conformance suite against the `tamias-driver-*` executable
/// at `driver` with default options. Returns every violation found —
/// an empty vec means the driver is conformant.
#[must_use]
pub fn check_driver(driver: &Path) -> Vec<Violation> {
    check_driver_with(driver, CheckOptions::default())
}

/// [`check_driver`] with explicit [`CheckOptions`] (short timeouts for
/// delay-injection tests, etc.).
#[must_use]
pub fn check_driver_with(driver: &Path, options: CheckOptions) -> Vec<Violation> {
    let mut out = Vec::new();
    let timeout = options.timeout;
    let mut session = match Session::spawn(driver) {
        Ok(s) => s,
        Err(e) => {
            out.push(violation(Check::Spawn, format!("{driver:?}: {e}")));
            return out;
        }
    };

    // -- 1. manifest -------------------------------------------------
    let manifest = match typed_call(
        &mut session,
        json!({"op": "manifest"}),
        timeout,
        Check::Manifest,
        &mut out,
    ) {
        Some(value) => match serde_json::from_value::<DriverManifest>(value) {
            Ok(m) => {
                let advertises = m.verbs.iter().any(|v| v == "serve")
                    || m.capabilities.iter().any(|c| c == "pavo");
                if !advertises {
                    out.push(violation(
                        Check::AdvertisesPavo,
                        format!(
                            "manifest doesn't advertise pavo service (verbs={:?}, capabilities={:?})",
                            m.verbs, m.capabilities
                        ),
                    ));
                }
                Some(m)
            }
            Err(e) => {
                out.push(violation(
                    Check::Manifest,
                    format!("manifest result isn't a DriverManifest: {e}"),
                ));
                None
            }
        },
        None => None,
    };

    // -- 2. views ------------------------------------------------------
    let summaries = typed_call(
        &mut session,
        json!({"op": "views"}),
        timeout,
        Check::Views,
        &mut out,
    )
    .and_then(
        |value| match serde_json::from_value::<Vec<ViewSummary>>(value) {
            Ok(v) => Some(v),
            Err(e) => {
                out.push(violation(
                    Check::Views,
                    format!("views result isn't Vec<ViewSummary>: {e}"),
                ));
                None
            }
        },
    );

    // views ↔ manifest consistency
    if let (Some(m), Some(s)) = (&manifest, &summaries) {
        let declared: BTreeSet<&str> = m.views.iter().map(|v| v.id.as_str()).collect();
        let served: BTreeSet<&str> = s.iter().map(|v| v.id.as_str()).collect();
        let missing: Vec<&str> = declared.difference(&served).copied().collect();
        let extra: Vec<&str> = served.difference(&declared).copied().collect();
        if !missing.is_empty() || !extra.is_empty() {
            out.push(violation(
                Check::ViewsConsistent,
                format!(
                    "views/manifest mismatch — in manifest not views: {missing:?}; in views not manifest: {extra:?}"
                ),
            ));
        }
    }

    // -- 3. view per declared view -------------------------------------
    // View ids to exercise: the manifest's when we have it, else the
    // views op's.
    let view_ids: Vec<String> = manifest
        .as_ref()
        .map(|m| m.views.iter().map(|v| v.id.clone()).collect())
        .or_else(|| {
            summaries
                .as_ref()
                .map(|s| s.iter().map(|v| v.id.clone()).collect())
        })
        .unwrap_or_default();
    let mut binding_names: BTreeSet<String> = BTreeSet::new();
    if let Some(m) = &manifest {
        for spec in &m.views {
            for b in &spec.queries {
                binding_names.insert(b.name.clone());
            }
        }
    }

    for id in &view_ids {
        let request = json!({"op": "view", "view": id});
        let Some(value) = typed_call(&mut session, request, timeout, Check::View, &mut out) else {
            continue;
        };
        match serde_json::from_value::<ViewResult>(value) {
            Ok(result) => {
                if result.view.id != *id {
                    out.push(violation(
                        Check::ViewShape,
                        format!("view '{id}' echoed id '{}'", result.view.id),
                    ));
                }
                for b in &result.view.queries {
                    if !result.data.contains_key(&b.name) {
                        out.push(violation(
                            Check::ViewShape,
                            format!("view '{id}' data missing binding '{}'", b.name),
                        ));
                    }
                }
                for name in result.view.queries.iter().map(|b| b.name.clone()) {
                    binding_names.insert(name);
                }
            }
            Err(e) => out.push(violation(
                Check::View,
                format!("view '{id}' result isn't a ViewResult: {e}"),
            )),
        }
    }

    // -- 4. query per declared binding ---------------------------------
    for name in &binding_names {
        let request = json!({"op": "query", "query": name});
        if let Some(value) = typed_call(&mut session, request, timeout, Check::Query, &mut out) {
            if let Err(e) = serde_json::from_value::<QueryResult>(value) {
                out.push(violation(
                    Check::Query,
                    format!("query '{name}' result isn't a QueryResult: {e}"),
                ));
            }
        }
    }

    // -- 5. malformed requests must get ok:false ------------------------
    for raw in ["{\"op\":\"bogus\"}", "this is not json"] {
        match session.request_raw(raw, timeout) {
            Ok(line) => match serde_json::from_str::<Response>(&line) {
                Ok(resp) if !resp.ok => {}
                Ok(resp) => out.push(violation(
                    Check::MalformedRejected,
                    format!("malformed request {raw:?} got ok:{}", resp.ok),
                )),
                Err(_) => out.push(violation(
                    Check::MalformedRejected,
                    format!("malformed request {raw:?} got non-Response line: {line}"),
                )),
            },
            Err(e) => out.push(violation(
                Check::MalformedRejected,
                format!("malformed request {raw:?}: {e}"),
            )),
        }
    }

    // -- 6. session still alive -----------------------------------------
    match session.call(&json!({"op": "views"}), timeout) {
        Ok(resp) if resp.ok => {}
        Ok(resp) => out.push(violation(
            Check::SessionAlive,
            format!(
                "final views probe got ok:false: {}",
                resp.error.unwrap_or_default()
            ),
        )),
        Err(e) => out.push(violation(Check::SessionAlive, format!("{e}"))),
    }

    out
}

/// One wire round-trip → the `result` value on `ok:true`.
///
/// Pushes a [`Violation`] under `check` and returns `None` on
/// transport failure, `ok:false`, or an unparseable response line.
fn typed_call(
    session: &mut Session,
    request: Value,
    timeout: Duration,
    check: Check,
    out: &mut Vec<Violation>,
) -> Option<Value> {
    match session.call(&request, timeout) {
        Ok(resp) if resp.ok => Some(resp.result.unwrap_or(Value::Null)),
        Ok(resp) => {
            out.push(violation(
                check,
                resp.error
                    .unwrap_or_else(|| "ok:false without error".to_string()),
            ));
            None
        }
        Err(e) => {
            out.push(violation(check, wire_detail(&request, &e)));
            None
        }
    }
}

/// Render a transport error with the request that triggered it.
fn wire_detail(request: &Value, e: &WireError) -> String {
    let op = request.get("op").and_then(Value::as_str).unwrap_or("?");
    let extra = request
        .get("view")
        .or_else(|| request.get("query"))
        .and_then(Value::as_str)
        .map(|s| format!(" '{s}'"))
        .unwrap_or_default();
    format!("op '{op}'{extra}: {e}")
}
