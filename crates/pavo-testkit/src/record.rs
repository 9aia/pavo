//! Record/replay — capture a driver's ndjson session to a transcript
//! and replay it deterministically.
//!
//! [`record_session`] speaks a list of requests to a live driver and
//! stores every request/response pair as a [`Transcript`];
//! [`save_transcript`]/[`load_transcript`] persist it as JSON.
//! [`replay_driver`] turns a transcript into a
//! [`FixtureDriver`][crate::fixture::FixtureDriver] that answers each
//! request from the recording (matched on canonical JSON) — renderer
//! and host tests run against it without spawning the real driver, and
//! get byte-identical responses every run.

use std::io;
use std::path::Path;
use std::time::Duration;

use pavo_protocol::Response;
use serde::{Deserialize, Serialize};
use serde_json::Value;

use crate::fixture::FixtureDriverBuilder;
use crate::wire::{Session, WireError};

/// One recorded round-trip: the request JSON and the full response
/// JSON (`{"ok":…}` envelope included, so `ok:false` replays too).
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct Entry {
    /// The request object as sent (`{"op":…}`).
    pub request: Value,
    /// The response object as received (`{"ok":…,"result"|"error":…}`).
    pub response: Value,
}

/// A recorded ndjson session — an ordered list of round-trips.
#[derive(Clone, Debug, Default, PartialEq, Serialize, Deserialize)]
pub struct Transcript {
    /// The recorded exchanges, in order.
    pub entries: Vec<Entry>,
}

impl Transcript {
    /// Number of recorded exchanges.
    #[must_use]
    pub fn len(&self) -> usize {
        self.entries.len()
    }

    /// Whether the transcript is empty.
    #[must_use]
    pub fn is_empty(&self) -> bool {
        self.entries.is_empty()
    }
}

/// Errors [`record_session`] can produce.
#[derive(Debug, thiserror::Error)]
pub enum RecordError {
    /// The driver couldn't be spawned or a request failed at the
    /// transport level.
    #[error("wire: {0}")]
    Wire(#[from] WireError),
    /// A request or response line wasn't valid JSON.
    #[error("invalid JSON on the wire: {0}")]
    Json(#[from] serde_json::Error),
}

/// Speak `requests` to `<driver> serve` and record every
/// request/response pair into a [`Transcript`]. Stops at the first
/// transport failure — the partial transcript is lost (the error
/// carries the cause).
///
/// # Errors
/// [`RecordError::Wire`] on spawn/transport failure;
/// [`RecordError::Json`] if a line isn't JSON.
pub fn record_session(
    driver: &Path,
    requests: &[Value],
    timeout: Duration,
) -> Result<Transcript, RecordError> {
    let mut session = Session::spawn(driver)?;
    let mut entries = Vec::with_capacity(requests.len());
    for request in requests {
        let line = serde_json::to_string(request)?;
        let raw = session.request_raw(&line, timeout)?;
        let response: Value = serde_json::from_str(&raw)?;
        // Sanity: the recorded response should be a Response envelope.
        let _ = serde_json::from_value::<Response>(response.clone())?;
        entries.push(Entry {
            request: request.clone(),
            response,
        });
    }
    Ok(Transcript { entries })
}

/// Persist a transcript as pretty JSON.
///
/// # Errors
/// [`io::Error`] on write failure, [`serde_json::Error`] on
/// serialization (shouldn't happen for well-formed transcripts).
pub fn save_transcript(transcript: &Transcript, path: &Path) -> Result<(), RecordError> {
    let text = serde_json::to_string_pretty(transcript)?;
    std::fs::write(path, text)?;
    Ok(())
}

/// Load a transcript saved by [`save_transcript`].
///
/// # Errors
/// [`io::Error`] on read failure, [`serde_json::Error`] on parse
/// failure.
pub fn load_transcript(path: &Path) -> Result<Transcript, RecordError> {
    let text = std::fs::read_to_string(path)?;
    Ok(serde_json::from_str(&text)?)
}

/// Turn a recorded [`Transcript`] into a replay [`FixtureDriver`] —
/// `tamias-driver-<name>` that answers `serve` requests from the
/// recording. Requests are matched on canonical JSON (object key
/// order ignored); anything unrecorded gets `ok:false`.
///
/// # Errors
/// Propagates [`FixtureDriverBuilder::build`] IO errors.
pub fn replay_driver(
    name: &str,
    transcript: &Transcript,
) -> io::Result<crate::fixture::FixtureDriver> {
    FixtureDriverBuilder::new(name)
        .transcript(transcript.clone())
        .build()
}

impl From<std::io::Error> for RecordError {
    fn from(e: std::io::Error) -> Self {
        // IO failures during record are rare (spawn is Wire::Io);
        // surface them through the wire variant's io error.
        Self::Wire(WireError::Io(e))
    }
}
