# Phase 1: Foundation — Summary

**Branch:** `feat/phase-1-foundation`
**Status:** Complete
**Date:** 2026-03-06

## Goal

Establish the project scaffolding: Cargo workspace, npm workspace, xtask manifest generator, and build-only CI pipeline.

## Completion Gate

All gates satisfied:

- `cargo build --workspace` passes
- `cargo xtask manifest` generates MANIFEST.md
- `cd client && npm install && npm run build` passes
- CI pipeline runs build checks on push

## Tasks Completed

### Task 1.1: Cargo Workspace Scaffold (mxdx-boo.1)

Created the Rust workspace with six crates and shared workspace dependencies.

**Crates created:**

| Crate | Type | Purpose |
|:---|:---|:---|
| `mxdx-types` | lib | Shared event schema types |
| `mxdx-matrix` | lib | matrix-sdk facade |
| `mxdx-policy` | bin | Policy Agent appservice |
| `mxdx-secrets` | bin | Secrets Coordinator |
| `mxdx-launcher` | bin | Launcher (non-interactive + interactive) |
| `mxdx-web` | bin | Web app (Axum, HTMX) |

**Workspace dependencies:** tokio, serde, serde_json, tracing, tracing-subscriber, anyhow, uuid, matrix-sdk 0.16, ruma 0.14, toml, axum 0.7, age 0.11, sysinfo, reqwest, tempfile, base64, flate2, lru.

### Task 1.2: xtask Manifest Generator (mxdx-boo.2)

Created `xtask/` crate with `cargo xtask manifest` command that:

- Scans all workspace crates for public symbols (functions, structs, enums, traits, type aliases)
- Generates the symbol tables section in MANIFEST.md
- Supports `--check` mode for CI verification (exits non-zero if MANIFEST.md is stale)

### Task 1.3: npm Workspace Scaffold (mxdx-boo.3)

Created the TypeScript workspace under `client/` with two packages.

**Packages created:**

| Package | Purpose |
|:---|:---|
| `@mxdx/client` | Browser Matrix client with E2EE (matrix-sdk-crypto-wasm, zod) |
| `@mxdx/web-ui` | HTMX dashboard + xterm.js terminal (@xterm/xterm, htmx.org) |

Both packages compile with TypeScript (`target: ES2022`, `module: ES2022`, `moduleResolution: bundler`).

### Task 1.4: Build-Only CI Pipeline (mxdx-boo.4)

Created `.github/workflows/ci.yml` with four jobs:

| Job | Purpose |
|:---|:---|
| `preflight` | Verifies cargo, rustc, node, npm are available |
| `build` | `cargo build --workspace` with rust-cache |
| `manifest` | `cargo xtask manifest --check` to verify MANIFEST.md is current |
| `client-build` | `cd client && npm ci && npm run build` |

Triggers on all branch pushes and PRs to main, with concurrency grouping.

## Artifacts

- `Cargo.toml` — workspace root
- `crates/mxdx-{types,matrix,policy,secrets,launcher,web}/` — six Rust crates
- `xtask/` — manifest generator
- `client/package.json` — npm workspace root
- `client/mxdx-client/` — TypeScript client package
- `client/mxdx-web-ui/` — TypeScript web UI package
- `.github/workflows/ci.yml` — CI pipeline
- `MANIFEST.md` — auto-generated module registry
