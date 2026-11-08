<script setup lang="ts">
// Dashboard: driver cards + merged chrome contributions + every
// view as a deep link. All data comes from the `pavo serve` bridge
// at request time (client-side fetch — the SSR shell streams first).
const { data: drivers, pending: driversPending, error: driversError } =
  await useFetch('/api/drivers', { server: false })
const { data: chrome } = await useFetch('/api/chrome', { server: false })
const { data: views } = await useFetch('/api/views', { server: false })

function viewHref(namespaced: string) {
  return `/view/${encodeURIComponent(namespaced)}`
}

function slotEntries() {
  if (!chrome.value?.ok) return []
  const r = chrome.value.result as Record<string, unknown[]>
  return Object.entries(r)
    .filter(([slot]) => slot !== 'extra')
    .flatMap(([slot, entries]) =>
      (entries as { id: string; title: string; route: string; driver: string }[]).map(
        (e) => ({ slot, ...e }),
      ),
    )
}
</script>

<template>
  <div>
    <h1>pavo</h1>
    <p class="muted">
      Visualization host — drivers discovered on PATH + $TAMIAS_HOME/drivers,
      rendered through the <code>pavo serve</code> bridge.
    </p>

    <section v-if="driversPending" class="muted">loading drivers…</section>
    <section v-else-if="driversError" class="error">
      bridge unreachable: {{ driversError.message }} — is `pavo serve` running?
    </section>
    <section v-else-if="drivers?.ok === false" class="error">
      {{ drivers.error }}
    </section>

    <template v-else-if="drivers?.ok">
      <h2>Drivers</h2>
      <div v-if="!drivers.result.length" class="muted">
        no pavo-capable drivers found — install a <code>tamias-driver-*</code>
        or set PATH/TAMIAS_HOME.
      </div>
      <div class="grid">
        <div v-for="d in drivers.result" :key="d.driver" class="card">
          <div class="card-head">
            <strong>{{ d.title }}</strong>
            <code>{{ d.driver }}</code>
            <span class="muted">v{{ d.version }} · {{ d.view_count }} view(s)</span>
          </div>
          <div v-if="d.manifest_error" class="error">{{ d.manifest_error }}</div>
          <ul v-else class="contribs">
            <li v-for="(n, slot) in d.contributes" :key="slot">
              <template v-if="n > 0">{{ slot }}: {{ n }}</template>
            </li>
          </ul>
        </div>
      </div>

      <h2>Views</h2>
      <div v-if="views?.ok && views.result.length" class="grid">
        <NuxtLink
          v-for="v in views.result"
          :key="v.namespaced"
          :to="viewHref(v.namespaced)"
          class="card link"
        >
          <strong>{{ v.title }}</strong>
          <code>{{ v.namespaced }}</code>
          <span class="muted">{{ v.route }}</span>
          <p v-if="v.description" class="muted">{{ v.description }}</p>
        </NuxtLink>
      </div>
      <p v-else-if="views?.ok" class="muted">no views.</p>

      <h2>Chrome contributions</h2>
      <ul v-if="slotEntries().length">
        <li v-for="e in slotEntries()" :key="`${e.slot}:${e.id}`">
          <code>{{ e.slot }}</code> —
          <NuxtLink :to="e.route">{{ e.title }}</NuxtLink>
          <span class="muted">({{ e.driver }} · {{ e.id }})</span>
        </li>
      </ul>
      <p v-else class="muted">no contributions.</p>
    </template>
  </div>
</template>

<style scoped>
.muted {
  opacity: 0.65;
}
.error {
  color: #c0392b;
}
.grid {
  display: grid;
  grid-template-columns: repeat(auto-fill, minmax(16rem, 1fr));
  gap: 0.75rem;
}
.card {
  border: 1px solid color-mix(in srgb, currentColor 15%, transparent);
  border-radius: 8px;
  padding: 0.75rem 1rem;
}
.card.link {
  display: flex;
  flex-direction: column;
  gap: 0.15rem;
  text-decoration: none;
  color: inherit;
}
.card.link:hover {
  border-color: currentColor;
}
.card-head {
  display: flex;
  gap: 0.5rem;
  align-items: baseline;
  flex-wrap: wrap;
}
.contribs {
  margin: 0.4rem 0 0;
  padding-left: 1.2rem;
  font-size: 0.85rem;
  opacity: 0.7;
}
.contribs li:empty {
  display: none;
}
</style>
