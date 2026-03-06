# mxdx Launcher & Client Runtime Implementation Plan

> **For Claude:** REQUIRED SUB-SKILL: Use superpowers:executing-plans to implement this plan task-by-task.

**Goal:** Build the real mxdx launcher and client as npm-distributed WASM packages that handle registration, credential storage, room discovery, command execution, and interactive terminal sessions over Matrix.

**Architecture:** Rust core (matrix-sdk + mxdx types) compiled to WASM via wasm-pack, running in Node.js. JS glue handles system I/O (child_process, keychain, prompts). Two npm packages: `@mxdx/launcher` and `@mxdx/client`, sharing a `@mxdx/core` WASM library.

**Tech Stack:** Rust + wasm-pack + wasm-bindgen, matrix-sdk (js + indexeddb features), Node.js 22, npm workspaces, keytar, commander, inquirer.

**Design doc:** `docs/plans/2026-03-06-launcher-client-runtime-design.md`

---

## Phase 0: WASM Proof-of-Concept (highest risk)

This phase proves that `matrix-sdk` compiles to WASM and can connect to a Matrix server from Node.js. If this fails, the entire design needs to pivot.

### Task 0.1: Install WASM tooling

**Files:**
- None (system setup)

**Step 1: Install wasm-pack and WASM target**

Run:
```bash
rustup target add wasm32-unknown-unknown
cargo install wasm-pack
```

Expected: Both install successfully.

**Step 2: Verify**

Run:
```bash
rustup target list --installed | grep wasm32-unknown-unknown
wasm-pack --version
```

Expected: Target listed, wasm-pack version printed.

**Step 3: Commit (nothing to commit — tooling only)**

---

### Task 0.2: Create mxdx-core-wasm crate

**Files:**
- Create: `crates/mxdx-core-wasm/Cargo.toml`
- Create: `crates/mxdx-core-wasm/src/lib.rs`
- Modify: `Cargo.toml` (workspace members)

**Step 1: Create the crate Cargo.toml**

```toml
[package]
name = "mxdx-core-wasm"
version = "0.1.0"
edition = "2021"

[lib]
crate-type = ["cdylib", "rlib"]

[dependencies]
wasm-bindgen = "0.2"
wasm-bindgen-futures = "0.4"
serde = { version = "1", features = ["derive"] }
serde_json = "1"
serde-wasm-bindgen = "0.6"
js-sys = "0.3"
web-sys = "0.3"
console_error_panic_hook = "0.1"

matrix-sdk = { version = "0.16", default-features = false, features = ["e2e-encryption", "js", "indexeddb"] }

mxdx-types = { path = "../mxdx-types" }

[dev-dependencies]
wasm-bindgen-test = "0.3"
```

**Step 2: Create minimal lib.rs with a wasm-bindgen-exported function**

```rust
use wasm_bindgen::prelude::*;

#[wasm_bindgen(start)]
pub fn init() {
    console_error_panic_hook::set_once();
}

/// Smoke test: returns the matrix-sdk version string to prove it compiled.
#[wasm_bindgen]
pub fn sdk_version() -> String {
    "matrix-sdk-0.16-wasm".to_string()
}
```

**Step 3: Add to workspace**

Add `"crates/mxdx-core-wasm"` to the `members` list in the root `Cargo.toml`.

**Step 4: Verify native compilation**

Run: `cargo check -p mxdx-core-wasm`
Expected: Compiles (native target, not WASM yet — just checking deps resolve).

**Step 5: Commit**

```bash
git add crates/mxdx-core-wasm/ Cargo.toml
git commit -m "feat: add mxdx-core-wasm crate scaffold"
```

---

### Task 0.3: WASM compilation proof

**Files:**
- Modify: `crates/mxdx-core-wasm/src/lib.rs`

**Step 1: Build with wasm-pack**

Run:
```bash
cd crates/mxdx-core-wasm
wasm-pack build --target nodejs --out-dir ../../packages/core/wasm
```

Expected: Builds successfully, produces `packages/core/wasm/` with `.wasm`, `.js`, `.d.ts` files.

If this fails, investigate the error. Common issues:
- `matrix-sdk` may need specific feature combinations for WASM
- `getrandom` needs `js` feature on WASM targets
- Some deps may need `[target.'cfg(target_family = "wasm")'.dependencies]` overrides

**Step 2: Test from Node.js**

Create a temporary test script:
```bash
node -e "const m = require('./packages/core/wasm'); console.log(m.sdk_version())"
```

Expected: Prints `matrix-sdk-0.16-wasm`.

**Step 3: Commit**

```bash
git add packages/core/wasm/ crates/mxdx-core-wasm/
git commit -m "feat: prove matrix-sdk compiles to WASM and loads in Node.js"
```

---

### Task 0.4: WASM MatrixClient — login and sync

**Files:**
- Modify: `crates/mxdx-core-wasm/src/lib.rs`

**Step 1: Export a MatrixClient wrapper via wasm-bindgen**

```rust
use wasm_bindgen::prelude::*;
use matrix_sdk::{
    config::SyncSettings,
    Client,
};
use std::time::Duration;

#[wasm_bindgen]
pub struct WasmMatrixClient {
    client: Client,
}

#[wasm_bindgen]
impl WasmMatrixClient {
    /// Login to a Matrix server. server_name can be "matrix.org" or a full URL.
    #[wasm_bindgen(constructor)]
    pub async fn new(server_name: &str, username: &str, password: &str) -> Result<WasmMatrixClient, JsValue> {
        console_error_panic_hook::set_once();

        let builder = Client::builder();
        let client = if server_name.contains("://") {
            builder.homeserver_url(server_name)
        } else {
            builder.server_name_or_homeserver_url(server_name)
        }
        .build()
        .await
        .map_err(|e| JsValue::from_str(&e.to_string()))?;

        client
            .matrix_auth()
            .login_username(username, password)
            .initial_device_display_name("mxdx-wasm")
            .await
            .map_err(|e| JsValue::from_str(&e.to_string()))?;

        // Initial sync to upload device keys
        client
            .sync_once(SyncSettings::default().timeout(Duration::from_secs(5)))
            .await
            .map_err(|e| JsValue::from_str(&e.to_string()))?;

        Ok(WasmMatrixClient { client })
    }

    /// Check if logged in.
    pub fn is_logged_in(&self) -> bool {
        self.client.user_id().is_some()
    }

    /// Get the user ID.
    pub fn user_id(&self) -> Option<String> {
        self.client.user_id().map(|u| u.to_string())
    }
}
```

**Step 2: Build with wasm-pack**

Run:
```bash
cd crates/mxdx-core-wasm
wasm-pack build --target nodejs --out-dir ../../packages/core/wasm
```

**Step 3: Test login against Tuwunel from Node.js**

Write a test script `packages/core/test-login.mjs`:
```javascript
import { WasmMatrixClient } from './wasm/mxdx_core_wasm.js';

// This requires a running Tuwunel instance
const client = await new WasmMatrixClient("http://localhost:PORT", "testuser", "testpass");
console.log("Logged in:", client.is_logged_in());
console.log("User ID:", client.user_id());
```

Expected: Prints `Logged in: true` and the user ID.

**Step 4: Commit**

```bash
git add crates/mxdx-core-wasm/ packages/core/
git commit -m "feat: WASM MatrixClient with login and sync"
```

---

## Phase 1: npm Workspace & Credential Storage

### Task 1.1: Create npm workspace structure

**Files:**
- Create: `package.json` (root)
- Create: `packages/core/package.json`
- Create: `packages/launcher/package.json`
- Create: `packages/launcher/bin/mxdx-launcher.js`
- Create: `packages/client/package.json`
- Create: `packages/client/bin/mxdx-client.js`
- Create: `packages/e2e-tests/package.json`

**Step 1: Create root package.json**

```json
{
  "private": true,
  "workspaces": [
    "packages/core",
    "packages/launcher",
    "packages/client",
    "packages/e2e-tests"
  ]
}
```

**Step 2: Create packages/core/package.json**

```json
{
  "name": "@mxdx/core",
  "version": "0.1.0",
  "private": true,
  "main": "index.js",
  "type": "module",
  "files": ["wasm/", "index.js", "index.d.ts"]
}
```

Create `packages/core/index.js`:
```javascript
export * from './wasm/mxdx_core_wasm.js';
```

**Step 3: Create packages/launcher/package.json**

```json
{
  "name": "@mxdx/launcher",
  "version": "0.1.0",
  "private": true,
  "type": "module",
  "bin": {
    "mxdx-launcher": "bin/mxdx-launcher.js"
  },
  "dependencies": {
    "@mxdx/core": "workspace:*",
    "commander": "^12.0.0",
    "inquirer": "^9.0.0",
    "keytar": "^7.9.0"
  }
}
```

Create `packages/launcher/bin/mxdx-launcher.js`:
```javascript
#!/usr/bin/env node
console.log("mxdx-launcher: not yet implemented");
process.exit(0);
```

**Step 4: Create packages/client/package.json**

```json
{
  "name": "@mxdx/client",
  "version": "0.1.0",
  "private": true,
  "type": "module",
  "bin": {
    "mxdx-client": "bin/mxdx-client.js"
  },
  "dependencies": {
    "@mxdx/core": "workspace:*",
    "commander": "^12.0.0",
    "inquirer": "^9.0.0",
    "keytar": "^7.9.0",
    "chalk": "^5.0.0"
  }
}
```

Create `packages/client/bin/mxdx-client.js`:
```javascript
#!/usr/bin/env node
console.log("mxdx-client: not yet implemented");
process.exit(0);
```

**Step 5: Create packages/e2e-tests/package.json**

```json
{
  "name": "@mxdx/e2e-tests",
  "version": "0.1.0",
  "private": true,
  "type": "module",
  "scripts": {
    "test": "node --test tests/"
  },
  "dependencies": {
    "@mxdx/core": "workspace:*"
  }
}
```

**Step 6: npm install and verify**

Run:
```bash
npm install
npx mxdx-launcher
npx mxdx-client
```

Expected: Both print their placeholder messages and exit 0.

**Step 7: Commit**

```bash
git add package.json packages/
git commit -m "feat: npm workspace with launcher, client, core, and e2e-tests packages"
```

---

### Task 1.2: Credential storage module

**Files:**
- Create: `packages/launcher/src/credentials.js`
- Create: `packages/launcher/src/credentials.test.js`

**Step 1: Write the test**

```javascript
import { describe, it, beforeEach, afterEach } from 'node:test';
import assert from 'node:assert';
import fs from 'node:fs';
import path from 'node:path';
import os from 'node:os';
import { CredentialStore } from '../src/credentials.js';

describe('CredentialStore', () => {
  let tmpDir;
  let store;

  beforeEach(() => {
    tmpDir = fs.mkdtempSync(path.join(os.tmpdir(), 'mxdx-cred-test-'));
    store = new CredentialStore({ configDir: tmpDir, useKeychain: false });
  });

  afterEach(() => {
    fs.rmSync(tmpDir, { recursive: true });
  });

  it('saves and loads credentials', async () => {
    await store.save({
      serverUrl: 'https://matrix.org',
      username: 'testuser',
      accessToken: 'syt_abc123',
      deviceId: 'ABCDEF',
    });
    const loaded = await store.load();
    assert.strictEqual(loaded.serverUrl, 'https://matrix.org');
    assert.strictEqual(loaded.username, 'testuser');
    assert.strictEqual(loaded.accessToken, 'syt_abc123');
    assert.strictEqual(loaded.deviceId, 'ABCDEF');
  });

  it('returns null when no credentials exist', async () => {
    const loaded = await store.load();
    assert.strictEqual(loaded, null);
  });

  it('encrypts credentials on disk', async () => {
    await store.save({
      serverUrl: 'https://matrix.org',
      username: 'testuser',
      accessToken: 'syt_secret',
      deviceId: 'DEV1',
    });
    const raw = fs.readFileSync(path.join(tmpDir, 'credentials.enc'), 'utf8');
    assert.ok(!raw.includes('syt_secret'), 'Credentials should be encrypted on disk');
  });
});
```

**Step 2: Run test to verify it fails**

Run: `cd packages/launcher && node --test src/credentials.test.js`
Expected: FAIL — module not found.

**Step 3: Implement CredentialStore**

```javascript
import fs from 'node:fs';
import path from 'node:path';
import crypto from 'node:crypto';
import os from 'node:os';

function deriveKey() {
  const material = `${os.hostname()}:${os.userInfo().uid}:mxdx-credential-store`;
  return crypto.createHash('sha256').update(material).digest();
}

function encrypt(plaintext, key) {
  const iv = crypto.randomBytes(16);
  const cipher = crypto.createCipheriv('aes-256-gcm', key, iv);
  const encrypted = Buffer.concat([cipher.update(plaintext, 'utf8'), cipher.final()]);
  const tag = cipher.getAuthTag();
  return Buffer.concat([iv, tag, encrypted]).toString('base64');
}

function decrypt(ciphertext, key) {
  const buf = Buffer.from(ciphertext, 'base64');
  const iv = buf.subarray(0, 16);
  const tag = buf.subarray(16, 32);
  const encrypted = buf.subarray(32);
  const decipher = crypto.createDecipheriv('aes-256-gcm', key, iv);
  decipher.setAuthTag(tag);
  return decipher.update(encrypted, null, 'utf8') + decipher.final('utf8');
}

export class CredentialStore {
  #configDir;
  #useKeychain;
  #key;

  constructor({ configDir, useKeychain = true } = {}) {
    this.#configDir = configDir || path.join(os.homedir(), '.config', 'mxdx');
    this.#useKeychain = useKeychain;
    this.#key = deriveKey();
  }

  async save(credentials) {
    // Try keychain first
    if (this.#useKeychain) {
      try {
        const keytar = await import('keytar');
        await keytar.setPassword('mxdx', 'credentials', JSON.stringify(credentials));
        return;
      } catch {
        // keytar not available, fall through to file
      }
    }

    // File-based encrypted storage
    fs.mkdirSync(this.#configDir, { recursive: true, mode: 0o700 });
    const filePath = path.join(this.#configDir, 'credentials.enc');
    const encrypted = encrypt(JSON.stringify(credentials), this.#key);
    fs.writeFileSync(filePath, encrypted, { mode: 0o600 });
  }

  async load() {
    // Try keychain first
    if (this.#useKeychain) {
      try {
        const keytar = await import('keytar');
        const stored = await keytar.getPassword('mxdx', 'credentials');
        if (stored) return JSON.parse(stored);
      } catch {
        // keytar not available, fall through to file
      }
    }

    // File-based
    const filePath = path.join(this.#configDir, 'credentials.enc');
    if (!fs.existsSync(filePath)) return null;
    const encrypted = fs.readFileSync(filePath, 'utf8');
    return JSON.parse(decrypt(encrypted, this.#key));
  }
}
```

**Step 4: Run test to verify it passes**

Run: `cd packages/launcher && node --test src/credentials.test.js`
Expected: 3 tests PASS.

**Step 5: Commit**

```bash
git add packages/launcher/src/credentials.js packages/launcher/src/credentials.test.js
git commit -m "feat: layered credential storage (keychain + encrypted file)"
```

---

### Task 1.3: Config file module

**Files:**
- Create: `packages/launcher/src/config.js`
- Create: `packages/launcher/src/config.test.js`

**Step 1: Write the test**

```javascript
import { describe, it, beforeEach, afterEach } from 'node:test';
import assert from 'node:assert';
import fs from 'node:fs';
import path from 'node:path';
import os from 'node:os';
import { LauncherConfig } from '../src/config.js';

describe('LauncherConfig', () => {
  let tmpDir;

  beforeEach(() => {
    tmpDir = fs.mkdtempSync(path.join(os.tmpdir(), 'mxdx-config-test-'));
  });

  afterEach(() => {
    fs.rmSync(tmpDir, { recursive: true });
  });

  it('creates config from CLI args', () => {
    const config = LauncherConfig.fromArgs({
      username: 'belthanior',
      servers: 'matrix.org,mxdx.dev',
      allowedCommands: 'echo,cat',
      telemetry: 'full',
      maxSessions: 10,
    });
    assert.strictEqual(config.username, 'belthanior');
    assert.deepStrictEqual(config.servers, ['matrix.org', 'mxdx.dev']);
    assert.deepStrictEqual(config.allowedCommands, ['echo', 'cat']);
  });

  it('saves and loads TOML config', () => {
    const configPath = path.join(tmpDir, 'launcher.toml');
    const config = LauncherConfig.fromArgs({
      username: 'test-host',
      servers: 'matrix.org',
      allowedCommands: 'echo',
    });
    config.save(configPath);
    assert.ok(fs.existsSync(configPath));

    const loaded = LauncherConfig.load(configPath);
    assert.strictEqual(loaded.username, 'test-host');
    assert.deepStrictEqual(loaded.servers, ['matrix.org']);
  });

  it('returns null when config file does not exist', () => {
    const loaded = LauncherConfig.load(path.join(tmpDir, 'nonexistent.toml'));
    assert.strictEqual(loaded, null);
  });
});
```

**Step 2: Run test to verify it fails**

Run: `cd packages/launcher && node --test src/config.test.js`
Expected: FAIL.

**Step 3: Implement LauncherConfig**

The config module reads/writes TOML. Use a lightweight TOML library (`smol-toml` — pure JS, no native deps).

Add to `packages/launcher/package.json` dependencies: `"smol-toml": "^1.0.0"`

```javascript
import fs from 'node:fs';
import os from 'node:os';
import path from 'node:path';
import * as TOML from 'smol-toml';

export class LauncherConfig {
  constructor({
    username,
    servers = [],
    allowedCommands = [],
    allowedCwd = ['/tmp'],
    telemetry = 'full',
    maxSessions = 5,
    adminUsers = [],
    registrationToken = null,
  } = {}) {
    this.username = username || os.hostname();
    this.servers = servers;
    this.allowedCommands = allowedCommands;
    this.allowedCwd = allowedCwd;
    this.telemetry = telemetry;
    this.maxSessions = maxSessions;
    this.adminUsers = adminUsers;
    this.registrationToken = registrationToken;
  }

  static fromArgs(args) {
    return new LauncherConfig({
      username: args.username,
      servers: args.servers ? args.servers.split(',') : [],
      allowedCommands: args.allowedCommands ? args.allowedCommands.split(',') : [],
      allowedCwd: args.allowedCwd ? args.allowedCwd.split(',') : ['/tmp'],
      telemetry: args.telemetry || 'full',
      maxSessions: args.maxSessions ? parseInt(args.maxSessions, 10) : 5,
      adminUsers: args.adminUser ? args.adminUser.split(',') : [],
      registrationToken: args.registrationToken || null,
    });
  }

  save(filePath) {
    const dir = path.dirname(filePath);
    fs.mkdirSync(dir, { recursive: true, mode: 0o700 });
    const toml = TOML.stringify({
      launcher: {
        username: this.username,
        servers: this.servers,
        allowed_commands: this.allowedCommands,
        allowed_cwd: this.allowedCwd,
        telemetry: this.telemetry,
        max_sessions: this.maxSessions,
        admin_users: this.adminUsers,
      },
    });
    fs.writeFileSync(filePath, toml, { mode: 0o600 });
  }

  static load(filePath) {
    if (!fs.existsSync(filePath)) return null;
    const content = fs.readFileSync(filePath, 'utf8');
    const parsed = TOML.parse(content);
    const l = parsed.launcher || {};
    return new LauncherConfig({
      username: l.username,
      servers: l.servers || [],
      allowedCommands: l.allowed_commands || [],
      allowedCwd: l.allowed_cwd || ['/tmp'],
      telemetry: l.telemetry || 'full',
      maxSessions: l.max_sessions || 5,
      adminUsers: l.admin_users || [],
    });
  }

  static defaultPath() {
    return path.join(os.homedir(), '.config', 'mxdx', 'launcher.toml');
  }
}
```

**Step 4: Run tests**

Run: `cd packages/launcher && npm install && node --test src/config.test.js`
Expected: 3 tests PASS.

**Step 5: Commit**

```bash
git add packages/launcher/
git commit -m "feat: launcher config read/write with TOML"
```

---

## Phase 2: Launcher Runtime

### Task 2.1: Interactive onboarding flow

**Files:**
- Create: `packages/launcher/src/onboarding.js`

Implement the interactive server selection, username/password prompts using `inquirer`. Writes config file on completion.

Server list: `["matrix.org", "mxdx.dev"]` + "Other (enter URL)" option.

### Task 2.2: CLI entrypoint with commander

**Files:**
- Modify: `packages/launcher/bin/mxdx-launcher.js`

Wire up `commander` with all CLI flags from the design. Logic:
1. Parse args
2. Check for config file (--config or default path)
3. If no config + no args → run onboarding
4. If args → create config from args, save it
5. If config exists → load it
6. Start launcher runtime

### Task 2.3: Launcher sync loop

**Files:**
- Create: `packages/launcher/src/runtime.js`

The core event loop:
1. Connect to Matrix via `WasmMatrixClient`
2. Call `get_or_create_launcher_space` (need to export this from WASM)
3. Enter sync loop — listen for `org.mxdx.command` events
4. On command: validate → execute via child_process → send output events

### Task 2.4: Process bridge (child_process wrapper)

**Files:**
- Create: `packages/launcher/src/process-bridge.js`
- Create: `packages/launcher/src/process-bridge.test.js`

Wraps Node.js `child_process.spawn` with:
- stdout/stderr line-by-line streaming
- Timeout enforcement
- Exit code capture
- Conversion to `org.mxdx.output` / `org.mxdx.result` event payloads

### Task 2.5: Export room topology + event sending from WASM

**Files:**
- Modify: `crates/mxdx-core-wasm/src/lib.rs`

Export via `wasm_bindgen`:
- `create_launcher_space(launcher_id)` → returns room IDs as JSON
- `find_launcher_space(launcher_id)` → returns room IDs or null
- `get_or_create_launcher_space(launcher_id)` → idempotent
- `send_event(room_id, event_type, content_json)` → sends Matrix event
- `sync_once()` → single sync cycle
- `start_sync(callback)` → continuous sync with JS callback for events

---

## Phase 3: Client Runtime

### Task 3.1: Client config + onboarding

**Files:**
- Create: `packages/client/src/config.js`
- Create: `packages/client/src/onboarding.js`

Same pattern as launcher — config file at `~/.config/mxdx/client.toml`, onboarding for first run.

### Task 3.2: Launcher discovery

**Files:**
- Create: `packages/client/src/discovery.js`

Connect to Matrix, sync, scan rooms for `org.mxdx.launcher.space:*` topics. Build list of online launchers with their room IDs.

### Task 3.3: CLI entrypoint + REPL

**Files:**
- Modify: `packages/client/bin/mxdx-client.js`
- Create: `packages/client/src/repl.js`

Commands: `launchers`, `exec <launcher> <cmd>`, `terminal <launcher>`, `telemetry <launcher>`, `secret request <scope>`.

### Task 3.4: Command execution (exec)

**Files:**
- Create: `packages/client/src/exec.js`

Send `org.mxdx.command` event to launcher's exec room, sync and stream `org.mxdx.output` events to stdout, print `org.mxdx.result` summary.

### Task 3.5: Terminal session (P2)

**Files:**
- Create: `packages/client/src/terminal.js`

Open `org.mxdx.terminal.open` session, bridge stdin/stdout to Matrix events. Uses `xterm-headless` for terminal emulation in Node.js.

### Task 3.6: Telemetry view (P3)

**Files:**
- Create: `packages/client/src/telemetry.js`

Read `org.mxdx.host_telemetry` state event from launcher's status room, format and display.

### Task 3.7: Secret request (P4)

**Files:**
- Create: `packages/client/src/secrets.js`

Send `org.mxdx.secret.request` DM to secrets coordinator, receive response.

---

## Phase 4: E2E Tests

### Task 4.1: Tuwunel test harness for Node.js

**Files:**
- Create: `packages/e2e-tests/src/tuwunel.js`

Port the Rust `TuwunelInstance` helper to JS:
- Start Tuwunel subprocess with temp data dir
- Wait for health check
- Expose port and registration token
- Stop on cleanup

### Task 4.2: Launcher onboarding E2E test

**Files:**
- Create: `packages/e2e-tests/tests/launcher-onboarding.test.js`

Start Tuwunel → spawn `npx mxdx-launcher` with --servers/--username/--registration-token → verify it registers, creates rooms, goes online.

### Task 4.3: Command round-trip E2E test

**Files:**
- Create: `packages/e2e-tests/tests/command-round-trip.test.js`

Start Tuwunel → start launcher → spawn `npx mxdx-client exec test-launcher -- echo hello` → assert stdout = "hello", exit 0.

### Task 4.4: Launcher reconnect E2E test

**Files:**
- Create: `packages/e2e-tests/tests/launcher-reconnect.test.js`

Start launcher → stop it → restart it → verify it finds existing rooms (no new rooms created).

### Task 4.5: Public server E2E test (optional)

**Files:**
- Create: `packages/e2e-tests/tests/public-server.test.js`

Gated on `test-credentials.toml`. Single round-trip test with 5s delays between operations.

---

## Dependency Chain

```
Phase 0: 0.1 → 0.2 → 0.3 → 0.4
Phase 1: 1.1 → 1.2, 1.3 (parallel)
Phase 2: 2.5 → 2.3 → 2.4 → 2.2 → 2.1
Phase 3: 3.1 → 3.2 → 3.3 → 3.4 → 3.5 → 3.6 → 3.7
Phase 4: 4.1 → 4.2 → 4.3 → 4.4 → 4.5

Phase 0 blocks Phase 1 and Phase 2.
Phase 1 blocks Phase 2 (credentials needed for launcher).
Phase 2 blocks Phase 3 (launcher must exist for client to talk to).
Phase 2 and Phase 3 block Phase 4 (E2E needs both).
```

## Risk Register

| Risk | Mitigation |
|------|-----------|
| matrix-sdk doesn't compile to WASM | Phase 0 is a dedicated proof. If it fails, pivot to matrix-js-sdk + matrix-sdk-crypto-wasm |
| IndexedDB not available in Node.js | Use `fake-indexeddb` npm package as polyfill for Node.js environments |
| keytar native addon fails on some platforms | Already handled: falls back to encrypted file |
| Rate limiting on public servers | Public server tests use 5s delays, single room, manual-only |
| wasm-bindgen async/await limitations | Use `wasm-bindgen-futures` for async Rust → JS Promise bridging |
