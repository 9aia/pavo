//! ViewNode IR helpers — the walk/evaluate utilities renderers need.
//!
//! The IR is a tree of [`ViewNode`]s; containers (`panel`, `row`,
//! `column`) nest `children`, everything else is a leaf. These
//! helpers give renderers three things without re-implementing the
//! traversal:
//!
//! - [`walk`] / [`find_nodes`] / [`children`] / [`kind_of`] — tree
//!   traversal and kind inspection.
//! - [`collect_queries`] — every `NodeBase::query` binding name in a
//!   tree, so a renderer/host knows which view-model keys a tree
//!   consumes.
//! - KV degradation — [`to_kv_fallback`] turns any *known*
//!   [`ViewNode`] into a [`KvNode`]; [`node_kind_json`] +
//!   [`kv_fallback_json`] do the same for a raw `serde_json::Value`
//!   whose `kind` this host doesn't know (the protocol's
//!   forward-compat rule: unknown kinds degrade to generic KV, never
//!   fail).

use std::collections::BTreeSet;

use pavo_protocol::{KvNode, ViewNode};
use serde_json::{Map, Value};

/// The `kind` discriminator of a node, as the wire spells it
/// (`"panel"`, `"table"`, …).
#[must_use]
pub fn kind_of(node: &ViewNode) -> &'static str {
    match node {
        ViewNode::Panel(_) => "panel",
        ViewNode::Table(_) => "table",
        ViewNode::Tree(_) => "tree",
        ViewNode::Graph(_) => "graph",
        ViewNode::Form(_) => "form",
        ViewNode::Kv(_) => "kv",
        ViewNode::Markdown(_) => "markdown",
        ViewNode::Embed(_) => "embed",
        ViewNode::Row(_) => "row",
        ViewNode::Column(_) => "column",
    }
}

/// A node's children — `panel`/`row`/`column` nest `Vec<ViewNode>`;
/// every other kind is a leaf and yields an empty slice.
#[must_use]
pub fn children(node: &ViewNode) -> &[ViewNode] {
    match node {
        ViewNode::Panel(n) => &n.children,
        ViewNode::Row(n) => &n.children,
        ViewNode::Column(n) => &n.children,
        _ => &[],
    }
}

/// Depth-first pre-order walk over the whole tree — `f` sees every
/// node exactly once, root first.
pub fn walk<'a>(root: &'a ViewNode, f: &mut impl FnMut(&'a ViewNode)) {
    f(root);
    for child in children(root) {
        walk(child, f);
    }
}

/// All nodes in the tree whose [`kind_of`] equals `kind`,
/// pre-order.
#[must_use]
pub fn find_nodes<'a>(root: &'a ViewNode, kind: &str) -> Vec<&'a ViewNode> {
    let mut out = Vec::new();
    walk(root, &mut |n| {
        if kind_of(n) == kind {
            out.push(n);
        }
    });
    out
}

/// Every query binding name referenced by `NodeBase::query` anywhere
/// in the tree — the set of view-model keys the tree consumes.
/// Sorted (`BTreeSet`) for deterministic output.
#[must_use]
pub fn collect_queries(root: &ViewNode) -> BTreeSet<String> {
    let mut out = BTreeSet::new();
    walk(root, &mut |n| {
        if let Some(q) = &n.base().query {
            out.insert(q.clone());
        }
    });
    out
}

/// Degrade any known [`ViewNode`] to a [`KvNode`] — the universal
/// fallback renderers use for kinds they don't implement (and the
/// shape [`kv_fallback_json`] produces for unknown wire kinds).
///
/// The node's scalar fields become pairs (`kind`, `id`, `title`,
/// plus kind-specific scalars like `text`/`component`); nested
/// objects/arrays are skipped — a degraded node shows what it can.
#[must_use]
pub fn to_kv_fallback(node: &ViewNode) -> KvNode {
    // Serializing a ViewNode can't fail in practice; on the impossible
    // error we degrade to an empty kv rather than panic.
    let value = serde_json::to_value(node).unwrap_or(Value::Null);
    let json = kv_fallback_json(&value);
    serde_json::from_value(json).unwrap_or_else(|_| KvNode {
        base: node.base().clone(),
        pairs: Vec::new(),
    })
}

/// The `kind` string of a raw node JSON — `None` when absent or not
/// a string. Renderers call this *before* deserializing so unknown
/// kinds can route to [`kv_fallback_json`] instead of failing.
#[must_use]
pub fn node_kind_json(node: &Value) -> Option<&str> {
    node.get("kind").and_then(Value::as_str)
}

/// Degrade a raw node JSON object to `{"kind":"kv", …}` — same rule
/// as [`to_kv_fallback`] but for trees that haven't (or can't)
/// deserialize into [`ViewNode`] because their `kind` is unknown to
/// this host version.
///
/// Output shape: `kind` forced to `"kv"`; `id`/`title`/`query`/
/// `description` are preserved as node fields when scalar; `pairs[]`
/// gets a synthesized `{"key":"kind","value":<original kind>}` entry
/// followed by every other scalar field. Objects/arrays don't become
/// pairs. Non-object input yields an empty kv node.
#[must_use]
pub fn kv_fallback_json(node: &Value) -> Value {
    let mut out = Map::new();
    out.insert("kind".to_string(), Value::from("kv"));
    let Some(obj) = node.as_object() else {
        out.insert("pairs".to_string(), Value::Array(Vec::new()));
        return Value::Object(out);
    };
    for key in ["id", "title", "query", "description"] {
        if let Some(v) = obj.get(key).filter(|v| is_scalar(v)) {
            out.insert(key.to_string(), v.clone());
        }
    }
    let mut pairs: Vec<Value> = Vec::new();
    // The original kind survives as the first pair — a degraded view
    // can still show "this was a hologram".
    if let Some(kind) = node_kind_json(node) {
        let mut pair = Map::new();
        pair.insert("key".to_string(), Value::from("kind"));
        pair.insert("value".to_string(), Value::from(kind));
        pairs.push(Value::Object(pair));
    }
    pairs.extend(obj.iter().filter_map(|(k, v)| {
        if matches!(
            k.as_str(),
            "kind" | "id" | "title" | "query" | "description"
        ) || !is_scalar(v)
        {
            return None;
        }
        let mut pair = Map::new();
        pair.insert("key".to_string(), Value::from(k.as_str()));
        pair.insert("value".to_string(), v.clone());
        Some(Value::Object(pair))
    }));
    out.insert("pairs".to_string(), Value::Array(pairs));
    Value::Object(out)
}

/// Scalars become kv pairs; objects/arrays don't (they'd nest
/// structure a degraded renderer can't show anyway).
fn is_scalar(v: &Value) -> bool {
    !(v.is_object() || v.is_array())
}

#[cfg(test)]
#[cfg_attr(coverage, coverage(off))]
mod tests {
    #![allow(clippy::unwrap_used)]

    use super::*;
    use pavo_protocol::KvNode;
    use serde_json::json;

    fn tree() -> ViewNode {
        serde_json::from_value(json!({
            "kind": "panel", "title": "Root",
            "children": [
                {"kind": "table", "id": "t1", "query": "items",
                 "columns": [{"key": "id", "title": "ID"}],
                 "rows": [["a"]]},
                {"kind": "column", "children": [
                    {"kind": "kv", "query": "meta",
                     "pairs": [{"key": "k", "value": 1}]},
                    {"kind": "markdown", "text": "hi"}
                ]}
            ]
        }))
        .unwrap()
    }

    #[test]
    fn kind_children_and_walk() {
        let root = tree();
        assert_eq!(kind_of(&root), "panel");
        assert_eq!(children(&root).len(), 2);
        assert!(children(&children(&root)[0]).is_empty());
        let mut kinds = Vec::new();
        walk(&root, &mut |n| kinds.push(kind_of(n).to_string()));
        assert_eq!(kinds, ["panel", "table", "column", "kv", "markdown"]);
    }

    #[test]
    fn find_nodes_and_queries() {
        let root = tree();
        let tables = find_nodes(&root, "table");
        assert_eq!(tables.len(), 1);
        assert_eq!(tables[0].base().id.as_deref(), Some("t1"));
        assert!(find_nodes(&root, "graph").is_empty());
        let queries = collect_queries(&root);
        assert!(queries.contains("items"));
        assert!(queries.contains("meta"));
        assert_eq!(queries.len(), 2);
    }

    #[test]
    fn kv_fallback_for_known_node() {
        let node: ViewNode = serde_json::from_value(json!({
            "kind": "markdown", "id": "m", "title": "Note",
            "text": "hello", "props": {"a": 1}
        }))
        .unwrap();
        let kv = to_kv_fallback(&node);
        assert_eq!(kv.base.id.as_deref(), Some("m"));
        assert_eq!(kv.base.title.as_deref(), Some("Note"));
        let keys: Vec<&str> = kv.pairs.iter().map(|p| p.key.as_str()).collect();
        assert!(keys.contains(&"kind"), "original kind survives as a pair");
        assert!(keys.contains(&"text"));
        assert!(
            !keys.contains(&"title"),
            "title is a node field, not a pair"
        );
        assert!(!keys.contains(&"props"), "objects don't degrade to pairs");
    }

    #[test]
    fn kv_fallback_json_for_unknown_kind() {
        let raw = json!({
            "kind": "hologram", "id": "h1", "title": "H",
            "intensity": 0.8, "nested": {"x": 1}
        });
        assert_eq!(node_kind_json(&raw), Some("hologram"));
        let kv = kv_fallback_json(&raw);
        assert_eq!(kv["kind"], "kv");
        assert_eq!(kv["id"], "h1");
        assert_eq!(kv["title"], "H");
        let pairs = kv["pairs"].as_array().unwrap();
        // synthesized kind pair + scalar fields; nested object skipped
        assert_eq!(pairs.len(), 2);
        assert_eq!(pairs[0]["key"], "kind");
        assert_eq!(pairs[0]["value"], "hologram");
        assert_eq!(pairs[1]["key"], "intensity");
        // non-object input → empty kv
        let empty = kv_fallback_json(&json!(42));
        assert_eq!(empty["kind"], "kv");
        assert!(empty["pairs"].as_array().unwrap().is_empty());
    }

    #[test]
    fn node_kind_json_absent() {
        assert_eq!(node_kind_json(&json!({})), None);
        assert_eq!(node_kind_json(&json!({"kind": 3})), None);
    }

    #[test]
    fn typed_kv_node_still_parses() {
        // sanity: the fallback output deserializes as KvNode
        let kv: KvNode = serde_json::from_value(kv_fallback_json(&json!({
            "kind": "x", "a": 1
        })))
        .unwrap();
        assert_eq!(kv.pairs.len(), 2); // synthesized kind pair + "a"
    }
}
