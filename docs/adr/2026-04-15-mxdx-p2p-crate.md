# ADR 2026-04-15: Create `mxdx-p2p` crate for interactive session transport

**Status:** Accepted
**Date:** 2026-04-15
**Implemented in:** Branch `brains/rust-p2p-interactive`, Phases 0-9 (930db6a)
**Related:** `docs/plans/2026-04-15-rust-p2p-interactive-storm.md`

## Context

Interactive terminal mode (TURN + P2P data channel) works in the existing npm+wasm code path. The Rust binaries (`mxdx-client`, `mxdx-worker`) ship only a stub and a divergent event schema. The design requires:

- Native (binaries) and wasm (web-console) support behind a shared abstraction
- State machine shared across native and wasm (or duplication invites drift)
- Wire compatibility with existing npm peers (`m.call.*`, AES-GCM frames, Megolm payloads)
- Clean integration with `mxdx-worker` and `mxdx-client` without rewriting their healthy code

Three organisational options were considered:

1. **Fully unified** — new crate owns transport and all runtime wiring. Rewrites healthy code in worker/client.
2. **Distributed** — spread types across `mxdx-types`, signaling into `mxdx-matrix`, transport into each binary. State machine risks drift between worker and client.
3. **Hybrid** — new `mxdx-p2p` crate owns the platform-agnostic pieces only (trait, cfg-gated impls, state machine, crypto, signaling, TURN). Runtime wiring stays in `mxdx-worker` and `mxdx-client`.

## Decision

Create a new workspace crate `crates/mxdx-p2p/` that owns:

- `WebRtcChannel` trait with two cfg-gated implementations (`datachannel-rs` for native, `web-sys::RtcPeerConnection` for wasm)
- `P2PTransport` state machine (one implementation for both targets)
- `P2PCrypto` (AES-256-GCM layer)
- `signaling/` module — `m.call.*` event de/serialization + glare resolver
- `turn` module — `/_matrix/client/v3/voip/turnServer` client with active-call refresh

Runtime wiring (SessionMux, BatchedSender configuration, session lifecycle, telemetry) stays in `mxdx-worker` and `mxdx-client`.

## Rationale

- **Single source of truth for the state machine.** Both worker and client (and browser web-console via `mxdx-core-wasm`) share one `P2PTransport`. Drift is impossible by construction.
- **Preserves existing worker/client architecture.** Session lifecycle and telemetry code are healthy and densely integrated — rewriting them would be high-risk churn for no benefit.
- **Cross-cutting concerns (crypto, signaling, TURN) belong together.** Putting them in separate crates would scatter the security-critical surface, increasing audit difficulty.
- **Matches matrix-sdk ecosystem convention.** matrix-sdk itself uses cfg-gated stores for native vs wasm; a single crate with two compile-time backends is idiomatic.
- **Delivery sequencing is naturally per-crate.** The 9-step rollout (crypto → TURN → channel → signaling → state machine → wiring → tests → wasm → default-on) maps cleanly to files within one crate.

## Consequences

- One new workspace member; CI gains `cargo test -p mxdx-p2p` and `wasm-pack test -p mxdx-p2p` jobs
- `crates/mxdx-worker/src/webrtc.rs` is **deleted** (stub, replaced by new crate)
- `crates/mxdx-types/src/events/webrtc.rs` is **deleted** (divergent schema `org.mxdx.session.webrtc.*` replaced by standard `m.call.*` — see companion ADR `2026-04-15-mcall-wire-format.md`)
- Worker and client depend on `mxdx-p2p`; `mxdx-core-wasm` re-exports the wasm surface to `@mxdx/core`
- The five currently-open npm-era P2P beads (`mxdx-8y1`, `mxdx-vuy`, `mxdx-4yf`, `mxdx-eud`, `mxdx-xi6`) are superseded by Rust-side equivalents in the map phase

## Alternatives considered and rejected

- **Approach 1 (fully unified):** Rejected. Forces rewriting session lifecycle code that already works.
- **Approach 2 (distributed):** Rejected. State-machine duplication across worker and client is a drift vector, especially around security-critical code (Verifying state, glare resolution). Splitting signaling into `mxdx-matrix` also conflates event encoding with homeserver client logic.
