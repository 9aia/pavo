<script setup lang="ts">
// `/jobs` — the aggregated job feed (`GET /api/jobs`): every
// `contributes.jobs` provider's `jobs` op flattened with driver
// attribution. Dense table grouped by status — active (non-terminal)
// groups first, terminal last; rows sort by `updated_at` desc.
// Status vocabularies are driver-owned, so grouping is by the raw
// string and "terminal" is only a display heuristic.
import type { JobRow } from '~/composables/platform'
import {
  isJobTerminal,
  routeTargetWithViews,
  timeAgo,
} from '~/composables/platform'
import type { ViewListEntry } from '~/utils/view-link'
import { viewLinkHref } from '~/utils/view-link'

const { data: jobsData, pending, error } = useJobs()
const { data: viewsData } = useViews()

const rows = computed<JobRow[]>(() => {
  const d = jobsData.value as { ok?: boolean; result?: JobRow[] } | null
  return d?.ok && Array.isArray(d.result) ? d.result : []
})
const views = computed<ViewListEntry[]>(() => {
  const d = viewsData.value as { ok?: boolean; result?: ViewListEntry[] } | null
  return d?.ok && Array.isArray(d.result) ? d.result : []
})
const apiError = computed(() => {
  const d = jobsData.value as { ok?: boolean; error?: string } | null
  if (error.value) return error.value.message
  if (d && d.ok === false) return d.error || 'request failed'
  return null
})

interface JobGroup {
  status: string
  terminal: boolean
  rows: (JobRow & { href: string | null })[]
}

// Rows + resolved hrefs — computed once views land so templates
// stay expression-simple.
const jobRows = computed(() =>
  rows.value.map((j) => ({ ...j, href: hrefFor(j.route) })),
)

const groups = computed<JobGroup[]>(() => {
  const byStatus = new Map<string, (JobRow & { href: string | null })[]>()
  for (const j of jobRows.value) {
    const s = j.status || '(no status)'
    const list = byStatus.get(s) ?? []
    list.push(j)
    byStatus.set(s, list)
  }
  return [...byStatus.entries()]
    .map(([status, list]) => ({
      status,
      terminal: isJobTerminal(status),
      rows: [...list].sort((a, b) =>
        (b.updated_at ?? '').localeCompare(a.updated_at ?? ''),
      ),
    }))
    .sort(
      (a, b) =>
        Number(a.terminal) - Number(b.terminal) || a.status.localeCompare(b.status),
    )
})

/** Route → href: view routes become `/view/<ref>` deep links once
 *  the views list resolves; anything else keeps its raw route. */
function hrefFor(route: string | undefined): string | null {
  if (!route) return null
  const t = routeTargetWithViews(route, views.value)
  return t.type === 'view' ? viewLinkHref(t.link) : t.to
}
</script>

<template>
  <div>
    <h1>Jobs <code class="muted">{{ rows.length }}</code></h1>
    <p v-if="pending" class="muted">loading…</p>
    <div v-else-if="apiError" class="error">{{ apiError }}</div>
    <p v-else-if="!rows.length" class="muted">
      no jobs — drivers opt in via <code>contributes.jobs</code> +
      the <code>jobs</code> wire op.
    </p>

    <section v-for="g in groups" :key="g.status" class="job-group">
      <h2>
        <span class="status-chip" :class="{ terminal: g.terminal }">{{ g.status }}</span>
        <span class="muted">{{ g.rows.length }}</span>
      </h2>
      <table class="jobs">
        <tbody>
          <tr v-for="j in g.rows" :key="`${j.driver}:${j.id}`">
            <td class="j-title">
              <NuxtLink v-if="j.href" :to="j.href" class="j-link">
                {{ j.title }}
              </NuxtLink>
              <template v-else>{{ j.title }}</template>
              <div v-if="j.detail" class="muted j-detail">{{ j.detail }}</div>
            </td>
            <td class="muted j-driver">{{ j.driver_title || j.driver }}</td>
            <td class="muted j-updated" :title="j.updated_at">
              {{ timeAgo(j.updated_at) }}
            </td>
          </tr>
        </tbody>
      </table>
    </section>
  </div>
</template>

<style scoped>
.muted {
  opacity: 0.65;
}
.error {
  color: #c0392b;
}
.job-group {
  margin-bottom: 1.25rem;
}
.job-group h2 {
  display: flex;
  align-items: baseline;
  gap: 0.5rem;
  font-size: 1rem;
  margin: 0.5rem 0 0.25rem;
}
.status-chip {
  border: 1px solid color-mix(in srgb, currentColor 25%, transparent);
  border-radius: 999px;
  padding: 0.05rem 0.6rem;
  font-size: 0.8rem;
  font-weight: 600;
}
.status-chip.terminal {
  opacity: 0.55;
}
.jobs {
  border-collapse: collapse;
  width: 100%;
}
.jobs td {
  border-top: 1px solid color-mix(in srgb, currentColor 10%, transparent);
  padding: 0.3rem 0.5rem 0.3rem 0;
  vertical-align: top;
}
.j-title {
  width: 100%;
}
.j-link {
  color: inherit;
}
.j-link:hover {
  text-decoration: underline;
}
.j-detail {
  font-size: 0.85rem;
}
.j-driver,
.j-updated {
  white-space: nowrap;
  font-size: 0.85rem;
  text-align: right;
}
</style>
