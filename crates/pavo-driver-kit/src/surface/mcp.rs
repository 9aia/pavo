//! Ops manifest → MCP tools over stdio (feature `mcp`).
//!
//! [`serve_stdio`] speaks MCP's stdio transport — newline-delimited
//! JSON-RPC 2.0, the same "one line in, one line out" discipline as
//! the pavo wire. Supported methods: `initialize`, `ping`,
//! `tools/list`, `tools/call`; `notifications/*` are absorbed without
//! reply. Each op becomes a tool named by its bare op name with an
//! `inputSchema` generated from typed params (`arg`/`opt` → `string`,
//! `flag` → `boolean`, `arg` params are `required`). `tools/call`
//! arguments are the wire params map verbatim — the driver's `cli.*`
//! dispatch owns semantics.
//!
//! `surfaces` (unbounded verbs) never become tools — a `tools/call`
//! must terminate. Drivers wanting `watch`/`mcp`/`acp` expose them on
//! a CLI surface instead.

use std::io::{self, BufRead, Write};

use serde_json::{json, Map, Value};

use super::{OpOutcome, OpSpec, OpsManifest, ParamKind, Surface};

/// One op → one MCP tool declaration.
fn tool(op: &OpSpec) -> Value {
    let mut properties = Map::new();
    let mut required = Vec::new();
    for param in &op.params {
        let ty = match param.kind {
            ParamKind::Flag => "boolean",
            ParamKind::Arg | ParamKind::Opt => "string",
        };
        let mut schema = json!({"type": ty});
        if let Some(doc) = &param.doc {
            schema["description"] = json!(doc);
        }
        if param.kind == ParamKind::Arg {
            required.push(json!(param.name));
        }
        properties.insert(param.name.clone(), schema);
    }
    let mut input = json!({"type": "object", "properties": properties});
    if !required.is_empty() {
        input["required"] = json!(required);
    }
    json!({
        "name": op.name,
        "description": if op.doc.is_empty() { op.name.clone() } else { op.doc.clone() },
        "inputSchema": input,
    })
}

/// `tools/call` → dispatch → MCP result: stdout is the text payload,
/// stderr joins as a second content block when non-empty, and a
/// nonzero op `code` sets `isError` (the engine's exit status, honest
/// data — not a protocol error).
fn call(manifest: &OpsManifest, surface: &mut dyn Surface, params: &Map<String, Value>) -> Value {
    let name = params
        .get("name")
        .and_then(Value::as_str)
        .unwrap_or_default();
    let Some(op) = manifest.op(name) else {
        return json!({
            "content": [{"type": "text", "text": format!("unknown tool: {name}")}],
            "isError": true,
        });
    };
    let arguments = params
        .get("arguments")
        .and_then(Value::as_object)
        .cloned()
        .unwrap_or_default();
    let OpOutcome {
        code,
        stdout,
        stderr,
    } = surface.op(op, &arguments);
    let mut content = vec![json!({"type": "text", "text": stdout})];
    if !stderr.is_empty() {
        content.push(json!({"type": "text", "text": stderr}));
    }
    json!({"content": content, "isError": code != 0})
}

/// One JSON-RPC request → its response value (`None` for
/// notifications, which must not be answered).
fn handle(manifest: &OpsManifest, surface: &mut dyn Surface, req: &Value) -> Option<Value> {
    let id = req.get("id").cloned();
    let method = req.get("method").and_then(Value::as_str).unwrap_or("");
    let result = |result: Value| {
        id.clone()
            .map(|id| json!({"jsonrpc": "2.0", "id": id, "result": result}))
    };
    let error = |code: i32, message: String| {
        id.clone().map(
            |id| json!({"jsonrpc": "2.0", "id": id, "error": {"code": code, "message": message}}),
        )
    };
    match method {
        "initialize" => result(json!({
            "protocolVersion": "2024-11-05",
            "capabilities": {"tools": {}},
            "serverInfo": {"name": manifest.engine, "version": env!("CARGO_PKG_VERSION")},
        })),
        "ping" => result(json!({})),
        "tools/list" => result(json!({
            "tools": manifest.ops.iter().map(tool).collect::<Vec<_>>(),
        })),
        "tools/call" => {
            let params = req
                .get("params")
                .and_then(Value::as_object)
                .cloned()
                .unwrap_or_default();
            result(call(manifest, surface, &params))
        }
        m if m.starts_with("notifications/") => None,
        "" => None,
        other => error(-32601, format!("method not found: {other}")),
    }
}

/// The MCP serve loop over any `BufRead`/`Write` pair — notifications
/// produce no line, parse failures answer `id:null` `-32700`.
///
/// # Errors
/// Propagates unexpected I/O failures; a dead client ends the loop
/// `Ok(())`, matching the pavo serve rule.
pub fn serve<R, W>(
    manifest: &OpsManifest,
    surface: &mut dyn Surface,
    input: R,
    mut output: W,
) -> io::Result<()>
where
    R: BufRead,
    W: Write,
{
    for line in input.lines() {
        let Ok(line) = line else { break };
        let line = line.trim();
        if line.is_empty() {
            continue;
        }
        let reply = match serde_json::from_str::<Value>(line) {
            Ok(req) => handle(manifest, surface, &req),
            Err(e) => Some(json!({
                "jsonrpc": "2.0", "id": Value::Null,
                "error": {"code": -32700, "message": format!("parse error: {e}")},
            })),
        };
        if let Some(reply) = reply {
            let text = serde_json::to_string(&reply).unwrap_or_else(|_| {
                "{\"jsonrpc\":\"2.0\",\"id\":null,\"error\":{\"code\":-32603,\"message\":\"serialization failed\"}}".to_string()
            });
            if writeln!(output, "{text}").is_err() || output.flush().is_err() {
                return Ok(()); // client went away
            }
        }
    }
    Ok(())
}

/// [`serve`] over real stdin/stdout — the `mcp` verb body.
///
/// # Errors
/// See [`serve`].
pub fn serve_stdio(manifest: &OpsManifest, surface: &mut dyn Surface) -> io::Result<()> {
    let stdin = io::stdin();
    let stdout = io::stdout();
    serve(manifest, surface, stdin.lock(), stdout.lock())
}

#[cfg(test)]
#[cfg_attr(coverage, coverage(off))]
mod tests {
    #![allow(clippy::unwrap_used)]

    use super::*;
    use serde_json::json;

    fn manifest() -> OpsManifest {
        OpsManifest::from_value(&json!({
            "engine": "fx",
            "ops": [{"query": "cli.find", "doc": "sym -> loc",
                     "params": [{"name": "symbol", "kind": "arg"},
                                {"name": "docs", "kind": "flag"}]}],
            "surfaces": ["watch"],
        }))
        .unwrap()
    }

    struct Echo;
    impl Surface for Echo {
        fn op(&mut self, op: &OpSpec, params: &Map<String, Value>) -> OpOutcome {
            OpOutcome {
                code: 0,
                stdout: format!("{}:{}", op.name, params["symbol"].as_str().unwrap()),
                stderr: String::new(),
            }
        }
    }

    fn lines(out: &[u8]) -> Vec<Value> {
        String::from_utf8_lossy(out)
            .lines()
            .map(|l| serde_json::from_str(l).unwrap())
            .collect()
    }

    #[test]
    fn tools_list_derives_input_schemas() {
        let m = manifest();
        let mut out = Vec::new();
        serve(
            &m,
            &mut Echo,
            b"{\"jsonrpc\":\"2.0\",\"id\":1,\"method\":\"tools/list\"}\n".as_slice(),
            &mut out,
        )
        .unwrap();
        let tools = lines(&out)[0]["result"]["tools"]
            .as_array()
            .unwrap()
            .clone();
        assert_eq!(tools.len(), 1); // `watch` is a surface, not a tool
        assert_eq!(tools[0]["name"], json!("find"));
        let schema = &tools[0]["inputSchema"];
        assert_eq!(schema["required"], json!(["symbol"]));
        assert_eq!(schema["properties"]["docs"]["type"], json!("boolean"));
    }

    #[test]
    fn initialize_ping_and_call_round_trip() {
        let m = manifest();
        let input = concat!(
            "{\"jsonrpc\":\"2.0\",\"id\":1,\"method\":\"initialize\",\"params\":{}}\n",
            "{\"jsonrpc\":\"2.0\",\"method\":\"notifications/initialized\"}\n",
            "{\"jsonrpc\":\"2.0\",\"id\":2,\"method\":\"ping\"}\n",
            "{\"jsonrpc\":\"2.0\",\"id\":3,\"method\":\"tools/call\",\"params\":{\"name\":\"find\",\"arguments\":{\"symbol\":\"main\",\"docs\":true}}}\n",
            "{\"jsonrpc\":\"2.0\",\"id\":4,\"method\":\"bogus\"}\n",
            "not json\n",
        );
        let mut out = Vec::new();
        serve(&m, &mut Echo, input.as_bytes(), &mut out).unwrap();
        let got = lines(&out);
        assert_eq!(got.len(), 5); // the notification answers nothing
        assert_eq!(got[0]["result"]["serverInfo"]["name"], json!("fx"));
        assert_eq!(got[1]["result"], json!({}));
        let call = &got[2]["result"];
        assert_eq!(call["content"][0]["text"], json!("find:main"));
        assert_eq!(call["isError"], json!(false));
        assert_eq!(got[3]["error"]["code"], json!(-32601));
        assert_eq!(got[4]["error"]["code"], json!(-32700));
        assert_eq!(got[4]["id"], json!(Value::Null));
    }
}
