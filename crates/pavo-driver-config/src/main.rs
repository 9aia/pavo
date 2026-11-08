#![cfg_attr(coverage, feature(coverage_attribute))]
#![cfg_attr(test, allow(clippy::unwrap_used))]
//! tamias-driver-config — the common config API driver for the 9aia
//! suite. Graduated from `examples/drivers/config` (the Python
//! prototype); now a first-party Rust driver in the pavo workspace.
//!
//! Answers pavo's wire ops (`manifest`/`views`/`view`/`query`/`render`)
//! over `serve`, plus the sibling CLI verbs `manifest` and
//! `capabilities`. The `query` surface is the product-facing API:
//!
//! | query      | params                    | result                          |
//! |------------|---------------------------|---------------------------------|
//! | `get`      | `key` (dotted)            | `{key, value, found}`           |
//! | `set`      | `key`, `value`            | `{key, value, path, written}`   |
//! | `resolve`  | —                         | merged config tree (nested)     |
//! | `config`   | —                         | merged config flattened to KV  |
//! | `sources`  | —                         | every contributing layer        |
//!
//! Other drivers read config by calling `tamias-driver-config serve`
//! `query get/resolve` — namespaced keys (`myco.theme`), never raw
//! files. Resolution order and write-back rules live in
//! [`config`]'s module docs; the user-facing contract is the crate
//! README.

mod config;
mod view;

use std::io::{BufRead, Write};
use std::path::PathBuf;
use std::process::ExitCode;

use pavo_protocol::wire::{parse_request, QueryResult, Request, Response, ViewResult};
use serde_json::{json, Map, Value};

use config::{load, Options};

fn main() -> ExitCode {
    let args: Vec<String> = std::env::args().skip(1).collect();
    // --config may appear anywhere in argv (serve included); strip it
    // before verb dispatch. `--config=PATH` and `--config PATH` both
    // work; last one wins.
    let mut explicit: Option<PathBuf> = None;
    let mut positional: Vec<String> = Vec::new();
    let mut i = 0;
    while i < args.len() {
        let arg = &args[i];
        if arg == "--config" {
            if let Some(v) = args.get(i + 1) {
                explicit = Some(PathBuf::from(v));
                i += 2;
                continue;
            }
            eprintln!("tamias-driver-config: --config requires a path");
            return ExitCode::from(2);
        }
        if let Some(v) = arg.strip_prefix("--config=") {
            explicit = Some(PathBuf::from(v));
            i += 1;
            continue;
        }
        positional.push(arg.clone());
        i += 1;
    }

    match positional.first().map(String::as_str) {
        Some("manifest") => {
            match serde_json::to_string_pretty(&view::manifest()) {
                Ok(s) => println!("{s}"),
                Err(e) => {
                    eprintln!("tamias-driver-config: manifest serialization failed: {e}");
                    return ExitCode::FAILURE;
                }
            }
            ExitCode::SUCCESS
        }
        Some("capabilities") => {
            let m = view::manifest();
            println!(
                "{}",
                json!({"capabilities": m.capabilities, "verbs": m.verbs})
            );
            ExitCode::SUCCESS
        }
        Some("serve") => serve(explicit),
        _ => {
            eprintln!(
                "usage: tamias-driver-config manifest|capabilities|serve [--config PATH]\n\
                 \n\
                 The common config API: `query get/set/resolve/config/sources` over \
                 the pavo wire. Config layers: defaults < ~/.config/pavo/ < pavo.toml/\
                 pavo.json in cwd ancestors < --config/PAVO_CONFIG < PAVO_CONFIG_* env."
            );
            ExitCode::from(2)
        }
    }
}

/// ndjson serve loop — one request per line on stdin, one response
/// per line on stdout. Nothing else may touch stdout.
fn serve(explicit: Option<PathBuf>) -> ExitCode {
    let opts = Options {
        explicit,
        ..Options::from_process()
    };
    let stdin = std::io::stdin();
    let mut out = std::io::stdout().lock();
    for line in stdin.lock().lines() {
        let Ok(line) = line else { break };
        let line = line.trim();
        if line.is_empty() {
            continue;
        }
        let response = match parse_request(line) {
            Ok(req) => dispatch(&req, &opts),
            Err(e) => Response::err(format!("{e}")),
        };
        let text = pavo_protocol::wire::response_line(&response).unwrap_or_else(|_| {
            "{\"ok\":false,\"error\":\"response serialization failed\"}".to_string()
        });
        if writeln!(out, "{text}").is_err() || out.flush().is_err() {
            return ExitCode::FAILURE; // client went away
        }
    }
    ExitCode::SUCCESS
}

fn dispatch(req: &Request, opts: &Options) -> Response {
    match req {
        Request::Manifest => Response::ok_typed(&view::manifest()),
        Request::Views => Response::ok_typed(&view::view_summaries()),
        Request::View { view, params } => view_result(view, params, opts),
        Request::Query { query, params } => query_op(query, params, opts),
        Request::Render { .. } => {
            Response::err("config driver ships no renderers — built-in rendering applies")
        }
    }
}

/// `view` op — resolve the spec, feed every query binding.
fn view_result(id: &str, params: &Map<String, Value>, opts: &Options) -> Response {
    let Some(spec) = view::manifest().views.into_iter().find(|v| v.id == id) else {
        return Response::err(format!("unknown view: {id}"));
    };
    let mut data = Map::new();
    for binding in &spec.queries {
        // The binding's static params carry the real query name under
        // "query"; caller params merge on top (they win).
        let real = binding
            .params
            .get("query")
            .and_then(Value::as_str)
            .unwrap_or(&binding.name)
            .to_string();
        let mut merged = binding.params.clone();
        for (k, v) in params {
            merged.insert(k.clone(), v.clone());
        }
        if let Some(payload) = query_data(&real, &merged, opts) {
            data.insert(binding.name.clone(), payload);
        } else {
            data.insert(
                binding.name.clone(),
                json!({"error": format!("unknown query: {real}")}),
            );
        }
    }
    Response::ok_typed(&ViewResult { view: spec, data })
}

/// `query` op — one named query. Unknown names are wire errors
/// (`ok:false`); known-but-failed queries return `{"error": …}` in
/// the data payload (partial degradation, per the wire contract).
fn query_op(query: &str, params: &Map<String, Value>, opts: &Options) -> Response {
    match query_data(query, params, opts) {
        Some(data) => Response::ok_typed(&QueryResult {
            query: query.to_string(),
            data,
        }),
        None => Response::err(format!("unknown query: {query}")),
    }
}

/// The query surface — `get`/`set`/`resolve`/`config`/`sources`.
/// Returns `None` for unknown names; `{"error": …}` payloads for
/// known names that failed (bad params, unwritable file).
fn query_data(query: &str, params: &Map<String, Value>, opts: &Options) -> Option<Value> {
    match query {
        "resolve" => Some(load(opts).merged),
        "config" => {
            let cfg = load(opts);
            Some(Value::Object(cfg.flat.into_iter().collect()))
        }
        "sources" => Some(sources_payload(opts)),
        "get" => {
            let key = params.get("key").and_then(Value::as_str)?;
            let cfg = load(opts);
            Some(match config::get(&cfg, key) {
                Some(v) => json!({"key": key, "value": v, "found": true}),
                None => json!({"key": key, "value": Value::Null, "found": false}),
            })
        }
        "set" => {
            let key = params.get("key").and_then(Value::as_str)?;
            if !params.contains_key("value") {
                return Some(json!({"error": "set requires params.value"}));
            }
            let value = params.get("value").cloned().unwrap_or(Value::Null);
            let cfg = load(opts);
            match config::set(&cfg, key, &value) {
                Ok(outcome) => Some(json!({
                    "key": outcome.key,
                    "value": outcome.value,
                    "path": outcome.path.display().to_string(),
                    "written": true,
                })),
                Err(e) => Some(json!({"error": e})),
            }
        }
        _ => None,
    }
}

/// `sources` payload — every layer in precedence order, including
/// pseudo-layers (defaults, env) and error/missing entries. This is
/// the provenance view: which file gave which keys.
fn sources_payload(opts: &Options) -> Value {
    let cfg = load(opts);
    let rows: Vec<Value> = cfg
        .sources
        .iter()
        .map(|s| {
            json!({
                "source": s.path.as_ref().map(|p| p.display().to_string())
                    .unwrap_or_else(|| s.label.clone()),
                "path": s.path.as_ref().map(|p| p.display().to_string()),
                "kind": s.kind,
                "keys": s.keys,
                "precedence": s.precedence,
                "error": s.error,
            })
        })
        .collect();
    Value::Array(rows)
}
