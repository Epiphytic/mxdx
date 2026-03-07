# mxdx Launcher, Client & Web Console — Implementation Plan

**Date:** 2026-03-06
**Goal:** Close the gap between current state and a production-ready npm+WASM launcher, client CLI, and web console with interactive terminals.
**Design doc:** `docs/plans/2026-03-05-mxdx-rebuild-design.md` (Rev 2)
**Workflow:** Agent team per `@epiphytic/agenticenti` — see `docs/agent-team-workflow.md`

---

## Current State

### What Works

| Component | Status | What Works |
|:---|:---|:---|
| `@mxdx/core` (WASM bindings) | 85% | Login, session restore, cross-signing, room creation (3 rooms), events, sync |
| `packages/launcher` | 70% | Non-interactive exec, telemetry posting, admin invites, sync loop |
| `packages/client` | 50% | `exec` command works E2E, `verify` partial, `launchers`/`telemetry` stubbed |
| `packages/e2e-tests` | 40% | Local onboarding + round-trip pass, public server login/cross-signing pass |

### Existing Code From Previous Agent (Old Layout)

Code exists in `client/` (separate from `packages/`) that was built by a previous agent. Some is reusable:

| Component | Location | Status | Reusable? |
|:---|:---|:---|:---|
| **TerminalSocket** | `client/mxdx-client/src/terminal.ts` | Complete (17 tests) | Yes — port to `packages/core` |
| **Terminal event schemas** (Zod) | `client/mxdx-client/src/types/terminal.ts` | Complete | Yes — port to `packages/core` |
| **MxdxClient** (browser fetch client) | `client/mxdx-client/src/client.ts` | Complete | No — replaced by `@mxdx/core` WasmMatrixClient |
| **CryptoClient** (browser E2EE) | `client/mxdx-client/src/crypto.ts` | Complete | No — replaced by WASM E2EE |
| **Discovery** (launcher finder) | `client/mxdx-client/src/discovery.ts` | Complete | No — uses raw Matrix API, not WASM |
| **Axum web backend** | `crates/mxdx-web/` | Complete (8 tests) | Partially — dashboard/SSE logic good, but needs Matrix integration |
| **Web UI package** | `client/mxdx-web-ui/` | Empty skeleton | No — `export {};` |

**Key decision:** The TerminalSocket and terminal schemas are well-tested protocol code that works in both Node.js and browser. Port these to `packages/core` so both CLI and browser clients can use them. Discard the old MxdxClient/CryptoClient/Discovery since `@mxdx/core` replaces them.

### What's Broken or Missing

**Foundation (Phase A):**
1. Room topology creates 3 rooms; should be 2 (exec + logs, no status room)
2. MSC4362 not enabled — telemetry state events are cleartext
3. Logs room unencrypted — only exec room has E2EE
4. Telemetry posted to wrong room (status instead of exec)

**Launcher (Phase B):**
5. `--max-sessions` and `--telemetry` flags parsed but not enforced
6. No exponential backoff on sync errors
7. No structured logging
8. No interactive session support (PTY/tmux/DM)

**Client CLI (Phase C):**
9. `launchers` command stubbed (prints help text, doesn't list)
10. `telemetry` reads from status room (wrong)
11. Missing `smol-toml` dependency
12. No interactive session support

**Web Console (Phase F):**
13. No frontend app shell (HTML/CSS/JS)
14. No browser WASM build target
15. No xterm.js integration page
16. Axum backend has no Matrix connection (uses mock AppState)
17. No auth flow in browser

**Testing:**
18. No npm E2E tests in CI
19. Public server tests create new rooms each run

---

## Agent Team

### Team Composition (7 roles — large project, 7 phases)

Per `agent-team-workflow.md` Section 1: 8+ tasks = full team.

| Role | Type | Responsibility |
|:---|:---|:---|
| **Product Owner** | Main session | Reviews, approves phase completions, resolves escalations |
| **Lead** | Spawned teammate | Coordinates phases, assigns work, reviews PRs, merges. Never writes code. |
| **Tester** | Spawned teammate | Writes failing tests for each [T] task, including security exploit tests |
| **Coder** | Spawned teammate | Implements npm/JS code until tests pass, opens PRs |
| **Coder-WASM** | Spawned teammate | Rust/WASM changes (Cargo.toml, lib.rs, wasm-pack builds) |
| **Security Reviewer** | Spawned teammate | Reviews E2EE configuration, auth flows, data exposure, CSP/SRI. Writes adversarial variant tests. Produces per-phase security reports. Encryption is constantly in play — every phase gets a security review. |
| **Documenter** | Spawned teammate | Updates MANIFEST.md, phase summaries, design doc alignment |

### Coordination Protocol

Per `agent-team-workflow.md`:
- **TDD Handoff:** [T] -> [C] -> Lead review -> merge
- **Phase Gate:** All issues closed + tests pass + docs updated + PO sign-off
- **Escalation:** 2 failed attempts -> Lead escalates to PO immediately
- **Branch:** Single branch `feat/launcher-client-v1` for Phases A-D. New branches for E-F.

---

## Phase A: Room Topology & MSC4362 (WASM Layer)

> **Epic:** Fix the foundation — correct room topology, all rooms E2EE, MSC4362 encrypted state events.
> **Agents:** Tester -> Coder-WASM -> Coder
> **Branch:** `feat/launcher-client-v1`
> **Completion gate:** WASM builds, `getOrCreateLauncherSpace()` creates 2 rooms (exec + logs), both E2EE with MSC4362, no `status_room_id` in topology. Launcher and client updated to match.

### Task A.1 [T]: Room Topology Tests

**Files:**
- Modify: `packages/e2e-tests/tests/launcher-onboarding.test.js`

Tests:
1. `getOrCreateLauncherSpace()` returns `{ space_id, exec_room_id, logs_room_id }` — NO `status_room_id`
2. Calling it twice returns same topology (idempotent)
3. Exec room is E2EE (encrypted event round-trip)
4. Logs room is E2EE (encrypted event round-trip)
5. Telemetry state event sendable to exec room

Expected: FAIL — topology returns 3 rooms, logs unencrypted.

### Task A.1 [C]: Fix Room Topology in WASM

**Files:**
- Modify: `crates/mxdx-core-wasm/Cargo.toml` — add `experimental-encrypted-state-events`
- Modify: `crates/mxdx-core-wasm/src/lib.rs`
- Rebuild: `wasm-pack build crates/mxdx-core-wasm --target nodejs --out-dir ../../packages/core/wasm`

Changes:
1. Remove status room creation from `create_launcher_space()`
2. Add `encrypt_state_events: true` to encryption content for both rooms
3. Add E2EE to logs room (currently unencrypted)
4. Return `{ space_id, exec_room_id, logs_room_id }` — no `status_room_id`
5. Update `findLauncherSpace()` to scan for 2 topic markers

### Task A.2 [C]: Update Launcher for New Topology

**Files:**
- Modify: `packages/launcher/src/runtime.js`

1. Remove all `status_room_id` references
2. Post telemetry to `exec_room_id` as encrypted state event
3. Admin invite loop: space, exec, logs (not status)

### Task A.3 [C]: Update Client for New Topology

**Files:**
- Modify: `packages/client/bin/mxdx-client.js`
- Modify: `packages/client/package.json` — add `smol-toml`, remove unused `inquirer`/`chalk`

1. `telemetry` command reads from `exec_room_id`
2. Remove `status_room_id` references

---

## Phase B: Launcher Hardening

> **Epic:** Production-ready non-interactive launcher.
> **Agents:** Tester -> Coder
> **Branch:** `feat/launcher-client-v1`
> **Completion gate:** Telemetry levels enforced, max-sessions enforced, structured logging, exponential backoff.

### Task B.1 [T]+[C]: Telemetry Detail Levels

**Files:** `packages/launcher/tests/telemetry.test.js`, `packages/launcher/src/runtime.js`

- `--telemetry full` posts all fields
- `--telemetry summary` posts hostname, platform, arch only
- Default: `full`

### Task B.2 [T]+[C]: Max Sessions Enforcement

**Files:** `packages/launcher/tests/sessions.test.js`, `packages/launcher/src/runtime.js`

- Track `#activeSessions` counter
- Reject with `org.mxdx.result` error at limit
- Default: 10

### Task B.3 [T]+[C]: Sync Resilience

**Files:** `packages/launcher/tests/resilience.test.js`, `packages/launcher/src/runtime.js`

- Exponential backoff: 1s, 2s, 4s, 8s, 16s, 30s max
- Reset on success

### Task B.4 [C]: Structured Logging

**Files:** `packages/launcher/src/runtime.js`, `packages/launcher/bin/mxdx-launcher.js`

- JSON logging: `{"level":"info","msg":"...","ts":"..."}`
- `--log-format text` for human-readable

---

## Phase C: Client CLI Completion

> **Epic:** Fully functional CLI client for non-interactive use.
> **Agents:** Tester -> Coder
> **Branch:** `feat/launcher-client-v1`
> **Completion gate:** `launchers` lists discovered launchers, `telemetry` reads encrypted state from exec room, proper error handling.

### Task C.1 [T]+[C]: Launcher Discovery

**Files:** `packages/client/tests/discovery.test.js`, `packages/client/bin/mxdx-client.js`

Replace `launchers` stub with real implementation. Requires new WASM method `listLauncherSpaces()`.

### Task C.2 [T]+[C]: Fix Telemetry Display

**Files:** `packages/client/bin/mxdx-client.js`

Read from `exec_room_id`. Support `--format json`.

### Task C.3 [C]: Error Handling & Cleanup

Remove dead dependencies, add timeouts on sync, proper error messages.

---

## Phase D: E2E Tests & CI

> **Epic:** Comprehensive test suite in CI.
> **Agents:** Tester -> Coder
> **Branch:** `feat/launcher-client-v1`
> **Completion gate:** Local E2E tests in CI. Public server tests reuse rooms, throttle creation, measure latency.

### Task D.1 [T]: Update Local E2E Tests

Update for 2-room topology. Test multiple commands, stderr, deduplication.

### Task D.2 [T]: Redesign Public Server Tests

- **Room reuse:** Fixed launcher IDs, find-or-create
- **Throttle:** 2s sleep between room creations
- **Latency:** Time from command send to first output event, assert < 10s
- **Idempotency:** Double `getOrCreateLauncherSpace()` returns same result

### Task D.3 [C]: Add `listLauncherSpaces()` WASM Method

New WASM method for C.1. Scan joined rooms for matching topic pattern.

### Task D.4 [CI]: npm E2E in CI Pipeline

Add job to `.github/workflows/ci.yml`. Public server tests manual-trigger only.

---

## Phase E: Interactive Terminal Sessions

> **Epic:** DM-based interactive terminal sessions with PTY forwarding. This is the core feature that makes mxdx a management console, not just a remote exec tool.
> **Agents:** Tester -> Coder-WASM -> Coder
> **Branch:** `feat/interactive-sessions`
> **Completion gate:** Client sends interactive session request, launcher creates encrypted DM, PTY I/O flows through Matrix events, session survives launcher sync drops, tmux provides persistence.
> **Security:** `history_visibility = joined` in DM `initial_state` (mxdx-aew), zlib bomb protection (mxdx-ccx)

### Architecture

```
Client                          Launcher
  |                                |
  |-- org.mxdx.command ----------->|  (action: "interactive", in exec room)
  |                                |-- creates DM room (E2EE, history_visibility: joined)
  |                                |-- spawns tmux session + PTY
  |<- org.mxdx.terminal.session -->|  (response with DM room_id, in exec room)
  |                                |
  |== DM Room (E2EE) =============|
  |                                |
  |-- org.mxdx.terminal.data ----->|  (stdin: keystrokes from user)
  |<- org.mxdx.terminal.data -----|  (stdout/stderr: PTY output)
  |-- org.mxdx.terminal.resize --->|  (window resize)
  |                                |
  |-- close DM / timeout -------->|  (kills tmux session)
```

### Task E.1 [C]: WASM — Add DM Room Creation

**Files:**
- Modify: `crates/mxdx-core-wasm/src/lib.rs`

Add methods:
```rust
#[wasm_bindgen(js_name = "createDmRoom")]
pub async fn create_dm_room(&self, user_id: &str) -> Result<String, JsValue>
```

Creates a DM with:
- `is_direct: true`
- `invite: [user_id]`
- `initial_state`: E2EE encryption + `history_visibility: joined` (mxdx-aew)
- Returns room_id as string

```rust
#[wasm_bindgen(js_name = "onRoomEvent")]
pub async fn on_room_event(&self, room_id: &str, event_type: &str, timeout_secs: u32) -> Result<String, JsValue>
```

Sync and wait for a specific event type in a room. Returns event content as JSON string. Needed for the TerminalSocket to receive terminal data without polling `collectRoomEvents`.

### Task E.2 [C]: Port TerminalSocket to `@mxdx/core`

**Files:**
- Create: `packages/core/terminal-socket.js` — port from `client/mxdx-client/src/terminal.ts`
- Create: `packages/core/terminal-types.js` — port Zod schemas from `client/mxdx-client/src/types/terminal.ts`
- Modify: `packages/core/index.js` — re-export TerminalSocket
- Modify: `packages/core/package.json` — add `zod` dependency

Port the TerminalSocket to plain JS (no TypeScript build step needed). Key adaptation:
- Replace `TerminalMatrixClient` interface with a wrapper around `WasmMatrixClient`
- The socket needs `sendEvent()` and an event listener. The WASM client has `sendEvent()` but no callback-based event listener — will need an adapter that polls `collectRoomEvents()` or uses the new `onRoomEvent()` method.

### Task E.3 [T]: Interactive Session Tests — Launcher Side

**Files:**
- Create: `packages/e2e-tests/tests/interactive-session.test.js`

Tests (against local Tuwunel):
1. Client sends `org.mxdx.command` with `action: "interactive"` to exec room
2. Launcher creates DM room, responds with `org.mxdx.terminal.session` event containing DM `room_id`
3. DM room has `history_visibility: joined` (mxdx-aew — verified via room state)
4. DM room is E2EE
5. Launcher spawns tmux session and PTY
6. Client sends terminal data to DM, receives PTY output back
7. Client sends resize event, launcher resizes PTY
8. Session ends when client closes DM (or timeout)
9. **Security (mxdx-ccx):** Compressed data exceeding 1MB decompressed limit is rejected

Expected: FAIL — interactive sessions not implemented.

### Task E.3 [C]: Launcher — Interactive Session Handling

**Files:**
- Create: `packages/launcher/src/pty-bridge.js` — PTY allocation + tmux management
- Modify: `packages/launcher/src/runtime.js` — detect interactive commands, create DMs, forward I/O

**PTY Bridge (`pty-bridge.js`):**
```javascript
export class PtyBridge {
  constructor(command, { cols, rows, cwd, env }) { ... }
  // Spawns: tmux new-session -d -s <id> -x <cols> -y <rows> <command>
  // Attaches PTY to tmux session

  write(data) { ... }    // stdin -> PTY
  onData(callback) { ... } // PTY -> stdout callback
  resize(cols, rows) { ... } // tmux resize
  kill() { ... }           // tmux kill-session
  get alive() { ... }
}
```

Uses `node-pty` (or `child_process` with `pty.js`) for PTY allocation. tmux wraps the PTY for persistence.

**Runtime changes:**
1. In `#processCommands()`, detect `content.action === "interactive"`
2. Create DM via `client.createDmRoom(sender_user_id)` — need to extract sender from event
3. Respond in exec room: `org.mxdx.terminal.session` with `{ request_id, status: "started", room_id }`
4. Spawn `PtyBridge` with command/cols/rows from request
5. Forward PTY output to DM as `org.mxdx.terminal.data` events (base64, with compression for >= 32 bytes)
6. Listen for incoming `org.mxdx.terminal.data` in DM (client keystrokes) -> write to PTY
7. Listen for `org.mxdx.terminal.resize` in DM -> resize PTY
8. On PTY exit: send `org.mxdx.terminal.session` with `status: "ended"`, close resources
9. Track against `maxSessions` counter

**Security (mxdx-ccx):** When decompressing incoming terminal data, use bounded decompression (max 1MB). Reject if exceeded.

### Task E.4 [T]+[C]: Client CLI — Interactive Session Command

**Files:**
- Create: `packages/client/src/interactive.js`
- Modify: `packages/client/bin/mxdx-client.js`

Add CLI command:
```
mxdx-client shell <launcher> [command]
  --cols <n>    Terminal columns (default: current terminal width)
  --rows <n>    Terminal rows (default: current terminal height)
```

Implementation:
1. Find launcher via `findLauncher()`
2. Send `org.mxdx.command` with `action: "interactive"` to exec room
3. Poll for `org.mxdx.terminal.session` response with `room_id`
4. Accept DM room invitation
5. Create TerminalSocket on DM room
6. Pipe process.stdin -> TerminalSocket.send()
7. Pipe TerminalSocket.onmessage -> process.stdout.write()
8. Handle SIGWINCH -> TerminalSocket.resize()
9. On TerminalSocket close -> restore terminal, exit

### Task E.5 [C]: Add Sender Extraction to WASM

**Files:**
- Modify: `crates/mxdx-core-wasm/src/lib.rs`

The current `collectRoomEvents()` returns events but doesn't include the sender. Interactive sessions need to know WHO sent the command to create the DM with them. Update event collection to include `sender` field in returned JSON.

---

## Phase F: Web Console

> **Epic:** Browser-based management console with xterm.js terminals, launcher dashboard, and direct E2EE connection to Matrix homeserver.
> **Agents:** Tester -> Coder-WASM -> Coder
> **Branch:** `feat/web-console`
> **Completion gate:** Browser loads dashboard, shows launcher list with telemetry, can open interactive terminal sessions via xterm.js over Matrix E2EE. No credentials touch the web server — browser connects directly to Tuwunel.
> **Security:** SRI for static assets (mxdx-web), CSP headers (already in Axum backend)

### Architecture

The web console is a **thin server + fat client**:

```
Browser (fat client)
├── @mxdx/core (WASM, browser target)  ← E2EE, Matrix protocol
├── TerminalSocket (from packages/core) ← Terminal I/O over Matrix
├── xterm.js                            ← Terminal rendering
└── App shell (HTML + JS)               ← Dashboard, auth, session management
    |
    | Matrix events (encrypted, direct to homeserver)
    v
Tuwunel Homeserver(s)

mxdx-web (Axum, thin server)
├── Serves static assets (HTML, JS, WASM, CSS)
├── SRI hashes for all assets
├── CSP headers
└── NO Matrix credentials — stateless
```

**Key insight:** The browser client uses the SAME `@mxdx/core` WASM bindings as the CLI client, just built with `--target web` instead of `--target nodejs`. The TerminalSocket code is shared. This means interactive terminal sessions work identically in browser and CLI.

### Task F.1 [C]: Browser WASM Build Target

**Files:**
- Modify: `crates/mxdx-core-wasm/src/lib.rs` — conditional compilation for browser vs node
- Create: `scripts/build-wasm.sh` — builds both targets

```bash
# Node.js target (existing)
wasm-pack build crates/mxdx-core-wasm --target nodejs --out-dir ../../packages/core/wasm

# Browser target (new)
wasm-pack build crates/mxdx-core-wasm --target web --out-dir ../../packages/web-console/wasm
```

Browser target differences:
- No `fake-indexeddb` polyfill (browser has native IndexedDB)
- No `node:crypto` — use `globalThis.crypto`
- `wasm-pack --target web` generates ES module with `init()` function

### Task F.2 [C]: Web Console Package Scaffold

**Files:**
- Create: `packages/web-console/package.json`
- Create: `packages/web-console/vite.config.js` — Vite for bundling
- Create: `packages/web-console/index.html` — App shell
- Create: `packages/web-console/src/main.js` — Entry point
- Create: `packages/web-console/src/auth.js` — Login form + session management
- Create: `packages/web-console/src/style.css` — Minimal CSS

**`index.html`** — Single-page app shell:
```html
<!DOCTYPE html>
<html>
<head>
  <meta charset="utf-8">
  <meta name="viewport" content="width=device-width, initial-scale=1">
  <title>mxdx Console</title>
  <link rel="stylesheet" href="/src/style.css">
  <link rel="stylesheet" href="node_modules/@xterm/xterm/css/xterm.css">
</head>
<body>
  <div id="app">
    <div id="login" class="screen"></div>
    <div id="dashboard" class="screen" hidden></div>
    <div id="terminal" class="screen" hidden></div>
  </div>
  <script type="module" src="/src/main.js"></script>
</body>
</html>
```

**`main.js`** — Boot sequence:
1. Load WASM: `import init, { WasmMatrixClient } from './wasm/mxdx_core_wasm.js'; await init();`
2. Check for stored session in `localStorage`
3. If no session: show login screen
4. If session: restore client, show dashboard

### Task F.3 [C]: Login & Session Flow

**Files:**
- Create: `packages/web-console/src/auth.js`

Browser login flow:
1. User enters homeserver URL, username, password
2. Call `WasmMatrixClient.login(server, username, password)`
3. Bootstrap cross-signing: `client.bootstrapCrossSigningIfNeeded(password)`
4. Export session: `client.exportSession()` -> store in `localStorage`
5. On reload: `WasmMatrixClient.restoreSession(localStorage.getItem('mxdx-session'))`

No keyring in browser — `localStorage` is the only option. Session includes access_token + device_id.

### Task F.4 [C]: Dashboard View

**Files:**
- Create: `packages/web-console/src/dashboard.js`

Dashboard shows:
1. List of discovered launchers (via `listLauncherSpaces()`)
2. For each launcher: name, hostname, platform, CPU, memory, uptime (from telemetry state event)
3. "Open Terminal" button per launcher
4. Auto-refresh every 10s via `syncOnce()` + re-render

Uses vanilla JS DOM manipulation — no framework. HTMX is for the Axum server-rendered path (future); the browser client is a standalone SPA.

### Task F.5 [C]: Terminal View with xterm.js

**Files:**
- Create: `packages/web-console/src/terminal-view.js`

Integration:
1. Import xterm.js: `import { Terminal } from '@xterm/xterm'`
2. Import TerminalSocket from `@mxdx/core`
3. Create adapter: TerminalSocket needs a Matrix client interface; wrap WasmMatrixClient
4. On "Open Terminal" click:
   a. Send interactive session request to launcher
   b. Wait for session response with DM room_id
   c. Accept DM invitation
   d. Create TerminalSocket(wrappedClient, dmRoomId)
   e. Create xterm.js Terminal, attach to DOM
   f. Wire: `terminal.onData(data => socket.send(data))`
   g. Wire: `socket.onmessage = (e) => terminal.write(new Uint8Array(e.data))`
   h. Wire: `terminal.onResize(({cols, rows}) => socket.resize(cols, rows))`
5. On close: `socket.close()`, remove terminal from DOM

### Task F.6 [C]: Non-Interactive Exec from Dashboard

**Files:**
- Create: `packages/web-console/src/exec-view.js`
- Modify: `packages/web-console/src/dashboard.js`

Add "Run Command" UI to dashboard:
1. Per-launcher command input + "Run" button
2. Send `org.mxdx.command` to exec room (non-interactive, same as CLI `exec`)
3. Poll for `org.mxdx.output` and `org.mxdx.result` events
4. Display output in a scrollable panel (stdout green, stderr red)
5. Show exit code when complete

This enables non-interactive remote exec from the browser, complementing the interactive terminal view.

### Task F.7 [T]: Web Console E2E Tests (Playwright)

**Files:**
- Create: `packages/e2e-tests/tests/web-console.test.js`

Tests (using Playwright against local Tuwunel + mxdx-web):
1. Login page renders, accepts credentials, stores session
2. After login, dashboard shows discovered launcher with telemetry
3. "Run Command" sends non-interactive exec, output displayed
4. "Open Terminal" creates interactive session, xterm.js renders
5. Typing in terminal sends data to launcher, output appears
6. Session persists across page reload (session restore from localStorage)

### Task F.8 [C]: Axum Static Asset Serving with SRI

**Files:**
- Modify: `crates/mxdx-web/src/routes/static_files.rs`

Update the Axum backend to serve the Vite-built output from `packages/web-console/dist/`:
1. Serve `index.html` for all non-API routes (SPA routing)
2. Serve static assets (JS, WASM, CSS) with SRI headers
3. Generate SRI hashes at build time, embed in CSP

The Axum backend remains stateless — it just serves files. All Matrix communication happens directly between browser and homeserver.

### Task F.9 [D]: Documentation

Update MANIFEST.md, design doc, README for the web console.

### Task F.S [S]: Security Review — Browser Auth, CSP, SRI, localStorage

Review:
- Browser WASM E2EE correctness
- Session storage in localStorage (what's exposed?)
- CSP headers block XSS vectors
- SRI hashes on all served assets
- No credentials leak to Axum server
- Cross-origin protections

---

## Dependency Graph

```
Phase A: Room Topology & MSC4362
  A.1T -> A.1C -> A.2C ─┐
               -> A.3C ─┤
  A.S (blocked by A.2C, A.3C)
                         |
  ┌──────────────────────┴──────────────────────┐
  |                                              |
Phase B (blocked by A.2C)     Phase C (blocked by A.3C)
  B.1T->B.1C ─┐                C.1T + D.1C -> C.1C ─┐
  B.2T->B.2C  ├─ parallel      C.2T -> C.2C          ├─ parallel
  B.3T->B.3C  │                C.3C                  ─┘
  B.4C       ─┘                |
  |                            |
  └──────────────┬─────────────┘
                 |
Phase D: E2E Tests & CI
  D.1C (listLauncherSpaces WASM, blocked by A.1C)
  D.2T, D.3T (blocked by A.2C, A.3C)
  D.4CI (blocked by D.2T)
  D.S: Security review (blocked by all B[C], C[C], D.2T, D.3T, A.S)
                 |
Phase E: Interactive Sessions (blocked by D.S)
  E.1C (WASM DM) ─┐
  E.2C (TerminalSocket) ├─> E.3T -> E.3C -> E.4T -> E.4C
  E.S (blocked by all E[C])
                 |
Phase F: Web Console (blocked by E.S)
  F.1C (browser WASM) -> F.3C -> F.4C ─┐
  F.2C (scaffold)                F.5C  ├─> F.8D
  F.7C (Axum)                   F.6C ─┘
  F.S (blocked by all F[C])
                 |
Phase G: Full System E2E (blocked by F.S)
  G.1T CLI non-interactive  ─┐
  G.2T CLI interactive        ├─ parallel
  G.3T Web non-interactive    │
  G.4T Web interactive        │
  G.5T Public server         ─┘
  G.S: Final security audit (blocked by all G[T])
```

**Critical path:** A.1T -> A.1C -> A.2C -> B/C -> D.S -> E -> F -> G

**Security gates:** D.S gates Phase E. E.S gates Phase F. F.S gates Phase G. G.S is the final audit.

**Parallel opportunities:**
- Phase B and Phase C run in parallel after Phase A
- B.1-B.4 are independent of each other
- C.1-C.3 are independent of each other
- D.1C can start as soon as A.1C is done
- E.1C and E.2C can run in parallel
- F.1C, F.2C, F.7C can run in parallel
- F.4C, F.5C, F.6C can run in parallel after F.3C
- G.1T through G.5T can all run in parallel

---

## Phase G: Full System E2E Integration

> **Epic:** Comprehensive end-to-end testing across all clients and session types. This is the final validation that everything works together.
> **Agents:** Tester
> **Branch:** `feat/e2e-integration`
> **Completion gate:** All 4 client-session combinations work E2E. Public server tests pass with both clients. Security Reviewer produces final system audit.

### Test Matrix

| # | Client | Session Type | Transport | Test Infrastructure |
|:---|:---|:---|:---|:---|
| G.1 | CLI (`mxdx-client exec`) | Non-interactive | Matrix E2EE | Local Tuwunel |
| G.2 | CLI (`mxdx-client shell`) | Interactive | Matrix E2EE + DM | Local Tuwunel |
| G.3 | Web console (Playwright) | Non-interactive | Matrix E2EE (browser WASM) | Playwright + Local Tuwunel |
| G.4 | Web console (Playwright) | Interactive | Matrix E2EE + DM + xterm.js | Playwright + Local Tuwunel |
| G.5 | Both clients | Both types | Matrix E2EE | Public server (matrix.org) |

### Task G.1 [T]: CLI Non-Interactive E2E

Full round-trip: client sends `echo`, `date`, multi-line command. Verify stdout, stderr separation, exit codes, timeout handling. Measure latency (command send to first output event).

### Task G.2 [T]: CLI Interactive E2E

Full round-trip: client opens shell, sends keystrokes, receives PTY output. Verify terminal resize, session persistence (tmux), clean session teardown. Test zlib bomb rejection.

### Task G.3 [T]: Web Console Non-Interactive E2E (Playwright)

Playwright test: login, discover launcher, run command from dashboard, verify output display. Tests the full browser WASM + Matrix E2EE path.

### Task G.4 [T]: Web Console Interactive E2E (Playwright)

Playwright test: login, open terminal, type commands in xterm.js, verify output renders. Tests TerminalSocket + xterm.js + browser WASM E2EE path. Verify DM creation, encryption, resize events.

### Task G.5 [T]: Public Server E2E — Both Clients

Against matrix.org with test-credentials.toml: CLI exec, CLI shell (if servers support PTY), web login + dashboard. Room reuse, latency measurement, cross-signing verification. Manual-trigger in CI.

### Task G.S [S]: Final Security Review — Full System Audit

Complete security review covering:
- E2EE on all rooms (exec, logs, DMs)
- MSC4362 encrypted state events working
- Cross-signing verification between client and launcher
- history_visibility = joined on all DMs
- Zlib bomb protection on compressed terminal data
- CSP + SRI on web console assets
- No credentials in localStorage except session token
- Config files mode 0600
- Command allowlist + cwd validation

---

## Out of Scope

Deferred to later plans:

- **Multi-homeserver failover** (Phase 11) — Only first server used
- **Policy agent** (Phase 8) — Rust-native appservice, fail-closed
- **Secrets coordinator** (Phase 9) — age double-encryption
- **Replay protection** — UUID LRU cache with TTL (Phase 8)
- **Native Rust launcher/client** — Second target, shares core logic
- **PWA offline support** — Service worker exists in Axum backend but not wired to SPA
- **Cloudflare Workers deployment** — Designed but not built

---

## Execution Notes

- **WASM rebuild required** after any change to `crates/mxdx-core-wasm/`:
  ```bash
  wasm-pack build crates/mxdx-core-wasm --target nodejs --out-dir ../../packages/core/wasm
  ```
- **Browser WASM** uses different target:
  ```bash
  wasm-pack build crates/mxdx-core-wasm --target web --out-dir ../../packages/web-console/wasm
  ```
- **getrandom v0.3** needs both `features = ["wasm_js"]` in Cargo.toml AND `--cfg getrandom_backend="wasm_js"` in `.cargo/config.toml`
- **matrix.org cross-signing** must be done via Element (OAuth-only). Tests verify state, don't set it up.
- **`serde_wasm_bindgen::to_value`** doesn't work for `serde_json::Value` — return JSON strings and `JSON.parse()` in JS.
- **TerminalSocket** works in both Node.js and browser (uses `CompressionStream`/`DecompressionStream` in browser, `node:zlib` in Node.js).
- **PTY in Node.js:** Use `node-pty` for cross-platform PTY allocation. Wrap in tmux for session persistence.
- **No framework for web console:** Vanilla JS + xterm.js + Vite. The console is small enough that a framework adds complexity without value.
