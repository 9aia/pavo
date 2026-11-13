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
//!   statusbar/routes/sidebar/headerActions/notifications/jobs +
//!   forward-compat extras).
//! - `view <driver>:<view> [--param k=v …] [--format …]` — resolve a
//!   deep link and render it: `text` (default) summarizes the
//!   ViewNode tree + bound data, `json` dumps the raw
//!   [`ViewResult`], `dot`/`mermaid` emit the view's graph nodes.
//!   `/path?query` routes (what `Action.route`/`item_route`
//!   templates produce, e.g. `/myco/item?id=T-1`) resolve back to a
//!   `<driver>:<view>` ref + params via declared view routes —
//!   `view`, `open` (non-TTY), `actions`, `invoke`, and `submit`
//!   all accept them.
//! - `query <driver> <query> [--param k=v …]` — raw `query` op
//!   passthrough.
//! - `submit <driver>:<view> [--form <id>] --param k=v …` — the
//!   `submit` wire op: send a form's field values to its driver and
//!   print the `SubmitResult` (message/errors/data; `--json` dumps
//!   the full payload including any updated view).
//! - `actions <driver>:<view>|<driver>` — list the actions a view
//!   (or a whole driver, manifest scope) offers: declared
//!   `actions[]` (manifest/node/tree-item) plus ops-manifest derived
//!   ones, with danger level and param schemas.
//! - `invoke <driver>:<view> <action>` — dispatch an action through
//!   the `invoke` wire op. `--param k=v` feeds `ParamSchema` params,
//!   `--target`/`--target-id` carry the opaque action target,
//!   `--interactive` prompts for missing params on a TTY, and
//!   `--yes` bypasses `confirm`/`type-target` confirmations
//!   (required off-TTY). Actions declaring `route` navigate instead
//!   of dispatching: the template is interpolated over the invoke
//!   context and the resulting route is printed (`{"route": …}`
//!   with `--json`), exit 0. A real invoke whose `ActionResult`
//!   carries `route` appends a `→ <route>` hint in text mode.
//! - `jobs` / `notifications` — the aggregated platform feeds: every
//!   provider driver's `jobs`/`notifications` wire op, tagged with
//!   its driver (`--json` dumps the flattened rows).
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
use pavo_core::{
    collect_actions, interpolate, interpolate_ctx_value, manifest_actions, ops_actions,
    validate_params, ActionScope, Host, Params, PavoError, ResolvedView,
};
use pavo_protocol::{Action, DangerLevel, ParamKind, ParamSchema};
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
    /// Show merged chrome contributions (navbar/dashboard/statusbar/
    /// routes/sidebar/headerActions/notifications/jobs + extras).
    Chrome {
        /// JSON output.
        #[arg(long)]
        json: bool,
    },
    /// Resolve + render a view — `pavo view myco:board` or a
    /// `/myco/item?id=T-1` route.
    View {
        /// `<driver>:<view>` deep link (bare `<driver>` = its first
        /// view) or a `/route?query` matching a declared view route /
        /// `item_route` template.
        target: String,
        /// View param, `key=value` (repeatable). Values parse as JSON
        /// when possible, else strings. A view's `filter_param`
        /// (ViewSpec, convention `q`) is just another `--param`.
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
    /// Submit a form's field values — `pavo submit config:edit
    /// --param key=myco.theme --param value=dark`.
    Submit {
        /// `<driver>:<view>` deep link the form lives in (bare
        /// `<driver>` = its first view).
        target: String,
        /// Form node id when the view carries several forms
        /// (the `id` of the `form` ViewNode).
        #[arg(long)]
        form: Option<String>,
        /// Field value, `key=value` (repeatable) — the form's field
        /// keys. Values parse as JSON when possible, else strings.
        #[arg(long = "param", value_name = "K=V")]
        params: Vec<String>,
        /// JSON output — dumps the full SubmitResult.
        #[arg(long)]
        json: bool,
    },
    /// List the actions a target offers — declared `actions[]`
    /// (manifest, view-node, tree-item scopes) plus ops-manifest
    /// derived actions, with danger level + param schemas.
    Actions {
        /// `<driver>:<view>` deep link, or bare `<driver>` for
        /// manifest-scope actions only.
        target: String,
        /// View param, `key=value` (repeatable) — used when
        /// resolving the view to collect node/item actions.
        #[arg(long = "param", value_name = "K=V")]
        params: Vec<String>,
        /// JSON output.
        #[arg(long)]
        json: bool,
    },
    /// Invoke an action — `pavo invoke myco:board refresh`.
    /// Dispatches the `invoke` wire op; `--json` dumps the full
    /// ActionResult.
    Invoke {
        /// `<driver>:<view>` deep link the action lives in (bare
        /// `<driver>` = its first view).
        target: String,
        /// Action id — see `pavo actions <driver>:<view>`.
        action: String,
        /// Action param, `key=value` (repeatable) — the action's
        /// `ParamSchema` names. Values parse as JSON when possible,
        /// else strings.
        #[arg(long = "param", value_name = "K=V")]
        params: Vec<String>,
        /// Opaque action target — JSON (`{"id":"T-1"}`) or a bare
        /// string. What the action's `target` context calls for
        /// (selected row, item id, …); the driver interprets it.
        #[arg(long = "target", value_name = "JSON-OR-STRING")]
        action_target: Option<String>,
        /// Shorthand for `--target <id>` as a plain string — also
        /// the `type-target` confirmation: the id *is* the typed
        /// confirmation, so `--yes --target-id T-1` is the
        /// non-interactive spelling.
        #[arg(long = "target-id", value_name = "ID")]
        target_id: Option<String>,
        /// Skip confirmation prompts. `confirm` actions require it
        /// off-TTY; `type-target` actions still need `--target-id`
        /// (the id is the confirmation — `--yes` only skips the
        /// interactive "type it" prompt).
        #[arg(long)]
        yes: bool,
        /// Prompt on a TTY for each action param not given via
        /// `--param` (uses the ParamSchema: enum lists options,
        /// bool y/n, defaults on empty input). Errors off-TTY.
        #[arg(long)]
        interactive: bool,
        /// JSON output — dumps the full ActionResult.
        #[arg(long)]
        json: bool,
    },
    /// List the aggregated job feed — every `contributes.jobs`
    /// provider driver's `jobs` wire op, tagged with its driver.
    Jobs {
        /// JSON output.
        #[arg(long)]
        json: bool,
    },
    /// List the aggregated notification feed — every
    /// `contributes.notifications` provider driver's `notifications`
    /// wire op, tagged with its driver.
    Notifications {
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
        /// view) or a `/route?query`. Omit to open the TUI dashboard.
        target: Option<String>,
    },
    /// Print the pavo capability manifest (JSON).
    Manifest,
    /// Print the CLI verb schema (JSON).
    Schema,
}

/// The built-in `view --format` values — all but `json` can be
/// claimed by renderer plugins (`renderers[].surfaces.cli.formats`,
/// empty = wildcard); a plugin claim substitutes its RenderModel for
/// the claimed nodes and the built-in covers the rest. `json` is the
/// raw `ViewResult` dump — host-owned, never routed to plugins.
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
        Some(Command::Submit {
            target,
            form,
            params,
            json,
        }) => cmd_submit(&target, form.as_deref(), &params, json).map(|_| 0),
        Some(Command::Actions {
            target,
            params,
            json,
        }) => cmd_actions(&target, &params, json).map(|_| 0),
        Some(Command::Invoke {
            target,
            action,
            params,
            action_target,
            target_id,
            yes,
            interactive,
            json,
        }) => cmd_invoke(
            &target,
            &action,
            &params,
            action_target.as_deref(),
            target_id.as_deref(),
            yes,
            interactive,
            json,
        )
        .map(|_| 0),
        Some(Command::Jobs { json }) => cmd_jobs(json).map(|_| 0),
        Some(Command::Notifications { json }) => cmd_notifications(json).map(|_| 0),
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
                        "submit", "actions", "invoke", "jobs",
                        "notifications", "serve", "open", "manifest",
                        "schema"
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
                "summary": "merged contribution slots (navbar/dashboard/statusbar/routes/\
                            sidebar/headerActions/notifications/jobs + extras)",
                "flags": {"--json": "bool"}
            },
            "view": {
                "summary": "resolve + render a <driver>:<view> deep link \
                            or /route?query (declared view routes)",
                "args": ["<driver>:<view> | /route?query",
                         "--param K=V (repeatable — filter_param is one)"],
                "flags": {"--format": "text|json|dot|mermaid|<plugin format>"}
            },
            "query": {
                "summary": "raw query-op passthrough",
                "args": ["<driver>", "<query>", "--param K=V (repeatable)"],
                "flags": {"--json": "bool"}
            },
            "submit": {
                "summary": "submit a form's field values (submit wire op)",
                "args": ["<driver>:<view>", "--form <id>", "--param K=V (repeatable)"],
                "flags": {"--json": "bool — dump the full SubmitResult"}
            },
            "actions": {
                "summary": "list actions offered by a view or driver \
                            (declared actions[] + ops-manifest derived)",
                "args": ["<driver>:<view> | <driver>", "--param K=V (repeatable)"],
                "flags": {"--json": "bool"}
            },
            "invoke": {
                "summary": "dispatch an action (invoke wire op)",
                "args": ["<driver>:<view>", "<action>",
                         "--param K=V (repeatable)", "--target <json|string>",
                         "--target-id <id>", "--yes", "--interactive"],
                "flags": {"--json": "bool — dump the full ActionResult"}
            },
            "jobs": {
                "summary": "aggregated job feed (contributes.jobs providers' jobs op)",
                "flags": {"--json": "bool"}
            },
            "notifications": {
                "summary": "aggregated notification feed (contributes.notifications providers)",
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
        "wire_ops": [
            "manifest", "views", "view", "query", "render", "submit",
            "invoke", "jobs", "notifications"
        ],
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
    cmd_drivers_host(&mut host, json)
}

/// [`cmd_drivers`] over an explicit host — split for tests.
fn cmd_drivers_host(host: &mut Host, json: bool) -> Result<(), Fail> {
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
        let contrib = [
            "navbar",
            "dashboard",
            "statusbar",
            "routes",
            "sidebar",
            "headerActions",
            "notifications",
            "jobs",
        ]
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

/// Serialize host feed entries (`jobs`/`notifications`) for `--json`
/// output and the serve bridge's `/api/jobs` + `/api/notifications`:
/// each item's fields flattened into one object with `driver` /
/// `driver_title` attribution — the host fields win over any
/// same-named item fields (including `extra` catch-all keys).
pub(crate) fn feed_rows<T: serde::Serialize>(
    entries: &[pavo_core::FeedEntry<T>],
) -> Result<Vec<Value>, serde_json::Error> {
    entries
        .iter()
        .map(|e| {
            let mut v = serde_json::to_value(&e.item)?;
            if let Value::Object(ref mut map) = v {
                map.insert("driver".to_string(), json!(e.driver));
                map.insert("driver_title".to_string(), json!(e.driver_title));
            }
            Ok(v)
        })
        .collect()
}

/// Per-slot contribution counts for the `drivers` summary (shared
/// with `serve`'s `/api/drivers`). Slot keys use the wire spellings —
/// `headerActions` is camelCase like the manifest field.
pub(crate) fn contrib_counts(c: &pavo_protocol::Contributes) -> Value {
    let mut map = Map::new();
    for (name, entries) in [
        ("navbar", &c.navbar),
        ("dashboard", &c.dashboard),
        ("statusbar", &c.statusbar),
        ("routes", &c.routes),
        ("sidebar", &c.sidebar),
        ("headerActions", &c.header_actions),
        ("notifications", &c.notifications),
        ("jobs", &c.jobs),
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
    cmd_views_host(&mut host, driver, json)
}

/// [`cmd_views`] over an explicit host — split for tests.
fn cmd_views_host(host: &mut Host, driver: Option<&str>, json: bool) -> Result<(), Fail> {
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
    cmd_chrome_host(&mut host, json)
}

/// [`cmd_chrome`] over an explicit host — split for tests.
fn cmd_chrome_host(host: &mut Host, json: bool) -> Result<(), Fail> {
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
            "sidebar": slot(&chrome.sidebar),
            // Wire spelling — matches `contributes.headerActions`.
            "headerActions": slot(&chrome.header_actions),
            "notifications": slot(&chrome.notifications),
            "jobs": slot(&chrome.jobs),
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
        ("sidebar", &chrome.sidebar),
        ("headerActions", &chrome.header_actions),
        ("notifications", &chrome.notifications),
        ("jobs", &chrome.jobs),
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
    cmd_view_host(&mut host, target, params, format)
}

/// [`cmd_view`] over an explicit host — split for tests.
fn cmd_view_host(host: &mut Host, target: &str, params: Params, format: &str) -> Result<(), Fail> {
    // `/route?query` deep links → `<driver>:<view>` + params.
    let (target, params) = resolve_deep_link(host, target, params)?;
    let resolved = host.resolve_view(&target, params)?;
    let out = view_output(host, &resolved, &target, format)?;
    print!("{out}");
    Ok(())
}

/// The `view` verb's format dispatch — returns the exact bytes to
/// print (split from [`cmd_view_host`] so tests can assert on
/// output).
///
/// Precedence: renderer plugins are consulted for **every** format
/// before the built-in interpretation runs — `text`/`dot`/`mermaid`
/// substitute claimed nodes' RenderModels per tree path and keep the
/// built-in rendering for unclaimed nodes. `json` is exempt (the raw
/// `ViewResult` dump is host-owned). Any *other* format string is a
/// plugin-only format: every claimed node's RenderModel is emitted;
/// no claims → usage error naming the built-ins.
fn view_output(
    host: &Host,
    resolved: &ResolvedView,
    target: &str,
    format: &str,
) -> Result<String, Fail> {
    match format {
        "json" => serde_json::to_string_pretty(&resolved.result)
            .map(|s| format!("{s}\n"))
            .map_err(|e| Fail::err(e.to_string())),
        "text" => {
            // Renderer plugins that claim `text` (or wildcard cli
            // formats) substitute their RenderModel per node; the
            // rest render built-in.
            let renders = host.render_view_plugins(resolved, "cli", Some("text"));
            let mut out = render::render_text_plugins(
                &resolved.result.view.root,
                &resolved.result.data,
                &renders,
            );
            // Action affordance: one footer line naming the declared
            // actions (danger-marked) so `pavo invoke` is discoverable.
            if let Ok(manifest) = host.manifest(&resolved.driver) {
                let mut seen = std::collections::BTreeSet::new();
                let hints: Vec<String> = collect_actions(&manifest, &resolved.result)
                    .iter()
                    .filter(|s| seen.insert(s.action.id.clone()))
                    .map(|s| {
                        let suffix = match s.action.danger {
                            DangerLevel::None => "",
                            DangerLevel::Confirm => " (confirm)",
                            DangerLevel::TypeTarget => " (type-target)",
                        };
                        format!("{}{}", s.action.id, suffix)
                    })
                    .collect();
                if !hints.is_empty() {
                    out.push_str(&format!("\nactions: {}\n", hints.join(", ")));
                }
            }
            Ok(out)
        }
        "dot" => {
            // Plugins claiming `dot` (or wildcard cli formats) take
            // precedence per node; unclaimed graphs still emit
            // digraph blocks.
            let renders = host.render_view_plugins(resolved, "cli", Some("dot"));
            render::render_dot_plugins(&resolved.result.view.root, &resolved.result.data, &renders)
                .map_err(|e| Fail::err(format!("{target}: {e}")))
        }
        "mermaid" => {
            let renders = host.render_view_plugins(resolved, "cli", Some("mermaid"));
            render::render_mermaid_plugins(
                &resolved.result.view.root,
                &resolved.result.data,
                &renders,
            )
            .map_err(|e| Fail::err(format!("{target}: {e}")))
        }
        other => {
            // Plugin format: route every claimed node through its
            // driver's `render` op and concatenate the models.
            let renders = host.render_view_plugins(resolved, "cli", Some(other));
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
            let mut out = String::new();
            for (_, model) in keyed {
                out.push_str(&render::render_model_cli(model));
            }
            Ok(out)
        }
    }
}

fn cmd_query(driver: &str, query: &str, params: &[String], json: bool) -> Result<(), Fail> {
    let params = parse_params(params)?;
    let mut host = Host::discover();
    cmd_query_host(&mut host, driver, query, params, json)
}

/// [`cmd_query`] over an explicit host — split for tests.
fn cmd_query_host(
    host: &mut Host,
    driver: &str,
    query: &str,
    params: Params,
    json: bool,
) -> Result<(), Fail> {
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

fn cmd_submit(target: &str, form: Option<&str>, params: &[String], json: bool) -> Result<(), Fail> {
    let values = parse_params(params)?;
    let mut host = Host::discover();
    cmd_submit_host(&mut host, target, form, values, json)
}

/// [`cmd_submit`] over an explicit host — split for tests.
fn cmd_submit_host(
    host: &mut Host,
    target: &str,
    form: Option<&str>,
    values: Params,
    json: bool,
) -> Result<(), Fail> {
    // `/route?query` → ref; submit has no view-param channel, so
    // route-derived params are dropped (noted on stderr).
    let (target, route_params) = resolve_deep_link(host, target, Params::new())?;
    if !route_params.is_empty() {
        eprintln!("pavo: note — route query params don't apply to submit");
    }
    let result = host.submit(&target, form, values)?;
    if json {
        println!(
            "{}",
            serde_json::to_string_pretty(&result).map_err(|e| Fail::err(e.to_string()))?
        );
    } else {
        // Text mode: message first, then data.
        if let Some(message) = &result.message {
            println!("{message}");
        }
        match &result.data {
            Some(data) => println!(
                "{}",
                serde_json::to_string_pretty(data).map_err(|e| Fail::err(e.to_string()))?
            ),
            None if result.message.is_none() && result.errors.is_empty() => {
                println!("(empty result)");
            }
            None => {}
        }
    }
    // Per-field validation errors mean the driver *rejected* the
    // submit (the wire still answered ok:true). Exit 1 in both
    // modes so CI can gate on it; `--json` callers also get the
    // field details in the payload on stdout.
    if !result.errors.is_empty() {
        let detail = result
            .errors
            .iter()
            .map(|(k, v)| format!("{k}: {}", v.as_str().unwrap_or("invalid")))
            .collect::<Vec<_>>()
            .join("; ");
        return Err(Fail {
            code: 1,
            msg: format!("submit rejected — {detail}"),
        });
    }
    Ok(())
}

// ---------- actions / invoke ----------

/// One action row for `pavo actions` and invoke's danger/param
/// lookup — owned clones so they outlive the manifest/view borrows.
struct ActionRow {
    /// `manifest` | `node` | `item` | `ops` (ops-manifest derived).
    scope: &'static str,
    /// View the action was found in (`driver:view` context).
    view: Option<String>,
    /// For item-scoped actions — the tree item's id.
    item_id: Option<String>,
    action: Action,
}

/// Parse `<driver>:<view>` / `<driver>` — same grammar as
/// `Host::parse_target` (kept local so the CLI can report usage
/// errors before touching the host).
fn split_view_ref(target: &str) -> Result<(String, Option<String>), Fail> {
    let t = target.trim();
    if t.is_empty() {
        return Err(Fail::usage("empty view ref"));
    }
    match t.split_once(':') {
        None => Ok((t.to_string(), None)),
        Some((driver, view)) if !driver.is_empty() && !view.is_empty() && !view.contains(':') => {
            Ok((driver.to_string(), Some(view.to_string())))
        }
        _ => Err(Fail::usage(format!(
            "malformed view ref '{target}' — expected <driver>:<view> or <driver>"
        ))),
    }
}

// ---------- `/route?query` deep links ----------
//
// `Action.route`, `ViewSpec.item_route`, `MenuEntry.route`, and
// `ActionResult.route` all produce host-relative route strings like
// `/myco/item?id=T-1`. The CLI is non-interactive, so "navigate"
// means resolving the route back to a `<driver>:<view>` ref + view
// params and rendering/invoking there. Matching is local to this
// file (pavo-core stays transport-pure): every discovered
// manifest's `ViewSpec.route` and `item_route` is a candidate
// pattern.
//
// Limits (v1):
// - Only *view* routes resolve — `contributes.routes` pages are
//   driver-owned and have no `resolve_view` target.
// - `{name}` placeholders match exactly one whole path segment and
//   capture into a view param named `name` (`{item.id}` captures as
//   `"item.id"`); literals must match segment-for-segment. The
//   candidate's own `?query` template is ignored for matching —
//   the request's query pairs become the params.
// - Best match wins: fewest captures, then longest pattern.

/// Match request path `segs` (decoded, split on `/`) against one
/// candidate route `pattern` (`/myco/item/{item.id}?id={item.id}` —
/// its query half is ignored). Returns the `{name}` captures on
/// success.
fn match_route_pattern(pattern: &str, segs: &[String]) -> Option<(Params, usize)> {
    let pat_path = pattern.split(['?', '#']).next().unwrap_or(pattern);
    let pat: Vec<&str> = pat_path.split('/').filter(|s| !s.is_empty()).collect();
    if pat.len() != segs.len() {
        return None;
    }
    let mut params = Params::new();
    let mut captures = 0;
    for (p, s) in pat.iter().zip(segs.iter()) {
        if let Some(name) = p.strip_prefix('{').and_then(|x| x.strip_suffix('}')) {
            let name = name.trim();
            if name.is_empty() {
                return None;
            }
            params.insert(name.to_string(), Value::String(s.clone()));
            captures += 1;
        } else if p != s {
            return None;
        }
    }
    Some((params, captures))
}

/// Segment count of a candidate pattern's path half (scoring).
fn route_pattern_len(pattern: &str) -> usize {
    pattern
        .split(['?', '#'])
        .next()
        .unwrap_or(pattern)
        .split('/')
        .filter(|s| !s.is_empty())
        .count()
}

/// Resolve a `/path?query` route to a `<driver>:<view>` ref + view
/// params by matching declared `ViewSpec.route`/`item_route`
/// patterns across all discovered manifests. `None` when nothing
/// matches — callers only invoke this for leading-`/` targets.
///
/// Query pairs become params via the same scalar rule as `--param`
/// ([`http::param_json`]: JSON when possible, else string) and
/// override `{name}` path captures on key conflicts.
#[must_use]
pub(crate) fn route_target(host: &Host, route: &str) -> Option<(String, Params)> {
    let route = route.trim();
    if !route.starts_with('/') {
        return None;
    }
    let (path, query) = route.split_once('?').unwrap_or((route, ""));
    let path = path.split('#').next().unwrap_or(path);
    let segs: Vec<String> = path
        .split('/')
        .filter(|s| !s.is_empty())
        .map(http::percent_decode)
        .collect();
    if segs.is_empty() {
        return None;
    }
    // Deterministic scan: driver kinds sorted, manifest view order.
    let mut kinds: Vec<String> = host.drivers().iter().map(|d| d.kind.clone()).collect();
    kinds.sort();
    // Best: (namespaced ref, captures, pattern len, is_item_route,
    // capture params). `ViewSpec.route` is canonical — it outranks
    // `item_route` templates on ties (an item_route like
    // `/item?id={item.id}` shares its path with the detail view's
    // own route and must not shadow it).
    let mut best: Option<(String, usize, usize, bool, Params)> = None;
    for kind in kinds {
        let Ok(manifest) = host.manifest(&kind) else {
            continue;
        };
        for spec in &manifest.views {
            for (pattern, is_item_route) in [
                (Some(spec.route.as_str()), false),
                (spec.item_route.as_deref(), true),
            ]
            .into_iter()
            .filter_map(|(p, i)| p.map(|p| (p, i)))
            {
                let Some((capture_params, captures)) = match_route_pattern(pattern, &segs) else {
                    continue;
                };
                if std::env::var_os("PAVO_DEBUG_ROUTE").is_some() {
                    eprintln!(
                        "  candidate {kind}:{} pattern={pattern:?} captures={captures}",
                        spec.id
                    );
                }
                let plen = route_pattern_len(pattern);
                let better = match &best {
                    None => true,
                    Some((_, bc, bl, bi, _)) => {
                        (captures, std::cmp::Reverse(plen), is_item_route)
                            < (*bc, std::cmp::Reverse(*bl), *bi)
                    }
                };
                if better {
                    best = Some((
                        format!("{kind}:{}", spec.id),
                        captures,
                        plen,
                        is_item_route,
                        capture_params,
                    ));
                }
            }
        }
    }
    if std::env::var_os("PAVO_DEBUG_ROUTE").is_some() {
        eprintln!("route_target({route:?}) -> {best:?}");
    }
    best.map(|(target, _, _, _, mut params)| {
        for (k, v) in http::parse_query(query) {
            if !k.is_empty() {
                params.insert(k, http::param_json(&v));
            }
        }
        (target, params)
    })
}

/// Normalize a CLI target that may be a `/route?query` deep link:
/// routes resolve to `<driver>:<view>` + params (explicit `params`
/// win over route-derived ones); anything else passes through
/// untouched. A leading-`/` target matching no declared route is a
/// hard error — the driver-ref grammar never produces leading
/// slashes, so it's unambiguously a route.
fn resolve_deep_link(host: &Host, target: &str, params: Params) -> Result<(String, Params), Fail> {
    if !target.trim_start().starts_with('/') {
        return Ok((target.to_string(), params));
    }
    match route_target(host, target) {
        Some((reference, mut route_params)) => {
            for (k, v) in params {
                route_params.insert(k, v);
            }
            Ok((reference, route_params))
        }
        None => Err(Fail::err(format!(
            "unknown route '{target}' — no view route or item_route \
             template matches (see `pavo views` for declared routes)"
        ))),
    }
}

/// The interpolation context for `Action.route`/`ActionResult.route`
/// templates — the same shape [`interpolate_ctx_value`] builds for
/// interactive hosts (`driver`/`view`/`data`/`target`/`params`/
/// `item`). The view resolve is best-effort and usually cache-warm
/// (`action_rows` already fetched it when the target names a view).
fn invoke_ctx(
    host: &Host,
    target: &str,
    view_params: &Params,
    item_id: Option<&str>,
    action_target: Option<&Value>,
    params: &Params,
) -> Value {
    let driver = split_view_ref(target).map(|(d, _)| d).unwrap_or_default();
    let view = host.resolve_view(target, view_params.clone()).ok();
    let item = item_id.map(|id| json!({ "id": id }));
    interpolate_ctx_value(
        &driver,
        view.as_ref().map(|r| &r.result),
        action_target,
        params,
        item.as_ref(),
    )
}

/// Collect the action rows visible at `target`: declared actions
/// (manifest-global always; node/tree-item when the target names a
/// resolvable view) plus ops-manifest derived actions as a
/// zero-declaration fallback. `strict_view` propagates view
/// resolution errors (the `actions` verb); invoke passes `false`
/// and falls back to manifest scope so undeclared/param-gated views
/// still dispatch.
fn action_rows(
    host: &Host,
    target: &str,
    view_params: Params,
    strict_view: bool,
) -> Result<Vec<ActionRow>, Fail> {
    let (driver, view_id) = split_view_ref(target)?;
    let manifest = host.manifest(&driver)?;
    let mut rows: Vec<ActionRow> = Vec::new();
    let mut declared: std::collections::BTreeSet<String> = std::collections::BTreeSet::new();

    // Node/item actions need the resolved view — only attempted when
    // the target names one (bare <driver> = manifest scope).
    let resolved = if view_id.is_some() {
        match host.resolve_view(target, view_params) {
            Ok(r) => Some(r),
            Err(e) if strict_view => return Err(e.into()),
            Err(_) => None,
        }
    } else {
        None
    };
    match &resolved {
        Some(r) => {
            for s in collect_actions(&manifest, &r.result) {
                declared.insert(s.action.id.clone());
                rows.push(ActionRow {
                    scope: match s.scope {
                        ActionScope::Manifest => "manifest",
                        ActionScope::Node => "node",
                        ActionScope::Item => "item",
                    },
                    view: s.view.clone(),
                    item_id: s.item_id.map(|id| id.into_owned()),
                    action: s.action.into_owned(),
                });
            }
        }
        None => {
            for a in manifest_actions(&manifest) {
                declared.insert(a.id.clone());
                rows.push(ActionRow {
                    scope: "manifest",
                    view: view_id.clone(),
                    item_id: None,
                    action: a.clone(),
                });
            }
        }
    }

    // Ops-manifest fallback: `query "ops"` answers
    // `{ops:[{name,doc,params,mutates}]}` on surface drivers — each
    // op becomes an action when no declaration claimed the id.
    // Drivers without an ops manifest just error → skipped.
    if let Ok(q) = host.query(&driver, "ops", Params::new()) {
        for a in ops_actions(&q.data) {
            if declared.insert(a.id.clone()) {
                rows.push(ActionRow {
                    scope: "ops",
                    view: view_id.clone(),
                    item_id: None,
                    action: a,
                });
            }
        }
    }
    Ok(rows)
}

fn danger_label(d: DangerLevel) -> &'static str {
    match d {
        DangerLevel::None => "none",
        DangerLevel::Confirm => "confirm",
        DangerLevel::TypeTarget => "type-target",
    }
}

fn kind_label(k: ParamKind) -> &'static str {
    match k {
        ParamKind::String => "string",
        ParamKind::Text => "text",
        ParamKind::Number => "number",
        ParamKind::Integer => "integer",
        ParamKind::Bool => "bool",
        ParamKind::Enum => "enum",
        ParamKind::Date => "date",
        ParamKind::Ref => "ref",
        ParamKind::StringList => "string[]",
    }
}

fn cmd_actions(target: &str, params: &[String], json: bool) -> Result<(), Fail> {
    let view_params = parse_params(params)?;
    let mut host = Host::discover();
    cmd_actions_host(&mut host, target, view_params, json)
}

/// [`cmd_actions`] over an explicit host — split for tests.
fn cmd_actions_host(
    host: &mut Host,
    target: &str,
    view_params: Params,
    json: bool,
) -> Result<(), Fail> {
    // `/route?query` → ref; `--param` are view params here, so they
    // merge over route-derived ones.
    let (target, view_params) = resolve_deep_link(host, target, view_params)?;
    let rows = action_rows(host, &target, view_params, true)?;
    if json {
        let out: Vec<Value> = rows
            .iter()
            .map(|r| {
                json!({
                    "scope": r.scope,
                    "view": r.view,
                    "item": r.item_id,
                    "id": r.action.id,
                    "label": r.action.label,
                    "danger": r.action.danger,
                    "keys": r.action.keys,
                    "target": r.action.target,
                    "route": r.action.route,
                    "description": r.action.description,
                    "params": r.action.params,
                })
            })
            .collect();
        println!(
            "{}",
            serde_json::to_string_pretty(&out).map_err(|e| Fail::err(e.to_string()))?
        );
        return Ok(());
    }
    if rows.is_empty() {
        println!("no actions");
        return Ok(());
    }
    for r in &rows {
        let a = &r.action;
        let scope = match r.scope {
            "item" => format!("item:{}", r.item_id.as_deref().unwrap_or("?")),
            s => s.to_string(),
        };
        println!(
            "{:<16} {:<28} {:<11} {}",
            scope,
            a.id,
            danger_label(a.danger),
            a.label
        );
        if !a.params.is_empty() {
            let ps = a
                .params
                .iter()
                .map(|p| {
                    format!(
                        "{}:{}{}",
                        p.name,
                        kind_label(p.kind),
                        if p.required { "!" } else { "" }
                    )
                })
                .collect::<Vec<_>>()
                .join(" ");
            println!("    params: {ps}");
        }
        if !a.keys.is_empty() {
            println!("    keys: {}", a.keys.join(", "));
        }
        if let Some(t) = &a.target {
            println!("    target: {t}");
        }
        // Route actions navigate instead of dispatching — surface
        // the template where danger/params would otherwise lead.
        if let Some(route) = &a.route {
            println!("    → {route}");
        }
        if let Some(d) = &a.description {
            println!("    {}", d.lines().next().unwrap_or(""));
        }
    }
    Ok(())
}

/// Parse one scalar the way `--param` values parse: JSON when
/// possible, else a string.
fn parse_scalar(raw: &str) -> Value {
    serde_json::from_str(raw).unwrap_or_else(|_| Value::String(raw.to_string()))
}

/// Read one line from stdin for `--interactive`/confirm prompts.
/// Prompts go to stderr so stdout stays machine-clean.
fn read_tty_line(prompt: &str) -> Result<String, Fail> {
    use std::io::Write;
    eprint!("{prompt}");
    let _ = std::io::stderr().flush();
    let mut line = String::new();
    std::io::stdin()
        .read_line(&mut line)
        .map_err(|e| Fail::err(format!("read stdin: {e}")))?;
    Ok(line.trim().to_string())
}

/// Enforce the action's [`DangerLevel`] before dispatch.
/// `action_target` is the already-parsed `--target`/`--target-id`
/// value. `--yes` short-circuits everything *except* the
/// type-target id requirement (the id is the confirmation).
fn check_danger(
    action: &str,
    danger: DangerLevel,
    action_target: Option<&Value>,
    yes: bool,
) -> Result<(), Fail> {
    match danger {
        DangerLevel::None => Ok(()),
        DangerLevel::Confirm => {
            if yes {
                return Ok(());
            }
            if !std::io::stdin().is_terminal() {
                return Err(Fail::usage(format!(
                    "action '{action}' needs confirmation — re-run with --yes \
                     (non-interactive) or on a TTY"
                )));
            }
            let ans = read_tty_line(&format!("invoke '{action}'? [y/N] "))?;
            if matches!(ans.to_ascii_lowercase().as_str(), "y" | "yes") {
                Ok(())
            } else {
                Err(Fail {
                    code: 1,
                    msg: format!("aborted — '{action}' not invoked"),
                })
            }
        }
        DangerLevel::TypeTarget => {
            // The typed id doubles as the action target — it must be
            // a plain string (`--target-id`, or `--target` carrying a
            // JSON string).
            let id = match action_target {
                Some(Value::String(s)) if !s.is_empty() => s.clone(),
                Some(_) => {
                    return Err(Fail::usage(format!(
                        "type-target action '{action}' needs a string id — \
                         pass --target-id <id>"
                    )));
                }
                None => {
                    return Err(Fail::usage(format!(
                        "type-target action '{action}' needs the target id — \
                         pass --target-id <id>"
                    )));
                }
            };
            if yes {
                return Ok(());
            }
            if !std::io::stdin().is_terminal() {
                return Err(Fail::usage(format!(
                    "type-target action '{action}' needs confirmation — \
                     re-run with --yes --target-id {id} (non-interactive) \
                     or on a TTY"
                )));
            }
            let ans = read_tty_line(&format!("type '{id}' to confirm '{action}': "))?;
            if ans == id {
                Ok(())
            } else {
                Err(Fail {
                    code: 1,
                    msg: format!("aborted — '{action}' not invoked"),
                })
            }
        }
    }
}

/// Prompt for one missing param on a TTY, guided by its
/// [`ParamSchema`]. Returns the coerced JSON value.
fn prompt_param(p: &ParamSchema) -> Result<Value, Fail> {
    let label = p.label.clone().unwrap_or_else(|| p.name.clone());
    let mut prompt = format!("{label}:");
    if let Some(h) = p.hint.as_deref().or(p.doc.as_deref()) {
        prompt.push_str(&format!(" ({h})"));
    }
    if !p.options.is_empty() {
        prompt.push_str(&format!(" [{}]", p.options.join("|")));
    }
    if let Some(d) = &p.default {
        prompt.push_str(&format!(" [default: {}]", d));
    }
    prompt.push(' ');
    let raw = read_tty_line(&prompt)?;
    if raw.is_empty() {
        if let Some(d) = &p.default {
            return Ok(d.clone());
        }
        return Ok(Value::Null);
    }
    let value = match p.kind {
        ParamKind::Bool => match raw.to_ascii_lowercase().as_str() {
            "y" | "yes" | "true" | "1" | "on" => Value::Bool(true),
            "n" | "no" | "false" | "0" | "off" => Value::Bool(false),
            _ => {
                return Err(Fail::usage(format!(
                    "param '{}': expected y/n, got '{raw}'",
                    p.name
                )))
            }
        },
        ParamKind::Integer => raw.parse::<i64>().map(Value::from).map_err(|_| {
            Fail::usage(format!("param '{}': expected integer, got '{raw}'", p.name))
        })?,
        ParamKind::Number => raw.parse::<f64>().map(Value::from).map_err(|_| {
            Fail::usage(format!("param '{}': expected number, got '{raw}'", p.name))
        })?,
        ParamKind::Enum => {
            // Exact option, or 1-based index into options.
            if p.options.iter().any(|o| o == &raw) {
                Value::String(raw)
            } else if let Ok(i) = raw.parse::<usize>() {
                match p.options.get(i.wrapping_sub(1)) {
                    Some(o) => Value::String(o.clone()),
                    None => {
                        return Err(Fail::usage(format!(
                            "param '{}': option {i} out of range [{}]",
                            p.name,
                            p.options.join("|")
                        )))
                    }
                }
            } else {
                return Err(Fail::usage(format!(
                    "param '{}': expected one of [{}], got '{raw}'",
                    p.name,
                    p.options.join("|")
                )));
            }
        }
        ParamKind::StringList => Value::Array(
            raw.split(',')
                .map(str::trim)
                .filter(|s| !s.is_empty())
                .map(|s| Value::String(s.to_string()))
                .collect(),
        ),
        _ => parse_scalar(&raw),
    };
    Ok(value)
}

#[allow(clippy::too_many_arguments)]
fn cmd_invoke(
    target: &str,
    action: &str,
    params: &[String],
    action_target: Option<&str>,
    target_id: Option<&str>,
    yes: bool,
    interactive: bool,
    json: bool,
) -> Result<(), Fail> {
    let mut params_map = parse_params(params)?;
    let action_target_json = match (action_target, target_id) {
        (Some(_), Some(_)) => {
            return Err(Fail::usage(
                "--target and --target-id are mutually exclusive",
            ))
        }
        (Some(raw), None) => Some(parse_scalar(raw)),
        (None, Some(id)) => Some(Value::String(id.to_string())),
        (None, None) => None,
    };
    let mut host = Host::discover();
    cmd_invoke_host(
        &mut host,
        target,
        action,
        &mut params_map,
        action_target_json,
        yes,
        interactive,
        json,
    )
}

/// [`cmd_invoke`] over an explicit host — split for tests.
#[allow(clippy::too_many_arguments)]
fn cmd_invoke_host(
    host: &mut Host,
    target: &str,
    action: &str,
    params: &mut Params,
    action_target: Option<Value>,
    yes: bool,
    interactive: bool,
    json: bool,
) -> Result<(), Fail> {
    // `/route?query` deep links → `<driver>:<view>`; route-derived
    // params are *view* params (they feed the view resolve inside
    // `action_rows`/`invoke_ctx`), never action params.
    let (target, view_params) = resolve_deep_link(host, target, Params::new())?;
    let target = target.as_str();

    // Locate the declaration (danger + param metadata + route).
    // Best-effort: undeclared actions still dispatch — the driver is
    // authoritative (ops-derived rows cover zero-declaration
    // drivers). The row is kept whole: item scope feeds `{item.*}`
    // interpolation for route templates.
    let declared_row = action_rows(host, target, view_params.clone(), false)
        .ok()
        .and_then(|rows| rows.into_iter().find(|r| r.action.id == action));
    let declared = declared_row.as_ref().map(|r| r.action.clone());
    let item_id = declared_row.as_ref().and_then(|r| r.item_id.as_deref());

    if let Some(a) = &declared {
        check_danger(action, a.danger, action_target.as_ref(), yes)?;
        // Fill declared defaults for absent params, then prompt for
        // the rest under --interactive.
        for p in &a.params {
            if !params.contains_key(&p.name) {
                if let Some(d) = &p.default {
                    params.insert(p.name.clone(), d.clone());
                }
            }
        }
        if interactive {
            if !std::io::stdin().is_terminal() {
                return Err(Fail::usage(
                    "--interactive needs a TTY — pass --param k=v instead",
                ));
            }
            for p in &a.params {
                let missing = match params.get(&p.name) {
                    None | Some(Value::Null) => true,
                    Some(Value::String(s)) => s.is_empty(),
                    _ => false,
                };
                if missing {
                    let v = prompt_param(p)?;
                    if !v.is_null() {
                        params.insert(p.name.clone(), v);
                    }
                }
            }
        }
        // Client-side validation (required/options/bounds) — drivers
        // stay authoritative via ActionResult.errors.
        let errors = validate_params(a, params);
        if !errors.is_empty() {
            let detail = errors
                .iter()
                .map(|(k, v)| format!("{k}: {}", v.as_str().unwrap_or("invalid")))
                .collect::<Vec<_>>()
                .join("; ");
            return Err(Fail {
                code: 1,
                msg: format!("invalid params for '{action}' — {detail}"),
            });
        }
    } else if interactive {
        return Err(Fail::usage(format!(
            "--interactive needs a declared action — '{action}' has no \
             ParamSchema (try `pavo actions {target}`)"
        )));
    }

    // Route actions navigate instead of dispatching — danger and
    // params still applied host-side above (a form may collect
    // params the template consumes), but no `invoke` wire op is
    // sent. The interpolated route is the whole output.
    if let Some(template) = declared.as_ref().and_then(|a| a.route.clone()) {
        let ctx = invoke_ctx(
            host,
            target,
            &view_params,
            item_id,
            action_target.as_ref(),
            params,
        );
        let route = interpolate(&template, &ctx);
        if json {
            println!(
                "{}",
                serde_json::to_string_pretty(&json!({ "route": route }))
                    .map_err(|e| Fail::err(e.to_string()))?
            );
        } else {
            println!("{route}");
        }
        return Ok(());
    }

    // `action_target` is cloned — `ActionResult.route` interpolation
    // below still needs it.
    let result = host.invoke(target, action, None, action_target.clone(), params.clone())?;
    if json {
        println!(
            "{}",
            serde_json::to_string_pretty(&result).map_err(|e| Fail::err(e.to_string()))?
        );
    } else {
        if let Some(message) = &result.message {
            println!("{message}");
        }
        match &result.data {
            Some(data) => println!(
                "{}",
                serde_json::to_string_pretty(data).map_err(|e| Fail::err(e.to_string()))?
            ),
            None if result.message.is_none() && result.errors.is_empty() => {
                println!("(empty result)");
            }
            None => {}
        }
        // Driver-requested redirect — interpolate the route template
        // over the invoke context and hint the destination. `--json`
        // carries the raw template in `result.route` already.
        if let Some(template) = &result.route {
            let ctx = invoke_ctx(
                host,
                target,
                &view_params,
                item_id,
                action_target.as_ref(),
                params,
            );
            println!("→ {}", interpolate(template, &ctx));
        }
    }
    // Per-param errors mean the driver *rejected* the invoke (the
    // wire still answered ok:true) — same convention as submit.
    if !result.errors.is_empty() {
        let detail = result
            .errors
            .iter()
            .map(|(k, v)| format!("{k}: {}", v.as_str().unwrap_or("invalid")))
            .collect::<Vec<_>>()
            .join("; ");
        return Err(Fail {
            code: 1,
            msg: format!("invoke rejected — {detail}"),
        });
    }
    Ok(())
}

fn cmd_jobs(json: bool) -> Result<(), Fail> {
    let host = Host::discover();
    cmd_jobs_host(&host, json)
}

/// [`cmd_jobs`] over an explicit host — split for tests.
fn cmd_jobs_host(host: &Host, json: bool) -> Result<(), Fail> {
    let entries = host.jobs();
    if json {
        let rows = feed_rows(&entries).map_err(|e| Fail::err(e.to_string()))?;
        println!(
            "{}",
            serde_json::to_string_pretty(&rows).map_err(|e| Fail::err(e.to_string()))?
        );
        return Ok(());
    }
    if entries.is_empty() {
        println!("no jobs");
        return Ok(());
    }
    for e in &entries {
        let j = &e.item;
        let mut line = format!("{:<24} {:<28} {}", j.id, j.title, j.status);
        if let Some(detail) = &j.detail {
            line.push_str(&format!("  — {detail}"));
        }
        line.push_str(&format!("  ({})", e.driver));
        println!("{line}");
    }
    Ok(())
}

fn cmd_notifications(json: bool) -> Result<(), Fail> {
    let host = Host::discover();
    cmd_notifications_host(&host, json)
}

/// [`cmd_notifications`] over an explicit host — split for tests.
fn cmd_notifications_host(host: &Host, json: bool) -> Result<(), Fail> {
    let entries = host.notifications();
    if json {
        let rows = feed_rows(&entries).map_err(|e| Fail::err(e.to_string()))?;
        println!(
            "{}",
            serde_json::to_string_pretty(&rows).map_err(|e| Fail::err(e.to_string()))?
        );
        return Ok(());
    }
    if entries.is_empty() {
        println!("no notifications");
        return Ok(());
    }
    for e in &entries {
        let n = &e.item;
        let severity = n.severity.as_deref().unwrap_or("info");
        let state = if n.read { "read" } else { "unread" };
        println!(
            "{:<24} {:<28} {:<8} {:<6} [{}]",
            n.id, n.title, severity, state, e.driver
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

#[cfg(test)]
#[cfg_attr(coverage, coverage(off))]
mod tests {
    #![allow(clippy::unwrap_used)]
    //! Unit tests for the CLI's pure helpers and the verb bodies
    //! over an injected fixture host (same sh-driver trick as the
    //! serve tests — no real tamias drivers needed).

    use super::*;
    use pavo_core::discovery::DriverInfo;

    /// Minimal sh fixture driver (`demo`) answering manifest + serve.
    const DEMO_DRIVER: &str = r#"#!/bin/sh
if [ "$1" = manifest ]; then
  printf '%s\n' '{"name":"demo","title":"Demo","version":"1.0.0","verbs":["manifest","serve"],"capabilities":["pavo"],"views":[{"id":"board","title":"Board","route":"/demo/board","queries":[{"name":"items","op":"query"}],"root":{"kind":"kv","pairs":[{"key":"k","value":"v"}]}},{"id":"item","title":"Item","route":"/demo/item","item_route":"/demo/item/{item.id}","root":{"kind":"kv","pairs":[]}}],"actions":[{"id":"refresh","label":"Refresh"},{"id":"rm","label":"Remove","danger":"confirm"},{"id":"wipe","label":"Wipe","danger":"type-target","target":"item"},{"id":"move","label":"Move","params":[{"name":"id","kind":"string","required":true},{"name":"pri","kind":"enum","options":["low","high"],"default":"low"}]},{"id":"open","label":"Open item","route":"/demo/item?id={target.id}"},{"id":"go","label":"Go next"}],"contributes":{"navbar":[{"id":"nav","title":"Nav","route":"/demo/board"}],"extra":{"sidebar":[{"id":"side","title":"Side","route":"/x"}]},"jobs":[{"id":"job","title":"Jobs","route":"/demo/jobs"}],"notifications":[{"id":"ntf","title":"Notify","route":"/demo/ntf"}]}}'
  exit 0
fi
if [ "$1" = serve ]; then
  while IFS= read -r line; do
    case "$line" in
      *'"op":"views"'*) printf '%s\n' '{"ok":true,"result":[{"id":"board","title":"Board","route":"/demo/board","queries":["items"]},{"id":"item","title":"Item","route":"/demo/item","queries":[]}]}' ;;
      *'"view":"item"'*) printf '%s\n' '{"ok":true,"result":{"view":{"id":"item","title":"Item","route":"/demo/item","root":{"kind":"kv","pairs":[]}},"data":{}}}' ;;
      *'"op":"view"'*) printf '%s\n' '{"ok":true,"result":{"view":{"id":"board","title":"Board","route":"/demo/board","root":{"kind":"kv","pairs":[{"key":"k","value":"v"}]},"queries":[{"name":"items","op":"query"}]},"data":{"items":[1,2]}}}' ;;
      *'"query":"ops"'*) printf '%s\n' '{"ok":true,"result":{"query":"ops","data":{"engine":"demo","ops":[{"name":"cli.ping","doc":"Ping the engine","params":[{"name":"verbose","kind":"flag"}]}]}}}' ;;
      *'"op":"query"'*) printf '%s\n' '{"ok":true,"result":{"query":"items","data":[1,2]}}' ;;
      *'"op":"submit"'*) printf '%s\n' '{"ok":true,"result":{"message":"saved","data":{"applied":1}}}' ;;
      *'"action":"go"'*) printf '%s\n' '{"ok":true,"result":{"message":"moved","route":"/demo/item?id={target.id}"}}' ;;
      *'"op":"invoke"'*) printf '%s\n' '{"ok":true,"result":{"message":"invoked","data":{"applied":1}}}' ;;
      *'"op":"jobs"'*) printf '%s\n' '{"ok":true,"result":[{"id":"j1","title":"Build","status":"running"}]}' ;;
      *'"op":"notifications"'*) printf '%s\n' '{"ok":true,"result":[{"id":"n1","title":"Heads up","severity":"info"}]}' ;;
      *'"op":"manifest"'*) printf '%s\n' '{"ok":true,"result":{"name":"demo","title":"Demo","version":"1.0.0","verbs":["serve"],"capabilities":["pavo"],"views":[{"id":"board","title":"Board","route":"/demo/board","root":{"kind":"kv","pairs":[]}},{"id":"item","title":"Item","route":"/demo/item","item_route":"/demo/item/{item.id}","root":{"kind":"kv","pairs":[]}}],"actions":[{"id":"refresh","label":"Refresh"},{"id":"rm","label":"Remove","danger":"confirm"},{"id":"wipe","label":"Wipe","danger":"type-target","target":"item"},{"id":"move","label":"Move","params":[{"name":"id","kind":"string","required":true},{"name":"pri","kind":"enum","options":["low","high"],"default":"low"}]},{"id":"open","label":"Open item","route":"/demo/item?id={target.id}"},{"id":"go","label":"Go next"}]}}' ;;
    esac
  done
  exit 0
fi
exit 1
"#;

    /// Host over the fixture driver (manifest lazy via `serve`).
    fn fixture_host() -> Host {
        let tmp = tempfile::tempdir().ok().unwrap();
        let bin = tmp.path().join("bin");
        std::fs::create_dir_all(&bin).ok().unwrap();
        let driver = bin.join("tamias-driver-demo");
        std::fs::write(&driver, DEMO_DRIVER).ok().unwrap();
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            std::fs::set_permissions(&driver, std::fs::Permissions::from_mode(0o755))
                .ok()
                .unwrap();
        }
        std::mem::forget(tmp); // keep the tempdir alive for the test
        Host::from_drivers(vec![DriverInfo {
            kind: "demo".to_string(),
            path: driver,
            manifest: None,
        }])
    }

    fn empty_host() -> Host {
        Host::from_drivers(Vec::new())
    }

    #[test]
    fn parse_params_covers_all_forms() {
        let args: Vec<String> = vec![
            "a=1".into(),
            "b=true".into(),
            "c={\"x\":2}".into(),
            "d=hello world".into(),
            "e=".into(),
        ];
        let p = parse_params(&args).ok().unwrap();
        assert_eq!(p["a"], json!(1));
        assert_eq!(p["b"], json!(true));
        assert_eq!(p["c"], json!({"x": 2}));
        assert_eq!(p["d"], json!("hello world"));
        assert_eq!(p["e"], json!(""));

        assert!(parse_params(&["noequals".to_string()]).is_err());
        assert!(parse_params(&["=v".to_string()]).is_err());
        assert!(parse_params(&[]).ok().unwrap().is_empty());
    }

    #[test]
    fn contrib_counts_and_schema_doc() {
        let c = pavo_protocol::Contributes::default();
        let v = contrib_counts(&c);
        assert_eq!(v["navbar"], json!(0));
        assert!(v["extra"].is_object());

        let doc = schema_doc();
        assert!(doc["verbs"]["view"].is_object());
        assert!(doc["exit_codes"].is_object());
    }

    #[test]
    fn tui_candidates_ends_with_path_fallback() {
        let c = tui_candidates();
        assert!(!c.is_empty());
        assert_eq!(c.last().unwrap().to_str().unwrap(), "pavo-tui");
    }

    #[test]
    fn run_dispatch_self_description_verbs() {
        // No-command → version line; manifest/schema print JSON.
        // (These don't touch the host.)
        let cli = Cli::parse_from(["pavo"]);
        assert!(run(cli).is_ok());
        let cli = Cli::parse_from(["pavo", "manifest"]);
        assert!(run(cli).is_ok());
        let cli = Cli::parse_from(["pavo", "schema"]);
        assert!(run(cli).is_ok());
    }

    #[test]
    fn drivers_views_chrome_text_and_json() {
        let mut host = fixture_host();
        cmd_drivers_host(&mut host, false).ok().unwrap();
        cmd_drivers_host(&mut host, true).ok().unwrap();
        cmd_views_host(&mut host, None, false).ok().unwrap();
        cmd_views_host(&mut host, None, true).ok().unwrap();
        cmd_views_host(&mut host, Some("demo"), false).ok().unwrap();
        cmd_views_host(&mut host, Some("ghost"), true).ok().unwrap();
        cmd_chrome_host(&mut host, false).ok().unwrap();
        cmd_chrome_host(&mut host, true).ok().unwrap();
        cmd_jobs_host(&host, false).ok().unwrap();
        cmd_jobs_host(&host, true).ok().unwrap();
        cmd_notifications_host(&host, false).ok().unwrap();
        cmd_notifications_host(&host, true).ok().unwrap();
    }

    #[test]
    fn empty_host_prints_empty_states() {
        let mut host = empty_host();
        cmd_drivers_host(&mut host, false).ok().unwrap();
        cmd_drivers_host(&mut host, true).ok().unwrap();
        cmd_views_host(&mut host, None, false).ok().unwrap();
        cmd_chrome_host(&mut host, false).ok().unwrap();
        cmd_jobs_host(&host, false).ok().unwrap();
        cmd_notifications_host(&host, false).ok().unwrap();
        // Errors propagate as Fail.
        assert!(cmd_views_host(&mut host, Some("x"), false).is_ok()); // empty → "no views"
        assert!(cmd_view_host(&mut host, "ghost:v", Params::new(), "text").is_err());
        assert!(cmd_query_host(&mut host, "ghost", "q", Params::new(), false).is_err());
    }

    #[test]
    fn view_and_query_formats() {
        let mut host = fixture_host();
        for fmt in ["text", "json"] {
            cmd_view_host(&mut host, "demo:board", Params::new(), fmt)
                .ok()
                .unwrap();
        }
        // dot/mermaid on a graph-less tree → error.
        assert!(cmd_view_host(&mut host, "demo:board", Params::new(), "dot").is_err());
        assert!(cmd_view_host(&mut host, "demo:board", Params::new(), "mermaid").is_err());
        // Unknown plugin format → error naming builtins.
        let err = cmd_view_host(&mut host, "demo:board", Params::new(), "bogus")
            .err()
            .unwrap();
        assert!(err.msg.contains("no renderer plugin"), "{}", err.msg);
        cmd_query_host(&mut host, "demo", "items", Params::new(), false)
            .ok()
            .unwrap();
        cmd_query_host(&mut host, "demo", "items", Params::new(), true)
            .ok()
            .unwrap();
        // Bad view ref → error.
        assert!(cmd_view_host(&mut host, "badref", Params::new(), "text").is_err());
    }

    /// View driver `dv` — one `graph` node (the plugin target).
    const DV_DRIVER: &str = r#"#!/bin/sh
if [ "$1" = manifest ]; then
  printf '%s\n' '{"name":"dv","title":"Dv","version":"0.1.0","verbs":["manifest","serve"],"capabilities":["pavo"],"contributes":{},"views":[{"id":"g","title":"G","route":"/dv/g","root":{"kind":"graph","nodes":[{"id":"a","label":"A"}],"edges":[]}}]}'
  exit 0
fi
if [ "$1" = serve ]; then
  while IFS= read -r line; do
    case "$line" in
      *'"op":"view"'*) printf '%s\n' '{"ok":true,"result":{"view":{"id":"g","title":"G","route":"/dv/g","root":{"kind":"graph","nodes":[{"id":"a","label":"A"}],"edges":[]}},"data":{}}}' ;;
      *'"op":"views"'*) printf '%s\n' '{"ok":true,"result":[{"id":"g","title":"G","route":"/dv/g"}]}' ;;
      *'"op":"manifest"'*) printf '%s\n' '{"ok":true,"result":{"name":"dv","title":"Dv","version":"0.1.0","verbs":["serve"],"capabilities":["pavo"],"views":[{"id":"g","title":"G","route":"/dv/g","root":{"kind":"graph","nodes":[],"edges":[]}}]}}' ;;
    esac
  done
  exit 0
fi
exit 1
"#;

    /// Renderer plugin `vz` — claims `graph` on cli formats
    /// `dot`+`mermaid` (the built-in names); `render` answers a text
    /// RenderModel so tests can tell plugin output from built-in.
    const VZ_DRIVER: &str = r#"#!/bin/sh
if [ "$1" = manifest ]; then
  printf '%s\n' '{"name":"vz","title":"Vz","version":"0.1.0","verbs":["manifest","serve"],"capabilities":["pavo"],"contributes":{},"views":[],"renderers":[{"id":"graph","kinds":["graph"],"priority":50,"surfaces":{"cli":{"formats":["dot","mermaid"]}}}]}'
  exit 0
fi
if [ "$1" = serve ]; then
  while IFS= read -r line; do
    case "$line" in
      *'"op":"render"'*) printf '%s\n' '{"ok":true,"result":{"type":"text","lines":["PLUGIN-RENDER"]}}' ;;
      *'"op":"views"'*) printf '%s\n' '{"ok":true,"result":[]}' ;;
      *'"op":"manifest"'*) printf '%s\n' '{"ok":true,"result":{"name":"vz","title":"Vz","version":"0.1.0","verbs":["serve"],"capabilities":["pavo"],"views":[],"renderers":[{"id":"graph","kinds":["graph"],"priority":50,"surfaces":{"cli":{"formats":["dot","mermaid"]}}}]}}' ;;
    esac
  done
  exit 0
fi
exit 1
"#;

    /// Write `tamias-driver-<name>` into `bin` and return its path.
    fn write_driver(bin: &std::path::Path, name: &str, script: &str) -> std::path::PathBuf {
        std::fs::create_dir_all(bin).ok().unwrap();
        let path = bin.join(format!("tamias-driver-{name}"));
        std::fs::write(&path, script).ok().unwrap();
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            std::fs::set_permissions(&path, std::fs::Permissions::from_mode(0o755))
                .ok()
                .unwrap();
        }
        path
    }

    /// Host over `dv` (+ optional `vz` plugin), manifests lazy.
    fn plugin_host(with_plugin: bool) -> (tempfile::TempDir, Host) {
        let tmp = tempfile::tempdir().ok().unwrap();
        let bin = tmp.path().join("bin");
        let dv = write_driver(&bin, "dv", DV_DRIVER);
        let mut drivers = vec![DriverInfo {
            kind: "dv".to_string(),
            path: dv,
            manifest: None,
        }];
        if with_plugin {
            let vz = write_driver(&bin, "vz", VZ_DRIVER);
            drivers.push(DriverInfo {
                kind: "vz".to_string(),
                path: vz,
                manifest: None,
            });
        }
        (tmp, Host::from_drivers(drivers))
    }

    #[test]
    fn plugin_format_beats_builtin_dot_and_mermaid() {
        let (_tmp, host) = plugin_host(true);
        let resolved = host.resolve_view("dv:g", Params::new()).ok().unwrap();
        // The plugin claims cli formats "dot"/"mermaid" for kind
        // "graph" — its RenderModel wins over the built-in renderers.
        for fmt in ["dot", "mermaid"] {
            let out = view_output(&host, &resolved, "dv:g", fmt).ok().unwrap();
            assert!(out.contains("PLUGIN-RENDER"), "{fmt}: {out}");
            assert!(!out.contains("digraph"), "{fmt}: {out}");
            assert!(!out.contains("graph TD"), "{fmt}: {out}");
        }
        // json stays host-owned: raw ViewResult, not plugin output.
        let out = view_output(&host, &resolved, "dv:g", "json").ok().unwrap();
        assert!(out.contains("\"root\""), "{out}");
        assert!(!out.contains("PLUGIN-RENDER"), "{out}");
        // Formats the plugin doesn't claim still error / built-in.
        assert!(view_output(&host, &resolved, "dv:g", "bogus").is_err());
    }

    #[test]
    fn builtin_dot_mermaid_without_plugin_claims() {
        let (_tmp, host) = plugin_host(false);
        let resolved = host.resolve_view("dv:g", Params::new()).ok().unwrap();
        let out = view_output(&host, &resolved, "dv:g", "dot").ok().unwrap();
        assert!(out.contains("digraph"), "{out}");
        let out = view_output(&host, &resolved, "dv:g", "mermaid")
            .ok()
            .unwrap();
        assert!(out.contains("graph TD"), "{out}");
    }

    #[test]
    fn submit_text_and_json() {
        let mut host = fixture_host();
        let mut values = Params::new();
        values.insert("k".to_string(), json!("v"));
        cmd_submit_host(&mut host, "demo:board", None, values.clone(), false)
            .ok()
            .unwrap();
        cmd_submit_host(&mut host, "demo:board", Some("f1"), values, true)
            .ok()
            .unwrap();
        // Unknown driver/view → error.
        assert!(cmd_submit_host(&mut host, "ghost:board", None, Params::new(), false).is_err());
        assert!(cmd_submit_host(&mut host, "demo:nope", None, Params::new(), false).is_err());
    }

    #[test]
    fn actions_lists_declared_and_ops_derived() {
        let mut host = fixture_host();
        // View scope: manifest actions + ops-derived "ping".
        cmd_actions_host(&mut host, "demo:board", Params::new(), false)
            .ok()
            .unwrap();
        cmd_actions_host(&mut host, "demo:board", Params::new(), true)
            .ok()
            .unwrap();
        // Bare driver = manifest scope (+ ops fallback).
        cmd_actions_host(&mut host, "demo", Params::new(), false)
            .ok()
            .unwrap();
        cmd_actions_host(&mut host, "demo", Params::new(), true)
            .ok()
            .unwrap();
        // Empty host → "no actions".
        let mut empty = empty_host();
        assert!(cmd_actions_host(&mut empty, "demo", Params::new(), false).is_err());
        // Unknown driver / unresolvable view → error.
        assert!(cmd_actions_host(&mut host, "ghost:board", Params::new(), false).is_err());
        assert!(cmd_actions_host(&mut host, "demo:nope", Params::new(), false).is_err());
    }

    #[test]
    fn invoke_dispatches_and_gates_danger() {
        let mut host = fixture_host();
        // danger:none dispatches without --yes (tests are non-TTY).
        cmd_invoke_host(
            &mut host,
            "demo:board",
            "refresh",
            &mut Params::new(),
            None,
            false,
            false,
            false,
        )
        .ok()
        .unwrap();
        // --json dumps the ActionResult.
        cmd_invoke_host(
            &mut host,
            "demo:board",
            "refresh",
            &mut Params::new(),
            None,
            false,
            false,
            true,
        )
        .ok()
        .unwrap();
        // confirm: non-TTY without --yes → usage error; --yes → ok.
        assert!(cmd_invoke_host(
            &mut host,
            "demo:board",
            "rm",
            &mut Params::new(),
            None,
            false,
            false,
            false
        )
        .is_err());
        cmd_invoke_host(
            &mut host,
            "demo:board",
            "rm",
            &mut Params::new(),
            None,
            true,
            false,
            false,
        )
        .ok()
        .unwrap();
        // type-target: --target-id (or a JSON string --target) is
        // required even with --yes.
        assert!(cmd_invoke_host(
            &mut host,
            "demo:board",
            "wipe",
            &mut Params::new(),
            None,
            true,
            false,
            false
        )
        .is_err());
        cmd_invoke_host(
            &mut host,
            "demo:board",
            "wipe",
            &mut Params::new(),
            Some(json!("i1")),
            true,
            false,
            false,
        )
        .ok()
        .unwrap();
        // --target + --target-id conflict → usage error (pre-host).
        assert!(cmd_invoke(
            "demo:board",
            "wipe",
            &[],
            Some("{}"),
            Some("i2"),
            true,
            false,
            false,
        )
        .is_err());
        // ParamSchema validation: required param missing → exit 1
        // before dispatch; defaults fill in; supplied params pass.
        let err = cmd_invoke_host(
            &mut host,
            "demo:board",
            "move",
            &mut Params::new(),
            None,
            false,
            false,
            false,
        )
        .err()
        .unwrap();
        assert!(err.msg.contains("invalid params"), "{}", err.msg);
        let mut p = Params::new();
        p.insert("id".to_string(), json!("T-1"));
        cmd_invoke_host(
            &mut host,
            "demo:board",
            "move",
            &mut p,
            None,
            false,
            false,
            false,
        )
        .ok()
        .unwrap();
        // --interactive off-TTY → usage error.
        assert!(cmd_invoke_host(
            &mut host,
            "demo:board",
            "move",
            &mut Params::new(),
            None,
            true,
            true,
            false
        )
        .is_err());
        // Undeclared action still dispatches (driver is authoritative).
        cmd_invoke_host(
            &mut host,
            "demo:board",
            "whatever",
            &mut Params::new(),
            None,
            false,
            false,
            true,
        )
        .ok()
        .unwrap();
        // Malformed ref → error.
        assert!(cmd_invoke_host(
            &mut host,
            "a:b:c",
            "x",
            &mut Params::new(),
            None,
            false,
            false,
            false
        )
        .is_err());
    }

    #[test]
    fn route_target_resolves_declared_routes() {
        let host = fixture_host();
        // Exact view route + query params (param_json scalar rule).
        let (r, p) = route_target(&host, "/demo/item?id=T-1").unwrap();
        assert_eq!(r, "demo:item");
        assert_eq!(p["id"], json!("T-1"));
        let (r, p) = route_target(&host, "/demo/item?n=5&ok=true").unwrap();
        assert_eq!(r, "demo:item");
        assert_eq!(p["n"], json!(5));
        assert_eq!(p["ok"], json!(true));
        // item_route path template captures {item.id} as "item.id".
        let (r, p) = route_target(&host, "/demo/item/T-9").unwrap();
        assert_eq!(r, "demo:item");
        assert_eq!(p["item.id"], json!("T-9"));
        // Bare route, no params.
        let (r, p) = route_target(&host, "/demo/board").unwrap();
        assert_eq!(r, "demo:board");
        assert!(p.is_empty());
        // Non-routes and unknown routes → None.
        assert!(route_target(&host, "demo:board").is_none());
        assert!(route_target(&host, "/nope/x").is_none());
        assert!(route_target(&host, "/").is_none());
        // Trailing slash tolerated.
        assert_eq!(route_target(&host, "/demo/board/").unwrap().0, "demo:board");
    }

    #[test]
    fn deep_link_normalization() {
        let host = fixture_host();
        // Non-route targets pass through with params intact.
        let mut p = Params::new();
        p.insert("a".into(), json!(1));
        let (t, params) = resolve_deep_link(&host, "demo:board", p).ok().unwrap();
        assert_eq!(t, "demo:board");
        assert_eq!(params["a"], json!(1));
        // Route → ref; explicit params override route-derived ones.
        let mut p = Params::new();
        p.insert("id".into(), json!("override"));
        let (t, params) = resolve_deep_link(&host, "/demo/item?id=T-1", p)
            .ok()
            .unwrap();
        assert_eq!(t, "demo:item");
        assert_eq!(params["id"], json!("override"));
        // Unknown route → error (leading '/' is unambiguously a route).
        assert!(resolve_deep_link(&host, "/nope", Params::new()).is_err());
    }

    #[test]
    fn view_and_actions_accept_route_deep_links() {
        let mut host = fixture_host();
        for target in ["/demo/item?id=T-1", "/demo/item/T-7", "/demo/board"] {
            for fmt in ["text", "json"] {
                cmd_view_host(&mut host, target, Params::new(), fmt)
                    .ok()
                    .unwrap();
            }
        }
        cmd_actions_host(&mut host, "/demo/item?id=T-1", Params::new(), false)
            .ok()
            .unwrap();
        cmd_actions_host(&mut host, "/demo/item?id=T-1", Params::new(), true)
            .ok()
            .unwrap();
        cmd_submit_host(&mut host, "/demo/board", None, Params::new(), false)
            .ok()
            .unwrap();
        assert!(cmd_view_host(&mut host, "/nope", Params::new(), "text").is_err());
        assert!(cmd_actions_host(&mut host, "/nope", Params::new(), false).is_err());
    }

    #[test]
    fn invoke_route_action_and_result_route() {
        let mut host = fixture_host();
        // Action.route → interpolated route printed, no wire invoke
        // (text + --json). {target.id} resolves over --target.
        cmd_invoke_host(
            &mut host,
            "demo:board",
            "open",
            &mut Params::new(),
            Some(json!({ "id": "T-1" })),
            false,
            false,
            false,
        )
        .ok()
        .unwrap();
        cmd_invoke_host(
            &mut host,
            "demo:board",
            "open",
            &mut Params::new(),
            Some(json!({ "id": "T-1" })),
            false,
            false,
            true,
        )
        .ok()
        .unwrap();
        // Route-shaped invoke target works too.
        cmd_invoke_host(
            &mut host,
            "/demo/board",
            "open",
            &mut Params::new(),
            Some(json!({ "id": "T-2" })),
            false,
            false,
            false,
        )
        .ok()
        .unwrap();
        // ActionResult.route → real dispatch + `→ route` text hint.
        cmd_invoke_host(
            &mut host,
            "demo:board",
            "go",
            &mut Params::new(),
            Some(json!({ "id": "T-3" })),
            false,
            false,
            false,
        )
        .ok()
        .unwrap();
        cmd_invoke_host(
            &mut host,
            "demo:board",
            "go",
            &mut Params::new(),
            Some(json!({ "id": "T-3" })),
            false,
            false,
            true,
        )
        .ok()
        .unwrap();
    }
}
