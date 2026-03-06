# Public Matrix Server Interoperability — Gap Analysis & Fix Plan

**Date:** 2026-03-06
**Status:** Draft
**Context:** All mxdx development and testing has used local Tuwunel instances. This document identifies every Tuwunel-specific assumption in the codebase and proposes fixes for public server compatibility.

---

## 1. Findings: Tuwunel-Specific Assumptions

### 1.1 Registration Token Authentication (CRITICAL)

**Files:**
- `crates/mxdx-matrix/src/client.rs:19` — hardcoded `REGISTRATION_TOKEN = "mxdx-test-token"`
- `crates/mxdx-matrix/src/client.rs:30-62` — `register_and_connect()` uses `m.login.registration_token` auth type
- `tests/helpers/src/tuwunel.rs:9` — same hardcoded token
- `tests/helpers/src/tuwunel.rs:175-222` — `register_user()` uses same pattern

**Issue:** `m.login.registration_token` is a valid Matrix spec auth type (MSC3231, merged into spec v1.2), but public servers like matrix.org do not enable token-based registration. They use UIAA flows with reCAPTCHA, email verification, or SSO.

**Impact:** `MatrixClient::register_and_connect()` cannot create accounts on public servers. The hardcoded token value is only meaningful for Tuwunel test instances.

**Fix:**
1. Add a `MatrixClient::login_and_connect(homeserver_url, username, password)` method that skips registration entirely — just builds the Client and calls `login_username()`. This is what public server usage needs.
2. Rename `register_and_connect()` to clarify it's for self-hosted servers with registration tokens.
3. Make the registration token a parameter instead of a hardcoded constant.
4. The test helper `TuwunelInstance::register_user()` is fine as-is — it's explicitly for test infrastructure.

**Priority:** P0 — blocks all public server usage

---

### 1.2 Appservice Registration via Admin Room (CRITICAL — Inherently Self-Hosted)

**Files:**
- `crates/mxdx-policy/src/appservice.rs:86-173` — `register_appservice()` sends `!admin appservices register` command
- `crates/mxdx-policy/src/appservice.rs:93-94` — finds `#admins` room (Tuwunel auto-creates this)
- `crates/mxdx-policy/src/appservice.rs:99` — Tuwunel-specific admin command format with YAML in markdown code block
- `crates/mxdx-policy/src/appservice.rs:156` — checks for "Appservice registered" response text (Tuwunel-specific)
- `crates/mxdx-policy/src/appservice.rs:159` — checks for "Command failed" response text (Tuwunel-specific)
- `crates/mxdx-policy/src/appservice.rs:175-225` — `find_admin_room()` looks for `#admins:` canonical alias
- `crates/mxdx-policy/src/appservice.rs:218-220` — fallback: assumes single joined room is admin room (Tuwunel behavior for first user)

**Issue:** This is entirely Tuwunel-specific. Other servers have different appservice registration mechanisms:
- **Synapse:** Requires editing `homeserver.yaml` and restarting
- **Dendrite:** Requires editing `dendrite.yaml` and restarting
- **Public servers:** Not possible at all

**Impact:** The policy engine's appservice functionality is inherently a self-hosted feature. This is acceptable and expected.

**Fix:**
1. Document that appservice registration is self-hosted only.
2. Add a Synapse-compatible registration path using the Synapse Admin API (`/_synapse/admin/v1/register`) as an alternative to the Tuwunel admin room approach.
3. Make the appservice registration strategy configurable: `tuwunel_admin_room`, `synapse_admin_api`, `manual` (user provides pre-registered config).
4. Add a `ManualAppserviceConfig` that reads a pre-registered appservice YAML — for when the server admin has already registered the appservice via their server's native mechanism.

**Priority:** P1 — only blocks appservice features, not core Matrix operations

---

### 1.3 `.localhost` TLD for Server Names (LOW)

**Files:**
- `tests/helpers/src/tuwunel.rs:30` — `server_name = format!("test-{}.localhost", port)`
- `tests/helpers/src/federation.rs:23` — `"hs-a.localhost"`
- `tests/helpers/src/federation.rs:27` — `"hs-b.localhost"`

**Issue:** Uses `.localhost` TLD (RFC 6761) which resolves to loopback. This is correct for test infrastructure but means test server names will never match real server names.

**Impact:** None for production code. This is test infrastructure only. The MatrixClient itself has no hardcoded server names — it takes `homeserver_url` as a parameter.

**Fix:** None needed. This is working as designed.

---

### 1.4 `http://` Scheme Assumption in Tests (LOW)

**Files:**
- `crates/mxdx-launcher/tests/e2e_full_system.rs:33` — `format!("http://127.0.0.1:{}", hs.port)`
- `crates/mxdx-launcher/tests/e2e_command.rs:66` — same pattern
- `crates/mxdx-launcher/tests/e2e_terminal_session.rs:10` — same pattern
- `crates/mxdx-secrets/tests/e2e_secret_request.rs:17` — same pattern

**Issue:** Tests use `http://` because local Tuwunel doesn't use TLS by default. Public servers require `https://`.

**Impact:** None for production code. `MatrixClient::register_and_connect()` takes `homeserver_url` as a string — it works with both `http://` and `https://`. The matrix-sdk Client builder handles TLS transparently.

**Fix:** None needed for production code. Test infrastructure correctly uses `http://` for local instances and `https://` for federated TLS instances.

---

### 1.5 Hardcoded Registration Token in MatrixClient (MEDIUM)

**Files:**
- `crates/mxdx-matrix/src/client.rs:19` — `const REGISTRATION_TOKEN: &str = "mxdx-test-token";`

**Issue:** The production `MatrixClient` has a hardcoded test registration token. This means:
1. It can only register users on servers configured with this exact token
2. It cannot register users on servers with different tokens
3. It cannot work with servers that don't support token registration

**Impact:** The MatrixClient is tightly coupled to the test infrastructure's token value.

**Fix:**
1. Remove the hardcoded constant from `client.rs`
2. Add `register_and_connect()` that accepts an optional registration token parameter
3. Add `login_and_connect()` that skips registration entirely

**Priority:** P0 — part of the same fix as 1.1

---

## 2. Findings: Standard Matrix API Usage (No Issues)

The following Matrix API calls are fully spec-compliant and work on all servers:

| API Call | File | Line | Spec Status |
|:---|:---|:---|:---|
| `POST /_matrix/client/v3/register` | client.rs | 37 | Standard (auth type is the variable) |
| `Client::builder().homeserver_url().sqlite_store().build()` | client.rs | 68-72 | matrix-sdk internal |
| `matrix_auth().login_username()` | client.rs | 76-79 | Standard `m.login.password` |
| `client.user_id()` | client.rs | 89 | Standard |
| `encryption().ed25519_key()` | client.rs | 94 | Standard E2EE |
| `client.create_room(request)` | client.rs | 113 | Standard `POST /createRoom` |
| `InitialStateEvent::new(EmptyStateKey, RoomEncryptionEventContent)` | client.rs | 107 | Standard |
| `client.join_room_by_id()` | client.rs | 133 | Standard |
| `room.send_raw(event_type, content)` | client.rs | 150 | Standard — custom event types allowed |
| `room.send_state_event_raw()` | client.rs | 167 | Standard |
| `client.sync_once(SyncSettings)` | client.rs | 174 | Standard `/sync` |
| `room.messages(MessagesOptions::backward())` | client.rs | 203 | Standard `/messages` |
| `CreationContent { room_type: Some(RoomType::Space) }` | rooms.rs | 37-38 | Standard (Spaces spec v1.2+) |
| `m.space.child` state event | rooms.rs | 56 | Standard |
| `is_direct: true` on CreateRoomRequest | rooms.rs | 81 | Standard |
| `HistoryVisibility::Joined` initial state | rooms.rs | 76-77 | Standard |
| `m.room.tombstone` state event | rooms.rs | 102 | Standard |
| `GET /rooms/{id}/state/{type}` | rooms.rs | 118-119 | Standard |
| `client.access_token()` | rooms.rs | 115 | Standard |

## 3. Findings: Custom Event Types

mxdx defines these custom event types (from `crates/mxdx-types/src/events/`):

| Event Type | Usage | Spec Compliance |
|:---|:---|:---|
| `org.mxdx.command` | Command execution requests | Custom type — fully allowed by spec |
| `org.mxdx.output` | Command output streaming | Custom type — fully allowed by spec |
| `org.mxdx.result` | Command execution results | Custom type — fully allowed by spec |
| `org.mxdx.telemetry` | System telemetry reports | Custom type — fully allowed by spec |
| `org.mxdx.launcher.*` | Launcher status events | Custom type — fully allowed by spec |
| `org.mxdx.terminal.*` | Terminal session events | Custom type — fully allowed by spec |
| `org.mxdx.secret.request` | Secret request events | Custom type — fully allowed by spec |
| `org.mxdx.secret.response` | Secret response events | Custom type — fully allowed by spec |

The Matrix spec explicitly allows arbitrary event types with the `org.` prefix convention for third-party namespaces. No compatibility issues here.

## 4. Findings: E2EE

E2EE is handled entirely by `matrix-sdk` v0.16 with the `e2e-encryption` and `sqlite` features. The SDK handles:
- Olm account creation and key upload
- Megolm session creation for encrypted rooms
- Key exchange via `/keys/upload`, `/keys/query`, `/keys/claim`
- Event encryption/decryption

This is all standard Matrix E2EE and works identically on all servers. The only behavioral difference across servers is key backup support, which mxdx doesn't use.

## 5. Findings: Behavioral Assumptions

### 5.1 Sync Timing for Key Exchange

**Files:**
- `crates/mxdx-launcher/tests/e2e_command.rs:87-90` — 4 sync cycles for key exchange
- `crates/mxdx-launcher/tests/e2e_full_system.rs:62-65` — same pattern

**Pattern:** After creating an encrypted room and having another user join, the code does 4 alternating `sync_once()` calls to exchange Megolm keys.

**Issue:** This works reliably on local Tuwunel because latency is negligible. On public servers with higher latency, this pattern may need more sync cycles or a smarter retry loop that checks whether keys have actually been exchanged.

**Fix:** Add a `wait_for_key_exchange()` helper method to MatrixClient that syncs in a loop until `room.is_encrypted().await` returns true and the room's members have uploaded device keys. This is more robust than a fixed number of sync cycles.

**Priority:** P2 — affects reliability, not correctness

### 5.2 Rate Limiting

**Issue:** Creating a launcher topology (`create_launcher_space`) creates 4 rooms in rapid succession. Public servers may rate-limit this.

**Fix:** Add optional rate-limiting delay configuration to MatrixClient, defaulting to 0ms for self-hosted and configurable for public servers.

**Priority:** P2

---

## 6. Prioritized Fix Plan

### P0: Login-Only Connection (Blocks all public server usage)

**Change:** Add `MatrixClient::login_and_connect()` alongside existing `register_and_connect()`

**Files to modify:**
- `crates/mxdx-matrix/src/client.rs`
  - Add `login_and_connect(homeserver_url, username, password) -> Result<Self>`
  - Make registration token a parameter in `register_and_connect()`
  - Remove hardcoded `REGISTRATION_TOKEN` constant

**Estimated scope:** ~30 lines added, ~5 lines modified

### P1: Appservice Registration Strategy

**Change:** Make appservice registration pluggable

**Files to modify:**
- `crates/mxdx-policy/src/appservice.rs`
  - Extract `register_appservice()` into a trait `AppserviceRegistrar`
  - Implement `TuwunelRegistrar` (current admin room approach)
  - Implement `ManualRegistrar` (reads pre-existing config)
  - Future: `SynapseRegistrar` (admin API)

**Files to add:**
- `crates/mxdx-policy/src/appservice/mod.rs` — trait + implementations
- `crates/mxdx-policy/src/appservice/tuwunel.rs` — current code
- `crates/mxdx-policy/src/appservice/manual.rs` — manual config

**Estimated scope:** ~100 lines refactored, ~50 lines new

### P2: Robust Key Exchange

**Change:** Replace fixed sync cycles with key-exchange-aware waiting

**Files to modify:**
- `crates/mxdx-matrix/src/client.rs`
  - Add `wait_for_key_exchange(room_id, peer_user_id, timeout) -> Result<()>`

**Estimated scope:** ~30 lines added

### P2: Rate Limit Awareness

**Change:** Add configurable delay between room creation calls

**Files to modify:**
- `crates/mxdx-matrix/src/rooms.rs`
  - Add optional delay parameter to `create_launcher_space()`
  - Or: add a `RateLimitConfig` to MatrixClient builder

**Estimated scope:** ~20 lines added

---

## 7. Features That Are Inherently Self-Hosted

These features cannot work on public servers and should be documented as such:

1. **Appservice registration** — requires server admin access
2. **Exclusive user namespace claiming** — requires appservice registration
3. **Virtual user creation** — requires appservice API
4. **Automated user registration** — requires registration token or admin API
5. **Federation configuration** — requires server config access

These are architectural constraints of the Matrix protocol, not mxdx bugs.

---

## 8. Test Matrix

| Test | Local Tuwunel | Public Server | Notes |
|:---|:---|:---|:---|
| Login | Pass | Pass (after P0 fix) | Needs `login_and_connect()` |
| E2EE setup | Pass | Pass | Standard matrix-sdk |
| Create encrypted room | Pass | Pass | Standard API |
| Send custom events | Pass | Pass | Standard API |
| Sync and receive events | Pass | Pass | May need more sync cycles |
| Create Space | Pass | Pass | Standard API |
| Link Space children | Pass | Pass | Standard API |
| Terminal DM creation | Pass | Pass | Standard API |
| Tombstone rooms | Pass | Pass | Standard API |
| Appservice registration | Pass | N/A | Self-hosted only |
| User registration (token) | Pass | N/A | Self-hosted only |
| Federation | Pass | N/A | Requires server config |
