# Write a driver

A pavo driver is an executable named `tamias-driver-<type>` that
answers the ndjson JSON-RPC wire on its `serve` verb — the same
convention as `myco serve`, `cuora serve`, `sepia serve`. The
driver owns its manifest and ViewNode trees; product state is only
needed when a `view`/`query` op actually runs.

## The contract in one paragraph

Spawned as `tamias-driver-<type> serve`, the driver reads one JSON
request per line on stdin and writes one JSON response per line on
stdout: `{"op":"manifest"}` → the
[`DriverManifest`](/reference/driver-manifest), `{"op":"views"}` →
view summaries, `{"op":"view","view":"…","params":{…}}` → a
`ViewResult` (tree + bound data), `{"op":"query",…}` → one query's
data. Every response is `{"ok":true,"result":…}` or
`{"ok":false,"error":"…"}`. Full spec: [Wire ops](/reference/wire-ops).

## Minimal shape

```python
#!/usr/bin/env python3
# tamias-driver-hello — stdlib only, like examples/drivers/*
import json, sys

def respond(req):
    op = req.get("op")
    if op == "manifest":
        return {"name": "hello", "title": "Hello", "version": "0.1.0",
                "verbs": ["manifest", "serve"], "capabilities": ["pavo"],
                "views": [view_spec()]}
    if op == "views":
        return [{"id": "hi", "title": "Hello", "route": "/hello/hi",
                 "queries": []}]
    if op == "view" and req.get("view") == "hi":
        return {"view": view_spec(), "data": {}}
    return {"ok": False}  # handled below

def view_spec():
    return {"id": "hi", "title": "Hello", "route": "/hello/hi",
            "queries": [],
            "root": {"kind": "markdown",
                     "text": "Hello from **pavo**!"}}

for line in sys.stdin:
    try:
        req = json.loads(line)
        if req.get("op") == "manifest":
            out = {"ok": True, "result": respond(req)}
        elif req.get("op") == "views":
            out = {"ok": True, "result": respond(req)}
        elif req.get("op") == "view":
            out = {"ok": True, "result": respond(req)}
        else:
            out = {"ok": False, "error": f"unknown op {req.get('op')!r}"}
    except Exception as e:
        out = {"ok": False, "error": str(e)}
    sys.stdout.write(json.dumps(out) + "\n")
    sys.stdout.flush()
```

Save it as `tamias-driver-hello`, `chmod +x`, put it on `PATH`, and
`pavo view hello:hi` renders the markdown node.

## Adding data: query bindings

Nodes pull data through `NodeBase.query` — a name that references a
[`QueryBinding`](/reference/driver-manifest) on the enclosing
`ViewSpec`:

```json
"queries": [{"name": "items", "op": "query", "params": {"kind": "items"}}],
"root": {"kind": "table",
         "query": "items",
         "columns": [{"key": "id", "title": "ID"},
                     {"key": "title", "title": "Title"}]}
```

When pavo resolves the view it calls the binding's `op` on your
`serve` and puts the result in `ViewResult.data["items"]`. The
`table` node reads `rows` from it (objects keyed by column `key`,
or inline `rows` arrays). A binding that fails lands as
`{"error": "…"}` in `data` — the view still renders (partial
degradation is the contract).

## Adding chrome: contributes

```json
"contributes": {
  "navbar":    [{"id": "hello:home", "title": "Hello", "route": "/hello/hi"}],
  "dashboard": [{"id": "hello:panel", "title": "Hello", "route": "/hello/hi"}],
  "statusbar": [{"id": "hello:status", "title": "ok", "route": "/hello/hi"}],
  "routes":    [{"id": "hello:admin", "title": "Admin", "route": "/hello/admin"}]
}
```

Slots merge across drivers by `priority` (lower wins, default
100). Unknown slot names round-trip via `contributes.extra` — safe
to emit slots a given host doesn't know yet.

## The proxy pattern (recommended for real products)

Don't reimplement your product's data layer — translate the wire.
`examples/drivers/myco/` shows the canonical shape:

- `manifest`, `views`, and capability probes are answered **locally**
  — pavo discovers the driver even when the product isn't running.
- `view`/`query` ops resolve each binding through one persistent
  `myco serve` child speaking **JSON-RPC 2.0** (myco's wire is not
  pavo's `{"op":…}` envelope — translation is the driver's job).
- If the product is absent, return `ok:false` with a clear error;
  the host degrades that binding, not the whole chrome.

The production versions live in each product repo
(`tamias-driver-kuda`, `tamias-driver-myco`, `tamias-driver-cuora`).

## Stay compatible

- **Additive-only**: new optional fields and new `kind` variants may
  appear; never change existing spellings. Renderers ignore unknown
  kinds (they degrade to KV) — but don't rely on that for your own
  core UX.
- **`node`/`props` are open**: put renderer hints in `props`; hosts
  pass them through untouched.
- **Test against the contract**: `pavo-testkit`'s `check_driver`
  runs the conformance suite over your binary; `FixtureDriver` can
  stand in for your driver in host tests. Golden-fixture guidance:
  `cargo xtask test` runs the example drivers through the suite.

## Reference implementations

| Driver | Shows |
| --- | --- |
| `examples/drivers/demo/` | board (table+kv+markdown), deps `graph`, timeline `tree`; all chrome slots |
| `examples/drivers/myco/` | thin proxy onto `myco serve`'s JSON-RPC wire; degrades when myco is absent |
| `crates/pavo-driver-config/` | first-party Rust driver — the common config API (see [Use the config API](/how-to/use-config-api)) |
