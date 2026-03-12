# ADR 0003: WASI Packaging Limitations for mxdx-launcher

**Date:** 2026-03-05
**Status:** Accepted

## Context

Phase 12 calls for compiling `mxdx-launcher` to `wasm32-wasip2` and distributing it as an npm package, enabling zero-install deployment via `npx @mxdx/launcher`.

## Decision

The `mxdx-launcher` binary depends on `matrix-sdk` (via `mxdx-matrix`), which explicitly rejects WASI compilation:

```
error: matrix-sdk currently requires a JavaScript environment for WASM.
```

This means a **fully functional** Matrix-connected launcher cannot be compiled to WASI preview 2 today. The two paths available are:

### Option A: CLI-only WASI build (selected)

Compile a WASI binary that handles CLI argument parsing (`--help`, `--version`, `--config-check`) but stubs out the Matrix connection layer with a clear error message:

```
mxdx-launcher: WASI build — Matrix networking not available in this distribution.
Use the native binary for production use.
```

This satisfies the "npx @mxdx/launcher --help exits 0" CI gate and enables:
- Config validation without a network connection
- Documentation of available flags
- Future: when matrix-sdk gains WASI support, drop the conditional compilation

### Option B: Defer WASI packaging

Mark Phase 12 as blocked until matrix-sdk supports WASI preview 2.

**We choose Option A** because it unblocks the CI gate and establishes the npm distribution infrastructure that will become fully functional when matrix-sdk adds WASI support.

## Implementation

`mxdx-launcher/Cargo.toml`:
```toml
[features]
# When building for WASI, skip matrix-sdk dependency entirely
native = ["dep:mxdx-matrix"]
default = ["native"]

[dependencies]
mxdx-matrix = { path = "../mxdx-matrix", optional = true }
```

`main.rs` uses `#[cfg(feature = "native")]` guards around Matrix code, and provides a minimal `--help` path for the WASI build.

## Consequences

**Positive:**
- Phase 12 CI gate passes: `npx @mxdx/launcher --help` exits 0
- npm package infrastructure is ready for when matrix-sdk adds WASI support
- Clear user-facing error explains why WASI build can't connect to Matrix

**Negative:**
- WASI build is not a fully functional launcher — users must be informed via docs
- Conditional compilation adds complexity to `main.rs`
- When matrix-sdk adds WASI support, the workarounds must be removed (tracked)

## References

- [matrix-sdk WASM support](https://github.com/matrix-org/matrix-rust-sdk/tree/main/crates/matrix-sdk-common)
- [wasm32-wasip2 target](https://doc.rust-lang.org/rustc/platform-support/wasm32-wasip2.html)
- Phase 12 plan: `docs/plans/2026-03-04-mxdx-management-console.md`
