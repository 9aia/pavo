//! Raw ndjson wire session — spawn `<driver> serve`, send one JSON
//! line, read one JSON line back, with a per-request timeout.
//!
//! This is the transport [`crate::conformance`] and
//! [`crate::record`] are built on; it's `pub` so tests can speak
//! malformed lines or probe transport failures the typed helpers
//! don't cover. Sync only — a stdout reader thread feeds an `mpsc`
//! channel so `recv_timeout` gives the timeout without an async
//! runtime (same shape as `pavo-core`'s `DriverClient`).

use std::io::{BufRead, BufReader, Write};
use std::path::{Path, PathBuf};
use std::process::{Child, ChildStdin, Command, Stdio};
use std::sync::mpsc::{channel, Receiver};
use std::thread;
use std::time::Duration;

use serde_json::Value;

use pavo_protocol::Response;

/// Errors a [`Session`] can produce. All are transport/protocol-level;
/// a driver answering `ok:false` is *not* an error here — inspect the
/// [`Response`] instead.
#[derive(Debug)]
pub enum WireError {
    /// Spawn/write/read failure.
    Io(std::io::Error),
    /// No response line within the request timeout.
    Timeout {
        /// The timeout that elapsed.
        timeout: Duration,
    },
    /// The driver's stdout closed (process exited or crashed).
    Exited {
        /// What was observed — `"stdout closed"`, an exit status, …
        detail: String,
    },
    /// A response line arrived but isn't a [`Response`] JSON object.
    BadResponse {
        /// The offending line (truncated).
        line: String,
    },
}

impl std::fmt::Display for WireError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::Io(e) => write!(f, "io error: {e}"),
            Self::Timeout { timeout } => write!(f, "timed out after {timeout:?}"),
            Self::Exited { detail } => write!(f, "driver exited: {detail}"),
            Self::BadResponse { line } => {
                let short: String = line.chars().take(120).collect();
                write!(f, "response line isn't a Response: {short}")
            }
        }
    }
}

impl std::error::Error for WireError {}

impl From<std::io::Error> for WireError {
    fn from(e: std::io::Error) -> Self {
        Self::Io(e)
    }
}

/// A live `<driver> serve` session: child + stdin + stdout line
/// channel. Dropping kills the child.
pub struct Session {
    child: Child,
    stdin: ChildStdin,
    rx: Receiver<String>,
    driver: PathBuf,
}

impl Session {
    /// Spawn `<driver> serve` with piped stdio.
    ///
    /// Freshly-written fixture drivers can hit `ETXTBSY`
    /// (`ExecutableFileBusy`) when exec'd in the same instant the
    /// writer's `close` lands — retry briefly rather than fail.
    ///
    /// # Errors
    /// [`WireError::Io`] if the process can't be spawned or its pipes
    /// can't be captured.
    pub fn spawn(driver: &Path) -> Result<Self, WireError> {
        let mut cmd = Command::new(driver);
        cmd.arg("serve")
            .stdin(Stdio::piped())
            .stdout(Stdio::piped())
            .stderr(Stdio::null());
        let mut child = spawn_retry(&mut cmd)?;
        let (Some(stdin), Some(stdout)) = (child.stdin.take(), child.stdout.take()) else {
            let _ = child.kill();
            let _ = child.wait();
            return Err(WireError::Io(std::io::Error::other(
                "stdio pipes not captured",
            )));
        };
        let (tx, rx) = channel::<String>();
        thread::spawn(move || {
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
        });
        Ok(Self {
            child,
            stdin,
            rx,
            driver: driver.to_path_buf(),
        })
    }

    /// The driver executable path.
    #[must_use]
    pub fn driver(&self) -> &Path {
        &self.driver
    }

    /// Whether the child has exited (best-effort `try_wait`).
    #[must_use]
    pub fn has_exited(&mut self) -> bool {
        self.child.try_wait().ok().flatten().is_some()
    }

    /// Send one raw line and return the raw response line.
    ///
    /// # Errors
    /// [`WireError::Io`] on write failure, [`WireError::Timeout`] when
    /// no line arrives in `timeout`, [`WireError::Exited`] when stdout
    /// closes first.
    pub fn request_raw(&mut self, line: &str, timeout: Duration) -> Result<String, WireError> {
        writeln!(self.stdin, "{line}")?;
        self.stdin.flush()?;
        match self.rx.recv_timeout(timeout) {
            Ok(line) => Ok(line),
            Err(std::sync::mpsc::RecvTimeoutError::Timeout) => Err(WireError::Timeout { timeout }),
            Err(std::sync::mpsc::RecvTimeoutError::Disconnected) => Err(WireError::Exited {
                detail: "stdout closed".to_string(),
            }),
        }
    }

    /// Send a JSON request value and parse the response line as a
    /// [`Response`]. `ok:false` is returned as `Ok(Response)` — it's a
    /// wire-level answer, not a transport failure.
    ///
    /// # Errors
    /// Transport failures per [`Session::request_raw`];
    /// [`WireError::BadResponse`] when the line isn't a `Response`.
    pub fn call(&mut self, request: &Value, timeout: Duration) -> Result<Response, WireError> {
        let line = serde_json::to_string(request).map_err(|e| {
            WireError::Io(std::io::Error::other(format!("request serialization: {e}")))
        })?;
        let raw = self.request_raw(&line, timeout)?;
        serde_json::from_str(&raw).map_err(|_| WireError::BadResponse { line: raw })
    }
}

/// `cmd.spawn()` with a short retry loop for `ExecutableFileBusy`
/// (ETXTBSY) — see [`Session::spawn`].
fn spawn_retry(cmd: &mut Command) -> Result<Child, WireError> {
    let mut attempt = 0u32;
    loop {
        match cmd.spawn() {
            Ok(child) => return Ok(child),
            Err(e) if e.kind() == std::io::ErrorKind::ExecutableFileBusy && attempt < 20 => {
                attempt += 1;
                thread::sleep(Duration::from_millis(10));
            }
            Err(e) => return Err(WireError::Io(e)),
        }
    }
}

impl Drop for Session {
    fn drop(&mut self) {
        let _ = self.child.kill();
        let _ = self.child.wait();
    }
}
