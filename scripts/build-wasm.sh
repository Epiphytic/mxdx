#!/usr/bin/env bash
set -euo pipefail

# Build WASM for both Node.js and browser targets.
# Usage: ./scripts/build-wasm.sh [--dev]

DEV_FLAG=""
if [[ "${1:-}" == "--dev" ]]; then
  DEV_FLAG="--dev"
fi

echo "==> Building WASM (Node.js target)..."
wasm-pack build crates/mxdx-core-wasm --target nodejs --out-dir ../../packages/core/wasm $DEV_FLAG

echo "==> Building WASM (browser target)..."
wasm-pack build crates/mxdx-core-wasm --target web --out-dir ../../packages/web-console/wasm $DEV_FLAG

echo "==> Done."
