//! Property tests for pavo-core pure logic — ViewNode tree helpers,
//! chrome merge determinism, renderer registry, and the TTL cache.
#![allow(clippy::unwrap_used)]

use std::collections::{BTreeSet, HashMap};
use std::time::Duration;

use pavo_core::renderers::RendererRegistry;
use pavo_core::viewnode;
use pavo_core::{merge_chrome, order_drivers, DepsError, TtlCache};
use pavo_protocol::manifest::*;
use pavo_protocol::view_node::*;
use proptest::prelude::*;
use proptest::strategy::BoxedStrategy;
use serde_json::{Map, Value};

// ---------- strategies ----------

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
        prop::collection::vec(json_value(depth - 1), 0..3).prop_map(Value::Array),
        prop::collection::hash_map(text(), json_value(depth - 1), 0..3)
            .prop_map(|m| Value::Object(m.into_iter().collect())),
    ]
    .boxed()
}

fn base_obj() -> BoxedStrategy<Map<String, Value>> {
    (
        prop::option::of(text()),
        prop::option::of(text()),
        prop::option::of(text()),
        prop::collection::hash_map(text(), json_value(1), 0..3),
    )
        .prop_map(|(id, title, query, props)| {
            let mut m = Map::new();
            for (k, v) in [("id", id), ("title", title), ("query", query)] {
                if let Some(v) = v {
                    m.insert(k.into(), Value::from(v));
                }
            }
            if !props.is_empty() {
                m.insert("props".into(), Value::Object(props.into_iter().collect()));
            }
            m
        })
        .boxed()
}

/// A ViewNode JSON object in valid wire shape (always parses).
fn node_json(depth: usize) -> BoxedStrategy<Value> {
    let leaf = prop_oneof![
        // kv — pairs optional
        leaf("kv", |m| {
            m.insert(
                "pairs".into(),
                Value::Array(
                    (0..3)
                        .map(|_| serde_json::json!({"key": "k", "value": 1}))
                        .collect(),
                ),
            );
        }),
        leaf("markdown", |m| {
            m.insert("text".into(), Value::from("hello"));
        }),
        leaf("table", |m| {
            m.insert(
                "columns".into(),
                serde_json::json!([{"key": "a", "title": "A"}]),
            );
            m.insert("rows".into(), serde_json::json!([["x"]]));
        }),
        leaf("tree", |m| {
            m.insert("items".into(), serde_json::json!([{"label": "root"}]));
        }),
        leaf("graph", |_| {}),
        leaf("form", |_| {}),
        leaf("embed", |m| {
            m.insert("component".into(), Value::from("viz"));
        }),
    ];
    if depth == 0 {
        return leaf.boxed();
    }
    let container = |kind: &'static str| {
        (
            base_obj(),
            prop::collection::vec(node_json(depth - 1), 0..3),
            prop::option::of(any::<u32>()),
        )
            .prop_map(move |(mut base, children, gap)| {
                base.insert("kind".into(), Value::from(kind));
                base.insert("children".into(), Value::Array(children));
                if kind != "panel" {
                    if let Some(g) = gap {
                        base.insert("gap".into(), Value::from(g));
                    }
                }
                Value::Object(base)
            })
            .boxed()
    };
    prop_oneof![4 => leaf, 1 => container("panel"), 1 => container("row"), 1 => container("column")]
        .boxed()
}

/// Build a leaf node object: base fields + kind + payload from `fill`.
fn leaf(kind: &'static str, fill: fn(&mut Map<String, Value>)) -> BoxedStrategy<Value> {
    base_obj()
        .prop_map(move |mut base| {
            base.insert("kind".into(), Value::from(kind));
            fill(&mut base);
            Value::Object(base)
        })
        .boxed()
}

fn view_node() -> BoxedStrategy<ViewNode> {
    node_json(3)
        .prop_map(|v| serde_json::from_value(v).unwrap())
        .boxed()
}

fn count_nodes(root: &ViewNode) -> usize {
    1 + viewnode::children(root)
        .iter()
        .map(count_nodes)
        .sum::<usize>()
}

// ---------- chrome merge strategies ----------

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

/// A `Contributes` with entries in every slot — built-ins and the
/// platform four — plus forward-compat `extra`.
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
                    menus: Vec::new(),
                    extra: extra.into_iter().collect(),
                }
            },
        )
        .boxed()
}

fn manifest() -> BoxedStrategy<DriverManifest> {
    contributes()
        .prop_map(|contributes| DriverManifest {
            name: "m".into(),
            title: "M".into(),
            icon: None,
            version: "0".into(),
            verbs: vec!["serve".into()],
            capabilities: vec![],
            deps: None,
            contributes,
            views: vec![],
            renderers: vec![],
            actions: vec![],
        })
        .boxed()
}

/// `(kind, manifest)` pairs — kinds drawn from a small alphabet so
/// collisions/ordering actually get exercised.
fn drivers() -> BoxedStrategy<Vec<(String, DriverManifest)>> {
    prop::collection::vec(
        (
            prop::sample::select(vec!["aaa", "bbb", "ccc", "drv"]).prop_map(str::to_string),
            manifest(),
        ),
        0..4,
    )
    .boxed()
}

// ---------- tests ----------

proptest! {
    /// walk() visits every node exactly once, root first, and the
    /// visited count equals the recursive node count.
    #[test]
    fn walk_visits_each_node_once(node in view_node()) {
        let mut count = 0usize;
        let mut first_kind = String::new();
        viewnode::walk(&node, &mut |n| {
            if count == 0 {
                first_kind = viewnode::kind_of(n).to_string();
            }
            count += 1;
        });
        prop_assert_eq!(count, count_nodes(&node));
        prop_assert_eq!(first_kind, viewnode::kind_of(&node));
    }

    /// collect_queries returns exactly the set of query names any
    /// node binds — nothing more, nothing less.
    #[test]
    fn collect_queries_is_complete_and_exact(node in view_node()) {
        let mut expected = BTreeSet::new();
        viewnode::walk(&node, &mut |n| {
            if let Some(q) = &n.base().query {
                expected.insert(q.clone());
            }
        });
        prop_assert_eq!(&viewnode::collect_queries(&node), &expected);
    }

    /// find_nodes returns precisely the nodes of the requested kind,
    /// in walk (pre-order) order.
    #[test]
    fn find_nodes_matches_walk_filter(node in view_node(), kind in prop::sample::select(vec!["panel","row","column","kv","markdown","table","tree","graph","form","embed","bogus"])) {
        let found = viewnode::find_nodes(&node, kind);
        let mut expected = 0usize;
        viewnode::walk(&node, &mut |n| {
            if viewnode::kind_of(n) == kind {
                expected += 1;
            }
        });
        prop_assert_eq!(found.len(), expected);
        for n in found {
            prop_assert_eq!(viewnode::kind_of(n), kind);
        }
        // children() is empty for every non-container kind.
        let container_kinds = ["panel", "row", "column"];
        if !container_kinds.contains(&viewnode::kind_of(&node)) {
            prop_assert!(viewnode::children(&node).is_empty());
        }
    }

    /// to_kv_fallback never panics, preserves node fields, and always
    /// yields a "kind" pair naming the original kind.
    #[test]
    fn to_kv_fallback_is_total_and_preserves(node in view_node()) {
        let kv = viewnode::to_kv_fallback(&node);
        prop_assert_eq!(&kv.base.id, &node.base().id);
        prop_assert_eq!(&kv.base.title, &node.base().title);
        prop_assert_eq!(&kv.base.query, &node.base().query);
        let kind_pair = kv
            .pairs
            .iter()
            .find(|p| p.key == "kind")
            .map(|p| p.value.as_str().unwrap_or(""));
        prop_assert_eq!(kind_pair, Some(viewnode::kind_of(&node)));
        // The fallback serializes back to a valid KvNode.
        let v = serde_json::to_value(&kv).unwrap();
        let back: KvNode = serde_json::from_value(v).unwrap();
        prop_assert_eq!(&back, &kv);
    }

    /// kv_fallback_json on arbitrary JSON: always emits kind="kv" and
    /// a pairs array of {key:string,value} objects; non-objects get
    /// empty pairs.
    #[test]
    fn kv_fallback_json_total(v in json_value(4)) {
        let out = viewnode::kv_fallback_json(&v);
        prop_assert_eq!(out.get("kind").and_then(Value::as_str), Some("kv"));
        let pairs = out.get("pairs").and_then(Value::as_array).unwrap();
        for pair in pairs {
            prop_assert!(pair.get("key").and_then(Value::as_str).is_some());
            prop_assert!(pair.get("value").is_some());
        }
        if !v.is_object() {
            prop_assert!(pairs.is_empty());
        }
        // Output always deserializes as a KvNode.
        let kv: Result<KvNode, _> = serde_json::from_value(out);
        prop_assert!(kv.is_ok());
    }

    /// merge_chrome is deterministic, total, namespaces ids, and
    /// emits sorted slots (priority asc, driver asc, id asc).
    #[test]
    fn merge_chrome_deterministic_sorted_namespaced(ds in drivers()) {
        let refs: Vec<(&str, &DriverManifest)> = ds
            .iter()
            .map(|(k, m)| (k.as_str(), m))
            .collect();
        let a = merge_chrome(refs.iter().copied());
        let b = merge_chrome(refs.iter().copied());
        prop_assert_eq!(&a, &b, "merge must be deterministic");

        let check = |entries: &[pavo_core::SlotEntry]| {
            for w in entries.windows(2) {
                let (x, y) = (&w[0], &w[1]);
                let ordered = (x.contribution.priority, &x.driver, &x.contribution.id)
                    <= (y.contribution.priority, &y.driver, &y.contribution.id);
                assert!(ordered, "slot not sorted: {x:?} > {y:?}");
            }
            for e in entries {
                assert!(
                    e.contribution.id.starts_with(&format!("{}:", e.driver)),
                    "id {} not namespaced by {}",
                    e.contribution.id,
                    e.driver
                );
            }
        };
        check(&a.navbar);
        check(&a.dashboard);
        check(&a.statusbar);
        check(&a.routes);
        check(&a.sidebar);
        check(&a.header_actions);
        check(&a.notifications);
        check(&a.jobs);
        for entries in a.extra.values() {
            check(entries);
        }
        // Every contribution from the input shows up exactly once.
        let total_in: usize = ds.iter().map(|(_, m)| {
            m.contributes.navbar.len()
                + m.contributes.dashboard.len()
                + m.contributes.statusbar.len()
                + m.contributes.routes.len()
                + m.contributes.sidebar.len()
                + m.contributes.header_actions.len()
                + m.contributes.notifications.len()
                + m.contributes.jobs.len()
                + m.contributes.extra.values().map(Vec::len).sum::<usize>()
        }).sum();
        let total_out = a.navbar.len()
            + a.dashboard.len()
            + a.statusbar.len()
            + a.routes.len()
            + a.sidebar.len()
            + a.header_actions.len()
            + a.notifications.len()
            + a.jobs.len()
            + a.extra.values().map(Vec::len).sum::<usize>();
        prop_assert_eq!(total_in, total_out);
    }

    /// order_drivers is deterministic; on success it emits each
    /// unique kind exactly once with every dep edge satisfied; on
    /// failure the error is a *valid* Missing or Cycle.
    #[test]
    fn order_drivers_topological(ds in prop::collection::vec(dep_driver(), 1..6)) {
        let refs: Vec<(&str, Option<&DriverDeps>)> = ds
            .iter()
            .map(|(k, d)| (k.as_str(), Some(d)))
            .collect();
        let a = order_drivers(refs.iter().copied());
        let b = order_drivers(refs.iter().copied());
        prop_assert_eq!(&a, &b, "order must be deterministic");

        // order() dedups by kind, first occurrence wins — mirror it.
        let mut first: HashMap<&str, &DriverDeps> = HashMap::new();
        for (k, d) in &ds {
            first.entry(k.as_str()).or_insert(d);
        }
        let kinds: BTreeSet<&str> = first.keys().copied().collect();

        match &a {
            Ok(order) => {
                // A permutation of the unique kinds.
                let mut got = order.clone();
                got.sort();
                let mut want: Vec<String> =
                    kinds.iter().map(|s| s.to_string()).collect();
                want.sort();
                prop_assert_eq!(got, want);
                let pos: HashMap<&str, usize> = order
                    .iter()
                    .enumerate()
                    .map(|(i, k)| (k.as_str(), i))
                    .collect();
                for (kind, deps) in &first {
                    for dep in deps.requires.iter().chain(&deps.optional) {
                        if kinds.contains(dep.as_str()) {
                            prop_assert!(
                                pos[dep.as_str()] < pos[*kind],
                                "dep {dep} must precede {kind}"
                            );
                        }
                    }
                }
            }
            Err(DepsError::Missing { driver, dep }) => {
                // The reported missing dep is real: `driver` requires
                // it and no input kind matches.
                let d = first[driver.as_str()];
                prop_assert!(d.requires.contains(dep));
                prop_assert!(!kinds.contains(dep.as_str()));
            }
            Err(DepsError::Cycle { drivers }) => {
                prop_assert!(!drivers.is_empty());
                for k in drivers {
                    prop_assert!(kinds.contains(k.as_str()));
                }
            }
        }
    }

    /// RendererRegistry: deterministic, entries sorted by id, and a
    /// claimed kind resolves on its declared surfaces only.
    #[test]
    fn renderer_registry_deterministic(ds in prop::collection::vec((text(), renderer_manifest()), 0..3)) {
        let pairs: Vec<(&str, &DriverManifest)> = ds.iter().map(|(k, m)| (k.as_str(), m)).collect();
        let r1 = RendererRegistry::from_manifests(pairs.iter().copied());
        let r2 = RendererRegistry::from_manifests(pairs.iter().copied());
        prop_assert_eq!(r1.entries(), r2.entries());
        // entries sorted by namespaced id
        let ids: Vec<&str> = r1.entries().iter().map(|b| b.id.as_str()).collect();
        let mut sorted = ids.clone();
        sorted.sort_unstable();
        prop_assert_eq!(ids, sorted);
        // handles() agrees with for_kind()
        for kind in ["graph", "table", "kv", "anything"] {
            for surface in ["cli", "tui", "web", "watch"] {
                for fmt in [None, Some("svg"), Some("text")] {
                    prop_assert_eq!(
                        r1.handles(kind, surface, fmt),
                        r1.for_kind(kind, surface, fmt).is_some()
                    );
                }
            }
        }
    }

    /// TtlCache: a random op sequence against a HashMap model — with
    /// a long TTL nothing expires mid-test, so the cache must mirror
    /// last-write-wins + invalidate + clear exactly.
    #[test]
    fn ttl_cache_mirrors_model(ops in prop::collection::vec(cache_op(), 0..30)) {
        let cache = TtlCache::new(Duration::from_secs(3600));
        let mut model: HashMap<String, i64> = HashMap::new();
        for op in ops {
            match op {
                Op::Insert(k, v) => {
                    cache.insert(k.clone(), v);
                    model.insert(k, v);
                }
                Op::Invalidate(k) => {
                    cache.invalidate(&k);
                    model.remove(&k);
                }
                Op::Clear => {
                    cache.clear();
                    model.clear();
                }
                Op::Check(k) => {
                    prop_assert_eq!(cache.get(&k), model.get(&k).copied());
                }
            }
            prop_assert_eq!(cache.len(), model.len());
            prop_assert_eq!(cache.is_empty(), model.is_empty());
        }
    }
}

#[derive(Clone, Debug)]
enum Op {
    Insert(String, i64),
    Invalidate(String),
    Clear,
    Check(String),
}

fn cache_op() -> BoxedStrategy<Op> {
    let key = || prop::sample::select(vec!["a".to_string(), "b".to_string(), "c".to_string()]);
    prop_oneof![
        (key(), any::<i64>()).prop_map(|(k, v)| Op::Insert(k, v)),
        key().prop_map(Op::Invalidate),
        Just(Op::Clear),
        key().prop_map(Op::Check),
    ]
    .boxed()
}

/// `(kind, deps)` pairs for order_drivers — kinds from a small
/// alphabet (duplicates exercise first-wins dedup), dep names drawn
/// from the same alphabet plus an always-absent "ghost".
fn dep_driver() -> BoxedStrategy<(String, DriverDeps)> {
    let kind = || prop::sample::select(vec!["a", "b", "c", "d"]).prop_map(str::to_string);
    let dep_name =
        || prop::sample::select(vec!["a", "b", "c", "d", "ghost"]).prop_map(str::to_string);
    (
        kind(),
        (
            prop::collection::vec(dep_name(), 0..3),
            prop::collection::vec(dep_name(), 0..3),
        )
            .prop_map(|(requires, optional)| DriverDeps { requires, optional }),
    )
        .boxed()
}

fn renderer_manifest() -> BoxedStrategy<DriverManifest> {
    (
        text(),
        prop::collection::vec(text(), 0..3),
        any::<bool>(),
        any::<bool>(),
        any::<i32>(),
    )
        .prop_map(|(id, kinds, cli, tui, priority)| DriverManifest {
            name: "m".into(),
            title: "M".into(),
            icon: None,
            version: "0".into(),
            verbs: vec!["serve".into()],
            capabilities: vec![],
            deps: None,
            contributes: Contributes::default(),
            views: vec![],
            renderers: vec![RendererDecl {
                id,
                kinds,
                surfaces: RendererSurfaces {
                    cli: cli.then(|| CliSurface { formats: vec![] }),
                    tui,
                    web: None,
                },
                priority,
                entry: None,
            }],
            actions: vec![],
        })
        .boxed()
}
