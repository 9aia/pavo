//! [`ViewNode`] constructors — the declarative tree as function
//! calls instead of nested struct literals (or JSON).
//!
//! Every kind function takes the shared [`NodeBase`] fields first
//! (`impl Into<NodeBase>` — pass [`base()`]'s builder or
//! `NodeBase::default()`), then the kind-specific payload. Part
//! helpers ([`table_column`], [`pair`], [`field`], [`tree_item`],
//! [`vertex`]) cover the sub-structs; anything not covered is a
//! public-field mutation away.
//!
//! ```
//! # use pavo_driver_kit as kit;
//! let tree = kit::panel(
//!     kit::base().title("Board").query("items"),
//!     vec![
//!         kit::table(
//!             kit::base(),
//!             vec![kit::table_column("id", "ID"), kit::table_column("title", "Title")],
//!             vec![vec!["T-1".into(), "ship it".into()]],
//!         ),
//!         kit::kv(kit::base().title("Counts"), vec![kit::pair("open", 3)]),
//!     ],
//! );
//! assert_eq!(tree.base().title.as_deref(), Some("Board"));
//! ```

use serde::Serialize;
use serde_json::{Map, Value};

use pavo_protocol::{
    ColumnNode, EmbedNode, FormField, FormFieldKind, FormNode, GraphEdge, GraphNode, GraphNodeItem,
    KvNode, KvPair, MarkdownNode, NodeBase, PanelNode, RowNode, TableColumn, TableNode, TreeItem,
    TreeNode, ViewNode,
};

/// Builder for the [`NodeBase`] fields every node kind shares —
/// `id`, `title`, `query`, `description`, `props`.
///
/// ```
/// # use pavo_driver_kit as kit;
/// let b = kit::base().id("n1").title("Board").query("items")
///     .description("the board").prop("density", "compact").build();
/// assert_eq!(b.query.as_deref(), Some("items"));
/// assert_eq!(b.props["density"], "compact");
/// ```
pub struct Base {
    base: NodeBase,
}

/// Start a [`NodeBase`] builder — chain setters, pass the result (or
/// `NodeBase::default()`) to any [`crate::node`] kind function.
#[must_use]
pub fn base() -> Base {
    Base {
        base: NodeBase::default(),
    }
}

impl Base {
    /// Stable id — deep-link target, diffing, renderer expansion state.
    #[must_use]
    pub fn id(mut self, id: impl Into<String>) -> Self {
        self.base.id = Some(id.into());
        self
    }

    /// Heading the renderer may display above the node.
    #[must_use]
    pub fn title(mut self, title: impl Into<String>) -> Self {
        self.base.title = Some(title.into());
        self
    }

    /// Binding name — references a [`crate::manifest::View`]'s
    /// [`pavo_protocol::QueryBinding`] (via [`crate::manifest::View::query`]).
    #[must_use]
    pub fn query(mut self, query: impl Into<String>) -> Self {
        self.base.query = Some(query.into());
        self
    }

    /// Longer description (tooltips, help panes).
    #[must_use]
    pub fn description(mut self, description: impl Into<String>) -> Self {
        self.base.description = Some(description.into());
        self
    }

    /// Renderer hint — kind-specific knob (`density`, `direction`,
    /// `collapsed`, …). Renderers ignore keys they don't know.
    #[must_use]
    pub fn prop(mut self, key: &str, value: impl Serialize) -> Self {
        self.base.props.insert(key.to_string(), to_value(value));
        self
    }

    /// Add an action scoped to this node — dispatched via the
    /// `invoke` wire op. See [`crate::action`].
    #[must_use]
    pub fn action(mut self, action: pavo_protocol::Action) -> Self {
        self.base.actions.push(action);
        self
    }

    /// Set all node actions at once.
    #[must_use]
    pub fn actions(mut self, actions: Vec<pavo_protocol::Action>) -> Self {
        self.base.actions = actions;
        self
    }

    /// Finish — the [`NodeBase`] the kind functions take.
    #[must_use]
    pub fn build(self) -> NodeBase {
        self.base
    }
}

impl From<Base> for NodeBase {
    fn from(base: Base) -> Self {
        base.base
    }
}

/// `panel` — a titled container grouping children; the universal
/// wrapper.
#[must_use]
pub fn panel(base: impl Into<NodeBase>, children: Vec<ViewNode>) -> ViewNode {
    ViewNode::Panel(PanelNode {
        base: base.into(),
        children,
    })
}

/// `row` — horizontal layout container (children flow left-to-right).
#[must_use]
pub fn row(base: impl Into<NodeBase>, children: Vec<ViewNode>) -> ViewNode {
    ViewNode::Row(RowNode {
        base: base.into(),
        children,
        gap: None,
    })
}

/// [`row`] with an explicit `gap` spacing hint.
#[must_use]
pub fn row_gap(base: impl Into<NodeBase>, gap: u32, children: Vec<ViewNode>) -> ViewNode {
    ViewNode::Row(RowNode {
        base: base.into(),
        children,
        gap: Some(gap),
    })
}

/// `column` — vertical layout container (children stack top-to-bottom).
#[must_use]
pub fn column(base: impl Into<NodeBase>, children: Vec<ViewNode>) -> ViewNode {
    ViewNode::Column(ColumnNode {
        base: base.into(),
        children,
        gap: None,
    })
}

/// [`column`] with an explicit `gap` spacing hint.
#[must_use]
pub fn column_gap(base: impl Into<NodeBase>, gap: u32, children: Vec<ViewNode>) -> ViewNode {
    ViewNode::Column(ColumnNode {
        base: base.into(),
        children,
        gap: Some(gap),
    })
}

/// `table` — declared columns plus inline rows (or query-fed data via
/// `base.query`). Cells are arbitrary JSON values.
#[must_use]
pub fn table(
    base: impl Into<NodeBase>,
    columns: Vec<TableColumn>,
    rows: Vec<Vec<Value>>,
) -> ViewNode {
    ViewNode::Table(TableNode {
        base: base.into(),
        columns,
        rows,
        sortable: false,
    })
}

/// [`table`] with `sortable` set — the renderer may offer column
/// sorting.
#[must_use]
pub fn sortable_table(
    base: impl Into<NodeBase>,
    columns: Vec<TableColumn>,
    rows: Vec<Vec<Value>>,
) -> ViewNode {
    ViewNode::Table(TableNode {
        base: base.into(),
        columns,
        rows,
        sortable: true,
    })
}

/// One [`TableColumn`] — `key` names the field in object-shaped
/// query-fed rows; align/width are set on the returned struct's
/// public fields.
#[must_use]
pub fn table_column(key: &str, title: &str) -> TableColumn {
    TableColumn {
        key: key.to_string(),
        title: title.to_string(),
        align: None,
        width: None,
    }
}

/// `tree` — hierarchical items (nested via [`TreeItem::children`]).
/// Query-fed trees take `base.query` and their bound result supplies
/// `items` in the same shape.
#[must_use]
pub fn tree(base: impl Into<NodeBase>, items: Vec<TreeItem>) -> ViewNode {
    ViewNode::Tree(TreeNode {
        base: base.into(),
        items,
        expandable: false,
    })
}

/// [`tree`] with expand/collapse affordances enabled.
#[must_use]
pub fn expandable_tree(base: impl Into<NodeBase>, items: Vec<TreeItem>) -> ViewNode {
    ViewNode::Tree(TreeNode {
        base: base.into(),
        items,
        expandable: true,
    })
}

/// One [`TreeItem`] — label plus optional detail/icon/children (all
/// public fields).
#[must_use]
pub fn tree_item(label: impl Into<String>) -> TreeItem {
    TreeItem {
        id: None,
        label: label.into(),
        detail: None,
        icon: None,
        children: Vec::new(),
        props: Map::new(),
        actions: Vec::new(),
    }
}

/// `graph` — nodes and edges; `from`/`to` reference vertex `id`s.
/// Layout is the renderer's problem (`layout` set on the node struct
/// for hints like `"mermaid"`/`"dot"`).
#[must_use]
pub fn graph(
    base: impl Into<NodeBase>,
    nodes: Vec<GraphNodeItem>,
    edges: Vec<GraphEdge>,
) -> ViewNode {
    ViewNode::Graph(GraphNode {
        base: base.into(),
        nodes,
        edges,
        layout: None,
    })
}

/// One [`GraphNodeItem`] — vertex `id` + `label`; `kind`/`props` are
/// public fields for style hints.
#[must_use]
pub fn vertex(id: &str, label: &str) -> GraphNodeItem {
    GraphNodeItem {
        id: id.to_string(),
        label: label.to_string(),
        kind: None,
        props: Map::new(),
    }
}

/// `form` — input fields, read-only preview (no `submit_op`).
#[must_use]
pub fn form(base: impl Into<NodeBase>, fields: Vec<FormField>) -> ViewNode {
    ViewNode::Form(FormNode {
        base: base.into(),
        fields,
        submit_op: None,
        submit_label: None,
    })
}

/// [`form`] with a `submit_op` — interactive on surfaces that answer
/// the wire `submit` op; `base.id` is the form's submit target.
#[must_use]
pub fn form_submit(
    base: impl Into<NodeBase>,
    fields: Vec<FormField>,
    submit_op: impl Into<String>,
) -> ViewNode {
    ViewNode::Form(FormNode {
        base: base.into(),
        fields,
        submit_op: Some(submit_op.into()),
        submit_label: None,
    })
}

/// One [`FormField`] — text input; `required`/`options`/`default`/
/// `value`/`hint`/`editable`/`placeholder` are public fields.
#[must_use]
pub fn field(key: &str, label: &str) -> FormField {
    field_kind(key, label, FormFieldKind::Text)
}

/// [`field`] with an explicit [`FormFieldKind`].
#[must_use]
pub fn field_kind(key: &str, label: &str, kind: FormFieldKind) -> FormField {
    FormField {
        key: key.to_string(),
        label: label.to_string(),
        kind,
        required: false,
        options: Vec::new(),
        default: None,
        value: None,
        hint: None,
        editable: true,
        placeholder: None,
    }
}

/// A `select` [`FormField`] — [`FormFieldKind::Select`] with options.
#[must_use]
pub fn select_field(key: &str, label: &str, options: Vec<String>) -> FormField {
    FormField {
        kind: FormFieldKind::Select,
        options,
        ..field_kind(key, label, FormFieldKind::Select)
    }
}

/// `kv` — key/value pairs: detail panes, metadata, and the universal
/// degradation target for unknown kinds.
#[must_use]
pub fn kv(base: impl Into<NodeBase>, pairs: Vec<KvPair>) -> ViewNode {
    ViewNode::Kv(KvNode {
        base: base.into(),
        pairs,
    })
}

/// One [`KvPair`] — `key` + arbitrary JSON `value` (serialized via
/// [`serde`]; unserializable values degrade to `null`, never panic).
#[must_use]
pub fn pair(key: &str, value: impl Serialize) -> KvPair {
    KvPair {
        key: key.to_string(),
        value: to_value(value),
        kind: None,
    }
}

/// `markdown` — free-form markdown text.
#[must_use]
pub fn markdown(base: impl Into<NodeBase>, text: impl Into<String>) -> ViewNode {
    ViewNode::Markdown(MarkdownNode {
        base: base.into(),
        text: text.into(),
    })
}

/// `embed` — escape hatch: the host resolves `component` against its
/// renderer registry; component props ride in `base.props`.
#[must_use]
pub fn embed(base: impl Into<NodeBase>, component: impl Into<String>) -> ViewNode {
    ViewNode::Embed(EmbedNode {
        base: base.into(),
        component: component.into(),
    })
}

/// Serialize a prop/pair value — `null` on failure (a driver must
/// never panic on the wire).
fn to_value(value: impl Serialize) -> Value {
    serde_json::to_value(value).unwrap_or(Value::Null)
}

#[cfg(test)]
#[cfg_attr(coverage, coverage(off))]
mod tests {
    #![allow(clippy::unwrap_used)]

    use super::*;
    use serde_json::json;

    /// Constructors emit the right `kind` wire shapes.
    #[test]
    fn kinds_serialize() {
        let nodes = vec![
            panel(base().title("P"), vec![]),
            row(base(), vec![]),
            row_gap(base(), 2, vec![]),
            column(base(), vec![]),
            column_gap(base(), 3, vec![]),
            table(base(), vec![table_column("k", "K")], vec![]),
            sortable_table(base(), vec![], vec![]),
            tree(base(), vec![tree_item("root")]),
            expandable_tree(base(), vec![]),
            graph(base(), vec![vertex("a", "A")], vec![]),
            form(base(), vec![field("k", "K")]),
            form_submit(
                base().id("f"),
                vec![select_field("s", "S", vec!["x".into()])],
                "go",
            ),
            kv(base(), vec![pair("k", 1)]),
            markdown(base(), "hi"),
            embed(base().prop("p", 1), "comp"),
        ];
        let kinds: Vec<&str> = nodes
            .iter()
            .map(|n| match n {
                ViewNode::Panel(_) => "panel",
                ViewNode::Row(_) => "row",
                ViewNode::Column(_) => "column",
                ViewNode::Table(_) => "table",
                ViewNode::Tree(_) => "tree",
                ViewNode::Graph(_) => "graph",
                ViewNode::Form(_) => "form",
                ViewNode::Kv(_) => "kv",
                ViewNode::Markdown(_) => "markdown",
                ViewNode::Embed(_) => "embed",
            })
            .collect();
        assert_eq!(
            kinds,
            [
                "panel", "row", "row", "column", "column", "table", "table", "tree", "tree",
                "graph", "form", "form", "kv", "markdown", "embed"
            ]
        );

        // Spot-check wire details: gap, sortable, submit_op, props.
        let row_gap = serde_json::to_value(&nodes[2]).unwrap();
        assert_eq!(row_gap["gap"], json!(2));
        let st = serde_json::to_value(&nodes[6]).unwrap();
        assert_eq!(st["sortable"], json!(true));
        let fs = serde_json::to_value(&nodes[11]).unwrap();
        assert_eq!(fs["submit_op"], json!("go"));
        assert_eq!(fs["id"], json!("f"));
        assert_eq!(fs["fields"][0]["kind"], json!("select"));
        let em = serde_json::to_value(&nodes[14]).unwrap();
        assert_eq!(em["component"], json!("comp"));
        assert_eq!(em["props"]["p"], json!(1));
    }

    /// `Base` covers the shared NodeBase fields.
    #[test]
    fn base_builder() {
        let b = base()
            .id("i")
            .title("t")
            .query("q")
            .description("d")
            .prop("k", "v")
            .build();
        assert_eq!(b.id.as_deref(), Some("i"));
        assert_eq!(b.title.as_deref(), Some("t"));
        assert_eq!(b.query.as_deref(), Some("q"));
        assert_eq!(b.description.as_deref(), Some("d"));
        assert_eq!(b.props["k"], json!("v"));
        // NodeBase itself satisfies Into<NodeBase> (identity).
        let n = markdown(NodeBase::default(), "x");
        assert!(n.base().title.is_none());
    }
}
