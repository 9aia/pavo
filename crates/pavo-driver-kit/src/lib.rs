#![cfg_attr(coverage, feature(coverage_attribute))]
#![cfg_attr(test, allow(clippy::unwrap_used))]
//! pavo-driver-kit — driver authoring for pavo view drivers.
//!
//! A `tamias-driver-<type>` executable is mostly boilerplate: the same
//! ndjson `serve` loop, the same manifest/view struct literals, the
//! same `ok:true`/`ok:false` plumbing — duplicated across every driver
//! in the suite (myco, cuora, sepia, sturn, mimus, tamias, …). This
//! crate absorbs the boilerplate so a driver's UI surface is **pure
//! declaration**: build a [`manifest::Manifest`], answer ops with
//! [`serve::serve_stdio`], and spend the rest of your time on data.
//!
//! The kit is deliberately small — four modules, no framework:
//!
//! - [`serve`] — the ndjson request/response loop every driver
//!   re-implements (`{"op":"…"}` in, `{"ok":…}` out, one per line).
//! - [`manifest`] — fluent builders for [`DriverManifest`],
//!   [`ViewSpec`], [`QueryBinding`], and [`Contribution`] — including
//!   the `deps` and cross-driver (`QueryBinding.driver`) declarations.
//! - [`node`] — [`ViewNode`] constructors plus a [`node::Base`] builder
//!   for the shared `id`/`title`/`query`/`description`/`props` fields.
//! - [`platform`] — [`JobItem`] / [`NotificationItem`] constructors
//!   for the `jobs`/`notifications` feed ops.
//! - [`surface`] — the surface-driver layer: an engine's **ops
//!   manifest** (`ops` verb / `cli.*` wire queries) rendered onto real
//!   surfaces — a generated clap CLI (feature `cli`), MCP tools over
//!   stdio (feature `mcp`), and the spawned-driver [`surface::client`]
//!   both standalone binaries (`tamias-driver-cli`, `-mcp`) are built
//!   on. Engine drivers may embed the same surfaces behind their own
//!   features — optional, never required.
//!
//! All types are `pavo-protocol`'s — the kit adds constructors and
//! the loop, never a parallel type hierarchy. Anything the kit doesn't
//! cover, drivers write against `pavo-protocol` directly.
//!
//! # Minimal driver
//!
//! ```no_run
//! use pavo_driver_kit as kit;
//! use pavo_protocol::wire::{Request, Response};
//!
//! fn main() {
//!     let manifest = kit::Manifest::new("demo", "Demo")
//!         .view(kit::view("board", "Board", "/demo/board",
//!             kit::column(kit::base().title("Board"), vec![
//!                 kit::markdown(kit::base(), "hello"),
//!             ])))
//!         .build();
//!     kit::serve_stdio(|req| match req {
//!         Request::Manifest => Response::ok_typed(&manifest),
//!         Request::Views => Response::ok(serde_json::json!([])),
//!         _ => Response::err("unsupported op"),
//!     })
//!     .ok();
//! }
//! ```
//!
//! See `examples/drivers/` for full drivers; `crates/pavo-driver-config`
//! is the first-party Rust driver (it predates the kit and shows the
//! hand-rolled equivalent).

pub mod manifest;
pub mod node;
pub mod platform;
pub mod serve;
pub mod surface;

pub use manifest::{
    binding, binding_on, binding_on_params, binding_params, capabilities_value, contribution,
    foreign_query, manifest_json, query, view, Manifest, View,
};
pub use node::{
    base, column, column_gap, embed, expandable_tree, field, field_kind, form, form_submit, graph,
    kv, markdown, pair, panel, row, row_gap, select_field, sortable_table, table, table_column,
    tree, tree_item, vertex, Base,
};
pub use platform::{job, notification};
pub use serve::{serve, serve_stdio};
