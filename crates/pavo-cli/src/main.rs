#![cfg_attr(coverage, feature(coverage_attribute))]
//! pavo — the CLI surface of the viz host.
//!
//! Verbs:
//!
//! - `drivers` — list discovered `tamias-driver-*` executables that
//!   speak pavo (manifest version, view count, contributes summary).
//! - `views [--driver <name>]` — every view as a `<driver>:<view>`
//!   ref with title/route.
//! - `chrome` — the merged contribution slots (navbar/dashboard/
//!   statusbar/routes + forward-compat extras).
//! - `view <driver>:<view> [--param k=v …] [--format …]` — resolve a
//!   deep link and render it: `text` (default) summarizes the
//!   ViewNode tree + bound data, `json` dumps the raw
//!   [`ViewResult`], `dot`/`mermaid` emit the view's graph nodes.
//! - `query <driver> <query> [--param k=v …]` — raw `query` op
//!   passthrough.
//! - `serve [--port N]` — the localhost HTTP bridge for the web host:
//!   `/api/*` JSON endpoints over the same [`Host`] plus static
//!   serving of the Nuxt build (`web/.output/public`). First stdout
//!   line is `listening http://127.0.0.1:<port>`.
//! - `open [<driver>:<view>]` — hand off to `pavo-tui` (the ratatui
//!   surface) with the deep link, or the bare dashboard when no
//!   target is given. On a non-TTY it falls back to `pavo view`
//!   text output for the given target.
//! - `manifest` / `schema` — the CLI's own capability manifest and
//!   verb schema (sibling convention: tamias/kuda).
//!
//! `--json` is a per-subcommand flag (sibling convention) on the
//! listing/query verbs; `view --format json` is its machine-readable
//! mode. Errors go to stderr; exit codes are CI-meaningful: `0` ok,
//! `1` view/driver error, `2` usage (bad `--param`, malformed view
//! ref, bad format — clap reports the last itself).
//!
//! Discovery is environment-driven like pavo-core's
//! [`Host::discover`]: drivers are found on `PATH` and under
//! `$TAMIAS_HOME/drivers` — set `PATH`/`TAMIAS_HOME` to point the CLI
//! at fixture drivers (integration tests do exactly this).

mod http;
mod render;
mod serve;

use clap::{Parser, Subcommand};
use pavo_core::{Host, Params, PavoError};
use serde_json::{json, Map, Value};
use std::io::IsTerminal;
use std::process::ExitCode;

#[derive(Parser)]
#[command(
    name = "pavo",
    version,
    about = "Visualization host for the 9aia tool suite"
)]
struct Cli {
    #[command(subcommand)]
    command: Option<Command>,
}

#[derive(Subcommand)]
enum Command {
    /// List discovered pavo-capable drivers.
    Drivers {
        /// JSON output.
        #[arg(long)]
        json: bool,
    },
    /// List every view as `<driver>:<view>` refs.
    Views {
        /// Only views of this driver.
        #[arg(long)]
        driver: Option<String>,
        /// JSON output.
        #[arg(long)]
        json: bool,
    },
    /// Show merged chrome contributions (navbar/dashboard/statusbar/routes).
    Chrome {
        /// JSON output.
        #[arg(long)]
        json: bool,
    },
    /// Resolve + render a view — `pavo view myco:board`.
    View {
        /// `<driver>:<view>` deep link (bare `<driver>` = its first view).
        target: String,
        /// View param, `key=value` (repeatable). Values parse as JSON
        /// when possible, else strings.
        #[arg(long = "param", value_name = "K=V")]
        params: Vec<String>,
        /// Output format — `text` (default), `json`, `dot`, `mermaid`,
        /// or any format a renderer plugin claims (e.g. `svg`,
        /// `ascii`; see a driver's manifest `renderers`).
        #[arg(long, default_value = "text")]
        format: String,
    },
    /// Raw `query` op passthrough against a driver's serve.
    Query {
        /// Driver kind.
        driver: String,
        /// Query/binding name (or raw product op for thin drivers).
        query: String,
        /// Query param, `key=value` (repeatable).
        #[arg(long = "param", value_name = "K=V")]
        params: Vec<String>,
        /// JSON output.
        #[arg(long)]
        json: bool,
    },
    /// Serve the web bridge — localhost HTTP API + static Nuxt host.
    Serve {
        /// Port to bind on 127.0.0.1 (0 = pick a free port; the
        /// actual port is printed as the first stdout line).
        #[arg(long, default_value_t = 0)]
        port: u16,
    },
    /// Open a view in the TUI (`pavo-tui`) — the interactive surface.
    /// Falls back to `view` text output when stdout is not a TTY.
    Open {
        /// `<driver>:<view>` deep link (bare `<driver>` = its first
        /// view). Omit to open the TUI dashboard.
        target: Option<String>,
    },
    /// Print the pavo capability manifest (JSON).
    Manifest,
    /// Print the CLI verb schema (JSON).
    Schema,
}

/// The built-in `view --format` values. Anything else is offered to
/// renderer plugins (manifest `renderers[].surfaces.cli.formats`).
const BUILTIN_FORMATS: [&str; 4] = ["text", "json", "dot", "mermaid"];

/// A CLI failure: exit code + human message (stderr).
struct Fail {
    code: u8,
    msg: String,
}

impl Fail {
    /// View/driver/runtime error → exit 1.
    fn err(msg: impl Into<String>) -> Self {
        Self {
            code: 1,
            msg: msg.into(),
        }
    }

    /// Usage error → exit 2.
    fn usage(msg: impl Into<String>) -> Self {
        Self {
            code: 2,
            msg: msg.into(),
        }
    }
}

impl From<PavoError> for Fail {
    fn from(e: PavoError) -> Self {
        // A malformed `<driver>:<view>` ref is a usage error; driver
        // and resolution failures are runtime errors.
        let code = match &e {
            PavoError::BadViewRef(_) => 2,
            _ => 1,
        };
        Self {
            code,
            msg: e.to_string(),
        }
    }
}

fn main() -> ExitCode {
    let cli = Cli::parse();
    match run(cli) {
        Ok(code) => ExitCode::from(code),
        Err(f) => {
            eprintln!("pavo: {}", f.msg);
            ExitCode::from(f.code)
        }
    }
}

/// `Ok` carries the process exit code (only `open` can produce a
/// non-zero one — it propagates `pavo-tui`'s status).
fn run(cli: Cli) -> Result<u8, Fail> {
    match cli.command {
        None => {
            println!("pavo {}", env!("CARGO_PKG_VERSION"));
            Ok(0)
        }
        Some(Command::Drivers { json }) => cmd_drivers(json).map(|_| 0),
        Some(Command::Views { driver, json }) => cmd_views(driver.as_deref(), json).map(|_| 0),
        Some(Command::Chrome { json }) => cmd_chrome(json).map(|_| 0),
        Some(Command::View {
            target,
            params,
            format,
        }) => cmd_view(&target, &params, &format).map(|_| 0),
        Some(Command::Query {
            driver,
            query,
            params,
            json,
        }) => cmd_query(&driver, &query, &params, json).map(|_| 0),
        Some(Command::Serve { port }) => serve::run(port)
            .map(|_| 0)
            .map_err(|e| Fail::err(format!("serve: {e}"))),
        Some(Command::Open { target }) => cmd_open(target.as_deref()),
        Some(Command::Manifest) => {
            println!(
                "{}",
                json!({
                    "name": "pavo",
                    "version": env!("CARGO_PKG_VERSION"),
                    "verbs": [
                        "drivers", "views", "chrome", "view", "query",
                        "serve", "open", "manifest", "schema"
                    ],
                })
            );
            Ok(0)
        }
        Some(Command::Schema) => {
            println!(
                "{}",
                serde_json::to_string_pretty(&schema_doc())
                    .map_err(|e| Fail::err(format!("schema serialization: {e}")))?
            );
            Ok(0)
        }
    }
}

/// The `schema` verb's document — a JSON description of the CLI's
/// verbs and flags (hand-maintained, sibling-style self-description).
fn schema_doc() -> Value {
    json!({
        "name": "pavo",
        "version": env!("CARGO_PKG_VERSION"),
        "description": "Visualization host — discover tamias drivers, render their views",
        "verbs": {
            "drivers": {
                "summary": "list discovered pavo-capable drivers",
                "flags": {"--json": "bool"}
            },
            "views": {
                "summary": "list all views as <driver>:<view> refs",
                "args": ["--driver <name>"],
                "flags": {"--json": "bool"}
            },
            "chrome": {
                "summary": "merged contribution slots (navbar/dashboard/statusbar/routes)",
                "flags": {"--json": "bool"}
            },
            "view": {
                "summary": "resolve + render a <driver>:<view> deep link",
                "args": ["<driver>:<view>", "--param K=V (repeatable)"],
                "flags": {"--format": "text|json|dot|mermaid|<plugin format>"}
            },
            "query": {
                "summary": "raw query-op passthrough",
                "args": ["<driver>", "<query>", "--param K=V (repeatable)"],
                "flags": {"--json": "bool"}
            },
            "serve": {
                "summary": "localhost HTTP bridge for the web host (api + static)",
                "flags": {"--port": "u16 (0 = free port, printed on stdout)"}
            },
            "open": {
                "summary": "open a view in pavo-tui (interactive surface); \
                            non-TTY falls back to view text output",
                "args": ["<driver>:<view> (optional — omit for the dashboard)"]
            },
            "manifest": {"summary": "print the pavo capability manifest (JSON)"},
            "schema": {"summary": "print this verb schema (JSON)"}
        },
        "wire_ops": ["manifest", "views", "view", "query", "render"],
        "exit_codes": {"0": "ok", "1": "view/driver error", "2": "usage"}
    })
}

// ---------- verbs ----------

/// `pavo open` — hand the deep link to `pavo-tui` on a TTY, else
/// degrade to `view` text output. Returns `pavo-tui`'s exit status.
fn cmd_open(target: Option<&str>) -> Result<u8, Fail> {
    if std::io::stdout().is_terminal() {
        return spawn_tui(target);
    }
    let Some(target) = target else {
        return Err(Fail::usage(
            "pavo open needs a terminal for the TUI — pass <driver>:<view> \
             (or use `pavo view`) for text output",
        ));
    };
    eprintln!("pavo: not a TTY — rendering text output instead of the TUI");
    cmd_view(target, &[], "text").map(|_| 0)
}

/// Candidate `pavo-tui` binaries: the sibling of this executable
/// (covers `cargo run`/target-dir installs) then `PATH`.
fn tui_candidates() -> Vec<std::path::PathBuf> {
    let mut out = Vec::new();
    if let Ok(exe) = std::env::current_exe() {
        if let Some(dir) = exe.parent() {
            out.push(dir.join("pavo-tui"));
        }
    }
    out.push(std::path::PathBuf::from("pavo-tui"));
    out
}

/// Spawn `pavo-tui` (inheriting stdio) with the optional deep link
/// and propagate its exit status.
fn spawn_tui(target: Option<&str>) -> Result<u8, Fail> {
    let mut last_err = String::new();
    for cand in tui_candidates() {
        let mut cmd = std::process::Command::new(&cand);
        if let Some(t) = target {
            cmd.arg(t);
        }
        match cmd.status() {
            Ok(status) => {
                // Signals yield no code; clamp exotic values to 1.
                let code = status
                    .code()
                    .and_then(|c| u8::try_from(c).ok())
                    .unwrap_or(1);
                return Ok(code);
            }
            Err(e) => last_err = format!("{}: {e}", cand.display()),
        }
    }
    Err(Fail::err(format!(
        "pavo-tui not found on PATH or next to pavo ({last_err}) — \
         install it (`cargo xtask install` or `cargo install --path crates/pavo-tui`) \
         or run `cargo run -p pavo-tui`"
    )))
}

fn cmd_drivers(json: bool) -> Result<(), Fail> {
    let mut host = Host::discover();
    let kinds: Vec<String> = host.drivers().iter().map(|d| d.kind.clone()).collect();
    let mut rows = Vec::new();
    for kind in kinds {
        match host.manifest(&kind) {
            Ok(m) => rows.push(json!({
                "driver": kind,
                "title": m.title,
                "version": m.version,
                "views": m.views.len(),
                "renderers": m.renderers.len(),
                "contributes": contrib_counts(&m.contributes),
            })),
            Err(e) => rows.push(json!({
                "driver": kind,
                "manifest_error": e.to_string(),
            })),
        }
    }
    if json {
        println!(
            "{}",
            serde_json::to_string(&rows).map_err(|e| Fail::err(e.to_string()))?
        );
        return Ok(());
    }
    if rows.is_empty() {
        println!("no pavo-capable drivers found (PATH + $TAMIAS_HOME/drivers)");
        return Ok(());
    }
    for row in &rows {
        if let Some(err) = row.get("manifest_error").and_then(Value::as_str) {
            println!(
                "{}  —  manifest error: {err}",
                row["driver"].as_str().unwrap_or("?")
            );
            continue;
        }
        let counts = &row["contributes"];
        let contrib = ["navbar", "dashboard", "statusbar", "routes"]
            .iter()
            .filter(|slot| counts.get(**slot).and_then(Value::as_u64).unwrap_or(0) > 0)
            .map(|slot| format!("{slot}:{}", counts[*slot].as_u64().unwrap_or(0)))
            .collect::<Vec<_>>()
            .join(" ");
        println!(
            "{}  {}  v{}  views:{}  renderers:{}  {}",
            row["driver"].as_str().unwrap_or("?"),
            row["title"].as_str().unwrap_or("?"),
            row["version"].as_str().unwrap_or("?"),
            row["views"].as_u64().unwrap_or(0),
            row["renderers"].as_u64().unwrap_or(0),
            if contrib.is_empty() {
                "contributes: —".to_string()
            } else {
                format!("contributes: {contrib}")
            }
        );
    }
    Ok(())
}

/// Per-slot contribution counts for the `drivers` summary (shared
/// with `serve`'s `/api/drivers`).
pub(crate) fn contrib_counts(c: &pavo_protocol::Contributes) -> Value {
    let mut map = Map::new();
    for (name, entries) in [
        ("navbar", &c.navbar),
        ("dashboard", &c.dashboard),
        ("statusbar", &c.statusbar),
        ("routes", &c.routes),
    ] {
        map.insert(name.to_string(), json!(entries.len()));
    }
    let extra: Map<String, Value> = c
        .extra
        .iter()
        .map(|(k, v)| (k.clone(), json!(v.len())))
        .collect();
    map.insert("extra".to_string(), Value::Object(extra));
    Value::Object(map)
}

fn cmd_views(driver: Option<&str>, json: bool) -> Result<(), Fail> {
    let mut host = Host::discover();
    let mut views = host.list_views();
    if let Some(d) = driver {
        views.retain(|v| v.driver == d);
    }
    if json {
        let rows: Vec<Value> = views
            .iter()
            .map(|v| {
                json!({
                    "driver": v.driver,
                    "driver_title": v.driver_title,
                    "id": v.id,
                    "namespaced": v.namespaced,
                    "title": v.title,
                    "route": v.route,
                    "description": v.description,
                    "queries": v.queries,
                })
            })
            .collect();
        println!(
            "{}",
            serde_json::to_string(&rows).map_err(|e| Fail::err(e.to_string()))?
        );
        return Ok(());
    }
    if views.is_empty() {
        println!("no views found");
        return Ok(());
    }
    for v in &views {
        println!("{:<24} {:<28} {}", v.namespaced, v.title, v.route);
        if let Some(desc) = &v.description {
            println!("{:<24}   {}", "", desc);
        }
    }
    Ok(())
}

fn cmd_chrome(json: bool) -> Result<(), Fail> {
    let mut host = Host::discover();
    let chrome = host.chrome();
    if json {
        let slot = |entries: &[pavo_core::SlotEntry]| -> Vec<Value> {
            entries
                .iter()
                .map(|e| {
                    json!({
                        "driver": e.driver,
                        "driver_title": e.driver_title,
                        "id": e.contribution.id,
                        "title": e.contribution.title,
                        "route": e.contribution.route,
                        "priority": e.contribution.priority,
                        "icon": e.contribution.icon,
                    })
                })
                .collect()
        };
        let extra: Map<String, Value> = chrome
            .extra
            .iter()
            .map(|(k, v)| (k.clone(), Value::Array(slot(v))))
            .collect();
        let doc = json!({
            "navbar": slot(&chrome.navbar),
            "dashboard": slot(&chrome.dashboard),
            "statusbar": slot(&chrome.statusbar),
            "routes": slot(&chrome.routes),
            "extra": Value::Object(extra),
        });
        println!(
            "{}",
            serde_json::to_string_pretty(&doc).map_err(|e| Fail::err(e.to_string()))?
        );
        return Ok(());
    }
    let mut any = false;
    for (name, entries) in [
        ("navbar", &chrome.navbar),
        ("dashboard", &chrome.dashboard),
        ("statusbar", &chrome.statusbar),
        ("routes", &chrome.routes),
    ] {
        println!("{name}:");
        if entries.is_empty() {
            println!("  (none)");
            continue;
        }
        any = true;
        for e in entries {
            let c = &e.contribution;
            println!("  {}  {}  → {}  [{}]", c.id, c.title, c.route, e.driver);
        }
    }
    for (slot_name, entries) in &chrome.extra {
        println!("{slot_name}:");
        any = true;
        for e in entries {
            let c = &e.contribution;
            println!("  {}  {}  → {}  [{}]", c.id, c.title, c.route, e.driver);
        }
    }
    if !any {
        println!("no chrome contributions");
    }
    Ok(())
}

fn cmd_view(target: &str, params: &[String], format: &str) -> Result<(), Fail> {
    let params = parse_params(params)?;
    let mut host = Host::discover();
    let resolved = host.resolve_view(target, params)?;
    match format {
        "json" => {
            println!(
                "{}",
                serde_json::to_string_pretty(&resolved.result)
                    .map_err(|e| Fail::err(e.to_string()))?
            );
        }
        "text" => {
            // Renderer plugins that claim `text` (or wildcard cli
            // formats) substitute their RenderModel per node; the
            // rest render built-in.
            let renders = host.render_view_plugins(&resolved, "cli", Some("text"));
            print!(
                "{}",
                render::render_text_plugins(
                    &resolved.result.view.root,
                    &resolved.result.data,
                    &renders
                )
            );
        }
        "dot" => match render::render_dot(&resolved.result.view.root, &resolved.result.data) {
            Ok(out) => print!("{out}"),
            Err(e) => return Err(Fail::err(format!("{target}: {e}"))),
        },
        "mermaid" => {
            match render::render_mermaid(&resolved.result.view.root, &resolved.result.data) {
                Ok(out) => print!("{out}"),
                Err(e) => return Err(Fail::err(format!("{target}: {e}"))),
            }
        }
        other => {
            // Plugin format: route every claimed node through its
            // driver's `render` op and concatenate the models.
            let renders = host.render_view_plugins(&resolved, "cli", Some(other));
            if renders.is_empty() {
                return Err(Fail::err(format!(
                    "{target}: no renderer plugin handles --format '{other}' \
                     (built-ins: {})",
                    BUILTIN_FORMATS.join(", ")
                )));
            }
            // Deterministic order: sort by path ("" root first, then
            // tree order).
            let mut keyed: Vec<(&String, &pavo_protocol::RenderModel)> = renders.iter().collect();
            keyed.sort_by(|a, b| a.0.cmp(b.0));
            for (_, model) in keyed {
                print!("{}", render::render_model_cli(model));
            }
        }
    }
    Ok(())
}

fn cmd_query(driver: &str, query: &str, params: &[String], json: bool) -> Result<(), Fail> {
    let params = parse_params(params)?;
    let mut host = Host::discover();
    let result = host.query(driver, query, params)?;
    if json {
        println!(
            "{}",
            serde_json::to_string_pretty(&result).map_err(|e| Fail::err(e.to_string()))?
        );
    } else {
        println!(
            "{}",
            serde_json::to_string_pretty(&result.data).map_err(|e| Fail::err(e.to_string()))?
        );
    }
    Ok(())
}

/// Parse repeated `--param k=v` args into a [`Params`] map. Values
/// parse as JSON when possible (`k=42`, `k=true`, `k={…}`), else
/// become strings. Missing `=` or an empty key is a usage error.
fn parse_params(args: &[String]) -> Result<Params, Fail> {
    let mut params = Params::new();
    for arg in args {
        let Some((key, raw)) = arg.split_once('=') else {
            return Err(Fail::usage(format!(
                "--param expects key=value, got '{arg}'"
            )));
        };
        if key.is_empty() {
            return Err(Fail::usage(format!("--param has an empty key: '{arg}'")));
        }
        let value = serde_json::from_str(raw).unwrap_or_else(|_| Value::String(raw.to_string()));
        params.insert(key.to_string(), value);
    }
    Ok(params)
}
