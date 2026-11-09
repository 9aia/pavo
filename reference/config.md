---
url: /pavo/reference/config.md
---
# Config

Host/product configuration for the pavo suite flows through the
**config API** — `tamias-driver-config`, a first-party driver that
answers namespaced `query` ops over the `serve` wire. There is no
`pavo.toml` read directly by the host; drivers and products query
the config driver (or adopt the API natively later).

Full contract: `crates/pavo-driver-config/README.md`; usage:
[Use the config API](/how-to/use-config-api).

## Queries

| query | params | returns |
| --- | --- | --- |
| `get` | `key` (dotted) | `{key, value, found}` |
| `set` | `key`, `value` | `{key, value, path, written}` |
| `resolve` | — | merged config tree (nested JSON) |
| `config` | — | merged config flattened to dotted keys |
| `sources` | — | every contributing layer, in precedence order |

Keys are namespaced dotted paths (`myco.theme`, `kuda.index`) —
callers never read config files directly.

## Layers (low → high precedence)

1. **defaults** — `theme=dark`, `statusbar=true`, `refresh_ms=5000`
2. **global** — `$XDG_CONFIG_HOME/pavo/pavo.{toml,json}` or
   `~/.config/pavo/pavo.{toml,json}`
3. **project** — `pavo.toml` and `pavo.json` in every ancestor of the
   cwd, filesystem root → cwd (nearest file wins)
4. **explicit** — `--config <path>` on argv (manual `serve` only) or
   `$PAVO_CONFIG`
5. **env** — `PAVO_CONFIG_*` scalar overrides

## Environment variables

| Var | Meaning |
| --- | --- |
| `TAMIAS_HOME` | tamias home override — also the default drivers dir parent (`$TAMIAS_HOME/drivers`) |
| `XDG_DATA_HOME` | fallback parent for `<tamias-home>` (`$XDG_DATA_HOME/tamias`) |
| `XDG_CONFIG_HOME` | fallback parent for global config (`$XDG_CONFIG_HOME/pavo/`) |
| `PAVO_CONFIG` | explicit config file path for `tamias-driver-config` |
| `PAVO_CONFIG_*` | env overrides — `PAVO_CONFIG_THEME=light` → `theme`; `PAVO_CONFIG_MYCO__THEME=x` → `myco.theme` (`__` = dot, `_` = literal underscore); values parse as JSON when possible |
| `PAVO_WEB_DIR` | `pavo serve` static web-root override |
| `NUXT_PUBLIC_PAVO_API` | web app: absolute bridge base (empty = same-origin `/api`) |
| `PAVO_API_TARGET` | `nuxt dev` only: `/api/*` proxy target |
| `NITRO_PRESET` | web build: `static` (default) or `node-server` |

## Formats

**TOML** and **JSON** only — no YAML dependency in the workspace
tree. `.yaml`/`.yml` files surface as `error` rows in `sources`
rather than being silently misparsed.

`set` writes the project-level file (explicit path → nearest
`pavo.toml`/`pavo.json` → fresh `./pavo.toml`); TOML writes preserve
comments via `toml_edit`. `null` and nested objects can't be stored
in TOML — use dotted keys for tables.
