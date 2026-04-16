# Rust Interactive Sessions (TURN + P2P) — Storm Spec

**Date:** 2026-04-15
**Phase:** Storm (BRAINS pipeline)
**Epic:** `mxdx-awe`
**Next phase:** `/brains:architect`

## Problem Statement

Interactive terminal mode (TURN + P2P data channel) works in the npm+wasm code path today (`packages/core/p2p-*.js`, `packages/launcher/src/runtime.js`). The Rust binaries (`mxdx-client`, `mxdx-worker`) currently ship only a stub (`crates/mxdx-worker/src/webrtc.rs` that always bails with "WebRTC not available") and a divergent event schema (`crates/mxdx-types/src/events/webrtc.rs` defining `org.mxdx.session.webrtc.*` which does not interoperate with npm).

This spec ports interactive mode to Rust with three hard requirements:

1. **End-to-end tests** using the beta hosting infrastructure (`test-credentials.toml`, ca1-beta.mxdx.dev + ca2-beta.mxdx.dev), including federated calls across the two beta homeservers.
2. **No performance regressions** versus the npm path — both absolute SLOs and within ±10% of the npm baseline on every comparable metric.
3. **Cardinal rule preserved:** every Matrix event and every byte on the P2P channel is end-to-end encrypted. No exceptions.

## Constraints (answered during brainstorm)

| Q | Decision |
|---|---|
| Q1. Interop | **B.** Rust adopts npm's wire format. Standard Matrix VoIP `m.call.*` events. The existing `org.mxdx.session.webrtc.*` Rust types are deleted. |
| Q2. WebRTC library | **A.** `datachannel-rs` (FFI to libdatachannel) for native — same C++ core as npm's `node-datachannel`. |
| Q3. Scope | **B.** In scope for both native binaries AND the browser web-console. Shared `WebRtcChannel` trait with cfg-gated native (`datachannel-rs`) and wasm (`web-sys::RtcPeerConnection`) impls. |
| Q4. Perf baseline | **C.** Both absolute SLOs AND ±10% vs npm on every comparable metric. |
| Q5. Tests | **A+C.** JS E2E suite in `packages/e2e-tests/` (acceptance, spawns Rust binaries as subprocesses) + Rust per-crate unit/integration tests (fast feedback). Uses beta servers (not local tuwunel), includes federated topology. |

## Chosen Approach (of 3 considered) — **Hybrid**

A new `mxdx-p2p` crate owns the platform-agnostic pieces (trait + cfg-gated impls, `P2PTransport` state machine, `P2PCrypto`, signaling, `TurnCredentials`). Runtime wiring (SessionMux, BatchedSender integration, session lifecycle) stays in `mxdx-worker` and `mxdx-client` where it already lives. `mxdx-core-wasm` re-exports the wasm surface to `@mxdx/core`.

**Rejected:** Approach 1 (fully unified — too much rewrite of healthy worker/client code); Approach 2 (distributed across existing crates — state-machine duplication risk). Hybrid contains cross-cutting risk in one crate without forcing a worker/client rewrite.

### Delivery sequence (9 steps)

1. `P2PCrypto` + cross-language vectors
2. `TurnCredentials` (HTTP)
3. `WebRtcChannel` trait + native impl + loopback integration test
4. `m.call.*` signaling event helpers + glare resolver
5. `P2PTransport` state machine
6. Worker + client wiring, feature-flagged
7. JS E2E suite (beta, single-HS + federated) — perf gate lands here
8. WASM impl + `mxdx-core-wasm` re-export + web-console swap
9. Default-on flip after 3 consecutive nightly perf runs green

Each step has a test gate that must be green before rolling past it (see §5.10).

---

## §1 — Architecture

### 1.1 Layout

```
                      ┌────────────────────────────────┐
                      │         Matrix Homeserver        │
                      │  (signaling: m.call.* events,    │
                      │   inherited Megolm encryption)   │
                      │  (TURN: GET /voip/turnServer)    │
                      └──┬───────────────────────────┬───┘
              signaling +│ TURN creds   signaling + │ TURN creds
                fallback │ + fallback     fallback  │ + fallback
                         │                          │
              ┌──────────▼──────────┐    ┌──────────▼──────────┐
              │     mxdx-client     │    │     mxdx-worker     │
              │   (or @mxdx/core in │    │    (native binary)   │
              │   the browser)      │    │                     │
              │                     │    │                     │
              │ session layer       │    │ session_mux         │
              │   │                 │    │   │                 │
              │ BatchedSender       │    │ BatchedSender       │
              │ (10ms P2P/200ms Mx) │    │ (10ms P2P/200ms Mx) │
              │   │                 │    │   │                 │
              │ mxdx-p2p::          │    │ mxdx-p2p::          │
              │  P2PTransport       │    │  P2PTransport       │
              │   └── WebRtcChannel │    │   └── WebRtcChannel │
              │       (native,      │◄──►│       (native,      │
              │        datachannel-rs)   │        datachannel-rs)
              └─────────────────────┘    └─────────────────────┘
                         ▲                         ▲
                         │                         │
                         │  Same trait, alt impl   │
                         │  in browser web-console:│
                         │                         │
              ┌──────────┴─────────────────────────┘
              │  web-console (@mxdx/core via mxdx-core-wasm)
              │   mxdx-p2p::P2PTransport
              │    └── WebRtcChannel (wasm, web-sys::RtcPeerConnection)
              └─────────────────────────────────────────────────
```

### 1.2 Cardinal-rule invariants

1. **Every byte on the P2P data channel is Megolm-ciphertext.** `MatrixClient` encrypts content for the room *before* P2P sees it. WebRTC DTLS is defense-in-depth, never the sole encryption.
2. **Matrix is always primary.** Sessions start on Matrix and remain on Matrix while P2P is establishing or torn down.
3. **Signaling events inherit room E2EE.** `m.call.invite/answer/candidates/hangup/select_answer` pass through the same `MatrixClient::send_event` path as every other event in the (encrypted) session room. No separate code path.
4. **TURN creds are short-lived.** Fetched per call from `/_matrix/client/v3/voip/turnServer`, rotated mid-session at `TTL/2` via ICE restart (not full tear-down), never persisted.
5. **Glare resolution uses the Matrix rule:** lower lexicographic `user_id` wins.
6. **Idle timeout 5 min** by default. Teardown releases TURN; reconnect is automatic on next activity.

### 1.3 Interoperability target — **semantic**, not byte-identical SDP

(Council refinement #1.) Wire compatibility with npm is byte-identical where it matters:
- AES-GCM frame JSON (`{c, iv}`)
- Megolm ciphertext envelope
- Matrix event JSON (call events, room events, to-device events)
- Base64/zlib framing of terminal data

SDP text itself is **semantically** interoperable — the two peers' `RTCPeerConnection` implementations (libdatachannel native vs browser `RTCPeerConnection`) will produce textually different but semantically equivalent SDP blobs. The negotiation still works because SDP is an interoperability protocol, not a bit-for-bit format. Tests assert the *connection succeeds*, not that the SDP strings match.

### 1.4 What stays unchanged

- `BatchedSender` shape (only its target is swapped; batch window switches from 200ms to 10ms when P2P open)
- `TerminalSocket` framing (zlib + base64 + sequence numbers)
- Session lifecycle, telemetry, retention, all session-room semantics
- Existing Matrix sync, room state, encryption pipelines

### 1.5 What's new

- `mxdx-p2p` crate (§2)
- Feature flag `p2p_enabled` (TOML) on both worker and client
- JS E2E test wave against beta servers (§5)

---

## §2 — Components

### 2.1 `mxdx-p2p` crate layout

```
crates/mxdx-p2p/
├── Cargo.toml                     # native deps gated under cfg(not(target_arch="wasm32"))
│                                  # wasm deps gated under cfg(target_arch="wasm32")
├── src/
│   ├── lib.rs                     # public re-exports + cfg-routing
│   ├── crypto.rs                  # P2PCrypto (AES-256-GCM, npm-vector-compatible)
│   ├── turn.rs                    # fetch_turn_credentials + TurnCredentials struct
│   ├── signaling/
│   │   ├── mod.rs
│   │   ├── events.rs              # m.call.invite/answer/candidates/hangup/select_answer
│   │   ├── glare.rs               # lexicographic user_id resolver (pure function)
│   │   └── parse.rs               # call event parsers
│   ├── channel/
│   │   ├── mod.rs                 # WebRtcChannel trait + ChannelEvent enum
│   │   ├── native.rs              # cfg(not(target_arch="wasm32")) — datachannel-rs
│   │   └── wasm.rs                # cfg(target_arch="wasm32") — web-sys::RtcPeerConnection
│   └── transport/
│       ├── mod.rs                 # P2PTransport public API
│       ├── state.rs               # P2PState enum + transitions
│       ├── driver.rs              # tokio::select (native) / wasm-bindgen-futures (wasm)
│       └── idle.rs                # idle-timeout watchdog
└── tests/
    ├── crypto_vectors.rs          # npm-generated vectors decrypted in Rust
    ├── glare_resolution.rs        # proptest
    ├── signaling_parse.rs         # golden + fuzz
    └── loopback.rs                # native-only: two P2PTransports via in-memory channels
```

### 2.2 `WebRtcChannel` trait — deliberately minimal

(Council refinement: trait exposes only what libdatachannel and browser WebRTC agree on semantically. No buffer-state, no close-reason granularity, no SCTP-level knobs.)

```rust
#[cfg_attr(not(target_arch = "wasm32"), async_trait::async_trait)]
#[cfg_attr(target_arch = "wasm32", async_trait::async_trait(?Send))]
pub trait WebRtcChannel {
    async fn create_offer(&mut self, ice_servers: &[IceServer]) -> Result<Sdp>;
    async fn accept_offer(&mut self, ice_servers: &[IceServer], remote: Sdp) -> Result<Sdp>;
    async fn accept_answer(&mut self, remote: Sdp) -> Result<()>;
    async fn add_ice_candidate(&mut self, c: IceCandidate) -> Result<()>;
    async fn restart_ice(&mut self, new_ice_servers: &[IceServer]) -> Result<Sdp>; // for TURN refresh
    async fn send(&self, frame: &[u8]) -> Result<()>;
    fn events(&mut self) -> &mut mpsc::Receiver<ChannelEvent>;
    async fn close(&mut self, reason: &str) -> Result<()>;
}

pub enum ChannelEvent {
    LocalIce(IceCandidate),
    Message(Bytes),
    Open,
    Closed { reason: String },
    Failure(String),
}

pub struct IceServer { pub urls: Vec<String>, pub username: Option<String>, pub credential: Option<String> }
pub struct Sdp { pub kind: SdpKind, pub sdp: String }
pub enum SdpKind { Offer, Answer }
```

### 2.3 `P2PCrypto` — AES-256-GCM, wire-locked to npm

```rust
pub struct P2PCrypto { key: Key<Aes256Gcm> }

impl P2PCrypto {
    pub fn generate() -> (Self, SealedKey);                  // SealedKey is the Sealed<P2PCryptoKey> newtype
    pub fn from_sealed(k: SealedKey) -> Self;
    pub fn encrypt(&self, plaintext: &[u8]) -> Result<EncryptedFrame>;
    pub fn decrypt(&self, frame: &EncryptedFrame) -> Result<Vec<u8>>;
}

#[derive(Serialize, Deserialize)]
pub struct EncryptedFrame {
    #[serde(rename = "c")] pub ciphertext: String,        // base64
    #[serde(rename = "iv")] pub iv: String,                // base64, 96-bit
}

// Council refinement #4: type-system enforcement of "encrypted payloads only"
pub struct Megolm<T>(T);                                   // constructor is pub(in mxdx_matrix)
pub struct SealedKey(pub(crate) Key<Aes256Gcm>);           // constructor is pub(in crate::crypto)
```

Wire format is bit-exact with `packages/core/p2p-crypto.js`; cross-language vectors in `tests/crypto_vectors.rs`.

### 2.4 `TurnCredentials` — with active-call refresh

(Council refinement #2.)

```rust
pub struct TurnCredentials {
    pub uris: Vec<String>,
    pub username: String,
    pub password: String,
    pub ttl: Duration,
    pub fetched_at: SystemTime,
}

pub async fn fetch_turn_credentials(homeserver: &Url, token: &str) -> Result<Option<TurnCredentials>>;

impl TurnCredentials {
    pub fn expires_at(&self) -> SystemTime;
    pub fn refresh_at(&self) -> SystemTime;                 // = fetched_at + ttl/2
    pub fn is_expired(&self) -> bool;
}
```

**Active-call refresh policy** (new in this spec):
- Background task per active `P2PTransport` wakes at `refresh_at()`
- Fetches new creds; if successful, calls `WebRtcChannel::restart_ice(new_ice_servers)` — swaps creds without tearing down the data channel
- If fetch fails: retry with backoff up to TTL; at TTL expiry without success, hang up with reason `turn_expired` and fall back to Matrix (next `try_send` triggers a fresh `start()` which will retry TURN fetch)
- **Expiry-during-reconnect race:** if a reconnect is pending AND creds are expired, the reconnect path re-fetches creds *before* sending `m.call.invite`, not in parallel. This serialization is explicit in the state machine (§2.6).

### 2.5 Public surface used by binaries

```rust
pub struct P2PTransport { /* owns state + tx/rx channels */ }

impl P2PTransport {
    // Council refinement #4: signature takes Megolm<Bytes>, not raw &[u8]
    pub async fn try_send(&self, payload: Megolm<Bytes>) -> SendOutcome;
    pub async fn start(&self, peer_user_id: &UserId, peer_device_id: Option<&DeviceId>);
    pub async fn hangup(&self, reason: &str);
    pub fn state(&self) -> P2PState;
    pub fn incoming(&mut self) -> &mut mpsc::Receiver<Megolm<Bytes>>;
}

pub enum SendOutcome { SentP2P, FallbackToMatrix, ChannelClosed }
```

`Megolm<Bytes>` is only constructible by `MatrixClient::encrypt_for_room`. This is structural enforcement of "no plaintext on the wire" — callers *cannot* pass plaintext because the type system refuses.

### 2.6 `P2PTransport` state machine

```rust
pub enum P2PState {
    Idle,
    FetchingTurn { since: Instant },                      // explicit state to serialize TURN+invite
    Inviting { call_id: CallId, started: Instant, our_offer: Sdp },
    Answering { call_id: CallId, party_id: PartyId },
    Glare { our_call: CallId, their_call: CallId, resolution: GlareResult },
    Connecting { call_id: CallId, channel: Box<dyn WebRtcChannel> },
    Verifying { call_id: CallId, channel: Box<dyn WebRtcChannel>, nonce: [u8; 32] },
    Open { call_id: CallId, channel: Box<dyn WebRtcChannel>, crypto: P2PCrypto, last_io: Instant },
    Failed { reason: String, retry_after: Instant },
}
```

The driver runs a single `tokio::select!` (native) or `wasm_bindgen_futures::spawn_local` loop (wasm) consuming: inbound `m.call.*` events, inbound channel events, outbound payloads (non-blocking), idle-timer ticks, turn-refresh ticks, explicit `start()`/`hangup()`.

### 2.7 `mxdx-matrix` integration

One new helper `send_call_event(&room_id, kind, payload)` (thin wrapper over existing `send_event`); one sync-filter addition for `m.call.*` types. No new crypto, no new sync logic — Megolm encryption is inherited from the existing pipeline.

### 2.8 Worker integration (`mxdx-worker`)

- `src/lib.rs` constructs `P2PTransport` per session-mux entry when `p2p_enabled = true`
- `src/session_mux.rs` routes outbound payloads through `try_send` first; on `FallbackToMatrix` uses existing path
- `src/batched_sender.rs` flips to 10ms window when transport reports `Open`
- **Deletes:** `src/webrtc.rs` (stub, replaced by new crate); `crates/mxdx-types/src/events/webrtc.rs` (divergent schema, replaced by `mxdx-p2p::signaling::events`)

### 2.9 Client integration (`mxdx-client`)

Symmetric to worker. `daemon/` constructs `P2PTransport` per attached session. New CLI flag `--no-p2p` for diagnostics.

### 2.10 `mxdx-core-wasm` re-export

```js
// @mxdx/core (browser)
import { P2PTransport, P2PCrypto, fetchTurnCredentials } from '@mxdx/core';
```

`packages/core/p2p-*.js` become deprecated shims importing from the new wasm exports — kept for one release cycle, then deleted. Web-console swap is delivery step 8.

### 2.11 Beads triage

Currently-open P2P-in-npm beads: `mxdx-8y1` (P2P WebRTC wrappers), `mxdx-vuy` (P2P Signaling), `mxdx-4yf` (P2P Integration), `mxdx-eud` (P2P UI), `mxdx-xi6` (P2P E2E). These describe npm-side work superseded by this Rust port. Handled as a tracking pass during `/brains:architect` (when the new Rust-side issues are created).

---

## §3 — Data Flow

### 3.1 Call setup (with council-strengthened verification)

Both sides reach `Idle` when terminal session goes live. Lower-`user_id` side initiates.

```
Worker (peer W)                     Matrix HS                     Client (peer C)
   │ GET /voip/turnServer ────────────────►                                      │
   │◄── TurnCredentials ──                                                        │
   │ P2PState = FetchingTurn → Inviting                                           │
   │                                                                              │
   │ P2PCrypto::generate() → (instance, SealedKey)                                │
   │ WebRtcChannel::create_offer(ice_servers) → sdp_offer                         │
   │                                                                              │
   │ MatrixClient::send_call_event(room, InviteEvent {                            │
   │   call_id, party_id, version: "1", lifetime: 30s,                            │
   │   offer: { type, sdp },                                                      │
   │   mxdx_session_key: sealed_key,      // room E2EE protects this              │
   │   session_uuid                                                               │
   │ })                                                                           │
   │ ──── m.call.invite (Megolm-encrypted by room) ──────────►                   │
   │                                           ──────────────► parse, verify      │
   │                                                          P2PCrypto::from_sealed(key)
   │                                                          accept_offer → sdp_answer
   │                                                          send_call_event(AnswerEvent{…})
   │                                           ◄── m.call.answer ────────────────
   │◄── accept_answer                                                             │
   │                                                                              │
   │ ──── m.call.candidates (batched 100/200ms) ◄─► ─────────────────────────────│
   │                                                                              │
   │ ChannelEvent::Open → state = Verifying                                       │
   │                                                                              │
   │                                                                              │
   │ Verifying transcript (council refinement #3):                                │
   │   transcript = domain_sep_tag                                                │
   │              || room_id                                                      │
   │              || session_uuid                                                 │
   │              || call_id                                                      │
   │              || our_nonce (32 random bytes)                                  │
   │              || peer_nonce (received over AES-GCM)                           │
   │              || our_party_id                                                 │
   │              || peer_party_id                                                │
   │              || sdp_fingerprint  (both peers' DTLS cert hashes from SDPs)    │
   │   signature = Ed25519_sign(device_key, transcript)                           │
   │                                                                              │
   │   Both sides exchange (nonce, signature) inside AES-GCM frames.              │
   │   Each verifies peer's signature against MatrixClient::device_keys(user, dev).ed25519.
   │   Mismatch → immediate hangup, audit log, mark device unverified_p2p for session.
   │                                                                              │
   │ → state = Open                                                               │
   │ BatchedSender flips to 10ms window                                           │
```

**Why the expanded transcript (council refinement #3):**
- `room_id + session_uuid + call_id` binds the verification to this specific call, preventing cross-call replay
- `our_party_id + peer_party_id` binds direction, preventing reflection attacks
- `sdp_fingerprint` binds the DTLS handshake to the Matrix-verified peer identity — the DTLS cert must be the one both sides negotiated
- `domain_sep_tag` is a constant ASCII string (`"mxdx.p2p.verify.v1"`) that prevents the signed blob from being misused as a signature over anything else
- `our_nonce + peer_nonce` ensures freshness

Note: DTLS fingerprint validation in SDPs already provides a first layer of TURN-MITM resistance (a MITM relay cannot forge the cert hash without breaking the Megolm-encrypted SDP). This Verifying step is defense-in-depth on top.

### 3.2 Steady-state outbound

```
caller code (e.g. exec stdout chunk)
   │
   ▼
BatchedSender::send(content)                            // accumulates within batch_window
   │ on flush:
   ▼
MatrixClient::encrypt_for_room(room_id, type, content) → Megolm<Bytes>
   │
   ▼
P2PTransport::try_send(megolm)                          // takes Megolm<Bytes>, not raw
   │
   ├──► state==Open?  → P2PCrypto::encrypt(megolm.0) → WebRtcChannel::send(frame_json)
   │                    → SendOutcome::SentP2P
   │
   └──► else          → SendOutcome::FallbackToMatrix
                          ▼
                       MatrixClient::send_megolm(room_id, megolm)  // identical payload
```

Both transports produce semantically equivalent Megolm-encrypted bytes against the same room session — fallback is transparent because the receiver's decrypt pipeline yields the same plaintext regardless of which path delivered it. (See ADR `2026-04-15-megolm-bytes-newtype.md` addendum: the ciphertexts are not byte-identical — matrix-sdk 0.16's encrypt_room_event_raw is pub(crate) — but the security invariant of "every byte on the wire is Megolm-encrypted by the same room session" holds on both paths.)

### 3.3 Steady-state inbound

```
WebRtcChannel::events() emits ChannelEvent::Message(frame_bytes)
   │
   ▼
parse EncryptedFrame { c, iv } → P2PCrypto::decrypt → Megolm<Bytes>
   │
   ▼
MatrixClient::decrypt_megolm(room_id, megolm) → DecryptedTimelineEvent
   │
   ▼
deliver to session_mux (same code path as Matrix-arrived events)
```

Receivers cannot tell which transport delivered a given event.

### 3.4 Idle timeout and reconnect (with TURN refresh coherent)

```
P2PState = Open, last_io updated on every send/recv, refresh_timer set to TTL/2
   │
   ├── refresh_timer fires:
   │     fetch_turn_credentials → TurnCredentials_new
   │     WebRtcChannel::restart_ice(new_ice_servers)
   │     (no state transition; channel keeps flowing)
   │
   ├── 5 minutes with no I/O:
   │     send m.call.hangup(reason="idle_timeout")
   │     close channel, release TURN
   │     state = Idle
   │
   ├── TURN expires with no refresh success:
   │     send m.call.hangup(reason="turn_expired")
   │     state = Failed { retry_after = now + 30s }
   │
   ▼
Next try_send():
   ├── state in {Idle, Failed(ready)} → FallbackToMatrix immediately,
   │                                      trigger start() in background (FetchingTurn → Inviting)
   ▼
caller posts via Matrix (no perceptible user latency)
```

`try_send` never blocks waiting for P2P. Caller's payload goes via Matrix immediately on every transitional state.

### 3.5 Glare resolution (unchanged from prior draft)

Both peers may issue `m.call.invite` for the same conceptual call. Deterministic rule: lower lexicographic `user_id` wins; loser hangs up own call and answers winner's; winner sends `m.call.select_answer`. `signaling::glare::resolve(...)` is a pure function — property-tested (§5.7) to always agree.

### 3.6 Cross-runtime mixed deployment — semantic interop

(Council refinement #1.) Every signaling event body, ICE candidate, AES-GCM frame, Megolm payload, and verification transcript structure is bit-identical across runtimes. SDP text is semantically equivalent (different generators, negotiated via standard WebRTC).

Four combinations × 2 topologies (single-HS, federated) = 8 tests in §5.4.

### 3.7 Signaling in encrypted rooms

Session rooms are E2EE (Megolm + MSC4362). `m.call.*` events in these rooms are Megolm-encrypted by the existing send path. The SDP, ICE, and embedded `mxdx_session_key` are protected against passive observers — only joined devices can decrypt them. This is what makes putting the AES key in the invite safe.

---

## §4 — Error Handling, Fallback, Security

### 4.1 Failure taxonomy

| Where | Failure | State transition | User effect |
|---|---|---|---|
| `fetch_turn_credentials` | HTTP 4xx/5xx, timeout, no TURN configured | `Idle` (stay Matrix-only); telemetry warns | None |
| Mid-call TURN refresh | Fetch fails | Retry with backoff until TTL expires; then hang up | None unless expiry hit |
| `create_offer` / `accept_offer` / `accept_answer` | Bad SDP, ICE init error | `Failed { retry_after = +30s }` | None |
| `m.call.invite` send | Matrix send error | `Failed { retry_after = backoff }` | None |
| Invite timeout (30s lifetime) | No answer | `Failed { retry_after = +30s }` | None |
| ICE never reaches `connected` (30s) | No candidate pair | `Failed { retry_after = +60s }`, hang up | None |
| Verifying: wrong signature | **Auth failure** | Immediate hangup, do NOT retry, mark device `unverified_p2p` for session, audit log | None (falls back to Matrix) |
| Verifying: replay detected (nonce reuse, stale transcript) | **Auth failure** | Same as above | None |
| `Open` then channel close/error | Channel drop | `Idle`; auto re-invite on next `try_send` | One batch may go via Matrix |
| `P2PCrypto::decrypt` failure inbound | Tag mismatch | Drop frame; if rate > 3/sec, hangup + `Failed { retry_after = +60s }` | None unless persistent |
| Megolm decrypt fail on P2P-arrived payload | Same as Matrix-arrived | Existing handling (decrypt-failed banner) | Existing UX |
| Megolm *encrypt* fail outbound | Room session not yet established | Hard error to caller; Matrix path also fails (same requirement) | Existing Matrix-path error |

### 4.2 Backoff + retry

Reuse `SyncBackoff` from Phase 6: initial 5s, exponential to 5min cap, full-jitter. Three consecutive Verifying failures against the same peer device → mark `unverified_p2p` for rest of session.

### 4.3 Fallback is transparent

Every `try_send` returns one of three outcomes immediately; there is no awaitable "wait for P2P" on the hot send loop. Caller code:

```rust
match transport.try_send(megolm).await {
    SendOutcome::SentP2P => Ok(()),
    SendOutcome::FallbackToMatrix | SendOutcome::ChannelClosed => {
        matrix_client.send_megolm(room, megolm).await
    }
}
```

### 4.4 Resource limits

| Resource | Limit | Why |
|---|---|---|
| Inbound frame size | 1 MiB | Matches existing `TerminalSocket` bomb cap |
| Inbound decrypt-failure rate | 3/sec → hangup | Defends against tag-spam from a relay |
| Outbound queue depth | 256 frames → overflow returns `FallbackToMatrix` | Prevents memory blowup on stalled channel |
| Concurrent `P2PTransport` | Unbounded by crate; capped by existing `session_mux` | Already bounded |
| TURN allocations | One per active call; released on hangup/idle | Avoids leaks |
| ICE candidates per `m.call.candidates` | 100 max, flush every 200ms | Matches npm batching |

### 4.5 Security invariants — chokepoints

(Council refinement #4 strengthens #1; refinement #5 strengthens the federated audit.)

| Invariant | Chokepoint | Negative test |
|---|---|---|
| **No plaintext on P2P (structural)** | `try_send` signature takes `Megolm<Bytes>`, constructible only by `MatrixClient::encrypt_for_room`. *Plaintext cannot compile.* | A test attempting to construct `Megolm` outside `mxdx-matrix` asserts compile failure (trybuild). |
| **No plaintext on signaling** | `MatrixClient::send_call_event` is `send_event` with call type — already Megolm-encrypted because rooms are E2EE. CI grep gate fails on any `send_raw\|skip_encryption\|unencrypted` match in `mxdx-p2p`. | `scripts/check-no-unencrypted-sends.sh` fails build on any match. |
| **AES key never in plaintext** | `SealedKey` newtype constructible only in `crypto.rs`; `build_invite(sealed_key)` takes it; invite event field is `pub(crate)`. | Trybuild test: constructing `SealedKey` outside crate fails. |
| **Peer verification bound to identity + call + SDP** | Verifying transcript includes `room_id + session_uuid + call_id + party_ids + sdp_fingerprint + nonces + domain_sep_tag`, signed by device Ed25519. State machine cannot reach `Open` without passing Verifying. | Integration test returns wrong signature; asserts channel never reaches `Open`, audit event emitted. |
| **No plaintext fallback** | `FallbackToMatrix` invokes `MatrixClient::send_megolm` which routes through `room.send_raw` → same Megolm room session encrypts in-flight. No code path posts original plaintext. Ciphertext is not byte-identical across transports (see ADR addendum) but both paths produce Megolm-encrypted bytes. | Mock `P2PCrypto::encrypt` to fail; assert caller still emits a Megolm-encrypted event against the same room session via the Matrix path (not necessarily byte-identical to what P2P would have produced). |
| **Idle teardown releases TURN** | Idle watchdog → `hangup` → `WebRtcChannel::close` → `PeerConnection::drop` releases TURN. | Native test asserts `PeerConnection` is dropped after idle hangup. |
| **Glare cannot deadlock** | `glare::resolve` is pure; property test asserts both peers always agree and resolution is total/deterministic. | Proptest over `(user_id, call_id)` pairs. |

### 4.6 Telemetry

Reuse existing `telemetry.rs` event bus. Add:
- `p2p.state_transition { from, to, reason, session_uuid }`
- `p2p.handshake_completed { session_uuid, total_ms, ice_ms, dtls_ms, verify_ms }`
- `p2p.turn_refresh { session_uuid, outcome }`
- `p2p.fallback { session_uuid, reason }`
- `p2p.security_event { kind: "verify_failure" | "decrypt_storm" | "replay_detected" | "wrong_peer", session_uuid, peer }` (always on)

### 4.7 Feature flag and rollout

`p2p_enabled: bool` in worker and client TOML config. Default **false** until perf SLOs (§5.5) green on three consecutive nightlies, then default **true**. Read once at process start; flip via config + restart.

---

## §5 — Testing Strategy (against beta servers, including federated)

### 5.1 Pyramid

```
┌─────────────────────────────────────────────────────────┐
│ packages/e2e-tests/tests/ (JS, against beta servers)     │ ← acceptance gate
│   spawns Rust binaries; logs into ca1-beta + ca2-beta    │   proves interop, perf, security
│   via test-credentials.toml                              │   at the user-visible level
│                                                          │
│ • rust-p2p-beta-single-hs.test.js                        │
│ • rust-p2p-beta-federated.test.js        ← federated     │
│ • rust-p2p-beta-fallback.test.js                         │
│ • rust-p2p-beta-glare.test.js                            │
│ • rust-p2p-beta-perf.test.js             ← perf gate     │
│ • rust-npm-interop-beta.test.js          ← Q1 proof      │
│ • rust-p2p-beta-security.test.js                         │
└──┬──────────────────────────────────────────────────────┘
   │ (skip cleanly when test-credentials.toml absent)
┌──┴──────────────────────────────────────────────────────┐
│ per-crate `tests/` (Rust, no homeserver needed)          │ ← integration
│ • mxdx-p2p/tests/loopback.rs                             │
│ • mxdx-p2p/tests/glare_resolution.rs                     │
│ • mxdx-p2p/tests/crypto_vectors.rs      ← cross-lang     │
│ • mxdx-p2p/tests/signaling_parse.rs                      │
│ • mxdx-worker/tests/p2p_wiring.rs                        │
│ • mxdx-client/tests/p2p_wiring.rs                        │
└──┬──────────────────────────────────────────────────────┘
┌──┴──────────────────────────────────────────────────────┐
│ src/ #[cfg(test)] mod tests — unit, pure logic          │
└──────────────────────────────────────────────────────────┘
```

### 5.2 New shared helper `packages/e2e-tests/src/beta.js`

Factor the inlined `loadCredentials()` from six existing test files. Add helpers:

```js
export function loadBetaCredentials();
export function skipIfNoBetaCredentials(t);
export async function loginBeta(creds, accountName, hsName);  // returns WasmMatrixClient or RustClientHandle
export async function provisionFederatedRoom(c1, c2);          // cross-HS room, encrypted
export async function provisionSameHsRoom(c1, c2);              // both on ca1
```

`loginBeta` returns whichever client shape the caller needs — lets one test file express all four interop combinations.

**Timing-tolerance helper** (council refinement #6):
```js
export function assertTimingTolerant(observedMs, expectedMs, toleranceMs = 200);
// Avoids per-tick assertions during JS shim deprecation.
```

### 5.3 Cross-language vector tests (no HS)

`mxdx-p2p/tests/crypto_vectors.rs` runs offline against checked-in vectors generated by npm code; plus a Node sidecar test in `packages/e2e-tests/tests/rust-npm-crypto-vectors.test.js` that decrypts Rust-generated frames from the JS side. Generator script at `packages/e2e-tests/scripts/regenerate-p2p-vectors.mjs`.

### 5.4 Four interop combinations × 2 topologies

|  | Same HS (ca1 only) | Federated (ca1↔ca2) |
|---|---|---|
| Rust client → Rust worker | t1a | **t1b (federated)** |
| npm client → Rust worker | t2a | **t2b (federated)** |
| Rust client → npm launcher | t3a | **t3b (federated)** |
| npm client → npm launcher | t4a | t4b (regression) |

Per test: 100 keystrokes, assert decrypted echoes in order, assert `transport=p2p` for ≥95% of messages.

### 5.5 Perf gate (Q4=C)

Reuses `perf-terminal.test.js` harness. Each metric collected for both Rust and npm against the same beta target.

| Metric | Single beta HS SLO | Federated SLO | Comparison |
|---|---|---|---|
| Handshake latency | ≤ 3,000 ms | ≤ 6,000 ms | Rust ±10% vs npm |
| Keystroke RTT | ≤ 100 ms | ≤ 250 ms | Rust ±10% vs npm |
| First-byte-after-fallback | ≤ 350 ms | ≤ 700 ms | Rust ±10% vs npm |
| Throughput (sustained) | ≥ 6 MB/s | ≥ 3 MB/s | Rust ±10% vs npm |
| Memory steady-state | ≤ 200 MB | ≤ 200 MB | Rust ±10% vs npm |
| CPU steady-state | ≤ 50% of core | ≤ 50% of core | Rust ±10% vs npm |

Test fails if any metric exceeds absolute SLO OR Rust more than 10% worse than npm. Median of 5 runs; per-run variance > 25% triggers single retry then hard fail.

**Network weather mitigation:** per-run HS-to-HS RTT measured; Rust comparison normalized by subtracting RTT floor. RTT > 200ms discards run, max 2 retries.

Results write to `packages/e2e-tests/results/rust-p2p-beta-perf-<git-sha>.json`.

### 5.6 Security tests on beta

`rust-p2p-beta-security.test.js`:
- **Wrong peer signature** — fixture worker with test-only flag; assert client never enters `Open`, emits `security_event{kind:"verify_failure"}`, falls back to Matrix.
- **Replay detection** — replay a captured verified nonce from a prior call; assert rejection with `security_event{kind:"replay_detected"}`.
- **Plaintext-on-wire fuzzer** — instrument with `wire-tap` build feature; run 1-min federated session; assert no logged frame parses as `org.mxdx.terminal.data` plaintext.
- **Crypto downgrade** — inject corrupted AES-GCM frames via `wire-tap`; assert rate-limited hangup after 3/sec.
- **Signaling tamper** — feed corrupted invite to parser; assert clean error, no `P2PCrypto` instance.
- **CI grep gate** — `check-no-unencrypted-sends.sh`, every push.
- **Federated key-leak audit (council refinement #5)** — privileged observer account on BOTH homeservers subscribes to the session room timeline AND the to-device streams for both participating accounts; captures all events for first 30s of a session; asserts no event of any type (room timeline OR to-device) contains plaintext matching the random terminal payloads we sent.

### 5.7 State-machine / protocol unit tests

Table-driven state-transition tests in `transport/state.rs` (`from × event → to`, with illegal-transition cases). `tokio::time::pause/advance` for idle-timer tests (ms-fast). Property-based glare resolution in `glare_resolution.rs` with proptest.

### 5.8 Loopback (native only)

`mxdx-p2p/tests/loopback.rs` wires two `P2PTransport` instances via in-memory channels (no Matrix, no TURN). Proves the full machine integrates in < 2s per run, every push.

### 5.9 Wasm wave (delivery step 8)

`wasm-pack test --headless --firefox crates/mxdx-p2p` for channel surface (no HS). Plus Playwright `web-console-rust-p2p-beta.test.js`: web-console against ca1-beta, Rust worker against ca2-beta, federated 30s session.

### 5.10 CI matrix

| Job | Trigger | Needs beta? | Runs |
|---|---|---|---|
| `rust-unit` | every push | no | `cargo test -p mxdx-p2p -p mxdx-types -p mxdx-matrix` |
| `rust-loopback` | every push | no | `cargo test -p mxdx-p2p --test loopback` |
| `cross-vectors` | every push | no | rust + node vector tests |
| `e2e-beta-single-hs` | every PR | yes | `rust-p2p-beta-single-hs` + fallback + glare |
| `e2e-beta-federated` | every PR | yes | `rust-p2p-beta-federated` + `rust-npm-interop-beta` |
| `e2e-beta-perf` | every PR + nightly | yes | `rust-p2p-beta-perf` (5 runs, median, ±10%) |
| `e2e-beta-security` | every PR | yes | `rust-p2p-beta-security` |
| `wasm-smoke` | every PR (post-wave 8) | yes | wasm-pack + Playwright federated |
| `security-grep` | every push | no | `check-no-unencrypted-sends.sh` |

`test-credentials.toml` supplied to CI via GitHub Actions secret; beta jobs skip with clear message on contributor forks.

### 5.11 Test ordering vs delivery sequence

| Step | Test gate |
|---|---|
| 1. `P2PCrypto` | `cross-vectors` green |
| 2. `TurnCredentials` | unit tests; manual `/voip/turnServer` smoke vs ca1-beta documented |
| 3. `WebRtcChannel` native | `rust-loopback` green |
| 4. `m.call.*` helpers | `signaling_parse` + `glare_resolution` green |
| 5. `P2PTransport` state machine | `loopback` green (full machine, not bypass) |
| 6. Worker + client wiring (flagged) | `e2e-beta-single-hs` green |
| 7. JS E2E full suite | `e2e-beta-federated` + `perf` + `security` green |
| 8. WASM + `mxdx-core-wasm` + web-console | `wasm-smoke` (federated) green |
| 9. Default-on flip | 3 consecutive nightly perf-green runs, both topologies |

---

## Council Feedback Summary

Three providers invoked (`gemini-3.1-pro`, `gpt-5.4`, `nemotron-ultra-253b`); two succeeded. Council rated design "excellent" with six hardening changes **all integrated** into this spec:

1. ✅ §1.3 / §3.6 — "semantic interoperability" instead of "bit-identical SDP"
2. ✅ §2.4 / §3.4 — explicit TURN active-call refresh policy (at TTL/2 via ICE restart) and expiry-during-reconnect serialization
3. ✅ §3.1 / §4.5 — Verifying transcript expanded to bind identity + room + call + direction + SDP fingerprints + domain separator (blocks replay and cross-call confusion)
4. ✅ §2.5 / §4.5 — `try_send` takes `Megolm<Bytes>` newtype, making plaintext structurally un-sendable
5. ✅ §5.6 — federated key-leak audit extended to to-device streams on both homeservers
6. ✅ §5.2 — timing-tolerant assertion helper for JS shim deprecation tests

No change invalidates the Hybrid architecture or the 9-step delivery sequence.

## Open Questions

- **Beads hygiene:** The currently-open P2P-in-npm beads (`mxdx-8y1`, `mxdx-vuy`, `mxdx-4yf`, `mxdx-eud`, `mxdx-xi6`) should be superseded with Rust-side equivalents. Handled during `/brains:architect`.
- **Perf baseline capture:** the npm baseline numbers for the comparison gate need to be captured once before Rust implementation begins so the ±10% comparison has a reference. Small task inside the architect phase.
- **Web-console deprecation timeline:** one release cycle for `packages/core/p2p-*.js` shims is proposed; the exact "release" is tied to npm publishing cadence. Confirm in implement phase.

## Next Phase

`/brains:architect` — produce:
- Crate-level `Cargo.toml` skeletons
- Module-level API surfaces
- ADRs (at minimum: `mxdx-p2p` crate creation rationale, choice of `datachannel-rs`, m.call.* vs custom schema decision, Sealed key pattern)
- Beads issue graph (epic + phase epics + task tasks with dependencies, covering the 9-step delivery)
- Supersede pass on the npm-era P2P beads
