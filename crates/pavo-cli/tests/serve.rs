//! Integration tests for `pavo serve` — spawn the real binary with
//! `--port 0`, read the bound port off the first stdout line, then
//! exercise the HTTP API over raw TCP (std-only, no HTTP client dep).

#![allow(clippy::unwrap_used)]

use std::io::{BufRead, BufReader, Read, Write};
use std::net::TcpStream;
use std::process::{Child, Command, Stdio};

/// Same fixture driver as cli.rs — eager manifest + serve wire.
const DEMO: &str = r#"#!/bin/sh
if [ "$1" = manifest ]; then
  printf '%s\n' '{"name":"demo","title":"Demo Driver","version":"1.2.3","verbs":["manifest","serve"],"capabilities":["pavo"],"contributes":{"navbar":[{"id":"nav","title":"Demo Nav","route":"/demo/board","priority":5}],"statusbar":[{"id":"st","title":"Demo status","route":"/demo/board"}]},"views":[{"id":"board","title":"Board","route":"/demo/board","description":"The board","queries":[{"name":"items","op":"query"}],"root":{"kind":"kv","pairs":[]}}]}'
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
      *'"op":"manifest"'*)
        printf '%s\n' '{"ok":true,"result":{"name":"demo","title":"Demo Driver","version":"1.2.3","verbs":["serve"],"capabilities":["pavo"],"views":[{"id":"board","title":"Board","route":"/demo/board","root":{"kind":"kv","pairs":[]}}]}}';;
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
