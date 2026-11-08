//! Minimal HTTP/1.1 primitives for `pavo serve` — a hand-rolled
//! localhost server (std-only, no async runtime, no deps).
//!
//! Scope is deliberately narrow: GET/HEAD-style JSON APIs plus static
//! file serving for the Nuxt build output. [`read_request`] parses one
//! request off a [`TcpStream`] with hard caps on head and body size;
//! [`Response`] serializes status line + headers + body with optional
//! CORS. Keep-alive is supported — the caller loops until the client
//! closes or sends `Connection: close`.

use std::collections::BTreeMap;
use std::io::{Read, Write};
use std::net::TcpStream;

/// Max bytes for the request head (request line + headers).
pub const MAX_HEAD: usize = 64 * 1024;
/// Max request body — the API is GET-only; this just bounds abuse.
pub const MAX_BODY: usize = 1024 * 1024;

/// A parsed HTTP request.
#[derive(Debug)]
pub struct Request {
    /// Uppercased method (`GET`, `HEAD`, …).
    pub method: String,
    /// Raw request target (path + query, still percent-encoded).
    pub target: String,
    /// Headers with lowercased names, in wire order.
    pub headers: Vec<(String, String)>,
    /// Request body bytes (bounded by [`MAX_BODY`]). Currently unused
    /// — the API is GET-only — but consumed so keep-alive framing
    /// stays correct if a client sends one.
    #[allow(dead_code)]
    pub body: Vec<u8>,
}

impl Request {
    /// First value of `name` (case-insensitive — names are stored
    /// lowercased).
    #[must_use]
    pub fn header(&self, name: &str) -> Option<&str> {
        self.headers
            .iter()
            .find(|(k, _)| k == name)
            .map(|(_, v)| v.as_str())
    }

    /// Whether the client asked to close after this response.
    #[must_use]
    pub fn wants_close(&self) -> bool {
        self.header("connection")
            .is_some_and(|v| v.eq_ignore_ascii_case("close"))
    }
}

/// Why [`read_request`] couldn't produce a request.
#[derive(Debug)]
pub enum ReadError {
    /// Malformed/oversized input — respond with this status.
    Http { status: u16, message: String },
    /// Underlying io failure.
    Io(std::io::Error),
}

impl From<std::io::Error> for ReadError {
    fn from(e: std::io::Error) -> Self {
        Self::Io(e)
    }
}

/// Read one request from the stream. Returns `Ok(None)` on clean EOF
/// (client closed an idle keep-alive connection).
///
/// # Errors
/// [`ReadError::Http`] on malformed input (caller should send the
/// status and close); [`ReadError::Io`] on transport failure.
pub fn read_request(stream: &mut TcpStream) -> Result<Option<Request>, ReadError> {
    // Read until the head terminator, bounded by MAX_HEAD.
    let mut head = Vec::new();
    let mut chunk = [0u8; 8192];
    let head_end = loop {
        if let Some(pos) = find_subslice(&head, b"\r\n\r\n") {
            break pos + 4;
        }
        if let Some(pos) = find_subslice(&head, b"\n\n") {
            break pos + 2;
        }
        if head.len() >= MAX_HEAD {
            return Err(ReadError::Http {
                status: 431,
                message: "request headers too large".to_string(),
            });
        }
        let n = stream.read(&mut chunk)?;
        if n == 0 {
            if head.is_empty() {
                return Ok(None);
            }
            return Err(ReadError::Http {
                status: 400,
                message: "connection closed mid-request".to_string(),
            });
        }
        head.extend_from_slice(&chunk[..n]);
    };

    // Split head into text lines (head is attacker-controlled but
    // bounded; lossy UTF-8 keeps parsing total).
    let head_text = String::from_utf8_lossy(&head[..head_end]);
    let mut lines = head_text.lines();
    let request_line = lines.next().unwrap_or("").trim_end_matches('\r');
    let mut parts = request_line.split_whitespace();
    let (Some(method), Some(target)) = (parts.next(), parts.next()) else {
        return Err(ReadError::Http {
            status: 400,
            message: "malformed request line".to_string(),
        });
    };
    // HTTP version token is ignored — we only speak 1.1 semantics.

    let mut headers = Vec::new();
    let mut content_length: Option<usize> = None;
    for line in lines {
        let line = line.trim_end_matches('\r');
        if line.is_empty() {
            continue;
        }
        let Some((name, value)) = line.split_once(':') else {
            return Err(ReadError::Http {
                status: 400,
                message: format!("malformed header: {line:?}"),
            });
        };
        let name = name.trim().to_ascii_lowercase();
        let value = value.trim().to_string();
        if name == "content-length" {
            match value.parse::<usize>() {
                Ok(n) => content_length = Some(n),
                Err(_) => {
                    return Err(ReadError::Http {
                        status: 400,
                        message: "bad Content-Length".to_string(),
                    });
                }
            }
        }
        headers.push((name, value));
    }

    // Body: whatever already arrived past the head, then read_exact.
    let mut body: Vec<u8> = head[head_end..].to_vec();
    let declared = content_length.unwrap_or(0);
    if declared > MAX_BODY {
        return Err(ReadError::Http {
            status: 413,
            message: "request body too large".to_string(),
        });
    }
    while body.len() < declared {
        let mut buf = vec![0u8; declared - body.len()];
        match stream.read(&mut buf) {
            Ok(0) => {
                return Err(ReadError::Http {
                    status: 400,
                    message: "connection closed mid-body".to_string(),
                });
            }
            Ok(n) => body.extend_from_slice(&buf[..n]),
            Err(e) if e.kind() == std::io::ErrorKind::Interrupted => continue,
            Err(e) => return Err(ReadError::Io(e)),
        }
    }
    body.truncate(declared);

    Ok(Some(Request {
        method: method.to_string(),
        target: target.to_string(),
        headers,
        body,
    }))
}

fn find_subslice(hay: &[u8], needle: &[u8]) -> Option<usize> {
    hay.windows(needle.len()).position(|w| w == needle)
}

/// An HTTP response to write back.
pub struct Response {
    /// Status code.
    pub status: u16,
    /// Content-Type header value.
    pub content_type: String,
    /// Body bytes.
    pub body: Vec<u8>,
    /// Add `Access-Control-Allow-Origin: *` (API endpoints).
    pub cors: bool,
    /// Extra headers (e.g. `Allow`).
    pub extra: Vec<(String, String)>,
}

impl Response {
    /// JSON body response.
    #[must_use]
    pub fn json(status: u16, value: &serde_json::Value, cors: bool) -> Self {
        Self {
            status,
            content_type: "application/json; charset=utf-8".to_string(),
            body: serde_json::to_vec(value).unwrap_or_else(|_| b"{}".to_vec()),
            cors,
            extra: Vec::new(),
        }
    }

    /// Raw body response.
    #[must_use]
    pub fn raw(status: u16, content_type: impl Into<String>, body: Vec<u8>, cors: bool) -> Self {
        Self {
            status,
            content_type: content_type.into(),
            body,
            cors,
            extra: Vec::new(),
        }
    }

    /// Serialize + write the response. `close` forces
    /// `Connection: close`.
    ///
    /// # Errors
    /// Propagates io failures.
    pub fn write(&self, stream: &mut TcpStream, close: bool) -> std::io::Result<()> {
        let reason = reason_phrase(self.status);
        let mut head = format!(
            "HTTP/1.1 {} {}\r\ncontent-type: {}\r\ncontent-length: {}\r\nconnection: {}\r\n",
            self.status,
            reason,
            self.content_type,
            self.body.len(),
            if close { "close" } else { "keep-alive" },
        );
        if self.cors {
            head.push_str("access-control-allow-origin: *\r\n");
        }
        for (name, value) in &self.extra {
            head.push_str(&format!("{name}: {value}\r\n"));
        }
        head.push_str("\r\n");
        stream.write_all(head.as_bytes())?;
        stream.write_all(&self.body)?;
        stream.flush()
    }
}

fn reason_phrase(status: u16) -> &'static str {
    match status {
        200 => "OK",
        204 => "No Content",
        301 => "Moved Permanently",
        302 => "Found",
        304 => "Not Modified",
        400 => "Bad Request",
        404 => "Not Found",
        405 => "Method Not Allowed",
        413 => "Payload Too Large",
        431 => "Request Header Fields Too Large",
        500 => "Internal Server Error",
        _ => "Status",
    }
}

/// Split a request target into (path, raw query string).
#[must_use]
pub fn split_target(target: &str) -> (&str, &str) {
    match target.split_once('?') {
        Some((p, q)) => (p, q),
        None => (target, ""),
    }
}

/// Parse a `a=1&b=2` query string into decoded pairs. Malformed pairs
/// (no `=`) are kept with an empty value; decoding is lossy-safe.
#[must_use]
pub fn parse_query(query: &str) -> Vec<(String, String)> {
    if query.is_empty() {
        return Vec::new();
    }
    query
        .split('&')
        .filter(|p| !p.is_empty())
        .map(|pair| match pair.split_once('=') {
            Some((k, v)) => (percent_decode(k), percent_decode(v)),
            None => (percent_decode(pair), String::new()),
        })
        .collect()
}

/// Percent-decode a URL component (`%XX` + `+` → space). Invalid
/// sequences pass through literally.
#[must_use]
pub fn percent_decode(input: &str) -> String {
    let bytes = input.as_bytes();
    let mut out = Vec::with_capacity(bytes.len());
    let mut i = 0;
    while i < bytes.len() {
        match bytes[i] {
            b'+' => {
                out.push(b' ');
                i += 1;
            }
            b'%' if i + 2 < bytes.len() => {
                let hex = &input[i + 1..i + 3];
                match u8::from_str_radix(hex, 16) {
                    Ok(b) => {
                        out.push(b);
                        i += 3;
                    }
                    Err(_) => {
                        out.push(b'%');
                        i += 1;
                    }
                }
            }
            b => {
                out.push(b);
                i += 1;
            }
        }
    }
    // Path/query targets are UTF-8 in practice; lossy keeps us total.
    String::from_utf8_lossy(&out).into_owned()
}

/// Look up a decoded query param (first occurrence wins).
#[must_use]
pub fn query_get<'a>(params: &'a [(String, String)], key: &str) -> Option<&'a str> {
    params
        .iter()
        .find(|(k, _)| k == key)
        .map(|(_, v)| v.as_str())
}

/// Collect `param.<key>=<value>` pairs into a map. Values parse as
/// JSON when possible (`k=42`, `k=true`, `k={…}`), else strings —
/// same rule as the CLI's `--param`.
#[must_use]
pub fn collect_params(pairs: &[(String, String)]) -> BTreeMap<String, serde_json::Value> {
    let mut out = BTreeMap::new();
    for (k, v) in pairs {
        if let Some(key) = k.strip_prefix("param.") {
            if !key.is_empty() {
                let value = serde_json::from_str(v)
                    .unwrap_or_else(|_| serde_json::Value::String(v.clone()));
                out.insert(key.to_string(), value);
            }
        }
    }
    out
}

#[cfg(test)]
mod tests {
    #![allow(clippy::unwrap_used)]
    use super::*;

    #[test]
    fn percent_decode_basic() {
        assert_eq!(percent_decode("a%20b+c"), "a b c");
        assert_eq!(percent_decode("demo%3Aboard"), "demo:board");
        assert_eq!(percent_decode("100%"), "100%");
        assert_eq!(percent_decode("%zz"), "%zz");
        assert_eq!(percent_decode("%E2%82%AC"), "€");
    }

    #[test]
    fn parse_query_pairs() {
        let p = parse_query("ref=demo%3Aboard&param.limit=5&flag&empty=");
        assert_eq!(p[0], ("ref".to_string(), "demo:board".to_string()));
        assert_eq!(p[1], ("param.limit".to_string(), "5".to_string()));
        assert_eq!(p[2], ("flag".to_string(), String::new()));
        assert_eq!(p[3], ("empty".to_string(), String::new()));
    }

    #[test]
    fn collect_params_json_and_strings() {
        let pairs =
            parse_query("param.a=1&param.b=true&param.c=hi&param.d=%7B%22x%22%3A1%7D&other=z");
        let m = collect_params(&pairs);
        assert_eq!(m["a"], serde_json::json!(1));
        assert_eq!(m["b"], serde_json::json!(true));
        assert_eq!(m["c"], serde_json::json!("hi"));
        assert_eq!(m["d"], serde_json::json!({"x": 1}));
        assert!(!m.contains_key("other"));
    }

    #[test]
    fn split_target_works() {
        assert_eq!(split_target("/api/view?ref=x"), ("/api/view", "ref=x"));
        assert_eq!(split_target("/api/health"), ("/api/health", ""));
    }

    #[test]
    fn query_get_first_wins() {
        let p = parse_query("a=1&a=2&b=x");
        assert_eq!(query_get(&p, "a"), Some("1"));
        assert_eq!(query_get(&p, "b"), Some("x"));
        assert_eq!(query_get(&p, "c"), None);
    }

    #[test]
    fn collect_params_skips_empty_and_non_param_keys() {
        let p = parse_query("param.=x&paramok=1&param.a=%7B%7D&flag");
        let m = collect_params(&p);
        assert_eq!(m.len(), 1);
        assert_eq!(m["a"], serde_json::json!({}));
    }

    /// A connected loopback pair: (client, server). The server end is
    /// what `read_request`/`Response::write` operate on.
    fn pair() -> (std::net::TcpStream, std::net::TcpStream) {
        let listener = std::net::TcpListener::bind("127.0.0.1:0").unwrap();
        let addr = listener.local_addr().unwrap();
        let client = std::net::TcpStream::connect(addr).unwrap();
        let (server, _) = listener.accept().unwrap();
        (client, server)
    }

    fn send(client: &mut std::net::TcpStream, bytes: &[u8]) {
        client.write_all(bytes).unwrap();
        client.flush().unwrap();
    }

    #[test]
    fn read_request_parses_head_and_headers() {
        let (mut client, mut server) = pair();
        send(
            &mut client,
            b"GET /api/health?x=1 HTTP/1.1\r\nHost: localhost\r\nX-Test: yes\r\n\r\n",
        );
        let req = read_request(&mut server).unwrap().unwrap();
        assert_eq!(req.method, "GET");
        assert_eq!(req.target, "/api/health?x=1");
        assert_eq!(req.header("host"), Some("localhost"));
        assert_eq!(req.header("x-test"), Some("yes"));
        assert_eq!(req.header("missing"), None);
        assert!(!req.wants_close());
    }

    #[test]
    fn read_request_accepts_lf_only_terminator() {
        let (mut client, mut server) = pair();
        send(&mut client, b"HEAD / HTTP/1.1\n\n");
        let req = read_request(&mut server).unwrap().unwrap();
        assert_eq!(req.method, "HEAD");
        assert_eq!(req.target, "/");
    }

    #[test]
    fn read_request_eof_on_idle_is_none() {
        let (client, mut server) = pair();
        drop(client);
        assert!(matches!(read_request(&mut server), Ok(None)));
    }

    #[test]
    fn read_request_malformed_variants_error() {
        // Bad request line.
        let (mut client, mut server) = pair();
        send(&mut client, b"garbage\r\n\r\n");
        assert!(matches!(
            read_request(&mut server),
            Err(ReadError::Http { status: 400, .. })
        ));
        drop((client, server));

        // Header without a colon.
        let (mut client, mut server) = pair();
        send(&mut client, b"GET / HTTP/1.1\r\nno-colon-here\r\n\r\n");
        assert!(matches!(
            read_request(&mut server),
            Err(ReadError::Http { status: 400, .. })
        ));
        drop((client, server));

        // Non-numeric Content-Length.
        let (mut client, mut server) = pair();
        send(
            &mut client,
            b"POST / HTTP/1.1\r\nContent-Length: abc\r\n\r\n",
        );
        assert!(matches!(
            read_request(&mut server),
            Err(ReadError::Http { status: 400, .. })
        ));
    }

    #[test]
    fn read_request_reads_body_by_content_length() {
        let (mut client, mut server) = pair();
        send(
            &mut client,
            b"POST /x HTTP/1.1\r\nContent-Length: 5\r\n\r\nhello",
        );
        let req = read_request(&mut server).unwrap().unwrap();
        assert_eq!(req.body, b"hello");
    }

    #[test]
    fn read_request_rejects_oversized_body_declaration() {
        let (mut client, mut server) = pair();
        let declared = MAX_BODY + 1;
        send(
            &mut client,
            format!("POST / HTTP/1.1\r\nContent-Length: {declared}\r\n\r\n").as_bytes(),
        );
        assert!(matches!(
            read_request(&mut server),
            Err(ReadError::Http { status: 413, .. })
        ));
    }

    #[test]
    fn read_request_rejects_oversized_head() {
        let (mut client, mut server) = pair();
        let filler = vec![b'A'; MAX_HEAD + 16];
        send(&mut client, &filler);
        assert!(matches!(
            read_request(&mut server),
            Err(ReadError::Http { status: 431, .. })
        ));
    }

    #[test]
    fn response_write_serializes_status_headers_body() {
        let (mut client, mut server) = pair();
        let resp = Response::json(200, &serde_json::json!({"ok": true}), true);
        resp.write(&mut server, false).unwrap();
        drop(server);
        let mut out = String::new();
        client.read_to_string(&mut out).unwrap();
        assert!(out.starts_with("HTTP/1.1 200 OK\r\n"), "{out}");
        assert!(out.contains("content-type: application/json"), "{out}");
        assert!(out.contains("access-control-allow-origin: *"), "{out}");
        assert!(out.contains("connection: keep-alive"), "{out}");
        assert!(out.ends_with("{\"ok\":true}"), "{out}");
    }

    #[test]
    fn response_write_extra_headers_and_close_and_unknown_status() {
        let (mut client, mut server) = pair();
        let mut resp = Response::raw(418, "text/plain", b"teapot".to_vec(), false);
        resp.extra.push(("allow".to_string(), "GET".to_string()));
        resp.write(&mut server, true).unwrap();
        drop(server);
        let mut out = String::new();
        client.read_to_string(&mut out).unwrap();
        assert!(out.starts_with("HTTP/1.1 418 Status\r\n"), "{out}");
        assert!(out.contains("allow: GET\r\n"), "{out}");
        assert!(out.contains("connection: close\r\n"), "{out}");
        assert!(!out.contains("access-control-allow-origin"), "{out}");
    }
}
