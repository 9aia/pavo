# Pavo

Visualization **host** for the 9aia org's tool suite — the factory's
screen. Named for *Pavo* — the peafowl genus; the display animal.

Products ship **tamias drivers**: executables named
`tamias-driver-<type>` found on `PATH` or in
`<tamias-home>/drivers/` (`$TAMIAS_HOME` or
`~/.local/share/tamias/drivers`). A driver may answer the extra verbs
`manifest` and `serve` beside the install verbs, emitting declarative
**ViewNode IR** JSON trees and view models over the org's ndjson
JSON-RPC convention. Pavo's renderers — CLI, TUI, web — interpret
those trees natively.

Install a driver → get visualization everywhere. There is no
`<product> ui` per product; pavo is the single entry point.

## Surfaces

- **CLI** (`pavo`) — `pavo drivers`, `pavo view <driver>:<view>
  --format json`, deep links; doubles as the protocol conformance
  harness. `--json` everywhere.
- **TUI** (`pavo-tui`) — ratatui host: driver rail, ViewNode canvas,
  detail pane, contribution-driven chrome.
- **Web** (`web/`, Nuxt SSR + islands) — streaming shell, per-view
  islands, deep links (`/myco/item/T-001`) as stable URLs.

## Layout

```
crates/    pavo-protocol · pavo-core · pavo-cli · pavo-tui · pavo-testkit
web/       Nuxt SSR host + serve bridge (stdio↔ws) — scaffold stage
protocol/  driver capability spec · contribution slots · ViewNode IR ·
           wire ops (versioned, additive-only within v1)
xtask      check / test / coverage / install (myco anatomy)
```

`pavo-protocol` is the canonical wire contract: driver manifests,
ViewNode IR, and the ndjson request/response types every renderer
speaks. JSON Schemas mirroring it live in `protocol/v1/`.

## Status

Scaffold. `pavo-protocol` is implemented; the host crates are
compiling stubs pending the v1 plan (P-003): `config` + `myco`
drivers, `pavo-core` discovery, CLI, TUI.
