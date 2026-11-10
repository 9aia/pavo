//! End-to-end: `tamias-driver-mcp <driver>` fronts a fake engine
//! driver — `initialize`/`tools/list`/`tools/call` over stdio JSON-RPC,
//! tools generated from the ops manifest.

#![cfg(unix)]
#![allow(clippy::unwrap_used)]

use std::io::{BufRead, BufReader, Write};
use std::os::unix::fs::PermissionsExt;
use std::path::PathBuf;
use std::process::{Command, Stdio};

const FAKE: &str = r#"#!/bin/sh
case "$1" in
ops)
cat <<'EOF'
{"engine":"fx","ops":[
 {"query":"cli.hello","doc":"say hi","params":[{"name":"who","kind":"arg"},{"name":"loud","kind":"flag"}]},
 {"query":"cli.fail","doc":"exit 3","params":[]}],
 "surfaces":["stream"]}
EOF
;;
serve)
while IFS= read -r line; do
  case "$line" in
    *cli.hello*) printf '%s\n' '{"ok":true,"result":{"query":"cli.hello","data":{"op":"hello","code":0,"stdout":"hello world\n","stderr":""}}}' ;;
    *cli.fail*)  printf '%s\n' '{"ok":true,"result":{"query":"cli.fail","data":{"op":"fail","code":3,"stdout":"","stderr":"boom\n"}}}' ;;
    *)           printf '%s\n' '{"ok":false,"error":"unsupported"}' ;;
  esac
done
;;
esac
"#;

fn fixture() -> (tempfile::TempDir, PathBuf) {
    let dir = tempfile::tempdir().unwrap();
    let driver = dir.path().join("tamias-driver-fx");
    let mut f = std::fs::File::create(&driver).unwrap();
    f.write_all(FAKE.as_bytes()).unwrap();
    std::fs::set_permissions(&driver, std::fs::Permissions::from_mode(0o755)).unwrap();
    (dir, driver)
}

/// Spawn the server, write `requests` (one JSON-RPC per line), then
/// read one response line per request that carries an `id`.
fn exchange(driver: &PathBuf, requests: &[&str]) -> Vec<serde_json::Value> {
    let mut child = Command::new(env!("CARGO_BIN_EXE_tamias-driver-mcp"))
        .arg(driver)
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .spawn()
        .unwrap();
    let mut stdin = child.stdin.take().unwrap();
    let stdout = BufReader::new(child.stdout.take().unwrap());
    for req in requests {
        stdin.write_all(req.as_bytes()).unwrap();
        stdin.write_all(b"\n").unwrap();
    }
    stdin.flush().unwrap();
    let wants = requests.iter().filter(|r| r.contains("\"id\"")).count();
    let mut replies = Vec::new();
    let mut lines = stdout.lines();
    for _ in 0..wants {
        replies.push(serde_json::from_str(&lines.next().unwrap().unwrap()).unwrap());
    }
    drop(stdin);
    let _ = child.wait();
    replies
}

#[test]
fn tools_list_and_call_over_stdio() {
    let (_d, driver) = fixture();
    let replies = exchange(
        &driver,
        &[
            r#"{"jsonrpc":"2.0","id":1,"method":"initialize","params":{"protocolVersion":"2024-11-05","capabilities":{},"clientInfo":{"name":"t","version":"0"}}}"#,
            r#"{"jsonrpc":"2.0","method":"notifications/initialized"}"#,
            r#"{"jsonrpc":"2.0","id":2,"method":"tools/list"}"#,
            r#"{"jsonrpc":"2.0","id":3,"method":"tools/call","params":{"name":"hello","arguments":{"who":"world","loud":true}}}"#,
            r#"{"jsonrpc":"2.0","id":4,"method":"tools/call","params":{"name":"fail","arguments":{}}}"#,
        ],
    );
    assert_eq!(replies.len(), 4);
    assert_eq!(
        replies[0]["result"]["serverInfo"]["name"],
        serde_json::json!("fx")
    );
    let tools = replies[1]["result"]["tools"].as_array().unwrap();
    let names: Vec<_> = tools.iter().map(|t| t["name"].as_str().unwrap()).collect();
    assert_eq!(names, ["hello", "fail"]); // `stream` surface excluded
    assert_eq!(
        tools[0]["inputSchema"]["required"],
        serde_json::json!(["who"])
    );
    assert_eq!(
        tools[0]["inputSchema"]["properties"]["loud"]["type"],
        serde_json::json!("boolean")
    );
    assert_eq!(
        replies[2]["result"]["content"][0]["text"],
        serde_json::json!("hello world\n")
    );
    assert_eq!(replies[2]["result"]["isError"], serde_json::json!(false));
    // nonzero op code → isError, stderr rides as a second content block
    assert_eq!(replies[3]["result"]["isError"], serde_json::json!(true));
    assert_eq!(
        replies[3]["result"]["content"][1]["text"],
        serde_json::json!("boom\n")
    );
}

#[test]
fn missing_driver_is_a_clean_error() {
    let out = Command::new(env!("CARGO_BIN_EXE_tamias-driver-mcp"))
        .arg("no-such-engine-9aia")
        .output()
        .unwrap();
    assert_eq!(out.status.code(), Some(2));
    assert!(String::from_utf8_lossy(&out.stderr).contains("no driver"));
}
