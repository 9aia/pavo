//! The process side of surface drivers — discovery and one-shot wire
//! calls against a spawned `tamias-driver-<type>`. Standalone surface
//! binaries (`tamias-driver-cli`, `tamias-driver-mcp`) are thin
//! `main`s over [`DriverClient`]; an engine driver embedding a
//! surface implements [`Surface`] itself and never spawns.
//!
//! Discovery mirrors the tamias driver contract: `tamias-driver-<name>`
//! on `PATH`, then `<tamias-home>/drivers/` (`$TAMIAS_HOME` →
//! `$XDG_DATA_HOME/tamias` → `~/.local/share/tamias`). A name that is
//! (or resolves to) an existing file — any arg containing a path
//! separator — is used as-is, which keeps tests and developer builds
//! (`target/debug/tamias-driver-kuda`) trivially usable.

use std::io::{BufRead, BufReader, Write};
use std::path::{Path, PathBuf};
use std::process::{Command, Stdio};

use serde_json::{json, Map, Value};

use super::{OpOutcome, OpSpec, OpsManifest, Surface};

/// `<tamias-home>/drivers/` — the second discovery root after `PATH`.
#[must_use]
pub fn drivers_dir() -> PathBuf {
    std::env::var_os("TAMIAS_HOME")
        .map(PathBuf::from)
        .or_else(|| std::env::var_os("XDG_DATA_HOME").map(|d| PathBuf::from(d).join("tamias")))
        .or_else(|| std::env::var_os("HOME").map(|h| PathBuf::from(h).join(".local/share/tamias")))
        .unwrap_or_else(|| PathBuf::from("./.tamias"))
        .join("drivers")
}

/// Resolve `driver` — an engine name (`kuda` → `tamias-driver-kuda`),
/// a full driver name, or a path — to an executable file.
#[must_use]
pub fn find_driver(driver: &str) -> Option<PathBuf> {
    let path = Path::new(driver);
    if path.components().count() > 1 {
        return path.is_file().then(|| path.to_path_buf());
    }
    let name = if let Some(stripped) = driver.strip_prefix("tamias-driver-") {
        stripped.to_string()
    } else {
        driver.to_string()
    };
    let name = format!("tamias-driver-{name}");
    std::env::var_os("PATH")
        .and_then(|p| {
            std::env::split_paths(&p)
                .map(|dir| dir.join(&name))
                .find(|c| c.is_file())
        })
        .or_else(|| {
            let candidate = drivers_dir().join(&name);
            candidate.is_file().then_some(candidate)
        })
}

/// Fetch a driver's ops manifest — the `ops` argv verb, one bounded
/// spawn. `Err` text is caller-renderable (spawn/exit/parse detail).
///
/// # Errors
/// Fails on spawn errors, non-zero exits, or an unparseable manifest.
pub fn ops_manifest(driver: &Path) -> Result<OpsManifest, String> {
    let out = Command::new(driver)
        .arg("ops")
        .output()
        .map_err(|e| format!("{} ops: {e}", driver.display()))?;
    if !out.status.success() {
        return Err(format!(
            "{} ops exited {}: {}",
            driver.display(),
            out.status,
            String::from_utf8_lossy(&out.stderr).trim()
        ));
    }
    let value: Value = serde_json::from_slice(&out.stdout)
        .map_err(|e| format!("{} ops: bad JSON: {e}", driver.display()))?;
    OpsManifest::from_value(&value).map_err(|e| format!("{} ops: {e}", driver.display()))
}

/// A [`Surface`] backed by a spawned `tamias-driver-<type>` binary.
pub struct DriverClient {
    path: PathBuf,
}

impl DriverClient {
    /// Wrap one resolved driver binary — see [`find_driver`].
    #[must_use]
    pub fn new(path: PathBuf) -> Self {
        Self { path }
    }

    /// One request line → one response line: spawn `<driver> serve`,
    /// write the request, read the first response, close stdin so the
    /// loop's EOF rule ends the child. `Err` = transport failure
    /// (spawn/write/read/parse), never a driver `ok:false`.
    fn request(&self, request: &Value) -> Result<Value, String> {
        let mut child = Command::new(&self.path)
            .arg("serve")
            .stdin(Stdio::piped())
            .stdout(Stdio::piped())
            .stderr(Stdio::null())
            .spawn()
            .map_err(|e| format!("{} serve: {e}", self.path.display()))?;
        let result = (|| {
            let mut stdin = child
                .stdin
                .take()
                .ok_or_else(|| "serve: no stdin".to_string())?;
            let stdout = child
                .stdout
                .take()
                .ok_or_else(|| "serve: no stdout".to_string())?;
            let line =
                serde_json::to_string(request).map_err(|e| format!("serve: request JSON: {e}"))?;
            stdin
                .write_all(line.as_bytes())
                .and_then(|()| stdin.write_all(b"\n"))
                .map_err(|e| format!("serve: write: {e}"))?;
            drop(stdin); // EOF lets serve exit after answering
            let mut reply = String::new();
            BufReader::new(stdout)
                .read_line(&mut reply)
                .map_err(|e| format!("serve: read: {e}"))?;
            serde_json::from_str(&reply).map_err(|e| format!("serve: bad reply: {e}"))
        })();
        let _ = child.wait();
        result
    }
}

impl Surface for DriverClient {
    /// `cli.<op>` as a one-shot wire query; the `{code,stdout,stderr}`
    /// convention in the response data becomes the [`OpOutcome`]. A
    /// transport failure reports as `code: 2` on stderr.
    fn op(&mut self, op: &OpSpec, params: &Map<String, Value>) -> OpOutcome {
        let request = json!({
            "op": "query",
            "query": format!("cli.{}", op.name),
            "params": params,
        });
        match self.request(&request) {
            Ok(resp) => OpOutcome::from_response(
                &serde_json::from_value(resp)
                    .unwrap_or_else(|_| pavo_protocol::wire::Response::err("bad response shape")),
            ),
            Err(e) => OpOutcome {
                code: 2,
                stdout: String::new(),
                stderr: e,
            },
        }
    }

    /// `<driver> run <name> <args>` with inherited stdio — unbounded
    /// surfaces stream through.
    fn surface(&mut self, name: &str, args: &[String]) -> i32 {
        match Command::new(&self.path)
            .arg("run")
            .arg(name)
            .args(args)
            .stdin(Stdio::inherit())
            .stdout(Stdio::inherit())
            .stderr(Stdio::inherit())
            .status()
        {
            Ok(status) => status.code().unwrap_or(1),
            Err(e) => {
                eprintln!("{} run {name}: {e}", self.path.display());
                2
            }
        }
    }
}

#[cfg(test)]
#[cfg_attr(coverage, coverage(off))]
mod tests {
    #![allow(clippy::unwrap_used)]

    use super::*;

    /// A path argument bypasses discovery; a bare name looks up
    /// `tamias-driver-<name>` (miss here — nothing is installed).
    #[test]
    fn find_driver_paths_and_misses() {
        assert!(find_driver("definitely-no-such-engine-9aia").is_none());
        let cwd = std::env::current_dir().unwrap();
        let fake = cwd.join("Cargo.toml"); // any existing file
        assert_eq!(
            find_driver(fake.to_str().unwrap()).as_deref(),
            Some(fake.as_path())
        );
        assert!(find_driver("Cargo.toml").is_none()); // no prefix, not on PATH
    }
}
