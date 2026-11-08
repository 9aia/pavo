//! Manifest + view specs — the pavo-facing half of the driver.
//!
//! One namespace, `config`: deep links `config:show` and
//! `config:sources`, queries `get`/`set`/`resolve`/`config`/`sources`.
//! Other drivers and products call the `query` op over the serve
//! wire — this is the "common config API" seed: namespaced dotted
//! keys (`myco.theme`), never raw file paths.

use pavo_protocol::{Contributes, Contribution, DriverManifest, QueryBinding, ViewNode, ViewSpec};
use serde_json::{json, Map, Value};

/// Parse a static ViewNode JSON literal — the trees below are
/// compile-time constants, so a parse failure is a programming bug,
/// not a runtime condition.
fn node(v: Value) -> ViewNode {
    serde_json::from_value(v).expect("static view tree is a valid ViewNode")
}

fn binding(name: &str, query: &str) -> QueryBinding {
    let mut params = Map::new();
    params.insert("query".to_string(), Value::from(query));
    QueryBinding {
        name: name.to_string(),
        op: "query".to_string(),
        params,
    }
}

/// `config:show` — resolved config KV + provenance table + notes.
fn show_view() -> ViewSpec {
    ViewSpec {
        id: "show".to_string(),
        title: "Config".to_string(),
        route: "/config".to_string(),
        description: Some(
            "Resolved pavo config (KV, dotted keys) plus every file/env \
             layer that contributed — the common config API surface."
                .to_string(),
        ),
        queries: vec![binding("config", "config"), binding("sources", "sources")],
        root: node(json!({
            "kind": "column",
            "title": "Config",
            "children": [
                {"kind": "kv", "title": "Resolved", "query": "config"},
                {"kind": "table", "title": "Sources", "query": "sources",
                 "columns": [
                    {"key": "source", "title": "Source"},
                    {"key": "kind", "title": "Kind", "width": 8},
                    {"key": "keys", "title": "Keys", "width": 6},
                    {"key": "precedence", "title": "Prec", "width": 5}
                 ]},
                {"kind": "markdown", "text":
                    "Layers low→high: built-in defaults < `~/.config/pavo/` \
                     (global) < `pavo.toml`/`pavo.json` in cwd ancestors \
                     (nearest wins) < `--config`/`PAVO_CONFIG` < `PAVO_CONFIG_*` \
                     env. TOML + JSON; YAML unsupported. `set` writes the \
                     project-level file."}
            ]
        })),
    }
}

/// `config:edit` — the interactive proof: a `form` node whose
/// `submit_op` is `"set"`. Submitting sends `{"op":"submit","view":
/// "edit","values":{"key":…,"value":…}}`; the driver writes the
/// key through the same `set` query path.
fn edit_view() -> ViewSpec {
    ViewSpec {
        id: "edit".to_string(),
        title: "Edit config".to_string(),
        route: "/config/edit".to_string(),
        description: Some(
            "Set a namespaced config key — the suite's first \
             interactive form (submit wire op)."
                .to_string(),
        ),
        queries: vec![],
        root: node(json!({
            "kind": "column",
            "title": "Edit config",
            "children": [
                {"kind": "form", "id": "set-config", "title": "Set key",
                 "submit_op": "set", "submit_label": "Save",
                 "fields": [
                    {"key": "key", "label": "Key", "kind": "text",
                     "required": true, "placeholder": "myco.theme",
                     "hint": "dotted namespaced path, e.g. myco.theme or kuda.index"},
                    {"key": "value", "label": "Value", "kind": "text",
                     "required": true, "placeholder": "dark",
                     "hint": "JSON scalar or string — 42, true, \"dark\""}
                 ]},
                {"kind": "markdown", "text":
                    "Writes the project-level `pavo.toml`/`pavo.json` \
                     (nearest ancestor of the driver's cwd). Values \
                     parse as JSON when possible, else strings."}
            ]
        })),
    }
}

/// `config:sources` — just the provenance table.
fn sources_view() -> ViewSpec {
    ViewSpec {
        id: "sources".to_string(),
        title: "Config sources".to_string(),
        route: "/config/sources".to_string(),
        description: Some(
            "Every config layer in resolution order with the file it \
             came from and how many keys it provided."
                .to_string(),
        ),
        queries: vec![binding("sources", "sources")],
        root: node(json!({
            "kind": "column",
            "title": "Config sources",
            "children": [
                {"kind": "table", "query": "sources", "columns": [
                    {"key": "source", "title": "Source"},
                    {"key": "path", "title": "Path"},
                    {"key": "kind", "title": "Kind", "width": 8},
                    {"key": "keys", "title": "Keys", "width": 6},
                    {"key": "precedence", "title": "Prec", "width": 5},
                    {"key": "error", "title": "Error"}
                 ]},
                {"kind": "markdown", "text":
                    "Provenance: `defaults` < `global` < `project` (root→cwd) \
                     < `--config`/`PAVO_CONFIG` < `env`. `error` rows carry \
                     the parse failure; `missing` means an explicit path \
                     doesn't exist yet (`set` will create it)."}
            ]
        })),
    }
}

/// The driver manifest — `manifest` verb payload.
#[must_use]
pub fn manifest() -> DriverManifest {
    DriverManifest {
        name: "config".to_string(),
        title: "Config".to_string(),
        icon: Some("⚙".to_string()),
        version: env!("CARGO_PKG_VERSION").to_string(),
        verbs: vec![
            "manifest".to_string(),
            "capabilities".to_string(),
            "serve".to_string(),
        ],
        capabilities: vec!["pavo".to_string(), "config".to_string()],
        contributes: Contributes {
            statusbar: vec![Contribution {
                id: "status".to_string(),
                title: "config".to_string(),
                route: "/config".to_string(),
                priority: 90,
                icon: None,
            }],
            ..Contributes::default()
        },
        views: vec![show_view(), sources_view(), edit_view()],
        renderers: Vec::new(),
    }
}

/// `views` op payload — summaries of [`manifest`]'s views.
#[must_use]
pub fn view_summaries() -> Vec<pavo_protocol::wire::ViewSummary> {
    manifest()
        .views
        .iter()
        .map(pavo_protocol::wire::ViewSummary::from)
        .collect()
}
