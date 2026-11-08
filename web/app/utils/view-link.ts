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
