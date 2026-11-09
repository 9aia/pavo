<script setup lang="ts">
// Deep-link view page: `/view/<driver>:<view>` plus optional params —
// path segments (`/view/myco:item/id=T-001`), bare query keys
// (`?id=T-001`), or the legacy `?param.id=T-001` form. See
// `app/utils/view-link.ts` for the grammar and precedence; everything
// is forwarded to the driver's view op as `param.<k>=<v>`, same rule
// as the CLI's `--param k=v`.
//
// Fetching + form-submit context live in `useView` — shared with
// chrome popovers (ViewPopover.vue) so embedded views behave
// identically to full pages.
import { parseViewLink } from '~/utils/view-link'

const route = useRoute()

const link = computed(() => parseViewLink(route.params.ref, route.query))
const ref = computed(() => link.value.ref)

const { pending, apiError, viewResult, refresh } = useView(
  ref,
  computed(() => link.value.params),
)
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
    <p v-else class="muted">no view.</p>
    <button class="refresh" @click="refresh()">refresh</button>
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
.refresh {
  margin-top: 1rem;
  background: none;
  border: 1px solid color-mix(in srgb, currentColor 20%, transparent);
  border-radius: 6px;
  color: inherit;
  font: inherit;
  font-size: 0.8rem;
  padding: 0.2rem 0.6rem;
  cursor: pointer;
  opacity: 0.7;
}
.refresh:hover {
  opacity: 1;
}
</style>
