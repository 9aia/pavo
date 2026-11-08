#![cfg_attr(coverage, feature(coverage_attribute))]
#![cfg_attr(test, allow(clippy::unwrap_used))]
//! pavo-core — host core: tamias driver discovery, contribution merge,
//! per-driver wire clients, ViewNode IR helpers, and the view-model
//! cache.
//!
//! TODO(P-003 v1): driver discovery over PATH + `<tamias-home>/drivers/`
//! (`$TAMIAS_HOME` or `~/.local/share/tamias/drivers`), manifest fetch +
//! validation against `pavo-protocol`, contribution-slot merge with
//! priority conflict resolution, a spawn-per-driver `serve` client
//! speaking ndjson, and a TTL cache for view models.
//!
//! This crate is a scaffold stub — no implementation yet.
