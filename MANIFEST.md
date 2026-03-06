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

<!-- BEGIN GENERATED SYMBOL TABLES -->

### mxdx-launcher

_No public symbols._

### mxdx-matrix

_No public symbols._

### mxdx-policy

_No public symbols._

### mxdx-secrets

_No public symbols._

### mxdx-types

_No public symbols._

### mxdx-web

_No public symbols._
<!-- END GENERATED SYMBOL TABLES -->



