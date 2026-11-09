---
url: /pavo/getting-started.md
---
# Getting Started

Install pavo, put a driver on `PATH`, and render a view.

## Install pavo

```sh
cargo install --git https://github.com/9aia/pavo pavo tamias-driver-config
# or, from a checkout:
cargo xtask install   # pavo + tamias-driver-config → ~/.cargo/bin
```

`tamias-driver-config` is the suite's common config API driver — it
ships with pavo so every install has at least one real driver.

## Install a view driver

Drivers are executables named `tamias-driver-<type>` found on `PATH`
or in `<tamias-home>/drivers/` (`$TAMIAS_HOME` or
`~/.local/share/tamias/drivers`). The repo ships reference drivers
under `examples/drivers/` — Python 3, stdlib only:

```sh
export PATH="$PWD/examples/drivers/demo:$PWD/examples/drivers/myco:$PWD/examples/drivers/viz-graph:$PWD/examples/drivers/platform-demo:$PATH"
```

Real product drivers (`tamias-driver-kuda`, `tamias-driver-myco`,
`tamias-driver-cuora`, …) install the same way — via tamias or onto
`PATH`. See [Install a driver](/how-to/install-a-driver).

## Render a view

```sh
pavo drivers                          # what pavo found
pavo views                            # every <driver>:<view> ref
pavo view demo:board                  # text summary of the tree
pavo view demo:deps --format mermaid  # graph → mermaid
pavo view demo:deps --format svg      # viz-graph renderer plugin
pavo query demo items --param status=open
pavo jobs                              # aggregated feed (platform-demo provider)
pavo notifications
pavo chrome                            # all eight contribution slots
```

## Open the TUI or web surface

```sh
pavo open                 # ratatui host, dashboard
pavo open demo:deps       # straight to a deep link

pavo serve --port 7420    # localhost bridge + web host
```

## Next

* [Your first view](/tutorials/first-view)
* [Architecture](/explanation/architecture)
* [CLI reference](/reference/cli)
