#![allow(clippy::unwrap_used)]
//! Property + fuzz tests for pavo-testkit: transcript serde
//! round-trips, replay-driver construction on arbitrary recordings,
//! and a deterministic raw-line fuzz loop against a live fixture
//! session (the wire must degrade to `ok:false`, never wedge).

use std::time::Duration;

use pavo_protocol::manifest::{QueryBinding, ViewSpec};
use pavo_protocol::view_node::{NodeBase, TableNode, ViewNode};
use pavo_testkit::fixture::FixtureDriver;
use pavo_testkit::record::{load_transcript, replay_driver, save_transcript, Entry, Transcript};
use pavo_testkit::wire::Session;
use proptest::prelude::*;
use proptest::strategy::BoxedStrategy;
use serde_json::{json, Map, Value};

fn text() -> BoxedStrategy<String> {
    prop::collection::vec(any::<char>(), 0..24)
        .prop_map(|cs| cs.into_iter().collect())
        .boxed()
}

fn json_value(depth: usize) -> BoxedStrategy<Value> {
    if depth == 0 {
        return prop_oneof![
            Just(Value::Null),
            any::<bool>().prop_map(Value::Bool),
            any::<i64>().prop_map(|n| Value::from(n.clamp(-9999, 9999))),
            text().prop_map(Value::String),
        ]
        .boxed();
    }
    prop_oneof![
        Just(Value::Null),
        any::<bool>().prop_map(Value::Bool),
        any::<i64>().prop_map(|n| Value::from(n.clamp(-9999, 9999))),
        text().prop_map(Value::String),
        prop::collection::vec(json_value(depth - 1), 0..4).prop_map(Value::Array),
        prop::collection::hash_map(text(), json_value(depth - 1), 0..4)
            .prop_map(|m| Value::Object(m.into_iter().collect())),
    ]
    .boxed()
}

fn entry() -> BoxedStrategy<Entry> {
    (json_value(3), json_value(3))
        .prop_map(|(request, response)| Entry { request, response })
        .boxed()
}

fn transcript() -> BoxedStrategy<Transcript> {
    prop::collection::vec(entry(), 0..8)
        .prop_map(|entries| Transcript { entries })
        .boxed()
}

proptest! {
    /// Transcript serde round-trips arbitrary JSON payloads.
    #[test]
    fn transcript_json_roundtrip(t in transcript()) {
        let s = serde_json::to_string(&t).unwrap();
        let back: Transcript = serde_json::from_str(&s).unwrap();
        let (n, empty) = (t.len(), t.is_empty());
        prop_assert_eq!(back.len(), n);
        prop_assert_eq!(back.is_empty(), empty);
        prop_assert_eq!(back, t);
    }

    /// save/load round-trip through the filesystem.
    #[test]
    fn transcript_save_load_roundtrip(t in transcript()) {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("t.json");
        save_transcript(&t, &path).unwrap();
        let back = load_transcript(&path).unwrap();
        prop_assert_eq!(back, t.clone());
    }

    /// A transcript of arbitrary garbage still builds a replay
    /// fixture driver (the script just records the bytes).
    #[test]
    fn replay_driver_builds_on_arbitrary_transcript(t in transcript()) {
        let fx = replay_driver("fxprop", &t).unwrap();
        prop_assert!(fx.path().exists());
    }
}

fn spec(id: &str) -> ViewSpec {
    ViewSpec {
        id: id.to_string(),
        title: format!("View {id}"),
        route: format!("/fx/{id}"),
        description: None,
        queries: vec![QueryBinding {
            name: "items".to_string(),
            op: "query".to_string(),
            driver: None,
            params: Default::default(),
        }],
        root: ViewNode::Table(TableNode {
            base: NodeBase {
                query: Some("items".to_string()),
                ..Default::default()
            },
            ..Default::default()
        }),
        item_route: None,
        filter_param: None,
        refresh_ms: None,
    }
}

/// Deterministic PRNG (xorshift64*) — fuzz inputs reproducible.
struct Rng(u64);
impl Rng {
    fn next(&mut self) -> u64 {
        let mut x = self.0;
        x ^= x >> 12;
        x ^= x << 25;
        x ^= x >> 27;
        self.0 = x;
        x.wrapping_mul(0x2545F4914F6CDD1D)
    }
    fn below(&mut self, n: u64) -> u64 {
        self.next() % n
    }
}

/// Fuzz-style: throw pseudo-random lines at a live fixture session.
/// Every response must come back (or time out cleanly) — the driver
/// must never wedge the wire.
#[test]
fn session_survives_arbitrary_wire_lines() {
    let fx = FixtureDriver::builder("fxwire")
        .view(spec("board"))
        .query("items", json!([["a", 1]]))
        .build()
        .unwrap();
    let mut session = Session::spawn(fx.path()).unwrap();
    let mut rng = Rng(0x9E3779B97F4A7C15);
    let pieces: Vec<&str> = vec![
        "{\"op\":\"views\"}",
        "{\"op\":\"bogus\"}",
        "{\"op\":\"view\",\"view\":\"board\"}",
        "not json",
        "",
        "{",
        "null",
        "[1,2,3]",
        "{\"op\":\"query\",\"query\":\"items\"}",
        "\"just a string\"",
        "{\"op\":\"manifest\"}",
    ];
    for i in 0..200 {
        // Mostly pick a canned line; sometimes concatenate two for
        // malformed-but-JSON-ish input.
        let line = if rng.below(4) == 0 {
            format!(
                "{} {}",
                pieces[rng.below(pieces.len() as u64) as usize],
                pieces[rng.below(pieces.len() as u64) as usize]
            )
        } else {
            pieces[rng.below(pieces.len() as u64) as usize].to_string()
        };
        let _ = session.request_raw(&line, Duration::from_millis(500));
        // Session must still be alive — the fixture never exits.
        if i % 50 == 49 {
            assert!(!session.has_exited(), "session died at iter {i}");
        }
    }
}

/// `Session::call` with arbitrary JSON request values — must return
/// Ok(Response) or a typed WireError, never panic.
#[test]
fn session_call_never_panics_on_arbitrary_requests() {
    let fx = FixtureDriver::builder("fxcall")
        .view(spec("board"))
        .query("items", json!([]))
        .build()
        .unwrap();
    let mut session = Session::spawn(fx.path()).unwrap();
    let mut rng = Rng(0xDEADBEEFCAFEF00D);
    for _ in 0..60 {
        // Small random JSON value.
        let depth = rng.below(3) as usize;
        let v = rand_json(&mut rng, depth);
        let _ = session.call(&v, Duration::from_millis(500));
    }
}

fn rand_json(rng: &mut Rng, depth: usize) -> Value {
    if depth == 0 {
        return match rng.below(4) {
            0 => Value::Null,
            1 => Value::Bool(rng.below(2) == 0),
            2 => json!(rng.next() as i64 % 1000),
            _ => json!(format!("s{}", rng.next() % 100)),
        };
    }
    match rng.below(3) {
        0 => Value::Array(
            (0..rng.below(4))
                .map(|_| rand_json(rng, depth - 1))
                .collect(),
        ),
        1 => {
            let mut m = Map::new();
            for _ in 0..rng.below(4) {
                m.insert(format!("k{}", rng.next() % 10), rand_json(rng, depth - 1));
            }
            Value::Object(m)
        }
        _ => rand_json(rng, 0),
    }
}
