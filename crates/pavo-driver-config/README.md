# tamias-driver-config

The **common config API** for the 9aia suite — a first-party pavo
driver (Rust, in-workspace `pavo-protocol`) that answers namespaced
config queries over the `serve` wire. Graduated from the Python
prototype that used to live at `examples/drivers/config`.

Other drivers and products read config by spawning
`tamias-driver-config serve` and issuing `query` ops — they never
open config files themselves:

```bash
printf '%s\n' '{"op":"query","query":"get","params":{"key":"myco.theme"}}' \
  | tamias-driver-config serve
# → {"ok":true,"result":{"query":"get","data":{"key":"myco.theme","value":"dark","found":true}}}
```

Products may later adopt the API natively; until then this driver is
the one config surface.

## Queries

| query      | params             | data                                          |
| ---------- | ------------------ | --------------------------------------------- |
| `get`      | `key` (dotted)     | `{key, value, found}`                         |
| `set`      | `key`, `value`     | `{key, value, path, written}` or `{error}`    |
| `resolve`  | —                  | merged config tree (nested JSON object)       |
| `config`   | —                  | merged config flattened to dotted keys        |
| `sources`  | —                  | every contributing layer, in precedence order |

`set` writes the **project-level** file: the `--config`/`PAVO_CONFIG`
path when one was given, else the nearest `pavo.toml`/`pavo.json`
found walking up from the cwd, else a fresh `./pavo.toml`. Dotted
keys are supported (`server.port` creates the `[server]` table);
TOML writes preserve comments/formatting (`toml_edit`), JSON is
rewritten pretty-printed. `null` and nested objects can't be stored
in TOML — use dotted keys for tables.

## Resolution order

Layers merge low → high precedence (later wins per-key, nested
tables merge recursively):

1. **defaults** — `theme=dark`, `statusbar=true`, `refresh_ms=5000`
2. **global** — `$XDG_CONFIG_HOME/pavo/pavo.{toml,json}` or
   `~/.config/pavo/pavo.{toml,json}`
3. **project** — `pavo.toml` and `pavo.json` in every ancestor of the
   cwd, filesystem root → cwd (nearest file wins)
4. **explicit** — `--config <path>` on argv (manual `serve` only —
   pavo's spawned `serve` can't pass argv) or `$PAVO_CONFIG`
5. **env** — `PAVO_CONFIG_*` scalar overrides:
   `PAVO_CONFIG_THEME=light` → `theme`;
   `PAVO_CONFIG_MYCO__THEME=x` → `myco.theme` (`__` = dot, `_` =
   literal underscore). Values parse as JSON when possible.

Formats: **TOML** and **JSON**. **YAML is unsupported** — no yaml dep
in the workspace tree; `.yaml`/`.yml` files surface as `error` rows in
`sources` rather than being silently misparsed.

## Views

| Deep link          | Route              | Shows                                        |
| ------------------ | ------------------ | -------------------------------------------- |
| `config:show`      | `/config`          | resolved KV + sources table + resolution doc |
| `config:sources`   | `/config/sources`  | provenance table only                        |
| `config:edit`      | `/config/edit`     | **interactive form** — set any key via submit |

`config:edit` carries a `form` node (`submit_op: "set"`). Submitting
`{"op":"submit","view":"edit","values":{"key":"myco.theme",
"value":"dark"}}` writes the key through the same path as `query
set` and answers a `SubmitResult` (`message`/`data`, or `errors`
per missing field). CLI equivalent: `pavo submit config:edit
--param key=myco.theme --param value=dark`.

Contributes a `statusbar` entry linking to `/config`.

## CLI

```
tamias-driver-config manifest|capabilities|serve [--config PATH]
```

`serve` is the ndjson JSON-RPC wire every pavo driver speaks
(`pavo-protocol::wire`); `manifest`/`capabilities` print the static
documents for discovery/debugging.

## Install / develop

```bash
cargo install --path crates/pavo-driver-config   # → tamias-driver-config on PATH
cargo xtask install                                # installs pavo + this driver
```

Conformance: `cargo test -p pavo-driver-config` runs
`pavo-testkit::check_driver` against the built binary.
