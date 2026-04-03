# E2E Test Infrastructure Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** Unified E2E test infrastructure that gates PRs and releases, tests release binaries (Rust + npm/WASM), and produces performance reports for regression tracking.

**Architecture:** A single shell script (`scripts/e2e-test-suite.sh`) orchestrates Rust and npm E2E suites against beta infrastructure. The Rust test harness gains `MXDX_BIN_DIR` for binary resolution and `TEST_PERF_OUTPUT` for performance JSON. CI adds an `e2e-release` job on PRs; the release pipeline splits into `e2e-gate` → `release` with binary publishing.

**Tech Stack:** Bash, Rust (cargo test, cargo2junit), Node 22 (built-in test runner + JUnit reporter), GitHub Actions, dorny/test-reporter, semantic-release

**Spec:** `docs/superpowers/specs/2026-04-03-e2e-test-infrastructure-design.md`

---

## File Structure

| File | Responsibility | Change |
|---|---|---|
| `crates/mxdx-worker/tests/e2e_profile.rs` | Rust E2E test harness | Modify: `cargo_bin()` env override, `report()` perf JSON |
| `scripts/e2e-test-suite.sh` | Unified E2E orchestrator | **Create** |
| `packages/e2e-tests/tests/public-server.test.js` | npm E2E tests against beta | Modify: add perf JSON output |
| `packages/e2e-tests/tests/public-session-persistence.test.js` | npm session persistence tests | Modify: add perf JSON output |
| `.github/workflows/ci.yml` | CI pipeline | Modify: add `e2e-release` job |
| `.github/workflows/release.yml` | Release pipeline | Modify: split into `e2e-gate` + `release` |
| `scripts/publish-crates.sh` | Crate publishing | Modify: add binary crates |
| `.releaserc.json` | semantic-release config | Modify: add GitHub Release assets |

---

### Task 1: Add `MXDX_BIN_DIR` Override to `cargo_bin()`

**Files:**
- Modify: `crates/mxdx-worker/tests/e2e_profile.rs:77-84`

- [ ] **Step 1: Modify `cargo_bin()` to check `MXDX_BIN_DIR` env var first**

In `crates/mxdx-worker/tests/e2e_profile.rs`, replace the `cargo_bin` function (lines 77-84):

```rust
fn cargo_bin(name: &str) -> std::path::PathBuf {
    // Allow override via MXDX_BIN_DIR for testing release-profile binaries
    if let Ok(dir) = std::env::var("MXDX_BIN_DIR") {
        let path = std::path::PathBuf::from(dir).join(name);
        assert!(path.exists(), "Binary not found at {} (via MXDX_BIN_DIR)", path.display());
        return path;
    }
    // Default: resolve relative to test binary (target/debug/)
    let mut path = std::env::current_exe().expect("cannot resolve test binary path");
    path.pop();
    path.pop();
    path.push(name);
    assert!(path.exists(), "Binary not found at {}", path.display());
    path
}
```

- [ ] **Step 2: Verify existing tests still compile and pass**

Run:
```bash
cargo test -p mxdx-worker --test e2e_profile -- --list
```
Expected: All test names listed (they won't run since they're `#[ignore]`). No compile errors.

- [ ] **Step 3: Commit**

```bash
git add crates/mxdx-worker/tests/e2e_profile.rs
git commit -m "feat(e2e): add MXDX_BIN_DIR env var override to cargo_bin()"
```

---

### Task 2: Add Performance JSON Output to `report()`

**Files:**
- Modify: `crates/mxdx-worker/tests/e2e_profile.rs:240-247`
- Modify: `crates/mxdx-worker/Cargo.toml` (dev-dependency)

- [ ] **Step 1: Add `serde_json` dev-dependency to mxdx-worker**

In `crates/mxdx-worker/Cargo.toml`, add to `[dev-dependencies]`:

```toml
serde_json = { workspace = true }
```

Check that `serde_json` is already in the workspace `Cargo.toml` dependencies (it is — used by many crates).

- [ ] **Step 2: Add `use` import for serde_json at top of e2e_profile.rs**

At the top of `crates/mxdx-worker/tests/e2e_profile.rs`, after the existing `use` statements (after line 18), add:

```rust
use std::path::PathBuf;
```

(`serde_json` will be used inline via `serde_json::json!` macro.)

- [ ] **Step 3: Replace `report()` function with perf JSON support**

Replace the `report` function (lines 240-247) in `crates/mxdx-worker/tests/e2e_profile.rs`:

```rust
fn report(test: &str, transport: &str, elapsed: Duration, exit_code: Option<i32>, stdout_lines: usize) {
    eprintln!(
        "| {:<30} | {:<12} | {:>8.1}s | {:>4} | {:>8} |",
        test, transport, elapsed.as_secs_f64(),
        exit_code.map(|c| c.to_string()).unwrap_or("?".into()),
        stdout_lines,
    );

    // Write performance JSON entry if TEST_PERF_OUTPUT is set
    if let Ok(path) = std::env::var("TEST_PERF_OUTPUT") {
        let status = match exit_code {
            Some(0) => "pass",
            Some(_) => "fail",
            None => "fail",
        };
        let entry = serde_json::json!({
            "name": test,
            "transport": transport,
            "duration_ms": elapsed.as_millis() as u64,
            "exit_code": exit_code,
            "stdout_lines": stdout_lines,
            "status": status,
        });

        // Append JSON line to file (one JSON object per line, wrapped by e2e-test-suite.sh)
        use std::io::Write;
        let mut file = std::fs::OpenOptions::new()
            .create(true)
            .append(true)
            .open(&path)
            .expect("failed to open TEST_PERF_OUTPUT file");
        writeln!(file, "{}", entry).expect("failed to write perf entry");
    }
}
```

- [ ] **Step 4: Verify compilation**

Run:
```bash
cargo test -p mxdx-worker --test e2e_profile -- --list
```
Expected: All test names listed, no compile errors.

- [ ] **Step 5: Commit**

```bash
git add crates/mxdx-worker/tests/e2e_profile.rs crates/mxdx-worker/Cargo.toml
git commit -m "feat(e2e): add performance JSON output to report() via TEST_PERF_OUTPUT"
```

---

### Task 3: Add Performance JSON Output to npm E2E Tests

**Files:**
- Modify: `packages/e2e-tests/tests/public-server.test.js`
- Modify: `packages/e2e-tests/tests/public-session-persistence.test.js`

- [ ] **Step 1: Add `writePerfEntry` helper to public-server.test.js**

In `packages/e2e-tests/tests/public-server.test.js`, after the `FIXED_LAUNCHER_ID` constant (after line 41), add:

```javascript
/**
 * Write a performance JSON entry to TEST_PERF_OUTPUT (if set).
 * One JSON object per line — the e2e-test-suite.sh wraps them with suite metadata.
 */
function writePerfEntry(name, transport, durationMs, exitCode, stdoutLines) {
  const perfPath = process.env.TEST_PERF_OUTPUT;
  if (!perfPath) return;
  const entry = JSON.stringify({
    name,
    transport,
    duration_ms: durationMs,
    exit_code: exitCode,
    stdout_lines: stdoutLines,
    status: exitCode === 0 ? 'pass' : 'fail',
  });
  fs.appendFileSync(perfPath, entry + '\n');
}
```

- [ ] **Step 2: Add perf entry to the round-trip test in public-server.test.js**

In the round-trip test (the `it('launcher starts and client executes a command...')` block), after the `latencyMs` calculation (after line 388 `console.log(...latency...)`), add the perf entry call:

```javascript
    writePerfEntry('launcher-client-round-trip', 'npm-public', latencyMs, clientResult.code,
      clientResult.stdout.split('\n').filter(Boolean).length);
```

- [ ] **Step 3: Add `writePerfEntry` helper to public-session-persistence.test.js**

In `packages/e2e-tests/tests/public-session-persistence.test.js`, after the `import` statements (after line 27), add:

```javascript
import fs from 'node:fs';

/**
 * Write a performance JSON entry to TEST_PERF_OUTPUT (if set).
 */
function writePerfEntry(name, transport, durationMs, exitCode, stdoutLines) {
  const perfPath = process.env.TEST_PERF_OUTPUT;
  if (!perfPath) return;
  const entry = JSON.stringify({
    name,
    transport,
    duration_ms: durationMs,
    exit_code: exitCode ?? 0,
    stdout_lines: stdoutLines ?? 0,
    status: (exitCode ?? 0) === 0 ? 'pass' : 'fail',
  });
  fs.appendFileSync(perfPath, entry + '\n');
}
```

Note: `public-session-persistence.test.js` already imports `fs` on line 26, so check first — if `fs` is already imported, skip the `import fs` line.

- [ ] **Step 4: Add perf entries to session persistence test**

In the main test body of `public-session-persistence.test.js`, at the end of the test (just before the final assertion or cleanup), add timing. The test should have its overall duration tracked. Wrap the main test logic by recording `Date.now()` at start and computing duration at end:

At the beginning of the test function (after `test('full session persistence flow'...`), add:
```javascript
  const testStart = Date.now();
```

At the end of the test (just before the closing `}`), add:
```javascript
  writePerfEntry('session-persistence', 'npm-public', Date.now() - testStart, 0, 0);
```

- [ ] **Step 5: Verify npm tests still parse**

Run:
```bash
node --check packages/e2e-tests/tests/public-server.test.js
node --check packages/e2e-tests/tests/public-session-persistence.test.js
```
Expected: No syntax errors.

- [ ] **Step 6: Commit**

```bash
git add packages/e2e-tests/tests/public-server.test.js packages/e2e-tests/tests/public-session-persistence.test.js
git commit -m "feat(e2e): add performance JSON output to npm E2E tests"
```

---

### Task 4: Create Unified E2E Test Script

**Files:**
- Create: `scripts/e2e-test-suite.sh`

- [ ] **Step 1: Create the script**

Create `scripts/e2e-test-suite.sh`:

```bash
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

# Run tests with JSON output for JUnit conversion, AND with --nocapture for stderr
# cargo2junit needs the JSON stream; we tee it so we also see progress
RUST_TEST_EXIT=0
cargo test -p mxdx-worker --test e2e_profile -- --ignored --test-threads=1 --nocapture 2>&1 | tee test-results/rust-e2e-raw.txt || RUST_TEST_EXIT=${PIPESTATUS[0]}

# Generate JUnit XML if cargo2junit is available
if command -v cargo2junit &>/dev/null; then
  echo "--- Generating Rust JUnit XML ---"
  # Re-run with --format=json for cargo2junit (suppressing output)
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
node scripts/purge-test-accounts.mjs

echo ""
echo "=== All E2E tests PASSED ==="
echo "Results in test-results/:"
ls -la test-results/
```

- [ ] **Step 2: Make the script executable**

Run:
```bash
chmod +x scripts/e2e-test-suite.sh
```

- [ ] **Step 3: Verify script syntax**

Run:
```bash
bash -n scripts/e2e-test-suite.sh
```
Expected: No syntax errors.

- [ ] **Step 4: Commit**

```bash
git add scripts/e2e-test-suite.sh
git commit -m "feat(e2e): add unified E2E test suite orchestrator script"
```

---

### Task 5: Add Binary Crates to publish-crates.sh

**Files:**
- Modify: `scripts/publish-crates.sh`

- [ ] **Step 1: Add mxdx-worker and mxdx-client to the CRATES array**

In `scripts/publish-crates.sh`, replace the CRATES array (lines 7-15):

```bash
CRATES=(
  mxdx-types
  mxdx-matrix
  mxdx-policy
  mxdx-secrets
  mxdx-launcher
  mxdx-web
  mxdx-core-wasm
  mxdx-worker
  mxdx-client
)
```

- [ ] **Step 2: Commit**

```bash
git add scripts/publish-crates.sh
git commit -m "feat(release): add mxdx-worker and mxdx-client binary crates to publish order"
```

---

### Task 6: Add E2E Release Job to CI Pipeline

**Files:**
- Modify: `.github/workflows/ci.yml`

- [ ] **Step 1: Add `e2e-release` job to ci.yml**

Append the following job at the end of `.github/workflows/ci.yml` (after the `npm-public-server` job):

```yaml

  e2e-release:
    needs: build
    runs-on: ubuntu-latest
    if: ${{ secrets.TEST_CREDENTIALS_TOML != '' }}
    timeout-minutes: 30
    steps:
      - uses: actions/checkout@v4

      # Pin to 1.93.1: rustc 1.94.0 has a trait solver regression
      # ("queries overflow the depth limit") when compiling matrix-sdk 0.16
      - uses: dtolnay/rust-toolchain@1.93.1
        with:
          targets: wasm32-unknown-unknown

      - uses: Swatinem/rust-cache@v2

      - uses: actions/setup-node@v4
        with:
          node-version: "22"

      - name: Install system deps
        run: sudo apt-get install -y libsqlite3-dev libssl-dev tmux

      - name: Install wasm-pack
        run: curl https://rustwasm.github.io/wasm-pack/installer/init.sh -sSf | sh

      - name: Write test credentials
        run: echo "$TEST_CREDENTIALS_TOML" > test-credentials.toml
        env:
          TEST_CREDENTIALS_TOML: ${{ secrets.TEST_CREDENTIALS_TOML }}

      - name: Run E2E test suite (release profile)
        run: ./scripts/e2e-test-suite.sh --profile release

      - name: Upload test results
        if: always()
        uses: actions/upload-artifact@v4
        with:
          name: e2e-test-results
          path: test-results/
          retention-days: 90

      - name: Render Rust test report
        if: always()
        uses: dorny/test-reporter@v1
        with:
          name: Rust E2E Tests
          path: test-results/rust-e2e-junit.xml
          reporter: java-junit
          fail-on-error: false

      - name: Render npm test report
        if: always()
        uses: dorny/test-reporter@v1
        with:
          name: npm E2E Tests
          path: test-results/npm-e2e-junit.xml
          reporter: java-junit
          fail-on-error: false
```

- [ ] **Step 2: Verify YAML syntax**

Run:
```bash
python3 -c "import yaml; yaml.safe_load(open('.github/workflows/ci.yml'))" 2>&1 || echo "YAML syntax error"
```
Expected: No output (valid YAML). If python3 or pyyaml is not available, use:
```bash
node -e "const fs=require('fs'); try { require('js-yaml').load(fs.readFileSync('.github/workflows/ci.yml')); } catch(e) { console.log(e.message); }"
```
Or visually inspect indentation.

- [ ] **Step 3: Commit**

```bash
git add .github/workflows/ci.yml
git commit -m "feat(ci): add e2e-release job with test reports and performance artifacts"
```

---

### Task 7: Split Release Pipeline into E2E Gate + Release

**Files:**
- Modify: `.github/workflows/release.yml`
- Modify: `.releaserc.json`

- [ ] **Step 1: Rewrite release.yml with two jobs**

Replace the entire contents of `.github/workflows/release.yml`:

```yaml
name: Release

on:
  push:
    branches: [main]

permissions:
  contents: write
  id-token: write
  issues: write
  pull-requests: write
  checks: write

jobs:
  e2e-gate:
    runs-on: ubuntu-latest
    timeout-minutes: 45
    steps:
      - uses: actions/checkout@v4
        with:
          fetch-depth: 0
          persist-credentials: false

      # Pin to 1.93.1: rustc 1.94.0 has a trait solver regression
      # ("queries overflow the depth limit") when compiling matrix-sdk 0.16
      - uses: dtolnay/rust-toolchain@1.93.1
        with:
          targets: wasm32-unknown-unknown

      - uses: Swatinem/rust-cache@v2

      - uses: actions/setup-node@v4
        with:
          node-version: 22
          registry-url: https://registry.npmjs.org

      - name: Install system deps
        run: sudo apt-get install -y libsqlite3-dev libssl-dev tmux

      - name: Install wasm-pack
        run: curl https://rustwasm.github.io/wasm-pack/installer/init.sh -sSf | sh

      - name: Write test credentials
        if: env.TEST_CREDENTIALS_TOML != ''
        run: echo "$TEST_CREDENTIALS_TOML" > test-credentials.toml
        env:
          TEST_CREDENTIALS_TOML: ${{ secrets.TEST_CREDENTIALS_TOML }}

      # Run unit tests first (fast gate)
      - name: Run Rust unit tests
        run: cargo test --workspace --lib --exclude mxdx-core-wasm

      # Run E2E test suite (builds release binaries + WASM, runs both suites)
      - name: Run E2E test suite (release profile)
        if: env.TEST_CREDENTIALS_TOML != ''
        run: ./scripts/e2e-test-suite.sh --profile release
        env:
          TEST_CREDENTIALS_TOML: ${{ secrets.TEST_CREDENTIALS_TOML }}

      # Build release binaries (if E2E was skipped due to missing secret, build here)
      - name: Build release binaries (fallback)
        if: env.TEST_CREDENTIALS_TOML == ''
        run: |
          cargo build --release -p mxdx-worker -p mxdx-client
          wasm-pack build crates/mxdx-core-wasm --target nodejs --out-dir ../../packages/core/wasm/nodejs
          wasm-pack build crates/mxdx-core-wasm --target web --out-dir ../../packages/core/wasm/web
          rm -f packages/core/wasm/nodejs/.gitignore packages/core/wasm/nodejs/package.json \
                packages/core/wasm/web/.gitignore packages/core/wasm/web/package.json
          npm install
        env:
          TEST_CREDENTIALS_TOML: ${{ secrets.TEST_CREDENTIALS_TOML }}

      - name: Build web-console
        run: cd packages/web-console && npx vite build

      - name: Smoke test dispatcher
        run: |
          node packages/mxdx/bin/mxdx.js --help
          node packages/mxdx/bin/mxdx.js --version
          node packages/mxdx/bin/mxdx.js launcher --help
          node packages/mxdx/bin/mxdx.js client --help

      # Upload release binaries as artifacts for the release job
      - name: Upload release binaries
        uses: actions/upload-artifact@v4
        with:
          name: release-binaries
          path: |
            target/release/mxdx-worker
            target/release/mxdx-client
          retention-days: 1

      # Upload test results
      - name: Upload test results
        if: always()
        uses: actions/upload-artifact@v4
        with:
          name: release-e2e-results
          path: test-results/
          retention-days: 90

      - name: Render Rust test report
        if: always() && hashFiles('test-results/rust-e2e-junit.xml') != ''
        uses: dorny/test-reporter@v1
        with:
          name: Release Rust E2E Tests
          path: test-results/rust-e2e-junit.xml
          reporter: java-junit
          fail-on-error: false

      - name: Render npm test report
        if: always() && hashFiles('test-results/npm-e2e-junit.xml') != ''
        uses: dorny/test-reporter@v1
        with:
          name: Release npm E2E Tests
          path: test-results/npm-e2e-junit.xml
          reporter: java-junit
          fail-on-error: false

  release:
    needs: e2e-gate
    runs-on: ubuntu-latest
    steps:
      - uses: actions/checkout@v4
        with:
          fetch-depth: 0
          persist-credentials: false

      # Pin to 1.93.1: rustc 1.94.0 has a trait solver regression
      - uses: dtolnay/rust-toolchain@1.93.1
        with:
          targets: wasm32-unknown-unknown

      - uses: Swatinem/rust-cache@v2

      - uses: actions/setup-node@v4
        with:
          node-version: 22
          registry-url: https://registry.npmjs.org

      - name: Install system deps
        run: sudo apt-get install -y libsqlite3-dev libssl-dev

      - name: Install wasm-pack
        run: curl https://rustwasm.github.io/wasm-pack/installer/init.sh -sSf | sh

      - name: Build WASM (nodejs)
        run: wasm-pack build crates/mxdx-core-wasm --target nodejs --out-dir ../../packages/core/wasm/nodejs

      - name: Build WASM (web)
        run: wasm-pack build crates/mxdx-core-wasm --target web --out-dir ../../packages/core/wasm/web

      - name: Clean wasm-pack artifacts
        run: rm -f packages/core/wasm/nodejs/.gitignore packages/core/wasm/nodejs/package.json packages/core/wasm/web/.gitignore packages/core/wasm/web/package.json

      - name: Install npm deps
        run: npm install

      - name: Build web-console
        run: cd packages/web-console && npx vite build

      # Download pre-built release binaries from e2e-gate
      - name: Download release binaries
        uses: actions/download-artifact@v4
        with:
          name: release-binaries
          path: release-binaries/

      - name: Make binaries executable
        run: chmod +x release-binaries/mxdx-worker release-binaries/mxdx-client

      # Rename binaries with platform suffix for GitHub Release
      - name: Prepare release assets
        run: |
          mkdir -p release-assets
          cp release-binaries/mxdx-worker release-assets/mxdx-worker-x86_64-unknown-linux-gnu
          cp release-binaries/mxdx-client release-assets/mxdx-client-x86_64-unknown-linux-gnu

      - name: Release
        env:
          GITHUB_TOKEN: ${{ secrets.GITHUB_TOKEN }}
          CARGO_REGISTRY_TOKEN: ${{ secrets.CARGO_REGISTRY_TOKEN }}
        run: npx semantic-release
```

- [ ] **Step 2: Update `.releaserc.json` to include release assets**

Replace the contents of `.releaserc.json`:

```json
{
  "branches": ["main"],
  "verifyConditions": [
    "@semantic-release/github",
    "@semantic-release/git"
  ],
  "plugins": [
    "@semantic-release/commit-analyzer",
    "@semantic-release/release-notes-generator",
    [
      "@semantic-release/exec",
      {
        "prepareCmd": "node scripts/bump-versions.mjs ${nextRelease.version}",
        "publishCmd": "bash scripts/publish-crates.sh && bash scripts/publish-npm.sh"
      }
    ],
    [
      "@semantic-release/github",
      {
        "successComment": false,
        "failTitle": false,
        "assets": [
          {"path": "release-assets/mxdx-worker-x86_64-unknown-linux-gnu", "label": "mxdx-worker (Linux x86_64)"},
          {"path": "release-assets/mxdx-client-x86_64-unknown-linux-gnu", "label": "mxdx-client (Linux x86_64)"}
        ]
      }
    ],
    [
      "@semantic-release/git",
      {
        "assets": [
          "Cargo.toml",
          "crates/*/Cargo.toml",
          "packages/*/package.json",
          "Cargo.lock"
        ],
        "message": "chore(release): ${nextRelease.version} [skip ci]"
      }
    ]
  ]
}
```

- [ ] **Step 3: Verify YAML and JSON syntax**

Run:
```bash
python3 -c "import yaml; yaml.safe_load(open('.github/workflows/release.yml'))" 2>&1 || echo "YAML error"
node -e "JSON.parse(require('fs').readFileSync('.releaserc.json'))" 2>&1 || echo "JSON error"
```
Expected: No errors.

- [ ] **Step 4: Commit**

```bash
git add .github/workflows/release.yml .releaserc.json
git commit -m "feat(release): split into e2e-gate + release jobs, add binary publishing"
```

---

### Task 8: Local Validation

**Files:** None (testing only)

- [ ] **Step 1: Verify all Rust tests compile**

Run:
```bash
cargo test -p mxdx-worker --test e2e_profile -- --list
```
Expected: All test names listed, no compile errors.

- [ ] **Step 2: Verify mxdx-client unit tests still pass**

Run:
```bash
cargo test -p mxdx-client
```
Expected: All tests pass.

- [ ] **Step 3: Verify npm test files have valid syntax**

Run:
```bash
node --check packages/e2e-tests/tests/public-server.test.js
node --check packages/e2e-tests/tests/public-session-persistence.test.js
```
Expected: No syntax errors.

- [ ] **Step 4: Verify the e2e-test-suite.sh script is executable and parses**

Run:
```bash
bash -n scripts/e2e-test-suite.sh && echo "OK"
test -x scripts/e2e-test-suite.sh && echo "Executable"
```
Expected: "OK" and "Executable".

- [ ] **Step 5: Verify YAML and JSON configs parse**

Run:
```bash
python3 -c "import yaml; yaml.safe_load(open('.github/workflows/ci.yml')); yaml.safe_load(open('.github/workflows/release.yml')); print('YAML OK')"
node -e "JSON.parse(require('fs').readFileSync('.releaserc.json')); console.log('JSON OK')"
```
Expected: "YAML OK" and "JSON OK".

- [ ] **Step 6: Run a quick E2E smoke test (if test-credentials.toml available)**

Run:
```bash
# Just the local echo test to verify MXDX_BIN_DIR and TEST_PERF_OUTPUT work
export MXDX_BIN_DIR="$(pwd)/target/debug"
export TEST_PERF_OUTPUT="/tmp/test-perf-smoke.jsonl"
cargo test -p mxdx-worker --test e2e_profile -- --ignored profile_echo_local --nocapture
cat /tmp/test-perf-smoke.jsonl
```
Expected: Test runs (pass or fail depending on credentials); if it passes, a JSON line appears in the perf output file.

---

## Dependency Graph

```
Task 1 (cargo_bin) ─┐
                     ├─→ Task 4 (e2e-test-suite.sh) ─→ Task 6 (CI pipeline)
Task 2 (report)    ─┘                                 ─→ Task 7 (Release pipeline)
                                                            ↑
Task 3 (npm perf)  ─→ Task 4                          Task 5 (publish-crates.sh)
                                                            │
                                                       Task 8 (validation)
```

Tasks 1, 2, 3 can be done in parallel. Task 4 depends on 1+2+3. Task 5 is independent. Tasks 6 and 7 depend on 4+5. Task 8 depends on all.

---

## Post-Implementation Checklist

After all tasks are complete:

1. **Set up GitHub secret**: Add `TEST_CREDENTIALS_TOML` to the repository secrets containing the full `test-credentials.toml` file
2. **Install cargo2junit** (optional, for JUnit generation): `cargo install cargo2junit`
3. **Configure required status check**: In GitHub repo settings → Branch protection rules → add `e2e-release` as a required check for PRs to `main`
4. **Test the full pipeline**: Push a branch, verify the `e2e-release` CI job runs, then merge to trigger the release pipeline with `e2e-gate` → `release`
