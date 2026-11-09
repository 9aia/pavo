<script setup lang="ts">
// Chrome-embedded view surface — renders a `ViewLink` (ref + params)
// through the shared `useView` pipeline inside a fixed panel, for
// header actions and any other popover that "is just a view".
//
// The panel is positioned by CSS (`position: fixed`, top-right);
// the parent owns open/close state and renders the scrim.
import type { ViewLink } from '~/utils/view-link'
import { viewLinkHref } from '~/utils/view-link'

const props = defineProps<{
  link: ViewLink
  /** Panel heading — defaults to the view title/ref once loaded. */
  title?: string
}>()

const emit = defineEmits<{ close: [] }>()

const ref = computed(() => props.link.ref)
// No top-level await — this component mounts via v-if inside a
// popover (no Suspense boundary of its own); useView's fetch is
// client-side and fills `pending`/`viewResult` reactively.
const { pending, apiError, viewResult, refresh } = useView(
  ref,
  computed(() => props.link.params),
)

const heading = computed(
  () => props.title || viewResult.value?.view?.title || ref.value,
)
const pageHref = computed(() => viewLinkHref(props.link))
</script>

<template>
  <div class="view-popover" role="dialog" :aria-label="heading">
    <header class="vp-head">
      <strong class="vp-title">{{ heading }}</strong>
      <code class="vp-ref">{{ ref }}</code>
      <span class="vp-actions">
        <NuxtLink :to="pageHref" class="vp-btn" title="open full page" @click="emit('close')">
          ↗
        </NuxtLink>
        <button class="vp-btn" title="refresh" @click="refresh()">⟳</button>
        <button class="vp-btn" title="close" @click="emit('close')">×</button>
      </span>
    </header>
    <div class="vp-body">
      <p v-if="pending" class="muted">loading {{ ref }}…</p>
      <div v-else-if="apiError" class="error">
        <p>{{ apiError }}</p>
        <p class="muted">unknown driver or view — check `pavo drivers` / `pavo views`.</p>
      </div>
      <template v-else-if="viewResult">
        <p v-if="viewResult.view?.description" class="muted">
          {{ viewResult.view.description }}
        </p>
        <ViewNode :node="viewResult.view.root" :data="viewResult.data || {}" />
      </template>
    </div>
  </div>
</template>

<style scoped>
.view-popover {
  position: fixed;
  top: 2.75rem;
  right: 1rem;
  z-index: 60;
  width: min(34rem, calc(100vw - 2rem));
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
.vp-head {
  display: flex;
  align-items: center;
  gap: 0.5rem;
  padding: 0.5rem 0.75rem;
  border-bottom: 1px solid color-mix(in srgb, currentColor 12%, transparent);
}
.vp-title {
  overflow: hidden;
  text-overflow: ellipsis;
  white-space: nowrap;
}
.vp-ref {
  font-size: 0.7em;
  opacity: 0.6;
  overflow: hidden;
  text-overflow: ellipsis;
  white-space: nowrap;
}
.vp-actions {
  margin-left: auto;
  display: flex;
  gap: 0.15rem;
}
.vp-btn {
  background: none;
  border: none;
  color: inherit;
  font: inherit;
  cursor: pointer;
  padding: 0.15rem 0.4rem;
  border-radius: 6px;
  opacity: 0.75;
  text-decoration: none;
}
.vp-btn:hover {
  opacity: 1;
  background: color-mix(in srgb, currentColor 8%, transparent);
}
.vp-body {
  padding: 0.75rem 1rem;
  overflow: auto;
}
.muted {
  opacity: 0.65;
}
.error {
  color: #c0392b;
}
</style>
