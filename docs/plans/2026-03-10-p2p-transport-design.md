# P2P Transport Layer Design

> **For Claude:** REQUIRED SUB-SKILL: Use superpowers:executing-plans to implement this plan task-by-task.

**Goal:** Bypass homeserver latency and rate limits for interactive terminal sessions via WebRTC P2P data channels, with transparent Matrix fallback.

**Architecture:** P2PTransport adapter sits between the existing TerminalSocket/BatchedSender and a platform-native WebRTC data channel. Signaling uses the standard Matrix VoIP call protocol (`m.call.*` events), giving us free TURN server provisioning and compatibility with existing Matrix call infrastructure. Terminal data is Megolm-encrypted before being placed on the data channel, preserving E2EE guarantees. Peer identity is verified via challenge-response after the data channel opens. Sessions start on Matrix immediately; P2P upgrades in parallel.

**Tech Stack:** Platform-native WebRTC (browser built-in `RTCPeerConnection`, `node-datachannel` for Node.js), standard Matrix call signaling (`m.call.invite/answer/candidates/hangup`), homeserver-provisioned TURN credentials, existing WASM E2EE layer.

---

## Architecture Overview

The P2P layer is a transparent transport upgrade for interactive terminal sessions. The existing Matrix path remains the default and the fallback.

```
                    ┌─────────────────────────────────┐
                    │       Matrix Homeserver          │
                    │  (signaling + TURN + fallback)   │
                    └──────────┬──────────┬────────────┘
                               │          │
                    signaling  │          │  signaling
                    + fallback │          │  + fallback
                    + TURN creds          + TURN creds
                               │          │
                    ┌──────────▼──┐  ┌────▼───────────┐
                    │   Client    │  │    Launcher     │
                    │             │  │                 │
                    │ TerminalSocket  TerminalSocket   │
                    │      │      │  │      │         │
                    │ BatchedSender  BatchedSender     │
                    │      │      │  │      │         │
                    │  P2PTransport  P2PTransport      │
                    │      │      │  │      │         │
                    │  ┌───▼──────┤  ├──────▼───┐     │
                    │  │ WebRTC   │◄─►│ WebRTC   │     │
                    │  │DataChannel  │DataChannel│     │
                    └──┴──────────┘  └──────────┴─────┘
```

**Key invariant:** TerminalSocket and BatchedSender are unchanged. They still produce/consume `org.mxdx.terminal.data` events with base64/zlib encoding and sequence numbers. The P2PTransport encrypts those events with the room's Megolm session keys before routing them over WebRTC, preserving E2EE guarantees identical to the Matrix path.

## Signaling Flow — Standard Matrix Call Protocol

WebRTC signaling uses the standard Matrix VoIP call events defined in the [Matrix Client-Server spec](https://spec.matrix.org/latest/client-server-api/#voice-over-ip). This gives us compatibility with existing Matrix TURN infrastructure and standard glare handling.

### Matrix call events used

| Event | Purpose | Key fields |
|:---|:---|:---|
| `m.call.invite` | SDP offer to initiate P2P channel | `call_id`, `party_id`, `version: "1"`, `lifetime`, `offer: { type, sdp }` |
| `m.call.answer` | SDP answer accepting P2P channel | `call_id`, `party_id`, `version: "1"`, `answer: { type, sdp }` |
| `m.call.candidates` | Batched ICE candidates | `call_id`, `party_id`, `version: "1"`, `candidates: [...]` |
| `m.call.hangup` | Terminate P2P channel | `call_id`, `party_id`, `version: "1"`, `reason` |
| `m.call.select_answer` | Caller selects which answer to use (glare) | `call_id`, `party_id`, `version: "1"`, `selected_party_id` |

### TURN server provisioning

TURN credentials come from the homeserver automatically:

```
GET /_matrix/client/v3/voip/turnServer
Authorization: Bearer <access_token>

Response:
{
  "username": "1443779631:@user:example.com",
  "password": "JlKfBy1QwLrO20385QyAtEyIv0=",
  "uris": [
    "turn:turn.example.com:3478?transport=udp",
    "turn:turn.example.com:3478?transport=tcp",
    "turns:turn.example.com:5349?transport=tcp"
  ],
  "ttl": 86400
}
```

Both peers fetch TURN credentials before creating their `RTCPeerConnection`. Credentials are refreshed when TTL expires. If the homeserver doesn't provide TURN (e.g., self-hosted without TURN configured), P2P falls back to direct/STUN-only connectivity.

### Call flow

The client initiates the call after a terminal session is established. Both sides may attempt to call each other — the standard Matrix glare resolution handles conflicts.

```
Client                          Matrix                         Launcher
  │                               │                               │
  │  (terminal session live)      │                               │
  │                               │                               │
  │  GET /voip/turnServer ───────►│                               │
  │◄─ { uris, username, ... } ───│                               │
  │                               │  GET /voip/turnServer ───────►│
  │                               │◄─ { uris, username, ... } ───│
  │                               │                               │
  ├─ m.call.invite ──────────────►│──────────────────────────────►│
  │  { call_id, party_id,        │                               │
  │    version: "1",              │                               │
  │    lifetime: 30000,           │                               │
  │    offer: { type, sdp } }    │                               │
  │                               │                               │
  │                               │◄── m.call.candidates ────────┤
  │◄──────────────────────────────│  { call_id, candidates: [...]}│
  │                               │                               │
  ├─ m.call.candidates ──────────►│──────────────────────────────►│
  │  { call_id, candidates: [...]}│                               │
  │                               │                               │
  │                               │◄── m.call.answer ────────────┤
  │◄──────────────────────────────│  { call_id, party_id,        │
  │                               │    answer: { type, sdp } }   │
  │                               │                               │
  │◄════════ DataChannel opens ═══════════════════════════════════►│
  │                               │                               │
  │  (terminal data flows over    │                               │
  │   data channel, Matrix idle)  │                               │
  │                               │                               │
  ├─ m.call.hangup ──────────────►│  (on session end or failure)  │
```

### Glare handling (bidirectional attempts)

Both sides may send `m.call.invite` simultaneously (e.g., both client and launcher attempt P2P). Per the Matrix spec:

1. Both peers detect glare (receiving an invite while their own is pending)
2. The invite from the peer with the **lexicographically smaller user ID** wins
3. The losing peer cancels their invite and answers the winning invite instead
4. The winning peer sends `m.call.select_answer` to confirm

This preserves the bidirectional attempt benefit (maximizing connectivity through UPnP, NAT traversal) while using standard conflict resolution.

### Call ID and party ID

- **`call_id`**: Generated per P2P connection attempt. Format: UUID. A new `call_id` is created each time a P2P channel is established (including reconnects after idle timeout). Not tied to the terminal session lifetime.
- **`party_id`**: Unique per participant per call. Format: UUID. Identifies which peer sent which events.
- **`version`**: Always `"1"` (string) for the modern call protocol with proper glare handling.
- **`lifetime`**: 30000ms (30 seconds) — how long the invite is valid. After expiry, both sides proceed on Matrix only.

### Call lifecycle is independent of terminal session

The P2P call is a transport optimization only. Hanging up the call (`m.call.hangup`) does NOT affect the terminal session — the session continues on Matrix fallback or remains idle.

**Idle timeout:** If no data flows over the P2P data channel for a configurable period (default 5 minutes), the P2P channel is proactively torn down (`m.call.hangup` with `reason: "idle_timeout"`). This frees WebRTC resources (TURN allocations, peer connection state) while the terminal session remains alive.

**Reconnect on activity:** When data appears again (user keystroke, PTY output), a fresh `m.call.invite` is sent to re-establish the P2P channel. The first message goes via Matrix immediately (no waiting for P2P), and the P2P channel upgrades the transport once connected.

```
Terminal Session ════════════════════════════════════════════════════►
                                                        (always alive)

P2P Call 1   ┌──────────────────┐
             │  m.call.invite   │
             │  ... data ...    │
             │  (5 min idle)    │
             │  m.call.hangup   │
             └──────────────────┘

             ... idle, Matrix fallback ...

P2P Call 2                        ┌──────────────────┐
                                  │  (data appears)  │
                                  │  m.call.invite   │
                                  │  ... data ...    │
                                  └──────────────────┘
```

Configuration:
- Launcher: `p2p_idle_timeout_s = 300` (TOML, default 5 minutes)
- Client: `p2p_idle_timeout_s = 300` (TOML, default 5 minutes)
- Browser: `localStorage` key `mxdx-p2p-idle-timeout-s` (default `300`)

### Data channel only — no media tracks

The SDP offer includes only a data channel, no audio or video tracks. The `RTCPeerConnection` is created with:

```javascript
const pc = new RTCPeerConnection({ iceServers: turnServers });
const dc = pc.createDataChannel('mxdx-terminal', {
  ordered: true,
  maxRetransmits: null,  // reliable
});
const offer = await pc.createOffer();
await pc.setLocalDescription(offer);
```

The resulting SDP will contain an `application` media section for the data channel but no `audio` or `video` sections.

## E2EE on the P2P Data Channel

**Critical design requirement:** WebRTC DTLS provides transport encryption (point-to-point), but this is NOT end-to-end encryption. A compromised TURN relay operator could theoretically intercept the DTLS session. To uphold the project's cardinal rule ("NEVER BYPASS END TO END ENCRYPTION"), terminal data MUST be Megolm-encrypted before placement on the data channel.

### Encryption approach

The P2PTransport calls the WASM client's encryption routines directly. The existing `sendEvent` path performs Megolm encryption internally — we extract that capability and apply it to P2P-bound payloads:

```
Sending (P2P path):
  plaintext content
    → WASM encrypt(roomId, type, content)  // Megolm encryption with room session keys
    → ciphertext JSON
    → dataChannel.send(ciphertext)

Receiving (P2P path):
  dataChannel.onmessage(ciphertext)
    → WASM decrypt(ciphertext)             // Megolm decryption
    → plaintext content
    → deliver to P2P inbox
```

The WASM client already exposes `encryptRoomEvent(roomId, eventType, content)` and `decryptEvent(event)` for Megolm operations. P2PTransport uses these directly — the same session keys, the same ratchet, the same trust chain.

**Fallback behavior:** If Megolm encryption fails (e.g., no outbound session keys for the room), the P2P transport falls back to Matrix (where `sendEvent` handles key sharing automatically). It does NOT send unencrypted data over the data channel.

### In-band encrypted frame format

```javascript
// Terminal data (encrypted)
{ type: "encrypted", ciphertext: "<megolm-encrypted-payload>", session_id: "...", sender_key: "..." }

// Control frames (NOT encrypted — no sensitive content)
{ type: "ack", seq: 5 }
{ type: "ping" }
{ type: "pong" }
{ type: "peer_verify", nonce: "...", signature: "..." }
```

Only terminal data (`org.mxdx.terminal.data`, `org.mxdx.terminal.resize`) is encrypted. Control frames (`ack`, `ping`, `pong`, `peer_verify`) are plaintext — they contain no terminal content.

## Peer Identity Verification

After the data channel opens, a challenge-response protocol verifies that the P2P peer is the same device that participated in the E2EE Matrix signaling. This provides defense-in-depth against TURN relay compromise or network-level MITM.

### Verification protocol

```
Peer A (initiator)                        Peer B (responder)
  │                                           │
  │  DataChannel opens                        │
  │                                           │
  ├─ { type: "peer_verify",                  │
  │    nonce: <32-byte random hex>,           │
  │    device_id: "ADEVICEID" } ────────────►│
  │                                           │
  │                                  B signs nonce with
  │                                  Ed25519 device key
  │                                           │
  │◄──── { type: "peer_verify",              │
  │        nonce: <same nonce>,               │
  │        device_id: "BDEVICEID",            │
  │        signature: <Ed25519 sig> } ────────┤
  │                                           │
  A verifies signature against                │
  B's device key from Matrix room membership  │
  │                                           │
  ├─ { type: "peer_verify",                  │
  │    nonce: <B's nonce or A's nonce>,       │
  │    signature: <Ed25519 sig> } ──────────►│
  │                                           │
  │                                  B verifies A's signature
  │                                           │
  │◄═══ Verified — begin encrypted data ═════►│
```

### Verification rules

- Both peers MUST complete verification before sending or accepting encrypted terminal data
- If verification fails (bad signature, unknown device, device not in room membership): close data channel immediately, log warning, fall back to Matrix
- Verification timeout: 10 seconds. If not completed, close channel and fall back to Matrix
- The device keys are already known from Matrix E2EE key exchange — no new key distribution needed

## P2PTransport Adapter

Sits between BatchedSender/TerminalSocket and the WebRTC data channel. Implements the same interface the existing code already uses. **Constructed via factory method, not post-construction mutation.**

### Interface contract

```javascript
// Sending (used by BatchedSender)
await transport.sendEvent(roomId, type, contentJson)

// Receiving (used by TerminalSocket polling loop)
const eventJson = await transport.onRoomEvent(roomId, type, timeoutSecs)
```

### Construction

P2PTransport is created via a static factory that receives all dependencies at construction time. There is no `_setTransport()` method — the transport is fully configured before it is handed to TerminalSocket.

```javascript
// Factory — returns a fully configured P2PTransport
const transport = P2PTransport.create({
  matrixClient,         // fallback transport (authenticated Matrix client)
  encryptFn,            // WASM Megolm encrypt: (roomId, type, content) → ciphertext
  decryptFn,            // WASM Megolm decrypt: (ciphertext) → plaintext
  verifySignatureFn,    // verify Ed25519 signature against device key
  signFn,               // sign nonce with own Ed25519 device key
  localDeviceId,        // this device's ID
  idleTimeoutMs,        // configurable idle timeout
  onStatusChange,       // UI callback
  onReconnectNeeded,    // reconnect trigger
  onHangup,             // signaling hangup callback
});

// TerminalSocket receives the transport at construction
const socket = new TerminalSocket(transport, roomId, ...);
```

TerminalSocket's constructor already accepts a client — P2PTransport is simply a different client implementation. No runtime swapping of `#client` is needed.

### Internal structure

```
┌─────────────────────────────────────────┐
│             P2PTransport                │
│                                         │
│  sendEvent(roomId, type, content)       │
│    ├─ if dataChannel.open && verified:  │
│    │    encrypt(content) → ciphertext   │
│    │    dataChannel.send(ciphertext)    │
│    │    add to pending ack buffer       │
│    └─ else:                             │
│         matrixClient.sendEvent(...)     │
│                                         │
│  onRoomEvent(roomId, type, timeout)     │
│    ├─ check p2pInbox queue first        │
│    ├─ if empty, wait up to timeout      │
│    └─ fallthrough to matrix polling     │
│                                         │
│  setDataChannel(channel)                │
│    ├─ start peer verification           │
│    ├─ on verified: enable P2P routing   │
│    └─ on failure: close channel         │
│                                         │
│  State:                                 │
│    #dataChannel    (WebRTC)             │
│    #matrixClient   (fallback)           │
│    #encryptFn      (Megolm encrypt)     │
│    #decryptFn      (Megolm decrypt)     │
│    #peerVerified   (boolean)            │
│    #p2pInbox       (Map<type, queue>)   │
│    #pendingAcks    (unacked sent events)│
│    #callId         (current call ID)    │
│    #status         'matrix'|'p2p'       │
│    #idleTimer      (idle timeout handle)│
│    #idleTimeoutMs  (configurable)       │
│    #reconnectBackoffMs (10s→5min exp)   │
│    #lastReconnectAt    (timestamp)      │
│    #onStatusChange (callback for UI)    │
└─────────────────────────────────────────┘
```

### Key behaviors

- **E2EE enforcement:** All terminal data is Megolm-encrypted before being placed on the data channel. If encryption fails, falls back to Matrix. NEVER sends unencrypted terminal data over the data channel.
- **Peer verification gate:** Data channel is not used for terminal data until peer identity verification completes. During verification, all data flows via Matrix.
- **Maximum frame size:** Incoming data channel messages are checked against a 64KB limit before `JSON.parse()`. Oversized frames are dropped and logged. This prevents memory exhaustion from malicious peers.
- **Transparent fallback:** If the data channel closes mid-session, `#status` flips to `'matrix'` and all events resume flowing through the homeserver. Sends `m.call.hangup` with `reason: "ice_failed"`. No disruption to terminal session.
- **Push-to-poll adapter:** Incoming WebRTC messages land in `#p2pInbox` keyed by event type. The `onRoomEvent` polling call resolves immediately from the inbox if data is waiting, or awaits a Promise that resolves on next push. P2PTransport is the sole consumer of both P2P inbox and Matrix events — TerminalSocket's polling loop calls `transport.onRoomEvent()` which internally multiplexes both sources. There is no separate polling of the raw Matrix client, eliminating event consumption races.
- **Status callback:** `onStatusChange('p2p' | 'matrix')` feeds the UI indicator.
- **Batch window switching:** On status change, BatchedSender's interval switches between `p2p_batch_ms` (default 10ms) and `batch_ms` (default 200ms).
- **Idle timeout:** Every `sendEvent` and incoming data channel message resets `#idleTimer`. When the timer fires (default 5 minutes of no data in either direction), the P2P channel is torn down (`m.call.hangup` with `reason: "idle_timeout"`), status flips to `'matrix'`. The terminal session continues unaffected.
- **Reconnect on activity with exponential backoff:** When `sendEvent` is called while `#status === 'matrix'` and P2P is enabled, a fresh `m.call.invite` is triggered in the background. The current event goes via Matrix immediately — no blocking on P2P reconnect. Both idle and failure reconnects use the same exponential backoff: **10s → 20s → 40s → 80s → 160s → 300s (5 min cap)**. Backoff resets to 10s after a successful P2P session (data channel opened and verified). This prevents signaling churn from periodic activity (e.g., cron jobs) and TURN allocation exhaustion from repeated failures.

## Delivery Guarantees

The P2P path uses application-level acknowledgements built on the existing `seq` numbers.

### Ack protocol (in-band on data channel)

```
Sender                              Receiver
  │                                     │
  ├─ { seq: 5, data... } ─────────────►│
  │                                     │
  │◄──────── { type: "ack", seq: 5 } ──┤
  │                                     │
```

- Sender keeps a **pending buffer** of events sent over P2P that haven't been acked
- Receiver sends an ack frame with the highest contiguous `seq` received
- Acks are batched — one ack per ~50ms covers all received events in that window
- If no ack arrives within 2 seconds, or the channel drops:
  1. All unacked events are requeued
  2. Coalesced with any new buffered data (same as existing 429 retry coalescing)
  3. Sent via Matrix fallback
  4. `m.call.hangup` sent with `reason: "ack_timeout"`

Receiver side already handles duplicates via `seq` — if the same data arrives over both P2P and Matrix, the duplicate is silently dropped.

## Fallback & Mid-Session Switching

### Transport states

```
                    ┌─────── idle timeout ──────┐
                    │                           │
connecting → p2p ──┼─── channel failure ───► matrix (fallback)
    │               │                           │
    └──► matrix ◄───┘                           │
         (initial) ◄── activity reconnect ──────┘
```

### Behavior

- **Startup:** Session starts on Matrix immediately. P2P call invite sent in parallel. Terminal is usable from the first moment.
- **Upgrade:** When the data channel opens, P2PTransport drains pending Matrix events, then switches to P2P. Matrix polling pauses but doesn't stop (checks periodically for in-flight events during switch).
- **Downgrade (failure):** Data channel closes or keepalive times out → `m.call.hangup` sent with `reason: "ice_failed"`, status flips to `'matrix'`, batch interval reverts to 200ms, Matrix polling resumes, unacked P2P events requeued via Matrix, UI updates.
- **Downgrade (idle):** No data in either direction for `p2p_idle_timeout_s` (default 300s) → `m.call.hangup` sent with `reason: "idle_timeout"`, WebRTC resources freed. Terminal session continues on Matrix or sits idle. UI shows `Matrix` (dim).
- **Reconnect on activity:** When data appears after idle hangup, the message goes via Matrix immediately and a fresh `m.call.invite` is sent in parallel. Exponential backoff (10s → 20s → ... → 5 min cap) prevents signaling churn from periodic activity.
- **Reconnect after failure:** Same exponential backoff as idle reconnects (10s → 5 min cap). Stops after 3 consecutive failures until next activity burst. Backoff resets to 10s after a successful P2P session.
- **Race conditions:** Events may arrive on both transports during switch. Existing sequence number dedup handles this.

## Peer Discovery

Peers advertise P2P capability through existing Matrix telemetry events.

### Extended telemetry

```javascript
// Existing org.mxdx.host_telemetry state event, new p2p field:
{
  hostname: "belthanior",
  platform: "linux",
  // ... existing fields ...
  p2p: {
    enabled: true,
  }
}
```

**Internal IP addresses** are NOT included in telemetry by default. State events persist indefinitely and are visible to all room members (including future joins), making them unsuitable for network topology data.

If LAN-first ICE candidate prioritization is needed, internal IPs can be exchanged during the signaling phase via ephemeral `m.call.candidates` events (which are E2EE and do not persist like state events). WebRTC's ICE already discovers LAN candidates automatically.

An optional `p2p_advertise_ips` configuration flag enables including internal IPs in telemetry for deployments using trusted, non-public homeservers:

```javascript
// Only when p2p_advertise_ips = true:
{
  p2p: {
    enabled: true,
    internal_ips: ["192.168.1.50", "10.0.0.5"],
  }
}
```

Client reads telemetry on dashboard load (already happens). TURN/STUN servers come from the homeserver's `/voip/turnServer` endpoint, not from telemetry.

## WebRTC Implementation

### Platform stack

| Environment | WebRTC Stack | Package |
|:---|:---|:---|
| Browser (web-console) | Built-in `RTCPeerConnection` | None |
| Node.js (launcher + CLI) | `node-datachannel` | Wraps libdatachannel (C++) |

### Data channel configuration

```javascript
{
  label: 'mxdx-terminal',
  ordered: true,
  maxRetransmits: null,   // reliable delivery
}
```

Reliable + ordered matches the existing seq-based model. Switching to unordered/unreliable is a future optimization.

### In-band frame format (over data channel)

```javascript
// Terminal data — Megolm-encrypted (NEVER sent as plaintext)
{ type: "encrypted", ciphertext: "<megolm-encrypted-payload>", session_id: "...", sender_key: "..." }

// Control frames — plaintext (no sensitive content)
{ type: "ack", seq: 5 }
{ type: "ping" }
{ type: "pong" }
{ type: "peer_verify", nonce: "...", device_id: "...", signature: "..." }
```

One JSON frame per WebRTC message. Maximum frame size: **64KB**. Data channels are message-oriented. Terminal data (`org.mxdx.terminal.data`, `org.mxdx.terminal.resize`) is always Megolm-encrypted before being placed in the `ciphertext` field. Control frames (ack/ping/pong/peer_verify) are plaintext — they contain no terminal content and are NOT sent as Matrix events.

### Keepalive

`ping` frame every 15 seconds over the data channel. If no `pong` within 5 seconds, mark channel dead, send `m.call.hangup`, and fall back to Matrix.

## Configuration

### Launcher config (TOML)

```toml
[p2p]
enabled = true
p2p_batch_ms = 10
idle_timeout_s = 300

# WARNING: Only enable on trusted, non-public homeservers.
# When true, internal LAN IP addresses are included in the
# org.mxdx.host_telemetry state event. State events persist
# indefinitely and are visible to all room members.
advertise_ips = false
```

ICE/TURN servers are NOT configured here — they come from the homeserver's `/voip/turnServer` endpoint.

### Client config (TOML)

```toml
[p2p]
enabled = true
p2p_batch_ms = 10
idle_timeout_s = 300
```

### Browser

`localStorage` keys: `mxdx-p2p-enabled` (default `true`), `mxdx-p2p-batch-ms` (default `10`), `mxdx-p2p-idle-timeout-s` (default `300`). Configurable from Settings page. All numeric values are validated and clamped to sane ranges on read: `batchMs` 1-1000, `idleTimeout` 30-3600.

## UI Indicators

Terminal toolbar `#terminal-status` element:

| Status | Display | Color |
|:---|:---|:---|
| Matrix only | `Matrix` | dim |
| P2P connecting | `P2P connecting...` | amber |
| P2P active | `P2P` | green |
| Fell back to Matrix | `Matrix (P2P lost)` | amber → dim after 5s |
| Rate limited | `Rate-limited by <homeserver>` | red |

## Capacity Considerations & Error Handling

### TURN allocation limits

Matrix homeservers (e.g., matrix.org) provision TURN credentials via `/voip/turnServer`. The TURN server (typically coturn) enforces per-user allocation quotas. matrix.org's recommended configuration is `user-quota=12`, meaning **12 concurrent TURN relay allocations per user**. Each active P2P terminal session consumes 1 allocation (one data channel = one SCTP-over-DTLS transport = one 5-tuple on the TURN server).

**Practical impact:** A user can have up to ~12 concurrent P2P terminal sessions. The idle timeout (default 5 minutes) helps — sleeping sessions release their TURN allocation, so only actively-used terminals consume a stream. Self-hosted homeservers can configure higher limits.

### Error detection

TURN allocation failures surface differently on each platform:

**Browser (`RTCPeerConnection`):**
- `icecandidateerror` event fires with specific STUN error codes:
  - **486** — Allocation Quota Reached (per-user limit hit)
  - **508** — Insufficient Capacity (server-wide limit hit)
  - **701** — Cannot reach TURN server (network/DNS/firewall)
- `iceconnectionstatechange` → `"failed"` when all candidate pairs exhausted

**Node.js (`node-datachannel` / libdatachannel):**
- No `icecandidateerror` equivalent — TURN error codes are not surfaced
- Only `onStateChange` → `"failed"` is available
- Must infer failure reason from context (no relay candidates gathered)

### Error handling strategy

```
icecandidateerror (browser only)
  ├─ errorCode 486 or 508:
  │    → UI: "P2P unavailable (TURN limit reached)"
  │    → Log warning with error code
  │    → Suppress reconnect attempts for 60s (server needs time to free allocations)
  │    → Terminal continues on Matrix
  │
  ├─ errorCode 701:
  │    → UI: "P2P unavailable (TURN unreachable)"
  │    → Log warning
  │    → Normal failure backoff (30s → exponential)
  │    → Terminal continues on Matrix
  │
  └─ other codes:
       → Log and treat as generic failure

iceConnectionState → "failed" (both platforms)
  ├─ if icecandidateerror 486/508 was seen:
  │    → Already handled above
  │
  └─ else (node-datachannel, or browser without specific error):
       → UI: "P2P unavailable"
       → m.call.hangup with reason: "ice_failed"
       → Normal failure backoff
       → Terminal continues on Matrix
```

### UI feedback for TURN errors

| Condition | Display | Color | Duration |
|:---|:---|:---|:---|
| TURN quota reached (486/508) | `P2P unavailable (TURN limit)` | amber | Until allocation freed |
| TURN unreachable (701) | `P2P unavailable` | amber → dim after 10s | Until next retry |
| ICE failed (generic) | `Matrix (P2P lost)` | amber → dim after 5s | Until next retry |
| No TURN configured on homeserver | `P2P (direct only)` | dim | Persistent |

When the homeserver's `/voip/turnServer` returns empty or 404 (no TURN configured), P2P still attempts direct/STUN-only connectivity but the UI indicates reduced capability.

## What Doesn't Change

- TerminalSocket interface (both implementations — receives transport at construction)
- Event schema (`org.mxdx.terminal.*`)
- Sequence numbering and gap detection
- PtyBridge and tmux persistence
- Matrix E2EE (identity, key exchange, encryption)
- Room topology (exec + DM rooms)

## New Components

| Component | Location | Purpose |
|:---|:---|:---|
| `P2PTransport` | `packages/core/p2p-transport.js` | Adapter between TerminalSocket and WebRTC/Matrix |
| `P2PSignaling` | `packages/core/p2p-signaling.js` | `m.call.*` event exchange via Matrix |
| `WebRTCChannel` (browser) | `packages/web-console/src/webrtc-channel.js` | Thin wrapper around native RTCPeerConnection |
| `WebRTCChannel` (node) | `packages/core/webrtc-channel-node.js` | Thin wrapper around node-datachannel |
| Config extensions | `packages/launcher/src/config.js`, `packages/client/src/config.js` | P2P config fields |
| Telemetry extension | `packages/launcher/src/runtime.js` | P2P capability in telemetry |
| UI indicators | `packages/web-console/src/terminal-view.js` | Status display updates |
