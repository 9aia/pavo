<script setup lang="ts">
// Tree-item "open" affordance — renders its slot (the item label)
// as a button when the owning view declares `ViewSpec.item_route`,
// else as inert text. Activation interpolates the template over
// `{driver, view, data, item}` (`item` = the TreeItem as
// interpolation JSON — props flattened, see
// `utils/commands.ts::itemValue`) and navigates through the same
// route resolution chrome uses (`resolveRoute` → `/view/…` deep
// link or raw path).
//
// Context arrives via inject: `pavoItemRoute` (the template,
// provided by useView) and `pavoNavCtx` ({driver, view, data}).
// Outside a view surface (no provider) the slot renders plainly.
import type { ComputedRef } from 'vue'
import { interpolate, interpolateCtx, itemValue } from '~/utils/commands'
import { useNavigateRoute } from '~/composables/platform'

interface NavCtx {
  driver: string
  view?: string
  data?: Record<string, any>
}

const props = defineProps<{
  /** The raw tree item — `props` keys are addressable as
   *  `{item.<prop>}` in the route template. */
  item: Record<string, any>
}>()

const template = inject<ComputedRef<string | null> | null>(
  'pavoItemRoute',
  null,
)
const navCtx = inject<ComputedRef<NavCtx> | null>('pavoNavCtx', null)
const navigate = useNavigateRoute()

const enabled = computed(() => !!template?.value)

async function open(): Promise<void> {
  const tpl = template?.value
  if (!tpl) return
  const ctx = navCtx?.value ?? { driver: '' }
  const route = interpolate(
    tpl,
    interpolateCtx({
      driver: ctx.driver,
      view: ctx.view,
      data: ctx.data,
      item: itemValue(props.item),
    }),
  )
  await navigate(route)
}
</script>

<template>
  <button
    v-if="enabled"
    type="button"
    class="ilink"
    :title="`open — ${item.label ?? item.id ?? ''}`"
    @click.stop="open()"
  >
    <slot />
  </button>
  <span v-else><slot /></span>
</template>

<style scoped>
.ilink {
  font: inherit;
  padding: 0;
  border: none;
  background: none;
  color: inherit;
  cursor: pointer;
  text-align: left;
}
.ilink:hover {
  text-decoration: underline;
}
</style>
