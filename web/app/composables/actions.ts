// Interaction layer plumbing — manifest fetches, the `invoke` wire
// op over `POST /api/call`, view-swap routing, and the module-level
// store the command palette reads.
//
// Bridge paths (crates/pavo-cli/src/serve.rs):
//   POST /api/call?driver=<kind>  body = wire Request JSON
//     {"op":"manifest"} → DriverManifest
//     {"op":"invoke","view":…,"action":…,"target":…,"params":…}
//       → ActionResult {message, errors, data, view?}
// There is no dedicated /api/invoke — the cross-driver proxy is
// the invoke path.
import type {
  Action,
  ActionResult,
  DriverManifest,
  MenuEntry,
  ScopedAction,
  ViewResult,
} from '~/utils/interactions'
import { collectActions, dedupeActions } from '~/utils/interactions'

// ---- manifest cache ----------------------------------------------------

const manifestCache = new Map<string, Promise<DriverManifest | null>>()

/** The driver's full manifest via the wire `manifest` op — cached
 *  per driver for the session (manifests are static per process). */
export function fetchManifest(driver: string): Promise<DriverManifest | null> {
  let p = manifestCache.get(driver)
  if (!p) {
    p = $fetch('/api/call', {
      method: 'POST',
      query: { driver },
      body: { op: 'manifest' },
    })
      .then((d: any) => (d?.ok ? (d.result as DriverManifest) : null))
      .catch(() => null)
    manifestCache.set(driver, p)
  }
  return p
}

/** All discovered driver names (`/api/drivers`) — for menu/palette
 *  enumeration. */
export function fetchDriverNames(): Promise<string[]> {
  return $fetch('/api/drivers')
    .then((d: any) => {
      const rows = d?.ok ? d.result : []
      return Array.isArray(rows)
        ? rows.map((r: any) => r.driver).filter((x: any) => typeof x === 'string')
        : []
    })
    .catch(() => [] as string[])
}

let allManifestsPromise: Promise<{ driver: string; manifest: DriverManifest }[]> | null = null

/** Every driver's manifest, fetched lazily once — palette menus
 *  need `contributes.menus[]` which `/api/chrome` doesn't merge. */
export function fetchAllManifests(): Promise<{ driver: string; manifest: DriverManifest }[]> {
  if (!allManifestsPromise) {
    allManifestsPromise = fetchDriverNames().then(async (names) => {
      const rows = await Promise.all(
        names.map(async (driver) => {
          const manifest = await fetchManifest(driver)
          return manifest ? { driver, manifest } : null
        }),
      )
      return rows.filter((r): r is { driver: string; manifest: DriverManifest } => !!r)
    })
  }
  return allManifestsPromise
}

// ---- invoke --------------------------------------------------------------

export interface InvokeRequest {
  view?: string
  action: string
  target?: any
  params?: Record<string, any>
}

/** Raw `invoke` wire call against one driver — returns the
 *  `{"ok":…}` envelope unwrapped to `{ok, result|error}`. */
export async function invokeWire(
  driver: string,
  req: InvokeRequest,
): Promise<{ ok: boolean; result?: ActionResult; error?: string }> {
  try {
    const d: any = await $fetch('/api/call', {
      method: 'POST',
      query: { driver },
      body: { op: 'invoke', ...req },
    })
    if (d?.ok) return { ok: true, result: d.result as ActionResult }
    return { ok: false, error: d?.error ?? 'invoke failed' }
  } catch (e: any) {
    return { ok: false, error: e?.data?.error ?? e?.message ?? String(e) }
  }
}

/** Dispatch a collected action. `driver` defaults to the scoped
 *  action's own; `view` defaults to its view context. `itemId`
 *  becomes the invoke `target` when no explicit target is given —
 *  the tree-row convention (`{"id":"T-012"}`-shaped drivers get
 *  what they expect either way since target is opaque JSON). */
export async function invokeScoped(
  scoped: ScopedAction,
  opts: { target?: any; params?: Record<string, any> } = {},
): Promise<{ ok: boolean; result?: ActionResult; error?: string }> {
  const driver = scoped.driver ?? scoped.action.driver
  if (!driver) return { ok: false, error: 'no driver for action' }
  const target =
    opts.target !== undefined
      ? opts.target
      : scoped.itemId !== undefined
        ? { id: scoped.itemId }
        : undefined
  return invokeWire(driver, {
    view: scoped.view,
    action: scoped.action.id,
    target,
    params: opts.params ?? {},
  })
}

// ---- view-swap registry ---------------------------------------------------

// `useView` registers its `pavoApplyView` callback here keyed by
// the `driver:view` ref, so invoke results can swap the fresh
// ViewResult into whichever surface owns that view (page or
// popover) — including surfaces (the palette) that sit outside the
// provide/inject tree.
const viewAppliers = new Map<string, (vr: any) => void>()

export function registerViewApplier(ref: string, fn: (vr: any) => void): void {
  viewAppliers.set(ref, fn)
}

export function unregisterViewApplier(ref: string, fn: (vr: any) => void): void {
  if (viewAppliers.get(ref) === fn) viewAppliers.delete(ref)
}

/** Apply a returned `view` to the matching live view surface.
 *  Returns true when a surface took it. */
export function applyViewResult(driver: string, viewId: string | undefined, vr: any): boolean {
  const fn = viewAppliers.get(viewId ? `${driver}:${viewId}` : '')
  if (!fn) return false
  fn(vr)
  return true
}

// ---- palette action store -------------------------------------------------

/** What the view page currently offers — the palette merges these
 *  with `contributes.menus[]` from every driver manifest. */
export interface PaletteActionEntry {
  kind: 'action'
  scoped: ScopedAction
  driver: string
}

export interface PaletteMenuEntry {
  kind: 'menu'
  entry: MenuEntry
  driver: string
}

export type PaletteEntry = PaletteActionEntry | PaletteMenuEntry

const paletteState = reactive({
  /** Scoped actions of the view on screen (empty off-view pages). */
  actions: [] as ScopedAction[],
  driver: '' as string,
  /** The live ViewResult — command resolution (`action:` scope
   *  lookup) and `{data.*}`/`{view.*}` interpolation context. */
  viewResult: null as ViewResult | null,
})

/** Called by view surfaces (the `/view/[...ref]` page) whenever
 *  their collected actions or view result change. */
export function setPaletteActions(
  driver: string,
  actions: ScopedAction[],
  viewResult: ViewResult | null = null,
): void {
  paletteState.driver = driver
  paletteState.actions = actions
  paletteState.viewResult = viewResult
}

export function usePaletteActions() {
  return paletteState
}

// ---- collection helper -----------------------------------------------------

/** Manifest + ViewResult → deduped scoped actions for a surface.
 *  One call shared by the view page (ActionBar) and the palette
 *  registry. */
export async function collectViewActions(
  driver: string,
  viewResult: any,
): Promise<ScopedAction[]> {
  const manifest = await fetchManifest(driver)
  if (!manifest || !viewResult) return []
  return dedupeActions(collectActions(manifest, viewResult))
}
