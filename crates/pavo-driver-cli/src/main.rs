//! tamias-driver-cli — the CLI **surface driver**: given any engine
//! driver's ops manifest (`<driver> ops` / `cli.*` wire queries), it
//! renders that engine's verbs as a real generated CLI — op →
//! subcommand, typed params → args — without the engine shipping any
//! CLI code of its own.
//!
//! ```text
//! tamias-driver-cli kuda find parse --workspace
//! tamias-driver-cli kuda --help          # generated usage from the manifest
//! tamias-driver-cli kuda status --root ~/repo
//! tamias-driver-cli kuda watch           # unbounded surface — argv passthrough
//! echo '{"token":"…"}' | tamias-driver-cli cuora unlock --params-stdin
//!                                            # params via stdin — secret values
//!                                            # never touch argv/shell history
//! tamias-driver-cli ./target/debug/tamias-driver-kuda find parse   # a path works too
//! ```
//!
//! Dispatch: `cli.<op>` rides a one-shot wire query against the
//! target's `serve`; manifest `surfaces` entries (`watch`, `mcp`,
//! `acp`, …) spawn `<driver> run <name> <args>` with inherited stdio
//! — they never return, so they stream rather than capture.
//!
//! Discovery verbs: `manifest` / `capabilities` / `serve` answer
//! honestly — this is a driver (`tamias-driver-cli` on PATH is how
//! users reach it), but a *surface*, not a view provider: `types`
//! is empty and `views` answers `[]`.

use pavo_driver_kit as kit;
use pavo_driver_kit::surface::{self, client};
use pavo_protocol::wire::{Request, Response};
use pavo_protocol::{Contributes, DriverManifest};
use serde_json::json;

/// This driver's own pavo manifest — a surface, not a view provider:
/// no views, no `pavo` capability, empty `types` (it must never claim
/// a content type for tamias's install dispatch).
fn manifest() -> DriverManifest {
    DriverManifest {
        name: "cli".to_string(),
        title: "Generated CLI surface".to_string(),
        icon: None,
        version: env!("CARGO_PKG_VERSION").to_string(),
        verbs: ["manifest", "capabilities", "serve"]
            .iter()
            .map(|v| v.to_string())
            .collect(),
        capabilities: vec!["cli".to_string(), "surface".to_string()],
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

fn serve() -> i32 {
    let manifest = manifest();
    match kit::serve_stdio(|req| dispatch(req, &manifest)) {
        Ok(()) => 0,
        Err(e) => {
            eprintln!("tamias-driver-cli: {e}");
            1
        }
    }
}

fn usage() -> i32 {
    eprintln!(
        "usage: tamias-driver-cli <engine|driver-path> <op> [args]   # generated CLI over the driver's ops manifest\n       tamias-driver-cli manifest|capabilities|serve          # discovery verbs"
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
            v["driver"] = json!("cli");
            println!("{v}");
            0
        }
        Some("serve") => serve(),
        Some("-h") | Some("--help") | None => usage(),
        Some(engine) => {
            let Some(path) = client::find_driver(engine) else {
                eprintln!(
                    "tamias-driver-cli: no driver `{engine}` — want tamias-driver-{engine} on PATH or under tamias-home/drivers, or pass a path"
                );
                std::process::exit(2)
            };
            let ops = match client::ops_manifest(&path) {
                Ok(ops) => ops,
                Err(e) => {
                    eprintln!("tamias-driver-cli: {e}");
                    std::process::exit(2)
                }
            };
            let bin = if ops.engine.is_empty() {
                engine.to_string()
            } else {
                ops.engine.clone()
            };
            let argv: Vec<String> = std::iter::once(bin.clone())
                .chain(args.iter().skip(1).cloned())
                .collect();
            let mut driver = client::DriverClient::new(path);
            surface::cli::run(&bin, &ops, &argv, &mut driver)
        }
    };
    std::process::exit(code);
}
