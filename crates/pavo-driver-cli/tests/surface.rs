//! End-to-end: `tamias-driver-cli <driver> <op>` fronts a fake engine
//! driver (a shell script answering `ops` / `serve` / `run`) — the
//! generated CLI parses, dispatches `cli.<op>` over the wire, prints
//! captured output, and propagates codes; `surfaces` stream through
//! `run` argv passthrough.

#![cfg(unix)]
#![allow(clippy::unwrap_used)]

use std::io::Write;
use std::os::unix::fs::PermissionsExt;
use std::path::PathBuf;
use std::process::{Command, Stdio};

/// `tamias-driver-fx` — minimal ops/ serve/run contract in sh.
const FAKE: &str = r#"#!/bin/sh
case "$1" in
ops)
cat <<'EOF'
{"engine":"fx","invoke":"query cli.<op>","ops":[
 {"query":"cli.hello","doc":"say hi","params":[{"name":"who","kind":"arg"},{"name":"loud","kind":"flag"}]},
 {"query":"cli.unlock","doc":"secret op","params":[{"name":"vault","kind":"arg"},{"name":"token","kind":"opt","secret":true}]},
 {"query":"cli.fail","doc":"exit 3","params":[]}],
 "surfaces":["stream"]}
EOF
;;
serve)
while IFS= read -r line; do
  case "$line" in
    *cli.hello*) printf '%s\n' '{"ok":true,"result":{"query":"cli.hello","data":{"op":"hello","code":0,"stdout":"hello world\n","stderr":""}}}' ;;
    # request JSON sorts keys — `query` (cli.<op>) trails `params`
    *token*creds*cli.unlock*) printf '%s\n' '{"ok":true,"result":{"query":"cli.unlock","data":{"op":"unlock","code":0,"stdout":"unlocked\n","stderr":""}}}' ;;
    *cli.unlock*)  printf '%s\n' '{"ok":true,"result":{"query":"cli.unlock","data":{"op":"unlock","code":1,"stdout":"","stderr":"denied\n"}}}' ;;
    *cli.fail*)  printf '%s\n' '{"ok":true,"result":{"query":"cli.fail","data":{"op":"fail","code":3,"stdout":"","stderr":"boom\n"}}}' ;;
    *)           printf '%s\n' '{"ok":false,"error":"unsupported"}' ;;
  esac
done
;;
run) echo "ran $2 $3" ;;
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

fn cli() -> Command {
    Command::new(env!("CARGO_BIN_EXE_tamias-driver-cli"))
}

#[test]
fn generated_help_lists_ops_and_surfaces() {
    let (_d, driver) = fixture();
    let out = cli().arg(&driver).output().unwrap();
    let text = format!(
        "{}{}",
        String::from_utf8_lossy(&out.stdout),
        String::from_utf8_lossy(&out.stderr)
    );
    assert!(text.contains("hello"), "{text}");
    assert!(text.contains("stream"), "{text}");
}

#[test]
fn op_dispatches_cli_query_and_prints_stdout() {
    let (_d, driver) = fixture();
    let out = cli()
        .arg(&driver)
        .args(["hello", "world", "--loud"])
        .output()
        .unwrap();
    assert_eq!(out.status.code(), Some(0));
    assert_eq!(String::from_utf8_lossy(&out.stdout), "hello world\n");
}

#[test]
fn engine_exit_code_propagates() {
    let (_d, driver) = fixture();
    let out = cli().arg(&driver).args(["fail"]).output().unwrap();
    assert_eq!(out.status.code(), Some(3));
    assert_eq!(String::from_utf8_lossy(&out.stderr), "boom\n");
}

#[test]
fn params_stdin_supplies_secret_params() {
    let (_d, driver) = fixture();
    let mut child = cli()
        .arg(&driver)
        .args(["unlock", "--params-stdin"])
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .spawn()
        .unwrap();
    child
        .stdin
        .as_mut()
        .unwrap()
        .write_all(br#"{"vault":"main","token":"creds"}"#)
        .unwrap();
    let out = child.wait_with_output().unwrap();
    assert_eq!(out.status.code(), Some(0));
    assert_eq!(String::from_utf8_lossy(&out.stdout), "unlocked\n");
    // no secret warning — nothing secret rode argv
    assert!(!String::from_utf8_lossy(&out.stderr).contains("secret-bearing"));
}

#[test]
fn secret_param_via_argv_warns_without_echoing_value() {
    let (_d, driver) = fixture();
    let out = cli()
        .arg(&driver)
        .args(["unlock", "main", "--token", "creds"])
        .output()
        .unwrap();
    assert_eq!(out.status.code(), Some(0));
    assert_eq!(String::from_utf8_lossy(&out.stdout), "unlocked\n");
    let stderr = String::from_utf8_lossy(&out.stderr);
    assert!(stderr.contains("`token` is secret-bearing"), "{stderr}");
    assert!(!stderr.contains("creds"), "value must never echo: {stderr}");
}

#[test]
fn missing_required_param_is_usage_error() {
    let (_d, driver) = fixture();
    // `unlock` requires `vault`; argv alone doesn't supply it
    let out = cli().arg(&driver).args(["unlock"]).output().unwrap();
    assert_eq!(out.status.code(), Some(2));
}

#[test]
fn surface_verbs_passthrough_to_run() {
    let (_d, driver) = fixture();
    let out = cli()
        .arg(&driver)
        .args(["stream", "--fast"])
        .output()
        .unwrap();
    assert_eq!(out.status.code(), Some(0));
    assert_eq!(String::from_utf8_lossy(&out.stdout), "ran stream --fast\n");
}

#[test]
fn discovery_verbs_answer() {
    let out = cli().arg("capabilities").output().unwrap();
    assert_eq!(out.status.code(), Some(0));
    let v: serde_json::Value = serde_json::from_slice(&out.stdout).unwrap();
    assert_eq!(v["types"], serde_json::json!([]));
    // pavo wire `serve` answers manifest + empty views
    let mut child = cli()
        .arg("serve")
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .spawn()
        .unwrap();
    child
        .stdin
        .as_mut()
        .unwrap()
        .write_all(b"{\"op\":\"manifest\"}\n{\"op\":\"views\"}\n")
        .unwrap();
    let out = child.wait_with_output().unwrap();
    let lines: Vec<serde_json::Value> = String::from_utf8_lossy(&out.stdout)
        .lines()
        .map(|l| serde_json::from_str(l).unwrap())
        .collect();
    assert_eq!(lines[0]["result"]["name"], serde_json::json!("cli"));
    assert_eq!(lines[1]["result"], serde_json::json!([]));
}

#[test]
fn missing_driver_is_a_clean_error() {
    let out = cli().arg("no-such-engine-9aia").output().unwrap();
    assert_eq!(out.status.code(), Some(2));
    assert!(String::from_utf8_lossy(&out.stderr).contains("no driver"));
}
