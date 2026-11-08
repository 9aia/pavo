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
   capabilities[], contributes, views[], renderers[]}`.
   Discoverable without spawning: pavo lists routes and chrome
   contributions straight from the manifest. `renderers[]` declares
   view-kind renderer plugins (below) — a driver may declare
   renderers with zero views.
2. **ViewNode IR** — internally-tagged `"kind"` nodes:
   `panel`, `table`, `tree`, `graph`, `form`, `kv`, `markdown`,
   `embed`, `row`, `column`. Every kind flattens the shared
   `NodeBase` fields (`id`, `title`, `query`, `description`,
   `props`) to the top level. `embed` is the escape hatch for rich
   renderers.
3. **Wire ops** — one JSON object per line:
   requests `{"op":"manifest|views|view|query|render", …}`,
   responses `{"ok":true,"result":…}` / `{"ok":false,"error":"…"}`.
   Same convention as `tamias-serve`, `myco serve`, `cuora serve`,
   `sepia serve`.

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
