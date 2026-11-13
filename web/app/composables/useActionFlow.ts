// Action invocation flow — the state machine behind ActionBar and
// CommandPalette: activate → (params form?) → (danger confirm?) →
// resolve → invoke → message/errors + optional view swap.
//
// Danger handling order: a param form (when the action declares
// `params[]`) collects input first, then `confirm`/`type-target`
// gates the dispatch — the destructive op is confirmed *with* its
// arguments in hand, not before they're known.
//
// Command layer (utils/commands.ts, mirroring pavo-core): an action
// declaring `route` resolves to `navigate` — the template is
// interpolated over the invoke context (`{target.*}`/`{params.*}`/
// `{item.*}`) and the host navigates locally, no wire round-trip.
// `danger`/`params` gates still apply beforehand. An
// `ActionResult.route` redirects *after* a successful invoke (and
// after any returned view swap).
import { reactive } from 'vue'
import type {
  Action,
  ActionResult,
  ScopedAction,
} from '~/utils/interactions'
import { coerceParam, paramIncluded, validateParams } from '~/utils/interactions'
import { interpolate, interpolateCtx, resolveScopedAction } from '~/utils/commands'
import { applyViewResult, invokeWire } from '~/composables/actions'
import { useNavigateRoute } from '~/composables/platform'

export interface ActionFlowState {
  /** Which gate is open, if any. */
  dialog: 'form' | 'confirm' | null
  /** The action being dispatched (null when idle). */
  action: Action | null
  scoped: ScopedAction | null
  /** Params collected by the form gate — carried into confirm/run. */
  params: Record<string, any>
  /** Explicit target override (palette/menu invocations pass one;
   *  item-scoped actions fall back to `{id: itemId}`). */
  target: any
  busy: boolean
  /** Status line — result message or transport error. */
  message: string | null
  /** Server-side per-param errors from ActionResult.errors. */
  errors: Record<string, string>
  /** Client-side validation errors (form gate). */
  localErrors: Record<string, string>
}

export function useActionFlow() {
  const state = reactive<ActionFlowState>({
    dialog: null,
    action: null,
    scoped: null,
    params: {},
    target: undefined,
    busy: false,
    message: null,
    errors: {},
    localErrors: {},
  })

  const navigate = useNavigateRoute()

  function reset(): void {
    state.dialog = null
    state.action = null
    state.scoped = null
    state.params = {}
    state.target = undefined
    state.errors = {}
    state.localErrors = {}
  }

  /** Entry point — a button/palette selection. Decides which gate
   *  (if any) the action needs, then dispatches. */
  function activate(scoped: ScopedAction, target?: any): void {
    if (state.busy) return
    state.message = null
    state.errors = {}
    state.localErrors = {}
    state.scoped = scoped
    state.action = scoped.action
    state.target = target
    state.params = {}
    const hasParams = (scoped.action.params ?? []).length > 0
    if (hasParams) {
      state.dialog = 'form'
    } else if (scoped.action.danger && scoped.action.danger !== 'none') {
      state.dialog = 'confirm'
    } else {
      void run()
    }
  }

  /** Form gate submitted — validate client-side, then either gate
   *  on danger or dispatch. */
  function submitForm(raw: Record<string, any>): void {
    const action = state.action
    if (!action) return
    // Coerce + filter to the params the action declares.
    const params: Record<string, any> = {}
    for (const p of action.params ?? []) {
      if (paramIncluded(p, raw[p.name])) {
        params[p.name] = coerceParam(p, raw[p.name])
      }
    }
    const errs = validateParams(action, params)
    if (Object.keys(errs).length) {
      state.localErrors = errs
      return
    }
    state.localErrors = {}
    state.params = params
    if (action.danger && action.danger !== 'none') {
      state.dialog = 'confirm'
    } else {
      state.dialog = null
      void run()
    }
  }

  /** Confirm gate accepted — dispatch. */
  function confirm(): void {
    state.dialog = null
    void run()
  }

  function cancel(): void {
    if (state.busy) return
    reset()
  }

  async function run(): Promise<void> {
    const scoped = state.scoped
    if (!scoped || state.busy) return
    // Command-layer resolution: `Action.route` → host-local
    // navigation (no invoke); anything else → a dispatch.
    const resolved = resolveScopedAction(scoped, {
      target: state.target,
      params: state.params,
    })
    if (resolved.type === 'navigate') {
      state.busy = false
      reset()
      void navigate(resolved.route)
      return
    }
    if (resolved.type !== 'dispatch') {
      state.busy = false
      state.message = 'unsupported command'
      return
    }
    state.busy = true
    state.message = null
    state.errors = {}
    const res = await invokeWire(resolved.driver, {
      view: resolved.view,
      action: resolved.action,
      target: resolved.target,
      params: resolved.params,
    })
    state.busy = false
    if (!res.ok) {
      state.message = res.error ?? 'invoke failed'
      return
    }
    const result: ActionResult = res.result ?? {}
    state.message = result.message ?? `${scoped.action.label} — done`
    state.errors = Object.fromEntries(
      Object.entries(result.errors ?? {}).map(([k, v]) => [k, String(v)]),
    )
    // Rejected invokes (per-param errors) keep the form context —
    // reopen the form gate so the user can fix and retry.
    if (Object.keys(state.errors).length && (scoped.action.params ?? []).length) {
      state.dialog = 'form'
      return
    }
    // A fresh ViewResult swaps into the owning surface when one is
    // registered for `driver:view`; otherwise the message stands
    // alone (the next manual refresh picks up mutations).
    if (result.view && resolved.driver) {
      applyViewResult(resolved.driver, resolved.view, result.view)
    }
    // Driver-requested redirect — interpolate over the same invoke
    // context and navigate *after* the view swap.
    if (result.route) {
      const route = interpolate(
        result.route,
        interpolateCtx({
          driver: resolved.driver,
          view: resolved.view,
          data: scoped.data,
          target: resolved.target,
          params: resolved.params,
          item:
            scoped.itemId !== undefined ? { id: scoped.itemId } : undefined,
        }),
      )
      reset()
      void navigate(route)
      return
    }
    reset()
  }

  return { state, activate, submitForm, confirm, cancel }
}
