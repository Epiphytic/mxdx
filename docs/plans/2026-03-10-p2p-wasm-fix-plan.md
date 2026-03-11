# P2P WASM Integration Fix Plan

> **For Claude:** REQUIRED SUB-SKILL: Use superpowers:executing-plans to implement this plan task-by-task.

**Goal:** Fix the P2P transport layer to actually work in production by adding missing WASM encryption/signing methods and fixing browser compatibility.

**Problem:** The P2P transport was built and tested with mocks, but the WASM client doesn't expose the required `encryptRoomEvent`, `decryptEvent`, `signWithDeviceKey`, or `verifyDeviceSignature` methods. Additionally, `node:crypto` imports break the browser build.

**Architecture:** Use ephemeral X25519 ECDH per P2P session for data encryption (AES-256-GCM), bound to Matrix device identity via Ed25519 signature verification. This avoids Megolm ratchet conflicts from a second OlmMachine and provides per-session forward secrecy.

---

## Task 1: Fix `node:crypto` imports for browser compatibility

**Files:**
- Modify: `packages/core/p2p-transport.js`
- Modify: `packages/core/p2p-signaling.js`

**What:** Replace `import { randomBytes } from 'node:crypto'` with a cross-platform helper that uses `globalThis.crypto.getRandomValues` (browser + Node 19+) with a dynamic `require('node:crypto')` fallback for older Node.

```javascript
function randomHex(byteCount) {
  if (typeof globalThis.crypto?.getRandomValues === 'function') {
    const buf = new Uint8Array(byteCount);
    globalThis.crypto.getRandomValues(buf);
    return Array.from(buf, b => b.toString(16).padStart(2, '0')).join('');
  }
  // eslint-disable-next-line no-restricted-modules
  return require('node:crypto').randomBytes(byteCount).toString('hex');
}
```

Replace all `randomBytes(N).toString('hex')` calls with `randomHex(N)`.

**Verify:** `node -e "import('./packages/core/p2p-transport.js')"` doesn't crash. Vite build for web-console resolves the import.

**Commit:** `fix: replace node:crypto with cross-platform randomHex in P2P modules`

---

## Task 2: Add Rust WASM method — `signWithDeviceKey`

**Files:**
- Modify: `crates/mxdx-core-wasm/Cargo.toml`
- Modify: `crates/mxdx-core-wasm/src/lib.rs`

**Deps to add to Cargo.toml:**
```toml
matrix-sdk-crypto = { version = "0.16", default-features = false }
matrix-sdk-indexeddb = { version = "0.16", default-features = false, features = ["e2e-encryption"] }
vodozemac = "0.9"
```

These are already transitive deps in Cargo.lock — no new downloads.

**Rust implementation:** Create a lazy `OlmMachine` field on `WasmMatrixClient` sharing the same IndexedDB crypto store (store name + `::matrix-sdk-crypto` suffix). `OlmMachine::sign()` only reads the static Ed25519 key — no ratchet conflicts.

```rust
#[wasm_bindgen(js_name = "signWithDeviceKey")]
pub async fn sign_with_device_key(&self, message: &str) -> Result<String, JsValue> {
    let machine = self.get_or_init_olm_machine().await?;
    let sigs = machine.sign(message).await.map_err(to_js_err)?;
    let user_id = machine.user_id();
    let device_key_id = format!("ed25519:{}", machine.device_id());
    // Extract our Ed25519 signature
    let sig = sigs.get_signature(user_id, &device_key_id.into())
        .ok_or_else(|| to_js_err("No signature found"))?;
    Ok(sig.to_base64())
}
```

**Commit:** `feat(wasm): add signWithDeviceKey for P2P peer verification`

---

## Task 3: Add Rust WASM method — `verifyDeviceSignature`

**Files:**
- Modify: `crates/mxdx-core-wasm/src/lib.rs`

**Rust implementation:** Use `client.encryption().get_device()` to get the remote device's Ed25519 public key, then verify with vodozemac.

```rust
#[wasm_bindgen(js_name = "verifyDeviceSignature")]
pub async fn verify_device_signature(
    &self, user_id: &str, message: &str, signature: &str, device_id: &str
) -> Result<bool, JsValue> {
    let uid = UserId::parse(user_id).map_err(to_js_err)?;
    let did = device_id.into();
    let device = self.client.encryption()
        .get_device(&uid, did).await.map_err(to_js_err)?
        .ok_or_else(|| to_js_err("Device not found"))?;
    let ed25519_key = device.ed25519_key()
        .ok_or_else(|| to_js_err("No Ed25519 key"))?;
    let sig = Ed25519Signature::from_base64(signature).map_err(to_js_err)?;
    Ok(ed25519_key.verify(message.as_bytes(), &sig).is_ok())
}
```

**JS interface change:** `verifySignatureFn` now takes `(userId, message, signature, deviceId)` — add `userId` parameter.

**Commit:** `feat(wasm): add verifyDeviceSignature for P2P peer verification`

---

## Task 4: Add Rust WASM methods — P2P session encryption (AES-256-GCM)

**Files:**
- Modify: `crates/mxdx-core-wasm/src/lib.rs`

**Why not Megolm:** Two OlmMachines sharing a store will advance the Megolm ratchet independently, causing message index conflicts. Instead, use ephemeral X25519 ECDH + AES-256-GCM per P2P session.

**New WASM methods:**
- `generateP2PKeypair() -> String` — returns `{ publicKey: "base64..." }`, stores private key internally
- `deriveP2PSessionKey(remotePublicKey: string) -> void` — X25519 ECDH → HKDF → AES-256-GCM key
- `encryptP2PFrame(plaintext: string) -> String` — AES-256-GCM encrypt, returns `{ ciphertext: "base64", nonce: "base64" }`
- `decryptP2PFrame(ciphertextJson: string) -> String` — AES-256-GCM decrypt, returns plaintext

**Use `x25519-dalek` or vodozemac's Curve25519 for ECDH.** Use `aes-gcm` crate for AES-256-GCM.

**Add to Cargo.toml:**
```toml
x25519-dalek = { version = "2", features = ["static_secrets"] }
aes-gcm = "0.10"
```

**Internal state on WasmMatrixClient:**
```rust
struct P2PSession {
    aes_key: aes_gcm::Key<Aes256Gcm>,
    nonce_counter: u64,
}
p2p_session: Option<P2PSession>,
p2p_private_key: Option<x25519_dalek::StaticSecret>,
```

**Commit:** `feat(wasm): add P2P session encryption with X25519 ECDH + AES-256-GCM`

---

## Task 5: Update JS P2P modules to use new WASM methods

**Files:**
- Modify: `packages/core/p2p-transport.js`
- Modify: `packages/web-console/src/terminal-view.js`
- Modify: `packages/launcher/src/runtime.js`

**Changes:**
1. `P2PTransport.create()` interface changes:
   - `encryptFn(plaintext) -> ciphertextJson` (was `encryptFn(roomId, type, content)`)
   - `decryptFn(ciphertextJson) -> plaintext` (same)
   - `signFn(message) -> signature` (same)
   - `verifySignatureFn(userId, message, signature, deviceId) -> bool` (added `userId`)
   - New: `generateKeypairFn() -> { publicKey }`, `deriveSessionKeyFn(remotePublicKey) -> void`

2. `peer_verify` frame adds `user_id` and `curve25519_key` fields

3. Verification flow in `#startVerification()`:
   - Generate ephemeral keypair via `generateKeypairFn()`
   - Send `{ type: 'peer_verify', nonce, device_id, user_id, curve25519_key }`
   - On receiving remote's `peer_verify`, sign their nonce AND exchange Curve25519 keys
   - After both sides verified, call `deriveSessionKeyFn(remoteCurve25519Key)` to establish AES key
   - THEN set `#peerVerified = true`

4. `terminal-view.js` wires:
   ```javascript
   encryptFn: (plaintext) => client.encryptP2PFrame(plaintext),
   decryptFn: (ciphertext) => client.decryptP2PFrame(ciphertext),
   signFn: (msg) => client.signWithDeviceKey(msg),
   verifySignatureFn: (userId, msg, sig, devId) => client.verifyDeviceSignature(userId, msg, sig, devId),
   generateKeypairFn: () => client.generateP2PKeypair(),
   deriveSessionKeyFn: (remotePk) => client.deriveP2PSessionKey(remotePk),
   ```

5. `runtime.js` same pattern but for Node.js WASM.

**Commit:** `feat: wire WASM P2P encryption and signing into transport layer`

---

## Task 6: WASM rebuild + browser build verification

**Steps:**
```bash
# Node.js target
wasm-pack build crates/mxdx-core-wasm --target nodejs --out-dir ../../packages/core/wasm

# Browser target
wasm-pack build crates/mxdx-core-wasm --target web --out-dir ../../packages/web-console/wasm

# Verify web-console builds
cd packages/web-console && npx vite build
```

**Commit:** `build: rebuild WASM for Node.js and browser targets with P2P methods`

---

## Task 7: Update tests for new WASM integration

**Files:**
- Modify: `packages/e2e-tests/tests/p2p-transport.test.js`
- Modify: `packages/e2e-tests/tests/p2p-e2e.test.js`
- Modify: `packages/e2e-tests/tests/p2p-tuwunel.test.js`

**Changes:**
- Update mock functions to match new signatures (`encryptFn(plaintext)` not `encryptFn(roomId, type, content)`)
- Update `verifySignatureFn` mock to accept `userId` parameter
- Add mocks for `generateKeypairFn` and `deriveSessionKeyFn`
- Tuwunel E2E test should use real WASM signing if WASM is available

**Verify:** All 36 tests still pass.

**Commit:** `test: update P2P tests for new WASM encryption interface`

---

## Task 8: End-to-end browser verification

**Steps:**
1. Start Tuwunel locally
2. Start launcher against Tuwunel
3. Run `npx vite dev` in packages/web-console
4. Open browser, log in, start terminal session
5. Verify in browser devtools:
   - No console errors from missing methods
   - P2P status indicator shows "Connecting..." then "P2P" (or "Matrix" if no TURN)
   - Terminal data flows (type commands, see output)
6. Check Network tab — terminal data should NOT appear as Matrix HTTP requests when P2P is active

**Commit:** (no code commit — manual verification)

---

## Security Notes

- Ed25519 signing key never exported from WASM — only accessed via OlmMachine::sign()
- Ephemeral X25519 keypairs provide forward secrecy per P2P session
- AES-256-GCM provides authenticated encryption (tampering detected)
- Peer identity bound to Matrix device via Ed25519 signature verification
- WASM boundary prevents JS from accessing raw key material
- No plaintext terminal data ever flows over the data channel
