// Dev-time /api proxy — `nuxt dev` has no bridge of its own, so
// forward every /api/* request to the `pavo serve` process.
//
// Target resolution: PAVO_API_TARGET > NUXT_PUBLIC_PAVO_API >
// http://127.0.0.1:7420. Run the bridge on that port
// (`pavo serve --port 7420`) or point the env vars at it.
//
// In the static build this route simply isn't shipped — the browser
// hits `pavo serve`'s own /api/* endpoints same-origin instead.
export default defineEventHandler((event) => {
  const target =
    process.env.PAVO_API_TARGET ||
    process.env.NUXT_PUBLIC_PAVO_API ||
    'http://127.0.0.1:7420'
  // event.path keeps the /api/... prefix — the bridge serves it
  // verbatim.
  return sendProxy(event, target + event.path)
})
