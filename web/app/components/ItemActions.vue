<script setup lang="ts">
// Per-row action buttons for tree items — `TreeItem.actions[]`
// dispatched through the same `invoke` flow as ActionBar, with the
// item's `id` as the target context (`{"id": …}` — the convention
// pavo-core documents and drivers like tamias-driver-actions read).
//
// The driver/view context comes from `pavoActionCtx` (provided by
// useView); outside a view surface the buttons stay disabled.
import type { ComputedRef } from 'vue'
import type { Action, ScopedAction } from '~/utils/interactions'
import { useActionFlow } from '~/composables/useActionFlow'

const props = defineProps<{
  actions: Action[]
  itemId?: string
}>()

interface ActionCtx {
  driver: string
  view?: string
}
const ctx = inject<ComputedRef<ActionCtx> | null>('pavoActionCtx', null)

const flow = useActionFlow()
const enabled = computed(() => !!ctx?.value?.driver)

function scoped(a: Action): ScopedAction {
  return {
    action: a,
    scope: 'item',
    view: ctx?.value?.view,
    itemId: props.itemId,
    driver: ctx?.value?.driver,
  }
}
</script>

<template>
  <span v-if="actions.length" class="iacts">
    <button
      v-for="a in actions"
      :key="a.id"
      class="iact"
      :class="{ danger: a.danger && a.danger !== 'none' }"
      :title="`${a.id}${a.description ? ` — ${a.description}` : ''}`"
      :disabled="!enabled || flow.state.busy"
      @click.stop="flow.activate(scoped(a), itemId ? { id: itemId } : undefined)"
    >
      {{ a.label }}
    </button>
    <ActionDialogs
      :state="flow.state"
      :submit-form="flow.submitForm"
      :confirm="flow.confirm"
      :cancel="flow.cancel"
    />
  </span>
</template>

<style scoped>
.iacts {
  display: inline-flex;
  gap: 0.25rem;
  margin-left: 0.4rem;
  vertical-align: middle;
}
.iact {
  font: inherit;
  font-size: 0.72rem;
  padding: 0.05rem 0.4rem;
  border: 1px solid color-mix(in srgb, currentColor 25%, transparent);
  border-radius: 5px;
  background: transparent;
  color: inherit;
  cursor: pointer;
  opacity: 0.85;
}
.iact:hover:not(:disabled) {
  opacity: 1;
  border-color: currentColor;
}
.iact:disabled {
  cursor: default;
  opacity: 0.4;
}
.iact.danger {
  border-color: #c0392b;
  color: #c0392b;
}
</style>
