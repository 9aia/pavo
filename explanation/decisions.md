# Decisions

ADR-lite — the decisions that shape pavo, the alternatives
rejected, and the consequence each carries.

## Declarative IR, not per-product UIs

**Decision:** drivers emit a ViewNode tree — data, not code — and
hosts interpret it on every surface.

**Alternatives:** each product ships its own TUI/web/CLI (UI silos,
N surfaces × M products of code); embed a scripting language in
drivers (untrusted-ish code on PATH becomes a host exploit).

**Consequence:** one driver visualizes everywhere; the worst a
buggy driver can do is emit a malformed tree, which degrades to
KV. The cost: the IR can't express everything (rich interaction
is v2) and drivers pay manifest+wire ceremony even for trivial
views. See [Why declarative views](why-declarative.md).

## Additive-only protocol within v1

**Decision:** new optional fields, `kind` variants, and ops may
appear at any time; existing spellings never change meaning.
Forward compatibility is mandatory.

**Alternatives:** versioned protocol revisions (`/v2` endpoints —
every consumer must track versions); breaking changes allowed
(surfaces and drivers drift apart on upgrade).

**Consequence:** a driver written today keeps working as the IR
grows; every consumer obligation (unknown kind → KV, open `props`,
`extra` round-trip) follows from this one rule.

## Drivers are executables, not plugins

**Decision:** a view provider is a `tamias-driver-*` binary on
`PATH` speaking ndjson JSON-RPC — the tamias distribution
mechanism, reused.

**Alternatives:** in-process plugin API (ABI/version coupling,
host crashes); per-product UIs again (see above); a daemon
registry (pavo stays a per-user localhost bridge, no service).

**Consequence:** installing visualization for a product is
`tamias install` — no pavo-side registry, no plugin sandbox.
`pavo-driver-kit` exists as an authoring convenience but is not a
second contract.

## Renderer plugins are opt-in claims, with mandatory fallback

**Decision:** a driver may claim whole view `kind`s via manifest
`renderers[]` and answer `render` with a canonical RenderModel —
but failure always falls back to built-in interpretation.

**Alternatives:** no escape hatch (the 90% IR becomes a ceiling);
mandatory plugins for rich kinds (one bad plugin breaks every
view of that kind).

**Consequence:** rich visuals (`svg`, custom renderers) exist
without letting plugins become a reliability liability — the
second contract is kept honest by construction.

## Chrome is contribution slots, not a shell API

**Decision:** products compose the shared screen by filling eight
declared `contributes` slots; the host merges deterministically
(priority → driver name → id).

**Alternatives:** a plugin shell API where drivers draw arbitrary
chrome (unbounded, unthemed); per-product pages (back to silos).

**Consequence:** the platform emerges from independent drivers —
ordering is stable, unknown slots round-trip via `extra`, and the
web shell renders slots it knows while ignoring the rest.
