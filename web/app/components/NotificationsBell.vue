<script setup lang="ts">
// Notification bell — header button with an unread badge over the
// aggregated `/api/notifications` feed. The popover lists items
// (title, body, severity, relative time, read state) and navigates
// on click; there is no write path for read state in v1, so `read`
// is display-only. Refetches each time the panel opens.
import type { NotificationRow } from '~/composables/platform'
import { resolveRoute, severityClass, timeAgo } from '~/composables/platform'
import { viewLinkHref } from '~/utils/view-link'

const router = useRouter()
const open = ref(false)

// Client-side fetch on mount (island model); `refresh()` on open.
const { data, pending, refresh } = useNotifications()

const items = computed<NotificationRow[]>(() => {
  const d = data.value as { ok?: boolean; result?: NotificationRow[] } | null
  return d?.ok && Array.isArray(d.result) ? d.result : []
})
const unread = computed(() => items.value.filter((n) => !n.read).length)

function toggle() {
  open.value = !open.value
  if (open.value) refresh()
}

async function activate(n: NotificationRow) {
  if (!n.route) return
  const target = await resolveRoute(n.route)
  open.value = false
  router.push(target.type === 'view' ? viewLinkHref(target.link) : target.to)
}

function onKey(ev: KeyboardEvent) {
  if (ev.key === 'Escape') open.value = false
}
onMounted(() => window.addEventListener('keydown', onKey))
onBeforeUnmount(() => window.removeEventListener('keydown', onKey))
</script>

<template>
  <div class="notif">
    <button
      class="action-btn"
      :class="{ active: open }"
      title="notifications"
      @click="toggle"
    >
      🔔<span v-if="unread" class="badge">{{ unread > 99 ? '99+' : unread }}</span>
    </button>

    <template v-if="open">
      <div class="scrim" @click="open = false" />
      <div class="notif-panel" role="dialog" aria-label="notifications">
        <header class="np-head">
          <strong>Notifications</strong>
          <span class="np-count muted">{{ unread }} unread</span>
          <button class="vp-btn" title="refresh" @click="refresh()">⟳</button>
          <button class="vp-btn" title="close" @click="open = false">×</button>
        </header>
        <div class="np-body">
          <p v-if="pending" class="muted">loading…</p>
          <p v-else-if="!items.length" class="muted">no notifications.</p>
          <ul v-else class="np-list">
            <li
              v-for="n in items"
              :key="`${n.driver}:${n.id}`"
              class="np-item"
              :class="{ unread: !n.read, clickable: !!n.route }"
              @click="activate(n)"
            >
              <span class="np-dot" :class="severityClass(n.severity)" />
              <div class="np-main">
                <div class="np-title">
                  {{ n.title }}
                  <span class="muted">{{ n.driver_title || n.driver }}</span>
                </div>
                <div v-if="n.body" class="np-body-text muted">{{ n.body }}</div>
                <div class="np-meta muted">
                  <span v-if="n.severity">{{ n.severity }}</span>
                  <span v-if="n.created_at" :title="n.created_at">
                    {{ timeAgo(n.created_at) }}
                  </span>
                </div>
              </div>
            </li>
          </ul>
        </div>
      </div>
    </template>
  </div>
</template>

<style scoped>
.notif {
  position: relative;
}
.action-btn {
  position: relative;
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
.badge {
  position: absolute;
  top: -0.3rem;
  right: -0.3rem;
  min-width: 1rem;
  height: 1rem;
  padding: 0 0.2rem;
  border-radius: 0.5rem;
  background: #c0392b;
  color: #fff;
  font-size: 0.65rem;
  font-weight: 700;
  line-height: 1rem;
  text-align: center;
}
.scrim {
  position: fixed;
  inset: 0;
  z-index: 55;
}
.notif-panel {
  position: fixed;
  top: 2.75rem;
  right: 1rem;
  z-index: 60;
  width: min(26rem, calc(100vw - 2rem));
  max-height: calc(100vh - 4rem);
  display: flex;
  flex-direction: column;
  background: Canvas;
  color: CanvasText;
  border: 1px solid color-mix(in srgb, currentColor 20%, transparent);
  border-radius: 10px;
  box-shadow: 0 8px 30px rgb(0 0 0 / 0.25);
  overflow: hidden;
}
.np-head {
  display: flex;
  align-items: center;
  gap: 0.5rem;
  padding: 0.5rem 0.75rem;
  border-bottom: 1px solid color-mix(in srgb, currentColor 12%, transparent);
}
.np-count {
  font-size: 0.8rem;
}
.vp-btn {
  margin-left: auto;
  background: none;
  border: none;
  color: inherit;
  font: inherit;
  cursor: pointer;
  padding: 0.15rem 0.4rem;
  border-radius: 6px;
  opacity: 0.75;
}
.vp-btn:last-of-type {
  margin-left: 0;
}
.vp-btn:hover {
  opacity: 1;
  background: color-mix(in srgb, currentColor 8%, transparent);
}
.np-body {
  overflow: auto;
}
.np-list {
  list-style: none;
  margin: 0;
  padding: 0;
}
.np-item {
  display: flex;
  gap: 0.6rem;
  padding: 0.55rem 0.85rem;
  border-bottom: 1px solid color-mix(in srgb, currentColor 8%, transparent);
}
.np-item.clickable {
  cursor: pointer;
}
.np-item.clickable:hover {
  background: color-mix(in srgb, currentColor 5%, transparent);
}
.np-item.unread .np-title {
  font-weight: 600;
}
.np-dot {
  flex: none;
  width: 0.55rem;
  height: 0.55rem;
  border-radius: 50%;
  margin-top: 0.35rem;
  background: currentColor;
  opacity: 0.4;
}
.np-dot.sev-error {
  background: #c0392b;
  opacity: 1;
}
.np-dot.sev-warning {
  background: #b8860b;
  opacity: 1;
}
.np-dot.sev-info {
  background: #2e6da4;
  opacity: 1;
}
.np-main {
  min-width: 0;
}
.np-title {
  display: flex;
  gap: 0.4rem;
  align-items: baseline;
}
.np-title .muted {
  font-size: 0.75rem;
}
.np-body-text {
  font-size: 0.85rem;
  margin-top: 0.1rem;
}
.np-meta {
  display: flex;
  gap: 0.75rem;
  font-size: 0.75rem;
  margin-top: 0.15rem;
}
.muted {
  opacity: 0.65;
}
</style>
