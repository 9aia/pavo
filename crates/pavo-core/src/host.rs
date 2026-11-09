//! [`Host`] — the composition root renderers talk to.
//!
//! Lifecycle: [`Host::discover`] scans for pavo-capable drivers and
//! keeps their manifests (eager where the `manifest` verb answered,
//! lazy via the `serve` wire otherwise). From there:
//!
//! - [`Host::chrome`] merges every driver's `contributes` into one
//!   [`Chrome`] (see [`crate::merge`] for the ordering policy).
//! - [`Host::list_views`] / [`Host::list_routes`] enumerate what the
//!   host can show — namespaced `<driver>:<view>` ids included.
//! - [`Host::resolve_view`] turns a deep link (`"myco:board"`, or a
//!   bare `"myco"` for the driver's first view) into a
//!   [`ViewResult`] by spawning/using that driver's `serve` client —
//!   including **cross-driver query bindings** (`QueryBinding.driver`),
//!   which the host resolves against the named driver's `serve`.
//! - [`Host::query`] / [`Host::client`] are the escape hatches for
//!   view-model ops and form submits.
//! - [`Host::jobs`] / [`Host::notifications`] fan the platform-feed
//!   ops out to every provider driver (manifests declaring the
//!   matching `contributes` slot).
//!
//! Drivers are kept in **dependency order** ([`crate::deps`]): a
//! driver's `deps.requires`/`deps.optional` sort it after the drivers
//! it names. Drivers whose manifests arrive lazily are unordered
//! until fetched — their `requires` are validated on first
//! [`Host::manifest`] call, and offenders are quarantined: they stay
//! in [`Host::drivers`] but every manifest/view/feed access surfaces
//! the [`PavoError::Deps`] error.
//!
//! View results are cached in a [`TtlCache`] (default 10s,
//! [`Host::with_view_cache_ttl`]) keyed by `driver:view` + canonical
//! params — repeat renders of the same deep link don't respawn
//! round-trips. `serve` clients are spawned lazily on first use and
//! live until the `Host` drops (each `DriverClient`'s `Drop` kills its
//! child).
//!
//! [`Host`] is `Send + Sync` and every method takes `&self` — share
//! one behind an `Arc` (e.g. `pavo serve`'s per-connection threads).
//! Concurrency is serialized **per driver**: each `DriverClient` sits
//! behind its own [`Mutex`], so requests to different drivers proceed
//! in parallel while requests to the same driver stay one-in-flight
//! on its single `serve` process (the ndjson wire is
//! request/response ordered — interleaved writes would misattribute
//! responses).
//!
//! Errors are typed: [`PavoError::UnknownDriver`] /
//! [`PavoError::UnknownView`] / [`PavoError::BadViewRef`] for bad
//! targets, driver/transport errors propagated from the client.

use std::collections::HashMap;
use std::sync::{Arc, Mutex};
use std::time::Duration;

use pavo_protocol::wire::{QueryResult, SubmitResult, ViewResult, ViewSummary};
use pavo_protocol::{
    DriverDeps, DriverManifest, JobItem, NotificationItem, QueryBinding, RenderModel, Request,
};
use serde_json::{json, Value};

use crate::cache::TtlCache;
use crate::client::{ClientConfig, DriverClient};
use crate::deps::{self, DepsError};
use crate::discovery::{self, DiscoveryOptions, DriverInfo};
use crate::error::PavoError;
use crate::merge::{merge_chrome, Chrome};
use crate::renderers::{plugin_nodes, RendererRegistry};
use crate::Params;

/// Default view-result cache TTL.
const DEFAULT_VIEW_CACHE_TTL: Duration = Duration::from_secs(10);

/// The pavo host — discovered drivers, lazy serve clients, merged
/// chrome, view cache. `Send + Sync`: all mutable state is behind
/// mutexes, and the only lock held across a driver round-trip is that
/// driver's own client lock.
pub struct Host {
    drivers: Vec<DriverInfo>,
    /// Live `serve` clients keyed by driver kind. The outer mutex
    /// guards only the map (lookup + lazy spawn); each value carries
    /// its own [`Mutex`] so requests to different drivers run in
    /// parallel while same-driver requests serialize on the wire.
    clients: Mutex<HashMap<String, Arc<Mutex<DriverClient>>>>,
    /// Manifests fetched lazily over the `serve` wire for drivers
    /// whose `manifest` probe didn't answer eagerly (those stay on
    /// [`DriverInfo::manifest`]).
    manifest_cache: Mutex<HashMap<String, DriverManifest>>,
    /// Drivers quarantined by dep validation at construction —
    /// kind → the [`DepsError`] every manifest/view/feed access for
    /// that driver surfaces. See [`crate::deps`].
    dep_failures: HashMap<String, DepsError>,
    client_config: ClientConfig,
    view_cache: TtlCache<ViewResult>,
}

/// A mutex-poisoned failure — a thread panicked while holding a host
/// lock. Surfaced as `io` so callers see an ordinary error.
fn poisoned(what: &str) -> PavoError {
    PavoError::Io(std::io::Error::other(format!("{what} lock poisoned")))
}

/// A resolved view: which driver served it plus the wire result.
#[derive(Clone, Debug)]
pub struct ResolvedView {
    /// Driver kind that owns the view.
    pub driver: String,
    /// The view id *within* the driver (un-namespaced).
    pub view: String,
    /// `<driver>:<view>` — the canonical deep-link id.
    pub namespaced: String,
    /// The `view` op's result — spec tree + query data.
    pub result: ViewResult,
}

/// One platform-feed item plus the driver that produced it — the
/// attribution `pavo_protocol::platform` item types deliberately
/// leave to the host ("the host knows which driver produced an
/// item").
#[derive(Clone, Debug, PartialEq)]
pub struct FeedEntry<T> {
    /// Driver kind that serves the item.
    pub driver: String,
    /// Driver display title (manifest `title`).
    pub driver_title: String,
    /// The feed item itself.
    pub item: T,
}

/// One row in [`Host::list_views`] — a view a driver offers.
#[derive(Clone, Debug)]
pub struct ViewEntry {
    /// Driver kind.
    pub driver: String,
    /// Driver display title.
    pub driver_title: String,
    /// View id within the driver.
    pub id: String,
    /// `<driver>:<id>` — pass this to [`Host::resolve_view`].
    pub namespaced: String,
    /// Route the view answers (e.g. `/myco/board`).
    pub route: String,
    /// Human-facing title.
    pub title: String,
    /// Optional description.
    pub description: Option<String>,
    /// Query binding names the view uses.
    pub queries: Vec<String>,
}

/// Where a [`RouteEntry`] comes from.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum RouteKind {
    /// A driver's declared view route — resolves via `resolve_view`.
    View,
    /// A `contributes.routes` entry — the driver owns the page itself.
    Slot(String),
}

/// One navigable route across all drivers.
#[derive(Clone, Debug)]
pub struct RouteEntry {
    /// Host-relative route (`/myco/board`).
    pub route: String,
    /// Driver kind.
    pub driver: String,
    /// Display title.
    pub title: String,
    /// View routes: the namespaced `<driver>:<view>` id. Slot routes:
    /// the namespaced contribution id.
    pub target: String,
    /// What kind of route this is.
    pub kind: RouteKind,
}

impl Host {
    /// Discover pavo-capable drivers with environment defaults and
    /// build a host around them. Never fails — discovery skips broken
    /// drivers; failures surface later at first use.
    #[must_use]
    pub fn discover() -> Self {
        Self::discover_with(DiscoveryOptions::default())
    }

    /// [`Host::discover`] with explicit scan/probe options (tests,
    /// embedders).
    #[must_use]
    pub fn discover_with(options: DiscoveryOptions) -> Self {
        Self::from_drivers(discovery::discover_with(&options))
    }

    /// Build a host from an explicit driver list — for tests and
    /// embedders that did their own discovery.
    ///
    /// Drivers are reordered into dependency order (see
    /// [`crate::deps`]) using the manifests discovery already fetched;
    /// drivers with lazy manifests sort unconstrained and get their
    /// `requires` validated on first [`Host::manifest`] fetch.
    /// Dep-graph failures don't fail construction — the offending
    /// drivers are quarantined via [`Host::manifest`] errors instead.
    #[must_use]
    pub fn from_drivers(mut drivers: Vec<DriverInfo>) -> Self {
        let dep_input: Vec<(&str, Option<&DriverDeps>)> = drivers
            .iter()
            .map(|d| {
                (
                    d.kind.as_str(),
                    d.manifest.as_ref().and_then(|m| m.deps.as_ref()),
                )
            })
            .collect();
        let mut dep_failures = HashMap::new();
        match deps::order(dep_input) {
            Ok(order) => {
                let pos: HashMap<String, usize> =
                    order.into_iter().enumerate().map(|(i, k)| (k, i)).collect();
                drivers.sort_by_key(|d| pos.get(&d.kind).copied().unwrap_or(usize::MAX));
            }
            Err(e) => {
                for offender in e.offenders() {
                    dep_failures.insert(offender, e.clone());
                }
            }
        }
        Self {
            drivers,
            clients: Mutex::new(HashMap::new()),
            manifest_cache: Mutex::new(HashMap::new()),
            dep_failures,
            client_config: ClientConfig::default(),
            view_cache: TtlCache::new(DEFAULT_VIEW_CACHE_TTL),
        }
    }

    /// Builder: override the per-request timeout / restart policy for
    /// all spawned clients.
    #[must_use]
    pub fn with_client_config(mut self, config: ClientConfig) -> Self {
        self.client_config = config;
        self
    }

    /// Builder: set the view-result cache TTL.
    #[must_use]
    pub fn with_view_cache_ttl(mut self, ttl: Duration) -> Self {
        self.view_cache = TtlCache::new(ttl);
        self
    }

    /// The discovered drivers in dependency order — drivers sort
    /// after the deps their manifests declare (name ascending as the
    /// tie-break). Dep-quarantined drivers stay listed; they error on
    /// use instead.
    #[must_use]
    pub fn drivers(&self) -> &[DriverInfo] {
        &self.drivers
    }

    /// Look up a driver by kind.
    #[must_use]
    pub fn driver(&self, kind: &str) -> Option<&DriverInfo> {
        self.drivers.iter().find(|d| d.kind == kind)
    }

    /// The driver's manifest — from discovery when eager, otherwise
    /// fetched once over the `serve` wire's `manifest` op and cached
    /// in [`Host::manifest_cache`].
    ///
    /// # Errors
    /// [`PavoError::UnknownDriver`] for undiscovered kinds; client
    /// errors propagate.
    pub fn manifest(&self, kind: &str) -> Result<DriverManifest, PavoError> {
        let info = self
            .drivers
            .iter()
            .find(|d| d.kind == kind)
            .ok_or_else(|| PavoError::UnknownDriver(kind.to_string()))?;
        // Dep-quarantined drivers fail loudly here — every consumer
        // (chrome, views, feeds, resolve) goes through manifest().
        if let Some(e) = self.dep_failures.get(kind) {
            return Err(PavoError::Deps(e.clone()));
        }
        if let Some(manifest) = &info.manifest {
            return Ok(manifest.clone());
        }
        if let Some(manifest) = self
            .manifest_cache
            .lock()
            .map_err(|_| poisoned("manifest cache"))?
            .get(kind)
            .cloned()
        {
            return Ok(manifest);
        }
        // Two threads may race the fetch — both answer correctly and
        // the last insert wins; the wire op is idempotent.
        let manifest = self.with_client(kind, DriverClient::manifest)?;
        // Eager manifests were dep-validated at construction; lazy
        // ones prove their `requires` now that we can see them.
        self.check_requires(kind, &manifest)?;
        self.manifest_cache
            .lock()
            .map_err(|_| poisoned("manifest cache"))?
            .insert(kind.to_string(), manifest.clone());
        Ok(manifest)
    }

    /// The live `serve` client for `kind`, spawning on first use.
    ///
    /// The returned [`Arc<Mutex<DriverClient>>`] is the per-driver
    /// serialization point — [`Host::with_client`] locks it for you.
    /// The spawn happens under the clients-map lock so two threads
    /// can't double-spawn; the lock is released before any request.
    ///
    /// # Errors
    /// [`PavoError::UnknownDriver`] / [`PavoError::MissingDriver`] /
    /// spawn errors.
    pub fn client(&self, kind: &str) -> Result<Arc<Mutex<DriverClient>>, PavoError> {
        let mut map = self.clients.lock().map_err(|_| poisoned("clients"))?;
        if let Some(client) = map.get(kind) {
            return Ok(Arc::clone(client));
        }
        let info = self
            .drivers
            .iter()
            .find(|d| d.kind == kind)
            .ok_or_else(|| PavoError::UnknownDriver(kind.to_string()))?;
        // Dep-quarantined drivers never spawn — every wire op goes
        // through here.
        if let Some(e) = self.dep_failures.get(kind) {
            return Err(PavoError::Deps(e.clone()));
        }
        let client = DriverClient::spawn(info.path.clone(), self.client_config.clone())?;
        let client = Arc::new(Mutex::new(client));
        map.insert(kind.to_string(), Arc::clone(&client));
        Ok(client)
    }

    /// Validate a lazily-fetched manifest's `deps.requires` against
    /// the discovered driver set — the same check
    /// [`crate::deps::order`] ran at construction for eager manifests.
    /// A self-requirement is a degenerate cycle.
    fn check_requires(&self, kind: &str, manifest: &DriverManifest) -> Result<(), PavoError> {
        let Some(deps) = &manifest.deps else {
            return Ok(());
        };
        for req in &deps.requires {
            if req == kind {
                return Err(DepsError::Cycle {
                    drivers: vec![kind.to_string()],
                }
                .into());
            }
            if !self.drivers.iter().any(|d| d.kind == *req) {
                return Err(DepsError::Missing {
                    driver: kind.to_string(),
                    dep: req.clone(),
                }
                .into());
            }
        }
        Ok(())
    }

    /// Run `f` against `kind`'s client under its per-driver lock —
    /// the only place a [`DriverClient`] is borrowed mutably.
    fn with_client<T>(
        &self,
        kind: &str,
        f: impl FnOnce(&mut DriverClient) -> Result<T, PavoError>,
    ) -> Result<T, PavoError> {
        let client = self.client(kind)?;
        let mut guard = client.lock().map_err(|_| poisoned("driver client"))?;
        f(&mut guard)
    }

    /// Merge every driver's `contributes` into one [`Chrome`].
    /// Drivers whose lazy manifest can't be fetched are skipped —
    /// their slots contribute nothing rather than failing the whole
    /// chrome.
    #[must_use]
    pub fn chrome(&self) -> Chrome {
        let kinds: Vec<String> = self.drivers.iter().map(|d| d.kind.clone()).collect();
        let mut pairs: Vec<(String, DriverManifest)> = Vec::new();
        for kind in kinds {
            // A driver whose manifest can't be produced contributes
            // nothing — skip it rather than fail the whole chrome.
            if let Ok(m) = self.manifest(&kind) {
                pairs.push((kind, m));
            }
        }
        merge_chrome(pairs.iter().map(|(k, m)| (k.as_str(), m)))
    }

    /// Every view every driver offers, namespaced. Fetches lazy
    /// manifests as needed; drivers that can't produce one are
    /// skipped.
    #[must_use]
    pub fn list_views(&self) -> Vec<ViewEntry> {
        let mut out = Vec::new();
        let kinds: Vec<String> = self.drivers.iter().map(|d| d.kind.clone()).collect();
        for kind in kinds {
            let Ok(manifest) = self.manifest(&kind) else {
                continue;
            };
            for spec in &manifest.views {
                out.push(ViewEntry {
                    driver: kind.clone(),
                    driver_title: manifest.title.clone(),
                    id: spec.id.clone(),
                    namespaced: format!("{kind}:{}", spec.id),
                    route: spec.route.clone(),
                    title: spec.title.clone(),
                    description: spec.description.clone(),
                    queries: spec.queries.iter().map(|q| q.name.clone()).collect(),
                });
            }
        }
        out.sort_by(|a, b| a.namespaced.cmp(&b.namespaced));
        out
    }

    /// Every navigable route: view routes (`RouteKind::View`) plus
    /// `contributes.routes` slot entries (`RouteKind::Slot`).
    #[must_use]
    pub fn list_routes(&self) -> Vec<RouteEntry> {
        let mut out = Vec::new();
        for view in self.list_views() {
            out.push(RouteEntry {
                route: view.route,
                driver: view.driver,
                title: view.title,
                target: view.namespaced,
                kind: RouteKind::View,
            });
        }
        for entry in self.chrome().routes {
            out.push(RouteEntry {
                route: entry.contribution.route.clone(),
                driver: entry.driver.clone(),
                title: entry.contribution.title.clone(),
                target: entry.contribution.id.clone(),
                kind: RouteKind::Slot("routes".to_string()),
            });
        }
        out.sort_by(|a, b| a.route.cmp(&b.route).then(a.target.cmp(&b.target)));
        out
    }

    /// Resolve a deep link to a rendered view.
    ///
    /// `target` is `"<driver>:<view>"` (e.g. `"myco:board"`); a bare
    /// `"<driver>"` resolves to the driver's first declared view.
    /// `params` are caller-supplied view params passed to the driver's
    /// `view` op. Results are cached per `driver:view` + params.
    ///
    /// # Errors
    /// [`PavoError::BadViewRef`] on unparseable targets,
    /// [`PavoError::UnknownDriver`]/[`PavoError::UnknownView`] for
    /// missing pieces, client errors propagate.
    pub fn resolve_view(&self, target: &str, params: Params) -> Result<ResolvedView, PavoError> {
        let (driver, view) = self.parse_target(target)?;
        let manifest = self.manifest(&driver)?;
        let view_id = match view {
            Some(v) => v,
            None => manifest
                .views
                .first()
                .map(|v| v.id.clone())
                .ok_or_else(|| PavoError::UnknownView {
                    driver: driver.clone(),
                    view: "<default>".to_string(),
                })?,
        };
        if !manifest.views.iter().any(|v| v.id == view_id) {
            return Err(PavoError::UnknownView {
                driver,
                view: view_id,
            });
        }

        // Cache key: namespaced id + canonical params JSON.
        let namespaced = format!("{driver}:{view_id}");
        let key = format!(
            "{namespaced}\u{1f}{}",
            serde_json::to_string(&Value::Object(params.clone())).unwrap_or_default()
        );
        if let Some(result) = self.view_cache.get(&key) {
            return Ok(ResolvedView {
                driver,
                view: view_id,
                namespaced,
                result,
            });
        }

        // params are needed again below for cross-driver bindings —
        // the view op consumes a clone.
        let mut result =
            self.with_client(&driver, |client| client.view(&view_id, params.clone()))?;
        self.resolve_cross_driver_queries(&driver, &mut result, &params);
        self.view_cache.insert(key, result.clone());
        Ok(ResolvedView {
            driver,
            view: view_id,
            namespaced,
            result,
        })
    }

    /// Resolve `QueryBinding.driver` bindings — the cross-driver half
    /// of the query contract. The view's own driver can't answer a
    /// binding that names a *foreign* driver, so the host issues the
    /// binding's `op` against that driver's `serve` and stores the
    /// payload in `result.data[binding.name]` — the same map key
    /// local bindings use, so renderers can't tell the difference.
    ///
    /// Params merge like local bindings: the binding's static `params`
    /// under the caller's view `params` (caller wins — the convention
    /// `pavo-driver-config` established). The wire `query` name
    /// follows the rule drivers apply locally: `params["query"]` when
    /// present, else the binding name.
    ///
    /// Failure policy is the wire's partial-degradation convention:
    /// a foreign driver that errors, times out, or isn't discovered
    /// puts `{"error": "…"}` in the binding's data slot rather than
    /// failing the view (see [`ViewResult::data`]). A binding whose
    /// `driver` equals the view's own driver is left untouched — the
    /// driver resolves it (or doesn't) as a local binding.
    fn resolve_cross_driver_queries(&self, owner: &str, result: &mut ViewResult, params: &Params) {
        for binding in &result.view.queries {
            let Some(foreign) = binding.driver.as_deref() else {
                continue;
            };
            if foreign == owner {
                continue;
            }
            let payload = self.foreign_query(foreign, binding, params);
            result.data.insert(binding.name.clone(), payload);
        }
    }

    /// Issue one cross-driver binding against `driver`'s `serve` and
    /// return the payload for `result.data`. Never fails — errors
    /// become `{"error": "…"}` payloads (see
    /// [`Host::resolve_cross_driver_queries`]).
    fn foreign_query(&self, driver: &str, binding: &QueryBinding, params: &Params) -> Value {
        // Static params under caller params — caller wins, matching
        // how drivers merge `view` params into bindings.
        let mut merged = binding.params.clone();
        for (k, v) in params {
            merged.insert(k.clone(), v.clone());
        }
        // The wire `query` name: `params["query"]` carries the real
        // product op (pavo-driver-config's convention); absent, the
        // binding name is the query.
        let query = binding
            .params
            .get("query")
            .and_then(Value::as_str)
            .unwrap_or(binding.name.as_str())
            .to_string();
        // Build the wire request from the binding's `op`. Only ops
        // whose required fields we can synthesize parse — `query`
        // gets `query`+`params`; anything else degrades to an error
        // payload below.
        let request = serde_json::from_value::<Request>(json!({
            "op": binding.op,
            "query": query,
            "params": merged,
        }));
        let request = match request {
            Ok(r) => r,
            Err(e) => {
                return json!({"error": format!("bad cross-driver binding op '{}': {e}", binding.op)});
            }
        };
        // A `query` op answers QueryResult{query,data} — the data map
        // wants the payload, not the envelope. Other ops (should a
        // binding name one) pass their result through verbatim.
        if matches!(request, Request::Query { .. }) {
            self.with_client(driver, |client| client.request::<QueryResult>(&request))
                .map(|qr| qr.data)
                .unwrap_or_else(|e| json!({"error": e.to_string()}))
        } else {
            self.with_client(driver, |client| client.request::<Value>(&request))
                .unwrap_or_else(|e| json!({"error": e.to_string()}))
        }
    }

    /// `{"op":"query"}` passthrough — one named query/binding (or raw
    /// product op for thin drivers) against `driver`'s `serve`.
    ///
    /// # Errors
    /// [`PavoError::UnknownDriver`] / client errors.
    pub fn query(
        &self,
        driver: &str,
        query: &str,
        params: Params,
    ) -> Result<QueryResult, PavoError> {
        self.with_client(driver, |client| client.query(query, params))
    }

    /// `{"op":"submit"}` — submit a form's field values to the driver
    /// that owns `target` (`"<driver>:<view>"`, bare `"<driver>"` =
    /// its first view, same as [`Host::resolve_view`]). `form`
    /// optionally names the form node's `id` when the view carries
    /// several forms.
    ///
    /// On success every cached result for that `driver:view` is
    /// invalidated — submits mutate driver state, so the next
    /// `resolve_view` must re-fetch (a returned
    /// [`SubmitResult::view`] is *not* inserted into the cache; it
    /// belongs to the submitting renderer).
    ///
    /// # Errors
    /// [`PavoError::BadViewRef`] / [`PavoError::UnknownDriver`] /
    /// [`PavoError::UnknownView`] for bad targets; `ok:false` driver
    /// answers (validation failures included — check
    /// [`SubmitResult::errors`] on `Ok`) propagate as
    /// [`PavoError::Driver`].
    pub fn submit(
        &self,
        target: &str,
        form: Option<&str>,
        values: Params,
    ) -> Result<SubmitResult, PavoError> {
        let (driver, view) = self.parse_target(target)?;
        let manifest = self.manifest(&driver)?;
        let view_id = match view {
            Some(v) => v,
            None => manifest
                .views
                .first()
                .map(|v| v.id.clone())
                .ok_or_else(|| PavoError::UnknownView {
                    driver: driver.clone(),
                    view: "<default>".to_string(),
                })?,
        };
        if !manifest.views.iter().any(|v| v.id == view_id) {
            return Err(PavoError::UnknownView {
                driver,
                view: view_id,
            });
        }
        let result = self.with_client(&driver, |client| client.submit(&view_id, form, values))?;
        // Submits mutate state — drop every cached params variant of
        // this view so the next resolve re-fetches.
        self.view_cache
            .invalidate_prefix(&format!("{driver}:{view_id}\u{1f}"));
        Ok(result)
    }

    /// `{"op":"views"}` passthrough — the driver's view summaries
    /// straight from its `serve` (may differ from manifest-derived
    /// [`Host::list_views`] if the driver computes views dynamically).
    ///
    /// # Errors
    /// [`PavoError::UnknownDriver`] / client errors.
    pub fn views(&self, driver: &str) -> Result<Vec<ViewSummary>, PavoError> {
        self.with_client(driver, DriverClient::views)
    }

    /// The merged renderer-plugin registry — every `renderers`
    /// declaration across all discovered drivers' manifests, resolved
    /// to kind → driver bindings (see [`crate::renderers`] for the
    /// conflict policy). Fetches lazy manifests as needed; drivers
    /// that can't produce one contribute no renderers.
    #[must_use]
    pub fn renderers(&self) -> RendererRegistry {
        let kinds: Vec<String> = self.drivers.iter().map(|d| d.kind.clone()).collect();
        let mut pairs: Vec<(String, DriverManifest)> = Vec::new();
        for kind in kinds {
            if let Ok(m) = self.manifest(&kind) {
                pairs.push((kind, m));
            }
        }
        RendererRegistry::from_manifests(pairs.iter().map(|(k, m)| (k.as_str(), m)))
    }

    /// `{"op":"render"}` against an explicit driver — the low-level
    /// renderer-plugin call. Prefer [`Host::render_by_kind`], which
    /// resolves the claiming driver from the registry.
    ///
    /// # Errors
    /// [`PavoError::UnknownDriver`] / client errors.
    pub fn render_node(
        &self,
        driver: &str,
        kind: &str,
        node: &Value,
        data: &Params,
        surface: &str,
        format: Option<&str>,
    ) -> Result<RenderModel, PavoError> {
        self.with_client(driver, |client| {
            client.render(kind, node, data, surface, format)
        })
    }

    /// Route one node to whichever driver claims `kind` on `surface`
    /// — the registry lookup + `render` op in one call. Used by
    /// `pavo serve`'s `/api/render`.
    ///
    /// # Errors
    /// [`PavoError::UnknownDriver`] when no renderer claims the kind
    /// for that surface; client errors propagate.
    pub fn render_by_kind(
        &self,
        kind: &str,
        node: &Value,
        data: &Params,
        surface: &str,
        format: Option<&str>,
    ) -> Result<RenderModel, PavoError> {
        let registry = self.renderers();
        let binding =
            registry
                .for_kind(kind, surface, format)
                .ok_or_else(|| PavoError::UnknownView {
                    driver: "<renderers>".to_string(),
                    view: format!("{kind} on {surface}"),
                })?;
        let driver = binding.driver.clone();
        self.render_node(&driver, kind, node, data, surface, format)
    }

    /// Render every plugin-claimed node in a resolved view for
    /// `surface` — one `render` op per node, keyed by the node's
    /// [`crate::renderers::PluginNode::path`]. Nodes whose plugin
    /// call fails are skipped (the caller falls back to built-in
    /// rendering for that node — a broken plugin degrades one node,
    /// never the whole view).
    ///
    /// `format` is the surface's format hint (CLI `--format`); pass
    /// `None` for the surface default.
    #[must_use]
    pub fn render_view_plugins(
        &self,
        resolved: &ResolvedView,
        surface: &str,
        format: Option<&str>,
    ) -> std::collections::HashMap<String, RenderModel> {
        let registry = self.renderers();
        let nodes = plugin_nodes(&resolved.result.view.root, &registry, surface, format);
        let mut out = std::collections::HashMap::new();
        // Group by driver so each client's requests stay sequential —
        // one driver lock is held for the whole group (the wire is
        // one-in-flight anyway), never two drivers' locks at once.
        let mut by_driver: std::collections::BTreeMap<String, Vec<_>> =
            std::collections::BTreeMap::new();
        for n in nodes {
            by_driver
                .entry(n.binding.driver.clone())
                .or_default()
                .push(n);
        }
        for (driver, group) in by_driver {
            let _ = self.with_client(&driver, |client| {
                for n in group {
                    if let Ok(model) =
                        client.render(&n.kind, &n.node, &resolved.result.data, surface, format)
                    {
                        out.insert(n.path, model);
                    }
                }
                Ok(())
            });
        }
        out
    }

    /// Fan a platform-feed op (`jobs`/`notifications`) out to every
    /// provider driver. A driver is a provider when its manifest's
    /// `contributes` declares the matching slot — the slot's entries
    /// are display metadata; the items themselves come over the wire.
    ///
    /// Providers are queried in host (dependency) order; per-driver
    /// failures — driver `ok:false`, timeout, spawn failure, an
    /// unfetchable manifest — skip that driver rather than failing
    /// the aggregation (one broken feed never takes down the rest).
    fn feed<T>(
        &self,
        op: impl Fn(&mut DriverClient) -> Result<Vec<T>, PavoError>,
        is_provider: impl Fn(&DriverManifest) -> bool,
    ) -> Vec<FeedEntry<T>> {
        let mut out = Vec::new();
        for info in &self.drivers {
            let Ok(manifest) = self.manifest(&info.kind) else {
                continue;
            };
            if !is_provider(&manifest) {
                continue;
            }
            let Ok(items) = self.with_client(&info.kind, |client| op(client)) else {
                continue;
            };
            for item in items {
                out.push(FeedEntry {
                    driver: info.kind.clone(),
                    driver_title: manifest.title.clone(),
                    item,
                });
            }
        }
        out
    }

    /// `{"op":"jobs"}` across every job-feed provider — each
    /// [`JobItem`] tagged with its source driver via [`FeedEntry`].
    /// Per-driver failures are skipped, never fatal.
    #[must_use]
    pub fn jobs(&self) -> Vec<FeedEntry<JobItem>> {
        self.feed(DriverClient::jobs, |m| !m.contributes.jobs.is_empty())
    }

    /// `{"op":"notifications"}` across every notification provider —
    /// each [`NotificationItem`] tagged with its source driver.
    /// Per-driver failures are skipped, never fatal.
    #[must_use]
    pub fn notifications(&self) -> Vec<FeedEntry<NotificationItem>> {
        self.feed(DriverClient::notifications, |m| {
            !m.contributes.notifications.is_empty()
        })
    }

    /// Drop a cached view result — call after mutations that should
    /// refresh the next `resolve_view`.
    pub fn invalidate_view(&self, driver: &str, view: &str, params: &Params) {
        let namespaced = format!("{driver}:{view}");
        let key = format!(
            "{namespaced}\u{1f}{}",
            serde_json::to_string(&Value::Object(params.clone())).unwrap_or_default()
        );
        self.view_cache.invalidate(&key);
    }

    /// Clear the whole view cache.
    pub fn clear_cache(&self) {
        self.view_cache.clear();
    }

    /// Parse `"driver:view"` / `"driver"` into `(driver, Option<view>)`.
    fn parse_target(&self, target: &str) -> Result<(String, Option<String>), PavoError> {
        let target = target.trim();
        if target.is_empty() {
            return Err(PavoError::BadViewRef(target.to_string()));
        }
        match target.split_once(':') {
            None => Ok((target.to_string(), None)),
            Some((driver, view)) => {
                if driver.is_empty() || view.is_empty() || view.contains(':') {
                    return Err(PavoError::BadViewRef(target.to_string()));
                }
                Ok((driver.to_string(), Some(view.to_string())))
            }
        }
    }
}

#[cfg(test)]
#[cfg_attr(coverage, coverage(off))]
mod tests {
    #![allow(clippy::unwrap_used)]

    use super::*;
    use crate::discovery::DiscoveryOptions;
    use std::path::Path;

    fn write_driver(dir: &Path, name: &str, body: &str) -> std::path::PathBuf {
        std::fs::create_dir_all(dir).unwrap();
        let path = dir.join(name);
        std::fs::write(&path, body).unwrap();
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            std::fs::set_permissions(&path, std::fs::Permissions::from_mode(0o755)).unwrap();
        }
        path
    }

    /// Fake pavo driver `fx`: eager manifest + serve wire.
    const FX: &str = r#"#!/bin/sh
if [ "$1" = manifest ]; then
  printf '%s\n' '{"name":"fx","title":"Fx","version":"0.1.0","verbs":["manifest","serve"],"capabilities":["pavo"],"contributes":{"navbar":[{"id":"nav","title":"Nav","route":"/fx/board","priority":5}],"routes":[{"id":"set","title":"Settings","route":"/fx/settings"}]},"views":[{"id":"board","title":"Board","route":"/fx/board","queries":[{"name":"items","op":"query"}],"root":{"kind":"kv","pairs":[]}}]}'
  exit 0
fi
if [ "$1" = serve ]; then
  while IFS= read -r line; do
    case "$line" in
      *'"op":"views"'*) printf '%s\n' '{"ok":true,"result":[{"id":"board","title":"Board","route":"/fx/board","queries":["items"]}]}' ;;
      *'"op":"view"'*) printf '%s\n' '{"ok":true,"result":{"view":{"id":"board","title":"Board","route":"/fx/board","root":{"kind":"kv","pairs":[]}},"data":{"items":[1,2]}}}' ;;
      *'"op":"query"'*) printf '%s\n' '{"ok":true,"result":{"query":"items","data":[1,2]}}' ;;
      *'"op":"submit"'*) printf '%s\n' '{"ok":true,"result":{"message":"saved","data":{"applied":1}}}' ;;
      *'"op":"manifest"'*) printf '%s\n' '{"ok":true,"result":{"name":"fx","title":"Fx","version":"0.1.0","verbs":["serve"],"capabilities":["pavo"],"views":[{"id":"board","title":"Board","route":"/fx/board","root":{"kind":"kv","pairs":[]}}]}}' ;;
    esac
  done
  exit 0
fi
exit 1
"#;

    /// capabilities-only driver `lz` — lazy manifest via serve.
    const LZ: &str = r#"#!/bin/sh
if [ "$1" = capabilities ]; then echo '{"capabilities":["pavo"],"verbs":["serve"]}'; exit 0; fi
if [ "$1" = serve ]; then
  while IFS= read -r line; do
    case "$line" in
      *'"op":"manifest"'*) printf '%s\n' '{"ok":true,"result":{"name":"lz","title":"Lazy","version":"1","verbs":["serve"],"capabilities":["pavo"],"contributes":{"statusbar":[{"id":"st","title":"St","route":"/lz/x"}]},"views":[{"id":"home","title":"Home","route":"/lz","root":{"kind":"kv","pairs":[]}}]}}' ;;
      *'"op":"view"'*) printf '%s\n' '{"ok":true,"result":{"view":{"id":"home","title":"Home","route":"/lz","root":{"kind":"kv","pairs":[]}},"data":{}}}' ;;
    esac
  done
  exit 0
fi
exit 1
"#;

    fn opts(bin: &Path, home: &Path) -> DiscoveryOptions {
        DiscoveryOptions {
            path: Some(bin.as_os_str().to_os_string()),
            tamias_home: Some(home.to_path_buf()),
            ..DiscoveryOptions::default()
        }
    }

    #[test]
    fn end_to_end_discover_chrome_views_resolve() {
        let tmp = tempfile::tempdir().unwrap();
        let bin = tmp.path().join("bin");
        write_driver(&bin, "tamias-driver-fx", FX);
        write_driver(&bin, "tamias-driver-lz", LZ);
        let home = tmp.path().join("home");

        // Short timeout: lz's serve never answers `submit`, and the
        // test shouldn't wait out the default 10s.
        let host = Host::discover_with(opts(&bin, &home)).with_client_config(ClientConfig {
            timeout: Duration::from_secs(2),
            max_restarts: 0,
            ..ClientConfig::default()
        });
        assert_eq!(host.drivers().len(), 2);

        // chrome merges eager + lazy manifests
        let chrome = host.chrome();
        assert_eq!(chrome.navbar.len(), 1);
        assert_eq!(chrome.navbar[0].contribution.id, "fx:nav");
        assert_eq!(chrome.statusbar.len(), 1);
        assert_eq!(chrome.statusbar[0].driver, "lz");
        // chrome.routes = contributes.routes only (view routes are
        // NOT chrome entries) → just fx:set.
        assert_eq!(chrome.routes.len(), 1);
        assert_eq!(chrome.routes[0].contribution.id, "fx:set");

        // views: fx:board + lz:home
        let views = host.list_views();
        assert_eq!(views.len(), 2);
        let fx = views.iter().find(|v| v.driver == "fx").unwrap();
        assert_eq!(fx.namespaced, "fx:board");

        // routes include view routes + slot routes
        let routes = host.list_routes();
        assert!(routes
            .iter()
            .any(|r| r.kind == RouteKind::View && r.route == "/fx/board"));
        assert!(routes.iter().any(|r| matches!(r.kind, RouteKind::Slot(_))));

        // resolve: namespaced, bare-driver default, params pass-through
        let v = host.resolve_view("fx:board", Params::new()).unwrap();
        assert_eq!(v.namespaced, "fx:board");
        assert_eq!(v.result.data["items"], serde_json::json!([1, 2]));
        let v2 = host.resolve_view("lz", Params::new()).unwrap();
        assert_eq!(v2.view, "home");
        // cached: second resolve hits the cache (same result, cheap)
        let v3 = host.resolve_view("fx:board", Params::new()).unwrap();
        assert_eq!(v3.result.data["items"], serde_json::json!([1, 2]));

        // query passthrough
        let q = host.query("fx", "items", Params::new()).unwrap();
        assert_eq!(q.data, serde_json::json!([1, 2]));

        // submit: namespaced + bare-driver targets, then cache drop
        let s = host.submit("fx:board", None, Params::new()).unwrap();
        assert_eq!(s.message.as_deref(), Some("saved"));
        assert!(host.submit("lz", None, Params::new()).is_err()); // lz has no submit op
        assert!(matches!(
            host.submit("fx:nope", None, Params::new()),
            Err(PavoError::UnknownView { .. })
        ));
        // resolve again post-submit — the view cache was invalidated
        // for fx:board (refetch succeeds against the same fixture).
        let v = host.resolve_view("fx:board", Params::new()).unwrap();
        assert_eq!(v.result.data["items"], serde_json::json!([1, 2]));
    }

    #[test]
    fn unknown_driver_and_view_errors() {
        let tmp = tempfile::tempdir().unwrap();
        let bin = tmp.path().join("bin");
        write_driver(&bin, "tamias-driver-fx", FX);
        let host = Host::discover_with(opts(&bin, tmp.path()));

        assert!(matches!(
            host.resolve_view("ghost:board", Params::new()),
            Err(PavoError::UnknownDriver(_))
        ));
        assert!(matches!(
            host.resolve_view("fx:nope", Params::new()),
            Err(PavoError::UnknownView { .. })
        ));
        for bad in ["", ":view", "drv:", "a:b:c"] {
            assert!(
                matches!(
                    host.resolve_view(bad, Params::new()),
                    Err(PavoError::BadViewRef(_))
                        | Err(PavoError::UnknownDriver(_))
                        | Err(PavoError::UnknownView { .. })
                ),
                "target {bad:?} should fail"
            );
        }
        assert!(matches!(
            host.client("ghost"),
            Err(PavoError::UnknownDriver(_))
        ));
        assert!(matches!(
            host.manifest("ghost"),
            Err(PavoError::UnknownDriver(_))
        ));
    }

    /// View driver `dv` with one `graph` node — the plugin target.
    const DV: &str = r#"#!/bin/sh
if [ "$1" = manifest ]; then
  printf '%s\n' '{"name":"dv","title":"Dv","version":"0.1.0","verbs":["manifest","serve"],"capabilities":["pavo"],"contributes":{},"views":[{"id":"g","title":"G","route":"/dv/g","root":{"kind":"graph","nodes":[{"id":"a","label":"A"}],"edges":[]}}]}'
  exit 0
fi
if [ "$1" = serve ]; then
  while IFS= read -r line; do
    case "$line" in
      *'"op":"view"'*) printf '%s\n' '{"ok":true,"result":{"view":{"id":"g","title":"G","route":"/dv/g","root":{"kind":"graph","nodes":[{"id":"a","label":"A"}],"edges":[]}},"data":{}}}' ;;
      *'"op":"views"'*) printf '%s\n' '{"ok":true,"result":[{"id":"g","title":"G","route":"/dv/g"}]}' ;;
    esac
  done
  exit 0
fi
exit 1
"#;

    /// Renderer plugin `vz` — claims `graph` on cli+svg; serve
    /// answers `render` with an svg RenderModel.
    const VZ: &str = r#"#!/bin/sh
if [ "$1" = manifest ]; then
  printf '%s\n' '{"name":"vz","title":"Vz","version":"0.1.0","verbs":["manifest","serve"],"capabilities":["pavo"],"contributes":{},"views":[],"renderers":[{"id":"graph","kinds":["graph"],"surfaces":{"cli":{"formats":["svg"]},"tui":true,"web":{"island":true}}}]}'
  exit 0
fi
if [ "$1" = serve ]; then
  while IFS= read -r line; do
    case "$line" in
      *'"op":"render"'*) printf '%s\n' '{"ok":true,"result":{"type":"svg","markup":"<svg>VZ</svg>"}}' ;;
      *'"op":"views"'*) printf '%s\n' '{"ok":true,"result":[]}' ;;
    esac
  done
  exit 0
fi
exit 1
"#;

    #[test]
    fn renderer_plugins_route_render_ops() {
        let tmp = tempfile::tempdir().unwrap();
        let bin = tmp.path().join("bin");
        write_driver(&bin, "tamias-driver-dv", DV);
        write_driver(&bin, "tamias-driver-vz", VZ);
        let host = Host::discover_with(opts(&bin, tmp.path()));

        // Registry sees the plugin claim (renderer-only driver).
        let reg = host.renderers();
        assert_eq!(reg.len(), 1);
        assert!(reg.handles("graph", "cli", Some("svg")));
        assert!(!reg.handles("graph", "cli", Some("ascii")));
        assert!(reg.handles("graph", "tui", None));

        // render_by_kind routes to vz.
        let node = serde_json::json!({"kind":"graph","nodes":[]});
        let model = host
            .render_by_kind("graph", &node, &Params::new(), "cli", Some("svg"))
            .unwrap();
        assert_eq!(
            model,
            RenderModel::Svg {
                markup: "<svg>VZ</svg>".into()
            }
        );
        // Unclaimed kind+surface → error.
        assert!(host
            .render_by_kind("table", &node, &Params::new(), "cli", Some("svg"))
            .is_err());

        // render_view_plugins finds the root graph node (path "").
        let resolved = host.resolve_view("dv:g", Params::new()).unwrap();
        let renders = host.render_view_plugins(&resolved, "cli", Some("svg"));
        assert_eq!(renders.len(), 1);
        assert!(matches!(renders[""], RenderModel::Svg { .. }));
        // cli+ascii isn't claimed → empty map → built-in fallback.
        assert!(host
            .render_view_plugins(&resolved, "cli", Some("ascii"))
            .is_empty());
        // tui IS claimed but vz answers svg anyway (fixture ignores
        // surface) — still routed.
        assert_eq!(host.render_view_plugins(&resolved, "tui", None).len(), 1);
    }

    #[test]
    fn empty_host() {
        let tmp = tempfile::tempdir().unwrap();
        let bin = tmp.path().join("bin");
        std::fs::create_dir_all(&bin).unwrap();
        let host = Host::discover_with(opts(&bin, tmp.path()));
        assert!(host.drivers().is_empty());
        assert!(host.list_views().is_empty());
        assert!(host.list_routes().is_empty());
        assert!(host.chrome().navbar.is_empty());
    }

    // ---------- deps ordering / quarantine ----------

    /// Minimal eager-manifest driver; `extra_manifest` splices extra
    /// top-level manifest fields (deps, contributes, …) into the JSON.
    fn dep_driver(name: &str, extra_manifest: &str) -> String {
        format!(
            r#"#!/bin/sh
if [ "$1" = manifest ]; then
  printf '%s\n' '{{"name":"{name}","title":"{name}","version":"0.1.0","verbs":["manifest","serve"],"capabilities":["pavo"]{extra_manifest}}}'
  exit 0
fi
if [ "$1" = serve ]; then
  while IFS= read -r line; do
    case "$line" in
      *'"op":"views"'*) printf '%s\n' '{{"ok":true,"result":[]}}' ;;
      *'"op":"manifest"'*) printf '%s\n' '{{"ok":true,"result":{{"name":"{name}","title":"{name}","version":"0.1.0","verbs":["serve"],"capabilities":["pavo"]}}}}' ;;
    esac
  done
  exit 0
fi
exit 1
"#
        )
    }

    #[test]
    fn deps_order_drivers_and_quarantine_missing_requires() {
        let tmp = tempfile::tempdir().unwrap();
        let bin = tmp.path().join("bin");
        // "aaa" requires "zzz" — dep order beats name order.
        write_driver(
            &bin,
            "tamias-driver-aaa",
            &dep_driver(
                "aaa",
                r#","deps":{"requires":["zzz"],"optional":["ghost"]}"#,
            ),
        );
        write_driver(&bin, "tamias-driver-zzz", &dep_driver("zzz", ""));
        // "bad" requires a driver that isn't discovered.
        write_driver(
            &bin,
            "tamias-driver-bad",
            &dep_driver("bad", r#","deps":{"requires":["ghost"]}"#),
        );
        let host = Host::discover_with(opts(&bin, tmp.path()));

        let kinds: Vec<&str> = host.drivers().iter().map(|d| d.kind.as_str()).collect();
        // order() failed wholesale (bad's missing requires), so
        // drivers keep discovery's name-sorted order — dep ordering
        // is all-or-nothing per host.
        assert_eq!(kinds, ["aaa", "bad", "zzz"]);

        // The quarantined driver fails loudly everywhere.
        assert!(matches!(host.manifest("bad"), Err(PavoError::Deps(_))));
        assert!(host.chrome().extra.is_empty()); // bad contributes nothing
        assert!(matches!(
            host.resolve_view("bad", Params::new()),
            Err(PavoError::Deps(_))
        ));
        assert!(matches!(
            host.query("bad", "x", Params::new()),
            Err(PavoError::Deps(_))
        ));
        // Healthy drivers are unaffected.
        assert!(host.manifest("zzz").is_ok());
        assert!(host.manifest("aaa").is_ok());
    }

    #[test]
    fn deps_order_drivers_on_success() {
        let tmp = tempfile::tempdir().unwrap();
        let bin = tmp.path().join("bin");
        // "aaa" requires "zzz" and optionally uses "mmm" — dep order
        // beats name order; optional-present also orders.
        write_driver(
            &bin,
            "tamias-driver-aaa",
            &dep_driver(
                "aaa",
                r#","deps":{"requires":["zzz"],"optional":["mmm","ghost"]}"#,
            ),
        );
        write_driver(&bin, "tamias-driver-mmm", &dep_driver("mmm", ""));
        write_driver(&bin, "tamias-driver-zzz", &dep_driver("zzz", ""));
        let host = Host::discover_with(opts(&bin, tmp.path()));
        let kinds: Vec<&str> = host.drivers().iter().map(|d| d.kind.as_str()).collect();
        // zzz (required) and mmm (optional, present) both precede aaa;
        // between the two deps, name order: mmm < zzz.
        assert_eq!(kinds, ["mmm", "zzz", "aaa"]);
    }

    #[test]
    fn dep_cycles_quarantine_all_members() {
        let tmp = tempfile::tempdir().unwrap();
        let bin = tmp.path().join("bin");
        write_driver(
            &bin,
            "tamias-driver-c1",
            &dep_driver("c1", r#","deps":{"requires":["c2"]}"#),
        );
        write_driver(
            &bin,
            "tamias-driver-c2",
            &dep_driver("c2", r#","deps":{"requires":["c1"]}"#),
        );
        write_driver(&bin, "tamias-driver-ok", &dep_driver("ok", ""));
        let host = Host::discover_with(opts(&bin, tmp.path()));

        for kind in ["c1", "c2"] {
            match host.manifest(kind) {
                Err(PavoError::Deps(e)) => {
                    assert!(e.to_string().contains("cycle"), "{e}");
                }
                other => panic!("expected Deps error for {kind}, got {other:?}"),
            }
        }
        assert!(host.manifest("ok").is_ok());
    }

    // ---------- cross-driver query bindings ----------

    /// View driver `dvq`: one view whose queries bind locally and to
    /// three foreign drivers — `cfg` (answers), `bad` (ok:false),
    /// `ghost` (undiscovered).
    const DVQ: &str = r#"#!/bin/sh
if [ "$1" = manifest ]; then
  printf '%s\n' '{"name":"dvq","title":"Dvq","version":"0.1.0","verbs":["manifest","serve"],"capabilities":["pavo"],"contributes":{},"views":[{"id":"g","title":"G","route":"/dvq/g","queries":[{"name":"local","op":"query"},{"name":"ledger","op":"query","driver":"cfg","params":{"query":"status"}},{"name":"broken","op":"query","driver":"bad"},{"name":"ghost","op":"query","driver":"ghost"}],"root":{"kind":"kv","pairs":[]}}]}'
  exit 0
fi
if [ "$1" = serve ]; then
  while IFS= read -r line; do
    case "$line" in
      *'"op":"views"'*) printf '%s\n' '{"ok":true,"result":[{"id":"g","title":"G","route":"/dvq/g","queries":["local","ledger","broken","ghost"]}]}' ;;
      *'"op":"view"'*) printf '%s\n' '{"ok":true,"result":{"view":{"id":"g","title":"G","route":"/dvq/g","queries":[{"name":"local","op":"query"},{"name":"ledger","op":"query","driver":"cfg","params":{"query":"status"}},{"name":"broken","op":"query","driver":"bad"},{"name":"ghost","op":"query","driver":"ghost"}],"root":{"kind":"kv","pairs":[]}},"data":{"local":[1]}}}' ;;
      *'"op":"query"'*) printf '%s\n' '{"ok":true,"result":{"query":"local","data":[9]}}' ;;
    esac
  done
  exit 0
fi
exit 1
"#;

    /// Foreign driver `cfg` — answers `query` ops with a payload.
    const CFG: &str = r#"#!/bin/sh
if [ "$1" = manifest ]; then
  printf '%s\n' '{"name":"cfg","title":"Cfg","version":"0.1.0","verbs":["manifest","serve"],"capabilities":["pavo"],"contributes":{},"views":[]}'
  exit 0
fi
if [ "$1" = serve ]; then
  while IFS= read -r line; do
    case "$line" in
      *'"op":"query"'*) printf '%s\n' '{"ok":true,"result":{"query":"status","data":{"ledger":"L1"}}}' ;;
      *'"op":"views"'*) printf '%s\n' '{"ok":true,"result":[]}' ;;
    esac
  done
  exit 0
fi
exit 1
"#;

    /// Foreign driver `bad` — every query answers ok:false.
    const BADQ: &str = r#"#!/bin/sh
if [ "$1" = manifest ]; then
  printf '%s\n' '{"name":"bad","title":"Bad","version":"0.1.0","verbs":["manifest","serve"],"capabilities":["pavo"],"contributes":{},"views":[]}'
  exit 0
fi
if [ "$1" = serve ]; then
  while IFS= read -r line; do
    case "$line" in
      *'"op":"query"'*) printf '%s\n' '{"ok":false,"error":"query unsupported"}' ;;
      *'"op":"views"'*) printf '%s\n' '{"ok":true,"result":[]}' ;;
    esac
  done
  exit 0
fi
exit 1
"#;

    #[test]
    fn cross_driver_query_bindings_resolve_and_degrade() {
        let tmp = tempfile::tempdir().unwrap();
        let bin = tmp.path().join("bin");
        write_driver(&bin, "tamias-driver-dvq", DVQ);
        write_driver(&bin, "tamias-driver-cfg", CFG);
        write_driver(&bin, "tamias-driver-bad", BADQ);
        let host = Host::discover_with(opts(&bin, tmp.path()));

        let resolved = host.resolve_view("dvq:g", Params::new()).unwrap();
        let data = &resolved.result.data;
        // Local binding untouched (driver answered it).
        assert_eq!(data["local"], serde_json::json!([1]));
        // Foreign binding resolved via cfg's serve — the QueryResult
        // envelope unwraps to its `data` payload.
        assert_eq!(data["ledger"], serde_json::json!({"ledger": "L1"}));
        // ok:false foreign driver → error payload, not a failed view.
        assert!(data["broken"]["error"]
            .as_str()
            .unwrap()
            .contains("query unsupported"));
        // Undiscovered foreign driver → error payload too.
        assert!(data["ghost"]["error"].as_str().is_some());
    }

    // ---------- platform feeds (jobs / notifications) ----------

    /// Feed provider `feed`: declares both slots, answers both ops.
    const FEED: &str = r#"#!/bin/sh
if [ "$1" = manifest ]; then
  printf '%s\n' '{"name":"feed","title":"Feed","version":"0.1.0","verbs":["manifest","serve"],"capabilities":["pavo"],"contributes":{"jobs":[{"id":"j","title":"Jobs","route":"/feed/jobs"}],"notifications":[{"id":"n","title":"Notes","route":"/feed/n"}]},"views":[]}'
  exit 0
fi
if [ "$1" = serve ]; then
  while IFS= read -r line; do
    case "$line" in
      *'"op":"jobs"'*) printf '%s\n' '{"ok":true,"result":[{"id":"j1","title":"Build","status":"running"}]}' ;;
      *'"op":"notifications"'*) printf '%s\n' '{"ok":true,"result":[{"id":"n1","title":"Hi","severity":"info"}]}' ;;
      *'"op":"views"'*) printf '%s\n' '{"ok":true,"result":[]}' ;;
    esac
  done
  exit 0
fi
exit 1
"#;

    /// Feed provider `feedbad`: declares `jobs` but answers ok:false.
    const FEEDBAD: &str = r#"#!/bin/sh
if [ "$1" = manifest ]; then
  printf '%s\n' '{"name":"feedbad","title":"FeedBad","version":"0.1.0","verbs":["manifest","serve"],"capabilities":["pavo"],"contributes":{"jobs":[{"id":"j","title":"Jobs","route":"/fb/jobs"}]},"views":[]}'
  exit 0
fi
if [ "$1" = serve ]; then
  while IFS= read -r line; do
    case "$line" in
      *'"op":"jobs"'*) printf '%s\n' '{"ok":false,"error":"no jobs"}' ;;
      *'"op":"views"'*) printf '%s\n' '{"ok":true,"result":[]}' ;;
    esac
  done
  exit 0
fi
exit 1
"#;

    #[test]
    fn jobs_and_notifications_fan_out_and_skip_failures() {
        let tmp = tempfile::tempdir().unwrap();
        let bin = tmp.path().join("bin");
        write_driver(&bin, "tamias-driver-feed", FEED);
        write_driver(&bin, "tamias-driver-feedbad", FEEDBAD);
        let host = Host::discover_with(opts(&bin, tmp.path()));

        // feedbad's ok:false is skipped; only feed's items arrive,
        // tagged with driver attribution.
        let jobs = host.jobs();
        assert_eq!(jobs.len(), 1);
        assert_eq!(jobs[0].driver, "feed");
        assert_eq!(jobs[0].driver_title, "Feed");
        assert_eq!(jobs[0].item.id, "j1");
        assert_eq!(jobs[0].item.status, "running");

        // feedbad declares no notifications slot → not queried;
        // feed's single item comes through.
        let notes = host.notifications();
        assert_eq!(notes.len(), 1);
        assert_eq!(notes[0].driver, "feed");
        assert_eq!(notes[0].item.id, "n1");
        assert_eq!(notes[0].item.severity.as_deref(), Some("info"));

        // The chrome merge sees the provider slots too.
        let chrome = host.chrome();
        assert_eq!(chrome.jobs.len(), 2); // feed:j + feedbad:j
        assert_eq!(chrome.notifications.len(), 1);
    }
}
