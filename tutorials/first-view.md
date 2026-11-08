# Your first view

This tutorial takes you from a fresh checkout to a rendered view —
using the `demo` example driver, which exercises the whole ViewNode
IR (table, kv, markdown, graph, tree) plus navbar/dashboard/statusbar
contributions.

## 1. Build and install pavo

```sh
cd pavo
cargo xtask install
```

This release-builds `pavo` and `tamias-driver-config` into
`~/.cargo/bin`.

## 2. Put the demo driver on PATH

```sh
export PATH="$PWD/examples/drivers/demo:$PATH"
pavo drivers
```

Expected output:

```
config  Config  v0.1.0  views:2  renderers:0  contributes: statusbar:1
demo  Demo  v0.1.0  views:3  renderers:0  contributes: navbar:2 dashboard:1 statusbar:1
```

`config` is the config API driver that installed with pavo; `demo`
is the example you just added. Drivers are discovered by executable
name — `tamias-driver-demo` — not by registration.

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
```

## 4. Pass params

Views declare query bindings; callers pass params that merge into
them:

```sh
pavo view demo:board --param status=open
pavo view demo:board --param status=done
```

`--param k=v` accepts repeated pairs; values parse as JSON when
possible (`n=42`, `ok=true`).

## 5. See the merged chrome

```sh
pavo chrome
```

Lists every driver's contribution slots — navbar entries, dashboard
panels, statusbar items, routes — merged deterministically. The same
chrome drives the TUI rail and the web dashboard.

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

## Next

- [Deep links](/tutorials/deep-links) — stable URLs for views and params
- [Write a driver](/how-to/write-a-driver) — build your own
- [The wire protocol](/reference/wire-ops) — the ops in detail
