# tamias-driver-viz-graph

The reference **renderer-plugin** driver — it renders view *kinds*,
not views. Its manifest declares zero views and one `renderers`
entry claiming the `graph` kind on every pavo surface:

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

Any view containing a `graph` node — from any driver — is routed to
this process through the `render` wire op:

```
{"op":"render","kind":"graph","node":{…ViewNode JSON…},
 "data":{…view data bindings…},"surface":"cli|tui|web",
 "format":"svg|ascii|…"}  →  {"ok":true,"result":<RenderModel>}
```

## RenderModel mapping

| surface / format | model returned |
|---|---|
| `cli --format svg` | `svg` — raw markup printed to stdout |
| `cli --format ascii` | `text` — edge list |
| `cli` (plain `text`) | built-in renderer (plugin's cli formats don't claim `text`) |
| `tui` | `rows` — node table (id/label/kind) |
| `web` | `svg` — island markup |

## Payload

The node's `query` field names a binding in the view `data` map —
`{"nodes":[{id,label,kind}],"edges":[{from,to,label}]}` — or
`node.nodes`/`node.edges` inline. Layout is deterministic: nodes
evenly spaced on an ellipse ordered by id, high-degree nodes pushed
slightly outward. Comfortable to ~50 nodes.

## Running

Renderer-only drivers declare no views; install it next to a view
driver (e.g. `demo`) and the host picks it up automatically:

```bash
export PATH="$PWD/examples/drivers/viz-graph:$PATH"
pavo view demo:deps --format svg     # plugin SVG
pavo view demo:deps --format ascii   # plugin edge list
pavo view demo:deps                  # built-in text (unchanged)
```

Stdlib only — `python3 tamias-driver-viz-graph serve` speaks ndjson.
Table/list renderers are deliberately out of scope (future plugins).
