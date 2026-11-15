---
layout: home

hero:
  name: pavo
  text: visualization host for the agent factory
  tagline: tamias drivers emit declarative ViewNode IR — pavo renders it on CLI, TUI, and web
  actions:
    - theme: brand
      text: Getting Started
      link: /getting-started
    - theme: alt
      text: Architecture
      link: /explanation/architecture

features:
  - icon: 🦚
    title: One host, every surface
    details: drivers ship views once; pavo interprets the same ViewNode IR on CLI, TUI (ratatui), and web (Nuxt)
    link: /explanation/architecture
    linkText: architecture
  - icon: 🧩
    title: Drivers, not plugins
    details: a view provider is a tamias-driver-* executable speaking an ndjson JSON-RPC wire — discovered on PATH or in the tamias drivers dir
    link: /how-to/write-a-driver
    linkText: write a driver
  - icon: 🎨
    title: Renderer plugins
    details: drivers can claim view kinds outright — a manifest renderers[] declaration plus the render op returning a canonical RenderModel
    link: /explanation/renderer-plugins
    linkText: renderer plugins
  - icon: 🖥️
    title: Platform chrome
    details: eight contribution slots, driver deps, cross-driver queries, and aggregated jobs/notifications feeds compose one shared screen
    link: /explanation/architecture#platform-host
    linkText: platform host
  - icon: 🛟
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

- **Learn it** → [Tutorial: your first view](tutorials/first-view.md)
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
