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

/** Find the registry entry claiming `kind` for the `web` surface —
 *  explicit kinds beat `"*"` (the bridge already resolved conflicts;
 *  here we just take the first match in registry order). */
export function webRendererFor(
  registry: RendererEntry[],
  kind: string,
): RendererEntry | undefined {
  return registry.find(
    (r) =>
      r.surfaces?.web &&
      (r.kinds.includes(kind) || r.kinds.includes('*')),
  )
}
