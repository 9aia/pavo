---
url: /pavo/reference/driver-manifest.md
---
# Driver manifest

What `tamias-driver-<type> manifest` (or `{"op":"manifest"}` on the
`serve` wire) answers — the discovery contract. Pavo reads it
without spawning `serve`, so routes and chrome contributions are
listable even when the product isn't running.

Rust source of truth: `crates/pavo-protocol/src/manifest.rs`;
`protocol/v1/driver-manifest.schema.json` mirrors it.

```json
{
  "name": "myco",
  "title": "Myco",
  "icon": "🗂️",
  "version": "0.4.0",
  "verbs": ["manifest", "serve"],
  "capabilities": ["pavo"],
  "contributes": { "navbar": [...], "routes": [...],
                   "sidebar": [...], "jobs": [...] },
  "deps": { "requires": ["config"], "optional": ["myco"] },
  "views": [ { "id": "board", "route": "/myco/board", "root": {…} } ],
  "renderers": [ { "id": "graph", "kinds": ["graph"], "surfaces": {…} } ]
}
```

## Fields

| field | type | meaning |
| --- | --- | --- |
| `name` | string | the `<type>` in `tamias-driver-<type>`; namespaces routes (`/<name>/<view>`) |
| `title` | string | human-facing title for chrome (navbar, window titles) |
| `icon` | string? | emoji, icon name, or renderer-resolvable path |
| `version` | string | semver |
| `verbs` | string\[] | wire verbs beyond tamias install verbs; pavo treats a driver as a view driver iff `"serve"` is listed (or `capabilities` contains `"pavo"`) |
| `capabilities` | string\[] | free-form role tags (`"pavo"`, `"config"`, `"chrome"`) |
| `deps` | object? | driver dependency declarations — see below |
| `contributes` | object | chrome slots — see below |
| `views` | ViewSpec\[] | views the driver renders |
| `renderers` | RendererDecl\[] | view-kind renderer plugins — may appear with zero views |

## deps — DriverDeps

```json
{ "requires": ["config"], "optional": ["myco"] }
```

Names are driver names — the `<type>` in `tamias-driver-<type>`
(also `DriverManifest::name`). Both kinds of entry mean the same
thing for ordering: **the dep sorts before the dependent** (the
host topologically sorts its driver list).

* `requires` — drivers this driver cannot function without. A
  missing required dep is a hard error: the host **quarantines**
  the dependent — it stays discovered but every manifest/view/feed
  access surfaces the deps error rather than half-rendering.
* `optional` — drivers used when present. Absent optional deps
  contribute no edge and never block discovery; features degrade.

Cycles (including a driver requiring itself) are host-side errors
and quarantine every member. Drivers whose manifests arrive lazily
are unordered until fetched; their `requires` are validated on
first manifest access. Ordering is deterministic: among satisfied
drivers, name ascending.

## contributes

Named places in pavo's chrome where drivers register entries. The
eight built-in slots:

| slot | lands in |
| --- | --- |
| `navbar` | top navigation bar (web/TUI chrome) |
| `dashboard` | the shared dashboard route — composable panels from many drivers |
| `statusbar` | bottom status bar — one-line summaries |
| `routes` | standalone routes the driver owns outside its views |
| `sidebar` | left sidebar shell — grouped nav entries (web host) |
| `headerActions` | top-bar action buttons — wire spelling is camelCase; icons that open a popover rendering the action's view, or plain navigation (web host) |
| `notifications` | marks the driver as a notification **provider** — entries are display metadata; item data flows through the `notifications` wire op as `NotificationItem`s |
| `jobs` | marks the driver as a job/queue feed **provider** — same pattern; data comes from the `jobs` wire op as `JobItem`s |

Unknown slots are preserved via `contributes.extra` (serde
flatten) — newer drivers degrade gracefully on older hosts.

### Contribution

```json
{"id": "demo:board", "title": "Board", "route": "/demo/board",
 "priority": 100, "icon": "📋"}
```

`id` is namespaced `<driver>:<id>` by convention so contributions
never collide; `route` is host-relative; `priority` orders entries
within a slot (lower wins, default 100, ties by discovery order).

## views — ViewSpec

```json
{"id": "board", "title": "Board", "route": "/demo/board",
 "description": "…",
 "queries": [{"name": "items", "op": "query",
              "params": {"kind": "items"}}],
 "root": { "kind": "panel", "children": [ …ViewNode… ] }}
```

* `id` — unique within the driver; `<driver>:<id>` is the deep link.
* `route` — the host-relative path the view answers.
* `queries` — named data sources the tree references via
  `NodeBase.query`. Each `QueryBinding` has `name` (what nodes
  reference), `op` (the wire op the host calls — typically
  `"query"`), and static `params` merged under caller-supplied view
  params. Optional `driver` makes it a **cross-driver binding**:
  the host issues the op against that driver's `serve` instead of
  the view's own and stores the payload in `data[name]` — failures
  degrade to `{"error": "…"}` like local bindings. A `driver`
  equal to the view's own driver is treated as a local binding.
* `root` — the declarative [`ViewNode`](/reference/view-nodes) tree.

## renderers — RendererDecl

```json
{"id": "graph", "kinds": ["graph"], "priority": 50,
 "entry": "graph",
 "surfaces": {
   "cli":  {"formats": ["svg", "ascii"]},
   "tui":  true,
   "web":  {"component": "viz-graph", "island": true}}}
```

| field | meaning |
| --- | --- |
| `id` | renderer id — namespaced `<driver>:<id>` by convention |
| `kinds` | node kinds claimed — ViewNode `kind` strings or `"*"` catch-all; explicit beats `"*"`, lower `priority` wins, ties by driver name then renderer id |
| `surfaces` | per-surface support; absent surface = unsupported |
| `surfaces.cli.formats` | `--format` values claimed; **empty = wildcard** (all formats incl. `text`) |
| `surfaces.tui` | `true` renders plugin nodes on the ratatui host |
| `surfaces.web.component` / `.island` | component hint; `island: true` = needs client-side execution |
| `priority` | ordering hint, default 100 |
| `entry` | optional entry-point hint (reserved; hosts currently always use the wire) |

Claims are canonical per node on their declared surfaces — a plugin
listing a built-in `--format` name (`text`/`dot`/`mermaid`) overrides
it for claimed nodes (`json` stays host-owned); unclaimed
kinds/surfaces keep built-in rendering, and plugin failure degrades
to it. See [Write a renderer plugin](/how-to/write-a-renderer-plugin)
and [Renderer plugins](/explanation/renderer-plugins).
