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
//! 5. `{"op":"invoke","action":<id>"}` for every declared action —
//!    manifest-global, node-scoped (`NodeBase.actions[]`), and
//!    tree-item-scoped (`TreeItem.actions[]`, inline or query-bound)
//!    — with the view/target context hosts would send → parses as
//!    [`ActionResult`]. `route`/`driver`-scoped actions are skipped
//!    (they don't dispatch over this driver's `invoke`). An
//!    undeclared action id must answer `ok:false`.
//! 6. Malformed requests — an unknown op and a non-JSON line — must
//!    both get an `ok:false` [`Response`].
//! 7. The process must still answer after all of the above (session
//!    liveness across multiple requests).
//!
//! Transport failures (timeout, EOF, unparseable response lines)
//! become violations on the check that hit them; the suite keeps
//! going so one broken op doesn't hide the rest.

use std::collections::BTreeSet;
use std::path::Path;
use std::time::Duration;

use pavo_protocol::wire::{ActionResult, QueryResult, Response, ViewResult, ViewSummary};
use pavo_protocol::{Action, DriverManifest, TreeItem, ViewNode};
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
    /// `invoke` op for a declared action: transport failure,
    /// `ok:false`, or a result that doesn't parse as [`ActionResult`].
    Invoke,
    /// `invoke` of an undeclared action id didn't get `ok:false`.
    InvokeUnknown,
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
            Self::Invoke => "invoke",
            Self::InvokeUnknown => "invoke_unknown",
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

    // Action probes collected while serving views — node/item-scoped
    // declarations live inside ViewResults; manifest-global ones are
    // added once `view_ids` is known (they still get a view context,
    // like `Host::invoke` always sends).
    let mut probes: Vec<InvokeProbe> = Vec::new();

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
                probes.extend(view_action_probes(&result));
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

    // -- 5. invoke per declared action ----------------------------------
    // Manifest-global actions dispatch with the driver's first view as
    // context (what `Host::invoke` falls back to); no view context at
    // all when the driver declares none.
    if let Some(m) = &manifest {
        for a in &m.actions {
            if dispatchable(a) {
                probes.push(InvokeProbe {
                    view: view_ids.first().cloned(),
                    action: a.id.clone(),
                    target: None,
                });
            }
        }
    }

    // Dedupe identical wire requests — the same action id declared at
    // several scopes collapses to one probe per distinct context.
    let mut seen_requests = BTreeSet::new();
    for probe in &probes {
        let mut request = json!({"op": "invoke", "action": probe.action, "params": {}});
        if let Some(v) = &probe.view {
            request["view"] = json!(v);
        }
        if let Some(t) = &probe.target {
            request["target"] = t.clone();
        }
        if !seen_requests.insert(serde_json::to_string(&request).unwrap_or_default()) {
            continue;
        }
        if let Some(value) = typed_call(&mut session, request, timeout, Check::Invoke, &mut out) {
            if let Err(e) = serde_json::from_value::<ActionResult>(value) {
                out.push(violation(
                    Check::Invoke,
                    format!(
                        "invoke '{}' result isn't an ActionResult: {e}",
                        probe.action
                    ),
                ));
            }
        }
    }

    // An undeclared action id must be rejected — `ok:false` is the
    // contract for undispatchable actions (unknown op, unknown
    // action). Runs even for action-less drivers: `ok:false` is the
    // right answer there too.
    let bogus = json!({"op": "invoke", "action": "pavo.conformance.no-such-action"});
    match session.call(&bogus, timeout) {
        Ok(resp) if !resp.ok => {}
        Ok(resp) => out.push(violation(
            Check::InvokeUnknown,
            format!(
                "undeclared action got ok:true: {}",
                serde_json::to_string(&resp.result.unwrap_or(Value::Null)).unwrap_or_default()
            ),
        )),
        Err(e) => out.push(violation(Check::InvokeUnknown, wire_detail(&bogus, &e))),
    }

    // -- 6. malformed requests must get ok:false ------------------------
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

    // -- 7. session still alive -----------------------------------------
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

/// One `invoke` probe — the wire fields a host would send for a
/// declared action.
struct InvokeProbe {
    /// View-id context (`None` only for manifest-global actions on
    /// view-less drivers).
    view: Option<String>,
    /// The action id.
    action: String,
    /// Opaque target — `{"id": …}` for item-scoped actions.
    target: Option<Value>,
}

/// Whether an [`Action`] dispatches over *this* driver's `invoke`.
/// `route` actions navigate host-locally (no wire round-trip, per
/// protocol); `driver`-scoped actions name another driver's serve —
/// v1 hosts may treat them as local, but the declaring driver isn't
/// obligated to resolve the id, so probing would risk false
/// violations.
fn dispatchable(a: &Action) -> bool {
    a.route.is_none() && a.driver.is_none()
}

/// Node- and tree-item-scoped action probes declared inside one
/// served [`ViewResult`] — node actions first (pre-order), then
/// tree-item actions (inline items, else query-bound `data` items —
/// same resolution rule renderers/hosts use). Item probes are
/// deduped to one per action id per view (first item wins) so a
/// 1000-row tree doesn't turn into 1000 invokes.
fn view_action_probes(result: &ViewResult) -> Vec<InvokeProbe> {
    let view = result.view.id.clone();
    let mut probes = Vec::new();
    let mut seen_node = BTreeSet::new();
    let mut seen_item = BTreeSet::new();
    walk_view_node(&result.view.root, &mut |node| {
        for a in &node.base().actions {
            if dispatchable(a) && seen_node.insert(a.id.clone()) {
                probes.push(InvokeProbe {
                    view: Some(view.clone()),
                    action: a.id.clone(),
                    target: None,
                });
            }
        }
        if let ViewNode::Tree(tree) = node {
            if !tree.items.is_empty() {
                collect_item_probes(&tree.items, &view, &mut seen_item, &mut probes);
            } else if let Some(items) = tree
                .base
                .query
                .as_ref()
                .and_then(|q| result.data.get(q))
                .and_then(payload_tree_items)
            {
                collect_item_probes(&items, &view, &mut seen_item, &mut probes);
            }
        }
    });
    probes
}

/// `TreeItem[]` or `{"items": [...]}` — the two payload shapes tree
/// query bindings carry (mirrors `pavo_core::actions`).
fn payload_tree_items(payload: &Value) -> Option<Vec<TreeItem>> {
    let value = if payload.is_array() {
        payload.clone()
    } else {
        payload.get("items")?.clone()
    };
    serde_json::from_value(value).ok()
}

/// Item-scoped probes for one item list, recursing into `children`.
fn collect_item_probes(
    items: &[TreeItem],
    view: &str,
    seen: &mut BTreeSet<String>,
    out: &mut Vec<InvokeProbe>,
) {
    for item in items {
        for a in &item.actions {
            if dispatchable(a) && seen.insert(a.id.clone()) {
                out.push(InvokeProbe {
                    view: Some(view.to_string()),
                    action: a.id.clone(),
                    // The canonical item target hosts send —
                    // `{"id": <item-id>}` (see
                    // `pavo_core::commands` dispatch).
                    target: item.id.as_ref().map(|id| json!({"id": id})),
                });
            }
        }
        collect_item_probes(&item.children, view, seen, out);
    }
}

/// A node's children — `panel`/`row`/`column` nest `Vec<ViewNode>`;
/// every other kind is a leaf (mirrors `pavo_core::viewnode`).
fn view_node_children(node: &ViewNode) -> &[ViewNode] {
    match node {
        ViewNode::Panel(n) => &n.children,
        ViewNode::Row(n) => &n.children,
        ViewNode::Column(n) => &n.children,
        _ => &[],
    }
}

/// Depth-first pre-order walk over a [`ViewNode`] tree.
fn walk_view_node<'a>(node: &'a ViewNode, f: &mut impl FnMut(&'a ViewNode)) {
    f(node);
    for child in view_node_children(node) {
        walk_view_node(child, f);
    }
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
        .get("query")
        .or_else(|| request.get("action"))
        .or_else(|| request.get("view"))
        .and_then(Value::as_str)
        .map(|s| format!(" '{s}'"))
        .unwrap_or_default();
    format!("op '{op}'{extra}: {e}")
}
