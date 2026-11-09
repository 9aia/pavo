---
url: /pavo/how-to/write-a-renderer-plugin.md
---
# Write a renderer plugin

View-kind renderers are themselves tamias drivers — there is no
separate plugin ecosystem. A driver whose manifest carries
`renderers[]` claims ViewNode kinds on one or more surfaces and
answers the `render` wire op with a canonical
[`RenderModel`](/reference/wire-ops#rendermodel).

Use this when a built-in interpretation isn't good enough —
`examples/drivers/viz-graph/` claims `graph` and renders SVG,
ascii edge lists, and mermaid-free layouts the built-ins don't.

## 1. Declare the claim in the manifest

```json
"renderers": [{
  "id": "graph",
  "kinds": ["graph"],
  "priority": 50,
  "entry": "graph",
  "surfaces": {
    "cli": { "formats": ["svg", "ascii"] },
    "tui": true,
    "web": { "component": "viz-graph", "island": true }
  }
}]
```

* `kinds` — ViewNode `kind` strings, or `["*"]` for a catch-all.
  Explicit kinds beat `"*"`; lower `priority` wins conflicts
  (default 100); ties break by driver name then renderer id — the
  same policy chrome contributions use.
* `surfaces.cli.formats` — the `--format` values you claim;
  **empty = wildcard** (you handle every format asked, including
  `text`). Built-in names are claimable: listing `dot`, `mermaid`,
  or `text` overrides the built-in renderer *for your claimed
  nodes* — unclaimed nodes keep the built-in output. `json` is
  never claimable (the raw `ViewResult` dump is host-owned).
* `surfaces.tui` — `true` to render on the ratatui host.
* `surfaces.web` — `component` is a hint for rich mounts;
  `island: true` marks output needing client-side execution.

A driver may declare renderers **with zero views** — a pure plugin
ships no routes of its own.

## 2. Answer the `render` op

For every node whose kind is claimed on the asking surface, the
host sends:

```
{"op":"render","kind":"graph","node":{…ViewNode verbatim…},
 "data":{…view data map…},"surface":"cli|tui|web","format":"…"?}
```

* `node` is the raw ViewNode JSON — unknown/future fields reach you
  untouched.
* `data` is the view's query-binding map verbatim — resolve
  `node.query` against it yourself.
* `surface` is `"cli"`, `"tui"`, or `"web"` (open string — hosts may
  add surfaces).
* `format` is the surface's format hint (`"svg"`, `"ascii"`, …)
  when one applies.

Respond with a `RenderModel` (`{"ok":true,"result":{…}}`):

| variant       | shape                                        | CLI            | TUI          | web              |
|---------------|----------------------------------------------|----------------|--------------|------------------|
| `rows`        | `{type,headers,aligns?,rows}`                | aligned table  | table rows   | `<table>`        |
| `svg`         | `{type,markup}` — self-contained `<svg>`     | raw markup     | `[svg]` hint | `v-html` island  |
| `text`        | `{type,lines}`                               | lines          | lines        | `<pre>`          |
| `placeholder` | `{type,component,props?}`                    | `[component]`  | box + detail | named component  |

Rules:

* **Built-ins are not reserved** — claim `graph` (or any kind) and
  you override built-in rendering on the surfaces you declare.
* **Fallback is mandatory** — when no plugin claims a kind+surface,
  or `render` errors/times out, hosts use the built-in
  interpretation. Plugin failure must never break a view.
* **No HTML in RenderModel** — self-contained markup is `svg` only.
* Unknown `type` values degrade like `placeholder` on every
  surface — keep to the four variants.

## 3. Test it

```bash
export PATH="$PWD/examples/drivers/viz-graph:$PATH"
pavo drivers                          # viz-graph … renderers:1
pavo view demo:deps --format svg      # plugin SVG (claims svg+ascii)
pavo view demo:deps --format mermaid  # built-in — plugin doesn't claim it
pavo view demo:deps --format ascii    # plugin edge list
```

`/api/renderers` on `pavo serve` lists every claim;
`/api/render` proxies a `render` op for the web surface's plugin
path. `pavo-testkit` conformance covers drivers that answer
`render` — see `check_driver`.

## Reference

`examples/drivers/viz-graph/tamias-driver-viz-graph` is the whole
implementation (~200 lines of Python): manifest claim, `render`
dispatch on `kind`/`surface`/`format`, three RenderModel shapes.
Copy it.
