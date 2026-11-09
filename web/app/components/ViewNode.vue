<script setup lang="ts">
// Recursive ViewNode IR renderer — one component per `kind`, with
// KV degradation for unknown kinds (the host's forward-compat rule).
//
// `node` is one ViewNode JSON object (`{kind, …}` — the Rust
// `NodeBase` fields are flattened onto it). `data` is the view's
// query-binding map; a node's `query` field names the binding whose
// payload feeds it.
import type { RendererEntry } from '~/composables/renderers'

const props = defineProps<{
  node: Record<string, any>
  data: Record<string, any>
}>()

const kind = computed(() => String(props.node?.kind ?? ''))

// ---- renderer plugins -------------------------------------------------
// A driver may claim this node's kind via its manifest `renderers`
// (e.g. viz-graph claims "graph"). Claimed nodes route through
// `GET /api/render` → RenderModel; unclaimed/failing nodes keep the
// built-in interpretation below.
const registry = ref<RendererEntry[]>([])
rendererRegistry().then((r) => {
  registry.value = r
})

const pluginRenderer = computed(() => webRendererFor(registry.value, kind.value))

const renderUrl = computed(() => {
  const r = pluginRenderer.value
  if (!r) return null
  const qs = new URLSearchParams({
    kind: kind.value,
    node: JSON.stringify(props.node ?? {}),
    data: JSON.stringify(props.data ?? {}),
    surface: 'web',
  })
  return `/api/render?${qs.toString()}`
})

// RenderModel wire shape: {ok, result: {type: rows|svg|text|placeholder, …}}
const { data: pluginResponse } = await useFetch(renderUrl, {
  server: false,
  watch: [renderUrl],
  immediate: false,
  ignoreResponseError: true,
})

const pluginModel = computed<any>(() => {
  const d = pluginResponse.value as any
  return d?.ok ? d.result : null
})
// Plugin path is active only when a renderer claims the kind AND the
// render op answered — otherwise built-in rendering takes over.
const pluginActive = computed(() => !!pluginRenderer.value && !!pluginModel.value)

// RenderModel → template helpers.
const pluginRows = computed<any[]>(() => pluginModel.value?.rows ?? [])
const pluginHeaders = computed<string[]>(() => pluginModel.value?.headers ?? [])
const pluginTextLines = computed<string[]>(() => pluginModel.value?.lines ?? [])

// The payload bound to this node via `node.query`, if any.
const bound = computed(() => {
  const q = props.node?.query
  if (!q) return undefined
  const d = props.data?.[q]
  return d && typeof d === 'object' && d.error ? undefined : d
})

// ---- kind-specific data resolution ---------------------------------

const tableColumns = computed(() => props.node.columns ?? [])

const tableRows = computed(() => {
  if (Array.isArray(props.node.rows) && props.node.rows.length) {
    return props.node.rows
  }
  const d = bound.value
  if (!Array.isArray(d)) return []
  return d.map((r: any) =>
    Array.isArray(r) ? r : tableColumns.value.map((c: any) => r?.[c.key]),
  )
})

interface TreeItem {
  id?: string
  label: string
  detail?: string
  icon?: string
  children?: TreeItem[]
}

function normTreeItem(x: any): TreeItem {
  if (typeof x === 'string') return { label: x }
  return {
    id: x?.id,
    label: String(x?.label ?? x?.id ?? x?.title ?? '?'),
    detail: x?.detail ?? x?.description,
    icon: x?.icon,
    children: Array.isArray(x?.children) ? x.children.map(normTreeItem) : [],
  }
}

const treeItems = computed<TreeItem[]>(() => {
  if (Array.isArray(props.node.items) && props.node.items.length) {
    return props.node.items.map(normTreeItem)
  }
  const d = bound.value
  const items = Array.isArray(d) ? d : d?.items
  return Array.isArray(items) ? items.map(normTreeItem) : []
})

const kvPairs = computed(() => {
  if (Array.isArray(props.node.pairs) && props.node.pairs.length) {
    return props.node.pairs
  }
  const d = bound.value
  const pairs = Array.isArray(d?.pairs) ? d.pairs : d
  if (Array.isArray(pairs)) {
    // [{key,value}] or [[k,v],…] or bare values
    return pairs.map((p: any) =>
      Array.isArray(p) && p.length === 2 && typeof p[0] === 'string'
        ? { key: p[0], value: p[1] }
        : p && typeof p === 'object' && 'key' in p
          ? p
          : { key: '?', value: p },
    )
  }
  if (pairs && typeof pairs === 'object') {
    return Object.entries(pairs).map(([key, value]) => ({ key, value }))
  }
  return []
})

const graphNodes = computed(() => {
  if (Array.isArray(props.node.nodes) && props.node.nodes.length) {
    return props.node.nodes
  }
  const d = bound.value
  return d?.nodes ?? []
})
const graphEdges = computed(() => {
  if (Array.isArray(props.node.edges) && props.node.edges.length) {
    return props.node.edges
  }
  const d = bound.value
  return d?.edges ?? []
})

const formFields = computed(() => props.node.fields ?? [])

// Tree items recurse through a tiny helper component so the main
// renderer stays one-level-deep per node.
const ViewNodeTreeItems = defineComponent({
  name: 'ViewNodeTreeItems',
  props: {
    items: { type: Array, required: true },
    data: { type: Object, required: true },
  },
  setup(tprops: any) {
    return () =>
      tprops.items.map((item: any, i: number) =>
        h('li', { key: item.id ?? item.label ?? i }, [
          item.icon ? h('span', item.icon + ' ') : null,
          item.label,
          item.detail ? h('span', { class: 'muted' }, ' ' + item.detail) : null,
          item.children?.length
            ? h(
                'ul',
                {},
                h(ViewNodeTreeItems, {
                  items: item.children,
                  data: tprops.data,
                }),
              )
            : null,
        ]),
      )
  },
})

function fmt(v: any): string {
  if (v === null || v === undefined) return '—'
  if (typeof v === 'object') return JSON.stringify(v)
  return String(v)
}
</script>

<template>
  <section class="vnode" :data-kind="kind">
    <h3 v-if="node.title" class="vtitle">{{ node.title }}</h3>
    <p v-if="node.description" class="muted">{{ node.description }}</p>

    <!-- renderer-plugin output: a driver claimed this kind and
         answered /api/render with a RenderModel. svg embeds verbatim
         (plugin owns its layout); rows/text/placeholder map to native
         elements. Falls through to built-in rendering on any failure. -->
    <template v-if="pluginActive">
      <div
        v-if="pluginModel.type === 'svg'"
        class="plugin-svg"
        v-html="pluginModel.markup"
      />
      <div v-else-if="pluginModel.type === 'rows'" class="scroll">
        <table>
          <thead>
            <tr>
              <th v-for="(h, i) in pluginHeaders" :key="i">{{ h }}</th>
            </tr>
          </thead>
          <tbody>
            <tr v-for="(row, ri) in pluginRows" :key="ri">
              <td v-for="(cell, ci) in row" :key="ci">{{ cell }}</td>
            </tr>
          </tbody>
        </table>
      </div>
      <pre v-else-if="pluginModel.type === 'text'" class="plugin-text">{{ pluginTextLines.join('\n') }}</pre>
      <div v-else class="embed">
        <strong>component:</strong> <code>{{ pluginModel.component }}</code>
        <pre v-if="pluginModel.props && Object.keys(pluginModel.props).length">{{ JSON.stringify(pluginModel.props, null, 2) }}</pre>
      </div>
    </template>

    <template v-else>
    <!-- layout containers -->
    <div v-if="kind === 'panel'" class="panel">
      <ViewNode
        v-for="(c, i) in node.children ?? []"
        :key="c.id ?? i"
        :node="c"
        :data="data"
      />
    </div>
    <div v-else-if="kind === 'row'" class="row" :style="{ gap: node.gap ? `${node.gap}px` : undefined }">
      <ViewNode
        v-for="(c, i) in node.children ?? []"
        :key="c.id ?? i"
        :node="c"
        :data="data"
      />
    </div>
    <div v-else-if="kind === 'column'" class="column" :style="{ gap: node.gap ? `${node.gap}px` : undefined }">
      <ViewNode
        v-for="(c, i) in node.children ?? []"
        :key="c.id ?? i"
        :node="c"
        :data="data"
      />
    </div>

    <!-- table -->
    <div v-else-if="kind === 'table'" class="scroll">
      <table>
        <thead>
          <tr>
            <th
              v-for="col in tableColumns"
              :key="col.key"
              :style="{ textAlign: col.align ?? undefined }"
            >
              {{ col.title ?? col.key }}
            </th>
          </tr>
        </thead>
        <tbody>
          <tr v-for="(row, ri) in tableRows" :key="ri">
            <td
              v-for="(cell, ci) in row"
              :key="ci"
              :style="{ textAlign: tableColumns[ci]?.align ?? undefined }"
            >
              {{ fmt(cell) }}
            </td>
          </tr>
          <tr v-if="!tableRows.length">
            <td :colspan="Math.max(tableColumns.length, 1)" class="muted">
              (no rows)
            </td>
          </tr>
        </tbody>
      </table>
    </div>

    <!-- tree -->
    <ul v-else-if="kind === 'tree'" class="tree">
      <li v-for="item in treeItems" :key="item.id ?? item.label">
        <span v-if="item.icon">{{ item.icon }}</span>
        {{ item.label }}
        <span v-if="item.detail" class="muted">{{ item.detail }}</span>
        <ul v-if="item.children?.length">
          <component :is="ViewNodeTreeItems" :items="item.children" :data="data" />
        </ul>
      </li>
      <li v-if="!treeItems.length" class="muted">(empty)</li>
    </ul>

    <!-- graph — client-only island -->
    <ClientOnly v-else-if="kind === 'graph'">
      <GraphIsland :nodes="graphNodes" :edges="graphEdges" :layout="node.layout" />
      <template #fallback>
        <div class="muted">graph: {{ graphNodes.length }} node(s) — rendering…</div>
      </template>
    </ClientOnly>

    <!-- form — interactive (submit wire op) when the driver declared
         submit_op; read-only field list otherwise -->
    <FormView v-if="kind === 'form' && node.submit_op" :node="node" />
    <div v-else-if="kind === 'form'" class="form">
      <dl>
        <template v-for="f in formFields" :key="f.key">
          <dt>{{ f.label ?? f.key }} <code class="muted">{{ f.key }}</code></dt>
          <dd>
            {{ fmt(f.value ?? f.default ?? (f.kind === 'bool' ? false : '')) }}
            <span v-if="f.hint" class="muted"> — {{ f.hint }}</span>
          </dd>
        </template>
      </dl>
    </div>

    <!-- kv -->
    <dl v-else-if="kind === 'kv'" class="kv">
      <template v-for="p in kvPairs" :key="p.key">
        <dt>{{ p.key }}</dt>
        <dd>{{ fmt(p.value) }}</dd>
      </template>
      <template v-if="!kvPairs.length">
        <dt class="muted">(no pairs)</dt>
      </template>
    </dl>

    <!-- markdown — pre-wrap text, no md dep in v1 -->
    <pre v-else-if="kind === 'markdown'" class="markdown">{{ node.text ?? '' }}</pre>

    <!-- embed — escape hatch placeholder -->
    <div v-else-if="kind === 'embed'" class="embed">
      <strong>embed:</strong> <code>{{ node.component }}</code>
      <pre v-if="node.props && Object.keys(node.props).length">{{ JSON.stringify(node.props, null, 2) }}</pre>
    </div>

    <!-- unknown kind → KV-style degradation of the raw node -->
    <div v-else class="unknown">
      <p class="muted">unknown node kind <code>{{ kind || '(missing)' }}</code> — degraded view:</p>
      <pre>{{ JSON.stringify(node, null, 2) }}</pre>
    </div>
    </template>
  </section>
</template>

<style scoped>
.vnode {
  margin: 0.5rem 0;
}
.vtitle {
  margin: 0.25rem 0;
  font-size: 1rem;
}
.plugin-svg :deep(svg) {
  max-width: 100%;
  height: auto;
}
.plugin-text {
  font-family: ui-monospace, monospace;
  white-space: pre-wrap;
}
.muted {
  opacity: 0.6;
  font-size: 0.9em;
}
.row {
  display: flex;
  flex-wrap: wrap;
}
.row > .vnode {
  flex: 1 1 16rem;
}
.column {
  display: flex;
  flex-direction: column;
}
.panel {
  border: 1px solid color-mix(in srgb, currentColor 12%, transparent);
  border-radius: 8px;
  padding: 0.75rem;
}
.scroll {
  overflow-x: auto;
}
table {
  border-collapse: collapse;
  width: 100%;
}
th,
td {
  border: 1px solid color-mix(in srgb, currentColor 15%, transparent);
  padding: 0.3rem 0.6rem;
  text-align: left;
}
th {
  background: color-mix(in srgb, currentColor 6%, transparent);
}
.tree,
.tree ul {
  list-style: none;
  padding-left: 1.2rem;
}
.kv {
  display: grid;
  grid-template-columns: max-content 1fr;
  gap: 0.15rem 1rem;
}
.kv dt {
  font-weight: 600;
}
.kv dd {
  margin: 0;
}
.markdown {
  white-space: pre-wrap;
  font-family: inherit;
}
.embed {
  border: 1px dashed color-mix(in srgb, currentColor 30%, transparent);
  border-radius: 8px;
  padding: 0.75rem;
}
.unknown pre,
.embed pre {
  overflow-x: auto;
  font-size: 0.8rem;
}
</style>
