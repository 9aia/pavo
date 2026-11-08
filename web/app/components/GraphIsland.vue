<script setup lang="ts">
// Graph ViewNode renderer — the first client-only island. Nodes are
// laid out on a circle (deterministic, no measurement); edges are SVG
// lines with optional labels. Renderers that need real layout should
// use `embed` + a registered component instead.
const props = defineProps<{
  nodes: any[]
  edges: any[]
  layout?: string
}>()

const W = 640
const H = 400
const CX = W / 2
const CY = H / 2
const R = Math.min(W, H) / 2 - 60

interface Placed {
  id: string
  label: string
  kind?: string
  x: number
  y: number
}

const placed = computed<Placed[]>(() => {
  const nodes = (props.nodes ?? []).filter((n) => n && n.id)
  const n = nodes.length
  return nodes.map((node, i) => {
    // Golden-angle spiral reads better than a plain circle for
    // larger graphs and degenerates to a ring for small ones.
    const angle = (i / Math.max(n, 1)) * Math.PI * 2 - Math.PI / 2
    const r = n <= 8 ? R : R * Math.sqrt((i + 1) / n)
    return {
      id: String(node.id),
      label: String(node.label ?? node.id),
      kind: node.kind,
      x: CX + r * Math.cos(angle),
      y: CY + r * Math.sin(angle),
    }
  })
})

const byId = computed(() => new Map(placed.value.map((p) => [p.id, p])))

const renderedEdges = computed(() =>
  (props.edges ?? [])
    .map((e) => {
      const from = byId.value.get(String(e.from))
      const to = byId.value.get(String(e.to))
      return from && to ? { ...e, from, to } : null
    })
    .filter(Boolean),
)

function edgeLabel(e: any): { x: number; y: number; text: string } | null {
  if (!e.label) return null
  return {
    x: (e.from.x + e.to.x) / 2,
    y: (e.from.y + e.to.y) / 2,
    text: String(e.label),
  }
}
</script>

<template>
  <svg
    class="graph"
    :viewBox="`0 0 ${W} ${H}`"
    role="img"
    :aria-label="`${placed.length} nodes, ${renderedEdges.length} edges`"
  >
    <defs>
      <marker
        id="pavo-arrow"
        viewBox="0 0 10 10"
        refX="9"
        refY="5"
        markerWidth="6"
        markerHeight="6"
        orient="auto-start-reverse"
      >
        <path d="M 0 0 L 10 5 L 0 10 z" class="arrowhead" />
      </marker>
    </defs>
    <line
      v-for="(e, i) in renderedEdges"
      :key="`e${i}`"
      :x1="e.from.x"
      :y1="e.from.y"
      :x2="e.to.x"
      :y2="e.to.y"
      class="edge"
      marker-end="url(#pavo-arrow)"
    />
    <text
      v-for="(e, i) in renderedEdges"
      :key="`el${i}`"
      v-bind="edgeLabel(e) ?? {}"
      class="edgelabel"
      text-anchor="middle"
    >
      {{ edgeLabel(e)?.text }}
    </text>
    <g v-for="p in placed" :key="p.id">
      <circle :cx="p.x" :cy="p.y" r="18" class="node" :data-kind="p.kind" />
      <text :x="p.x" :y="p.y + 4" class="nodelabel" text-anchor="middle">
        {{ p.label }}
      </text>
    </g>
  </svg>
  <p v-if="!placed.length" class="muted">(empty graph)</p>
</template>

<style scoped>
.graph {
  width: 100%;
  max-width: 42rem;
  height: auto;
  border: 1px solid color-mix(in srgb, currentColor 12%, transparent);
  border-radius: 8px;
}
.edge {
  stroke: currentColor;
  stroke-opacity: 0.4;
  stroke-width: 1.5;
}
.arrowhead {
  fill: currentColor;
  fill-opacity: 0.6;
}
.edgelabel {
  font-size: 10px;
  fill: currentColor;
  fill-opacity: 0.6;
}
.node {
  fill: color-mix(in srgb, currentColor 12%, transparent);
  stroke: currentColor;
  stroke-opacity: 0.6;
}
.nodelabel {
  font-size: 11px;
  fill: currentColor;
}
.muted {
  opacity: 0.6;
}
</style>
