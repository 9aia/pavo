# pavo

*Visualization host for the 9aia org's tool suite — the factory's
screen.*

Named for *Pavo* — the peafowl genus; the display animal. Products
ship **tamias drivers** (`tamias-driver-<type>` on `PATH` or in
`<tamias-home>/drivers/`) that emit declarative **ViewNode IR**
JSON trees; pavo's renderers — CLI, TUI, web — interpret those
trees natively. **Renderer plugins** let a driver claim view kinds
outright via a manifest `renderers[]` declaration + the `render`
wire op; built-in rendering stays the fallback for every unclaimed
kind. Install a driver → get visualization everywhere — there is
no `<product> ui` per product; pavo is the single entry point.

## Quickstart

```sh
cargo xtask install    # pavo + tamias-driver-config → ~/.cargo/bin

export PATH="$PWD/examples/drivers/demo:$PWD/examples/drivers/viz-graph:$PATH"
pavo drivers                          # discovered drivers
pavo views                            # every <driver>:<view> ref
pavo view demo:deps --format mermaid  # built-in render
pavo view demo:deps --format svg      # renderer-plugin output
pavo serve --port 7420                # localhost bridge + web host
cd web && npm run dev                 # Nuxt dev server → :3000
```

`./scripts/smoke-examples.sh` runs the example drivers end-to-end.
Real product drivers (`tamias-driver-myco`, `-kuda`, `-cuora`, …)
ship from their own repos and are discovered the same way.

## Layout

Rust + TypeScript monorepo — `crates/` (Rust workspace), `web/`
(Nuxt 4), `protocol/` (versioned wire contracts, additive-only
within v1). `pavo-protocol` is the public wire contract;
`pavo-driver-kit` is the driver-authoring SDK; `pavo-testkit` is
the conformance harness. Full crate table, commands, and boundary:
[AGENTS.md](AGENTS.md).

## Docs

- [Docs site](https://9aia.com/pavo) — tutorials, how-tos,
  reference, explanation (VitePress + Diátaxis, `docs/` worktree)
- [web/README.md](web/README.md) — the Nuxt host workflow
- [protocol/v1/README.md](protocol/v1/README.md) — wire spec +
  driver-authoring guide
- [CONTRIBUTING.md](CONTRIBUTING.md) · [SECURITY.md](SECURITY.md)
- [LICENSE](LICENSE) · [project ledger](project/index.md)
