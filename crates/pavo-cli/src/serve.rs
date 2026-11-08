//! `pavo serve` — the localhost HTTP bridge: pavo-core's [`Host`] on
//! one side, the Nuxt web app (or any JSON client) on the other.
//!
//! Hand-rolled HTTP/1.1 over [`std::net::TcpListener`], thread-per-
//! connection, std-only (see [`crate::http`] for the wire bits). The
//! [`Host`] is shared across threads behind a mutex — its methods need
//! `&mut` (lazy manifest fetches, client spawns) and driver round-trips
//! are serialized anyway by each driver's single `serve` process.
//!
//! Routes:
//!
//! - `GET /api/health` → `{ok:true}`
//! - `GET /api/drivers` → per-driver manifest summary
//! - `GET /api/views` → every `<driver>:<view>` entry
//! - `GET /api/chrome` → merged chrome slots
//! - `GET /api/view?ref=<driver>:<view>` (+ `param.<k>=<v>`) → ViewResult
//! - `GET /api/raw?ref=…` → same as `/api/view`
//! - `GET /api/query?driver=<d>&query=<q>` (+ `param.<k>=<v>`) → QueryResult
//! - `GET /api/renderers` → the renderer-plugin registry (kind claims)
//! - `GET /api/render?kind=<k>&node=<json>&data=<json>&surface=<s>[&format=<f>]`
//!   → RenderModel — routes to the claiming driver's `render` op
//! - `GET /*` → static files from the Nuxt output (`web/.output/public`,
//!   `web/dist`, or `$PAVO_WEB_DIR`), else a built-in landing page.
//!
//! All `/api/*` responses use the `{"ok":…,"result"/"error"}` envelope
//! with `Access-Control-Allow-Origin: *` — this is a localhost dev
//! tool, not a hardened server.

use std::io;
use std::net::TcpListener;
use std::path::{Path, PathBuf};
use std::sync::{Arc, Mutex};

use pavo_core::{Host, Params, PavoError};
use serde_json::{json, Value};

use crate::http::{self, ReadError, Request, Response};

/// Run the bridge until killed. Binds `127.0.0.1:port`; port `0`
/// picks a free port. The first stdout line is always
/// `listening http://127.0.0.1:<port>` so wrappers/tests can learn it.
///
/// # Errors
/// [`io::Error`] if the listener can't bind.
pub fn run(port: u16) -> io::Result<()> {
    let listener = TcpListener::bind(("127.0.0.1", port))?;
    let bound = listener.local_addr()?.port();
    println!("listening http://127.0.0.1:{bound}");
    use std::io::Write;
    let _ = std::io::stdout().flush();

    let host = Arc::new(Mutex::new(Host::discover()));
    let web_root = find_web_root();

    for stream in listener.incoming() {
        match stream {
            Ok(mut stream) => {
                let host = Arc::clone(&host);
                let root = web_root.clone();
                std::thread::spawn(move || {
                    let _ = handle_connection(&mut stream, &host, root.as_deref());
                });
            }
            Err(e) => {
                eprintln!("pavo serve: accept error: {e}");
            }
        }
    }
    Ok(())
}

/// Serve one connection (keep-alive loop). Errors are logged, never
/// fatal — one bad client can't take the bridge down.
fn handle_connection(
    stream: &mut std::net::TcpStream,
    host: &Arc<Mutex<Host>>,
    web_root: Option<&Path>,
) -> io::Result<()> {
    loop {
        match http::read_request(stream) {
            Ok(None) => return Ok(()),
            Ok(Some(req)) => {
                let (response, close) = route(&req, host, web_root);
                let close = close || req.wants_close();
                let _ = response.write(stream, close);
                if close {
                    return Ok(());
                }
            }
            Err(ReadError::Http { status, message }) => {
                let resp = Response::raw(
                    status,
                    "text/plain; charset=utf-8",
                    message.into_bytes(),
                    false,
                );
                let _ = resp.write(stream, true);
                return Ok(());
            }
            Err(ReadError::Io(e)) => return Err(e),
        }
    }
}

/// Route one request → (response, close_after). `close_after` is set
/// for HEAD and for errors where keep-alive state is uncertain.
fn route(req: &Request, host: &Arc<Mutex<Host>>, web_root: Option<&Path>) -> (Response, bool) {
    let (path, query) = http::split_target(&req.target);
    let path = http::percent_decode(path);
    let pairs = http::parse_query(query);

    // HEAD behaves like GET with an empty body.
    let head_only = req.method.eq_ignore_ascii_case("HEAD");

    if !path.starts_with("/api/") {
        if req.method != "GET" && !head_only {
            return (api_error(405, "method not allowed", false), true);
        }
        let resp = static_response(&path, web_root);
        let resp = if head_only {
            Response {
                body: Vec::new(),
                ..resp
            }
        } else {
            resp
        };
        return (resp, false);
    }

    // /api/* — JSON envelope, CORS on.
    if req.method != "GET" && !head_only {
        let mut resp = api_error(405, "method not allowed", true);
        resp.extra
            .push(("allow".to_string(), "GET, HEAD".to_string()));
        return (resp, false);
    }

    let resp = api_route(path.as_str(), &pairs, host);
    let resp = if head_only {
        Response {
            body: Vec::new(),
            ..resp
        }
    } else {
        resp
    };
    (resp, false)
}

// ---------- /api routing ----------

fn api_route(path: &str, pairs: &[(String, String)], host: &Arc<Mutex<Host>>) -> Response {
    let endpoint = path.trim_start_matches("/api/");
    let mut guard = match host.lock() {
        Ok(g) => g,
        Err(_) => return api_error(500, "host lock poisoned", true),
    };
    let host = &mut *guard;

    let result: Result<Value, ApiFail> = match endpoint {
        "health" => Ok(json!({})),
        "drivers" => api_drivers(host),
        "views" => api_views(host),
        "chrome" => api_chrome(host),
        "view" | "raw" => api_view(host, pairs),
        "query" => api_query(host, pairs),
        "renderers" => api_renderers(host),
        "render" => api_render(host, pairs),
        _ => Err(ApiFail::new(404, format!("unknown endpoint '{endpoint}'"))),
    };

    match result {
        Ok(result) => Response::json(200, &json!({"ok": true, "result": result}), true),
        Err(f) => Response::json(f.status, &json!({"ok": false, "error": f.message}), true),
    }
}

/// A routing-level failure with an HTTP status.
struct ApiFail {
    status: u16,
    message: String,
}

impl ApiFail {
    fn new(status: u16, message: impl Into<String>) -> Self {
        Self {
            status,
            message: message.into(),
        }
    }
}

impl From<PavoError> for ApiFail {
    fn from(e: PavoError) -> Self {
        let status = match &e {
            PavoError::BadViewRef(_) => 400,
            PavoError::UnknownDriver(_) | PavoError::UnknownView { .. } => 404,
            _ => 500,
        };
        Self::new(status, e.to_string())
    }
}

fn api_error(status: u16, message: &str, cors: bool) -> Response {
    Response::json(status, &json!({"ok": false, "error": message}), cors)
}

fn api_drivers(host: &mut Host) -> Result<Value, ApiFail> {
    let kinds: Vec<String> = host.drivers().iter().map(|d| d.kind.clone()).collect();
    let mut rows = Vec::new();
    for kind in kinds {
        match host.manifest(&kind) {
            Ok(m) => rows.push(json!({
                "driver": kind,
                "title": m.title,
                "version": m.version,
                "view_count": m.views.len(),
                "contributes": crate::contrib_counts(&m.contributes),
            })),
            Err(e) => rows.push(json!({
                "driver": kind,
                "manifest_error": e.to_string(),
            })),
        }
    }
    Ok(json!(rows))
}

fn api_views(host: &mut Host) -> Result<Value, ApiFail> {
    let views = host.list_views();
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
    Ok(json!(rows))
}

fn api_chrome(host: &mut Host) -> Result<Value, ApiFail> {
    let chrome = host.chrome();
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
    let extra: serde_json::Map<String, Value> = chrome
        .extra
        .iter()
        .map(|(k, v)| (k.clone(), Value::Array(slot(v))))
        .collect();
    Ok(json!({
        "navbar": slot(&chrome.navbar),
        "dashboard": slot(&chrome.dashboard),
        "statusbar": slot(&chrome.statusbar),
        "routes": slot(&chrome.routes),
        "extra": Value::Object(extra),
    }))
}

/// Resolve the `driver:view` target: `?ref=` wins, else `?driver=` +
/// `?view=` (bare driver = its first view, same as the CLI).
fn view_target(pairs: &[(String, String)]) -> Result<String, ApiFail> {
    if let Some(r) = http::query_get(pairs, "ref") {
        return Ok(r.to_string());
    }
    match (
        http::query_get(pairs, "driver"),
        http::query_get(pairs, "view"),
    ) {
        (Some(d), Some(v)) => Ok(format!("{d}:{v}")),
        (Some(d), None) => Ok(d.to_string()),
        _ => Err(ApiFail::new(
            400,
            "missing view ref — pass ?ref=<driver>:<view> or ?driver=&view=",
        )),
    }
}

fn api_view(host: &mut Host, pairs: &[(String, String)]) -> Result<Value, ApiFail> {
    let target = view_target(pairs)?;
    let params: Params = http::collect_params(pairs).into_iter().collect();
    let resolved = host.resolve_view(&target, params)?;
    serde_json::to_value(&resolved.result).map_err(|e| ApiFail::new(500, e.to_string()))
}

fn api_query(host: &mut Host, pairs: &[(String, String)]) -> Result<Value, ApiFail> {
    let driver = http::query_get(pairs, "driver")
        .ok_or_else(|| ApiFail::new(400, "missing 'driver' param"))?;
    let query = http::query_get(pairs, "query")
        .ok_or_else(|| ApiFail::new(400, "missing 'query' param"))?;
    let params: Params = http::collect_params(pairs).into_iter().collect();
    let result = host.query(driver, query, params)?;
    serde_json::to_value(&result).map_err(|e| ApiFail::new(500, e.to_string()))
}

/// The renderer-plugin registry — every `renderers` declaration,
/// resolved to driver bindings (sorted by namespaced id).
fn api_renderers(host: &mut Host) -> Result<Value, ApiFail> {
    let registry = host.renderers();
    let rows: Vec<Value> = registry
        .entries()
        .iter()
        .map(|b| {
            json!({
                "id": b.id,
                "driver": b.driver,
                "driver_title": b.driver_title,
                "kinds": b.decl.kinds,
                "surfaces": b.decl.surfaces,
                "priority": b.decl.priority,
                "entry": b.decl.entry,
            })
        })
        .collect();
    Ok(json!(rows))
}

/// `GET /api/render` — route one node to its claiming renderer
/// driver. `node`/`data` are URL-encoded JSON (`data` defaults to
/// `{}`); `surface` is required (`cli`/`tui`/`web`); `format` is the
/// optional format hint.
fn api_render(host: &mut Host, pairs: &[(String, String)]) -> Result<Value, ApiFail> {
    let kind =
        http::query_get(pairs, "kind").ok_or_else(|| ApiFail::new(400, "missing 'kind' param"))?;
    let surface = http::query_get(pairs, "surface")
        .ok_or_else(|| ApiFail::new(400, "missing 'surface' param"))?;
    let node_raw = http::query_get(pairs, "node").unwrap_or("{}");
    let node: Value = serde_json::from_str(node_raw)
        .map_err(|e| ApiFail::new(400, format!("'node' isn't valid JSON: {e}")))?;
    let data: Params = match http::query_get(pairs, "data") {
        Some(raw) => serde_json::from_str(raw)
            .map_err(|e| ApiFail::new(400, format!("'data' isn't a JSON object: {e}")))?,
        None => Params::new(),
    };
    let format = http::query_get(pairs, "format");
    let model = host.render_by_kind(kind, &node, &data, surface, format)?;
    serde_json::to_value(&model).map_err(|e| ApiFail::new(500, e.to_string()))
}

// ---------- static files + built-in fallback ----------

/// Locate the web root: `$PAVO_WEB_DIR`, then `web/.output/public` /
/// `web/dist` relative to the cwd, then the same under the repo root
/// (derived from the crate manifest dir — works from any cwd inside
/// a pavo checkout).
fn find_web_root() -> Option<PathBuf> {
    // Explicit override wins outright — set-but-missing means "no web
    // root" (tests use this to force the built-in landing page).
    if let Some(dir) = std::env::var_os("PAVO_WEB_DIR") {
        let p = PathBuf::from(dir);
        return p.is_dir().then_some(p);
    }
    let mut candidates: Vec<PathBuf> = Vec::new();
    for base in [
        PathBuf::from("."),
        Path::new(env!("CARGO_MANIFEST_DIR")).join("../.."),
    ] {
        candidates.push(base.join("web/.output/public"));
        candidates.push(base.join("web/dist"));
    }
    candidates.into_iter().find(|p| p.is_dir())
}

fn mime_for(path: &Path) -> &'static str {
    match path.extension().and_then(|e| e.to_str()) {
        Some("html") | Some("htm") => "text/html; charset=utf-8",
        Some("css") => "text/css; charset=utf-8",
        Some("js") | Some("mjs") => "text/javascript; charset=utf-8",
        Some("json") | Some("map") => "application/json; charset=utf-8",
        Some("svg") => "image/svg+xml",
        Some("png") => "image/png",
        Some("jpg") | Some("jpeg") => "image/jpeg",
        Some("gif") => "image/gif",
        Some("webp") => "image/webp",
        Some("ico") => "image/x-icon",
        Some("woff") => "font/woff",
        Some("woff2") => "font/woff2",
        Some("ttf") => "font/ttf",
        Some("wasm") => "application/wasm",
        Some("txt") | Some("md") => "text/plain; charset=utf-8",
        _ => "application/octet-stream",
    }
}

/// Resolve `path` under `root` safely — rejects `..` escapes and
/// non-UTF8 tricks. Returns the file path or None.
fn resolve_static(root: &Path, path: &str) -> Option<PathBuf> {
    let rel = path.trim_start_matches('/');
    if rel.split('/').any(|seg| seg == "..") {
        return None;
    }
    let candidate = root.join(rel);
    // Reject symlink escapes too — canonicalize both sides.
    let canon = candidate.canonicalize().ok()?;
    let root_canon = root.canonicalize().ok()?;
    if !canon.starts_with(&root_canon) {
        return None;
    }
    if canon.is_file() {
        return Some(canon);
    }
    if canon.is_dir() {
        let index = canon.join("index.html");
        if index.is_file() {
            return Some(index);
        }
    }
    None
}

/// `GET /*` handler: static file if a web root exists, else the
/// built-in landing page (and a minimal `/view/<ref>` page).
fn static_response(path: &str, web_root: Option<&Path>) -> Response {
    if let Some(root) = web_root {
        if let Some(file) = resolve_static(root, path) {
            let body = std::fs::read(&file).unwrap_or_default();
            return Response::raw(200, mime_for(&file), body, false);
        }
        // SPA-ish fallback: page-like paths (no extension) get the
        // Nuxt SPA shell — `200.html` when the static export emitted
        // one (its payload isn't pinned to a prerendered route, unlike
        // `index.html`), else `index.html`.
        let last = path.rsplit('/').next().unwrap_or("");
        if !last.contains('.') {
            for fallback in ["/200.html", "/index.html"] {
                if let Some(file) = resolve_static(root, fallback) {
                    let body = std::fs::read(&file).unwrap_or_default();
                    return Response::raw(200, mime_for(&file), body, false);
                }
            }
        }
        return Response::raw(
            404,
            "text/plain; charset=utf-8",
            format!("not found: {path}").into_bytes(),
            false,
        );
    }

    // No web build — the built-in mini UI keeps the bridge useful.
    match path {
        "/" => landing_page(),
        p if p.starts_with("/view/") => {
            let reference = http::percent_decode(p.trim_start_matches("/view/"));
            view_page_stub(&reference)
        }
        _ => Response::raw(
            404,
            "text/plain; charset=utf-8",
            "not found — no web build (run `npm run build` in web/)"
                .as_bytes()
                .to_vec(),
            false,
        ),
    }
}

fn html_escape(s: &str) -> String {
    s.replace('&', "&amp;")
        .replace('<', "&lt;")
        .replace('>', "&gt;")
        .replace('"', "&quot;")
}

fn html_page(title: &str, body: &str) -> Response {
    let doc = format!(
        "<!doctype html><html><head><meta charset=\"utf-8\">\
         <meta name=\"viewport\" content=\"width=device-width,initial-scale=1\">\
         <title>{}</title>\
         <style>body{{font-family:system-ui,sans-serif;max-width:60rem;margin:2rem auto;padding:0 1rem}}\
         a{{color:#06c}}code{{background:#eee;padding:.1em .3em;border-radius:3px}}\
         .card{{border:1px solid #ddd;border-radius:8px;padding:1rem;margin:.75rem 0}}\
         .muted{{color:#666;font-size:.9em}}</style></head><body>{}</body></html>",
        html_escape(title),
        body,
    );
    Response::raw(200, "text/html; charset=utf-8", doc.into_bytes(), false)
}

/// Built-in landing: driver cards + view links. Uses a throwaway
/// Host (the shared one is mutex-guarded in `route` — landing runs
/// outside it, so discover again; discovery is cheap probing).
fn landing_page() -> Response {
    let mut host = Host::discover();
    let drivers = host.drivers().to_vec();
    let views = host.list_views();
    let chrome = host.chrome();

    let mut body = String::from(
        "<h1>pavo</h1><p class=\"muted\">localhost bridge — \
        no Nuxt build found. Build <code>web/</code> for the full UI, or use the API below.</p>",
    );

    if !chrome.navbar.is_empty() || !chrome.dashboard.is_empty() || !chrome.statusbar.is_empty() {
        body.push_str("<h2>Chrome</h2><ul>");
        for slot_entries in [
            ("navbar", &chrome.navbar),
            ("dashboard", &chrome.dashboard),
            ("statusbar", &chrome.statusbar),
            ("routes", &chrome.routes),
        ] {
            for e in slot_entries.1 {
                body.push_str(&format!(
                    "<li><b>{}</b> {} → <code>{}</code> <span class=\"muted\">({} · {})</span></li>",
                    slot_entries.0,
                    html_escape(&e.contribution.title),
                    html_escape(&e.contribution.route),
                    html_escape(&e.driver),
                    html_escape(&e.contribution.id),
                ));
            }
        }
        body.push_str("</ul>");
    }

    body.push_str(&format!("<h2>Drivers ({})</h2>", drivers.len()));
    if drivers.is_empty() {
        body.push_str("<p class=\"muted\">none discovered — set PATH/TAMIAS_HOME.</p>");
    }
    for d in &drivers {
        let view_count = host.manifest(&d.kind).map(|m| m.views.len()).unwrap_or(0);
        body.push_str(&format!(
            "<div class=\"card\"><b>{}</b> <span class=\"muted\">{} · {} view(s)</span></div>",
            html_escape(&d.kind),
            html_escape(d.title()),
            view_count,
        ));
    }

    body.push_str(&format!("<h2>Views ({})</h2>", views.len()));
    for v in &views {
        let reference = html_escape(&v.namespaced);
        body.push_str(&format!(
            "<div class=\"card\"><a href=\"/view/{reference}\"><b>{}</b></a> \
             <span class=\"muted\">{} → {}</span><br><code>/api/view?ref={reference}</code></div>",
            html_escape(&v.title),
            html_escape(&v.namespaced),
            html_escape(&v.route),
        ));
    }
    html_page("pavo", &body)
}

/// Minimal pre-build view page: fetches nothing — renders the
/// ViewResult JSON inline so the bridge is debuggable without Nuxt.
fn view_page_stub(reference: &str) -> Response {
    // Discover a fresh host (same reasoning as landing_page).
    let mut host = Host::discover();
    let params = Params::new();
    let body_inner = match host.resolve_view(reference, params) {
        Ok(resolved) => format!(
            "<p><a href=\"/\">← all views</a></p><h1>{}</h1><pre>{}</pre>",
            html_escape(&resolved.namespaced),
            html_escape(&serde_json::to_string_pretty(&resolved.result).unwrap_or_default()),
        ),
        Err(e) => format!(
            "<p><a href=\"/\">← all views</a></p><h1>{}</h1><p><b>error:</b> {}</p>",
            html_escape(reference),
            html_escape(&e.to_string()),
        ),
    };
    html_page(reference, &body_inner)
}
