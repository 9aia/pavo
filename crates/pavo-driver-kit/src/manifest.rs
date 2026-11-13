//! Manifest builders — [`DriverManifest`], [`ViewSpec`],
//! [`QueryBinding`], and [`Contribution`] as fluent data, not struct
//! literals.
//!
//! The shapes are `pavo-protocol`'s; the kit only removes the noise:
//! sensible defaults (`verbs: ["manifest","serve"]`,
//! `capabilities: ["pavo"]`, priority `100`), push-style slot methods,
//! and the two query-binding conventions drivers keep rewriting —
//! the own-driver `query` op ([`query`]) and the cross-driver
//! `QueryBinding.driver` binding ([`foreign_query`]).

use serde_json::{json, Map, Value};

use pavo_protocol::{
    Contributes, Contribution, DriverManifest, QueryBinding, RendererDecl, ViewNode, ViewSpec,
};

/// Fluent builder for a [`DriverManifest`] — the `manifest` verb's
/// payload.
///
/// Defaults match the common case: `verbs` is `["manifest","serve"]`,
/// `capabilities` is `["pavo"]`, and `version` is the *kit's* crate
/// version — call [`Manifest::version`] with
/// `env!("CARGO_PKG_VERSION")` (evaluated in *your* crate) when the
/// driver versions independently.
///
/// ```
/// # use pavo_driver_kit as kit;
/// # use pavo_protocol::ViewNode;
/// let m = kit::Manifest::new("demo", "Demo")
///     .version(env!("CARGO_PKG_VERSION"))
///     .icon("🦚")
///     .capability("config")
///     .optional("config")
///     .view(kit::view("board", "Board", "/demo/board",
///         kit::markdown(kit::base().title("Hi"), "hello")))
///     .sidebar(kit::contribution("home", "Home", "/demo/board"))
///     .build();
/// assert_eq!(m.name, "demo");
/// assert_eq!(m.deps.unwrap().optional, vec!["config".to_string()]);
/// assert_eq!(m.contributes.sidebar.len(), 1);
/// ```
pub struct Manifest {
    manifest: DriverManifest,
}

impl Manifest {
    /// Start a manifest — `name` is the `<type>` in
    /// `tamias-driver-<type>`; `title` is the chrome label.
    #[must_use]
    pub fn new(name: &str, title: &str) -> Self {
        Self {
            manifest: DriverManifest {
                name: name.to_string(),
                title: title.to_string(),
                icon: None,
                version: env!("CARGO_PKG_VERSION").to_string(),
                verbs: vec!["manifest".to_string(), "serve".to_string()],
                capabilities: vec!["pavo".to_string()],
                deps: None,
                contributes: Contributes::default(),
                views: Vec::new(),
                renderers: Vec::new(),
                actions: Vec::new(),
            },
        }
    }

    /// Driver version — pass `env!("CARGO_PKG_VERSION")` so it
    /// evaluates in the *driver's* crate, not the kit's.
    #[must_use]
    pub fn version(mut self, version: impl Into<String>) -> Self {
        self.manifest.version = version.into();
        self
    }

    /// Icon hint (emoji, icon name, or renderer-resolvable path).
    #[must_use]
    pub fn icon(mut self, icon: impl Into<String>) -> Self {
        self.manifest.icon = Some(icon.into());
        self
    }

    /// Extra wire verb beyond the `manifest`/`serve` defaults (e.g.
    /// `"capabilities"` for drivers that answer that probe).
    #[must_use]
    pub fn verb(mut self, verb: impl Into<String>) -> Self {
        self.manifest.verbs.push(verb.into());
        self
    }

    /// Extra capability tag beyond `"pavo"`.
    #[must_use]
    pub fn capability(mut self, capability: impl Into<String>) -> Self {
        self.manifest.capabilities.push(capability.into());
        self
    }

    /// A driver this one **requires** — absence is a discovery error
    /// (see [`pavo_protocol::DriverDeps`]).
    #[must_use]
    pub fn requires(mut self, driver: impl Into<String>) -> Self {
        self.deps_mut().requires.push(driver.into());
        self
    }

    /// A driver this one uses **when present** — absence degrades
    /// gracefully, never blocks discovery.
    #[must_use]
    pub fn optional(mut self, driver: impl Into<String>) -> Self {
        self.deps_mut().optional.push(driver.into());
        self
    }

    /// Add a view — accepts a [`ViewSpec`] or the [`View`] builder
    /// (via [`Into<ViewSpec>`]).
    #[must_use]
    pub fn view(mut self, spec: impl Into<ViewSpec>) -> Self {
        self.manifest.views.push(spec.into());
        self
    }

    /// Declare a view-kind renderer plugin (the `renderers[]` slot).
    #[must_use]
    pub fn renderer(mut self, decl: RendererDecl) -> Self {
        self.manifest.renderers.push(decl);
        self
    }

    /// `contributes.navbar` entry.
    #[must_use]
    pub fn navbar(mut self, entry: Contribution) -> Self {
        self.manifest.contributes.navbar.push(entry);
        self
    }

    /// `contributes.dashboard` entry.
    #[must_use]
    pub fn dashboard(mut self, entry: Contribution) -> Self {
        self.manifest.contributes.dashboard.push(entry);
        self
    }

    /// `contributes.statusbar` entry.
    #[must_use]
    pub fn statusbar(mut self, entry: Contribution) -> Self {
        self.manifest.contributes.statusbar.push(entry);
        self
    }

    /// `contributes.routes` entry — a standalone route outside the
    /// driver's views.
    #[must_use]
    pub fn routes(mut self, entry: Contribution) -> Self {
        self.manifest.contributes.routes.push(entry);
        self
    }

    /// `contributes.sidebar` entry — the left-sidebar shell slot.
    #[must_use]
    pub fn sidebar(mut self, entry: Contribution) -> Self {
        self.manifest.contributes.sidebar.push(entry);
        self
    }

    /// `contributes.headerActions` entry (wire spelling) — top-bar
    /// actions; `route` names the view the host renders in the
    /// popover/panel.
    #[must_use]
    pub fn header_actions(mut self, entry: Contribution) -> Self {
        self.manifest.contributes.header_actions.push(entry);
        self
    }

    /// `contributes.notifications` entry — marks the driver as a
    /// notification *provider*; item data flows through the
    /// `notifications` wire op ([`crate::platform::notification`]).
    #[must_use]
    pub fn notifications(mut self, entry: Contribution) -> Self {
        self.manifest.contributes.notifications.push(entry);
        self
    }

    /// `contributes.jobs` entry — marks the driver as a job-feed
    /// provider; item data flows through the `jobs` wire op
    /// ([`crate::platform::job`]).
    #[must_use]
    pub fn jobs(mut self, entry: Contribution) -> Self {
        self.manifest.contributes.jobs.push(entry);
        self
    }

    /// A contribution in a slot the kit doesn't name — lands in
    /// `Contributes::extra` and round-trips untouched (forward
    /// compat for newer slots on older hosts).
    #[must_use]
    pub fn extra_slot(mut self, slot: &str, entry: Contribution) -> Self {
        self.manifest
            .contributes
            .extra
            .entry(slot.to_string())
            .or_default()
            .push(entry);
        self
    }

    /// Finish — the [`DriverManifest`] to answer `manifest` with.
    #[must_use]
    pub fn build(self) -> DriverManifest {
        self.manifest
    }

    fn deps_mut(&mut self) -> &mut pavo_protocol::DriverDeps {
        self.manifest
            .deps
            .get_or_insert_with(pavo_protocol::DriverDeps::default)
    }
}

/// Fluent builder for a [`ViewSpec`] — a route plus its query
/// bindings and declarative root.
///
/// ```
/// # use pavo_driver_kit as kit;
/// # use pavo_protocol::ViewNode;
/// let spec = kit::view("board", "Board", "/demo/board",
///     kit::markdown(kit::base(), "hi"))
///     .description("the board")
///     .query(kit::query("items", "items"))
///     .build();
/// assert_eq!(spec.id, "board");
/// assert_eq!(spec.queries[0].name, "items");
/// ```
pub struct View {
    spec: ViewSpec,
}

/// Start a view — `id` is unique within the driver, `route` the
/// host-relative path (`/<name>/<view>` by convention), `root` the
/// declarative tree ([`crate::node`] constructors build it).
#[must_use]
pub fn view(id: &str, title: &str, route: &str, root: ViewNode) -> View {
    View {
        spec: ViewSpec {
            id: id.to_string(),
            title: title.to_string(),
            route: route.to_string(),
            description: None,
            queries: Vec::new(),
            root,
        },
    }
}

impl View {
    /// Longer description for picker UIs.
    #[must_use]
    pub fn description(mut self, description: impl Into<String>) -> Self {
        self.spec.description = Some(description.into());
        self
    }

    /// Add a query binding — the name [`crate::node::Base::query`]
    /// fields in the tree reference.
    #[must_use]
    pub fn query(mut self, binding: QueryBinding) -> Self {
        self.spec.queries.push(binding);
        self
    }

    /// Finish — the [`ViewSpec`] for [`Manifest::view`].
    #[must_use]
    pub fn build(self) -> ViewSpec {
        self.spec
    }
}

impl From<View> for ViewSpec {
    fn from(view: View) -> Self {
        view.spec
    }
}

/// A [`QueryBinding`] for the driver's own `query` wire op carrying
/// the real product op name — the convention `pavo-driver-config`
/// established (`params["query"]` is the query the driver dispatches
/// on; the binding `name` is what nodes reference).
///
/// ```
/// # use pavo_driver_kit as kit;
/// let b = kit::query("items", "items");
/// assert_eq!(b.op, "query");
/// assert_eq!(b.params["query"], "items");
/// ```
#[must_use]
pub fn query(name: &str, product_op: &str) -> QueryBinding {
    binding_params(
        name,
        "query",
        Map::from_iter([("query".to_string(), Value::from(product_op))]),
    )
}

/// A [`QueryBinding`] resolved against a *foreign* driver — the
/// cross-driver half of the query contract (`QueryBinding.driver`).
/// The host issues the binding's `op` against `driver`'s `serve`
/// instead of the view's own; `product_op` rides in `params["query"]`
/// like [`query`].
///
/// ```
/// # use pavo_driver_kit as kit;
/// let b = kit::foreign_query("config", "cfg", "get");
/// assert_eq!(b.driver.as_deref(), Some("config"));
/// assert_eq!(b.params["query"], "get");
/// ```
#[must_use]
pub fn foreign_query(driver: &str, name: &str, product_op: &str) -> QueryBinding {
    binding_on_params(
        driver,
        name,
        "query",
        Map::from_iter([("query".to_string(), Value::from(product_op))]),
    )
}

/// A generic [`QueryBinding`] — `op` is the wire op the host calls on
/// the driver's `serve` (`"query"` for the common case; see
/// [`query`] for the shorthand).
#[must_use]
pub fn binding(name: &str, op: &str) -> QueryBinding {
    QueryBinding {
        name: name.to_string(),
        op: op.to_string(),
        driver: None,
        params: Map::new(),
    }
}

/// [`binding`] with static params merged under caller-supplied view
/// params on every invocation.
#[must_use]
pub fn binding_params(name: &str, op: &str, params: Map<String, Value>) -> QueryBinding {
    QueryBinding {
        params,
        ..binding(name, op)
    }
}

/// A generic cross-driver [`QueryBinding`] — like [`binding`] but the
/// host resolves `driver` (a driver name, the `<type>` in
/// `tamias-driver-<type>`) and issues `op` against *its* `serve`.
#[must_use]
pub fn binding_on(driver: &str, name: &str, op: &str) -> QueryBinding {
    QueryBinding {
        driver: Some(driver.to_string()),
        ..binding(name, op)
    }
}

/// [`binding_on`] with static params.
#[must_use]
pub fn binding_on_params(
    driver: &str,
    name: &str,
    op: &str,
    params: Map<String, Value>,
) -> QueryBinding {
    QueryBinding {
        params,
        ..binding_on(driver, name, op)
    }
}

/// A [`Contribution`] — one entry in a `contributes` slot.
/// `priority` defaults to `100` (lower sorts first); set
/// `entry.priority`/`entry.icon` directly for the rest — the fields
/// are public.
///
/// ```
/// # use pavo_driver_kit as kit;
/// let mut c = kit::contribution("board", "Board", "/demo/board");
/// c.priority = 10;
/// assert_eq!(c.id, "board");
/// ```
#[must_use]
pub fn contribution(id: &str, title: &str, route: &str) -> Contribution {
    Contribution {
        id: id.to_string(),
        title: title.to_string(),
        route: route.to_string(),
        priority: 100,
        icon: None,
    }
}

/// The `manifest` verb payload — pretty-printed JSON for stdout.
/// Serialization of a well-formed manifest can't fail; the fallback
/// keeps the verb total.
#[must_use]
pub fn manifest_json(manifest: &DriverManifest) -> String {
    serde_json::to_string_pretty(manifest).unwrap_or_else(|_| "{}".to_string())
}

/// The `capabilities` verb payload — `{"capabilities":…,"verbs":…}`,
/// the shape every sibling driver answers.
#[must_use]
pub fn capabilities_value(manifest: &DriverManifest) -> Value {
    json!({
        "capabilities": manifest.capabilities,
        "verbs": manifest.verbs,
    })
}

#[cfg(test)]
#[cfg_attr(coverage, coverage(off))]
mod tests {
    #![allow(clippy::unwrap_used)]

    use super::*;
    use serde_json::json;

    /// The full builder surface lands in the manifest JSON.
    #[test]
    fn manifest_builder_covers_slots_and_deps() {
        let m = Manifest::new("demo", "Demo")
            .version("9.9.9")
            .icon("🦚")
            .verb("capabilities")
            .capability("config")
            .requires("core")
            .optional("config")
            .view(view(
                "board",
                "Board",
                "/demo/board",
                crate::node::markdown(crate::node::base().title("Hi"), "hello"),
            ))
            .sidebar(contribution("home", "Home", "/demo/board"))
            .header_actions(contribution("jobs", "Jobs", "/demo/jobs"))
            .notifications(contribution("bell", "Alerts", "/demo/board"))
            .jobs(contribution("feed", "Jobs", "/demo/jobs"))
            .extra_slot("future", contribution("x", "X", "/x"))
            .build();
        let v = serde_json::to_value(&m).unwrap();
        assert_eq!(v["name"], "demo");
        assert_eq!(v["version"], "9.9.9");
        assert_eq!(v["verbs"], json!(["manifest", "serve", "capabilities"]));
        assert_eq!(v["deps"]["requires"], json!(["core"]));
        assert_eq!(v["deps"]["optional"], json!(["config"]));
        assert_eq!(v["contributes"]["sidebar"][0]["id"], "home");
        assert_eq!(v["contributes"]["headerActions"][0]["id"], "jobs");
        assert_eq!(v["contributes"]["notifications"][0]["id"], "bell");
        assert_eq!(v["contributes"]["jobs"][0]["id"], "feed");
        assert_eq!(v["contributes"]["future"][0]["id"], "x");
        assert_eq!(v["views"][0]["id"], "board");
    }

    /// Defaults: pavo capability, manifest+serve verbs, no deps.
    #[test]
    fn manifest_defaults() {
        let m = Manifest::new("d", "D").build();
        assert_eq!(m.capabilities, vec!["pavo".to_string()]);
        assert_eq!(m.verbs, vec!["manifest".to_string(), "serve".to_string()]);
        assert!(m.deps.is_none());
        assert!(serde_json::to_value(&m).unwrap().get("deps").is_none());
    }

    /// `query`/`foreign_query` produce the two binding conventions.
    #[test]
    fn query_bindings() {
        let local = query("items", "items");
        assert_eq!(local.op, "query");
        assert!(local.driver.is_none());
        assert_eq!(local.params["query"], "items");

        let foreign = foreign_query("config", "cfg", "get");
        assert_eq!(foreign.driver.as_deref(), Some("config"));
        assert_eq!(foreign.params["query"], "get");

        let generic = binding_on_params(
            "config",
            "raw",
            "query",
            Map::from_iter([("key".to_string(), json!("myco.theme"))]),
        );
        assert_eq!(generic.params["key"], json!("myco.theme"));
    }

    /// `manifest_json`/`capabilities_value` match the sibling-verb
    /// shapes drivers hand-roll.
    #[test]
    fn verb_payloads() {
        let m = Manifest::new("d", "D").build();
        let caps = capabilities_value(&m);
        assert_eq!(caps["capabilities"], json!(["pavo"]));
        assert!(manifest_json(&m).contains("\"name\": \"d\""));
    }
}
