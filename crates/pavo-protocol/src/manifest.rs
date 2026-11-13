//! Driver manifest — what a `tamias-driver-<type>` executable answers
//! to the `manifest` verb. Discoverable without spawning the driver
//! for views: pavo can list routes and contributions straight from
//! this document.

use std::collections::BTreeMap;

use serde::{Deserialize, Serialize};
use serde_json::{Map, Value};

use crate::action::Action;
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
    /// Driver dependency declarations — which other drivers this one
    /// expects (or optionally uses). See [`DriverDeps`].
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub deps: Option<DriverDeps>,
    /// Contribution slots — where this driver's entries land in the
    /// composed chrome. See [`Contributes`].
    #[serde(default)]
    pub contributes: Contributes,
    /// Views this driver can render — each binds a route to a
    /// declarative [`ViewNode`] tree plus the queries that feed it.
    #[serde(default)]
    pub views: Vec<ViewSpec>,
    /// View-kind renderer plugins this driver ships — each declares
    /// the node `kinds` it renders and which `surfaces` it supports.
    /// See [`RendererDecl`]. A driver may declare renderers with zero
    /// views (pure renderer plugin) or alongside views.
    #[serde(default)]
    pub renderers: Vec<RendererDecl>,
    /// Driver-global actions — verbs offered on every view (new
    /// item, refresh, settings, …). View/node/item-scoped actions
    /// live on [`crate::view_node::NodeBase::actions`] and
    /// [`crate::view_node::TreeItem::actions`]. All dispatch through
    /// the wire `invoke` op. See [`crate::action`].
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub actions: Vec<Action>,
}

/// Driver dependency declarations — the ordering half of the plugin
/// contract. Names are driver names: the `<type>` in
/// `tamias-driver-<type>` (and `DriverManifest::name`).
///
/// Semantics mirror product `requiredDeps`/`optionalDeps`:
///
/// - `requires` — drivers that must be present and healthy for this
///   driver to function. A missing required dep is a **discovery
///   error**: the host refuses (or quarantines) the driver rather
///   than half-rendering it.
/// - `optional` — drivers whose absence merely degrades features.
///   Missing optional deps never block discovery; the driver/host
///   falls back gracefully.
///
/// Ordering is a host concern — pavo-core topo-sorts discovered
/// drivers over this graph (`order_drivers`); the manifest only
/// carries the declaration.
///
/// Wire shape:
///
/// ```json
/// { "requires": ["config"], "optional": ["myco"] }
/// ```
#[derive(Clone, Debug, Default, PartialEq, Serialize, Deserialize)]
pub struct DriverDeps {
    /// Driver names this driver cannot function without — absence is
    /// a discovery error.
    #[serde(default)]
    pub requires: Vec<String>,
    /// Driver names this driver uses when present — absence degrades
    /// gracefully.
    #[serde(default)]
    pub optional: Vec<String>,
}

/// One view-kind renderer a driver ships — the manifest half of the
/// renderer-plugin contract (the wire half is the `render` op).
///
/// A renderer claims node `kinds` (ViewNode `kind` strings, or `"*"`
/// for any kind the host doesn't build in). When a surface meets a
/// node of a claimed kind, the host routes it to this driver's
/// `render` op instead of its built-in interpretation; the driver
/// answers with a [`crate::render_model::RenderModel`].
///
/// Wire shape:
///
/// ```json
/// { "id": "graph", "kinds": ["graph"],
///   "surfaces": { "cli": {"formats": ["svg", "ascii"]},
///                 "tui": true,
///                 "web": {"island": true} },
///   "priority": 100 }
/// ```
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct RendererDecl {
    /// Renderer id — namespaced `<driver>:<id>` by convention; stable
    /// for diagnostics and `/api/renderers` listings.
    pub id: String,
    /// Node kinds this renderer handles — ViewNode `kind` strings
    /// (`"graph"`, `"timeline"`, …) or `"*"` for a catch-all. Explicit
    /// kinds always beat `"*"` claims; among competing claims the
    /// lowest [`RendererDecl::priority`] wins (ties: driver kind asc,
    /// then renderer id asc — the chrome-merge ordering policy).
    #[serde(default)]
    pub kinds: Vec<String>,
    /// Per-surface support. Absent surface = unsupported there.
    #[serde(default)]
    pub surfaces: RendererSurfaces,
    /// Ordering hint within a kind — lower wins. Defaults to `100`
    /// (same convention as [`Contribution::priority`]).
    #[serde(default = "default_priority")]
    pub priority: i32,
    /// Optional entry-point hint — a component/module name the host
    /// may prefer over `serve`-wire rendering (reserved; hosts
    /// currently always use the wire).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub entry: Option<String>,
}

/// Which surfaces a [`RendererDecl`] supports and how.
#[derive(Clone, Debug, Default, PartialEq, Serialize, Deserialize)]
pub struct RendererSurfaces {
    /// CLI support — `formats` lists the `pavo view --format` values
    /// the renderer claims beyond the built-ins (`text`/`json`/`dot`/
    /// `mermaid`). An empty list is a wildcard: the renderer handles
    /// every format it is asked for, including `text`.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub cli: Option<CliSurface>,
    /// TUI support — `true` renders plugin nodes on the ratatui host.
    #[serde(default)]
    pub tui: bool,
    /// Web support — `component` names a web component/island the
    /// host may mount; `island` marks the renderer as needing
    /// client-side execution (SSR-incompatible output).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub web: Option<WebSurface>,
}

/// CLI surface declaration for a [`RendererDecl`].
#[derive(Clone, Debug, Default, PartialEq, Serialize, Deserialize)]
pub struct CliSurface {
    /// `--format` values this renderer claims. Empty = wildcard.
    #[serde(default)]
    pub formats: Vec<String>,
}

/// Web surface declaration for a [`RendererDecl`].
#[derive(Clone, Debug, Default, PartialEq, Serialize, Deserialize)]
pub struct WebSurface {
    /// Optional component/island name — hint for hosts that resolve
    /// `placeholder` models or `embed` nodes; the `render` op remains
    /// the canonical path.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub component: Option<String>,
    /// `true` when the renderer's output requires client-side
    /// execution (canvas, wasm, interactive svg) — hosts should treat
    /// the node as an island.
    #[serde(default)]
    pub island: bool,
}

/// Contribution slots — the named places in pavo's chrome where
/// drivers may register entries. A *chrome driver* owns the frame;
/// view drivers fill slots. pavo merges contributions across drivers,
/// resolving conflicts by driver priority.
///
/// The built-in slots are `navbar`, `dashboard`, `statusbar`,
/// `routes`, `sidebar`, `headerActions`, `notifications`, and
/// `jobs`. Unknown slots are preserved via [`Contributes::extra`]
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
    /// Entries for a left sidebar shell — grouped nav entries hosts
    /// may cluster by `group`-like semantics later. Same
    /// [`Contribution`] shape as the other slots: `route` navigates,
    /// `priority` orders, `icon` decorates.
    #[serde(default)]
    pub sidebar: Vec<Contribution>,
    /// Top-bar actions — buttons/icons in the header that open a
    /// popover or panel. `route` points at the view the host renders
    /// inside that surface.
    #[serde(default, rename = "headerActions")]
    pub header_actions: Vec<Contribution>,
    /// Marks the driver as a notification provider. Entries are
    /// display metadata (label, icon, ordering); the notification
    /// *data* flows through the `notifications` wire op as
    /// [`crate::platform::NotificationItem`]s.
    #[serde(default)]
    pub notifications: Vec<Contribution>,
    /// Marks the driver as a job/queue feed provider. Same pattern
    /// as `notifications`: entries are display metadata, data comes
    /// from the `jobs` wire op as [`crate::platform::JobItem`]s.
    #[serde(default)]
    pub jobs: Vec<Contribution>,
    /// Command-palette/menu entries — the dmenu-style quick-launch
    /// list hosts synthesize from `actions[]` + these declarations.
    /// Entries either `invoke` an action or `route` to a view.
    /// See [`MenuEntry`].
    #[serde(default)]
    pub menus: Vec<MenuEntry>,
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

/// One entry in a [`Contributes::menus`] slot — a command-palette /
/// quick-launch item.
///
/// An entry resolves to a **command** — `command` when set (the
/// unified grammar: `action:<id>` | `route:<path>` | `host.<builtin>`
/// | `driver:<name>:<id>`), else the legacy `action`/`route` pair —
/// or is display-only (a section label). `keywords` feed host-side
/// fuzzy matching (dmenu-style): users find entries by typing any
/// keyword, not just the title.
///
/// Wire shape:
///
/// ```json
/// { "id": "new-task", "title": "New task", "icon": "plus",
///   "action": "todo.add", "keywords": ["create", "todo"],
///   "section": "myco", "priority": 10 }
/// ```
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct MenuEntry {
    /// Stable id — namespaced `<driver>:<id>` by convention.
    pub id: String,
    /// Label shown in the palette/menu.
    pub title: String,
    /// Optional icon hint (same resolution rules as
    /// [`DriverManifest::icon`]).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub icon: Option<String>,
    /// Action to invoke — resolved against the driver's
    /// `actions[]`/node actions/ops-derived actions like the
    /// `invoke` wire op. May be `"<action-id>"` or
    /// `"<driver>:<action-id>"` for cross-driver entries.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub action: Option<String>,
    /// Route to navigate to instead of invoking — same semantics
    /// as [`Contribution::route`]. May carry `{…}` template
    /// placeholders (see [`crate::action::Action::route`]).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub route: Option<String>,
    /// Generic command target — the unified command grammar
    /// (`action:<id>` | `route:<path>` | `host.<builtin>` |
    /// `driver:<name>:<id>`; see `pavo_core::commands`). When
    /// present it **wins** over `action`/`route`; the legacy fields
    /// stay for compat. Entries whose command doesn't parse are
    /// display-only.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub command: Option<String>,
    /// Suggested keyboard bindings for this entry — same role as
    /// [`crate::action::Action::keys`]: declarations suggest, the
    /// host's user keymap overrides by command id.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub keys: Vec<String>,
    /// Extra fuzzy-match terms — aliases, synonyms, related ids.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub keywords: Vec<String>,
    /// Optional grouping label hosts may cluster entries under.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub section: Option<String>,
    /// Ordering hint within the merged menu; lower sorts first.
    #[serde(default = "default_priority")]
    pub priority: i32,
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
    /// Per-view row-navigation template — when set, activating a
    /// [`crate::view_node::TreeItem`] (Enter/click) navigates the
    /// host to this interpolated route instead of doing nothing.
    /// Template placeholders resolve over the item:
    /// `{item.id}`, `{item.label}`, `{item.<prop>}` (item `props`
    /// keys are addressable directly — see
    /// `pavo_core::commands::interpolate`). Example:
    /// `"/myco/item?id={item.id}"`.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub item_route: Option<String>,
    /// Names this view's free-text filter query param — the
    /// convention is `"q"`. The host's `host.filter` builtin opens a
    /// filter dialog that re-resolves the view with
    /// `params[<filter_param>]` set; no new wire op. When absent,
    /// `host.filter` falls back to `"q"` if the view's query
    /// bindings accept it, else the builtin is unavailable.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub filter_param: Option<String>,
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
    /// Optional target driver — when set, the binding is a
    /// **cross-driver** query: the host resolves `driver` (a driver
    /// name, the `<type>` in `tamias-driver-<type>`) and issues the
    /// op against *that* driver's `serve` instead of the view's own.
    /// This is the data half of the cross-driver query contract;
    /// pavo-core resolves it when assembling view data (failures
    /// degrade to an `{"error": …}` payload in the binding's slot).
    /// Older hosts may ignore the field and treat the binding as local.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub driver: Option<String>,
    /// Static params merged into every invocation of this binding.
    #[serde(default, skip_serializing_if = "Map::is_empty")]
    pub params: Map<String, Value>,
}

#[cfg(test)]
#[cfg_attr(coverage, coverage(off))]
mod tests {
    #![allow(clippy::unwrap_used)]
    use super::*;
    use serde_json::json;

    /// `MenuEntry.command`/`keys` and `ViewSpec.item_route`/
    /// `filter_param` — additive optional fields: absent defaults,
    /// present round-trips, unknown siblings ignored.
    #[test]
    fn menu_entry_and_view_spec_new_fields() {
        let m: MenuEntry = serde_json::from_value(json!({
            "id":"go","title":"Go","command":"route:/myco/item?id={item.id}",
            "keys":["g i"],"future":true
        }))
        .unwrap();
        assert_eq!(m.command.as_deref(), Some("route:/myco/item?id={item.id}"));
        assert_eq!(m.keys, vec!["g i".to_string()]);
        assert!(m.action.is_none() && m.route.is_none());
        let v = serde_json::to_value(&m).unwrap();
        assert_eq!(v["command"], "route:/myco/item?id={item.id}");
        assert_eq!(v["keys"], json!(["g i"]));
        assert!(v.get("future").is_none());

        // Legacy-only entry still parses clean.
        let legacy: MenuEntry = serde_json::from_value(json!({
            "id":"x","title":"X","action":"todo.add","route":"/v"
        }))
        .unwrap();
        assert!(legacy.command.is_none());
        assert!(legacy.keys.is_empty());

        let spec: ViewSpec = serde_json::from_value(json!({
            "id":"board","title":"Board","route":"/myco/board",
            "item_route":"/myco/item?id={item.id}",
            "filter_param":"q",
            "root":{"kind":"kv","pairs":[]}
        }))
        .unwrap();
        assert_eq!(spec.item_route.as_deref(), Some("/myco/item?id={item.id}"));
        assert_eq!(spec.filter_param.as_deref(), Some("q"));
        let v = serde_json::to_value(&spec).unwrap();
        assert_eq!(v["item_route"], "/myco/item?id={item.id}");
        assert_eq!(v["filter_param"], "q");

        let bare: ViewSpec = serde_json::from_value(json!({
            "id":"v","title":"V","route":"/v","root":{"kind":"kv"}
        }))
        .unwrap();
        assert!(bare.item_route.is_none());
        assert!(bare.filter_param.is_none());
        assert!(serde_json::to_value(&bare)
            .unwrap()
            .get("item_route")
            .is_none());
    }
}
