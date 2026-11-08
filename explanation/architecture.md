# Architecture

Pavo is the visualization **host** for the 9aia org's tool suite —
the factory's screen. Named for *Pavo* — the peafowl genus; the
display animal.

The core idea: **products ship drivers, not UIs.** A product
(kuda, myco, cuora, …) ships a `tamias-driver-<type>` executable
that speaks an ndjson JSON-RPC wire and emits a declarative
**ViewNode IR** — a tree of typed nodes (`panel`, `table`, `tree`,
`graph`, `form`, `kv`, `markdown`, `embed`, `row`, `column`) plus
view-model data. Pavo's renderers — CLI, TUI, web — interpret those
trees natively. There is no `<product> ui` per product; pavo is the
single entry point, and a driver installed once visualizes on every
surface.

```
tamias-driver-* (serve, stdio)          pavo host
┌──────────────────────────┐      ┌─────────────────────────────┐
│ manifest → routes+chrome │ ───▶ │ pavo-core Host              │
│ view    → ViewNode + data│ ◀─── │  discovery → manifests      │
│ query   → data payload   │      │  → DriverClient (1 proc/    │
│ render  → RenderModel    │      │    driver, timeout+restart) │
└──────────────────────────┘      │  → merge chrome, resolve     │
                                  │    <driver>:<view> links     │
        renderers: CLI · TUI · web│  → view-model cache          │
                                  └─────────────────────────────┘
```

## The layers

- **`pavo-protocol`** — the public wire contract: driver manifests,
  ViewNode IR, RenderModel, ndjson ops. Pure serde, no I/O. The
  JSON Schemas in `protocol/v1/` are hand-maintained mirrors;
  versioning is additive-only within v1.
- **`pavo-core`** — host core: driver discovery (PATH +
  `<tamias-home>/drivers/`), contribution merge, per-driver
  `DriverClient`s, ViewNode walk/evaluate helpers, the renderer
  registry, the view-model TTL cache. Sync and std-only.
- **`pavo-cli`** (`pavo`) — the binary: `drivers`/`views`/`chrome`/
  `view`/`query`/`serve`/`open` verbs, `--json` everywhere, and the
  protocol conformance harness. Also `pavo serve`, the localhost
  HTTP bridge for the web host.
- **`pavo-tui`** (`pavo-tui`) — the ratatui surface: driver rail,
  ViewNode canvas, detail pane, contribution-driven chrome.
- **`pavo-testkit`** — conformance: `FixtureDriver` builds fake
  `tamias-driver-*` executables, `check_driver` runs the wire
  contract suite, record/replay transcripts give renderers
  deterministic sessions.
- **`pavo-driver-config`** (`tamias-driver-config`) — first-party
  driver: the common config API (namespaced `query` ops over
  layered TOML/JSON config).
- **`web/`** — the Nuxt 4 SSR shell + client-side data islands,
  served as static files by `pavo serve`.
- **`examples/drivers/`** — reference drivers: `demo` (full IR +
  chrome), `myco` (thin proxy pattern), `viz-graph` (renderer
  plugin).

<!-- crate-map -->
| crate | layer | purpose |
|---|---|---|
| `pavo-protocol` | domain | Wire contract — driver manifests, ViewNode IR, ndjson ops (the public API) |
| `pavo-core` | host | Host core — tamias driver discovery, contribution merge, per-driver clients, view-model cache |
| `pavo-driver-config` | adapter | tamias-driver-config — the common config API driver for the 9aia suite (pavo view driver) |
| `pavo-testkit` | adapter | Recorded driver sessions → conformance fixtures for all pavo surfaces |
| `pavo-tui` | adapter | ratatui host — chrome slots + ViewNode renderer for pavo |
| `pavo` | binary | pavo — visualization host: discover tamias drivers, render their views (CLI/TUI/web) |

## The wire

Drivers are spawned per surface need: pavo keeps one `serve`
process per driver (`DriverClient`), speaks `{"op":…}` →
`{"ok":…}` ndjson, and caches view models briefly. Discovery is
cheap — `manifest` answers without product state, so `pavo drivers`
and `pavo views` work even when the underlying product isn't
running. Full contract: [Wire ops](/reference/wire-ops),
[Driver manifest](/reference/driver-manifest).

## Deep links are the UI

A view is a `<driver>:<view>` ref plus optional params. On the CLI
it's an argument; on the web it's a URL
(`/view/myco:item/id=T-001`) that agents and humans can drop into
PRs, issues, and handoffs. The TUI takes the same ref via
`pavo open`. See [Deep links](/tutorials/deep-links).

## Why this shape

See [Why declarative views](/explanation/why-declarative) and
[Boundaries](/explanation/boundaries).
