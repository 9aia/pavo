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
   capabilities[], contributes, views[]}`. Discoverable without
   spawning: pavo lists routes and chrome contributions straight
   from the manifest.
2. **ViewNode IR** — internally-tagged `"kind"` nodes:
   `panel`, `table`, `tree`, `graph`, `form`, `kv`, `markdown`,
   `embed`, `row`, `column`. Every kind flattens the shared
   `NodeBase` fields (`id`, `title`, `query`, `description`,
   `props`) to the top level. `embed` is the escape hatch for rich
   renderers.
3. **Wire ops** — one JSON object per line:
   requests `{"op":"manifest|views|view|query", …}`, responses
   `{"ok":true,"result":…}` / `{"ok":false,"error":"…"}`. Same
   convention as `tamias-serve`, `myco serve`, `cuora serve`,
   `sepia serve`.
