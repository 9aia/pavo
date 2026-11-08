#![cfg_attr(coverage, feature(coverage_attribute))]
#![cfg_attr(test, allow(clippy::unwrap_used))]
//! pavo-testkit — conformance harness for pavo drivers and renderers.
//!
//! Three pieces, all built on `pavo-protocol`'s wire types:
//!
//! - [`fixture`] — [`fixture::FixtureDriver`], a builder that writes a
//!   tempdir `tamias-driver-<name>` executable (a Python 3 script,
//!   stdlib only) answering `manifest`/`views`/`view`/`query` over the
//!   ndjson `serve` wire. Manifest, view specs, canned query results,
//!   and failure modes (bad JSON line, `ok:false` errors, delayed
//!   responses, mid-session exit) are all configurable — use it for
//!   host/renderer tests without hand-writing shell fixtures.
//! - [`conformance`] — [`conformance::check_driver`] runs the driver
//!   contract suite over the wire and returns structured
//!   [`conformance::Violation`]s instead of panicking.
//! - [`record`] — capture a driver's ndjson session to a transcript
//!   file ([`record::record_session`], [`record::save_transcript`])
//!   and replay it deterministically as a new [`fixture::FixtureDriver`]
//!   ([`record::replay_driver`]) — renderer tests without spawning
//!   processes.
//!
//! [`wire`] exposes the raw [`wire::Session`] for tests that need to
//! speak malformed lines or inspect transport errors directly.

pub mod conformance;
pub mod fixture;
pub mod record;
pub mod wire;

pub use conformance::{check_driver, check_driver_with, Check, CheckOptions, Violation};
pub use fixture::{Failure, FixtureDriver, FixtureDriverBuilder};
pub use record::{load_transcript, record_session, replay_driver, save_transcript, Transcript};
pub use wire::{Session, WireError};
