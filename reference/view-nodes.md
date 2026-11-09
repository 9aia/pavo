---
url: /pavo/reference/view-nodes.md
---
# ViewNode IR

The declarative UI tree a driver emits and every pavo renderer
(CLI, TUI, web) interprets. Wire shape: each node is a JSON object
with a `"kind"` discriminator — internally tagged, no wrapper
envelope. Rust source of truth:
`crates/pavo-protocol/src/view_node.rs`;
`protocol/v1/view-node.schema.json` mirrors it.

```json
{ "kind": "table", "title": "Board", "query": "items",
  "columns": [{ "key": "id", "title": "ID" }],
  "rows": [["T-001", "pavo scaffold"]] }
```

## Shared fields (NodeBase)

Every kind flattens these to the top level:

| field | type | meaning |
| --- | --- | --- |
| `id` | string? | stable id — deep-link target, diffing, expansion state |
| `title` | string? | heading the renderer may show above the node |
| `query` | string? | names a `QueryBinding` on the enclosing `ViewSpec`; kind-specific meaning (`table`→`rows`, `kv`→`pairs`, `tree`→`items`, `graph`→`nodes`/`edges`) |
| `description` | string? | longer help text (tooltips, help panes) |
| `props` | object | renderer hints (`density`, `direction`, `collapsed`, …); renderers ignore unknown keys |

## Kinds

| `kind` | purpose | key fields |
| --- | --- | --- |
| `panel` | titled container grouping children | `children[]` |
| `row` | horizontal layout | `children[]`, `gap?` |
| `column` | vertical layout | `children[]`, `gap?` |
| `table` | tabular data | `columns[]` (`key`,`title`,`align?`,`width?`), `rows?` (inline cell arrays win over query data), `sortable?` |
| `tree` | hierarchical items | `items[]` (`id?`,`label`,`detail?`,`icon?`,`children[]`,`props`), `expandable?` |
| `graph` | nodes + edges; layout is the renderer's problem | `nodes[]` (`id`,`label`,`kind?`,`props`), `edges[]` (`from`,`to`,`label?`), `layout?` (`"dot"`,`"mermaid"`,`"layered"`,…) |
| `form` | input fields + submit op — **interactive** via the `submit` wire op | `fields[]`, `submit_op?`, `submit_label?` |
| `kv` | key/value pairs — detail panes, metadata, **universal degradation target** | `pairs[]` (`key`,`value`,`kind?`) |
| `markdown` | free-form markdown | `text` |
| `embed` | escape hatch for rich renderers | `component` (registry/web-component name); component props ride in `props` |

### Field details

* **`table.columns[].key`** — for query-fed data, rows arrive as
  objects keyed by `key`; inline `rows` are positional arrays of
  arbitrary JSON cells (renderers stringify).
* **`table.rows`** — inline rows ship static previews; when both
  inline rows and `query` data exist, inline wins.
* **`tree.items`** — recursive; `id` is required for interactive
  trees, optional for static previews. Omitted `items` (vs `[]`)
  matters on the typed round-trip so an empty inline list can't
  shadow query-bound data.
* **`graph`** — the IR floor covers structure only. Real layout is
  an `embed`/renderer-plugin concern; `layout` is a hint renderers
  may ignore.
* **`form.fields[].kind`** — `text` (default), `textarea`, `number`,
  `bool`, `select` (with `options`). `required`, `default`, `value`,
  `hint`, `editable` (default `true`; `false` renders read-only and
  is never submitted), and `placeholder` are supported. Submission
  goes through the [`submit` op](/reference/wire-ops#submit); the
  form node's `id` is the `form` param when a view has several
  forms, and `submit_op` names the driver-side action (`"set"`,
  `"update"`, `"add"`, …).
* **`embed`** — for visuals no built-in model covers (live graphs,
  timelines, WASM). `component` resolves against the host's
  renderer registry and, on web, registered components.

## Degradation is mandatory

Renderers that meet an unknown `kind` must degrade to generic
key/value rendering rather than failing — forward compatibility is
a host obligation, and the protocol is additive-only within v1. The
same applies to `props` keys and enum values.
