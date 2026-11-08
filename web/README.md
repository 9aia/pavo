# web/ — pavo Nuxt host

The web surface of the pavo viz host: a **Nuxt 4** app (SSR shell +
client-side data islands) rendered against the `pavo serve` localhost
bridge. Deep links (`/view/<driver>:<view>`) are stable URLs agents
and humans can drop into PRs, issues, and handoffs.

## Architecture

```
browser ──► pavo serve (127.0.0.1) ──► pavo-core Host ──► tamias-driver-* serve (stdio)
              │  /api/* JSON        │
              └── static: web/.output/public (this app's build)
```

- **`pavo serve`** (`crates/pavo-cli`) is the only transport. It binds
  `127.0.0.1`, prints `listening http://127.0.0.1:<port>` as its first
  stdout line, and serves:
  - `GET /api/health|drivers|views|chrome|view|raw|query` — JSON
    `{"ok":…,"result"/"error"}` envelopes over the same `Host` the CLI
    uses (CORS `*` — localhost dev tool).
  - `GET /*` — static files from `web/.output/public`, then
    `web/dist`, then `$PAVO_WEB_DIR`; with no build present it serves
    a built-in landing page listing drivers/views.
- **This app** fetches `/api/*` **client-side** (`useFetch` with
  `server: false`): the SSR/prerendered shell streams instantly and
  each panel hydrates itself — the island model. That also makes the
  static export work: `npm run build` prerenders shells only, and the
  browser fetches live data from whatever serves `/api`.
- **View rendering** — `app/components/ViewNode.vue` interprets the
  ViewNode IR recursively (panel/row/column layouts, table, tree, kv,
  markdown, read-only form, embed placeholder, KV degradation for
  unknown kinds). `graph` nodes render in `GraphIsland.vue`, a
  client-only island (deterministic circle/spiral SVG layout — real
  layout is an `embed` concern).

## Dev workflow

Two terminals:

```bash
# 1. the bridge (any port; 7420 matches the dev proxy default)
export PATH="$PWD/../examples/drivers/demo:$PATH"   # your drivers
pavo serve --port 7420        # or: cargo run -p pavo -- serve --port 7420

# 2. the app
cd web && npm install && npm run dev    # http://localhost:3000
```

`server/api/[...].ts` proxies `/api/*` to the bridge in dev
(`PAVO_API_TARGET` > `NUXT_PUBLIC_PAVO_API` > `http://127.0.0.1:7420`).

## Production-ish flow (what `pavo serve` serves)

```bash
cd web && npm run build      # static prerender → .output/public
pavo serve                   # auto-detects web/.output/public
```

Open the printed `http://127.0.0.1:<port>/` — the dashboard lists
drivers, views (deep links `/view/demo%3Aboard`), and chrome
contributions; `/api/*` is same-origin, no CORS needed.

`npm run generate` is equivalent (`nuxt generate`).
`NITRO_PRESET=node-server npm run build` produces a standalone SSR
build instead — then `server/api/*` also works and
`NUXT_PUBLIC_PAVO_API` points the browser at the bridge directly.

## Env vars

| Var | Where | Meaning |
| --- | --- | --- |
| `NUXT_PUBLIC_PAVO_API` | build/dev | absolute bridge base, e.g. `http://127.0.0.1:7420`. Empty (default) = same-origin `/api/…`. |
| `PAVO_API_TARGET` | `nuxt dev` only | proxy target for `server/api/[...].ts`. |
| `NITRO_PRESET` | build | `static` (default) or `node-server`. |
| `PAVO_WEB_DIR` | `pavo serve` | explicit web-root override for static files. |

`pavo serve --port 0` picks a free port and prints it — with the
default same-origin setup you don't need to know it for the web UI,
but set `NUXT_PUBLIC_PAVO_API=http://127.0.0.1:<port>` if the Nuxt dev
server should bypass its proxy.


## Security notes

`npm audit` currently reports 12 transitive vulnerabilities (6 low,
6 moderate) inside the Nuxt/Nitro build chain (`deepmerge`,
`glob`/`minimatch`, `archiver` via `readdir-glob`, `@vercel/nft`).
Every fix path requires `npm audit fix --force`, which downgrades
`nuxt` to 4.1.3 — a breaking change — so they are intentionally left
unfixed. Exposure is limited to the local dev/build toolchain; the
shipped artifact is static files served by `pavo serve`.
Re-check on Nuxt upgrades.

## Status

v1: dashboard + view deep links + all ViewNode kinds (graph as a
client-only SVG island). Not yet: interactive forms (read-only
field list), `embed` component registry, view params UI (params can
still be passed as `?param.<key>=<value>` query args), markdown
rendering (pre-wrap text), driver-contributed `routes` pages (they
link out but only views render natively).
