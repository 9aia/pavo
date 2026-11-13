//! Integration tests for `pavo serve` — spawn the real binary with
//! `--port 0`, read the bound port off the first stdout line, then
//! exercise the HTTP API over raw TCP (std-only, no HTTP client dep).

#![allow(clippy::unwrap_used)]

use std::io::{BufRead, BufReader, Read, Write};
use std::net::TcpStream;
use std::process::{Child, Command, Stdio};

use pavo_protocol::manifest::ViewSpec;
use pavo_protocol::view_node::{NodeBase, TableNode, ViewNode};
use pavo_protocol::Action;
use pavo_testkit::fixture::FixtureDriver;

/// Same fixture driver as cli.rs — eager manifest + serve wire.
const DEMO: &str = r#"#!/bin/sh
if [ "$1" = manifest ]; then
  printf '%s\n' '{"name":"demo","title":"Demo Driver","version":"1.2.3","verbs":["manifest","serve"],"capabilities":["pavo"],"contributes":{"navbar":[{"id":"nav","title":"Demo Nav","route":"/demo/board","priority":5}],"statusbar":[{"id":"st","title":"Demo status","route":"/demo/board"}],"sidebar":[{"id":"side","title":"Side","route":"/demo/side"}],"headerActions":[{"id":"act","title":"Act","route":"/demo/board"}],"notifications":[{"id":"ntf","title":"Notify","route":"/demo/ntf"}],"jobs":[{"id":"job","title":"Jobs","route":"/demo/jobs"}]},"views":[{"id":"board","title":"Board","route":"/demo/board","description":"The board","queries":[{"name":"items","op":"query"}],"root":{"kind":"kv","pairs":[]}}]}'
  exit 0
fi
if [ "$1" = serve ]; then
  while IFS= read -r line; do
    case "$line" in
      *'"op":"views"'*)
        printf '%s\n' '{"ok":true,"result":[{"id":"board","title":"Board","route":"/demo/board","queries":["items"]}]}';;
      *'"op":"view","view":"board"'*)
        printf '%s\n' '{"ok":true,"result":{"view":{"id":"board","title":"Board","route":"/demo/board","description":"The board","queries":[{"name":"items","op":"query"}],"root":{"kind":"kv","pairs":[]}},"data":{"items":[["T-1","Alpha","done"]]}}}';;
      *'"op":"view"'*)
        printf '%s\n' '{"ok":false,"error":"unknown view"}';;
      *'"op":"query"'*)
        printf '%s\n' '{"ok":true,"result":{"query":"items","data":[["T-1","Alpha","done"]]}}';;
      *'"op":"jobs"'*)
        printf '%s\n' '{"ok":true,"result":[{"id":"j1","title":"Build","status":"running","detail":"step 2/5"}]}';;
      *'"op":"notifications"'*)
        printf '%s\n' '{"ok":true,"result":[{"id":"n1","title":"Heads up","severity":"warning","read":false}]}';;
      *'"op":"manifest"'*)
        printf '%s\n' '{"ok":true,"result":{"name":"demo","title":"Demo Driver","version":"1.2.3","verbs":["serve"],"capabilities":["pavo"],"contributes":{"jobs":[{"id":"job","title":"Jobs","route":"/demo/jobs"}],"notifications":[{"id":"ntf","title":"Notify","route":"/demo/ntf"}]},"views":[{"id":"board","title":"Board","route":"/demo/board","root":{"kind":"kv","pairs":[]}}]}}';;
    esac
  done
  exit 0
fi
exit 1
"#;

struct Serve {
    child: Child,
    port: u16,
    _tmp: tempfile::TempDir,
    _fixtures: Vec<FixtureDriver>,
}

impl Drop for Serve {
    fn drop(&mut self) {
        let _ = self.child.kill();
        let _ = self.child.wait();
    }
}

/// Spawn `pavo serve --port 0` against the fixture driver; returns
/// once the `listening …` line arrives.
fn serve() -> Serve {
    serve_with(Vec::new())
}

/// [`serve`] plus extra testkit [`FixtureDriver`]s installed on the
/// fixture PATH (driver + `.fixture.json` copied into `bin`).
fn serve_with(fixtures: Vec<FixtureDriver>) -> Serve {
    let tmp = tempfile::tempdir().unwrap();
    let bin = tmp.path().join("bin");
    std::fs::create_dir_all(&bin).unwrap();
    let driver = bin.join("tamias-driver-demo");
    std::fs::write(&driver, DEMO).unwrap();
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        std::fs::set_permissions(&driver, std::fs::Permissions::from_mode(0o755)).unwrap();
    }
    for fx in &fixtures {
        let file = format!("tamias-driver-{}", fx.name());
        std::fs::copy(fx.path(), bin.join(&file)).unwrap();
        std::fs::copy(
            fx.dir().join(format!("{file}.fixture.json")),
            bin.join(format!("{file}.fixture.json")),
        )
        .unwrap();
    }
    // `pavo serve` gets PATH=<bin> only — the FixtureDriver is a
    // Python script (`#!/usr/bin/env python3`), so link the real
    // interpreter in (unix-only).
    #[cfg(unix)]
    if !fixtures.is_empty() {
        let found = std::env::var("PATH")
            .unwrap_or_default()
            .split(':')
            .map(|d| std::path::PathBuf::from(d).join("python3"))
            .find(|p| p.is_file());
        if let Some(src) = found {
            let _ = std::os::unix::fs::symlink(&src, bin.join("python3"));
        }
    }

    let mut child = Command::new(env!("CARGO_BIN_EXE_pavo"))
        .args(["serve", "--port", "0"])
        .env("PATH", &bin)
        .env("TAMIAS_HOME", tmp.path().join("home"))
        // Force the built-in landing page regardless of repo state.
        .env("PAVO_WEB_DIR", tmp.path().join("no-such-dir"))
        .stdout(Stdio::piped())
        .stderr(Stdio::null())
        .spawn()
        .unwrap();

    let stdout = child.stdout.take().unwrap();
    let mut reader = BufReader::new(stdout);
    let mut line = String::new();
    reader.read_line(&mut line).unwrap();
    let port: u16 = line
        .trim()
        .strip_prefix("listening http://127.0.0.1:")
        .and_then(|p| p.parse().ok())
        .unwrap_or_else(|| panic!("bad listening line: {line:?}"));

    Serve {
        child,
        port,
        _tmp: tmp,
        _fixtures: fixtures,
    }
}

/// Minimal HTTP/1.0-style GET — `Connection: close` so the server
/// closes after responding and we can read to EOF. Returns
/// (status, headers-lowercased, body).
fn http_get(port: u16, path: &str) -> (u16, String, String) {
    http_request(port, "GET", path, "")
}

fn http_request(port: u16, method: &str, path: &str, body: &str) -> (u16, String, String) {
    let mut stream = TcpStream::connect(("127.0.0.1", port)).unwrap();
    stream
        .write_all(
            format!(
                "{method} {path} HTTP/1.1\r\nhost: 127.0.0.1\r\n\
                 content-length: {}\r\nconnection: close\r\n\r\n{body}",
                body.len()
            )
            .as_bytes(),
        )
        .unwrap();
    let mut raw = Vec::new();
    stream.read_to_end(&mut raw).unwrap();
    let text = String::from_utf8_lossy(&raw).into_owned();
    let (head, body) = text.split_once("\r\n\r\n").unwrap_or((&text, ""));
    let status: u16 = head
        .split_whitespace()
        .nth(1)
        .and_then(|s| s.parse().ok())
        .unwrap_or(0);
    (status, head.to_string(), body.to_string())
}

fn json_body(body: &str) -> serde_json::Value {
    serde_json::from_str(body).unwrap()
}

#[test]
fn health_and_envelope() {
    let srv = serve();
    let (status, head, body) = http_get(srv.port, "/api/health");
    assert_eq!(status, 200);
    assert!(head.contains("access-control-allow-origin: *"), "{head}");
    assert_eq!(
        json_body(&body),
        serde_json::json!({"ok": true, "result": {}})
    );
}

#[test]
fn drivers_views_chrome() {
    let srv = serve();

    let (status, _, body) = http_get(srv.port, "/api/drivers");
    assert_eq!(status, 200);
    let v = json_body(&body);
    assert_eq!(v["ok"], true);
    let row = &v["result"][0];
    assert_eq!(row["driver"], "demo");
    assert_eq!(row["title"], "Demo Driver");
    assert_eq!(row["view_count"], 1);
    assert_eq!(row["contributes"]["navbar"], 1);
    assert_eq!(row["contributes"]["statusbar"], 1);

    let (_, _, body) = http_get(srv.port, "/api/views");
    let v = json_body(&body);
    assert_eq!(v["result"][0]["namespaced"], "demo:board");
    assert_eq!(v["result"][0]["route"], "/demo/board");

    let (_, _, body) = http_get(srv.port, "/api/chrome");
    let v = json_body(&body);
    assert_eq!(v["result"]["navbar"][0]["id"], "demo:nav");
    assert_eq!(v["result"]["statusbar"][0]["id"], "demo:st");
    // Platform slots ride the same chrome payload.
    assert_eq!(v["result"]["sidebar"][0]["id"], "demo:side");
    assert_eq!(v["result"]["headerActions"][0]["id"], "demo:act");
    assert_eq!(v["result"]["notifications"][0]["id"], "demo:ntf");
    assert_eq!(v["result"]["jobs"][0]["id"], "demo:job");
}

#[test]
fn feeds_and_call_endpoints() {
    let srv = serve();

    // Aggregated feeds: item fields + driver attribution, flattened.
    let (status, _, body) = http_get(srv.port, "/api/jobs");
    assert_eq!(status, 200);
    let v = json_body(&body);
    assert_eq!(v["ok"], true);
    assert_eq!(v["result"][0]["id"], "j1");
    assert_eq!(v["result"][0]["status"], "running");
    assert_eq!(v["result"][0]["driver"], "demo");
    assert_eq!(v["result"][0]["driver_title"], "Demo Driver");

    let (status, _, body) = http_get(srv.port, "/api/notifications");
    assert_eq!(status, 200);
    let v = json_body(&body);
    assert_eq!(v["result"][0]["id"], "n1");
    assert_eq!(v["result"][0]["severity"], "warning");
    assert_eq!(v["result"][0]["read"], false);

    // POST /api/call proxies a wire Request to the named driver.
    let (status, _, body) = http_request(
        srv.port,
        "POST",
        "/api/call?driver=demo",
        r#"{"op":"query","query":"items","params":{}}"#,
    );
    assert_eq!(status, 200);
    let v = json_body(&body);
    assert_eq!(v["ok"], true);
    assert_eq!(v["result"]["query"], "items");
    assert_eq!(v["result"]["data"][0][0], "T-1");

    // Feed ops proxy too.
    let (status, _, body) = http_request(
        srv.port,
        "POST",
        "/api/call?driver=demo",
        r#"{"op":"jobs"}"#,
    );
    assert_eq!(status, 200);
    assert_eq!(json_body(&body)["result"][0]["id"], "j1");

    // Error surface: missing driver → 400, bad body → 400,
    // unknown driver → 404, GET on /api/call → 400 (empty body).
    let (status, _, _) = http_request(srv.port, "POST", "/api/call", r#"{"op":"jobs"}"#);
    assert_eq!(status, 400);
    let (status, _, _) = http_request(srv.port, "POST", "/api/call?driver=demo", "{bad");
    assert_eq!(status, 400);
    let (status, _, _) = http_request(
        srv.port,
        "POST",
        "/api/call?driver=ghost",
        r#"{"op":"jobs"}"#,
    );
    assert_eq!(status, 404);
    let (status, _, _) = http_get(srv.port, "/api/call?driver=demo");
    assert_eq!(status, 400);

    // POST is still rejected elsewhere.
    let (status, _, body) = http_request(srv.port, "POST", "/api/jobs", "{}");
    assert_eq!(status, 405);
    assert_eq!(json_body(&body)["ok"], false);
}

#[test]
fn view_and_query_endpoints() {
    let srv = serve();

    for path in [
        "/api/view?ref=demo:board",
        "/api/view?ref=demo%3Aboard",
        "/api/view?driver=demo&view=board",
        "/api/raw?ref=demo:board",
    ] {
        let (status, _, body) = http_get(srv.port, path);
        assert_eq!(status, 200, "{path}");
        let v = json_body(&body);
        assert_eq!(v["ok"], true, "{path}");
        assert_eq!(v["result"]["view"]["id"], "board", "{path}");
        assert_eq!(v["result"]["data"]["items"][0][0], "T-1", "{path}");
    }

    // params pass through (fixture ignores them, but the request must
    // not break the wire)
    let (status, _, body) = http_get(
        srv.port,
        "/api/view?ref=demo:board&param.limit=5&param.name=alpha",
    );
    assert_eq!(status, 200);
    assert_eq!(json_body(&body)["ok"], true);

    let (status, _, body) = http_get(srv.port, "/api/query?driver=demo&query=items");
    assert_eq!(status, 200);
    let v = json_body(&body);
    assert_eq!(v["result"]["query"], "items");
    assert_eq!(v["result"]["data"][0][0], "T-1");
}

#[test]
fn errors_and_methods() {
    let srv = serve();

    // unknown endpoint → 404 envelope
    let (status, _, body) = http_get(srv.port, "/api/bogus");
    assert_eq!(status, 404);
    assert_eq!(json_body(&body)["ok"], false);

    // unknown view → 404 (driver answered ok:false → surfaced)
    let (status, _, body) = http_get(srv.port, "/api/view?ref=demo:nope");
    assert_eq!(status, 404);
    assert_eq!(json_body(&body)["ok"], false);

    // missing ref → 400
    let (status, _, _) = http_get(srv.port, "/api/view");
    assert_eq!(status, 400);

    // unknown driver → 404
    let (status, _, _) = http_get(srv.port, "/api/view?ref=ghost:x");
    assert_eq!(status, 404);

    // POST → 405
    let (status, _, body) = http_request(srv.port, "POST", "/api/health", "{}");
    assert_eq!(status, 405);
    assert_eq!(json_body(&body)["ok"], false);

    // bad path outside /api → 404 text
    let (status, _, _) = http_get(srv.port, "/definitely-not-here.txt");
    assert_eq!(status, 404);
}

#[test]
fn builtin_landing_and_view_page() {
    let srv = serve();

    let (status, _, body) = http_get(srv.port, "/");
    assert_eq!(status, 200);
    assert!(body.contains("pavo"), "{body}");
    assert!(body.contains("demo:board"), "{body}");
    assert!(body.contains("/api/view?ref=demo:board"), "{body}");

    let (status, _, body) = http_get(srv.port, "/view/demo%3Aboard");
    assert_eq!(status, 200);
    assert!(body.contains("demo:board"), "{body}");

    let (status, _, _) = http_get(srv.port, "/view/ghost%3Ax");
    assert_eq!(status, 200); // page renders the error inline
}

// -- /api/invoke ---------------------------------------------------------
// The web surface's action path end-to-end: POST body → Host::invoke →
// ActionResult passthrough, driven by a pavo-testkit FixtureDriver.

fn act(id: &str) -> Action {
    serde_json::from_value(serde_json::json!({"id": id, "label": id})).unwrap()
}

fn act_full(id: &str, extra: serde_json::Value) -> Action {
    let mut v = serde_json::json!({"id": id, "label": id});
    if let (Some(base), Some(add)) = (v.as_object_mut(), extra.as_object()) {
        for (k, val) in add {
            base.insert(k.clone(), val.clone());
        }
    }
    serde_json::from_value(v).unwrap()
}

/// `act` driver: one view + manifest actions — `refresh` (canned
/// result), `rm` (confirm), `move` (required param), `fragile`
/// (driver-side per-param errors).
fn actions_driver() -> FixtureDriver {
    let board = ViewSpec {
        id: "board".to_string(),
        title: "Board".to_string(),
        route: "/act/board".to_string(),
        description: None,
        queries: vec![],
        root: ViewNode::Table(TableNode {
            base: NodeBase::default(),
            ..Default::default()
        }),
        item_route: None,
        filter_param: None,
        refresh_ms: None,
    };
    FixtureDriver::builder("act")
        .view(board)
        .action(act("refresh"))
        .action(act_full("rm", serde_json::json!({"danger": "confirm"})))
        .action(act_full(
            "move",
            serde_json::json!({"params": [
                {"name": "id", "kind": "string", "required": true}
            ]}),
        ))
        .action(act("fragile"))
        .invoke(
            "refresh",
            serde_json::json!({"message": "reloaded", "data": {"n": 2}}),
        )
        .invoke("fragile", serde_json::json!({"errors": {"note": "nope"}}))
        .build()
        .unwrap()
}

#[test]
fn api_invoke_end_to_end() {
    let srv = serve_with(vec![actions_driver()]);

    // ref + action + target + params → ActionResult passthrough.
    let (status, _, body) = http_request(
        srv.port,
        "POST",
        "/api/invoke",
        r#"{"ref":"act:board","action":"refresh","target":{"id":"T-1"},"params":{"x":1}}"#,
    );
    assert_eq!(status, 200);
    let v = json_body(&body);
    assert_eq!(v["ok"], true, "{body}");
    assert_eq!(v["result"]["message"], "reloaded");
    assert_eq!(v["result"]["data"]["n"], 2);

    // driver+view form; default dispatch echoes context.
    let (status, _, body) = http_request(
        srv.port,
        "POST",
        "/api/invoke",
        r#"{"driver":"act","view":"board","action":"move","params":{"id":"T-9"}}"#,
    );
    assert_eq!(status, 200);
    let v = json_body(&body);
    assert_eq!(v["ok"], true, "{body}");
    assert_eq!(v["result"]["data"]["params"]["id"], "T-9");

    // Driver-side per-param errors stay ok:true on the wire — the
    // web host reads `result.errors` (no confirmation gate here).
    let (status, _, body) = http_request(
        srv.port,
        "POST",
        "/api/invoke",
        r#"{"ref":"act:board","action":"fragile"}"#,
    );
    assert_eq!(status, 200);
    let v = json_body(&body);
    assert_eq!(v["ok"], true, "{body}");
    assert_eq!(v["result"]["errors"]["note"], "nope");

    // ok:false from the driver surfaces as an error envelope.
    let (status, _, body) = http_request(
        srv.port,
        "POST",
        "/api/invoke",
        r#"{"ref":"act:board","action":"ghost"}"#,
    );
    assert_ne!(status, 200);
    assert_eq!(json_body(&body)["ok"], false);

    // Bad bodies → 400; unknown driver → 404.
    let (status, _, _) = http_request(srv.port, "POST", "/api/invoke", "{bad");
    assert_eq!(status, 400);
    let (status, _, _) = http_request(srv.port, "POST", "/api/invoke", r#"{"ref":"act:board"}"#);
    assert_eq!(status, 400);
    let (status, _, _) = http_request(
        srv.port,
        "POST",
        "/api/invoke",
        r#"{"driver":"ghost","action":"a"}"#,
    );
    assert_eq!(status, 404);
}
