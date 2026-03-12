#!/bin/bash
set -euo pipefail

# Publish Rust crates in dependency order
# Waits between publishes for crates.io index to update

CRATES=(
  mxdx-types
  mxdx-matrix
  mxdx-policy
  mxdx-secrets
  mxdx-launcher
  mxdx-web
  mxdx-core-wasm
)

for crate in "${CRATES[@]}"; do
  echo "Publishing $crate..."
  cargo publish -p "$crate" --no-verify
  echo "Waiting for crates.io index update..."
  sleep 30
done

echo "All crates published."
