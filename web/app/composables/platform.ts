// Platform chrome + feeds — typed wrappers over the `pavo serve`
// bridge endpoints the shell composes: `/api/chrome` (merged
// contribution slots), `/api/jobs`, `/api/notifications`, and
// `/api/views` (for resolving contribution `route`s to view refs).
//
// Wire shapes (see crates/pavo-cli/src/serve.rs — the source of
// truth):
//   /api/chrome        → { ok, result: { navbar, dashboard,
//                          statusbar, routes, sidebar,
//                          headerActions, notifications, jobs,
//                          extra } } — each slot a SlotEntry[]
//   /api/jobs          → { ok, result: JobRow[] }  (JobItem fields
//                          flattened with driver/driver_title)
//   /api/notifications → { ok, result: NotificationRow[] }
//   /api/views         → { ok, result: ViewListEntry[] }
import type { ViewLink, ViewListEntry } from '~/utils/view-link'
import { routeToViewLink } from '~/utils/view-link'

// ---- types ---------------------------------------------------------

/** One merged chrome contribution (any slot). */
export interface SlotEntry {
  driver: string
  driver_title?: string
  id: string
  title: string
  route: string
  priority?: number
  icon?: string | null
}

export interface ChromeSlots {
  navbar: SlotEntry[]
  dashboard: SlotEntry[]
  statusbar: SlotEntry[]
  routes: SlotEntry[]
  sidebar: SlotEntry[]
  headerActions: SlotEntry[]
  notifications: SlotEntry[]
  jobs: SlotEntry[]
  extra?: Record<string, SlotEntry[]>
}

/** Flat `/api/jobs` row — JobItem + driver attribution. */
export interface JobRow {
  id: string
  title: string
  status: string
  detail?: string
  route?: string
  updated_at?: string
  driver: string
  driver_title?: string
  [k: string]: any
}

/** Flat `/api/notifications` row — NotificationItem + attribution. */
export interface NotificationRow {
  id: string
  title: string
  body?: string
  severity?: string
  route?: string
  created_at?: string
  read: boolean
  driver: string
  driver_title?: string
  [k: string]: any
}

// ---- fetches ---------------------------------------------------------

export function useChrome() {
  return useFetch('/api/chrome', { server: false })
}

export function useJobs() {
  return useFetch('/api/jobs', { server: false })
}

export function useNotifications() {
  return useFetch('/api/notifications', { server: false })
}

export function useViews() {
  return useFetch('/api/views', { server: false })
}

// ---- route resolution -------------------------------------------------

// Module-level cache — the views list is small and every chrome
// surface resolves routes against it; one fetch per page load.
let viewsIndexPromise: Promise<ViewListEntry[]> | null = null

/** The `/api/views` list, fetched lazily once (client-side callers
 *  only — guard with `import.meta.client`/onMounted during SSR). */
export function fetchViewIndex(): Promise<ViewListEntry[]> {
  if (!viewsIndexPromise) {
    viewsIndexPromise = $fetch('/api/views')
      .then((d: any) => {
        const rows = d?.result ?? d
        return Array.isArray(rows) ? (rows as ViewListEntry[]) : []
      })
      .catch(() => [] as ViewListEntry[])
  }
  return viewsIndexPromise
}

export type RouteTarget =
  | { type: 'view'; link: ViewLink }
  | { type: 'link'; to: string }

/**
 * Resolve a chrome/feed `route` to where the host should send the
 * user: a `ViewLink` (render inline via ViewPopover, or navigate to
 * `viewLinkHref(link)`) when the route names a view, else a plain
 * link target (host routes, driver-owned `routes[]` pages).
 */
export async function resolveRoute(route: string): Promise<RouteTarget> {
  // Synchronous subset first (`/view/…`, `driver:view`) — no fetch.
  const direct = routeToViewLink(route, [])
  if (direct) return { type: 'view', link: direct }
  const views = await fetchViewIndex()
  const link = routeToViewLink(route, views)
  return link ? { type: 'view', link } : { type: 'link', to: route }
}

/** Sync variant for render paths that already hold the views list
 *  (sidebar/jobs pages await `useViews()`/`fetchViewIndex()`). */
export function routeTargetWithViews(
  route: string,
  views: ViewListEntry[],
): RouteTarget {
  const link = routeToViewLink(route, views)
  return link ? { type: 'view', link } : { type: 'link', to: route }
}

// ---- feed helpers -----------------------------------------------------

// Status vocabularies are driver-owned and free-form; "terminal"
// is a display heuristic for the jobs UI, not a protocol rule.
const TERMINAL_STATUS_RE =
  /done|complete|finish|fail|error|cancel|skip|abort/i

/** Heuristic: does this job status look finished (vs active)? */
export function isJobTerminal(status: string): boolean {
  return TERMINAL_STATUS_RE.test(status)
}

/** Count of non-terminal jobs — the header badge number. */
export function activeJobCount(rows: JobRow[]): number {
  return rows.filter((j) => j.status && !isJobTerminal(j.status)).length
}

/** Short relative age for RFC 3339 timestamps (`3m`, `2h`, `5d`);
 *  falls back to the raw string when unparseable. */
export function timeAgo(iso: string | undefined | null): string {
  if (!iso) return ''
  const t = Date.parse(iso)
  if (Number.isNaN(t)) return iso
  const s = Math.max(0, (Date.now() - t) / 1000)
  if (s < 60) return `${Math.floor(s)}s`
  if (s < 3600) return `${Math.floor(s / 60)}m`
  if (s < 86400) return `${Math.floor(s / 3600)}h`
  if (s < 86400 * 30) return `${Math.floor(s / 86400)}d`
  return iso.slice(0, 10)
}

/** Severity → css class suffix for notification dots. */
export function severityClass(severity: string | undefined): string {
  const s = (severity ?? '').toLowerCase()
  if (/err|crit|fatal|alert/.test(s)) return 'sev-error'
  if (/warn|caution/.test(s)) return 'sev-warning'
  if (/ok|success|info|note/.test(s)) return 'sev-info'
  return 'sev-neutral'
}
