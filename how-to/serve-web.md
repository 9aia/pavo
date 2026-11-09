# Serve the web surface

`pavo serve` is the localhost HTTP bridge between the browser and
the driver plane — the only transport the web host uses. It binds
`127.0.0.1` (localhost dev tool; CORS `*` on `/api/*`), prints
`listening http://127.0.0.1:<port>` as its first stdout line, and
serves two things:

- **`/api/*`** — JSON `{"ok":…,"result"/"error"}` envelopes over the
  same `Host` the CLI uses:
  `GET /api/health|drivers|views|chrome|view|raw|query|submit|renderers|render`
- **`/*`** — static files from `web/.output/public`, then `web/dist`,
  then `$PAVO_WEB_DIR`; with no build present it serves a built-in
  landing page that still understands `/view/` deep links.

## Dev workflow (two terminals)

```bash
# 1. the bridge (7420 matches the Nuxt dev proxy default)
export PATH="$PWD/examples/drivers/demo:$PATH"
pavo serve --port 7420        # or: cargo run -p pavo -- serve --port 7420

# 2. the app
cd web && npm install && npm run dev    # http://localhost:3000
```

`web/server/api/[...].ts` proxies `/api/*` to the bridge in dev —
target precedence: `PAVO_API_TARGET` > `NUXT_PUBLIC_PAVO_API` >
`http://127.0.0.1:7420`.

## Production-ish flow

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

## Environment variables

| Var | Where | Meaning |
| --- | --- | --- |
| `NUXT_PUBLIC_PAVO_API` | build/dev | absolute bridge base, e.g. `http://127.0.0.1:7420`. Empty (default) = same-origin `/api/…`. |
| `PAVO_API_TARGET` | `nuxt dev` only | proxy target for `server/api/[...].ts`. |
| `NITRO_PRESET` | build | `static` (default) or `node-server`. |
| `PAVO_WEB_DIR` | `pavo serve` | explicit web-root override for static files. |
| `PAVO_CONFIG` | `tamias-driver-config` | explicit config file path (see [Use the config API](/how-to/use-config-api)). |

`pavo serve --port 0` picks a free port and prints it.

## What the web app renders

`web/app/components/ViewNode.vue` interprets the ViewNode IR
recursively — panel/row/column layouts, table, tree, kv, markdown,
interactive form (`submit_op` → `FormView` island hitting `/api/submit`), embed placeholder, KV degradation for unknown
kinds. `graph` nodes render in `GraphIsland.vue`, a client-only
SVG island (deterministic circle/spiral layout) — the *fallback*:
a renderer plugin claiming `graph` for the `web` surface renders
its canonical `RenderModel` via `/api/render` instead (placeholder
while in flight, built-in only if the plugin fails or doesn't
claim the kind). Driver-contributed `routes` link
out; only views render natively in v1.

## Security notes

`npm audit` currently reports transitive vulnerabilities inside the
Nuxt/Nitro build chain (see `web/README.md` for the breakdown);
exposure is limited to the local dev/build toolchain — the shipped
artifact is static files served by `pavo serve`. Re-check on Nuxt
upgrades.
