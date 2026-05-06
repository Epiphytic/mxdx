# Security Review: rust-npm-binary-parity Phase 3

**Date:** 2026-04-29
**Scope:** Config schema canonicalization (T-3.1 through T-3.6)
**Files reviewed:**
- `crates/mxdx-types/src/config.rs`
- `packages/launcher/src/config.js`
- `packages/client/src/config.js`

---

## Secrets Scan

**Result: Clean.**

- All `password`, `credential`, and `token` matches in `config.rs` are inside test TOML fixture strings, not hardcoded production values.
- `password` field in `LauncherConfig` and `ClientConfig` is runtime-only: stored on the in-memory object, never written to `ownedFields` in `save()`, never persisted to disk.
- `registrationToken` in `LauncherConfig` is now correctly persisted (via `registration_token` key) only when non-null — no silent exposure.
- No `.env` files committed. No base64-encoded keys found.

---

## OWASP Assessment

### Injection (A03)
- TOML deserialization uses `smol-toml` (JS) and the `toml` crate (Rust). Both are data-only parsers; there is no eval, exec, or dynamic query construction involved. No injection risk from config parsing.

### Broken Authentication (A07)
- `authorized_users`, `allowed_commands`, and `trust_anchor` are now verified to survive migration byte-for-byte (security-critical field survival test). Silent zero-fielding of these values would be an authorization bypass — the migration correctness fix in T4 Nurture (merge-not-replace) eliminates this risk.

### Sensitive Data Exposure (A02)
- Config files are written at `0o600` (owner read/write only) on all Unix writes: migration backup, flat rewrite, and `save()`. Directory created at `0o700`.
- Migration backup (`.legacy.bak`) inherits the original file's permissions in Rust; written at `0o600` in JS.
- Passwords are never logged; `registrationToken` is only written to disk when explicitly set — not logged.

### Broken Access Control (A01)
- `filePath` in JS is CLI-user-controlled via `--config`. This is expected for a local CLI tool; the threat actor would need local user access, which already exceeds the trust boundary. Accepted.
- The `authorized_users` authorization list is security-critical. Migration now guarantees it survives without any data loss or reordering.

### Security Misconfiguration (A05)
- No default credentials in any config struct. All security-critical fields (`authorized_users`, `trust_anchor`, `allowed_commands`) default to empty/None — fail-closed, not fail-open.
- `max_sessions` defaults to 5, limiting resource exhaustion.

### XSS / Insufficient Logging / Other Categories
- Not applicable to config schema layer (no web output, no logging of secrets, no XML).

---

## Dependency Audit

### Rust (`cargo audit`)
| Advisory | Crate | Severity | Impact | Disposition |
|----------|-------|----------|--------|-------------|
| RUSTSEC-2024-0388 | `derivative` 2.2.0 | Warning (unmaintained) | Low — no vulnerability, unmaintained proc-macro | Accept: transitive dep, no known exploit |
| RUSTSEC-2026-0097 | `rand` 0.9.2 | Warning (soundness) | Low — requires attacker-controlled custom logger | Accept: no custom logger in mxdx; pre-existing advisory |

No critical or high Rust advisories. 2 pre-existing warnings accepted.

### Node.js (`npm audit`)
| Advisory | Package | Severity | Impact | Disposition |
|----------|---------|----------|--------|-------------|
| GHSA-c2c7-rcm5-vvqj | `picomatch` 4.0.0–4.0.3 | High | ReDoS via extglob quantifiers | Low impact: build-time dev dep, not in runtime config path. File follow-up cleanup task. |
| GHSA-qx2v-qp2m-jg93 | `postcss` <8.5.10 | Moderate | XSS in CSS stringify | Build-time only; no user-controlled CSS processed in config layer. File follow-up. |

3 npm advisories — all build-time/dev dependencies, none in the config schema code paths reviewed here.

---

## Threat Model

### Assets
1. `authorized_users` — controls who can issue commands to the worker
2. `allowed_commands` / `allowed_cwd` — controls what commands can be run
3. `trust_anchor` — root of cross-signing trust
4. `registration_token` — one-time token for initial device registration

### Trust Boundaries
| Boundary | Risk |
|----------|------|
| Config file on disk → runtime | File controlled by local user (expected). Permissions 0o600 limit external read. |
| Legacy config migration | Migration must not silently drop or zero security fields (mitigated by merge-not-replace + survival test). |
| CLI `--config` flag | Path user-controlled; accepted for local CLI. |
| npm `save()` round-trip | Unrelated Rust-written keys (e.g., `authorized_users`) must survive — ensured by owned-key merge pattern. |

### STRIDE Analysis
| Threat | Category | Mitigation |
|--------|----------|------------|
| Migration zeroes `authorized_users` → all users authorized | Elevation of Privilege | Merge-not-replace fix; survival test as CI gate |
| `registration_token` leaked to disk unintentionally | Information Disclosure | Not in `ownedFields` for client; persisted only when explicitly set for launcher |
| Config file readable by other users | Information Disclosure | 0o600 permissions on all write paths |
| Malicious config file with huge TOML doc causing OOM | DoS | Low risk: local user controls config. No external config ingestion. Accept. |
| `.legacy.bak` written to attacker-controlled path | Tampering | Backup path is always `filePath + ".legacy.bak"` — suffix append, no traversal |

---

## Findings

| Severity | Category | Finding | File | Status |
|----------|----------|---------|------|--------|
| High (pre-T4) | Broken Access Control | Migration dropped unrelated top-level keys — could zero `authorized_users` if it existed outside the section | `config.rs`, `config.js` (both) | **Fixed in T4 Nurture** |
| Medium (pre-T4) | Sensitive Data | `||` coercion of `batch_ms=0` would silently substitute default, masking user intent | `config.js` (both) | **Fixed in T4 Nurture (`??`)** |
| Low | Sensitive Data | No file size limit on config read; local user could OOM process with huge TOML | `config.rs`, `config.js` | Accept (local trust boundary) |
| Low | Dependency | npm picomatch ReDoS (build-time dev dep) | `package.json` | File follow-up cleanup task |
| Low | Dependency | npm postcss XSS (build-time dev dep) | `package.json` | File follow-up cleanup task |
| Info | Dependency | Rust `derivative` unmaintained; `rand` soundness warning | `Cargo.lock` | Pre-existing; accept |

---

## Remediations Applied

1. **Merge-not-replace migration (High):** `migrate_legacy_section_if_needed()` now builds merged TOML table (full doc minus section key, plus section fields). Same fix in JS `load()` and `save()` for both launcher and client. — commit `f0fba50`
2. **`??` null-coalescing (Medium):** All numeric/string defaults in JS `load()` paths changed from `||` to `??`. — commit `f0fba50`
3. **`registrationToken` persisted (Medium):** Now written to `registration_token` in `save()` when non-null; loaded in `load()` via `??`. — commit `f0fba50`
4. **`defaultPath()` corrected (Low):** `ClientConfig.defaultPath()` now returns `~/.mxdx/client.toml`. — commit `f0fba50`

---

## Remaining Risks

- **npm dev-dep ReDoS / XSS advisories:** Build-time only, no runtime impact on config layer. Accepted for this phase; follow-up cleanup task filed.
- **No config file size limit:** Accepted — local user controls their own config file; no external ingestion path exists.
- **TOCTOU on existsSync + readFileSync:** Accepted — CLI process runs as the same user who owns the file; no privilege separation concern.

---

## Council Feedback

Single-mode review (no `--parallel` flag passed). No council invocation.
