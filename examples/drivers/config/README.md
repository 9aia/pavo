# tamias-driver-config

The generic config driver — `config:show` renders the resolved pavo
config as a KV tree plus a table of config *sources*, and the `get` /
`set` queries read and write the underlying file.

Python 3 stdlib only (`tomllib` for TOML — needs Python 3.11+).

## Install

```bash
export PATH="$PWD/examples/drivers/config:$PATH"
# or:
ln -s "$PWD/examples/drivers/config/tamias-driver-config" \
      "${TAMIAS_HOME:-~/.local/share/tamias}/drivers/tamias-driver-config"
```

## Config resolution order

First existing candidate wins:

1. `--config <path>` argument — only when invoking the driver
   directly (`tamias-driver-config --config f.toml serve`); pavo's
   spawned `serve` can't pass argv, so this is for manual use.
2. `PAVO_CONFIG` environment variable.
3. `~/.config/pavo/config.toml`
4. `~/.config/pavo/config.json`
5. Built-in defaults (`theme=dark`, `statusbar=true`, `refresh_ms=5000`).

Formats: **TOML** (via `tomllib`) and **JSON** (by `.json` extension
or `{`-leading content). **YAML is not supported** — the stdlib has no
YAML parser; the driver reports a clear error for `.yaml`/`.yml` files
rather than silently misparsing them.

## Views

| Deep link      | Route     | Shows                                             |
| -------------- | --------- | ------------------------------------------------- |
| `config:show`  | `/config` | resolved config (kv, dotted keys) + sources table |

Contributes a `statusbar` entry linking to `/config` — the *view*
carries the live source count (chrome slots are static in wire v1).

## Queries

- `get` — params `{"key": "server.port"}` (dotted lookup over the
  merged config) → `{"found": bool, "value": …}`.
- `set` — params `{"key": "accent", "value": "blue"}` writes a
  **top-level** key back to the resolved file (TOML: replaces the
  `key = …` line or inserts before the first `[section]`; JSON:
  rewrites the object). Dotted keys are rejected — sectioned writes
  are deliberately out of scope for the example.
- `config` / `sources` — the bindings feeding `config:show`.

```bash
PAVO_CONFIG=~/pavo.toml pavo view config:show
printf '%s\n' '{"op":"query","query":"set","params":{"key":"theme","value":"light"}}' \
  | tamias-driver-config serve
```
