# Changelog

All notable changes to this project will be documented in this file.

The format is based on [Keep a Changelog](https://keepachangelog.com/en/1.1.0/),
and this project adheres to [Semantic Versioning](https://semver.org/spec/v2.0.0.html).

## [Unreleased]

### Added

- **P2P transport enabled by default.** Interactive terminal sessions now use
  peer-to-peer WebRTC data channels (with AES-256-GCM encryption over
  Megolm-authenticated keys) for dramatically lower latency. The P2P path is
  layered on top of the existing Matrix transport: if the data channel cannot
  be established, the system falls back to Matrix automatically with zero
  message loss. All traffic remains end-to-end encrypted on both paths.
  - Worker: `p2p.enabled` in `worker.toml` now defaults to `true`.
  - Client: `p2p.enabled` in `client.toml` now defaults to `true`.
  - Use `--no-p2p` (client CLI) or `p2p.enabled = false` (config file) to
    force Matrix-only mode for diagnostics or incident response.
- New `mxdx-p2p` crate: cross-platform P2P transport with `datachannel-rs`
  (native) and `web-sys` (WASM) backends behind a shared `WebRtcChannel` trait.
- `Megolm<Bytes>` newtype: compile-time enforcement that every payload passed
  to P2P or Matrix send paths has been Megolm-encrypted. Plaintext on the wire
  is a type error.
- `SealedKey` newtype: AES-256-GCM session key can only be constructed inside
  the crypto module and transported via Megolm-encrypted `m.call.invite`.
- Ed25519-signed Verifying handshake with device-key binding via
  `OlmMachine::sign()`.
- Standard Matrix VoIP `m.call.*` wire format for full interop between Rust
  and npm peers.
- Nightly perf monitoring CI (`e2e-beta-perf.yml`) with streak-based gating.

### Changed

- `P2pConfig.enabled` default flipped from `false` to `true` (Phase-9 T-91).
- npm P2P shims (`p2p-crypto.js`, `p2p-signaling.js`, `p2p-transport.js`)
  deprecated in favor of WASM exports from `@mxdx/core`.

### Security

- Every Matrix event and every byte on the P2P data channel is E2EE — no
  exceptions. The `Megolm<Bytes>` and `SealedKey` newtypes make plaintext
  transmission a compile-time error.
- `scripts/check-no-unencrypted-sends.sh` CI gate prevents `send_raw`,
  `skip_encryption`, or `unencrypted` patterns in production code.
- Trybuild tests verify `Megolm` and `SealedKey` constructors are
  inaccessible outside their defining crates.
