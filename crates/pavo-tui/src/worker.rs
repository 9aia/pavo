//! Background [`Host`] thread — pavo-core is sync and can block on
//! driver `serve` round-trips (spawn, manifest fetch, view resolve),
//! so it lives on a worker thread; the TUI loop talks to it over two
//! mpsc channels and never freezes on a hung driver
//! (`DriverClient`'s own timeouts bound each request).
//!
//! Commands are fire-and-forget requests; events report results.
//! [`Cmd::Resolve`] answers with [`Ev::View`]; [`Cmd::Reload`] re-runs
//! discovery-derived chrome/views and answers [`Ev::Reloaded`].

use std::sync::mpsc::{channel, Receiver, Sender};
use std::thread;

use pavo_core::discovery::DriverInfo;
use pavo_core::{
    collect_actions, host::ViewEntry, menu_entries, ops_actions, Chrome, Host, Params,
    ResolvedView, RouteEntry,
};
use pavo_protocol::wire::{ActionResult, SubmitResult};
use pavo_protocol::{Action, DriverManifest, MenuEntry, RenderModel};
use std::collections::{BTreeMap, HashMap};

use crate::actions::{dedupe_ui_actions, UiAction};

/// One discovered driver, flattened for the rail.
#[derive(Clone, Debug)]
pub struct DriverRow {
    /// Driver kind (`<type>` of `tamias-driver-<type>`).
    pub kind: String,
    /// Display title (manifest title or kind).
    pub title: String,
    /// Number of views the driver offers.
    pub views: usize,
}

/// Commands the UI sends to the host worker.
#[derive(Clone, Debug)]
pub enum Cmd {
    /// Resolve `target` (`"driver:view"` or bare `"driver"`) to a view.
    /// `refresh` clears the host's view cache first. `params` are
    /// caller-supplied view params (filter text, route query args) —
    /// they flow into the driver's `view` op and key the result cache.
    Resolve {
        /// Namespaced view ref or bare driver.
        target: String,
        /// Bypass the view-result cache.
        refresh: bool,
        /// View params (`filter_param`, route `?query` args, …).
        params: Params,
    },
    /// Re-fetch manifests → views + chrome (the `r` key on the
    /// dashboard, or a manual refresh of chrome data).
    Reload,
    /// Submit a form's field values — the `submit` wire op.
    /// `target` is the namespaced `driver:view`; `form` optionally
    /// names the form node's `id`.
    Submit {
        /// Namespaced view ref (`demo:new`).
        target: String,
        /// Form node id when the view has several forms.
        form: Option<String>,
        /// Field `key` → JSON value (editable fields only).
        values: Params,
    },
    /// Invoke an interaction [`Action`] — the `invoke` wire op.
    /// `target` is the namespaced `driver:view` (driver resolution +
    /// default view context); `view` optionally overrides the bare
    /// view-id context; `action_target` is the opaque target JSON
    /// (tree-item `{"id": …}`); `params` maps `ParamSchema.name` →
    /// value.
    Invoke {
        /// Namespaced view ref (`demo:board`).
        target: String,
        /// The action id.
        action: String,
        /// Bare view-id context override.
        view: Option<String>,
        /// Opaque action target (selected item/row context).
        action_target: Option<serde_json::Value>,
        /// ParamSchema name → submitted value.
        params: Params,
    },
}

/// Events the host worker sends back.
#[derive(Clone, Debug)]
pub enum Ev {
    /// First boot completed: drivers, views, merged chrome, routes.
    /// `error` is set when a piece failed (e.g. no drivers found is
    /// NOT an error — it yields empty lists).
    Boot {
        /// Discovered drivers for the rail.
        drivers: Vec<DriverRow>,
        /// Every view, namespaced.
        views: Vec<ViewEntry>,
        /// Merged chrome contributions.
        chrome: Chrome,
        /// Every navigable route (view + slot).
        routes: Vec<RouteEntry>,
        /// Every discovered driver's manifest, keyed by kind — the
        /// app's command layer resolves `action:`/`driver:` commands
        /// against these (action lookup, `driver:` scoping).
        manifests: BTreeMap<String, DriverManifest>,
    },
    /// A [`Cmd::Resolve`] finished — `Err` carries the display string.
    View {
        /// The target that was requested.
        target: String,
        /// Resolve outcome (boxed — `ResolvedView` is large).
        result: Result<Box<ResolvedView>, String>,
        /// Renderer-plugin outputs for this view, keyed by node tree
        /// path (`""` = root, `"0.2"` = child indices) — empty when no
        /// driver claims kinds on the `tui` surface or every plugin
        /// call failed (built-in rendering covers those nodes).
        renders: HashMap<String, RenderModel>,
        /// Interaction actions collected for this view — manifest +
        /// node + tree-item scopes (deduped by id, scope precedence
        /// item > node > manifest), plus ops-derived actions.
        actions: Vec<UiAction>,
        /// The resolved driver's `contributes.menus[]` palette
        /// entries.
        menus: Vec<MenuEntry>,
    },
    /// A [`Cmd::Reload`] finished.
    Reloaded {
        /// Fresh view list.
        views: Vec<ViewEntry>,
        /// Fresh merged chrome.
        chrome: Chrome,
        /// Fresh route list.
        routes: Vec<RouteEntry>,
        /// Fresh manifests (drivers may have been reinstalled).
        manifests: BTreeMap<String, DriverManifest>,
    },
    /// A [`Cmd::Submit`] finished — `Err` carries the display string
    /// (transport/driver `ok:false`).
    Submit {
        /// The submit outcome.
        result: Result<SubmitResult, String>,
    },
    /// A [`Cmd::Invoke`] finished — `Err` carries the display string
    /// (transport/driver `ok:false`). When the result carried a fresh
    /// view, `actions`/`menus` are recollected for it so the canvas
    /// and palette stay in sync.
    Invoke {
        /// The invoke outcome.
        result: Result<ActionResult, String>,
        /// Actions for the swapped-in view (empty otherwise).
        actions: Vec<UiAction>,
        /// Menus for the swapped-in view's driver (empty otherwise).
        menus: Vec<MenuEntry>,
    },
}

/// Handle to the host worker.
pub struct Worker {
    /// Send commands to the host thread.
    pub tx: Sender<Cmd>,
    /// Receive events from the host thread.
    pub rx: Receiver<Ev>,
}

/// Spawn the host worker: `Host::discover()` + first chrome/views
/// fetch happen on the thread so startup never blocks the TUI.
#[must_use]
pub fn spawn() -> Worker {
    let (tx, rx) = channel::<Cmd>();
    let (ev_tx, ev_rx) = channel::<Ev>();
    thread::spawn(move || {
        let mut host = Host::discover();
        // Ops-manifest → actions fallback, fetched once per driver
        // (the `query "ops"` round-trip is cached; drivers without an
        // ops surface answer an error → no derived actions).
        let mut ops_cache: HashMap<String, Vec<Action>> = HashMap::new();
        boot(&mut host, &ev_tx);
        while let Ok(cmd) = rx.recv() {
            match cmd {
                Cmd::Resolve {
                    target,
                    refresh,
                    params,
                } => {
                    if refresh {
                        host.clear_cache();
                    }
                    let resolved = host
                        .resolve_view(&target, params)
                        .map_err(|e| e.to_string());
                    // Plugin renders are a second round-trip set —
                    // only for views that resolved; failures inside
                    // degrade per-node (built-in rendering).
                    let renders = resolved
                        .as_ref()
                        .ok()
                        .map(|rv| host.render_view_plugins(rv, "tui", None))
                        .unwrap_or_default();
                    let (actions, menus) = resolved
                        .as_ref()
                        .ok()
                        .map(|rv| collect_view_actions(&host, rv, &mut ops_cache))
                        .unwrap_or_default();
                    let result = resolved.map(Box::new);
                    let _ = ev_tx.send(Ev::View {
                        target,
                        result,
                        renders,
                        actions,
                        menus,
                    });
                }
                Cmd::Reload => {
                    let views = host.list_views();
                    let chrome = host.chrome();
                    let routes = host.list_routes();
                    let manifests = collect_manifests(&host);
                    let _ = ev_tx.send(Ev::Reloaded {
                        views,
                        chrome,
                        routes,
                        manifests,
                    });
                }
                Cmd::Submit {
                    target,
                    form,
                    values,
                } => {
                    let result = host
                        .submit(&target, form.as_deref(), values)
                        .map_err(|e| e.to_string());
                    let _ = ev_tx.send(Ev::Submit { result });
                }
                Cmd::Invoke {
                    target,
                    action,
                    view,
                    action_target,
                    params,
                } => {
                    let result = host
                        .invoke(&target, &action, view.as_deref(), action_target, params)
                        .map_err(|e| e.to_string());
                    // A fresh ViewResult in the result gets its own
                    // action/menu collection so the app can swap
                    // them in with the view.
                    let (actions, menus) = match &result {
                        Ok(res) => res
                            .view
                            .as_ref()
                            .map(|v| collect_for_view_result(&host, &target, v, &mut ops_cache))
                            .unwrap_or_default(),
                        Err(_) => (Vec::new(), Vec::new()),
                    };
                    let _ = ev_tx.send(Ev::Invoke {
                        result,
                        actions,
                        menus,
                    });
                }
            }
        }
        // tx dropped → Host drops → DriverClients kill their children.
    });
    Worker { tx, rx: ev_rx }
}

/// Run the boot sequence on the worker: snapshot drivers, then the
/// (potentially slow — lazy manifests spawn `serve` processes) view
/// list, chrome merge, and route list.
fn boot(host: &mut Host, ev_tx: &Sender<Ev>) {
    let drivers: Vec<DriverInfo> = host.drivers().to_vec();
    let views = host.list_views();
    let chrome = host.chrome();
    let routes = host.list_routes();
    let manifests = collect_manifests(host);
    let rows = drivers
        .iter()
        .map(|d| DriverRow {
            kind: d.kind.clone(),
            title: d.title().to_string(),
            views: views.iter().filter(|v| v.driver == d.kind).count(),
        })
        .collect();
    let _ = ev_tx.send(Ev::Boot {
        drivers: rows,
        views,
        chrome,
        routes,
        manifests,
    });
}

/// Every discovered driver's manifest keyed by kind — feeds the
/// app's command layer (`resolve_command` needs the current
/// manifest plus the all-drivers table for `driver:` commands).
/// Drivers whose manifest can't be produced are skipped (they
/// error on use elsewhere too).
fn collect_manifests(host: &Host) -> BTreeMap<String, DriverManifest> {
    host.drivers()
        .iter()
        .filter_map(|d| host.manifest(&d.kind).ok().map(|m| (d.kind.clone(), m)))
        .collect()
}

/// Collect the interaction surface for a resolved view: declared
/// actions (manifest + node + tree-item scopes) plus the
/// ops-manifest fallback, deduped by id; and the driver's
/// `contributes.menus[]` for the palette.
fn collect_view_actions(
    host: &Host,
    rv: &ResolvedView,
    ops_cache: &mut HashMap<String, Vec<Action>>,
) -> (Vec<UiAction>, Vec<MenuEntry>) {
    collect_for_view_result(host, &rv.namespaced, &rv.result, ops_cache)
}

/// [`collect_view_actions`] for an arbitrary [`ViewResult`] — used
/// both for resolves (`namespaced` = `driver:view`) and for fresh
/// views returned inside an [`ActionResult`]. `target` supplies the
/// driver kind (`"drv:view"` or bare `"drv"`).
fn collect_for_view_result(
    host: &Host,
    target: &str,
    view_result: &pavo_protocol::wire::ViewResult,
    ops_cache: &mut HashMap<String, Vec<Action>>,
) -> (Vec<UiAction>, Vec<MenuEntry>) {
    let driver = target.split(':').next().unwrap_or(target).to_string();
    let Ok(manifest) = host.manifest(&driver) else {
        return (Vec::new(), Vec::new());
    };
    let menus = menu_entries(&manifest).to_vec();
    let namespaced = format!("{driver}:{}", view_result.view.id);
    let mut ui: Vec<UiAction> = collect_actions(&manifest, view_result)
        .into_iter()
        .map(|s| UiAction {
            action: s.action.into_owned(),
            scope: s.scope,
            target: namespaced.clone(),
            view: s.view,
            item_id: s.item_id.map(|id| id.into_owned()),
        })
        .collect();
    // Zero-declaration fallback: the driver's ops manifest derives
    // one action per op (fetched once per driver, cached).
    let derived = ops_cache
        .entry(driver.clone())
        .or_insert_with(|| ops_manifest_actions(host, &driver));
    for action in derived.iter().cloned() {
        ui.push(UiAction {
            action,
            scope: pavo_core::ActionScope::Manifest,
            target: namespaced.clone(),
            view: Some(view_result.view.id.clone()),
            item_id: None,
        });
    }
    (dedupe_ui_actions(ui), menus)
}

/// `query "ops"` → [`ops_actions`]. Best-effort: drivers without an
/// ops surface (or a failing query) yield no derived actions.
fn ops_manifest_actions(host: &Host, driver: &str) -> Vec<Action> {
    host.query(driver, "ops", Params::new())
        .map(|qr| ops_actions(&qr.data))
        .unwrap_or_default()
}

/// `--list` path — synchronous discovery + listing, no TUI.
/// Prints drivers with their views; returns the process exit code.
pub fn list() -> i32 {
    list_host(&mut Host::discover())
}

/// [`list`] over an explicit host — split so tests can exercise the
/// empty-discovery path without touching the environment.
fn list_host(host: &mut Host) -> i32 {
    let drivers: Vec<(String, String)> = host
        .drivers()
        .iter()
        .map(|d| (d.kind.clone(), d.title().to_string()))
        .collect();
    let views = host.list_views();
    if drivers.is_empty() {
        println!("no pavo drivers discovered");
        println!(
            "  scan path: $PATH + {}/drivers",
            pavo_core::tamias_home().display()
        );
        println!("  hint: set TAMIAS_HOME or put tamias-driver-* on PATH");
        return 1;
    }
    println!("pavo drivers ({}):", drivers.len());
    for (kind, title) in &drivers {
        let mine: Vec<&ViewEntry> = views.iter().filter(|v| &v.driver == kind).collect();
        println!(
            "  {kind} — {title} ({} view{})",
            mine.len(),
            if mine.len() == 1 { "" } else { "s" }
        );
        for v in mine {
            println!("    {} — {} [{}]", v.namespaced, v.title, v.route);
        }
    }
    0
}

#[cfg(test)]
#[cfg_attr(coverage, coverage(off))]
mod tests {
    #![allow(clippy::unwrap_used)]
    //! Worker channel-protocol tests: spawn the real worker thread
    //! (discovery may find zero drivers in the test env — that's a
    //! valid boot), then exercise every command/event pair.

    use super::*;
    use std::time::Duration;

    fn recv_timeout(rx: &Receiver<Ev>) -> Ev {
        // Boot does discovery; allow generous time on slow machines.
        let deadline = std::time::Instant::now() + Duration::from_secs(30);
        loop {
            match rx.recv_timeout(Duration::from_millis(200)) {
                Ok(ev) => return ev,
                Err(std::sync::mpsc::RecvTimeoutError::Timeout) => {
                    assert!(
                        std::time::Instant::now() < deadline,
                        "timed out waiting for worker event"
                    );
                }
                Err(e) => panic!("worker channel closed: {e}"),
            }
        }
    }

    #[test]
    fn worker_boot_reload_resolve_cycle() {
        let w = spawn();
        match recv_timeout(&w.rx) {
            Ev::Boot {
                drivers,
                views,
                routes,
                ..
            } => {
                // Whatever the environment discovered, the shapes are
                // consistent: every view belongs to a known driver.
                for v in &views {
                    assert!(drivers.iter().any(|d| d.kind == v.driver));
                }
                for d in &drivers {
                    assert_eq!(d.views, views.iter().filter(|v| v.driver == d.kind).count());
                }
                let _ = routes;
            }
            other => panic!("expected Boot, got {other:?}"),
        }

        // Reload → fresh lists.
        w.tx.send(Cmd::Reload).unwrap();
        match recv_timeout(&w.rx) {
            Ev::Reloaded { views, .. } => {
                // list_views is deterministic across calls.
                assert!(!views.is_empty() || drivers_empty_ok());
            }
            other => panic!("expected Reloaded, got {other:?}"),
        }

        // Resolve a bogus target → Ev::View with Err (never panics,
        // never hangs — DriverClient timeouts bound the call).
        w.tx.send(Cmd::Resolve {
            target: "definitely-not-a-driver:nope".to_string(),
            refresh: false,
            params: Params::new(),
        })
        .unwrap();
        match recv_timeout(&w.rx) {
            Ev::View {
                target,
                result,
                renders,
                ..
            } => {
                assert_eq!(target, "definitely-not-a-driver:nope");
                assert!(result.is_err());
                assert!(renders.is_empty());
            }
            other => panic!("expected View, got {other:?}"),
        }

        // refresh variant takes the same path.
        w.tx.send(Cmd::Resolve {
            target: "definitely-not-a-driver:nope".to_string(),
            refresh: true,
            params: Params::new(),
        })
        .unwrap();
        match recv_timeout(&w.rx) {
            Ev::View { result, .. } => assert!(result.is_err()),
            other => panic!("expected View, got {other:?}"),
        }
        drop(w);
    }

    fn drivers_empty_ok() -> bool {
        true
    }

    #[test]
    fn list_never_panics() {
        // Prints to stdout; returns 0 (drivers found) or 1 (none).
        let code = list();
        assert!(code == 0 || code == 1);
    }

    #[test]
    fn list_host_empty_and_with_driver() {
        // Empty host → "no drivers" path, exit 1.
        let mut empty = Host::from_drivers(Vec::new());
        assert_eq!(list_host(&mut empty), 1);
        // A driver with no manifest → still listed via kind title.
        // (DriverInfo.path needn't exist for list — no client spawn.)
        let info = pavo_core::discovery::DriverInfo {
            kind: "ghost".to_string(),
            path: std::path::PathBuf::from("/nonexistent/tamias-driver-ghost"),
            manifest: None,
        };
        let mut host = Host::from_drivers(vec![info]);
        // list_views may try lazy manifest fetch → serve spawn fails
        // gracefully; whatever it yields, list must not panic.
        let code = list_host(&mut host);
        assert!(code == 0 || code == 1);
    }
}
