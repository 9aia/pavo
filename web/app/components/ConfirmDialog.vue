<script setup lang="ts">
// Danger gate for `invoke` actions — two modes:
//   confirm       → message + confirm/cancel
//   type-target   → additionally requires typing the target's id
//                   (the "type the repo name to delete" pattern;
//                   expected string comes from targetConfirmId)
import { computed, ref } from 'vue'
import type { Action } from '~/utils/interactions'
import { targetConfirmId } from '~/utils/interactions'

const props = defineProps<{
  action: Action
  /** The invoke target context — used to derive the expected id. */
  target?: any
  busy?: boolean
}>()

const emit = defineEmits<{
  confirm: []
  cancel: []
}>()

const mode = computed(() => props.action.danger ?? 'none')
const expected = computed(() => targetConfirmId(props.target))
const typed = ref('')
const canConfirm = computed(() => {
  if (mode.value !== 'type-target') return true
  // With a derivable target id the user must reproduce it exactly;
  // without one, any non-empty confirmation still gates the op.
  return expected.value
    ? typed.value === expected.value
    : typed.value.trim().length > 0
})
</script>

<template>
  <div class="cf-backdrop" @click.self="emit('cancel')">
    <div class="cf-dialog" role="alertdialog" :aria-label="action.label">
      <header class="cf-head">
        <strong class="cf-danger">⚠ {{ action.label }}</strong>
        <code class="muted">{{ action.id }}</code>
      </header>
      <p v-if="action.description" class="cf-desc">{{ action.description }}</p>
      <p v-else class="cf-desc">
        Run <code>{{ action.id }}</code>
        <template v-if="expected"> on <code>{{ expected }}</code></template>
        ?
      </p>

      <template v-if="mode === 'type-target'">
        <p class="cf-typeline">
          Type <code>{{ expected }}</code> to confirm:
        </p>
        <input
          v-model="typed"
          class="cf-input"
          type="text"
          autocomplete="off"
          :placeholder="expected"
          @keydown.enter.prevent="canConfirm && !busy && emit('confirm')"
        />
      </template>

      <footer class="cf-foot">
        <button
          class="cf-go"
          :disabled="!canConfirm || busy"
          @click="emit('confirm')"
        >
          {{ busy ? '…' : mode === 'type-target' ? 'Delete' : 'Confirm' }}
        </button>
        <button :disabled="busy" @click="emit('cancel')">Cancel</button>
      </footer>
    </div>
  </div>
</template>

<style scoped>
.cf-backdrop {
  position: fixed;
  inset: 0;
  z-index: 80;
  background: rgb(0 0 0 / 0.35);
  display: flex;
  align-items: center;
  justify-content: center;
  padding: 1rem;
}
.cf-dialog {
  width: min(26rem, 100%);
  background: Canvas;
  color: CanvasText;
  border: 1px solid #c0392b;
  border-radius: 10px;
  box-shadow: 0 8px 30px rgb(0 0 0 / 0.3);
  padding: 0.9rem 1.1rem;
  display: flex;
  flex-direction: column;
  gap: 0.5rem;
}
.cf-head {
  display: flex;
  align-items: baseline;
  gap: 0.5rem;
}
.cf-head code {
  font-size: 0.75em;
  opacity: 0.6;
}
.cf-danger {
  color: #c0392b;
}
.cf-desc {
  margin: 0;
}
.cf-typeline {
  margin: 0.25rem 0 0;
}
.cf-input {
  font: inherit;
  padding: 0.3rem 0.5rem;
  border: 1px solid color-mix(in srgb, currentColor 25%, transparent);
  border-radius: 6px;
  background: transparent;
  color: inherit;
}
.cf-foot {
  display: flex;
  gap: 0.5rem;
  margin-top: 0.25rem;
}
.cf-foot button {
  font: inherit;
  padding: 0.35rem 1rem;
  border: 1px solid currentColor;
  border-radius: 6px;
  background: transparent;
  color: inherit;
  cursor: pointer;
}
.cf-foot button:disabled {
  opacity: 0.5;
  cursor: default;
}
.cf-go {
  border-color: #c0392b !important;
  color: #c0392b !important;
}
.muted {
  opacity: 0.6;
  font-size: 0.9em;
}
</style>
