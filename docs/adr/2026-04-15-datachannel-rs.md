# ADR 2026-04-15: Use `datachannel-rs` for native WebRTC, `web-sys` for wasm

**Status:** Accepted
**Date:** 2026-04-15
**Implemented in:** Branch `brains/rust-p2p-interactive`, Phase 3 (T-31) + Phase 8 (T-80)
**Related:** `docs/plans/2026-04-15-rust-p2p-interactive-storm.md`, `docs/adr/2026-04-15-mxdx-p2p-crate.md`

## Context

The Rust port of interactive mode requires a WebRTC implementation with two properties:

1. **Semantic interoperability** with the existing npm peer (`node-datachannel`). The wire format (SDP shape, DTLS ciphers, SCTP DATA chunks) must be compatible enough that an npm peer and a Rust peer can negotiate and exchange data.
2. **Native + wasm** support from the same source tree via cfg-gating.

Three Rust options were evaluated:

| Option | Native | Wasm | Maturity | Interop with node-datachannel |
|---|---|---|---|---|
| `datachannel-rs` | ✅ (FFI to libdatachannel C++) | ❌ | High (libdatachannel is battle-tested) | **Identical** — shares the exact same C++ core as `node-datachannel` |
| `webrtc-rs` | ✅ (pure Rust) | ❌ (not supported) | Medium (Pion-derived, used in production) | Semantic (standards-compliant, but different SDP/ICE implementation) |
| `str0m` | ✅ (pure Rust, sans-IO) | Partial | Low (newest, smallest community) | Semantic |

None of the pure-Rust options compile cleanly to wasm (WebRTC in browsers is always provided by the browser runtime, not a library). For wasm, the only option is to bind the browser's `RTCPeerConnection` via `web-sys`.

## Decision

- **Native target** (`cfg(not(target_arch = "wasm32"))`): use `datachannel-rs` (v0.12 or latest compatible), which links to `libdatachannel` C++.
- **Wasm target** (`cfg(target_arch = "wasm32")`): use `web-sys::RtcPeerConnection` + `wasm-bindgen` + `wasm-bindgen-futures`.

Both are hidden behind the `WebRtcChannel` trait defined in `crates/mxdx-p2p/src/channel/mod.rs`. The trait surface is deliberately minimal (create/accept offer/answer, add ICE, send bytes, events receiver, close) — it exposes only the operations both backends can implement identically.

## Rationale

### Why `datachannel-rs` for native

- **Bit-for-bit shared core with npm peer.** `node-datachannel` and `datachannel-rs` are both thin FFI wrappers over the same libdatachannel C++ implementation. DTLS fingerprints, SCTP framing, ICE candidate priority, SDP structure — all identical. This is the single largest risk-reducer for cross-runtime interop tests (Q1 of the storm).
- **Battle-tested in production WebRTC.** libdatachannel has been used in telepresence, gaming, and IoT deployments for years.
- **Mature FFI.** The Rust bindings are well-maintained and follow typical FFI patterns.

### Why `web-sys` for wasm

- No other option. The browser's built-in `RTCPeerConnection` is the only way to do WebRTC in a browser runtime. Any attempt to ship a Rust/C++ implementation via wasm would produce a multi-megabyte binary, bypass the browser's hardware acceleration and ICE policy, and likely be blocked by browser security policy for RTP-like traffic.
- `web-sys` provides typed bindings; `wasm-bindgen-futures` converts JS promises to Rust futures for the async trait.

### Consequences accepted

- **Native builds have a C++ dependency chain.** libdatachannel pulls in OpenSSL/libsrtp/libjuice/usrsctp. This is a known cost; the project already has similar native deps (sqlite via matrix-sdk). Cross-compilation to musl targets will need care — documented in the implement phase.
- **SDP text is not byte-identical across native and wasm.** Browser `RTCPeerConnection` and libdatachannel produce textually different SDP blobs that are semantically compatible. Tests assert connection success, not SDP equality (see storm §1.3 "semantic interoperability").
- **Trait surface is leaky potential.** libdatachannel and browser WebRTC differ in buffer-state exposure, close-reason granularity, and some ICE edge cases. Mitigation: the trait exposes only the minimum required operations; anything backend-specific is handled inside the impl and translated to a common event vocabulary.

## Alternatives considered and rejected

- **`webrtc-rs` everywhere (native + compile to wasm via pure Rust):** rejected because webrtc-rs does not target wasm, and even if it did, a pure-Rust WebRTC stack in the browser would bypass the platform's RTP/ICE policy and be blocked.
- **libdatachannel compiled to wasm (via Emscripten):** rejected for performance (multi-megabyte payload, no hardware acceleration) and platform policy (browsers restrict raw UDP).
- **`str0m` for native:** rejected because it is younger and less community-tested; interop risk with `node-datachannel` is higher without a shared C++ core.

## Consequences

- New `Cargo.toml` entries (native only): `datachannel = "0.12"` and transitive `libdatachannel-sys` build
- New `Cargo.toml` entries (wasm only): `web-sys` features (`RtcPeerConnection`, `RtcDataChannel`, `RtcIceCandidate`, etc.), `wasm-bindgen`, `wasm-bindgen-futures`, `js-sys`
- CI: native builds on Linux/macOS require the libdatachannel native deps (OpenSSL headers etc.); a lightweight "build-only" CI job is added
- `wasm-pack` build for `mxdx-core-wasm` continues to work (libdatachannel does not enter the wasm dep graph due to cfg-gating)
