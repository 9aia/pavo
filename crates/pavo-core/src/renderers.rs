//! Renderer registry — collect `renderers` declarations from every
//! discovered driver's manifest into a kind → driver lookup, and find
//! the plugin-rendered nodes inside a resolved view tree.
//!
//! This is the manifest half of the view-kind renderer contract (the
//! wire half is the `render` op, see [`crate::client::DriverClient::render`]):
//! a driver declares [`pavo_protocol::RendererDecl`]s; the host routes
//! matching nodes to that driver's `serve` process and gets back a
//! [`pavo_protocol::RenderModel`].
//!
//! ## Claim resolution (deterministic)
//!
//! - **Explicit kinds beat `"*"`.** A renderer listing `"graph"`
//!   always wins over a catch-all for the same node.
//! - **Among competing claims** for one kind (or the wildcard pool),
//!   lowest [`RendererDecl::priority`] wins; ties break by driver
//!   kind ascending, then renderer id ascending — the same policy
//!   [`crate::merge`] applies to chrome contributions (driver-name
//!   order substitutes for discovery order for determinism).
//! - **Built-in kinds are not reserved.** A plugin may claim `graph`
//!   (or even `table`) and override the host's built-in rendering on
//!   the surfaces it declares — that is the migration path. Kinds no
//!   plugin claims keep their built-in interpretation, and unknown
//!   kinds still degrade to KV.
//!
//! Surface matching: `"cli"` requires `surfaces.cli`; a `format`
//! argument further narrows to `cli.formats` (empty = wildcard,
//! claims every format including `text`). `"tui"` requires
//! `surfaces.tui == true`; `"web"` requires `surfaces.web` present.

use std::collections::BTreeMap;

use pavo_protocol::{DriverManifest, RendererDecl, ViewNode};
use serde_json::Value;

/// A renderer claim resolved to its driver — what the host needs to
/// route a `render` op.
#[derive(Clone, Debug, PartialEq)]
pub struct RendererBinding {
    /// Driver kind that owns the renderer.
    pub driver: String,
    /// Driver display title (manifest `title`).
    pub driver_title: String,
    /// The renderer's namespaced id — `<driver>:<decl.id>`.
    pub id: String,
    /// The declaration (kinds/surfaces/priority/entry).
    pub decl: RendererDecl,
}

/// The merged renderer registry across all discovered drivers.
#[derive(Clone, Debug, Default)]
pub struct RendererRegistry {
    /// kind → winning binding for explicitly claimed kinds.
    explicit: BTreeMap<String, RendererBinding>,
    /// Winning `"*"` claim (at most one — same conflict policy).
    wildcard: Option<RendererBinding>,
    /// Every binding, for `/api/renderers`-style listings (sorted by
    /// namespaced id).
    all: Vec<RendererBinding>,
}

/// One plugin-rendered node found in a view tree.
#[derive(Clone, Debug)]
pub struct PluginNode {
    /// Tree path — child indices joined by `.`; the root is `""`.
    /// Stable key renderers use to match results back to nodes.
    pub path: String,
    /// The node's `kind`.
    pub kind: String,
    /// The winning binding for this node+surface.
    pub binding: RendererBinding,
    /// The node serialized to raw JSON — passed verbatim to the
    /// driver's `render` op.
    pub node: Value,
}

impl RendererRegistry {
    /// Build the registry from `(driver kind, manifest)` pairs —
    /// same input shape as [`crate::merge::merge_chrome`]. Drivers
    /// with no `renderers` contribute nothing.
    #[must_use]
    pub fn from_manifests<'a, I>(drivers: I) -> Self
    where
        I: IntoIterator<Item = (&'a str, &'a DriverManifest)>,
    {
        let mut candidates: Vec<RendererBinding> = Vec::new();
        for (kind, manifest) in drivers {
            for decl in &manifest.renderers {
                candidates.push(RendererBinding {
                    driver: kind.to_string(),
                    driver_title: manifest.title.clone(),
                    id: format!("{kind}:{}", decl.id),
                    decl: decl.clone(),
                });
            }
        }
        // Deterministic candidate order: priority asc, driver kind
        // asc, renderer id asc — first wins per kind.
        candidates.sort_by(|a, b| {
            a.decl
                .priority
                .cmp(&b.decl.priority)
                .then_with(|| a.driver.cmp(&b.driver))
                .then_with(|| a.id.cmp(&b.id))
        });
        let mut registry = Self::default();
        for binding in candidates {
            if binding.decl.kinds.iter().any(|k| k == "*") {
                if registry.wildcard.is_none() {
                    registry.wildcard = Some(binding.clone());
                }
                registry.all.push(binding);
                continue;
            }
            for kind in &binding.decl.kinds {
                registry
                    .explicit
                    .entry(kind.clone())
                    .or_insert_with(|| binding.clone());
            }
            registry.all.push(binding);
        }
        registry.all.sort_by(|a, b| a.id.cmp(&b.id));
        registry
    }

    /// The winning binding for node `kind` on `surface` — explicit
    /// claims first, then the wildcard. `format` narrows CLI claims
    /// (`cli.formats`; empty list = wildcard). Returns `None` when no
    /// renderer claims the kind for that surface.
    #[must_use]
    pub fn for_kind(
        &self,
        kind: &str,
        surface: &str,
        format: Option<&str>,
    ) -> Option<&RendererBinding> {
        let binding = self.explicit.get(kind).or(self.wildcard.as_ref())?;
        supports_surface(binding, surface, format).then_some(binding)
    }

    /// Whether ANY registered renderer claims `kind` on `surface` —
    /// cheaper than [`RendererRegistry::for_kind`] when only presence
    /// matters (e.g. "does this view need plugin renders?").
    #[must_use]
    pub fn handles(&self, kind: &str, surface: &str, format: Option<&str>) -> bool {
        self.for_kind(kind, surface, format).is_some()
    }

    /// Every binding, sorted by namespaced id — the `/api/renderers`
    /// listing shape.
    #[must_use]
    pub fn entries(&self) -> &[RendererBinding] {
        &self.all
    }

    /// Number of renderer declarations registered.
    #[must_use]
    pub fn len(&self) -> usize {
        self.all.len()
    }

    /// Whether the registry is empty.
    #[must_use]
    pub fn is_empty(&self) -> bool {
        self.all.is_empty()
    }
}

/// Does `binding` support `surface` (and `format` on cli)?
fn supports_surface(binding: &RendererBinding, surface: &str, format: Option<&str>) -> bool {
    match surface {
        "cli" => match &binding.decl.surfaces.cli {
            None => false,
            Some(cli) => match format {
                // No format asked (e.g. "does it handle cli at all")
                // → presence of the cli surface is enough.
                None => true,
                Some(f) => cli.formats.is_empty() || cli.formats.iter().any(|x| x == f),
            },
        },
        "tui" => binding.decl.surfaces.tui,
        "web" => binding.decl.surfaces.web.is_some(),
        // Unknown surfaces: only an explicit `"*"`-style surface can't
        // exist — RendererSurfaces is closed — so unsupported.
        _ => false,
    }
}

/// Find every node in `root`'s tree claimed by a renderer plugin for
/// `surface`/`format`, pre-order, with stable [`PluginNode::path`]
/// keys. Nodes whose kind has no plugin claim are skipped — callers
/// fall back to built-in rendering for them.
///
/// `data` is unused here (kept out of the signature — plugins resolve
/// bindings themselves from the `data` map they're handed); it lives
/// on [`PluginNode::node`] only via the node's own `query` field.
#[must_use]
pub fn plugin_nodes(
    root: &ViewNode,
    registry: &RendererRegistry,
    surface: &str,
    format: Option<&str>,
) -> Vec<PluginNode> {
    let mut out = Vec::new();
    walk_plugin(root, registry, surface, format, String::new(), &mut out);
    out
}

fn walk_plugin(
    node: &ViewNode,
    registry: &RendererRegistry,
    surface: &str,
    format: Option<&str>,
    path: String,
    out: &mut Vec<PluginNode>,
) {
    let kind = crate::viewnode::kind_of(node);
    if let Some(binding) = registry.for_kind(kind, surface, format) {
        // Serialize the node verbatim — unknown/future fields survive
        // to the plugin (forward compat is the plugin's problem too).
        let json = serde_json::to_value(node).unwrap_or(Value::Null);
        out.push(PluginNode {
            path: path.clone(),
            kind: kind.to_string(),
            binding: binding.clone(),
            node: json,
        });
        // A plugin-rendered container still counts once — plugins own
        // the whole subtree semantics for claimed kinds. But children
        // of UNCLAIMED containers must still be visited: only descend
        // when this node itself wasn't claimed.
        return;
    }
    for (i, child) in pavo_core_children(node).iter().enumerate() {
        let child_path = if path.is_empty() {
            i.to_string()
        } else {
            format!("{path}.{i}")
        };
        walk_plugin(child, registry, surface, format, child_path, out);
    }
}

/// Children accessor — delegates to [`crate::viewnode::children`].
fn pavo_core_children(node: &ViewNode) -> &[ViewNode] {
    crate::viewnode::children(node)
}

#[cfg(test)]
#[cfg_attr(coverage, coverage(off))]
mod tests {
    #![allow(clippy::unwrap_used)]
    use super::*;
    use pavo_protocol::{CliSurface, RendererSurfaces, WebSurface};
    use serde_json::json;

    fn manifest(renderers: Vec<RendererDecl>) -> DriverManifest {
        DriverManifest {
            name: "drv".to_string(),
            title: "Drv".to_string(),
            icon: None,
            version: "0".to_string(),
            verbs: vec!["serve".to_string()],
            capabilities: vec!["pavo".to_string()],
            deps: None,
            contributes: Default::default(),
            views: vec![],
            renderers,
            actions: vec![],
        }
    }

    fn decl(id: &str, kinds: &[&str], priority: i32) -> RendererDecl {
        RendererDecl {
            id: id.to_string(),
            kinds: kinds.iter().map(|s| s.to_string()).collect(),
            surfaces: RendererSurfaces {
                cli: Some(CliSurface {
                    formats: vec!["svg".to_string(), "text".to_string()],
                }),
                tui: true,
                web: Some(WebSurface {
                    component: None,
                    island: true,
                }),
            },
            priority,
            entry: None,
        }
    }

    #[test]
    fn registry_resolves_explicit_and_wildcard() {
        let graph = decl("graph", &["graph"], 100);
        let any = decl("any", &["*"], 100);
        let m1 = manifest(vec![graph]);
        let m2 = manifest(vec![any]);
        let reg = RendererRegistry::from_manifests([("aaa", &m1), ("zzz", &m2)]);
        // explicit beats wildcard regardless of driver order
        let b = reg.for_kind("graph", "cli", Some("svg")).unwrap();
        assert_eq!(b.driver, "aaa");
        // wildcard covers unclaimed kinds on its declared surfaces
        let b = reg.for_kind("timeline", "tui", None).unwrap();
        assert_eq!(b.driver, "zzz");
        let b = reg.for_kind("table", "cli", Some("text")).unwrap();
        assert_eq!(b.driver, "zzz");
        // …but not on surfaces the wildcard doesn't declare
        assert!(reg.for_kind("table", "watch", None).is_none());
        // no claim at all for an empty-kind case is covered above;
        // a registry with no renderers handles nothing.
        assert!(RendererRegistry::default()
            .for_kind("graph", "cli", Some("svg"))
            .is_none());
    }

    #[test]
    fn priority_and_driver_order_resolve_conflicts() {
        // Two drivers claim "graph"; lower priority wins.
        let hi = manifest(vec![decl("g", &["graph"], 50)]);
        let lo = manifest(vec![decl("g", &["graph"], 10)]);
        let reg = RendererRegistry::from_manifests([("aaa", &hi), ("zzz", &lo)]);
        assert_eq!(
            reg.for_kind("graph", "cli", Some("svg")).unwrap().driver,
            "zzz"
        );
        // Same priority → driver kind asc.
        let a = manifest(vec![decl("g", &["graph"], 10)]);
        let b = manifest(vec![decl("g", &["graph"], 10)]);
        let reg = RendererRegistry::from_manifests([("bbb", &b), ("aaa", &a)]);
        assert_eq!(
            reg.for_kind("graph", "cli", Some("svg")).unwrap().driver,
            "aaa"
        );
    }

    #[test]
    fn cli_format_matching() {
        let m = manifest(vec![decl("g", &["graph"], 100)]);
        let reg = RendererRegistry::from_manifests([("d", &m)]);
        assert!(reg.for_kind("graph", "cli", Some("svg")).is_some());
        assert!(reg.for_kind("graph", "cli", Some("text")).is_some());
        assert!(reg.for_kind("graph", "cli", Some("ascii")).is_none());
        assert!(reg.for_kind("graph", "cli", None).is_some());
        assert!(reg.for_kind("graph", "web", None).is_some());
        assert!(reg.for_kind("graph", "tui", None).is_some());
        assert!(reg.for_kind("graph", "watch", None).is_none());
    }

    #[test]
    fn empty_cli_formats_is_wildcard() {
        let mut d = decl("g", &["graph"], 100);
        d.surfaces.cli = Some(CliSurface { formats: vec![] });
        let m = manifest(vec![d]);
        let reg = RendererRegistry::from_manifests([("d", &m)]);
        assert!(reg.for_kind("graph", "cli", Some("anything")).is_some());
    }

    #[test]
    fn plugin_nodes_walks_tree_with_paths() {
        let m = manifest(vec![decl("g", &["graph"], 100)]);
        let reg = RendererRegistry::from_manifests([("d", &m)]);
        let root: ViewNode = serde_json::from_value(json!({
            "kind": "column", "children": [
                {"kind": "kv", "pairs": []},
                {"kind": "panel", "children": [
                    {"kind": "graph", "nodes": [], "edges": []}
                ]}
            ]
        }))
        .unwrap();
        let nodes = plugin_nodes(&root, &reg, "tui", None);
        assert_eq!(nodes.len(), 1);
        assert_eq!(nodes[0].path, "1.0");
        assert_eq!(nodes[0].kind, "graph");
        assert_eq!(nodes[0].node["kind"], "graph");
        // cli+svg also finds it; cli+unlisted-format doesn't
        assert_eq!(plugin_nodes(&root, &reg, "cli", Some("svg")).len(), 1);
        assert!(plugin_nodes(&root, &reg, "cli", Some("dot")).is_empty());
    }

    #[test]
    fn claimed_container_suppresses_descendants() {
        // A plugin claiming "column" owns the whole subtree — its
        // children aren't separately routed.
        let m = manifest(vec![decl("c", &["column"], 100)]);
        let reg = RendererRegistry::from_manifests([("d", &m)]);
        let root: ViewNode = serde_json::from_value(json!({
            "kind": "column", "children": [
                {"kind": "graph", "nodes": [], "edges": []}
            ]
        }))
        .unwrap();
        let nodes = plugin_nodes(&root, &reg, "tui", None);
        assert_eq!(nodes.len(), 1);
        assert_eq!(nodes[0].kind, "column");
        assert_eq!(nodes[0].path, "");
    }
}
