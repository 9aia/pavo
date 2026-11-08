//! `pavo serve` — the localhost HTTP bridge: pavo-core's [`Host`] on
//! one side, the Nuxt web app (or any JSON client) on the other.
//!
//! Hand-rolled HTTP/1.1 over [`std::net::TcpListener`], thread-per-
//! connection, std-only (see [`crate::http`] for the wire bits). One
//! [`Host`] is shared across threads behind an `Arc` — it is `Sync`:
//! lazy manifest fetches, client spawns, and the view cache are
//! internally locked, and each driver's `serve` client carries its own
//! mutex so requests to *different* drivers run in parallel while
//! same-driver requests stay serialized on the wire.
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
use std::sync::Arc;

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

    let host = Arc::new(Host::discover());
    let web_root = find_web_root();
    serve(listener, host, web_root)
}

/// The accept loop, split from [`run`] so tests can bind their own
/// listener (and learn the port) without parsing stdout.
fn serve(listener: TcpListener, host: Arc<Host>, web_root: Option<PathBuf>) -> io::Result<()> {
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
    host: &Arc<Host>,
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
fn route(req: &Request, host: &Arc<Host>, web_root: Option<&Path>) -> (Response, bool) {
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

fn api_route(path: &str, pairs: &[(String, String)], host: &Host) -> Response {
    let endpoint = path.trim_start_matches("/api/");

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

#[cfg(test)]
impl std::fmt::Debug for ApiFail {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "ApiFail({} {})", self.status, self.message)
    }
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

fn api_drivers(host: &Host) -> Result<Value, ApiFail> {
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

fn api_views(host: &Host) -> Result<Value, ApiFail> {
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

fn api_chrome(host: &Host) -> Result<Value, ApiFail> {
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

fn api_view(host: &Host, pairs: &[(String, String)]) -> Result<Value, ApiFail> {
    let target = view_target(pairs)?;
    let params: Params = http::collect_params(pairs).into_iter().collect();
    let resolved = host.resolve_view(&target, params)?;
    serde_json::to_value(&resolved.result).map_err(|e| ApiFail::new(500, e.to_string()))
}

fn api_query(host: &Host, pairs: &[(String, String)]) -> Result<Value, ApiFail> {
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
fn api_renderers(host: &Host) -> Result<Value, ApiFail> {
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
fn api_render(host: &Host, pairs: &[(String, String)]) -> Result<Value, ApiFail> {
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
/// Host (landing runs outside the shared one, so discover again;
/// discovery is cheap probing).
fn landing_page() -> Response {
    let host = Host::discover();
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
    let host = Host::discover();
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

#[cfg(test)]
mod tests {
    #![allow(clippy::unwrap_used)]
    use super::*;
    use pavo_core::PavoError;
    use std::sync::Arc;

    fn host() -> Arc<Host> {
        Arc::new(Host::discover())
    }

    fn req(method: &str, target: &str) -> Request {
        Request {
            method: method.to_string(),
            target: target.to_string(),
            headers: vec![],
            body: vec![],
        }
    }

    fn body_string(resp: &Response) -> String {
        String::from_utf8_lossy(&resp.body).into_owned()
    }

    #[test]
    fn mime_for_common_and_unknown() {
        assert!(mime_for(Path::new("a.html")).starts_with("text/html"));
        assert!(mime_for(Path::new("a.css")).starts_with("text/css"));
        assert!(mime_for(Path::new("a.mjs")).starts_with("text/javascript"));
        assert!(mime_for(Path::new("a.woff2")).starts_with("font/woff2"));
        assert!(mime_for(Path::new("a.bin")).starts_with("application/octet"));
        assert!(mime_for(Path::new("noext")).starts_with("application/octet"));
        assert!(mime_for(Path::new(".hidden")).starts_with("application/octet"));
    }

    #[test]
    fn resolve_static_serves_files_dirs_and_rejects_escapes() {
        let tmp = tempfile::tempdir().unwrap();
        let root = tmp.path();
        std::fs::write(root.join("index.html"), "<h1>root</h1>").unwrap();
        std::fs::create_dir(root.join("sub")).unwrap();
        std::fs::write(root.join("sub").join("page.html"), "page").unwrap();
        std::fs::create_dir(root.join("dirindex")).unwrap();
        std::fs::write(root.join("dirindex").join("index.html"), "di").unwrap();

        assert_eq!(
            resolve_static(root, "/index.html").unwrap(),
            root.join("index.html").canonicalize().unwrap()
        );
        // Directory → index.html inside it.
        assert!(resolve_static(root, "/dirindex").is_some());
        // `..` escapes rejected outright.
        assert!(resolve_static(root, "/../etc/passwd").is_none());
        assert!(resolve_static(root, "/sub/../../x").is_none());
        // Missing file.
        assert!(resolve_static(root, "/nope.txt").is_none());
    }

    #[test]
    fn html_escape_covers_markup_chars() {
        assert_eq!(
            html_escape("<a href=\"x\">&'</a>"),
            "&lt;a href=&quot;x&quot;&gt;&amp;'&lt;/a&gt;"
        );
        assert_eq!(html_escape("plain"), "plain");
    }

    #[test]
    fn view_target_prefers_ref_then_driver_view() {
        let p = parse_query_for_test("ref=demo%3Aboard&driver=x&view=y");
        assert_eq!(view_target(&p).unwrap(), "demo:board");

        let p = parse_query_for_test("driver=myco&view=item");
        assert_eq!(view_target(&p).unwrap(), "myco:item");

        // Bare driver = its first view.
        let p = parse_query_for_test("driver=myco");
        assert_eq!(view_target(&p).unwrap(), "myco");

        let p = parse_query_for_test("view=only");
        let err = view_target(&p).unwrap_err();
        assert_eq!(err.status, 400);
        assert!(err.message.contains("missing view ref"));
    }

    fn parse_query_for_test(q: &str) -> Vec<(String, String)> {
        crate::http::parse_query(q)
    }

    #[test]
    fn api_fail_maps_error_kinds_to_status() {
        let cases = [
            (PavoError::BadViewRef("x".into()), 400),
            (PavoError::UnknownDriver("d".into()), 404),
            (
                PavoError::UnknownView {
                    driver: "d".into(),
                    view: "v".into(),
                },
                404,
            ),
            (PavoError::Io(std::io::Error::other("x")), 500),
        ];
        for (e, want) in cases {
            assert_eq!(ApiFail::from(e).status, want);
        }
    }

    #[test]
    fn route_health_and_unknown_endpoint() {
        let host = host();
        let (resp, _) = route(&req("GET", "/api/health"), &host, None);
        assert_eq!(resp.status, 200);
        let body: Value = serde_json::from_str(&body_string(&resp)).unwrap();
        assert_eq!(body["ok"], true);

        let (resp, _) = route(&req("GET", "/api/nope"), &host, None);
        assert_eq!(resp.status, 404);
        let body: Value = serde_json::from_str(&body_string(&resp)).unwrap();
        assert_eq!(body["ok"], false);
        assert!(body["error"].as_str().unwrap().contains("unknown endpoint"));
    }

    #[test]
    fn route_method_rules() {
        let host = host();
        // POST to an api endpoint → 405 + Allow.
        let (resp, _) = route(&req("POST", "/api/health"), &host, None);
        assert_eq!(resp.status, 405);
        assert!(resp
            .extra
            .iter()
            .any(|(k, v)| k == "allow" && v == "GET, HEAD"));

        // HEAD gets an empty body.
        let (resp, _) = route(&req("HEAD", "/api/health"), &host, None);
        assert_eq!(resp.status, 200);
        assert!(resp.body.is_empty());

        // POST to a static path → 405.
        let (resp, _) = route(&req("POST", "/anything"), &host, None);
        assert_eq!(resp.status, 405);
    }

    #[test]
    fn static_response_without_web_root_serves_landing_and_404() {
        let (resp, _) = route(&req("GET", "/"), &host(), None);
        assert_eq!(resp.status, 200);
        assert!(resp.content_type.contains("text/html"));
        let body = body_string(&resp);
        assert!(body.contains("<h1>pavo</h1>"), "{body}");

        // /view/<ref> stub renders the error inline for a bad ref.
        let (resp, _) = route(&req("GET", "/view/ghost%3Anope"), &host(), None);
        assert_eq!(resp.status, 200);
        assert!(body_string(&resp).contains("error"));

        // Unknown static path → 404 with the no-build hint.
        let (resp, _) = route(&req("GET", "/missing.png"), &host(), None);
        assert_eq!(resp.status, 404);
        assert!(body_string(&resp).contains("no web build"));
    }

    #[test]
    fn static_response_with_web_root_prefers_spa_fallback() {
        let tmp = tempfile::tempdir().unwrap();
        let root = tmp.path();
        std::fs::write(root.join("200.html"), "SPA-200").unwrap();
        std::fs::write(root.join("index.html"), "IDX").unwrap();

        // Real file wins over fallback.
        let resp = static_response("/index.html", Some(root));
        assert_eq!(resp.status, 200);
        assert_eq!(body_string(&resp), "IDX");

        // Extensionless path → 200.html (preferred over index.html).
        let resp = static_response("/view/demo:board", Some(root));
        assert_eq!(resp.status, 200);
        assert_eq!(body_string(&resp), "SPA-200");

        // Path with an extension misses → 404.
        let resp = static_response("/missing.js", Some(root));
        assert_eq!(resp.status, 404);

        // Without 200.html the fallback chain reaches index.html.
        std::fs::remove_file(root.join("200.html")).unwrap();
        let resp = static_response("/dashboard", Some(root));
        assert_eq!(body_string(&resp), "IDX");
    }

    #[test]
    fn find_web_root_honors_pavo_web_dir() {
        let tmp = tempfile::tempdir().unwrap();
        // SAFETY-FREE env mutation: single-threaded concern only if a
        // concurrent test calls find_web_root — none do.
        std::env::set_var("PAVO_WEB_DIR", tmp.path());
        assert_eq!(find_web_root(), Some(tmp.path().to_path_buf()));
        std::env::set_var("PAVO_WEB_DIR", tmp.path().join("missing"));
        assert_eq!(find_web_root(), None);
        std::env::remove_var("PAVO_WEB_DIR");
    }

    /// Minimal sh fixture driver (`demo`) answering manifest + the
    /// serve wire — kept alive via `TempDir::keep`.
    const DEMO_DRIVER: &str = r#"#!/bin/sh
if [ "$1" = manifest ]; then
  printf '%s\n' '{"name":"demo","title":"Demo","version":"1.0.0","verbs":["manifest","serve"],"capabilities":["pavo"],"views":[{"id":"board","title":"Board","route":"/demo/board","queries":[{"name":"items","op":"query"}],"root":{"kind":"kv","pairs":[{"key":"k","value":"v"}]}}]}'
  exit 0
fi
if [ "$1" = serve ]; then
  while IFS= read -r line; do
    case "$line" in
      *'"op":"views"'*) printf '%s\n' '{"ok":true,"result":[{"id":"board","title":"Board","route":"/demo/board","queries":["items"]}]}' ;;
      *'"op":"view"'*) printf '%s\n' '{"ok":true,"result":{"view":{"id":"board","title":"Board","route":"/demo/board","root":{"kind":"kv","pairs":[{"key":"k","value":"v"}]},"queries":[{"name":"items","op":"query"}]},"data":{}}}' ;;
      *'"op":"query"'*) printf '%s\n' '{"ok":true,"result":{"query":"items","data":[1,2]}}' ;;
      *'"op":"manifest"'*) printf '%s\n' '{"ok":true,"result":{"name":"demo","title":"Demo","version":"1.0.0","verbs":["serve"],"capabilities":["pavo"],"views":[{"id":"board","title":"Board","route":"/demo/board","root":{"kind":"kv","pairs":[]}}]}}' ;;
    esac
  done
  exit 0
fi
exit 1
"#;

    /// A Host over a tempdir fixture driver — env-free discovery.
    fn fixture_host() -> Arc<Host> {
        let tmp = tempfile::tempdir().unwrap();
        let bin = tmp.path().join("bin");
        std::fs::create_dir_all(&bin).unwrap();
        let driver = bin.join("tamias-driver-demo");
        std::fs::write(&driver, DEMO_DRIVER).unwrap();
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            std::fs::set_permissions(&driver, std::fs::Permissions::from_mode(0o755)).unwrap();
        }
        let root = tmp.path().to_path_buf();
        // Keep the tempdir alive for the process lifetime.
        let _keep = tmp.keep();
        let home = root.join("home");
        std::fs::create_dir_all(&home).unwrap();
        let opts = pavo_core::discovery::DiscoveryOptions {
            path: Some(std::ffi::OsString::from(&bin)),
            tamias_home: Some(home),
            ..Default::default()
        };
        Arc::new(Host::discover_with(opts))
    }

    fn api(host: &Arc<Host>, target: &str) -> Response {
        let (path, query) = http::split_target(target);
        let pairs = http::parse_query(query);
        api_route(path, &pairs, host)
    }

    fn envelope(resp: &Response) -> Value {
        serde_json::from_str(&body_string(resp)).unwrap()
    }

    #[test]
    fn api_drivers_views_chrome_with_fixture() {
        let host = fixture_host();
        let body = envelope(&api(&host, "/api/drivers"));
        assert_eq!(body["ok"], true);
        assert_eq!(body["result"][0]["driver"], "demo");
        assert_eq!(body["result"][0]["view_count"], 1);

        let body = envelope(&api(&host, "/api/views"));
        assert_eq!(body["result"][0]["namespaced"], "demo:board");

        let body = envelope(&api(&host, "/api/chrome"));
        assert_eq!(body["ok"], true);
        assert!(body["result"]["navbar"].is_array());

        let body = envelope(&api(&host, "/api/renderers"));
        assert_eq!(body["ok"], true);
        assert_eq!(body["result"].as_array().unwrap().len(), 0);
    }

    #[test]
    fn api_view_and_query_roundtrip() {
        let host = fixture_host();
        let body = envelope(&api(&host, "/api/view?ref=demo%3Aboard"));
        assert_eq!(body["ok"], true, "{body}");
        assert_eq!(body["result"]["view"]["id"], "board");

        // driver+view split form + param passthrough.
        let body = envelope(&api(&host, "/api/view?driver=demo&view=board&param.x=1"));
        assert_eq!(body["ok"], true, "{body}");

        // /api/raw alias.
        let body = envelope(&api(&host, "/api/raw?ref=demo:board"));
        assert_eq!(body["ok"], true, "{body}");

        // Missing ref → 400 envelope.
        let resp = api(&host, "/api/view");
        assert_eq!(resp.status, 400);
        assert_eq!(envelope(&resp)["ok"], false);

        // Unknown driver → 404.
        let resp = api(&host, "/api/view?ref=ghost:board");
        assert_eq!(resp.status, 404);

        // Malformed ref → 400 (BadViewRef maps to 400).
        let resp = api(&host, "/api/view?ref=demo%3A");
        assert_eq!(resp.status, 400);
    }

    #[test]
    fn api_query_roundtrip_and_errors() {
        let host = fixture_host();
        let body = envelope(&api(&host, "/api/query?driver=demo&query=items"));
        assert_eq!(body["ok"], true, "{body}");
        assert_eq!(body["result"]["data"], serde_json::json!([1, 2]));

        // Missing params → 400.
        let resp = api(&host, "/api/query?driver=demo");
        assert_eq!(resp.status, 400);
        let resp = api(&host, "/api/query?query=items");
        assert_eq!(resp.status, 400);

        // Unknown driver → 404.
        let resp = api(&host, "/api/query?driver=ghost&query=x");
        assert_eq!(resp.status, 404);
    }

    #[test]
    fn api_render_validates_params() {
        let host = fixture_host();
        // Missing kind → 400.
        let resp = api(&host, "/api/render?surface=cli");
        assert_eq!(resp.status, 400);
        // Missing surface → 400.
        let resp = api(&host, "/api/render?kind=graph");
        assert_eq!(resp.status, 400);
        // node isn't JSON → 400.
        let resp = api(&host, "/api/render?kind=graph&surface=cli&node=%7Bbad");
        assert_eq!(resp.status, 400);
        // data isn't a JSON object → 400.
        let resp = api(
            &host,
            "/api/render?kind=graph&surface=cli&node=%7B%7D&data=%5B1%5D",
        );
        assert_eq!(resp.status, 400);
        // Well-formed but unclaimed kind → host-level error (500 or 404).
        let resp = api(
            &host,
            "/api/render?kind=graph&surface=cli&node=%7B%22kind%22%3A%22graph%22%7D",
        );
        assert!(resp.status >= 400, "{}", resp.status);
    }

    #[test]
    fn head_request_to_static_path_has_empty_body() {
        let host = host();
        let (resp, _) = route(&req("HEAD", "/missing.png"), &host, None);
        assert_eq!(resp.status, 404);
        assert!(resp.body.is_empty());
    }

    // ---------- concurrency ----------

    use std::time::{Duration, Instant};

    /// A pavo manifest JSON with no views — `host.query` doesn't
    /// consult view declarations, so empty is fine for these tests.
    fn bare_manifest(name: &str) -> String {
        format!(
            "{{\"name\":\"{name}\",\"title\":\"{name}\",\"version\":\"1.0.0\",\"verbs\":[\"manifest\",\"serve\"],\"capabilities\":[\"pavo\"],\"views\":[]}}"
        )
    }

    /// Write `body` as an executable `tamias-driver-<name>` under
    /// `dir`.
    fn write_exec(dir: &Path, name: &str, body: &str) {
        std::fs::create_dir_all(dir).unwrap();
        let path = dir.join(name);
        std::fs::write(&path, body).unwrap();
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            std::fs::set_permissions(&path, std::fs::Permissions::from_mode(0o755)).unwrap();
        }
    }

    /// `gate` driver: query `"wait"` touches `started`, then blocks
    /// until `release` exists; every other query answers `"fast"`.
    fn gate_driver(started: &Path, release: &Path) -> String {
        let manifest = bare_manifest("gate");
        let wire = format!("{{\"ok\":true,\"result\":{manifest}}}");
        format!(
            r#"#!/bin/sh
if [ "$1" = manifest ]; then
  printf '%s\n' '{manifest}'
  exit 0
fi
if [ "$1" = serve ]; then
  while IFS= read -r line; do
    case "$line" in
      *'"op":"manifest"'*) printf '%s\n' '{wire}' ;;
      *'"query":"wait"'*)
        touch '{started}'
        while [ ! -f '{release}' ]; do sleep 0.05; done
        printf '%s\n' '{{"ok":true,"result":{{"query":"wait","data":"released"}}}}' ;;
      *'"op":"query"'*) printf '%s\n' '{{"ok":true,"result":{{"query":"x","data":"fast"}}}}' ;;
    esac
  done
  exit 0
fi
exit 1
"#,
            manifest = manifest,
            wire = wire,
            started = started.display(),
            release = release.display(),
        )
    }

    /// `snap` driver: every query answers `"snap"` immediately.
    fn snap_driver() -> String {
        let manifest = bare_manifest("snap");
        let wire = format!("{{\"ok\":true,\"result\":{manifest}}}");
        format!(
            r#"#!/bin/sh
if [ "$1" = manifest ]; then
  printf '%s\n' '{manifest}'
  exit 0
fi
if [ "$1" = serve ]; then
  while IFS= read -r line; do
    case "$line" in
      *'"op":"manifest"'*) printf '%s\n' '{wire}' ;;
      *'"op":"query"'*) printf '%s\n' '{{"ok":true,"result":{{"query":"ping","data":"snap"}}}}' ;;
    esac
  done
  exit 0
fi
exit 1
"#,
            manifest = manifest,
            wire = wire,
        )
    }

    /// `echo` driver: answers each query with its own name as `data`
    /// (after a beat, to widen any would-be race window), and appends
    /// `begin`/`end` markers to `log`.
    fn echo_driver(log: &Path) -> String {
        let manifest = bare_manifest("echo");
        let wire = format!("{{\"ok\":true,\"result\":{manifest}}}");
        format!(
            r#"#!/bin/sh
if [ "$1" = manifest ]; then
  printf '%s\n' '{manifest}'
  exit 0
fi
if [ "$1" = serve ]; then
  while IFS= read -r line; do
    case "$line" in
      *'"op":"manifest"'*) printf '%s\n' '{wire}' ;;
      *'"op":"query"'*)
        q=$(printf '%s' "$line" | sed -n 's/.*"query":"\([^"]*\)".*/\1/p')
        echo "begin $q" >> '{log}'
        sleep 0.05
        echo "end $q" >> '{log}'
        printf '{{"ok":true,"result":{{"query":"%s","data":"%s"}}}}\n' "$q" "$q" ;;
    esac
  done
  exit 0
fi
exit 1
"#,
            manifest = manifest,
            wire = wire,
            log = log.display(),
        )
    }

    /// A Host over a tempdir `bin` — env-free discovery, tempdir kept.
    fn fixture_host_in(bin: &Path, root: &Path) -> Arc<Host> {
        let home = root.join("home");
        std::fs::create_dir_all(&home).unwrap();
        let opts = pavo_core::discovery::DiscoveryOptions {
            path: Some(std::ffi::OsString::from(bin)),
            tamias_home: Some(home),
            ..Default::default()
        };
        Arc::new(Host::discover_with(opts))
    }

    /// Poll `path` until it exists or `deadline` passes.
    fn wait_for(path: &Path, deadline: Duration) {
        let start = Instant::now();
        while !path.exists() {
            assert!(
                start.elapsed() < deadline,
                "{} never appeared",
                path.display()
            );
            std::thread::sleep(Duration::from_millis(10));
        }
    }

    /// The bridge's whole point: a blocked driver must not stall
    /// requests to other drivers. `gate` parks inside its `"wait"`
    /// query until released; `snap` must answer meanwhile.
    #[test]
    fn different_drivers_run_in_parallel() {
        let tmp = tempfile::tempdir().unwrap();
        let root = tmp.path().to_path_buf();
        let _keep = tmp.keep();
        let bin = root.join("bin");
        let started = root.join("gate-started");
        let release = root.join("gate-release");
        write_exec(&bin, "tamias-driver-gate", &gate_driver(&started, &release));
        write_exec(&bin, "tamias-driver-snap", &snap_driver());
        let host = fixture_host_in(&bin, &root);

        // Slow request on `gate` — parks until we release it.
        let host2 = Arc::clone(&host);
        let slow = std::thread::spawn(move || api(&host2, "/api/query?driver=gate&query=wait"));

        // Wait until the gate driver is provably inside its request.
        wait_for(&started, Duration::from_secs(10));

        // `snap` answers while `gate` is still blocked — under the old
        // global Host mutex this call would queue behind `wait`.
        let snap = api(&host, "/api/query?driver=snap&query=ping");
        let body = envelope(&snap);
        assert_eq!(body["ok"], true, "{body}");
        assert_eq!(body["result"]["data"], "snap");

        // Release the gate; its parked request completes.
        std::fs::write(&release, b"").unwrap();
        let slow_resp = slow.join().unwrap();
        let body = envelope(&slow_resp);
        assert_eq!(body["ok"], true, "{body}");
        assert_eq!(body["result"]["data"], "released");
    }

    /// Same-driver requests serialize on that driver's client lock —
    /// each caller gets *its own* response back (no cross-talk on the
    /// shared ndjson stream), and the driver's begin/end log never
    /// interleaves.
    #[test]
    fn same_driver_requests_serialize() {
        let tmp = tempfile::tempdir().unwrap();
        let root = tmp.path().to_path_buf();
        let _keep = tmp.keep();
        let bin = root.join("bin");
        let log = root.join("echo.log");
        write_exec(&bin, "tamias-driver-echo", &echo_driver(&log));
        let host = fixture_host_in(&bin, &root);

        let mut handles = Vec::new();
        for i in 0..4 {
            for name in ["aa", "bb"] {
                let host = Arc::clone(&host);
                let query = format!("q{i}{name}");
                handles.push(std::thread::spawn(move || {
                    api(&host, &format!("/api/query?driver=echo&query={query}"))
                }));
            }
        }
        for (i, h) in handles.into_iter().enumerate() {
            let resp = h.join().unwrap();
            let body = envelope(&resp);
            assert_eq!(body["ok"], true, "request {i}: {body}");
            // The echo driver returns the query name as `data` —
            // a swapped response would fail here.
            let expected = if i % 2 == 0 { "aa" } else { "bb" };
            let round = i / 2;
            assert_eq!(
                body["result"]["data"],
                format!("q{round}{expected}"),
                "request {i} got a foreign response"
            );
        }

        // begin/end markers strictly alternate — no interleaving.
        let log_text = std::fs::read_to_string(&log).unwrap();
        let mut open = false;
        for line in log_text.lines() {
            if line.starts_with("begin ") {
                assert!(!open, "interleaved begin while open: {line}");
                open = true;
            } else if line.starts_with("end ") {
                assert!(open, "end without begin: {line}");
                open = false;
            }
        }
        assert!(!open, "unclosed begin at end of log");
        assert_eq!(log_text.lines().count(), 16);
    }
}

#[cfg(test)]
#[cfg_attr(coverage, coverage(off))]
mod socket_tests {
    #![allow(clippy::unwrap_used)]
    //! Real-socket tests: `handle_connection` and the `serve` accept
    //! loop over localhost TCP — keep-alive, malformed heads, static
    //! fallback and API routing end to end.

    use super::*;
    use std::io::{Read, Write};
    use std::net::{TcpListener, TcpStream};
    use std::time::Duration;

    /// Connect to `port`, send `bytes`, shutdown write, read the
    /// whole reply (server closes or we hit EOF).
    fn roundtrip(port: u16, bytes: &[u8]) -> String {
        let mut c = TcpStream::connect(("127.0.0.1", port)).unwrap();
        c.set_read_timeout(Some(Duration::from_secs(10))).unwrap();
        c.write_all(bytes).unwrap();
        c.shutdown(std::net::Shutdown::Write).unwrap();
        let mut out = String::new();
        c.read_to_string(&mut out).unwrap();
        out
    }

    /// Read one HTTP response (head + content-length body) from an
    /// open keep-alive connection.
    fn read_http_response(c: &mut TcpStream) -> String {
        let mut out = Vec::new();
        let mut buf = [0u8; 4096];
        let head_end;
        loop {
            let n = c.read(&mut buf).unwrap();
            assert!(n > 0, "connection closed mid-response");
            out.extend_from_slice(&buf[..n]);
            if let Some(p) = find_bytes(&out, b"\r\n\r\n") {
                head_end = p + 4;
                break;
            }
        }
        let head = String::from_utf8_lossy(&out[..head_end]).into_owned();
        let len: usize = head
            .lines()
            .find_map(|l| {
                l.to_ascii_lowercase()
                    .strip_prefix("content-length:")
                    .and_then(|v| v.trim().parse().ok())
            })
            .unwrap_or(0);
        while out.len() < head_end + len {
            let n = c.read(&mut buf).unwrap();
            assert!(n > 0, "connection closed mid-body");
            out.extend_from_slice(&buf[..n]);
        }
        String::from_utf8_lossy(&out[..head_end + len]).into_owned()
    }

    fn find_bytes(hay: &[u8], needle: &[u8]) -> Option<usize> {
        hay.windows(needle.len()).position(|w| w == needle)
    }

    /// Spawn `serve` on an ephemeral port; returns the port. The
    /// accept loop runs forever — the thread leaks until test exit,
    /// which is fine.
    fn serve_port(host: Arc<Host>) -> u16 {
        let listener = TcpListener::bind(("127.0.0.1", 0)).unwrap();
        let port = listener.local_addr().unwrap().port();
        std::thread::spawn(move || {
            let _ = serve(listener, host, None);
        });
        port
    }

    #[test]
    fn handle_connection_keepalive_and_errors() {
        // Direct handle_connection over a socketpair-ish TCP pair.
        let listener = TcpListener::bind(("127.0.0.1", 0)).unwrap();
        let port = listener.local_addr().unwrap().port();
        let host = Arc::new(Host::discover());
        let (tx, rx) = std::sync::mpsc::channel::<()>();
        std::thread::spawn(move || {
            let (mut server, _) = listener.accept().unwrap();
            let _ = handle_connection(&mut server, &host, None);
            let _ = tx.send(());
        });

        let mut c = TcpStream::connect(("127.0.0.1", port)).unwrap();
        c.set_read_timeout(Some(Duration::from_secs(10))).unwrap();

        // Two keep-alive requests on one connection.
        c.write_all(b"GET /api/health HTTP/1.1\r\nHost: x\r\n\r\n")
            .unwrap();
        let first = read_http_response(&mut c);
        assert!(first.starts_with("HTTP/1.1 200"), "{first}");
        assert!(first.contains("\"ok\":true"), "{first}");

        c.write_all(b"GET /api/health HTTP/1.1\r\nConnection: close\r\n\r\n")
            .unwrap();
        let second = read_http_response(&mut c);
        assert!(second.contains("connection: close"), "{second}");
        // Server returned after close → channel fires.
        rx.recv_timeout(Duration::from_secs(10)).unwrap();

        // Malformed head → 400 + close.
        let listener = TcpListener::bind(("127.0.0.1", 0)).unwrap();
        let port = listener.local_addr().unwrap().port();
        let host = Arc::new(Host::discover());
        std::thread::spawn(move || {
            let (mut server, _) = listener.accept().unwrap();
            let _ = handle_connection(&mut server, &host, None);
        });
        let out = roundtrip(port, b"garbage\r\n\r\n");
        assert!(out.starts_with("HTTP/1.1 400"), "{out}");

        // Bare EOF → clean return.
        let listener = TcpListener::bind(("127.0.0.1", 0)).unwrap();
        let port = listener.local_addr().unwrap().port();
        let host = Arc::new(Host::discover());
        let (tx2, rx2) = std::sync::mpsc::channel::<()>();
        std::thread::spawn(move || {
            let (mut server, _) = listener.accept().unwrap();
            let _ = handle_connection(&mut server, &host, None);
            let _ = tx2.send(());
        });
        drop(TcpStream::connect(("127.0.0.1", port)).unwrap());
        rx2.recv_timeout(Duration::from_secs(10)).unwrap();
    }

    #[test]
    fn serve_accept_loop_routes_api_and_static() {
        // fixture_host is defined in the sibling tests module —
        // rebuild a minimal empty host here to keep modules decoupled.
        let host = Arc::new(Host::discover());
        let port = serve_port(host);

        let out = roundtrip(port, b"GET /api/health HTTP/1.1\r\n\r\n");
        assert!(out.starts_with("HTTP/1.1 200"), "{out}");
        assert!(out.contains("\"ok\":true"), "{out}");

        // Static fallback: no web root → landing page.
        let out = roundtrip(port, b"GET / HTTP/1.1\r\n\r\n");
        assert!(out.contains("<h1>pavo</h1>"), "{out}");

        // Unknown api endpoint → 404 envelope.
        let out = roundtrip(port, b"GET /api/nope HTTP/1.1\r\n\r\n");
        assert!(out.starts_with("HTTP/1.1 404"), "{out}");

        // LF-only terminator also parses.
        let out = roundtrip(port, b"GET /api/health HTTP/1.1\n\n");
        assert!(out.contains("\"ok\":true"), "{out}");
    }
}
