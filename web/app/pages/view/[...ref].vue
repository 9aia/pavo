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
import { collectActions, dedupeActions } from '~/utils/interactions'
import { fetchManifest, setPaletteActions } from '~/composables/actions'

const route = useRoute()

const link = computed(() => parseViewLink(route.params.ref, route.query))
const ref = computed(() => link.value.ref)
const driver = computed(() => ref.value.split(':')[0] ?? '')

const { pending, apiError, viewResult, refresh } = useView(
  ref,
  computed(() => link.value.params),
)

// Interaction actions — manifest-global + node/item-scoped,
// collected from the driver manifest (wire `manifest` op, cached)
// and the live ViewNode tree. Deduped item > node > manifest; the
// same list feeds the ActionBar and the command palette.
const manifest = ref<any>(null)
watch(
  driver,
  async (d) => {
    manifest.value = d ? await fetchManifest(d) : null
  },
  { immediate: true },
)

const collected = computed(() =>
  collectActions(manifest.value, viewResult.value),
)
const allActions = computed(() => dedupeActions(collected.value))
// The bar shows view-level verbs only — item-scoped actions render
// on their tree rows (ItemActions.vue), not as N duplicate buttons.
// Filter *before* dedupe so an item-scoped declaration can't shadow
// a same-id manifest/node action here.
const barActions = computed(() =>
  dedupeActions(collected.value.filter((s) => s.scope !== 'item')),
)

// Palette registry — the full deduped set (item actions collapse
// to one entry per id, first row's target). Cleared on unmount so
// off-view pages don't offer stale actions.
watch(allActions, (list) => setPaletteActions(driver.value, list), {
  immediate: true,
})
onBeforeUnmount(() => setPaletteActions('', []))
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
      <ActionBar :actions="barActions" />
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
