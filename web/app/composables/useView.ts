// Shared view fetch — the single pipeline behind both the
// `/view/[...ref]` page and chrome surfaces that embed a view
// (header-action popovers). Client-side only (`server: false`):
// the SSR shell streams first, each island hydrates itself.
//
// Also provides the form-submit context (`pavoViewRef` /
// `pavoApplyView`, consumed by FormView.vue) so forms work
// identically in pages and popovers.
import type { ComputedRef, MaybeRefOrGetter, Ref } from 'vue'
import { computed, provide, toValue } from 'vue'
import type { ViewParam } from '~/utils/view-link'

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
  provide('pavoApplyView', (vr: any) => {
    if (data.value) {
      data.value = { ok: true, result: vr } as typeof data.value
    }
  })

  return { data, pending, error, refresh, apiError, viewResult }
}
