# Renderer plugins

Built-in rendering is the floor, not the ceiling. A driver can ship
a **renderer plugin** — a claim on ViewNode *kinds* plus a `render`
op that returns a canonical `RenderModel` — and pavo routes matching
nodes to it instead of interpreting them natively.

There is no separate plugin ecosystem: plugins are tamias drivers.
`examples/drivers/viz-graph/` claims `graph` on all three surfaces
and renders SVG/ascii the built-ins don't; its manifest declares
**zero views** — it renders other drivers' trees.

## The two halves

**Manifest half** — `renderers[]` declares the claim:

```json
{"id": "graph", "kinds": ["graph"], "priority": 50,
 "surfaces": {
   "cli": {"formats": ["svg", "ascii"]},
   "tui": true,
   "web": {"component": "viz-graph", "island": true}}}
```

`kinds` lists ViewNode kinds (or `"*"` for catch-all); `surfaces`
says where the claim holds and which `--format` values it takes
(empty `formats` = wildcard). Absent surface = unsupported there.

**Wire half** — the `render` op:

```
→ {"op":"render","kind":"graph","node":{…ViewNode verbatim…},
   "data":{…view bindings…},"surface":"cli","format":"svg"}
← {"ok":true,"result":{"type":"svg","markup":"<svg>…"}}
```

`node` arrives untyped so unknown/future fields survive;
`data` is the same binding map `view` returns, so the plugin
resolves `node.query` itself.

## Claim resolution

Deterministic, same policy as chrome contributions:

1. Explicit kinds beat `"*"`.
2. Lower `priority` wins (default 100).
3. Ties break by driver name, then renderer id.

`pavo`'s `RendererRegistry` (pavo-core `renderers` module) folds
all manifests into this order once at discovery; `/api/renderers`
on `pavo serve` exposes the resolved claims to the web surface.

## RenderModel — the canonical output

Plugins don't ship surface code; they answer one of four tagged
models and each surface maps it natively:

| variant | CLI | TUI | web |
| --- | --- | --- | --- |
| `rows` | aligned table | table rows | `<table>` |
| `svg` | raw markup | `[svg]` hint | `v-html` island |
| `text` | lines | lines | `<pre>` |
| `placeholder` | `[component]` | box + detail | named component |

The set is deliberately small — plugins needing richer output use
`placeholder` (web components) or `svg` (self-contained markup).
**No HTML in RenderModel**: self-contained markup is `svg` only.

## The fallback contract

- **Built-ins are not reserved.** A plugin may claim `graph` (or
  any kind) and override built-in rendering on the surfaces it
  declares.
- **Fallback is mandatory.** When no plugin claims a kind+surface,
  or `render` errors/times out, hosts use the built-in
  interpretation — unknown kinds still degrade to KV. Plugin failure
  must never break a view; that's what makes plugins safe to
  discover on `PATH` alongside everything else.
- **`web.component` is a hint.** v1 renders every RenderModel
  generically; `component`/`island` name rich mounts for hosts that
  resolve them (dynamic-imported components are a v2 concern).

## Why plugins instead of `embed`

`embed` names a component for *one node*; a renderer plugin claims
a *kind* — every `graph` node in every view, from every driver.
Plugins are how `graph` itself migrated: the IR keeps the
structure, the plugin owns the rendering. `table`/`list` plugins
are the expected next migrations.
