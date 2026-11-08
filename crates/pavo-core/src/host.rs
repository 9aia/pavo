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
//! Errors are typed: [`PavoError::UnknownDriver`] /
//! [`PavoError::UnknownView`] / [`PavoError::BadViewRef`] for bad
//! targets, driver/transport errors propagated from the client.

use std::collections::HashMap;
use std::time::Duration;

use pavo_protocol::wire::{QueryResult, ViewResult, ViewSummary};
use pavo_protocol::DriverManifest;
use serde_json::Value;

use crate::cache::TtlCache;
use crate::client::{ClientConfig, DriverClient};
use crate::discovery::{self, DiscoveryOptions, DriverInfo};
use crate::error::PavoError;
use crate::merge::{merge_chrome, Chrome};
use crate::Params;

/// Default view-result cache TTL.
const DEFAULT_VIEW_CACHE_TTL: Duration = Duration::from_secs(10);

/// The pavo host — discovered drivers, lazy serve clients, merged
/// chrome, view cache.
pub struct Host {
    drivers: Vec<DriverInfo>,
    clients: HashMap<String, DriverClient>,
    client_config: ClientConfig,
    view_cache: TtlCache<ViewResult>,
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
            clients: HashMap::new(),
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
    /// on the [`DriverInfo`].
    ///
    /// # Errors
    /// [`PavoError::UnknownDriver`] for undiscovered kinds; client
    /// errors propagate.
    pub fn manifest(&mut self, kind: &str) -> Result<DriverManifest, PavoError> {
        let index = self
            .drivers
            .iter()
            .position(|d| d.kind == kind)
            .ok_or_else(|| PavoError::UnknownDriver(kind.to_string()))?;
        if self.drivers[index].manifest.is_none() {
            let manifest = self.client(kind)?.manifest()?;
            self.drivers[index].manifest = Some(manifest);
        }
        self.drivers[index]
            .manifest
            .clone()
            .ok_or_else(|| PavoError::UnknownDriver(kind.to_string()))
    }

    /// The live `serve` client for `kind`, spawning on first use.
    ///
    /// # Errors
    /// [`PavoError::UnknownDriver`] / [`PavoError::MissingDriver`] /
    /// spawn errors.
    pub fn client(&mut self, kind: &str) -> Result<&mut DriverClient, PavoError> {
        if !self.clients.contains_key(kind) {
            let info = self
                .drivers
                .iter()
                .find(|d| d.kind == kind)
                .ok_or_else(|| PavoError::UnknownDriver(kind.to_string()))?;
            let client = DriverClient::spawn(info.path.clone(), self.client_config.clone())?;
            self.clients.insert(kind.to_string(), client);
        }
        self.clients
            .get_mut(kind)
            .ok_or_else(|| PavoError::UnknownDriver(kind.to_string()))
    }

    /// Merge every driver's `contributes` into one [`Chrome`].
    /// Drivers whose lazy manifest can't be fetched are skipped —
    /// their slots contribute nothing rather than failing the whole
    /// chrome.
    #[must_use]
    pub fn chrome(&mut self) -> Chrome {
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
    pub fn list_views(&mut self) -> Vec<ViewEntry> {
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
    pub fn list_routes(&mut self) -> Vec<RouteEntry> {
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
    pub fn resolve_view(
        &mut self,
        target: &str,
        params: Params,
    ) -> Result<ResolvedView, PavoError> {
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

        let result = self.client(&driver)?.view(&view_id, params)?;
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
        &mut self,
        driver: &str,
        query: &str,
        params: Params,
    ) -> Result<QueryResult, PavoError> {
        self.client(driver)?.query(query, params)
    }

    /// `{"op":"views"}` passthrough — the driver's view summaries
    /// straight from its `serve` (may differ from manifest-derived
    /// [`Host::list_views`] if the driver computes views dynamically).
    ///
    /// # Errors
    /// [`PavoError::UnknownDriver`] / client errors.
    pub fn views(&mut self, driver: &str) -> Result<Vec<ViewSummary>, PavoError> {
        self.client(driver)?.views()
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

        let mut host = Host::discover_with(opts(&bin, &home));
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
    }

    #[test]
    fn unknown_driver_and_view_errors() {
        let tmp = tempfile::tempdir().unwrap();
        let bin = tmp.path().join("bin");
        write_driver(&bin, "tamias-driver-fx", FX);
        let mut host = Host::discover_with(opts(&bin, tmp.path()));

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

    #[test]
    fn empty_host() {
        let tmp = tempfile::tempdir().unwrap();
        let bin = tmp.path().join("bin");
        std::fs::create_dir_all(&bin).unwrap();
        let mut host = Host::discover_with(opts(&bin, tmp.path()));
        assert!(host.drivers().is_empty());
        assert!(host.list_views().is_empty());
        assert!(host.list_routes().is_empty());
        assert!(host.chrome().navbar.is_empty());
    }
}
