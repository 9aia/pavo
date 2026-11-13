#!/usr/bin/env bash
# pavo release — thin shim over tarsi-serve (the org release plane,
# project ADR-0005; repo config: ./tarsi.toml). One call runs
# plan → changelog → bump → tag → push; publish targets are off
# (ADR-0004: crates.io is tamias-only, GitHub Releases unused).
# Operator-run on main — `tarsi-serve release` is dry-run by
# default; --yes is the explicit opt-in.
#
# No tamias pack: pavo distributes via `cargo xtask install`
# (release-build → ~/.cargo/bin) and git deps — the tag is the
# release artifact.
set -euo pipefail
cd "$(git rev-parse --show-toplevel)"
# worktree-local env (gitignored); the service env file is canonical
[ -f ./.env ] && { set -a; source ./.env; set +a; }

tarsi-serve release --dir . --yes

echo "released: see per-unit tags pavo-*-v<ver>"
echo "next: cargo xtask install  # or depend on the new tag via git deps"
