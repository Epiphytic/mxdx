# Comprehensive E2EE Security Audit

**Date:** 2026-03-12
**Scope:** Full codebase audit for any paths that bypass end-to-end encryption
**Verdict:** **PASS** — No unencrypted terminal data paths found

---

## Audit Methodology

Four parallel security audit agents examined the codebase, each covering a distinct attack surface:

1. **P2P Transport Layer** — WebRTC data channels, AES-256-GCM encryption, peer verification
2. **Matrix Transport Layer** — All sendEvent/sendStateEvent calls, WASM client encryption
3. **Credentials & Secrets** — Storage, logging, CLI args, keychain integration
4. **Web Console** — Browser-side fetch/XHR/WebSocket, IndexedDB, DOM exposure

Manual verification was then performed on flagged areas.

---

## 1. P2P Transport Layer — PASS

**Files reviewed:** `packages/core/p2p-transport.js`, `packages/core/webrtc-channel-node.js`

### Encryption

- All terminal data (`org.mxdx.terminal.data`, `org.mxdx.terminal.resize`) is encrypted with **AES-256-GCM** before being placed on the WebRTC data channel
- Encryption uses Web Crypto API (`crypto.subtle`) with a shared key derived during peer verification
- Each message gets a unique 12-byte IV (crypto.getRandomValues)
- Ciphertext format: `[12-byte IV][ciphertext+tag]` — standard authenticated encryption

### Gating Logic

`sendEvent()` in `p2p-transport.js` only routes over P2P when ALL conditions are met:
- `this.#status === 'p2p'`
- `this.#peerVerified === true`
- `this.#dataChannel` exists and is open
- `this.#p2pCrypto` (AES key) exists
- Event type is in `ENCRYPTED_EVENT_TYPES` whitelist

If **any** condition fails, the event falls back to Matrix transport (Megolm encrypted).

### Event Type Whitelist

Only two event types are routed over P2P:
- `org.mxdx.terminal.data`
- `org.mxdx.terminal.resize`

All other events (signaling, telemetry, exec) always go through Matrix.

### Peer Verification

- Handshake sends `device_id` over the DTLS-encrypted data channel
- Device IDs are public Matrix metadata — no confidentiality concern
- Verification confirms the remote peer matches the expected Matrix device

### TURN Relay Security

- TURN credentials fetched via `/_matrix/client/v3/voip/turnServer`
- HTTPS enforced for non-loopback servers (`turn-credentials.js`)
- `redirect: 'error'` prevents credential exfiltration via redirects
- Credentials are ephemeral (not cached to disk)
- Even with a compromised TURN relay, terminal data remains AES-256-GCM encrypted — the relay only sees ciphertext

### Previous Finding Resolution

The pre-implementation security review (2026-03-10) identified **C1: P2P data channel bypasses E2EE** as a critical blocking issue. This was resolved by adding the AES-256-GCM encryption layer, which encrypts all terminal data before it reaches the data channel.

---

## 2. Matrix Transport Layer — PASS

**Files reviewed:** `packages/launcher/src/runtime.js`, `packages/core/index.js`, `crates/mxdx-core-wasm/src/lib.rs`

### All sendEvent Paths

12 `sendEvent` call sites were identified in `packages/launcher/src/runtime.js`. Every call goes through `this.#client.sendEvent()` or `this.#transport.sendEvent()`, both of which route through the WASM Matrix client's Megolm encryption.

No raw HTTP POST/PUT calls are used for sending room events.

### sendStateEvent Paths

State events (telemetry, room configuration) use `sendStateEvent()` through the WASM client, which applies MSC4362 encrypted state events.

### WASM Client Encryption

- `crates/mxdx-core-wasm/src/lib.rs` uses `matrix-sdk` with `experimental-encrypted-state-events`
- `send_event()` calls `room.send_raw()` which encrypts via Megolm before transmission
- The WASM boundary does not expose any unencrypted send path

### Registration Path

- Account registration uses raw HTTP POST to `/_matrix/client/v3/register`
- This is correct — registration happens before authentication and contains no sensitive room data
- Immediately followed by `login_username()` which establishes the encrypted session

---

## 3. Credentials & Secrets — PASS

**Files reviewed:** `packages/core/credentials.js`, `packages/launcher/bin/mxdx-launcher.js`, `packages/client/bin/mxdx-client.js`

### Storage

- OS keychain (keytar) is the primary credential store
- Fallback: AES-256-GCM encrypted file with key derived from machine-specific entropy
- No plaintext passwords stored on disk

### Logging

- Structured logging (pino) configured with `redact` paths for sensitive fields
- Password values are never included in log output
- Access tokens are not logged

### Informational Notes

| Observation | Risk | Mitigation |
|---|---|---|
| `ClientConfig` constructor accepts password parameter | Low | `save()` method excludes password from serialized config |
| CLI `--password` flag visible via `ps`/`procfs` | Low | Documented as "first run only" — password stored in keychain after first use |

---

## 4. Web Console — PASS

**Files reviewed:** `packages/web-console/src/terminal-socket.js`, `packages/web-console/src/main.js`, `packages/web-console/index.html`

### No Bypass Paths

- No raw `fetch()`, `XMLHttpRequest`, or `WebSocket` calls that send terminal data
- All terminal I/O flows through the WASM Matrix client or P2P transport (AES-256-GCM)
- xterm.js `onData` handler sends keystrokes through `sendEvent()` — encrypted path only

### IndexedDB

- WASM crypto store uses IndexedDB for session persistence
- IndexedDB is origin-scoped (same-origin policy)
- No cross-origin data exposure

### DOM

- Terminal output rendered to xterm.js canvas — not accessible via DOM inspection
- No sensitive data written to `localStorage` or `sessionStorage`

---

## 5. Cleanup Operations — PASS

**Files reviewed:** `packages/core/cleanup.js`

- Uses raw `fetch()` to Matrix REST API for device/room/event management
- Operations: device listing/deletion, room leave/forget, event redaction
- Accesses only **metadata** (event_id, type, origin_server_ts) — never decrypted content
- Uses Bearer token authentication with 429 retry logic
- No encryption bypass — cleanup operates on the server-side management API, not on message content

---

## 6. BatchedSender — PASS

**Files reviewed:** `packages/core/batched-sender.js`

- Batches terminal output events before sending via `sendEvent()`
- Does not perform its own encryption — delegates to the transport layer
- Adaptive intervals: 5ms (P2P) / 200ms (Matrix)
- Rate limit retry (429) with exponential backoff
- No data leaves the batch buffer except through the encrypted `sendEvent()` path

---

## Summary

| Component | Encryption | Verdict |
|---|---|---|
| P2P data channel | AES-256-GCM (Web Crypto API) | PASS |
| Matrix room events | Megolm (WASM matrix-sdk) | PASS |
| Matrix state events | MSC4362 encrypted state | PASS |
| TURN relay path | AES-256-GCM (opaque to relay) | PASS |
| Credential storage | OS keychain / AES-256-GCM file | PASS |
| Web console I/O | Delegated to WASM/P2P transport | PASS |
| Cleanup operations | Metadata only, no content access | PASS |
| Batched sender | Delegates to encrypted transport | PASS |

**No paths were found that bypass end-to-end encryption for terminal data, command execution, or telemetry.**

The previous critical finding (C1 from 2026-03-10 review) — P2P data sent in cleartext — has been fully resolved with the AES-256-GCM encryption layer.
