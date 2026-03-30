# WebRTC / P2P Porting to Rust — Design Document

> **Status**: Design only — no code changes in this phase.

## Context

The npm/WASM side has a complete P2P transport layer for low-latency interactive terminal sessions. This document maps the path for porting it to Rust. The existing design document at `docs/plans/2026-03-10-p2p-transport-design.md` remains the authoritative spec; this document focuses on Rust-specific implementation decisions.

## Components to Port

### 1. P2PTransport (State Machine)

**npm**: `packages/core/p2p-transport.js`
**Rust target**: `crates/mxdx-worker/src/p2p_transport.rs` (or a new `mxdx-p2p` crate)

**Approach**: Implement as a Rust enum-based state machine:
```rust
enum P2PState {
    Matrix,              // Using Matrix for all events
    Connecting(CallId),  // WebRTC connection in progress
    Verifying(Channel),  // Data channel open, peer verification pending
    P2P(VerifiedChannel), // Fully operational P2P
}
```

**Library**: `webrtc-rs` (pure Rust, no native deps) or `datachannel-rs` (libdatachannel FFI, battle-tested). Recommend `datachannel-rs` for parity with npm's `node-datachannel`.

**Key considerations**:
- State transitions driven by tokio channels, not callbacks
- Transparent fallback: `P2PTransport` implements a trait matching `WorkerRoomOps`/`ClientRoomOps`
- Idle timeout via `tokio::time::sleep`
- Reconnect backoff: reuse `SyncBackoff` pattern from Phase 6

### 2. P2PCrypto (AES-256-GCM)

**npm**: `packages/core/p2p-crypto.js`
**Rust target**: `crates/mxdx-types/src/p2p_crypto.rs`

**Approach**: Use `aes-gcm` crate (already in workspace from Phase 2 `FileKeychain`):
```rust
pub struct P2PCrypto {
    key: aes_gcm::Key<Aes256Gcm>,
}

impl P2PCrypto {
    pub fn generate() -> (Self, String) { /* random key, base64 export */ }
    pub fn from_base64(key: &str) -> Result<Self> { ... }
    pub fn encrypt(&self, plaintext: &[u8]) -> Result<EncryptedFrame> { ... }
    pub fn decrypt(&self, frame: &EncryptedFrame) -> Result<Vec<u8>> { ... }
}
```

**Wire format** (must match npm):
```json
{ "c": "<base64-ciphertext>", "iv": "<base64-96bit-iv>" }
```

### 3. P2PSignaling (Matrix VoIP Events)

**npm**: `packages/core/p2p-signaling.js`
**Rust target**: `crates/mxdx-matrix/src/p2p_signaling.rs`

Standard Matrix VoIP events via existing `send_event()`:
- `m.call.invite` — SDP offer + session key (encrypted by Megolm)
- `m.call.answer` — SDP answer
- `m.call.candidates` — Batched ICE candidates
- `m.call.hangup` — Tear down
- `m.call.select_answer` — Glare resolution

**Glare handling**: Lower lexicographic user_id wins (Matrix spec).

### 4. TURN Credentials

**npm**: `packages/core/turn-credentials.js`
**Rust target**: `crates/mxdx-matrix/src/turn.rs`

Simple HTTP call to `/_matrix/client/v3/voip/turnServer`:
```rust
pub async fn fetch_turn_credentials(
    homeserver: &str,
    access_token: &str,
) -> Result<Option<TurnCredentials>> { ... }
```

### 5. SessionMux (Multi-Session Router)

**npm**: `packages/launcher/src/runtime.js` (SessionMux class)
**Rust target**: `crates/mxdx-worker/src/session_mux.rs`

Routes events by `session_id` across a shared P2P channel:
```rust
pub struct SessionMux {
    sessions: HashMap<String, SessionHandle>,
    transport: Arc<P2PTransport>,
}
```

Batch window switching: 10ms (P2P) vs 200ms (Matrix) via `BatchedSender` config.

## Dependency Decision

| Option | Pros | Cons |
|---|---|---|
| `datachannel-rs` | FFI to battle-tested C++ lib, npm parity | Native dep, cross-compile complexity |
| `webrtc-rs` | Pure Rust, no native deps | Less mature, larger binary |
| `str0m` | Pure Rust, lightweight | Newer, less community adoption |

**Recommendation**: Start with `datachannel-rs` for maximum compatibility with the npm `node-datachannel` implementation. Evaluate `str0m` as a pure-Rust alternative if cross-compilation becomes an issue.

## Security Invariants

1. **Terminal data is always encrypted** — Megolm (via Matrix room E2EE) + AES-256-GCM (P2P layer). Double encryption is intentional defense-in-depth.
2. **Session key in E2EE invite** — The P2P AES key is sent inside a Megolm-encrypted `m.call.invite` event. NEVER in plaintext.
3. **Peer verification** — Challenge-response nonce exchange after data channel opens. Protects against TURN relay MitM.
4. **No unencrypted fallback** — If P2P crypto fails, fall back to Matrix (which has its own E2EE), never send unencrypted.

## Implementation Order

1. `P2PCrypto` — smallest, tests against known npm vectors
2. `P2PSignaling` — Matrix event wrappers
3. `TurnCredentials` — HTTP client call
4. `P2PTransport` — state machine + WebRTC integration
5. `SessionMux` — multi-session routing
6. Wire into worker + client binaries

## Testing Strategy

- **P2PCrypto**: Cross-ecosystem test vectors (encrypt in npm, decrypt in Rust and vice versa)
- **P2PSignaling**: Unit tests with mock Matrix client
- **P2PTransport**: Integration test with two local peers (no TURN needed for localhost)
- **E2E**: Full test with tuwunel, worker, client, verify P2P handshake + terminal session
- **Profiling**: Compare latency: Matrix-only vs P2P vs SSH baseline
