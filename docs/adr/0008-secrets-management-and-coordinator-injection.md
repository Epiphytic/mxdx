# ADR 0008: Secrets Management and Coordinator Injection

**Date:** 2026-03-22
**Status:** Accepted

## Context

`mxdx-secrets` already has a well-designed double-encryption protocol:
- `SecretStore`: age x25519 encrypted key/value store — each value encrypted with the coordinator's public key
- `SecretCoordinator`: on request, decrypts the value from the store, re-encrypts to the worker's one-time ephemeral public key
- Result: the plaintext is only ever readable by the worker's ephemeral identity

The existing protocol is correct. The gap is **where the coordinator's age `Identity` (private key) lives**. Currently it's in process memory, loaded at startup. This means: if the coordinator process is compromised, the attacker has the key and can decrypt every stored secret.

The goal: the coordinator should be able to broker secrets it cannot directly read.

## Decision

### Architecture: mxdx-secrets as a Cloud Run Service

`mxdx-secrets` becomes a standalone HTTP service (Cloud Run) rather than an embedded crate. The coordinator calls it via HTTP. The coordinator has no key material — it only has an API token to call the service.

```
Client → posts task with secret references
Coordinator → calls mxdx-secrets HTTP API to resolve secrets
mxdx-secrets → decrypts (via KMS), re-encrypts to worker ephemeral key, returns ciphertext
Coordinator → DMs ciphertext to worker
Worker → decrypts with its ephemeral private key
```

The coordinator is a pass-through. It never sees plaintext.

### KMS-Backed Keystore

`mxdx-secrets` uses **envelope encryption**:

1. Each secret value is encrypted with a **data encryption key (DEK)** — a per-secret AES-256 key
2. The DEK is encrypted with a **key encryption key (KEK)** stored in Google Cloud KMS (or AWS KMS)
3. The encrypted DEK is stored alongside the ciphertext
4. On each request, `mxdx-secrets` calls KMS to decrypt the DEK, uses it to decrypt the value, then discards the DEK from memory

The `mxdx-secrets` service process never holds the KEK. It cannot decrypt stored values without a live KMS call. If the service process is compromised between requests, the attacker gets no key material.

This replaces the existing age x25519 `Identity`-in-memory model for the store. The double-encryption layer (re-encrypting to the worker's ephemeral key) is preserved — it's applied after decryption from the KMS-backed store.

### Secret References in Task Payloads

Tasks reference secrets by name in the payload using a `secrets` field:

```json
{
  "prompt": "deploy the thing",
  "secrets": ["github.token", "aws.deploy_key"]
}
```

The coordinator sees `secrets` in the payload, resolves each via `mxdx-secrets`, and DMs the values to the worker before the task starts. Workers declare which secrets they need during capability advertisement (optional — runtime requests are also supported).

Secret names use dot-notation scopes: `{service}.{key}` (e.g. `github.token`, `npm.publish_token`).

### Three Delivery Paths

**Path 1: Client DMs worker directly**

The client holds the secret and sends it via Matrix DM to the worker identity. No coordinator involvement. Used for ephemeral or per-session credentials the client manages directly.

**Path 2: Coordinator pre-injection (preferred for managed secrets)**

1. Client posts task with `"secrets": ["github.token"]`
2. Coordinator sees the secrets field; calls `mxdx-secrets` API: `POST /resolve` with scope + worker's ephemeral public key
3. `mxdx-secrets` decrypts (KMS round-trip) → re-encrypts to worker ephemeral key → returns ciphertext
4. Coordinator DMs the ciphertext to the worker's private room before posting the task to the worker room
5. Worker reads the DM, decrypts with its ephemeral private key, secrets are available before execution starts

**Path 3: Worker runtime request**

1. Worker encounters a secret reference during execution
2. Worker generates a fresh ephemeral age key pair
3. Worker posts `org.mxdx.secrets.request` to the coordinator room with `{ scope, ephemeral_public_key, task_uuid, request_id }`
4. Coordinator calls `mxdx-secrets` → DMs ciphertext back to worker
5. Worker decrypts with ephemeral private key

This is the existing `SecretRequestEvent` / `SecretResponseEvent` protocol — the only change is the coordinator now calls the HTTP service instead of directly querying an in-process store.

### Temporary / Task-Scoped Credentials

`mxdx-secrets` can issue **temporary credentials** scoped to a task UUID with a TTL equal to `timeout_secs`:

- On `POST /resolve?task_uuid=X&ttl=1800`, the service issues a scoped token valid only for that task's duration
- These are not stored secrets — they're generated at request time (e.g. a temporary GitHub App installation token, a short-lived AWS STS token)
- After TTL expiry, the credential is automatically revoked (via the upstream provider's API or a local revocation list in `mxdx-secrets`)

### Authorization Model

`mxdx-secrets` authorizes requests per scope per caller identity:

- Coordinator has a service account identity (API token or mTLS certificate)
- Each secret has an ACL: `{ scope: "github.token", allowed_callers: ["coordinator@project.iam"] }`
- Workers do not call `mxdx-secrets` directly — only the coordinator does, on their behalf
- The coordinator's ACL is broad; it brokers for any authorized task. Per-task scoping is enforced by requiring the `task_uuid` on all requests (for audit logging)

### mxdx-secrets HTTP API

```
POST /secrets/resolve
{
  "scope": "github.token",
  "ephemeral_public_key": "<age x25519 pubkey>",
  "task_uuid": "<uuid>",
  "ttl_seconds": 1800
}
→ { "encrypted_value": "<base64 age ciphertext>" }

POST /secrets/store (admin only)
{ "scope": "github.token", "value": "ghp_..." }

DELETE /secrets/{scope} (admin only)
```

### What Changes in the Existing Crate

The existing `mxdx-secrets` crate (`SecretStore`, `SecretCoordinator`) becomes the **core library** for the Cloud Run service:

- `SecretStore`: replace age x25519 in-memory identity with KMS envelope encryption. The `Identity` field is removed; `get()` calls KMS to decrypt the DEK on each access.
- `SecretCoordinator`: unchanged in structure — still does double-encryption to the worker's ephemeral key. Gets its store from the KMS-backed implementation.
- Add `src/server.rs`: Axum HTTP server wrapping `SecretCoordinator`
- Add `src/kms.rs`: KMS client abstraction (GCP KMS initially; trait for testability)
- `src/main.rs`: wire up server + KMS + store

The double-encryption tests in `coordinator.rs` remain valid and valuable — mock the KMS layer for unit tests.

### Coordinator Changes

The fabric `CoordinatorBot` gains:
- A `SecretsBroker` struct with an HTTP client to `mxdx-secrets`
- On task receive: if `payload.secrets` is non-empty, resolve each before routing the task
- On `org.mxdx.secrets.request` event: forward to `SecretsBroker`, DM response to worker

The coordinator does not hold the `SecretCoordinator` in process — it only holds the HTTP client and API token.

## Consequences

**Positive:**
- Coordinator cannot decrypt stored secrets — compromise of the coordinator process yields no key material
- KMS access is audited by the cloud provider — every secret resolution is logged
- Temporary task-scoped credentials are revoked automatically — no long-lived credentials in worker environments
- The existing double-encryption protocol is preserved — even if the Matrix transport is compromised, plaintext is only accessible to the worker's ephemeral key
- `mxdx-secrets` can scale and deploy independently of the coordinator

**Negative:**
- Each secret resolution requires a KMS round-trip (GCP KMS: ~10-50ms) — adds latency to task startup
- Cloud Run + KMS adds operational complexity and cost
- KMS is a new trust dependency — GCP/AWS key management policies must be correctly configured
- If KMS is unavailable, secret injection fails and tasks cannot start (acceptable: fail closed is correct for secrets)

## Out of Scope

- Secret rotation (updating stored values) — admin operation, out of scope for v1
- Per-worker ACLs (currently coordinator ACL is broad) — future enhancement
- Secrets for non-fabric Matrix commands — fabric tasks only for now

## Related

- mxdx-secrets crate: `crates/mxdx-secrets/`
- ADR-0006: OpenClaw Fabric Callback Plugin (task payload schema)
- ADR-0005: Worker Capability Advertisement
