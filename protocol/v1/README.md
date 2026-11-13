# pavo protocol v1

The wire contract between **pavo** (the viz host) and **tamias
drivers** (view providers). Three artifacts, all versioned together:

| File | Contract | Rust source of truth |
| --- | --- | --- |
| `driver-manifest.schema.json` | What `tamias-driver-<type> manifest` answers | `crates/pavo-protocol/src/manifest.rs` |
| `view-node.schema.json` | The declarative UI tree (`ViewNode` IR) | `crates/pavo-protocol/src/view_node.rs` |
| `wire.schema.json` | ndjson request/response ops | `crates/pavo-protocol/src/wire.rs` |

Design context: `project/project/proposals/proposal-0003-*` and
`project/project/notes/pavo-design.md` in the org `project` repo.

## Versioning policy

- **Additive-only within v1.** New optional fields, new `kind`
  variants, new `op`s, and new contribution slots may appear at any
  time; existing fields and variant spellings never change meaning.
- **Unknown fields are ignored** by hosts and drivers — forward
  compatibility is mandatory, not optional. Renderers that meet an
  unknown `kind` must degrade to generic KV rendering.
- **Breaking changes bump the version directory** (`protocol/v2/`)
  with a migration note; v1 stays frozen for existing drivers.
- The Rust types in `pavo-protocol` are canonical; the JSON Schemas
  are hand-maintained mirrors kept honest by review (a schema-vs-Rust
  drift check is planned for `pavo-testkit`).

## The three contracts

1. **Driver manifest** — `{name, title, icon?, version, verbs[],
   capabilities[], deps?, contributes, views[], renderers[]}`.
   Discoverable without spawning: pavo lists routes and chrome
   contributions straight from the manifest. `renderers[]` declares
   view-kind renderer plugins (below) — a driver may declare
   renderers with zero views. `deps` declares driver dependencies
   (`requires`/`optional`, below); `contributes` fills the chrome
   slots — the original four (`navbar`, `dashboard`, `statusbar`,
   `routes`) plus the platform slots `sidebar`, `headerActions`,
   `notifications`, `jobs`, and `menus` (all additive; old
   manifests that put these keys in the forward-compat `extra`
   map still parse).
2. **ViewNode IR** — internally-tagged `"kind"` nodes:
   `panel`, `table`, `tree`, `graph`, `form`, `kv`, `markdown`,
   `embed`, `row`, `column`. Every kind flattens the shared
   `NodeBase` fields (`id`, `title`, `query`, `description`,
   `props`, `actions`) to the top level. `embed` is the escape
   hatch for rich renderers.
3. **Wire ops** — one JSON object per line:
   requests `{"op":"manifest|views|view|query|render|submit|invoke|
   jobs|notifications", …}`, responses `{"ok":true,"result":…}` /
   `{"ok":false,"error":"…"}`.
   Same convention as `tamias-serve`, `myco serve`, `cuora serve`,
   `sepia serve`.

## Interactive forms (`submit`)

`form` ViewNodes are interactive: fields marked `editable` (default
`true`; `false` renders read-only and is never submitted) collect
user input, and surfaces post it with the `submit` op:

```
{"op":"submit","view":"edit","form":"set-config",
 "values":{"key":"myco.theme","value":"dark"}}
→ {"ok":true,"result":{"message":"set myco.theme",
    "data":{…},"errors":{},"view":{…optional fresh ViewResult…}}}
```

- `view` — the view id the form lives in (same namespace as `view`).
- `form` — optional; the form node's `id` (`NodeBase.id`) when the
  view carries several forms. Absent = the driver's default/first
  form.
- `values` — field `key` → submitted JSON value.

The driver resolves the form node, reads its `submit_op`
(`"set"`, `"update"`, `"add"`, … — driver-defined), and answers a
[`SubmitResult`](#): `message` (status line), `errors` (per-field
validation failures — non-empty means *rejected*, hosts should
re-render, not navigate), `data` (driver payload), `view` (optional
updated `ViewResult` to swap in). Drivers without form support
answer `ok:false` — unknown ops were already protocol errors, so
`submit` joins the v1 op set additively.

## Interaction actions (`invoke`)

`submit` covers forms; **`invoke` is the general action verb** —
the protocol's answer to "any interaction a CLI/TUI would offer":

```
{"op":"invoke","view":"item","action":"todo.done",
 "target":{"id":"T-012"},"params":{"note":"shipped"}}
→ {"ok":true,"result":{"message":"done","errors":{},
    "data":{…},"view":{…optional fresh ViewResult…}}}
```

- `view` — optional view-id context (node/item-scoped actions
  resolve against it; manifest-global actions may omit it).
- `action` — the action's `id`.
- `target` — opaque JSON the driver interprets: the selected row's
  raw value, an item id, …
- `params` — `ParamSchema.name` → submitted value.

The result is [`ActionResult`](#): `message`, per-param `errors`
(non-empty = rejected, re-render don't navigate), `data`, and an
optional fresh `view` to swap in — the same refresh-after-mutation
contract `submit` uses.

### `Action` — the declaration

One type, three declaration sites:

- **`DriverManifest.actions[]`** — driver-global verbs.
- **`NodeBase.actions[]`** — scoped to any ViewNode (inherited by
  all kinds).
- **`TreeItem.actions[]`** — per-row actions (trees are the only
  item type with a stable `id` today).

```json
{"id":"todo.done","label":"Mark done","icon":"check",
 "danger":"confirm","keys":["dd","D"],"target":"item",
 "params":[{"name":"id","kind":"ref","required":true,
            "ref_query":"items"},
           {"name":"note","kind":"text"}]}
```

- `danger`: `none` (fire immediately) | `confirm` (dialog/modal;
  CLI `--yes`/TTY prompt) | `type-target` (type the target's id —
  the destructive-op pattern). Ops-derived actions from
  `mutates:true` ops default to `confirm`.
- `keys`: keyboard shortcuts — TUI binds them; other hosts may
  map or ignore.
- `params`: [`ParamSchema`](#) — typed inputs
  (`string|text|number|integer|bool|enum|date|ref|string[]`) with
  `required`/`pattern`/`min`/`max`/`options`/`default`/`hint`.
  `ref` kind names a `ref_query` binding whose data populates a
  picker. **Hosts auto-generate forms from these** — hand-authored
  `form` nodes remain for custom UX.
- `target`: a context hint (`"item"`, `"row"`, …); the actual
  context rides the invoke request's `target`.
- `driver`: reserved cross-driver target (same convention as
  `QueryBinding.driver`); v1 hosts may treat the action as local.

### Menus — the command palette

`contributes.menus[]` declares palette/quick-launch entries:

```json
{"id":"new-task","title":"New task","icon":"plus",
 "action":"todo.add","keywords":["create","todo"],
 "section":"myco","priority":10}
```

An entry either `action`s (invoke, resolved like the wire op —
`"<action-id>"` or `"<driver>:<action-id>"`) or `route`s (navigate,
same as `Contribution.route`). `keywords` feed host-side fuzzy
matching (dmenu-style). Hosts also synthesize palette entries from
declared `actions[]` and ops-derived actions — `menus[]` is for
curated grouping/ordering/aliases, not a requirement.

### Ops-derived actions (zero-declaration fallback)

Drivers whose ops manifest params carry schemas get actions +
auto-forms for free: `mutates:true` ops → `danger:confirm`
actions, param kinds → `ParamSchema`s. The derivation lives in
pavo-core/driver-kit, not the wire — drivers only enrich their
ops-manifest `params` with schema metadata.

## Platform feeds (`jobs` / `notifications`)

Two additive v1 ops let drivers act as **platform-feed providers** —
the plugin-architecture extension point where "slots are the
extension point, ops are the capability":

```
{"op":"jobs"}           → Vec<JobItem>
{"op":"notifications"} → Vec<NotificationItem>
```

- A driver opts in by declaring the matching `contributes` slot:
  `jobs` or `notifications` (entries are display metadata — label,
  icon, ordering — same `Contribution` shape as every slot).
- The op returns the feed items (`wire.schema.json` `jobItem` /
  `notificationItem`; Rust: `pavo_protocol::platform`). Fields are
  deliberately minimal: stable `id`, `title`, free-form
  `status`/`severity`, RFC 3339 timestamp strings
  (`updated_at`/`created_at`), optional `detail`/`body`, optional
  `route` for "open" navigation, and `read` (notifications only).
  Unknown fields are ignored — same forward-compat rule as
  everywhere else.
- Both ops are **optional capabilities**: a driver without the feed
  answers `ok:false` (or `[]`); hosts treat either as "no feed".

### Driver deps (`deps`)

`DriverManifest.deps` declares cross-driver dependencies — names are
driver names (the `<type>` in `tamias-driver-<type>`):

```json
"deps": { "requires": ["config"], "optional": ["myco"] }
```

`requires` absent at discovery = **error** (the driver can't
function); `optional` absent = **degrade gracefully**. This mirrors
the product `requiredDeps`/`optionalDeps` semantics. Ordering —
topological sort over the dep graph — is a host concern deferred to
a later phase; v1 only carries the declaration.

### Cross-driver query bindings (`QueryBinding.driver`)

A `queries[]` binding may name a target driver:

```json
{ "name": "ledger", "op": "query", "driver": "myco",
  "params": { "product_op": "status" } }
```

`driver` set = the host issues the op against *that* driver's
`serve` (cross-driver data flow — the "host is the bus" half of the
architecture). Host-side resolution is a later phase; the field is
data the host understands, and v1 hosts may ignore it (treat the
binding as local).

## Renderer plugins

View-kind renderers are themselves tamias drivers — there is no
separate plugin ecosystem. A driver whose manifest carries
`renderers[]` claims ViewNode kinds on one or more surfaces:

```json
"renderers": [{
  "id": "graph",
  "kinds": ["graph"],          // or ["*"] for a catch-all
  "priority": 50,              // lower wins conflicts (default 100)
  "entry": "graph",            // optional tooling hint
  "surfaces": {
    "cli": { "formats": ["svg", "ascii"] },  // empty = all formats
    "tui": true,
    "web": { "component": "viz-graph", "island": true }
  }
}]
```

Claim resolution is deterministic: explicit kinds beat `"*"`,
lower `priority` wins, ties break by driver name then renderer id —
the same policy chrome contributions use.

For every node whose kind is claimed on the asking surface, the
host sends the driver:

```
{"op":"render","kind":"graph","node":{…ViewNode verbatim…},
 "data":{…view data map…},"surface":"cli|tui|web","format":"…"?}
```

and expects a **RenderModel** back — the canonical intermediate
output (`wire.schema.json` `renderModel`):

| variant       | CLI            | TUI          | web              |
|---------------|----------------|--------------|------------------|
| `rows`        | aligned table  | table rows   | `<table>`        |
| `svg`         | raw markup     | `[svg]` hint | `v-html` island  |
| `text`        | lines          | lines        | `<pre>`          |
| `placeholder` | `[component]`  | box + detail | named component  |

Rules:

- **Built-ins are not reserved.** A plugin may claim `graph` (or any
  kind) and override built-in rendering on the surfaces it declares.
  `graph` is the reference migration; `table`/`list` plugins are
  future work.
- **Plugin claims take precedence, per node.** On every surface, a
  node whose kind+surface is claimed renders via the plugin's
  RenderModel — including `cli` format names that collide with
  built-ins (`text`, `dot`, `mermaid` are claimable via
  `cli.formats`); unclaimed nodes keep the built-in interpretation.
  The one exception is `--format json`: the raw `ViewResult` dump is
  host-owned and never routed to plugins.
- **Fallback is mandatory.** When no plugin claims a kind+surface, or
  the `render` op errors/times out, hosts use the built-in
  interpretation (unknown kinds still degrade to KV). Plugin failure
  must never break a view.
- **`node` is verbatim.** Unknown/future ViewNode fields reach the
  plugin untouched — forward compatibility applies to renderers too.
- **`web.component`** is a v2 hint for dynamic-imported rich
  components; v1 renders every RenderModel generically.
- **No HTML in RenderModel.** Self-contained markup is `svg` only.

Reference implementation: `examples/drivers/viz-graph/`.

## Writing a driver

Rust drivers should not hand-roll the wire: **`pavo-driver-kit`**
(`crates/pavo-driver-kit`) is the authoring SDK — the ndjson `serve`
loop every sibling driver duplicates (`serve`/`serve_stdio`), fluent
builders for `DriverManifest`/`ViewSpec`/`QueryBinding`/`Contribution`
(including `deps` and cross-driver `QueryBinding.driver`
bindings), `ViewNode` constructors for every kind, and
`JobItem`/`NotificationItem` helpers for the platform feeds. Its
crate-level docs are the reference; this README stays the *contract*.

Non-Rust drivers implement the same contract directly — the Python
examples under `examples/drivers/` are the reference
(`tamias-driver-platform` shows the platform slots end-to-end).

## The config API (driver-level convention)

`tamias-driver-config` (`crates/pavo-driver-config`) is the suite's
**common config API** — not part of the v1 wire contract itself, but a
convention layered on the `query` op. Drivers that need product or
host config spawn it over `serve` and issue:

- `query get`    `{key: "myco.theme"}` → `{key, value, found}`
- `query set`    `{key, value}` → writes the project-level file
- `query resolve` → the full merged config tree
- `query sources` → every contributing layer (provenance)

Keys are **namespaced dotted paths** (`myco.theme`, `kuda.index`) —
callers never read config files directly. Layers, low→high:
built-in defaults < `~/.config/pavo/` (global) < `pavo.toml`/
`pavo.json` in cwd ancestors (project, nearest wins) <
`--config`/`$PAVO_CONFIG` < `PAVO_CONFIG_*` env overrides. TOML +
JSON only (no YAML dep in the workspace tree). Full contract:
`crates/pavo-driver-config/README.md`.
