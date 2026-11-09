# Boundaries

Pavo owns **visualization hosting** — driver discovery, ViewNode
interpretation, contribution-slot merge, renderer-plugin dispatch,
and the three renderers (CLI, TUI, web). Neighbors own theirs:

- **tamias** — distribution. Drivers are tamias content packages:
  executables named `tamias-driver-<type>` found on `PATH` or in
  `<tamias-home>/drivers/`. Pavo never re-implements transport or
  content types; it only *executes* what tamias installed.
- **products** (myco, kuda, sturn, mimus, cuora, …) — state owners.
  They ship drivers that proxy their existing `serve` wires (see
  the [proxy pattern](/how-to/write-a-driver#the-proxy-pattern-recommended-for-real-products)).
  Pavo never reads product internals — no myco files, no kuda
  sqlite, no store schemas. If a driver needs product state, the
  product's own `serve` is the API.
- **cuora** — secrets. Driver credentials via `cuora://` refs;
  pavo never sees values.
- **rampho** — the `SceneNode → ViewNode` precedent: media render
  pipelines are rampho's, UI trees are pavo's.
- **peregrin** — orchestration. Multi-actor concerns (who may view
  what, from where) are policy; pavo serves one localhost user.
  Note: peregrin's `tamias-driver-peregrin` is a tamias *content*
  driver for bundle install/deploy — it does not yet serve pavo
  views.
- **sepia** — session lifecycle; unrelated to rendering.

## What pavo deliberately doesn't do

- **No plugin system beyond drivers.** "Files, not frameworks" —
  a driver is an executable on `PATH`; renderer plugins are the
  same mechanism with a `renderers[]` manifest claim.
- **No product semantics.** ViewNode kinds are generic
  (`table`, not `issue-list`); meaning lives in the driver's
  queries and labels. The config API is namespaced dotted keys,
  never file paths.
- **No host-side writes.** `form` nodes submit through the
  driver's own `submit` wire op — pavo hosts collect field values
  and display the `SubmitResult`, but every mutation semantic
  (what `submit_op` means, what gets written) belongs to the
  driver. Drivers stay proxies; pavo never touches product state.
- **No daemon.** `pavo serve` is a per-user localhost bridge bound
  to `127.0.0.1`, not a service. Multi-user or remote hosting
  would be a different tool.
- **No HTML from drivers.** The IR is declarative; `svg` is the
  only self-contained markup a plugin may emit. Anything richer is
  `placeholder`/`embed` resolved host-side.
