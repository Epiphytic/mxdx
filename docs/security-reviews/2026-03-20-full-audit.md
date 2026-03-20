# Security Review: mxdx Management Console (Full Codebase)

**Date:** 2026-03-20
**Reviewer:** jcode security-review skill
**Scope:** Full codebase audit - all Rust crates (`mxdx-types`, `mxdx-matrix`, `mxdx-policy`, `mxdx-secrets`, `mxdx-launcher`, `mxdx-web`, `mxdx-core-wasm`), TypeScript packages (`@mxdx/client`, `@mxdx/web-ui`), CI configuration, test helpers

## Summary

The mxdx codebase demonstrates strong security awareness overall. E2EE is enforced via matrix-sdk with recommended defaults, secrets use double encryption (age over Megolm), command execution is protected by an allowlist/denylist capability system with path traversal prevention, and the web layer has CSP headers with no CORS relaxation. The project's security-first culture is evident in the decompression bomb protection, replay detection, config permission checks, and tmux session name validation.

However, several findings range from **High** to **Low** severity. The most concerning issues are: (1) the `CommandEvent.env` field allows arbitrary environment variable injection bypassing the command allowlist, (2) plaintext passwords stored in the launcher config file, (3) the `TerminalSessionRequestEvent.command` field is not validated through the executor's allowlist, (4) the CryptoClient decryption uses `TrustRequirement.Untrusted`, and (5) the `new_with_test_key()` method on SecretStore is technically guarded by `#[cfg(test)]` but the pattern deserves vigilance. Several additional medium/low findings round out the report.

---

## Findings

### [HIGH] SEC-01: `CommandEvent.env` Allows Arbitrary Environment Variable Injection

**Location:** `crates/mxdx-types/src/events/command.rs:8`, `crates/mxdx-launcher/src/executor.rs:36-46`

**Description:** The `CommandEvent` type includes an `env: HashMap<String, String>` field containing arbitrary environment variables. The `execute_command()` function does not apply any of these environment variables (the current implementation ignores them), but the `TerminalSessionRequestEvent` in `terminal.rs:18` also has an `env: HashMap<String, String>` that IS passed through (see `client.ts:70` where `env: { TERM: "xterm-256color" }` is sent). More importantly, the executor's `validate_command()` function has no validation for the `env` field at all - it validates the command name, cwd, and arguments, but not environment variables.

When the full integration is wired up (the executor currently just spawns the command without env), a malicious Matrix event could set `LD_PRELOAD`, `PATH`, `HOME`, `LD_LIBRARY_PATH`, or other dangerous env vars to:
- Hijack library loading (`LD_PRELOAD=/tmp/evil.so`)
- Redirect PATH to execute different binaries (`PATH=/tmp/evil:$PATH`)
- Influence build tools (`CC`, `RUSTFLAGS`, `npm_config_*`)
- Override credential resolution (`AWS_ACCESS_KEY_ID`, `GIT_ASKPASS`)

**Impact:** Remote code execution via environment variable manipulation. Even with command allowlisting, a crafted `PATH` or `LD_PRELOAD` can cause the allowlisted command to execute attacker-controlled code.

**Recommendation:** 
1. Add an `allowed_env_keys` allowlist to `CapabilitiesConfig`
2. Strip all env vars not on the allowlist before execution
3. Unconditionally block dangerous vars: `LD_PRELOAD`, `LD_LIBRARY_PATH`, `PATH`, `DYLD_*`, `BASH_ENV`, `ENV`, `CDPATH`
4. Consider env sanitization in `validate_command()` alongside args validation

---

### [HIGH] SEC-02: Plaintext Passwords in Launcher Config File

**Location:** `crates/mxdx-launcher/src/config.rs:23-27`

**Description:** `HomeserverConfig` stores `username` and `password` as plaintext strings deserialized from the TOML config file:
```rust
pub struct HomeserverConfig {
    pub url: String,
    pub username: String,
    pub password: String,
}
```

While `validate_config_permissions()` warns if the file is readable by group/others, it only logs a warning and does not reject insecure permissions. The passwords sit in plaintext on disk.

**Impact:** Credential exposure if the config file is read by another process, included in a backup, logged, or accessed via a path traversal vulnerability. The project's own AGENTS.md states "All sensitive info should be stored encrypted at rest" and "Use OS keystores/keychains for sensitive client/server info when available."

**Recommendation:**
1. Integrate with OS keychain (e.g., `keyring` crate) for credential storage
2. Support environment variable references in config (e.g., `password = "${MXDX_HS1_PASSWORD}"`)
3. Make `validate_config_permissions()` return `Err` (not just warn) when permissions are insecure
4. Consider using age-encrypted config sections decrypted at runtime

---

### [HIGH] SEC-03: Terminal Session Command Not Validated Through Executor Allowlist

**Location:** `crates/mxdx-types/src/events/terminal.rs:17-22`, `crates/mxdx-launcher/src/terminal/session.rs:14-28`

**Description:** `TerminalSessionRequestEvent` contains a `command: String` field that is passed directly to `TmuxSession::create()`, which spawns it as the shell command for the tmux session:
```rust
// session.rs
pub async fn create(session_id: &str, command: &str, ...) -> Result<Self, anyhow::Error> {
    let tmux = TmuxSession::create(session_id, command, cols, rows).await?;
// tmux.rs
Command::new("tmux").args(["new-session", "-d", "-s", name, ..., command])
```

This command string is NOT validated through the `validate_command()` function's allowlist system. Any Matrix user who can send a `TerminalSessionRequestEvent` to the launcher's room can execute arbitrary commands.

**Impact:** Remote code execution. An attacker who gains access to the Matrix room (even with a compromised account that shouldn't have shell access) can spawn arbitrary processes.

**Recommendation:**
1. Route terminal session commands through `validate_command()` 
2. Consider a separate `allowed_terminal_commands` allowlist (e.g., only `/bin/bash`, `/bin/sh`)
3. Validate the `command` field before creating the tmux session
4. Enforce the `max_sessions` limit as a DoS mitigation

---

### [MEDIUM] SEC-04: CryptoClient Uses TrustRequirement::Untrusted for Decryption

**Location:** `client/mxdx-client/src/crypto.ts:79`

**Description:** The `CryptoClient.decrypt()` method uses `TrustRequirement.Untrusted`:
```typescript
const settings = new DecryptionSettings(TrustRequirement.Untrusted);
```

This means the client will decrypt messages from devices that haven't been cross-signed or verified. In a fleet management system where commands control servers, this weakens the chain of trust.

**Impact:** A compromised or impersonating device could send commands that the client would decrypt and present as legitimate. This is a meaningful degradation of E2EE trust, especially for a security service.

**Recommendation:**
1. Use `TrustRequirement.CrossSignedOrLegacy` or `TrustRequirement.CrossSigned` for command events
2. At minimum, allow the trust requirement to be configurable per-event-type
3. Terminal data (visual output) could reasonably use `Untrusted`, but command execution events should require verification

---

### [MEDIUM] SEC-05: Unencrypted Status and Logs Rooms in Launcher Space

**Location:** `crates/mxdx-matrix/src/rooms.rs:67-77`

**Description:** The launcher space topology creates three child rooms: `exec` (encrypted), `status` (unencrypted), and `logs` (unencrypted):
```rust
let status_room_id = self.create_named_unencrypted_room(...).await?;
let logs_room_id = self.create_named_unencrypted_room(...).await?;
```

The WASM client (`mxdx-core-wasm`) creates both exec and logs rooms with E2EE, but the native Rust client creates status and logs without encryption.

**Impact:** System telemetry (hostname, OS, CPU, memory, disk, network stats) and log data are transmitted in plaintext over Matrix. This leaks operational intelligence about the fleet: server specs, load patterns, hostnames, and potentially sensitive log content. The project's own rules state: "All communications must be private, auditable, and end to end encrypted."

**Recommendation:**
1. Enable E2EE for all launcher rooms, including status and logs
2. If there's a deliberate reason to keep some rooms unencrypted (e.g., monitoring aggregation), document it in an ADR and require explicit opt-in

---

### [MEDIUM] SEC-06: Recovery State File Contains Sensitive Session Metadata in Plaintext

**Location:** `crates/mxdx-launcher/src/terminal/recovery.rs:27-39`

**Description:** `RecoveryState` is persisted as a JSON file containing `SessionState` entries with `dm_room_id`, `command`, and `session_id`. This file is written to the launcher's `data_dir` without encryption or permission restrictions.

**Impact:** The recovery state leaks which Matrix rooms contain terminal sessions, what commands were run, and session identifiers. An attacker with filesystem read access could use this to target specific rooms for eavesdropping or injection.

**Recommendation:**
1. Encrypt the recovery state file at rest (use the launcher's age identity)
2. Set restrictive file permissions (0600) when writing
3. Consider whether the command field needs to be persisted

---

### [MEDIUM] SEC-07: `env` Command Blocked but Partial Shell Injection Vectors Remain

**Location:** `crates/mxdx-launcher/src/executor.rs:176-181`

**Description:** The `env` command is correctly blocked, and `git -c`, `git submodule foreach`, and `docker compose -f` are filtered. However, the argument validation is an incomplete denylist approach. Several bypass vectors exist:
- `bash -c "arbitrary command"` (if bash is on the allowlist)
- `sh -c "..."` or `python -c "..."`
- `git clone --config core.sshCommand="evil"` (only `-c` and `--config` are blocked, not `--config` embedded in other args)
- `npm run` with a malicious `package.json` already in the cwd
- Commands with semicolons or pipe characters in arguments (though tmux session input bypasses this entirely)

**Impact:** If any shell interpreter is on the command allowlist, the allowlist is effectively meaningless.

**Recommendation:**
1. Block shell interpreters (`bash`, `sh`, `dash`, `zsh`, `fish`, `python`, `python3`, `perl`, `ruby`, `node`) from the allowlist, or
2. If shells must be allowed, add argument validation for `-c` and `--eval` flags
3. Consider switching from denylist argument validation to a positive-match approach
4. Document the security model: allowlist is for the primary command, not a sandbox

---

### [MEDIUM] SEC-08: No Authentication on Web Dashboard / SSE Endpoints

**Location:** `crates/mxdx-web/src/routes/dashboard.rs`, `crates/mxdx-web/src/routes/sse.rs`

**Description:** The web dashboard at `/dashboard` and SSE endpoint at `/sse/launchers` have no authentication or authorization middleware. Anyone who can reach the web server can view all launcher telemetry data (CPU, memory, hostname, status).

The server binds to `127.0.0.1:3000` which limits exposure to localhost, but:
- Port forwarding, reverse proxies, or container networking could expose it
- Any process on the same host can access it
- If deployed behind a load balancer, all internal traffic can reach it

**Impact:** Information disclosure of fleet topology and resource utilization to unauthorized viewers.

**Recommendation:**
1. Add authentication middleware (Matrix access token validation or session cookies)
2. Rate-limit the SSE endpoint to prevent resource exhaustion
3. Document that the web UI MUST NOT be exposed to untrusted networks without auth

---

### [MEDIUM] SEC-09: Appservice Tokens (`as_token`, `hs_token`) in Config Without Rotation

**Location:** `crates/mxdx-policy/src/config.rs:9-13`, `crates/mxdx-policy/src/appservice.rs:75-76`

**Description:** The `PolicyConfig` stores `as_token` and `hs_token` as static strings. These are the shared secrets between the homeserver and appservice. There is no mechanism for token rotation, and compromise of either token would allow impersonation.

**Impact:** A compromised `as_token` allows an attacker to act as the appservice (impersonating managed users). A compromised `hs_token` allows an attacker to impersonate the homeserver to the appservice.

**Recommendation:**
1. Load tokens from environment variables or a secrets manager rather than config files
2. Implement token rotation support
3. Apply `validate_config_permissions()` to the policy config file

---

### [LOW] SEC-10: `Denylist` Capability Mode Has No Validation

**Location:** `crates/mxdx-launcher/src/config.rs:45-51`, `crates/mxdx-launcher/src/executor.rs:194`

**Description:** The `CapabilityMode` enum has a `Denylist` variant, but `validate_command()` only checks the `Allowlist` mode:
```rust
if config.mode == CapabilityMode::Allowlist && !config.allowed_commands.contains(&cmd.to_string()) {
```

When `mode == Denylist`, the allowlist check is skipped entirely, meaning ALL commands are permitted. There is no denylist enforcement implemented. Since `Allowlist` is the default, this doesn't create an immediate vulnerability, but if a user configures `mode = "denylist"` expecting protection, they get none.

**Impact:** Misconfiguration could lead to unrestricted command execution.

**Recommendation:**
1. Implement actual denylist enforcement, or
2. Remove the `Denylist` variant and document that only allowlist mode is supported
3. Add a test that verifies denylist mode blocks dangerous commands

---

### [LOW] SEC-11: WASM IndexedDB Crypto Store Uses No Passphrase

**Location:** `crates/mxdx-core-wasm/src/lib.rs:105,166,274`

**Description:** All calls to `.indexeddb_store(&store_name, None)` pass `None` as the passphrase parameter. This means the Matrix crypto store (containing Megolm session keys, device keys, and cross-signing keys) is stored in IndexedDB without encryption.

**Impact:** Any JavaScript running in the same origin can read the crypto store and extract session keys. While this is the common pattern for browser Matrix clients, for a security-critical fleet management tool, it's worth noting.

**Recommendation:**
1. Consider using a passphrase derived from user credentials
2. Document this as a known limitation of browser-based E2EE
3. Recommend that sensitive deployments use the native launcher binary rather than the WASM client

---

### [LOW] SEC-12: SQLite Crypto Store Uses No Passphrase

**Location:** `crates/mxdx-matrix/src/client.rs:42,119`

**Description:** The native `MatrixClient` uses `.sqlite_store(store_dir.path(), None)` with no encryption passphrase. The sqlite store contains E2EE key material.

**Impact:** Any process with filesystem access to the temp directory can read E2EE keys. The store is in a `TempDir` which provides some protection, but temp directories are often world-readable.

**Recommendation:**
1. Use a passphrase for the sqlite store, derived from the launcher's age identity
2. Ensure the temp directory has restrictive permissions
3. Consider using the launcher's data_dir instead of a temp directory for persistence

---

### [LOW] SEC-13: XSS Risk in Dashboard HTML Rendering

**Location:** `crates/mxdx-web/src/routes/dashboard.rs:38-51`, `crates/mxdx-web/src/routes/sse.rs:28-48`

**Description:** The `render_launcher_cards()` and `render_launcher_oob_fragment()` functions use `format!()` to interpolate `LauncherInfo` fields directly into HTML without HTML-escaping:
```rust
format!(r#"<div class="launcher-card" data-id="{id}">
  <h2>{id}</h2>
  ...
  <p>Host: {hostname}</p>
```

If `launcher_id` or `hostname` contain HTML characters (e.g., `<script>alert(1)</script>`), they will be rendered as HTML.

**Impact:** Stored XSS if an attacker controls a launcher's identity event. The CSP mitigates script execution (`script-src 'self'`), but HTML injection can still deface the UI, inject forms, or leak data via CSS.

**Recommendation:**
1. HTML-escape all interpolated values (use a crate like `askama` with auto-escaping, or `html_escape`)
2. Validate `launcher_id` and `hostname` at the input boundary

---

### [LOW] SEC-14: `validate_config_permissions()` Only Warns, Doesn't Enforce

**Location:** `crates/mxdx-launcher/src/config.rs:84-96`

**Description:** When the config file has insecure permissions (readable by group/others), the function logs a warning but returns `Ok(())`. For a file containing plaintext passwords (see SEC-02), this should be a hard failure.

**Recommendation:** Return `Err` when permissions are insecure, or at least when the config contains passwords.

---

### [LOW] SEC-15: TerminalSocket Client-Side Has No Decompression Size Limit

**Location:** `client/mxdx-client/src/terminal.ts:69-94`

**Description:** The `decompress()` function in the TypeScript client decompresses data without any size limit, unlike the Rust side which has `decode_decompress_bounded()` with a `max_bytes` parameter. A malicious server could send a compressed payload that decompresses to an enormous size, causing OOM in the browser.

**Recommendation:** Add a decompression size limit matching the Rust side's 1MB limit.

---

### [INFO] SEC-16: `new_with_test_key()` Properly Gated by `#[cfg(test)]`

**Location:** `crates/mxdx-secrets/src/store.rs:31-33`

**Description:** `SecretStore::new_with_test_key()` generates a random key for testing. It is correctly gated behind `#[cfg(test)]` so it cannot be compiled into production builds. This was previously flagged as a concern (mxdx-tky) and has been properly addressed.

**Status:** Pass - no action needed.

---

### [INFO] SEC-17: No `cargo audit` in CI Pipeline

**Location:** `.github/workflows/ci.yml`

**Description:** The CI pipeline does not run `cargo audit` to check for known CVEs in dependencies. Given the security-critical nature of this project, automated vulnerability scanning should be mandatory.

**Recommendation:** Add a `cargo audit` step to CI.

---

### [INFO] SEC-18: Registration Token Hardcoded in Test Helpers

**Location:** `tests/helpers/src/tuwunel.rs:9`

**Description:** The test registration token `"mxdx-test-token"` is hardcoded. This is acceptable for test infrastructure but should never leak into production configurations.

**Status:** Pass - only used in test context.

---

## Passed Checks

- ✅ No hardcoded secrets in production code
- ✅ No secrets in log output (tracing calls verified)
- ✅ E2EE enabled with `with_recommended_defaults()` for room encryption
- ✅ Double encryption (age over Megolm) for secrets protocol
- ✅ Decompression bomb protection in Rust (`decode_decompress_bounded`)
- ✅ Replay detection via LRU cache with TTL in PolicyEngine
- ✅ Tmux session name validation (alphanumeric + underscore + hyphen only)
- ✅ Path traversal prevention via `normalize_path()` with `..` handling
- ✅ CSP headers on web responses
- ✅ No CORS relaxation on web endpoints
- ✅ `test-credentials.toml` properly gitignored
- ✅ No `unsafe` code in the codebase
- ✅ No `panic!()` macro in production code
- ✅ No `eval()` or dynamic code execution in TypeScript
- ✅ No console.log statements leaking data in client code
- ✅ `Drop` implementation on TmuxSession for cleanup
- ✅ Regex escaping in appservice namespace patterns
- ✅ Service worker with integrity verification
- ✅ History visibility set to `Joined` for terminal DM rooms
- ✅ Room creation timeout to prevent indefinite hangs on rate limiting
- ✅ Non-empty string validation on launcher_id config
- ✅ Convention of `format!()` for structured tracing (no secret interpolation)
- ✅ Using `age` crate (established crypto) rather than custom encryption

---

## Risk Matrix

| ID | Severity | Component | Exploitability | Fix Effort |
|:---|:---|:---|:---|:---|
| SEC-01 | HIGH | launcher/executor | Remote (Matrix event) | Medium |
| SEC-02 | HIGH | launcher/config | Local filesystem | Medium |
| SEC-03 | HIGH | launcher/terminal | Remote (Matrix event) | Low |
| SEC-04 | MEDIUM | client/crypto | Remote (MITM device) | Low |
| SEC-05 | MEDIUM | matrix/rooms | Network observer | Low |
| SEC-06 | MEDIUM | launcher/recovery | Local filesystem | Low |
| SEC-07 | MEDIUM | launcher/executor | Remote (Matrix event) | Medium |
| SEC-08 | MEDIUM | web/routes | Network (localhost) | Medium |
| SEC-09 | MEDIUM | policy/config | Local filesystem | Medium |
| SEC-10 | LOW | launcher/executor | Misconfiguration | Low |
| SEC-11 | LOW | core-wasm | Same-origin JS | Low |
| SEC-12 | LOW | matrix/client | Local filesystem | Low |
| SEC-13 | LOW | web/dashboard | Remote (Matrix event) | Low |
| SEC-14 | LOW | launcher/config | Misconfiguration | Low |
| SEC-15 | LOW | client/terminal | Remote (Matrix event) | Low |

---

## Recommendations (Prioritized)

### Block Merge / Critical Path
1. **SEC-03**: Validate terminal session commands through the allowlist before spawning tmux
2. **SEC-01**: Add env var allowlist/denylist to the executor's `validate_command()`
3. **SEC-02**: Move credentials out of plaintext config files

### Should Fix Before Production
4. **SEC-05**: Encrypt all launcher rooms (status, logs) to comply with the project's own E2EE mandate
5. **SEC-04**: Raise `TrustRequirement` for command decryption
6. **SEC-08**: Add authentication to web dashboard endpoints
7. **SEC-07**: Expand argument injection prevention or document limitations

### Defense in Depth
8. **SEC-09**: Load appservice tokens from env vars / secrets manager
9. **SEC-06**: Encrypt recovery state at rest
10. **SEC-13**: HTML-escape all dynamic values in dashboard rendering
11. **SEC-10**: Implement or remove denylist mode
12. **SEC-14**: Make config permission check a hard failure
13. **SEC-15**: Add decompression size limit in TypeScript client
14. **SEC-11/12**: Add passphrases to crypto stores
15. **SEC-17**: Add `cargo audit` to CI

---

*This review is read-only. No files were modified.*
