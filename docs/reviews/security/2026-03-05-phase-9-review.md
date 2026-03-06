# Phase 9 Security Review

## Date: 2026-03-06
## Reviewer: PO

## Checklist

| Check | Status | Notes |
|:---|:---|:---|
| Double encryption: secret never appears as plaintext in Matrix event (mxdx-adr2) | **PASS** | `coordinator.rs:80` encrypts plaintext to the worker's ephemeral age x25519 public key before returning. The Matrix event only contains base64-encoded age ciphertext. Verified by E2E test `worker_requests_secret_with_double_encryption`. |
| Ephemeral key is one-time use -- not reused across requests | **PASS** | The `ephemeral_public_key` field on `SecretRequestEvent` is a per-request value. The coordinator does not cache or store it -- each call to `handle_secret_request` parses the key fresh from the request (`coordinator.rs:68`). Worker-side E2E test generates a new `Identity::generate()` per request. |
| `new_with_test_key()` is `#[cfg(test)]` gated (mxdx-tky) | **PASS** | `store.rs:30-31` has `#[cfg(test)]` on `new_with_test_key`. The E2E test in `e2e_secret_request.rs` correctly uses `SecretStore::new(Identity::generate())` instead. |
| Unauthorized requests denied | **PASS** | `coordinator.rs:39` checks `authorized_scopes.contains(&request.scope)` before any secret retrieval. Returns `granted: false` with error message. Verified by unit test `unauthorized_scope_denied` and E2E test `unauthorized_worker_cannot_get_secret`. |
| Audit trail: secret access logged to audit room | **MISSING** | No audit trail logging found in any of the reviewed files. The coordinator does not emit tracing events or send audit messages to a Matrix room on grant or denial. This must be added before Phase 9 sign-off. |

## Adversarial Findings

### Finding 1: No replay protection for SecretRequestEvent
- **Severity**: Medium
- **Location**: `coordinator.rs:38` -- `handle_secret_request`
- **Description**: The coordinator does not track previously seen `request_id` values. An attacker who can replay a captured `SecretRequestEvent` (same `request_id`, same `ephemeral_public_key`) will receive a valid response encrypted to the same ephemeral key. If the attacker also captured the ephemeral private key (e.g., from worker memory compromise), replaying the request yields the secret again without re-authorization. Even without the private key, the coordinator will re-encrypt the secret to the attacker-supplied key if they forge a new request reusing the same `request_id`.
- **Recommendation**: Track consumed `request_id` values in a set and reject duplicates. Consider binding the `request_id` to the sender's Matrix user ID so a different user cannot replay another user's request.

### Finding 2: No sender identity verification in coordinator
- **Severity**: High
- **Location**: `coordinator.rs:38` -- `handle_secret_request`
- **Description**: The `SecretCoordinator::handle_secret_request` takes a `&SecretRequestEvent` but does not verify *who* sent it. Authorization is scope-based only -- any entity that can send a `SecretRequestEvent` with an authorized scope to the room will receive the secret. The E2E test deserializes the event content but does not pass the Matrix sender (`@user:server`) to the coordinator for identity-based authorization.
- **Recommendation**: Extend the coordinator to accept the sender's Matrix user ID alongside the request. Validate that the sender is an authorized worker for the requested scope. The `authorized_scopes` model should become `HashMap<OwnedUserId, HashSet<String>>` or similar.

### Finding 3: Error messages leak scope existence
- **Severity**: Low
- **Location**: `coordinator.rs:44,52`
- **Description**: Denial responses distinguish between "unauthorized scope" and "secret not found", allowing an attacker to enumerate which scopes have secrets stored. An unauthorized user can probe scope names and learn which ones exist.
- **Recommendation**: Return a uniform denial message (e.g., "request denied") for both unauthorized scope and missing secret cases. Log the specific reason server-side only.

### Finding 4: ttl_seconds field is not enforced
- **Severity**: Low
- **Location**: `secret.rs:7` -- `SecretRequestEvent::ttl_seconds`
- **Description**: The `ttl_seconds` field exists on the request event but is never read or enforced by the coordinator. A worker can request a secret with `ttl_seconds: u64::MAX` and no expiry is applied. The secret, once decrypted, lives in worker memory indefinitely.
- **Recommendation**: Document whether TTL enforcement is deferred to a later phase. If it should be enforced, the coordinator or worker agent should implement secret expiry.

### Finding 5: age encryption uses recipient-based encryption correctly
- **Severity**: Informational (positive)
- **Location**: `store.rs:83-93`, `coordinator.rs:98-109`
- **Description**: Both `encrypt_value` and `encrypt_to_recipient` use `age::Encryptor::with_recipients()`, which performs proper x25519 key agreement -- not passphrase-based encryption. This is the correct approach for the double-encryption design. The `age` crate handles nonce generation internally, so each encryption produces unique ciphertext even for the same plaintext.

### Finding 6: No timing side-channel in authorization check
- **Severity**: Informational (positive)
- **Location**: `coordinator.rs:39`
- **Description**: The authorization check uses `HashSet::contains`, which is O(1) and does not perform variable-time string comparison. Since the check is a simple set membership test (not a secret comparison), there is no meaningful timing side-channel. The scope value is not a secret -- it is supplied by the requester.

## Summary

Phase 9 secret management has a **solid cryptographic foundation**. The double-encryption design (mxdx-adr2) is correctly implemented: secrets are encrypted at rest with the coordinator's age identity, then re-encrypted to the worker's ephemeral key before transmission over Matrix. The `#[cfg(test)]` gate on test key generation is properly enforced.

Two significant gaps remain:

1. **No sender identity verification** (High) -- the coordinator authorizes by scope alone, not by who is asking. Any room member can request any authorized scope.
2. **No replay protection** (Medium) -- request IDs are not tracked, allowing replayed requests.
3. **No audit trail** (Missing checklist item) -- secret access is not logged to an audit room.

These must be addressed before Phase 9 sign-off. The cryptographic primitives and age integration are sound.
