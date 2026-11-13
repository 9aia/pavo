//! pavo-core — host core: tamias driver discovery, contribution merge,
//! per-driver wire clients, ViewNode IR helpers, and the view-model
//! cache.
//!
//! The host's job (per P-003): find `tamias-driver-*` executables that
//! speak pavo (a `manifest` verb answering [`pavo_protocol::DriverManifest`],
//! or `capabilities` advertising it), merge their `contributes` slots
//! into one chrome description, and resolve `<driver>:<view>` deep
//! links into [`pavo_protocol::ViewResult`]s by speaking the ndjson
//! wire (`{"op":"…"}` → `{"ok":…}`) over each driver's `serve` verb.
//!
//! Everything here is sync and std-only — no async runtime, no
//! renderer deps. Renderers (CLI/TUI/web) consume [`Host`]'s output;
//! they never see driver processes.
//!
//! Module map:
//!
//! - [`discovery`] — scan `PATH` + `<tamias-home>/drivers/` for
//!   pavo-capable `tamias-driver-*` executables.
//! - [`client`] — [`client::DriverClient`], one spawned `serve`
//!   process per driver, request/response with timeout + one restart.
//! - [`merge`] — fold every driver's `contributes` into a single
//!   [`merge::Chrome`] (navbar/dashboard/statusbar/routes/sidebar/
//!   headerActions/notifications/jobs + `extra`).
//! - [`deps`] — topological ordering over `DriverManifest.deps`
//!   (`requires`/`optional`); [`host::Host`] keeps drivers in dep
//!   order and quarantines offenders.
//! - [`host`] — [`host::Host`], the composition root renderers use:
//!   discovery → manifests → clients → views/routes/chrome.
//! - [`viewnode`] — walk/evaluate helpers over the ViewNode IR
//!   (`find_nodes`, `collect_queries`, KV degradation).
//! - [`renderers`] — renderer-plugin registry: manifest `renderers[]`
//!   declarations → kind claims, conflict resolution, `render` ops.
//! - [`paths`] — `<tamias-home>` resolution (`$TAMIAS_HOME` →
//!   `$XDG_DATA_HOME/tamias` → `~/.local/share/tamias` → `./.tamias`).
//! - [`cache`] — small TTL cache for view models.
//! - [`error`] — the crate's [`error::PavoError`] type.

#![cfg_attr(coverage, feature(coverage_attribute))]
#![cfg_attr(test, allow(clippy::unwrap_used))]

pub mod actions;
pub mod cache;
pub mod client;
pub mod deps;
pub mod discovery;
pub mod error;
pub mod host;
pub mod merge;
pub mod paths;
pub mod renderers;
pub mod viewnode;

pub use actions::{
    collect_actions, dedupe_actions, manifest_actions, menu_entries, ops_actions, param_form_field,
    validate_params, ActionScope, ScopedAction,
};
pub use cache::TtlCache;
pub use client::{ClientConfig, DriverClient};
pub use deps::{order as order_drivers, DepsError};
pub use discovery::{discover, discover_with, DiscoveryOptions, DriverInfo};
pub use error::PavoError;
pub use host::{FeedEntry, Host, ResolvedView, RouteEntry, RouteKind, ViewEntry};
pub use merge::{merge_chrome, Chrome, SlotEntry};
pub use paths::{drivers_dir, tamias_home};
pub use renderers::{plugin_nodes, PluginNode, RendererBinding, RendererRegistry};
pub use viewnode::{
    children, collect_queries, find_nodes, kind_of, kv_fallback_json, node_kind_json,
    to_kv_fallback, walk,
};

/// Caller-supplied view/query params — a JSON object passed through to
/// the driver's `serve` verbatim (merged over each binding's static
/// params by the driver).
pub type Params = serde_json::Map<String, serde_json::Value>;
