<script setup lang="ts">
// Deep-link view page: `/view/<driver>:<view>` (the colon may be
// percent-encoded). Extra `?param.<key>=<value>` query params are
// forwarded to the driver's view op, same rule as the CLI's
// `--param k=v`.
const route = useRoute()

const ref = computed(() => String(route.params.ref))

// Forward `param.*` query args verbatim; everything else is ignored.
const paramQs = computed(() => {
  const parts: string[] = []
  for (const [k, v] of Object.entries(route.query)) {
    if (!k.startsWith('param.')) continue
    for (const val of Array.isArray(v) ? v : [v]) {
      parts.push(`${encodeURIComponent(k)}=${encodeURIComponent(String(val))}`)
    }
  }
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
      <h1>{{ ref }}</h1>
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
