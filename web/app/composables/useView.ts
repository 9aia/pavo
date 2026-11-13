// Shared view fetch — the single pipeline behind both the
// `/view/[...ref]` page and chrome surfaces that embed a view
// (header-action popovers). Client-side only (`server: false`):
// the SSR shell streams first, each island hydrates itself.
//
// Also provides the form-submit context (`pavoViewRef` /
// `pavoApplyView`, consumed by FormView.vue) so forms work
// identically in pages and popovers.
import type { ComputedRef, MaybeRefOrGetter, Ref } from 'vue'
import { computed, onBeforeUnmount, provide, toValue, watch } from 'vue'
import type { ViewParam } from '~/utils/view-link'
import { registerViewApplier, unregisterViewApplier } from '~/composables/actions'

export function useView(
  ref: MaybeRefOrGetter<string>,
  params: MaybeRefOrGetter<ViewParam[]> = [],
) {
  // Normalize params into the bridge's `param.<k>=<v>` wire form —
  // same rule as the CLI's `--param k=v`.
  const apiUrl = computed(() => {
    const r = toValue(ref)
    const ps = toValue(params)
    const paramQs = ps
      .map(
        ([k, v]) =>
          `${encodeURIComponent(`param.${k}`)}=${encodeURIComponent(v)}`,
      )
      .join('&')
    return `/api/view?ref=${encodeURIComponent(r)}${paramQs ? `&${paramQs}` : ''}`
  })

  // No `await`: callers include v-if-mounted components (chrome
  // popovers) that can't suspend — useFetch returns live refs
  // immediately and the client-side fetch fills them in.
  const { data, pending, error, refresh } = useFetch(apiUrl, {
    server: false,
    watch: [apiUrl],
    // The bridge answers domain errors (unknown driver/view) with a
    // non-2xx status AND an `{ok:false,error}` envelope — don't throw,
    // let the envelope message render.
    ignoreResponseError: true,
  })

  const viewResult = computed(() => {
    const d = data.value as { ok?: boolean; result?: any; error?: string } | null
    return d?.ok ? d.result : null
  })
  const apiError = computed(() => {
    const d = data.value as { ok?: boolean; error?: string } | null
    if (error.value) return error.value.message
    if (d && d.ok === false) return d.error || 'request failed'
    return null
  })

  // Form submits (`/api/submit`) need the view ref; a returned
  // `view` swaps the whole ViewResult in place.
  const viewRef = computed(() => toValue(ref))
  provide<Ref<string> | ComputedRef<string>>('pavoViewRef', viewRef)
  const applyView = (vr: any) => {
    if (data.value) {
      data.value = { ok: true, result: vr } as typeof data.value
    }
  }
  provide('pavoApplyView', applyView)

  // Action-dispatch context for deeply nested surfaces (tree-row
  // ItemActions inside ViewNode): `{driver, view}` — the invoke
  // op's driver target and view-id context. `view` prefers the
  // resolved ViewSpec id, falling back to the ref's view segment.
  provide(
    'pavoActionCtx',
    computed(() => {
      const r = viewRef.value
      const [d, v] = r.includes(':') ? r.split(/:(.*)/s) : [r, '']
      const specId = (data.value as any)?.result?.view?.id as
        | string
        | undefined
      return { driver: d || '', view: specId || v || undefined }
    }),
  )

  // `ViewSpec.item_route` — the per-view tree-row navigation
  // template, plus the interpolation context (`{driver, view, data}`
  // — `item` is supplied per row by ItemLink.vue). Consumed by
  // ViewNode tree rendering; absent template → rows stay inert.
  provide(
    'pavoItemRoute',
    computed(
      () => (viewResult.value?.view as any)?.item_route ?? null,
    ),
  )
  provide(
    'pavoNavCtx',
    computed(() => {
      const d = data.value as any
      const r = viewRef.value
      return {
        driver: r.split(':')[0] ?? '',
        view: d?.result?.view?.id as string | undefined,
        data: d?.result?.data as Record<string, any> | undefined,
      }
    }),
  )

  // Register the swap callback in the module registry too — invoke
  // results dispatched outside this component's tree (the command
  // palette) reach it via `applyViewResult(driver, viewId, vr)`.
  // The key is the `driver:view` ref; watch covers ref changes.
  watch(
    viewRef,
    (r, _old, onCleanup) => {
      if (!r) return
      registerViewApplier(r, applyView)
      onCleanup(() => unregisterViewApplier(r, applyView))
    },
    { immediate: true },
  )
  onBeforeUnmount(() => unregisterViewApplier(viewRef.value, applyView))

  // Alias registration: invoke results key the swap by the *spec*
  // view id (`driver:<ViewSpec.id>`), which can differ from the
  // URL's view segment — register that spelling too once the
  // ViewResult lands.
  let aliasKey: string | null = null
  watch(
    () => (data.value as any)?.result?.view?.id as string | undefined,
    (specId) => {
      if (!specId) return
      const driver = viewRef.value.split(':')[0]
      if (!driver) return
      const alias = `${driver}:${specId}`
      if (alias === viewRef.value || alias === aliasKey) return
      if (aliasKey) unregisterViewApplier(aliasKey, applyView)
      aliasKey = alias
      registerViewApplier(alias, applyView)
    },
  )
  onBeforeUnmount(() => {
    if (aliasKey) unregisterViewApplier(aliasKey, applyView)
  })

  // Auto-refresh — `ViewSpec.refresh_ms` (snake_case on the wire)
  // declares the view's data goes stale on a timer; poll the same
  // endpoint on that interval. Same 1s–5min bounds the TUI
  // enforces; the timer re-arms whenever the resolved spec (or the
  // ref, via `viewResult`) changes, and stops on unmount.
  const MIN_REFRESH_MS = 1000
  const MAX_REFRESH_MS = 300_000
  let pollTimer: ReturnType<typeof setInterval> | null = null
  const stopPolling = () => {
    if (pollTimer !== null) {
      clearInterval(pollTimer)
      pollTimer = null
    }
  }
  watch(
    () => (viewResult.value?.view as any)?.refresh_ms as
      | number
      | undefined,
    (ms) => {
      stopPolling()
      if (
        typeof ms === 'number' &&
        ms >= MIN_REFRESH_MS &&
        ms <= MAX_REFRESH_MS
      ) {
        pollTimer = setInterval(() => {
          void refresh()
        }, ms)
      }
    },
    { immediate: true },
  )
  onBeforeUnmount(stopPolling)

  return { data, pending, error, refresh, apiError, viewResult }
}
