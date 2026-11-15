# Contributing

Issues and PRs are **disabled** — the project is agent-maintained and
dogfoods its own PM ledger (`project/`). Fork freely under MIT.

## Layout

`crates/` — flat Rust workspace, ports-and-adapters.
`pavo-protocol` is the public wire contract (pure serde, no I/O);
`pavo-core` is host logic; `pavo-cli`/`pavo-tui` are the surfaces;
`pavo-driver-*` are first-party drivers; `pavo-testkit` is the
conformance harness. `web/` is the Nuxt host (npm), `protocol/` the
versioned spec + JSON Schema mirrors, `xtask/` repo chores. See the
crate table in [AGENTS.md](AGENTS.md#layout).

## Rules of thumb

- `pavo-protocol` changes move Rust types, `protocol/v1/*.schema.json`
  mirrors, and docs together — wire compat is additive-only within v1.
- New **pure logic** → tested in its crate; new **I/O** stays at the
  edges. No `unsafe`.
- `cargo xtask check` and `cargo xtask test` must stay green;
  `./scripts/smoke-examples.sh` exercises `examples/drivers/*`
  end-to-end.
- Web: `cd web && npm run dev` / `npm run build` — keep the static
  export path the `pavo serve` bridge auto-detects working.
- Commits: conventional prefixes (`feat:`/`fix:`/`refactor:`/`docs:`/`chore:`).
