#![allow(clippy::unwrap_used)]
//! Serde round-trips + fixtures for the whole wire contract.

use pavo_protocol::manifest::{
    Contributes, Contribution, DriverDeps, DriverManifest, QueryBinding, ViewSpec,
};
use pavo_protocol::platform::{JobItem, NotificationItem};
use pavo_protocol::view_node::{
    Align, ColumnNode, EmbedNode, FormField, FormFieldKind, FormNode, GraphEdge, GraphNode,
    GraphNodeItem, KvNode, KvPair, MarkdownNode, NodeBase, PanelNode, RowNode, TableColumn,
    TableNode, TreeItem, TreeNode, ViewNode,
};
use pavo_protocol::wire::{
    parse_request, response_line, QueryResult, Request, Response, ViewResult, ViewSummary,
};
use serde_json::{json, Map, Value};

fn roundtrip<T>(value: &T)
where
    T: serde::Serialize + for<'de> serde::Deserialize<'de> + PartialEq + std::fmt::Debug,
{
    let text = serde_json::to_string(value).unwrap();
    let back: T = serde_json::from_str(&text).unwrap();
    assert_eq!(*value, back, "round-trip mismatch for {text}");
}

// ---------- fixtures ----------

/// A realistic driver manifest — the shape `myco`'s driver would ship.
const MANIFEST_JSON: &str = r#"{
  "name": "myco",
  "title": "Myco",
  "icon": "📋",
  "version": "0.4.0",
  "verbs": ["manifest", "serve"],
  "capabilities": ["pavo"],
  "contributes": {
    "navbar": [
      { "id": "myco:board", "title": "Board", "route": "/myco/board", "priority": 10 }
    ],
    "dashboard": [
      { "id": "myco:summary", "title": "Work ledger", "route": "/myco/board", "priority": 20, "icon": "📋" }
    ],
    "statusbar": [
      { "id": "myco:counts", "title": "items", "route": "/myco/board" }
    ],
    "routes": [
      { "id": "myco:settings", "title": "Myco settings", "route": "/myco/settings", "priority": 90 }
    ]
  },
  "views": [
    {
      "id": "board",
      "title": "Board",
      "route": "/myco/board",
      "description": "All work items by status",
      "queries": [
        { "name": "items", "op": "query", "params": { "product_op": "list" } },
        { "name": "counts", "op": "query", "params": { "product_op": "status_counts" } }
      ],
      "root": {
        "kind": "column",
        "gap": 1,
        "children": [
          { "kind": "kv", "query": "counts", "pairs": [] },
          {
            "kind": "table",
            "title": "Items",
            "query": "items",
            "sortable": true,
            "columns": [
              { "key": "id", "title": "ID", "width": 8 },
              { "key": "title", "title": "Title" },
              { "key": "status", "title": "Status", "align": "left", "width": 12 }
            ],
            "rows": [["T-001", "pavo scaffold", "in-progress"]]
          }
        ]
      }
    }
  ]
}"#;

/// A fixture ViewNode tree exercising every kind — what a kuda
/// module-graph view might emit.
const VIEW_TREE_JSON: &str = r###"{
  "kind": "panel",
  "id": "kuda:graph-view",
  "title": "Module graph",
  "query": "graph",
  "props": { "density": "compact" },
  "children": [
    {
      "kind": "row",
      "gap": 2,
      "children": [
        {
          "kind": "tree",
          "title": "Modules",
          "expandable": true,
          "items": [
            {
              "id": "kuda-core",
              "label": "kuda-core",
              "detail": "pure domain",
              "children": [
                { "id": "kuda-core::digest", "label": "digest", "icon": "◇" }
              ]
            }
          ]
        },
        {
          "kind": "graph",
          "layout": "layered",
          "nodes": [
            { "id": "kuda-core", "label": "kuda-core", "kind": "module" },
            { "id": "kuda-store", "label": "kuda-store", "kind": "module" }
          ],
          "edges": [
            { "from": "kuda-core", "to": "kuda-store", "label": "uses" }
          ]
        }
      ]
    },
    { "kind": "markdown", "text": "## Notes\n\nDrift: 2 modules." },
    {
      "kind": "form",
      "title": "Rescan",
      "submit_op": "rescan",
      "submit_label": "Rescan",
      "fields": [
        { "key": "path", "label": "Path", "kind": "text", "required": true, "hint": "repo root" },
        { "key": "deep", "label": "Deep scan", "kind": "bool", "default": false },
        { "key": "level", "label": "Level", "kind": "select", "options": ["fast", "full"], "value": "fast" }
      ]
    },
    {
      "kind": "embed",
      "component": "kuda-drift-timeline",
      "props": { "window": "7d" }
    }
  ]
}"###;

// ---------- manifest ----------

#[test]
fn manifest_fixture_roundtrips() {
    let manifest: DriverManifest = serde_json::from_str(MANIFEST_JSON).unwrap();
    assert_eq!(manifest.name, "myco");
    assert_eq!(manifest.views.len(), 1);
    assert_eq!(manifest.contributes.navbar.len(), 1);
    assert_eq!(manifest.contributes.extra.len(), 0);
    roundtrip(&manifest);

    // The serialized form must keep `kind` tags flat (no envelope).
    let text = serde_json::to_string(&manifest).unwrap();
    assert!(text.contains("\"kind\":\"column\""));
}

#[test]
fn manifest_minimal_defaults() {
    let manifest: DriverManifest =
        serde_json::from_str(r#"{ "name": "x", "title": "X", "version": "0.0.1" }"#).unwrap();
    assert!(manifest.icon.is_none());
    assert!(manifest.verbs.is_empty());
    assert!(manifest.capabilities.is_empty());
    assert!(manifest.views.is_empty());
    assert_eq!(manifest.contributes.navbar.len(), 0);
    roundtrip(&manifest);
}

#[test]
fn contributes_extra_slots_roundtrip() {
    let c: Contributes = serde_json::from_str(
        r#"{ "navbar": [], "custom_slot": [ { "id": "a:b", "title": "T", "route": "/x" } ] }"#,
    )
    .unwrap();
    assert_eq!(c.extra["custom_slot"].len(), 1);
    roundtrip(&c);
}

#[test]
fn contribution_priority_defaults() {
    let c: Contribution =
        serde_json::from_str(r#"{ "id": "d:e", "title": "T", "route": "/r" }"#).unwrap();
    assert_eq!(c.priority, 100);
    roundtrip(&c);
}

// ---------- platform slots, deps, cross-driver queries ----------

#[test]
fn contributes_platform_slots_roundtrip() {
    // The four new first-class slots reuse the Contribution shape.
    let c: Contributes = serde_json::from_str(
        r#"{
          "sidebar": [ { "id": "myco:nav", "title": "Myco", "route": "/myco/board" } ],
          "headerActions": [ { "id": "myco:quick", "title": "Quick add", "route": "/myco/new" } ],
          "notifications": [ { "id": "myco:notify", "title": "Notifications", "route": "/myco/inbox" } ],
          "jobs": [ { "id": "myco:jobs", "title": "Jobs", "route": "/myco/jobs" } ]
        }"#,
    )
    .unwrap();
    assert_eq!(c.sidebar.len(), 1);
    assert_eq!(c.header_actions.len(), 1);
    assert_eq!(c.notifications.len(), 1);
    assert_eq!(c.jobs.len(), 1);
    // `headerActions` serializes under its camelCase wire key.
    let text = serde_json::to_string(&c).unwrap();
    assert!(text.contains("\"headerActions\""), "{text}");
    assert!(text.contains("\"sidebar\""), "{text}");
    roundtrip(&c);

    // Old manifests that kept these keys inside `extra`… — actually
    // serde flatten now claims them, which is the intended upgrade:
    // the typed fields win and parse identically.
    let old: Contributes = serde_json::from_str(
        r#"{ "jobs": [ { "id": "a:b", "title": "T", "route": "/x" } ],
             "future_slot": [ { "id": "a:c", "title": "U", "route": "/y" } ] }"#,
    )
    .unwrap();
    assert_eq!(old.jobs.len(), 1);
    assert_eq!(old.extra["future_slot"].len(), 1);
}

#[test]
fn manifest_deps_roundtrip() {
    let m: DriverManifest = serde_json::from_str(
        r#"{ "name": "myco", "title": "Myco", "version": "1",
             "deps": { "requires": ["config"], "optional": ["kuda"] } }"#,
    )
    .unwrap();
    let deps = m.deps.as_ref().unwrap();
    assert_eq!(deps.requires, vec!["config".to_string()]);
    assert_eq!(deps.optional, vec!["kuda".to_string()]);
    // `deps` absent → None and absent on the wire (skip_serializing_if).
    let m2: DriverManifest =
        serde_json::from_str(r#"{ "name": "x", "title": "X", "version": "0" }"#).unwrap();
    assert!(m2.deps.is_none());
    assert!(!serde_json::to_string(&m2).unwrap().contains("deps"));
    // Sparse deps object defaults both lists.
    let d: DriverDeps = serde_json::from_str(r#"{ "requires": ["a"] }"#).unwrap();
    assert!(d.optional.is_empty());
    roundtrip(&m);
    roundtrip(&d);
}

#[test]
fn query_binding_driver_roundtrip() {
    // Cross-driver binding: `driver` names the target driver's serve.
    let q: QueryBinding = serde_json::from_str(
        r#"{ "name": "items", "op": "query", "driver": "myco",
             "params": { "product_op": "list" } }"#,
    )
    .unwrap();
    assert_eq!(q.driver.as_deref(), Some("myco"));
    let text = serde_json::to_string(&q).unwrap();
    assert!(text.contains("\"driver\":\"myco\""), "{text}");
    roundtrip(&q);
    // Absent → None, omitted on the wire.
    let q2: QueryBinding = serde_json::from_str(r#"{ "name": "n", "op": "query" }"#).unwrap();
    assert!(q2.driver.is_none());
    assert!(!serde_json::to_string(&q2).unwrap().contains("driver"));
}

// ---------- platform feeds (jobs / notifications ops) ----------

#[test]
fn platform_feed_requests_parse() {
    for (line, name) in [
        (r#"{"op":"jobs"}"#, "jobs"),
        (r#"{"op":"notifications"}"#, "notifications"),
    ] {
        let req = parse_request(line).unwrap();
        let back = serde_json::to_string(&req).unwrap();
        assert_eq!(back, line, "{name} wire shape drifted");
        assert!(matches!(
            parse_request(&back).unwrap(),
            Request::Jobs | Request::Notifications
        ));
    }
}

#[test]
fn job_and_notification_items_roundtrip() {
    let job = JobItem {
        id: "build-42".into(),
        title: "Build pavo".into(),
        status: "running".into(),
        detail: Some("step 3/7".into()),
        route: Some("/myco/job/build-42".into()),
        updated_at: Some("2026-01-02T15:04:05Z".into()),
        extra: Default::default(),
    };
    roundtrip(&job);
    // Sparse shape parses; unknown fields land in `extra`.
    let j2: JobItem =
        serde_json::from_str(r#"{ "id": "j", "title": "T", "status": "queued", "progress": 40 }"#)
            .unwrap();
    assert_eq!(j2.extra["progress"], json!(40));

    let note = NotificationItem {
        id: "n-7".into(),
        title: "Build failed".into(),
        body: Some("cargo xtask check exited 1".into()),
        severity: Some("error".into()),
        route: Some("/myco/job/build-42".into()),
        created_at: Some("2026-01-02T15:04:05Z".into()),
        read: false,
        extra: Default::default(),
    };
    roundtrip(&note);
    let n2: NotificationItem =
        serde_json::from_str(r#"{ "id": "n", "title": "Hi", "read": true, "x": 1 }"#).unwrap();
    assert!(n2.read);
    assert_eq!(n2.extra["x"], json!(1));
    // Response envelopes carrying feed results round-trip too.
    roundtrip(&Response::ok_typed(&vec![job]));
    roundtrip(&Response::ok_typed(&vec![note]));
}

// ---------- view node ----------

#[test]
fn view_tree_fixture_roundtrips() {
    let node: ViewNode = serde_json::from_str(VIEW_TREE_JSON).unwrap();
    // Walk the tree — every kind present.
    let ViewNode::Panel(panel) = &node else {
        panic!("expected panel");
    };
    assert_eq!(panel.base.title.as_deref(), Some("Module graph"));
    assert_eq!(panel.base.query.as_deref(), Some("graph"));
    let kinds: Vec<&str> = panel
        .children
        .iter()
        .map(|n| match n {
            ViewNode::Row(_) => "row",
            ViewNode::Markdown(_) => "markdown",
            ViewNode::Form(_) => "form",
            ViewNode::Embed(_) => "embed",
            _ => "other",
        })
        .collect();
    assert_eq!(kinds, ["row", "markdown", "form", "embed"]);
    roundtrip(&node);
}

#[test]
fn every_kind_roundtrips() {
    let nodes = vec![
        serde_json::from_str::<ViewNode>(r#"{ "kind": "panel", "children": [] }"#).unwrap(),
        serde_json::from_str::<ViewNode>(
            r#"{ "kind": "table", "columns": [ { "key": "a", "title": "A" } ] }"#,
        )
        .unwrap(),
        serde_json::from_str::<ViewNode>(r#"{ "kind": "tree", "items": [ { "label": "root" } ] }"#)
            .unwrap(),
        serde_json::from_str::<ViewNode>(r#"{ "kind": "graph", "nodes": [], "edges": [] }"#)
            .unwrap(),
        serde_json::from_str::<ViewNode>(r#"{ "kind": "form", "fields": [] }"#).unwrap(),
        serde_json::from_str::<ViewNode>(
            r#"{ "kind": "kv", "pairs": [ { "key": "k", "value": 1 } ] }"#,
        )
        .unwrap(),
        serde_json::from_str::<ViewNode>(r#"{ "kind": "markdown", "text": "hi" }"#).unwrap(),
        serde_json::from_str::<ViewNode>(r#"{ "kind": "embed", "component": "x" }"#).unwrap(),
        serde_json::from_str::<ViewNode>(r#"{ "kind": "row", "children": [] }"#).unwrap(),
        serde_json::from_str::<ViewNode>(r#"{ "kind": "column", "children": [] }"#).unwrap(),
    ];
    for node in &nodes {
        roundtrip(node);
    }
}

#[test]
fn node_base_fields_flatten() {
    let node: ViewNode = serde_json::from_str(
        r#"{ "kind": "kv", "id": "n1", "title": "T", "query": "q",
             "description": "d", "props": { "x": 1 }, "pairs": [] }"#,
    )
    .unwrap();
    let base = node.base();
    assert_eq!(base.id.as_deref(), Some("n1"));
    assert_eq!(base.title.as_deref(), Some("T"));
    assert_eq!(base.query.as_deref(), Some("q"));
    assert_eq!(base.description.as_deref(), Some("d"));
    assert_eq!(base.props["x"], json!(1));

    // Serialized form keeps fields flat — no "base" envelope.
    let text = serde_json::to_string(&node).unwrap();
    assert!(text.contains("\"kind\":\"kv\""));
    assert!(!text.contains("\"base\""));
}

#[test]
fn typed_nodes_roundtrip() {
    roundtrip(&PanelNode {
        base: NodeBase {
            title: Some("p".into()),
            ..NodeBase::default()
        },
        children: vec![],
    });
    roundtrip(&RowNode {
        base: NodeBase::default(),
        children: vec![],
        gap: Some(2),
    });
    roundtrip(&ColumnNode {
        base: NodeBase::default(),
        children: vec![],
        gap: None,
    });
    roundtrip(&TableNode {
        base: NodeBase::default(),
        columns: vec![TableColumn {
            key: "k".into(),
            title: "K".into(),
            align: Some(Align::Right),
            width: Some(10),
        }],
        rows: vec![vec![json!("v")]],
        sortable: true,
    });
    roundtrip(&TreeNode {
        base: NodeBase::default(),
        items: vec![TreeItem {
            id: Some("i".into()),
            label: "l".into(),
            detail: None,
            icon: None,
            children: vec![],
            props: Map::new(),
            actions: vec![],
        }],
        expandable: true,
    });
    roundtrip(&GraphNode {
        base: NodeBase::default(),
        nodes: vec![GraphNodeItem {
            id: "a".into(),
            label: "A".into(),
            kind: Some("module".into()),
            props: Map::new(),
        }],
        edges: vec![GraphEdge {
            from: "a".into(),
            to: "b".into(),
            label: None,
        }],
        layout: Some("dot".into()),
    });
    roundtrip(&FormNode {
        base: NodeBase::default(),
        fields: vec![FormField {
            key: "k".into(),
            label: "K".into(),
            kind: FormFieldKind::Number,
            required: true,
            options: vec![],
            default: Some(json!(0)),
            value: None,
            hint: None,
            editable: true,
            placeholder: Some("0".into()),
        }],
        submit_op: Some("save".into()),
        submit_label: None,
    });
    roundtrip(&KvNode {
        base: NodeBase::default(),
        pairs: vec![KvPair {
            key: "k".into(),
            value: json!({"nested": true}),
            kind: Some("link".into()),
        }],
    });
    roundtrip(&MarkdownNode {
        base: NodeBase::default(),
        text: "# hi".into(),
    });
    roundtrip(&EmbedNode {
        base: NodeBase {
            props: Map::from_iter([("k".to_string(), json!(1))]),
            ..NodeBase::default()
        },
        component: "viz".into(),
    });
}

#[test]
fn unknown_fields_are_ignored() {
    // Forward-compat: newer drivers may add fields; v1 hosts ignore them.
    let node: ViewNode =
        serde_json::from_str(r#"{ "kind": "kv", "future_field": 42, "pairs": [] }"#).unwrap();
    assert!(matches!(node, ViewNode::Kv(_)));

    let manifest: DriverManifest = serde_json::from_str(
        r#"{ "name": "x", "title": "X", "version": "1",
             "future_section": { "a": 1 } }"#,
    )
    .unwrap();
    assert_eq!(manifest.name, "x");
}

// ---------- wire ----------

#[test]
fn request_ops_roundtrip() {
    let reqs = [
        Request::Manifest,
        Request::Views,
        Request::View {
            view: "board".into(),
            params: Map::from_iter([("filter".to_string(), json!("open"))]),
        },
        Request::Query {
            query: "items".into(),
            params: Map::new(),
        },
    ];
    for req in &reqs {
        roundtrip(req);
    }
    // Exact wire shapes.
    assert_eq!(
        serde_json::to_string(&Request::Manifest).unwrap(),
        r#"{"op":"manifest"}"#
    );
    assert_eq!(
        serde_json::to_string(&Request::Views).unwrap(),
        r#"{"op":"views"}"#
    );
    assert_eq!(
        serde_json::to_string(&Request::View {
            view: "board".into(),
            params: Map::new(),
        })
        .unwrap(),
        r#"{"op":"view","view":"board"}"#
    );
}

#[test]
fn parse_request_accepts_ndjson_lines() {
    assert!(matches!(
        parse_request(r#"{"op":"manifest"}"#).unwrap(),
        Request::Manifest
    ));
    assert!(matches!(
        parse_request(r#"{"op":"view","view":"board","params":{"x":1}}"#).unwrap(),
        Request::View { .. }
    ));
    // Unknown op → WireError::UnknownOp.
    assert!(matches!(
        parse_request(r#"{"op":"explode"}"#),
        Err(pavo_protocol::WireError::UnknownOp { .. })
    ));
    // Missing op.
    assert!(parse_request(r#"{"view":"board"}"#).is_err());
    // Malformed JSON.
    assert!(parse_request("not json").is_err());
}

#[test]
fn response_shapes() {
    let ok = Response::ok(json!({"hello": "world"}));
    assert!(ok.ok);
    assert_eq!(ok.error, None);
    let line = response_line(&ok).unwrap();
    assert_eq!(line, r#"{"ok":true,"result":{"hello":"world"}}"#);

    let err = Response::err("driver exploded");
    let line = response_line(&err).unwrap();
    assert_eq!(line, r#"{"ok":false,"error":"driver exploded"}"#);

    // Round-trip both arms.
    roundtrip(&ok);
    roundtrip(&err);

    // Typed helper.
    let typed = Response::ok_typed(&json!({"a": 1}));
    assert_eq!(typed.result_as::<Value>().unwrap(), json!({"a": 1}));
}

#[test]
fn result_types_roundtrip() {
    let summary = ViewSummary::from(
        &serde_json::from_str::<ViewSpec>(
            r#"{ "id": "board", "title": "Board", "route": "/b",
                 "queries": [ { "name": "items", "op": "query" } ],
                 "root": { "kind": "kv", "pairs": [] } }"#,
        )
        .unwrap(),
    );
    assert_eq!(summary.id, "board");
    assert_eq!(summary.queries, vec!["items".to_string()]);
    roundtrip(&summary);

    let view_result = ViewResult {
        view: serde_json::from_str::<ViewSpec>(
            r#"{ "id": "v", "title": "V", "route": "/v",
                 "root": { "kind": "markdown", "text": "x" } }"#,
        )
        .unwrap(),
        data: Map::from_iter([("items".to_string(), json!([1, 2, 3]))]),
    };
    roundtrip(&view_result);

    roundtrip(&QueryResult {
        query: "items".into(),
        data: json!({"rows": []}),
    });
}

// ---------- renderer plugins ----------

#[test]
fn manifest_renderers_roundtrip() {
    // A renderer-plugin driver: zero views, one `renderers` claim.
    let m: DriverManifest = serde_json::from_str(
        r#"{
          "name": "viz-graph", "title": "Graph viz", "version": "0.1.0",
          "verbs": ["manifest", "serve"], "capabilities": ["pavo"],
          "contributes": {}, "views": [],
          "renderers": [{
            "id": "graph", "kinds": ["graph"], "priority": 50,
            "entry": "graph",
            "surfaces": {
              "cli": { "formats": ["svg", "ascii"] },
              "tui": true,
              "web": { "component": "viz-graph", "island": true }
            }
          }]
        }"#,
    )
    .unwrap();
    assert_eq!(m.renderers.len(), 1);
    let r = &m.renderers[0];
    assert_eq!(r.id, "graph");
    assert_eq!(r.kinds, vec!["graph".to_string()]);
    assert_eq!(r.priority, 50);
    assert!(r.surfaces.tui);
    assert!(r.surfaces.web.as_ref().unwrap().island);
    roundtrip(&m);

    // Minimal decl: kinds+surfaces required; priority/entry default.
    let m2: DriverManifest = serde_json::from_str(
        r#"{ "name": "x", "title": "X", "version": "0",
             "renderers": [{ "id": "any", "kinds": ["*"],
                             "surfaces": { "tui": true } }] }"#,
    )
    .unwrap();
    let r2 = &m2.renderers[0];
    assert_eq!(r2.priority, 100);
    assert!(r2.entry.is_none());
    assert!(r2.surfaces.cli.is_none());
    assert!(r2.surfaces.web.is_none());
    roundtrip(&m2);

    // No renderers key → empty vec (old manifests unaffected).
    let m3: DriverManifest =
        serde_json::from_str(r#"{ "name": "y", "title": "Y", "version": "0" }"#).unwrap();
    assert!(m3.renderers.is_empty());
}

#[test]
fn render_request_roundtrips_and_parses() {
    let req = Request::Render {
        kind: "graph".into(),
        node: json!({"kind": "graph", "nodes": [], "edges": [],
                     "future_field": {"kept": true}}),
        data: Map::from_iter([("deps".to_string(), json!({"nodes": []}))]),
        surface: "cli".into(),
        format: Some("svg".into()),
    };
    roundtrip(&req);
    let line = serde_json::to_string(&req).unwrap();
    assert!(line.contains(r#""op":"render""#), "{line}");
    assert!(line.contains(r#""surface":"cli""#), "{line}");
    assert!(line.contains(r#""format":"svg""#), "{line}");
    // format omitted → absent (skip_serializing_if).
    let req2 = Request::Render {
        kind: "graph".into(),
        node: json!({}),
        data: Map::new(),
        surface: "tui".into(),
        format: None,
    };
    let line2 = serde_json::to_string(&req2).unwrap();
    assert!(!line2.contains("format"), "{line2}");
    assert!(matches!(
        parse_request(&line).unwrap(),
        Request::Render { .. }
    ));
}

#[test]
fn render_model_wire_shapes() {
    use pavo_protocol::RenderModel;
    // Tagged union shapes match wire.schema.json renderModel.
    let cases = [
        (RenderModel::text(["a"]), r#"{"type":"text","lines":["a"]}"#),
        (
            RenderModel::rows(vec!["h".into()], vec![vec!["c".into()]]),
            r#"{"type":"rows","headers":["h"],"rows":[["c"]]}"#,
        ),
        (
            RenderModel::Svg {
                markup: "<svg/>".into(),
            },
            r#"{"type":"svg","markup":"<svg/>"}"#,
        ),
    ];
    for (model, expected) in cases {
        assert_eq!(serde_json::to_string(&model).unwrap(), expected);
        roundtrip(&model);
    }
}
