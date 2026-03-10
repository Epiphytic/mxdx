# P2P Transport Layer Design

> **For Claude:** REQUIRED SUB-SKILL: Use superpowers:executing-plans to implement this plan task-by-task.

**Goal:** Bypass homeserver latency and rate limits for interactive terminal sessions via WebRTC P2P data channels, with transparent Matrix fallback.

**Architecture:** P2PTransport adapter sits between the existing TerminalSocket/BatchedSender and a platform-native WebRTC data channel. Matrix handles identity, E2EE, signaling, and peer discovery. The P2P channel carries the same encrypted Matrix events over a faster pipe. Sessions start on Matrix immediately; P2P upgrades in parallel.

**Tech Stack:** Platform-native WebRTC (browser built-in `RTCPeerConnection`, `node-datachannel` for Node.js), Matrix signaling events, existing WASM E2EE layer.

---

## Architecture Overview

The P2P layer is a transparent transport upgrade for interactive terminal sessions. The existing Matrix path remains the default and the fallback.

```
                    ┌─────────────────────────────────┐
                    │       Matrix Homeserver          │
                    │  (signaling + fallback data)     │
                    └──────────┬──────────┬────────────┘
                               │          │
                    signaling  │          │  signaling
                    + fallback │          │  + fallback
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

**Key invariant:** TerminalSocket and BatchedSender are unchanged. They still produce/consume `org.mxdx.terminal.data` events with base64/zlib encoding and sequence numbers. The P2PTransport routes those events over WebRTC instead of Matrix.

## Signaling Flow

WebRTC SDP offer/answer exchange and ICE candidate sharing use existing Matrix exec room events.

### New Matrix event types

```
org.mxdx.p2p.offer    { request_id, role, sdp, ice_servers }
org.mxdx.p2p.answer   { request_id, role, sdp }
org.mxdx.p2p.ice      { request_id, role, candidate }
```

### Bidirectional handshake

Both sides create offers simultaneously after the session starts. Whichever data channel opens first wins. The other is discarded.

```
Client                          Matrix                         Launcher
  │                               │                               │
  │  (session established)        │                               │
  │                               │                               │
  ├─ org.mxdx.p2p.offer ────────►│──────────────────────────────►│
  │  { request_id, role:'client'} │                               │
  │                               │◄── org.mxdx.p2p.offer ───────┤
  │◄──────────────────────────────│  { request_id, role:'launcher'}│
  │                               │                               │
  │  (both sides create answers   │                               │
  │   for the other's offer)      │                               │
  │                               │                               │
  ├─ org.mxdx.p2p.answer ───────►│──────────────────────────────►│
  │                               │◄── org.mxdx.p2p.answer ──────┤
  │◄──────────────────────────────│                               │
  │                               │                               │
  │◄════ first DataChannel open wins ════════════════════════════►│
  │      (other connection torn down)                             │
```

**Conflict resolution:** Each offer includes a `role` field. If both data channels open simultaneously, the connection initiated by the `client` role wins (deterministic tiebreaker). The launcher closes its initiated connection.

**Timeout:** If P2P handshake doesn't complete within 5 seconds, both sides proceed on Matrix only. The terminal session is already live.

**Bidirectional rationale:** Many clients can take advantage of UPnP to receive traffic. Both sides attempting to connect maximizes the chance of a direct connection.

## P2PTransport Adapter

Sits between BatchedSender/TerminalSocket and the WebRTC data channel. Implements the same interface the existing code already uses.

### Interface contract

```javascript
// Sending (used by BatchedSender)
await transport.sendEvent(roomId, type, contentJson)

// Receiving (used by TerminalSocket polling loop)
const eventJson = await transport.onRoomEvent(roomId, type, timeoutSecs)
```

### Internal structure

```
┌─────────────────────────────────────────┐
│             P2PTransport                │
│                                         │
│  sendEvent(roomId, type, content)       │
│    ├─ if dataChannel.open:              │
│    │    dataChannel.send(content)       │
│    │    add to pending ack buffer       │
│    └─ else:                             │
│         matrixClient.sendEvent(...)     │
│                                         │
│  onRoomEvent(roomId, type, timeout)     │
│    ├─ check p2pInbox queue first        │
│    ├─ if empty, wait up to timeout      │
│    └─ fallthrough to matrix polling     │
│                                         │
│  State:                                 │
│    #dataChannel    (WebRTC)             │
│    #matrixClient   (fallback)           │
│    #p2pInbox       (Map<type, queue>)   │
│    #pendingAcks    (unacked sent events)│
│    #status         'matrix'|'p2p'       │
│    #onStatusChange (callback for UI)    │
└─────────────────────────────────────────┘
```

### Key behaviors

- **Transparent fallback:** If the data channel closes mid-session, `#status` flips to `'matrix'` and all events resume flowing through the homeserver. No disruption.
- **Push-to-poll adapter:** Incoming WebRTC messages land in `#p2pInbox` keyed by event type. The `onRoomEvent` polling call resolves immediately from the inbox if data is waiting, or awaits a Promise that resolves on next push.
- **Status callback:** `onStatusChange('p2p' | 'matrix')` feeds the UI indicator.
- **Batch window switching:** On status change, BatchedSender's interval switches between `p2p_batch_ms` (default 10ms) and `batch_ms` (default 200ms).

## Delivery Guarantees

The P2P path uses application-level acknowledgements built on the existing `seq` numbers.

### Ack protocol

```
Sender                              Receiver
  │                                     │
  ├─ { seq: 5, data... } ─────────────►│
  │                                     │
  │◄──────── { type: "ack", seq: 5 } ──┤
  │                                     │
```

- Sender keeps a **pending buffer** of events sent over P2P that haven't been acked
- Receiver sends `org.mxdx.p2p.ack` with the highest contiguous `seq` received
- Acks are batched — one ack per ~50ms covers all received events in that window
- If no ack arrives within 2 seconds, or the channel drops:
  1. All unacked events are requeued
  2. Coalesced with any new buffered data (same as existing 429 retry coalescing)
  3. Sent via Matrix fallback

Receiver side already handles duplicates via `seq` — if the same data arrives over both P2P and Matrix, the duplicate is silently dropped.

## Fallback & Mid-Session Switching

### Transport states

```
connecting → p2p → matrix (fallback)
    │                  │
    └──► matrix ◄──────┘
         (initial)
```

### Behavior

- **Startup:** Session starts on Matrix immediately. P2P handshake runs in parallel. Terminal is usable from the first moment.
- **Upgrade:** When the data channel opens, P2PTransport drains pending Matrix events, then switches to P2P. Matrix polling pauses but doesn't stop (checks periodically for in-flight events during switch).
- **Downgrade:** Data channel closes or keepalive times out → status flips to `'matrix'`, batch interval reverts to 200ms, Matrix polling resumes, unacked P2P events requeued via Matrix, UI updates.
- **Race conditions:** Events may arrive on both transports during switch. Existing sequence number dedup handles this.
- **Reconnect after downgrade:** Fresh P2P handshake attempted after 30 seconds. Exponential backoff up to 5 minutes. Stops after 3 failures until next session.

## Peer Discovery

Peers advertise reachability through existing Matrix telemetry events.

### Extended telemetry

```javascript
// Existing org.mxdx.telemetry event, new p2p field:
{
  hostname: "belthanior",
  platform: "linux",
  // ... existing fields ...
  p2p: {
    enabled: true,
    ice_servers: [
      { urls: "stun:stun.l.google.com:19302" }
    ],
    internal_ips: ["192.168.1.50", "10.0.0.5"],
    external_ip: null,
    port: 9443,
  }
}
```

Client reads telemetry on dashboard load (already happens). Uses `internal_ips` for LAN-first ICE candidates, `ice_servers` for STUN/TURN configuration.

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

### Frame format

```javascript
{ type: "org.mxdx.terminal.data", content: { data, encoding, seq } }
{ type: "org.mxdx.terminal.resize", content: { cols, rows } }
{ type: "org.mxdx.p2p.ack", seq: 5 }
{ type: "org.mxdx.p2p.ping" }
{ type: "org.mxdx.p2p.pong" }
```

One JSON frame per WebRTC message. Data channels are message-oriented.

### Keepalive

`org.mxdx.p2p.ping` every 15 seconds. If no pong within 5 seconds, mark channel dead and fall back to Matrix.

## Configuration

### Launcher config (TOML)

```toml
[p2p]
enabled = true
p2p_batch_ms = 10
ice_servers = [
  "stun:stun.l.google.com:19302"
]
```

### Client config (TOML)

```toml
[p2p]
enabled = true
p2p_batch_ms = 10
```

### Browser

`localStorage` keys: `mxdx-p2p-enabled` (default `true`), `mxdx-p2p-batch-ms` (default `10`). Configurable from Settings page.

## UI Indicators

Terminal toolbar `#terminal-status` element:

| Status | Display | Color |
|:---|:---|:---|
| Matrix only | `Matrix` | dim |
| P2P connecting | `P2P connecting...` | amber |
| P2P active | `P2P` | green |
| Fell back to Matrix | `Matrix (P2P lost)` | amber → dim after 5s |
| Rate limited | `Rate-limited by <homeserver>` | red |

## What Doesn't Change

- TerminalSocket (both implementations)
- Event schema (`org.mxdx.terminal.*`)
- Sequence numbering and gap detection
- PtyBridge and tmux persistence
- Matrix E2EE (identity, key exchange, encryption)
- Room topology (exec + DM rooms)

## New Components

| Component | Location | Purpose |
|:---|:---|:---|
| `P2PTransport` | `packages/core/p2p-transport.js` | Adapter between TerminalSocket and WebRTC/Matrix |
| `P2PSignaling` | `packages/core/p2p-signaling.js` | SDP/ICE exchange via Matrix events |
| `WebRTCChannel` (browser) | `packages/web-console/src/webrtc-channel.js` | Thin wrapper around native RTCPeerConnection |
| `WebRTCChannel` (node) | `packages/core/webrtc-channel-node.js` | Thin wrapper around node-datachannel |
| Config extensions | `packages/launcher/src/config.js`, `packages/client/src/config.js` | P2P config fields |
| Telemetry extension | `packages/launcher/src/runtime.js` | P2P discovery block in telemetry |
| UI indicators | `packages/web-console/src/terminal-view.js` | Status display updates |
