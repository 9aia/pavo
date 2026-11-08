#!/usr/bin/env bash
# Build the VitePress site and publish .vitepress/dist to the `gh-pages`
# branch. Runs identically by hand (from the docs/ worktree); PAVO_REPO
# points cargo work at the real checkout when cwd is a detached docs
# worktree.
set -euo pipefail

cd "$(dirname "${BASH_SOURCE[0]}")/.."
DOCS="$(pwd)"
REPO="${PAVO_REPO:-$HOME/GitHub/9aia/pavo}"
export PAVO_ROOT="$REPO"

echo "==> bun install"
bun install

echo "==> gen crate map (PAVO_ROOT=$REPO)"
python3 gen-crate-map.py

echo "==> rustdoc → public/api (cargo doc in $REPO)"
python3 gen-api.py

echo "==> vitepress build"
bunx vitepress build

# Branch deploys still get a Jekyll pass unless .nojekyll is present.
touch .vitepress/dist/.nojekyll

echo "==> deploy .vitepress/dist → gh-pages"
bunx gh-pages -d .vitepress/dist -b gh-pages \
  -r git@github.com:9aia/pavo.git \
  -m "docs: deploy $(git rev-parse --short HEAD)"

echo "done — https://9aia.com/pavo/ serves origin/gh-pages"
