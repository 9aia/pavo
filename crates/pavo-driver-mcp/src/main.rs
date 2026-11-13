//! tamias-driver-mcp — the MCP **surface driver**: given any engine
//! driver's ops manifest (`<driver> ops`), it serves those ops as
//! MCP tools over stdio — `tools/list` derives a tool + `inputSchema`
//! per op, `tools/call` dispatches through the target's `cli.*` wire
//! query. Engines ship zero MCP code; any MCP client (agent IDE,
//! orchestrator) reaches their ops through one binary.
//!
//! ```text
//! tamias-driver-mcp kuda          # stdio MCP server fronting tamias-driver-kuda
//! tamias-driver-mcp ./target/debug/tamias-driver-kuda
//! ```
//!
//! `surfaces` entries (`watch`, `mcp`, `acp`) never become tools — a
//! `tools/call` must terminate. Reach them through `tamias-driver-cli`.
//!
//! Discovery verbs: `manifest` / `capabilities` / `serve` answer
//! honestly — a *surface*, not a view provider (empty `types`,
//! `views` → `[]`).

use pavo_driver_kit as kit;
use pavo_driver_kit::surface::{client, mcp};
use pavo_protocol::wire::{Request, Response};
use pavo_protocol::{Contributes, DriverManifest};
use serde_json::json;

/// This driver's own pavo manifest — surface, not view provider.
fn manifest() -> DriverManifest {
    DriverManifest {
        name: "mcp".to_string(),
        title: "MCP surface".to_string(),
        icon: None,
        version: env!("CARGO_PKG_VERSION").to_string(),
        verbs: ["manifest", "capabilities", "serve"]
            .iter()
            .map(|v| v.to_string())
            .collect(),
        capabilities: vec!["mcp".to_string(), "surface".to_string()],
        deps: None,
        contributes: Contributes::default(),
        views: Vec::new(),
        renderers: Vec::new(),
        actions: Vec::new(),
    }
}

fn dispatch(req: &Request, manifest: &DriverManifest) -> Response {
    match req {
        Request::Manifest => Response::ok_typed(manifest),
        Request::Views => Response::ok(json!([])),
        _ => Response::err("surface driver — ops manifest + surfaces only, no views"),
    }
}

fn serve_pavo() -> i32 {
    let manifest = manifest();
    match kit::serve_stdio(|req| dispatch(req, &manifest)) {
        Ok(()) => 0,
        Err(e) => {
            eprintln!("tamias-driver-mcp: {e}");
            1
        }
    }
}

fn usage() -> i32 {
    eprintln!(
        "usage: tamias-driver-mcp <engine|driver-path>   # MCP stdio server over the driver's ops\n       tamias-driver-mcp manifest|capabilities|serve  # discovery verbs"
    );
    2
}

fn main() {
    let args: Vec<String> = std::env::args().skip(1).collect();
    let code = match args.first().map(String::as_str) {
        Some("manifest") => {
            println!("{}", kit::manifest_json(&manifest()));
            0
        }
        Some("capabilities") => {
            let mut v = kit::capabilities_value(&manifest());
            v["types"] = json!([]);
            v["driver"] = json!("mcp");
            println!("{v}");
            0
        }
        Some("serve") => serve_pavo(),
        Some("-h") | Some("--help") | None => usage(),
        Some(engine) => {
            let Some(path) = client::find_driver(engine) else {
                eprintln!(
                    "tamias-driver-mcp: no driver `{engine}` — want tamias-driver-{engine} on PATH or under tamias-home/drivers, or pass a path"
                );
                std::process::exit(2)
            };
            let ops = match client::ops_manifest(&path) {
                Ok(ops) => ops,
                Err(e) => {
                    eprintln!("tamias-driver-mcp: {e}");
                    std::process::exit(2)
                }
            };
            let mut driver = client::DriverClient::new(path);
            match mcp::serve_stdio(&ops, &mut driver) {
                Ok(()) => 0,
                Err(e) => {
                    eprintln!("tamias-driver-mcp: {e}");
                    1
                }
            }
        }
    };
    std::process::exit(code);
}
