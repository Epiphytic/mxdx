# Key Backup, Re-Encryption, and REST-Based Room Discovery

**Date:** 2026-04-07
**Status:** Approved
**Author:** brainstorming session

## Problem

The mxdx E2E suite has been failing intermittently with three classes of bug:

1. **Worker creates duplicate launcher rooms on every startup** instead of reusing
   existing ones. `find_launcher_space` enumerates `Client::joined_rooms()` and
   reads `Room::topic()` from the SDK's local cache, which is incomplete after
   session restore — older rooms with no recent timeline activity have no topic
   event in the cache, so the worker decides "no match found" and creates a new
   set, leading to "multiple rooms match launcher topic" warnings and UUID-lock
   collisions across restarts.
2. **Worker/client lose megolm keys across restarts.** A worker started with a
   restored session has only the megolm sessions cached locally; if a previous
   instance created a session that this instance never received via to-device,
   the worker silently drops events. Symptoms: client posts `session.start`,
   worker never logs receiving the task, t20 hangs.
3. **The launcher space room itself is unencrypted.** The MSC4362 fix encrypted
   the exec/logs rooms but `create_launcher_space` never enabled encryption on
   the space room, violating the project's "every Matrix event is E2EE" rule.

Plus a smaller issue: the diagnose tool's `encrypt_state_events` parser only
checks the canonical key, missing tuwunel's unstable
`io.element.msc4362.encrypt_state_events`.

## Goals

- All Matrix rooms (space, exec, logs) created by mxdx are end-to-end encrypted
  including state events.
- A worker or client started after a previous instance has all the megolm keys
  it needs to decrypt historical events, with no race against to-device delivery.
- Workers reuse their own existing launcher rooms instead of creating duplicates.
- Pre-existing unencrypted rooms self-heal: workers detect them on startup and
  tombstone-replace them.
- The diagnose tool can optionally decrypt state events using the same backup,
  for live troubleshooting of stuck E2E runs.

## Non-Goals

- Backup version rotation (out of scope; documented as future admin command).
- Per-room/per-event re-keying or selective backup.
- Migrating existing rooms' message history to the replacement room (we
  explicitly drop history when self-healing).
- Backup support in the WASM build (the npm launcher and web console will get
  this in a follow-up; matrix-sdk's backup APIs work fine in WASM but are
  out of scope here).

## Architecture decisions

| Decision | Choice | Reason |
|---|---|---|
| Recovery key storage | Auto-generate, store in chained keychain (OS primary, file fallback) under `mxdx:backup:{server}:{matrix_user}:{unix_user}` | Matches existing session-restore pattern; no friction; per-launcher isolation |
| Bootstrap on second host | Cross-signing / SSSS via `Encryption::recovery().recover_from_secret_storage()` | Already bootstrapped today; zero user intervention |
| Backup mode | One backup version per Matrix account, `download_room_keys()` unconditionally on every startup | Bulletproof; bandwidth is negligible; eliminates the "missing key on startup" race |
| Failure mode | Fatal on first run (creating); warn-and-continue on subsequent runs (downloading); set `backup.degraded` flag | Catches misconfig early; tolerates transient hiccups; visible in diagnose |
| Diagnose decryption | Opt-in `--decrypt` flag; uses temp-store matrix-sdk Client | Default stays safe (no key material in stdout); temp store avoids contention |
| Space-room encryption fix | Tombstone-and-replace any room missing encryption or `encrypt_state_events`, on every worker startup, per-room (not topology-wide) | Self-heals existing deployments; per-room avoids unnecessary churn |
| REST helper home | New `mxdx-matrix/src/rest.rs` shared module | Three call sites already exist (cleanup, diagnose, find_launcher_space); centralize to prevent re-fix drift |

## Components

### `mxdx-matrix/src/rest.rs` (new)

Direct Matrix client-server REST helpers, no SDK dependency. Used by
`find_launcher_space`, `cleanup`, `diagnose`, room re-encryption check, and
future "go direct to API" call sites.

```rust
pub struct RestClient { homeserver: String, access_token: String, http: reqwest::Client }
impl RestClient {
    pub fn new(homeserver: &str, access_token: &str) -> Self;
    pub async fn list_joined_rooms(&self) -> Result<Vec<OwnedRoomId>>;
    pub async fn list_invited_rooms(&self) -> Result<Vec<OwnedRoomId>>;
    pub async fn get_room_topic(&self, room: &RoomId) -> Result<Option<String>>;
    pub async fn get_room_name(&self, room: &RoomId) -> Result<Option<String>>;
    pub async fn get_room_encryption(&self, room: &RoomId) -> Result<Option<EncryptionState>>;
    pub async fn get_room_tombstone(&self, room: &RoomId) -> Result<Option<OwnedRoomId>>;
}

pub struct EncryptionState {
    pub algorithm: String,
    pub encrypt_state_events: bool,  // accepts canonical OR io.element.msc4362.* keys
}
```

All methods enforce a 10s timeout. Errors return `Result` rather than panic.
Mocked HTTP unit tests in `crates/mxdx-matrix/tests/rest_test.rs`.

### `mxdx-matrix/src/backup.rs` (new)

Wraps matrix-sdk's `Encryption::backups()` and `Encryption::recovery()`.

```rust
pub struct BackupState {
    pub enabled: bool,
    pub version: Option<String>,
    pub keys_downloaded: u64,
    pub degraded: bool,           // true if subsequent-run failed but we kept going
    pub error: Option<String>,
}

pub async fn ensure_backup(
    client: &matrix_sdk::Client,
    keychain: &dyn KeychainBackend,
    server: &str,
    matrix_user: &UserId,
    unix_user: &str,
    is_first_run: bool,
) -> Result<BackupState>;

pub async fn download_all_keys(client: &matrix_sdk::Client) -> Result<u64>;
```

Three flows:
- **Server has no backup version** → generate curve25519, create version, store
  private key in keychain. Returns `BackupState { enabled: true, ... }`.
- **Server has backup, recovery key in local keychain** → load from keychain,
  feed to `recovery().recover()`, then `download_all_keys()`.
- **Server has backup, recovery key NOT in local keychain** →
  `recovery().recover_from_secret_storage()`, cache result in keychain, then
  download.

Failure semantics:
- `is_first_run = true`: any failure → `Err`.
- `is_first_run = false`: failure → log WARN, return `BackupState { degraded: true, error: Some(...) }`.

### `mxdx-matrix/src/reencrypt.rs` (new)

```rust
pub async fn verify_or_replace_topology(
    client: &matrix_sdk::Client,
    rest: &RestClient,
    topology: LauncherTopology,
    authorized_users: &[OwnedUserId],
) -> Result<LauncherTopology>;
```

For each of (space, exec, logs):
1. `rest.get_room_encryption(room)` → check `algorithm == megolm` AND
   `encrypt_state_events == true`.
2. If broken: create encrypted replacement, copy essential state (name, topic,
   power_levels), invite authorized users, send `m.room.tombstone` on the old
   room pointing at the new, update parent space's `m.space.child` if applicable,
   leave-and-forget the old room.
3. Idempotent: a topology that's already correct is a no-op.

### `mxdx-matrix/src/rooms.rs` (modified)

- `create_launcher_space`: space room initial_state now includes
  `m.room.encryption` with megolm + `with_encrypted_state()`.
- `find_launcher_space`: rewritten to use `rest::list_joined_rooms` +
  `rest::get_room_topic` + `rest::get_room_tombstone`. Follows tombstones to
  the latest replacement room. No longer reads from SDK cache.

### Worker startup (`mxdx-worker/src/lib.rs::connect`)

After Matrix login + cross-signing bootstrap, before task loop:

```rust
let rest = RestClient::new(&server, &access_token);
let backup_state = backup::ensure_backup(&client, &keychain, &server,
    user_id, unix_user, is_first_run).await?;
backup::download_all_keys(&client).await?;
let topology = rest_find_or_create_launcher_space(&client, &rest, &launcher_id).await?;
let topology = reencrypt::verify_or_replace_topology(&client, &rest, topology, &authorized).await?;
// proceed to task loop
```

Same flow on `mxdx-client/src/daemon/mod.rs` daemon startup, except clients
don't run `verify_or_replace_topology` (clients never create or modify rooms).

### Diagnose tool (`mxdx-client/src/diagnose.rs`)

- Fix `encrypt_state_events` parser to accept both keys.
- New `--decrypt` flag: when set, spawn a temp-store matrix-sdk Client (sqlite
  store in `/tmp/mxdx-diagnose-{pid}/`), log in, run `backup::ensure_backup`
  with `is_first_run=false` (must already exist), `download_all_keys`, then
  decrypt every encrypted state event in the joined rooms via the SDK's room
  API. Cleanup temp dir on exit (drop guard). Default behavior unchanged.
- New top-level field in JSON output: `backup: { enabled, version, degraded, error, keys_downloaded }`.

## Data flow

```
Worker startup:
  login → cross-signing bootstrap → backup::ensure → download_all_keys
    → REST find_launcher_space (follows tombstones)
    → reencrypt::verify_or_replace_topology
    → task loop

Client daemon startup:
  login → cross-signing bootstrap → backup::ensure → download_all_keys
    → REST find_launcher_space (follows tombstones)
    → JSON-RPC loop

Diagnose --decrypt:
  spawn temp matrix-sdk Client in /tmp/mxdx-diagnose-{pid}
    → login → backup::ensure(is_first_run=false) → download_all_keys
    → for each joined room: enumerate state, decrypt encrypted state events
    → emit JSON, drop client (auto-cleanup of temp store)
```

## Failure modes

| Scenario | Behavior |
|---|---|
| First run, tuwunel doesn't support `/room_keys` | Worker exits with clear error |
| First run, cross-signing bootstrap failed earlier | Worker exits — must fix cross-signing first |
| Subsequent run, backup version mismatch on server | Warn, regenerate version, store new key |
| Subsequent run, recovery key in keychain rejected | Try secret-storage; if that fails, warn-and-continue with `degraded=true` |
| Subsequent run, network blip during `download_all_keys` | Warn, continue; partial keys are fine; next run finishes the rest |
| Re-encrypt: copy state to new room fails | Don't tombstone old room; return error; next startup retries |
| Re-encrypt: tombstone send fails after new room is created | Old room stays; new room is fully functional; next startup picks the latest by tombstone-following |
| Diagnose --decrypt: temp store creation fails | Emit error in JSON output; non-decrypt fields still populated |

## Testing

Mocked unit tests:
- `rest.rs`: 8-10 tests against a mockito HTTP server covering 200/404/timeout/malformed-JSON for each endpoint.
- `backup.rs`: 4-5 tests with mocked matrix-sdk traits where possible.

Integration tests (`#[ignore]`, beta credentials required):
- `t11_backup_round_trip`: start worker, post a session, kill worker, start a fresh worker on a different store dir, confirm it downloads the megolm session from backup and decrypts.
- `t12_unencrypted_room_self_heal`: manually create an unencrypted room with the launcher topic, start worker, confirm tombstone + encrypted replacement.
- `t13_diagnose_decrypts_state`: start worker, post a session, run `mxdx-client diagnose --decrypt`, confirm output contains decrypted `org.mxdx.session.completed` content.
- Existing `t10`–`t41` should continue to pass.

## Build sequence

Each step must compile clean before the next:

1. `mxdx-matrix/src/rest.rs` + unit tests
2. Refactor `find_launcher_space` and `cleanup` onto `rest::*`. `cargo check --workspace`.
3. `mxdx-matrix/src/backup.rs` (compile-only)
4. Wire `backup::ensure_backup` + `download_all_keys` into worker `connect()`. Manual smoke test.
5. `mxdx-matrix/src/reencrypt.rs`. Wire after backup setup.
6. Fix `create_launcher_space` to encrypt the space room.
7. Diagnose: parser fix + `--decrypt` flag.
8. Full baseline E2E: should pass with no duplicates, no missing keys, no decryption gaps.

## File summary

| File | Status | Purpose |
|---|---|---|
| `crates/mxdx-matrix/src/rest.rs` | new | REST helper module |
| `crates/mxdx-matrix/src/backup.rs` | new | Backup facade |
| `crates/mxdx-matrix/src/reencrypt.rs` | new | Room re-encryption flow |
| `crates/mxdx-matrix/src/rooms.rs` | modified | space encryption + REST-based find_launcher_space |
| `crates/mxdx-matrix/tests/rest_test.rs` | new | Unit tests for rest.rs |
| `crates/mxdx-worker/src/lib.rs` | modified | Wire backup + reencrypt into connect() |
| `crates/mxdx-client/src/daemon/mod.rs` | modified | Wire backup into daemon startup |
| `crates/mxdx-client/src/diagnose.rs` | modified | Parser fix + `--decrypt` flag |
| `crates/mxdx-client/src/cleanup.rs` | modified | Converge on `rest::*` helpers |
| `crates/mxdx-types/src/keychain.rs` | modified | New `backup_keychain_key()` helper |
| `crates/mxdx-worker/tests/e2e_profile.rs` | modified | New tests t11/t12/t13 |
