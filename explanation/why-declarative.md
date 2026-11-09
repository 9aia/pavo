---
url: /pavo/explanation/why-declarative.md
---
# Why declarative views

Pavo could have let every product ship its own TUI, its own web
app, its own CLI flags. Instead drivers emit a **declarative**
tree — data, not code — and hosts interpret it. The reasons:

## One driver, every surface

A `ViewNode` tree is surface-agnostic JSON. The same `myco:board`
spec renders as an aligned table on the CLI, ratatui widgets in the
TUI, and DOM in the web app — with zero per-surface driver code.
When a fifth surface appears (a chat bridge, an editor panel), it
interprets the same IR.

## Drivers stay dumb and safe

A driver is a separate process speaking a narrow wire. It can't
inject HTML, run host code, or touch renderer internals — the worst
a buggy driver can do is emit a malformed tree, which degrades to
KV. That boundary is what lets pavo host *untrusted-ish* product
drivers discovered on `PATH`.

## Degradation is a feature, not a fallback

The protocol is **additive-only within v1**: new optional fields,
new `kind` variants, new ops may appear at any time; existing
spellings never change meaning. Every consumer obligation follows:

* Renderers meeting an unknown `kind` degrade to generic KV.
* `props` is an open map — renderers ignore keys they don't know.
* `contributes.extra` round-trips unknown chrome slots.
* Plugin `render` failures fall back to built-in interpretation.

A driver written today keeps working as the IR grows; a renderer
written today keeps working as drivers grow. Forward compatibility
is mandatory, not optional — it's what makes "install a driver,
get visualization everywhere" true across versions.

## Data binding, not data copying

Nodes don't embed data — they name a `query` binding, and the host
resolves bindings to view-model data before rendering (cached,
with partial degradation per binding). Drivers declare *what* data
a view needs; hosts decide *when* to fetch it. That split is what
lets `pavo view --param` and deep-link params flow through one
mechanism, and lets the web surface hydrate panels independently.

## Escape hatches where they belong

Declarative doesn't mean lowest-common-denominator. `embed` names a
component for rich visuals; `renderers[]` + the `render` op let a
driver claim a whole kind and answer with a canonical
`RenderModel`; `graph`'s IR floor covers structure while layout is
explicitly the renderer's problem. The IR covers the 90%; plugins
cover the 10% — and fallback is mandatory so plugins can fail
softly. See [Renderer plugins](/explanation/renderer-plugins).

## The cost

Honest trade-offs: the IR can't express everything (rich
interaction beyond single-form submit — buttons, wizards — is v2),
renderer plugins add a second contract to keep
honest, and every driver pays the manifest+wire ceremony even for a
trivial view. The bet is that uniform surfaces + safe degradation
outweigh per-product UI silos — the same bet the org's other tools
(sturn's WAL, tamias's ref grammar, cuora's refs) already make.
