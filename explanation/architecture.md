---
url: /pavo/explanation/architecture.md
---
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

Drivers are also how the host composes a **platform**: chrome
contributions (`contributes` slots), dependency declarations
(`deps`), cross-driver query bindings (`QueryBinding.driver`), and
live feeds (`jobs`/`notifications` ops) let products build the
shared screen out of independent pieces — see
[Platform host](#platform-host) below.

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

* **`pavo-protocol`** — the public wire contract: driver manifests,
  ViewNode IR, RenderModel, ndjson ops. Pure serde, no I/O. The
  JSON Schemas in `protocol/v1/` are hand-maintained mirrors;
  versioning is additive-only within v1.
* **`pavo-core`** — host core: driver discovery (PATH +
  `<tamias-home>/drivers/`), contribution merge, per-driver
  `DriverClient`s, ViewNode walk/evaluate helpers, the renderer
  registry, the view-model TTL cache. Sync and std-only.
* **`pavo-cli`** (`pavo`) — the binary: `drivers`/`views`/`chrome`/
  `view`/`query`/`submit`/`jobs`/`notifications`/`serve`/`open`
  verbs, `--json` everywhere, and the protocol conformance harness.
  Also `pavo serve`, the localhost HTTP bridge for the web host.
* **`pavo-tui`** (`pavo-tui`) — the ratatui surface: driver rail,
  ViewNode canvas, detail pane, contribution-driven chrome.
* **`pavo-testkit`** — conformance: `FixtureDriver` builds fake
  `tamias-driver-*` executables, `check_driver` runs the wire
  contract suite, record/replay transcripts give renderers
  deterministic sessions.
* **`pavo-driver-config`** (`tamias-driver-config`) — first-party
  driver: the common config API (namespaced `query` ops over
  layered TOML/JSON config).
* **`pavo-driver-kit`** — the driver-authoring SDK: the ndjson
  `serve` loop plus fluent builders for manifests, views, query
  bindings, contributions, ViewNode trees, and feed items. Most
  product drivers are written on it; the kit wraps
  `pavo-protocol`, never replaces it.
* **`web/`** — the Nuxt 4 SSR shell + client-side data islands,
  served as static files by `pavo serve`.
* **`examples/drivers/`** — reference drivers: `demo` (full IR +
  chrome), `platform-demo` (the platform host end-to-end), `myco`
  (thin proxy pattern), `viz-graph` (renderer plugin).

| crate | layer | purpose |
|---|---|---|
| `pavo-protocol` | domain | Wire contract — driver manifests, ViewNode IR, ndjson ops (the public API) |
| `pavo-core` | host | Host core — tamias driver discovery, contribution merge, per-driver clients, view-model cache |
| `pavo-driver-config` | adapter | tamias-driver-config — the common config API driver for the 9aia suite (pavo view driver) |
| `pavo-driver-kit` | adapter | pavo-driver-kit — declarative driver authoring for tamias drivers (manifest builders, ViewNode constructors, ndjson serve loop) |
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

## Platform host

The manifest is not just a discovery document — it's how drivers
compose into one platform. Four mechanisms, all additive within
protocol v1:

* **Chrome slots.** `contributes` has eight built-in slots —
  `navbar`, `dashboard`, `statusbar`, `routes`, `sidebar`,
  `headerActions`, `notifications`, `jobs`. The host merges every
  driver's entries into one `Chrome` (priority, then driver name,
  then id — deterministic); the web shell renders them as a
  sidebar, header actions, a notification bell, and a `/jobs`
  page. The TUI lists slot contributions on its dashboard but has
  no dedicated chrome for the platform slots and doesn't poll the
  feeds — documented limitation; `pavo jobs` /
  `pavo notifications` and `/api/jobs` / `/api/notifications` are
  the feed faces.
* **Deps.** `deps.requires`/`deps.optional` declare a driver-name
  dependency graph. The host topologically sorts drivers (deps
  first, name-ascending tie-break) so merge and feed order is
  stable; a missing `requires` dep or a cycle **quarantines** the
  offender — discovered but erroring on every access — while
  absent `optional` deps merely degrade.
* **Cross-driver queries.** A `QueryBinding.driver` field lets one
  driver's view pull data from *another* driver's `serve` — the
  host resolves the binding against the named driver and merges
  the payload into `ViewResult.data` under the binding name.
  Failures degrade to `{"error": …}` in that slot, same as local
  bindings. This is the data half of the platform story: e.g. a
  product's board can show `config` driver's theme without either
  product knowing the other's internals.
* **Feeds.** `contributes.jobs`/`notifications` mark a driver as a
  provider; the `jobs`/`notifications` wire ops return
  `JobItem`/`NotificationItem` lists (RFC 3339 timestamps;
  `status`/`severity` are driver-owned vocabularies). The host
  aggregates across providers into `FeedEntry<T>` — driver-
  attributed — skipping per-driver failures, so one broken feed
  never takes down the rest.

`pavo serve` exposes all of it: `/api/chrome` returns every slot,
`/api/jobs` + `/api/notifications` return flat driver-attributed
rows, and `POST /api/call?driver=<kind>` proxies any wire `Request`
to a named driver — the cross-driver escape hatch for API clients.

## Deep links are the UI

A view is a `<driver>:<view>` ref plus optional params. On the CLI
it's an argument; on the web it's a URL
(`/view/myco:item/id=T-001`) that agents and humans can drop into
PRs, issues, and handoffs. The TUI takes the same ref via
`pavo open`. See [Deep links](/tutorials/deep-links).

## Why this shape

See [Why declarative views](/explanation/why-declarative) and
[Boundaries](/explanation/boundaries).
