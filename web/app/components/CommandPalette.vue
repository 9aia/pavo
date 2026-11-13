<script setup lang="ts">
// Command palette — Ctrl+K (⌘K) global quick-launch over the
// interaction protocol: every collected view action plus every
// driver's `contributes.menus[]` entries (keywords, section
// grouping, priority ordering). Hand-rolled subsequence fuzzy
// match — no dep.
//
// Selection rides the unified command layer (utils/commands.ts —
// the TS mirror of pavo-core's commands.rs): `MenuEntry.command`
// wins over the legacy `action`/`route` pair; `route:`/`Action.route`
// navigate via `useNavigateRoute`, `action:`/`driver:` dispatch
// through `useActionFlow` (params forms + danger gates identical to
// ActionBar), `host.*` builtins map to web equivalents. `MenuEntry.keys`
// are display-only hints (the web host has no keymap).
import type { Action, MenuEntry, ScopedAction } from '~/utils/interactions'
import { fuzzyScore } from '~/utils/interactions'
import {
  BUILTINS,
  commandId,
  menuCommand,
  resolveCommand,
  type Command,
  type HostBuiltin,
  type Resolved,
} from '~/utils/commands'
import {
  fetchAllManifests,
  usePaletteActions,
} from '~/composables/actions'
import { useActionFlow } from '~/composables/useActionFlow'
import { useNavigateRoute } from '~/composables/platform'

const open = ref(false)
const query = ref('')
const cursor = ref(0)
const inputEl = ref<HTMLInputElement | null>(null)

const flow = useActionFlow()
const palette = usePaletteActions()
const navigate = useNavigateRoute()

// ---- host builtins ---------------------------------------------------------

/** Builtins the web host implements — `editor`/`quit` have no web
 *  equivalent and stay hidden (they'd resolve Unsupported anyway). */
const SUPPORTED_BUILTINS = new Set<HostBuiltin>([
  'filter',
  'help',
  'reload',
  'palette',
])

/** Builtin activation — window events the owning surface (the view
 *  page) listens for; `palette` is a no-op here because selecting a
 *  palette row already closed the palette. */
function runBuiltin(b: HostBuiltin): void {
  window.dispatchEvent(new Event(`pavo:host:${b}`))
}

function builtinItems(): PaletteItem[] {
  return BUILTINS.filter((b) => SUPPORTED_BUILTINS.has(b.builtin)).map((b) => ({
    key: `host:${b.id}`,
    title: b.label,
    section: 'host',
    keywords: [b.id, ...b.keys],
    priority: 100,
    hint: b.keys.join('/'),
    run: () => runBuiltin(b.builtin),
  }))
}

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

/** Resolve one menu entry's command against the palette context —
 *  `action:` resolves on the *current* driver (the view on screen,
 *  full node/item scope via its ViewResult; off-view pages fall
 *  back to the entry's owning driver, manifest scope), `driver:` on
 *  the named driver's manifest, `route:`/`Action.route` navigate,
 *  `host.*` hits the builtin map. Returns the palette `run` or
 *  `null` when the entry resolves Unsupported (hidden). */
function resolveMenuRun(
  cmd: Command,
  entry: MenuEntry,
  owner: string,
): (() => void) | null {
  // `action:` commands target the current driver; with no view on
  // screen the owning driver is the only sensible context.
  const ctxDriver = palette.driver || owner
  const ctxManifest = manifests.value.find((r) => r.driver === ctxDriver)?.manifest
  const resolved: Resolved = resolveCommand(cmd, {
    driver: ctxDriver,
    manifest: ctxManifest,
    viewResult: ctxDriver === palette.driver ? palette.viewResult : null,
    drivers: manifests.value.map((r) => ({ driver: r.driver, manifest: r.manifest })),
  })
  switch (resolved.type) {
    case 'navigate':
      return () => void navigate(resolved.route)
    case 'builtin':
      return SUPPORTED_BUILTINS.has(resolved.builtin)
        ? () => runBuiltin(resolved.builtin)
        : null
    case 'unsupported':
      return null
    case 'dispatch': {
      // Rebuild a ScopedAction for the flow — `declared` carries
      // params/danger when the id was found in scope; undeclared
      // ids degrade to a bare invoke (driver answers ok:false).
      const itemId =
        resolved.target && typeof resolved.target === 'object'
          ? (resolved.target as Record<string, any>).id
          : undefined
      const scoped: ScopedAction = {
        action:
          resolved.declared ??
          ({
            id: resolved.action,
            label: entry.title,
            icon: entry.icon,
            danger: 'none',
            params: [],
          } satisfies Action),
        scope: typeof itemId === 'string' ? 'item' : 'manifest',
        driver: resolved.driver,
        view: resolved.view,
        itemId: typeof itemId === 'string' ? itemId : undefined,
      }
      return () => flow.activate(scoped)
    }
  }
}

/** Menus → palette items via `menuCommand` + `resolveCommand`.
 *  `command` wins over the legacy `action`/`route` pair; a legacy
 *  bare `action` (no `driver:` prefix, no `command`) keeps its
 *  historical owner-driver scope by resolving as `driver:<owner>:<id>`
 *  — the grammar's `action:` means *current* driver, which is only
 *  spelled explicitly via `command`. */
function menuItems(): PaletteItem[] {
  const items: PaletteItem[] = []
  for (const { driver, manifest } of manifests.value) {
    for (const m of (manifest.contributes?.menus ?? []) as MenuEntry[]) {
      let cmd = menuCommand(m)
      if (
        !m.command &&
        m.action &&
        !m.action.includes(':') &&
        cmd?.type === 'action'
      ) {
        cmd = { type: 'driver', driver, id: m.action }
      }
      if (!cmd) continue // display-only entry (section label)
      const run = resolveMenuRun(cmd, m, driver)
      if (!run) continue // unsupported on this host — hidden
      const keys = m.keys ?? []
      items.push({
        key: `menu:${driver}:${m.id}`,
        title: m.title,
        section: m.section ?? manifest.title ?? driver,
        keywords: [m.id, ...(m.keywords ?? []), ...keys],
        priority: m.priority ?? 100,
        icon: m.icon,
        hint: [commandId(cmd), ...keys].filter(Boolean).join(' · '),
        run,
      })
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

const allItems = computed<PaletteItem[]>(() => [
  ...actionItems(),
  ...menuItems(),
  ...builtinItems(),
])

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
