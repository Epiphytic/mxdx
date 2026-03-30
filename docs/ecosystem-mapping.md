# Ecosystem Feature Mapping: Rust ↔ npm/WASM

> **Rule**: When a feature is modified in either ecosystem, the developer MUST update the corresponding entry and file in the other ecosystem. This document is the single source of truth.

## Feature Parity Table

| Feature | npm File | npm Function | Rust File | Rust Function | Status |
|---|---|---|---|---|---|
| **Session & Identity** | | | | | |
| Session restore | `packages/core/session.js` | `connectWithSession()` | `crates/mxdx-matrix/src/session.rs` | `connect_with_session()` | Done (Phase 3) |
| Session data export | `packages/core/session.js` | `client.exportSession()` | `crates/mxdx-matrix/src/client.rs` | `export_session()` | Done (Phase 3) |
| OS keychain | `packages/core/credentials.js` | `CredentialStore` (keytar) | `crates/mxdx-types/src/keychain_os.rs` | `OsKeychain` | Done (Phase 2) |
| File keychain | `packages/core/credentials.js` | `#getSecret/#setSecret` | `crates/mxdx-types/src/keychain_file.rs` | `FileKeychain` | Done (Phase 2) |
| Chained keychain | `packages/core/credentials.js` | `CredentialStore` | `crates/mxdx-types/src/keychain_chain.rs` | `ChainedKeychain` | Done (Phase 2) |
| Crypto persistence | `packages/core/persistent-indexeddb.js` | `saveIndexedDB()` | `crates/mxdx-matrix/src/client.rs` | `login_and_connect_persistent()` | Done (Phase 1) |
| Device reuse | `packages/core/session.js` | `restoreSession()` | `crates/mxdx-matrix/src/session.rs` | `connect_with_session()` | Done (Phase 3) |
| Config write-back | `packages/launcher/src/runtime.js:382` | password removal | `crates/mxdx-types/src/config.rs` | `remove_passwords_from_config()` | Done (Phase 4) |
| | | | | | |
| **Matrix Connectivity** | | | | | |
| Multi-HS client | `packages/core/multi-hs-client.js` | `MultiHsClient` | `crates/mxdx-matrix/src/multi_hs.rs` | `MultiHsClient` | Done |
| Circuit breaker | `packages/core/multi-hs-client.js` | `_recordFailure` | `crates/mxdx-matrix/src/multi_hs.rs` | `record_failure()` | Done |
| Cross-signing sync | `packages/core/session.js:100` | `bootstrapCrossSigningIfNeeded` | `crates/mxdx-matrix/src/multi_hs.rs` | `bootstrap_and_sync_trust()` | Done |
| Event deduplication | `packages/core/multi-hs-client.js` | `_isDuplicate()` | `crates/mxdx-matrix/src/multi_hs.rs` | `EventDedup` | Done |
| | | | | | |
| **Worker Features** | | | | | |
| Batched output | `packages/core/batched-sender.js` | `BatchedSender` | `crates/mxdx-worker/src/batched_sender.rs` | `BatchedSender` | Done (Phase 5) |
| Exponential backoff | `packages/launcher/src/runtime.js:507` | backoff logic | `crates/mxdx-worker/src/lib.rs` | `SyncBackoff` | Done (Phase 6) |
| Session disk persistence | `packages/launcher/src/runtime.js` | `#saveSessionsFile` | `crates/mxdx-worker/src/session_persist.rs` | `save_sessions()` | Done (Phase 6) |
| Session recovery | `packages/launcher/src/runtime.js` | `#loadSessionsFile` | `crates/mxdx-worker/src/session_persist.rs` | `recover_sessions()` | Done (Phase 6) |
| mxdx-exec wrapper | (no equivalent — npm uses direct PTY) | — | `crates/mxdx-worker/src/bin/mxdx_exec.rs` | `main()` | Done |
| | | | | | |
| **P2P / WebRTC** | | | | | |
| P2P transport | `packages/core/p2p-transport.js` | `P2PTransport` | — | — | Design (Phase 7) |
| P2P crypto | `packages/core/p2p-crypto.js` | `P2PCrypto` | — | — | Design (Phase 7) |
| P2P signaling | `packages/core/p2p-signaling.js` | `P2PSignaling` | — | — | Design (Phase 7) |
| SessionMux | `packages/launcher/src/runtime.js` | `SessionMux` | — | — | Design (Phase 7) |
| TURN credentials | `packages/core/turn-credentials.js` | `fetchTurnCredentials` | — | — | Design (Phase 7) |
| | | | | | |
| **Shared Types** | | | | | |
| Session events | `packages/core/events/session.js` | event constants | `crates/mxdx-types/src/events/session.rs` | `SESSION_*` constants | Done |
| Config loading | `packages/core/config.js` | config file parsing | `crates/mxdx-types/src/config.rs` | `load_merged_config()` | Done |
| Server accounts | `packages/core/config.js` | `AccountConfig` | `crates/mxdx-matrix/src/multi_hs.rs` | `ServerAccount` | Done |

## Key Format Compatibility

Both ecosystems use identical formats for cross-ecosystem credential sharing:

| Item | Format | Example |
|---|---|---|
| Session keychain key | `mxdx:{username}@{normalized_server}:session` | `mxdx:alice@matrix.org:session` |
| Password keychain key | `mxdx:{username}@{normalized_server}:password` | `mxdx:alice@matrix.org:password` |
| Keytar service name | `mxdx` | — |
| File keychain encryption | AES-256-GCM, key=SHA256(hostname:uid:mxdx-credential-store) | — |
| File keychain wire format | `IV(16) \|\| AuthTag(16) \|\| Ciphertext` → base64 | — |
| File keychain path | `~/.config/mxdx/{sanitized_key}.enc` | — |
| Crypto store (npm) | IndexedDB → encrypted snapshot | `~/.config/mxdx/indexeddb-snapshot.enc` |
| Crypto store (Rust) | SQLite via matrix-sdk | `~/.mxdx/crypto/{role}/{server_hash}/` |

## Server normalization

Both ecosystems normalize server URLs identically:
- Strip `https://` or `http://` prefix
- Strip trailing `/`
- Example: `https://matrix.org/` → `matrix.org`
