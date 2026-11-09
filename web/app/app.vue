<script setup lang="ts">
// App shell — the platform frame: left sidebar (brand + grouped nav
// merged from the `sidebar`/`navbar` chrome slots), top header bar
// (headerActions + jobs + notifications), content outlet, optional
// statusbar footer. Chrome is fetched client-side like everything
// else — the SSR shell renders instantly, islands hydrate.
import type { ChromeSlots } from '~/composables/platform'

const { data: chrome } = await useChrome()
const slots = computed<ChromeSlots | null>(() => {
  const d = chrome.value as { ok?: boolean; result?: ChromeSlots } | null
  return d?.ok ? (d.result ?? null) : null
})

const navOpen = ref(false)
</script>

<template>
  <div class="shell" :class="{ 'nav-open': navOpen }">
    <AppSidebar
      :sidebar="slots?.sidebar ?? []"
      :navbar="slots?.navbar ?? []"
      :open="navOpen"
      @close="navOpen = false"
    />
    <div class="bodycol">
      <AppHeader :chrome="slots" @toggle-nav="navOpen = !navOpen" />
      <main class="content">
        <NuxtPage />
      </main>
      <footer v-if="slots?.statusbar?.length" class="statusbar">
        <span
          v-for="e in slots.statusbar"
          :key="e.id"
          class="status"
          :title="`${e.driver} · ${e.id}`"
        >
          {{ e.title }}
        </span>
      </footer>
    </div>
  </div>
</template>

<style>
:root {
  color-scheme: light dark;
}
body {
  margin: 0;
  font-family: system-ui, -apple-system, sans-serif;
}
.shell {
  display: flex;
  min-height: 100vh;
}
.bodycol {
  flex: 1;
  min-width: 0;
  display: flex;
  flex-direction: column;
}
.content {
  flex: 1;
  padding: 1rem;
  max-width: 72rem;
  width: 100%;
  margin: 0 auto;
  box-sizing: border-box;
}
.statusbar {
  display: flex;
  gap: 1.5rem;
  padding: 0.35rem 1rem;
  border-top: 1px solid color-mix(in srgb, currentColor 15%, transparent);
  font-size: 0.8rem;
  opacity: 0.7;
}
</style>
