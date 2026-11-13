// Interaction-protocol types + pure helpers — the TS mirror of
// `crates/pavo-protocol/src/{action,manifest,view_node,wire}.rs`
// and the collection semantics of `crates/pavo-core/src/actions.rs`.
//
// Wire contract: `protocol/v1/README.md` → "Interaction actions
// (`invoke`)". All shapes are open interfaces — unknown fields are
// ignored (the protocol's forward-compat rule), so never rely on
// exhaustiveness here.

// ---- wire types ------------------------------------------------------

export type ActionDanger = 'none' | 'confirm' | 'type-target'

export type ParamKind =
  | 'string'
  | 'text'
  | 'number'
  | 'integer'
  | 'bool'
  | 'enum'
  | 'date'
  | 'ref'
  | 'string-list'

export interface ParamSchema {
  name: string
  kind?: ParamKind
  required?: boolean
  label?: string
  doc?: string
  hint?: string
  placeholder?: string
  default?: any
  pattern?: string
  min?: number
  max?: number
  options?: string[]
  ref_query?: string
  [k: string]: any
}

export interface Action {
  id: string
  label: string
  icon?: string
  danger?: ActionDanger
  keys?: string[]
  params?: ParamSchema[]
  target?: string
  description?: string
  driver?: string
  /** Navigate-instead-of-dispatch — a route **template** the host
   *  interpolates (`{target.*}`/`{params.*}`/`{item.*}`) and
   *  navigates to locally when the action is activated, *instead of*
   *  sending `invoke`. `danger`/`params` still gate host-side. */
  route?: string
  [k: string]: any
}

/** `invoke` result — same shape as SubmitResult (message/errors/
 *  data/fresh view). `errors` is keyed by ParamSchema.name. */
export interface ActionResult {
  message?: string
  errors?: Record<string, any>
  data?: any
  view?: ViewResult
  /** Driver-requested redirect — a route **template** (same `{…}`
   *  syntax/context as `Action.route`) the host interpolates over
   *  the invoke context and navigates to *after* applying
   *  `view`/`data`. */
  route?: string
  [k: string]: any
}

export interface MenuEntry {
  id: string
  title: string
  icon?: string
  action?: string
  route?: string
  /** Generic command target — the unified command grammar
   *  (`action:<id>` | `route:<path>` | `host.<builtin>` |
   *  `driver:<name>:<id>`; see `utils/commands.ts`). When present it
   *  **wins** over `action`/`route`. */
  command?: string
  /** Suggested keyboard bindings — display-only on the web (no
   *  keymap); shown in palette hints. */
  keys?: string[]
  keywords?: string[]
  section?: string
  priority?: number
  [k: string]: any
}

/** The slice of DriverManifest the interaction layer reads — open
 *  interface, everything else passes through untouched. */
export interface DriverManifest {
  name: string
  title?: string
  actions?: Action[]
  contributes?: { menus?: MenuEntry[]; [k: string]: any }
  views?: { id: string; [k: string]: any }[]
  [k: string]: any
}

export interface ViewSpec {
  id: string
  title?: string
  route?: string
  description?: string
  queries?: any[]
  root?: ViewNode
  /** Per-view row-navigation template — when set, activating a tree
   *  item navigates the host to this interpolated route
   *  (`{item.id}`, `{item.<prop>}` — see `utils/commands.ts`). */
  item_route?: string
  /** Names this view's free-text filter query param — convention
   *  `"q"`. The host's filter UI re-resolves the view with
   *  `params[<filter_param>]` set; absent → fallback `"q"`. */
  filter_param?: string
  [k: string]: any
}

export interface ViewResult {
  view: ViewSpec
  data?: Record<string, any>
  [k: string]: any
}

/** One ViewNode — `kind` plus flattened NodeBase fields (`id`,
 *  `title`, `query`, `description`, `props`, `actions`). Children
 *  ride `children[]`; trees carry `items[]` of TreeItem. */
export interface ViewNode {
  kind?: string
  id?: string
  actions?: Action[]
  children?: ViewNode[]
  items?: TreeItem[]
  [k: string]: any
}

export interface TreeItem {
  id?: string
  label?: string
  children?: TreeItem[]
  actions?: Action[]
  [k: string]: any
}

// ---- collection (mirror of pavo-core actions.rs) ----------------------

export type ActionScope = 'manifest' | 'node' | 'item'

/** An Action plus the context needed to dispatch it — the TS twin
 *  of `ScopedAction`. `view` is the ViewSpec id (invoke's `view`
 *  context); `itemId` is the tree row id for item-scoped actions. */
export interface ScopedAction {
  action: Action
  scope: ActionScope
  view?: string
  itemId?: string
  /** Owning driver name — filled by the caller (the manifest's
   *  `name`, or the ref prefix when collecting ad hoc). */
  driver?: string
  /** The view's query-binding data map — `{data.*}` in route
   *  template interpolation (`Action.route`, `ActionResult.route`). */
  data?: Record<string, any>
}

function collectTreeItems(
  items: TreeItem[] | undefined,
  view: string | undefined,
  data: Record<string, any> | undefined,
  out: ScopedAction[],
): void {
  for (const item of items ?? []) {
    for (const action of item.actions ?? []) {
      out.push({ action, scope: 'item', view, itemId: item.id, data })
    }
    collectTreeItems(item.children, view, data, out)
  }
}

/** Pre-order walk over `children[]` — every container kind
 *  (panel/row/column) nests nodes there. */
function walkNodes(node: ViewNode | undefined, fn: (n: ViewNode) => void): void {
  if (!node || typeof node !== 'object') return
  fn(node)
  for (const c of node.children ?? []) walkNodes(c, fn)
}

/**
 * The effective item list of a tree node — inline `items[]` wins;
 * a `query`-bound node takes `data[query].items` (or a bare array)
 * in the same TreeItem shape. Same precedence the renderers use.
 */
function effectiveTreeItems(
  node: ViewNode,
  data: Record<string, any> | undefined,
): TreeItem[] | undefined {
  if (Array.isArray(node.items) && node.items.length) return node.items
  const q = node.query
  if (!q || !data) return undefined
  const d = data[q]
  if (Array.isArray(d)) return d as TreeItem[]
  if (Array.isArray(d?.items)) return d.items as TreeItem[]
  return undefined
}

/**
 * Every action visible in `viewResult`, in declaration order —
 * manifest-global first, then node actions (pre-order), then
 * tree-item actions (pre-order per tree). Mirrors
 * `pavo_core::actions::collect_actions`; hosts dedupe with
 * [`dedupeActions`] (item > node > manifest on id collisions).
 */
export function collectActions(
  manifest: DriverManifest | null | undefined,
  viewResult: ViewResult | null | undefined,
): ScopedAction[] {
  const out: ScopedAction[] = []
  const view = viewResult?.view?.id
  const driver = manifest?.name
  const data = viewResult?.data
  for (const action of manifest?.actions ?? []) {
    out.push({ action, scope: 'manifest', view, driver, data })
  }
  walkNodes(viewResult?.view?.root, (node) => {
    for (const action of node.actions ?? []) {
      out.push({ action, scope: 'node', view, driver, data })
    }
    if (node.kind === 'tree' || Array.isArray(node.items)) {
      collectTreeItems(effectiveTreeItems(node, data), view, data, out)
    }
  })
  return out
}

const SCOPE_RANK: Record<ActionScope, number> = { manifest: 0, node: 1, item: 2 }

/**
 * Dedupe by `action.id` — the more specific scope wins on
 * collision (item > node > manifest), first occurrence within a
 * scope keeps its position.
 */
export function dedupeActions(actions: ScopedAction[]): ScopedAction[] {
  const best = new Map<string, ScopedAction>()
  for (const sa of actions) {
    const id = sa.action.id
    const prev = best.get(id)
    if (!prev || SCOPE_RANK[sa.scope] > SCOPE_RANK[prev.scope]) {
      best.set(id, sa)
    }
  }
  // Preserve first-seen order of the *winning* entries.
  const order = new Map<string, number>()
  actions.forEach((sa, i) => {
    if (!order.has(sa.action.id)) order.set(sa.action.id, i)
  })
  return [...best.values()].sort(
    (a, b) => (order.get(a.action.id) ?? 0) - (order.get(b.action.id) ?? 0),
  )
}

// ---- params: coercion + validation ------------------------------------

function empty(v: any): boolean {
  return v === undefined || v === null || v === ''
}

/**
 * Raw form value → the JSON value submitted in invoke `params`.
 * `integer`/`number` → numbers when parseable; `bool` → boolean;
 * `string-list` → comma-split array; everything else → string.
 */
export function coerceParam(param: ParamSchema, raw: any): any {
  const kind = param.kind ?? 'string'
  if (kind === 'bool') return !!raw
  if (kind === 'string-list') {
    if (Array.isArray(raw)) return raw.map(String)
    return String(raw ?? '')
      .split(',')
      .map((s) => s.trim())
      .filter((s) => s !== '')
  }
  if ((kind === 'number' || kind === 'integer') && !empty(raw)) {
    const n = Number(raw)
    if (!Number.isNaN(n)) return kind === 'integer' ? Math.trunc(n) : n
  }
  return raw
}

/**
 * Should this param ride the invoke `params` map? Required params
 * and non-empty values always do; `bool` always does (false is a
 * real value); untouched optionals are omitted so drivers don't
 * see spurious empty strings.
 */
export function paramIncluded(param: ParamSchema, raw: any): boolean {
  const kind = param.kind ?? 'string'
  if (kind === 'bool') return true
  if (param.required) return true
  const v = coerceParam(param, raw)
  if (kind === 'string-list') return Array.isArray(v) && v.length > 0
  return !empty(v)
}

/**
 * Client-side validation — `required`, `pattern`, `min`/`max`,
 * `options` membership. Returns per-param error strings keyed by
 * `ParamSchema.name` (empty map = valid). Mirrors
 * `pavo_core::actions::validate_params`; drivers stay
 * authoritative — `ActionResult.errors` is the truth.
 */
export function validateParams(
  action: Action,
  values: Record<string, any>,
): Record<string, string> {
  const errors: Record<string, string> = {}
  for (const p of action.params ?? []) {
      const raw = values[p.name]
      const missing =
        raw === undefined ||
        raw === null ||
        (typeof raw === 'string' && raw === '') ||
        (Array.isArray(raw) && raw.length === 0)
      if (p.required && missing) {
        errors[p.name] = 'required'
        continue
      }
      if (missing) continue
      const v = coerceParam(p, raw)
      // enum/options membership
      if (p.options?.length) {
        const ok = Array.isArray(v)
          ? v.every((x) => p.options!.includes(String(x)))
          : p.options.includes(String(v))
        if (!ok) {
          errors[p.name] = 'invalid option'
          continue
        }
      }
      // numeric bounds
      if (typeof v === 'number') {
        if ((p.min !== undefined && v < p.min) || (p.max !== undefined && v > p.max)) {
          errors[p.name] = 'out of range'
          continue
        }
      }
      // pattern (string kinds) — guarded: a bad regex is the
      // driver's problem to reject, not ours to crash on.
      if (p.pattern && (p.kind === undefined || ['string', 'text', 'ref', 'date', 'enum'].includes(p.kind))) {
        try {
          if (!new RegExp(p.pattern).test(String(v))) {
            errors[p.name] = `must match ${p.pattern}`
          }
        } catch {
          /* invalid pattern — skip client check */
        }
      }
  }
  return errors
}

// ---- display helpers ----------------------------------------------------

/** `snake_case`/`kebab-case`/`camelCase` → `Snake case` — same
 *  rule as pavo-core's `humanize`. */
export function humanize(name: string): string {
  let out = ''
  let prevSep = true
  for (let i = 0; i < name.length; i++) {
    const c = name[i]!
    if (c === '_' || c === '-') {
      if (!prevSep && out) out += ' '
      prevSep = true
      continue
    }
    if (/[A-Z]/.test(c) && i > 0 && !prevSep) out += ' '
    out += c.toLowerCase()
    prevSep = false
  }
  if (!out) return name
  return out[0]!.toUpperCase() + out.slice(1)
}

/**
 * The id a `type-target` confirmation asks the user to type.
 * `action.target` is a context hint; the actual target rides the
 * invoke request — derive the expected string from it: bare
 * strings pass through, objects contribute `.id`/`.value`/first
 * scalar, anything else stringifies.
 */
export function targetConfirmId(target: any): string {
  if (target === undefined || target === null) return ''
  if (typeof target === 'string') return target
  if (typeof target === 'number' || typeof target === 'boolean') return String(target)
  if (typeof target === 'object') {
    for (const k of ['id', 'value', 'key', 'name']) {
      const v = (target as Record<string, any>)[k]
      if (typeof v === 'string' || typeof v === 'number') return String(v)
    }
    try {
      return JSON.stringify(target)
    } catch {
      return String(target)
    }
  }
  return String(target)
}

/**
 * Hand-rolled subsequence fuzzy match (dmenu-style, no dep).
 * Returns a score (lower = better) or `null` when `query` isn't a
 * subsequence of `text`. Bonuses: match at start, consecutive
 * chars, word-start hits; penalty per skipped char.
 */
export function fuzzyScore(query: string, text: string): number | null {
  const q = query.trim().toLowerCase()
  const t = text.toLowerCase()
  if (!q) return 0
  if (t.includes(q)) {
    const idx = t.indexOf(q)
    return idx * 2 - (idx === 0 ? 10 : 0)
  }
  let qi = 0
  let score = 0
  let last = -2
  let streak = 0
  for (let ti = 0; ti < t.length && qi < q.length; ti++) {
    if (t[ti] === q[qi]) {
      score += ti - last - 1 // gap penalty
      if (ti === last + 1) {
        streak++
        score -= streak * 2 // consecutive bonus
      } else {
        streak = 0
        if (ti === 0 || /[\s\-_./:]/.test(t[ti - 1]!)) score -= 4 // word-start bonus
      }
      last = ti
      qi++
    }
  }
  return qi === q.length ? score : null
}
