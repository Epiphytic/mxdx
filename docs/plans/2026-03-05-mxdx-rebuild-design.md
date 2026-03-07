# mxdx Management Console — Rebuild Design

**Date:** 2026-03-05
**Revised:** 2026-03-06
**Status:** Approved (Rev 2)
**Context:** The first implementation attempt failed due to: (1) team lead losing context as the main session, (2) CI/CD tests blocking merges of code they depended on, (3) incorrect assumptions about tuwunel's interface, (4) outdated library versions, (5) security findings not addressed in design. This is a fresh start with lessons learned.

**Rev 2 changes:** Architecture updated to reflect npm+WASM as v1 shipping surface for client/launcher, all rooms E2EE with MSC4362 encrypted state events, simplified room topology (exec + logs, no separate status room), and actual npm workspace layout.

**Goal:** Rebuild the mxdx Management Console from scratch with correct phase ordering, security baked in, and a team structure that doesn't burn out the coordinator.

**Reference docs:**
- `docs/mxdx-management-console.md` — Full spec
- `docs/mxdx-architecture.md` — Core architecture
- `docs/mxdx-development-methodology.md` — TDD protocol
- `docs/reviews/security/2026-03-05-design-review-plan-and-spec.md` — Security findings

---

## 1. Architecture

### Dual-Target Strategy

The v1 release ships npm packages backed by Rust compiled to WASM. Native Rust binaries will follow as a second target, sharing maximum code through the WASM bridge.

| Layer | v1 (npm+WASM) | Future (native) |
|:---|:---|:---|
| **Core logic** | `crates/mxdx-core-wasm` (Rust -> wasm-pack -> Node.js) | `crates/mxdx-matrix` (native Rust) |
| **Launcher** | `packages/launcher` (Node.js, imports `@mxdx/core`) | `crates/mxdx-launcher` (native binary) |
| **Client CLI** | `packages/client` (Node.js, imports `@mxdx/core`) | Native CLI (reuses mxdx-matrix) |
| **Web client** | Shares `@mxdx/core` WASM bindings (browser target) | N/A |
| **Policy/Secrets/Web** | Rust-native from the start | Same |

**Code sharing:** The `mxdx-core-wasm` crate wraps `matrix-sdk` 0.16 and exposes a `WasmMatrixClient` via `wasm-bindgen`. Both Node.js packages and the future browser client consume this. The `@mxdx/core` npm package re-exports the WASM bindings plus shared JS utilities (session management, credential store).

### npm Workspace Layout (Actual)

```
packages/
├── core/                  # @mxdx/core — WASM bindings + shared session/credentials
│   ├── wasm/              # wasm-pack output (mxdx_core_wasm.js, .wasm, .d.ts)
│   ├── index.js           # Re-exports WASM + polyfills (fake-indexeddb/auto)
│   ├── session.js         # connectWithSession() — shared login/session/cross-signing
│   └── credentials.js     # CredentialStore — keyring + encrypted file fallback
├── launcher/              # mxdx-launcher — Node.js launcher runtime
│   ├── bin/mxdx-launcher.js
│   └── src/
│       ├── config.js      # LauncherConfig (TOML parsing, CLI args)
│       ├── runtime.js     # LauncherRuntime (sync loop, command processing)
│       └── process-bridge.js  # executeCommand() — child_process wrapper
├── client/                # mxdx-client — CLI client
│   ├── bin/mxdx-client.js
│   └── src/
│       ├── config.js      # ClientConfig
│       ├── discovery.js   # findLauncher() — space discovery
│       └── exec.js        # execCommand() — send command, wait for result
└── e2e-tests/             # E2E tests (local Tuwunel + public server)
```

### WASM Build

```bash
wasm-pack build crates/mxdx-core-wasm --target nodejs --out-dir ../../packages/core/wasm
```

Key WASM constraints:
- `fake-indexeddb/auto` must be imported before WASM loads (E2EE crypto store)
- `serde_wasm_bindgen::to_value` doesn't work for `serde_json::Value` — return JSON strings and `JSON.parse()` in JS
- `getrandom` v0.3 needs both `features = ["wasm_js"]` and `--cfg getrandom_backend="wasm_js"` in `.cargo/config.toml`

---

## 2. Room Topology

### Per-Launcher Rooms

Each launcher creates a Matrix space with two child rooms. All rooms are end-to-end encrypted.

```
Space (mxdx:<launcher_id>)
├── Exec Room (E2EE + MSC4362 encrypted state)
│   ├── org.mxdx.command events (commands from clients)
│   ├── org.mxdx.output events (stdout/stderr, base64-encoded)
│   ├── org.mxdx.result events (exit code, errors)
│   └── org.mxdx.host_telemetry state events (MSC4362 encrypted)
└── Logs Room (E2EE)
    └── Launcher process logs, system logs
```

**Why two rooms instead of three:** Telemetry state events live in the exec room as MSC4362 encrypted state events. No separate status room needed.

**MSC4362 (Simplified Encrypted State Events):** Encrypts state events via Megolm using packed `state_key` format `{type}:{state_key}`. Enabled by setting `encrypt_state_events: true` in the `m.room.encryption` event. Supported in `matrix-sdk` 0.16 behind `experimental-encrypted-state-events` feature flag.

### Interactive Sessions

Interactive terminal sessions use DMs, not the space rooms:

1. Client sends `org.mxdx.command` with `action: "interactive"` to exec room
2. Launcher creates a DM with the requesting user
3. DM has `history_visibility: joined` set in `initial_state` (not post-creation)
4. Terminal I/O flows through the DM as `org.mxdx.terminal_data` events
5. Launcher manages a tmux session underneath for persistence
6. If failover occurs, new launcher identity creates a new DM; tmux session persists

### Room Encryption Configuration

All rooms use:
```json
{
  "type": "m.room.encryption",
  "content": {
    "algorithm": "m.megolm.v1.aes-sha2",
    "encrypt_state_events": true
  }
}
```

This ensures telemetry state events in the exec room are encrypted alongside message events.

---

## 3. Session Model

### Non-Interactive Sessions

- Initiated by: authorized user posts `org.mxdx.command` in exec room
- Execution: launcher runs the command directly (no tmux)
- Output: `org.mxdx.output` events in exec room with `request_id` correlation
- stdout and stderr labeled separately via `stream` field
- Session ends when process exits; `org.mxdx.result` sent with exit code

### Interactive Sessions

- Initiated by: authorized user posts `org.mxdx.command` with `action: "interactive"` in exec room
- Launcher creates a DM with the user (`history_visibility: joined` in `initial_state`)
- Terminal data flows as `org.mxdx.terminal_data` events in the DM
- Requires tmux for session persistence
- If failover happens, new hot identity creates new DM; tmux session persists
- User never needs to track which launcher identity is hot

### Output Event Format

Both session types use the same output event format:
```json
{
  "type": "org.mxdx.output",
  "content": {
    "request_id": "...",
    "stream": "stdout",
    "data": "...",
    "encoding": "raw+base64",
    "seq": 0
  }
}
```

The `stream` field is either `"stdout"` or `"stderr"`.

---

## 4. Cross-Signing & Identity Verification

### Bootstrap Flow

Both launcher and client use the shared `connectWithSession()` helper from `@mxdx/core`:

1. Attempt session restore from keyring
2. If no session: login with password, bootstrap cross-signing, verify own identity
3. Store session + password in keyring (keytar with AES-256-GCM encrypted file fallback)

Cross-signing bootstrap uses the two-step UIA flow:
1. Call `bootstrapCrossSigning(null)` — server returns 401 with UIA session ID
2. Retry with `Password` auth including the session ID from step 1

**matrix.org limitation:** matrix.org only supports `m.oauth` for cross-signing key uploads. Use Element to set up cross-signing on matrix.org. The `bootstrapCrossSigningIfNeeded` + `verifyOwnIdentity` + sync flow will correctly see cross-signing state from Element.

### Verification Commands

The client CLI exposes:
- `mxdx-client verify <user_id>` — Cross-sign verify another user

---

## 5. Team Structure

### Roles (7)

| Role | Type | Responsibility |
|:---|:---|:---|
| **Product Owner** | Main session (human + Claude) | Reviews Lead's decisions, approves phase completions, resolves escalations |
| **Lead** | Spawned teammate | Coordinates phases, assigns work, reviews code, merges PRs, monitors `bd blocked`. Never writes code. |
| **Tester** | Spawned teammate | Writes failing tests first (including security exploit tests), closes [T] issues |
| **Coder** | Spawned teammate | Implements until tests pass, runs `cargo xtask manifest`, opens PRs |
| **Security Reviewer** | Spawned teammate | Reviews each phase against security checklist, writes adversarial variant tests, produces security report |
| **DevOps** | Spawned teammate | Preflight script, CI pipeline evolution, tuwunel research spike, WASI packaging |
| **Documenter** | Spawned teammate | Keeps MANIFEST.md, AGENTS.md, ADRs, README in sync. Writes phase summaries. Formats security reports. Detects spec drift. |

### Coordination Protocol

**TDD Handoff:**
1. Tester claims [T] issue -> writes failing tests (including security exploit tests) on feature branch -> confirms failure -> closes [T]
2. Tester messages Coder: "Task X.Y [T] done — mxdx-XXX closed, branch feat/phase-N pushed"
3. Coder picks up [C] issue (now unblocked) -> implements until tests green -> opens PR -> messages Lead
4. Lead reviews + merges PR -> closes [C] issue

**Security Verification (after each phase):**
1. Security Reviewer reads the Tester's security tests — do they actually exercise the attack vector?
2. Security Reviewer reads the Coder's fix — is it a real mitigation or a test-aware workaround?
3. Security Reviewer writes one adversarial variant the Coder didn't see — if the fix is genuine, this variant should also be blocked
4. If the adversarial variant exposes a gap, Security Reviewer files a new beads issue blocking phase completion

**Phase Completion Gate:**
1. Lead checks `bd epic status` — all [T] and [C] issues closed
2. Lead verifies CI green on the phase branch
3. Security Reviewer produces phase review doc
4. Documenter updates MANIFEST.md, phase summary, test matrix
5. DevOps updates CI pipeline for the new phase's tests
6. Lead reports to Product Owner for sign-off

### Context Management Rules

- Lead never writes code — only coordinates, reviews, merges. Keeps context free for tracking team state.
- Teammates get exactly what they need — task prompt includes: spec section, security requirements, branch name, file list. Nothing extra.
- Escalation is fast — if a teammate is stuck for more than 2 attempts, Lead escalates to Product Owner immediately. No spinning.
- Lead merges feature branches (not Product Owner). This was the bottleneck last time.
- Product Owner approves phase completion — Lead reports gate status, PO signs off.

---

## 6. Phase Structure & Dependency Graph

### Phases

```
Phase 0: Preflight & Research                              [DONE]
  |
Phase 1: Foundation (scaffold, xtask, build-only CI)       [DONE]
  |
Phase 2: Types (event schemas, Rust + TypeScript)           [DONE]
  |
Phase 3: Test Infrastructure (tuwunel helper)               [DONE]
  |
Phase 4: Matrix Client Facade (Rust + WASM bindings)        [DONE]
  |
Phase 5: Launcher v1 - Non-Interactive (npm+WASM)           [IN PROGRESS]
  |
Phase 6: Terminal - Interactive Sessions (DM-based, PTY)
  |
Phase 7: Browser Client (xterm.js, shares @mxdx/core)
  |
  +-- Phase 8: Policy Agent (appservice, fail-closed) [parallel]
  +-- Phase 9: Secrets Coordinator (age double-encrypted) [parallel]
  +-- Phase 10: Web App (HTMX dashboard, SSE) [parallel]
  |
Phase 11: Multi-Homeserver (failover, federation)
  |
Phase 12: Integration & Hardening (full E2E, security, WASI)
```

Phases 8, 9, 10 can run in parallel since they are independent layers on top of the core loop.

### Key Architecture Notes Per Phase

| Phase | Target | Notes |
|:---|:---|:---|
| 5 (Launcher v1) | npm+WASM | `packages/launcher` using `@mxdx/core` WasmMatrixClient |
| 6 (Terminal) | npm+WASM | DM-based sessions, tmux underneath, compression |
| 7 (Browser) | Browser WASM | Shares `@mxdx/core` bindings, xterm.js frontend |
| 8 (Policy) | Rust native | Appservice binary, fail-closed |
| 9 (Secrets) | Rust native | age double-encryption, ephemeral key exchange |
| 10 (Web App) | Rust native | Axum + HTMX, SSE |
| 11 (Failover) | Both | npm launcher handles identity rotation; Rust for federation |
| 12 (Hardening) | Both | Full E2E across both targets |

### CI Evolution

| Phase | CI Jobs Added |
|:---|:---|
| 0 (Preflight) | `preflight` — verify all required tools |
| 1 (Foundation) | `cargo build --workspace`, `cargo xtask manifest --check` |
| 2 (Types) | + `cargo test -p mxdx-types` |
| 3 (Test Infra) | + `cargo test -p mxdx-test-helpers` (requires tuwunel in CI) |
| 4 (Matrix) | + `cargo test -p mxdx-matrix`, WASM build check |
| 5 (Launcher) | + npm launcher tests, E2E local + public server tests |
| 6 (Terminal) | + terminal integration tests (requires tmux in CI) |
| 7 (Browser) | + browser client tests (playwright) |
| 8-10 | + respective crate tests |
| 12 (Hardening) | + full E2E suite, security report artifact, `cargo audit`, `npm audit` |

**Key rule:** A CI job is only added when the code it tests exists. No job ever references a crate or binary that hasn't been merged yet.

### Branch Strategy

- Each phase gets one feature branch: `feat/phase-N-name`
- Tester and Coder work on the same branch
- PRs merge to `main` at phase completion
- No long-lived branches

---

## 7. Security Integration

### Blocking Findings (Built Into Tasks)

| Finding | ID | Severity | Where It Lands |
|:---|:---|:---|:---|
| OS-assigned ports (no hardcoded) | mxdx-ji1 | CRITICAL | Phase 3 — TuwunelInstance::start() always uses port 0 |
| Secret double-encryption (age ephemeral keys) | mxdx-adr2 | CRITICAL | Phase 9 — required for v1, ADR-0002 rescinded |
| `cwd` validation against allowlist | mxdx-71v | HIGH | Phase 5 — executor rejects cwd outside configured prefixes |
| Command argument injection protection | mxdx-jjf | HIGH | Phase 5 — deny patterns for git -c, docker -f, env injection |
| `history_visibility = joined` via `initial_state` | mxdx-aew | HIGH | Phase 6 — set in DM creation call, not post-creation |
| Zlib bomb protection (bounded decompression) | mxdx-ccx | HIGH | Phase 6 — decode_decompress_bounded() with streaming limit |
| Replay protection via uuid LRU cache with TTL | mxdx-rpl | MEDIUM | Phase 8 — specified mechanism, bounded cache |
| `new_with_test_key()` behind #[cfg(test)] | mxdx-tky | MEDIUM | Phase 9 — CI check that it doesn't appear in release builds |
| Config file permission validation | mxdx-cfg | MEDIUM | Phase 5 — verify 0600 on startup, warn on group/world-readable |
| SRI + CORS for web assets | mxdx-web | LOW | Phase 10 — Subresource Integrity hashes, same-origin CORS |
| seq field defined as u64, no wraparound | mxdx-seq | MEDIUM | Phase 6 — explicitly u64, test for max value behavior |
| Telemetry detail levels configurable | mxdx-tel | MEDIUM | Phase 5 — full vs. summary mode in config |

### Security Test-Driven Design

For each blocking finding, the Tester writes an exploit test that demonstrates the vulnerability. The Coder makes the protection work. The Security Reviewer then:
1. Reviews the test — does it actually exercise the attack vector?
2. Reviews the fix — is it genuine or test-aware?
3. Writes one adversarial variant the Coder didn't see
4. Documents all three checks in the phase review

### Accepted Risks (Documented in ADRs)

- ~~State events not E2EE (Matrix protocol limitation)~~ **Mitigated:** MSC4362 encrypted state events enabled for all rooms
- Compromised orchestrator can send arbitrary commands (acknowledged in architecture doc)
- Browser IndexedDB key storage not secure against XSS (future: consider WebAuthn)
- Telemetry detail exposure to room members (mitigated by configurable detail levels)
- matrix.org requires OAuth for cross-signing key uploads (must use Element for bootstrap)

---

## 8. E2E Test Strategy

### Local Tests (Tuwunel)

Run against a local Tuwunel instance. Test the full npm+WASM stack:
- WasmMatrixClient login/register
- Room creation (space + exec + logs, all E2EE)
- Cross-signing bootstrap and verification
- Command execution round-trip (launcher + client)
- Interactive session DM creation
- MSC4362 encrypted state events

### Public Server Tests (matrix.org)

Test against real public infrastructure. Key design principles:
- **Reuse rooms:** Check if launcher space exists before creating. Create only if needed.
- **Throttle creation:** Rate-limit room creation to avoid matrix.org rate limits (1 room/second max).
- **All E2EE:** Every room created must be encrypted.
- **Cross-signing via Element:** Don't attempt programmatic cross-signing on matrix.org (OAuth-only). Verify state set up via Element.

Tests:
1. Login both accounts via WasmMatrixClient
2. Verify cross-signing state between accounts
3. Create/find launcher space with E2EE rooms
4. Send and receive encrypted custom events
5. Send MSC4362 encrypted state events (telemetry)
6. Launcher + Client round-trip (exec command, verify output)
7. Communication latency measurement (post to first response)

---

## 9. Beads Task Structure

### Issue Hierarchy

```
Epic (per phase)
  [T] Test task (Tester)
  [C] Code task (Coder) -- blocked by [T]
  [S] Security review task (Security Reviewer) -- blocked by all [C] in phase
  [D] Documentation task (Documenter) -- blocked by all [C] in phase
  [CI] CI update task (DevOps) -- blocked by all [C] in phase
```

### Task Naming Convention

```
[T] Phase N.M: description
[C] Phase N.M: description
[S] Phase N: Security review
[D] Phase N: Documentation sync
[CI] Phase N: CI pipeline update
```

### Dependency Rules

- [C] is always blocked by its paired [T]
- [S], [D], [CI] are blocked by all [C] tasks in their phase
- Phase N+1 tasks are blocked by Phase N's [S], [D], and [CI] tasks
- Phases 8/9/10 are blocked only by Phase 7, not by each other

---

## 10. Key Design Decisions

| # | Decision | Rationale |
|:---|:---|:---|
| 1 | npm+WASM for v1 client/launcher | Low friction release; `npx` install, no compilation needed for users |
| 2 | Native Rust binaries as second target | Performance, but shares core logic through WASM bridge |
| 3 | All rooms E2EE with MSC4362 | No cleartext state events; telemetry encrypted alongside messages |
| 4 | Two rooms per launcher (exec + logs) | Telemetry as encrypted state in exec room; no separate status room needed |
| 5 | Interactive sessions use DMs | Clean separation; user never tracks launcher identity |
| 6 | Logs room for launcher/system logs | Separate from command exec traffic; E2EE |
| 7 | Cross-signing via `connectWithSession()` | Shared helper handles full lifecycle for both launcher and client |
| 8 | Team lead is spawned teammate, not main session | Main session lost context last time |
| 9 | CI evolves with the code | No test job references unbuilt code |
| 10 | Tuwunel researched before any code | Ground truth ADR prevents cascading incorrect assumptions |
| 11 | stdout/stderr labeled separately via stream field | Consistent across both session types |
| 12 | Security findings are task requirements, not a separate phase | Baked into relevant tasks; exploit TDD + adversarial verification |
| 13 | Phases 8/9/10 run in parallel | Policy, secrets, web app are independent layers |
| 14 | Browser client shares `@mxdx/core` | Same WASM bindings as CLI; maximum code reuse |

---

## 11. Dependency Versions

| Package | Version | Notes |
|:---|:---|:---|
| matrix-sdk | 0.16 | Features: e2e-encryption, sqlite, experimental-encrypted-state-events |
| ruma | 0.14 | Features: client-api-c, events, appservice-api-c |
| wasm-pack | latest | Target: nodejs (v1), web (browser client) |
| fake-indexeddb | ^6.2.5 | Polyfill for WASM crypto store in Node.js |
| tokio | 1 | Full features (Rust crates only) |
| axum | 0.7 | Web framework (Phase 10) |
| age | 0.11 | Encryption for secret store (Phase 9) |
| sysinfo | 0.33 | Telemetry collection (Rust native) |
| commander | ^13 | CLI arg parsing (npm packages) |

Cargo.lock committed to version control. CI runs `cargo audit` and `npm audit`.
