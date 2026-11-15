# Getting started

From a fresh checkout to a rendered view — using the `demo` example
driver, which exercises the whole ViewNode IR (table, kv, markdown,
graph, tree) plus navbar/dashboard/statusbar contributions.

## 1. Install pavo

```sh
cargo install --git https://github.com/9aia/pavo pavo tamias-driver-config
# or, from a checkout:
cargo xtask install   # pavo + tamias-driver-config → ~/.cargo/bin
```

`tamias-driver-config` is the suite's common config API driver — it
ships with pavo so every install has at least one real driver.

## 2. Put a driver on PATH

Drivers are executables named `tamias-driver-<type>` found on `PATH`
or in `<tamias-home>/drivers/` (`$TAMIAS_HOME` or
`~/.local/share/tamias/drivers`). The repo ships reference drivers
under `examples/drivers/` — Python 3, stdlib only:

```sh
export PATH="$PWD/examples/drivers/demo:$PWD/examples/drivers/myco:$PWD/examples/drivers/viz-graph:$PWD/examples/drivers/platform-demo:$PATH"
pavo drivers
```

Expected output:

```
config  Config  v0.1.0  views:2  renderers:0  contributes: statusbar:1
demo  Demo  v0.1.0  views:3  renderers:0  contributes: navbar:2 dashboard:1 statusbar:1
```

`config` is the config API driver that installed with pavo; `demo`
is the example you just added. Drivers are discovered by executable
name — `tamias-driver-demo` — not by registration. Real product
drivers (`tamias-driver-kuda`, `tamias-driver-myco`,
`tamias-driver-cuora`, …) install the same way — via tamias or onto
`PATH`. See [Install a driver](/how-to/install-a-driver).

## 3. List and render views

```sh
pavo views
```

```
demo:board      /demo/board     Board
config:show     /config        Config
demo:deps       /demo/deps     Dependencies
…
```

```sh
pavo view demo:board
```

The default `text` format summarizes the ViewNode tree plus the data
bound to it. Try the other formats:

```sh
pavo view demo:board --format json       # raw ViewResult
pavo view demo:deps --format dot         # graph → graphviz
pavo view demo:deps --format mermaid     # graph → mermaid
pavo view demo:deps --format svg         # viz-graph renderer plugin
```

## 4. Pass params

Views declare query bindings; callers pass params that merge into
them:

```sh
pavo view demo:board --param status=open
pavo view demo:board --param status=done
```

`--param k=v` accepts repeated pairs; values parse as JSON when
possible (`n=42`, `ok=true`). For ad-hoc queries:

```sh
pavo query demo items --param status=open
```

## 5. See the merged chrome

```sh
pavo chrome
```

Lists every driver's contribution slots — navbar entries, dashboard
panels, statusbar items, routes — merged deterministically. The same
chrome drives the TUI rail and the web dashboard:

```sh
pavo jobs                              # aggregated feed (platform-demo provider)
pavo notifications
```

## 6. Open the TUI or web surface

```sh
pavo open                 # ratatui host, dashboard
pavo open demo:deps       # straight to a deep link

pavo serve --port 7420    # localhost bridge + web host
```

## What just happened

`pavo view demo:board` resolved the deep link `demo:board`, spawned
`tamias-driver-demo serve`, and spoke the wire:

```
→ {"op":"view","view":"board","params":{}}
← {"ok":true,"result":{"view":{…ViewSpec…},"data":{…}}}
```

The driver returned a declarative tree (`panel` → `kv` + `table` +
`markdown`) plus query-fed data; pavo's built-in renderers
interpreted it. No product code ran inside pavo — the driver is a
separate process, and the IR is the only thing that crossed the
boundary.

## What you now know

- pavo is a host: drivers are `tamias-driver-<type>` executables on
  `PATH` discovered by name; views are `<driver>:<view>` deep links.
- Views return declarative ViewNode IR + data; pavo's renderers
  (`text`, `json`, `dot`, `mermaid`, plugin `svg`) interpret it —
  product code never runs inside pavo.
- `--param k=v` feeds view query bindings; `chrome` merges every
  driver's navbar/dashboard/statusbar contributions.
- `pavo open` is the ratatui TUI; `pavo serve` is the localhost
  bridge + web host.

## Next

- [Share a deep link](/how-to/deep-links) — stable URLs for views and params
- [Write a driver](/how-to/write-a-driver) — build your own
- [The wire protocol](/reference/wire-ops) — the ops in detail
- [Architecture](/explanation/architecture) ·
  [concepts](/explanation/concepts) ·
  [decisions](/explanation/decisions)
- [CLI reference](/reference/cli)
