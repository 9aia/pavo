<script setup lang="ts">
// Action bar — the web rendering of a view's `actions[]`
// (manifest-global + node/item-scoped, already collected and
// deduped by the caller). Safe actions fire on click; actions with
// `params[]` open the auto-generated form; `danger` actions gate
// through ConfirmDialog. All dispatch rides `useActionFlow` → the
// `invoke` wire op via POST /api/call.
import type { ScopedAction } from '~/utils/interactions'
import { useActionFlow } from '~/composables/useActionFlow'

const props = defineProps<{
  actions: ScopedAction[]
}>()

const flow = useActionFlow()

const normal = computed(() =>
  props.actions.filter((s) => !s.action.danger || s.action.danger === 'none'),
)
const dangerous = computed(() =>
  props.actions.filter((s) => s.action.danger && s.action.danger !== 'none'),
)

function tip(s: ScopedAction): string {
  const a = s.action
  const parts = [a.id]
  if (s.scope === 'item' && s.itemId) parts.push(`on ${s.itemId}`)
  else if (s.scope === 'node') parts.push('node')
  if (a.keys?.length) parts.push(a.keys.join('/'))
  if (a.description) parts.push(a.description)
  return parts.join(' · ')
}
</script>

<template>
  <div v-if="actions.length" class="abar">
    <div class="abtns">
      <button
        v-for="s in normal"
        :key="`${s.scope}:${s.action.id}:${s.itemId ?? ''}`"
        class="abtn"
        :title="tip(s)"
        :disabled="flow.state.busy"
        @click="flow.activate(s)"
      >
        <span v-if="s.action.icon">{{ s.action.icon }}</span>
        {{ s.action.label }}
      </button>
      <button
        v-for="s in dangerous"
        :key="`d:${s.scope}:${s.action.id}:${s.itemId ?? ''}`"
        class="abtn danger"
        :title="tip(s)"
        :disabled="flow.state.busy"
        @click="flow.activate(s)"
      >
        <span v-if="s.action.icon">{{ s.action.icon }}</span>
        {{ s.action.label }}
      </button>
    </div>
    <p v-if="flow.state.message" class="amsg" :class="{ error: !!flow.state.errors && Object.keys(flow.state.errors).length }">
      {{ flow.state.message }}
    </p>
    <ActionDialogs
      :state="flow.state"
      :submit-form="flow.submitForm"
      :confirm="flow.confirm"
      :cancel="flow.cancel"
    />
  </div>
</template>

<style scoped>
.abar {
  margin: 0.5rem 0;
}
.abtns {
  display: flex;
  flex-wrap: wrap;
  gap: 0.4rem;
}
.abtn {
  font: inherit;
  font-size: 0.85rem;
  padding: 0.25rem 0.7rem;
  border: 1px solid color-mix(in srgb, currentColor 25%, transparent);
  border-radius: 6px;
  background: transparent;
  color: inherit;
  cursor: pointer;
}
.abtn:hover:not(:disabled) {
  border-color: currentColor;
}
.abtn:disabled {
  opacity: 0.5;
  cursor: default;
}
.abtn.danger {
  border-color: #c0392b;
  color: #c0392b;
}
.amsg {
  margin: 0.35rem 0 0;
  font-size: 0.9em;
}
.amsg.error {
  color: #c0392b;
}
</style>
