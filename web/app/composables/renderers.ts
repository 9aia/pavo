// Renderer-plugin registry — fetched once per page load from the
// pavo bridge (`GET /api/renderers`). ViewNode.vue consults it to
// decide whether a node `kind` is claimed by a driver renderer
// (manifest `renderers[]`) before falling back to built-in
// interpretation.
//
// Wire shape (one entry per RendererDecl, driver-resolved):
//   { id: "viz-graph:graph", driver: "viz-graph",
//     kinds: ["graph"], priority: 100, entry?,
//     surfaces: { cli?: {formats: []}, tui?: bool,
//                 web?: {component?, island?} } }
//
// `web.component` is intentionally ignored in v1 (protocol decision):
// every RenderModel renders through the generic mapping in
// ViewNode.vue — `component`/`island` are hints for future
// dynamic-imported mounts, not load-bearing.

export interface RendererSurfaceCli {
  formats?: string[]
}
export interface RendererSurfaceWeb {
  component?: string
  island?: boolean
}
export interface RendererSurfaces {
  cli?: RendererSurfaceCli
  tui?: boolean
  web?: RendererSurfaceWeb
}
export interface RendererEntry {
  id: string
  driver: string
  driver_title?: string
  kinds: string[]
  surfaces: RendererSurfaces
  priority?: number
  entry?: string
}

// Module-level promise — every ViewNode instance shares one fetch.
let pending: Promise<RendererEntry[]> | null = null

/** The renderer registry, fetched lazily once. Empty on bridge
 *  errors (built-in rendering covers everything then). */
export function rendererRegistry(): Promise<RendererEntry[]> {
  if (!pending) {
    pending = $fetch('/api/renderers')
      .then((d: any) => {
        const rows = d?.result ?? d
        return Array.isArray(rows) ? (rows as RendererEntry[]) : []
      })
      .catch(() => [] as RendererEntry[])
  }
  return pending
}

/** Find the registry entry claiming `kind` for the `web` surface.
 *
 *  Mirrors the host's claim resolution (`RendererRegistry::for_kind`
 *  in pavo-core): explicit kinds beat `"*"`; among competing claims
 *  the lowest `priority` wins, ties break by driver name then
 *  renderer id. `/api/renderers` lists every declaration sorted by
 *  id, so the winner isn't necessarily first — resolve it here.
 *
 *  This is advisory only: `GET /api/render` re-resolves the claim
 *  server-side and is authoritative — if the plugin path fails the
 *  caller falls back to built-in rendering regardless. */
export function webRendererFor(
  registry: RendererEntry[],
  kind: string,
): RendererEntry | undefined {
  const onWeb = registry.filter((r) => r.surfaces?.web)
  const wildcard = (r: RendererEntry) => r.kinds.includes('*')
  // Explicit claims shadow the wildcard pool entirely (same as the
  // host: the kind's winning binding decides surface support).
  const explicit = onWeb.filter((r) => !wildcard(r) && r.kinds.includes(kind))
  const pool = explicit.length ? explicit : onWeb.filter(wildcard)
  return [...pool].sort(
    (a, b) =>
      (a.priority ?? 100) - (b.priority ?? 100) ||
      a.driver.localeCompare(b.driver) ||
      a.id.localeCompare(b.id),
  )[0]
}
