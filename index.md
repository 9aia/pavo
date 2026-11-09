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
