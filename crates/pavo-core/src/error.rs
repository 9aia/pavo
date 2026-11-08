//! Error type for pavo-core — one enum covering discovery, the serve
//! client, and host-level resolution failures.

use std::path::PathBuf;
use std::time::Duration;

/// Everything pavo-core can fail with.
///
/// Transport-level failures (io, timeout, unexpected exit) are the
/// ones [`client::DriverClient`](crate::client::DriverClient) retries
/// once after a restart; [`PavoError::Driver`] (the driver answered
/// `{"ok":false}`) and [`PavoError::Wire`] (unparseable/mistyped
/// payloads) are not retried — the driver is alive and complaining.
#[derive(Debug, thiserror::Error)]
pub enum PavoError {
    /// Filesystem/process error (spawn, read, write).
    #[error("io error: {0}")]
    Io(#[from] std::io::Error),

    /// JSON (de)serialization failure on our side of the wire.
    #[error("json error: {0}")]
    Json(#[from] serde_json::Error),

    /// The driver answered `{"ok":false,"error":…}` — its own error
    /// text, surfaced verbatim.
    #[error("driver '{driver}' error: {message}")]
    Driver {
        /// Driver executable label (file name).
        driver: String,
        /// The driver's error string.
        message: String,
    },

    /// No response line within the configured timeout.
    #[error("driver '{driver}' timed out after {timeout:?}")]
    Timeout {
        /// Driver executable label (file name).
        driver: String,
        /// The timeout that elapsed.
        timeout: Duration,
    },

    /// The driver's stdout closed (process exited or crashed) while a
    /// request was in flight.
    #[error("driver '{driver}' exited unexpectedly: {detail}")]
    Exited {
        /// Driver executable label (file name).
        driver: String,
        /// What we know — e.g. the stderr tail.
        detail: String,
    },

    /// A response line arrived but wasn't a well-formed
    /// [`pavo_protocol::Response`] or its `result` didn't match the
    /// requested type.
    #[error("malformed wire data from '{driver}': {message}")]
    Wire {
        /// Driver executable label (file name).
        driver: String,
        /// What was wrong.
        message: String,
    },

    /// `resolve_view`/`manifest`/`client` named a driver that wasn't
    /// discovered (or isn't pavo-capable).
    #[error("unknown driver '{0}'")]
    UnknownDriver(String),

    /// The driver exists but its manifest has no view with that id.
    #[error("unknown view '{view}' in driver '{driver}'")]
    UnknownView {
        /// Driver kind (`<type>` of `tamias-driver-<type>`).
        driver: String,
        /// The unresolved view id.
        view: String,
    },

    /// A `<driver>:<view>` target that doesn't parse (empty halves,
    /// stray colons in the driver half, etc.).
    #[error("bad view reference '{0}' — expected <driver>:<view>")]
    BadViewRef(String),

    /// Internal: a driver path was expected to exist but doesn't.
    #[error("driver binary not found: {0}")]
    MissingDriver(PathBuf),
}

impl PavoError {
    /// Whether this is a transport failure worth retrying on a fresh
    /// process. `Driver` (the driver answered `ok:false`) and
    /// `Wire`/`Json` are the driver's own answers — retrying changes
    /// nothing.
    pub(crate) fn is_retryable(&self) -> bool {
        matches!(
            self,
            Self::Io(_) | Self::Timeout { .. } | Self::Exited { .. }
        )
    }
}
