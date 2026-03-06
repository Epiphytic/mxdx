# mxdx Management Console

Matrix-native fleet management system with interactive browser terminals, E2EE, multi-homeserver redundancy, and a WASI-packaged launcher.

All communication flows over Matrix E2EE rooms using the `org.mxdx.*` event namespace. Non-interactive commands use threaded replies. Interactive terminals use DMs. A stateless web app connects browsers directly to Tuwunel homeservers.

## Prerequisites

- **Rust** (stable toolchain)
- **Node.js** >= 22
- **Tuwunel** >= 1.5.0 (Matrix homeserver) -- see `docs/adr/2026-03-05-tuwunel-ground-truth.md`
- **tmux** (for interactive terminal sessions)

Run `bash scripts/preflight.sh` to verify your environment.

## Build

```bash
# Rust workspace
cargo build --workspace

# TypeScript client packages
cd client && npm ci && npm run build
```

## Test

```bash
# Rust unit + integration tests (integration tests require Tuwunel)
cargo test --workspace

# TypeScript tests
cd client && npm test
```

## Project Structure

### Rust Crates (`crates/`)

| Crate | Purpose |
|:---|:---|
| `mxdx-types` | Shared `org.mxdx.*` event schema types |
| `mxdx-matrix` | matrix-sdk facade (E2EE, room topology) |
| `mxdx-launcher` | Launcher binary (command execution, terminal sessions, telemetry) |
| `mxdx-policy` | Policy Agent appservice (access control) |
| `mxdx-secrets` | Secrets Coordinator (age-encrypted store, DM delivery) |
| `mxdx-web` | Web dashboard (Axum, HTMX, SSE) |

### TypeScript Packages (`client/`)

| Package | Purpose |
|:---|:---|
| `@mxdx/client` | Browser Matrix client with E2EE (matrix-sdk-crypto-wasm) |
| `@mxdx/web-ui` | HTMX dashboard + xterm.js terminal |

### Other

| Path | Purpose |
|:---|:---|
| `xtask/` | `cargo xtask manifest` -- generates MANIFEST.md symbol tables |
| `tests/helpers/` | Test infrastructure (TuwunelInstance, FederatedPair) |
| `scripts/preflight.sh` | Environment verification |
| `docs/` | Plans, ADRs, phase summaries, security reviews |

## Documentation

- **Architecture:** `docs/mxdx-architecture.md`
- **Management Console Design:** `docs/mxdx-management-console.md`
- **Build Plan:** `docs/plans/2026-03-05-mxdx-rebuild-plan.md`
- **Phase Summaries:** `docs/phases/`
- **Security Reviews:** `docs/reviews/security/`
- **ADRs:** `docs/adr/`
- **Module Registry:** `MANIFEST.md`
