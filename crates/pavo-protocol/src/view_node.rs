//! ViewNode IR — the declarative UI tree drivers emit and every pavo
//! renderer (CLI, TUI, web) interprets.
//!
//! Wire shape: each node is a JSON object with a `"kind"` discriminator
//! (internally tagged — no wrapper envelope):
//!
//! ```json
//! { "kind": "table", "title": "Board", "query": "items",
//!   "columns": [{ "key": "id", "title": "ID" }],
//!   "rows": [["T-001", "pavo scaffold"]] }
//! ```
//!
//! Nodes bind to data through [`NodeBase::query`], which names a
//! [`crate::manifest::QueryBinding`] in the enclosing [`ViewSpec`];
//! the host resolves bindings to view-model data before rendering.
//! Renderers that don't know a `kind` must degrade to a generic
//! key/value rendering rather than failing — forward compatibility
//! is a host obligation.

use serde::{Deserialize, Serialize};
use serde_json::{Map, Value};

/// The ViewNode IR — one node of a declarative view tree.
///
/// Internally tagged on `"kind"` with lowercase variant names:
/// `panel`, `table`, `tree`, `graph`, `form`, `kv`, `markdown`,
/// `embed`, `row`, `column`. Every variant wraps a node struct that
/// carries the shared [`NodeBase`] fields flattened to the top level.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(tag = "kind", rename_all = "lowercase")]
pub enum ViewNode {
    /// A titled container grouping children — the universal wrapper.
    Panel(PanelNode),
    /// Tabular data: declared columns plus rows (inline or query-fed).
    Table(TableNode),
    /// Hierarchical data (nested items, breadcrumbs, file trees).
    Tree(TreeNode),
    /// Nodes + edges — the escape hatch for graph-shaped state;
    /// typically rendered by a view-kind driver.
    Graph(GraphNode),
    /// An input form: fields, defaults, and a submit op.
    Form(FormNode),
    /// Key/value pairs — detail panes, metadata, degraded fallback
    /// for unknown kinds.
    Kv(KvNode),
    /// Free-form markdown text.
    Markdown(MarkdownNode),
    /// Escape hatch: host renders `component` with `props` — used for
    /// rich renderers (WASM/web components) on capable surfaces.
    Embed(EmbedNode),
    /// Horizontal layout: children flow left-to-right.
    Row(RowNode),
    /// Vertical layout: children stack top-to-bottom.
    Column(ColumnNode),
}

/// Fields every ViewNode kind shares, flattened onto each node's JSON
/// object so authors write one flat shape regardless of kind.
#[derive(Clone, Debug, Default, PartialEq, Serialize, Deserialize)]
pub struct NodeBase {
    /// Optional stable id — targets for deep links, diffing, and
    /// renderer-side expansion state.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub id: Option<String>,
    /// Optional heading the renderer may display above the node.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub title: Option<String>,
    /// Optional binding name — references a
    /// [`crate::manifest::QueryBinding`] whose result supplies this
    /// node's data. Meaning is kind-specific: `table` reads `rows`,
    /// `kv` reads `pairs`, `tree` reads `items`, `graph` reads
    /// `nodes`/`edges`.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub query: Option<String>,
    /// Optional longer description (tooltips, help panes).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub description: Option<String>,
    /// Renderer hints — kind-specific knobs (`density`, `direction`,
    /// `collapsed`, …). Renderers ignore keys they don't know.
    #[serde(default, skip_serializing_if = "Map::is_empty")]
    pub props: Map<String, Value>,
}

/// `panel` — a titled container grouping child nodes.
#[derive(Clone, Debug, Default, PartialEq, Serialize, Deserialize)]
pub struct PanelNode {
    /// Shared node fields.
    #[serde(flatten)]
    pub base: NodeBase,
    /// Child nodes rendered inside the panel.
    #[serde(default)]
    pub children: Vec<ViewNode>,
}

/// `row` — horizontal layout container.
#[derive(Clone, Debug, Default, PartialEq, Serialize, Deserialize)]
pub struct RowNode {
    /// Shared node fields.
    #[serde(flatten)]
    pub base: NodeBase,
    /// Children laid out left-to-right.
    #[serde(default)]
    pub children: Vec<ViewNode>,
    /// Spacing hint in arbitrary units (renderer-defined scale).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub gap: Option<u32>,
}

/// `column` — vertical layout container.
#[derive(Clone, Debug, Default, PartialEq, Serialize, Deserialize)]
pub struct ColumnNode {
    /// Shared node fields.
    #[serde(flatten)]
    pub base: NodeBase,
    /// Children stacked top-to-bottom.
    #[serde(default)]
    pub children: Vec<ViewNode>,
    /// Spacing hint in arbitrary units (renderer-defined scale).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub gap: Option<u32>,
}

/// Column alignment hint for [`TableColumn`].
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum Align {
    /// Left-aligned (default for text).
    #[default]
    Left,
    /// Centered.
    Center,
    /// Right-aligned (default for numbers).
    Right,
}

/// `table` — tabular data with declared columns.
///
/// Rows may be declared inline (`rows`) or supplied by the bound
/// query (`base.query`); inline rows win when both are present so a
/// driver can ship static previews.
#[derive(Clone, Debug, Default, PartialEq, Serialize, Deserialize)]
pub struct TableNode {
    /// Shared node fields.
    #[serde(flatten)]
    pub base: NodeBase,
    /// Column declarations, in display order.
    #[serde(default)]
    pub columns: Vec<TableColumn>,
    /// Inline rows — each row is a list of cell values aligned to
    /// `columns`. Cell values are arbitrary JSON (`string`, `number`,
    /// `bool`, `null`); renderers stringify.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub rows: Vec<Vec<Value>>,
    /// When true the renderer may offer sorting by column.
    #[serde(default)]
    pub sortable: bool,
}

/// One column declaration in a [`TableNode`].
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct TableColumn {
    /// Field key — matches the cell position in `rows` by index, and
    /// names the column in query-fed data where rows arrive as
    /// objects (`{ "<key>": value, … }`).
    pub key: String,
    /// Column heading.
    pub title: String,
    /// Alignment hint; renderers may infer from data when absent.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub align: Option<Align>,
    /// Preferred width in characters/columns (renderer-defined unit).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub width: Option<u32>,
}

/// `tree` — hierarchical items.
#[derive(Clone, Debug, Default, PartialEq, Serialize, Deserialize)]
pub struct TreeNode {
    /// Shared node fields.
    #[serde(flatten)]
    pub base: NodeBase,
    /// Top-level items; each may nest `children` recursively.
    /// When `base.query` is set, the bound result supplies `items`
    /// in this same shape. Omitted when empty (like
    /// [`TableNode::rows`]) so an empty inline list can't shadow
    /// query-bound data on the typed round-trip.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub items: Vec<TreeItem>,
    /// When true the renderer shows expand/collapse affordances.
    #[serde(default)]
    pub expandable: bool,
}

/// One node in a [`TreeNode`] hierarchy.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct TreeItem {
    /// Stable id (deep-link target, selection key). Required for
    /// interactive trees; optional for static previews.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub id: Option<String>,
    /// Display label.
    pub label: String,
    /// Optional secondary text rendered muted after the label.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub detail: Option<String>,
    /// Optional icon hint.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub icon: Option<String>,
    /// Nested children.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub children: Vec<TreeItem>,
    /// Arbitrary per-item props for rich renderers.
    #[serde(default, skip_serializing_if = "Map::is_empty")]
    pub props: Map<String, Value>,
}

/// `graph` — nodes and edges. The IR floor covers structure only;
/// layout is the renderer's problem (a view-kind driver may take
/// over entirely via `embed`).
#[derive(Clone, Debug, Default, PartialEq, Serialize, Deserialize)]
pub struct GraphNode {
    /// Shared node fields.
    #[serde(flatten)]
    pub base: NodeBase,
    /// Graph vertices.
    #[serde(default)]
    pub nodes: Vec<GraphNodeItem>,
    /// Graph edges; `from`/`to` reference [`GraphNodeItem::id`].
    #[serde(default)]
    pub edges: Vec<GraphEdge>,
    /// Layout hint for renderers that support it (`"dot"`, `"mermaid"`,
    /// `"layered"`, …). Unknown values are ignored.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub layout: Option<String>,
}

/// One vertex in a [`GraphNode`].
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct GraphNodeItem {
    /// Vertex id — edge endpoints reference this.
    pub id: String,
    /// Display label.
    pub label: String,
    /// Optional kind/style hint (`"module"`, `"file"`, `"warning"`, …).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub kind: Option<String>,
    /// Arbitrary per-node props.
    #[serde(default, skip_serializing_if = "Map::is_empty")]
    pub props: Map<String, Value>,
}

/// One edge in a [`GraphNode`].
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct GraphEdge {
    /// Source vertex id.
    pub from: String,
    /// Target vertex id.
    pub to: String,
    /// Optional edge label.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub label: Option<String>,
}

/// `form` — input fields plus a submit op.
#[derive(Clone, Debug, Default, PartialEq, Serialize, Deserialize)]
pub struct FormNode {
    /// Shared node fields.
    #[serde(flatten)]
    pub base: NodeBase,
    /// Field declarations, in display order.
    #[serde(default)]
    pub fields: Vec<FormField>,
    /// Wire op invoked on submit (e.g. `"set"`, `"update"`). When
    /// absent the renderer may only preview the form.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub submit_op: Option<String>,
    /// Submit button label; renderers default to "Submit".
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub submit_label: Option<String>,
}

/// Field kinds a [`FormField`] may declare.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum FormFieldKind {
    /// Single-line text.
    #[default]
    Text,
    /// Multi-line text.
    Textarea,
    /// Integer or float.
    Number,
    /// Boolean toggle/checkbox.
    Bool,
    /// One-of selection from `options`.
    Select,
}

/// One field in a [`FormNode`].
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct FormField {
    /// Field key — the name submitted with the form data.
    pub key: String,
    /// Human-facing label.
    pub label: String,
    /// Input kind; renderers may fall back to text for kinds they
    /// don't support.
    #[serde(default)]
    pub kind: FormFieldKind,
    /// Whether the field must be non-empty on submit.
    #[serde(default)]
    pub required: bool,
    /// Options for `select` fields.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub options: Vec<String>,
    /// Default value (JSON — string/number/bool per `kind`).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub default: Option<Value>,
    /// Current value when the form renders existing state.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub value: Option<Value>,
    /// Help text rendered under the field.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub hint: Option<String>,
}

/// `kv` — key/value pairs: detail panes, metadata, and the universal
/// degradation target for unknown node kinds.
#[derive(Clone, Debug, Default, PartialEq, Serialize, Deserialize)]
pub struct KvNode {
    /// Shared node fields.
    #[serde(flatten)]
    pub base: NodeBase,
    /// Pairs in display order.
    #[serde(default)]
    pub pairs: Vec<KvPair>,
}

/// One key/value pair in a [`KvNode`].
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct KvPair {
    /// Pair key.
    pub key: String,
    /// Pair value — arbitrary JSON; renderers stringify or format
    /// per their capabilities.
    pub value: Value,
    /// Optional hint for rich renderers (e.g. `"link"`, `"badge"`).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub kind: Option<String>,
}

/// `markdown` — free-form markdown text.
#[derive(Clone, Debug, Default, PartialEq, Serialize, Deserialize)]
pub struct MarkdownNode {
    /// Shared node fields.
    #[serde(flatten)]
    pub base: NodeBase,
    /// Markdown source.
    #[serde(default)]
    pub text: String,
}

/// `embed` — escape hatch for rich renderers: the host looks up
/// `component` (a registered renderer or web component name) and
/// hands it `props`. Drivers needing custom visuals (live graphs,
/// timelines, WASM) use this rather than extending the IR.
#[derive(Clone, Debug, Default, PartialEq, Serialize, Deserialize)]
pub struct EmbedNode {
    /// Shared node fields.
    #[serde(flatten)]
    pub base: NodeBase,
    /// Component/renderer name — resolved against the host's
    /// renderer registry (and, on web, registered web components).
    pub component: String,
    /// Props passed verbatim to the component.
    #[serde(default, skip_serializing_if = "Map::is_empty")]
    pub props: Map<String, Value>,
}

/// Convenience: look up a node's shared fields without matching.
impl ViewNode {
    /// The node's [`NodeBase`] — id/title/query/description/props.
    #[must_use]
    pub fn base(&self) -> &NodeBase {
        match self {
            Self::Panel(n) => &n.base,
            Self::Table(n) => &n.base,
            Self::Tree(n) => &n.base,
            Self::Graph(n) => &n.base,
            Self::Form(n) => &n.base,
            Self::Kv(n) => &n.base,
            Self::Markdown(n) => &n.base,
            Self::Embed(n) => &n.base,
            Self::Row(n) => &n.base,
            Self::Column(n) => &n.base,
        }
    }

    /// Mutable access to the node's [`NodeBase`].
    pub fn base_mut(&mut self) -> &mut NodeBase {
        match self {
            Self::Panel(n) => &mut n.base,
            Self::Table(n) => &mut n.base,
            Self::Tree(n) => &mut n.base,
            Self::Graph(n) => &mut n.base,
            Self::Form(n) => &mut n.base,
            Self::Kv(n) => &mut n.base,
            Self::Markdown(n) => &mut n.base,
            Self::Embed(n) => &mut n.base,
            Self::Row(n) => &mut n.base,
            Self::Column(n) => &mut n.base,
        }
    }
}

#[cfg(test)]
mod tests {
    #![allow(clippy::unwrap_used)]
    use super::*;
    use serde_json::json;

    /// Every kind parses and `base()`/`base_mut()` reach its NodeBase.
    #[test]
    fn base_accessors_cover_all_kinds() {
        let kinds = [
            json!({"kind":"panel","title":"P"}),
            json!({"kind":"table","title":"T","columns":[],"rows":[]}),
            json!({"kind":"tree","title":"Tr","items":[]}),
            json!({"kind":"graph","title":"G","nodes":[],"edges":[]}),
            json!({"kind":"form","title":"F","fields":[]}),
            json!({"kind":"kv","title":"K","pairs":[]}),
            json!({"kind":"markdown","title":"M","text":"hi"}),
            json!({"kind":"embed","title":"E","component":"c"}),
            json!({"kind":"row","title":"R","children":[]}),
            json!({"kind":"column","title":"C","children":[]}),
        ];
        for (i, v) in kinds.into_iter().enumerate() {
            let mut node: ViewNode = serde_json::from_value(v).unwrap();
            assert!(node.base().title.is_some(), "kind {i}");
            node.base_mut().id = Some(format!("n{i}"));
            assert_eq!(node.base().id.as_deref(), Some(format!("n{i}").as_str()));
        }
    }
}
