<script setup lang="ts">
// Command palette — Ctrl+K (⌘K) global quick-launch over the
// interaction protocol: every collected view action plus every
// driver's `contributes.menus[]` entries (keywords, section
// grouping, priority ordering). Hand-rolled subsequence fuzzy
// match — no dep.
//
// Selection: menu `route`s navigate (view routes resolve through
// the same routeToViewLink rules as chrome); `action`s — declared
// or menu-synthesized — run through `useActionFlow`, so params
// forms and danger gates behave identically to ActionBar clicks.
import type { Action, MenuEntry, ScopedAction } from '~/utils/interactions'
import { fuzzyScore } from '~/utils/interactions'
import {
  fetchAllManifests,
  usePaletteActions,
} from '~/composables/actions'
import { useActionFlow } from '~/composables/useActionFlow'
import { resolveRoute } from '~/composables/platform'
import { viewLinkHref } from '~/utils/view-link'

const router = useRouter()
const open = ref(false)
const query = ref('')
const cursor = ref(0)
const inputEl = ref<HTMLInputElement | null>(null)

const flow = useActionFlow()
const palette = usePaletteActions()

// ---- entry model ---------------------------------------------------------

interface PaletteItem {
  key: string
  title: string
  section: string
  keywords: string[]
  priority: number
  icon?: string
  hint?: string
  run: () => void
}

const manifests = ref<{ driver: string; manifest: any }[]>([])
const manifestsLoaded = ref(false)

async function ensureManifests(): Promise<void> {
  if (manifestsLoaded.value) return
  manifestsLoaded.value = true
  manifests.value = await fetchAllManifests()
}

/** Menus → palette items. `action` entries resolve against the
 *  owning manifest's `actions[]` first (params/danger/label come
 *  from the declaration); unknown ids degrade to a bare invoke. */
function menuItems(): PaletteItem[] {
  const items: PaletteItem[] = []
  for (const { driver, manifest } of manifests.value) {
    const declared = new Map<string, Action>(
      (manifest.actions ?? []).map((a: Action) => [a.id, a]),
    )
    for (const m of (manifest.contributes?.menus ?? []) as MenuEntry[]) {
      const section = m.section ?? manifest.title ?? driver
      const base = {
        key: `menu:${driver}:${m.id}`,
        title: m.title,
        section,
        keywords: [m.id, ...(m.keywords ?? [])],
        priority: m.priority ?? 100,
        icon: m.icon,
        hint: m.action ? `action · ${driver}` : `route · ${driver}`,
      }
      if (m.route) {
        const route = m.route
        items.push({
          ...base,
          run: () => {
            void resolveRoute(route).then((t) => {
              router.push(t.type === 'view' ? viewLinkHref(t.link) : t.to)
            })
          },
        })
      } else if (m.action) {
        // "<action-id>" local, "<driver>:<action-id>" cross-driver.
        const colon = m.action.indexOf(':')
        const actionDriver =
          colon > 0 && manifests.value.some((r) => r.driver === m.action!.slice(0, colon))
            ? m.action!.slice(0, colon)
            : driver
        const actionId = colon > 0 && actionDriver !== driver ? m.action!.slice(colon + 1) : m.action!
        const local = actionDriver === driver ? declared.get(actionId) : undefined
        const action: Action = local ?? {
          id: actionId,
          label: m.title,
          icon: m.icon,
          danger: 'none',
          params: [],
        }
        const scoped: ScopedAction = {
          action,
          scope: 'manifest',
          driver: actionDriver,
        }
        items.push({ ...base, run: () => flow.activate(scoped) })
      }
      // Entries with neither action nor route are section labels —
      // skip them (sections already group the list).
    }
  }
  return items
}

/** View actions → palette items (the store the view page fills). */
function actionItems(): PaletteItem[] {
  return palette.actions.map((s) => ({
    key: `act:${s.driver}:${s.scope}:${s.action.id}:${s.itemId ?? ''}`,
    title: s.action.label,
    section: s.driver || 'actions',
    keywords: [s.action.id, ...(s.action.keys ?? [])],
    priority: s.scope === 'item' ? 0 : s.scope === 'node' ? 5 : 10,
    icon: s.action.icon,
    hint: `${s.driver} · ${s.scope}${s.itemId ? ` · ${s.itemId}` : ''}`,
    run: () => flow.activate(s),
  }))
}

const allItems = computed<PaletteItem[]>(() => [...actionItems(), ...menuItems()])

const filtered = computed<{ section: string; items: PaletteItem[] }[]>(() => {
  const q = query.value
  const scored = allItems.value
    .map((it) => {
      const hay = [it.title, it.keywords.join(' '), it.section].join(' ')
      const score = fuzzyScore(q, hay) ?? fuzzyScore(q, it.title)
      return score === null ? null : { it, score }
    })
    .filter((x): x is { it: PaletteItem; score: number } => x !== null)
    .sort((a, b) => a.score - b.score || a.it.priority - b.it.priority)
  const groups = new Map<string, PaletteItem[]>()
  for (const { it } of scored.slice(0, 60)) {
    const list = groups.get(it.section) ?? []
    list.push(it)
    groups.set(it.section, list)
  }
  return [...groups.entries()]
    .sort(([a], [b]) => a.localeCompare(b))
    .map(([section, items]) => ({ section, items: items.slice(0, 12) }))
})

const flat = computed(() => filtered.value.flatMap((g) => g.items))

watch(flat, () => {
  cursor.value = 0
})

function toggle(): void {
  open.value = !open.value
  if (open.value) {
    query.value = ''
    cursor.value = 0
    void ensureManifests()
    nextTick(() => inputEl.value?.focus())
  }
}

/** Header-button path — AppHeader dispatches `pavo:palette`. */
function onPaletteEvent(): void {
  if (!open.value) toggle()
}

function select(item: PaletteItem | undefined): void {
  if (!item) return
  open.value = false
  query.value = ''
  item.run()
}

function onKeydown(e: KeyboardEvent): void {
  if ((e.ctrlKey || e.metaKey) && e.key.toLowerCase() === 'k') {
    e.preventDefault()
    toggle()
    return
  }
  if (!open.value) return
  if (e.key === 'Escape') {
    open.value = false
    query.value = ''
  } else if (e.key === 'ArrowDown') {
    e.preventDefault()
    cursor.value = Math.min(cursor.value + 1, flat.value.length - 1)
  } else if (e.key === 'ArrowUp') {
    e.preventDefault()
    cursor.value = Math.max(cursor.value - 1, 0)
  } else if (e.key === 'Enter') {
    e.preventDefault()
    select(flat.value[cursor.value])
  }
}

onMounted(() => {
  window.addEventListener('keydown', onKeydown)
  window.addEventListener('pavo:palette', onPaletteEvent)
})
onBeforeUnmount(() => {
  window.removeEventListener('keydown', onKeydown)
  window.removeEventListener('pavo:palette', onPaletteEvent)
})
</script>

<template>
  <div v-if="open" class="pal-backdrop" @click.self="open = false">
    <div class="pal" role="dialog" aria-label="command palette">
      <input
        ref="inputEl"
        v-model="query"
        class="pal-input"
        type="text"
        placeholder="Type a command or search…"
        @keydown.stop
      />
      <div class="pal-list">
        <template v-for="g in filtered" :key="g.section">
          <div class="pal-section">{{ g.section }}</div>
          <button
            v-for="it in g.items"
            :key="it.key"
            class="pal-item"
            :class="{ active: flat[cursor] === it }"
            @click="select(it)"
            @mousemove="cursor = flat.indexOf(it)"
          >
            <span v-if="it.icon" class="pal-icon">{{ it.icon }}</span>
            <span class="pal-title">{{ it.title }}</span>
            <code v-if="it.hint" class="pal-hint">{{ it.hint }}</code>
          </button>
        </template>
        <p v-if="!flat.length" class="pal-empty muted">
          no matches{{ manifestsLoaded ? '' : ' — loading commands…' }}
        </p>
      </div>
      <footer class="pal-foot muted">
        <span>↑↓ navigate</span><span>↵ run</span><span>esc close</span>
        <span class="pal-kbd">ctrl+k</span>
      </footer>
    </div>
  </div>
  <ActionDialogs
    :state="flow.state"
    :submit-form="flow.submitForm"
    :confirm="flow.confirm"
    :cancel="flow.cancel"
  />
  <p v-if="!open && flow.state.message" class="pal-toast">{{ flow.state.message }}</p>
</template>

<style scoped>
.pal-backdrop {
  position: fixed;
  inset: 0;
  z-index: 70;
  background: rgb(0 0 0 / 0.3);
  display: flex;
  justify-content: center;
  padding-top: 12vh;
}
.pal {
  width: min(36rem, calc(100vw - 2rem));
  max-height: 60vh;
  display: flex;
  flex-direction: column;
  background: Canvas;
  color: CanvasText;
  border: 1px solid color-mix(in srgb, currentColor 20%, transparent);
  border-radius: 10px;
  box-shadow: 0 12px 40px rgb(0 0 0 / 0.35);
  overflow: hidden;
}
.pal-input {
  font: inherit;
  padding: 0.7rem 1rem;
  border: none;
  border-bottom: 1px solid color-mix(in srgb, currentColor 12%, transparent);
  background: transparent;
  color: inherit;
  outline: none;
}
.pal-list {
  overflow: auto;
  padding: 0.35rem 0;
}
.pal-section {
  padding: 0.4rem 1rem 0.15rem;
  font-size: 0.72rem;
  font-weight: 700;
  text-transform: uppercase;
  letter-spacing: 0.04em;
  opacity: 0.55;
}
.pal-item {
  display: flex;
  align-items: baseline;
  gap: 0.5rem;
  width: 100%;
  font: inherit;
  font-size: 0.9rem;
  text-align: left;
  padding: 0.4rem 1rem;
  border: none;
  background: transparent;
  color: inherit;
  cursor: pointer;
}
.pal-item.active {
  background: color-mix(in srgb, currentColor 8%, transparent);
}
.pal-title {
  overflow: hidden;
  text-overflow: ellipsis;
  white-space: nowrap;
}
.pal-hint {
  margin-left: auto;
  font-size: 0.72em;
  opacity: 0.55;
  overflow: hidden;
  text-overflow: ellipsis;
  white-space: nowrap;
}
.pal-empty {
  padding: 1rem;
  text-align: center;
}
.pal-foot {
  display: flex;
  gap: 1rem;
  padding: 0.4rem 1rem;
  border-top: 1px solid color-mix(in srgb, currentColor 12%, transparent);
  font-size: 0.75rem;
}
.pal-kbd {
  margin-left: auto;
  border: 1px solid color-mix(in srgb, currentColor 20%, transparent);
  border-radius: 4px;
  padding: 0 0.35rem;
}
.pal-toast {
  position: fixed;
  bottom: 1rem;
  left: 50%;
  transform: translateX(-50%);
  z-index: 75;
  margin: 0;
  padding: 0.4rem 0.9rem;
  border: 1px solid color-mix(in srgb, currentColor 20%, transparent);
  border-radius: 8px;
  background: Canvas;
  color: CanvasText;
  box-shadow: 0 4px 16px rgb(0 0 0 / 0.25);
  font-size: 0.9rem;
}
.muted {
  opacity: 0.6;
}
</style>
