<script setup lang="ts">
// Left sidebar — the platform's primary nav. IA decision: the
// `navbar` and `sidebar` chrome slots merge into ONE grouped list
// (navbar is folded in; the header stays actions-only). Entries
// group by `driver_title` (falling back to `driver`), alphabetically,
// after a built-in "pavo" group (dashboard + jobs). `sidebar` slot
// entries list before `navbar` ones within each group — both arrive
// pre-sorted by priority from the bridge merge.
//
// Entry `route`s resolve through the `/api/views` list: view routes
// (`/demo/board`, `driver:view`, `/view/…`) become `/view/<ref>`
// deep links; anything else keeps its raw route (driver-owned
// `routes[]` pages aren't rendered natively yet).
import type { SlotEntry } from '~/composables/platform'
import type { ViewListEntry } from '~/utils/view-link'
import { routeToViewLink, viewLinkHref } from '~/utils/view-link'

const props = defineProps<{
  sidebar: SlotEntry[]
  navbar: SlotEntry[]
  open: boolean
}>()
const emit = defineEmits<{ close: [] }>()

const route = useRoute()
const { data: viewsData } = useViews()

const views = computed<ViewListEntry[]>(() => {
  const d = viewsData.value as { ok?: boolean; result?: ViewListEntry[] } | null
  return d?.ok && Array.isArray(d.result) ? d.result : []
})

interface NavItem {
  key: string
  title: string
  href: string
  icon?: string | null
  external?: boolean
}
interface NavGroup {
  label: string
  items: NavItem[]
}

function hrefFor(e: { route: string }): string {
  const link = routeToViewLink(e.route, views.value)
  return link ? viewLinkHref(link) : e.route
}

const groups = computed<NavGroup[]>(() => {
  const byGroup = new Map<string, NavItem[]>()
  // sidebar slot first, then navbar — per-group order is
  // deterministic (bridge already priority-sorted each slot).
  for (const e of [...props.sidebar, ...props.navbar]) {
    const label = e.driver_title || e.driver || 'other'
    const list = byGroup.get(label) ?? []
    list.push({
      key: `${e.driver}:${e.id}`,
      title: e.title,
      href: hrefFor(e),
      icon: e.icon,
    })
    byGroup.set(label, list)
  }
  const driverGroups = [...byGroup.entries()]
    .sort(([a], [b]) => a.localeCompare(b))
    .map(([label, items]) => ({ label, items }))

  const builtins: NavGroup = {
    label: 'pavo',
    items: [
      { key: 'home', title: 'Dashboard', href: '/' },
      { key: 'jobs', title: 'Jobs', href: '/jobs' },
    ],
  }
  return [builtins, ...driverGroups]
})

function isActive(href: string): boolean {
  if (href === '/') return route.path === '/'
  return route.path === href || route.path.startsWith(`${href}/`)
}

// Close the mobile drawer on navigation.
watch(
  () => route.path,
  () => emit('close'),
)
</script>

<template>
  <!-- display:contents wrapper — the scrim must stay outside the
       transformed .sidebar (a fixed element inside a transformed
       ancestor is contained by it, not the viewport). -->
  <div class="sidebar-wrap">
    <div v-if="open" class="scrim" @click="emit('close')" />
    <aside class="sidebar" :class="{ open }">
      <NuxtLink to="/" class="brand" @click="emit('close')">pavo</NuxtLink>
      <nav class="nav">
        <div v-for="g in groups" :key="g.label" class="nav-group">
          <div class="nav-label">{{ g.label }}</div>
          <NuxtLink
            v-for="item in g.items"
            :key="item.key"
            :to="item.href"
            class="navlink"
            :class="{ active: isActive(item.href) }"
          >
            <span v-if="item.icon" class="nav-icon">{{ item.icon }}</span>
            {{ item.title }}
          </NuxtLink>
        </div>
      </nav>
    </aside>
  </div>
</template>

<style scoped>
.sidebar-wrap {
  display: contents;
}
.sidebar {
  width: 12.5rem;
  flex: none;
  display: flex;
  flex-direction: column;
  border-right: 1px solid color-mix(in srgb, currentColor 15%, transparent);
  padding: 0.75rem 0.5rem;
  box-sizing: border-box;
}
.brand {
  font-weight: 700;
  font-size: 1.1rem;
  text-decoration: none;
  color: inherit;
  padding: 0.25rem 0.5rem 0.75rem;
}
.nav {
  display: flex;
  flex-direction: column;
  gap: 0.9rem;
  overflow-y: auto;
}
.nav-label {
  font-size: 0.7rem;
  text-transform: uppercase;
  letter-spacing: 0.05em;
  opacity: 0.5;
  padding: 0 0.5rem 0.2rem;
}
.navlink {
  display: flex;
  align-items: center;
  gap: 0.4rem;
  padding: 0.28rem 0.5rem;
  border-radius: 6px;
  color: inherit;
  text-decoration: none;
  opacity: 0.8;
  font-size: 0.92rem;
}
.navlink:hover {
  opacity: 1;
  background: color-mix(in srgb, currentColor 6%, transparent);
}
.navlink.active {
  opacity: 1;
  background: color-mix(in srgb, currentColor 10%, transparent);
  font-weight: 600;
}
.nav-icon {
  flex: none;
}
.scrim {
  display: none;
}

@media (max-width: 52rem) {
  .sidebar {
    position: fixed;
    top: 0;
    left: 0;
    bottom: 0;
    z-index: 70;
    background: Canvas;
    transform: translateX(-100%);
    transition: transform 0.15s ease;
    box-shadow: 0 0 30px rgb(0 0 0 / 0.3);
  }
  .sidebar.open {
    transform: translateX(0);
  }
  .scrim {
    display: block;
    position: fixed;
    inset: 0;
    z-index: 65;
    background: rgb(0 0 0 / 0.35);
  }
}
</style>
