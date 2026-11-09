//! Property tests for the wire contract — serde round-trips and
//! parse-totality over generated inputs. The protocol's forward-compat
//! rule means *any* JSON must parse-or-fail cleanly, never panic.
#![allow(clippy::unwrap_used)]

use pavo_protocol::manifest::*;
use pavo_protocol::view_node::*;
use pavo_protocol::wire::*;
use pavo_protocol::{Align, RenderModel};
use proptest::prelude::*;
use proptest::strategy::{BoxedStrategy, Strategy};
use serde_json::{Map, Value};

// ---------- strategies ----------

fn text() -> BoxedStrategy<String> {
    prop::collection::vec(any::<char>(), 0..24)
        .prop_map(|cs| cs.into_iter().collect())
        .boxed()
}

/// Arbitrary JSON value, recursion bounded by `depth`.
fn json_value(depth: usize) -> BoxedStrategy<Value> {
    if depth == 0 {
        return prop_oneof![
            Just(Value::Null),
            any::<bool>().prop_map(Value::Bool),
            any::<i64>().prop_map(|n| Value::from(n.clamp(-1_000_000, 1_000_000))),
            text().prop_map(Value::String),
        ]
        .boxed();
    }
    prop_oneof![
        Just(Value::Null),
        any::<bool>().prop_map(Value::Bool),
        any::<i64>().prop_map(|n| Value::from(n.clamp(-1_000_000, 1_000_000))),
        text().prop_map(Value::String),
        prop::collection::vec(json_value(depth - 1), 0..4).prop_map(Value::Array),
        prop::collection::hash_map(text(), json_value(depth - 1), 0..4)
            .prop_map(|m| Value::Object(m.into_iter().collect())),
    ]
    .boxed()
}

fn props() -> BoxedStrategy<Map<String, Value>> {
    prop::collection::hash_map(text(), json_value(2), 0..4)
        .prop_map(|m| m.into_iter().collect())
        .boxed()
}

fn base() -> BoxedStrategy<NodeBase> {
    (
        prop::option::of(text()),
        prop::option::of(text()),
        prop::option::of(text()),
        prop::option::of(text()),
        props(),
    )
        .prop_map(|(id, title, query, description, props)| NodeBase {
            id,
            title,
            query,
            description,
            props,
        })
        .boxed()
}

fn opt_str() -> BoxedStrategy<Option<String>> {
    prop::option::of(text()).boxed()
}

fn align() -> BoxedStrategy<Align> {
    prop_oneof![Just(Align::Left), Just(Align::Center), Just(Align::Right)].boxed()
}

fn field_kind() -> BoxedStrategy<FormFieldKind> {
    prop_oneof![
        Just(FormFieldKind::Text),
        Just(FormFieldKind::Textarea),
        Just(FormFieldKind::Number),
        Just(FormFieldKind::Bool),
        Just(FormFieldKind::Select),
    ]
    .boxed()
}

/// A well-formed ViewNode tree, recursion bounded by `depth`.
fn view_node(depth: usize) -> BoxedStrategy<ViewNode> {
    let leaf = prop_oneof![
        markdown_node(),
        kv_node(),
        embed_node(),
        table_node(),
        tree_node(1),
        graph_node(),
        form_node(),
    ];
    if depth == 0 {
        return leaf.boxed();
    }
    let children = || prop::collection::vec(view_node(depth - 1), 0..3);
    prop_oneof![
        3 => leaf,
        1 => (base(), children()).prop_map(|(base, children)| ViewNode::Panel(PanelNode { base, children })),
        1 => (base(), children(), prop::option::of(any::<u32>())).prop_map(|(base, children, gap)| ViewNode::Row(RowNode { base, children, gap })),
        1 => (base(), children(), prop::option::of(any::<u32>())).prop_map(|(base, children, gap)| ViewNode::Column(ColumnNode { base, children, gap })),
    ]
    .boxed()
}

fn markdown_node() -> BoxedStrategy<ViewNode> {
    (base(), text())
        .prop_map(|(base, text)| ViewNode::Markdown(MarkdownNode { base, text }))
        .boxed()
}

fn kv_node() -> BoxedStrategy<ViewNode> {
    (
        base(),
        prop::collection::vec((text(), json_value(2), prop::option::of(text())), 0..4),
    )
        .prop_map(|(base, triples)| {
            ViewNode::Kv(KvNode {
                base,
                pairs: triples
                    .into_iter()
                    .map(|(key, value, kind)| KvPair { key, value, kind })
                    .collect(),
            })
        })
        .boxed()
}

fn embed_node() -> BoxedStrategy<ViewNode> {
    // Component props ride in base.props (the single "props" wire key).
    (base(), text())
        .prop_map(|(base, component)| ViewNode::Embed(EmbedNode { base, component }))
        .boxed()
}

fn table_node() -> BoxedStrategy<ViewNode> {
    (
        base(),
        prop::collection::vec(
            (
                text(),
                text(),
                prop::option::of(align()),
                prop::option::of(any::<u32>()),
            ),
            0..4,
        ),
        prop::collection::vec(prop::collection::vec(json_value(1), 0..4), 0..4),
        any::<bool>(),
    )
        .prop_map(|(base, cols, rows, sortable)| {
            ViewNode::Table(TableNode {
                base,
                columns: cols
                    .into_iter()
                    .map(|(key, title, align, width)| TableColumn {
                        key,
                        title,
                        align,
                        width,
                    })
                    .collect(),
                rows,
                sortable,
            })
        })
        .boxed()
}

fn tree_node(depth: usize) -> BoxedStrategy<ViewNode> {
    fn item(depth: usize) -> BoxedStrategy<TreeItem> {
        // depth 0 → leaf items only (children empty); deeper levels
        // nest one more generation. Guard keeps strategy construction
        // finite.
        let children: BoxedStrategy<Vec<TreeItem>> = if depth == 0 {
            Just(Vec::new()).boxed()
        } else {
            prop::collection::vec(item(depth - 1), 0..3).boxed()
        };
        (
            prop::option::of(text()),
            text(),
            prop::option::of(text()),
            prop::option::of(text()),
            children,
            props(),
        )
            .prop_map(|(id, label, detail, icon, children, props)| TreeItem {
                id,
                label,
                detail,
                icon,
                children,
                props,
            })
            .boxed()
    }
    (
        base(),
        prop::collection::vec(item(depth), 0..4),
        any::<bool>(),
    )
        .prop_map(|(base, items, expandable)| {
            ViewNode::Tree(TreeNode {
                base,
                items,
                expandable,
            })
        })
        .boxed()
}

fn graph_node() -> BoxedStrategy<ViewNode> {
    (
        base(),
        prop::collection::vec((text(), text(), prop::option::of(text()), props()), 0..4),
        prop::collection::vec((text(), text(), prop::option::of(text())), 0..4),
        opt_str(),
    )
        .prop_map(|(base, nodes, edges, layout)| {
            ViewNode::Graph(GraphNode {
                base,
                nodes: nodes
                    .into_iter()
                    .map(|(id, label, kind, props)| GraphNodeItem {
                        id,
                        label,
                        kind,
                        props,
                    })
                    .collect(),
                edges: edges
                    .into_iter()
                    .map(|(from, to, label)| GraphEdge { from, to, label })
                    .collect(),
                layout,
            })
        })
        .boxed()
}

/// `Option<Value>` can't distinguish `Some(Null)` from `None` on the
/// wire (serde maps explicit `null` → `None`), so generators for
/// those fields emit only non-null payloads.
fn opt_json(depth: usize) -> BoxedStrategy<Option<Value>> {
    prop::option::of(json_value(depth).prop_filter("non-null json", |v| !v.is_null())).boxed()
}

fn form_node() -> BoxedStrategy<ViewNode> {
    (
        base(),
        prop::collection::vec(
            (
                text(),
                text(),
                field_kind(),
                any::<bool>(),
                prop::collection::vec(text(), 0..4),
                opt_json(1),
                opt_json(1),
                prop::option::of(text()),
            ),
            0..4,
        ),
        opt_str(),
        opt_str(),
    )
        .prop_map(|(base, fields, submit_op, submit_label)| {
            ViewNode::Form(FormNode {
                base,
                fields: fields
                    .into_iter()
                    .map(
                        |(key, label, kind, required, options, default, value, hint)| FormField {
                            key,
                            label,
                            kind,
                            required,
                            options,
                            default,
                            value,
                            hint,
                            editable: true,
                            placeholder: None,
                        },
                    )
                    .collect(),
                submit_op,
                submit_label,
            })
        })
        .boxed()
}

fn contribution() -> BoxedStrategy<Contribution> {
    (
        text(),
        text(),
        text(),
        any::<i32>(),
        prop::option::of(text()),
    )
        .prop_map(|(id, title, route, priority, icon)| Contribution {
            id,
            title,
            route,
            priority,
            icon,
        })
        .boxed()
}

fn contributes() -> BoxedStrategy<Contributes> {
    (
        prop::collection::vec(contribution(), 0..3),
        prop::collection::vec(contribution(), 0..3),
        prop::collection::vec(contribution(), 0..3),
        prop::collection::vec(contribution(), 0..3),
        prop::collection::vec(contribution(), 0..3),
        prop::collection::vec(contribution(), 0..3),
        prop::collection::vec(contribution(), 0..3),
        prop::collection::vec(contribution(), 0..3),
        prop::collection::hash_map(text(), prop::collection::vec(contribution(), 0..2), 0..2),
    )
        .prop_map(
            |(
                navbar,
                dashboard,
                statusbar,
                routes,
                sidebar,
                header_actions,
                notifications,
                jobs,
                extra,
            )| {
                Contributes {
                    navbar,
                    dashboard,
                    statusbar,
                    routes,
                    sidebar,
                    header_actions,
                    notifications,
                    jobs,
                    extra: extra.into_iter().collect(),
                }
            },
        )
        .boxed()
}

fn view_spec() -> BoxedStrategy<ViewSpec> {
    (
        text(),
        text(),
        text(),
        prop::option::of(text()),
        prop::collection::vec(
            (text(), text(), props(), prop::option::of(text())).prop_map(
                |(name, op, params, driver)| QueryBinding {
                    name,
                    op,
                    driver,
                    params,
                },
            ),
            0..3,
        ),
        view_node(2),
    )
        .prop_map(|(id, title, route, description, queries, root)| ViewSpec {
            id,
            title,
            route,
            description,
            queries,
            root,
        })
        .boxed()
}

fn renderer_decl() -> BoxedStrategy<RendererDecl> {
    (
        text(),
        prop::collection::vec(text(), 0..3),
        any::<bool>(),
        any::<bool>(),
        any::<bool>(),
        any::<i32>(),
        prop::option::of(text()),
    )
        .prop_map(|(id, kinds, cli, tui, web, priority, entry)| RendererDecl {
            id,
            kinds,
            surfaces: RendererSurfaces {
                cli: cli.then_some(CliSurface {
                    formats: Vec::new(),
                }),
                tui,
                web: web.then_some(WebSurface {
                    component: None,
                    island: false,
                }),
            },
            priority,
            entry,
        })
        .boxed()
}

fn driver_deps() -> BoxedStrategy<Option<DriverDeps>> {
    prop::option::of(
        (
            prop::collection::vec(text(), 0..3),
            prop::collection::vec(text(), 0..3),
        )
            .prop_map(|(requires, optional)| DriverDeps { requires, optional }),
    )
    .boxed()
}

fn manifest() -> BoxedStrategy<DriverManifest> {
    (
        text(),
        text(),
        prop::option::of(text()),
        text(),
        prop::collection::vec(text(), 0..4),
        prop::collection::vec(text(), 0..4),
        driver_deps(),
        contributes(),
        prop::collection::vec(view_spec(), 0..3),
        prop::collection::vec(renderer_decl(), 0..3),
    )
        .prop_map(
            |(
                name,
                title,
                icon,
                version,
                verbs,
                capabilities,
                deps,
                contributes,
                views,
                renderers,
            )| {
                DriverManifest {
                    name,
                    title,
                    icon,
                    version,
                    verbs,
                    capabilities,
                    deps,
                    contributes,
                    views,
                    renderers,
                }
            },
        )
        .boxed()
}

fn render_model() -> BoxedStrategy<RenderModel> {
    prop_oneof![
        (
            prop::collection::vec(text(), 0..4),
            prop::collection::vec(align(), 0..4),
            prop::collection::vec(prop::collection::vec(text(), 0..4), 0..4),
        )
            .prop_map(|(headers, aligns, rows)| RenderModel::Rows {
                headers,
                aligns,
                rows
            }),
        text().prop_map(|markup| RenderModel::Svg { markup }),
        prop::collection::vec(text(), 0..4).prop_map(|lines| RenderModel::Text { lines }),
        (text(), props())
            .prop_map(|(component, props)| RenderModel::Placeholder { component, props }),
    ]
    .boxed()
}

fn job_item() -> BoxedStrategy<pavo_protocol::JobItem> {
    (
        text(),
        text(),
        text(),
        prop::option::of(text()),
        prop::option::of(text()),
        prop::option::of(text()),
        props(),
    )
        .prop_map(
            |(id, title, status, detail, route, updated_at, extra)| pavo_protocol::JobItem {
                id,
                title,
                status,
                detail,
                route,
                updated_at,
                extra,
            },
        )
        .boxed()
}

fn notification_item() -> BoxedStrategy<pavo_protocol::NotificationItem> {
    (
        text(),
        text(),
        prop::option::of(text()),
        prop::option::of(text()),
        prop::option::of(text()),
        prop::option::of(text()),
        any::<bool>(),
        props(),
    )
        .prop_map(
            |(id, title, body, severity, route, created_at, read, extra)| {
                pavo_protocol::NotificationItem {
                    id,
                    title,
                    body,
                    severity,
                    route,
                    created_at,
                    read,
                    extra,
                }
            },
        )
        .boxed()
}

fn response() -> BoxedStrategy<Response> {
    prop_oneof![
        json_value(2).prop_map(Response::ok),
        text().prop_map(Response::err),
        (
            any::<bool>(),
            prop::option::of(json_value(2)),
            prop::option::of(text())
        )
            .prop_map(|(ok, result, error)| Response { ok, result, error }),
    ]
    .boxed()
}

// ---------- ViewNode round-trips ----------

proptest! {
    /// Any generated well-formed tree serializes, deserializes, and
    /// compares equal — the IR's typed round-trip is lossless.
    #[test]
    fn view_node_json_roundtrip(node in view_node(3)) {
        let v = serde_json::to_value(&node).unwrap();
        let back: ViewNode = serde_json::from_value(v.clone()).unwrap();
        prop_assert_eq!(&back, &node);
        // And a second serialize is byte-identical (stable wire shape).
        prop_assert_eq!(serde_json::to_value(&back).unwrap(), v);
    }

    /// Arbitrary JSON: ViewNode deserialization is total — it either
    /// parses or errors, never panics (proptest fails on panic).
    /// When it parses, the typed round-trip is lossless.
    #[test]
    fn arbitrary_json_never_panics_on_view_node(v in json_value(5)) {
        if let Ok(node) = serde_json::from_value::<ViewNode>(v.clone()) {
            let re = serde_json::to_value(&node).unwrap();
            let back: ViewNode = serde_json::from_value(re).unwrap();
            prop_assert_eq!(&back, &node);
        }
    }

    /// Manifests round-trip losslessly through JSON.
    #[test]
    fn manifest_roundtrip(m in manifest()) {
        let v = serde_json::to_value(&m).unwrap();
        let back: DriverManifest = serde_json::from_value(v).unwrap();
        prop_assert_eq!(&back, &m);
    }

    /// RenderModel variants round-trip through the tagged wire shape.
    #[test]
    fn render_model_roundtrip(m in render_model()) {
        let v = serde_json::to_value(&m).unwrap();
        let back: RenderModel = serde_json::from_value(v).unwrap();
        prop_assert_eq!(&back, &m);
    }

    /// Platform feed items round-trip losslessly (extra catch-all
    /// included).
    #[test]
    fn platform_items_roundtrip(
        job in job_item(),
        note in notification_item(),
    ) {
        let v = serde_json::to_value(&job).unwrap();
        let back: pavo_protocol::JobItem = serde_json::from_value(v).unwrap();
        prop_assert_eq!(&back, &job);
        let v = serde_json::to_value(&note).unwrap();
        let back: pavo_protocol::NotificationItem = serde_json::from_value(v).unwrap();
        prop_assert_eq!(&back, &note);
    }
}

// ---------- wire ops ----------

proptest! {
    /// Any JSON value as a request line: parse_request returns Ok or
    /// Err — never panics. When it parses, re-serializing and
    /// re-parsing yields an equal Request (stable wire shape).
    #[test]
    fn parse_request_total_on_arbitrary_json(v in json_value(4)) {
        let line = serde_json::to_string(&v).unwrap();
        if let Ok(req) = pavo_protocol::wire::parse_request(&line) {
            let reline = serde_json::to_string(&req).unwrap();
            let back = pavo_protocol::wire::parse_request(&reline).unwrap();
            prop_assert_eq!(&back, &req);
        }
    }

    /// parse_request on *arbitrary strings* — garbage in, Result out.
    #[test]
    fn parse_request_total_on_arbitrary_strings(s in ".{0,200}") {
        let _ = pavo_protocol::wire::parse_request(&s);
    }

    /// Every well-formed Request serializes to a single ndjson line
    /// that parse_request reads back equal.
    #[test]
    fn request_roundtrip(req in request_value()) {
        let line = serde_json::to_string(&req).unwrap();
        prop_assert!(!line.contains('\n'));
        let back = pavo_protocol::wire::parse_request(&line).unwrap();
        prop_assert_eq!(serde_json::to_value(&back).unwrap(), serde_json::to_value(&req).unwrap());
    }

    /// Responses serialize to one line and deserialize back equal
    /// through the raw JSON shape.
    #[test]
    fn response_wire_roundtrip(resp in response()) {
        let line = pavo_protocol::wire::response_line(&resp).unwrap();
        prop_assert!(!line.contains('\n'));
        let parsed: Response = serde_json::from_str(&line).unwrap();
        // `Some(Null)` and `None` are indistinguishable on the wire
        // (serde maps explicit null → None for Option fields), so
        // compare the semantic payload: ok flag, effective result,
        // and error.
        prop_assert_eq!(parsed.ok, resp.ok);
        prop_assert_eq!(parsed.error.clone(), resp.error);
        prop_assert_eq!(
            parsed.result.clone().unwrap_or(Value::Null),
            resp.result.clone().unwrap_or(Value::Null)
        );
        // result_as::<Value> recovers the payload verbatim.
        if let Some(result) = &resp.result {
            prop_assert_eq!(parsed.result_as::<Value>().unwrap(), result.clone());
        }
    }
}

/// Well-formed Request values for the round-trip property.
fn request_value() -> BoxedStrategy<Request> {
    prop_oneof![
        Just(Request::Manifest),
        Just(Request::Views),
        (text(), props()).prop_map(|(view, params)| Request::View { view, params }),
        (text(), props()).prop_map(|(query, params)| Request::Query { query, params }),
        (
            text(),
            json_value(3),
            props(),
            text(),
            prop::option::of(text())
        )
            .prop_map(|(kind, node, data, surface, format)| Request::Render {
                kind,
                node,
                data,
                surface,
                format,
            }),
        Just(Request::Jobs),
        Just(Request::Notifications),
    ]
    .boxed()
}
