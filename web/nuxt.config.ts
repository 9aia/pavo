// https://nuxt.com/docs/api/configuration/nuxt-config
export default defineNuxtConfig({
  compatibilityDate: '2025-07-15',
  // The web host is a rendered surface — keep SSR on; view data is
  // fetched client-side (islands) so static export works too.
  ssr: true,
  devtools: { enabled: true },
  runtimeConfig: {
    // Server-side bridge base (reserved for future SSR islands).
    pavoApi: process.env.NUXT_PAVO_API || '',
    public: {
      // Base URL for the pavo bridge API. Empty = same-origin `/api/…`
      // — correct behind `pavo serve` static hosting and, in dev,
      // through the nitro devProxy below. Set
      // NUXT_PUBLIC_PAVO_API=http://127.0.0.1:PORT to hit the bridge
      // directly (CORS is wide open on the bridge).
      pavoApi: process.env.NUXT_PUBLIC_PAVO_API || '',
    },
  },
  nitro: {
    // Static output by default: `npm run build` produces a fully
    // prerendered site in `.output/public`, which `pavo serve` picks
    // up automatically (shell + client-side data fetching — see
    // README). Override with NITRO_PRESET=node-server for a standalone
    // SSR deployment (which also activates server/api/*).
    preset: process.env.NITRO_PRESET || 'static',
  },
  // Note: `nitro.devProxy` does NOT apply under `nuxt dev` (Nuxt 4
  // dev runs on vite, not the nitro dev server) — /api forwarding in
  // dev is handled by `server/api/[...].ts` instead.
})
