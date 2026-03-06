# mxdx Launcher & Client Runtime Design

**Date:** 2026-03-06
**Status:** Approved
**Authors:** Liam Helmer, Claude

---

## 1. Overview

Build the real mxdx launcher and client as npm-distributed packages. Both are Rust compiled to WASM (`wasm32-unknown-unknown`) via `wasm-pack`, running in Node.js. The launcher handles registration/login, credential storage, room discovery/creation, and a sync loop listening for commands. The client provides interactive fleet management — command execution, terminal sessions, telemetry, and secret management.

### Packages

| Package | Purpose | Entry point |
|---------|---------|-------------|
| `@mxdx/core` | Shared Rust→WASM: MatrixClient, event types, room topology, policy | Library only |
| `@mxdx/launcher` | Launcher runtime | `npx mxdx launcher` |
| `@mxdx/client` | Interactive client | `npx mxdx client` |

---

## 2. Architecture

```
┌─ Rust (compiled to WASM) ─────────────────────────┐
│  matrix-sdk (js + indexeddb features)              │
│  mxdx event schema / validation / policy           │
│  Command validation, telemetry collection          │
└────────────────────┬───────────────────────────────┘
                     │ wasm-bindgen JS bindings
┌─ Node.js glue ────┴───────────────────────────────┐
│  child_process (command execution, tmux)            │
│  Credential storage (keyring → encrypted file)     │
│  Interactive prompts (inquirer/prompts)             │
│  CLI argument parsing                              │
│  xterm.js terminal I/O (client side)               │
└────────────────────────────────────────────────────┘
```

**Key split**: Matrix protocol + E2EE + business logic lives in Rust/WASM. System I/O (processes, files, keychain, TTY) lives in JS/Node.js and is called from Rust via `wasm-bindgen` exports, or orchestrated by the JS entrypoint.

### WASM Compilation

The `matrix-sdk` crate supports `wasm32-unknown-unknown` with the `js` feature flag (enables JavaScript API usage on WASM) and `indexeddb` for persistent storage. The full Matrix client (not just crypto) runs in WASM.

```toml
[target.'cfg(target_family = "wasm")'.dependencies]
matrix-sdk = { version = "0.16", default-features = false, features = ["indexeddb", "e2e-encryption", "js"] }
```

---

## 3. Credential Storage (Layered)

1. **OS keychain** via `keytar` npm package (macOS Keychain, Linux Secret Service, Windows Credential Manager)
2. **Encrypted file** fallback at `~/.config/mxdx/credentials.enc`, encrypted with machine-derived key (hostname + user UID hashed)
3. **What's stored**: server URL, username, device_id, access_token (not password — after initial login, we use the token)

Try keychain first. If unavailable (headless server, container), fall back to encrypted file.

---

## 4. Launcher Runtime (`@mxdx/launcher`)

### Two Modes

**Interactive onboarding** (`npx mxdx launcher`):
```
Welcome to mxdx launcher setup.

Select a Matrix server:
  > matrix.org
    mxdx.dev
    Other (enter URL)

Username: belthanior
Password: ********
Email (for account recovery): user@example.com

Registering account... done
Storing credentials... done
Creating launcher rooms... done
Launcher is online. Listening for commands.
```

Server list is hardcoded with an "Other" option for custom URLs.

**Automated** (no interactivity):
```bash
npx mxdx launcher \
  --username $(hostname) \
  --servers s1.mxdx.dev,s2.mxdx.dev \
  --registration-token ${REG_TOKEN} \
  --admin-user @admin:s1.mxdx.dev,@admin:s2.mxdx.dev \
  --allowed-commands bash,sudo
```

### Startup Sequence

1. Check for `~/.config/mxdx/launcher.toml`
2. If exists: load and start
3. If not: run onboarding (interactive or from CLI args) → write config → start
4. Load credentials from keychain/encrypted file
5. `MatrixClient::login_and_connect` (or `register_and_connect` if registration token provided)
6. Store credentials in keychain/encrypted file
7. `get_or_create_launcher_space(launcher_id)` — find existing rooms by topic or create new ones
8. Invite admin users (if specified) to the space at power level 100
9. Post `org.mxdx.host_telemetry` state event to status room
10. Enter sync loop:
    - Listen for `org.mxdx.command` events in exec room
    - Validate command against capabilities config (allowlist/denylist)
    - Execute via `child_process` (Node.js side)
    - Stream `org.mxdx.output` events back as stdout/stderr lines arrive
    - Send `org.mxdx.result` event when command completes
    - Periodically update telemetry state event
11. Handle `org.mxdx.terminal.open` — spawn tmux session, bridge PTY I/O to Matrix events

### CLI Flags

```
npx mxdx launcher [OPTIONS]

Options:
  --username <name>           Username (default: hostname)
  --servers <url,...>         Comma-separated server URLs
  --registration-token <tok>  Auto-register with this token
  --admin-user <mxid,...>     Admin users to invite at PL100
  --allowed-commands <cmd,..> Command allowlist (default: none)
  --allowed-cwd <path,...>    Allowed working directories
  --config <path>             Config file path
  --telemetry <full|summary>  Telemetry detail level
  --max-sessions <n>          Max concurrent sessions (default: 5)
```

---

## 5. Client Runtime (`@mxdx/client`)

### Interactive REPL Mode

```
npx mxdx client
Connecting to matrix.org as @liamhelmer... done
Discovering launchers... found 3 online.

mxdx> launchers
  NAME              SERVER          STATUS    CPU   MEM
  belthanior        matrix.org      online    12%   4.2G/32G
  deploy-prod-01    s1.mxdx.dev     online    67%   14G/64G
  ci-runner-03      s2.mxdx.dev     offline   -     -

mxdx> exec belthanior echo hello world
hello world
[exit 0, 0.8s]

mxdx> terminal belthanior
Connecting terminal session... done
(interactive tmux session via xterm.js-style I/O)
^] to detach

mxdx> telemetry belthanior
  Hostname:    belthanior
  OS:          Ubuntu 24.04
  CPU:         AMD Ryzen 9 (16 cores) - 12%
  Memory:      4.2G / 32G

mxdx> secret request deploy.token --from belthanior
Requesting secret... granted.
deploy.token = tok_abc123... (expires in 1h)
```

### Automated Mode (pipe-friendly)

```bash
# Single command, exit after result
npx mxdx client exec belthanior -- echo hello

# Stream output, exit with command's exit code
npx mxdx client exec belthanior -- cargo test 2>&1 | tee build.log

# JSON output for scripting
npx mxdx client launchers --format json
```

### Feature Priority

| Priority | Feature | Commands |
|----------|---------|----------|
| P0 | Connect + discover | `launchers`, `status <name>` |
| P1 | Send commands + output | `exec <launcher> <cmd>` |
| P2 | Interactive terminal | `terminal <launcher>` |
| P3 | Telemetry/fleet view | `telemetry <launcher>` |
| P4 | Secret management | `secret request <scope>` |

### CLI

```
npx mxdx client [COMMAND] [OPTIONS]

Commands:
  (none)                      Interactive REPL mode
  launchers                   List discovered launchers
  exec <launcher> -- <cmd>    Execute command on launcher
  terminal <launcher>         Open interactive terminal
  telemetry <launcher>        Show host telemetry
  secret request <scope>      Request a secret

Options:
  --server <url>              Matrix server (default: from config)
  --username <name>           Username
  --format <text|json>        Output format (default: text)
  --config <path>             Config file path
```

Same config-generation behavior as launcher: first run without config → onboarding → write `~/.config/mxdx/client.toml` → start.

---

## 6. E2E Tests

Tests use the **real npm packages** against a local Tuwunel instance.

### Test Architecture

```
Test Process (Node.js)
  |
  |-- Start Tuwunel (local homeserver)
  |-- Spawn: npx mxdx launcher --username test-launcher \
  |          --servers http://localhost:PORT \
  |          --registration-token mxdx-test-token \
  |          --allowed-commands echo,seq,cat
  |
  |-- Wait for launcher to be online (poll for room topology)
  |
  |-- Spawn: npx mxdx client exec test-launcher -- echo hello
  |          --server http://localhost:PORT \
  |          --username test-client --format json
  |
  |-- Assert: stdout contains "hello", exit code 0
```

### Test Cases

| Test | What it validates |
|------|-------------------|
| `launcher_onboarding` | Registers, creates config, creates rooms, goes online |
| `launcher_reconnect` | Finds existing rooms on second start (no new rooms) |
| `client_discovers_launcher` | `launchers` command lists the test launcher as online |
| `command_round_trip` | `exec echo hello` returns output with correct exit code |
| `streaming_output` | `exec seq 1 100` receives all 100 lines in order |
| `terminal_session` | Opens terminal, sends keystrokes, receives PTY output |
| `telemetry_visible` | `telemetry` command shows host info from launcher |
| `rate_limit_resilience` | Launcher handles 429 gracefully (timeout, clear error) |
| `credential_persistence` | Stops, restarts, reconnects without re-prompting |

### Public Server Tests

Optional tests gated on `test-credentials.toml`:
- Only run manually (`npm test -- --public-server`)
- Single test does the full round-trip in one session (minimal API calls)
- 5-second delays between room operations to respect rate limits
- Leaves rooms on cleanup
- Separate from Tuwunel E2E suite (which is the CI gate)

---

## 7. Build & Packaging

### Directory Structure

```
crates/mxdx-core-wasm/        (new crate, lib target)
  Cargo.toml                   target wasm32-unknown-unknown
  src/lib.rs                   Re-exports matrix-sdk + mxdx types with wasm-bindgen

packages/core/                 (npm: @mxdx/core)
  package.json
  index.js                     Re-exports wasm-pack output
  index.d.ts                   TypeScript definitions

packages/launcher/             (npm: @mxdx/launcher)
  package.json
  bin/mxdx-launcher.js         CLI entrypoint
  src/onboarding.js            Interactive setup flow
  src/credentials.js           Keychain + encrypted file storage
  src/process-bridge.js        child_process wrapper for command execution
  src/config.js                Config file read/write

packages/client/               (npm: @mxdx/client)
  package.json
  bin/mxdx-client.js           CLI entrypoint
  src/repl.js                  Interactive REPL
  src/terminal.js              xterm.js terminal session bridge
  src/formatters.js            Output formatting (text, JSON)

packages/e2e-tests/            (test package, not published)
  package.json
  tests/                       E2E test suite
```

### Build Commands

```bash
# Build WASM core
cd crates/mxdx-core-wasm && wasm-pack build --target nodejs --out-dir ../../packages/core/wasm

# Run launcher locally
cd packages/launcher && node bin/mxdx-launcher.js

# Run E2E tests (starts Tuwunel automatically)
cd packages/e2e-tests && npm test

# Run public server tests (requires test-credentials.toml)
cd packages/e2e-tests && npm test -- --public-server
```

### npm Distribution

```bash
npx @mxdx/launcher    # Zero-install launcher
npx @mxdx/client      # Zero-install client
```

---

## 8. Dependencies

### Rust/WASM (crates/mxdx-core-wasm)

- `matrix-sdk` with features: `js`, `indexeddb`, `e2e-encryption`
- `wasm-bindgen` for JS interop
- `serde`, `serde_json` for serialization
- Existing mxdx crates: `mxdx-types`, policy engine, telemetry

### Node.js (packages/*)

- `keytar` — OS keychain access
- `inquirer` or `prompts` — interactive prompts
- `commander` or `yargs` — CLI argument parsing
- `xterm-headless` + `node-pty` — terminal session I/O (client P2)
- `chalk` — colored output

---

## 9. Migration from Current Architecture

The existing Rust `mxdx-matrix` crate (wrapping `matrix-sdk` with `sqlite` store) continues to work for native builds and Tuwunel-based tests. The new `mxdx-core-wasm` crate shares the same `mxdx-types` and event schemas but uses `indexeddb` instead of `sqlite` and compiles to WASM.

The current native E2E tests (`e2e_full_system.rs`) remain as a fast CI gate. The new npm-based E2E tests (`packages/e2e-tests`) test the real user-facing binaries.
