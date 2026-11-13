//! The serve client — one spawned `tamias-driver-<type> serve` process
//! per driver, speaking the pavo ndjson wire: one JSON
//! [`pavo_protocol::Request`] per line on stdin, one
//! [`pavo_protocol::Response`] per line on stdout.
//!
//! Transport shape mirrors `tamias-serve`'s loop (and every sibling
//! `serve`): requests `{"op":"…", …}` → `{"ok":true,"result":…}` /
//! `{"ok":false,"error":"…"}`.
//!
//! Reliability policy:
//!
//! - **Timeout** — every request waits at most
//!   [`ClientConfig::timeout`] (default 10s) for its response line.
//! - **Restart + retry** — transport failures only (io error, EOF,
//!   timeout, malformed response) kill the child and retry the request
//!   once on a fresh process, up to [`ClientConfig::max_restarts`]
//!   extra attempts. A driver that *answers* `ok:false`, or returns a
//!   `result` that doesn't deserialize, is not retried — it's alive
//!   and complaining.
//! - **Clean shutdown** — [`DriverClient`]'s `Drop` kills and reaps
//!   the child; stderr is drained on a thread (never blocks the
//!   driver) with a bounded tail kept for diagnostics.
//!
//! Sync only — a stdout reader thread feeds an `mpsc` channel so
//! `recv_timeout` gives us the request timeout without an async
//! runtime. [`DriverClient`] is `!Sync`-ish by design: requests are
//! `&mut self`, one in flight at a time (the wire is request/response
//! ordered anyway).

use std::collections::VecDeque;
use std::io::{BufRead, BufReader, Write};
use std::path::PathBuf;
use std::process::{Child, ChildStdin, ChildStdout, Command, Stdio};
use std::sync::mpsc::{channel, Receiver};
use std::sync::{Arc, Mutex};
use std::thread;
use std::time::Duration;

use pavo_protocol::platform::{JobItem, NotificationItem};
use pavo_protocol::wire::{ActionResult, QueryResult, SubmitResult, ViewResult, ViewSummary};
use pavo_protocol::{DriverManifest, RenderModel, Request, Response};
use serde::de::DeserializeOwned;

use crate::error::PavoError;
use crate::Params;

/// Tunables for [`DriverClient`].
#[derive(Clone, Debug)]
pub struct ClientConfig {
    /// Per-request response timeout. Default 10s.
    pub timeout: Duration,
    /// Extra attempts after a transport failure — each restarts the
    /// driver process. Default 1 ("one restart + retry").
    pub max_restarts: u32,
    /// How many stderr lines to retain for diagnostics
    /// ([`DriverClient::stderr_tail`]). Default 20.
    pub stderr_tail: usize,
}

impl Default for ClientConfig {
    fn default() -> Self {
        Self {
            timeout: Duration::from_secs(10),
            max_restarts: 1,
            stderr_tail: 20,
        }
    }
}

/// A live `serve` session: child + stdin + the stdout line channel.
struct Session {
    child: Child,
    stdin: ChildStdin,
    rx: Receiver<String>,
    stderr: Arc<Mutex<VecDeque<String>>>,
}

/// Client for one driver's `serve` verb.
///
/// Spawn with [`DriverClient::spawn`]; issue typed requests with the
/// `manifest`/`views`/`view`/`query` helpers or the generic
/// [`DriverClient::request`]. Dropping kills the child.
pub struct DriverClient {
    /// Executable path.
    driver: PathBuf,
    /// Human label for errors — the file name.
    label: String,
    config: ClientConfig,
    session: Option<Session>,
}

impl DriverClient {
    /// Spawn `<driver> serve` with piped stdio.
    ///
    /// # Errors
    /// [`PavoError::Io`] if the process can't be spawned;
    /// [`PavoError::MissingDriver`] is reserved for callers that check
    /// existence first — a missing file surfaces as `Io` here.
    pub fn spawn(driver: PathBuf, config: ClientConfig) -> Result<Self, PavoError> {
        let label = driver
            .file_name()
            .and_then(|n| n.to_str())
            .unwrap_or("driver")
            .to_string();
        let mut client = Self {
            driver,
            label,
            config,
            session: None,
        };
        client.ensure_session()?;
        Ok(client)
    }

    /// The driver executable path.
    #[must_use]
    pub fn path(&self) -> &std::path::Path {
        &self.driver
    }

    /// The driver's file-name label.
    #[must_use]
    pub fn label(&self) -> &str {
        &self.label
    }

    /// Recent stderr lines (oldest → newest), for post-mortem
    /// diagnostics after [`PavoError::Exited`]/timeouts.
    #[must_use]
    pub fn stderr_tail(&self) -> Vec<String> {
        self.session
            .as_ref()
            .and_then(|s| s.stderr.lock().ok())
            .map(|d| d.iter().cloned().collect())
            .unwrap_or_default()
    }

    /// `{"op":"manifest"}` → the driver's [`DriverManifest`].
    ///
    /// # Errors
    /// Transport/protocol failures per [`PavoError`]; `ok:false`
    /// surfaces as [`PavoError::Driver`].
    pub fn manifest(&mut self) -> Result<DriverManifest, PavoError> {
        self.request(&Request::Manifest)
    }

    /// `{"op":"views"}` → view summaries (no trees).
    ///
    /// # Errors
    /// As [`DriverClient::manifest`].
    pub fn views(&mut self) -> Result<Vec<ViewSummary>, PavoError> {
        self.request(&Request::Views)
    }

    /// `{"op":"view","view":…,"params":…}` → the resolved
    /// [`ViewResult`] (tree + query data).
    ///
    /// # Errors
    /// As [`DriverClient::manifest`].
    pub fn view(&mut self, view: &str, params: Params) -> Result<ViewResult, PavoError> {
        self.request(&Request::View {
            view: view.to_string(),
            params,
        })
    }

    /// `{"op":"query","query":…,"params":…}` → one query payload.
    ///
    /// # Errors
    /// As [`DriverClient::manifest`].
    pub fn query(&mut self, query: &str, params: Params) -> Result<QueryResult, PavoError> {
        self.request(&Request::Query {
            query: query.to_string(),
            params,
        })
    }

    /// `{"op":"render","kind":…,"node":…,"data":…,"surface":…,
    /// "format":…}` → a [`RenderModel`] — the renderer-plugin verb.
    /// Only meaningful for drivers whose manifest declares
    /// `renderers`; hosts route by kind via
    /// [`crate::host::Host::render_by_kind`].
    ///
    /// # Errors
    /// As [`DriverClient::manifest`].
    pub fn render(
        &mut self,
        kind: &str,
        node: &serde_json::Value,
        data: &Params,
        surface: &str,
        format: Option<&str>,
    ) -> Result<RenderModel, PavoError> {
        self.request(&Request::Render {
            kind: kind.to_string(),
            node: node.clone(),
            data: data.clone(),
            surface: surface.to_string(),
            format: format.map(str::to_string),
        })
    }

    /// `{"op":"submit","view":…,"form":…,"values":…}` →
    /// [`SubmitResult`] — the form-submission verb. `view` is the
    /// driver's view id; `form` optionally names the form node's
    /// `id` when the view has several; `values` maps field keys to
    /// submitted JSON values.
    ///
    /// # Errors
    /// As [`DriverClient::manifest`]. A driver without form support
    /// answers `ok:false` → [`PavoError::Driver`].
    pub fn submit(
        &mut self,
        view: &str,
        form: Option<&str>,
        values: Params,
    ) -> Result<SubmitResult, PavoError> {
        self.request(&Request::Submit {
            view: view.to_string(),
            form: form.map(str::to_string),
            values,
        })
    }

    /// `{"op":"invoke","view":…,"action":…,"target":…,"params":…}`
    /// → [`ActionResult`] — the action verb. `view` is the optional
    /// view-id context; `action` the action id; `target` opaque JSON
    /// the driver interprets; `params` maps `ParamSchema.name` →
    /// value.
    ///
    /// # Errors
    /// As [`DriverClient::manifest`]. A driver without action
    /// support answers `ok:false` → [`PavoError::Driver`].
    pub fn invoke(
        &mut self,
        view: Option<&str>,
        action: &str,
        target: Option<&serde_json::Value>,
        params: Params,
    ) -> Result<ActionResult, PavoError> {
        self.request(&Request::Invoke {
            view: view.map(str::to_string),
            action: action.to_string(),
            target: target.cloned(),
            params,
        })
    }

    /// `{"op":"jobs"}` → the driver's job/queue feed — meaningful
    /// only for drivers whose manifest declares `contributes.jobs`;
    /// others answer `ok:false`/`[]` per the optional-capability
    /// convention (both surface as `Err`/`Ok(vec![])` here — hosts
    /// treat either as "no feed").
    ///
    /// # Errors
    /// As [`DriverClient::manifest`].
    pub fn jobs(&mut self) -> Result<Vec<JobItem>, PavoError> {
        self.request(&Request::Jobs)
    }

    /// `{"op":"notifications"}` → the driver's notification feed —
    /// same optional-capability convention as [`DriverClient::jobs`]
    /// (`contributes.notifications` marks the provider).
    ///
    /// # Errors
    /// As [`DriverClient::manifest`].
    pub fn notifications(&mut self) -> Result<Vec<NotificationItem>, PavoError> {
        self.request(&Request::Notifications)
    }

    /// Generic request → typed result. The escape hatch for ops the
    /// typed helpers don't cover (and for conformance harnesses).
    ///
    /// # Errors
    /// Transport failures retry per [`ClientConfig::max_restarts`];
    /// `ok:false` and result-decode failures return immediately.
    pub fn request<T: DeserializeOwned>(&mut self, request: &Request) -> Result<T, PavoError> {
        let line = serde_json::to_string(request)?;
        let mut attempt = 0u32;
        loop {
            match self.roundtrip(&line) {
                Ok(value) => {
                    return serde_json::from_value(value).map_err(|e| PavoError::Wire {
                        driver: self.label.clone(),
                        message: format!("result doesn't match requested type: {e}"),
                    });
                }
                Err(e) if e.is_retryable() && attempt < self.config.max_restarts => {
                    attempt += 1;
                    self.kill_session();
                }
                Err(e) => return Err(e),
            }
        }
    }

    /// One request/response round on the live session.
    fn roundtrip(&mut self, line: &str) -> Result<serde_json::Value, PavoError> {
        self.ensure_session()?;
        let Some(session) = self.session.as_mut() else {
            return Err(PavoError::Io(std::io::Error::other(
                "session missing after spawn",
            )));
        };

        writeln!(session.stdin, "{line}").and_then(|()| session.stdin.flush())?;

        let raw = match session.rx.recv_timeout(self.config.timeout) {
            Ok(line) => line,
            Err(std::sync::mpsc::RecvTimeoutError::Timeout) => {
                return Err(PavoError::Timeout {
                    driver: self.label.clone(),
                    timeout: self.config.timeout,
                });
            }
            Err(std::sync::mpsc::RecvTimeoutError::Disconnected) => {
                return Err(self.exited_err("stdout closed"));
            }
        };

        let response: Response = serde_json::from_str(&raw).map_err(|e| PavoError::Wire {
            driver: self.label.clone(),
            message: format!("response line isn't a Response: {e}"),
        })?;
        if response.ok {
            Ok(response.result.unwrap_or(serde_json::Value::Null))
        } else {
            Err(PavoError::Driver {
                driver: self.label.clone(),
                message: response
                    .error
                    .unwrap_or_else(|| "unknown driver error".to_string()),
            })
        }
    }

    /// Spawn (or respawn) the `serve` process.
    fn ensure_session(&mut self) -> Result<(), PavoError> {
        if self.session.is_some() {
            return Ok(());
        }
        if !self.driver.is_file() {
            return Err(PavoError::MissingDriver(self.driver.clone()));
        }
        let mut cmd = Command::new(&self.driver);
        cmd.arg("serve")
            .stdin(Stdio::piped())
            .stdout(Stdio::piped())
            .stderr(Stdio::piped());
        // Freshly-written drivers can hit ETXTBSY (`ExecutableFileBusy`)
        // when exec'd the instant the writer's close lands — retry
        // briefly (same shape as pavo-testkit's `spawn_retry`).
        let mut attempt = 0u32;
        let mut child = loop {
            match cmd.spawn() {
                Ok(child) => break child,
                Err(e) if e.kind() == std::io::ErrorKind::ExecutableFileBusy && attempt < 20 => {
                    attempt += 1;
                    thread::sleep(Duration::from_millis(10));
                }
                Err(e) => return Err(e.into()),
            }
        };
        let taken = (child.stdin.take(), child.stdout.take(), child.stderr.take());
        let (Some(stdin), Some(stdout), Some(stderr)) = taken else {
            // Don't leak a spawned child on a partial-pipe failure.
            let _ = child.kill();
            let _ = child.wait();
            return Err(self.io_err("stdio pipes not captured"));
        };

        let (tx, rx) = channel::<String>();
        thread::spawn(move || pump_lines(stdout, tx));

        let buf = Arc::new(Mutex::new(VecDeque::new()));
        let buf2 = Arc::clone(&buf);
        let keep = self.config.stderr_tail;
        thread::spawn(move || pump_stderr(stderr, buf2, keep));

        self.session = Some(Session {
            child,
            stdin,
            rx,
            stderr: buf,
        });
        Ok(())
    }

    /// Kill and reap the current session, if any.
    fn kill_session(&mut self) {
        if let Some(mut session) = self.session.take() {
            let _ = session.child.kill();
            let _ = session.child.wait();
        }
    }

    fn io_err(&self, message: &str) -> PavoError {
        PavoError::Io(std::io::Error::other(message))
    }

    fn exited_err(&self, detail: &str) -> PavoError {
        let tail = self
            .session
            .as_ref()
            .and_then(|s| s.stderr.lock().ok())
            .map(|d| {
                d.iter()
                    .rev()
                    .take(5)
                    .cloned()
                    .collect::<Vec<_>>()
                    .into_iter()
                    .rev()
                    .collect::<Vec<_>>()
                    .join(" | ")
            })
            .unwrap_or_default();
        PavoError::Exited {
            driver: self.label.clone(),
            detail: if tail.is_empty() {
                detail.to_string()
            } else {
                format!("{detail}; stderr: {tail}")
            },
        }
    }
}

impl Drop for DriverClient {
    fn drop(&mut self) {
        self.kill_session();
    }
}

/// Forward every stdout line into the channel until EOF or the
/// receiver goes away.
fn pump_lines(stdout: ChildStdout, tx: std::sync::mpsc::Sender<String>) {
    for line in BufReader::new(stdout).lines() {
        match line {
            Ok(line) => {
                if tx.send(line).is_err() {
                    break;
                }
            }
            Err(_) => break,
        }
    }
}

/// Drain stderr into a bounded tail buffer — prevents the driver
/// blocking on a full stderr pipe while keeping diagnostics.
fn pump_stderr(stderr: impl std::io::Read, buf: Arc<Mutex<VecDeque<String>>>, keep: usize) {
    for line in BufReader::new(stderr).lines() {
        let Ok(line) = line else { break };
        if let Ok(mut d) = buf.lock() {
            if d.len() >= keep {
                d.pop_front();
            }
            d.push_back(line);
        }
    }
}

#[cfg(test)]
#[cfg_attr(coverage, coverage(off))]
mod tests {
    #![allow(clippy::unwrap_used)]

    use super::*;

    fn write_driver(dir: &std::path::Path, name: &str, body: &str) -> PathBuf {
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

    /// A serve-speaking fake driver: answers manifest/views/view/query,
    /// errors on `"op":"boom"`, sleeps forever on `"op":"slow"`.
    fn serve_script() -> &'static str {
        r#"#!/bin/sh
while IFS= read -r line; do
  case "$line" in
    *'"op":"manifest"'*) printf '%s\n' '{"ok":true,"result":{"name":"fx","title":"Fx","version":"0.1.0","verbs":["serve"],"capabilities":["pavo"],"views":[{"id":"board","title":"Board","route":"/fx/board","root":{"kind":"kv","pairs":[]}}]}}' ;;
    *'"op":"views"'*) printf '%s\n' '{"ok":true,"result":[{"id":"board","title":"Board","route":"/fx/board","queries":[]}]}' ;;
    *'"op":"view"'*) printf '%s\n' '{"ok":true,"result":{"view":{"id":"board","title":"Board","route":"/fx/board","root":{"kind":"kv","pairs":[{"key":"k","value":"v"}]}},"data":{"items":[1]}}}' ;;
    *'"op":"query"'*) printf '%s\n' '{"ok":true,"result":{"query":"items","data":[1,2,3]}}' ;;
    *'"op":"submit"'*) printf '%s\n' '{"ok":true,"result":{"message":"saved","data":{"applied":1}}}' ;;
    *'"op":"invoke"'*) printf '%s\n' '{"ok":true,"result":{"message":"done","data":{"applied":1},"errors":{"note":"required"}}}' ;;
    *'"op":"boom"'*) printf '%s\n' '{"ok":false,"error":"driver says no"}' ;;
    *'"op":"slow"'*) sleep 30 ;;
  esac
done
exit 0
"#
    }

    #[test]
    fn typed_requests_roundtrip() {
        let tmp = tempfile::tempdir().unwrap();
        let driver = write_driver(tmp.path(), "tamias-driver-fx", serve_script());
        let mut client = DriverClient::spawn(driver, ClientConfig::default()).unwrap();

        let m = client.manifest().unwrap();
        assert_eq!(m.name, "fx");
        assert_eq!(client.views().unwrap()[0].id, "board");
        let v = client.view("board", Params::new()).unwrap();
        assert_eq!(v.data["items"], serde_json::json!([1]));
        let q = client.query("items", Params::new()).unwrap();
        assert_eq!(q.data, serde_json::json!([1, 2, 3]));
        let s = client.submit("board", None, Params::new()).unwrap();
        assert_eq!(s.message.as_deref(), Some("saved"));
        // invoke: view context + opaque target + params ride the
        // wire; the ActionResult parses typed (errors map included).
        let target = serde_json::json!({"id": "T-1"});
        let r = client
            .invoke(Some("board"), "task.done", Some(&target), Params::new())
            .unwrap();
        assert_eq!(r.message.as_deref(), Some("done"));
        assert_eq!(r.data.unwrap()["applied"], 1);
        assert_eq!(r.errors["note"], "required");
    }

    #[test]
    fn driver_error_surfaces_verbatim() {
        let tmp = tempfile::tempdir().unwrap();
        let err_script = r#"#!/bin/sh
while IFS= read -r line; do
  case "$line" in
    *'"op":"views"'*) printf '%s\n' '{"ok":false,"error":"views unsupported"}' ;;
  esac
done
"#;
        let driver = write_driver(tmp.path(), "tamias-driver-fx", err_script);
        let cfg = ClientConfig {
            timeout: Duration::from_millis(300),
            max_restarts: 0,
            ..ClientConfig::default()
        };
        let mut client = DriverClient::spawn(driver, cfg).unwrap();
        match client.views() {
            Err(PavoError::Driver { message, .. }) => {
                assert_eq!(message, "views unsupported");
            }
            other => panic!("expected Driver error, got {other:?}"),
        }
        // ok:false is not retried — the fake only ever answers once.
    }

    #[test]
    fn timeout_then_restart_exhaustion() {
        let tmp = tempfile::tempdir().unwrap();
        // serve that never answers "views"
        let driver = write_driver(
            tmp.path(),
            "tamias-driver-fx",
            "#!/bin/sh\nwhile IFS= read -r line; do sleep 30; done\n",
        );
        let cfg = ClientConfig {
            timeout: Duration::from_millis(200),
            max_restarts: 1,
            ..ClientConfig::default()
        };
        let mut client = DriverClient::spawn(driver, cfg).unwrap();
        match client.views() {
            Err(PavoError::Timeout { .. }) => {}
            other => panic!("expected Timeout, got {other:?}"),
        }
    }

    #[test]
    fn restart_recovers_from_process_death() {
        let tmp = tempfile::tempdir().unwrap();
        let count = tmp.path().join("count");
        // First serve invocation exits immediately; the respawn serves.
        let script = format!(
            "#!/bin/sh\nif [ \"$1\" = serve ]; then\n  if [ ! -f {count} ]; then touch {count}; exit 1; fi\nfi\n{body}",
            count = count.display(),
            body = serve_script().trim_start_matches("#!/bin/sh\n")
        );
        let driver = write_driver(tmp.path(), "tamias-driver-fx", &script);
        let mut client = DriverClient::spawn(driver, ClientConfig::default()).unwrap();
        // First request: the initial spawn already died → ensure_session
        // is fine (spawn succeeded), write hits dead stdin or EOF →
        // retry on respawn succeeds.
        let views = client.views().unwrap();
        assert_eq!(views[0].id, "board");
    }

    #[test]
    fn missing_driver_is_error() {
        let cfg = ClientConfig::default();
        let r = DriverClient::spawn(PathBuf::from("/nonexistent/tamias-driver-ghost"), cfg);
        assert!(matches!(r, Err(PavoError::MissingDriver(_))));
    }

    #[test]
    fn drop_kills_child() {
        let tmp = tempfile::tempdir().unwrap();
        let driver = write_driver(tmp.path(), "tamias-driver-fx", serve_script());
        // Drop must not hang; the killed child's wait reaps it.
        let client = DriverClient::spawn(driver, ClientConfig::default()).unwrap();
        drop(client);
    }
}
