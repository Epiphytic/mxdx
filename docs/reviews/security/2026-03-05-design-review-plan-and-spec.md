# Security Review: Implementation Plan & Management Console Specification

**Date:** 2026-03-05
**Scope:** `docs/plans/2026-03-04-mxdx-management-console.md` (implementation plan), `docs/mxdx-management-console.md` (specification), `docs/mxdx-architecture.md` (core architecture)
**Review type:** Design-phase security review (pre-implementation)
**Scanner output:** N/A (design review, no code to scan yet beyond scaffolding)

---

## Trust Boundary Map

```
                            UNTRUSTED
                               |
   Browser (user input,   -----+-----  Federated homeservers
   xterm.js keystrokes)        |        (external Matrix traffic)
                               |
                          TRUST BOUNDARY 1: Matrix E2EE
                               |
                     Tuwunel homeserver(s)
                               |
                          TRUST BOUNDARY 2: Appservice intercept
                               |
                     Policy Agent (fail-closed)
                               |
                          TRUST BOUNDARY 3: Capability enforcement
                               |
                     Launcher process
                               |
                          TRUST BOUNDARY 4: PTY/tmux bridge
                               |
                     Host operating system
                               |
                          TRUST BOUNDARY 5: Secret delivery
                               |
                     Secrets Coordinator (HSM-backed)
```

**Input entry points:**
1. Browser xterm.js keystrokes -> Matrix events -> Launcher PTY stdin
2. Matrix events from any federated user -> Policy Agent -> Launcher
3. Launcher TOML config file (local filesystem)
4. Admin user commands via Matrix DM
5. Web app HTMX requests (stateless, but serves assets)

**Sensitive data exit points:**
1. Secret values in `org.mxdx.secret_response` events (E2EE DMs)
2. Terminal output (may contain secrets leaked by commands)
3. Telemetry data (host info, network topology, service inventory)
4. Audit log events (contain metadata about secret access)

---

## Findings (by severity)

### [CRITICAL-DESIGN] Plan Task 0.3 / Spec SS6: Hardcoded Ports in Test Infrastructure

- **Location:** Plan lines 452-466 (Task 0.3), lines 598-606 (Task 0.4), and throughout all test code examples
- **Attack vector:** Tests use hardcoded ports (6167, 6168, 6200, 6201, 6300, 6301, etc.). In CI environments or on developer machines, these ports may already be in use or could be claimed by an attacker (port squatting) to intercept test Matrix traffic. An attacker with local access could bind to expected test ports before tests run, capturing registration credentials and access tokens sent over plaintext HTTP.
- **Impact:** Test credentials and access tokens intercepted. In CI, a malicious process could register as the "homeserver" and capture all test traffic.
- **Remediation:** Use port 0 (OS-assigned) for all test instances. `TuwunelInstance::start()` should accept `0` and report the actual bound port. All test code examples in the plan must be updated. This is critical because the plan prescribes the pattern that all future tests will follow.
- **Evidence:** Plan line 452: `TuwunelInstance::start(6167)`, line 453: port 6168, line 595: ports 6200/6201, etc. Every task through Phase 15 uses this pattern.

### [CRITICAL-DESIGN] Plan Task 2.1 / Spec SS5.6: Secret Values in Plaintext Events

- **Location:** Architecture doc SS5.6, Plan lines 1342-1353 (Task 5.3)
- **Attack vector:** `org.mxdx.secret_response` events contain the secret value in the `value` field as plaintext within the E2EE envelope. If a device's Megolm session keys are compromised (key backup attack, compromised device, or future quantum decryption), all historical secrets delivered via these DMs become readable.
- **Impact:** Full retroactive exposure of all secrets ever delivered through the system.
- **Remediation:** Consider double-encrypting the `value` field using a per-request ephemeral key exchange (e.g., X25519 DH between coordinator and requester), so that even with Megolm key compromise, the secret values require a separate key to decrypt. At minimum, document this as an accepted risk with a timeline for mitigation.
- **Evidence:** Architecture doc line 206: `"value": "ghs_xxxxxxxxxxxx"` in secret_response schema.

### [HIGH-DESIGN] Plan Task 4.2 / Spec SS8: Command Injection via `cwd` Field

- **Location:** Architecture doc line 137 (`"cwd": "/workspace/girt"`), Plan line 1186-1198 (Task 4.2)
- **Attack vector:** The `org.mxdx.command` event includes a `cwd` field. If the launcher uses this directly in `std::process::Command::current_dir(cwd)` without validation, an attacker with command-send privileges could set `cwd` to sensitive directories (e.g., `/etc`, `/root/.ssh`, the coordinator's data directory) to exfiltrate data or escalate. Combined with an allowed command like `git` (which reads `.git/config` in cwd), this enables reading arbitrary file content.
- **Impact:** Directory traversal leading to information disclosure or privilege escalation.
- **Remediation:** The launcher MUST validate `cwd` against an allowlist of permitted working directories (or a base path prefix). The plan should add a `cwd` validation test alongside the command allowlist tests in Task 4.2: `cwd = "/etc" -> ResultEvent { status: "error" }`.
- **Evidence:** Architecture doc line 137 shows `cwd` as a free-form string. Plan Task 4.2 tests only validate command allowlist, not `cwd`.

### [HIGH-DESIGN] Plan Task 4.2: Command Allowlist Bypass via Arguments

- **Location:** Plan lines 1186-1203 (Task 4.2), Spec SS6 lines 458-469
- **Attack vector:** The capability config allows commands like `"cargo"`, `"git"`, `"docker compose *"`. But the plan's executor test only validates the command binary name. Allowed commands like `git` can execute arbitrary code: `git -c core.pager='malicious_command' log`, `git submodule foreach 'evil'`. Similarly, `cargo` can run arbitrary build scripts. `docker compose *` with a crafted `docker-compose.yml` (via cwd manipulation) can mount host filesystem.
- **Impact:** Arbitrary code execution on the host despite command allowlist.
- **Remediation:**
  1. Document that the allowlist is a defense-in-depth measure, not a sandbox.
  2. Add argument validation rules (deny patterns like `-c`, `--config`, `submodule foreach` for git).
  3. At minimum, add tests in Task 4.2 for known bypass vectors.
  4. Consider running commands in a namespace/container for actual isolation.
- **Evidence:** Spec line 464: `"docker compose *"` allows arbitrary compose files. Architecture doc line 49: `"allowed_commands": ["cargo", "git", "npm", "node"]` — all of these support executing arbitrary code via arguments.

### [HIGH-DESIGN] Plan Task 7.4: Terminal Session Room History Visibility Race Condition

- **Location:** Plan lines 1601-1604 (Task 7.4), Spec SS8 lines 559-569
- **Attack vector:** When a terminal session room is created (step 4 in SS8), there is a window between room creation and setting `history_visibility = joined`. If an attacker joins during this window (or if the default `history_visibility` is `shared`), they could read terminal output sent before the state event was applied.
- **Impact:** Leakage of terminal output (which may contain secrets, credentials, or sensitive data) to unauthorized users.
- **Remediation:** Set `history_visibility = joined` as an initial state event in the room creation request (using `initial_state` parameter in `createRoom`), not as a separate state event after creation. Add a test that verifies the `initial_state` includes `history_visibility` before any messages are sent.
- **Evidence:** Plan line 1602: "History visibility must be set to `joined` on creation" but the spec SS8 line 566 lists it as step 6 (after room creation, invite, and other steps), leaving a gap.

### [HIGH-DESIGN] Plan Task 7.1-7.4: PTY Data as Attack Surface

- **Location:** Plan lines 1457-1615 (Phase 7), Spec SS8 lines 549-557
- **Attack vector:** The PTY bridge is described as a "dumb pipe" (Spec line 631), but incoming `org.mxdx.terminal.data` events are decoded from base64 and potentially decompressed (zlib) before being written to the PTY. A malicious event could contain:
  1. A zlib bomb (small compressed payload that decompresses to gigabytes)
  2. Malformed base64 causing unbounded memory allocation
  3. Terminal escape sequences that exploit vulnerabilities in tmux or the underlying terminal emulator
- **Impact:** Denial of service (memory exhaustion), or in rare cases, terminal emulator exploits.
- **Remediation:**
  1. Enforce `max_payload_bytes = 65536` (from Spec SS6 line 497) BEFORE decompression, and also enforce a max decompressed size (e.g., 256KB).
  2. Validate base64 length before decoding.
  3. Add tests in Task 7.2 for oversized payloads and zlib bomb detection.
- **Evidence:** Spec line 497: `max_payload_bytes = 65536` but the plan's compression tests (Task 7.2, lines 1499-1524) only test correct compression/decompression, not malicious inputs.

### [HIGH-DESIGN] Spec SS6: TOML Config Parsing Without Validation

- **Location:** Spec SS6 lines 438-498, Plan Task 4.1 line 1155
- **Attack vector:** The launcher config is a TOML file with fields like `allowed_commands`, `denied_commands`, `max_sessions`, etc. If the config file is writable by a lower-privileged user (or a compromised process), they could modify `allowed_commands` to include dangerous commands, increase `max_sessions` to enable DoS, or disable rate limits.
- **Impact:** Privilege escalation via config modification.
- **Remediation:**
  1. On startup, verify config file permissions (owner-readable only, e.g., mode 0600).
  2. Log a warning if config is world-readable or group-writable.
  3. Add a config validation step in Task 4.1 that checks for dangerous configurations (e.g., `mode = "unrestricted"` should require explicit confirmation).
- **Evidence:** Spec line 458: `mode = "allowlist"` vs `"unrestricted"` — no validation described for the `unrestricted` mode.

### [MEDIUM-DESIGN] Plan Task 0.3: Test Helper Uses Plaintext HTTP

- **Location:** Plan lines 496-541 (Task 0.3)
- **Attack vector:** `TuwunelInstance::start()` configures the test homeserver to listen on `http://` (not HTTPS). Test clients connect over plaintext HTTP, transmitting registration credentials and access tokens in the clear. While this is a test environment, if tests run on shared CI infrastructure, other processes could sniff test traffic.
- **Impact:** Test credentials exposed on shared networks/CI runners.
- **Remediation:** Document this as a known test-environment risk. Consider using localhost-only binding (which the plan does: `address = "127.0.0.1"` on line 509) and note that this is sufficient for single-host CI. For multi-host test setups, add TLS with self-signed certs.
- **Evidence:** Plan line 531: `http://localhost:{}/_matrix/client/versions` — plaintext HTTP throughout.

### [MEDIUM-DESIGN] Spec SS4.1: Sequence Number Overflow / Wraparound

- **Location:** Spec SS4.1 line 178 (`"seq": 12345`), Plan Task 7.3 (ring buffer)
- **Attack vector:** The `seq` field in `org.mxdx.terminal.data` is described as an incrementing integer. For long-running sessions, this could overflow. If the client or launcher doesn't handle wraparound, it could cause:
  1. Infinite retransmit loops (client sees `seq` go backward, requests retransmit)
  2. Ring buffer corruption
  3. Denial of service
- **Impact:** Terminal session becomes unusable; potential infinite event loop.
- **Remediation:** Define `seq` as `u64` (sufficient for 584 billion years at 1 event/ms). Add a test for the maximum sequence number behavior. Explicitly document that `seq` MUST NOT wrap.
- **Evidence:** Neither the spec nor the plan defines the integer type or wraparound behavior for `seq`.

### [MEDIUM-DESIGN] Plan Task 3.2: Replay Protection Mechanism Unspecified

- **Location:** Plan lines 1131-1133 (Task 3.2)
- **Attack vector:** The plan requires a test for replay protection (`replayed_event_does_not_cause_double_execution`) but doesn't specify the mechanism. Without a clear design, the implementation might use event_id deduplication only, which could be bypassed if an attacker can generate a new event_id with identical content.
- **Impact:** Command double-execution leading to data corruption or repeated operations.
- **Remediation:** Specify the replay protection mechanism: use the `uuid` field in command events as an idempotency key, stored in a bounded LRU cache with TTL. The plan should include the design in Task 3.2, not just the test.
- **Evidence:** Plan line 1131: test name exists but no implementation guidance. Architecture doc doesn't specify replay protection mechanism.

### [MEDIUM-DESIGN] Spec SS6: Failover Identity Event Spoofing

- **Location:** Spec SS6 lines 420-434, Plan Task 9.2
- **Attack vector:** During failover, the launcher updates the `org.mxdx.launcher.identity` state event to change the `primary` field. If an attacker compromises a secondary identity, they could update this state event to claim themselves as primary, potentially redirecting all commands to a compromised identity.
- **Impact:** Man-in-the-middle on all launcher commands during failover.
- **Remediation:** All launcher identities should be at power level 100 (the plan specifies this), but the clients should also verify that identity updates come from a known launcher account. Add a test in Task 9.2 that verifies a non-launcher user cannot update the identity state event.
- **Evidence:** Spec line 432: "Updates `org.mxdx.launcher.identity` state event" — but no verification that the updater is actually a launcher identity.

### [MEDIUM-DESIGN] Plan Task 5.2: age Encryption Key Management in Tests

- **Location:** Plan lines 1289-1314 (Task 5.2)
- **Attack vector:** The test uses `SecretStore::new_with_test_key()` which implies a hardcoded test key. If this key or the test key generation pattern leaks into production code (e.g., via a default fallback), the entire secret store could be decrypted.
- **Impact:** Full secret store compromise if test key is used in production.
- **Remediation:** Ensure `new_with_test_key()` is gated behind `#[cfg(test)]` and cannot be compiled into release builds. Add a CI check that `new_with_test_key` does not appear in non-test code.
- **Evidence:** Plan line 1292: `SecretStore::new_with_test_key()` — no indication of cfg-gating.

### [MEDIUM-DESIGN] Spec SS4.6: Telemetry Leaks Infrastructure Details

- **Location:** Spec SS4.6 lines 289-366
- **Attack vector:** The `org.mxdx.host_telemetry` state event contains detailed infrastructure information: IP addresses, network routes, service inventory, device names, CPU model, OS version, GPU model. If any room member is compromised, this provides a detailed reconnaissance map.
- **Impact:** Full infrastructure reconnaissance for an attacker who compromises any room member with access to the status room.
- **Remediation:**
  1. Consider making telemetry detail levels configurable (full vs. summary).
  2. Ensure status rooms have strict membership (launcher + admins only).
  3. Consider encrypting telemetry state events (currently state events are not E2EE in Matrix).
- **Evidence:** Spec line 318-336: Full network interface details including IPs, routes, and per-interface stats exposed as state events.

### [MEDIUM-DESIGN] Spec SS10: State Events Are Not E2EE

- **Location:** Spec SS4.5-4.6 (launcher identity and telemetry are state events)
- **Attack vector:** Matrix state events are NOT encrypted by Megolm — they are stored in plaintext on the homeserver. This means `org.mxdx.launcher.identity` (with all account MXIDs) and `org.mxdx.host_telemetry` (with full infrastructure details) are readable by the homeserver operator and anyone with database access.
- **Impact:** Infrastructure details and launcher identity information exposed to homeserver operator/database.
- **Remediation:** Document this as an accepted risk (state events must be unencrypted for room state to function). Consider moving sensitive telemetry fields to timeline events (which are E2EE) and keeping only minimal metadata in state events. At minimum, note this in a threat model section.
- **Evidence:** Spec line 625: "E2EE everywhere" but state events are an exception the spec doesn't acknowledge. Spec lines 267-287 and 289-366 define sensitive data as state events.

### [LOW-DESIGN] Plan Task 0.1: Workspace Dependencies Pin to Major Version Only

- **Location:** Plan lines 234-244 (Cargo.toml)
- **Attack vector:** Dependencies like `matrix-sdk = "0.8"` will resolve to the latest 0.8.x. A compromised or buggy patch release could be pulled in automatically.
- **Impact:** Supply chain risk from unaudited dependency updates.
- **Remediation:** Use `Cargo.lock` (already standard for binary crates) and pin exact versions in CI. Consider using `cargo-vet` or `cargo-crev` for dependency auditing. The plan should include `Cargo.lock` in version control.
- **Evidence:** Plan line 242: `matrix-sdk = { version = "0.8", ... }` — semver range, not exact pin.

### [LOW-DESIGN] Plan Task 10.4: Service Worker Cache Poisoning

- **Location:** Plan lines 1849-1873 (Task 10.4)
- **Attack vector:** The service worker caches "HTML shell, JS bundles, WASM crypto module" with a cache-first strategy for static assets. If an attacker can poison the CDN or MITM the first load, the poisoned assets would persist in the service worker cache.
- **Impact:** Persistent XSS or credential theft via poisoned cached assets.
- **Remediation:** Use Subresource Integrity (SRI) hashes for all cached assets. The service worker should verify integrity before caching. Add CSP headers that prevent inline scripts.
- **Evidence:** Plan line 1870: "Strategy: cache-first for static assets" — no mention of integrity verification.

### [LOW-DESIGN] Plan: Missing CORS Configuration for Web App

- **Location:** Plan Phase 10 (Task 10.1-10.4)
- **Attack vector:** The web app serves HTMX partials and static assets. Without explicit CORS headers, a malicious site could potentially load HTMX partials or trigger HTMX requests cross-origin.
- **Impact:** CSRF on HTMX endpoints if they have side effects; information leakage via cross-origin requests.
- **Remediation:** Add explicit CORS configuration to the Axum scaffold (Task 10.1). HTMX partials should only be served to same-origin requests.
- **Evidence:** No CORS configuration mentioned anywhere in the plan or spec.

---

## Dependency Audit

| Package | Version | Notes | Risk |
|---------|---------|-------|------|
| matrix-sdk | 0.8 | Core dependency, handles crypto. Active project. | Verify latest 0.8.x before pinning. |
| matrix-sdk-crypto-wasm | ^6.0.0 | Browser E2EE. Compiled from Rust. | Verify WASM supply chain (reproducible builds). |
| ruma | 0.10 | Matrix protocol types. | Low risk, well-maintained. |
| reqwest | 0.12 | HTTP client (test helpers). | Check for SSRF if used with user-controlled URLs. |
| tempfile | 3 | Temp directories for tests. | Verify cleanup on test failure (Drop impl). |
| syn | 2 | Code parsing for xtask. Build-time only. | Low risk. |
| @bytecodealliance/jco | ^1.0 | WASI runtime for npm launcher. | Verify no known CVEs. |
| @xterm/xterm | unspecified | Terminal emulator. | Check for escape sequence vulnerabilities. |
| age (planned) | unspecified | Encryption for secret store. | Verify implementation uses audited library. |

**Note:** Full `cargo audit` and `npm audit` should be run after dependencies are resolved. This review covers the design-time dependency choices only.

---

## Design-Level Threat Summary

### Threats Adequately Addressed by Design

1. **Fail-closed Policy Agent** — Appservice exclusive namespace prevents bypassing. Well-tested pattern.
2. **E2EE for command/result/secret traffic** — Megolm encryption prevents homeserver-level snooping on timeline events.
3. **Ephemeral worker identities** — Tombstoning prevents credential reuse.
4. **HSM-backed secret storage** — Key material protected from software compromise.
5. **Command allowlist** — Defense-in-depth layer (though bypassable, see HIGH finding above).
6. **PTY as dumb pipe** — No shell interpolation of Matrix event content.

### Threats Partially Addressed

1. **Secret exposure in E2EE DMs** — Protected by Megolm, but vulnerable to key compromise (see CRITICAL finding).
2. **Telemetry exposure** — E2EE rooms, but state events are unencrypted (see MEDIUM finding).
3. **Terminal session isolation** — Per-room, but history_visibility timing matters (see HIGH finding).

### Threats Not Addressed by Design

1. **Compromised orchestrator** — Can send arbitrary commands to any launcher it has access to. The architecture doc acknowledges this (SS6.8).
2. **Agent code integrity** — No code signing or verification. Acknowledged as future work.
3. **Network traffic analysis** — Matrix event patterns reveal activity even if content is encrypted.
4. **Browser key storage** — IndexedDB is not a secure store; a browser extension or XSS could exfiltrate Megolm keys.

---

## Summary

| Severity | Count |
|----------|-------|
| Critical (Design) | 2 |
| High (Design) | 4 |
| Medium (Design) | 6 |
| Low (Design) | 3 |

---

## Recommendation

**MERGE_WITH_FIXES** (for plan document)

The overall security architecture is strong — fail-closed Policy Agent, E2EE everywhere, HSM-backed secrets, ephemeral workers. The design reflects serious security thinking.

However, the following must be addressed before implementation begins:

**Must fix in plan (blocking):**
1. Replace all hardcoded test ports with OS-assigned ports (CRITICAL)
2. Add `cwd` validation to Task 4.2 requirements (HIGH)
3. Fix `history_visibility` to use `initial_state` in room creation (HIGH)
4. Add zlib bomb / oversized payload tests to Task 7.2 (HIGH)
5. Specify replay protection mechanism in Task 3.2 (MEDIUM)

**Should fix in plan (non-blocking but tracked):**
1. Document secret value double-encryption as future work with timeline (CRITICAL)
2. Add command argument validation tests for known bypass vectors (HIGH)
3. Gate test key constructors behind `#[cfg(test)]` (MEDIUM)
4. Document state event plaintext exposure in threat model (MEDIUM)
5. Add SRI and CORS requirements to Phase 10 (LOW)

**Accept as known risks (document in ADR):**
1. Compromised orchestrator threat (acknowledged in architecture doc)
2. Agent code integrity (future work)
3. State events not E2EE (Matrix protocol limitation)
4. Telemetry detail level exposure to room members
