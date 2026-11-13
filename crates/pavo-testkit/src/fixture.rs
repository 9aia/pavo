//! [`FixtureDriver`] — a builder that materializes a fake
//! `tamias-driver-<name>` executable in a tempdir.
//!
//! The generated driver is a Python 3 script (stdlib only, mirroring
//! the shell-script fixture style used across pavo-core/pavo-cli
//! tests but able to answer *configurable* JSON instead of pattern
//! matching). It implements:
//!
//! - `tamias-driver-<name> manifest` — prints the manifest verbatim
//!   (what pavo discovery probes).
//! - `tamias-driver-<name> capabilities` — advertises `pavo`/`serve`.
//! - `tamias-driver-<name> serve` — the ndjson wire: `manifest`,
//!   `views`, `view`, `query`, `invoke`, plus `ok:false` for unknown
//!   ops and malformed JSON.
//!
//! Behaviour is driven by a sibling `<name>.fixture.json` config the
//! builder writes next to the executable — manifest, view specs,
//! canned query payloads, a raw `views` override, per-op error
//! injection, a failure-mode block, or a recorded [`Transcript`][crate::record::Transcript]
//! for deterministic replay.

use std::io;
use std::path::PathBuf;

use pavo_protocol::manifest::ViewSpec;
use pavo_protocol::{Action, DriverManifest};
use serde_json::{Map, Value};
use std::collections::BTreeMap;

use crate::conformance::Violation;
use crate::record::Transcript;

/// The generated driver's filename stem config file name suffix.
const CONFIG_SUFFIX: &str = ".fixture.json";

/// Python source written as `tamias-driver-<name>`. Reads
/// `<name>.fixture.json` next to `__file__`; stdlib only.
const DRIVER_PY: &str = r#"#!/usr/bin/env python3
"""pavo-testkit fixture driver — generated, do not edit."""
import json
import os
import sys
import time

HERE = os.path.dirname(os.path.abspath(__file__))
CFG_PATH = os.path.join(HERE, os.path.basename(__file__) + ".fixture.json")
with open(CFG_PATH, "r", encoding="utf-8") as _f:
    CFG = json.load(_f)


def emit(obj):
    sys.stdout.write(json.dumps(obj, separators=(",", ":")) + "\n")
    sys.stdout.flush()


def respond_ok(result):
    emit({"ok": True, "result": result})


def respond_err(msg):
    emit({"ok": False, "error": str(msg)})


def canon(obj):
    return json.dumps(obj, sort_keys=True, separators=(",", ":"))


FAIL = CFG.get("failures") or {}
DELAY_MS = int(FAIL.get("delay_ms") or 0)
ERR_ALL = FAIL.get("error_all")
MANIFEST_GARBAGE = bool(FAIL.get("manifest_garbage"))
VIEWS_OVERRIDE = FAIL.get("views_override")
QUERY_ERRORS = CFG.get("query_errors") or {}
VIEW_ERRORS = CFG.get("view_errors") or {}
QUERIES = CFG.get("queries") or {}
VIEWS = CFG.get("views") or []
ACTIONS_CFG = CFG.get("actions") or []
INVOKE = CFG.get("invoke") or {}
TRANSCRIPT = CFG.get("transcript")
MANIFEST = CFG.get("manifest")

_count = [0]
_bad_json_done = [False]


def summaries():
    if VIEWS_OVERRIDE is not None:
        return VIEWS_OVERRIDE
    out = []
    for spec in VIEWS:
        s = {
            "id": spec["id"],
            "title": spec["title"],
            "route": spec["route"],
            "queries": [q.get("name") for q in spec.get("queries", [])],
        }
        if spec.get("description"):
            s["description"] = spec["description"]
        out.append(s)
    return out


def query_payload(name):
    if name in QUERY_ERRORS:
        return {"error": QUERY_ERRORS[name]}
    return QUERIES.get(name)


def view_result(view_id):
    for spec in VIEWS:
        if spec.get("id") == view_id:
            data = {}
            for b in spec.get("queries", []) or []:
                data[b.get("name")] = query_payload(b.get("name"))
            return {"view": spec, "data": data}
    return None


def action_specs():
    """action id → declaration. Sources: the manifest's `actions[]`,
    the config's `actions` list, `actions` arrays found while walking
    the view specs (node/inline-item declarations), and the same
    walk over query payloads (query-bound tree-item actions live in
    `data`, not specs)."""
    specs = {}

    def note(a):
        if isinstance(a, dict) and a.get("id"):
            specs.setdefault(a["id"], a)

    for a in (MANIFEST or {}).get("actions") or []:
        note(a)
    for a in ACTIONS_CFG:
        note(a)

    def walk(node):
        if isinstance(node, dict):
            for a in node.get("actions") or []:
                note(a)
            for v in node.values():
                walk(v)
        elif isinstance(node, list):
            for v in node:
                walk(v)

    for spec in VIEWS:
        walk(spec)
    for payload in QUERIES.values():
        walk(payload)
    return specs


def invoke_dispatch(obj):
    """`{"op":"invoke"}` — canned overrides win; otherwise declared
    actions get a required-param check (the ecosystem convention:
    missing required params → `errors`, still ok:true) and a default
    ActionResult echoing the request context. Undeclared actions are
    `ok:false`, like every real driver."""
    aid = obj.get("action")
    params = obj.get("params") or {}
    if aid in INVOKE:
        r = INVOKE[aid]
        if isinstance(r, str):
            return {"ok": False, "error": r}
        return {"ok": True, "result": r}
    spec = action_specs().get(aid)
    if spec is None:
        return {"ok": False, "error": "unknown action: %s" % aid}
    errors = {}
    for p in spec.get("params") or []:
        name = p.get("name")
        v = params.get(name)
        if p.get("required") and (v is None or v == ""):
            errors[name] = "required"
    if errors:
        return {"ok": True, "result": {"errors": errors}}
    return {"ok": True, "result": {
        "message": "invoked %s" % aid,
        "data": {"action": aid, "view": obj.get("view"),
                 "target": obj.get("target"), "params": params}}}


def dispatch(obj):
    """Map a parsed request object to a response dict (or raw str)."""
    if TRANSCRIPT is not None:
        key = canon(obj)
        for entry in TRANSCRIPT:
            if canon(entry.get("request")) == key:
                return entry.get("response")
        return {"ok": False, "error": "no recorded response for request"}
    op = obj.get("op")
    if op == "manifest":
        return {"ok": True, "result": MANIFEST or {}}
    if op == "views":
        if ERR_ALL:
            return {"ok": False, "error": ERR_ALL}
        return {"ok": True, "result": summaries()}
    if op == "view":
        if ERR_ALL:
            return {"ok": False, "error": ERR_ALL}
        vid = obj.get("view")
        if vid in VIEW_ERRORS:
            return {"ok": False, "error": VIEW_ERRORS[vid]}
        result = view_result(vid)
        if result is None:
            return {"ok": False, "error": "unknown view: %s" % vid}
        return {"ok": True, "result": result}
    if op == "query":
        if ERR_ALL:
            return {"ok": False, "error": ERR_ALL}
        q = obj.get("query")
        if q in QUERY_ERRORS:
            return {"ok": False, "error": QUERY_ERRORS[q]}
        return {"ok": True, "result": {"query": q, "data": QUERIES.get(q)}}
    if op == "invoke":
        if ERR_ALL:
            return {"ok": False, "error": ERR_ALL}
        return invoke_dispatch(obj)
    return {"ok": False, "error": "unknown op: %s" % op}


def serve():
    for line in sys.stdin:
        line = line.strip()
        if not line:
            continue
        _count[0] += 1
        exit_after = FAIL.get("exit_after")
        if exit_after is not None and _count[0] >= int(exit_after):
            return 0  # die mid-session, without answering
        exit_on = FAIL.get("exit_on")
        if exit_on and exit_on in line:
            return 0
        if DELAY_MS:
            time.sleep(DELAY_MS / 1000.0)
        if FAIL.get("bad_json_once") and not _bad_json_done[0]:
            _bad_json_done[0] = True
            sys.stdout.write("<<<this is not json>>>\n")
            sys.stdout.flush()
            continue
        try:
            obj = json.loads(line)
        except Exception:
            respond_err("invalid JSON request")
            continue
        if not isinstance(obj, dict):
            respond_err("request must be a JSON object")
            continue
        if obj.get("op") == "manifest" and MANIFEST_GARBAGE:
            sys.stdout.write("{\"name\": 1, \"oops\": true\n")
            sys.stdout.flush()
            continue
        resp = dispatch(obj)
        if isinstance(resp, str):
            sys.stdout.write(resp + "\n")
            sys.stdout.flush()
        else:
            emit(resp)
    return 0


def main():
    args = sys.argv[1:]
    if not args:
        sys.stderr.write("usage: %s manifest|capabilities|serve\n" % sys.argv[0])
        return 1
    verb = args[0]
    if verb == "manifest":
        sys.stdout.write(json.dumps(MANIFEST or {}, indent=2) + "\n")
        return 0
    if verb == "capabilities":
        caps = {"capabilities": ["pavo"], "verbs": ["manifest", "serve"]}
        if MANIFEST:
            caps = {
                "capabilities": MANIFEST.get("capabilities", ["pavo"]),
                "verbs": MANIFEST.get("verbs", ["manifest", "serve"]),
            }
        sys.stdout.write(json.dumps(caps) + "\n")
        return 0
    if verb == "serve":
        return serve()
    return 1


if __name__ == "__main__":
    sys.exit(main())
"#;

/// Failure-mode knobs for [`FixtureDriverBuilder`]. All default to
/// off/`None`; set the ones a test needs.
#[derive(Clone, Debug, Default)]
pub struct Failure {
    /// Emit one non-JSON line as the first `serve` response.
    pub bad_json_once: bool,
    /// Every `serve` request (except `manifest`) gets `ok:false` with
    /// this message.
    pub error_all: Option<String>,
    /// Sleep this many milliseconds before each response.
    pub delay_ms: u64,
    /// Exit the `serve` process (no response) upon the Nth request.
    pub exit_after: Option<u32>,
    /// Exit the `serve` process when a request line contains this
    /// substring (e.g. `"view":"board"`).
    pub exit_on: Option<String>,
    /// The `manifest` op returns a line that isn't a valid manifest.
    pub manifest_garbage: bool,
    /// Replace the `views` op result with this raw JSON array —
    /// for manifest/views consistency violation tests.
    pub views_override: Option<Vec<Value>>,
}

/// A materialized fixture driver. Keeps its tempdir alive for the
/// value's lifetime; [`FixtureDriver::path`] is the executable.
#[derive(Debug)]
pub struct FixtureDriver {
    name: String,
    /// Executable path — `…/tamias-driver-<name>`.
    pub path: PathBuf,
    _tmp: tempfile::TempDir,
}

impl FixtureDriver {
    /// Start building a fixture driver named `<name>` (the `<type>`
    /// in `tamias-driver-<name>`).
    #[must_use]
    pub fn builder(name: &str) -> FixtureDriverBuilder {
        FixtureDriverBuilder::new(name)
    }

    /// The driver kind (`<name>`).
    #[must_use]
    pub fn name(&self) -> &str {
        &self.name
    }

    /// Path to the `tamias-driver-<name>` executable.
    #[must_use]
    pub fn path(&self) -> &std::path::Path {
        &self.path
    }

    /// The tempdir the driver + config live in.
    #[must_use]
    pub fn dir(&self) -> &std::path::Path {
        self._tmp.path()
    }

    /// Run the conformance suite against this driver.
    ///
    /// # Panics
    /// Never — violations are returned, not panicked.
    #[must_use]
    pub fn check(&self) -> Vec<Violation> {
        crate::conformance::check_driver(&self.path)
    }

    /// Run `tamias-driver-<name> <verb>` (e.g. `"manifest"`,
    /// `"capabilities"`) to completion — for tests exercising the
    /// discovery probes rather than the `serve` wire. Retries briefly
    /// on `ExecutableFileBusy` like [`crate::wire::Session::spawn`].
    ///
    /// # Errors
    /// [`io::Error`] if the process can't be spawned or waited on.
    pub fn run_verb(&self, verb: &str) -> io::Result<std::process::Output> {
        let mut attempt = 0u32;
        loop {
            match std::process::Command::new(&self.path).arg(verb).output() {
                Err(e) if e.kind() == io::ErrorKind::ExecutableFileBusy && attempt < 20 => {
                    attempt += 1;
                    std::thread::sleep(std::time::Duration::from_millis(10));
                }
                result => return result,
            }
        }
    }
}

/// Builder for [`FixtureDriver`]. See the module docs for the wire
/// surface the generated driver speaks.
#[derive(Debug)]
pub struct FixtureDriverBuilder {
    name: String,
    manifest: Option<DriverManifest>,
    views: Vec<ViewSpec>,
    queries: Map<String, Value>,
    query_errors: BTreeMap<String, String>,
    view_errors: BTreeMap<String, String>,
    failure: Failure,
    transcript: Option<Transcript>,
    renderers: Vec<pavo_protocol::RendererDecl>,
    actions: Vec<Action>,
    invoke: Map<String, Value>,
}

impl FixtureDriverBuilder {
    /// A builder for driver kind `<name>`.
    #[must_use]
    pub fn new(name: &str) -> Self {
        Self {
            name: name.to_string(),
            manifest: None,
            views: Vec::new(),
            queries: Map::new(),
            query_errors: BTreeMap::new(),
            view_errors: BTreeMap::new(),
            failure: Failure::default(),
            transcript: None,
            renderers: Vec::new(),
            actions: Vec::new(),
            invoke: Map::new(),
        }
    }

    /// Set the manifest — used for both the `manifest` verb (discovery
    /// probe) and the `manifest` wire op. When unset, a minimal
    /// pavo-advertising manifest is generated from `name` + `views`.
    #[must_use]
    pub fn manifest(mut self, manifest: DriverManifest) -> Self {
        self.manifest = Some(manifest);
        self
    }

    /// Add a [`ViewSpec`] the driver serves via `view`/`views`. Its
    /// `queries` bindings are auto-fed from [`Self::query`] /
    /// [`Self::query_error`] payloads.
    #[must_use]
    pub fn view(mut self, spec: ViewSpec) -> Self {
        self.views.push(spec);
        self
    }

    /// Set the canned data payload for query/binding `name` — returned
    /// by `{"op":"query","query":name}` and placed in `view` results'
    /// `data` map under the binding name.
    #[must_use]
    pub fn query(mut self, name: &str, data: Value) -> Self {
        self.queries.insert(name.to_string(), data);
        self
    }

    /// Make query/binding `name` degrade: `query` ops get `ok:false`
    /// and `view` data carries `{"error": msg}` for the binding.
    #[must_use]
    pub fn query_error(mut self, name: &str, message: &str) -> Self {
        self.query_errors
            .insert(name.to_string(), message.to_string());
        self
    }

    /// Make `view` for `view_id` answer `ok:false` with `message`.
    #[must_use]
    pub fn view_error(mut self, view_id: &str, message: &str) -> Self {
        self.view_errors
            .insert(view_id.to_string(), message.to_string());
        self
    }

    /// Declare renderer plugins in the generated manifest — the
    /// manifest `renderers` list. The `render` op itself isn't
    /// canned; pair this with [`Self::transcript`] for render-op
    /// fixtures.
    #[must_use]
    pub fn renderers(mut self, decls: Vec<pavo_protocol::RendererDecl>) -> Self {
        self.renderers = decls;
        self
    }

    /// Declare an [`Action`] — merged into the generated manifest's
    /// `actions[]` and used by the `invoke` op's default dispatch
    /// (required-param checks, echo result). With a custom
    /// [`Self::manifest`] the actions still feed invoke param
    /// metadata but aren't added to the manifest — declare them
    /// there yourself.
    #[must_use]
    pub fn action(mut self, action: Action) -> Self {
        self.actions.push(action);
        self
    }

    /// Canned `invoke` result for `action_id` — `result` is answered
    /// verbatim as the `ok:true` payload (an [`ActionResult`]-shaped
    /// object: `message`/`errors`/`data`/`view`/`route`/`extra`).
    /// Overrides the default dispatch for that action; undeclared
    /// actions configured here still answer (fixture flexibility).
    #[must_use]
    pub fn invoke(mut self, action_id: &str, result: Value) -> Self {
        self.invoke.insert(action_id.to_string(), result);
        self
    }

    /// Make `invoke` for `action_id` answer `ok:false` with
    /// `message` — the driver-rejection path.
    #[must_use]
    pub fn invoke_error(mut self, action_id: &str, message: &str) -> Self {
        self.invoke
            .insert(action_id.to_string(), Value::String(message.to_string()));
        self
    }

    /// Replace the failure-mode configuration.
    #[must_use]
    pub fn failure(mut self, failure: Failure) -> Self {
        self.failure = failure;
        self
    }

    /// Replay mode: answer every `serve` request from a recorded
    /// [`Transcript`] instead of the manifest/views/queries config.
    /// Unmatched requests get `ok:false`.
    #[must_use]
    pub fn transcript(mut self, transcript: Transcript) -> Self {
        self.transcript = Some(transcript);
        self
    }

    /// Write `tamias-driver-<name>` + its `.fixture.json` config into
    /// a fresh tempdir and return the [`FixtureDriver`].
    ///
    /// # Errors
    /// [`io::Error`] if the tempdir or files can't be written, or the
    /// executable bit can't be set (non-unix).
    pub fn build(self) -> io::Result<FixtureDriver> {
        let tmp = tempfile::tempdir()?;
        let driver_path = tmp.path().join(format!("tamias-driver-{}", self.name));
        let config_path = tmp
            .path()
            .join(format!("tamias-driver-{}{}", self.name, CONFIG_SUFFIX));

        let manifest = self.manifest.unwrap_or_else(|| DriverManifest {
            name: self.name.clone(),
            title: self.name.clone(),
            icon: None,
            version: "0.0.0".to_string(),
            verbs: vec!["manifest".to_string(), "serve".to_string()],
            capabilities: vec!["pavo".to_string()],
            deps: None,
            contributes: Default::default(),
            views: self.views.clone(),
            renderers: self.renderers.clone(),
            actions: self.actions.clone(),
        });

        let mut config = Map::new();
        config.insert(
            "manifest".to_string(),
            serde_json::to_value(&manifest).unwrap_or(Value::Null),
        );
        config.insert(
            "views".to_string(),
            serde_json::to_value(&self.views).unwrap_or(Value::Null),
        );
        config.insert("queries".to_string(), Value::Object(self.queries.clone()));
        config.insert(
            "actions".to_string(),
            serde_json::to_value(&self.actions).unwrap_or(Value::Null),
        );
        if !self.invoke.is_empty() {
            config.insert("invoke".to_string(), Value::Object(self.invoke.clone()));
        }
        if !self.query_errors.is_empty() {
            config.insert(
                "query_errors".to_string(),
                serde_json::to_value(&self.query_errors).unwrap_or(Value::Null),
            );
        }
        if !self.view_errors.is_empty() {
            config.insert(
                "view_errors".to_string(),
                serde_json::to_value(&self.view_errors).unwrap_or(Value::Null),
            );
        }
        if let Some(t) = &self.transcript {
            // The python side expects a bare list of {request,response}
            // entries (Transcript's own shape is {"entries": [...]}).
            config.insert(
                "transcript".to_string(),
                serde_json::to_value(&t.entries).unwrap_or(Value::Null),
            );
        }
        // failures: skip empty object for readability of the config.
        let failures =
            serde_json::to_value(FailureConfig::from(&self.failure)).unwrap_or(Value::Null);
        if failures.as_object().is_some_and(|m| !m.is_empty()) {
            config.insert("failures".to_string(), failures);
        }

        std::fs::write(&driver_path, DRIVER_PY)?;
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            std::fs::set_permissions(&driver_path, std::fs::Permissions::from_mode(0o755))?;
        }
        std::fs::write(
            &config_path,
            serde_json::to_string_pretty(&Value::Object(config))?,
        )?;

        Ok(FixtureDriver {
            name: self.name,
            path: driver_path,
            _tmp: tmp,
        })
    }
}

/// Serde mirror of [`Failure`] — `Option` fields skip when `None` so
/// the generated config stays small.
#[derive(serde::Serialize)]
struct FailureConfig {
    #[serde(skip_serializing_if = "std::ops::Not::not")]
    bad_json_once: bool,
    #[serde(skip_serializing_if = "Option::is_none")]
    error_all: Option<String>,
    #[serde(skip_serializing_if = "is_zero")]
    delay_ms: u64,
    #[serde(skip_serializing_if = "Option::is_none")]
    exit_after: Option<u32>,
    #[serde(skip_serializing_if = "Option::is_none")]
    exit_on: Option<String>,
    #[serde(skip_serializing_if = "std::ops::Not::not")]
    manifest_garbage: bool,
    #[serde(skip_serializing_if = "Option::is_none")]
    views_override: Option<Vec<Value>>,
}

fn is_zero(v: &u64) -> bool {
    *v == 0
}

impl From<&Failure> for FailureConfig {
    fn from(f: &Failure) -> Self {
        Self {
            bad_json_once: f.bad_json_once,
            error_all: f.error_all.clone(),
            delay_ms: f.delay_ms,
            exit_after: f.exit_after,
            exit_on: f.exit_on.clone(),
            manifest_garbage: f.manifest_garbage,
            views_override: f.views_override.clone(),
        }
    }
}
