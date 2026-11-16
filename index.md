---
layout: home

hero:
  name: pavo
  text: visualization host for the agent factory
  tagline: tamias drivers emit declarative ViewNode IR — pavo renders it on CLI, TUI, and web
  actions:
    - theme: brand
      text: Getting Started
      link: /tutorials/getting-started
    - theme: alt
      text: Architecture
      link: /explanation/architecture

features:
  - icon: '<svg xmlns="http://www.w3.org/2000/svg" width="48" height="48" viewBox="0 0 24 24" fill="none" stroke="currentColor" stroke-width="2" stroke-linecap="round" stroke-linejoin="round"><path d="M14.086 18.412A2 2 0 0112.67 19H5v-7.672a2 2 0 01.586-1.414L11.75 3.75a6 6 0 118.49 8.49z" /> <path d="M16 8 2 22" /> <path d="M17.488 15H9" /></svg>'
    title: One host, every surface
    details: drivers ship views once; pavo interprets the same ViewNode IR on CLI, TUI (ratatui), and web (Nuxt)
    link: /explanation/architecture
    linkText: architecture
  - icon: '<svg xmlns="http://www.w3.org/2000/svg" width="48" height="48" viewBox="0 0 24 24" fill="none" stroke="currentColor" stroke-width="2" stroke-linecap="round" stroke-linejoin="round"><path d="M15.39 4.39a1 1 0 0 0 1.68-.474 2.5 2.5 0 1 1 3.014 3.015 1 1 0 0 0-.474 1.68l1.683 1.682a2.414 2.414 0 0 1 0 3.414L19.61 15.39a1 1 0 0 1-1.68-.474 2.5 2.5 0 1 0-3.014 3.015 1 1 0 0 1 .474 1.68l-1.683 1.682a2.414 2.414 0 0 1-3.414 0L8.61 19.61a1 1 0 0 0-1.68.474 2.5 2.5 0 1 1-3.014-3.015 1 1 0 0 0 .474-1.68l-1.683-1.682a2.414 2.414 0 0 1 0-3.414L4.39 8.61a1 1 0 0 1 1.68.474 2.5 2.5 0 1 0 3.014-3.015 1 1 0 0 1-.474-1.68l1.683-1.682a2.414 2.414 0 0 1 3.414 0z" /></svg>'
    title: Drivers, not plugins
    details: a view provider is a tamias-driver-* executable speaking an ndjson JSON-RPC wire — discovered on PATH or in the tamias drivers dir
    link: /how-to/write-a-driver
    linkText: write a driver
  - icon: '<svg xmlns="http://www.w3.org/2000/svg" width="48" height="48" viewBox="0 0 24 24" fill="none" stroke="currentColor" stroke-width="2" stroke-linecap="round" stroke-linejoin="round"><path d="M12 22a1 1 0 0 1 0-20 10 9 0 0 1 10 9 5 5 0 0 1-5 5h-2.25a1.75 1.75 0 0 0-1.4 2.8l.3.4a1.75 1.75 0 0 1-1.4 2.8z" /> <circle cx="13.5" cy="6.5" r=".5" fill="currentColor" /> <circle cx="17.5" cy="10.5" r=".5" fill="currentColor" /> <circle cx="6.5" cy="12.5" r=".5" fill="currentColor" /> <circle cx="8.5" cy="7.5" r=".5" fill="currentColor" /></svg>'
    title: Renderer plugins
    details: drivers can claim view kinds outright — a manifest renderers[] declaration plus the render op returning a canonical RenderModel
    link: /explanation/renderer-plugins
    linkText: renderer plugins
  - icon: '<svg xmlns="http://www.w3.org/2000/svg" width="48" height="48" viewBox="0 0 24 24" fill="none" stroke="currentColor" stroke-width="2" stroke-linecap="round" stroke-linejoin="round"><rect width="20" height="14" x="2" y="3" rx="2" /> <line x1="8" x2="16" y1="21" y2="21" /> <line x1="12" x2="12" y1="17" y2="21" /></svg>'
    title: Platform chrome
    details: eight contribution slots, driver deps, cross-driver queries, and aggregated jobs/notifications feeds compose one shared screen
    link: /explanation/architecture#platform-host
    linkText: platform host
  - icon: '<svg xmlns="http://www.w3.org/2000/svg" width="48" height="48" viewBox="0 0 24 24" fill="none" stroke="currentColor" stroke-width="2" stroke-linecap="round" stroke-linejoin="round"><circle cx="12" cy="12" r="10" /> <path d="m4.93 4.93 4.24 4.24" /> <path d="m14.83 9.17 4.24-4.24" /> <path d="m14.83 14.83 4.24 4.24" /> <path d="m9.17 14.83-4.24 4.24" /> <circle cx="12" cy="12" r="4" /></svg>'
    title: Graceful by contract
    details: additive-only protocol, unknown kinds degrade to KV, plugin failure never breaks a view
    link: /explanation/why-declarative
    linkText: why declarative
---

pavo is the org's **surface** — the visualization host every engine
drivers into. Products ship `tamias-driver-*` executables emitting
a declarative ViewNode IR; pavo interprets the same tree on CLI,
TUI, and web. It owns visualization hosting and nothing else:
product state stays with the products, distribution with tamias,
policy with peregrin.

## Which quadrant answers which goal

- **Learn it** → [Getting started](tutorials/getting-started.md) —
  install, first view, the wire
- **Do a task** → [How-to](/how-to/install-a-driver) — install a
  driver, write a driver, renderer plugins, serve web, config API,
  deep links
- **Understand why** → [Architecture](/explanation/architecture),
  [concepts](/explanation/concepts),
  [decisions](/explanation/decisions),
  [why declarative](/explanation/why-declarative),
  [boundaries](/explanation/boundaries)
- **Look something up** → [CLI](/reference/cli),
  [ViewNode IR](/reference/view-nodes),
  [wire ops](/reference/wire-ops),
  [driver manifest](/reference/driver-manifest)
