# Final Security Review: mxdx Management Console

**Date:** 2026-03-06
**Scope:** All phases (1-12), full codebase
**Review type:** Final security gate
**Prerequisite:** Design review (`docs/reviews/security/2026-03-05-design-review-plan-and-spec.md`), phase-specific reviews (Phases 6, 8, 9, 10)

---

## Executive Summary

The mxdx management console has a strong security architecture: fail-closed policy enforcement via Matrix appservice exclusive namespaces, E2EE for all command/result/secret traffic, double-encryption for secrets delivery, ephemeral worker identities, and defense-in-depth command allowlisting with argument injection protection.

Of the 13 tracked security findings from the design review, **10 are fully remediated with passing tests**, **1 is partially remediated**, and **2 carry-forward items** were identified during phase reviews that are not yet addressed. Additionally, phase-specific reviews surfaced **5 new findings** requiring attention.

**Recommendation: CONDITIONAL SIGN-OFF** -- see Blockers section.

---

## Finding Status Table

### Design Review Findings (Original 13)

| ID | Severity | Description | Status | Evidence |
|:---|:---|:---|:---|:---|
| mxdx-ji1 | CRITICAL | Hardcoded test ports | **REMEDIATED** | `TuwunelInstance` uses port 0; all tests OS-assigned. Phase 3 summary confirms. |
| mxdx-adr2 | CRITICAL | Secret values in plaintext Matrix events | **REMEDIATED** | Double encryption via age x25519. `worker_requests_secret_with_double_encryption` E2E test passes (`crates/mxdx-secrets/tests/e2e_secret_request.rs:15`). |
| mxdx-71v | HIGH | Command injection via `cwd` field | **REMEDIATED** | `normalize_path` + prefix check. `test_security_cwd_outside_prefix_is_rejected`, `test_security_cwd_traversal_rejected` pass (`crates/mxdx-launcher/src/executor.rs:270,278`). |
| mxdx-jjf | HIGH | Command allowlist bypass via arguments | **REMEDIATED** | Per-command deny patterns. Tests: `test_security_git_dash_c_blocked`, `test_security_git_submodule_foreach_blocked`, `test_security_docker_compose_dash_f_blocked`, `test_security_env_prefix_injection_blocked` (`crates/mxdx-launcher/src/executor.rs:293-333`). |
| mxdx-aew | HIGH | history_visibility race condition | **REMEDIATED** | `HistoryVisibility::Joined` set in `initial_state` at room creation. E2E test `launcher_creates_terminal_dm_on_session_request` (`crates/mxdx-launcher/tests/e2e_terminal_session.rs:4`). Phase 6 review confirms PASS. |
| mxdx-8bm | HIGH | PTY data as attack surface | **REMEDIATED** | tmux `-l` literal mode, session name regex `[a-zA-Z0-9_-]+`, no `sh -c`. `is_valid_session_name` + `tmux_session_name_validated` test (`crates/mxdx-launcher/src/terminal/tmux.rs:8,168`). Phase 6 review confirms PASS. |
| mxdx-ccx | HIGH (added) | Zlib bomb / oversized payload | **REMEDIATED** | Streaming 8KB chunk decompression with byte limit. `test_security_zlib_bomb_rejected_before_pty_write`, `test_security_decompression_streams_and_fails_fast` (`crates/mxdx-launcher/src/terminal/compression.rs:97,105`). Phase 6 review confirms PASS. |
| mxdx-cfg | MEDIUM | TOML config file permissions | **REMEDIATED** | `validate_config_permissions` checks 0600 (`crates/mxdx-launcher/src/config.rs:82`). Phase 5 summary confirms. |
| mxdx-seq | MEDIUM | Sequence number overflow | **REMEDIATED** | `u64` for seq, tested near `u64::MAX`. `seq_counter_supports_u64_range` (`crates/mxdx-launcher/tests/e2e_terminal_session.rs:78`). Phase 6 review confirms PASS. |
| mxdx-rpl | MEDIUM | Replay protection unspecified | **REMEDIATED** | Bounded LRU cache (10,000 entries) with 1-hour TTL. `test_security_replayed_event_does_not_double_execute` + `replay_cache_ttl_expires_entries` (`crates/mxdx-policy/tests/policy_enforcement.rs:105`). Phase 8 review confirms PASS. |
| mxdx-tky | MEDIUM | Test key not cfg(test) gated | **REMEDIATED** | `#[cfg(test)]` on `new_with_test_key` (`crates/mxdx-secrets/src/store.rs:30-31`). `test_key_constructor_is_test_only` test (`crates/mxdx-secrets/src/store.rs:134`). Phase 9 review confirms PASS. |
| mxdx-tel | MEDIUM | Telemetry leaks infrastructure details | **REMEDIATED** | Configurable detail levels (Summary/Full). `config_supports_telemetry_detail_levels` + `telemetry_both_levels` tests (`crates/mxdx-launcher/src/config.rs:152`, `crates/mxdx-launcher/tests/e2e_full_system.rs:457`). |
| mxdx-web | LOW | Missing CORS / CSP for web app | **PARTIAL** | CORS: removed layer entirely (browser same-origin applies). CSP: implemented with `csp_header_is_set` test (`crates/mxdx-web/src/routes/mod.rs:131`). SRI: service worker structure exists but server never sends `X-Content-Hash` header -- **SRI is dead code**. |

### Phase Review Findings (New)

| Source | Severity | Description | Status | Evidence |
|:---|:---|:---|:---|:---|
| Phase 9 Review, Finding 2 | HIGH | No sender identity verification in SecretCoordinator -- scope-only auth | **OPEN** | `coordinator.rs:38` does not check Matrix sender ID. Any room member can request any authorized scope. |
| Phase 10 Review, Finding 1 | HIGH (fixed) | CORS allows origin "null" | **REMEDIATED** | Removed CORS layer entirely. Phase 10 summary confirms. |
| Phase 9 Review, Finding 1 | MEDIUM | No replay protection for SecretRequestEvent | **OPEN** | `request_id` not tracked; replayed requests get valid responses. |
| Phase 10 Review, Finding 3 | MEDIUM | HTMX partials lack server-side origin check | **OPEN** | Fleet metadata accessible to any caller on `/dashboard`. Mitigated by localhost-only binding. |
| Phase 10 Review, Finding 4 | MEDIUM | SSE endpoint has no connection limits | **OPEN** | Unbounded SSE connections could exhaust memory. Mitigated by localhost-only binding. |
| Phase 10 Review, Finding 2 | MEDIUM | SRI verification is dead code | **OPEN** | `sw.js:verifyIntegrity` checks `X-Content-Hash` but server never sets it. |
| Phase 9 Review | MISSING | No audit trail for secret access | **OPEN** | Coordinator does not log grants/denials to audit room. |
| Phase 6 Review, Finding 1 | MEDIUM | tmux command not validated against allowlist in session creation path | **OPEN** | `TmuxSession::create` takes unvalidated `command` parameter. Validation exists in executor but not wired into session path. |
| Phase 6 Review, Finding 3 | LOW | Recovery state file not permission-checked | **OPEN** | Recovery JSON written without 0600 permissions. Room IDs could leak topology. |
| Phase 8 Review, Finding 2 | LOW | YAML injection in appservice registration | **OPEN** | Hand-rolled YAML formatter. Config is operator-controlled, so risk is low. |
| Phase 9 Review, Finding 3 | LOW | Error messages leak scope existence | **OPEN** | Distinct "unauthorized" vs "not found" responses enable scope enumeration. |
| Phase 11 | -- | Non-launcher identity state event update blocked | **REMEDIATED** | `security_non_launcher_cannot_update_identity_event` test (`crates/mxdx-launcher/src/multi_hs.rs:336`). |

---

## Security Test Summary

**11 `test_security_*` prefixed tests** found in the codebase:

| Test | Location | Finding |
|:---|:---|:---|
| `test_security_cwd_outside_prefix_is_rejected` | executor.rs:270 | mxdx-71v |
| `test_security_cwd_traversal_rejected` | executor.rs:278 | mxdx-71v |
| `test_security_cwd_none_uses_default` | executor.rs:285 | mxdx-71v |
| `test_security_git_dash_c_blocked` | executor.rs:293 | mxdx-jjf |
| `test_security_git_submodule_foreach_blocked` | executor.rs:309 | mxdx-jjf |
| `test_security_docker_compose_dash_f_blocked` | executor.rs:321 | mxdx-jjf |
| `test_security_env_prefix_injection_blocked` | executor.rs:333 | mxdx-jjf |
| `test_security_zlib_bomb_rejected_before_pty_write` | compression.rs:97 | mxdx-ccx |
| `test_security_decompression_streams_and_fails_fast` | compression.rs:105 | mxdx-ccx |
| `test_security_policy_agent_down_blocks_all_agent_actions` | policy_enforcement.rs:47 | Phase 8 fail-closed |
| `test_security_replayed_event_does_not_double_execute` | policy_enforcement.rs:105 | mxdx-rpl |

**Additional security-relevant tests** (not `test_security_*` prefixed):

| Test | Location | Finding |
|:---|:---|:---|
| `launcher_creates_terminal_dm_on_session_request` | e2e_terminal_session.rs:4 | mxdx-aew |
| `seq_counter_supports_u64_range` | e2e_terminal_session.rs:78 | mxdx-seq |
| `worker_requests_secret_with_double_encryption` | e2e_secret_request.rs:15 | mxdx-adr2 |
| `test_key_constructor_is_test_only` | store.rs:134 | mxdx-tky |
| `csp_header_is_set` | routes/mod.rs:131 | mxdx-web |
| `tmux_session_name_validated` | tmux.rs:168 | mxdx-8bm |
| `security_non_launcher_cannot_update_identity_event` | multi_hs.rs:336 | Phase 11 |
| `config_supports_telemetry_detail_levels` | config.rs:152 | mxdx-tel |
| `agent_namespace_is_exclusive` | appservice_registration.rs | Phase 8 |

---

## Accepted Risks

These are documented in the design document (`docs/plans/2026-03-05-mxdx-rebuild-design.md:212`):

| Risk | Justification |
|:---|:---|
| State events not E2EE | Matrix protocol limitation. State events must be unencrypted for room state to function. Sensitive data moved to timeline events where possible. Telemetry detail levels (mxdx-tel) reduce exposure. |
| Compromised orchestrator | Acknowledged in architecture doc SS6.8. Out of scope for current threat model -- an orchestrator with room access can send commands by design. Mitigated by E2EE (homeserver cannot impersonate). |
| Browser IndexedDB key storage | Not secure against XSS or malicious browser extensions. Future consideration: WebAuthn. Mitigated by CSP headers (mxdx-web). |
| Telemetry detail exposure to room members | Mitigated by configurable detail levels (Summary vs Full). Status rooms should have strict membership. |
| LRU replay cache eviction (Phase 8, Finding 1) | 10,000 capacity with 1-hour TTL is adequate for fleet management scale. Exploitation requires authorized user generating >10,000 events in TTL window. |

---

## Carry-Forward Items

Items that should be addressed in a hardening pass or future release:

### Blockers (must fix before production deployment)

1. **No sender identity verification in SecretCoordinator** (Phase 9 Review, HIGH) -- Any room member can request any authorized scope. Must implement per-user scope authorization (`HashMap<OwnedUserId, HashSet<String>>`).

2. **No audit trail for secret access** (Phase 9 Review, MISSING) -- Secret grants and denials are not logged. Required for compliance and incident response.

### Should Fix

3. **tmux command not validated against allowlist in session path** (Phase 6 Review, MEDIUM) -- The executor validates commands, but the terminal session creation path does not. Wire executor validation into `TerminalSession::create`.

4. **No replay protection for SecretRequestEvent** (Phase 9 Review, MEDIUM) -- Track consumed `request_id` values. Bind to sender Matrix user ID.

5. **SRI verification is dead code** (Phase 10 Review, MEDIUM) -- Either implement `X-Content-Hash` headers on static file responses or embed a hash manifest in `sw.js`.

6. **HTMX partials lack server-side origin check** (Phase 10 Review, MEDIUM) -- Add `Sec-Fetch-Site: same-origin` check or `HX-Request` header validation. Fleet metadata accessible to any caller.

7. **SSE endpoint has no connection limits** (Phase 10 Review, MEDIUM) -- Add `ConcurrencyLimitLayer`. Currently mitigated by localhost-only binding.

### Low Priority

8. **Recovery state file permissions** (Phase 6 Review, LOW) -- Set 0600 on save.
9. **YAML injection in appservice registration** (Phase 8 Review, LOW) -- Use proper YAML serialization library or validate config fields.
10. **Error messages leak scope existence** (Phase 9 Review, LOW) -- Return uniform denial message.
11. **`style-src 'unsafe-inline'` in CSP** (Phase 10 Review, LOW) -- Remove if inline styles not needed, or document as accepted trade-off.

---

## CI Security Infrastructure

The `.github/workflows/security-report.yml` workflow:
- Triggers on tagged releases (`v*`) and manual dispatch
- Runs all `test_security_*` tests
- Runs `cargo audit` and `npm audit`
- Collects phase review documents
- Publishes security report as GitHub Release artifact

This provides continuous regression testing for all security controls.

---

## Recommendation

**CONDITIONAL SIGN-OFF**

The project demonstrates strong security engineering across all 12 phases. All 13 original design review findings have been addressed (12 fully remediated, 1 partial). The security test coverage is comprehensive, with 20+ security-relevant tests mapped to specific findings.

**Two blockers prevent unconditional sign-off:**

1. **SecretCoordinator sender identity verification** (HIGH) -- Without this, any room member can extract secrets for any authorized scope. This undermines the entire secrets management system.

2. **Secret access audit trail** (MISSING) -- Required for incident response and compliance. Cannot determine post-facto whether secrets were accessed by unauthorized parties.

Once these two items are remediated with passing tests, the project meets the security bar for production deployment. All other carry-forward items are hardening improvements that can be addressed in subsequent releases.
