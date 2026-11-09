<script setup lang="ts">
// `headerActions` chrome slot — icon buttons in the top bar's right
// side. Each entry's `route` resolves through `resolveRoute`: view
// routes open a ViewPopover panel (chrome actions are just views);
// plain routes navigate. One panel at a time; a scrim closes it.
import type { SlotEntry } from '~/composables/platform'
import type { ViewLink } from '~/utils/view-link'

const props = defineProps<{ entries: SlotEntry[] }>()

const router = useRouter()

// The open panel: which action + the resolved view link (null while
// resolving or when the route turned out to be a plain link).
const open = ref<{ entry: SlotEntry; link: ViewLink } | null>(null)

async function activate(e: SlotEntry) {
  if (open.value?.entry.id === e.id) {
    open.value = null
    return
  }
  const target = await resolveRoute(e.route)
  if (target.type === 'view') {
    open.value = { entry: e, link: target.link }
  } else {
    open.value = null
    router.push(target.to)
  }
}

function close() {
  open.value = null
}

// Esc closes the panel.
function onKey(ev: KeyboardEvent) {
  if (ev.key === 'Escape') close()
}
onMounted(() => window.addEventListener('keydown', onKey))
onBeforeUnmount(() => window.removeEventListener('keydown', onKey))
</script>

<template>
  <div class="header-actions">
    <button
      v-for="e in props.entries"
      :key="e.id"
      class="action-btn"
      :class="{ active: open?.entry.id === e.id }"
      :title="`${e.title} — ${e.driver}`"
      @click="activate(e)"
    >
      {{ e.icon || e.title }}
    </button>

    <template v-if="open">
      <div class="scrim" @click="close" />
      <ViewPopover :link="open.link" :title="open.entry.title" @close="close" />
    </template>
  </div>
</template>

<style scoped>
.header-actions {
  display: flex;
  align-items: center;
  gap: 0.25rem;
}
.action-btn {
  background: none;
  border: 1px solid transparent;
  border-radius: 8px;
  color: inherit;
  font: inherit;
  font-size: 1rem;
  line-height: 1;
  padding: 0.3rem 0.45rem;
  cursor: pointer;
  opacity: 0.8;
}
.action-btn:hover,
.action-btn.active {
  opacity: 1;
  border-color: color-mix(in srgb, currentColor 20%, transparent);
  background: color-mix(in srgb, currentColor 6%, transparent);
}
.scrim {
  position: fixed;
  inset: 0;
  z-index: 55;
}
</style>
