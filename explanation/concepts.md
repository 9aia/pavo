---
url: /pavo/explanation/concepts.md
---
# Concepts

pavo's vocabulary is the wire contract made human: a **driver**
ships **views**, a **host** interprets them, and everything in
between is a **node** in a declarative tree.

## Wire entities

* **Driver** — a `tamias-driver-<type>` executable discovered on
  `PATH` or in `<tamias-home>/drivers/`. Speaks ndjson JSON-RPC
  over stdio; declares a manifest (routes, views, chrome
  contributions, deps) and answers ops (`view`, `query`,
  `submit`, `render`, `jobs`, `notifications`).
* **ViewNode IR** — the declarative tree drivers emit:
  `panel`, `table`, `tree`, `graph`, `form`, `kv`, `markdown`,
  `embed`, `row`, `column`, `svg`, `placeholder`. Data is *not*
  embedded — nodes name `query` bindings the host resolves.
* **QueryBinding** — `{op, params?, driver?}`: which wire op
  fetches a node's data. `driver` set = cross-driver query into
  another driver's `serve`.
* **RenderModel** — the canonical output of a renderer plugin's
  `render` op: resolved nodes + view-model data, surface-agnostic.
* **Manifest `contributes`** — eight chrome slots (`navbar`,
  `dashboard`, `statusbar`, `routes`, `sidebar`,
  `headerActions`, `notifications`, `jobs`) a driver fills to
  compose the shared screen.
* **`deps`** — `requires`/`optional` driver-name graph; the host
  topologically sorts and quarantines offenders.

## Host entities

* **View ref** — `<driver>:<view>`; on the CLI an argument, on the
  web a `/view/...` URL — the same link everywhere.
* **View-model cache** — host-side TTL cache of resolved binding
  data; partial degradation per binding.
* **Renderer plugin** — a driver that claims view `kind`s via
  manifest `renderers[]` and answers `render` with a
  RenderModel; failure falls back to built-in interpretation.
* **Surfaces** — the three interpreters of the same IR: CLI
  (text), TUI (ratatui), web (Nuxt via `pavo serve` bridge).

## Invariants

* **Additive-only v1** — new optional fields, kinds, ops may
  appear anytime; existing spellings never change meaning.
* **Unknown kinds degrade** — renderers meet an unknown `kind` as
  generic KV; `props` is an open map.
* **Drivers stay proxies** — pavo hosts collect `form` values and
  display `SubmitResult`; every mutation semantic belongs to the
  driver.
