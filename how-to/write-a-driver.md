---
url: /pavo/how-to/write-a-driver.md
---
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
data. `{"op":"submit","view":"…","form":"…"?","values":{…}}` →
a `SubmitResult` backs interactive `form` nodes (see
[Wire ops → submit](/reference/wire-ops#submit)). Drivers that feed
the platform — job queues, notification streams — also answer
`{"op":"jobs"}` and `{"op":"notifications"}` (see
[Platform feeds](#platform-feeds-jobs--notifications) below).
Every response is `{"ok":true,"result":…}` or
`{"ok":false,"error":"…"}`. Full spec:
[Wire ops](/reference/wire-ops).

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
  "routes":    [{"id": "hello:admin", "title": "Admin", "route": "/hello/admin"}],
  "sidebar":   [{"id": "hello:nav", "title": "Hello", "route": "/hello/hi"}],
  "headerActions": [{"id": "hello:quick", "title": "Quick", "route": "/hello/hi"}],
  "notifications": [{"id": "hello:feed", "title": "Alerts", "route": "/hello/hi"}],
  "jobs":      [{"id": "hello:queue", "title": "Jobs", "route": "/hello/jobs"}]
}
```

The eight built-in slots: `navbar`, `dashboard`, `statusbar`,
`routes`, `sidebar`, `headerActions`, `notifications`, and `jobs`
(wire spelling is camelCase for `headerActions`). Slots merge
across drivers by `priority` (lower wins, default 100). The
`notifications`/`jobs` entries are *display metadata* — the feed
data itself flows through the matching wire ops (see
[Platform feeds](#platform-feeds-jobs--notifications)). Unknown
slot names round-trip via `contributes.extra` — safe to emit slots
a given host doesn't know yet. Full field reference:
[Driver manifest → contributes](/reference/driver-manifest#contributes).

## Platform feeds: jobs + notifications

Two optional wire ops let a driver publish live feeds the host
aggregates across all providers:

```json
{"op":"jobs"}           → Vec<JobItem>
{"op":"notifications"}  → Vec<NotificationItem>
```

Declaring the `contributes.jobs` / `contributes.notifications` slot
marks the driver as a *provider*; the host calls the matching op on
every provider and merges the results (each item attributed to its
driver). Per-driver failures skip that driver — one broken feed
never takes down the rest.

`JobItem` is `{id, title, status, detail?, route?, updated_at?}`;
`NotificationItem` is `{id, title, body?, severity?, route?,
created_at?, read?}`. Timestamps are RFC 3339 strings; `status`/
`severity` are **driver-owned vocabularies** — hosts render them
verbatim or degrade to neutral styling. `route` resolves like a
contribution route (deep link or host path). Drivers without the
feed may answer `ok:false` or an empty list — hosts tolerate both.
Full shapes: [Wire ops → Types](/reference/wire-ops#types).

## Declaring deps and cross-driver queries

Two manifest fields let drivers compose with each other:

```json
"deps": {"requires": ["config"], "optional": ["myco"]}
```

`deps.requires` names drivers this driver can't function without —
a missing one quarantines the driver (it errors on every manifest/
view/feed access rather than half-rendering). `deps.optional` names
drivers used when present; absence degrades gracefully. The host
topologically sorts drivers so deps come first; cycles are errors.

A [`QueryBinding`](/reference/driver-manifest) may also name a
`driver` — a **cross-driver query**: the host resolves the binding
against *that* driver's `serve` and merges the payload into the
view's `data` under the binding name, so your tree can't tell the
difference. Failures degrade to `{"error": "…"}` in that binding's
slot. `examples/drivers/platform-demo/` exercises all of this.

## Rust drivers — pavo-driver-kit

A driver can be any language — the wire is ndjson JSON. For Rust,
**`pavo-driver-kit` is the recommended way to write one**: it
absorbs the boilerplate every driver re-implements — the ndjson
`serve` loop, `ok`/`err` response plumbing, and fluent builders for
the manifest, views, query bindings, contributions, ViewNode trees,
and platform feed items. A driver's UI surface becomes pure
declaration:

```toml
pavo-protocol   = "0.1"   # crates.io — the wire contract types
pavo-driver-kit = { git = "https://github.com/9aia/pavo" }  # not yet on crates.io
```

```rust
use pavo_driver_kit as kit;
use pavo_protocol::wire::{Request, Response};

fn main() {
    let manifest = kit::Manifest::new("hello", "Hello")
        .version(env!("CARGO_PKG_VERSION"))
        .view(kit::view("hi", "Hello", "/hello/hi",
            kit::markdown(kit::base().title("Hi"), "Hello from **pavo**!")))
        .sidebar(kit::contribution("home", "Hello", "/hello/hi"))
        .build();
    kit::serve_stdio(|req| match req {
        Request::Manifest => Response::ok_typed(&manifest),
        Request::Views   => Response::ok(serde_json::json!([])),
        _                => Response::err("unsupported op"),
    }).ok();
}
```

What the kit covers:

* **`serve::serve` / `serve::serve_stdio`** — the request/response
  ndjson loop (one `{"op":…}` in, one `{"ok":…}` out per line);
  `serve` takes any `Read`/`Write` pair for tests.
* **`Manifest`** — fluent `DriverManifest` builder: `.view(…)`,
  `.navbar/.dashboard/.statusbar/.routes/.sidebar/
  .header_actions/.notifications/.jobs(…)` slot pushes,
  `.requires(…)`/`.optional(…)` dep declarations, `.renderer(…)`.
* **Query bindings** — `kit::query(name, op)` for own-driver
  bindings, `kit::foreign_query(driver, name, op)` /
  `kit::binding_on(driver, …)` for cross-driver ones.
* **`node`** — `ViewNode` constructors (`panel`, `row`, `column`,
  `table`, `tree`, `graph`, `form`, `kv`, `markdown`, `embed`, …)
  plus `kit::base()` for the shared `id`/`title`/`query`/
  `description`/`props` fields.
* **`platform::job` / `platform::notification`** — `JobItem` /
  `NotificationItem` constructors for the feed ops.

All kit types are `pavo-protocol`'s — it adds constructors and the
loop, never a parallel type hierarchy. Anything the kit doesn't
cover, write against `pavo-protocol` directly (or hand-roll the
loop — the wire is the contract, not the kit). `pavo-testkit`
(conformance suite) is likewise not yet published — depend on it
by git like the product drivers do. See
`crates/pavo-driver-config/` for a first-party driver (it predates
the kit and shows the hand-rolled equivalent) and the product
drivers (myco, kuda, cuora, sturn, mimus, tamias, bupha, sepia)
for kit-based examples.

## The proxy pattern (recommended for real products)

Don't reimplement your product's data layer — translate the wire.
`examples/drivers/myco/` shows the canonical shape:

* `manifest`, `views`, and capability probes are answered **locally**
  — pavo discovers the driver even when the product isn't running.
* `view`/`query` ops resolve each binding through one persistent
  `myco serve` child speaking **JSON-RPC 2.0** (myco's wire is not
  pavo's `{"op":…}` envelope — translation is the driver's job).
* If the product is absent, return `ok:false` with a clear error;
  the host degrades that binding, not the whole chrome.

The production versions live in each product repo
(`tamias-driver-kuda`, `tamias-driver-myco`, `tamias-driver-cuora`).

## Stay compatible

* **Additive-only**: new optional fields and new `kind` variants may
  appear; never change existing spellings. Renderers ignore unknown
  kinds (they degrade to KV) — but don't rely on that for your own
  core UX.
* **`node`/`props` are open**: put renderer hints in `props`; hosts
  pass them through untouched.
* **Test against the contract**: `pavo-testkit`'s `check_driver`
  runs the conformance suite over your binary; `FixtureDriver` can
  stand in for your driver in host tests. Golden-fixture guidance:
  `cargo xtask test` runs the example drivers through the suite.

## Reference implementations

| Driver | Shows |
| --- | --- |
| `examples/drivers/demo/` | board (table+kv+markdown), deps `graph`, timeline `tree`; all chrome slots |
| `examples/drivers/platform-demo/` | the platform host end-to-end — sidebar/headerActions slots, `deps`, a cross-driver `QueryBinding.driver`, and canned `jobs`/`notifications` feeds |
| `examples/drivers/myco/` | thin proxy onto `myco serve`'s JSON-RPC wire; degrades when myco is absent |
| `crates/pavo-driver-config/` | first-party Rust driver — the common config API (see [Use the config API](/how-to/use-config-api)); predates `pavo-driver-kit` |
| product repos (`tamias-driver-myco`, `-kuda`, `-cuora`, `-sturn`, `-mimus`, `-tamias`, `-bupha`, `-sepia`) | production drivers migrated onto `pavo-driver-kit` |
