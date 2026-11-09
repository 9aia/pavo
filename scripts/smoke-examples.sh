#!/usr/bin/env bash
# smoke-examples.sh — run the example drivers end-to-end through pavo.
#
# Puts the built tamias-driver-config (target/debug) plus
# examples/drivers/{demo,myco,platform-demo} on PATH (the same discovery
# mechanism a real install uses) and exercises the main verbs. Works
# from anywhere; builds pavo first if needed.
#
#   ./scripts/smoke-examples.sh
#
# Env overrides: PAVO=/path/to/pavo, TAMIAS_HOME=/path/to/home.
set -euo pipefail
cd "$(dirname "$0")/.."
ROOT=$(pwd)

PAVO_BIN="${PAVO:-$ROOT/target/debug/pavo}"
if [ ! -x "$PAVO_BIN" ]; then
    cargo build -p pavo
fi

cargo build -p pavo-driver-config
export PATH="$ROOT/target/debug:$ROOT/examples/drivers/demo:$ROOT/examples/drivers/myco:$ROOT/examples/drivers/platform-demo:$PATH"
export TAMIAS_HOME="${TAMIAS_HOME:-/tmp/pavo-smoke-home}"
mkdir -p "$TAMIAS_HOME/drivers"

echo "== pavo drivers =="
"$PAVO_BIN" drivers
echo
echo "== pavo views =="
"$PAVO_BIN" views
echo
echo "== pavo view demo:board =="
"$PAVO_BIN" view demo:board
echo
echo "== pavo view demo:deps --format mermaid =="
"$PAVO_BIN" view demo:deps --format mermaid
echo
echo "== pavo view demo:timeline =="
"$PAVO_BIN" view demo:timeline
echo
echo "== pavo view config:show =="
"$PAVO_BIN" view config:show
echo
echo "== pavo view myco:board (degraded ok if myco absent) =="
"$PAVO_BIN" view myco:board || true
echo
echo "== pavo view platform:board (cross-driver binding via config) =="
"$PAVO_BIN" view platform:board
echo
echo "== pavo jobs / notifications (platform feeds) =="
"$PAVO_BIN" jobs
"$PAVO_BIN" notifications
echo
echo "smoke-examples: OK"
