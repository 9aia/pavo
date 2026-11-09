//! ViewNode → flat render model for the ratatui canvas.
//!
//! [`build_view`] walks a resolved view's node tree (as raw JSON, same
//! as `pavo-cli`'s `render.rs` — the protocol's forward-compat rule
//! holds end-to-end: an unknown `kind` degrades through
//! [`kv_fallback_json`] to a key/value listing, never an error) and
//! produces plain rows the renderer styles. Selectable rows carry an
//! index into [`Model::selections`]; the detail pane renders the
//! matching [`Selection`].
//!
//! Traversal semantics deliberately mirror `pavo-cli/src/render.rs`:
//! containers (`panel`/`row`/`column`) print their title then children
//! indented two spaces per level (`row` does not attempt side-by-side
//! layout), inline `rows`/`items`/`pairs` win over query-bound data,
//! and a bound payload of `{"error": "…"}` renders as an error row
//! instead of data.
//!
//! [`build_dashboard`] and [`build_note`] synthesize models for the
//! chrome pseudo-views (contribution listing, error/info panes) that
//! aren't driver views at all.

use std::collections::HashMap;

use pavo_core::{kv_fallback_json, node_kind_json, Chrome, ViewEntry};
use pavo_protocol::{KvNode, RenderModel, TreeItem, ViewNode};
use serde_json::{Map, Value};

/// Visual weight of a canvas row.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum RowStyle {
    /// Body content.
    Normal,
    /// Column headers / emphasized structure.
    Header,
    /// Secondary text — separators, hints, edge lists.
    Dim,
    /// Errors — failed query bindings, resolve failures.
    Error,
    /// Node/panel titles.
    Title,
}

/// One display line of the canvas.
#[derive(Clone, Debug)]
pub struct Row {
    /// Pre-rendered text (indentation + glyphs already applied).
    pub text: String,
    /// Base style; the renderer may override for the selected row.
    pub style: RowStyle,
    /// Index into [`Model::selections`] when this row is selectable.
    pub sel: Option<usize>,
}

/// Detail-pane content for one selectable row — title plus ordered
/// `key → value` fields (empty key = plain line). Values may contain
/// `\n`; the pane splits them.
#[derive(Clone, Debug, Default)]
pub struct Selection {
    /// Heading — what was selected (view title, row id, node label…).
    pub title: String,
    /// Ordered detail fields.
    pub fields: Vec<(String, String)>,
}

/// The flat canvas: rows to draw plus the selection table.
#[derive(Clone, Debug, Default)]
pub struct Model {
    /// Display rows, top to bottom.
    pub rows: Vec<Row>,
    /// Detail content per selectable row (indexed by `Row::sel`).
    pub selections: Vec<Selection>,
    /// Indices into `rows` that are selectable, in display order —
    /// j/k navigation walks this list.
    pub selectables: Vec<usize>,
    /// Every `graph` node's DOT dump, concatenated — `g` writes this
    /// to a file. `None` when the view has no graphs.
    pub graph_dot: Option<String>,
    /// Interactive forms in the view (document order) — forms with a
    /// `submit_op`. The app opens an edit session from
    /// [`Model::form_marks`].
    pub forms: Vec<FormSpec>,
    /// Rows that belong to an interactive form: field rows
    /// (`field: Some`) and the submit row (`field: None`). Enter on a
    /// marked row opens the form editor instead of the detail pane.
    pub form_marks: Vec<FormMark>,
}

/// One field of an interactive [`FormSpec`] — everything the editor
/// needs to render and submit it.
#[derive(Clone, Debug)]
pub struct FormFieldSpec {
    /// Field key — the `values` map key on submit.
    pub key: String,
    /// Human-facing label.
    pub label: String,
    /// Field kind string (`text`, `textarea`, `number`, `bool`,
    /// `select`, or a driver extension — unknown kinds edit as text).
    pub kind: String,
    /// Must be non-empty on submit.
    pub required: bool,
    /// `editable:false` renders read-only and is never submitted.
    pub editable: bool,
    /// Options for `select` fields.
    pub options: Vec<String>,
    /// Default value (JSON).
    pub default: Option<Value>,
    /// Current value when the form renders existing state.
    pub value: Option<Value>,
    /// Help text.
    pub hint: Option<String>,
    /// Placeholder shown while the field is empty.
    pub placeholder: Option<String>,
}

/// An interactive form: `submit_op` present, fields declared. `id`
/// is the node's `NodeBase.id` — the wire `submit` op's optional
/// `form` param.
#[derive(Clone, Debug)]
pub struct FormSpec {
    /// Form node id (`NodeBase.id`), when the driver set one.
    pub id: Option<String>,
    /// Form title (node `title`, else `"form"`).
    pub title: String,
    /// Driver-side action (`"set"`, `"add"`, …) — display only; the
    /// wire op is always `submit`.
    pub submit_op: String,
    /// Submit button label (default `"Submit"`).
    pub submit_label: String,
    /// Field declarations in display order.
    pub fields: Vec<FormFieldSpec>,
}

/// Binds a canvas row to a spot in [`Model::forms`].
#[derive(Clone, Copy, Debug)]
pub struct FormMark {
    /// Index into [`Model::rows`].
    pub row: usize,
    /// Index into [`Model::forms`].
    pub form: usize,
    /// Index into [`FormSpec::fields`]; `None` marks the form's
    /// submit row (opens the editor at the first editable field).
    pub field: Option<usize>,
}

/// Build the canvas model for a resolved view. `width` is the canvas
/// width in columns — tables size their columns to fit (truncating
/// cells with `…` when they can't). Convenience wrapper — no plugin
/// renders; the app goes through [`build_view_with_plugins`].
#[must_use]
#[allow(dead_code)]
pub fn build_view(root: &ViewNode, data: &Map<String, Value>, width: usize) -> Model {
    build_view_with_plugins(root, data, width, &HashMap::new())
}

/// [`build_view`] with renderer-plugin substitution: nodes whose tree
/// path (child indices joined by `.`, root = `""`) has a
/// [`RenderModel`] in `renders` — produced by a driver `render` op for
/// surface `"tui"` — emit the plugin's output instead of the built-in
/// interpretation. Paths come from
/// [`pavo_core::renderers::plugin_nodes`].
#[must_use]
pub fn build_view_with_plugins(
    root: &ViewNode,
    data: &Map<String, Value>,
    width: usize,
    renders: &HashMap<String, RenderModel>,
) -> Model {
    // ViewNode → JSON can't fail for a well-formed node; on the
    // impossible error we render an empty doc rather than panic.
    let value = serde_json::to_value(root).unwrap_or(Value::Null);
    let mut b = Builder::default();
    node_json_path(&value, data, 0, width, String::new(), renders, &mut b);
    b.finish()
}

/// Build the dashboard pseudo-view: every view every driver offers,
/// plus the merged chrome contributions (dashboard/navbar/statusbar/
/// routes/sidebar/headerActions/notifications/jobs), grouped under
/// dim section headers. Every entry is selectable — the app resolves
/// view targets on Enter.
///
/// The platform slots (`sidebar`, `header_actions`, `notifications`,
/// `jobs`) get no dedicated TUI chrome — there is no sidebar pane, no
/// header action bar, and the feed ops (`host.jobs()` /
/// `host.notifications()`) aren't fetched on the worker — so they
/// render here as contribution listings like every other slot. Feed
/// *data* lives behind the `pavo jobs`/`pavo notifications` CLI verbs
/// (and `/api/jobs`, `/api/notifications` on the web bridge).
#[must_use]
pub fn build_dashboard(
    views: &[ViewEntry],
    chrome: &Chrome,
    route_target: &dyn Fn(&str) -> Option<String>,
    width: usize,
) -> Model {
    let _ = width; // no tables here — nothing to fit
    let mut b = Builder::default();
    b.row(
        "dashboard — jump to any view or contribution",
        RowStyle::Title,
    );

    b.row("views", RowStyle::Dim);
    for v in views {
        let text = format!("{}:{} — {}", v.driver, v.id, v.title);
        let mut fields = vec![
            ("view".to_string(), v.namespaced.clone()),
            ("driver".to_string(), v.driver.clone()),
            ("title".to_string(), v.title.clone()),
            ("route".to_string(), v.route.clone()),
        ];
        if let Some(d) = &v.description {
            fields.push(("description".to_string(), d.clone()));
        }
        if !v.queries.is_empty() {
            fields.push(("queries".to_string(), v.queries.join(", ")));
        }
        b.sel_row(&text, RowStyle::Normal, v.namespaced.clone(), fields);
    }
    if views.is_empty() {
        b.row("  (no views discovered)", RowStyle::Dim);
    }

    section(
        &mut b,
        "dashboard contributions",
        &chrome.dashboard,
        route_target,
    );
    section(&mut b, "navbar", &chrome.navbar, route_target);
    section(&mut b, "statusbar", &chrome.statusbar, route_target);
    section(&mut b, "routes", &chrome.routes, route_target);
    section(&mut b, "sidebar", &chrome.sidebar, route_target);
    section(
        &mut b,
        "header actions",
        &chrome.header_actions,
        route_target,
    );
    // Provider *metadata* only — the feeds themselves are wire ops
    // the TUI worker doesn't call (see the fn docs).
    section(&mut b, "notifications", &chrome.notifications, route_target);
    section(&mut b, "jobs", &chrome.jobs, route_target);
    b.finish()
}

/// A titled key/value note — error panes, route-info panes, the empty
/// state. Every field is a selectable row.
#[must_use]
pub fn build_note(title: &str, fields: &[(String, String)], error: bool) -> Model {
    let mut b = Builder::default();
    b.row(
        title,
        if error {
            RowStyle::Error
        } else {
            RowStyle::Title
        },
    );
    for (k, v) in fields {
        let text = if k.is_empty() {
            v.clone()
        } else {
            format!("{k}: {v}")
        };
        b.sel_row(
            &text,
            if error {
                RowStyle::Error
            } else {
                RowStyle::Normal
            },
            title.to_string(),
            vec![(k.clone(), v.clone())],
        );
    }
    b.finish()
}

/// Loading placeholder.
#[must_use]
pub fn build_loading(target: &str) -> Model {
    let mut b = Builder::default();
    b.row(&format!("loading {target}…"), RowStyle::Dim);
    b.finish()
}

// ---------- builder ----------

#[derive(Default)]
struct Builder {
    rows: Vec<Row>,
    selections: Vec<Selection>,
    dots: Vec<String>,
    forms: Vec<FormSpec>,
    form_marks: Vec<FormMark>,
}

impl Builder {
    fn row(&mut self, text: &str, style: RowStyle) {
        self.rows.push(Row {
            text: text.to_string(),
            style,
            sel: None,
        });
    }

    /// Push a selectable row; returns nothing — `sel` indexes
    /// `selections`.
    fn sel_row(
        &mut self,
        text: &str,
        style: RowStyle,
        title: String,
        fields: Vec<(String, String)>,
    ) {
        let sel = self.selections.len();
        self.selections.push(Selection { title, fields });
        self.rows.push(Row {
            text: text.to_string(),
            style,
            sel: Some(sel),
        });
    }

    /// Mark the most recently pushed row as belonging to an
    /// interactive form (`field: None` = the submit row).
    fn mark_form(&mut self, form: usize, field: Option<usize>) {
        if let Some(row) = self.rows.len().checked_sub(1) {
            self.form_marks.push(FormMark { row, form, field });
        }
    }

    fn finish(self) -> Model {
        let selectables = self
            .rows
            .iter()
            .enumerate()
            .filter_map(|(i, r)| r.sel.map(|_| i))
            .collect();
        Model {
            rows: self.rows,
            selections: self.selections,
            selectables,
            graph_dot: if self.dots.is_empty() {
                None
            } else {
                Some(self.dots.join("\n"))
            },
            forms: self.forms,
            form_marks: self.form_marks,
        }
    }
}

fn section(
    b: &mut Builder,
    name: &str,
    entries: &[pavo_core::SlotEntry],
    route_target: &dyn Fn(&str) -> Option<String>,
) {
    if entries.is_empty() {
        return;
    }
    b.row(name, RowStyle::Dim);
    for e in entries {
        let c = &e.contribution;
        let target = route_target(&c.route);
        let text = format!("  {} — {} ({})", c.title, c.route, e.driver);
        let mut fields = vec![
            ("title".to_string(), c.title.clone()),
            ("route".to_string(), c.route.clone()),
            ("driver".to_string(), e.driver.clone()),
            ("id".to_string(), c.id.clone()),
        ];
        if let Some(t) = &target {
            fields.push(("resolves to".to_string(), t.clone()));
        } else {
            fields.push(("note".to_string(), "driver-owned route".to_string()));
        }
        // Selection title doubles as the jump target when resolvable.
        b.sel_row(
            &text,
            RowStyle::Normal,
            target.unwrap_or_else(|| c.title.clone()),
            fields,
        );
    }
}

// ---------- node dispatch (mirrors pavo-cli render.rs) ----------

/// The node walker with a tree `path` (plugin-render lookup key) and
/// the plugin `renders` map — a claimed node emits its
/// [`RenderModel`] instead of the built-in dispatch.
fn node_json_path(
    v: &Value,
    data: &Map<String, Value>,
    depth: usize,
    width: usize,
    path: String,
    renders: &HashMap<String, RenderModel>,
    b: &mut Builder,
) {
    let ind = "  ".repeat(depth);
    if let Some(model) = renders.get(&path) {
        render_model_rows(model, depth, width, b);
        return;
    }
    match node_kind_json(v).unwrap_or("") {
        "panel" | "row" | "column" => {
            if let Some(title) = scalar_str(v.get("title")) {
                b.row(&format!("{ind}{title}"), RowStyle::Title);
            }
            if let Some(children) = v.get("children").and_then(Value::as_array) {
                for (i, child) in children.iter().enumerate() {
                    let child_path = if path.is_empty() {
                        i.to_string()
                    } else {
                        format!("{path}.{i}")
                    };
                    node_json_path(child, data, depth + 1, width, child_path, renders, b);
                }
            }
        }
        "table" => table_json(v, data, depth, width, b),
        "tree" => tree_json(v, data, depth, b),
        "graph" => graph_json(v, data, depth, b),
        "form" => form_json(v, depth, b),
        "kv" => kv_json(v, data, depth, b),
        "markdown" => markdown_json(v, depth, b),
        "embed" => embed_json(v, depth, b),
        // Unknown or missing kind — degrade to KV per the protocol's
        // forward-compat rule.
        _ => {
            let fallback = kv_fallback_json(v);
            match serde_json::from_value::<KvNode>(fallback) {
                Ok(kv) => kv_node(&kv, depth, b),
                Err(_) => b.row(&format!("{ind}(unrenderable node)"), RowStyle::Dim),
            }
        }
    }
}

/// The node's bound query payload, if `query` names a key in `data`.
fn bound<'a>(v: &'a Value, data: &'a Map<String, Value>) -> Option<(&'a str, &'a Value)> {
    let query = v.get("query").and_then(Value::as_str)?;
    data.get(query).map(|d| (query, d))
}

/// Emit the `[query 'q' error: …]` row when the bound payload is a
/// degradation marker. Returns true when emitted (caller returns).
fn bound_error(v: &Value, data: &Map<String, Value>, ind: &str, b: &mut Builder) -> bool {
    let Some((query, payload)) = bound(v, data) else {
        return false;
    };
    if let Some(err) = payload.get("error").and_then(Value::as_str) {
        b.row(
            &format!("{ind}[query '{query}' error: {err}]"),
            RowStyle::Error,
        );
        return true;
    }
    false
}

fn title_row(v: &Value, ind: &str, b: &mut Builder) {
    if let Some(title) = scalar_str(v.get("title")) {
        b.row(&format!("{ind}{title}"), RowStyle::Title);
    }
}

// ---------- table ----------

struct Col {
    key: String,
    title: String,
    align: String,
}

fn table_json(v: &Value, data: &Map<String, Value>, depth: usize, width: usize, b: &mut Builder) {
    let ind = "  ".repeat(depth);
    title_row(v, &ind, b);
    if bound_error(v, data, &ind, b) {
        return;
    }
    let mut columns: Vec<Col> = v
        .get("columns")
        .and_then(Value::as_array)
        .map(|cols| {
            cols.iter()
                .map(|c| Col {
                    key: c
                        .get("key")
                        .and_then(Value::as_str)
                        .unwrap_or("")
                        .to_string(),
                    title: c
                        .get("title")
                        .and_then(Value::as_str)
                        .or_else(|| c.get("key").and_then(Value::as_str))
                        .unwrap_or("")
                        .to_string(),
                    align: c
                        .get("align")
                        .and_then(Value::as_str)
                        .unwrap_or("left")
                        .to_string(),
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
                                    .map(|c| cell_str(map.get(&c.key).unwrap_or(&Value::Null)))
                                    .collect(),
                            );
                        }
                        other => rows.push(vec![cell_str(other)]),
                    }
                }
            }
            other => {
                b.row(&format!("{ind}{}", cell_str(other)), RowStyle::Dim);
                return;
            }
        }
    }
    if columns.is_empty() && !rows.is_empty() {
        let n = rows.iter().map(Vec::len).max().unwrap_or(0);
        columns = (1..=n)
            .map(|i| Col {
                key: i.to_string(),
                title: i.to_string(),
                align: "left".to_string(),
            })
            .collect();
    }
    if columns.is_empty() {
        b.row(&format!("{ind}(empty table)"), RowStyle::Dim);
        return;
    }
    for row in &mut rows {
        row.resize(columns.len(), String::new());
        row.truncate(columns.len());
    }

    // Column widths = max(header, cells); shrink the widest until the
    // row fits `width` (cells truncate with '…' at emit time).
    let avail = width.saturating_sub(depth * 2).max(columns.len() * 4);
    let sep = 2usize;
    let mut widths: Vec<usize> = columns.iter().map(|c| c.title.chars().count()).collect();
    for row in &rows {
        for (i, cell) in row.iter().enumerate() {
            if i < widths.len() {
                widths[i] = widths[i].max(cell.chars().count());
            }
        }
    }
    loop {
        let total: usize = widths.iter().sum::<usize>() + sep * (widths.len() - 1);
        if total <= avail {
            break;
        }
        // Shrink the widest column that can still shrink.
        let Some((idx, _)) = widths
            .iter()
            .enumerate()
            .filter(|(_, w)| **w > 3)
            .max_by_key(|(_, w)| *w)
        else {
            break;
        };
        widths[idx] -= 1;
    }

    let mut emit = |cells: &[String], style: RowStyle, sel: Option<Selection>| {
        let parts: Vec<String> = cells
            .iter()
            .enumerate()
            .map(|(i, cell)| fit(cell, widths[i], &columns[i].align))
            .collect();
        let text = format!("{ind}{}", parts.join(&" ".repeat(sep)));
        if let Some(sel) = sel {
            let sel_idx = b.selections.len();
            b.selections.push(sel);
            b.rows.push(Row {
                text,
                style,
                sel: Some(sel_idx),
            });
        } else {
            b.row(&text, style);
        }
    };

    let header_cells: Vec<String> = columns.iter().map(|c| c.title.clone()).collect();
    emit(&header_cells, RowStyle::Header, None);
    let sep_cells: Vec<String> = widths.iter().map(|w| "-".repeat(*w)).collect();
    emit(&sep_cells, RowStyle::Dim, None);
    let col_meta: Vec<(String, String)> = columns
        .iter()
        .map(|c| (c.title.clone(), c.key.clone()))
        .collect();
    for row in &rows {
        let fields: Vec<(String, String)> = col_meta
            .iter()
            .enumerate()
            .map(|(i, (title, _))| (title.clone(), row.get(i).cloned().unwrap_or_default()))
            .collect();
        let node_title = v
            .get("title")
            .and_then(Value::as_str)
            .unwrap_or("table")
            .to_string();
        emit(
            row,
            RowStyle::Normal,
            Some(Selection {
                title: format!("{node_title} row"),
                fields,
            }),
        );
    }
}

// ---------- tree ----------

fn tree_json(v: &Value, data: &Map<String, Value>, depth: usize, b: &mut Builder) {
    let ind = "  ".repeat(depth);
    title_row(v, &ind, b);
    if bound_error(v, data, &ind, b) {
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
        if let Some((_, payload)) = bound(v, data) {
            if inline.is_none() && !payload.is_array() {
                b.row(&format!("{ind}{}", cell_str(payload)), RowStyle::Dim);
                return;
            }
        }
        b.row(&format!("{ind}(empty tree)"), RowStyle::Dim);
        return;
    }
    let prefix = format!("{ind}  ");
    tree_items(&items, &prefix, b);
}

fn tree_items(items: &[TreeItem], prefix: &str, b: &mut Builder) {
    for (i, item) in items.iter().enumerate() {
        let last = i + 1 == items.len();
        let conn = if last { "└─ " } else { "├─ " };
        let mut text = format!("{prefix}{conn}{}", item.label);
        if let Some(detail) = &item.detail {
            text.push_str(&format!("  — {detail}"));
        }
        let mut fields = vec![("label".to_string(), item.label.clone())];
        if let Some(d) = &item.detail {
            fields.push(("detail".to_string(), d.clone()));
        }
        if let Some(id) = &item.id {
            fields.push(("id".to_string(), id.clone()));
        }
        if let Some(icon) = &item.icon {
            fields.push(("icon".to_string(), icon.clone()));
        }
        if !item.children.is_empty() {
            fields.push(("children".to_string(), item.children.len().to_string()));
        }
        if !item.props.is_empty() {
            fields.push((
                "props".to_string(),
                pretty(&Value::Object(item.props.clone())),
            ));
        }
        b.sel_row(&text, RowStyle::Normal, item.label.clone(), fields);
        let child_prefix = format!("{prefix}{}", if last { "   " } else { "│  " });
        tree_items(&item.children, &child_prefix, b);
    }
}

// ---------- graph ----------

fn graph_json(v: &Value, data: &Map<String, Value>, depth: usize, b: &mut Builder) {
    let ind = "  ".repeat(depth);
    title_row(v, &ind, b);
    if bound_error(v, data, &ind, b) {
        return;
    }
    let (nodes, edges) = graph_parts(v, data);
    if edges.is_empty() && nodes.is_empty() {
        b.row(&format!("{ind}(empty graph)"), RowStyle::Dim);
        return;
    }
    // id → label lookup so edges read as names where possible.
    let mut labels: Map<String, Value> = Map::new();
    for node in &nodes {
        if let (Some(id), Some(label)) = (
            node.get("id").and_then(Value::as_str),
            node.get("label").and_then(Value::as_str),
        ) {
            labels.insert(id.to_string(), Value::from(label));
        }
    }
    let disp = |id: &str| -> String {
        labels
            .get(id)
            .and_then(Value::as_str)
            .map(str::to_string)
            .unwrap_or_else(|| id.to_string())
    };

    // Simple deterministic layered layout (longest-path layering +
    // barycenter ordering) — edges between adjacent layers draw as
    // box-drawing connectors; longer edges stay in the edge list.
    let layout = ascii_layout(&nodes, &edges);
    if !layout.is_empty() {
        b.row(&format!("{ind}layout"), RowStyle::Dim);
        for line in layout {
            b.row(&format!("{ind}  {line}"), RowStyle::Normal);
        }
    }

    b.row(&format!("{ind}nodes"), RowStyle::Dim);
    for node in &nodes {
        let id = node.get("id").and_then(Value::as_str).unwrap_or("");
        let label = node.get("label").and_then(Value::as_str).unwrap_or(id);
        let kind = node.get("kind").and_then(Value::as_str);
        let mut text = format!("{ind}  {label}");
        if let Some(k) = kind {
            text.push_str(&format!(" [{k}]"));
        }
        if !id.is_empty() {
            text.push_str(&format!(" ({id})"));
        }
        let mut fields = vec![
            ("id".to_string(), id.to_string()),
            ("label".to_string(), label.to_string()),
        ];
        if let Some(k) = kind {
            fields.push(("kind".to_string(), k.to_string()));
        }
        if let Some(props) = node
            .get("props")
            .filter(|p| !p.as_object().is_some_and(Map::is_empty))
        {
            fields.push(("props".to_string(), pretty(props)));
        }
        b.sel_row(&text, RowStyle::Normal, label.to_string(), fields);
    }

    b.row(&format!("{ind}edges"), RowStyle::Dim);
    for edge in &edges {
        let from = edge.get("from").and_then(Value::as_str).unwrap_or("");
        let to = edge.get("to").and_then(Value::as_str).unwrap_or("");
        let mut text = format!("{ind}  {} -> {}", disp(from), disp(to));
        if let Some(label) = edge.get("label").and_then(Value::as_str) {
            text.push_str(&format!("  ({label})"));
        }
        let fields = vec![
            ("from".to_string(), disp(from)),
            ("to".to_string(), disp(to)),
            (
                "label".to_string(),
                edge.get("label")
                    .and_then(Value::as_str)
                    .unwrap_or("")
                    .to_string(),
            ),
        ];
        b.sel_row(&text, RowStyle::Dim, format!("{from} -> {to}"), fields);
    }

    // DOT dump for the 'g' key — same shape as pavo-cli render_dot.
    b.dots.push(dot_block(&nodes, &edges));
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

// ---------- graph ASCII layout ----------

/// Render `(nodes, edges)` as a layered ASCII diagram: longest-path
/// layering, one barycenter ordering sweep per direction, node boxes
/// `┌─┐│x│└─┘`, and `│ ─ ┌ ┐ └ ┘` connectors for edges between
/// adjacent layers. Edges spanning multiple layers (or cycles) are
/// not drawn — they remain in the edge list below. Deterministic,
/// allocation-light; fine for dozens of nodes at 60fps.
fn ascii_layout(nodes: &[Value], edges: &[Value]) -> Vec<String> {
    /// One grid cell.
    #[derive(Clone, Copy)]
    struct Cell(char);

    let ids: Vec<String> = nodes
        .iter()
        .map(|n| {
            n.get("id")
                .and_then(Value::as_str)
                .unwrap_or("")
                .to_string()
        })
        .collect();
    let index: HashMap<String, usize> = ids
        .iter()
        .enumerate()
        .map(|(i, id)| (id.clone(), i))
        .collect();
    let labels: Vec<String> = nodes
        .iter()
        .enumerate()
        .map(|(i, n)| {
            n.get("label")
                .and_then(Value::as_str)
                .filter(|l| !l.is_empty())
                .unwrap_or(ids[i].as_str())
                .to_string()
        })
        .collect();
    // (from_idx, to_idx) — unknown endpoints dropped.
    let links: Vec<(usize, usize)> = edges
        .iter()
        .filter_map(|e| {
            let f = e.get("from").and_then(Value::as_str)?;
            let t = e.get("to").and_then(Value::as_str)?;
            Some((*index.get(f)?, *index.get(t)?))
        })
        .collect();
    let n = ids.len();
    if n == 0 {
        return Vec::new();
    }

    // Longest-path layering, bounded relaxation (cycle-safe).
    let mut layer = vec![0usize; n];
    for _ in 0..n {
        let mut changed = false;
        for &(f, t) in &links {
            let want = layer[f] + 1;
            if want > layer[t] {
                layer[t] = want;
                changed = true;
            }
        }
        if !changed {
            break;
        }
    }
    let depth = layer.iter().copied().max().unwrap_or(0);

    // Group node indices per layer (stable, input order).
    let mut by_layer: Vec<Vec<usize>> = vec![Vec::new(); depth + 1];
    for (i, &l) in layer.iter().enumerate() {
        by_layer[l].push(i);
    }

    // Barycenter ordering: a few top-down + bottom-up sweeps sorting
    // each layer by the mean position of its already-placed neighbors.
    let mut pos = vec![0usize; n]; // index within its layer
    let refresh_pos = |by_layer: &[Vec<usize>], pos: &mut [usize]| {
        for layer_nodes in by_layer {
            for (i, &node) in layer_nodes.iter().enumerate() {
                pos[node] = i;
            }
        }
    };
    refresh_pos(&by_layer, &mut pos);
    for sweep in 0..4 {
        let forward = sweep % 2 == 0;
        let order: Vec<usize> = if forward {
            (1..=depth).collect()
        } else {
            (0..depth).rev().collect()
        };
        for li in order {
            // Neighbor positions in adjacent layers (already placed
            // this sweep): predecessors on the way down, successors
            // on the way up.
            let want_preds = forward;
            by_layer[li].sort_by_key(|&node| {
                // Mean position of adjacent-layer neighbors already
                // placed this sweep (predecessors going down,
                // successors going up); unconnected nodes keep place.
                let mut sum = 0usize;
                let mut cnt = 0usize;
                for &(f, t) in &links {
                    let other = if want_preds {
                        (t == node).then_some(f)
                    } else {
                        (f == node).then_some(t)
                    };
                    if let Some(o) = other {
                        if layer[o].abs_diff(layer[node]) == 1 {
                            sum += pos[o];
                            cnt += 1;
                        }
                    }
                }
                sum.checked_div(cnt).unwrap_or(pos[node])
            });
            refresh_pos(&by_layer, &mut pos);
        }
    }

    // Geometry: per-layer column pitch, per-node box width, fixed
    // 4-row pitch (3 box rows + 1 connector gap).
    const BOX_H: usize = 3;
    const PITCH_Y: usize = 4;
    let widths: Vec<usize> = labels
        .iter()
        .map(|l| l.chars().count().max(1) + 4)
        .collect();
    let layer_pitch: Vec<usize> = by_layer
        .iter()
        .map(|ns| ns.iter().map(|&i| widths[i]).max().unwrap_or(0) + 3)
        .collect();
    // A layer's width is its node count times the per-node pitch.
    let layer_w: Vec<usize> = by_layer
        .iter()
        .enumerate()
        .map(|(li, ns)| ns.len() * layer_pitch[li])
        .collect();
    let mut layer_x = vec![0usize; depth + 1];
    for li in 1..=depth {
        layer_x[li] = layer_x[li - 1] + layer_w[li - 1];
    }
    let total_w = layer_x[depth] + layer_w[depth];
    let rows_n = depth * PITCH_Y + BOX_H;

    // Node boxes.
    let mut grid = vec![vec![Cell(' '); total_w]; rows_n];
    let mut center = vec![(0usize, 0usize); n]; // (x_center, y_top)
    for (li, ns) in by_layer.iter().enumerate() {
        for (slot, &node) in ns.iter().enumerate() {
            let w = widths[node];
            let x = layer_x[li] + slot * layer_pitch[li] + layer_pitch[li].saturating_sub(w) / 2;
            let y = li * PITCH_Y;
            center[node] = (x + w / 2, y);
            let label = &labels[node];
            let label_len = label.chars().count();
            let pad = w.saturating_sub(2 + label_len);
            let left = pad / 2;
            let right = pad - left;
            let top: String = format!("┌{}┐", "─".repeat(w.saturating_sub(2)));
            let mid = format!("│{}{}{}│", " ".repeat(left), label, " ".repeat(right));
            let bot: String = format!("└{}┘", "─".repeat(w.saturating_sub(2)));
            for (dy, line) in [top, mid, bot].iter().enumerate() {
                for (dx, ch) in line.chars().enumerate() {
                    if x + dx < total_w && y + dy < rows_n {
                        grid[y + dy][x + dx] = Cell(ch);
                    }
                }
            }
        }
    }

    // Edges between adjacent layers only. Two passes — horizontal
    // runs first, then stubs — so later edges don't erase earlier
    // corners on a shared gap row.
    let mut drawn: Vec<(usize, usize, usize)> = Vec::new(); // (fx, tx, gap_y)
    let mut long_edges = 0usize;
    for &(f, t) in &links {
        if layer[t] != layer[f] + 1 {
            long_edges += 1;
            continue;
        }
        let (fx, fy) = center[f];
        let (tx, _ty) = center[t];
        drawn.push((fx, tx, fy + BOX_H));
    }
    for &(fx, tx, gap_y) in &drawn {
        let (lo, hi) = if fx <= tx { (fx, tx) } else { (tx, fx) };
        if gap_y < rows_n {
            for cell in grid[gap_y].iter_mut().take(hi + 1).skip(lo) {
                *cell = Cell('─');
            }
        }
    }
    for &(fx, tx, gap_y) in &drawn {
        // Source stub: down out of the box, turning toward target.
        if gap_y < rows_n && fx < total_w {
            grid[gap_y][fx] = Cell(if tx > fx {
                '└'
            } else if tx < fx {
                '┘'
            } else {
                '│'
            });
        }
        // Target stub: horizontal arrives, turns down into the box.
        if gap_y < rows_n && tx < total_w {
            grid[gap_y][tx] = Cell(if fx < tx {
                '┐'
            } else if fx > tx {
                '┌'
            } else {
                '│'
            });
        }
    }

    let mut out: Vec<String> = grid
        .iter()
        .map(|row| {
            let s: String = row.iter().map(|c| c.0).collect();
            s.trim_end().to_string()
        })
        .filter(|s| !s.trim().is_empty())
        .collect();
    if long_edges > 0 {
        out.push(format!(
            "({long_edges} edge(s) span layers — see edge list)"
        ));
    }
    out
}

fn dot_block(nodes: &[Value], edges: &[Value]) -> String {
    let mut out = String::from("digraph \"pavo\" {\n");
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
    out
}

// ---------- form / kv / markdown / embed ----------

fn form_json(v: &Value, depth: usize, b: &mut Builder) {
    let ind = "  ".repeat(depth);
    title_row(v, &ind, b);
    // Interactive = the driver declared a submit action. Forms
    // without `submit_op` stay a read-only field listing (the
    // renderer "may only preview" per the protocol docs).
    let submit_op = v.get("submit_op").and_then(Value::as_str);
    let Some(submit_op) = submit_op.filter(|s| !s.is_empty()) else {
        form_readonly(v, &ind, b);
        return;
    };

    // Register the spec first so field/submit rows can mark it.
    let form_idx = b.forms.len();
    let fields: Vec<FormFieldSpec> = v
        .get("fields")
        .and_then(Value::as_array)
        .map(|arr| arr.iter().map(form_field_spec).collect())
        .unwrap_or_default();
    let title = v
        .get("title")
        .and_then(Value::as_str)
        .unwrap_or("form")
        .to_string();
    b.forms.push(FormSpec {
        id: v.get("id").and_then(Value::as_str).map(str::to_string),
        title: title.clone(),
        submit_op: submit_op.to_string(),
        submit_label: v
            .get("submit_label")
            .and_then(Value::as_str)
            .unwrap_or("Submit")
            .to_string(),
        fields,
    });

    for (fi, f) in b.forms[form_idx].fields.clone().iter().enumerate() {
        let mut text = format!("{ind}- {}: {} [{}]", f.key, f.label, f.kind);
        if f.required {
            text.push_str(" (required)");
        }
        // Show the effective value: `value` (existing state) wins
        // over `default`; placeholder marks an empty editable field.
        let current = f.value.as_ref().or(f.default.as_ref());
        match current {
            Some(val) => text.push_str(&format!(" = {}", cell_str(val))),
            None if f.editable => {
                if let Some(p) = &f.placeholder {
                    text.push_str(&format!(" = {p} (placeholder)"));
                }
            }
            None => {}
        }
        if f.editable {
            text.push_str(" ✎");
        } else {
            text.push_str(" (read-only)");
        }
        if let Some(options) = (!f.options.is_empty()).then(|| f.options.join("|")) {
            text.push_str(&format!(" options={options}"));
        }
        if let Some(hint) = &f.hint {
            text.push_str(&format!(" — {hint}"));
        }
        let mut detail = vec![
            ("key".to_string(), f.key.clone()),
            ("label".to_string(), f.label.clone()),
            ("kind".to_string(), f.kind.clone()),
        ];
        if f.required {
            detail.push(("required".to_string(), "yes".to_string()));
        }
        if !f.editable {
            detail.push(("editable".to_string(), "no".to_string()));
        }
        for (k, val) in [("default", &f.default), ("value", &f.value)] {
            if let Some(val) = val {
                detail.push((k.to_string(), cell_str(val)));
            }
        }
        if let Some(hint) = &f.hint {
            detail.push(("hint".to_string(), hint.clone()));
        }
        let style = if f.editable {
            RowStyle::Normal
        } else {
            RowStyle::Dim
        };
        b.sel_row(&text, style, f.label.clone(), detail);
        if f.editable {
            b.mark_form(form_idx, Some(fi));
        }
    }

    // The submit row — Enter opens the editor at the first editable
    // field; the actual `submit` op fires from inside the session.
    let label = b.forms[form_idx].submit_label.clone();
    let op = b.forms[form_idx].submit_op.clone();
    b.sel_row(
        &format!("{ind}{label} → {op}"),
        RowStyle::Dim,
        format!("{title} submit"),
        vec![
            ("form".to_string(), title),
            ("submit_op".to_string(), op),
            ("label".to_string(), label),
        ],
    );
    b.mark_form(form_idx, None);
}

/// Parse one raw field object into a [`FormFieldSpec`]. `editable`
/// defaults to true per the protocol (`editable:false` opts out).
fn form_field_spec(f: &Value) -> FormFieldSpec {
    FormFieldSpec {
        key: f
            .get("key")
            .and_then(Value::as_str)
            .unwrap_or("")
            .to_string(),
        label: f
            .get("label")
            .and_then(Value::as_str)
            .unwrap_or("")
            .to_string(),
        kind: f
            .get("kind")
            .and_then(Value::as_str)
            .unwrap_or("text")
            .to_string(),
        required: f.get("required").and_then(Value::as_bool) == Some(true),
        editable: f.get("editable").and_then(Value::as_bool) != Some(false),
        options: f
            .get("options")
            .and_then(Value::as_array)
            .map(|arr| arr.iter().map(cell_str).collect())
            .unwrap_or_default(),
        default: f.get("default").cloned(),
        value: f.get("value").cloned(),
        hint: f.get("hint").and_then(Value::as_str).map(str::to_string),
        placeholder: f
            .get("placeholder")
            .and_then(Value::as_str)
            .map(str::to_string),
    }
}

/// The pre-forms-v2 rendering: a read-only field listing for forms
/// without a `submit_op`.
fn form_readonly(v: &Value, ind: &str, b: &mut Builder) {
    if let Some(fields) = v.get("fields").and_then(Value::as_array) {
        for f in fields {
            let key = f.get("key").and_then(Value::as_str).unwrap_or("");
            let label = f.get("label").and_then(Value::as_str).unwrap_or(key);
            let kind = f.get("kind").and_then(Value::as_str).unwrap_or("text");
            let mut text = format!("{ind}- {key}: {label} [{kind}]");
            if f.get("required").and_then(Value::as_bool) == Some(true) {
                text.push_str(" (required)");
            }
            if let Some(default) = f.get("default") {
                text.push_str(&format!(" default={}", cell_str(default)));
            }
            if let Some(value) = f.get("value") {
                text.push_str(&format!(" value={}", cell_str(value)));
            }
            if let Some(options) = f.get("options").and_then(Value::as_array) {
                let opts: Vec<String> = options.iter().map(cell_str).collect();
                text.push_str(&format!(" options={}", opts.join("|")));
            }
            if let Some(hint) = f.get("hint").and_then(Value::as_str) {
                text.push_str(&format!(" — {hint}"));
            }
            let mut detail = vec![
                ("key".to_string(), key.to_string()),
                ("label".to_string(), label.to_string()),
                ("kind".to_string(), kind.to_string()),
            ];
            if f.get("required").and_then(Value::as_bool) == Some(true) {
                detail.push(("required".to_string(), "yes".to_string()));
            }
            for (k, vk) in [("default", "default"), ("value", "value")] {
                if let Some(val) = f.get(vk) {
                    detail.push((k.to_string(), cell_str(val)));
                }
            }
            if let Some(hint) = f.get("hint").and_then(Value::as_str) {
                detail.push(("hint".to_string(), hint.to_string()));
            }
            b.sel_row(&text, RowStyle::Normal, label.to_string(), detail);
        }
    }
    if let Some(op) = v
        .get("submit_op")
        .and_then(Value::as_str)
        .filter(|s| !s.is_empty())
    {
        let label = v
            .get("submit_label")
            .and_then(Value::as_str)
            .unwrap_or("Submit");
        b.row(&format!("{ind}submit → {op} ({label})"), RowStyle::Dim);
    }
}

fn kv_json(v: &Value, data: &Map<String, Value>, depth: usize, b: &mut Builder) {
    let ind = "  ".repeat(depth);
    title_row(v, &ind, b);
    if bound_error(v, data, &ind, b) {
        return;
    }
    let mut pairs: Vec<(String, Value, Option<String>)> = v
        .get("pairs")
        .and_then(Value::as_array)
        .map(|arr| {
            arr.iter()
                .filter_map(|p| {
                    Some((
                        p.get("key").and_then(Value::as_str)?.to_string(),
                        p.get("value").cloned().unwrap_or(Value::Null),
                        p.get("kind").and_then(Value::as_str).map(str::to_string),
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
                    .map(|(k, val)| (k.clone(), val.clone(), None))
                    .collect();
            } else {
                b.row(&format!("{ind}{}", cell_str(payload)), RowStyle::Dim);
                return;
            }
        }
    }
    if pairs.is_empty() {
        b.row(&format!("{ind}(empty)"), RowStyle::Dim);
        return;
    }
    for (key, value, kind) in pairs {
        let mut text = format!("{ind}{key}: {}", cell_str(&value));
        if let Some(k) = &kind {
            text.push_str(&format!("  [{k}]"));
        }
        let fields = vec![
            ("key".to_string(), key.clone()),
            ("value".to_string(), detail_value(&value)),
        ];
        b.sel_row(&text, RowStyle::Normal, key.clone(), fields);
    }
}

/// Render a typed [`KvNode`] — used for the unknown-kind fallback.
fn kv_node(kv: &KvNode, depth: usize, b: &mut Builder) {
    let ind = "  ".repeat(depth);
    if let Some(title) = &kv.base.title {
        b.row(&format!("{ind}{title}"), RowStyle::Title);
    }
    if kv.pairs.is_empty() {
        b.row(&format!("{ind}(empty)"), RowStyle::Dim);
        return;
    }
    for pair in &kv.pairs {
        let mut text = format!("{ind}{}: {}", pair.key, cell_str(&pair.value));
        if let Some(k) = &pair.kind {
            text.push_str(&format!("  [{k}]"));
        }
        let fields = vec![
            ("key".to_string(), pair.key.clone()),
            ("value".to_string(), detail_value(&pair.value)),
        ];
        b.sel_row(&text, RowStyle::Normal, pair.key.clone(), fields);
    }
}

fn markdown_json(v: &Value, depth: usize, b: &mut Builder) {
    let ind = "  ".repeat(depth);
    title_row(v, &ind, b);
    let text = v.get("text").and_then(Value::as_str).unwrap_or("");
    for line in text.lines() {
        b.row(&format!("{ind}{line}"), RowStyle::Normal);
    }
}

fn embed_json(v: &Value, depth: usize, b: &mut Builder) {
    let ind = "  ".repeat(depth);
    title_row(v, &ind, b);
    let component = v.get("component").and_then(Value::as_str).unwrap_or("?");
    let mut text = format!("{ind}[embed: {component}]");
    if let Some(props) = v
        .get("props")
        .filter(|p| p.as_object().is_some_and(|m| !m.is_empty()))
    {
        text.push_str(&format!(
            " {}",
            serde_json::to_string(props).unwrap_or_default()
        ));
    }
    let mut fields = vec![("component".to_string(), component.to_string())];
    if let Some(props) = v.get("props") {
        if !props.as_object().is_some_and(Map::is_empty) {
            fields.push(("props".to_string(), pretty(props)));
        }
    }
    if let Some(desc) = v.get("description").and_then(Value::as_str) {
        fields.push(("description".to_string(), desc.to_string()));
    }
    b.sel_row(&text, RowStyle::Dim, format!("embed: {component}"), fields);
}

// ---------- renderer-plugin RenderModel → rows ----------

/// Draw a plugin's [`RenderModel`] on the canvas — the TUI mapping:
/// `rows` → aligned selectable table, `text` → plain lines, `svg` →
/// a dim placeholder (the TUI can't embed markup), `placeholder` →
/// a selectable component row.
fn render_model_rows(model: &RenderModel, depth: usize, width: usize, b: &mut Builder) {
    let ind = "  ".repeat(depth);
    match model {
        RenderModel::Text { lines } => {
            for line in lines {
                b.row(&format!("{ind}{line}"), RowStyle::Normal);
            }
        }
        RenderModel::Rows {
            headers,
            aligns,
            rows,
        } => {
            plugin_rows(headers, aligns, rows, depth, width, b);
        }
        RenderModel::Svg { markup } => {
            b.row(
                &format!("{ind}[svg: {} bytes — see web/cli]", markup.len()),
                RowStyle::Dim,
            );
        }
        RenderModel::Placeholder { component, props } => {
            let mut text = format!("{ind}[component: {component}]");
            if !props.is_empty() {
                text.push_str(&format!(
                    " {}",
                    serde_json::to_string(props).unwrap_or_default()
                ));
            }
            let fields = vec![
                ("component".to_string(), component.clone()),
                ("props".to_string(), pretty(&Value::Object(props.clone()))),
            ];
            b.sel_row(
                &text,
                RowStyle::Dim,
                format!("component: {component}"),
                fields,
            );
        }
    }
}

/// `RenderModel::Rows` → canvas rows: header + separator + one
/// selectable row per body row (detail = column → cell).
fn plugin_rows(
    headers: &[String],
    aligns: &[pavo_protocol::Align],
    rows: &[Vec<String>],
    depth: usize,
    width: usize,
    b: &mut Builder,
) {
    let ind = "  ".repeat(depth);
    let ncols = headers
        .len()
        .max(rows.iter().map(Vec::len).max().unwrap_or(0));
    if ncols == 0 {
        b.row(&format!("{ind}(empty)"), RowStyle::Dim);
        return;
    }
    let align_at = |i: usize| -> &str {
        aligns
            .get(i)
            .map(|a| match a {
                pavo_protocol::Align::Left => "left",
                pavo_protocol::Align::Center => "center",
                pavo_protocol::Align::Right => "right",
            })
            .unwrap_or("left")
    };
    // Same width-fitting policy as table_json: shrink widest until fit.
    let avail = width.saturating_sub(depth * 2).max(ncols * 4);
    let sep = 2usize;
    let mut widths: Vec<usize> = (0..ncols)
        .map(|i| headers.get(i).map_or(0, |h| h.chars().count()))
        .collect();
    for row in rows {
        for (i, cell) in row.iter().enumerate() {
            if i < widths.len() {
                widths[i] = widths[i].max(cell.chars().count());
            }
        }
    }
    loop {
        let total: usize = widths.iter().sum::<usize>() + sep * (widths.len() - 1);
        if total <= avail {
            break;
        }
        let Some((idx, _)) = widths
            .iter()
            .enumerate()
            .filter(|(_, w)| **w > 3)
            .max_by_key(|(_, w)| *w)
        else {
            break;
        };
        widths[idx] -= 1;
    }
    let emit = |cells: &[String], style: RowStyle, sel: Option<Selection>, b: &mut Builder| {
        let parts: Vec<String> = cells
            .iter()
            .enumerate()
            .map(|(i, cell)| fit(cell, widths[i], align_at(i)))
            .collect();
        let text = format!("{ind}{}", parts.join(&" ".repeat(sep)));
        match sel {
            Some(sel) => {
                let idx = b.selections.len();
                b.selections.push(sel);
                b.rows.push(Row {
                    text,
                    style,
                    sel: Some(idx),
                });
            }
            None => b.row(&text, style),
        }
    };
    let header_cells: Vec<String> = (0..ncols)
        .map(|i| headers.get(i).cloned().unwrap_or_default())
        .collect();
    emit(&header_cells, RowStyle::Header, None, b);
    let sep_cells: Vec<String> = widths.iter().map(|w| "-".repeat(*w)).collect();
    emit(&sep_cells, RowStyle::Dim, None, b);
    for row in rows {
        let cells: Vec<String> = (0..ncols)
            .map(|i| row.get(i).cloned().unwrap_or_default())
            .collect();
        let fields: Vec<(String, String)> = (0..ncols)
            .map(|i| {
                (
                    headers.get(i).cloned().unwrap_or_else(|| format!("col{i}")),
                    cells[i].clone(),
                )
            })
            .collect();
        emit(
            &cells,
            RowStyle::Normal,
            Some(Selection {
                title: "plugin row".to_string(),
                fields,
            }),
            b,
        );
    }
}

// ---------- shared helpers ----------

/// Stringify a JSON cell for terminal output: strings raw,
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

/// Detail-pane value: scalars inline, objects/arrays pretty-printed.
fn detail_value(v: &Value) -> String {
    match v {
        Value::Object(_) | Value::Array(_) => pretty(v),
        _ => cell_str(v),
    }
}

fn pretty(v: &Value) -> String {
    serde_json::to_string_pretty(v).unwrap_or_default()
}

fn scalar_str(v: Option<&Value>) -> Option<String> {
    v.and_then(Value::as_str).map(str::to_string)
}

/// Truncate to `width` chars with a trailing `…`, then pad to exactly
/// `width` per the alignment hint.
fn fit(text: &str, width: usize, align: &str) -> String {
    let truncated = if text.chars().count() > width {
        if width == 0 {
            String::new()
        } else if width == 1 {
            "…".to_string()
        } else {
            let mut t: String = text.chars().take(width - 1).collect();
            t.push('…');
            t
        }
    } else {
        text.to_string()
    };
    let len = truncated.chars().count();
    if len >= width {
        return truncated;
    }
    let fill = width - len;
    match align {
        "right" => format!("{}{truncated}", " ".repeat(fill)),
        "center" => {
            let left = fill / 2;
            format!("{}{truncated}{}", " ".repeat(left), " ".repeat(fill - left))
        }
        _ => format!("{truncated}{}", " ".repeat(fill)),
    }
}

/// Escape a string for a DOT quoted string.
fn dot_str(s: &str) -> String {
    s.replace('\\', "\\\\").replace('"', "\\\"")
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

    fn texts(model: &Model) -> Vec<&str> {
        model.rows.iter().map(|r| r.text.as_str()).collect()
    }

    #[test]
    fn kv_renders_pairs_and_bound_object() {
        let node = root(json!({
            "kind": "column", "title": "Board",
            "children": [
                {"kind": "kv", "query": "counts", "pairs": []},
            ]
        }));
        let m = build_view(&node, &data(), 80);
        let t = texts(&m);
        assert!(t.iter().any(|l| l.contains("Board")), "{t:?}");
        assert!(t.iter().any(|l| l.contains("open: 1")), "{t:?}");
        assert!(t.iter().any(|l| l.contains("done: 1")), "{t:?}");
        // every pair row is selectable with a detail
        assert!(!m.selectables.is_empty());
        let sel = &m.selections[m.rows[m.selectables[0]].sel.unwrap()];
        assert!(!sel.fields.is_empty());
    }

    #[test]
    fn table_inline_and_bound_rows_with_headers() {
        let node = root(json!({
            "kind": "table", "title": "Items", "query": "items",
            "columns": [
                {"key": "0", "title": "ID"},
                {"key": "1", "title": "Title"},
                {"key": "2", "title": "Status", "align": "right"}
            ]
        }));
        let m = build_view(&node, &data(), 80);
        let t = texts(&m);
        assert!(t.iter().any(|l| l.contains("ID")), "{t:?}");
        assert!(t.iter().any(|l| l.contains("T-1")), "{t:?}");
        assert!(t.iter().any(|l| l.contains("Beta")), "{t:?}");
        // header + separator + 2 rows selectable? header/sep not.
        let sel_rows = m.rows.iter().filter(|r| r.sel.is_some()).count();
        assert_eq!(sel_rows, 2);
        // detail of first row carries column titles → cells
        let first = m.rows.iter().find(|r| r.sel.is_some()).unwrap();
        let sel = &m.selections[first.sel.unwrap()];
        assert_eq!(sel.fields[0], ("ID".to_string(), "T-1".to_string()));
    }

    #[test]
    fn table_object_payload_keyed_by_column() {
        let mut d = Map::new();
        d.insert(
            "rows".to_string(),
            json!([{"name": "obj-row", "n": 3}, {"name": "two", "n": 4}]),
        );
        let node = root(json!({
            "kind": "table", "query": "rows",
            "columns": [{"key": "name", "title": "Name"}, {"key": "n", "title": "N"}]
        }));
        let m = build_view(&node, &d, 80);
        let t = texts(&m);
        assert!(
            t.iter().any(|l| l.contains("obj-row") && l.contains('3')),
            "{t:?}"
        );
    }

    #[test]
    fn table_truncates_to_width() {
        let node = root(json!({
            "kind": "table",
            "columns": [{"key": "a", "title": "A"}, {"key": "b", "title": "B"}],
            "rows": [["aaaaaaaaaaaaaaaaaaaa", "bbbbbbbbbbbbbbbbbbbb"]]
        }));
        let m = build_view(&node, &data(), 20);
        for r in &m.rows {
            assert!(r.text.chars().count() <= 22, "{} too wide", r.text);
        }
        assert!(
            m.rows.iter().any(|r| r.text.contains('…')),
            "{:?}",
            texts(&m)
        );
    }

    #[test]
    fn tree_renders_nested_glyphs() {
        let node = root(json!({
            "kind": "tree", "items": [
                {"label": "root", "detail": "top", "children": [
                    {"label": "child", "children": [{"label": "leaf"}]},
                    {"label": "child2"}
                ]},
                {"label": "second"}
            ]
        }));
        let m = build_view(&node, &data(), 80);
        let t = texts(&m);
        assert!(t.iter().any(|l| l.contains("├─ root")), "{t:?}");
        assert!(t.iter().any(|l| l.contains("├─ child")), "{t:?}");
        assert!(t.iter().any(|l| l.trim_end().ends_with("└─ leaf")), "{t:?}");
        assert!(t.iter().any(|l| l.contains("└─ child2")), "{t:?}");
        assert!(t.iter().any(|l| l.contains("└─ second")), "{t:?}");
        assert!(t.iter().any(|l| l.contains("— top")), "{t:?}");
        // 5 selectable items
        assert_eq!(m.rows.iter().filter(|r| r.sel.is_some()).count(), 5);
    }

    #[test]
    fn graph_lists_nodes_edges_and_dot() {
        let node = root(json!({
            "kind": "graph", "query": "deps",
            "nodes": [], "edges": []
        }));
        let m = build_view(&node, &data(), 80);
        let t = texts(&m);
        assert!(t.iter().any(|l| l.contains("nodes")), "{t:?}");
        assert!(
            t.iter().any(|l| l.contains('A') && l.contains("(a)")),
            "{t:?}"
        );
        assert!(t.iter().any(|l| l.contains("A -> B")), "{t:?}");
        let dot = m.graph_dot.unwrap();
        assert!(dot.contains("digraph"), "{dot}");
        assert!(dot.contains("\"a\" -> \"b\" [label=\"uses\"]"), "{dot}");
    }

    #[test]
    fn ascii_layout_draws_boxes_connectors_and_notes_long_edges() {
        let nodes = vec![
            json!({"id": "a", "label": "Alpha"}),
            json!({"id": "b", "label": "Beta"}),
            json!({"id": "c", "label": "Gamma"}),
        ];
        let edges = vec![
            json!({"from": "a", "to": "b"}),
            json!({"from": "b", "to": "c"}),
            json!({"from": "a", "to": "c"}), // spans two layers → skipped
        ];
        let out = ascii_layout(&nodes, &edges);
        let text = out.join("\n");
        assert!(text.contains('┌'), "{text}");
        assert!(text.contains("Alpha"), "{text}");
        assert!(text.contains('─'), "{text}");
        assert!(text.contains("1 edge(s) span layers"), "{text}");

        // Cycles and empty inputs must not panic; output is stable.
        let cyc_nodes = vec![json!({"id": "x"}), json!({"id": "y"})];
        let cyc_edges = vec![
            json!({"from": "x", "to": "y"}),
            json!({"from": "y", "to": "x"}),
        ];
        let once = ascii_layout(&cyc_nodes, &cyc_edges);
        assert_eq!(once, ascii_layout(&cyc_nodes, &cyc_edges));
        assert!(ascii_layout(&[], &[]).is_empty());
        assert!(ascii_layout(&cyc_nodes, &[])
            .iter()
            .any(|l| l.contains('x')));
    }

    #[test]
    fn graph_model_includes_layout_section() {
        let node = root(json!({
            "kind": "graph",
            "nodes": [{"id": "a", "label": "A"}, {"id": "b", "label": "B"}],
            "edges": [{"from": "a", "to": "b"}]
        }));
        let m = build_view(&node, &data(), 100);
        let t = texts(&m);
        assert!(t.iter().any(|l| l.contains("layout")), "{t:?}");
        assert!(t.iter().any(|l| l.contains('┌')), "{t:?}");
    }

    #[test]
    fn form_kv_markdown_embed_render() {
        let node = root(json!({
            "kind": "column",
            "children": [
                {"kind": "form", "submit_op": "set", "fields": [
                    {"key": "name", "label": "Name", "kind": "text", "required": true,
                     "default": "x"}
                ]},
                {"kind": "kv", "pairs": [{"key": "k", "value": 1}]},
                {"kind": "markdown", "text": "hello **world**"},
                {"kind": "embed", "component": "timeline", "props": {"x": 1}}
            ]
        }));
        let m = build_view(&node, &data(), 80);
        let t = texts(&m);
        assert!(
            t.iter().any(|l| l.contains("name: Name [text] (required)")),
            "{t:?}"
        );
        assert!(t.iter().any(|l| l.contains("Submit → set")), "{t:?}");
        // Interactive form: one FormSpec, editable field + submit row
        // marked, read-only field unmarked.
        assert_eq!(m.forms.len(), 1);
        assert_eq!(m.forms[0].submit_op, "set");
        assert_eq!(m.form_marks.len(), 2);
        assert!(t.iter().any(|l| l.contains("k: 1")), "{t:?}");
        assert!(t.iter().any(|l| l.contains("hello **world**")), "{t:?}");
        assert!(t.iter().any(|l| l.contains("[embed: timeline]")), "{t:?}");
        // no graph → no dot
        assert!(m.graph_dot.is_none());
    }

    #[test]
    fn unknown_kind_degrades_to_kv() {
        // Unknown kinds can't deserialize as ViewNode — but a *known*
        // container can hold... no, it can't either. Exercise the JSON
        // fallback path through a panel whose child serde would fail;
        // instead verify kv_fallback_json output renders via build on a
        // raw value by round-tripping what the wire would produce.
        // Here: build a ViewNode tree is impossible, so test the pieces:
        let raw = json!({"kind": "hologram", "id": "h1", "intensity": 0.8});
        let fallback = kv_fallback_json(&raw);
        let kv: KvNode = serde_json::from_value(fallback).unwrap();
        let mut b = Builder::default();
        kv_node(&kv, 0, &mut b);
        let m = b.finish();
        let t = texts(&m);
        assert!(t.iter().any(|l| l.contains("kind: hologram")), "{t:?}");
        assert!(t.iter().any(|l| l.contains("intensity: 0.8")), "{t:?}");
    }

    #[test]
    fn bound_query_error_payload() {
        let mut d = Map::new();
        d.insert("items".to_string(), json!({"error": "boom"}));
        let node = root(json!({"kind": "table", "query": "items",
            "columns": [{"key": "a", "title": "A"}]}));
        let m = build_view(&node, &d, 80);
        let t = texts(&m);
        assert!(
            t.iter().any(|l| l.contains("[query 'items' error: boom]")),
            "{t:?}"
        );
        assert!(m
            .rows
            .iter()
            .all(|r| r.style != RowStyle::Header || !r.text.contains('A')));
    }

    #[test]
    fn empty_states() {
        let node = root(json!({"kind": "kv", "pairs": []}));
        let m = build_view(&node, &data(), 80);
        assert!(texts(&m).iter().any(|l| l.contains("(empty)")));
        let g = root(json!({"kind": "graph", "nodes": [], "edges": []}));
        let m2 = build_view(&g, &data(), 80);
        assert!(texts(&m2).iter().any(|l| l.contains("(empty graph)")));
    }

    #[test]
    fn dashboard_lists_views_and_contributions() {
        let views = vec![ViewEntry {
            driver: "demo".to_string(),
            driver_title: "Demo".to_string(),
            id: "board".to_string(),
            namespaced: "demo:board".to_string(),
            route: "/demo/board".to_string(),
            title: "Board".to_string(),
            description: None,
            queries: vec!["items".to_string()],
        }];
        let chrome = Chrome::default();
        let m = build_dashboard(&views, &chrome, &|_| None, 80);
        let t = texts(&m);
        assert!(t.iter().any(|l| l.contains("demo:board — Board")), "{t:?}");
        // view rows resolve via their Selection title
        let first = m.rows.iter().find(|r| r.sel.is_some()).unwrap();
        assert_eq!(m.selections[first.sel.unwrap()].title, "demo:board");
    }

    #[test]
    fn dashboard_lists_platform_slot_contributions() {
        use pavo_core::merge::SlotEntry;
        use pavo_protocol::manifest::Contribution;

        let entry = |id: &str, title: &str| SlotEntry {
            driver: "demo".to_string(),
            driver_title: "Demo".to_string(),
            contribution: Contribution {
                id: format!("demo:{id}"),
                title: title.to_string(),
                route: format!("/demo/{id}"),
                priority: 1,
                icon: None,
            },
        };
        let chrome = Chrome {
            sidebar: vec![entry("side", "Side nav")],
            header_actions: vec![entry("act", "Quick action")],
            notifications: vec![entry("ntf", "Notify")],
            jobs: vec![entry("job", "Jobs")],
            ..Chrome::default()
        };
        let m = build_dashboard(&[], &chrome, &|_| None, 80);
        let t = texts(&m);
        for want in [
            "sidebar",
            "header actions",
            "notifications",
            "jobs",
            "Side nav",
            "Quick action",
            "Notify",
            "Jobs",
        ] {
            assert!(t.iter().any(|l| l.contains(want)), "{want} missing: {t:?}");
        }
        // Unresolvable routes get the same "driver-owned route" note
        // as navbar contributions.
        let sel = m
            .rows
            .iter()
            .find(|r| r.sel.is_some() && r.text.contains("Side nav"))
            .unwrap();
        let fields = &m.selections[sel.sel.unwrap()].fields;
        assert!(fields
            .iter()
            .any(|(k, v)| k == "note" && v == "driver-owned route"));
    }

    #[test]
    fn note_and_loading_models() {
        let m = build_note("boom", &[("err".to_string(), "nope".to_string())], true);
        assert!(texts(&m).iter().any(|l| l.contains("boom")));
        assert!(texts(&m).iter().any(|l| l.contains("err: nope")));
        let l = build_loading("demo:board");
        assert!(texts(&l).iter().any(|l| l.contains("loading demo:board")));
    }

    #[test]
    fn plugin_render_models_substitute_by_path() {
        // column[ kv, graph ] → graph at path "1".
        let node = root(json!({
            "kind": "column", "title": "Mixed",
            "children": [
                {"kind": "kv", "query": "counts", "pairs": []},
                {"kind": "graph", "query": "deps"}
            ]
        }));
        let mut renders = HashMap::new();
        renders.insert(
            "1".to_string(),
            RenderModel::Rows {
                headers: vec!["id".into(), "label".into()],
                aligns: vec![],
                rows: vec![vec!["a".into(), "Alpha".into()]],
            },
        );
        let m = build_view_with_plugins(&node, &data(), 80, &renders);
        let t = texts(&m);
        // kv still built-in; graph node replaced by plugin rows.
        assert!(t.iter().any(|l| l.contains("open: 1")), "{t:?}");
        assert!(
            t.iter().any(|l| l.contains("id") && l.contains("label")),
            "{t:?}"
        );
        assert!(
            t.iter().any(|l| l.contains("a") && l.contains("Alpha")),
            "{t:?}"
        );
        // plugin body rows are selectable
        assert!(m
            .rows
            .iter()
            .any(|r| r.sel.is_some() && r.text.contains("Alpha")));

        // Other RenderModel variants map to their TUI degradations.
        let mut renders2 = HashMap::new();
        renders2.insert(
            "1".to_string(),
            RenderModel::Svg {
                markup: "<svg/>".into(),
            },
        );
        let m2 = build_view_with_plugins(&node, &data(), 80, &renders2);
        assert!(
            texts(&m2).iter().any(|l| l.contains("[svg:")),
            "{:?}",
            texts(&m2)
        );

        let mut renders3 = HashMap::new();
        renders3.insert("1".to_string(), RenderModel::text(["hello", "world"]));
        let m3 = build_view_with_plugins(&node, &data(), 80, &renders3);
        let t3 = texts(&m3);
        assert!(t3.iter().any(|l| l.trim() == "hello"), "{t3:?}");
        assert!(t3.iter().any(|l| l.trim() == "world"), "{t3:?}");

        let mut renders4 = HashMap::new();
        renders4.insert(
            "1".to_string(),
            RenderModel::Placeholder {
                component: "viz-gantt".into(),
                props: Default::default(),
            },
        );
        let m4 = build_view_with_plugins(&node, &data(), 80, &renders4);
        assert!(
            texts(&m4).iter().any(|l| l.contains("viz-gantt")),
            "{:?}",
            texts(&m4)
        );
    }

    #[test]
    fn markdown_and_panel_titles() {
        let node = root(json!({
            "kind": "panel", "title": "P",
            "children": [{"kind": "row", "title": "R",
                "children": [{"kind": "markdown", "text": "x"}]}]
        }));
        let m = build_view(&node, &data(), 80);
        let t = texts(&m);
        assert!(t.iter().any(|l| l.trim() == "P"), "{t:?}");
        assert!(t.iter().any(|l| l.contains("R")), "{t:?}");
        assert!(t.iter().any(|l| l.contains("  x")), "{t:?}");
    }
}

#[cfg(test)]
#[cfg_attr(coverage, coverage(off))]
mod proptests {
    #![allow(clippy::unwrap_used)]
    //! Property/fuzz tests: the canvas model builder must be total
    //! over arbitrary node JSON (KV-fallback contract — garbage in,
    //! degraded rows out, never a panic) and over arbitrary
    //! RenderModels / chrome inputs.

    use super::*;
    use proptest::prelude::*;
    use proptest::strategy::BoxedStrategy;

    fn text() -> BoxedStrategy<String> {
        prop::collection::vec(any::<char>(), 0..16)
            .prop_map(|cs| cs.into_iter().collect())
            .boxed()
    }

    fn json_value(depth: usize) -> BoxedStrategy<Value> {
        if depth == 0 {
            return prop_oneof![
                Just(Value::Null),
                any::<bool>().prop_map(Value::Bool),
                any::<i64>().prop_map(|n| Value::from(n.clamp(-999, 999))),
                text().prop_map(Value::String),
            ]
            .boxed();
        }
        prop_oneof![
            Just(Value::Null),
            any::<bool>().prop_map(Value::Bool),
            any::<i64>().prop_map(|n| Value::from(n.clamp(-999, 999))),
            text().prop_map(Value::String),
            prop::collection::vec(json_value(depth - 1), 0..4).prop_map(Value::Array),
            prop::collection::hash_map(text(), json_value(depth - 1), 0..4)
                .prop_map(|m| Value::Object(m.into_iter().collect())),
        ]
        .boxed()
    }

    fn data_map() -> BoxedStrategy<Map<String, Value>> {
        prop::collection::hash_map(text(), json_value(3), 0..4)
            .prop_map(|m| m.into_iter().collect())
            .boxed()
    }

    fn render_model() -> BoxedStrategy<RenderModel> {
        prop_oneof![
            (
                prop::collection::vec(text(), 0..4),
                prop::collection::vec(text(), 0..4),
                prop::collection::vec(prop::collection::vec(text(), 0..4), 0..4),
            )
                .prop_map(|(headers, aligns, rows)| RenderModel::Rows {
                    headers,
                    aligns: aligns
                        .iter()
                        .map(|a| match a.as_str() {
                            "center" => pavo_protocol::Align::Center,
                            "right" => pavo_protocol::Align::Right,
                            _ => pavo_protocol::Align::Left,
                        })
                        .collect(),
                    rows,
                }),
            text().prop_map(|markup| RenderModel::Svg { markup }),
            prop::collection::vec(text(), 0..4).prop_map(|lines| RenderModel::Text { lines }),
            (text(), json_value(2)).prop_map(|(component, props)| RenderModel::Placeholder {
                component,
                props: props.as_object().cloned().unwrap_or_default(),
            }),
        ]
        .boxed()
    }

    proptest! {
        /// Arbitrary JSON through the canvas walker at assorted
        /// widths — total, and the model invariants hold: every
        /// selectable index points at a row with `sel`, selections
        /// and selectables stay in sync.
        #[test]
        fn node_walk_never_panics(
            v in json_value(5),
            data in data_map(),
            width in prop_oneof![Just(0usize), Just(1), Just(20), Just(80), Just(400)],
        ) {
            let mut b = Builder::default();
            node_json_path(&v, &data, 0, width, String::new(), &HashMap::new(), &mut b);
            let model = b.finish();
            for &ri in &model.selectables {
                prop_assert!(ri < model.rows.len());
                prop_assert!(model.rows[ri].sel.is_some());
            }
            for row in &model.rows {
                if let Some(sel) = row.sel {
                    prop_assert!(sel < model.selections.len());
                }
            }
        }

        /// Plugin-render substitution path on arbitrary JSON +
        /// arbitrary renders map.
        #[test]
        fn node_walk_with_plugins_never_panics(
            v in json_value(4),
            data in data_map(),
            width in prop_oneof![Just(0usize), Just(10), Just(120)],
            renders in prop::collection::hash_map(
                prop_oneof![Just(String::new()), text()],
                render_model(),
                0..3,
            ),
        ) {
            let mut b = Builder::default();
            node_json_path(
                &v,
                &data,
                0,
                width,
                String::new(),
                &renders,
                &mut b,
            );
            let _ = b.finish();
        }

        /// render_model_rows on arbitrary models at assorted widths.
        #[test]
        fn render_model_rows_never_panics(
            m in render_model(),
            depth in 0usize..4,
            width in prop_oneof![Just(0usize), Just(1), Just(30), Just(200)],
        ) {
            let mut b = Builder::default();
            render_model_rows(&m, depth, width, &mut b);
            let model = b.finish();
            prop_assert!(model.rows.len() <= m_rows_bound(&m) + 8);
        }

        /// build_note/build_loading/build_dashboard on arbitrary
        /// strings — total.
        #[test]
        fn chrome_models_never_panics(
            title in text(),
            fields in prop::collection::vec((text(), text()), 0..8),
            error in any::<bool>(),
        ) {
            let _ = build_note(&title, &fields, error);
            let _ = build_loading(&title);
            let _ = build_dashboard(&[], &Chrome::default(), &|_| None, 80);
        }
    }

    fn m_rows_bound(m: &RenderModel) -> usize {
        match m {
            RenderModel::Text { lines } => lines.len(),
            RenderModel::Rows { rows, .. } => rows.len() + 2,
            _ => 1,
        }
    }
}
