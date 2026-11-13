#![cfg_attr(coverage, feature(coverage_attribute))]
#![cfg_attr(test, allow(clippy::unwrap_used))]
//! pavo-protocol — the canonical wire contract for the pavo viz host.
//!
//! Pure serde types, no I/O. Three contracts, all versioned
//! (`protocol/v1/` JSON Schemas mirror these types):
//!
//! - **Driver manifest** ([`manifest::DriverManifest`]) — what a
//!   `tamias-driver-<type>` executable answers to `manifest`: name,
//!   surfaces it contributes to, and the views it can render.
//! - **ViewNode IR** ([`view_node::ViewNode`]) — the declarative UI
//!   tree a driver emits; renderers (CLI/TUI/web) interpret it.
//! - **Wire ops** ([`wire::Request`] / [`wire::Response`]) — the
//!   ndjson JSON-RPC convention every sibling `serve` speaks:
//!   requests are `{"op":"…", …}`, responses are
//!   `{"ok":true,"result":…}` or `{"ok":false,"error":"…"}`.
//!
//! Interaction actions ([`action`]) cut across all three: an
//! [`Action`] is declared on the manifest, a view node, or a tree
//! item and dispatched through the `invoke` wire op, answering an
//! [`wire::ActionResult`]; [`manifest::MenuEntry`] entries feed the
//! host command palette.
//!
//! Versioning policy: **additive-only within v1** — new optional
//! fields, new enum variants, and new ops may appear; existing
//! fields and variant spellings never change meaning.

pub mod action;
pub mod manifest;
pub mod platform;
pub mod render_model;
pub mod view_node;
pub mod wire;

pub use action::{Action, DangerLevel, ParamKind, ParamSchema};
pub use manifest::{
    CliSurface, Contributes, Contribution, DriverDeps, DriverManifest, MenuEntry, QueryBinding,
    RendererDecl, RendererSurfaces, ViewSpec, WebSurface,
};
pub use platform::{JobItem, NotificationItem};
pub use render_model::RenderModel;
pub use view_node::{
    Align, ColumnNode, EmbedNode, FormField, FormFieldKind, FormNode, GraphEdge, GraphNode,
    GraphNodeItem, KvNode, KvPair, MarkdownNode, NodeBase, PanelNode, RowNode, TableColumn,
    TableNode, TreeItem, TreeNode, ViewNode,
};
pub use wire::{ActionResult, Request, Response, SubmitResult, WireError};
