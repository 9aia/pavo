<script setup lang="ts">
// Auto-generated param form for an `invoke` action — one widget per
// ParamSchema.kind (the host half of the protocol's "hosts
// auto-generate forms from params[]" rule). Validation is UX-only;
// the driver stays authoritative via ActionResult.errors.
import { computed, reactive } from 'vue'
import type { Action, ParamSchema } from '~/utils/interactions'
import { humanize } from '~/utils/interactions'

const props = defineProps<{
  action: Action
  /** Server-side errors keyed by param name (ActionResult.errors). */
  errors?: Record<string, string>
  /** Client-side validation errors from the flow. */
  localErrors?: Record<string, string>
  busy?: boolean
}>()

const emit = defineEmits<{
  submit: [values: Record<string, any>]
  cancel: []
}>()

const params = computed<ParamSchema[]>(() => props.action.params ?? [])

function seed(p: ParamSchema): any {
  if (p.default !== undefined && p.default !== null) return p.default
  switch (p.kind ?? 'string') {
    case 'bool':
      return false
    case 'enum':
      return p.options?.[0] ?? ''
    case 'string-list':
      return Array.isArray(p.default) ? p.default.join(', ') : ''
    default:
      return ''
  }
}

const values = reactive<Record<string, any>>({})
for (const p of params.value) values[p.name] = seed(p)

function fieldError(name: string): string | undefined {
  return props.localErrors?.[name] ?? props.errors?.[name]
}

function hint(p: ParamSchema): string | undefined {
  const parts = [p.hint ?? p.doc]
  if (p.kind === 'ref' && p.ref_query) parts.push(`options from "${p.ref_query}"`)
  if (p.kind === 'string-list') parts.push('comma-separated')
  if (p.kind === 'date') parts.push('YYYY-MM-DD')
  return parts.filter(Boolean).join(' · ') || undefined
}

function inputType(p: ParamSchema): string {
  switch (p.kind ?? 'string') {
    case 'number':
    case 'integer':
      return 'number'
    case 'date':
      return 'date'
    default:
      return 'text'
  }
}
</script>

<template>
  <div class="af-backdrop" @click.self="emit('cancel')">
    <form class="af-dialog" role="dialog" @submit.prevent="emit('submit', { ...values })">
      <header class="af-head">
        <strong>{{ action.label }}</strong>
        <code class="muted">{{ action.id }}</code>
        <button type="button" class="af-x" title="cancel" @click="emit('cancel')">×</button>
      </header>
      <p v-if="action.description" class="muted af-desc">{{ action.description }}</p>

      <div v-for="p in params" :key="p.name" class="field">
        <span class="flabel">
          {{ p.label ?? humanize(p.name) }}
          <code class="muted">{{ p.name }}</code>
          <span v-if="p.required" class="req" title="required">*</span>
        </span>

        <label v-if="(p.kind ?? 'string') === 'bool'" class="fbool">
          <input v-model="values[p.name]" type="checkbox" />
          <span>{{ values[p.name] ? 'true' : 'false' }}</span>
        </label>
        <select
          v-else-if="p.kind === 'enum' || (p.kind === 'ref' && p.options?.length)"
          v-model="values[p.name]"
        >
          <option value="" :disabled="p.required">—</option>
          <option v-for="o in p.options ?? []" :key="o" :value="o">{{ o }}</option>
        </select>
        <textarea
          v-else-if="p.kind === 'text'"
          v-model="values[p.name]"
          :placeholder="p.placeholder"
          rows="3"
        />
        <input
          v-else
          v-model="values[p.name]"
          :type="inputType(p)"
          :placeholder="p.placeholder"
          :step="p.kind === 'integer' ? 1 : 'any'"
          :min="p.min"
          :max="p.max"
          :pattern="p.pattern"
        />
        <p v-if="fieldError(p.name)" class="error">{{ fieldError(p.name) }}</p>
        <p v-else-if="hint(p)" class="muted hint">{{ hint(p) }}</p>
      </div>

      <footer class="af-foot">
        <button type="submit" :disabled="busy">{{ busy ? '…' : 'Run' }}</button>
        <button type="button" :disabled="busy" @click="emit('cancel')">Cancel</button>
      </footer>
    </form>
  </div>
</template>

<style scoped>
.af-backdrop {
  position: fixed;
  inset: 0;
  z-index: 80;
  background: rgb(0 0 0 / 0.35);
  display: flex;
  align-items: flex-start;
  justify-content: center;
  padding: 4rem 1rem 1rem;
}
.af-dialog {
  width: min(30rem, 100%);
  max-height: calc(100vh - 5rem);
  overflow: auto;
  background: Canvas;
  color: CanvasText;
  border: 1px solid color-mix(in srgb, currentColor 20%, transparent);
  border-radius: 10px;
  box-shadow: 0 8px 30px rgb(0 0 0 / 0.3);
  padding: 0.75rem 1rem 1rem;
  display: flex;
  flex-direction: column;
  gap: 0.6rem;
}
.af-head {
  display: flex;
  align-items: baseline;
  gap: 0.5rem;
}
.af-head code {
  font-size: 0.75em;
  opacity: 0.6;
}
.af-x {
  margin-left: auto;
  background: none;
  border: none;
  color: inherit;
  font-size: 1.1rem;
  cursor: pointer;
  opacity: 0.7;
}
.af-x:hover {
  opacity: 1;
}
.af-desc {
  margin: 0;
}
.field {
  display: flex;
  flex-direction: column;
  gap: 0.2rem;
}
.flabel {
  font-weight: 600;
  font-size: 0.9rem;
}
.req {
  color: #c0392b;
}
.field input[type='text'],
.field input[type='number'],
.field input[type='date'],
.field select,
.field textarea {
  font: inherit;
  padding: 0.3rem 0.5rem;
  border: 1px solid color-mix(in srgb, currentColor 25%, transparent);
  border-radius: 6px;
  background: transparent;
  color: inherit;
  max-width: 100%;
}
.field textarea {
  resize: vertical;
}
.fbool {
  display: flex;
  align-items: center;
  gap: 0.4rem;
}
.hint {
  margin: 0;
  font-size: 0.85em;
}
.error {
  color: #c0392b;
  margin: 0;
  font-size: 0.9em;
}
.af-foot {
  display: flex;
  gap: 0.5rem;
  margin-top: 0.25rem;
}
.af-foot button {
  font: inherit;
  padding: 0.35rem 1rem;
  border: 1px solid currentColor;
  border-radius: 6px;
  background: transparent;
  color: inherit;
  cursor: pointer;
}
.af-foot button:disabled {
  opacity: 0.5;
  cursor: default;
}
.muted {
  opacity: 0.6;
  font-size: 0.9em;
}
</style>
