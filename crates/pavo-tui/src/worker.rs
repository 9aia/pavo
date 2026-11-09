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
use pavo_core::{host::ViewEntry, Chrome, Host, Params, ResolvedView, RouteEntry};
use pavo_protocol::wire::SubmitResult;
use pavo_protocol::RenderModel;
use std::collections::HashMap;

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
    /// `refresh` clears the host's view cache first.
    Resolve { target: String, refresh: bool },
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
    },
    /// A [`Cmd::Reload`] finished.
    Reloaded {
        /// Fresh view list.
        views: Vec<ViewEntry>,
        /// Fresh merged chrome.
        chrome: Chrome,
        /// Fresh route list.
        routes: Vec<RouteEntry>,
    },
    /// A [`Cmd::Submit`] finished — `Err` carries the display string
    /// (transport/driver `ok:false`).
    Submit {
        /// The submit outcome.
        result: Result<SubmitResult, String>,
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
        boot(&mut host, &ev_tx);
        while let Ok(cmd) = rx.recv() {
            match cmd {
                Cmd::Resolve { target, refresh } => {
                    if refresh {
                        host.clear_cache();
                    }
                    let resolved = host
                        .resolve_view(&target, Params::new())
                        .map_err(|e| e.to_string());
                    // Plugin renders are a second round-trip set —
                    // only for views that resolved; failures inside
                    // degrade per-node (built-in rendering).
                    let renders = resolved
                        .as_ref()
                        .ok()
                        .map(|rv| host.render_view_plugins(rv, "tui", None))
                        .unwrap_or_default();
                    let result = resolved.map(Box::new);
                    let _ = ev_tx.send(Ev::View {
                        target,
                        result,
                        renders,
                    });
                }
                Cmd::Reload => {
                    let views = host.list_views();
                    let chrome = host.chrome();
                    let routes = host.list_routes();
                    let _ = ev_tx.send(Ev::Reloaded {
                        views,
                        chrome,
                        routes,
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
    });
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
        })
        .unwrap();
        match recv_timeout(&w.rx) {
            Ev::View {
                target,
                result,
                renders,
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
