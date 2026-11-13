// The unified command layer — TS mirror of
// `crates/pavo-core/src/commands.rs`. Everything invocable resolves
// to one grammar, one resolver, every host:
//
//   action:<id>              invoke the action (driver dispatch)
//   route:<path>             host navigation (template over target/params)
//   host.<builtin>           host capability (filter, help, reload, …)
//   driver:<name>:<id>       cross-driver action
//
// `parseCommand` turns a command string into a `Command`;
// `resolveCommand` maps it plus a `ResolveContext` to a `Resolved`
// the host executes: `dispatch` (invoke wire op), `navigate`
// (interpolated route — host-local, no wire round-trip), `builtin`
// (host capability), or `unsupported` (display-only).
//
// Route templates (`{a.b}` placeholders) are interpolated by
// `interpolate` over a JSON-ish context — `interpolateCtx` builds
// the shape (`{driver, view, data, target, params, item}`), same
// rules as `pavo_core::commands::interpolate`: dot paths index
// object keys / array positions, missing keys → empty string,
// `{{`/`}}` escape literal braces, lone braces stay literal.
//
// Dependency-free — pure functions only; navigation itself lives
// in `composables/platform.ts` (`useNavigateRoute`).
import type {
  Action,
  DriverManifest,
  MenuEntry,
  ScopedAction,
  ViewResult,
} from '~/utils/interactions'
import { collectActions } from '~/utils/interactions'

// ---- host builtins -------------------------------------------------------

export type HostBuiltin =
  | 'filter'
  | 'help'
  | 'reload'
  | 'editor'
  | 'palette'
  | 'quit'

export interface BuiltinDecl {
  builtin: HostBuiltin
  /** Canonical command id (`host.<name>`). */
  id: string
  label: string
  /** Suggested default keys — declarations suggest; a host keymap
   *  overrides by command id. Display-only on the web (no keymap). */
  keys: string[]
  description: string
}

/** The host-builtin catalog — mirrors `BUILTINS` in commands.rs.
 *  Order is display order. */
export const BUILTINS: BuiltinDecl[] = [
  {
    builtin: 'filter',
    id: 'host.filter',
    label: 'Filter view',
    keys: ['/'],
    description:
      "Filter the current view's filter param (default \"q\") and re-resolve.",
  },
  {
    builtin: 'help',
    id: 'host.help',
    label: 'Help',
    keys: ['?'],
    description: 'Show this help / keybinding list.',
  },
  {
    builtin: 'reload',
    id: 'host.reload',
    label: 'Reload view',
    keys: ['R'],
    description: 'Re-resolve the current view, bypassing caches.',
  },
  {
    builtin: 'editor',
    id: 'host.editor',
    label: 'Open in editor',
    keys: ['e'],
    description: 'Open the current target in $EDITOR (host-dependent).',
  },
  {
    builtin: 'palette',
    id: 'host.palette',
    label: 'Command palette',
    keys: ['ctrl+p'],
    description: 'Open the fuzzy command palette.',
  },
  {
    builtin: 'quit',
    id: 'host.quit',
    label: 'Quit',
    keys: ['q'],
    description: 'Exit pavo.',
  },
]

const BUILTIN_BY_NAME: Record<string, HostBuiltin> = {
  filter: 'filter',
  help: 'help',
  reload: 'reload',
  editor: 'editor',
  palette: 'palette',
  quit: 'quit',
}

// ---- Command --------------------------------------------------------------

export type Command =
  /** `action:<id>` — dispatch on the *current* driver. */
  | { type: 'action'; id: string }
  /** `route:<path>` — host-local navigation; `<path>` is a route
   *  template interpolated over the invoke context. */
  | { type: 'route'; template: string }
  /** `host.<builtin>` — a host capability. */
  | { type: 'host'; builtin: HostBuiltin }
  /** `driver:<name>:<id>` — an action on a *named* driver. */
  | { type: 'driver'; driver: string; id: string }

/** Parse the command grammar — `null` for anything outside the
 *  four namespaces (unknown namespaces degrade to display-only). */
export function parseCommand(spec: string): Command | null {
  const s = spec.trim()
  if (!s) return null
  if (s.startsWith('action:')) {
    const id = s.slice('action:'.length)
    return id ? { type: 'action', id } : null
  }
  if (s.startsWith('route:')) {
    const template = s.slice('route:'.length)
    return template ? { type: 'route', template } : null
  }
  if (s.startsWith('driver:')) {
    const rest = s.slice('driver:'.length)
    const idx = rest.indexOf(':')
    if (idx === -1) return null
    const driver = rest.slice(0, idx)
    const id = rest.slice(idx + 1)
    return driver && id ? { type: 'driver', driver, id } : null
  }
  if (s.startsWith('host.')) {
    const name = s.slice('host.'.length)
    const builtin = BUILTIN_BY_NAME[name]
    return builtin ? { type: 'host', builtin } : null
  }
  return null
}

/** The canonical command-id string — round-trips through
 *  `parseCommand`; the spelling keymaps bind by. */
export function commandId(cmd: Command): string {
  switch (cmd.type) {
    case 'action':
      return `action:${cmd.id}`
    case 'route':
      return `route:${cmd.template}`
    case 'host':
      return BUILTINS.find((b) => b.builtin === cmd.builtin)?.id ?? `host.${cmd.builtin}`
    case 'driver':
      return `driver:${cmd.driver}:${cmd.id}`
  }
}

// ---- interpolation ---------------------------------------------------------

/** Dot-path lookup over a JSON-ish value — see `interpolate`. */
function lookup(ctx: unknown, path: string): string {
  if (!path) return ''
  let cur: any = ctx
  for (const seg of path.split('.')) {
    if (cur === null || cur === undefined) return ''
    if (Array.isArray(cur)) {
      if (!/^\d+$/.test(seg)) return ''
      cur = cur[Number(seg)]
    } else if (typeof cur === 'object') {
      cur = (cur as Record<string, any>)[seg]
    } else {
      return ''
    }
    if (cur === undefined) return ''
  }
  return valueString(cur)
}

/** JSON value → interpolated string (Rust `value_string`). */
function valueString(v: any): string {
  if (v === null || v === undefined) return ''
  if (typeof v === 'string') return v
  if (
    typeof v === 'number' ||
    typeof v === 'boolean' ||
    typeof v === 'bigint'
  ) {
    return String(v)
  }
  try {
    return JSON.stringify(v)
  } catch {
    return ''
  }
}

/** Interpolate a route template — `{path.to.value}` placeholders
 *  resolved against `ctx`. Rules mirror `interpolate` in
 *  commands.rs: missing keys/nulls/non-container traversals →
 *  empty string; strings verbatim, numbers/bools JSON-spelled,
 *  arrays/objects compact JSON; `{{`/`}}` escape literal braces;
 *  a lone `{` or `}` stays literal. */
export function interpolate(template: string, ctx: unknown): string {
  let out = ''
  let rest = template
  for (;;) {
    const pos = rest.search(/[{}]/)
    if (pos === -1) {
      out += rest
      break
    }
    out += rest.slice(0, pos)
    const tail = rest.slice(pos)
    if (tail.startsWith('{{')) {
      out += '{'
      rest = tail.slice(2)
      continue
    }
    if (tail.startsWith('}}')) {
      out += '}'
      rest = tail.slice(2)
      continue
    }
    if (tail.startsWith('}')) {
      out += '}'
      rest = tail.slice(1)
      continue
    }
    // '{' — find the closing '}'.
    const end = tail.indexOf('}', 1)
    if (end === -1) {
      out += '{'
      rest = tail.slice(1)
      continue
    }
    out += lookup(ctx, tail.slice(1, end).trim())
    rest = tail.slice(end + 1)
  }
  return out
}

/** A `TreeItem` as interpolation JSON — `{item.id}`, `{item.label}`,
 *  `{item.detail}`, `{item.icon}`, plus every `props` key flattened
 *  to top level. Mirrors `item_value` in commands.rs (`children`/
 *  `actions` are *not* included). */
export function itemValue(item: Record<string, any>): Record<string, any> {
  const map: Record<string, any> = { ...(item.props ?? {}) }
  if (item.id !== undefined && item.id !== null) map.id = item.id
  map.label = item.label
  if (item.detail !== undefined && item.detail !== null) map.detail = item.detail
  if (item.icon !== undefined && item.icon !== null) map.icon = item.icon
  return map
}

/** The interpolation context — what `interpolate` resolves `{a.b}`
 *  paths against. Mirrors `interpolate_ctx_value`: `driver` always
 *  present; `view` (the ViewSpec id) + `data` (the view's
 *  query-binding map) when a view result exists; `target`, `params`,
 *  `item` when supplied. */
export function interpolateCtx(opts: {
  driver?: string
  view?: string
  data?: Record<string, any>
  target?: any
  params?: Record<string, any>
  item?: any
}): Record<string, any> {
  const map: Record<string, any> = {
    driver: opts.driver ?? '',
    params: opts.params ?? {},
  }
  if (opts.view !== undefined) map.view = opts.view
  if (opts.data !== undefined) map.data = opts.data
  if (opts.target !== undefined) map.target = opts.target
  if (opts.item !== undefined) map.item = opts.item
  return map
}

// ---- menu entries -----------------------------------------------------------

/** The command a `MenuEntry` resolves to — `command` wins when
 *  present; else the legacy pair: `action` (`"<id>"` →
 *  `action:<id>`, `"<driver>:<id>"` → `driver:<driver>:<id>`) or
 *  `route` (`route:<path>`). `null` = display-only entry.
 *  Mirrors `menu_command` in commands.rs. */
export function menuCommand(entry: MenuEntry): Command | null {
  if (entry.command) {
    const cmd = parseCommand(entry.command)
    if (cmd) return cmd
    // Unparseable command — fall through to legacy fields rather
    // than dropping the entry entirely.
  }
  const action = entry.action
  if (action) {
    const idx = action.indexOf(':')
    if (idx > 0) {
      const driver = action.slice(0, idx)
      const id = action.slice(idx + 1)
      if (driver && id) return { type: 'driver', driver, id }
    }
    return { type: 'action', id: action }
  }
  if (entry.route) {
    return parseCommand(`route:${entry.route}`)
  }
  return null
}

// ---- resolve ---------------------------------------------------------------

/** What `resolveCommand` needs — the TS twin of `ResolveContext`.
 *  `viewResult` enables node/item-scoped action lookup and `data`
 *  in interpolation; `drivers` is the lookup table for
 *  `driver:<name>:<id>` (manifests of all discovered drivers). */
export interface ResolveContext {
  /** Current driver kind — default dispatch target for `action:`. */
  driver: string
  manifest?: DriverManifest | null
  viewResult?: ViewResult | null
  drivers?:
    | ReadonlyMap<string, DriverManifest>
    | { driver: string; manifest: DriverManifest }[]
  target?: any
  params?: Record<string, any>
}

/** The outcome of `resolveCommand` — what the host executes. */
export type Resolved =
  | {
      type: 'dispatch'
      driver: string
      view?: string
      action: string
      /** The declared Action when found in scope (params/danger UX);
       *  `null` for undeclared ids — dispatch anyway, the driver
       *  answers `ok:false` on unknowns. */
      declared: Action | null
      target?: any
      params: Record<string, any>
    }
  | { type: 'navigate'; route: string }
  | { type: 'builtin'; builtin: HostBuiltin }
  | { type: 'unsupported' }

interface FoundAction {
  action: Action
  view?: string
  itemId?: string
}

const RESOLVE_SCOPE_RANK = { manifest: 1, node: 2, item: 3 } as const

/** Find `id` among collected scoped actions — highest scope
 *  precedence wins (item > node > manifest). With no view result,
 *  manifest scope only. Mirrors `lookup_action`. */
function lookupAction(
  manifest: DriverManifest | null | undefined,
  viewResult: ViewResult | null | undefined,
  id: string,
): FoundAction | null {
  if (viewResult) {
    let best: ScopedAction | null = null
    for (const s of collectActions(manifest, viewResult)) {
      if (s.action.id !== id) continue
      if (!best || RESOLVE_SCOPE_RANK[s.scope] > RESOLVE_SCOPE_RANK[best.scope]) {
        best = s
      }
    }
    return best
      ? { action: best.action, view: best.view, itemId: best.itemId }
      : null
  }
  const a = manifest?.actions?.find((x) => x.id === id)
  return a ? { action: a } : null
}

function driversMap(
  drivers: ResolveContext['drivers'],
): ReadonlyMap<string, DriverManifest> {
  if (!drivers) return new Map()
  if (Array.isArray(drivers)) {
    const m = new Map<string, DriverManifest>()
    for (const d of drivers) m.set(d.driver, d.manifest)
    return m
  }
  return drivers
}

function resolveAction(
  driver: string,
  manifest: DriverManifest | null | undefined,
  viewResult: ViewResult | null | undefined,
  id: string,
  ctx: ResolveContext,
): Resolved {
  const found = lookupAction(manifest, viewResult, id)
  const viewId = viewResult?.view?.id
  const data = viewResult?.data
  // An action declaring `route` navigates instead of dispatching —
  // interpolate over the invoke context (item = {"id":…} for
  // item-scoped finds).
  if (found?.action.route) {
    const item =
      found.itemId !== undefined ? { id: found.itemId } : undefined
    return {
      type: 'navigate',
      route: interpolate(
        found.action.route,
        interpolateCtx({
          driver,
          view: viewId,
          data,
          target: ctx.target,
          params: ctx.params,
          item,
        }),
      ),
    }
  }
  return {
    type: 'dispatch',
    driver,
    view: found?.view ?? viewId,
    action: id,
    declared: found?.action ?? null,
    target:
      found?.itemId !== undefined ? { id: found.itemId } : ctx.target,
    params: ctx.params ?? {},
  }
}

/** Resolve a `Command` against the current context — the single
 *  entry point for palette activation and menu entries. See the
 *  module docs for the `Resolved` variants. Mirrors
 *  `resolve_command`. */
export function resolveCommand(cmd: Command, ctx: ResolveContext): Resolved {
  switch (cmd.type) {
    case 'host':
      return { type: 'builtin', builtin: cmd.builtin }
    case 'route':
      return {
        type: 'navigate',
        route: interpolate(
          cmd.template,
          interpolateCtx({
            driver: ctx.driver,
            view: ctx.viewResult?.view?.id,
            data: ctx.viewResult?.data,
            target: ctx.target,
            params: ctx.params,
          }),
        ),
      }
    case 'action':
      return resolveAction(ctx.driver, ctx.manifest, ctx.viewResult, cmd.id, ctx)
    case 'driver':
      if (cmd.driver === ctx.driver) {
        // Same driver — full scope (node/item actions too).
        return resolveAction(ctx.driver, ctx.manifest, ctx.viewResult, cmd.id, ctx)
      }
      {
        const manifest = driversMap(ctx.drivers).get(cmd.driver)
        // Foreign driver: manifest scope only — no ViewResult.
        return manifest
          ? resolveAction(cmd.driver, manifest, null, cmd.id, ctx)
          : { type: 'unsupported' }
      }
  }
}

/** Resolve an already-collected [`ScopedAction`] (what
 *  `useActionFlow.activate` receives) — the activation fast path:
 *  `Action.route` navigates instead of dispatching, everything
 *  else becomes a dispatch ready for `invokeWire`. Mirrors the
 *  `resolve_action` route branch + `ResolvedInvoke` assembly. */
export function resolveScopedAction(
  scoped: ScopedAction,
  opts: { target?: any; params?: Record<string, any> } = {},
): Resolved {
  const action = scoped.action
  const driver = scoped.driver ?? action.driver ?? ''
  const target =
    opts.target !== undefined
      ? opts.target
      : scoped.itemId !== undefined
        ? { id: scoped.itemId }
        : undefined
  const params = opts.params ?? {}
  const item = scoped.itemId !== undefined ? { id: scoped.itemId } : undefined
  if (action.route) {
    return {
      type: 'navigate',
      route: interpolate(
        action.route,
        interpolateCtx({
          driver,
          view: scoped.view,
          data: scoped.data,
          target,
          params,
          item,
        }),
      ),
    }
  }
  return {
    type: 'dispatch',
    driver,
    view: scoped.view,
    action: action.id,
    declared: action,
    target,
    params,
  }
}
