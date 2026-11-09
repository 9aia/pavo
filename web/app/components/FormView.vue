<script setup lang="ts">
// Interactive `form` ViewNode — the web surface of the `submit`
// wire op. Rendered only when the form declares `submit_op`
// (otherwise ViewNode falls back to the read-only field list).
//
// The view ref (`driver:view`) and the view-swap callback are
// provided by the `/view/[...ref]` page (`pavoViewRef` /
// `pavoApplyView`); outside a view page the form renders but the
// submit button stays disabled.
import type { ComputedRef, Ref } from 'vue'

const props = defineProps<{
  node: Record<string, any>
}>()

const viewRef = inject<Ref<string> | ComputedRef<string> | null>(
  'pavoViewRef',
  null,
)
const applyView = inject<(vr: any) => void>('pavoApplyView', () => {})

interface Field {
  key: string
  label?: string
  kind?: string
  required?: boolean
  editable?: boolean
  options?: string[]
  default?: any
  value?: any
  hint?: string
  placeholder?: string
}

const fields = computed<Field[]>(() => props.node?.fields ?? [])
const editableFields = computed(() =>
  fields.value.filter((f) => f.key && f.editable !== false),
)
const canSubmit = computed(() => !!viewRef?.value && !!props.node?.submit_op)

function seed(f: Field): any {
  if (f.value !== undefined && f.value !== null) return f.value
  if (f.default !== undefined && f.default !== null) return f.default
  if (f.kind === 'bool') return false
  if (f.kind === 'select') return f.options?.[0] ?? ''
  return ''
}

// Live field values, seeded from `value`/`default` once per mount —
// the node prop is fixed for the life of this component instance.
const values = ref<Record<string, any>>({})
for (const f of fields.value) {
  values.value[f.key] = seed(f)
}

const busy = ref(false)
const message = ref<string | null>(null)
const errors = ref<Record<string, string>>({})
const hasErrors = computed(() => Object.keys(errors.value).length > 0)

function empty(v: any): boolean {
  return v === undefined || v === null || v === ''
}

// `number` fields submit JSON numbers when parseable; `bool` submits
// real booleans; everything else submits the string verbatim.
function coerce(f: Field, v: any): any {
  if (f.kind === 'number' && !empty(v)) {
    const n = Number(v)
    return Number.isNaN(n) ? v : n
  }
  if (f.kind === 'bool') return !!v
  return v
}

async function submit() {
  if (!canSubmit.value || busy.value) return
  busy.value = true
  message.value = null
  errors.value = {}
  // Local required check — same rule the driver enforces, saves a
  // round-trip and keeps the message next to the fields.
  const local: Record<string, string> = {}
  for (const f of editableFields.value) {
    if (f.required && empty(values.value[f.key])) local[f.key] = 'required'
  }
  if (Object.keys(local).length) {
    errors.value = local
    message.value = 'missing required fields'
    busy.value = false
    return
  }
  const payload: Record<string, any> = {}
  for (const f of editableFields.value) {
    payload[f.key] = coerce(f, values.value[f.key])
  }
  const qs = new URLSearchParams({
    ref: String(viewRef?.value ?? ''),
    values: JSON.stringify(payload),
  })
  if (props.node?.id) qs.set('form', String(props.node.id))
  try {
    const d: any = await $fetch(`/api/submit?${qs.toString()}`)
    if (d?.ok) {
      const res = d.result ?? {}
      message.value = res.message ?? 'submitted'
      errors.value = Object.fromEntries(
        Object.entries(res.errors ?? {}).map(([k, v]) => [k, String(v)]),
      )
      // A fresh ViewResult swaps in place of the current view.
      if (res.view) applyView(res.view)
    } else {
      message.value = d?.error ?? 'submit failed'
    }
  } catch (e: any) {
    message.value = `submit failed: ${e?.data?.error ?? e?.message ?? e}`
  } finally {
    busy.value = false
  }
}
</script>

<template>
  <form class="fform" @submit.prevent="submit">
    <div
      v-for="f in fields"
      :key="f.key"
      class="field"
      :class="{ readonly: f.editable === false }"
    >
      <span class="flabel">
        {{ f.label ?? f.key }}
        <code class="muted">{{ f.key }}</code>
        <span v-if="f.required" class="req" title="required">*</span>
      </span>

      <template v-if="f.editable !== false">
        <select v-if="f.kind === 'select'" v-model="values[f.key]">
          <option
            v-for="o in f.options ?? []"
            :key="o"
            :value="o"
          >
            {{ o }}
          </option>
        </select>
        <label v-else-if="f.kind === 'bool'" class="fbool">
          <input v-model="values[f.key]" type="checkbox" />
          <span>{{ values[f.key] ? 'true' : 'false' }}</span>
        </label>
        <textarea
          v-else-if="f.kind === 'textarea'"
          v-model="values[f.key]"
          :placeholder="f.placeholder"
          rows="3"
        />
        <input
          v-else
          v-model="values[f.key]"
          :type="f.kind === 'number' ? 'number' : 'text'"
          :placeholder="f.placeholder"
        />
        <p v-if="errors[f.key]" class="error">{{ errors[f.key] }}</p>
      </template>
      <p v-else class="muted readonly-value">
        {{ f.value ?? f.default ?? '' }}
        <span v-if="f.hint"> — {{ f.hint }}</span>
      </p>
      <p v-if="f.editable !== false && f.hint" class="muted hint">
        {{ f.hint }}
      </p>
    </div>

    <div class="fform-actions">
      <button type="submit" :disabled="!canSubmit || busy">
        {{ busy ? '…' : node.submit_label ?? 'Submit' }}
      </button>
      <span v-if="!viewRef?.value" class="muted">
        (no view ref — form is preview-only here)
      </span>
      <span v-if="node.submit_op" class="muted">
        submits via <code>{{ node.submit_op }}</code>
      </span>
    </div>
    <p v-if="message" class="fform-message" :class="{ error: hasErrors }">
      {{ message }}
    </p>
  </form>
</template>

<style scoped>
.fform {
  display: flex;
  flex-direction: column;
  gap: 0.6rem;
  max-width: 34rem;
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
.readonly {
  opacity: 0.65;
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
.fform-actions {
  display: flex;
  align-items: center;
  gap: 0.75rem;
  margin-top: 0.25rem;
}
.fform-actions button {
  font: inherit;
  padding: 0.35rem 1rem;
  border: 1px solid currentColor;
  border-radius: 6px;
  background: transparent;
  color: inherit;
  cursor: pointer;
}
.fform-actions button:disabled {
  opacity: 0.5;
  cursor: default;
}
.fform-message {
  margin: 0;
}
.fform-message.error {
  color: #c0392b;
}
.muted {
  opacity: 0.6;
  font-size: 0.9em;
}
</style>
