<script setup lang="ts">
// Deep-link view page: `/view/<driver>:<view>` plus optional params —
// path segments (`/view/myco:item/id=T-001`), bare query keys
// (`?id=T-001`), or the legacy `?param.id=T-001` form. See
// `app/utils/view-link.ts` for the grammar and precedence; everything
// is forwarded to the driver's view op as `param.<k>=<v>`, same rule
// as the CLI's `--param k=v`.
//
// P-033 web host: `ViewSpec.filter_param` (fallback `"q"`) drives the
// filter bar — typing re-resolves the view with that param set (the
// `host.filter` builtin's web equivalent; `/` focuses the input).
// `host.reload`/`host.help` map to refresh()/the help overlay; they
// arrive as `pavo:host:*` window events from the command palette.
//
// Fetching + form-submit context live in `useView` — shared with
// chrome popovers (ViewPopover.vue) so embedded views behave
// identically to full pages.
import { parseViewLink, type ViewParam } from '~/utils/view-link'
import { collectActions, dedupeActions } from '~/utils/interactions'
import { BUILTINS } from '~/utils/commands'
import { fetchManifest, setPaletteActions } from '~/composables/actions'

const route = useRoute()

const link = computed(() => parseViewLink(route.params.ref, route.query))
const ref = computed(() => link.value.ref)
const driver = computed(() => ref.value.split(':')[0] ?? '')

// ---- filter (ViewSpec.filter_param, fallback "q") -------------------
//
// The param name lives in a ref (synced from the loaded ViewSpec by
// the watcher below) rather than reading `viewResult` inside the
// params getter — `useFetch` evaluates the URL eagerly, so the
// getter must not depend on useView's own return value.

const filterText = ref('')
/** Debounced applied value — what actually rides the view params. */
const appliedFilter = ref('')
const filterParamName = ref('q')
let filterTimer: ReturnType<typeof setTimeout> | undefined
watch(filterText, (v) => {
  clearTimeout(filterTimer)
  filterTimer = setTimeout(() => {
    appliedFilter.value = v.trim()
  }, 250)
})

const viewParams = computed<ViewParam[]>(() => {
  const q = appliedFilter.value
  if (!q) return link.value.params
  // UI filter wins over a same-named URL param (params are
  // last-wins downstream); clearing restores the URL verbatim.
  const fp = filterParamName.value
  return [
    ...link.value.params.filter(([k]) => k !== fp),
    [fp, q],
  ]
})

const { pending, apiError, viewResult, refresh } = useView(ref, viewParams)

watch(
  () => (viewResult.value?.view as any)?.filter_param as string | undefined,
  (v) => {
    filterParamName.value = v ?? 'q'
  },
  { immediate: true },
)
const filterParam = filterParamName

const filterInput = ref<HTMLInputElement | null>(null)
function focusFilter(): void {
  filterInput.value?.focus()
  filterInput.value?.select()
}
function clearFilter(): void {
  filterText.value = ''
  appliedFilter.value = ''
}

// ---- host builtins (pavo:host:* events from the palette) --------------

const helpOpen = ref(false)
const WEB_BUILTINS = BUILTINS.filter((b) =>
  ['filter', 'help', 'reload', 'palette'].includes(b.builtin),
)

function onHostEvent(e: Event): void {
  const name = (e as CustomEvent | Event).type.replace('pavo:host:', '')
  if (name === 'filter') focusFilter()
  else if (name === 'reload') void refresh()
  else if (name === 'help') helpOpen.value = !helpOpen.value
  // 'palette' needs no listener — the palette component owns itself.
}

function onKeydown(e: KeyboardEvent): void {
  const t = e.target as HTMLElement | null
  const typing =
    !!t &&
    (t.tagName === 'INPUT' ||
      t.tagName === 'TEXTAREA' ||
      t.tagName === 'SELECT' ||
      t.isContentEditable)
  if (typing) return
  if (e.key === '/') {
    e.preventDefault()
    focusFilter()
  } else if (e.key === '?') {
    e.preventDefault()
    helpOpen.value = !helpOpen.value
  } else if (e.key === 'Escape' && helpOpen.value) {
    helpOpen.value = false
  }
}

onMounted(() => {
  window.addEventListener('pavo:host:filter', onHostEvent)
  window.addEventListener('pavo:host:reload', onHostEvent)
  window.addEventListener('pavo:host:help', onHostEvent)
  window.addEventListener('keydown', onKeydown)
})
onBeforeUnmount(() => {
  window.removeEventListener('pavo:host:filter', onHostEvent)
  window.removeEventListener('pavo:host:reload', onHostEvent)
  window.removeEventListener('pavo:host:help', onHostEvent)
  window.removeEventListener('keydown', onKeydown)
  clearTimeout(filterTimer)
})

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
// to one entry per id, first row's target) plus the live ViewResult
// (command resolution + `{data.*}` interpolation). Cleared on
// unmount so off-view pages don't offer stale actions.
watch(
  [allActions, viewResult],
  ([list, vr]) => setPaletteActions(driver.value, list, vr),
  { immediate: true },
)
onBeforeUnmount(() => setPaletteActions('', [], null))
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
      <form class="filterbar" @submit.prevent="appliedFilter = filterText.trim()">
        <span class="fmag" aria-hidden="true">⌕</span>
        <input
          ref="filterInput"
          v-model="filterText"
          class="finput"
          type="search"
          :placeholder="`filter (${filterParam})…  ·  / to focus`"
          :aria-label="`filter view by ${filterParam}`"
        />
        <button
          v-if="appliedFilter"
          type="button"
          class="fchip"
          :title="`clear ${filterParam} filter`"
          @click="clearFilter"
        >
          {{ filterParam }}: {{ appliedFilter }} ×
        </button>
      </form>
      <ActionBar :actions="barActions" />
      <ViewNode :node="viewResult.view.root" :data="viewResult.data || {}" />
    </template>
    <p v-else class="muted">no view.</p>
    <button class="refresh" @click="refresh()">refresh</button>

    <div v-if="helpOpen" class="help-backdrop" @click.self="helpOpen = false">
      <div class="help-panel" role="dialog" aria-label="keyboard shortcuts">
        <h2>commands</h2>
        <ul>
          <li v-for="b in WEB_BUILTINS" :key="b.id">
            <kbd>{{ b.keys.join(' / ') }}</kbd>
            <strong>{{ b.label }}</strong>
            <span class="muted">— {{ b.description }}</span>
          </li>
        </ul>
        <p class="muted">ctrl+k command palette · esc close</p>
      </div>
    </div>
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
.filterbar {
  display: flex;
  align-items: center;
  gap: 0.4rem;
  margin: 0.4rem 0;
}
.fmag {
  opacity: 0.55;
}
.finput {
  flex: 1;
  max-width: 24rem;
  font: inherit;
  font-size: 0.9rem;
  padding: 0.25rem 0.6rem;
  border: 1px solid color-mix(in srgb, currentColor 20%, transparent);
  border-radius: 6px;
  background: transparent;
  color: inherit;
}
.finput:focus {
  outline: none;
  border-color: currentColor;
}
.fchip {
  font: inherit;
  font-size: 0.78rem;
  padding: 0.15rem 0.55rem;
  border: 1px solid color-mix(in srgb, currentColor 25%, transparent);
  border-radius: 999px;
  background: transparent;
  color: inherit;
  cursor: pointer;
  opacity: 0.85;
  white-space: nowrap;
}
.fchip:hover {
  opacity: 1;
  border-color: currentColor;
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
.help-backdrop {
  position: fixed;
  inset: 0;
  z-index: 80;
  background: rgb(0 0 0 / 0.3);
  display: flex;
  justify-content: center;
  padding-top: 10vh;
}
.help-panel {
  width: min(30rem, calc(100vw - 2rem));
  background: Canvas;
  color: CanvasText;
  border: 1px solid color-mix(in srgb, currentColor 20%, transparent);
  border-radius: 10px;
  box-shadow: 0 12px 40px rgb(0 0 0 / 0.35);
  padding: 1rem 1.25rem;
}
.help-panel h2 {
  margin: 0 0 0.5rem;
  font-size: 1rem;
}
.help-panel ul {
  list-style: none;
  margin: 0;
  padding: 0;
}
.help-panel li {
  display: flex;
  align-items: baseline;
  gap: 0.6rem;
  padding: 0.25rem 0;
  font-size: 0.9rem;
}
.help-panel kbd {
  min-width: 3.5rem;
  font-size: 0.78rem;
  border: 1px solid color-mix(in srgb, currentColor 25%, transparent);
  border-radius: 4px;
  padding: 0.05rem 0.35rem;
  text-align: center;
}
</style>
