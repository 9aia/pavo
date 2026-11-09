---
url: /pavo/how-to/use-config-api.md
---
# Use the config API

`tamias-driver-config` is the suite's **common config API** — a
first-party pavo driver (Rust, in the workspace) that answers
namespaced config queries over the `serve` wire. It is a
convention layered on the `query` op, not part of the v1 wire
contract itself: drivers that need product or host config spawn it
and issue queries — they never open config files themselves.

```bash
printf '%s\n' '{"op":"query","query":"get","params":{"key":"myco.theme"}}' \
  | tamias-driver-config serve
# → {"ok":true,"result":{"query":"get","data":{"key":"myco.theme","value":"dark","found":true}}}
```

Products may later adopt the API natively; until then this driver
is the one config surface. It installs with pavo
(`cargo xtask install`) and contributes a `config:show` view plus a
statusbar entry.

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

Contributes a `statusbar` entry linking to `/config`.

```bash
pavo view config:show
pavo view config:sources
PAVO_CONFIG_THEME=light pavo view config:show   # env override wins
```

## Keys are namespaced

Callers use dotted paths — `myco.theme`, `kuda.index`, `pavo.refresh_ms`
— never file paths or raw TOML. `resolve` returns the merged tree;
`sources` returns every contributing layer so UIs can show
provenance ("this value came from `~/.config/pavo/pavo.toml`").
