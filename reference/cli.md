# CLI reference

```
Visualization host for the 9aia tool suite

Usage: pavo [COMMAND]

Commands:
  drivers     List discovered tamias-driver-* executables that speak pavo
  views       List every <driver>:<view> ref with title/route
  chrome      Show the merged contribution slots
  view        Resolve a deep link and render it
  query       Raw query-op passthrough to a driver
  serve       Run the localhost HTTP bridge for the web host
  open        Hand off to pavo-tui (falls back to `view` text off-TTY)
  manifest    Print the machine-readable capability manifest
  schema      Print the verb schema
  help        Print this message or the help of the given subcommand(s)

Options:
  -h, --help     Print help
  -V, --version  Print version
```

`--json` is a per-subcommand flag on the listing/query verbs;
`view --format json` is its machine-readable mode. Errors go to
stderr; exit codes are CI-meaningful: `0` ok, `1` view/driver error,
`2` usage.

## drivers

```sh
pavo drivers [--json]
```

Lists every discovered `tamias-driver-*` that speaks pavo — name,
title, version, view count, renderer count, contributes summary.
Discovery scans `PATH` and `<tamias-home>/drivers/`
(`$TAMIAS_HOME` → `$XDG_DATA_HOME/tamias` → `~/.local/share/tamias`
→ `./.tamias`).

## views

```sh
pavo views [--driver <name>] [--json]
```

Every view as a `<driver>:<view>` ref with title, route, and query
names. `--driver` filters to one driver.

## chrome

```sh
pavo chrome [--json]
```

The merged contribution slots — `navbar`, `dashboard`, `statusbar`,
`routes`, plus forward-compat `extra` slots — resolved across all
drivers by priority.

## view

```sh
pavo view <driver>:<view> [--param k=v …] [--format FMT] [--json]
```

Resolves the deep link and renders the ViewNode tree plus bound
data.

| `--format` | Output |
| --- | --- |
| `text` (default) | summary of the tree + bound data |
| `json` | raw `ViewResult` |
| `dot` | graph nodes → graphviz |
| `mermaid` | graph nodes → mermaid |
| `<plugin>` | any format a renderer plugin claims (e.g. `svg`, `ascii`) |

`--param k=v` merges caller params into every query binding;
repeatable, values parse as JSON when possible. `--json` prints the
machine-readable envelope.

## query

```sh
pavo query <driver> <query> [--param k=v …] [--json]
```

Raw `query` op passthrough — `{"op":"query","query":"<query>",
"params":{…}}` — for probing a driver's data without a view.

## serve

```sh
pavo serve [--port N]
```

The localhost HTTP bridge (`127.0.0.1`; `--port 0` picks a free
port). First stdout line is `listening http://127.0.0.1:<port>`.
Endpoints and static serving: [Serve the web surface](/how-to/serve-web).

## open

```sh
pavo open [<driver>:<view>]
```

Hands off to `pavo-tui` with the deep link, or the bare dashboard
when no target is given. On a non-TTY it falls back to `pavo view`
text output.

## manifest / schema

```sh
pavo manifest   # the CLI's own capability manifest (sibling convention)
pavo schema     # the verb schema
```
