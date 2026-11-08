//! Driver manifest — what a `tamias-driver-<type>` executable answers
//! to the `manifest` verb. Discoverable without spawning the driver
//! for views: pavo can list routes and contributions straight from
//! this document.

use std::collections::BTreeMap;

use serde::{Deserialize, Serialize};
use serde_json::{Map, Value};

use crate::view_node::ViewNode;

/// The driver manifest — the discoverability contract.
///
/// Wire shape (JSON):
///
/// ```json
/// {
///   "name": "myco",
///   "title": "Myco",
///   "version": "0.4.0",
///   "verbs": ["manifest", "serve"],
///   "capabilities": ["pavo"],
///   "contributes": { "navbar": [...], "routes": [...] },
///   "views": [ { "id": "board", "route": "/myco/board", "root": {…} } ]
/// }
/// ```
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct DriverManifest {
    /// Driver name — the `<type>` in `tamias-driver-<type>` and the
    /// namespace for its routes (`/<name>/<view>`).
    pub name: String,
    /// Human-facing title for chrome (navbar entries, window titles).
    pub title: String,
    /// Optional icon — an emoji, an icon name, or a path the renderer
    /// knows how to resolve.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub icon: Option<String>,
    /// Driver version (semver string).
    pub version: String,
    /// Wire verbs this executable answers beyond the tamias install
    /// verbs — e.g. `["manifest", "serve"]` for pavo drivers.
    /// Capability-negotiated: pavo treats a driver as a view driver
    /// iff `serve` is listed (or `capabilities` contains `"pavo"`).
    #[serde(default)]
    pub verbs: Vec<String>,
    /// Capability tags — free-form strings products use to declare
    /// roles (`"pavo"`, `"config"`, `"chrome"`).
    #[serde(default)]
    pub capabilities: Vec<String>,
    /// Contribution slots — where this driver's entries land in the
    /// composed chrome. See [`Contributes`].
    #[serde(default)]
    pub contributes: Contributes,
    /// Views this driver can render — each binds a route to a
    /// declarative [`ViewNode`] tree plus the queries that feed it.
    #[serde(default)]
    pub views: Vec<ViewSpec>,
}

/// Contribution slots — the named places in pavo's chrome where
/// drivers may register entries. A *chrome driver* owns the frame;
/// view drivers fill slots. pavo merges contributions across drivers,
/// resolving conflicts by driver priority.
///
/// The four built-in slots are `navbar`, `dashboard`, `statusbar`,
/// and `routes`. Unknown slots are preserved via [`Contributes::extra`]
/// so newer drivers degrade gracefully on older hosts.
#[derive(Clone, Debug, Default, PartialEq, Serialize, Deserialize)]
pub struct Contributes {
    /// Entries for the top navigation bar (web/TUI chrome).
    #[serde(default)]
    pub navbar: Vec<Contribution>,
    /// Entries for the shared `dashboard` route — composable panels
    /// from many drivers on one screen.
    #[serde(default)]
    pub dashboard: Vec<Contribution>,
    /// Entries for the bottom status bar — typically one-line
    /// summaries (staleness, counts, claims).
    #[serde(default)]
    pub statusbar: Vec<Contribution>,
    /// Standalone routes the driver owns outside its views — e.g. a
    /// settings page. Each becomes a navigable URL on the web host.
    #[serde(default)]
    pub routes: Vec<Contribution>,
    /// Forward-compat escape hatch: any slot name not listed above
    /// round-trips untouched. Renderers ignore slots they don't know.
    #[serde(flatten)]
    pub extra: BTreeMap<String, Vec<Contribution>>,
}

/// One contributed entry in a slot.
///
/// `route` is a host-relative path (`/myco/board`, `/dashboard`);
/// renderers resolve it against the driver's views or its own
/// routes. `priority` orders entries within a slot — lower wins;
/// ties break by driver discovery order (stable, deterministic).
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct Contribution {
    /// Stable id — namespaced as `<driver>:<id>` by convention so
    /// contributions never collide across drivers.
    pub id: String,
    /// Label shown in chrome.
    pub title: String,
    /// Route the entry navigates to.
    pub route: String,
    /// Ordering hint within the slot; lower sorts first. Defaults to
    /// `100` when absent so drivers can leave it unset.
    #[serde(default = "default_priority")]
    pub priority: i32,
    /// Optional icon hint (same resolution rules as
    /// [`DriverManifest::icon`]).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub icon: Option<String>,
}

fn default_priority() -> i32 {
    100
}

/// One view a driver offers — a route plus the declarative tree and
/// the queries that feed it.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct ViewSpec {
    /// View id, unique within the driver (`board`, `item`).
    pub id: String,
    /// Human-facing title.
    pub title: String,
    /// Route the view answers — `<driver>:<view>` deep links resolve
    /// here (e.g. `myco:board` → `/myco/board`).
    pub route: String,
    /// Optional longer description for picker UIs.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub description: Option<String>,
    /// Query bindings — named data sources the [`ViewNode`] tree
    /// references via `NodeBase::query`. Each `name` maps to a wire
    /// `op` (usually `query` on the driver's `serve`) plus static
    /// params merged with caller-supplied view params.
    #[serde(default)]
    pub queries: Vec<QueryBinding>,
    /// The declarative UI tree — the renderers' input.
    pub root: ViewNode,
}

/// A named query binding inside a [`ViewSpec`]: `name` is what
/// ViewNodes reference; `op` is the wire op the host calls; `params`
/// are static arguments merged under caller-supplied view params.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct QueryBinding {
    /// Binding name — referenced by `query` fields in the tree.
    pub name: String,
    /// Wire op to invoke on the driver's `serve` (typically `"query"`,
    /// with the real product op carried in `params`).
    pub op: String,
    /// Static params merged into every invocation of this binding.
    #[serde(default, skip_serializing_if = "Map::is_empty")]
    pub params: Map<String, Value>,
}
