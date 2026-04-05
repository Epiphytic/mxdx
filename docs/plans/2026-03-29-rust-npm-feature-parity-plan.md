# Rust/npm Feature Parity Plan

## Context

The Rust binaries (`mxdx-worker`, `mxdx-client`) have working Matrix connectivity with E2EE, multi-homeserver failover, cross-signing trust sync, and passing E2E tests. However, they create a **new Matrix device on every restart** because the crypto store uses `tempfile::TempDir` and the keychain is `InMemoryKeychain`. The npm/WASM side has mature persistence: OS keychain via keytar, encrypted file fallback, IndexedDB crypto snapshots, and session restore via `connect_with_token`. This plan ports those features to Rust so both ecosystems behave identically, then creates a mapping document so changes in one are mirrored in the other.

---

## Development Methodology: Test-Driven Design

All phases MUST be executed using TDD (Red → Green → Refactor):

1. **Write failing tests first** for each feature being ported, before writing any implementation code.
2. **Unit tests** for each new module/function as it's created.
3. **Integration tests** for cross-module interactions (e.g., keychain + session restore).
4. **E2E regression gate** after every phase completes (see below).

### E2E Regression Gate

After completing each phase, run the **full E2E test suite** and the **profiling benchmarks**:

```bash
# Full E2E suite (5 tests)
cargo test -p mxdx-worker --test e2e_binary -- --ignored --nocapture

# Profiling benchmarks (local + federated + SSH baseline)
cargo test -p mxdx-worker --test e2e_profile -- --ignored --nocapture
```

**Record a baseline table** before Phase 1 begins. After each phase, compare results:

| Workload | SSH Baseline | mxdx-local | mxdx-federated | Phase |
|---|---|---|---|---|
| echo | — | — | — | baseline |
| exit-code | — | — | — | baseline |
| md5sum(10k) | — | — | — | baseline |
| ping(30s) | — | — | — | baseline |
| ping(5min) | — | — | — | baseline |

**Blocking rules:**
- If any E2E test **fails** → investigate and fix before continuing to the next phase.
- If any E2E test or profiling benchmark takes **>10% longer** than the previous phase → investigate the regression before continuing. Document the cause and either fix it or explicitly justify why the regression is acceptable (e.g., added encryption overhead).
- New feature tests (TURN, P2P, interactive sessions) should be **added to the suite** as they become testable, expanding the regression baseline going forward.

**Test results must be printed/logged** at the end of each phase with a comparison to the prior phase, so regressions are visible immediately.

---

## Phase 1: Persistent Crypto Store

**Problem**: `MatrixClient._store_dir` is `tempfile::TempDir` — deleted on process exit. All E2EE keys lost.

**Fix**: Use `~/.mxdx/crypto/{user_id_hash}/` instead of a temp dir.

**Files**:
- Modify: `crates/mxdx-matrix/src/client.rs` — Replace `_store_dir: tempfile::TempDir` with an enum `StoreDir { Temp(TempDir), Persistent(PathBuf) }`. Add `login_and_connect_persistent()` and `connect_with_token_persistent()` that use a stable `PathBuf`. Keep temp-based constructors for tests.
- Modify: `crates/mxdx-matrix/src/multi_hs.rs` — `MultiHsClient::connect()` accepts optional `store_base_path: Option<PathBuf>`, creates per-server subdirs.
- Reuse: `dirs::home_dir()` already available in `mxdx-types` deps.

**Test**: Create client with persistent path, drop it, create new client at same path, verify SQLite file survives.

---

## Phase 2: OS Keychain Backend

**Problem**: `KeychainBackend` trait exists (`crates/mxdx-types/src/identity.rs:38`) but only `InMemoryKeychain` is implemented. npm uses keytar (service `"mxdx"`, key format `mxdx:{username}@{server}:{field}`).

**Fix**: Implement `OsKeychain` using `keyring` crate + `FileKeychain` with AES-256-GCM fallback.

**Files**:
- Create: `crates/mxdx-types/src/keychain_os.rs` — `OsKeychain` implementing `KeychainBackend` via `keyring` crate. Service: `"mxdx"`. Must match npm key format for cross-ecosystem credential sharing.
- Create: `crates/mxdx-types/src/keychain_file.rs` — `FileKeychain` with AES-256-GCM. Key derivation: `SHA256(hostname:uid:mxdx-credential-store)` matching `packages/core/credentials.js`. Wire format: `iv(16) || tag(16) || ciphertext` base64-encoded. File permissions `0o600`.
- Create: `crates/mxdx-types/src/keychain_chain.rs` — `ChainedKeychain` tries OS keychain first, falls back to file.
- Modify: `crates/mxdx-types/src/lib.rs` — export new modules.
- Modify: `crates/mxdx-types/Cargo.toml` — add `keyring = "3"`, `aes-gcm = "0.10"`, `sha2 = "0.10"`.

**Reference**: `packages/core/credentials.js` lines 130-189 (keytar), 115-128 (file fallback), 37-40 (key format).

**Test**: OsKeychain set/get/delete (ignore in CI), FileKeychain round-trip + permission check, ChainedKeychain fallback.

---

## Phase 3: Session Restore + Device Reuse

**Problem**: Rust always calls `login_and_connect()` → new device. npm tries `restoreSession()` first → same device.

**Fix**: Port `packages/core/session.js` `connectWithSession()` flow to Rust.

**Files**:
- Create: `crates/mxdx-matrix/src/session.rs` — `SessionData { user_id, device_id, access_token, homeserver_url }` + `connect_with_session(keychain, server, username, password, store_path)` that:
  1. Loads session from keychain (`mxdx:{username}@{server}:session`)
  2. If found → `connect_with_token_persistent()` (Phase 1)
  3. If restore fails → load password from keychain or prompt via TTY
  4. Fresh login → `login_and_connect_persistent()`
  5. Bootstrap cross-signing if fresh
  6. Save session + password to keychain
- Modify: `crates/mxdx-matrix/src/client.rs` — add `export_session() -> SessionData` method.
- Modify: `crates/mxdx-matrix/src/multi_hs.rs` — `MultiHsClient::connect()` variant accepting keychain for session restore per server.
- Modify: `crates/mxdx-worker/src/lib.rs` lines 107, 114 — replace `InMemoryKeychain::new()` with `ChainedKeychain::new()`.
- Modify: `crates/mxdx-client/src/matrix.rs` — `connect_multi()` uses `connect_with_session()`.

**Reference**: `packages/core/session.js` lines 24-131 (full flow).

**Test**: E2E test: start worker, connect, stop, restart, verify same device_id reused (check log output). Delete keychain → fresh login on next start.

---

## Phase 4: Config Write-Back

**Problem**: After keychain save, npm removes password from TOML config. Rust leaves it in plaintext.

**Files**:
- Modify: `crates/mxdx-types/src/config.rs` — add `remove_passwords_from_config(filename)` that loads TOML, strips `password` fields from `[[accounts]]`, writes back with `0o600` perms.
- Modify: `crates/mxdx-worker/src/lib.rs` — call after fresh login + keychain save.
- Modify: `crates/mxdx-client/src/main.rs` — same.

**Reference**: `packages/launcher/src/runtime.js` lines 382-391.

**Test**: Write TOML with passwords, call remove, verify passwords gone, other fields preserved.

---

## Phase 5: BatchedSender

**Problem**: Rust posts raw events per output chunk. npm batches over 200ms with zlib compression and 429 backoff.

**Files**:
- Create: `crates/mxdx-worker/src/batched_sender.rs` — `BatchedSender` with:
  - Configurable batch window (default 200ms, 5ms for P2P later)
  - `flate2` deflate compression when payload >= 32 bytes
  - 429 detection: parse `retry_after_ms`, wait, coalesce queued data
  - Sequence numbers, optional `session_id`
  - Uses `tokio::time` + `tokio::sync::mpsc`
- Modify: `crates/mxdx-worker/src/lib.rs` — wire output streaming through `BatchedSender`.

**Reference**: `packages/core/batched-sender.js` lines 90-260.

**Test**: Push 10 small chunks → verify 1 batched event. Simulate 429 → verify retry + coalescing.

---

## Phase 6: Exponential Backoff + Session Disk Persistence

**Backoff**: Add 1s→30s doubling backoff to worker sync loop (`lib.rs` main loop) and client sync loop. Reset on success.

**Session Persistence**: Add `sessions.json` / `session-rooms.json` to `~/.mxdx/` with save on claim/complete, load + tmux recovery on startup.

**Files**:
- Modify: `crates/mxdx-worker/src/lib.rs` — backoff in main loop.
- Modify: `crates/mxdx-worker/src/session.rs` — `save_to_disk()` / `load_from_disk()` / `recover_sessions()`.

**Reference**: `packages/launcher/src/runtime.js` lines 206-304 (persistence), 507-509 (backoff).

---

## Phase 7: WebRTC / P2P (Design Only — Deferred)

No code changes. Document the path:
- Use `webrtc-rs` or `datachannel-rs` for Rust WebRTC
- Port `p2p-crypto.js` using `aes-gcm` crate (same as Phase 2)
- Port `p2p-signaling.js` as thin Matrix event wrapper
- Port `p2p-transport.js` state machine using Rust enums + tokio
- Port `SessionMux` as multi-session router

---

## Phase 8: Ecosystem Mapping Document

Create `docs/ecosystem-mapping.md` with a table mapping every feature between ecosystems:

| Feature | npm File | npm Function | Rust File | Rust Function | Status |
|---|---|---|---|---|---|
| Session restore | `packages/core/session.js` | `connectWithSession()` | `crates/mxdx-matrix/src/session.rs` | `connect_with_session()` | Phase 3 |
| OS keychain | `packages/core/credentials.js` | `CredentialStore` | `crates/mxdx-types/src/keychain_os.rs` | `OsKeychain` | Phase 2 |
| File keychain | `packages/core/credentials.js` | `#getSecret/#setSecret` | `crates/mxdx-types/src/keychain_file.rs` | `FileKeychain` | Phase 2 |
| Crypto persistence | `packages/core/persistent-indexeddb.js` | `saveIndexedDB()` | `crates/mxdx-matrix/src/client.rs` | `sqlite_store(persistent_path)` | Phase 1 |
| Batched output | `packages/core/batched-sender.js` | `BatchedSender` | `crates/mxdx-worker/src/batched_sender.rs` | `BatchedSender` | Phase 5 |
| Session disk persistence | `packages/launcher/src/runtime.js` | `#saveSessionsFile` | `crates/mxdx-worker/src/session.rs` | `save_to_disk()` | Phase 6 |
| Config write-back | `packages/launcher/src/runtime.js:382` | password removal | `crates/mxdx-types/src/config.rs` | `remove_passwords_from_config()` | Phase 4 |
| Multi-HS client | `packages/core/multi-hs-client.js` | `MultiHsClient` | `crates/mxdx-matrix/src/multi_hs.rs` | `MultiHsClient` | Done |
| Circuit breaker | `packages/core/multi-hs-client.js` | `_recordFailure` | `crates/mxdx-matrix/src/multi_hs.rs` | `record_failure()` | Done |
| Cross-signing sync | `packages/core/session.js:100` | `bootstrapCrossSigningIfNeeded` | `crates/mxdx-matrix/src/multi_hs.rs` | `bootstrap_and_sync_trust()` | Done |
| mxdx-exec wrapper | (no equivalent — npm uses direct PTY) | — | `crates/mxdx-worker/src/bin/mxdx_exec.rs` | `main()` | Done |
| P2P transport | `packages/core/p2p-transport.js` | `P2PTransport` | — | — | Phase 7 |
| P2P crypto | `packages/core/p2p-crypto.js` | `P2PCrypto` | — | — | Phase 7 |
| P2P signaling | `packages/core/p2p-signaling.js` | `P2PSignaling` | — | — | Phase 7 |
| SessionMux | `packages/launcher/src/runtime.js` | `SessionMux` | — | — | Phase 7 |
| TURN credentials | `packages/core/turn-credentials.js` | `fetchTurnCredentials` | — | — | Phase 7 |

**Rule**: When a feature is modified in either ecosystem, the developer MUST update the corresponding entry and file in the other ecosystem. The mapping document is the single source of truth.

---

## Execution Order

```
Phase 1 (crypto store) → Phase 2 (keychain) → Phase 3 (session restore) → Phase 4 (config)
                                                      ↓
                                                Phase 6 (session disk + backoff)

Phase 5 (batched sender) — independent, can parallel with Phase 2+
Phase 7 (WebRTC) — deferred
Phase 8 (mapping doc) — written incrementally as each phase completes
```

## Verification

### Per-Phase Gate (must pass before advancing)

1. **Unit tests**: `cargo test -p mxdx-worker -p mxdx-client -p mxdx-matrix -p mxdx-types` — all pass (including newly written TDD tests for this phase)
2. **E2E suite**: `cargo test -p mxdx-worker --test e2e_binary -- --ignored --nocapture` — all tests pass
3. **Profiling benchmarks**: `cargo test -p mxdx-worker --test e2e_profile -- --ignored --nocapture` — all pass, no >10% regression vs prior phase
4. **Phase-specific tests**: As described in each phase section (written TDD-style before implementation)
5. **Security review**: No plaintext credentials in files, proper file permissions, keychain entries encrypted
6. **Regression comparison table**: Print side-by-side timing comparison with previous phase; investigate any >10% slowdown

### New Tests Added Per Phase

As features are ported, add corresponding E2E tests to the suite:
- **Phase 3**: Device reuse test (restart worker, verify same device_id)
- **Phase 5**: Batched output test (verify event count reduction under load)
- **Phase 6**: Session recovery test (kill worker, restart, verify session resumes)
- **Phase 7**: TURN connectivity test, P2P interactive session test (when implemented)

These new tests become part of the regression baseline for all subsequent phases.

### Final Verification (after all phases)

- Start worker → connect → stop → restart → verify **same device_id**, no re-login
- npm launcher and Rust worker on same machine → verify they **share keychain entries**
- Run profiling benchmarks → verify batched sender reduces event count
- Full comparison table: baseline vs Phase 1 vs Phase 2 vs ... vs final — confirm no unaccounted regressions
- Compare mxdx with/without federation, with/without TURN servers, and SSH baseline — all executing the same commands for consistent comparison
