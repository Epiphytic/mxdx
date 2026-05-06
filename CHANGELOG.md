# Changelog

All notable changes to this project will be documented in this file.

The format is based on [Keep a Changelog](https://keepachangelog.com/en/1.1.0/),
and this project adheres to [Semantic Versioning](https://semver.org/spec/v2.0.0.html).

## [Unreleased]

### Added

- **WASM session-loop migration (Phase 4 — ADR 2026-04-29 Pillar 2).** The
  launcher session lifecycle, command routing, telemetry, batching, and P2P
  state tracking have been moved from JavaScript into `mxdx-core-wasm`. The
  npm `runtime.js` is now a thin OS-bound shell (265 lines, down from ~1700).
  New exports from `@mxdx/core`:
  - `WasmSessionManager` — pure-Rust session registry and command dispatcher.
    Accepts a batch of Matrix events, returns `SendAction` objects for JS to
    execute against the Matrix client and OS APIs.
  - `SessionTransportManager` — pure-Rust P2P connection state machine
    (refCounts, rate limits, attempt IDs, settled flags). NodeWebRTCChannel
    and P2PSignaling remain JS-side (OS-bound native addon).
  - `WasmBatchedSender` — buffers raw PTY bytes, compresses, and returns a
    ready-to-send Matrix event payload JSON string. Implements full
    `M_LIMIT_EXCEEDED` / 429 retry-with-coalesce semantics via a
    structured state-machine API (`takePayload` / `markSent` /
    `markRateLimited` / `markError` / `parseRetryAfterMs`); JS owns the
    `setTimeout` driving and the actual `sendEvent` call, WASM owns
    compression, sequencing, and in-flight payload retention. The launcher
    hot path (`packages/launcher/src/batched-sender-wasm.js`) drives this
    state machine.
  - `buildTelemetryPayload` — constructs the `org.mxdx.host_telemetry` state
    event payload from OS metrics supplied by JS.
  - `compressTerminalData` / `processTerminalInput` — zlib+base64 encode/decode
    with 1 MB decompression limit (zlib-bomb protection).

- **Canonical config schema (Phase 3 — ADR 2026-04-29 Pillar 1).** Both the
  Rust binaries and the npm packages now read and write `worker.toml` /
  `client.toml` in a flat top-level TOML key layout (no `[launcher]` or
  `[client]` section wrappers). Key changes for users:
  - **Automatic migration.** On first start after upgrade, a config file that
    still uses the legacy `[launcher]`/`[client]` wrapper is auto-migrated to
    the flat layout. The original is preserved as `<file>.legacy.bak` and a
    warning is printed to stderr. No data is lost.
  - **Security fields guaranteed to survive migration.** `authorized_users`,
    `allowed_commands`, and `trust_anchor` are verified to be byte-for-byte
    identical after migration. Silent loss of these fields is treated as a
    security defect, not graceful degradation.
  - **Forward compatibility.** Unknown TOML keys are silently ignored by both
    runtimes (no `deny_unknown_fields`). Fields added in future releases do not
    break older binaries reading the same config file.
  - **Cross-runtime field preservation.** npm `save()` now merges only its own
    fields; Rust-written fields (e.g. `authorized_users`) survive an npm
    config-writer round-trip unchanged.
  - **New fields in Rust types.** `WorkerConfig` gains `telemetry`, `use_tmux`,
    `batch_ms`, `p2p_batch_ms`, `p2p_advertise_ips`, `p2p_turn_only`,
    `registration_token`, `admin_user`. `ClientConfig` gains `batch_ms`,
    `p2p_batch_ms`, `registration_token`. All are `Option<T>` with serde
    defaults so existing configs remain valid.

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
