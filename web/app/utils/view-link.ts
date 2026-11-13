// Deep-link grammar for `/view/<driver>:<view>` pages.
//
//   /view/myco:item                      → ref only
//   /view/myco:item/id=T-001             → path params (one `k=v` per
//                                          extra segment; a bare `k`
//                                          means an empty value)
//   /view/myco:item?id=T-001&status=open → bare query keys = params
//   ?param.id=T-001                      → legacy form, still works
//
// Precedence (lowest → highest): path segments < bare query keys <
// `param.*` query keys. The page emits everything to the bridge as
// `param.<k>=<v>` pairs in this order and the server is last-wins,
// so later sources override earlier ones.
import type { LocationQuery } from 'vue-router'

export type ViewParam = [key: string, value: string]

export interface ViewLink {
  /** The `driver:view` (or bare `driver`) ref — the first path segment. */
  ref: string
  /** Ordered (key, value) params; duplicates allowed — downstream is last-wins. */
  params: ViewParam[]
}

/**
 * Parse the catch-all route param (`route.params.ref` — `string[]`
 * for `/view/a/b/c`, possibly a bare string) plus the route query
 * into a ref + ordered param list.
 */
export function parseViewLink(refParam: unknown, query: LocationQuery): ViewLink {
  const segments = Array.isArray(refParam)
    ? refParam
    : refParam == null
      ? []
      : [String(refParam)]
  const ref = segments[0] ?? ''
  const params: ViewParam[] = []

  // Path params: each segment after the ref is `key=value`, split at
  // the first '=' (values may contain '='). No '=' → empty value.
  // Empty keys (`=v`, bare `=`) are dropped, same rule as the bridge.
  for (const seg of segments.slice(1)) {
    const eq = seg.indexOf('=')
    const key = eq === -1 ? seg : seg.slice(0, eq)
    if (key !== '') {
      params.push([key, eq === -1 ? '' : seg.slice(eq + 1)])
    }
  }

  // Query params in two passes so `param.*` always beats a bare key:
  // pass 1 = bare `?k=v`, pass 2 = legacy `?param.k=v` → `k`.
  const entries = Object.entries(query)
  for (const paramNs of [false, true]) {
    for (const [k, v] of entries) {
      const isParamNs = k.startsWith('param.')
      if (isParamNs !== paramNs) continue
      const key = isParamNs ? k.slice('param.'.length) : k
      if (key === '') continue
      for (const val of Array.isArray(v) ? v : [v]) {
        if (val != null) params.push([key, String(val)])
      }
    }
  }

  return { ref, params }
}

/** One row of `GET /api/views` — only the fields route resolution needs. */
export interface ViewListEntry {
  namespaced: string
  route?: string
  title?: string
}

/**
 * `/view/<ref>` href for a parsed link — params ride as path
 * segments (`k=v`, key and value encoded separately so a literal
 * `=` inside a value survives the first-`=` split).
 */
export function viewLinkHref(link: ViewLink): string {
  const segs = link.params.map(
    ([k, v]) => `${encodeURIComponent(k)}=${encodeURIComponent(v)}`,
  )
  return `/view/${encodeURIComponent(link.ref)}${segs.length ? `/${segs.join('/')}` : ''}`
}

/** Convenience for a bare ref. */
export function viewHref(ref: string): string {
  return viewLinkHref({ ref, params: [] })
}

/** A `driver:view` ref with no slashes/spaces — e.g. `myco:item`. */
const VIEW_REF_RE = /^[^/\s:]+:[^/\s:]+$/

/** Percent-decode one component — bad escapes pass through. */
function dec(s: string): string {
  try {
    return decodeURIComponent(s)
  } catch {
    return s
  }
}

/** `a=b&c=d` query string → ordered view params (bare-key form —
 *  same precedence slot as `?k=v` on a `/view/…` URL). Repeated
 *  keys are kept; downstream is last-wins. */
function queryParams(qs: string): ViewParam[] {
  const out: ViewParam[] = []
  if (!qs) return out
  for (const pair of qs.split('&')) {
    if (!pair) continue
    const eq = pair.indexOf('=')
    const key = eq === -1 ? pair : pair.slice(0, eq)
    if (!key) continue
    out.push([dec(key), dec(eq === -1 ? '' : pair.slice(eq + 1))])
  }
  return out
}

/** Split `path?query#hash` → `[path, query]` (hash dropped). */
function splitPathQuery(r: string): [string, string] {
  let end = r.length
  for (const ch of ['?', '#']) {
    const i = r.indexOf(ch)
    if (i !== -1 && i < end) end = i
  }
  const path = r.slice(0, end)
  let qs = r.slice(end)
  if (!qs.startsWith('?')) return [path, '']
  qs = qs.slice(1)
  const hash = qs.indexOf('#')
  return [path, hash === -1 ? qs : qs.slice(0, hash)]
}

/**
 * Resolve a chrome/feed/command `route` string to a view link, or
 * `null` when it isn't a view route.
 *
 * Contribution `route`s are driver-relative view routes
 * (`/demo/board` — the `ViewSpec.route` form), `/view/…` deep links,
 * or bare `driver:view` refs; anything else (host routes like
 * `/dashboard`, driver-owned `routes[]` pages) returns `null` and
 * the caller falls back to plain navigation. Query strings on view
 * routes (`/myco/item?id=T-1` — the `item_route`/`Action.route`
 * template style) ride along as view params.
 *
 * `views` is the `/api/views` list used to map `ViewSpec.route` →
 * `namespaced` ref; pass `[]` for the synchronous subset
 * (`/view/…`, `d:v`) and re-check with the full list when async
 * resolution is available.
 */
export function routeToViewLink(
  route: string,
  views: ViewListEntry[],
): ViewLink | null {
  const r = route.trim()
  if (!r) return null

  // Already a web deep link — reuse the page grammar verbatim.
  if (r.startsWith('/view/')) {
    const [pathPart, qs] = splitPathQuery(r.slice('/view/'.length))
    // vue-router hands the page decoded segments; a raw route string
    // still carries %XX — decode to match (bad escapes pass through).
    const segments = pathPart
      .split('/')
      .filter((s) => s !== '')
      .map(dec)
    if (!segments.length) return null
    const link = parseViewLink(segments, {})
    // `?k=v` on the route string appends after path params (wins).
    link.params.push(...queryParams(qs))
    return link
  }

  // Bare `driver:view` ref.
  if (VIEW_REF_RE.test(r)) return { ref: r, params: [] }

  // ViewSpec.route (`/demo/board`, optionally `?params`) →
  // namespaced ref via /api/views.
  const [path, qs] = splitPathQuery(r)
  const hit = views.find(
    (v) => v.route === path || v.route === path.replace(/\/+$/, ''),
  )
  return hit ? { ref: hit.namespaced, params: queryParams(qs) } : null
}
