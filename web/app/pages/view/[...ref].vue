<script setup lang="ts">
// Deep-link view page: `/view/<driver>:<view>` plus optional params —
// path segments (`/view/myco:item/id=T-001`), bare query keys
// (`?id=T-001`), or the legacy `?param.id=T-001` form. See
// `app/utils/view-link.ts` for the grammar and precedence; everything
// is forwarded to the driver's view op as `param.<k>=<v>`, same rule
// as the CLI's `--param k=v`.
import { parseViewLink } from '~/utils/view-link'

const route = useRoute()

const link = computed(() => parseViewLink(route.params.ref, route.query))
const ref = computed(() => link.value.ref)

// Normalize every param source into the bridge's `param.<k>=<v>`
// wire form, in precedence order (path < bare query < `param.*`).
const paramQs = computed(() => {
  const parts = link.value.params.map(
    ([k, v]) => `${encodeURIComponent(`param.${k}`)}=${encodeURIComponent(v)}`,
  )
  return parts.length ? `&${parts.join('&')}` : ''
})

const apiUrl = computed(
  () => `/api/view?ref=${encodeURIComponent(ref.value)}${paramQs.value}`,
)

const { data, pending, error, refresh } = await useFetch(apiUrl, {
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

// Form submits (`/api/submit`) need the view ref; a returned
// `view` swaps the whole ViewResult in place. Provided to
// FormView.vue via inject — no prop threading through ViewNode's
// recursion.
provide('pavoViewRef', ref)
provide('pavoApplyView', (vr: any) => {
  if (data.value) {
    data.value = { ok: true, result: vr } as typeof data.value
  }
})
const apiError = computed(() => {
  const d = data.value as { ok?: boolean; error?: string } | null
  if (error.value) return error.value.message
  if (d && d.ok === false) return d.error || 'request failed'
  return null
})
</script>

<template>
  <div>
    <p><NuxtLink to="/">← dashboard</NuxtLink></p>
    <div v-if="pending" class="muted">loading {{ ref }}…</div>
    <div v-else-if="apiError" class="error">
      <h1>{{ ref || '(no view ref)' }}</h1>
      <p>{{ apiError }}</p>
      <p class="muted">unknown driver or view — check `pavo drivers` / `pavo views`.</p>
    </div>
    <template v-else-if="viewResult">
      <h1>
        {{ viewResult.view?.title || ref }}
        <code class="ref">{{ ref }}</code>
      </h1>
      <p v-if="viewResult.view?.description" class="muted">
        {{ viewResult.view.description }}
      </p>
      <ViewNode :node="viewResult.view.root" :data="viewResult.data || {}" />
    </template>
  </div>
</template>

<style scoped>
.muted {
  opacity: 0.65;
}
.error {
  color: #c0392b;
}
.ref {
  font-size: 0.6em;
  font-weight: normal;
  opacity: 0.7;
  margin-left: 0.5rem;
}
</style>
