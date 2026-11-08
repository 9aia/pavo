//! Integration tests — the kit proven against [`FixtureDriver`]
//! itself: happy path conformant, every violation class detected.

#![allow(clippy::unwrap_used)]

use std::path::PathBuf;
use std::process::Command;
use std::time::Duration;

use pavo_protocol::manifest::{QueryBinding, ViewSpec};
use pavo_protocol::view_node::{NodeBase, TableNode, ViewNode};
use pavo_protocol::DriverManifest;
use pavo_testkit::conformance::Check;
use pavo_testkit::fixture::FixtureDriver;
use pavo_testkit::record::{
    load_transcript, record_session, replay_driver, save_transcript, Transcript,
};
use pavo_testkit::wire::Session;
use pavo_testkit::{check_driver_with, CheckOptions, Failure};
use serde_json::{json, Value};

fn spec(id: &str, queries: &[&str]) -> ViewSpec {
    ViewSpec {
        id: id.to_string(),
        title: format!("View {id}"),
        route: format!("/fx/{id}"),
        description: None,
        queries: queries
            .iter()
            .map(|name| QueryBinding {
                name: name.to_string(),
                op: "query".to_string(),
                params: Default::default(),
            })
            .collect(),
        root: ViewNode::Table(TableNode {
            base: NodeBase {
                query: queries.first().map(|s| s.to_string()),
                ..Default::default()
            },
            ..Default::default()
        }),
    }
}

fn happy_driver() -> FixtureDriver {
    FixtureDriver::builder("fx")
        .view(spec("board", &["items", "counts"]))
        .view(spec("deps", &[]))
        .query("items", json!([["T-1", "Alpha"], ["T-2", "Beta"]]))
        .query("counts", json!({"open": 1, "done": 1}))
        .build()
        .unwrap()
}

fn checks(violations: &[pavo_testkit::Violation]) -> Vec<Check> {
    violations.iter().map(|v| v.check).collect()
}

#[test]
fn fixture_driver_answers_manifest_and_capabilities_verbs() {
    let fx = happy_driver();
    let out = fx.run_verb("manifest").unwrap();
    assert!(out.status.success());
    let m: DriverManifest = serde_json::from_slice(&out.stdout).unwrap();
    assert_eq!(m.name, "fx");
    assert_eq!(m.views.len(), 2);

    let out = fx.run_verb("capabilities").unwrap();
    let v: Value = serde_json::from_slice(&out.stdout).unwrap();
    assert!(v["capabilities"]
        .as_array()
        .unwrap()
        .iter()
        .any(|c| c == "pavo"));
}

#[test]
fn happy_path_is_conformant() {
    let fx = happy_driver();
    let violations = fx.check();
    assert!(violations.is_empty(), "{violations:?}");
}

#[test]
fn manifest_garbage_is_detected() {
    let fx = FixtureDriver::builder("fx")
        .failure(Failure {
            manifest_garbage: true,
            ..Default::default()
        })
        .build()
        .unwrap();
    let violations = fx.check();
    assert!(
        checks(&violations).contains(&Check::Manifest),
        "{violations:?}"
    );
}

#[test]
fn non_pavo_manifest_is_detected() {
    let fx = FixtureDriver::builder("fx")
        .manifest(DriverManifest {
            name: "fx".to_string(),
            title: "Fx".to_string(),
            icon: None,
            version: "1".to_string(),
            verbs: vec!["manifest".to_string()],
            capabilities: vec![],
            contributes: Default::default(),
            views: vec![],
            renderers: vec![],
        })
        .build()
        .unwrap();
    let violations = fx.check();
    assert!(
        checks(&violations).contains(&Check::AdvertisesPavo),
        "{violations:?}"
    );
}

#[test]
fn views_manifest_mismatch_is_detected() {
    let fx = FixtureDriver::builder("fx")
        .view(spec("board", &[]))
        .failure(Failure {
            views_override: Some(vec![json!({
                "id": "ghost", "title": "Ghost", "route": "/fx/ghost", "queries": []
            })]),
            ..Default::default()
        })
        .build()
        .unwrap();
    let violations = fx.check();
    assert!(
        checks(&violations).contains(&Check::ViewsConsistent),
        "{violations:?}"
    );
}

#[test]
fn view_error_is_detected() {
    let fx = FixtureDriver::builder("fx")
        .view(spec("board", &[]))
        .view_error("board", "view exploded")
        .build()
        .unwrap();
    let violations = fx.check();
    assert!(checks(&violations).contains(&Check::View), "{violations:?}");
    assert!(
        violations
            .iter()
            .any(|v| v.detail.contains("view exploded")),
        "{violations:?}"
    );
}

#[test]
fn query_error_is_detected() {
    let fx = FixtureDriver::builder("fx")
        .view(spec("board", &["items"]))
        .query_error("items", "db down")
        .build()
        .unwrap();
    let violations = fx.check();
    assert!(
        checks(&violations).contains(&Check::Query),
        "{violations:?}"
    );
}

#[test]
fn degraded_binding_still_conforms() {
    // A binding whose data is {"error": …} is the contract's partial
    // degradation path — view/query ops still succeed.
    let fx = FixtureDriver::builder("fx")
        .view(spec("board", &["items"]))
        .query_error("items", "db down")
        .build()
        .unwrap();
    // query op itself errors → Query violation is expected; but the
    // view must still parse. Assert the degraded data shape directly.
    let mut s = Session::spawn(fx.path()).unwrap();
    let resp = s
        .call(
            &json!({"op": "view", "view": "board"}),
            Duration::from_secs(5),
        )
        .unwrap();
    assert!(resp.ok);
    let result = resp.result.unwrap();
    assert_eq!(result["data"]["items"]["error"], "db down");
}

#[test]
fn bad_json_line_is_detected() {
    let fx = FixtureDriver::builder("fx")
        .failure(Failure {
            bad_json_once: true,
            ..Default::default()
        })
        .build()
        .unwrap();
    let violations = fx.check();
    assert!(!violations.is_empty(), "expected violations, got none");
    assert!(
        violations
            .iter()
            .any(|v| v.detail.contains("Response") || v.check == Check::Manifest),
        "{violations:?}"
    );
}

#[test]
fn mid_session_exit_is_detected() {
    let fx = FixtureDriver::builder("fx")
        .view(spec("board", &[]))
        .failure(Failure {
            exit_after: Some(2), // manifest ok, views dies
            ..Default::default()
        })
        .build()
        .unwrap();
    let violations = fx.check();
    assert!(
        checks(&violations).iter().any(|c| matches!(
            c,
            Check::Views | Check::View | Check::SessionAlive | Check::MalformedRejected
        )),
        "{violations:?}"
    );
}

#[test]
fn delay_times_out() {
    let fx = FixtureDriver::builder("fx")
        .failure(Failure {
            delay_ms: 5_000,
            ..Default::default()
        })
        .build()
        .unwrap();
    let violations = check_driver_with(
        fx.path(),
        CheckOptions {
            timeout: Duration::from_millis(200),
        },
    );
    assert!(!violations.is_empty(), "expected timeout violations");
}

#[test]
fn error_all_is_detected() {
    let fx = FixtureDriver::builder("fx")
        .failure(Failure {
            error_all: Some("everything is broken".to_string()),
            ..Default::default()
        })
        .build()
        .unwrap();
    let violations = fx.check();
    assert!(
        violations
            .iter()
            .any(|v| v.detail.contains("everything is broken")),
        "{violations:?}"
    );
}

#[test]
fn record_and_replay_roundtrip() {
    let fx = happy_driver();
    // Record every op the conformance suite will send, so the replay
    // answers all of them (malformed probes degrade to ok:false on
    // their own — "this is not json" never reaches dispatch).
    let requests = vec![
        json!({"op": "manifest"}),
        json!({"op": "views"}),
        json!({"op": "view", "view": "board"}),
        json!({"op": "view", "view": "deps"}),
        json!({"op": "query", "query": "items"}),
        json!({"op": "query", "query": "counts"}),
        json!({"op": "bogus"}),
    ];
    let transcript = record_session(fx.path(), &requests, Duration::from_secs(5)).unwrap();
    assert_eq!(transcript.len(), 7);

    // persist + reload
    let tmp = tempfile::tempdir().unwrap();
    let file = tmp.path().join("session.json");
    save_transcript(&transcript, &file).unwrap();
    let loaded: Transcript = load_transcript(&file).unwrap();
    assert_eq!(loaded, transcript);

    // replay is fully conformant — every op the suite sends was
    // recorded, and unrecorded/malformed requests degrade to ok:false.
    let replayed = replay_driver("replay", &loaded).unwrap();
    let violations = replayed.check();
    assert!(violations.is_empty(), "{violations:?}");

    // determinism: two identical requests → identical responses
    let mut s = Session::spawn(replayed.path()).unwrap();
    for _ in 0..2 {
        let resp = s
            .call(
                &json!({"op": "query", "query": "items"}),
                Duration::from_secs(5),
            )
            .unwrap();
        assert_eq!(resp.result.unwrap()["data"][0][0], "T-1");
    }
}

#[test]
fn replay_unknown_request_gets_error() {
    let fx = happy_driver();
    let transcript = record_session(
        fx.path(),
        &[json!({"op": "manifest"})],
        Duration::from_secs(5),
    )
    .unwrap();
    let replayed = replay_driver("replay", &transcript).unwrap();
    let mut s = Session::spawn(replayed.path()).unwrap();
    let resp = s
        .call(&json!({"op": "views"}), Duration::from_secs(5))
        .unwrap();
    assert!(!resp.ok);
    assert!(resp.error.unwrap().contains("no recorded response"));
}

#[test]
fn conformance_accepts_path_and_fixture() {
    // check_driver takes &Path; FixtureDriver::check delegates.
    let fx = happy_driver();
    let direct = pavo_testkit::check_driver(fx.path());
    assert!(direct.is_empty(), "{direct:?}");
}

#[test]
fn bogus_op_gets_error_from_fixture() {
    let fx = happy_driver();
    let mut s = Session::spawn(fx.path()).unwrap();
    let resp = s
        .call(&json!({"op": "explode"}), Duration::from_secs(5))
        .unwrap();
    assert!(!resp.ok);
}

/// The shipped example drivers must pass the same conformance suite —
/// they're the ecosystem's reference implementations. Skipped when
/// `python3` isn't on PATH (the drivers are Python scripts).
#[test]
fn example_drivers_are_conformant() {
    if Command::new("python3")
        .arg("--version")
        .output()
        .map(|o| !o.status.success())
        .unwrap_or(true)
    {
        eprintln!("skipping: python3 not available");
        return;
    }
    let drivers_dir = PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("../../examples/drivers");
    let mut found = 0;
    for entry in std::fs::read_dir(&drivers_dir).unwrap() {
        let dir = entry.unwrap().path();
        if !dir.is_dir() {
            continue;
        }
        for file in std::fs::read_dir(&dir).unwrap() {
            let path = file.unwrap().path();
            let name = path.file_name().unwrap().to_string_lossy().into_owned();
            if !name.starts_with("tamias-driver-") || name.contains('.') {
                continue;
            }
            found += 1;
            let violations = pavo_testkit::check_driver(&path);
            assert!(violations.is_empty(), "{name}: {violations:?}");
        }
    }
    assert!(found >= 3, "expected >=3 example drivers, found {found}");
}

// A hand-rolled driver that answers ok:true to *everything* — proves
// MalformedRejected fires even when the driver is otherwise fine.
#[test]
fn always_ok_driver_fails_malformed_check() {
    let tmp = tempfile::tempdir().unwrap();
    let dir = tmp.path().join("bin");
    std::fs::create_dir_all(&dir).unwrap();
    let path: PathBuf = dir.join("tamias-driver-rowdy");
    std::fs::write(
        &path,
        "#!/bin/sh\nwhile IFS= read -r line; do printf '%s\\n' '{\"ok\":true,\"result\":{}}'; done\n",
    )
    .unwrap();
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        std::fs::set_permissions(&path, std::fs::Permissions::from_mode(0o755)).unwrap();
    }
    let violations = pavo_testkit::check_driver(&path);
    // manifest result {} parses? DriverManifest needs name/title/version
    // → Manifest violation; bogus op → ok:true → MalformedRejected.
    assert!(
        checks(&violations).contains(&Check::MalformedRejected),
        "{violations:?}"
    );
}
