//! The surface-driver layer — an engine's **ops manifest** rendered
//! onto real surfaces (a generated CLI, an MCP tool server) by drivers
//! that know drivers, so engines stay blind.
//!
//! A view driver exposes its engine's verbs two ways: the `ops` argv
//! verb (or `{"op":"query","query":"ops"}` wire query) answers an ops
//! manifest — `{engine, invoke, ops:[{query,doc,params,mutates}],
//! surfaces}` — and each op is reachable as a `cli.<name>` wire query
//! whose `data` is `{op, argv, code, stdout, stderr}` (the engine ran
//! in a captured child, never on the serve stream). Surface drivers
//! consume exactly that contract:
//!
//! - [`cli`] (feature `cli`) — ops manifest → a generated clap CLI:
//!   op → subcommand, params → args. `tamias-driver-cli` is the
//!   standalone binary; an engine driver can also link the kit and
//!   serve its own ops in-process (`tamias-driver-kuda cli …`).
//! - [`mcp`] (feature `mcp`) — ops manifest → MCP tools over stdio:
//!   op → `tools/list` entry with a generated `inputSchema`, call →
//!   `tools/call`. `tamias-driver-mcp` is the standalone binary.
//! - [`client`] (feature `client`) — the process side standalone
//!   surfaces need: `tamias-driver-<name>` discovery, `ops` manifest
//!   fetch, one-shot wire queries, `run` passthrough.
//!
//! [`OpSpec`]s carry **typed params** so surfaces never guess:
//!
//! | kind    | CLI spelling                  | wire param  | MCP schema |
//! |---------|-------------------------------|-------------|------------|
//! | `arg`   | positional `<name>`           | `{"name":s}`| `string`, required |
//! | `opt`   | `--name <v>` / `--name=v` / bare `--name` | string or `true` | `string` |
//! | `flag`  | `--name`                      | `{"name":true}` | `boolean` |
//!
//! A manifest that still lists params as bare strings (`"params":
//! ["name"]`) parses each as `opt` — the permissive kind, accepted as
//! `--name`, `--name v`, or `--name=v`.
//!
//! **Secret-bearing params.** A [`ParamSpec`] may set `"secret": true`
//! to declare that its value must not ride argv (shell history,
//! `/proc`). Surfaces honor it declaratively: the generated CLI adds
//! `--params-stdin` to every op — a JSON object on stdin merged over
//! the argv params (**stdin wins per key**) — and warns on stderr
//! when a secret param still arrives via argv (the value itself is
//! never echoed). MCP `tools/call` already carries JSON arguments,
//! so no argv leak exists there and it needs nothing extra.
//!
//! `surfaces` names the engine's *unbounded* surfaces (`watch`,
//! `mcp`, `acp`, …) — verbs that never return and so can never be
//! request/response ops. The CLI surface exposes them as streaming
//! subcommands (argv passthrough, inherited stdio); the MCP surface
//! omits them — a tool call must terminate.

use serde::{Deserialize, Serialize};
use serde_json::{Map, Value};
use std::fmt;

#[cfg(feature = "cli")]
pub mod cli;
pub mod client;
#[cfg(feature = "mcp")]
pub mod mcp;

/// How an op param is spelled on a generated surface.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum ParamKind {
    /// Required positional — `find main` → `{"symbol": "main"}`.
    Arg,
    /// Named option — `--name <v>` / `--name=v`; bare `--name` → `true`.
    /// The default for untyped (bare-string) manifest params.
    #[default]
    Opt,
    /// Boolean flag — `--name` → `{"name": true}`.
    Flag,
}

/// One op param — name, surface kind, optional doc.
#[derive(Clone, Debug, PartialEq, Serialize)]
pub struct ParamSpec {
    /// Wire params key / CLI arg name.
    pub name: String,
    /// How surfaces spell it.
    pub kind: ParamKind,
    /// One-line help (mirrors the engine's own doc).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub doc: Option<String>,
    /// `true` marks the value secret-bearing — it must not ride argv;
    /// surfaces steer callers to `--params-stdin` and never echo the
    /// value. Additive within the manifest contract: absent = false.
    #[serde(default, skip_serializing_if = "std::ops::Not::not")]
    pub secret: bool,
    /// Optional [`pavo_protocol::ParamSchema`] — the typed form
    /// metadata hosts use to auto-generate action forms (enum
    /// options, ref pickers, validation). Additive: absent params
    /// derive a bare `string`/`bool` schema from `kind`.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub schema: Option<pavo_protocol::ParamSchema>,
}

/// Params deserialize from either the typed object form
/// (`{"name":"docs","kind":"flag"}`) or the legacy bare-string form
/// (`"docs"` → `opt`).
impl<'de> Deserialize<'de> for ParamSpec {
    fn deserialize<D>(d: D) -> Result<Self, D::Error>
    where
        D: serde::Deserializer<'de>,
    {
        #[derive(Deserialize)]
        struct Typed {
            name: String,
            #[serde(default)]
            kind: ParamKind,
            #[serde(default)]
            doc: Option<String>,
            #[serde(default)]
            secret: bool,
            #[serde(default)]
            schema: Option<pavo_protocol::ParamSchema>,
        }
        #[derive(Deserialize)]
        #[serde(untagged)]
        enum Repr {
            Name(String),
            Typed(Box<Typed>),
        }
        Ok(match Repr::deserialize(d)? {
            Repr::Name(name) => ParamSpec {
                name,
                kind: ParamKind::Opt,
                doc: None,
                secret: false,
                schema: None,
            },
            Repr::Typed(t) => ParamSpec {
                name: t.name,
                kind: t.kind,
                doc: t.doc,
                secret: t.secret,
                schema: t.schema,
            },
        })
    }
}

/// One engine op from the manifest — `cli.<name>` on the wire,
/// `<name>` as a generated subcommand / MCP tool.
#[derive(Clone, Debug, Default, PartialEq, Serialize)]
pub struct OpSpec {
    /// Bare op name (`find`) — the `cli.` prefix is stripped on parse.
    pub name: String,
    /// One-line doc.
    #[serde(default)]
    pub doc: String,
    /// Typed params.
    #[serde(default)]
    pub params: Vec<ParamSpec>,
    /// `true` when the op mutates engine state — surfaces may gate or
    /// label it; nothing auto-invokes a mutator.
    #[serde(default)]
    pub mutates: bool,
}

impl<'de> Deserialize<'de> for OpSpec {
    fn deserialize<D>(d: D) -> Result<Self, D::Error>
    where
        D: serde::Deserializer<'de>,
    {
        #[derive(Deserialize)]
        struct Repr {
            /// `"cli.find"` — the wire query spelling.
            #[serde(default)]
            query: Option<String>,
            /// `"find"` — drivers may emit the bare name instead.
            #[serde(default)]
            name: Option<String>,
            #[serde(default)]
            doc: String,
            #[serde(default)]
            params: Vec<ParamSpec>,
            #[serde(default)]
            mutates: bool,
        }
        let r = Repr::deserialize(d)?;
        let name = r
            .name
            .or_else(|| {
                r.query
                    .as_deref()
                    .map(|q| q.strip_prefix("cli.").unwrap_or(q).to_string())
            })
            .unwrap_or_default();
        Ok(OpSpec {
            name,
            doc: r.doc,
            params: r.params,
            mutates: r.mutates,
        })
    }
}

/// A parsed ops manifest — what a driver's `ops` verb/`ops` query emits.
#[derive(Clone, Debug, Default, PartialEq)]
pub struct OpsManifest {
    /// Engine name (`kuda`) — also the `<type>` in
    /// `tamias-driver-<type>` by convention.
    pub engine: String,
    /// The manifest's own how-to-invoke blurb (display only).
    pub invoke: String,
    /// Request/response ops — `cli.<name>` wire queries.
    pub ops: Vec<OpSpec>,
    /// Unbounded engine surfaces — stream, never return.
    pub surfaces: Vec<String>,
}

/// Errors parsing an ops manifest [`Value`].
#[derive(Debug)]
pub enum ManifestError {
    /// Top level wasn't a JSON object.
    NotAnObject,
    /// `ops` was present but not an array.
    BadOps,
}

impl fmt::Display for ManifestError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::NotAnObject => write!(f, "ops manifest is not a JSON object"),
            Self::BadOps => write!(f, "ops manifest `ops` is not an array"),
        }
    }
}

impl std::error::Error for ManifestError {}

impl OpsManifest {
    /// Parse the `ops` verb/query payload — tolerant: unknown fields
    /// are ignored, `ops` defaults to `[]`, params accept both spellings.
    ///
    /// # Errors
    /// [`ManifestError`] when the payload isn't an object or `ops`
    /// isn't an array.
    pub fn from_value(value: &Value) -> Result<Self, ManifestError> {
        let obj = value.as_object().ok_or(ManifestError::NotAnObject)?;
        let ops: Vec<OpSpec> = match obj.get("ops") {
            None | Some(Value::Null) => Vec::new(),
            Some(v) => serde_json::from_value(v.clone()).map_err(|_| ManifestError::BadOps)?,
        };
        Ok(Self {
            engine: obj
                .get("engine")
                .and_then(Value::as_str)
                .unwrap_or_default()
                .to_string(),
            invoke: obj
                .get("invoke")
                .and_then(Value::as_str)
                .unwrap_or_default()
                .to_string(),
            ops,
            surfaces: obj
                .get("surfaces")
                .and_then(Value::as_array)
                .map(|a| {
                    a.iter()
                        .filter_map(Value::as_str)
                        .map(str::to_string)
                        .collect()
                })
                .unwrap_or_default(),
        })
    }

    /// The op named `name` (bare — `find`, not `cli.find`).
    #[must_use]
    pub fn op(&self, name: &str) -> Option<&OpSpec> {
        self.ops.iter().find(|o| o.name == name)
    }
}

/// What one op run produced — the surface-facing half of the
/// `cli.<op>` query data convention (`{op, argv, code, stdout,
/// stderr}`). Surfaces print `stdout`/`stderr` and propagate `code`;
/// they never see the engine's argv.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct OpOutcome {
    /// The engine's exit status (`find` miss, `doctor` failure, …) —
    /// honest data, not a wire error.
    pub code: i32,
    /// Captured stdout.
    pub stdout: String,
    /// Captured stderr.
    pub stderr: String,
}

impl OpOutcome {
    /// Build from a `cli.<op>` query's `data` payload — tolerant of
    /// missing fields (`code` → 0, streams → empty).
    #[must_use]
    pub fn from_data(data: &Value) -> Self {
        Self {
            code: data
                .get("code")
                .and_then(Value::as_i64)
                .and_then(|c| i32::try_from(c).ok())
                .unwrap_or(0),
            stdout: data
                .get("stdout")
                .and_then(Value::as_str)
                .unwrap_or_default()
                .to_string(),
            stderr: data
                .get("stderr")
                .and_then(Value::as_str)
                .unwrap_or_default()
                .to_string(),
        }
    }

    /// Extract from a wire [`Response`](pavo_protocol::wire::Response):
    /// `ok` reads `result.data`; a wire error becomes `code: 2` with
    /// the error text on `stderr` (2 = misuse, distinct from engine
    /// statuses that ride `ok:true`).
    #[must_use]
    pub fn from_response(resp: &pavo_protocol::wire::Response) -> Self {
        if !resp.ok {
            return Self {
                code: 2,
                stdout: String::new(),
                stderr: resp.error.clone().unwrap_or_default(),
            };
        }
        Self::from_data(
            resp.result
                .as_ref()
                .and_then(|r| r.get("data"))
                .unwrap_or(&Value::Null),
        )
    }
}

/// The dispatch surface implementations plug into — one op runner
/// (captured: request/response) plus one unbounded-surface runner
/// (streaming: inherited stdio). Two implementors exist:
///
/// - [`client::DriverClient`] (feature `client`) — a spawned
///   `tamias-driver-<type>`; `op` = one-shot `cli.<name>` wire query,
///   `surface` = `run <name> <args>` with inherited stdio.
/// - an engine driver's own impl — `op` reuses its `cli.*` dispatch,
///   `surface` calls its engine lib in-process.
pub trait Surface {
    /// Run one request/response op with resolved params.
    fn op(&mut self, op: &OpSpec, params: &Map<String, Value>) -> OpOutcome;

    /// Run one unbounded surface verb (`watch`, `mcp`, `acp`) with
    /// trailing argv — streams until the engine stops it. Default:
    /// report unsupported (exit 2); implementors that front a `run`
    /// passthrough override.
    fn surface(&mut self, name: &str, args: &[String]) -> i32 {
        let _ = args;
        eprintln!("surface `{name}` is not wired for this driver");
        2
    }
}

#[cfg(test)]
#[cfg_attr(coverage, coverage(off))]
mod tests {
    #![allow(clippy::unwrap_used)]

    use super::*;
    use pavo_protocol::wire::Response;
    use serde_json::json;

    #[test]
    fn parses_typed_and_legacy_params() {
        let m = OpsManifest::from_value(&json!({
            "engine": "kuda",
            "invoke": "query cli.<op>",
            "ops": [
                {"query": "cli.find", "doc": "symbol -> file:line",
                 "params": [{"name": "symbol", "kind": "arg"},
                            {"name": "workspace", "kind": "flag", "doc": "all roots"}]},
                {"query": "cli.status", "params": ["workspace"]},
            ],
            "surfaces": ["watch", "mcp"],
        }))
        .unwrap();
        assert_eq!(m.engine, "kuda");
        let find = m.op("find").unwrap();
        assert_eq!(find.params[0].kind, ParamKind::Arg);
        assert_eq!(find.params[1].kind, ParamKind::Flag);
        assert_eq!(find.params[1].doc.as_deref(), Some("all roots"));
        assert!(!find.params[0].secret);
        // bare-string params degrade to the permissive opt
        assert_eq!(m.op("status").unwrap().params[0].kind, ParamKind::Opt);
        assert_eq!(m.surfaces, vec!["watch", "mcp"]);
        assert!(m.op("watch").is_none());
    }

    #[test]
    fn secret_param_round_trips_and_defaults_false() {
        let m = OpsManifest::from_value(&json!({
            "engine": "cuora",
            "ops": [{"name": "unlock",
                     "params": [{"name": "token", "kind": "opt", "secret": true},
                                {"name": "user"}]}],
        }))
        .unwrap();
        let op = m.op("unlock").unwrap();
        assert!(op.params[0].secret);
        assert!(!op.params[1].secret); // absent = false
                                       // serialization omits the marker when false, keeps it when true
        let v = serde_json::to_value(&op.params[0]).unwrap();
        assert_eq!(v["secret"], json!(true));
        let v = serde_json::to_value(&op.params[1]).unwrap();
        assert!(v.get("secret").is_none());
    }

    #[test]
    fn parses_name_spelling_and_tolerates_absent_ops() {
        let m = OpsManifest::from_value(&json!({
            "engine": "fx",
            "ops": [{"name": "ping"}],
        }))
        .unwrap();
        assert_eq!(m.op("ping").unwrap().name, "ping");
        let empty = OpsManifest::from_value(&json!({"engine": "fx"})).unwrap();
        assert!(empty.ops.is_empty());
        assert!(OpsManifest::from_value(&json!([])).is_err());
        assert!(OpsManifest::from_value(&json!({"ops": {}})).is_err());
    }

    #[test]
    fn outcome_reads_query_data_and_wire_errors() {
        let resp = Response::ok(json!({
            "query": "cli.find",
            "data": {"op": "find", "argv": ["find", "x"], "code": 1,
                     "stdout": "", "stderr": "not found"},
        }));
        let o = OpOutcome::from_response(&resp);
        assert_eq!((o.code, o.stderr.as_str()), (1, "not found"));
        let bad = OpOutcome::from_response(&Response::err("spawn blew up"));
        assert_eq!(bad.code, 2);
        assert_eq!(bad.stderr, "spawn blew up");
    }
}
