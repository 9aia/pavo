//! Driver discovery — find `tamias-driver-*` executables that speak
//! pavo, on `PATH` and in `<tamias-home>/drivers/`.
//!
//! Scan order mirrors `tamias-driver::discovered_drivers`: every `PATH`
//! dir in order, then the drivers dir; the first executable claiming a
//! `<type>` wins, results sorted by kind for determinism.
//!
//! A candidate is **pavo-capable** when either probe advertises it:
//!
//! 1. `tamias-driver-<type> manifest` exits 0 with a body that parses
//!    as [`pavo_protocol::DriverManifest`] and either lists `"serve"`
//!    in `verbs` or `"pavo"` in `capabilities` (the manifest's own
//!    capability-negotiation rule). The manifest is kept **eagerly** —
//!    pavo can list routes/contributions without spawning `serve`.
//! 2. Failing that, `tamias-driver-<type> capabilities` exits 0 with
//!    JSON whose `capabilities`/`verbs` arrays mention `"pavo"` or
//!    `"serve"`. No manifest is available yet — [`DriverInfo::manifest`]
//!    is `None` and [`Host`](crate::host::Host) fetches one lazily over
//!    the `serve` wire's `manifest` op.
//!
//! Candidates that error, time out ([`DiscoveryOptions::probe_timeout`]
//! per probe), or answer non-pavo JSON are skipped — discovery never
//! fails wholesale because one driver is broken.

use std::ffi::OsString;
use std::io;
use std::path::{Path, PathBuf};
use std::process::{Command, Stdio};
use std::thread::sleep;
use std::time::{Duration, Instant};

use pavo_protocol::DriverManifest;
use serde_json::Value;

use crate::paths;

/// How [`discover_with`] should scan and probe.
///
/// All fields optional — `None` means "use the environment"
/// (`PATH`, `TAMIAS_HOME`/`XDG_DATA_HOME`/`HOME` via
/// [`crate::paths`]). Tests inject tempdirs here instead of mutating
/// process env.
#[derive(Clone, Debug)]
pub struct DiscoveryOptions {
    /// `PATH` value to scan (an [`OsString`], split on the platform
    /// separator). `None` → `std::env::var_os("PATH")`.
    pub path: Option<OsString>,
    /// Tamias home override — drivers are looked up in
    /// `<tamias_home>/drivers`. `None` → [`crate::paths::tamias_home`].
    pub tamias_home: Option<PathBuf>,
    /// Per-probe budget for `manifest`/`capabilities` invocations.
    /// Probes that exceed it are killed and the candidate skipped.
    pub probe_timeout: Duration,
}

impl Default for DiscoveryOptions {
    fn default() -> Self {
        Self {
            path: None,
            tamias_home: None,
            probe_timeout: Duration::from_secs(5),
        }
    }
}

/// A discovered pavo-capable driver.
#[derive(Clone, Debug)]
pub struct DriverInfo {
    /// The `<type>` in `tamias-driver-<type>` — also the driver's
    /// namespace for views (`<kind>:<view>`) and routes.
    pub kind: String,
    /// Path to the executable.
    pub path: PathBuf,
    /// The driver's [`DriverManifest`], fetched eagerly via the
    /// `manifest` verb when it answers one. `None` when only the
    /// `capabilities` probe advertised pavo — fetch lazily through
    /// [`crate::client::DriverClient::manifest`] (the `serve` wire's
    /// `manifest` op).
    pub manifest: Option<DriverManifest>,
}

impl DriverInfo {
    /// Display title — the manifest's `title` when known, else the
    /// kind. For chrome fallbacks before a lazy manifest arrives.
    #[must_use]
    pub fn title(&self) -> &str {
        self.manifest
            .as_ref()
            .map_or(self.kind.as_str(), |m| m.title.as_str())
    }
}

/// Discover all pavo-capable drivers using environment defaults
/// (`PATH` + [`crate::paths::drivers_dir`]).
///
/// Equivalent to `discover_with(DiscoveryOptions::default())`.
#[must_use]
pub fn discover() -> Vec<DriverInfo> {
    discover_with(&DiscoveryOptions::default())
}

/// Discover all pavo-capable drivers per `options`.
///
/// Never fails wholesale: unreadable dirs, unexecutable files, and
/// drivers that don't answer the probes are all skipped.
#[must_use]
pub fn discover_with(options: &DiscoveryOptions) -> Vec<DriverInfo> {
    let candidates = scan_candidates(&scan_dirs(options));
    let mut found: Vec<DriverInfo> = candidates
        .iter()
        .filter_map(|(kind, path)| probe_driver(kind, path, options.probe_timeout))
        .collect();
    found.sort_by(|a, b| a.kind.cmp(&b.kind));
    found
}

/// `(kind, path)` candidates in scan order — PATH dirs first, drivers
/// dir last, first claim on a kind wins.
fn scan_dirs(options: &DiscoveryOptions) -> Vec<PathBuf> {
    let mut dirs: Vec<PathBuf> = options
        .path
        .clone()
        .or_else(|| std::env::var_os("PATH"))
        .map(|p| std::env::split_paths(&p).collect())
        .unwrap_or_default();
    let drivers = match &options.tamias_home {
        Some(home) => home.join("drivers"),
        None => paths::drivers_dir(),
    };
    dirs.push(drivers);
    dirs
}

/// Scan `dirs` for `tamias-driver-*` files, dedup by kind.
fn scan_candidates(dirs: &[PathBuf]) -> Vec<(String, PathBuf)> {
    let mut found: Vec<(String, PathBuf)> = Vec::new();
    for dir in dirs {
        let Ok(read) = std::fs::read_dir(dir) else {
            continue;
        };
        for entry in read.flatten() {
            let path = entry.path();
            let Some(kind) = path
                .file_name()
                .and_then(|n| n.to_str())
                .and_then(|n| n.strip_prefix("tamias-driver-"))
                .filter(|k| !k.is_empty())
            else {
                continue;
            };
            if !path.is_file() || found.iter().any(|(k, _)| k == kind) {
                continue;
            }
            found.push((kind.to_string(), path));
        }
    }
    found
}

/// Probe one candidate: `manifest` first (eager manifest), then
/// `capabilities` (lazy). `None` when neither advertises pavo.
fn probe_driver(kind: &str, path: &Path, timeout: Duration) -> Option<DriverInfo> {
    if let Ok(stdout) = probe(path, "manifest", timeout) {
        if let Ok(manifest) = serde_json::from_slice::<DriverManifest>(&stdout) {
            if manifest_is_pavo(&manifest) {
                return Some(DriverInfo {
                    kind: kind.to_string(),
                    path: path.to_path_buf(),
                    manifest: Some(manifest),
                });
            }
            // A manifest that doesn't advertise pavo is authoritative —
            // don't let a stale capabilities probe override it.
            return None;
        }
        // Unparseable manifest body — fall through to capabilities.
    }
    if let Ok(stdout) = probe(path, "capabilities", timeout) {
        if let Ok(value) = serde_json::from_slice::<Value>(&stdout) {
            if capabilities_is_pavo(&value) {
                return Some(DriverInfo {
                    kind: kind.to_string(),
                    path: path.to_path_buf(),
                    manifest: None,
                });
            }
        }
    }
    None
}

/// Run `tamias-driver-<kind> <verb>` with a timeout; `Ok(stdout)` only
/// when the process exits 0 in time. Anything else → `Err`.
fn probe(path: &Path, verb: &str, timeout: Duration) -> io::Result<Vec<u8>> {
    let mut cmd = Command::new(path);
    cmd.arg(verb)
        .stdin(Stdio::null())
        .stdout(Stdio::piped())
        .stderr(Stdio::null());
    run_with_timeout(&mut cmd, timeout)
}

/// Spawn `cmd`, wait up to `timeout` for exit, return captured stdout
/// on success. On timeout the child is killed and reaped.
fn run_with_timeout(cmd: &mut Command, timeout: Duration) -> io::Result<Vec<u8>> {
    let mut child = cmd.spawn()?;
    let deadline = Instant::now() + timeout;
    loop {
        match child.try_wait()? {
            Some(status) if status.success() => {
                let mut out = Vec::new();
                use std::io::Read;
                if let Some(mut stdout) = child.stdout.take() {
                    let _ = stdout.read_to_end(&mut out);
                }
                return Ok(out);
            }
            Some(_) => {
                let _ = child.wait();
                return Err(io::Error::other("probe exited non-zero"));
            }
            None if Instant::now() >= deadline => {
                let _ = child.kill();
                let _ = child.wait();
                return Err(io::Error::new(io::ErrorKind::TimedOut, "probe timed out"));
            }
            None => sleep(Duration::from_millis(10)),
        }
    }
}

/// Whether a parsed [`DriverManifest`] advertises pavo service —
/// `"serve"` in `verbs` or `"pavo"` in `capabilities`, per the
/// manifest's capability-negotiation doc.
fn manifest_is_pavo(manifest: &DriverManifest) -> bool {
    manifest.verbs.iter().any(|v| v == "serve") || manifest.capabilities.iter().any(|c| c == "pavo")
}

/// Whether a raw `capabilities` JSON body advertises pavo — checks the
/// `capabilities` and `verbs` arrays for `"pavo"`/`"serve"`.
fn capabilities_is_pavo(value: &Value) -> bool {
    ["capabilities", "verbs"].iter().any(|key| {
        value.get(*key).and_then(Value::as_array).is_some_and(|a| {
            a.iter()
                .any(|v| v.as_str().is_some_and(|s| s == "pavo" || s == "serve"))
        })
    })
}

#[cfg(test)]
#[cfg_attr(coverage, coverage(off))]
mod tests {
    #![allow(clippy::unwrap_used)]

    use super::*;

    fn write_driver(dir: &Path, name: &str, body: &str) -> PathBuf {
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

    const PAVO_MANIFEST: &str = r#"{
        "name":"fx","title":"Fx","version":"0.1.0",
        "verbs":["manifest","serve"],"capabilities":["pavo"],
        "views":[{"id":"board","title":"Board","route":"/fx/board",
                  "root":{"kind":"kv","pairs":[]}}]
    }"#;

    fn pavo_script(manifest_body: &str) -> String {
        format!(
            "#!/bin/sh\nif [ \"$1\" = manifest ]; then printf '%s\\n' '{manifest_body}'; exit 0; fi\nif [ \"$1\" = capabilities ]; then echo '{{\"capabilities\":[\"pavo\"],\"verbs\":[\"serve\"]}}'; exit 0; fi\nexit 1\n"
        )
    }

    #[test]
    fn discovers_pavo_driver_via_manifest() {
        let tmp = tempfile::tempdir().unwrap();
        let bin = tmp.path().join("bin");
        write_driver(&bin, "tamias-driver-fx", &pavo_script(PAVO_MANIFEST));
        write_driver(&bin, "tamias-driver-nope", "#!/bin/sh\nexit 1\n");
        write_driver(
            &bin,
            "tamias-driver-tamias",
            "#!/bin/sh\nif [ \"$1\" = capabilities ]; then echo '{\"types\":[\"files\"]}'; exit 0; fi\nexit 1\n",
        );
        let opts = DiscoveryOptions {
            path: Some(bin.clone().into_os_string()),
            tamias_home: Some(tmp.path().join("home")),
            ..DiscoveryOptions::default()
        };
        let found = discover_with(&opts);
        assert_eq!(found.len(), 1);
        assert_eq!(found[0].kind, "fx");
        let m = found[0].manifest.as_ref().unwrap();
        assert_eq!(m.name, "fx");
        assert_eq!(m.views[0].id, "board");
    }

    #[test]
    fn capabilities_only_driver_is_lazy() {
        let tmp = tempfile::tempdir().unwrap();
        let bin = tmp.path().join("bin");
        // manifest verb fails; capabilities advertises pavo
        write_driver(
            &bin,
            "tamias-driver-lazy",
            "#!/bin/sh\nif [ \"$1\" = capabilities ]; then echo '{\"capabilities\":[\"pavo\"]}'; exit 0; fi\nexit 1\n",
        );
        let opts = DiscoveryOptions {
            path: Some(bin.into_os_string()),
            tamias_home: Some(tmp.path().join("home")),
            ..DiscoveryOptions::default()
        };
        let found = discover_with(&opts);
        assert_eq!(found.len(), 1);
        assert_eq!(found[0].kind, "lazy");
        assert!(found[0].manifest.is_none());
        assert_eq!(found[0].title(), "lazy");
    }

    #[test]
    fn drivers_dir_is_scanned_and_path_wins_dedup() {
        let tmp = tempfile::tempdir().unwrap();
        let home = tmp.path().join("home");
        let dd = home.join("drivers");
        write_driver(&dd, "tamias-driver-dup", &pavo_script(PAVO_MANIFEST));
        let bin = tmp.path().join("bin");
        // PATH copy reports a different title — PATH must win.
        write_driver(
            &bin,
            "tamias-driver-dup",
            &pavo_script(r#"{"name":"dup","title":"PathWins","version":"1","verbs":["serve"]}"#),
        );
        let opts = DiscoveryOptions {
            path: Some(bin.into_os_string()),
            tamias_home: Some(home),
            ..DiscoveryOptions::default()
        };
        let found = discover_with(&opts);
        assert_eq!(found.len(), 1);
        assert_eq!(found[0].title(), "PathWins");
    }

    #[test]
    fn slow_driver_is_skipped_by_probe_timeout() {
        let tmp = tempfile::tempdir().unwrap();
        let bin = tmp.path().join("bin");
        write_driver(&bin, "tamias-driver-slow", "#!/bin/sh\nsleep 30\n");
        let opts = DiscoveryOptions {
            path: Some(bin.into_os_string()),
            tamias_home: Some(tmp.path().join("home")),
            probe_timeout: Duration::from_millis(200),
        };
        let found = discover_with(&opts);
        assert!(found.is_empty());
    }

    #[test]
    fn non_pavo_manifest_json_is_skipped() {
        let tmp = tempfile::tempdir().unwrap();
        let bin = tmp.path().join("bin");
        // parses as DriverManifest but doesn't advertise pavo
        write_driver(
            &bin,
            "tamias-driver-plain",
            &pavo_script(r#"{"name":"plain","title":"Plain","version":"1","verbs":["manifest"]}"#),
        );
        let opts = DiscoveryOptions {
            path: Some(bin.into_os_string()),
            tamias_home: Some(tmp.path().join("home")),
            ..DiscoveryOptions::default()
        };
        assert!(discover_with(&opts).is_empty());
    }
}
