//! ViewNode → text / Graphviz-dot / Mermaid renderers.
//!
//! The CLI's `view` verb renders a resolved [`pavo_protocol::ViewResult`]
//! on a terminal: `--format text` walks the node tree and prints a
//! human-readable summary, `--format dot`/`--format mermaid` emit the
//! view's `graph` nodes in those graph description languages.
//!
//! Everything works on the node's **raw JSON** rather than the typed
//! [`ViewNode`] enum so the protocol's forward-compat rule holds
//! end-to-end: a node whose `kind` this host doesn't know degrades to
//! [`kv_fallback_json`] (a generic key/value listing) instead of
//! failing the render. Query-bound data comes from the
//! [`ViewResult::data`](pavo_protocol::wire::ViewResult) map, keyed by
//! the node's `query` field.
//!
//! Format choices (documented, deterministic):
//!
//! - **text** — containers (`panel`/`row`/`column`) print their title
//!   then children indented two spaces per level; `row` does not
//!   attempt side-by-side layout (children stack like `column`).
//! - **dot** — one `digraph` block *per* graph node, concatenated with
//!   blank lines, so graphs never merge ids across views.
//! - **mermaid** — a *single* `graph TD` merging every graph node in
//!   the view; vertex ids are rewritten to `n0`, `n1`, … in first-seen
//!   order so ids from different graphs can't collide.

use std::collections::HashMap;

use pavo_protocol::{KvNode, TreeItem, ViewNode};
use serde_json::{Map, Value};

use pavo_core::{kv_fallback_json, node_kind_json};

/// Render the view tree + bound data as indented plain text.
#[must_use]
pub fn render_text(root: &ViewNode, data: &Map<String, Value>) -> String {
    // ViewNode → JSON can't fail for a well-formed node; on the
    // impossible error we render an empty doc rather than panic.
    let value = serde_json::to_value(root).unwrap_or(Value::Null);
    let mut out = String::new();
    node_text(&value, data, 0, &mut out);
    out
}

/// Render every `graph` node in the view as Graphviz DOT — one
/// `digraph` block per graph node, concatenated.
///
/// # Errors
/// `Err` when the view contains no `graph` nodes.
pub fn render_dot(root: &ViewNode, data: &Map<String, Value>) -> Result<String, String> {
    let graphs = collect_graphs(root, data);
    if graphs.is_empty() {
        return Err("view contains no graph nodes".to_string());
    }
    let mut out = String::new();
    for (i, (nodes, edges)) in graphs.iter().enumerate() {
        if i > 0 {
            out.push('\n');
        }
        out.push_str(&format!("digraph \"pavo-{i}\" {{\n"));
        for node in nodes {
            let id = node.get("id").and_then(Value::as_str).unwrap_or("");
            let label = node.get("label").and_then(Value::as_str).unwrap_or(id);
            out.push_str(&format!(
                "  \"{}\" [label=\"{}\"];\n",
                dot_str(id),
                dot_str(label)
            ));
        }
        for edge in edges {
            let from = edge.get("from").and_then(Value::as_str).unwrap_or("");
            let to = edge.get("to").and_then(Value::as_str).unwrap_or("");
            match edge.get("label").and_then(Value::as_str) {
                Some(label) => out.push_str(&format!(
                    "  \"{}\" -> \"{}\" [label=\"{}\"];\n",
                    dot_str(from),
                    dot_str(to),
                    dot_str(label)
                )),
                None => out.push_str(&format!(
                    "  \"{}\" -> \"{}\";\n",
                    dot_str(from),
                    dot_str(to)
                )),
            }
        }
        out.push_str("}\n");
    }
    Ok(out)
}

/// Render every `graph` node in the view as a single Mermaid
/// `graph TD` — all graphs merged, vertex ids rewritten to `n0`, `n1`,
/// … in first-seen order (nodes first, then edge endpoints).
///
/// # Errors
/// `Err` when the view contains no `graph` nodes.
pub fn render_mermaid(root: &ViewNode, data: &Map<String, Value>) -> Result<String, String> {
    let graphs = collect_graphs(root, data);
    if graphs.is_empty() {
        return Err("view contains no graph nodes".to_string());
    }
    let mut out = String::from("graph TD\n");
    let mut ids: HashMap<String, String> = HashMap::new();
    let mut emitted: std::collections::HashSet<String> = std::collections::HashSet::new();
    let mut next = 0usize;
    // Allocate ids for every vertex up front so node lines precede
    // edges deterministically.
    for (nodes, _) in &graphs {
        for node in nodes {
            if let Some(id) = node.get("id").and_then(Value::as_str) {
                if !ids.contains_key(id) {
                    ids.insert(id.to_string(), format!("n{next}"));
                    next += 1;
                }
            }
        }
    }
    for (nodes, edges) in &graphs {
        for node in nodes {
            let Some(id) = node.get("id").and_then(Value::as_str) else {
                continue;
            };
            let Some(safe) = ids.get(id) else { continue };
            // First-seen label wins — the same raw id across graphs is
            // one merged vertex, declared once.
            if !emitted.insert(id.to_string()) {
                continue;
            }
            let label = node.get("label").and_then(Value::as_str).unwrap_or(id);
            out.push_str(&format!("  {safe}[\"{}\"]\n", mermaid_str(label)));
        }
        for edge in edges {
            let (Some(from), Some(to)) = (
                edge.get("from").and_then(Value::as_str),
                edge.get("to").and_then(Value::as_str),
            ) else {
                continue;
            };
            // Edge endpoints may reference ids never declared as
            // nodes — allocate on demand.
            for endpoint in [from, to] {
                if !ids.contains_key(endpoint) {
                    ids.insert(endpoint.to_string(), format!("n{next}"));
                    next += 1;
                }
            }
            let (Some(a), Some(b)) = (ids.get(from), ids.get(to)) else {
                continue;
            };
            match edge.get("label").and_then(Value::as_str) {
                Some(label) => out.push_str(&format!(
                    "  {a}-->|{}|{b}\n",
                    mermaid_str(label).replace('|', "/")
                )),
                None => out.push_str(&format!("  {a} --> {b}\n")),
            }
        }
    }
    Ok(out)
}

// ---------- text rendering ----------

fn node_text(v: &Value, data: &Map<String, Value>, depth: usize, out: &mut String) {
    let ind = "  ".repeat(depth);
    match node_kind_json(v).unwrap_or("") {
        "panel" | "row" | "column" => {
            if let Some(title) = scalar_str(v.get("title")) {
                out.push_str(&format!("{ind}{title}\n"));
            }
            if let Some(children) = v.get("children").and_then(Value::as_array) {
                for child in children {
                    node_text(child, data, depth + 1, out);
                }
            }
        }
        "table" => table_text(v, data, depth, out),
        "tree" => tree_text(v, data, depth, out),
        "graph" => graph_text(v, data, depth, out),
        "form" => form_text(v, depth, out),
        "kv" => kv_text(v, data, depth, out),
        "markdown" => markdown_text(v, depth, out),
        "embed" => embed_text(v, depth, out),
        // Unknown or missing kind — degrade to KV per the protocol's
        // forward-compat rule (kv_fallback_json preserves scalar
        // fields as pairs and synthesizes a `kind` pair).
        _ => {
            let fallback = kv_fallback_json(v);
            match serde_json::from_value::<KvNode>(fallback) {
                Ok(kv) => kv_node_text(&kv, depth, out),
                Err(_) => out.push_str(&format!("{ind}(unrenderable node)\n")),
            }
        }
    }
}

/// The node's bound query payload, if `query` names a key in `data`.
/// Returns `(query_name, payload)`.
fn bound<'a>(v: &'a Value, data: &'a Map<String, Value>) -> Option<(&'a str, &'a Value)> {
    let query = v.get("query").and_then(Value::as_str)?;
    data.get(query).map(|d| (query, d))
}

/// Print the `{ind}[query 'q' error: …]` line when the bound payload is
/// a degradation marker (`{"error": "…"}`). Returns true when printed.
fn bound_error(v: &Value, data: &Map<String, Value>, ind: &str, out: &mut String) -> bool {
    let Some((query, payload)) = bound(v, data) else {
        return false;
    };
    if let Some(err) = payload.get("error").and_then(Value::as_str) {
        out.push_str(&format!("{ind}[query '{query}' error: {err}]\n"));
        return true;
    }
    false
}

fn title_line(v: &Value, ind: &str, out: &mut String) {
    if let Some(title) = scalar_str(v.get("title")) {
        out.push_str(&format!("{ind}{title}\n"));
    }
}

fn table_text(v: &Value, data: &Map<String, Value>, depth: usize, out: &mut String) {
    let ind = "  ".repeat(depth);
    title_line(v, &ind, out);
    if bound_error(v, data, &ind, out) {
        return;
    }
    // Columns: declared `columns[].{key,title}`; fall back to
    // positional `1..n` when rows exist but no columns are declared.
    let mut columns: Vec<(String, String)> = v
        .get("columns")
        .and_then(Value::as_array)
        .map(|cols| {
            cols.iter()
                .map(|c| {
                    (
                        c.get("key")
                            .and_then(Value::as_str)
                            .unwrap_or("")
                            .to_string(),
                        c.get("title")
                            .and_then(Value::as_str)
                            .unwrap_or_else(|| c.get("key").and_then(Value::as_str).unwrap_or(""))
                            .to_string(),
                    )
                })
                .collect()
        })
        .unwrap_or_default();
    // Align hints, parallel to `columns`.
    let aligns: Vec<String> = v
        .get("columns")
        .and_then(Value::as_array)
        .map(|cols| {
            cols.iter()
                .map(|c| {
                    c.get("align")
                        .and_then(Value::as_str)
                        .unwrap_or("left")
                        .to_string()
                })
                .collect()
        })
        .unwrap_or_default();

    // Rows: inline `rows` win; otherwise the bound query payload —
    // an array of arrays, or an array of objects keyed by column key.
    let mut rows: Vec<Vec<String>> = Vec::new();
    if let Some(inline) = v.get("rows").and_then(Value::as_array) {
        for row in inline {
            if let Some(cells) = row.as_array() {
                rows.push(cells.iter().map(cell_str).collect());
            }
        }
    } else if let Some((_, payload)) = bound(v, data) {
        match payload {
            Value::Array(items) => {
                for item in items {
                    match item {
                        Value::Array(cells) => {
                            rows.push(cells.iter().map(cell_str).collect());
                        }
                        Value::Object(map) => {
                            rows.push(
                                columns
                                    .iter()
                                    .map(|(key, _)| cell_str(map.get(key).unwrap_or(&Value::Null)))
                                    .collect(),
                            );
                        }
                        other => rows.push(vec![cell_str(other)]),
                    }
                }
            }
            other => {
                out.push_str(&format!("{ind}{}\n", cell_str(other)));
                return;
            }
        }
    }
    if columns.is_empty() && !rows.is_empty() {
        let n = rows.iter().map(Vec::len).max().unwrap_or(0);
        columns = (1..=n).map(|i| (i.to_string(), i.to_string())).collect();
    }
    if columns.is_empty() {
        out.push_str(&format!("{ind}(empty table)\n"));
        return;
    }
    // Normalize row lengths to the column count.
    for row in &mut rows {
        row.resize(columns.len(), String::new());
        row.truncate(columns.len());
    }
    // Column widths = max(header, cells) in chars.
    let mut widths: Vec<usize> = columns
        .iter()
        .map(|(_, title)| title.chars().count())
        .collect();
    for row in &rows {
        for (i, cell) in row.iter().enumerate() {
            if i < widths.len() {
                widths[i] = widths[i].max(cell.chars().count());
            }
        }
    }
    let header: Vec<String> = columns
        .iter()
        .enumerate()
        .map(|(i, (_, title))| pad(title, widths[i], align_at(&aligns, i)))
        .collect();
    out.push_str(&format!("{ind}{}\n", header.join("  ")));
    let sep: Vec<String> = widths.iter().map(|w| "-".repeat(*w)).collect();
    out.push_str(&format!("{ind}{}\n", sep.join("  ")));
    for row in &rows {
        let line: Vec<String> = row
            .iter()
            .enumerate()
            .map(|(i, cell)| pad(cell, widths[i], align_at(&aligns, i)))
            .collect();
        out.push_str(&format!("{ind}{}\n", line.join("  ")));
    }
}

fn tree_text(v: &Value, data: &Map<String, Value>, depth: usize, out: &mut String) {
    let ind = "  ".repeat(depth);
    title_line(v, &ind, out);
    if bound_error(v, data, &ind, out) {
        return;
    }
    let mut items: Vec<TreeItem> = Vec::new();
    let inline = v.get("items").and_then(Value::as_array);
    let bound_items = bound(v, data).and_then(|(_, d)| {
        d.get("items")
            .and_then(Value::as_array)
            .or_else(|| d.as_array())
    });
    let source = inline.or(bound_items);
    if let Some(arr) = source {
        for item in arr {
            if let Ok(t) = serde_json::from_value::<TreeItem>(item.clone()) {
                items.push(t);
            }
        }
    }
    if items.is_empty() {
        // Bound payload that isn't an items array — show it raw.
        if let Some((_, payload)) = bound(v, data) {
            if inline.is_none() && !payload.is_array() {
                out.push_str(&format!("{ind}{}\n", cell_str(payload)));
                return;
            }
        }
        out.push_str(&format!("{ind}(empty tree)\n"));
        return;
    }
    for item in &items {
        tree_item_text(item, depth, out);
    }
}

fn tree_item_text(item: &TreeItem, depth: usize, out: &mut String) {
    let ind = "  ".repeat(depth);
    out.push_str(&format!("{ind}- {}", item.label));
    if let Some(detail) = &item.detail {
        out.push_str(&format!("  — {detail}"));
    }
    out.push('\n');
    for child in &item.children {
        tree_item_text(child, depth + 1, out);
    }
}

fn graph_text(v: &Value, data: &Map<String, Value>, depth: usize, out: &mut String) {
    let ind = "  ".repeat(depth);
    title_line(v, &ind, out);
    if bound_error(v, data, &ind, out) {
        return;
    }
    let (nodes, edges) = graph_parts(v, data);
    if edges.is_empty() && nodes.is_empty() {
        out.push_str(&format!("{ind}(empty graph)\n"));
        return;
    }
    // id → label lookup so edges read as names where possible.
    let mut labels: HashMap<&str, &str> = HashMap::new();
    for node in &nodes {
        if let (Some(id), Some(label)) = (
            node.get("id").and_then(Value::as_str),
            node.get("label").and_then(Value::as_str),
        ) {
            labels.insert(id, label);
        }
    }
    let disp = |id: &str| -> String {
        labels
            .get(id)
            .map(|s| (*s).to_string())
            .unwrap_or_else(|| id.to_string())
    };
    if edges.is_empty() {
        for node in &nodes {
            let id = node.get("id").and_then(Value::as_str).unwrap_or("");
            out.push_str(&format!("{ind}- {} ({id})\n", node_label(node, id)));
        }
        return;
    }
    for edge in &edges {
        let from = edge.get("from").and_then(Value::as_str).unwrap_or("");
        let to = edge.get("to").and_then(Value::as_str).unwrap_or("");
        out.push_str(&format!("{ind}{} -> {}", disp(from), disp(to)));
        if let Some(label) = edge.get("label").and_then(Value::as_str) {
            out.push_str(&format!("  ({label})"));
        }
        out.push('\n');
    }
}

fn node_label<'a>(node: &'a Value, id: &'a str) -> &'a str {
    node.get("label").and_then(Value::as_str).unwrap_or(id)
}

fn form_text(v: &Value, depth: usize, out: &mut String) {
    let ind = "  ".repeat(depth);
    title_line(v, &ind, out);
    if let Some(fields) = v.get("fields").and_then(Value::as_array) {
        for f in fields {
            let key = f.get("key").and_then(Value::as_str).unwrap_or("");
            let label = f.get("label").and_then(Value::as_str).unwrap_or(key);
            let kind = f.get("kind").and_then(Value::as_str).unwrap_or("text");
            out.push_str(&format!("{ind}- {key}: {label} [{kind}]"));
            if f.get("required").and_then(Value::as_bool) == Some(true) {
                out.push_str(" (required)");
            }
            if let Some(default) = f.get("default") {
                out.push_str(&format!(" default={}", cell_str(default)));
            }
            if let Some(value) = f.get("value") {
                out.push_str(&format!(" value={}", cell_str(value)));
            }
            if let Some(options) = f.get("options").and_then(Value::as_array) {
                let opts: Vec<String> = options.iter().map(cell_str).collect();
                out.push_str(&format!(" options={}", opts.join("|")));
            }
            if let Some(hint) = f.get("hint").and_then(Value::as_str) {
                out.push_str(&format!(" — {hint}"));
            }
            out.push('\n');
        }
    }
    if let Some(op) = v.get("submit_op").and_then(Value::as_str) {
        let label = v
            .get("submit_label")
            .and_then(Value::as_str)
            .unwrap_or("Submit");
        out.push_str(&format!("{ind}submit → {op} ({label})\n"));
    }
}

fn kv_text(v: &Value, data: &Map<String, Value>, depth: usize, out: &mut String) {
    let ind = "  ".repeat(depth);
    title_line(v, &ind, out);
    if bound_error(v, data, &ind, out) {
        return;
    }
    let mut pairs: Vec<(String, Value)> = v
        .get("pairs")
        .and_then(Value::as_array)
        .map(|arr| {
            arr.iter()
                .filter_map(|p| {
                    Some((
                        p.get("key").and_then(Value::as_str)?.to_string(),
                        p.get("value").cloned().unwrap_or(Value::Null),
                    ))
                })
                .collect()
        })
        .unwrap_or_default();
    // Query-bound kv: an object payload becomes the pairs.
    if pairs.is_empty() {
        if let Some((_, payload)) = bound(v, data) {
            if let Value::Object(map) = payload {
                pairs = map
                    .iter()
                    .map(|(k, val)| (k.clone(), val.clone()))
                    .collect();
            } else {
                out.push_str(&format!("{ind}{}\n", cell_str(payload)));
                return;
            }
        }
    }
    if pairs.is_empty() {
        out.push_str(&format!("{ind}(empty)\n"));
        return;
    }
    kv_node_text(
        &KvNode {
            base: Default::default(),
            pairs: pairs
                .into_iter()
                .map(|(key, value)| pavo_protocol::KvPair {
                    key,
                    value,
                    kind: None,
                })
                .collect(),
        },
        depth,
        out,
    );
}

fn kv_node_text(kv: &KvNode, depth: usize, out: &mut String) {
    let ind = "  ".repeat(depth);
    for pair in &kv.pairs {
        out.push_str(&format!("{ind}{}: {}\n", pair.key, cell_str(&pair.value)));
    }
}

fn markdown_text(v: &Value, depth: usize, out: &mut String) {
    let ind = "  ".repeat(depth);
    title_line(v, &ind, out);
    let text = v.get("text").and_then(Value::as_str).unwrap_or("");
    for line in text.lines() {
        out.push_str(&format!("{ind}{line}\n"));
    }
}

fn embed_text(v: &Value, depth: usize, out: &mut String) {
    let ind = "  ".repeat(depth);
    title_line(v, &ind, out);
    let component = v.get("component").and_then(Value::as_str).unwrap_or("?");
    out.push_str(&format!("{ind}[embed: {component}]"));
    if let Some(props) = v
        .get("props")
        .filter(|p| p.as_object().is_some_and(|m| !m.is_empty()))
    {
        out.push_str(&format!(
            " {}",
            serde_json::to_string(props).unwrap_or_default()
        ));
    }
    out.push('\n');
}

// ---------- shared helpers ----------

/// Collect `(nodes, edges)` value arrays for every `graph` node in
/// the tree, honoring query-bound payloads (`{"nodes":[…],"edges":[…]}`).
fn collect_graphs<'a>(
    root: &'a ViewNode,
    data: &'a Map<String, Value>,
) -> Vec<(Vec<Value>, Vec<Value>)> {
    let value = serde_json::to_value(root).unwrap_or(Value::Null);
    let mut graphs = Vec::new();
    collect_graph_json(&value, data, &mut graphs);
    graphs
}

fn collect_graph_json<'a>(
    v: &'a Value,
    data: &'a Map<String, Value>,
    out: &mut Vec<(Vec<Value>, Vec<Value>)>,
) {
    if node_kind_json(v) == Some("graph") {
        let (nodes, edges) = graph_parts(v, data);
        out.push((nodes, edges));
        return;
    }
    if let Some(children) = v.get("children").and_then(Value::as_array) {
        for child in children {
            collect_graph_json(child, data, out);
        }
    }
}

/// A graph node's effective `(nodes, edges)` — inline fields win,
/// falling back to the bound query payload's `nodes`/`edges` keys.
fn graph_parts(v: &Value, data: &Map<String, Value>) -> (Vec<Value>, Vec<Value>) {
    let mut nodes = v
        .get("nodes")
        .and_then(Value::as_array)
        .cloned()
        .unwrap_or_default();
    let mut edges = v
        .get("edges")
        .and_then(Value::as_array)
        .cloned()
        .unwrap_or_default();
    if let Some((_, payload)) = bound(v, data) {
        if payload.get("error").and_then(Value::as_str).is_none() {
            if let Some(n) = payload.get("nodes").and_then(Value::as_array) {
                if !n.is_empty() {
                    nodes = n.clone();
                }
            }
            if let Some(e) = payload.get("edges").and_then(Value::as_array) {
                if !e.is_empty() {
                    edges = e.clone();
                }
            }
        }
    }
    (nodes, edges)
}

/// Stringify a JSON cell/pair value for terminal output: strings raw,
/// numbers/bools via Display, null empty, anything else compact JSON.
fn cell_str(v: &Value) -> String {
    match v {
        Value::String(s) => s.clone(),
        Value::Null => String::new(),
        Value::Number(n) => n.to_string(),
        Value::Bool(b) => b.to_string(),
        other => serde_json::to_string(other).unwrap_or_default(),
    }
}

fn scalar_str(v: Option<&Value>) -> Option<String> {
    v.and_then(Value::as_str).map(str::to_string)
}

/// Alignment for column `i`: declared `align`, default left.
fn align_at(aligns: &[String], i: usize) -> &str {
    aligns.get(i).map(String::as_str).unwrap_or("left")
}

/// Pad `text` to `width` chars per the alignment hint.
fn pad(text: &str, width: usize, align: &str) -> String {
    let len = text.chars().count();
    if len >= width {
        return text.to_string();
    }
    let fill = width - len;
    match align {
        "right" => format!("{}{text}", " ".repeat(fill)),
        "center" => {
            let left = fill / 2;
            format!("{}{text}{}", " ".repeat(left), " ".repeat(fill - left))
        }
        _ => format!("{text}{}", " ".repeat(fill)),
    }
}

/// Escape a string for a DOT quoted string.
fn dot_str(s: &str) -> String {
    s.replace('\\', "\\\\").replace('"', "\\\"")
}

/// Escape a string for a Mermaid `[…]`/`-->|…|` label.
fn mermaid_str(s: &str) -> String {
    s.replace('"', "#quot;").replace('\n', " ")
}

#[cfg(test)]
#[cfg_attr(coverage, coverage(off))]
mod tests {
    #![allow(clippy::unwrap_used)]

    use super::*;
    use serde_json::json;

    fn data() -> Map<String, Value> {
        let mut m = Map::new();
        m.insert(
            "items".to_string(),
            json!([["T-1", "Alpha", "done"], ["T-2", "Beta", "wip"]]),
        );
        m.insert("counts".to_string(), json!({"open": 1, "done": 1}));
        m.insert(
            "deps".to_string(),
            json!({"nodes": [{"id": "a", "label": "A"}, {"id": "b", "label": "B"}],
                   "edges": [{"from": "a", "to": "b", "label": "uses"}]}),
        );
        m
    }

    fn root(v: Value) -> ViewNode {
        serde_json::from_value(v).unwrap()
    }

    #[test]
    fn text_renders_containers_table_kv_markdown() {
        let node = root(json!({
            "kind": "column", "title": "Board",
            "children": [
                {"kind": "kv", "query": "counts", "pairs": []},
                {"kind": "table", "title": "Items", "query": "items",
                 "columns": [
                    {"key": "0", "title": "ID"},
                    {"key": "1", "title": "Title"},
                    {"key": "2", "title": "Status", "align": "right"}
                 ]},
                {"kind": "markdown", "text": "hello **world**"}
            ]
        }));
        let out = render_text(&node, &data());
        assert!(out.contains("Board"), "{out}");
        assert!(out.contains("open: 1"), "{out}");
        assert!(out.contains("done: 1"), "{out}");
        assert!(out.contains("ID"), "{out}");
        assert!(out.contains("T-1"), "{out}");
        assert!(out.contains("Alpha"), "{out}");
        assert!(out.contains("hello **world**"), "{out}");
        // table separator line present
        assert!(out.contains("---"), "{out}");
    }

    #[test]
    fn text_table_inline_rows_and_object_payload() {
        let node = root(json!({
            "kind": "table",
            "columns": [{"key": "name", "title": "Name"}],
            "rows": [["inline"]]
        }));
        let out = render_text(&node, &data());
        assert!(out.contains("inline"), "{out}");

        // object-array payload keyed by column keys
        let mut d = Map::new();
        d.insert("rows".to_string(), json!([{"name": "obj-row", "extra": 1}]));
        let node2 = root(json!({
            "kind": "table", "query": "rows",
            "columns": [{"key": "name", "title": "Name"}]
        }));
        let out2 = render_text(&node2, &d);
        assert!(out2.contains("obj-row"), "{out2}");
    }

    #[test]
    fn text_tree_graph_form_embed() {
        let node = root(json!({
            "kind": "column",
            "children": [
                {"kind": "tree", "items": [
                    {"label": "root", "detail": "top", "children": [
                        {"label": "child"}
                    ]}
                ]},
                {"kind": "graph", "nodes": [
                    {"id": "a", "label": "Alpha"}, {"id": "b", "label": "Beta"}
                 ], "edges": [{"from": "a", "to": "b", "label": "calls"}]},
                {"kind": "form", "submit_op": "set", "fields": [
                    {"key": "name", "label": "Name", "kind": "text", "required": true}
                ]},
                {"kind": "embed", "component": "timeline", "props": {"x": 1}}
            ]
        }));
        let out = render_text(&node, &data());
        assert!(out.contains("- root  — top"), "{out}");
        assert!(out.contains("- child"), "{out}");
        assert!(out.contains("Alpha -> Beta"), "{out}");
        assert!(out.contains("(calls)"), "{out}");
        assert!(out.contains("name: Name [text] (required)"), "{out}");
        assert!(out.contains("submit → set"), "{out}");
        assert!(out.contains("[embed: timeline]"), "{out}");
    }

    #[test]
    fn text_unknown_kind_degrades_to_kv() {
        // An unknown kind can't deserialize as ViewNode — exercise the
        // JSON path directly via a container holding it is impossible,
        // so test the fallback helper on the raw value.
        let raw = json!({"kind": "hologram", "id": "h1", "intensity": 0.8});
        let fallback = kv_fallback_json(&raw);
        let kv: KvNode = serde_json::from_value(fallback).unwrap();
        let mut out = String::new();
        kv_node_text(&kv, 0, &mut out);
        assert!(out.contains("kind: hologram"), "{out}");
        assert!(out.contains("intensity: 0.8"), "{out}");
    }

    #[test]
    fn text_bound_query_error_payload() {
        let mut d = Map::new();
        d.insert("items".to_string(), json!({"error": "boom"}));
        let node = root(json!({"kind": "table", "query": "items",
            "columns": [{"key": "a", "title": "A"}]}));
        let out = render_text(&node, &d);
        assert!(out.contains("[query 'items' error: boom]"), "{out}");
    }

    #[test]
    fn dot_renders_each_graph() {
        let node = root(json!({
            "kind": "column",
            "children": [
                {"kind": "graph", "nodes": [
                    {"id": "a", "label": "Alpha"}, {"id": "b", "label": "B\"eta"}
                 ], "edges": [{"from": "a", "to": "b", "label": "x"}]},
                {"kind": "panel", "children": [
                    {"kind": "graph", "query": "deps", "nodes": [], "edges": []}
                ]}
            ]
        }));
        let out = render_dot(&node, &data()).unwrap();
        assert_eq!(out.matches("digraph").count(), 2, "{out}");
        assert!(out.contains("digraph \"pavo-0\""), "{out}");
        assert!(out.contains("digraph \"pavo-1\""), "{out}");
        assert!(out.contains("B\\\"eta"), "{out}");
        // second graph uses bound data
        assert!(out.contains("\"a\" -> \"b\" [label=\"uses\"]"), "{out}");
    }

    #[test]
    fn dot_and_mermaid_error_without_graphs() {
        let node = root(json!({"kind": "kv", "pairs": []}));
        assert!(render_dot(&node, &data()).is_err());
        assert!(render_mermaid(&node, &data()).is_err());
    }

    #[test]
    fn mermaid_merges_graphs_with_safe_ids() {
        let node = root(json!({
            "kind": "column",
            "children": [
                {"kind": "graph", "nodes": [
                    {"id": "a b", "label": "A"}, {"id": "c", "label": "C"}
                 ], "edges": [{"from": "a b", "to": "c"}]},
                {"kind": "graph", "nodes": [
                    {"id": "a b", "label": "dup"}, {"id": "d", "label": "D"}
                 ], "edges": [{"from": "c", "to": "d", "label": "go"}]}
            ]
        }));
        let out = render_mermaid(&node, &data()).unwrap();
        assert!(out.starts_with("graph TD\n"), "{out}");
        // ids rewritten — raw "a b" never appears as an id
        assert!(out.contains("n0[\"A\"]"), "{out}");
        assert!(out.contains("n0 --> n1"), "{out}");
        // second graph reuses the same safe id for "a b" (first-seen);
        // its node line is emitted once with the first label
        assert_eq!(out.matches("n0[").count(), 1, "{out}");
        assert!(!out.contains("dup"), "{out}");
        assert!(out.contains("-->|go|"), "{out}");
        assert!(!out.contains("a b["), "{out}");
    }
}
