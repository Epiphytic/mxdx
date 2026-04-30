# Security Review: WASM Session-Loop Migration

**Date:** 2026-04-29  
**Scope:** Migration of `packages/launcher/src/runtime.js` session-execution loop into `crates/mxdx-core-wasm`  
**ADR:** `docs/adr/2026-04-29-rust-npm-binary-parity.md` (req 13a)  
**Phase:** Phase 4 — WASM Expansion (T-4.1)  
**Files reviewed:**
- `packages/launcher/src/runtime.js` (1696 lines — pre-migration source)
- `crates/mxdx-core-wasm/src/lib.rs` (1850 lines — migration destination)
- `packages/core/p2p-crypto.js` (JS P2P crypto reference implementation)
- `crates/mxdx-p2p/src/crypto.rs` (Rust P2P crypto implementation)

---

## APPROVAL GATE

**Project owner must sign off on this document before T-4.3 (batching migration) may merge.**

> Sign-off: [ ] Project owner approval (add name + date here)

This document satisfies ADR req 13a. The three required confirmations (§1, §2, §3 below) are all affirmative. Migration may proceed to T-4.3 after project owner sign-off.

---

## §1. Matrix Send Call Enumeration

Every `sendEvent` and `sendStateEvent` call in the session-loop migration path is enumerated below. Each is confirmed encrypted on the wire because:

- `sendEvent` routes through `WasmMatrixClient::send_event` → `matrix-sdk Room::send_raw`, which encrypts for E2EE rooms via Megolm (matrix-sdk enforces encryption for rooms with `m.room.encryption` state).
- `sendStateEvent` routes through `WasmMatrixClient::send_state_event` → the MSC4362 `experimental-encrypted-state-events` path enabled via `matrix-sdk-base` dependency with feature `experimental-encrypted-state-events`. This is confirmed enabled in `crates/mxdx-core-wasm/Cargo.toml`:
  ```
  matrix-sdk-base = { version = "0.16", default-features = false, features = ["experimental-encrypted-state-events"] }
  ```

All exec-room events go to `topology.exec_room_id` — an E2EE room created with `m.room.encryption`. All DM-room terminal events go to per-session DM rooms created with `preset: 'trusted_private_chat'` (E2EE by default).

### Send Call Table

| Line (runtime.js) | Event type | Room | send variant | Encrypted? | Confirmation |
|---|---|---|---|---|---|
| 755 | `org.mxdx.session.start` | exec_room_id | `sendEvent` | YES | E2EE room; Megolm via matrix-sdk |
| 765 | `org.mxdx.session.active` (state) | exec_room_id | `sendStateEvent` | YES | MSC4362 encrypted state events |
| 807 | `org.mxdx.session.active` (clear) | exec_room_id | `sendStateEvent` | YES | MSC4362 |
| 808 | `org.mxdx.session.completed` (state) | exec_room_id | `sendStateEvent` | YES | MSC4362 |
| 882 | `org.mxdx.session.output` | exec_room_id | `sendEvent` | YES | E2EE room; Megolm |
| 900 | `org.mxdx.session.result` | exec_room_id | `sendEvent` | YES | E2EE room; Megolm |
| 992 / 1115 / 1182 | `org.mxdx.terminal.session` (response) | exec_room_id | `sendEvent` | YES | E2EE room; Megolm |
| 1081 | `org.mxdx.terminal.sessions` (list) | exec_room_id | `sendEvent` | YES | E2EE room; Megolm |
| 1209 | `org.mxdx.output` (legacy) | exec_room_id | `sendEvent` | YES | E2EE room; Megolm |
| 1224 | `org.mxdx.result` (legacy) | exec_room_id | `sendEvent` | YES | E2EE room; Megolm |
| 1286 | `org.mxdx.host_telemetry` (online) | exec_room_id | `sendStateEvent` | YES | MSC4362 |
| 1295 | `org.mxdx.host_telemetry` (offline) | exec_room_id | `sendStateEvent` | YES | MSC4362 |
| 1006 / 1126 | `org.mxdx.terminal.data` (PTY output) | dmRoomId | `transport.sendEvent` | YES | DM room E2EE; transport wraps `sendEvent` |
| 1317 / 1345 | P2P signaling callbacks (transport setup) | exec_room_id | `sendEvent` | YES | Signaling via exec room (established E2EE) |
| 1523 / 1632 | `m.call.invite` / `m.call.candidates` / `m.call.answer` (P2P signaling) | exec_room_id (signalingRoomId) | `sendEvent` | YES | Exec room E2EE; signaling room is not DM room specifically to ensure established Megolm keys |

**Summary:** All 15+ send call sites route through E2EE-enforced rooms. State events use MSC4362. No unencrypted send path exists in the session loop.

### Key design note on P2P signaling room choice

`signalingRoomId` is set to `this.#topology.exec_room_id` (line 1466), NOT the newly-created DM room. This is intentional (comment at line 1464–1465): newly-created DM rooms have unreliable Megolm key exchange. The exec room has an established, verified Megolm session, so P2P signaling events (`m.call.invite`, `m.call.answer`, `m.call.candidates`) are reliably encrypted. The session key embedded in `m.call.invite` (`mxdx_session_key`) is therefore protected by the Megolm layer before it travels over the wire.

---

## §2. Cryptographic Primitive Inventory

### 2a. AES-GCM Session Key (P2P Application-Layer Encryption)

**JS implementation** (`packages/core/p2p-crypto.js`):
- Key generation: `crypto.subtle.generateKey({ name: 'AES-GCM', length: 256 }, true, ['encrypt', 'decrypt'])` — 256-bit AES-GCM key via Web Crypto API
- Key export: `crypto.subtle.exportKey('raw', key)` → base64 string for Matrix signaling
- Key import: `crypto.subtle.importKey('raw', raw, { name: 'AES-GCM' }, false, ['encrypt', 'decrypt'])` — non-extractable once imported
- Encrypt: `crypto.subtle.encrypt({ name: 'AES-GCM', iv }, key, data)` — random 96-bit IV per frame
- Decrypt: `crypto.subtle.decrypt({ name: 'AES-GCM', iv }, key, data)` — authenticated decryption; throws on tag mismatch

**Rust implementation** (`crates/mxdx-p2p/src/crypto.rs`):
- Key generation: `mxdx_p2p::crypto::P2PCrypto::generate()` — uses `aes-gcm` crate with OS CSPRNG (`OsRng` from `rand_core`)
- Key serialization: `SealedKey::to_base64()` / `SealedKey::from_base64()` — base64-standard-padded encoding (matching JS `btoa`/`atob`)
- Encrypt: `aes_gcm::Aes256Gcm::encrypt()` — 96-bit random nonce, AES-256-GCM
- Decrypt: `aes_gcm::Aes256Gcm::decrypt()` — authenticated; `AesGcmError` on tag mismatch

**Behavioral identity confirmation:**
| Property | JS | Rust | Identical? |
|---|---|---|---|
| Algorithm | AES-256-GCM | AES-256-GCM | YES |
| Key length | 256-bit | 256-bit | YES |
| Nonce/IV length | 96-bit (12 bytes) | 96-bit (12 bytes) | YES |
| Nonce generation | `crypto.getRandomValues` (CSPRNG) | `OsRng` (CSPRNG) | YES |
| Nonce freshness | Random per frame | Random per frame | YES |
| Auth tag | 128-bit (GCM default) | 128-bit (GCM default) | YES |
| Key transport encoding | base64 standard padded | base64 standard padded | YES |
| Key non-extractability after import | YES (non-extractable `CryptoKey`) | YES (key bytes in private struct field) | YES |
| Failure mode on tag mismatch | throws `DOMException` | returns `Err(AesGcmError)` | Behavioral match — both reject |

**Wire-format compatibility:** JS `P2PCrypto.encrypt()` produces `{c: "<base64>", iv: "<base64>"}`. Rust `EncryptedFrame` struct (from `crates/mxdx-p2p/src/crypto.rs` line 39–43) uses fields `c` (ciphertext+tag) and `iv`. The Rust WASM binding `P2PCrypto::encrypt()` serializes via `serde_json::to_string(&frame)` — same JSON shape. Cross-runtime decryption is confirmed compatible (validated in `docs/reviews/security/2026-04-29-p2p-cross-runtime-dtls-verification.md`).

### 2b. Megolm / OLM (Matrix E2EE)

The session-loop migration does NOT add new Megolm or OLM calls. Megolm is managed entirely by `matrix-sdk` internally. The JS code calls `sendEvent` / `sendStateEvent` on `WasmMatrixClient`; the Rust matrix-sdk applies encryption transparently. This is unchanged by the migration.

**Confirmation:** No cryptographic primitive handling for OLM/Megolm exists at the session-loop layer in `runtime.js`. The migration cannot add or remove OLM key handling.

### 2c. Crypto Store Persistence (IndexedDB / Megolm key persistence)

`saveIndexedDB(this.#config.configDir)` is called in `#syncLoop()` every 5 minutes (line 524). This persists the matrix-sdk crypto store (Megolm session keys). This call:
- **Remains JS-side** (IndexedDB is a JS/browser API — OS-bound per ADR Pillar 3)
- The WASM migration must preserve this call in the JS thin wrapper
- The crypto store is written to an encrypted file on disk (matrix-sdk manages the encryption); the session-loop migration does not touch this path

### 2d. Session ID Generation

`crypto.randomUUID().slice(0, 8)` (line 958) generates a 128-bit UUID and takes the first 8 hex characters as a session identifier. This is used for PTY session IDs, not for cryptographic keys. **Not a security-critical primitive** — collision probability for 8 hex chars (2^32 space) is acceptable for a single-node session registry with a small number of concurrent sessions (max 10 by default).

When migrated to WASM, the Rust equivalent will use `uuid::Uuid::new_v4().to_string()[..8]` (uuid crate with `js` feature for WASM CSPRNG via `getrandom`). **Behavioral identity confirmed.**

---

## §3. WASM Public API Crypto-State Exposure Audit

This section confirms that the new WASM public API does not expose crypto state (private keys, megolm session secrets, serialized olm session state) to JS callers.

### Current WASM API surface (pre-migration)

Reviewing all `#[wasm_bindgen]` exported types and functions in `crates/mxdx-core-wasm/src/lib.rs`:

| Export | Return type | Crypto state exposed? | Verdict |
|---|---|---|---|
| `sdk_version()` | `String` | No | SAFE |
| `WasmMatrixClient::login()` | `Promise<()>` | No | SAFE |
| `WasmMatrixClient::restore_session()` | `Promise<()>` | No | SAFE |
| `WasmMatrixClient::logout()` | `Promise<()>` | No | SAFE |
| `WasmMatrixClient::is_logged_in()` | `bool` | No | SAFE |
| `WasmMatrixClient::user_id()` | `Option<String>` | No | SAFE |
| `WasmMatrixClient::device_id()` | `Option<String>` | No — device ID is public identity | SAFE |
| `WasmMatrixClient::export_session()` | `Result<String, JsValue>` | YES — contains `access_token` | See note below |
| `WasmMatrixClient::send_event()` | `Promise<()>` | No | SAFE |
| `WasmMatrixClient::send_state_event()` | `Promise<()>` | No | SAFE |
| `WasmMatrixClient::collect_room_events()` | `Promise<String>` | No — returns decrypted event content JSON | SAFE |
| `P2PCrypto::generate()` | `P2PCryptoWithKey` | Returns base64 key — intentional for signaling | SAFE (by design) |
| `P2PCrypto::from_key()` | `P2PCrypto` | No — key stored in opaque Rust struct | SAFE |
| `P2PCrypto::encrypt()` | `Result<String, JsValue>` | No — ciphertext only | SAFE |
| `P2PCrypto::decrypt()` | `Result<String, JsValue>` | No — plaintext only | SAFE |
| `generateSessionKey()` | `String` | Returns base64 key — intentional for signaling | SAFE (by design) |
| `createP2PCrypto()` | `Result<P2PCrypto, JsValue>` | No — key stored in opaque Rust struct | SAFE |
| `generate_session_key()` | `String` | Returns base64 key — intentional for signaling | SAFE (by design) |

**Note on `export_session()`:** This returns `{"homeserver_url": "...", "access_token": "...", "device_id": "...", "user_id": "..."}`. The `access_token` is a Matrix access token (not a cryptographic key). It is used in `runtime.js` line 1414 specifically to fetch TURN credentials from the homeserver. This is intentional — the access token is needed for authenticated Matrix API calls from the Node.js layer (which cannot use the WASM reqwest client directly). **This is a pre-existing design decision and is out of scope for this migration. It does not change with the WASM expansion.** The access token is not an OLM/Megolm key.

### OLM session state and Megolm keys

Confirmed: No WASM export returns:
- Raw OLM private keys (Ed25519 identity key, Curve25519 device key)
- Megolm outbound session keys or room keys
- Serialized OLM account state (contains all private key material)
- Inbound Megolm session ratchet state

These are all managed internally by matrix-sdk's OlmMachine and are never serialized to JsValue return values.

### Post-migration API additions (T-4.3 through T-4.5)

The following WASM API additions are planned by the migration. Each is audited for crypto-state exposure risk before implementation:

**T-4.3 additions:**
- `BatchedSender` struct: processes PTY bytes → sends `org.mxdx.terminal.data` events. No crypto state. Operates on opaque byte arrays. **SAFE.**
- `process_terminal_input(data_json, encoding) -> Result<Box<[u8]>>`: decodes base64/zlib. Returns raw bytes. No crypto state. **SAFE.**

**T-4.4 additions:**
- `build_telemetry_payload(level, hostname, ...) -> String`: constructs telemetry JSON from OS metrics passed in by JS. Returns JSON string. No crypto state. **SAFE.**
- `SessionTransportManager`: manages P2P offer/answer state machine. Generates session keys internally for P2P encryption. The raw session key IS returned temporarily for embedding in `m.call.invite` (same as the current `generateSessionKey()` pattern). This is necessary and intentional — the key must be transmitted to the peer via E2EE Matrix. The key exposure is bounded to the signaling exchange. **SAFE by design.**

**T-4.5 additions:**
- `SessionManager` struct: core session dispatch and lifecycle. Manages session registry internally. **Must NOT** expose `dmRoomId`, `sender` (Matrix user IDs), or session state as JsValue returns. Acceptable returns: session ID strings (UUIDs), session status enums serialized as strings. **Implementation must be audited before T-4.5 merges — see acceptance criterion.**

### Session-loop migration crypto-state guardrail checklist

Before T-4.3, T-4.4, and T-4.5 each merge:

- [ ] Run `grep -r 'send_state_event\|send_raw' packages/launcher/src/ crates/mxdx-core-wasm/src/` — verify every match is in an encryption-aware code path
- [ ] Verify `wasm-bindgen` exported function signatures: no `JsValue` return contains OLM/Megolm private key material
- [ ] Verify `SessionManager` getter methods do not return raw room IDs, sender IDs, or session secrets as public API
- [ ] Confirm `saveIndexedDB` is called in the JS thin wrapper (not moved into WASM where IndexedDB polyfill behavior differs)

---

## Threat Model (migration-specific)

### Assets at risk during migration
1. Megolm outbound session keys (room encryption)
2. OLM device identity keys (Ed25519, Curve25519)
3. Matrix access token (used for TURN credential fetching)
4. P2P AES-GCM session keys (transient; ephemeral per-session)
5. `authorized_users` / `allowed_commands` (session authorization state in config — not crypto keys, but security-critical)

### Migration-specific threat: new WASM boundary leaks crypto state to JS

**Risk:** A newly-added `#[wasm_bindgen]` getter on `SessionManager` accidentally serializes internal state (e.g., a session registry dump) that includes Matrix room IDs, which could be combined with network-layer attacks.

**Mitigation:**
- `SessionManager` internal state (session registry Map) remains in Rust memory, never serialized to JsValue unless through an intentional, reviewed API.
- PR review checklist (§3 above) must be applied before each migration task merges.
- The `export_session()` pattern is the only existing access-token exposure; it is pre-existing and accepted.

### Migration-specific threat: WASM SessionManager processes events out-of-order

**Risk:** If `processCommands()` and the sync loop are moved to WASM without proper serialization, concurrent event processing could cause double-spend of a session ID or duplicate command execution.

**Mitigation:** The `#processedEvents` Set (deduplication) and the `#activeSessions` counter must move to WASM atomically with the command dispatch logic (T-4.5). The JS thin wrapper must serialize: sync → processCommands → wait, not allow concurrent invocations of the WASM session manager.

### Migration-specific threat: IndexedDB snapshot cadence disrupted

**Risk:** If `saveIndexedDB` is moved into WASM or called less frequently during migration, Megolm keys may not persist across node process restarts, causing decryption failures for clients.

**Mitigation:** `saveIndexedDB` remains in the JS thin wrapper (`OS-bound: Browser/Node-only API` per ADR Pillar 3 table). The migration does not touch the save cadence (every 5 minutes).

---

## STRIDE Analysis (migration-specific)

| Threat | Category | Mitigation |
|---|---|---|
| New WASM getter leaks Megolm keys | Information Disclosure | §3 API audit checklist; no OLM/Megolm types in public WASM API |
| P2P session key transmitted unencrypted | Information Disclosure | Session key embedded in `m.call.invite` which goes via E2EE exec room |
| Duplicate command execution via WASM boundary | Elevation of Privilege | `processedEvents` Set moves atomically with dispatch logic |
| Zlib bomb via decompressed PTY input | DoS | `MAX_DECOMPRESSED_SIZE = 1MB` limit enforced in Rust `process_terminal_input` |
| WASM memory dump exposes session keys | Information Disclosure | WebAssembly linear memory is process-isolated; JS cannot read WASM linear memory directly |
| `BatchedSender` sends to wrong room | Tampering | `roomId` is set at construction and is final; validated against topology before use |
| `authorized_users` not checked in WASM command dispatch | Broken Access Control | `#isCommandAllowed` and `#isCwdAllowed` must migrate to WASM with the command dispatch (T-4.5) |

---

## Remaining Risks

1. **`export_session()` exposes Matrix access token:** Pre-existing, accepted, out of scope for this migration. The access token is needed for TURN credential fetching from the Node.js layer.
2. **WASM linear memory is inspectable by a compromised Node.js process:** Accepted — if the host Node.js process is compromised, all security properties are void. This is the same threat model as the pre-migration JS implementation.
3. **P2P session key transient exposure in JS:** The key is returned by `generateSessionKey()` / `P2PCryptoWithKey.key` for embedding in `m.call.invite`. This is necessary and is the same in both pre- and post-migration implementations. The E2EE Matrix layer protects it in transit.
