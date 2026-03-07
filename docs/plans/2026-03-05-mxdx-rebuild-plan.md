# mxdx Management Console — Rebuild Implementation Plan

> **For Claude:** REQUIRED: Enable agent teams (`CLAUDE_CODE_EXPERIMENTAL_AGENT_TEAMS=1`), then start the build by spawning the team defined in the **Agent Team** section below. Use superpowers:executing-plans to implement this plan task-by-task.

**Goal:** Build the mxdx Management Console — a Matrix-native fleet management system with interactive browser terminals, E2EE, multi-homeserver redundancy, and a WASI-packaged launcher.

**Architecture:** v1 ships as npm packages backed by Rust compiled to WASM (`@mxdx/core` + `packages/launcher` + `packages/client`). All communication flows over Matrix E2EE rooms (including MSC4362 encrypted state events) using the `org.mxdx.*` event namespace. Non-interactive commands use the exec room. Interactive terminals use DMs initiated by the launcher. Room topology: Space + Exec (E2EE + encrypted state for commands/telemetry) + Logs (E2EE for launcher/system logs). Policy, secrets, and web app are Rust-native. Native Rust binaries for launcher/client follow as a second target.

**Tech Stack:** Rust/WASM (wasm-pack, matrix-sdk 0.16), Node.js (npm workspace), fake-indexeddb (WASM crypto polyfill), Tuwunel (homeserver), HTMX, xterm.js, tmux, age 0.11, Axum 0.7.

**Revised:** 2026-03-06 — Updated architecture to npm+WASM v1, all rooms E2EE with MSC4362, simplified room topology (no separate status room).

**Design doc:** `docs/plans/2026-03-05-mxdx-rebuild-design.md`
**Security review:** `docs/reviews/security/2026-03-05-design-review-plan-and-spec.md`

---

## Agent Team

### Team Composition (7 roles)

| Role | Type | Setup Command | Responsibility |
|:---|:---|:---|:---|
| **Product Owner** | Main session | — | Reviews Lead's decisions, approves phase completions, resolves escalations |
| **Lead** | Spawned teammate | `npx -y @epiphytic/agenticenti compose team-leader rust typescript-node` | Coordinates phases, assigns work, reviews code, merges PRs. Never writes code. |
| **Tester** | Spawned teammate | `npx -y @epiphytic/agenticenti compose tester rust typescript-node` | Writes failing tests (including security exploit tests), closes [T] issues |
| **Coder** | Spawned teammate | `npx -y @epiphytic/agenticenti compose coder rust typescript-node` | Implements until tests pass, runs `cargo xtask manifest`, opens PRs |
| **Security Reviewer** | Spawned teammate | `npx -y @epiphytic/agenticenti compose security-reviewer rust typescript-node` | Reviews phases, writes adversarial variants, produces security reports |
| **DevOps** | Spawned teammate | `npx -y @epiphytic/agenticenti compose devops rust typescript-node` | Preflight, CI evolution, tuwunel research, WASI packaging |
| **Documenter** | Spawned teammate | `npx -y @epiphytic/agenticenti compose documenter rust typescript-node` | Keeps MANIFEST.md, AGENTS.md, ADRs in sync. Phase summaries. Spec drift detection. |

### Startup Prompt

```
Enable agent teams and create the mxdx rebuild team. Spawn 6 teammates, giving
each their agenticenti setup command as the first instruction:

- Lead: First run `npx -y @epiphytic/agenticenti compose team-leader rust typescript-node`.
  Then: Read AGENTS.md, the design doc at docs/plans/2026-03-05-mxdx-rebuild-design.md,
  and this plan. Coordinate phases in dependency order. Assign [T] tasks to Tester,
  [C] tasks to Coder, [S] tasks to Security Reviewer, [D] tasks to Documenter,
  [CI] tasks to DevOps. After each phase: verify completion gate, trigger Security
  Reviewer, then report to Product Owner. Monitor `bd blocked` to catch stuck teammates.
  All PRs go through Lead review. YOU MERGE PRs — do not wait for Product Owner.

- Tester: First run `npx -y @epiphytic/agenticenti compose tester rust typescript-node`.
  Then: writes failing tests for every [T] beads issue. Run `bd ready` to find
  unblocked [T] tasks. Claim with `bd update <id> --status=in_progress`. After
  tests are written and confirmed failing, close with `bd close <id>`, then
  message Coder: "Task X.Y [T] done — branch <branch> pushed".

- Coder: First run `npx -y @epiphytic/agenticenti compose coder rust typescript-node`.
  Then: implements code for every [C] beads issue once the paired [T] is closed.
  Run `bd ready` to find unblocked [C] tasks. After implementation, run
  `cargo xtask manifest` and commit the updated MANIFEST.md before opening a PR.
  Notify Lead for review. Close issue after merge.

- Security Reviewer: First run `npx -y @epiphytic/agenticenti compose security-reviewer rust typescript-node`.
  Then: claims [S] tasks from beads as phases complete. For each phase:
  (1) Review Tester's security tests — do they exercise the real attack vector?
  (2) Review Coder's fix — is it genuine or test-aware?
  (3) Write one adversarial variant the Coder didn't see.
  File review doc to docs/reports/security/. Message Lead when complete.

- DevOps: First run `npx -y @epiphytic/agenticenti compose devops rust typescript-node`.
  Then: handles preflight, CI pipeline, tuwunel research, and WASI packaging tasks.
  Run `bd ready` to find available work.

- Documenter: First run `npx -y @epiphytic/agenticenti compose documenter rust typescript-node`.
  Then: After each PR merge, verify MANIFEST.md matches `cargo xtask manifest` output.
  After each phase, write phase summary to docs/phases/phase-N-summary.md.
  Format Security Reviewer findings into CI artifact structure. Compare implemented
  behavior against spec docs and flag divergences to Lead.

IMPORTANT — Development Standards (all teammates must follow):
1. No mock implementations, no shortcuts. Build the real thing every time.
2. If something appears impossible or blocked, STOP and message Lead immediately.
   Lead will escalate to Product Owner rather than guess or fake it.
3. All Rust code: after creating or modifying any file, run `cargo xtask manifest`
   to regenerate MANIFEST.md. Commit the result with the code.
4. Security findings are task requirements, not suggestions. Every finding ID
   referenced in a task MUST be addressed in the implementation.
5. CI must be green before any PR merges. If CI fails, fix it — do not skip.
```

### Coordination Protocol

See design doc Section 1 for full protocol. Key points:
- TDD Handoff: [T] -> [C] -> Lead review -> merge
- Security Verification: test review + implementation review + adversarial variant
- Phase Gate: all issues closed + CI green + security review + docs updated + PO sign-off
- Escalation: 2 failed attempts -> Lead escalates to PO immediately

---

## Repo Layout

```
mxdx/
├── Cargo.toml                     # Workspace root
├── crates/
│   ├── mxdx-types/                # Shared event schema types
│   ├── mxdx-core-wasm/            # WASM bindings (WasmMatrixClient via wasm-bindgen)
│   ├── mxdx-matrix/               # Native Rust Matrix client facade (matrix-sdk wrapper)
│   ├── mxdx-policy/               # Policy Agent appservice binary (Rust native)
│   ├── mxdx-secrets/              # Secrets Coordinator binary (Rust native)
│   ├── mxdx-launcher/             # Native Rust launcher binary (future target)
│   └── mxdx-web/                  # Web app (Axum, HTMX templates, Rust native)
├── packages/                      # npm workspace (v1 shipping surface)
│   ├── core/                      # @mxdx/core — WASM re-exports + session/credentials
│   │   ├── wasm/                  # wasm-pack output
│   │   ├── index.js               # Re-exports + fake-indexeddb polyfill
│   │   ├── session.js             # connectWithSession() shared helper
│   │   └── credentials.js         # CredentialStore (keyring + encrypted file)
│   ├── launcher/                  # mxdx-launcher (npm, uses @mxdx/core)
│   │   ├── bin/mxdx-launcher.js
│   │   └── src/                   # config, runtime, process-bridge
│   ├── client/                    # mxdx-client CLI (npm, uses @mxdx/core)
│   │   ├── bin/mxdx-client.js
│   │   └── src/                   # config, discovery, exec
│   └── e2e-tests/                 # E2E tests (local Tuwunel + public server)
├── tests/
│   ├── helpers/                   # TuwunelInstance, FederatedPair (Rust)
│   ├── e2e/                       # Rust E2E test binaries
│   └── federation/                # Multi-homeserver tests
├── xtask/                         # cargo xtask manifest
├── scripts/
│   └── preflight.sh               # Environment verification
├── docs/
│   ├── plans/
│   ├── adr/
│   ├── phases/                    # Phase summary docs
│   ├── reports/security/          # Security review reports
│   └── templates/
├── .github/workflows/
├── MANIFEST.md
└── AGENTS.md
```

---

## Phase 0: Preflight & Research

> **Epic:** Verify environment, research Tuwunel ground truth.
> **Agents:** DevOps (both tasks).
> **Completion gate:** All tools verified (or gaps acknowledged by Product Owner), Tuwunel ADR written and validated by actually running tuwunel.
> **Branch:** `feat/phase-0-preflight`

### Task 0.0: Environment Preflight Script [DevOps]

**Files:**
- Create: `scripts/preflight.sh`

**Step 1: Write the preflight script**

```bash
#!/usr/bin/env bash
set -euo pipefail

PASS=0
FAIL=0
BLOCKED_PHASES=""

check() {
    local name="$1" cmd="$2" blocks="$3" install="$4"
    if command -v "$cmd" &>/dev/null || eval "$cmd" &>/dev/null 2>&1; then
        echo "  [PASS] $name ($(command -v "$cmd" 2>/dev/null || echo 'found'))"
        ((PASS++))
    else
        echo "  [FAIL] $name -- not found"
        echo "         Install: $install"
        echo "         Blocks: $blocks"
        ((FAIL++))
        BLOCKED_PHASES="$BLOCKED_PHASES $blocks"
    fi
}

check_lib() {
    local name="$1" pkg="$2" blocks="$3" install="$4"
    if pkg-config --exists "$pkg" 2>/dev/null; then
        echo "  [PASS] $name"
        ((PASS++))
    else
        echo "  [FAIL] $name -- not found"
        echo "         Install: $install"
        echo "         Blocks: $blocks"
        ((FAIL++))
        BLOCKED_PHASES="$BLOCKED_PHASES $blocks"
    fi
}

echo "mxdx Preflight Check"
echo "===================="
echo ""

# Core tools
check "cargo" "cargo" "Phase 1+" "curl --proto '=https' --tlsv1.2 -sSf https://sh.rustup.rs | sh"
check "rustc" "rustc" "Phase 1+" "curl --proto '=https' --tlsv1.2 -sSf https://sh.rustup.rs | sh"
check "node" "node" "Phase 2+ (TS types)" "install via nvm or package manager"
check "npm" "npm" "Phase 2+ (TS types)" "comes with node"
check "git" "git" "Phase 1+" "sudo apt-get install -y git"
check "tuwunel" "tuwunel" "Phase 3+" "see docs/adr/2026-03-05-tuwunel-ground-truth.md"
check "tmux" "tmux" "Phase 6 (interactive terminals)" "sudo apt-get install -y tmux"

# Dev headers
check_lib "libsqlite3-dev" "sqlite3" "Phase 4+ (matrix-sdk)" "sudo apt-get install -y libsqlite3-dev"
check_lib "libssl-dev" "openssl" "Phase 4+ (matrix-sdk)" "sudo apt-get install -y libssl-dev"

# Security & packaging
check "softhsm2-util" "softhsm2-util" "Phase 9 (secrets)" "sudo apt-get install -y softhsm2"
check "mkcert" "mkcert" "Phase 11 (federation TLS)" "go install filippo.io/mkcert@latest"

# WASI target
if rustup target list --installed 2>/dev/null | grep -q "wasm32-wasip2"; then
    echo "  [PASS] wasm32-wasip2 target"
    ((PASS++))
else
    echo "  [FAIL] wasm32-wasip2 target -- not installed"
    echo "         Install: rustup target add wasm32-wasip2"
    echo "         Blocks: Phase 12 (WASI)"
    ((FAIL++))
    BLOCKED_PHASES="$BLOCKED_PHASES Phase-12"
fi

# Playwright (browser E2E)
if npx playwright --version &>/dev/null 2>&1; then
    echo "  [PASS] playwright"
    ((PASS++))
else
    echo "  [FAIL] playwright -- not found"
    echo "         Install: npx playwright install"
    echo "         Blocks: Phase 7 (browser E2E)"
    ((FAIL++))
    BLOCKED_PHASES="$BLOCKED_PHASES Phase-7"
fi

echo ""
echo "Results: $PASS passed, $FAIL failed"

if [ "$FAIL" -gt 0 ]; then
    echo "Blocked phases:$BLOCKED_PHASES"
    echo ""
    read -p "Proceed with gaps? [y/N] " -n 1 -r
    echo
    if [[ ! $REPLY =~ ^[Yy]$ ]]; then
        echo "Install missing tools and re-run."
        exit 1
    fi
    echo "Proceeding with acknowledged gaps."
fi
```

**Step 2: Make executable and test**

Run: `chmod +x scripts/preflight.sh && bash scripts/preflight.sh`
Expected: Lists all tools with PASS/FAIL status.

**Step 3: Commit**

```bash
git add scripts/preflight.sh
git commit -m "chore: add environment preflight check script"
```

---

### Task 0.1: Tuwunel Research Spike [DevOps]

**Files:**
- Create: `docs/adr/2026-03-05-tuwunel-ground-truth.md`

**Step 1: Install tuwunel**

Fetch the latest release from https://github.com/matrix-construct/tuwunel/releases/latest. Use WebFetch to find the correct binary/package for the current platform. Install it.

**Step 2: Document by experimentation**

Actually run tuwunel. For each of these, try it and document what happens:

1. **Installation:** What binary name? What packages are available? How does CI install it?
2. **CLI flags:** Run `tuwunel --help`. Document all flags, especially config file flag.
3. **Config format:** Create a minimal config, start tuwunel, see what works. Document the actual TOML keys that tuwunel accepts for: server_name, database path, port, address, registration, log level.
4. **Health check:** What endpoint responds when tuwunel is ready? `/_matrix/client/versions`? How long does startup take?
5. **User registration:** Try registering a user via the Matrix client API (`POST /_matrix/client/v3/register`). Document what works.
6. **Federation:** Try starting two instances. What config do they need to see each other? Do they need TLS? Do they need well-known files?
7. **Appservice:** How to register an appservice? Config file path? API endpoint? Document the registration YAML format tuwunel expects.
8. **Shutdown:** How to gracefully stop? SIGTERM? SIGINT? How long does it take?
9. **Quirks:** Anything unexpected. Default values that differ from Synapse. Required fields that aren't obvious.

**Step 3: Write the ADR**

Write `docs/adr/2026-03-05-tuwunel-ground-truth.md` with all findings, organized by the 9 categories above. Include exact commands and config snippets that were validated by running them.

**Step 4: Commit**

```bash
git add docs/adr/2026-03-05-tuwunel-ground-truth.md
git commit -m "docs(adr): add tuwunel ground truth from research spike"
```

---

## Phase 1: Foundation

> **Epic:** Cargo workspace scaffold, xtask manifest generator, npm workspace, build-only CI.
> **Agents:** Coder (scaffold), DevOps (CI).
> **Completion gate:** `cargo build --workspace` passes, `cargo xtask manifest` works, `cd client && npm install && npm run build` works, CI runs build check.
> **Branch:** `feat/phase-1-foundation`

### Task 1.1 [C]: Cargo Workspace Scaffold

**Files:**
- Create: `Cargo.toml` (workspace)
- Create: `crates/mxdx-types/Cargo.toml`, `crates/mxdx-types/src/lib.rs`
- Create: `crates/mxdx-matrix/Cargo.toml`, `crates/mxdx-matrix/src/lib.rs`
- Create: `crates/mxdx-policy/Cargo.toml`, `crates/mxdx-policy/src/main.rs`
- Create: `crates/mxdx-secrets/Cargo.toml`, `crates/mxdx-secrets/src/main.rs`
- Create: `crates/mxdx-launcher/Cargo.toml`, `crates/mxdx-launcher/src/main.rs`
- Create: `crates/mxdx-web/Cargo.toml`, `crates/mxdx-web/src/main.rs`

**Step 1: Write workspace Cargo.toml**

```toml
[workspace]
members = [
    "crates/mxdx-types",
    "crates/mxdx-matrix",
    "crates/mxdx-policy",
    "crates/mxdx-secrets",
    "crates/mxdx-launcher",
    "crates/mxdx-web",
    "xtask",
]
resolver = "2"

[workspace.dependencies]
tokio = { version = "1", features = ["full"] }
serde = { version = "1", features = ["derive"] }
serde_json = "1"
tracing = "0.1"
tracing-subscriber = { version = "0.3", features = ["json"] }
anyhow = "1"
uuid = { version = "1", features = ["v4"] }
matrix-sdk = { version = "0.16", features = ["e2e-encryption", "sqlite"] }
ruma = { version = "0.14", features = ["client-api-c", "events", "appservice-api-c"] }
toml = "0.8"
axum = "0.7"
age = "0.11"
sysinfo = "0.33"
reqwest = { version = "0.12", features = ["json"] }
tempfile = "3"
base64 = "0.22"
flate2 = "1"
lru = "0.12"
```

**Step 2: Create each crate with minimal Cargo.toml**

Each lib crate gets:
```toml
[package]
name = "mxdx-types"  # adjust per crate
version = "0.1.0"
edition = "2021"

[dependencies]
```

Each binary crate gets a `fn main() {}` stub.
Each lib crate gets an empty `pub` module in `lib.rs`.

Dependencies per crate (only what that crate needs):
- `mxdx-types`: serde, serde_json, uuid
- `mxdx-matrix`: mxdx-types (path), matrix-sdk, ruma, anyhow, tokio, tracing, serde_json, tempfile
- `mxdx-policy`: mxdx-types (path), mxdx-matrix (path), anyhow, tokio, tracing, serde, serde_json, ruma, lru
- `mxdx-secrets`: mxdx-types (path), mxdx-matrix (path), anyhow, tokio, tracing, serde, serde_json, age
- `mxdx-launcher`: mxdx-types (path), mxdx-matrix (path), anyhow, tokio, tracing, serde, serde_json, toml, sysinfo, uuid, base64, flate2
- `mxdx-web`: mxdx-types (path), axum, tokio, tracing, serde, serde_json

**Step 3: Verify it compiles**

Run: `cargo build --workspace`
Expected: All crates compile with zero errors.

**Step 4: Commit**

```bash
git add Cargo.toml Cargo.lock crates/
git commit -m "chore: initialize cargo workspace with all crates"
```

---

### Task 1.2 [C]: xtask Manifest Generator

**Files:**
- Create: `xtask/Cargo.toml`
- Create: `xtask/src/main.rs`
- Create: `MANIFEST.md`

**Step 1: Write xtask Cargo.toml**

```toml
[package]
name = "xtask"
version = "0.1.0"
edition = "2021"

[[bin]]
name = "xtask"
path = "src/main.rs"

[dependencies]
syn = { version = "2", features = ["full", "visit"] }
walkdir = "2"
anyhow = "1"
```

**Step 2: Implement xtask/src/main.rs**

Full implementation that:
1. Parses args: `cargo xtask manifest` (write) or `cargo xtask manifest --check` (diff)
2. Walks `crates/*/src/**/*.rs` using walkdir
3. Parses each file with `syn::parse_file`
4. Visits all `pub` items: fn, struct, enum, trait, impl blocks (extract method names)
5. Emits a Markdown table per crate to a staging buffer
6. Replaces relevant sections in MANIFEST.md in-place (idempotent)
7. `--check` mode: compare staging vs file, exit 1 if different

**Step 3: Create initial MANIFEST.md**

```markdown
# Module & Agent Manifest

## Crates

| Crate | Path | Purpose |
|:---|:---|:---|
| mxdx-types | crates/mxdx-types | Shared event schema types |
| mxdx-matrix | crates/mxdx-matrix | matrix-sdk facade |
| mxdx-policy | crates/mxdx-policy | Policy Agent appservice binary |
| mxdx-secrets | crates/mxdx-secrets | Secrets Coordinator binary |
| mxdx-launcher | crates/mxdx-launcher | Launcher binary |
| mxdx-web | crates/mxdx-web | Web app (Axum, HTMX) |

## npm Packages

| Package | Path | Purpose |
|:---|:---|:---|
| @mxdx/client | client/mxdx-client | Browser Matrix client with E2EE |
| @mxdx/web-ui | client/mxdx-web-ui | HTMX dashboard + xterm.js terminal |

## External Facades

| Facade | Crate | Wraps |
|:---|:---|:---|
| MatrixClient | mxdx-matrix | matrix-sdk — never call matrix-sdk directly |
| CryptoClient | client/mxdx-client/src/crypto.ts | matrix-sdk-crypto-wasm |
```

**Step 4: Run xtask**

Run: `cargo xtask manifest`
Expected: MANIFEST.md updated with symbol tables (mostly empty at this point).

Run: `cargo xtask manifest --check`
Expected: Exit 0 (no diff).

**Step 5: Commit**

```bash
git add xtask/ MANIFEST.md
git commit -m "chore: add xtask manifest generator with syn parsing"
```

---

### Task 1.3 [C]: npm Workspace Scaffold

**Files:**
- Create: `client/package.json`
- Create: `client/mxdx-client/package.json`
- Create: `client/mxdx-client/src/index.ts`
- Create: `client/mxdx-client/tsconfig.json`
- Create: `client/mxdx-web-ui/package.json`
- Create: `client/mxdx-web-ui/src/index.ts`

**Step 1: Create workspace root**

```json
{
  "name": "@mxdx/workspace",
  "private": true,
  "workspaces": ["mxdx-client", "mxdx-web-ui"],
  "scripts": {
    "test": "vitest run",
    "test:watch": "vitest",
    "build": "npm run build --workspaces"
  }
}
```

**Step 2: Create mxdx-client package**

```json
{
  "name": "@mxdx/client",
  "version": "0.1.0",
  "type": "module",
  "main": "dist/index.js",
  "types": "dist/index.d.ts",
  "scripts": {
    "build": "tsc",
    "test": "vitest run"
  },
  "dependencies": {
    "@matrix-org/matrix-sdk-crypto-wasm": "^17.1.0",
    "zod": "^3"
  },
  "devDependencies": {
    "typescript": "^5",
    "vitest": "^3"
  }
}
```

**Step 3: Create mxdx-web-ui package similarly**

**Step 4: Install and verify**

Run: `cd client && npm install && npm run build`
Expected: TypeScript compiles without errors.

**Step 5: Commit**

```bash
git add client/
git commit -m "chore: initialize npm workspace for mxdx-client and web-ui"
```

---

### Task 1.4 [CI]: Build-Only CI Pipeline

**Files:**
- Create: `.github/workflows/ci.yml`

**Step 1: Write CI config**

```yaml
name: CI

on:
  push:
    branches: ["**"]
  pull_request:
    branches: [main]

concurrency:
  group: ${{ github.workflow }}-${{ github.ref }}
  cancel-in-progress: true

jobs:
  preflight:
    runs-on: ubuntu-latest
    steps:
      - uses: actions/checkout@v4
      - uses: dtolnay/rust-toolchain@stable
      - uses: actions/setup-node@v4
        with:
          node-version: "22"
      - name: Check required tools
        run: |
          cargo --version
          rustc --version
          node --version
          npm --version

  build:
    needs: preflight
    runs-on: ubuntu-latest
    steps:
      - uses: actions/checkout@v4
      - uses: dtolnay/rust-toolchain@stable
      - uses: Swatinem/rust-cache@v2
      - name: Install system deps
        run: sudo apt-get install -y libsqlite3-dev libssl-dev
      - run: cargo build --workspace

  manifest:
    needs: build
    runs-on: ubuntu-latest
    steps:
      - uses: actions/checkout@v4
      - uses: dtolnay/rust-toolchain@stable
      - uses: Swatinem/rust-cache@v2
      - name: Install system deps
        run: sudo apt-get install -y libsqlite3-dev libssl-dev
      - run: cargo xtask manifest --check
        name: Verify MANIFEST.md is up to date

  client-build:
    needs: preflight
    runs-on: ubuntu-latest
    steps:
      - uses: actions/checkout@v4
      - uses: actions/setup-node@v4
        with:
          node-version: "22"
      - run: cd client && npm ci && npm run build
```

**Step 2: Commit**

```bash
git add .github/workflows/ci.yml
git commit -m "ci: add build-only CI pipeline (Phase 1)"
```

---

### Task 1.5 [D]: Initial Documentation

**Files:**
- Create: `AGENTS.md`
- Create: `docs/phases/phase-1-summary.md`

**Step 1: Write AGENTS.md**

Document the project conventions, bd commands, non-interactive shell rules, and the team structure from the design doc. Reference the design doc for full protocol details.

**Step 2: Write phase summary**

```markdown
# Phase 1: Foundation — Summary

**Status:** Complete
**Branch:** feat/phase-1-foundation
**What was built:**
- Cargo workspace with 6 crates + xtask
- npm workspace with 2 packages
- Build-only CI pipeline (preflight + build + manifest check + client build)
- MANIFEST.md with auto-generated symbol tables
- AGENTS.md with project conventions

**CI state after this phase:**
- preflight: tool version checks
- build: cargo build --workspace
- manifest: cargo xtask manifest --check
- client-build: npm ci && npm run build
```

**Step 3: Commit**

```bash
git add AGENTS.md docs/phases/
git commit -m "docs: add AGENTS.md and Phase 1 summary"
```

---

## Phase 2: Event Schema & Types

> **Epic:** Shared Rust types + TypeScript definitions for all `org.mxdx.*` events.
> **Agents:** Tester (writes type tests) -> Coder (implements types).
> **Completion gate:** All event types serialize/deserialize round-trip correctly. TypeScript types match Rust 1:1.
> **Branch:** `feat/phase-2-types`
> **CI update:** Add `cargo test -p mxdx-types` and `cd client && npm test` jobs.

### Task 2.1 [T]: Core Event Type Tests (Rust)

**Files:**
- Create: `crates/mxdx-types/src/events/mod.rs`
- Create: `crates/mxdx-types/src/events/command.rs` (tests only)
- Create: `crates/mxdx-types/src/events/output.rs` (tests only)
- Create: `crates/mxdx-types/src/events/result.rs` (tests only)
- Create: `crates/mxdx-types/src/events/telemetry.rs` (tests only)
- Create: `crates/mxdx-types/src/events/secret.rs` (tests only)

**Step 1: Write tests for CommandEvent**

```rust
// crates/mxdx-types/src/events/command.rs
#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn command_event_round_trips_json() {
        let cmd = CommandEvent {
            uuid: "550e8400-e29b-41d4-a716-446655440000".into(),
            action: CommandAction::Exec,
            cmd: "cargo build --release".into(),
            args: vec!["--features".into(), "gpu".into()],
            env: [("RUST_LOG".into(), "info".into())].into(),
            cwd: Some("/workspace".into()),
            timeout_seconds: Some(3600),
        };
        let json = serde_json::to_string(&cmd).unwrap();
        let parsed: CommandEvent = serde_json::from_str(&json).unwrap();
        assert_eq!(parsed.uuid, cmd.uuid);
        assert_eq!(parsed.action, CommandAction::Exec);
        assert_eq!(parsed.args, cmd.args);
    }

    #[test]
    fn command_event_rejects_unknown_action() {
        let json = r#"{"uuid":"x","action":"fly_to_moon","cmd":"x","args":[],"env":{}}"#;
        let result: Result<CommandEvent, _> = serde_json::from_str(json);
        assert!(result.is_err());
    }
}
```

**Step 2: Write tests for OutputEvent**

```rust
// crates/mxdx-types/src/events/output.rs
#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn output_event_has_stream_field() {
        let out = OutputEvent {
            uuid: "test-1".into(),
            stream: OutputStream::Stdout,
            data: "aGVsbG8=".into(),
            encoding: "raw+base64".into(),
            seq: 0,
        };
        let json = serde_json::to_string(&out).unwrap();
        assert!(json.contains(r#""stream":"stdout"#));
        let parsed: OutputEvent = serde_json::from_str(&json).unwrap();
        assert_eq!(parsed.stream, OutputStream::Stdout);
    }

    #[test]
    fn output_event_supports_stderr() {
        let out = OutputEvent {
            uuid: "test-1".into(),
            stream: OutputStream::Stderr,
            data: "ZXJyb3I=".into(),
            encoding: "raw+base64".into(),
            seq: 1,
        };
        let json = serde_json::to_string(&out).unwrap();
        assert!(json.contains(r#""stream":"stderr"#));
    }
}
```

Write similar round-trip + rejection tests for: ResultEvent, HostTelemetryEvent, SecretRequestEvent, SecretResponseEvent. Every event must have a round-trip test AND an invalid-JSON test.

**Step 3: Run to confirm failures**

Run: `cargo test -p mxdx-types`
Expected: FAIL — types not defined yet.

**Step 4: Commit test stubs**

```bash
git add crates/mxdx-types/
git commit -m "test(types): add failing round-trip tests for core event types"
```

---

### Task 2.1 [C]: Core Event Type Implementation

**Files:**
- Modify: `crates/mxdx-types/src/events/command.rs` (add structs)
- Modify: `crates/mxdx-types/src/events/output.rs` (add structs)
- Modify: `crates/mxdx-types/src/events/result.rs`
- Modify: `crates/mxdx-types/src/events/telemetry.rs`
- Modify: `crates/mxdx-types/src/events/secret.rs`
- Modify: `crates/mxdx-types/src/lib.rs` (add mod events)

**Step 1: Implement types according to spec**

Reference `docs/mxdx-architecture.md` Section 5 and `docs/mxdx-management-console.md` Section 4 for exact field definitions.

Key types:
- `CommandEvent`: uuid, action (enum: Exec, Kill, Signal), cmd, args, env, cwd, timeout_seconds
- `OutputEvent`: uuid, stream (enum: Stdout, Stderr), data, encoding, seq
- `ResultEvent`: uuid, status (enum: Exit, Error, Timeout, Killed), exit_code, summary
- `HostTelemetryEvent`: timestamp, hostname, os, arch, uptime_seconds, load_avg, cpu, memory, disk, network, services, devices
- `SecretRequestEvent`: request_id, scope, ttl_seconds, reason
- `SecretResponseEvent`: request_id, granted, value (Option), error (Option)

All types: `#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]`
Use `#[serde(rename_all = "snake_case")]` for enums.

**Step 2: Run tests**

Run: `cargo test -p mxdx-types`
Expected: PASS — all round-trip tests green.

**Step 3: Run xtask**

Run: `cargo xtask manifest`

**Step 4: Commit**

```bash
git add crates/mxdx-types/ MANIFEST.md
git commit -m "feat(types): implement core event types with serde round-trip"
```

---

### Task 2.2 [T]: Terminal Event Type Tests

**Files:**
- Create: `crates/mxdx-types/src/events/terminal.rs` (tests)
- Create: `crates/mxdx-types/src/events/launcher.rs` (tests)

Events to test (from spec Section 4):
- `TerminalDataEvent`: data, encoding, seq (seq is u64 — mxdx-seq)
- `TerminalResizeEvent`: cols, rows
- `TerminalSessionRequestEvent`: uuid, command, env, cols, rows
- `TerminalSessionResponseEvent`: uuid, status, room_id (Option)
- `TerminalRetransmitEvent`: from_seq (u64), to_seq (u64)
- `LauncherIdentityEvent`: launcher_id, accounts (Vec), primary, capabilities, version

Every type: round-trip test + invalid JSON test + missing required field test.

**Security test (mxdx-seq):**
```rust
#[test]
fn seq_field_is_u64_and_handles_large_values() {
    let event = TerminalDataEvent {
        data: "dGVzdA==".into(),
        encoding: "raw+base64".into(),
        seq: u64::MAX,
    };
    let json = serde_json::to_string(&event).unwrap();
    let parsed: TerminalDataEvent = serde_json::from_str(&json).unwrap();
    assert_eq!(parsed.seq, u64::MAX);
}
```

Run: `cargo test -p mxdx-types`
Expected: FAIL — terminal types not defined.

---

### Task 2.2 [C]: Terminal Event Type Implementation

Implement all terminal and launcher event types. Same pattern as 2.1 [C].

Run: `cargo test -p mxdx-types`
Expected: PASS.

---

### Task 2.3 [T]+[C]: TypeScript Type Definitions

**Files:**
- Create: `client/mxdx-client/src/types/events.ts`
- Create: `client/mxdx-client/src/types/index.ts`
- Create: `client/mxdx-client/tests/event-types.test.ts`

Write hand-crafted TypeScript types matching Rust 1:1. Use Zod schemas for runtime validation. Write Vitest tests that parse sample JSON and validate against Zod schemas.

The `stream` field must be `"stdout" | "stderr"` in TypeScript, matching the Rust `OutputStream` enum.

Run: `cd client && npm test`
Expected: PASS.

---

### Task 2.4 [CI]: Add Type Test Jobs

**Files:**
- Modify: `.github/workflows/ci.yml`

Add jobs:
```yaml
  types-test:
    needs: build
    runs-on: ubuntu-latest
    steps:
      - uses: actions/checkout@v4
      - uses: dtolnay/rust-toolchain@stable
      - uses: Swatinem/rust-cache@v2
      - name: Install system deps
        run: sudo apt-get install -y libsqlite3-dev libssl-dev
      - run: cargo test -p mxdx-types

  client-test:
    needs: client-build
    runs-on: ubuntu-latest
    steps:
      - uses: actions/checkout@v4
      - uses: actions/setup-node@v4
        with:
          node-version: "22"
      - run: cd client && npm ci && npm test
```

---

## Phase 3: Test Infrastructure

> **Epic:** Real Tuwunel test helpers, proven by actually starting tuwunel.
> **Agents:** Tester (writes helpers + tests), DevOps (CI integration).
> **Completion gate:** `TuwunelInstance::start().await` starts a real tuwunel, health check passes, user registration works. `FederatedPair::start().await` starts two federated instances.
> **Branch:** `feat/phase-3-test-infra`
> **CI update:** Add integration test job with tuwunel installed.
> **Security:** All ports OS-assigned (mxdx-ji1). No hardcoded ports anywhere.

### Task 3.1 [T]+[C]: TuwunelInstance Helper

**Files:**
- Create: `tests/helpers/Cargo.toml`
- Create: `tests/helpers/src/lib.rs`
- Create: `tests/helpers/src/tuwunel.rs`
- Create: `tests/helpers/src/matrix_client.rs`

**CRITICAL SECURITY (mxdx-ji1):** All ports MUST be OS-assigned. `TuwunelInstance::start()` takes NO port argument. It binds port 0 and reports the actual port.

**Step 1: Write Cargo.toml for test helpers**

```toml
[package]
name = "mxdx-test-helpers"
version = "0.1.0"
edition = "2021"

[dependencies]
tokio = { workspace = true }
anyhow = { workspace = true }
reqwest = { workspace = true }
tempfile = { workspace = true }
serde = { workspace = true }
serde_json = { workspace = true }
```

Add `"tests/helpers"` to workspace members in root Cargo.toml.

**Step 2: Write failing test**

```rust
// tests/helpers/src/tuwunel.rs
#[cfg(test)]
mod tests {
    use super::*;

    #[tokio::test]
    async fn tuwunel_starts_and_responds_to_health_check() {
        let instance = TuwunelInstance::start().await.unwrap();
        let resp = reqwest::get(format!(
            "http://127.0.0.1:{}/_matrix/client/versions",
            instance.port
        ))
        .await
        .unwrap();
        assert!(resp.status().is_success());
        instance.stop().await;
    }

    #[tokio::test]
    async fn tuwunel_can_register_user() {
        let instance = TuwunelInstance::start().await.unwrap();
        let client = instance.register_user("testuser", "testpass").await.unwrap();
        assert!(!client.access_token.is_empty());
        instance.stop().await;
    }
}
```

Run: `cargo test -p mxdx-test-helpers`
Expected: FAIL — TuwunelInstance not defined.

**Step 3: Implement TuwunelInstance**

REFERENCE: `docs/adr/2026-03-05-tuwunel-ground-truth.md` for config format and CLI flags.

Key implementation details:
- Pick free port by binding `127.0.0.1:0`, reading assigned port, then dropping the listener
- Create TempDir for data
- Write config using the format documented in the ADR
- Spawn tuwunel process
- Wait for health check (poll `/_matrix/client/versions` up to 30s)
- `register_user()` uses Matrix client API `POST /_matrix/client/v3/register`
- `stop()` kills the process
- `Drop` impl kills the process (cleanup safety net)

```rust
pub struct TuwunelInstance {
    pub port: u16,
    process: std::process::Child,
    _data_dir: tempfile::TempDir,
}
```

**Step 4: Run tests**

Run: `cargo test -p mxdx-test-helpers`
Expected: PASS — both tests green (requires tuwunel installed).

**Step 5: Commit**

```bash
git add tests/helpers/ Cargo.toml
git commit -m "test(infra): add TuwunelInstance helper with OS-assigned ports (mxdx-ji1)"
```

---

### Task 3.2 [T]+[C]: FederatedPair Helper

**Files:**
- Create: `tests/helpers/src/federation.rs`

**Step 1: Write failing federation test**

```rust
#[cfg(test)]
mod tests {
    use super::*;

    #[tokio::test]
    async fn two_instances_can_federate() {
        let pair = FederatedPair::start().await.unwrap();
        let user_a = pair.hs_a.register_user("alice", "pass").await.unwrap();
        let user_b = pair.hs_b.register_user("bob", "pass").await.unwrap();

        let room_id = user_a.create_room().await.unwrap();
        user_a.invite(&room_id, &user_b.mxid()).await.unwrap();

        let invite = user_b.wait_for_invite(
            &room_id,
            std::time::Duration::from_secs(10),
        ).await;
        assert!(invite.is_ok(), "Federation invite not received: {:?}", invite);

        pair.stop().await;
    }
}
```

**Step 2: Implement FederatedPair**

REFERENCE: `docs/adr/2026-03-05-tuwunel-ground-truth.md` Section 6 (Federation setup).

Two TuwunelInstance's with federation config pointing at each other. May require TLS — use mkcert if the ADR indicates TLS is required.

**Step 3: Run tests, commit**

---

### Task 3.3 [CI]: Add Integration Test Job

**Files:**
- Modify: `.github/workflows/ci.yml`

Add job that installs tuwunel and runs test helper tests:
```yaml
  integration:
    needs: build
    runs-on: ubuntu-latest
    steps:
      - uses: actions/checkout@v4
      - uses: dtolnay/rust-toolchain@stable
      - uses: Swatinem/rust-cache@v2
      - name: Install system deps
        run: sudo apt-get install -y libsqlite3-dev libssl-dev
      - name: Install tuwunel
        run: |
          # Use method documented in docs/adr/2026-03-05-tuwunel-ground-truth.md
          # Example (adjust based on ADR findings):
          curl -fsSL https://github.com/matrix-construct/tuwunel/releases/latest/download/tuwunel-x86_64-v1-linux-gnu.deb \
            -o /tmp/tuwunel.deb && sudo dpkg -i /tmp/tuwunel.deb
      - run: cargo test -p mxdx-test-helpers
```

---

## Phase 4: Matrix Client Facade

> **Epic:** `mxdx-matrix` — the single facade all Rust code uses for Matrix operations.
> **No crate except `mxdx-matrix` may import `matrix-sdk` directly.**
> **Completion gate:** Can connect to local Tuwunel, create encrypted room, send/receive typed events, create DMs.
> **Branch:** `feat/phase-4-matrix-client`
> **CI update:** Add `cargo test -p mxdx-matrix` job.

### Task 4.1 [T]: MatrixClient Connect + E2EE Tests

**Files:**
- Create: `crates/mxdx-matrix/src/client.rs`
- Create: `crates/mxdx-matrix/src/error.rs`
- Create: `crates/mxdx-matrix/tests/connect.rs`

**Step 1: Write integration tests**

```rust
// crates/mxdx-matrix/tests/connect.rs
use mxdx_test_helpers::TuwunelInstance;

#[tokio::test]
async fn client_connects_and_initializes_crypto() {
    let hs = TuwunelInstance::start().await.unwrap();
    let client = mxdx_matrix::MatrixClient::register_and_connect(
        &format!("http://127.0.0.1:{}", hs.port),
        "testbot",
        "password123",
    ).await.unwrap();

    assert!(client.is_logged_in());
    assert!(client.crypto_enabled());
    hs.stop().await;
}

#[tokio::test]
async fn two_clients_exchange_encrypted_event() {
    let hs = TuwunelInstance::start().await.unwrap();
    let alice = mxdx_matrix::MatrixClient::register_and_connect(
        &format!("http://127.0.0.1:{}", hs.port), "alice", "pass",
    ).await.unwrap();
    let bob = mxdx_matrix::MatrixClient::register_and_connect(
        &format!("http://127.0.0.1:{}", hs.port), "bob", "pass",
    ).await.unwrap();

    let room_id = alice.create_encrypted_room(&[bob.user_id()]).await.unwrap();
    bob.join_room(&room_id).await.unwrap();

    // Exchange keys
    alice.sync_once().await.unwrap();
    bob.sync_once().await.unwrap();

    let payload = serde_json::json!({"type": "org.mxdx.command", "content": {"uuid": "test-1"}});
    alice.send_event(&room_id, payload.clone()).await.unwrap();

    let events = bob.sync_and_collect_events(
        &room_id,
        std::time::Duration::from_secs(5),
    ).await.unwrap();
    assert!(events.iter().any(|e| e["content"]["uuid"] == "test-1"));
    hs.stop().await;
}
```

Run: `cargo test -p mxdx-matrix`
Expected: FAIL — MatrixClient not defined.

---

### Task 4.1 [C]: MatrixClient Implementation

**Files:**
- Implement: `crates/mxdx-matrix/src/client.rs`
- Implement: `crates/mxdx-matrix/src/error.rs`
- Modify: `crates/mxdx-matrix/src/lib.rs`

Wrap `matrix-sdk` 0.16. Key methods:
- `register_and_connect(homeserver_url, username, password) -> Result<Self>`
- `is_logged_in() -> bool`
- `crypto_enabled() -> bool`
- `user_id() -> OwnedUserId`
- `create_encrypted_room(invite: &[OwnedUserId]) -> Result<OwnedRoomId>`
- `create_dm(user_id: &UserId) -> Result<OwnedRoomId>` — for interactive sessions
- `join_room(room_id) -> Result<()>`
- `send_event(room_id, content: Value) -> Result<()>`
- `send_state_event(room_id, event_type, state_key, content) -> Result<()>`
- `sync_once() -> Result<()>`
- `sync_and_collect_events(room_id, timeout) -> Result<Vec<Value>>`

Store path uses tempdir internally for tests (avoid path conflicts).

---

### Task 4.2 [T]+[C]: Room Topology Helpers

**Files:**
- Create: `crates/mxdx-matrix/src/rooms.rs`

Helpers for the room topology from spec Section 3:
- `create_launcher_space(launcher_id)` -> Space + exec room (E2EE + MSC4362 encrypted state) + logs room (E2EE)
- `create_terminal_session_dm(user_id)` -> DM room with history_visibility=joined in initial_state (mxdx-aew)
- `tombstone_room(room_id, replacement)`

Every helper tested against real Tuwunel.

**Security test (mxdx-aew):**
```rust
#[tokio::test]
async fn terminal_dm_has_joined_history_visibility_from_creation() {
    let hs = TuwunelInstance::start().await.unwrap();
    let launcher = mxdx_matrix::MatrixClient::register_and_connect(
        &format!("http://127.0.0.1:{}", hs.port), "launcher", "pass",
    ).await.unwrap();
    let user = mxdx_matrix::MatrixClient::register_and_connect(
        &format!("http://127.0.0.1:{}", hs.port), "user", "pass",
    ).await.unwrap();

    let room_id = launcher.create_terminal_session_dm(user.user_id()).await.unwrap();

    // Verify history_visibility is set in the room state
    let state = launcher.get_room_state(&room_id, "m.room.history_visibility").await.unwrap();
    assert_eq!(state["history_visibility"], "joined");

    hs.stop().await;
}
```

---

### Task 4.3 [CI]+[D]: Phase 4 CI and Docs

Add `cargo test -p mxdx-matrix` to CI (requires tuwunel). Write phase summary.
## Phase 5: Launcher v1 — Non-Interactive Sessions

> **Epic:** Core launcher: Matrix identity, command execution with security, output streaming with stdout/stderr separation, basic telemetry.
> **Agents:** Tester -> Coder.
> **Completion gate:** Orchestrator sends `org.mxdx.command` -> launcher executes -> streams output as threaded replies with separate stdout/stderr -> sends `org.mxdx.result`. Telemetry state events readable.
> **Branch:** `feat/phase-5-launcher-v1`
> **CI update:** Add `cargo test -p mxdx-launcher --lib` job.
> **Security:** cwd validation (mxdx-71v), argument injection (mxdx-jjf), config permissions (mxdx-cfg), telemetry levels (mxdx-tel).

### Task 5.1 [T]: Launcher Config Tests

**Files:**
- Create: `crates/mxdx-launcher/src/config.rs` (tests only first)

**Step 1: Write tests**

```rust
#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn valid_config_parses() {
        let toml = r#"
            [global]
            launcher_id = "belthanior"
            data_dir = "/tmp/mxdx"

            [[homeservers]]
            url = "https://hs1.example.com"
            username = "launcher-1"
            password = "secret"

            [capabilities]
            mode = "allowlist"
            allowed_commands = ["cargo", "git", "npm"]
            allowed_cwd_prefixes = ["/workspace"]
            max_sessions = 10

            [telemetry]
            detail_level = "full"
            poll_interval_seconds = 30
        "#;
        let config: LauncherConfig = toml::from_str(toml).unwrap();
        assert_eq!(config.global.launcher_id, "belthanior");
        assert_eq!(config.capabilities.mode, CapabilityMode::Allowlist);
        assert_eq!(config.capabilities.allowed_cwd_prefixes, vec!["/workspace"]);
    }

    #[test]
    fn invalid_config_fails_fast() {
        let toml = r#"
            [global]
            launcher_id = ""
        "#;
        let result: Result<LauncherConfig, _> = toml::from_str(toml);
        assert!(result.is_err());
    }

    #[test]
    fn config_supports_telemetry_detail_levels() {
        // mxdx-tel: configurable telemetry detail
        let full_toml = r#"
            [global]
            launcher_id = "test"
            data_dir = "/tmp"
            [[homeservers]]
            url = "https://hs.example.com"
            username = "u"
            password = "p"
            [telemetry]
            detail_level = "summary"
        "#;
        let config: LauncherConfig = toml::from_str(full_toml).unwrap();
        assert_eq!(config.telemetry.detail_level, TelemetryDetail::Summary);
    }
}
```

Run: `cargo test -p mxdx-launcher -- config`
Expected: FAIL — types not defined.

---

### Task 5.1 [C]: Launcher Config Implementation

**Files:**
- Implement: `crates/mxdx-launcher/src/config.rs`
- Create: `crates/mxdx-launcher/src/identity.rs`

Config parsed from TOML. Validate on startup. On invalid config: log JSON error and exit 1.

**Security (mxdx-cfg):** On startup, check config file permissions. Warn if group or world readable.

```rust
pub fn validate_config_permissions(path: &Path) -> Result<()> {
    use std::os::unix::fs::PermissionsExt;
    let metadata = std::fs::metadata(path)?;
    let mode = metadata.permissions().mode();
    if mode & 0o077 != 0 {
        tracing::warn!(
            path = %path.display(),
            mode = format!("{:04o}", mode & 0o777),
            "Config file is readable by group or others. Recommended: chmod 0600"
        );
    }
    Ok(())
}
```

Identity module: Matrix registration/login. First run registers, subsequent runs login with stored credentials.

---

### Task 5.2 [T]: Command Execution Security Tests

**Files:**
- Create: `crates/mxdx-launcher/src/executor.rs` (tests only)

**Security tests (mxdx-71v, mxdx-jjf):**

```rust
#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn command_on_allowlist_is_permitted() {
        let config = test_config_with_allowlist(&["cargo", "git"]);
        let result = validate_command(&config, "cargo", &["build"], Some("/workspace"));
        assert!(result.is_ok());
    }

    #[test]
    fn command_not_on_allowlist_is_rejected() {
        let config = test_config_with_allowlist(&["cargo"]);
        let result = validate_command(&config, "rm", &["-rf", "/"], Some("/workspace"));
        assert!(result.is_err());
        assert!(result.unwrap_err().to_string().contains("not permitted"));
    }

    // mxdx-71v: cwd validation
    #[test]
    fn test_security_cwd_outside_prefix_is_rejected() {
        let config = test_config_with_cwd_prefixes(&["/workspace"]);
        let result = validate_command(&config, "cargo", &["build"], Some("/etc"));
        assert!(result.is_err());
        assert!(result.unwrap_err().to_string().contains("cwd not permitted"));
    }

    #[test]
    fn test_security_cwd_traversal_rejected() {
        let config = test_config_with_cwd_prefixes(&["/workspace"]);
        let result = validate_command(&config, "cargo", &["build"], Some("/workspace/../../etc"));
        assert!(result.is_err());
    }

    #[test]
    fn test_security_cwd_none_uses_default() {
        let config = test_config_with_cwd_prefixes(&["/workspace"]);
        let result = validate_command(&config, "cargo", &["build"], None);
        // None cwd should use a safe default, not reject
        assert!(result.is_ok());
    }

    // mxdx-jjf: argument injection
    #[test]
    fn test_security_git_dash_c_blocked() {
        let config = test_config_with_allowlist(&["git"]);
        let result = validate_command(&config, "git", &["-c", "core.pager=evil", "log"], Some("/workspace"));
        assert!(result.is_err());
        assert!(result.unwrap_err().to_string().contains("argument not permitted"));
    }

    #[test]
    fn test_security_git_submodule_foreach_blocked() {
        let config = test_config_with_allowlist(&["git"]);
        let result = validate_command(&config, "git", &["submodule", "foreach", "evil"], Some("/workspace"));
        assert!(result.is_err());
    }

    #[test]
    fn test_security_docker_compose_dash_f_blocked() {
        let config = test_config_with_allowlist(&["docker"]);
        let result = validate_command(
            &config,
            "docker",
            &["compose", "-f", "/tmp/evil.yml", "up"],
            Some("/workspace"),
        );
        assert!(result.is_err());
    }

    #[test]
    fn test_security_env_prefix_injection_blocked() {
        let config = test_config_with_allowlist(&["cargo"]);
        let result = validate_command(
            &config,
            "env",
            &["MALICIOUS=true", "cargo", "build"],
            Some("/workspace"),
        );
        assert!(result.is_err());
    }
}
```

Run: `cargo test -p mxdx-launcher -- test_security`
Expected: FAIL — validate_command not defined.

---

### Task 5.2 [C]: Command Execution Implementation

**Files:**
- Implement: `crates/mxdx-launcher/src/executor.rs`

**Implementation requirements:**

1. `validate_command(config, cmd, args, cwd)` — validates before execution:
   - Check cmd against allowlist
   - Canonicalize cwd path, check against allowed_cwd_prefixes (mxdx-71v)
   - Check args against per-command deny patterns (mxdx-jjf):
     - git: deny `-c`, `--config`, `submodule foreach`
     - docker: deny `compose -f`, `compose --file` with paths outside cwd
     - General: deny `env` prefix injection
   - Return descriptive error on rejection

2. `execute_command(config, cmd_event)` — runs the command:
   - Use `std::process::Command::new(cmd).args(args)` — NEVER shell interpolation
   - Set cwd via `.current_dir()`
   - Capture stdout and stderr separately
   - Stream output as `OutputEvent` with `stream: Stdout` or `stream: Stderr`
   - Output events are threaded replies (m.relates_to.rel_type = m.thread) to the command event
   - Include `seq` field starting at 0, incrementing per chunk
   - On completion, send `ResultEvent`

Run: `cargo test -p mxdx-launcher`
Expected: PASS — all security tests green.

---

### Task 5.3 [T]+[C]: Output Streaming E2E

**Files:**
- Create: `crates/mxdx-launcher/tests/e2e_command.rs`

**E2E test:**

```rust
#[tokio::test]
async fn launcher_executes_command_and_streams_output() {
    let hs = TuwunelInstance::start().await.unwrap();
    let orchestrator = mxdx_matrix::MatrixClient::register_and_connect(
        &format!("http://127.0.0.1:{}", hs.port), "orchestrator", "pass",
    ).await.unwrap();

    // Start a test launcher
    let launcher = start_test_launcher(&hs, &["echo", "seq"]).await;
    let room = orchestrator.create_encrypted_room(&[launcher.user_id()]).await.unwrap();
    launcher.join_room(&room).await.unwrap();
    orchestrator.sync_once().await.unwrap();
    launcher.sync_once().await.unwrap();

    // Send command
    let cmd = CommandEvent {
        uuid: uuid::Uuid::new_v4().to_string(),
        action: CommandAction::Exec,
        cmd: "echo".into(),
        args: vec!["hello-world".into()],
        env: Default::default(),
        cwd: None,
        timeout_seconds: Some(10),
    };
    orchestrator.send_command(&room, &cmd).await.unwrap();

    // Wait for result
    let result = wait_for_result(&room, &cmd.uuid, Duration::from_secs(10)).await.unwrap();
    assert_eq!(result.status, ResultStatus::Exit);
    assert_eq!(result.exit_code, Some(0));

    // Verify output events have stream labels
    let outputs = collect_output_events(&room, &cmd.uuid, Duration::from_secs(5)).await;
    assert!(outputs.iter().any(|o| o.stream == OutputStream::Stdout));

    hs.stop().await;
}

#[tokio::test]
async fn command_output_separates_stdout_and_stderr() {
    let hs = TuwunelInstance::start().await.unwrap();
    // Use a command that writes to both stdout and stderr
    // e.g., `sh -c 'echo out; echo err >&2'`
    // Verify output events have both Stdout and Stderr stream values
}

#[tokio::test]
async fn large_output_streams_in_order() {
    // Run `seq 1 100` — verify all 100 lines arrive, in order by seq field
}
```

---

### Task 5.4 [T]+[C]: Basic Telemetry

**Files:**
- Create: `crates/mxdx-launcher/src/telemetry/mod.rs`
- Create: `crates/mxdx-launcher/src/telemetry/system.rs`

Collect and publish `org.mxdx.host_telemetry` as state event. Use `sysinfo` crate.

**Security (mxdx-tel):** Respect `detail_level` from config:
- `full`: all fields (cpu, memory, disk, network, services, devices)
- `summary`: only hostname, os, arch, uptime, load_avg, basic cpu/memory percentages

```rust
#[test]
fn telemetry_summary_mode_excludes_detailed_fields() {
    let telemetry = collect_telemetry(TelemetryDetail::Summary);
    assert!(telemetry.network.is_none());
    assert!(telemetry.services.is_none());
    assert!(telemetry.devices.is_none());
}

#[test]
fn telemetry_full_mode_includes_all_fields() {
    let telemetry = collect_telemetry(TelemetryDetail::Full);
    assert!(telemetry.network.is_some());
    assert!(telemetry.cpu.model.is_some());
}
```

---

### Task 5.5 [S]: Phase 5 Security Review

**Role:** Security Reviewer
**Output:** `docs/reports/security/2026-03-05-phase-5-review.md`

Review checklist:
- [ ] Command execution uses `Command::new(cmd).args(args)`, never shell interpolation
- [ ] cwd validation: canonicalized path checked against allowed prefixes (mxdx-71v)
- [ ] Argument injection: git -c, docker -f, env prefix all blocked (mxdx-jjf)
- [ ] Config file permissions checked on startup (mxdx-cfg)
- [ ] Telemetry detail levels configurable (mxdx-tel)
- [ ] No secrets in log output
- [ ] Allowlist cannot be bypassed via path traversal

Write adversarial variants for mxdx-71v and mxdx-jjf.

---

## Phase 6: Terminal — Interactive Sessions

> **Epic:** PTY bridge, tmux integration, adaptive compression, DM-based session lifecycle.
> **Agents:** Tester -> Coder + Security Reviewer.
> **Completion gate:** Full interactive terminal round-trip: user posts session request in room -> launcher creates DM -> PTY bridged to tmux -> terminal.data events flow bidirectionally in DM.
> **Branch:** `feat/phase-6-terminal`
> **CI update:** Add terminal integration tests (requires tmux in CI).
> **Security:** history_visibility via initial_state (mxdx-aew), zlib bomb protection (mxdx-ccx), seq as u64 (mxdx-seq).

### Task 6.1 [T]: PTY/tmux Integration Tests

**Files:**
- Create: `crates/mxdx-launcher/src/terminal/mod.rs`
- Create: `crates/mxdx-launcher/src/terminal/pty.rs` (tests)
- Create: `crates/mxdx-launcher/src/terminal/tmux.rs` (tests)

```rust
#[tokio::test]
async fn tmux_session_creates_and_captures_output() {
    let session = TmuxSession::create("test-pty", "/bin/bash", 80, 24).await.unwrap();
    session.send_input("echo hello-tmux\n").await.unwrap();

    let output = session.capture_pane_until(
        "hello-tmux",
        Duration::from_secs(2),
    ).await.unwrap();
    assert!(output.contains("hello-tmux"));

    session.kill().await.unwrap();
}

#[tokio::test]
async fn tmux_session_name_validated() {
    // Only [a-zA-Z0-9_-] allowed
    let result = TmuxSession::create("../../evil", "/bin/bash", 80, 24).await;
    assert!(result.is_err());
}

#[tokio::test]
async fn tmux_session_resize_works() {
    let session = TmuxSession::create("test-resize", "/bin/bash", 80, 24).await.unwrap();
    session.resize(120, 40).await.unwrap();
    // Verify by checking tmux display-message
    session.kill().await.unwrap();
}
```

---

### Task 6.1 [C]: PTY/tmux Implementation

**Files:**
- Implement: `crates/mxdx-launcher/src/terminal/pty.rs`
- Implement: `crates/mxdx-launcher/src/terminal/tmux.rs`

`TmuxSession`:
- `create(name, command, cols, rows)` — validates name (regex: `[a-zA-Z0-9_-]+`), spawns detached tmux session
- `send_input(data)` — writes to tmux via `tmux send-keys`
- `capture_pane()` — reads tmux pane content
- `resize(cols, rows)` — `tmux resize-window`
- `kill()` — `tmux kill-session`

SECURITY: Never shell-exec user strings. Use `Command::new("tmux").args(...)`.

---

### Task 6.2 [T]: Adaptive Compression Tests

**Files:**
- Create: `crates/mxdx-launcher/src/terminal/compression.rs` (tests)

```rust
#[test]
fn small_payload_uses_raw_base64() {
    let data = b"hi"; // 2 bytes
    let (encoded, encoding) = compress_encode(data);
    assert_eq!(encoding, "raw+base64");
    let decoded = decode_decompress_bounded(&encoded, &encoding, 1_048_576).unwrap();
    assert_eq!(decoded, data);
}

#[test]
fn large_payload_uses_zlib_base64() {
    let data = vec![b'x'; 100];
    let (encoded, encoding) = compress_encode(&data);
    assert_eq!(encoding, "zlib+base64");
    let decoded = decode_decompress_bounded(&encoded, &encoding, 1_048_576).unwrap();
    assert_eq!(decoded, data);
}

#[test]
fn boundary_at_32_bytes_uses_zlib() {
    let data = vec![b'a'; 32];
    let (_, encoding) = compress_encode(&data);
    assert_eq!(encoding, "zlib+base64");
}

// mxdx-ccx: zlib bomb protection
#[test]
fn test_security_zlib_bomb_rejected_before_pty_write() {
    let bomb_data = vec![b'a'; 2 * 1024 * 1024]; // 2MB
    let (encoded, encoding) = compress_encode(&bomb_data);
    let result = decode_decompress_bounded(&encoded, &encoding, 1_048_576); // 1MB limit
    assert!(result.is_err(), "zlib bomb should be rejected");
}

#[test]
fn test_security_decompression_streams_and_fails_fast() {
    // Verify that decode_decompress_bounded doesn't allocate the full
    // decompressed size before checking — it should fail during streaming
    let bomb_data = vec![b'a'; 5 * 1024 * 1024]; // 5MB
    let (encoded, encoding) = compress_encode(&bomb_data);
    let start = std::time::Instant::now();
    let _ = decode_decompress_bounded(&encoded, &encoding, 1_048_576);
    // Should fail fast, not after decompressing 5MB
    assert!(start.elapsed() < Duration::from_millis(100));
}
```

---

### Task 6.2 [C]: Adaptive Compression Implementation

**Files:**
- Implement: `crates/mxdx-launcher/src/terminal/compression.rs`

`compress_encode(data: &[u8]) -> (String, String)`:
- < 32 bytes: base64 encode, return ("raw+base64")
- >= 32 bytes: zlib compress then base64 encode, return ("zlib+base64")

`decode_decompress_bounded(encoded: &str, encoding: &str, max_bytes: usize) -> Result<Vec<u8>>`:
- **mxdx-ccx:** Use streaming flate2 decompressor. Read in chunks. Count output bytes. If output exceeds `max_bytes`, return error immediately. Do NOT decompress fully then check size.

---

### Task 6.3 [T]+[C]: PTY Batching + Ring Buffer

**Files:**
- Create: `crates/mxdx-launcher/src/terminal/batcher.rs`
- Create: `crates/mxdx-launcher/src/terminal/ring_buffer.rs`

Batcher: accumulates PTY output for 15ms or until 4KB, whichever first.
Ring buffer: last 1000 events per session. O(1) lookup by seq range for retransmit.

Tests:
- Batcher holds items for 15ms then flushes
- Batcher flushes early at 4KB
- RingBuffer stores and retrieves by seq range
- RingBuffer evicts oldest when full

---

### Task 6.4 [T]: DM-Based Session Lifecycle Tests

**Files:**
- Create: `crates/mxdx-launcher/tests/e2e_terminal_session.rs`

```rust
#[tokio::test]
async fn full_interactive_terminal_round_trip() {
    let hs = TuwunelInstance::start().await.unwrap();
    let user = mxdx_matrix::MatrixClient::register_and_connect(
        &format!("http://127.0.0.1:{}", hs.port), "user", "pass",
    ).await.unwrap();
    let launcher = start_test_launcher_with_terminal(&hs).await;

    let exec_room = create_exec_room(&user, &launcher).await;

    // Request interactive session
    let req_uuid = uuid::Uuid::new_v4().to_string();
    user.send_event(&exec_room, TerminalSessionRequestEvent {
        uuid: req_uuid.clone(),
        command: "/bin/bash".into(),
        env: [("TERM".into(), "xterm-256color".into())].into(),
        cols: 80,
        rows: 24,
    }.into()).await.unwrap();

    // Launcher should create a DM with the user
    let response = wait_for_event::<TerminalSessionResponseEvent>(
        &exec_room, &req_uuid, Duration::from_secs(10),
    ).await.unwrap();
    assert_eq!(response.status, "created");
    let dm_room_id = response.room_id.unwrap();

    // User should receive a DM invite
    let invite = user.wait_for_invite(&dm_room_id, Duration::from_secs(5)).await.unwrap();
    user.join_room(&dm_room_id).await.unwrap();

    // Send input via DM
    user.send_event(&dm_room_id, TerminalDataEvent {
        data: base64_encode("echo hello-from-matrix\n"),
        encoding: "raw+base64".into(),
        seq: 0,
    }.into()).await.unwrap();

    // Wait for output in DM
    let output_events = collect_terminal_data_events(&dm_room_id, Duration::from_secs(5)).await;
    let all_output: String = output_events.iter()
        .map(|e| decode_event_data(e))
        .collect();
    assert!(all_output.contains("hello-from-matrix"));

    hs.stop().await;
}

// mxdx-aew: history visibility
#[tokio::test]
async fn test_security_late_joiner_cannot_read_pre_join_history() {
    let hs = TuwunelInstance::start().await.unwrap();
    let user = mxdx_matrix::MatrixClient::register_and_connect(
        &format!("http://127.0.0.1:{}", hs.port), "user", "pass",
    ).await.unwrap();
    let launcher = start_test_launcher_with_terminal(&hs).await;
    let late_joiner = mxdx_matrix::MatrixClient::register_and_connect(
        &format!("http://127.0.0.1:{}", hs.port), "late", "pass",
    ).await.unwrap();

    // Create session, send some data
    let dm_room_id = create_terminal_session(&user, &launcher).await;
    user.send_event(&dm_room_id, terminal_data_event("secret-output")).await.unwrap();

    // Late joiner joins after data was sent
    launcher.invite_to_room(&dm_room_id, late_joiner.user_id()).await.unwrap();
    late_joiner.join_room(&dm_room_id).await.unwrap();

    // Late joiner should NOT see pre-join messages
    let events = late_joiner.sync_and_collect_events(
        &dm_room_id, Duration::from_secs(3),
    ).await.unwrap();
    assert!(
        !events.iter().any(|e| {
            e.get("content")
                .and_then(|c| c.get("data"))
                .and_then(|d| d.as_str())
                .map(|s| decode_base64(s).contains("secret-output"))
                .unwrap_or(false)
        }),
        "Late joiner should not see pre-join terminal history"
    );

    hs.stop().await;
}
```

---

### Task 6.4 [C]: Session Lifecycle Implementation

**Files:**
- Create: `crates/mxdx-launcher/src/terminal/session.rs`

When a `TerminalSessionRequestEvent` is received:
1. Validate command against allowlist
2. Create a tmux session
3. **Create DM** with the requesting user via `MatrixClient::create_terminal_session_dm()` — sets `history_visibility = joined` in `initial_state` (mxdx-aew)
4. Set power levels: launcher at 100, user at 50
5. Send `TerminalSessionResponseEvent` to the exec room with the DM room_id
6. Bridge PTY output from tmux to DM as `TerminalDataEvent`
7. Bridge input from DM to tmux stdin

---

### Task 6.5 [T]+[C]: Launcher Crash Recovery

On restart:
1. Reconnect to homeserver
2. List existing tmux sessions via `tmux list-sessions`
3. Match session names to known terminal DM rooms (stored in launcher state file)
4. Re-attach PTY bridges

Test: Kill launcher while session active. Restart. Verify PTY bridge resumes.

---

### Task 6.6 [S]: Phase 6 Security Review

**Output:** `docs/reports/security/2026-03-05-phase-6-review.md`

Checklist:
- [ ] PTY bytes from Matrix go to tmux stdin — no shell interpolation (mxdx-8bm)
- [ ] Command validated against allowlist before session creation
- [ ] Power levels set: only launcher sends data events to DM
- [ ] history_visibility = joined in initial_state, verified by test (mxdx-aew)
- [ ] Zlib bomb rejected before PTY write, streaming check (mxdx-ccx)
- [ ] seq is u64, tested with u64::MAX (mxdx-seq)
- [ ] tmux session names validated: [a-zA-Z0-9_-]+

Adversarial variants for mxdx-aew, mxdx-ccx, mxdx-8bm.

---

## Phase 7: Browser Client

> **Epic:** Full TerminalSocket implementation, xterm.js integration, launcher discovery.
> **Agents:** Tester -> Coder.
> **Completion gate:** xterm.js AttachAddon binds to TerminalSocket, sends/receives PTY data over Matrix DMs.
> **Branch:** `feat/phase-7-browser-client`
> **CI update:** Add browser client test job.

### Task 7.1 [T]+[C]: matrix-sdk-crypto-wasm Integration

**Files:**
- Modify: `client/mxdx-client/src/crypto.ts`
- Create: `client/mxdx-client/tests/crypto.test.ts`

Wrap `@matrix-org/matrix-sdk-crypto-wasm` in a `CryptoClient` facade:
- Initialize WASM module once (singleton)
- `encrypt(roomId, event)` and `decrypt(event)` methods
- Store keys in IndexedDB

Test: Vitest test that initializes OlmMachine and performs encrypt/decrypt round-trip.

---

### Task 7.2 [T]+[C]: MxdxClient + Launcher Discovery

**Files:**
- Implement: `client/mxdx-client/src/client.ts`
- Implement: `client/mxdx-client/src/discovery.ts`
- Create: `client/mxdx-client/tests/discovery.test.ts`

`MxdxClient` interface:
- `connect(homeserver, accessToken)`
- `listLaunchers()` — reads `org.mxdx.launcher.identity` state events
- `getLauncherStatus(launcherId)` — reads telemetry state event
- `createTerminalSession(launcherId, command, cols, rows)` — sends request event
- `attachTerminalSession(sessionId)` — returns TerminalSocket connected to DM room

---

### Task 7.3 [T]+[C]: TerminalSocket Implementation

**Files:**
- Implement: `client/mxdx-client/src/terminal.ts`
- Create: `client/mxdx-client/tests/terminal-socket.test.ts`

From spec Section 5:
- On `attachTerminalSession()`: join DM room, start Matrix sync for that room
- On incoming `org.mxdx.terminal.data`: decompress, reorder by seq, emit via `onmessage`
- On `send()`: compress, base64 encode, send as `org.mxdx.terminal.data` to DM
- On `resize()`: send `org.mxdx.terminal.resize`
- On sync drop: buffer pending, reconnect with exponential backoff (1s, 2s, 4s... max 30s)

**xterm.js compatibility test:**
```typescript
it("TerminalSocket is compatible with xterm.js AttachAddon", async () => {
  const socket = new TerminalSocket(mockDmRoom, mockMatrixClient);
  // Verify it has the WebSocket-like interface AttachAddon expects:
  // binaryType, send(), close(), onmessage, onclose, onerror
  expect(socket.binaryType).toBe("arraybuffer");
  expect(typeof socket.send).toBe("function");
  expect(typeof socket.close).toBe("function");
});
```

---

### Task 7.4 [T]+[C]: Sequence Gap Handling

**Files:**
- Modify: `client/mxdx-client/src/terminal.ts`

From spec Section 9:
1. Buffer incoming events for 500ms when gap detected
2. If gap not filled, send `org.mxdx.terminal.retransmit`
3. If retransmit fails, accept gap and continue

```typescript
it("requests retransmit when sequence gap detected", async () => {
  const socket = createTestTerminalSocket();
  socket._deliverEvent({ seq: 0, data: encode("a") });
  socket._deliverEvent({ seq: 2, data: encode("c") }); // gap at seq=1

  await new Promise(r => setTimeout(r, 600));

  expect(sentEvents).toContainEqual(
    expect.objectContaining({
      type: "org.mxdx.terminal.retransmit",
      content: { from_seq: 1, to_seq: 1 },
    })
  );
});
```

---

## Phase 8: Policy Agent (parallel with 9, 10)

> **Epic:** Fail-closed access control appservice.
> **Agents:** Tester -> Coder + Security Reviewer.
> **Completion gate:** Unauthorized commands blocked; authorized pass; Policy Agent down = fail-closed.
> **Branch:** `feat/phase-8-policy-agent`
> **Security:** Replay protection (mxdx-rpl).

### Task 8.1 [T]+[C]: Appservice Registration

**Files:**
- Create: `crates/mxdx-policy/src/appservice.rs`
- Create: `crates/mxdx-policy/src/config.rs`
- Create: `crates/mxdx-policy/registration.yaml.template`

Register with Tuwunel as appservice. Claims `@agent-*` namespace (exclusive). Reference `docs/adr/2026-03-05-tuwunel-ground-truth.md` Section 7 for registration method.

Test: Start Tuwunel with appservice, try creating `@agent-test:localhost` without appservice -> verify M_FORBIDDEN.

---

### Task 8.2 [T]: Policy Enforcement Tests

**Files:**
- Create: `crates/mxdx-policy/tests/policy_enforcement.rs`

```rust
#[tokio::test]
async fn authorized_user_command_reaches_launcher() { ... }

#[tokio::test]
async fn unauthorized_user_command_is_rejected() { ... }

#[tokio::test]
async fn test_security_policy_agent_down_blocks_all_agent_actions() {
    // Start tuwunel with appservice config
    // Stop the policy agent process
    // Try to send command to @agent-test — must get M_FORBIDDEN
}

// mxdx-rpl: replay protection
#[tokio::test]
async fn test_security_replayed_event_does_not_double_execute() {
    // Send a command event with uuid "cmd-1"
    // Verify it executes once
    // Replay the same event (same uuid)
    // Verify the launcher does not execute it again
    // Mechanism: uuid stored in LRU cache with TTL
}
```

---

### Task 8.2 [C]: Policy Enforcement Implementation

**Files:**
- Create: `crates/mxdx-policy/src/policy.rs`

Replay protection (mxdx-rpl): Use `lru::LruCache` with capacity 10000 and TTL of 1 hour. Store command UUIDs. Before processing any command, check the cache. If present, drop the event silently.

---

### Task 8.3 [S]: Phase 8 Security Review

Checklist:
- [ ] Fail-closed: appservice down = M_FORBIDDEN (verified by test)
- [ ] Exclusive namespace prevents bypass
- [ ] Replay protection uses bounded LRU with TTL (mxdx-rpl)
- [ ] Authorized user check is prefix-based, not exact match (verify no regex injection)

---

## Phase 9: Secrets Coordinator (parallel with 8, 10)

> **Epic:** HSM-backed secret broker with age double-encrypted store.
> **Agents:** Tester -> Coder + Security Reviewer.
> **Completion gate:** Workers request secrets via E2EE DM; secrets are double-encrypted with ephemeral age keys; unauthorized requests denied; audit trail.
> **Branch:** `feat/phase-9-secrets`
> **Security:** Double encryption required (mxdx-adr2), test key gating (mxdx-tky).

### Task 9.1 [T]: Secret Store Tests

**Files:**
- Create: `crates/mxdx-secrets/src/store.rs` (tests)

```rust
#[test]
fn secrets_store_add_retrieve_round_trip() {
    let store = SecretStore::new_with_test_key();
    store.add("github.token", "ghp_testtoken123").unwrap();
    let retrieved = store.get("github.token").unwrap().unwrap();
    assert_eq!(retrieved, "ghp_testtoken123");
}

#[test]
fn secrets_store_unknown_key_returns_none() {
    let store = SecretStore::new_with_test_key();
    assert!(store.get("nonexistent").unwrap().is_none());
}

#[test]
fn secrets_store_survives_serialize_deserialize() {
    let store = SecretStore::new_with_test_key();
    store.add("key1", "value1").unwrap();
    let serialized = store.serialize().unwrap();
    let store2 = SecretStore::deserialize(&serialized, store.key()).unwrap();
    assert_eq!(store2.get("key1").unwrap().unwrap(), "value1");
}
```

**Security (mxdx-tky):** Verify `new_with_test_key` is behind `#[cfg(test)]`:
```rust
// This test verifies the cfg gate exists — it compiles only in test mode
#[test]
fn test_key_constructor_is_test_only() {
    // If this compiles, new_with_test_key is available in test mode
    let _ = SecretStore::new_with_test_key();
    // The CI check will verify it doesn't appear in release builds
}
```

---

### Task 9.1 [C]: Secret Store Implementation

**Files:**
- Implement: `crates/mxdx-secrets/src/store.rs`

Use `age` crate for encryption. `SecretStore` holds an age identity (private key) and encrypts each value with it.

**mxdx-tky:** `new_with_test_key()` must be `#[cfg(test)]` gated:
```rust
#[cfg(test)]
pub fn new_with_test_key() -> Self {
    // Fixed key for deterministic tests
}
```

---

### Task 9.2 [T]+[C]: Double-Encrypted Secret Delivery

**Files:**
- Create: `crates/mxdx-secrets/src/coordinator.rs`
- Create: `crates/mxdx-secrets/tests/e2e_secret_request.rs`

**Double encryption (mxdx-adr2):** Secret delivery uses age ephemeral key exchange:
1. Worker sends `SecretRequestEvent` with a one-time age public key
2. Coordinator encrypts the secret value with the worker's one-time public key using age
3. Response contains the age-encrypted ciphertext, not the plaintext secret
4. Worker decrypts with their one-time private key

This means even if Megolm session keys are compromised, the secret value requires the ephemeral private key to decrypt.

**E2E test:**
```rust
#[tokio::test]
async fn worker_requests_secret_with_double_encryption() {
    let hs = TuwunelInstance::start().await.unwrap();
    let coordinator = start_test_coordinator(&hs, &[("github.token", "ghp_test")]).await;
    let worker = mxdx_matrix::MatrixClient::register_and_connect(
        &format!("http://127.0.0.1:{}", hs.port), "worker", "pass",
    ).await.unwrap();

    // Generate ephemeral age keypair
    let ephemeral_key = age::x25519::Identity::generate();
    let public_key = ephemeral_key.to_public().to_string();

    let dm_room = worker.create_dm_with(coordinator.user_id()).await.unwrap();
    worker.send_event(&dm_room, SecretRequestEvent {
        request_id: "req-001".into(),
        scope: "github.token".into(),
        ttl_seconds: 3600,
        reason: "Test request".into(),
        ephemeral_public_key: public_key,
    }.into()).await.unwrap();

    let response = wait_for_event::<SecretResponseEvent>(
        &dm_room, "req-001", Duration::from_secs(5),
    ).await.unwrap();
    assert!(response.granted);

    // Value is age-encrypted, not plaintext
    let encrypted_value = response.encrypted_value.unwrap();
    let decrypted = age_decrypt(&ephemeral_key, &encrypted_value).unwrap();
    assert_eq!(decrypted, "ghp_test");
}

#[tokio::test]
async fn unauthorized_worker_cannot_get_secret() {
    // Worker not authorized by policy -> response.granted == false
}
```

---

### Task 9.3 [S]: Phase 9 Security Review

Checklist:
- [ ] Double encryption: secret never appears as plaintext in Matrix event (mxdx-adr2)
- [ ] Ephemeral key is one-time use — not reused across requests
- [ ] `new_with_test_key()` is `#[cfg(test)]` gated (mxdx-tky)
- [ ] Unauthorized requests denied
- [ ] Audit trail: secret access logged to audit room

Adversarial variant: replay a `SecretRequestEvent` with same ephemeral key — should the coordinator reject reused public keys?
## Phase 10: Web App (parallel with 8, 9)

> **Epic:** Rust + Axum HTMX web service with SSE live updates.
> **Agents:** Tester -> Coder.
> **Completion gate:** Dashboard serves launcher cards via HTMX. SSE pushes status updates. PWA works offline.
> **Branch:** `feat/phase-10-web-app`
> **Security:** SRI + CORS (mxdx-web).

### Task 10.1 [T]+[C]: Axum Scaffold + Routes

**Files:**
- Modify: `crates/mxdx-web/Cargo.toml`
- Implement: `crates/mxdx-web/src/main.rs`
- Create: `crates/mxdx-web/src/routes/mod.rs`
- Create: `crates/mxdx-web/src/routes/dashboard.rs`
- Create: `crates/mxdx-web/src/state.rs`

The web service is **stateless** — it holds no Matrix credentials. It serves:
- Static assets (HTML, JS, CSS, WASM)
- HTMX partials (server-rendered HTML fragments)
- PWA manifest + service worker

```rust
#[tokio::test]
async fn dashboard_returns_200() {
    let app = build_test_app();
    let response = app
        .oneshot(Request::builder().uri("/dashboard").body(Body::empty()).unwrap())
        .await
        .unwrap();
    assert_eq!(response.status(), StatusCode::OK);
}

#[tokio::test]
async fn dashboard_shows_launcher_cards() {
    let app = build_test_app_with_launchers(vec![
        MockLauncher { id: "belthanior", status: "online", cpu: 23.5 },
    ]);
    let response = app
        .oneshot(Request::builder().uri("/dashboard").body(Body::empty()).unwrap())
        .await
        .unwrap();
    let body = String::from_utf8(
        axum::body::to_bytes(response.into_body(), usize::MAX).await.unwrap().to_vec()
    ).unwrap();
    assert!(body.contains("belthanior"));
    assert!(body.contains("23.5"));
}
```

**Security (mxdx-web): CORS configuration**
```rust
// Add CORS middleware to Axum router
use tower_http::cors::{CorsLayer, AllowOrigin};

let cors = CorsLayer::new()
    .allow_origin(AllowOrigin::exact(
        "same-origin".parse().unwrap() // only same-origin requests
    ))
    .allow_methods([Method::GET]);
```

---

### Task 10.2 [T]+[C]: SSE for Live Launcher Status

**Files:**
- Create: `crates/mxdx-web/src/routes/sse.rs`

SSE endpoint pushes HTMX `hx-swap-oob` fragments when launcher telemetry changes. The endpoint reads from shared state (populated by Matrix sync background task).

```rust
#[tokio::test]
async fn sse_pushes_launcher_update() {
    let (state, trigger) = create_test_state_with_trigger();
    let app = build_test_app_with_state(state);

    // Connect to SSE
    let response = app
        .oneshot(Request::builder().uri("/sse/launchers").body(Body::empty()).unwrap())
        .await
        .unwrap();
    assert_eq!(response.status(), StatusCode::OK);

    // Trigger a telemetry update
    trigger.send(LauncherUpdate {
        id: "belthanior",
        cpu: 45.2,
    }).unwrap();

    // Read SSE event from response body stream
    // Verify it contains HTMX oob-swap fragment with updated CPU
}
```

---

### Task 10.3 [T]+[C]: PWA Manifest + Service Worker

**Files:**
- Create: `crates/mxdx-web/static/manifest.webmanifest`
- Create: `crates/mxdx-web/static/sw.js`
- Create: `crates/mxdx-web/src/routes/static_files.rs`

```json
{
  "name": "mxdx Management Console",
  "short_name": "mxdx",
  "start_url": "/",
  "display": "standalone",
  "background_color": "#1a1a2e",
  "theme_color": "#0f3460",
  "icons": [{ "src": "/icons/icon-192.png", "sizes": "192x192", "type": "image/png" }]
}
```

**Security (mxdx-web): Subresource Integrity**

Service worker caches static assets with SRI verification:
```javascript
// sw.js
const ASSETS = [
  { url: '/js/app.js', integrity: 'sha384-...' },
  { url: '/wasm/crypto.wasm', integrity: 'sha384-...' },
];

self.addEventListener('install', (event) => {
  event.waitUntil(
    caches.open('mxdx-v1').then(async (cache) => {
      for (const asset of ASSETS) {
        const response = await fetch(asset.url, { integrity: asset.integrity });
        await cache.put(asset.url, response);
      }
    })
  );
});
```

Strategy: cache-first for static assets (with SRI), network-first for HTMX partials.

Add CSP headers in Axum middleware:
```rust
.layer(SetResponseHeaderLayer::overriding(
    header::CONTENT_SECURITY_POLICY,
    "default-src 'self'; script-src 'self'; style-src 'self' 'unsafe-inline'; connect-src 'self' wss:; worker-src 'self'"
))
```

---

### Task 10.4 [S]: Phase 10 Security Review

Checklist:
- [ ] CORS restricts to same-origin (mxdx-web)
- [ ] SRI hashes on all cached assets (mxdx-web)
- [ ] CSP headers prevent inline scripts
- [ ] No Matrix credentials stored in web app (stateless)
- [ ] HTMX partials served only to same-origin requests

---

## Phase 11: Multi-Homeserver

> **Epic:** Launcher registers on 2-3 federated homeservers. Failover when primary fails.
> **Agents:** Tester -> Coder.
> **Completion gate:** Launcher fails over from hs_a to hs_b transparently; commands still flow; interactive sessions create new DMs on new identity.
> **Branch:** `feat/phase-11-multi-hs`
> **CI update:** Add federation test job (requires mkcert for TLS).

### Task 11.1 [T]+[C]: Multi-Account Config + Startup

**Files:**
- Modify: `crates/mxdx-launcher/src/config.rs` (add multi-homeserver config)
- Create: `crates/mxdx-launcher/src/multi_hs.rs`

Startup sequence (from spec Section 6):
1. Connect to all homeservers concurrently (`tokio::join!`)
2. Measure sync latency to each (ping `/sync` with `timeout=0`)
3. Select lowest-latency as primary (hot identity)
4. Create Space and rooms on primary
5. Invite other identities at power level 100
6. Invite configured admin accounts at power level 100
7. All identities start listening; only primary handles responses

```rust
#[tokio::test]
async fn multi_hs_launcher_selects_lowest_latency_primary() {
    let pair = FederatedPair::start().await.unwrap();
    let launcher = MultiHsLauncher::start(&[
        format!("http://127.0.0.1:{}", pair.hs_a.port),
        format!("http://127.0.0.1:{}", pair.hs_b.port),
    ]).await.unwrap();

    // Primary should be set
    assert!(launcher.primary().is_some());
    // Both identities should be connected
    assert_eq!(launcher.connected_count(), 2);

    pair.stop().await;
}
```

---

### Task 11.2 [T]: Failover Tests

**Files:**
- Create: `tests/federation/failover.rs`

```rust
#[tokio::test]
async fn launcher_fails_over_from_primary_to_secondary() {
    let pair = FederatedPair::start().await.unwrap();
    let launcher = MultiHsLauncher::start(&[
        format!("http://127.0.0.1:{}", pair.hs_a.port),
        format!("http://127.0.0.1:{}", pair.hs_b.port),
    ]).await.unwrap();

    let initial_primary_port = launcher.primary_port();

    // Kill the primary homeserver
    if initial_primary_port == pair.hs_a.port {
        pair.hs_a.stop().await;
    } else {
        pair.hs_b.stop().await;
    }

    // Wait for failover detection (health check interval + threshold)
    tokio::time::sleep(Duration::from_secs(20)).await;

    // Primary should have changed
    assert_ne!(launcher.primary_port(), initial_primary_port);

    // Commands should still work through new primary
    let result = launcher.execute_test_command("echo", &["failover-test"]).await.unwrap();
    assert_eq!(result.exit_code, Some(0));
}

#[tokio::test]
async fn interactive_session_creates_new_dm_after_failover() {
    // Start session on primary
    // Kill primary
    // Request new session -> should create DM from new hot identity
    // Old DM no longer receives new data
}

#[tokio::test]
async fn test_security_non_launcher_cannot_update_identity_event() {
    // Verify a non-launcher user cannot update org.mxdx.launcher.identity state event
    let pair = FederatedPair::start().await.unwrap();
    let launcher = MultiHsLauncher::start(&[
        format!("http://127.0.0.1:{}", pair.hs_a.port),
        format!("http://127.0.0.1:{}", pair.hs_b.port),
    ]).await.unwrap();

    let attacker = mxdx_matrix::MatrixClient::register_and_connect(
        &format!("http://127.0.0.1:{}", pair.hs_a.port), "attacker", "pass",
    ).await.unwrap();

    // Attacker tries to update launcher identity state event in exec room
    let exec_room = launcher.exec_room_id();
    // Attacker should not be able to join or send state events
    let result = attacker.send_state_event(
        &exec_room,
        "org.mxdx.launcher.identity",
        "",
        serde_json::json!({"primary": "attacker"}),
    ).await;
    assert!(result.is_err());
}
```

---

### Task 11.2 [C]: Failover Implementation

**Files:**
- Implement: `crates/mxdx-launcher/src/multi_hs.rs`

Health check loop:
- Every 5 seconds, ping primary's `/sync`
- After 3 consecutive failures (FAIL_THRESHOLD), trigger failover
- Select next lowest-latency connected identity as new primary
- Update `org.mxdx.launcher.identity` state event
- Active interactive sessions: create new DMs from new hot identity, send session migration notice to old DMs

States: `Active -> Failing -> Failover -> Active` (or `Unavailable` if all homeservers down)

---

### Task 11.3 [CI]: Federation Test Job

**Files:**
- Modify: `.github/workflows/ci.yml`

```yaml
  federation:
    needs: build
    runs-on: ubuntu-latest
    if: github.ref == 'refs/heads/main' || github.event_name == 'workflow_dispatch'
    steps:
      - uses: actions/checkout@v4
      - uses: dtolnay/rust-toolchain@stable
      - uses: Swatinem/rust-cache@v2
      - name: Install system deps
        run: sudo apt-get install -y libsqlite3-dev libssl-dev
      - name: Install tuwunel
        run: |
          curl -fsSL https://github.com/matrix-construct/tuwunel/releases/latest/download/tuwunel-x86_64-v1-linux-gnu.deb \
            -o /tmp/tuwunel.deb && sudo dpkg -i /tmp/tuwunel.deb
      - name: Install mkcert
        run: |
          curl -fsSL https://dl.filippo.io/mkcert/latest?for=linux/amd64 -o /usr/local/bin/mkcert
          chmod +x /usr/local/bin/mkcert
          mkcert -install
      - name: Generate TLS certs
        run: |
          mkcert -cert-file /tmp/tls.crt -key-file /tmp/tls.key localhost 127.0.0.1
      - name: Run federation tests
        env:
          MXDX_TEST_TLS_CERT: /tmp/tls.crt
          MXDX_TEST_TLS_KEY: /tmp/tls.key
        run: cargo test -p mxdx-test-helpers --test federation -- --include-ignored
```

---

## Phase 12: Integration & Hardening

> **Epic:** Full E2E test suite, security report CI artifact, WASI packaging.
> **Agents:** Tester (E2E), DevOps (WASI + CI), Security Reviewer (final report).
> **Completion gate:** Full stack E2E passes. Security report published as GitHub Release artifact. WASI launcher runs --help.
> **Branch:** `feat/phase-12-hardening`

### Task 12.1 [T]+[C]: Full E2E Test Suite

**Files:**
- Create: `tests/e2e/full_system.rs`

End-to-end test that exercises the entire stack:

```rust
#[tokio::test]
#[ignore = "system test -- requires full stack"]
async fn full_system_e2e() {
    // 1. Start tuwunel
    let hs = TuwunelInstance::start().await.unwrap();

    // 2. Start policy agent (appservice)
    let policy = start_policy_agent(&hs).await;

    // 3. Start secrets coordinator
    let coordinator = start_secrets_coordinator(&hs, &[
        ("deploy.key", "secret-deploy-key"),
    ]).await;

    // 4. Start launcher
    let launcher = start_launcher(&hs, LauncherConfig {
        capabilities: CapabilityConfig {
            mode: CapabilityMode::Allowlist,
            allowed_commands: vec!["echo".into(), "seq".into()],
            allowed_cwd_prefixes: vec!["/tmp".into()],
            ..Default::default()
        },
        ..test_launcher_config(&hs)
    }).await;

    // 5. Register an admin user
    let admin = mxdx_matrix::MatrixClient::register_and_connect(
        &format!("http://127.0.0.1:{}", hs.port), "admin", "pass",
    ).await.unwrap();

    // 6. Discover launcher
    let launcher_space = find_launcher_space(&admin, "test-launcher").await.unwrap();
    assert!(launcher_space.is_some());

    // 7. Send non-interactive command
    let exec_room = find_exec_room(&admin, &launcher_space.unwrap()).await.unwrap();
    let cmd = CommandEvent {
        uuid: uuid::Uuid::new_v4().to_string(),
        action: CommandAction::Exec,
        cmd: "echo".into(),
        args: vec!["e2e-test".into()],
        env: Default::default(),
        cwd: Some("/tmp".into()),
        timeout_seconds: Some(10),
    };
    admin.send_command(&exec_room, &cmd).await.unwrap();
    let result = wait_for_result(&exec_room, &cmd.uuid, Duration::from_secs(10)).await.unwrap();
    assert_eq!(result.status, ResultStatus::Exit);
    assert_eq!(result.exit_code, Some(0));

    // 8. Verify output has stdout stream label
    let outputs = collect_output_events(&exec_room, &cmd.uuid, Duration::from_secs(5)).await;
    assert!(outputs.iter().all(|o| o.stream == OutputStream::Stdout));

    // 9. Read telemetry
    let telemetry = read_telemetry_state(&admin, &launcher_space.unwrap()).await.unwrap();
    assert!(!telemetry.hostname.is_empty());

    // 10. Request interactive terminal session
    let req_uuid = uuid::Uuid::new_v4().to_string();
    admin.send_event(&exec_room, TerminalSessionRequestEvent {
        uuid: req_uuid.clone(),
        command: "echo".into(), // use echo for simple test
        env: Default::default(),
        cols: 80,
        rows: 24,
    }.into()).await.unwrap();

    let session_response = wait_for_event::<TerminalSessionResponseEvent>(
        &exec_room, &req_uuid, Duration::from_secs(10),
    ).await.unwrap();
    assert_eq!(session_response.status, "created");

    // 11. Request secret (double-encrypted)
    let ephemeral_key = age::x25519::Identity::generate();
    let dm = admin.create_dm_with(coordinator.user_id()).await.unwrap();
    admin.send_event(&dm, SecretRequestEvent {
        request_id: "e2e-secret".into(),
        scope: "deploy.key".into(),
        ttl_seconds: 60,
        reason: "E2E test".into(),
        ephemeral_public_key: ephemeral_key.to_public().to_string(),
    }.into()).await.unwrap();
    let secret_response = wait_for_event::<SecretResponseEvent>(
        &dm, "e2e-secret", Duration::from_secs(5),
    ).await.unwrap();
    assert!(secret_response.granted);
    let decrypted = age_decrypt(&ephemeral_key, &secret_response.encrypted_value.unwrap()).unwrap();
    assert_eq!(decrypted, "secret-deploy-key");

    // Cleanup
    hs.stop().await;
}
```

---

### Task 12.2 [CI]: Security Report Artifact

**Files:**
- Create: `.github/workflows/security-report.yml`
- Create: `docs/reports/security/security-test-matrix.md`

```yaml
name: Security Report

on:
  push:
    tags: ['v*']
  workflow_dispatch:

jobs:
  security-report:
    runs-on: ubuntu-latest
    steps:
      - uses: actions/checkout@v4
      - uses: dtolnay/rust-toolchain@stable
      - uses: Swatinem/rust-cache@v2
      - uses: actions/setup-node@v4
        with:
          node-version: "22"

      - name: Install system deps
        run: sudo apt-get install -y libsqlite3-dev libssl-dev tmux

      - name: Install tuwunel
        run: |
          curl -fsSL https://github.com/matrix-construct/tuwunel/releases/latest/download/tuwunel-x86_64-v1-linux-gnu.deb \
            -o /tmp/tuwunel.deb && sudo dpkg -i /tmp/tuwunel.deb

      - name: Run security tests
        run: |
          cargo test --workspace -- test_security_ 2>&1 | tee /tmp/security-test-output.txt

      - name: Cargo audit
        run: |
          cargo install cargo-audit --locked
          cargo audit 2>&1 | tee /tmp/cargo-audit.txt

      - name: npm audit
        run: |
          cd client && npm ci && npm audit 2>&1 | tee /tmp/npm-audit.txt

      - name: Collect reports
        run: |
          mkdir -p /tmp/security-report
          cp /tmp/security-test-output.txt /tmp/security-report/
          cp /tmp/cargo-audit.txt /tmp/security-report/
          cp /tmp/npm-audit.txt /tmp/security-report/
          cp docs/reports/security/*.md /tmp/security-report/
          echo "## Security Report $(date -I)" > /tmp/security-report/SUMMARY.md
          echo "" >> /tmp/security-report/SUMMARY.md
          echo "### Security Test Results" >> /tmp/security-report/SUMMARY.md
          grep -c "test_security.*ok" /tmp/security-test-output.txt | xargs -I{} echo "Passing security tests: {}" >> /tmp/security-report/SUMMARY.md
          grep -c "test_security.*FAILED" /tmp/security-test-output.txt | xargs -I{} echo "Failing security tests: {}" >> /tmp/security-report/SUMMARY.md || true

      - name: Upload security report
        uses: actions/upload-artifact@v4
        with:
          name: security-report
          path: /tmp/security-report/

      - name: Attach to release
        if: startsWith(github.ref, 'refs/tags/')
        uses: softprops/action-gh-release@v2
        with:
          files: /tmp/security-report/**
```

**Security test matrix document:**

```markdown
# Security Test Matrix

Maps security claims to their corresponding tests.

| Finding ID | Claim | Test | Phase |
|:---|:---|:---|:---|
| mxdx-ji1 | No hardcoded test ports | TuwunelInstance uses port 0 | 3 |
| mxdx-71v | cwd validated against allowlist | test_security_cwd_outside_prefix_is_rejected | 5 |
| mxdx-71v | path traversal blocked | test_security_cwd_traversal_rejected | 5 |
| mxdx-jjf | git -c blocked | test_security_git_dash_c_blocked | 5 |
| mxdx-jjf | git submodule foreach blocked | test_security_git_submodule_foreach_blocked | 5 |
| mxdx-jjf | docker compose -f blocked | test_security_docker_compose_dash_f_blocked | 5 |
| mxdx-jjf | env prefix injection blocked | test_security_env_prefix_injection_blocked | 5 |
| mxdx-aew | history_visibility=joined from creation | test_security_late_joiner_cannot_read_pre_join_history | 6 |
| mxdx-ccx | zlib bomb rejected | test_security_zlib_bomb_rejected_before_pty_write | 6 |
| mxdx-ccx | decompression fails fast | test_security_decompression_streams_and_fails_fast | 6 |
| mxdx-seq | seq is u64, handles max | seq_field_is_u64_and_handles_large_values | 2 |
| mxdx-rpl | replay protection | test_security_replayed_event_does_not_double_execute | 8 |
| mxdx-adr2 | secrets double-encrypted | worker_requests_secret_with_double_encryption | 9 |
| mxdx-tky | test key is cfg(test) only | test_key_constructor_is_test_only + CI check | 9 |
| mxdx-cfg | config permissions checked | validate_config_permissions warns on bad perms | 5 |
| mxdx-web | CORS same-origin | CORS middleware configured | 10 |
| mxdx-web | SRI on cached assets | service worker verifies integrity | 10 |
```

---

### Task 12.3 [C]: WASI Packaging

**Files:**
- Modify: `crates/mxdx-launcher/Cargo.toml` (add feature flags)
- Create: `packages/npm/mxdx-launcher/package.json`

From ADR-0003: matrix-sdk may not support WASI. Build with conditional compilation:

```toml
# crates/mxdx-launcher/Cargo.toml
[features]
default = ["native"]
native = ["dep:matrix-sdk", "dep:mxdx-matrix"]

[target.'cfg(not(target_arch = "wasm32"))'.dependencies]
matrix-sdk = { workspace = true }
mxdx-matrix = { path = "../mxdx-matrix" }
```

```rust
// src/main.rs
fn main() {
    #[cfg(target_arch = "wasm32")]
    {
        eprintln!("mxdx-launcher WASI build: Matrix networking not yet supported.");
        eprintln!("Use --help or --version. Full functionality requires native build.");
        // Parse args for --help and --version
    }

    #[cfg(not(target_arch = "wasm32"))]
    {
        // Full native launcher
        tokio::runtime::Runtime::new().unwrap().block_on(run());
    }
}
```

Build:
```bash
# Native
cargo build -p mxdx-launcher

# WASI (CLI-only)
cargo build -p mxdx-launcher --target wasm32-wasip2 --no-default-features
```

Test: `wasmtime target/wasm32-wasip2/debug/mxdx-launcher.wasm -- --help` should print usage.

---

### Task 12.4 [S]: Final Security Review

**Role:** Security Reviewer
**Output:** `docs/reports/security/2026-03-05-final-review.md`

Full review against all findings from `docs/reviews/security/2026-03-05-design-review-plan-and-spec.md`:
- Verify every blocking finding is addressed with a passing test
- Verify every accepted risk has an ADR
- Verify the security test matrix is complete
- Run all adversarial variants one final time
- Sign off on the security report artifact

---

### Task 12.5 [D]: Final Documentation

**Role:** Documenter

1. Verify MANIFEST.md matches `cargo xtask manifest` output
2. Update AGENTS.md with final team structure and conventions
3. Write final phase summary
4. Verify all ADRs are up to date
5. Check spec drift — compare implemented behavior against `docs/mxdx-management-console.md`
6. Update README with getting started instructions

---

## Beads Task Summary

Total tasks per phase:

| Phase | [T] | [C] | [S] | [D] | [CI] | Total |
|:---|:---|:---|:---|:---|:---|:---|
| 0: Preflight | 0 | 0 | 0 | 0 | 2 | 2 |
| 1: Foundation | 0 | 3 | 0 | 1 | 1 | 5 |
| 2: Types | 2 | 3 | 0 | 1 | 1 | 7 |
| 3: Test Infra | 2 | 2 | 0 | 1 | 1 | 6 |
| 4: Matrix Client | 2 | 3 | 0 | 1 | 1 | 7 |
| 5: Launcher v1 | 3 | 4 | 1 | 1 | 1 | 10 |
| 6: Terminal | 4 | 5 | 1 | 1 | 1 | 12 |
| 7: Browser Client | 3 | 4 | 0 | 1 | 1 | 9 |
| 8: Policy Agent | 2 | 2 | 1 | 1 | 1 | 7 |
| 9: Secrets | 2 | 2 | 1 | 1 | 1 | 7 |
| 10: Web App | 3 | 3 | 1 | 1 | 1 | 9 |
| 11: Multi-HS | 2 | 2 | 0 | 1 | 1 | 6 |
| 12: Hardening | 1 | 1 | 1 | 1 | 1 | 5 |
| **Total** | **26** | **34** | **6** | **12** | **13** | **92** |

---

## Execution Notes

### Merge Order (Critical Path)

```
Phase 0 -> Phase 1 -> Phase 2 -> Phase 3 -> Phase 4 -> Phase 5 -> Phase 6 -> Phase 7
                                                                                 |
                                                                    +-- Phase 8 (parallel)
                                                                    +-- Phase 9 (parallel)
                                                                    +-- Phase 10 (parallel)
                                                                                 |
                                                                    Phase 11 -> Phase 12
```

### Key Rules

1. **Every PR is CI-green at merge time.** No exceptions.
2. **CI jobs only reference existing code.** Each phase adds its own CI jobs.
3. **Security findings are task requirements.** Finding IDs in tasks are not optional.
4. **Lead merges PRs.** Product Owner approves phase completions.
5. **Escalate fast.** 2 failed attempts -> Lead -> Product Owner.
6. **No mocks.** Real tuwunel, real matrix-sdk, real tmux.
7. **Documenter keeps docs current.** After every PR merge.
