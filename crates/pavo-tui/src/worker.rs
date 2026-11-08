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
                    let result = host
                        .resolve_view(&target, Params::new())
                        .map(Box::new)
                        .map_err(|e| e.to_string());
                    let _ = ev_tx.send(Ev::View { target, result });
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
    let mut host = Host::discover();
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
