<script setup lang="ts">
// Top header bar — right side carries the platform actions:
// driver-contributed `headerActions` (icon buttons opening view
// popovers), a jobs indicator linking to `/jobs` with an active-count
// badge, and the notifications bell. The left side only holds the
// mobile sidebar toggle (the brand lives in the sidebar).
import type { ChromeSlots } from '~/composables/platform'
import { activeJobCount } from '~/composables/platform'

const props = defineProps<{ chrome: ChromeSlots | null }>()
const emit = defineEmits<{ toggleNav: [] }>()

const { data: jobsData } = useJobs()
const activeJobs = computed(() => {
  const d = jobsData.value as { ok?: boolean; result?: any[] } | null
  return d?.ok && Array.isArray(d.result) ? activeJobCount(d.result) : 0
})
</script>

<template>
  <header class="topbar">
    <button class="navtoggle" title="menu" aria-label="menu" @click="emit('toggleNav')">
      ☰
    </button>
    <span class="spacer" />
    <HeaderActions v-if="chrome?.headerActions?.length" :entries="chrome.headerActions" />
    <NuxtLink to="/jobs" class="jobs-link" title="jobs">
      ⚙<span v-if="activeJobs" class="badge">{{ activeJobs }}</span>
    </NuxtLink>
    <NotificationsBell />
  </header>
</template>

<style scoped>
.topbar {
  display: flex;
  align-items: center;
  gap: 0.5rem;
  padding: 0.4rem 1rem;
  border-bottom: 1px solid color-mix(in srgb, currentColor 15%, transparent);
}
.navtoggle {
  display: none;
  background: none;
  border: none;
  color: inherit;
  font-size: 1.1rem;
  cursor: pointer;
  padding: 0.2rem 0.4rem;
}
.spacer {
  flex: 1;
}
.jobs-link {
  position: relative;
  color: inherit;
  text-decoration: none;
  opacity: 0.8;
  font-size: 1rem;
  line-height: 1;
  padding: 0.3rem 0.45rem;
}
.jobs-link:hover {
  opacity: 1;
}
.badge {
  position: absolute;
  top: -0.3rem;
  right: -0.3rem;
  min-width: 1rem;
  height: 1rem;
  padding: 0 0.2rem;
  border-radius: 0.5rem;
  background: #2e6da4;
  color: #fff;
  font-size: 0.65rem;
  font-weight: 700;
  line-height: 1rem;
  text-align: center;
}
@media (max-width: 52rem) {
  .navtoggle {
    display: block;
  }
}
</style>
