# Wrap-up: Rust Interactive Sessions (TURN + P2P)

**Slug:** rust-p2p-interactive
**Paused:** false
**Branch:** brains/rust-p2p-interactive
**Commits:** 50 (since diverging from main)
**Diff:** 131 files changed, 19,028 insertions, 516 deletions

## Per-Phase Summary

### Phase 0: Scaffolding
- Tasks completed: 4/4 (T-00 through T-03)
- Created `crates/mxdx-p2p` workspace skeleton
- Deleted divergent npm-era WebRTC schema (webrtc.rs, 174 lines)
- Added 4 CI jobs + `scripts/check-no-unencrypted-sends.sh` (E2EE grep gate)
- Issues found: pre-existing test failure in keychain_chain.rs (unrelated)

### Phase 1: P2PCrypto + Cross-Language Vectors
- Tasks completed: 4/4 (T-10 through T-12)
- P2PCrypto (AES-256-GCM) + SealedKey newtype
- Cross-language vector fixtures (byte-exact with npm)
- Megolm<Bytes> newtype with trybuild compile-time enforcement
- Architecture decision: semantic equivalence instead of byte-identity (later reversed in Phase 7 retrofit)

### Phase 2: TurnCredentials
- Tasks completed: 3/3 (T-20 through T-22)
- TURN credential fetch + TurnRefreshTask (TTL/2 virtual-time tested)
- Issues found: none

### Phase 3: WebRTC Channel Wrappers
- Tasks completed: 3/3 (T-30 through T-34)
- Native WebRtcChannel via datachannel-rs v0.16 FFI
- RestartIce returns RestartIceUnsupported (datachannel-sys 0.23 limitation)

### Phase 4: m.call.* Signaling + Glare
- Tasks completed: 5/5 (T-40 through T-44)
- Standard Matrix VoIP event types + parser + glare resolver
- Field-name reconciliation: `session_key` → `mxdx_session_key` (coordinated Rust+npm)
- New ADR: coordinated-rust-npm-releases.md

### Phase 5: P2PTransport State Machine
- Tasks completed: 5/5 (T-50 through T-54)
- 9-state P2PTransport with non-blocking try_send
- Ed25519-signed Verifying handshake transcript + orchestration
- 249 tests passing in mxdx-p2p crate

### Phase 6: Worker + Client Wiring
- Tasks completed: 8/8 (T-60 through T-64, mxdx-btk, mxdx-fqt, mxdx-clr)
- P2PTransport wired into mxdx-worker + mxdx-client daemon
- BatchedSender dynamic window flip (10ms P2P / 200ms Matrix)
- Matrix-backed HandshakeSigner via ephemeral-key hybrid (later retrofitted in Phase 7)
- npm Ed25519 handshake primitives (p2p-verify.js, coordinated release)
- npm perf baseline captured
- Zero regressions with p2p_enabled=false

### Phase 7: JS E2E Suite + Testing-Feature Retrofits
- Tasks completed: 10/10 (T-70 through T-75, mxdx-61h, mxdx-awe.52, mxdx-awe.53, mxdx-awe.51)
- **Testing-feature retrofits:**
  - Megolm fallback: byte-identical ciphertext via OlmMachine::encrypt_room_event_raw
  - Device-sign: direct OlmMachine::sign() replaces ephemeral-key flow
  - Handshake integration: p2p-verify.js wired into live flow with v=2 protocol negotiation
- **Beta E2E test suite:**
  - Single-HS + fallback + glare against ca1-beta
  - Federated P2P (ca1↔ca2)
  - 8-combination Rust/npm interop matrix
  - Perf gate (absolute SLOs + ±10% vs npm baseline)
  - Security suite (P0): wrong-peer sig, replay, plaintext fuzzer, crypto downgrade, signaling tamper
- New ADRs: matrix-sdk-testing-feature.md, ephemeral-key-cross-cert.md (Superseded)

### Phase 8: WASM + web-console
- Tasks completed: 6/6 (T-80 through T-84, T-83a)
- WasmWebRtcChannel (web-sys::RtcPeerConnection)
- P2P surface re-exported from mxdx-core-wasm
- web-console imports crypto/TURN from WASM
- npm shims deprecated (launcher stays on npm path, documented opt-out)
- 12 WASM tests (7 channel + 5 crypto), 257 native tests pass

### Phase 9: Default-On Flip + Cleanup
- Tasks completed: 5/5 (T-90, T-91, T-C0, T-C1, T-C2)
- `p2p_enabled` default flipped to `true`
- Nightly perf monitoring via `scripts/check-perf-streak.sh`
- 5 npm-era P2P beads superseded
- ADRs annotated with "Implemented in" commits
- MANIFEST.md updated
- Final security review: all gates passed

## Outstanding Work

| Bead | Priority | Description |
|---|---|---|
| mxdx-awe.48 | P2 | T-NUR: Monitor P2P health post-default-on |
| mxdx-61h (notes) | P1 | Phase 7 filed follow-ups for testing-feature deps |
| mxdx-5qp | P2 | npm subprocess wiring in interop test |
| mxdx-muk | P3 | .gitignore for test artifacts |
| mxdx-cpd | P2 | Musl cross-compile verification |
| mxdx-2oi | P2 | Document send_megolm signature tweak |

## Known Gaps and Limitations

1. **WASM P2PTransport driver not fully ported** — web-console still uses JS `P2PTransport` for orchestration; only crypto/TURN are WASM. Full WASM driver is follow-up work.
2. **web-console production build** — pre-existing issue: p2p-verify.js imports `node:crypto`, breaks Vite Rollup. Not a regression from this branch.
3. **Launcher stays on npm P2P path** — documented opt-out in T-83a. Shims marked launcher-internal, not deleted.
4. **Interop test npm combinations** — placeholdered (npm subprocess spawning needs mxdx-5qp follow-up).
5. **Musl cross-compile** — not verified (mxdx-cpd follow-up from Phase 3).

## Security Posture

- **Cardinal rule maintained:** Every Matrix event and every byte on the P2P data channel is E2EE.
- **Structural enforcement:** `Megolm<Bytes>` newtype (package-private constructor) + `SealedKey` (crate-private) make plaintext-on-wire a compile error. Trybuild tests enforce across refactors.
- **CI gates:** `check-no-unencrypted-sends.sh` grep gate + `wire-format-parity` cross-language vectors.
- **Testing-feature policy:** matrix-sdk `testing` feature enabled per ADR 2026-04-16-matrix-sdk-testing-feature.md. Review triggers documented.
- **Handshake:** Ed25519-signed transcript binding SDP fingerprints to Matrix identity, device-key-signed (not ephemeral-key).

## ADRs Created/Updated

| ADR | Status |
|---|---|
| 2026-04-15-mxdx-p2p-crate.md | Implemented |
| 2026-04-15-datachannel-rs.md | Implemented |
| 2026-04-15-mcall-wire-format.md | Implemented (+ addendum) |
| 2026-04-15-megolm-bytes-newtype.md | Implemented (+ 2 addenda) |
| 2026-04-16-coordinated-rust-npm-releases.md | Accepted (policy) |
| 2026-04-16-matrix-sdk-testing-feature.md | Accepted (policy) |
| 2026-04-16-ephemeral-key-cross-cert.md | Superseded (by Phase 7 retrofit) |

## Suggested Follow-up Plans

1. **WASM P2PTransport driver** — port the full state machine to WASM so web-console can drop the JS P2PTransport entirely.
2. **npm launcher → WASM migration** — when the unified worker vision (CLAUDE.md) is ready, migrate launcher off npm P2P shims.
3. **matrix-sdk upstream PR** — contribute a non-`testing`-gated `Client::olm_machine()` accessor to matrix-rust-sdk. Would let mxdx drop the `testing` feature.
