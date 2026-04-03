#!/bin/bash
set -euo pipefail

# Unified E2E test suite orchestrator.
# Runs Rust and npm E2E tests against beta infrastructure.
#
# Usage: ./scripts/e2e-test-suite.sh --profile release|debug [--skip-build]

PROFILE="release"
SKIP_BUILD=false

while [[ $# -gt 0 ]]; do
  case $1 in
    --profile) PROFILE="$2"; shift 2 ;;
    --skip-build) SKIP_BUILD=true; shift ;;
    *) echo "Unknown option: $1"; exit 1 ;;
  esac
done

if [[ "$PROFILE" != "release" && "$PROFILE" != "debug" ]]; then
  echo "Error: --profile must be 'release' or 'debug'"
  exit 1
fi

echo "=== E2E Test Suite (profile: $PROFILE, skip-build: $SKIP_BUILD) ==="

# ── Build ──────────────────────────────────────────────────────────────

if [[ "$SKIP_BUILD" == "false" ]]; then
  echo "--- Building Rust binaries (${PROFILE}) ---"
  if [[ "$PROFILE" == "release" ]]; then
    cargo build --release -p mxdx-worker -p mxdx-client
  else
    cargo build -p mxdx-worker -p mxdx-client
  fi

  echo "--- Building WASM (nodejs) ---"
  wasm-pack build crates/mxdx-core-wasm --target nodejs --out-dir ../../packages/core/wasm/nodejs

  echo "--- Building WASM (web) ---"
  wasm-pack build crates/mxdx-core-wasm --target web --out-dir ../../packages/core/wasm/web

  echo "--- Cleaning wasm-pack artifacts ---"
  rm -f packages/core/wasm/nodejs/.gitignore packages/core/wasm/nodejs/package.json \
        packages/core/wasm/web/.gitignore packages/core/wasm/web/package.json

  echo "--- Installing npm dependencies ---"
  npm install
fi

# ── Set binary directory ───────────────────────────────────────────────

if [[ "$PROFILE" == "release" ]]; then
  export MXDX_BIN_DIR="$(pwd)/target/release"
else
  export MXDX_BIN_DIR="$(pwd)/target/debug"
fi

echo "MXDX_BIN_DIR=${MXDX_BIN_DIR}"

# ── Create test-results directory ──────────────────────────────────────

mkdir -p test-results
GIT_SHA=$(git rev-parse --short HEAD 2>/dev/null || echo "unknown")
TIMESTAMP=$(date -u +"%Y-%m-%dT%H:%M:%SZ")

# ── Rust E2E Tests ─────────────────────────────────────────────────────

echo ""
echo "=== Rust E2E Tests ==="

export TEST_PERF_OUTPUT="$(pwd)/test-results/rust-e2e-perf-entries.jsonl"
rm -f "$TEST_PERF_OUTPUT"

# Run tests with --nocapture for stderr progress
RUST_TEST_EXIT=0
cargo test -p mxdx-worker --test e2e_profile -- --ignored --test-threads=1 --nocapture 2>&1 | tee test-results/rust-e2e-raw.txt || RUST_TEST_EXIT=${PIPESTATUS[0]}

# Generate JUnit XML if cargo2junit is available
if command -v cargo2junit &>/dev/null; then
  echo "--- Generating Rust JUnit XML ---"
  cargo test -p mxdx-worker --test e2e_profile -- --ignored --test-threads=1 -Z unstable-options --format json 2>/dev/null \
    | cargo2junit > test-results/rust-e2e-junit.xml 2>/dev/null || true
else
  echo "cargo2junit not found — skipping JUnit XML generation"
  echo "Install with: cargo install cargo2junit"
fi

# Wrap perf entries in suite metadata
if [[ -f "$TEST_PERF_OUTPUT" ]]; then
  echo "--- Wrapping Rust perf entries ---"
  ENTRIES=$(cat "$TEST_PERF_OUTPUT")
  cat > test-results/rust-e2e-perf.json <<PERF_EOF
{
  "suite": "rust-e2e",
  "profile": "${PROFILE}",
  "timestamp": "${TIMESTAMP}",
  "git_sha": "${GIT_SHA}",
  "tests": [
$(echo "$ENTRIES" | sed 's/^/    /' | paste -sd ',' - | sed 's/,/,\n/g')
  ]
}
PERF_EOF
  rm -f "$TEST_PERF_OUTPUT"
fi

if [[ $RUST_TEST_EXIT -ne 0 ]]; then
  echo "!!! Rust E2E tests FAILED (exit code: $RUST_TEST_EXIT) !!!"
  echo "Skipping npm tests and account purge (preserving state for debugging)."
  exit $RUST_TEST_EXIT
fi

# ── npm E2E Tests ──────────────────────────────────────────────────────

echo ""
echo "=== npm E2E Tests ==="

export TEST_PERF_OUTPUT="$(pwd)/test-results/npm-e2e-perf-entries.jsonl"
rm -f "$TEST_PERF_OUTPUT"

NPM_TEST_EXIT=0
node --test \
  --test-reporter=spec --test-reporter-destination=stdout \
  --test-reporter=junit --test-reporter-destination=test-results/npm-e2e-junit.xml \
  packages/e2e-tests/tests/public-server.test.js \
  || NPM_TEST_EXIT=$?

# Wrap npm perf entries in suite metadata
if [[ -f "$TEST_PERF_OUTPUT" ]]; then
  echo "--- Wrapping npm perf entries ---"
  ENTRIES=$(cat "$TEST_PERF_OUTPUT")
  cat > test-results/npm-e2e-perf.json <<PERF_EOF
{
  "suite": "npm-e2e",
  "profile": "${PROFILE}",
  "timestamp": "${TIMESTAMP}",
  "git_sha": "${GIT_SHA}",
  "tests": [
$(echo "$ENTRIES" | sed 's/^/    /' | paste -sd ',' - | sed 's/,/,\n/g')
  ]
}
PERF_EOF
  rm -f "$TEST_PERF_OUTPUT"
fi

if [[ $NPM_TEST_EXIT -ne 0 ]]; then
  echo "!!! npm E2E tests FAILED (exit code: $NPM_TEST_EXIT) !!!"
  echo "Skipping account purge (preserving state for debugging)."
  exit $NPM_TEST_EXIT
fi

# ── Account Purge (success only) ──────────────────────────────────────

echo ""
echo "=== Purging test accounts ==="
if [[ -f scripts/purge-test-accounts.mjs ]]; then
  node scripts/purge-test-accounts.mjs
else
  echo "scripts/purge-test-accounts.mjs not found — skipping purge"
fi

echo ""
echo "=== All E2E tests PASSED ==="
echo "Results in test-results/:"
ls -la test-results/
