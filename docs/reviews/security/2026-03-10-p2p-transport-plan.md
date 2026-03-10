# Security Review: P2P Transport Layer Implementation Plan

**Reviewed:** `docs/plans/2026-03-10-p2p-transport-plan.md` and `docs/plans/2026-03-10-p2p-transport-design.md`
**Date:** 2026-03-10
**Reviewer:** Security Review Agent

---

## Overall Assessment: **CONDITIONAL PASS** — 3 Critical, 5 High, 4 Medium findings

The design correctly preserves the existing E2EE layer (terminal data is encrypted by the WASM Matrix client before reaching P2PTransport), reuses standard Matrix call protocol instead of inventing custom signaling, and maintains transparent Matrix fallback. However, there are several security gaps that must be addressed before implementation.

---

## CRITICAL Findings

### C1: P2P data channel bypasses E2EE — terminal data sent in cleartext over WebRTC

**Location:** Design doc §P2PTransport Adapter, Plan Tasks 8/10/11

The design states the P2PTransport routes `org.mxdx.terminal.data` events over the WebRTC data channel. However, the `sendEvent` interface on the Matrix client performs Megolm encryption before transmission. When P2PTransport intercepts the call and sends the JSON payload directly over the data channel, **the data is NOT encrypted by Megolm** — it's sent as plaintext JSON over DTLS.

WebRTC's DTLS provides transport encryption (TLS 1.2+), but this is **not end-to-end encryption** — it's point-to-point between the two WebRTC endpoints. If a TURN relay is in the path, the TURN server's operator could theoretically intercept the DTLS session (via a compromised TURN server or implementation flaw). This violates the project's cardinal rule: **"NEVER BYPASS END TO END ENCRYPTION IN ANY CHANGES UNDER ANY CIRCUMSTANCES"**.

**Recommendation:** Terminal data MUST be encrypted with the same Megolm session keys before being placed on the data channel. Options:
1. Call the WASM client's encryption routines directly (encrypt content → send ciphertext over data channel → decrypt on receive)
2. Use the existing `sendEvent` path to encrypt, intercept the encrypted payload before it hits the Matrix HTTP transport, and route the ciphertext over the data channel instead
3. Add a separate encryption layer using the room's Megolm keys

This is a **blocking** issue — the plan cannot proceed without addressing it.

### C2: `_setTransport` and `_sender` expose internal state mutation

**Location:** Plan Task 10 — `packages/core/terminal-socket.js`

```javascript
_setTransport(transport) {
  this.#client = transport;  // Replaces the authenticated Matrix client
}
get _sender() {
  return this.#sender;       // Exposes the BatchedSender instance
}
```

The underscore-prefix convention signals "private" but these are public methods on a security-critical class. `_setTransport()` allows **any caller** to replace the encrypted Matrix client with an arbitrary object that could:
- Log all terminal data in cleartext
- Route data to an attacker-controlled endpoint
- Drop the encryption layer entirely

**Recommendation:**
- Use a constructor injection pattern or factory method instead of post-construction mutation
- If runtime transport switching is required, add validation that the replacement implements the same encryption guarantees
- Consider a `Symbol`-keyed method to prevent casual access

### C3: No authentication of P2P peer identity

**Location:** Plan Tasks 7/8, Design doc §Signaling Flow

The signaling flow exchanges SDP via Matrix events (which are E2EE and sender-verified). However, once the WebRTC data channel opens, there is no verification that the peer on the data channel is the same entity that sent the Matrix signaling events. A MITM on the network path could:

1. Intercept the SDP offer/answer (impossible if Matrix E2EE is working, but defense-in-depth matters)
2. More realistically: if TURN relay is compromised, inject a different peer connection

**Recommendation:** After the data channel opens, perform a challenge-response authentication using the Matrix identity:
- Peer A sends a random nonce over the data channel
- Peer B signs it with their Matrix device key and returns the signature
- Peer A verifies the signature against the device key from the Matrix room membership

This ensures the data channel peer is the same device that participated in the E2EE signaling.

---

## HIGH Findings

### H1: Internal IP addresses leaked in telemetry

**Location:** Plan Task 9

```javascript
p2p: {
  enabled: this.#config.p2pEnabled,
  internal_ips: this.#getInternalIps(),  // LAN topology exposure
  external_ip: null,
}
```

Internal IP addresses are posted as a Matrix state event in the exec room. Even though the room is E2EE, state events persist indefinitely and are visible to all room members (including future joins). This leaks internal network topology to anyone who gains room access.

**Recommendation:**
- Remove `internal_ips` from telemetry entirely — ICE handles LAN candidate discovery automatically via WebRTC
- If LAN prioritization is needed, exchange IPs only during the signaling phase (ephemeral `m.call.*` events, not state events)
- At minimum, respect the `telemetry` level config (current plan publishes IPs even at non-"full" levels)

### H2: No size limit on incoming P2P data channel messages

**Location:** Plan Task 8 — P2PTransport, Design doc §In-band frame format

The plan describes receiving JSON frames from the data channel and parsing them. There's no maximum message size check. A malicious peer could send multi-megabyte frames causing:
- Memory exhaustion via JSON.parse on very large strings
- Buffer allocation issues

The existing Matrix path has natural rate limits and message size limits enforced by the homeserver. The P2P path has none.

**Recommendation:** Add a maximum frame size check (e.g., 64KB) before `JSON.parse()`. Drop and log oversized frames.

### H3: `fetchTurnCredentials` — access token in URL construction lacks validation

**Location:** Plan Task 4

```javascript
const url = homeserverUrl.replace(/\/$/, '') + '/_matrix/client/v3/voip/turnServer';
```

If `homeserverUrl` is user-controlled (from config) and contains path traversal or SSRF payloads, this could be exploited. More critically, the Bearer token is sent to whatever URL is constructed.

**Recommendation:**
- Validate `homeserverUrl` is a proper URL with `https:` scheme before use
- Use `new URL()` for path construction instead of string concatenation
- Ensure `fetchFn` doesn't follow redirects to non-homeserver domains (credential exfiltration risk)

### H4: Reconnect-on-activity has no rate limiting for signaling

**Location:** Plan Task 8, Design doc §Reconnect on activity

When activity appears after idle hangup, a fresh `m.call.invite` is sent immediately. If a session alternates between brief activity and idle (e.g., a cron job printing a line every 5 minutes), this creates a rapid cycle of:
- `m.call.invite` → connection setup → idle timeout → `m.call.hangup` → repeat

This generates unbounded signaling traffic and TURN allocation churn. An adversary controlling the PTY output could deliberately trigger this to exhaust TURN allocations or create signaling noise.

**Recommendation:** Add a minimum cooldown between reconnect attempts after idle hangup (e.g., 30 seconds). The design mentions "no backoff for idle reconnects" — this should be reconsidered.

### H5: `onRoomEvent` polling from P2PTransport creates event consumption race

**Location:** Plan Task 8/10

The existing `TerminalSocket` polls `client.onRoomEvent()` for `org.mxdx.terminal.data`. When P2PTransport replaces `#client` via `_setTransport()`, the transport's `onRoomEvent` must serve both P2P inbox events AND fall through to Matrix polling. But the existing TerminalSocket also has its own polling loop.

If both TerminalSocket's polling AND some other consumer poll the same Matrix transport, events may be consumed by the wrong consumer, causing data loss. This isn't a direct security vulnerability but can cause terminal session corruption and data integrity issues.

**Recommendation:** Ensure the P2PTransport's `onRoomEvent` implementation properly multiplexes between P2P inbox and Matrix fallback without event loss. Add sequence number validation to detect gaps.

---

## MEDIUM Findings

### M1: localStorage for P2P settings lacks integrity protection

**Location:** Plan Task 13

P2P settings (enabled, batch_ms, idle_timeout) stored in `localStorage` can be modified by any JavaScript on the same origin. A compromised or malicious browser extension could:
- Disable P2P (`mxdx-p2p-enabled = false`) forcing all traffic through Matrix (DoS on TURN)
- Set `mxdx-p2p-idle-timeout-s = 0` to prevent P2P from ever establishing
- Set `mxdx-p2p-batch-ms = 0` to create excessive data channel sends

**Recommendation:** Accept this risk for now (same-origin limitation applies), but document that localStorage values should be validated with sane ranges (e.g., `batchMs` clamped to 1-1000, `idleTimeout` clamped to 30-3600).

### M2: `stun:stun.l.google.com:19302` hardcoded in tests

**Location:** Plan Tasks 5, 14

Tests depend on Google's public STUN server. This creates:
- External network dependency in tests (flaky CI)
- DNS leak to Google during test execution

**Recommendation:** Use a local STUN server in tests, or mock the ICE layer for unit tests. Only use real STUN in E2E tests that explicitly require network connectivity.

### M3: `m.call.hangup` with custom reasons may confuse Matrix clients

**Location:** Design doc, Plan Task 7

Standard `m.call.hangup` reasons are: `ice_failed`, `invite_timeout`, `user_hangup`, `user_media_failed`, `user_busy`, `unknown_error`. The design adds custom reasons: `idle_timeout`, `ack_timeout`.

If another Matrix client is in the room (e.g., Element), it may misinterpret or choke on non-standard reasons.

**Recommendation:** Use `user_hangup` as the standard reason, and add the custom reason in a separate `org.mxdx.hangup_reason` field in the content. Or document this as an accepted deviation.

### M4: Keepalive ping/pong timing exposes session activity patterns

**Location:** Design doc §Keepalive

Ping frames every 15 seconds over the data channel are visible to TURN relay operators (even with DTLS, the timing and packet sizes are observable). This creates a side channel that reveals:
- Whether a session is active (pings present) vs idle (no pings)
- When activity occurs (ping interval changes or stops)

**Recommendation:** Accept this risk (inherent to any keepalive) but document it in the threat model. Consider randomizing ping intervals (15 ± 5 seconds).

---

## Positive Security Observations

1. **Standard protocol reuse**: Using `m.call.*` instead of custom signaling eliminates an entire class of protocol-design bugs
2. **TURN from homeserver**: No user-configured ICE servers means no credential leakage risk from misconfigured TURN URIs
3. **Transparent fallback**: Matrix path always works — P2P failures don't disrupt sessions
4. **Sequence-based dedup**: Existing `seq` numbering handles transport-switch races correctly
5. **Zlib bomb protection**: Already present in existing code, carried through to P2P path
6. **DOM safety**: Task 13 uses `createElement`/`textContent` instead of `innerHTML`

---

## Summary of Required Actions Before Implementation

| Priority | ID | Action |
|:---|:---|:---|
| **BLOCKING** | C1 | Ensure terminal data is E2E encrypted on the data channel, not just DTLS |
| **BLOCKING** | C2 | Redesign transport injection to prevent arbitrary client replacement |
| **BLOCKING** | C3 | Add peer identity verification after data channel opens |
| High | H1 | Remove internal IPs from state-event telemetry |
| High | H2 | Add max frame size on data channel receive |
| High | H3 | Validate homeserver URL before TURN credential fetch |
| High | H4 | Add cooldown on idle-reconnect signaling |
| High | H5 | Verify event consumption doesn't race between TerminalSocket and P2PTransport |
| Medium | M1-M4 | Address in implementation or document as accepted risk |

The plan's structure and test coverage are solid. The critical gap is that the P2P data channel as designed **silently downgrades from E2EE to transport-only encryption**, which directly violates the project's security mandate.
