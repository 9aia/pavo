# web/ — pavo Nuxt host (scaffold)

The web surface of the pavo viz host. **Not yet scaffolded** — this
stub pins the intended architecture so the Rust side can land first
(P-003 v2).

## Intended architecture

- **Nuxt 3 SSR** — the shell streams instantly; pages compose
  driver-contributed routes (`contributes.routes`) and views.
- **Hydration + islands** — each view panel is an island fetching
  its own view-model, so graphs/timelines stay interactive without
  shipping a SPA bundle for everything. Static-exportable views need
  no bridge at all.
- **`pavo serve` bridge** — a localhost stdio↔ws/http bridge keeps
  drivers stdio-only like every sibling `serve`; pavo owns the web
  transport. No driver ever binds a port.
- **Deep links are the killer feature** — `/myco/item/T-001`,
  `/kuda/module/kuda-core`, `/tamias/store`: stable URLs agents and
  humans drop into PRs, issues, and handoffs.
- **Dashboard composition** — the `dashboard` route merges
  `contributes.dashboard` entries from all installed drivers; each
  panel is an island owned by its driver.

## Contracts consumed

- `protocol/v1/*.schema.json` — driver manifests, ViewNode IR, wire
  ops. A TS client generated from these schemas is planned as a
  public API (embeds, tamias-site pages).

## Status

Stub. When picked up: `npx nuxi init` (or hand-scaffold
`nuxt.config.ts` + `pages/` + `server/` bridge), then wire the
bridge to `pavo-core`'s driver clients.
