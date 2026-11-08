# Pavo

Visualization **host** for the 9aia org's tool suite — the factory's
screen. Named for *Pavo* — the peafowl genus; the display animal.
Rust + TypeScript monorepo: `crates/` (Rust workspace), `web/` (Nuxt),
`protocol/` (versioned wire contracts). Pavo discovers **tamias
drivers** — executables named `tamias-driver-<type>` on `PATH` or in
`<tamias-home>/drivers/` (`$TAMIAS_HOME` or
`~/.local/share/tamias/drivers`) — and renders the declarative
ViewNode IR they emit on three surfaces: CLI, TUI, web. Drivers are
the only plugin mechanism — and they can also be **renderer plugins**:
a manifest `renderers[]` declaration claims view *kinds* and answers
the `render` wire op with a canonical `RenderModel` (see
`protocol/v1/README.md` → Renderer plugins). Built-in rendering stays
as the fallback for every unclaimed kind/surface.

<!-- myco:agent-awareness -->

## Agent awareness

- Session start / any work discussion: run `myco status --prompt`
  before proposing next steps — know the board first.
- User describes new work → offer to capture it (`myco todo add`,
  `issue new`, `proposal new`). Don't wait to be asked.
- Picking up a task: `myco resume <ID>` is the briefing.
- Wrapping up: `myco handoff <ID>` anything resumable; keep statuses
  honest (claimed → in-progress → blocked/done).

<!-- /myco:agent-awareness -->

<!-- sturn:agent-awareness -->

## Sturn awareness

- **No PRs.** The pushed branch/commit is the deliverable; review
  streams continuously (sturn WAL), merge is a human/sturn call.
- sturn-wired repo (`sturn.toml` present): `sturn claim "<glob>"
  --ws <ws>` *before* edits — claims surface conflicts in ms.
- Ops via `sturn write|rename|delete`; `sturn snap` folds the ws
  into a real commit; `sturn push` publishes heads + refs/sturn/*.
- `sturn leave` on exit — drops the actor's claims.

<!-- /sturn:agent-awareness -->

<!-- kuda:agent-awareness -->

## Kuda awareness

- `.kuda/` cards are committed artifacts — read them before
  re-exploring (`kuda map`, `kuda card <mod>`); `kuda status`
  reports per-card drift vs HEAD.
- Code changes that invalidate a card → regenerate it (`kuda
  sync`) in the same change; cards ride the diff they describe.
- `.kuda/index.sqlite` is gitignored + rebuildable; the `*.yaml`
  cards are the checked-in source of truth.

<!-- /kuda:agent-awareness -->

## Layout

```
crates/    pavo-protocol · pavo-core · pavo-cli · pavo-tui · pavo-testkit
web/       Nuxt SSR host + serve bridge (stdio↔ws) — scaffold stage
protocol/  driver capability spec · ViewNode IR · wire ops (versioned)
xtask      check / test / coverage / install (myco anatomy)
```

| Crate | Role |
| --- | --- |
| `pavo-protocol` | the public wire contract — driver manifests, ViewNode IR, ndjson ops. Pure serde, no I/O |
| `pavo-core` | host core — tamias driver discovery, contribution merge, per-driver clients, view-model cache |
| `pavo-cli` | the `pavo` binary — hub verbs, `--json` everywhere; protocol conformance harness |
| `pavo-tui` | ratatui host — chrome slots + ViewNode renderer |
| `pavo-testkit` | recorded driver sessions → conformance for all surfaces |

`protocol/v1/*.schema.json` are hand-maintained mirrors of
`pavo-protocol`; `protocol/v1/README.md` states the versioning
policy (**additive-only within v1** — keep them in sync).

## Boundary

Pavo owns **visualization hosting** — driver discovery, ViewNode
interpretation, contribution-slot merge, and the three renderers.
Neighbors own theirs:

- **tamias** — distribution; drivers are tamias content packages
  found via `<tamias-home>/drivers/`. Pavo never re-implements
  transport or content types.
- **products** (myco, kuda, sturn, mimus, …) — state owners; they
  ship drivers that proxy their existing `serve` wires. Pavo never
  reads product internals (myco files, kuda sqlite, tamias store
  schemas).
- **cuora** — secrets; driver credentials via `cuora://` refs.
- **rampho** — the `SceneNode → ViewNode` precedent; media render
  pipelines are rampho's, UI trees are pavo's.

## Commands

```bash
cargo xtask check          # fmt + clippy -D warnings
cargo xtask test           # whole suite (nextest if present)
cargo xtask coverage       # llvm-cov report (HTML → target/llvm-cov/html)
cargo xtask install        # release-install pavo → ~/.cargo/bin
./scripts/smoke-examples.sh  # run examples/drivers/* end-to-end via pavo
```

Git hooks live in `.hooks/` when present (`core.hooksPath` =
`.hooks`): pre-commit `cargo fmt --check`, pre-push `cargo xtask
check`. Scaffold stage — hooks not yet installed.

## Rules

- Workspace lints are shared (`Cargo.toml [workspace.lints]`):
  `unsafe_code` forbidden, `unwrap_used` warned — `-D warnings` in
  CI means no warnings tolerated. Tests `#![allow]` liberally.
- No `unsafe`. Edition 2021.
- **`pavo-protocol` is the public API.** Wire types are
  serde-serializable, forward-compatible (unknown fields ignored,
  unknown `kind`s degrade to KV), and mirrored by
  `protocol/v1/*.schema.json` — change Rust types, schemas, and
  this doc together.
- **ViewNode IR is declarative.** Drivers emit trees + view models;
  hosts interpret — or hand the node to a renderer plugin (a driver
  declaring `renderers[]` + answering `render` with a `RenderModel`).
  `embed` remains the escape hatch for rich components no model
  covers.
- **`--json` everywhere** on the CLI; exit codes CI-meaningful.
- Agents never hand-edit `project/` when a `myco` verb exists.
- `.agents/skills/` is the canonical skill location; `skills-lock.json`
  pins upstream skills.

## Docs

`docs/` worktree (VitePress → GitHub Pages) is planned, not yet
created. Module doc comments (`//!`) in `pavo-protocol` are the
reference seed — keep them accurate; generated docs will feed from
them.
