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

## Examples

`examples/drivers/` ships three reference `tamias-driver-*`
executables (Python 3, stdlib only — they run anywhere python3 does):

| Driver | Directory | Shows |
| --- | --- | --- |
| `tamias-driver-demo` | `examples/drivers/demo/` | board (table+kv+markdown), deps `graph`, timeline `tree`; navbar+dashboard+statusbar |
| `tamias-driver-config` | `examples/drivers/config/` | `config:show` — resolved TOML/JSON config as KV + sources; `get`/`set` queries |
| `tamias-driver-myco` | `examples/drivers/myco/` | thin proxy: translates pavo ops onto `myco serve`'s JSON-RPC wire; degrades when myco is absent |

Put them on `PATH` (or symlink into `$TAMIAS_HOME/drivers/`) and pavo
discovers them like any installed driver:

```bash
export PATH="$PWD/examples/drivers/config:$PWD/examples/drivers/demo:$PWD/examples/drivers/myco:$PATH"
pavo drivers
pavo view demo:deps --format mermaid
```

```
$ pavo drivers
config  Config  v0.1.0  views:1  contributes: statusbar:1
demo  Demo  v0.1.0  views:3  contributes: navbar:2 dashboard:1 statusbar:1
myco  Myco  v0.1.0  views:3  contributes: navbar:1

$ pavo view demo:deps --format mermaid
graph TD
  n0["pavo-protocol"]
  n1["pavo-core"]
  n2["pavo-cli"]
  n3["pavo-tui"]
  n4["pavo-testkit"]
  n5["tamias"]
  n6["myco"]
  n0-->|ref types|n5
  n1-->|uses|n0
  n1-->|drivers|n5
  n2-->|uses|n1
  n2-->|uses|n0
  n3-->|uses|n1
  n4-->|conformance|n0
  n4-->|fixtures|n5
```

`./scripts/smoke-examples.sh` runs the whole set end-to-end
(`pavo drivers`/`views`/`view` over all three drivers).

`pavo-testkit` (`crates/pavo-testkit`) is the conformance harness:
`FixtureDriver` builds fake `tamias-driver-*` executables,
`check_driver` runs the wire contract suite, and record/replay
transcripts give renderers deterministic sessions. The example
drivers are themselves held to the suite in its tests.

## Status

`pavo-protocol`, `pavo-core`, `pavo-cli`, `pavo-testkit`, and the
example drivers are implemented and tested (`cargo xtask check` /
`cargo xtask test`). `pavo-tui` and `web/` are scaffolds.
