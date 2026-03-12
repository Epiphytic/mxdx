# mxdx Package Publishing Design

**Date:** 2026-03-12
**Status:** Approved

---

## Goals

1. Publish all Rust crates to crates.io under the `mxdx-*` namespace
2. Publish npm packages so that `npx -y mxdx launcher`, `npx -y mxdx client`, and `npx -y mxdx web-console` all "just work"
3. Provide an `mx` CLI alias for `mxdx`
4. Each scoped package (`@mxdx/launcher`, `@mxdx/client`, `@mxdx/web-console`) works standalone via `npx -y @mxdx/<name>`
5. CI automates publishing via semantic-release on push to `main`

---

## npm Packages

### `@mxdx/core` (~40MB)

The shared library containing WASM bindings and all common JS modules.

- **Ships both WASM targets**: `wasm/nodejs/` and `wasm/web/`
- **Exports**: index.js, credentials.js, session.js, terminal-socket.js, terminal-types.js, cleanup.js, batched-sender.js, turn-credentials.js, webrtc-channel-node.js, p2p-signaling.js, p2p-transport.js, p2p-crypto.js, persistent-indexeddb.js
- **No bin entry** — library only
- `publishConfig: { "access": "public" }`

### `@mxdx/launcher` (~70KB + core dep)

The launcher agent that runs on managed hosts.

- `bin: { "mxdx-launcher": "bin/mxdx-launcher.js" }`
- Depends on `@mxdx/core`
- `npx -y @mxdx/launcher start` works standalone

### `@mxdx/client` (~22KB + core dep)

The CLI for fleet management.

- `bin: { "mxdx-client": "bin/mxdx-client.js" }`
- Depends on `@mxdx/core`
- `npx -y @mxdx/client exec my-launcher echo hello` works standalone

### `@mxdx/web-console` (~2MB built + core dep)

The browser-based management console.

- `bin: { "mxdx-web-console": "bin/mxdx-web-console.js" }`
- **Pre-built Vite SPA** shipped in `dist/` — no user-facing build step
- Bin script serves `dist/` with a minimal embedded Node.js static file server
- Depends on `@mxdx/core` for the web WASM target
- `npx -y @mxdx/web-console` starts the console on localhost

### `mxdx` (meta-package, ~5KB)

The unscoped entry point for fleet operators.

- `bin: { "mxdx": "bin/mxdx.js", "mx": "bin/mxdx.js" }`
- Depends on `@mxdx/launcher`, `@mxdx/client`, `@mxdx/web-console`
- Dispatcher script: parses first argument, resolves and executes the target package's bin
- `mxdx launcher [args...]` → runs `@mxdx/launcher`
- `mxdx client [args...]` → runs `@mxdx/client`
- `mxdx web-console [args...]` → runs `@mxdx/web-console`
- `mxdx` (no args) → help text with subcommand list and quickstart URL (`https://github.com/Epiphytic/mxdx/blob/main/docs/quickstart.md`)
- `mx` is an alias — identical behavior to `mxdx`

### npm Publish Order

Respects dependency graph:

1. `@mxdx/core`
2. `@mxdx/launcher`, `@mxdx/client`, `@mxdx/web-console` (parallel)
3. `mxdx`

---

## Rust Crates (crates.io)

All library crates published to reserve the namespace. Internal-only crates are excluded.

| Crate | Publish | Dependencies |
|---|---|---|
| `mxdx-types` | Yes | None (foundational) |
| `mxdx-matrix` | Yes | mxdx-types |
| `mxdx-policy` | Yes | mxdx-types, mxdx-matrix |
| `mxdx-secrets` | Yes | mxdx-types, mxdx-matrix |
| `mxdx-launcher` | Yes | mxdx-types (+ optional: mxdx-matrix, mxdx-policy, mxdx-secrets) |
| `mxdx-web` | Yes | mxdx-types |
| `mxdx-core-wasm` | Yes | mxdx-types |
| `xtask` | No | Internal build tool |
| `mxdx-test-helpers` | No | Internal test infra |

### Crate Publish Order

Respects the dependency DAG:

1. `mxdx-types`
2. `mxdx-matrix`
3. `mxdx-policy`, `mxdx-secrets` (parallel, both depend on types + matrix)
4. `mxdx-launcher`, `mxdx-web`, `mxdx-core-wasm` (parallel)

Each Cargo.toml needs: `license`, `description`, `repository`, `homepage`, `keywords`, `categories` fields for crates.io compliance.

---

## CI: Release Workflow

Modeled on agenticenti's `release.yml` with semantic-release.

### Trigger

Push to `main` branch.

### Steps

1. **Checkout** with full history (`fetch-depth: 0`)
2. **Setup** Rust toolchain (stable) + wasm-pack + Node.js 22
3. **Test** — `cargo test --workspace` + npm e2e tests
4. **Build WASM** — both nodejs and web targets
5. **Build web-console** — `vite build` to produce `dist/`
6. **Publish Rust crates** — `cargo publish` in dependency order with `--no-verify` (already tested)
7. **Publish npm packages** — in dependency order via semantic-release exec plugin
8. **GitHub release** — changelog from conventional commits

### Secrets Required

- `CARGO_REGISTRY_TOKEN` — crates.io API token
- `NPM_TOKEN` — npmjs publish token (or use `--provenance` with OIDC)
- `GITHUB_TOKEN` — for release creation (provided by Actions)

### semantic-release Config

- Analyzes conventional commits (`feat:`, `fix:`, `refactor:`, etc.)
- Bumps version across all package.json files and Cargo.toml files
- Publishes with `--provenance` for npm keyless signing
- Commits version bumps back to git with `[skip ci]`

---

## Existing CI (`ci.yml`) Extensions

- Add `npm pack` smoke test for each publishable package
- Verify `mxdx` dispatcher resolves all subcommands
- Verify `mx` alias works

---

## Web Console Serving

The `@mxdx/web-console` bin script (`bin/mxdx-web-console.js`) serves the pre-built SPA:

- Uses Node.js built-in `http.createServer` + `fs` to serve `dist/`
- Handles: index.html fallback for SPA routing, correct MIME types, WASM `application/wasm` content-type
- Default port: 5173 (matches current Vite dev server)
- `--port <n>` flag to override
- Prints URL to stdout on startup

No external dependencies needed for serving — Node.js built-ins only.
