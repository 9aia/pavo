---
url: /pavo/reference/wire-ops.md
---
# Wire ops

The ndjson JSON-RPC convention every pavo driver speaks on its
`serve` verb — the same framing as `tamias-serve`, `myco serve`,
`cuora serve`, `sepia serve`. One JSON object per line on stdin,
one response per line on stdout.

Rust source of truth: `crates/pavo-protocol/src/wire.rs`;
`protocol/v1/wire.schema.json` mirrors it.

## Envelope

Requests are discriminated by `op` (internally tagged, lowercase):

```json
{"op":"manifest"}
{"op":"views"}
{"op":"view","view":"board","params":{"status":"open"}}
{"op":"query","query":"items","params":{"limit":5}}
{"op":"render","kind":"graph","node":{…},"data":{…},
 "surface":"cli","format":"svg"}
{"op":"submit","view":"edit","form":"set-config",
 "values":{"key":"myco.theme","value":"dark"}}
```

Responses:

```json
{"ok":true,"result":…}
{"ok":false,"error":"…"}
```

Fields for the non-matching arm are absent (not null) — consumers
match on `ok` alone. A driver must never crash the wire: malformed
input and serialization failures become `ok:false` responses.

## Ops

### `manifest`

`{"op":"manifest"}` → [`DriverManifest`](/reference/driver-manifest).
Discovery verb — cheap, must not require product state. This is how
pavo decides a `tamias-driver-*` is a view driver (`"serve"` in
`verbs` or `"pavo"` in `capabilities`).

### `views`

`{"op":"views"}` → `Vec<ViewSummary>` — `{id, title, route,
description?, queries[]}` per view, without the `root` trees.
Lightweight listing for `pavo views` and picker UIs; full trees come
from `view`.

### `view`

`{"op":"view","view":"<id>","params":{…}}` →
[`ViewResult`](/reference/wire-ops#viewresult):

```json
{"view": { …ViewSpec including root tree… },
 "data": { "<binding-name>": <payload>, … }}
```

The host resolves each `QueryBinding` on the spec (calling its `op`
with static params merged under caller `params`) and puts results
in `data` keyed by binding name — `data["items"]` feeds every node
whose `query == "items"`. **Partial degradation**: a binding that
fails appears as `{"error": "…"}` in `data` rather than failing the
whole view.

### `query`

`{"op":"query","query":"<name>","params":{…}}` →
`{"query":"<name>","data":<payload>}` — execute a single named
query binding, or a raw product op for thin drivers. Powers
`pavo query` and ad-hoc data probes; also the transport the
[config API](/how-to/use-config-api) rides (`query` = `get`/`set`/
`resolve`/`sources`/`config`).

### `render`

`{"op":"render","kind":"<kind>","node":{…ViewNode verbatim…},
"data":{…view data map…},"surface":"cli|tui|web","format":"…"?}`
→ [`RenderModel`](/reference/wire-ops#rendermodel) — the
renderer-plugin verb. Only sent to drivers whose manifest claims
the node's kind on the asking surface (see
[Renderer plugins](/explanation/renderer-plugins)).

* `node` — raw ViewNode JSON, passed untyped so unknown/future
  fields survive.
* `data` — the same binding map `view` results carry.
* `surface` — open string; hosts may add surfaces.
* `format` — the surface's format hint when one applies.

### `submit`

`{"op":"submit","view":"<id>","form":"<id>"?,"values":{…}}` →
[`SubmitResult`](#submitresult) — the form-submission verb. A
surface collects the editable fields of a `form` ViewNode and posts
their `key` → value pairs here.

* `view` — the view the form lives in (same namespace as `view`).
* `form` — optional form-node `id` when the view carries several
  forms; absent = the driver's default/first form.
* `values` — submitted field values keyed by `FormField.key`.

The driver resolves the form node, reads its `submit_op`
(driver-defined: `"set"`, `"update"`, `"add"`, …), and answers a
`SubmitResult`. Drivers without form support answer `ok:false`.
`pavo submit <driver>:<view> --param k=v …` is the CLI face;
`GET /api/submit?ref=…&values={…}` the web-bridge face.

## Types

### ViewSummary

`{id, title, route, description?, queries[]}` — a `ViewSpec` minus
`root`, for cheap listing.

### ViewResult

`{view: ViewSpec, data: {<binding>: payload}}` — the resolved view.
Failed bindings carry `{"error": "…"}` payloads.

### QueryResult

`{query: string, data: any}` — one binding's data payload; shape is
driver-defined, hosts pass it through verbatim.

### SubmitResult

`{message?, errors?, data?, view?}` — every field optional; a bare
`{"message":"…"}` is complete. `errors` is a per-field validation
map keyed by `FormField.key` — non-empty means the driver *rejected*
the submit (hosts re-render with the messages; `pavo submit` exits
1\). `view`, when present, is a fresh `ViewResult` to swap in.
Unknown fields survive typed round-trips.

### RenderModel

The canonical intermediate output a renderer plugin emits —
internally tagged on `"type"`:

| `type` | shape | CLI | TUI | web |
| --- | --- | --- | --- | --- |
| `rows` | `{type, headers[], aligns?[], rows[][]}` | aligned table | table rows | `<table>` |
| `svg` | `{type, markup}` — self-contained `<svg>` | raw markup | `[svg]` hint | `v-html` island |
| `text` | `{type, lines[]}` | lines | lines | `<pre>` |
| `placeholder` | `{type, component, props?}` | `[component]` | box + detail | named component |

Rules: no HTML in RenderModel (self-contained markup is `svg`
only); unknown `type` degrades like `placeholder`; plugins needing
richer output use `placeholder` or `svg`.

## Versioning

**Additive-only within v1**: new optional fields, new `op`s, new
`kind`/`type` variants may appear at any time; existing fields and
spellings never change meaning. Unknown ops are protocol errors —
hosts surface the driver's raw error text. Breaking changes bump the
version directory (`protocol/v2/`) with a migration note; v1 stays
frozen for existing drivers.
