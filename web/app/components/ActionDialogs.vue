<script setup lang="ts">
// Renders the dialog gates of a `useActionFlow()` instance — the
// param form and/or the danger confirm. Shared by ActionBar and
// CommandPalette so both surfaces run the identical invoke flow.
import type { ActionFlowState } from '~/composables/useActionFlow'

const props = defineProps<{
  state: ActionFlowState
  submitForm: (values: Record<string, any>) => void
  confirm: () => void
  cancel: () => void
}>()
</script>

<template>
  <ActionFormDialog
    v-if="state.dialog === 'form' && state.action"
    :key="`form:${state.action.id}`"
    :action="state.action"
    :errors="state.errors"
    :local-errors="state.localErrors"
    :busy="state.busy"
    @submit="submitForm"
    @cancel="cancel"
  />
  <ConfirmDialog
    v-else-if="state.dialog === 'confirm' && state.action"
    :key="`confirm:${state.action.id}`"
    :action="state.action"
    :target="state.target ?? state.scoped?.itemId"
    :busy="state.busy"
    @confirm="confirm"
    @cancel="cancel"
  />
</template>
