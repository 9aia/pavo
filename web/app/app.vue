<script setup lang="ts">
// App shell: header with merged navbar contributions (chrome) +
// the page outlet. Chrome is fetched client-side like everything
// else — the SSR shell renders instantly, islands hydrate.
const { data: chrome } = await useFetch('/api/chrome', { server: false })
</script>

<template>
  <div class="shell">
    <header class="topbar">
      <NuxtLink to="/" class="brand">pavo</NuxtLink>
      <nav v-if="chrome?.ok">
        <NuxtLink
          v-for="e in chrome.result.navbar"
          :key="e.id"
          :to="e.route"
          class="navlink"
        >
          {{ e.title }}
        </NuxtLink>
      </nav>
      <span class="spacer" />
      <nav>
        <NuxtLink to="/" class="navlink">dashboard</NuxtLink>
      </nav>
    </header>
    <main class="content">
      <NuxtPage />
    </main>
    <footer v-if="chrome?.ok && chrome.result.statusbar.length" class="statusbar">
      <span
        v-for="e in chrome.result.statusbar"
        :key="e.id"
        class="status"
        :title="`${e.driver} · ${e.id}`"
      >
        {{ e.title }}
      </span>
    </footer>
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
  flex-direction: column;
  min-height: 100vh;
}
.topbar {
  display: flex;
  align-items: center;
  gap: 1rem;
  padding: 0.5rem 1rem;
  border-bottom: 1px solid color-mix(in srgb, currentColor 15%, transparent);
}
.brand {
  font-weight: 700;
  text-decoration: none;
  color: inherit;
}
nav {
  display: flex;
  gap: 0.75rem;
}
.navlink {
  color: inherit;
  text-decoration: none;
  opacity: 0.8;
}
.navlink:hover {
  opacity: 1;
  text-decoration: underline;
}
.spacer {
  flex: 1;
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
