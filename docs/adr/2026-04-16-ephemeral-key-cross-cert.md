# ADR 2026-04-16: Ephemeral Ed25519 handshake key scoped to Megolm-protected room state

**Status:** Superseded (by Phase 7 device-sign retrofit — see ADR `2026-04-16-matrix-sdk-testing-feature.md`)
**Date:** 2026-04-16
**Related:** `docs/plans/2026-04-15-rust-p2p-interactive-storm.md` §3.1, bead `mxdx-btk`, follow-up `mxdx-btk2`

## Context

Phase 5 T-53 shipped the storm-spec Ed25519-signed Verifying handshake (`crates/mxdx-p2p/src/transport/verify.rs`) with `HandshakeSigner` + `HandshakePeerKeySource` traits and test-stub `EphemeralKeySigner` + `InMemoryPeerKeySource`. Production impls (`MatrixHandshakeSigner` + `MatrixPeerKeySource`) are Phase 6's job (bead `mxdx-btk`).

The original design (storm §3.1) specified signing the transcript with the Matrix device's long-term Ed25519 key. Code-level investigation of matrix-sdk 0.16 during Phase 6 grooming showed:

- `matrix_sdk::Encryption::ed25519_key()` returns the device's public Ed25519 key as a `String`, but no corresponding public `sign()` method exists.
- `matrix_sdk::Encryption::olm_machine_for_testing()` is feature-gated to the `testing` feature; no production accessor.
- `matrix_sdk_crypto::OlmMachine::sign(&str)` exists but is unreachable from outside `matrix-sdk` without touching private APIs.
- No public `Device::sign()` or `Account::sign()` escape hatch.

Result: **we cannot sign with the device's long-term Ed25519 key from mxdx production code.** Cross-signing an ephemeral key with the device key (option (b) as described in `mxdx-btk`) also requires the same unreachable sign primitive.

## Decision

**Use an ephemeral Ed25519 keypair per Matrix session**, not cross-signed by the device key. Trust the ephemeral public key because:

1. **The peer's ephemeral public key is published in a Megolm-encrypted state event** (`m.mxdx.p2p.ephemeral_key`) in the session room. Only joined devices can decrypt Megolm, so only joined devices can learn the ephemeral public key.

2. **The session room is E2EE with MSC4362** (encrypted state events). An attacker outside the room cannot inject, modify, or replace the state event.

3. **The publishing device is cross-signed.** Before accepting a peer's ephemeral public key, the verifier calls `Device::is_cross_signed_by_owner()` on the publishing device. A device the attacker injected into the user's account (without cross-signing) is rejected.

4. **The ephemeral keypair's lifetime is scoped to the Matrix session.** When the session ends (idle timeout, hangup, session room teardown), the keypair is dropped. A compromised session's ephemeral key cannot be used to authenticate future sessions.

## Rationale

- **Preserves the Verifying handshake's threat model against network MITM.** The ephemeral key proves possession of Megolm-room membership, which is already the authoritative trust root for this project (see cardinal rule + MSC4362).
- **Does not weaken the cardinal rule.** The state event is Megolm-encrypted; the ephemeral key is protected in transit and at rest.
- **Does not weaken cross-signing trust.** Adversaries still cannot inject a rogue device without tripping cross-signing verification, and `Device::is_cross_signed_by_owner()` gates ephemeral key acceptance.
- **Unblocks Phase 6 without reaching into matrix-sdk internals.** Depending on private APIs would create a maintenance burden on every matrix-sdk bump.

## Security trade-off

Compared to the original storm §3.1 design, we lose **one** property:

- **Original:** Ephemeral key is *cryptographically bound* to the device's long-term identity via a chain of Ed25519 signatures all the way up to the Matrix device key. An attacker who compromises the Megolm session but not the device key cannot forge a new ephemeral key.
- **This ADR:** Ephemeral key is bound to the Megolm session (via the state event) and the device cross-signing chain (via `is_cross_signed_by_owner`). An attacker who compromises the Megolm session key (one of {device, backup, key-forward-attacker}) AND has the compromised device cross-sign their injected device can forge a new ephemeral key for the peer.

The attack surface in practice: anyone with the Megolm session key of an mxdx session room is already trusted by the cardinal rule to read the entire session's plaintext. Granting such an attacker the ability to forge an ephemeral P2P handshake signature does not materially worsen the position. The attacker can already read/write the terminal stream.

**Net conclusion:** in mxdx's trust model, Megolm-session-access is the load-bearing trust root. Binding ephemeral keys to Megolm access is coherent with that root.

## Consequences

- `crates/mxdx-matrix` gains `publish_ephemeral_key(room_id, device_id, ephemeral_pk_bytes)` (sends Megolm-encrypted state event of type `m.mxdx.p2p.ephemeral_key` with state_key `"{device_id}"` and content `{ ephemeral_ed25519_b64, published_at }`).
- `crates/mxdx-matrix` gains `get_ephemeral_key(room_id, user_id, device_id)` that (a) reads the state event, (b) checks the publishing device is cross-signed, (c) returns the ephemeral public key on success.
- `crates/mxdx-p2p` gains `MatrixHandshakeSigner` (wraps an ephemeral Ed25519 keypair + holds a reference to the `MatrixClient` so it can publish the public half) and `MatrixPeerKeySource` (wraps a `MatrixClient` reference to look up peers' ephemeral keys).
- Both runtimes (Rust and npm) must agree on the state event shape. This triggers the **second coordinated Rust/npm release** under ADR `2026-04-16-coordinated-rust-npm-releases.md` — Phase 6 mxdx-fqt npm upgrade AND this ephemeral-key event.
- A follow-up bead `mxdx-btk2` is opened to revisit cross-signing once matrix-sdk exposes a device sign API or the mxdx fork patches one in.

## Event shape (`m.mxdx.p2p.ephemeral_key`)

```jsonc
{
  "type": "m.mxdx.p2p.ephemeral_key",
  "state_key": "<device_id>",           // e.g. "WORKER1"
  "content": {
    "ephemeral_ed25519_b64": "<32-byte base64url, no padding>",
    "published_at": 1742870400000       // unix-ms, advisory freshness hint only
  }
}
```

State event, Megolm-encrypted as per MSC4362 (already enabled project-wide). `state_key` is the publisher's device_id, so multiple devices on the same account can each publish their own ephemeral key without conflict.

Freshness: `published_at` is advisory. A peer who sees a stale ephemeral key can prompt a refresh by triggering a new `m.call.invite` — the publisher responds by rotating the ephemeral key and republishing.

## Alternatives considered and rejected

- **Wait for matrix-sdk 0.17+ or patch matrix-sdk.** Rejected for Phase 6. Blocks the Rust P2P rollout on external release timing. Filed as `mxdx-btk2` to revisit.
- **Use the device's curve25519 identity key (Olm).** Rejected: curve25519 is the identity key for Olm session establishment, not a signing key — would misuse the primitive.
- **Skip the Verifying handshake entirely; trust Megolm alone.** Rejected. The Verifying handshake adds defense-in-depth against TURN MITM (the peer's DTLS fingerprint is bound to the Matrix-encrypted exchange). Dropping it weakens the design below the storm-spec floor.
- **Use a pre-shared long-lived ephemeral key per user.** Rejected: session-scoped keys are standard hygiene. A per-session keypair costs ~64 bytes of state room content; trivial.

## Implementation notes

- The state event is published once per session on transport `Start` (before the first Verifying handshake). If the publish fails, the transport falls back to Matrix-only mode for that session (no P2P benefit, no security regression).
- Republish on Matrix reconnect: if the state event is missing on first lookup, the peer's `MatrixPeerKeySource` returns `PeerKeyUnknown`, and the Verifying handshake falls back.
- Because state events are at the room level but the ephemeral key is per-device, the `state_key` MUST be the device_id (not the user_id). This permits federated multi-device setups where the same user has multiple devices in the same session room.
