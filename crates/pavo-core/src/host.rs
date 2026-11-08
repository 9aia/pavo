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
//!   [`ViewResult`] by spawning/using that driver's `serve` client.
//! - [`Host::query`] / [`Host::client`] are the escape hatches for
//!   view-model ops and form submits.
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
use pavo_protocol::{DriverManifest, RenderModel};
use serde_json::Value;

use crate::cache::TtlCache;
use crate::client::{ClientConfig, DriverClient};
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
    #[must_use]
    pub fn from_drivers(drivers: Vec<DriverInfo>) -> Self {
        Self {
            drivers,
            clients: Mutex::new(HashMap::new()),
            manifest_cache: Mutex::new(HashMap::new()),
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

    /// The discovered drivers, sorted by kind.
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
        let client = DriverClient::spawn(info.path.clone(), self.client_config.clone())?;
        let client = Arc::new(Mutex::new(client));
        map.insert(kind.to_string(), Arc::clone(&client));
        Ok(client)
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

        let result = self.with_client(&driver, |client| client.view(&view_id, params))?;
        self.view_cache.insert(key, result.clone());
        Ok(ResolvedView {
            driver,
            view: view_id,
            namespaced,
            result,
        })
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
}
