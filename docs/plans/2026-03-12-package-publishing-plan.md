# mxdx Package Publishing Implementation Plan

> **For Claude:** REQUIRED SUB-SKILL: Use superpowers:executing-plans to implement this plan task-by-task.

**Goal:** Publish all Rust crates to crates.io and all npm packages to npmjs, with a `mxdx`/`mx` CLI meta-package, and CI release automation via semantic-release.

**Architecture:** Each Rust crate gets crates.io metadata and publishes in dependency order. npm packages are updated from `private: true` to publishable with proper `files`, `publishConfig`, and version fields. A new `mxdx` meta-package provides a dispatcher binary (`mxdx`/`mx`) that delegates to `@mxdx/launcher`, `@mxdx/client`, and `@mxdx/web-console`. The web-console ships a pre-built Vite SPA with a built-in static file server. A `release.yml` GitHub Actions workflow automates the full publish pipeline.

**Tech Stack:** Cargo (crates.io), npm (npmjs.org), semantic-release, GitHub Actions, wasm-pack, Vite

---

## Task 1: Add crates.io metadata to all publishable Rust crates

**Files:**
- Modify: `Cargo.toml` (workspace root)
- Modify: `crates/mxdx-types/Cargo.toml`
- Modify: `crates/mxdx-matrix/Cargo.toml`
- Modify: `crates/mxdx-policy/Cargo.toml`
- Modify: `crates/mxdx-secrets/Cargo.toml`
- Modify: `crates/mxdx-launcher/Cargo.toml`
- Modify: `crates/mxdx-web/Cargo.toml`
- Modify: `crates/mxdx-core-wasm/Cargo.toml`
- Modify: `xtask/Cargo.toml`
- Modify: `tests/helpers/Cargo.toml`

**Step 1: Add shared metadata to workspace root**

Add to `Cargo.toml` (workspace root), after `resolver = "2"`:

```toml
[workspace.package]
version = "0.1.0"
edition = "2021"
license = "MIT"
repository = "https://github.com/Epiphytic/mxdx"
homepage = "https://github.com/Epiphytic/mxdx"
keywords = ["matrix", "fleet-management", "e2ee", "terminal", "webrtc"]
categories = ["command-line-utilities", "network-programming", "cryptography"]
```

**Step 2: Update each publishable crate's Cargo.toml**

Each crate replaces `version` and `edition` with `version.workspace = true` and `edition.workspace = true`, and adds the shared fields plus a unique `description`. Example for `mxdx-types`:

```toml
[package]
name = "mxdx-types"
version.workspace = true
edition.workspace = true
license.workspace = true
repository.workspace = true
homepage.workspace = true
keywords.workspace = true
categories.workspace = true
description = "Core type definitions for mxdx fleet management"
```

Descriptions for each crate:
- `mxdx-types`: "Core type definitions for mxdx fleet management"
- `mxdx-matrix`: "Matrix SDK client facade for mxdx with E2EE support"
- `mxdx-policy`: "Access control and appservice registration for mxdx"
- `mxdx-secrets`: "Age encryption and secret management for mxdx"
- `mxdx-launcher`: "Fleet management launcher agent for mxdx"
- `mxdx-web`: "Web server backend for mxdx management console"
- `mxdx-core-wasm`: "WASM bindings for mxdx Matrix operations"

**Step 3: Mark non-publishable crates**

Add `publish = false` to `xtask/Cargo.toml` and `tests/helpers/Cargo.toml`:

```toml
[package]
name = "xtask"
publish = false
```

```toml
[package]
name = "mxdx-test-helpers"
publish = false
```

**Step 4: Convert inter-crate path dependencies to support both local and crates.io**

Each inter-crate dependency needs both `path` and `version` so that `cargo publish` resolves correctly. For every `mxdx-*` path dependency, add `version = "0.1.0"`:

```toml
# Example in mxdx-matrix/Cargo.toml
mxdx-types = { path = "../mxdx-types", version = "0.1.0" }

# Example in mxdx-policy/Cargo.toml
mxdx-types = { path = "../mxdx-types", version = "0.1.0" }
mxdx-matrix = { path = "../mxdx-matrix", version = "0.1.0" }
```

Do this for ALL inter-crate dependencies in:
- `crates/mxdx-matrix/Cargo.toml` — `mxdx-types`
- `crates/mxdx-policy/Cargo.toml` — `mxdx-types`, `mxdx-matrix`
- `crates/mxdx-secrets/Cargo.toml` — `mxdx-types`, `mxdx-matrix`
- `crates/mxdx-launcher/Cargo.toml` — `mxdx-types`, `mxdx-matrix` (optional)
- `crates/mxdx-core-wasm/Cargo.toml` — `mxdx-types`

Do NOT add version to dev-dependencies on `mxdx-test-helpers` (it's `publish = false`).

**Step 5: Verify workspace builds**

Run: `cargo build --workspace`
Expected: Compiles successfully with no errors.

**Step 6: Dry-run publish in dependency order**

Run:
```bash
cargo publish -p mxdx-types --dry-run
cargo publish -p mxdx-matrix --dry-run
cargo publish -p mxdx-policy --dry-run
cargo publish -p mxdx-secrets --dry-run
cargo publish -p mxdx-launcher --dry-run
cargo publish -p mxdx-web --dry-run
cargo publish -p mxdx-core-wasm --dry-run
```
Expected: All pass without errors. Fix any missing fields or dependency issues.

**Step 7: Commit**

```bash
git add Cargo.toml crates/*/Cargo.toml xtask/Cargo.toml tests/helpers/Cargo.toml
git commit -m "chore: add crates.io metadata to all publishable Rust crates"
```

---

## Task 2: Prepare `@mxdx/core` for npm publishing

**Files:**
- Modify: `packages/core/package.json`
- Create: `packages/core/p2p-crypto.js` (already exists, ensure in files list)
- Create: `packages/core/persistent-indexeddb.js` (already exists, ensure in files list)

**Step 1: Update package.json**

Replace the contents of `packages/core/package.json`:

```json
{
  "name": "@mxdx/core",
  "version": "0.1.0",
  "description": "WASM bindings and shared modules for mxdx fleet management",
  "type": "module",
  "main": "index.js",
  "files": [
    "wasm/",
    "index.js",
    "credentials.js",
    "session.js",
    "terminal-socket.js",
    "terminal-types.js",
    "cleanup.js",
    "batched-sender.js",
    "turn-credentials.js",
    "webrtc-channel-node.js",
    "p2p-signaling.js",
    "p2p-transport.js",
    "p2p-crypto.js",
    "persistent-indexeddb.js"
  ],
  "publishConfig": {
    "access": "public"
  },
  "engines": {
    "node": ">=22"
  },
  "repository": {
    "type": "git",
    "url": "git+https://github.com/Epiphytic/mxdx.git",
    "directory": "packages/core"
  },
  "homepage": "https://github.com/Epiphytic/mxdx",
  "license": "MIT",
  "keywords": ["matrix", "e2ee", "wasm", "fleet-management", "webrtc"],
  "dependencies": {
    "fake-indexeddb": "^6.2.5",
    "node-datachannel": "^0.32.1",
    "zod": "^3.23.0"
  }
}
```

Key changes: removed `private: true`, added `publishConfig`, `engines`, `repository`, `homepage`, `license`, `keywords`, `description`, added `p2p-crypto.js` and `persistent-indexeddb.js` to `files`.

**Step 2: Verify the WASM directory structure**

The `@mxdx/core` package needs to ship both WASM targets. Currently `packages/core/wasm/` has the nodejs target. We need to restructure:

```
packages/core/wasm/nodejs/   ← wasm-pack --target nodejs output
packages/core/wasm/web/      ← wasm-pack --target web output
```

Update the WASM build commands:
```bash
wasm-pack build crates/mxdx-core-wasm --target nodejs --out-dir ../../packages/core/wasm/nodejs
wasm-pack build crates/mxdx-core-wasm --target web --out-dir ../../packages/core/wasm/web
```

Update `packages/core/index.js` to import from `./wasm/nodejs/` instead of `./wasm/`.
Update `packages/web-console/src/main.js` (or wherever it imports WASM) to import from `@mxdx/core/wasm/web/` or its local copy.

**Step 3: Verify npm pack**

Run: `cd packages/core && npm pack --dry-run`
Expected: Lists all expected files, no unexpected inclusions.

**Step 4: Commit**

```bash
git add packages/core/
git commit -m "chore: prepare @mxdx/core for npm publishing"
```

---

## Task 3: Prepare `@mxdx/launcher` for npm publishing

**Files:**
- Modify: `packages/launcher/package.json`

**Step 1: Update package.json**

```json
{
  "name": "@mxdx/launcher",
  "version": "0.1.0",
  "description": "Matrix-native fleet management launcher agent",
  "type": "module",
  "bin": {
    "mxdx-launcher": "bin/mxdx-launcher.js"
  },
  "files": [
    "bin/",
    "src/"
  ],
  "publishConfig": {
    "access": "public"
  },
  "engines": {
    "node": ">=22"
  },
  "repository": {
    "type": "git",
    "url": "git+https://github.com/Epiphytic/mxdx.git",
    "directory": "packages/launcher"
  },
  "homepage": "https://github.com/Epiphytic/mxdx",
  "license": "MIT",
  "keywords": ["matrix", "fleet-management", "launcher", "e2ee", "terminal"],
  "dependencies": {
    "@mxdx/core": "0.1.0",
    "commander": "^12.0.0",
    "inquirer": "^12.0.0",
    "smol-toml": "^1.0.0"
  }
}
```

Key changes: removed `private: true`, added publishing metadata, pinned `@mxdx/core` to `0.1.0` instead of `*`.

**Step 2: Verify npm pack**

Run: `cd packages/launcher && npm pack --dry-run`
Expected: Includes `bin/mxdx-launcher.js` and `src/` files.

**Step 3: Commit**

```bash
git add packages/launcher/package.json
git commit -m "chore: prepare @mxdx/launcher for npm publishing"
```

---

## Task 4: Prepare `@mxdx/client` for npm publishing

**Files:**
- Modify: `packages/client/package.json`

**Step 1: Update package.json**

```json
{
  "name": "@mxdx/client",
  "version": "0.1.0",
  "description": "CLI client for mxdx fleet management",
  "type": "module",
  "bin": {
    "mxdx-client": "bin/mxdx-client.js"
  },
  "files": [
    "bin/",
    "src/"
  ],
  "publishConfig": {
    "access": "public"
  },
  "engines": {
    "node": ">=22"
  },
  "repository": {
    "type": "git",
    "url": "git+https://github.com/Epiphytic/mxdx.git",
    "directory": "packages/client"
  },
  "homepage": "https://github.com/Epiphytic/mxdx",
  "license": "MIT",
  "keywords": ["matrix", "fleet-management", "cli", "e2ee", "terminal"],
  "dependencies": {
    "@mxdx/core": "0.1.0",
    "commander": "^12.0.0",
    "smol-toml": "^1.0.0"
  }
}
```

**Step 2: Verify npm pack**

Run: `cd packages/client && npm pack --dry-run`

**Step 3: Commit**

```bash
git add packages/client/package.json
git commit -m "chore: prepare @mxdx/client for npm publishing"
```

---

## Task 5: Prepare `@mxdx/web-console` for npm publishing with static server

**Files:**
- Modify: `packages/web-console/package.json`
- Create: `packages/web-console/bin/mxdx-web-console.js`

**Step 1: Update package.json**

```json
{
  "name": "@mxdx/web-console",
  "version": "0.1.0",
  "description": "Browser-based management console for mxdx fleet management",
  "type": "module",
  "bin": {
    "mxdx-web-console": "bin/mxdx-web-console.js"
  },
  "scripts": {
    "dev": "vite",
    "build": "vite build",
    "preview": "vite preview"
  },
  "files": [
    "bin/",
    "dist/"
  ],
  "publishConfig": {
    "access": "public"
  },
  "engines": {
    "node": ">=22"
  },
  "repository": {
    "type": "git",
    "url": "git+https://github.com/Epiphytic/mxdx.git",
    "directory": "packages/web-console"
  },
  "homepage": "https://github.com/Epiphytic/mxdx",
  "license": "MIT",
  "keywords": ["matrix", "fleet-management", "web-console", "xterm", "terminal"],
  "dependencies": {
    "@xterm/xterm": "^5.5.0",
    "@xterm/addon-fit": "^0.10.0",
    "zod": "^3.23.0"
  },
  "devDependencies": {
    "vite": "^6.0.0"
  }
}
```

Note: `dist/` is in `files` — the pre-built SPA ships in the tarball. `vite` stays as devDependency for local dev but isn't needed by consumers.

**Step 2: Create the static file server bin script**

Create `packages/web-console/bin/mxdx-web-console.js`:

```javascript
#!/usr/bin/env node

import { createServer } from 'node:http';
import { readFile } from 'node:fs/promises';
import { join, extname } from 'node:path';
import { fileURLToPath } from 'node:url';
import { existsSync } from 'node:fs';

const __dirname = fileURLToPath(new URL('..', import.meta.url));
const distDir = join(__dirname, 'dist');

if (!existsSync(distDir)) {
  console.error('Error: dist/ directory not found. Run "npm run build" first or install from npm.');
  process.exit(1);
}

const MIME_TYPES = {
  '.html': 'text/html; charset=utf-8',
  '.js':   'application/javascript; charset=utf-8',
  '.mjs':  'application/javascript; charset=utf-8',
  '.css':  'text/css; charset=utf-8',
  '.json': 'application/json; charset=utf-8',
  '.wasm': 'application/wasm',
  '.png':  'image/png',
  '.svg':  'image/svg+xml',
  '.ico':  'image/x-icon',
};

const port = parseInt(process.argv.find((a, i) => process.argv[i - 1] === '--port') || '5173', 10);

const server = createServer(async (req, res) => {
  let filePath = join(distDir, req.url === '/' ? 'index.html' : req.url);

  try {
    const data = await readFile(filePath);
    const ext = extname(filePath);
    res.writeHead(200, { 'Content-Type': MIME_TYPES[ext] || 'application/octet-stream' });
    res.end(data);
  } catch {
    // SPA fallback: serve index.html for unmatched routes
    try {
      const index = await readFile(join(distDir, 'index.html'));
      res.writeHead(200, { 'Content-Type': 'text/html; charset=utf-8' });
      res.end(index);
    } catch {
      res.writeHead(404);
      res.end('Not Found');
    }
  }
});

server.listen(port, () => {
  console.log(`mxdx web console running at http://localhost:${port}`);
});
```

**Step 3: Build the dist/ for local testing**

Run:
```bash
cd packages/web-console
# Ensure web WASM is built
wasm-pack build ../../crates/mxdx-core-wasm --target web --out-dir ../../packages/web-console/wasm
npx vite build
```
Expected: `packages/web-console/dist/` created with built SPA.

**Step 4: Test the static server**

Run: `node packages/web-console/bin/mxdx-web-console.js`
Expected: Prints "mxdx web console running at http://localhost:5173", serves the SPA.

**Step 5: Add dist/ to .gitignore**

The built dist/ should not be committed — it's built by CI before publish.

Add to root `.gitignore`:
```
packages/web-console/dist/
```

**Step 6: Verify npm pack**

Run: `cd packages/web-console && npm pack --dry-run`
Expected: Includes `bin/mxdx-web-console.js` and `dist/` contents.

Note: `npm pack --dry-run` will only include `dist/` if it exists locally. In CI, the release workflow builds it before publish.

**Step 7: Commit**

```bash
git add packages/web-console/package.json packages/web-console/bin/mxdx-web-console.js .gitignore
git commit -m "feat: add static file server for @mxdx/web-console npm distribution"
```

---

## Task 6: Create `mxdx` meta-package with `mx` alias

**Files:**
- Create: `packages/mxdx/package.json`
- Create: `packages/mxdx/bin/mxdx.js`
- Modify: `package.json` (root — add to workspaces)

**Step 1: Create the dispatcher bin script**

Create `packages/mxdx/bin/mxdx.js`:

```javascript
#!/usr/bin/env node

import { execFileSync } from 'node:child_process';
import { createRequire } from 'node:module';
import { dirname, join } from 'node:path';

const require = createRequire(import.meta.url);

const SUBCOMMANDS = {
  launcher:       '@mxdx/launcher/bin/mxdx-launcher.js',
  client:         '@mxdx/client/bin/mxdx-client.js',
  'web-console':  '@mxdx/web-console/bin/mxdx-web-console.js',
};

const HELP = `
mxdx — Matrix-native fleet management

Usage:
  mxdx <command> [options]
  mx <command> [options]

Commands:
  launcher      Start or manage a launcher agent on this host
  client        CLI for fleet management (exec, shell, telemetry)
  web-console   Start the browser-based management console

Options:
  --help        Show this help message
  --version     Show version

Quickstart:
  https://github.com/Epiphytic/mxdx/blob/main/docs/quickstart.md

Examples:
  mxdx launcher start --servers http://localhost:8008
  mxdx client exec my-launcher echo hello
  mxdx web-console --port 3000
  mx launcher start
`.trim();

const args = process.argv.slice(2);
const command = args[0];

if (!command || command === '--help' || command === '-h') {
  console.log(HELP);
  process.exit(0);
}

if (command === '--version' || command === '-v') {
  const pkg = createRequire(import.meta.url)('../package.json');
  console.log(`mxdx v${pkg.version}`);
  process.exit(0);
}

const target = SUBCOMMANDS[command];
if (!target) {
  console.error(`Unknown command: ${command}`);
  console.error(`Run "mxdx --help" for available commands.`);
  process.exit(1);
}

// Resolve the target package's bin script
let binPath;
try {
  binPath = require.resolve(target);
} catch {
  console.error(`Package not found: ${target.split('/')[0]}/${target.split('/')[1]}`);
  console.error(`Install it with: npm install ${target.split('/bin/')[0]}`);
  process.exit(1);
}

// Delegate to the target with remaining args
const childArgs = args.slice(1);
try {
  execFileSync(process.execPath, [binPath, ...childArgs], { stdio: 'inherit' });
} catch (err) {
  process.exit(err.status || 1);
}
```

**Step 2: Create package.json**

Create `packages/mxdx/package.json`:

```json
{
  "name": "mxdx",
  "version": "0.1.0",
  "description": "Matrix-native fleet management with E2EE terminals and P2P transport",
  "type": "module",
  "bin": {
    "mxdx": "bin/mxdx.js",
    "mx": "bin/mxdx.js"
  },
  "files": [
    "bin/"
  ],
  "engines": {
    "node": ">=22"
  },
  "repository": {
    "type": "git",
    "url": "git+https://github.com/Epiphytic/mxdx.git",
    "directory": "packages/mxdx"
  },
  "homepage": "https://github.com/Epiphytic/mxdx",
  "license": "MIT",
  "keywords": ["matrix", "fleet-management", "e2ee", "terminal", "webrtc", "p2p"],
  "dependencies": {
    "@mxdx/launcher": "0.1.0",
    "@mxdx/client": "0.1.0",
    "@mxdx/web-console": "0.1.0"
  }
}
```

**Step 3: Add to root workspace**

Modify root `package.json` to add `packages/mxdx` to workspaces:

```json
{
  "private": true,
  "workspaces": [
    "packages/core",
    "packages/launcher",
    "packages/client",
    "packages/web-console",
    "packages/mxdx",
    "packages/e2e-tests"
  ],
  "devDependencies": {
    "@playwright/test": "^1.58.2"
  }
}
```

**Step 4: Test the dispatcher locally**

Run:
```bash
npm install
node packages/mxdx/bin/mxdx.js --help
node packages/mxdx/bin/mxdx.js --version
node packages/mxdx/bin/mxdx.js launcher --help
node packages/mxdx/bin/mxdx.js client --help
```
Expected: Help text prints for each, launcher/client delegate correctly.

**Step 5: Verify npm pack**

Run: `cd packages/mxdx && npm pack --dry-run`
Expected: Only `bin/mxdx.js` and `package.json` included.

**Step 6: Commit**

```bash
git add packages/mxdx/ package.json
git commit -m "feat: create mxdx meta-package with mx CLI alias"
```

---

## Task 7: Restructure WASM build for dual-target shipping

**Files:**
- Modify: `packages/core/index.js`
- Modify: any web-console imports that reference `./wasm/`
- Modify: `.github/workflows/ci.yml` (WASM build commands)

**Step 1: Update WASM build commands**

The new canonical build commands:
```bash
wasm-pack build crates/mxdx-core-wasm --target nodejs --out-dir ../../packages/core/wasm/nodejs
wasm-pack build crates/mxdx-core-wasm --target web --out-dir ../../packages/core/wasm/web
```

**Step 2: Update `packages/core/index.js`**

Change the WASM import path from `./wasm/mxdx_core_wasm.js` to `./wasm/nodejs/mxdx_core_wasm.js`.

Read the current file first to find the exact import line.

**Step 3: Update web-console WASM imports**

The web console currently imports from a local `./wasm/` directory. It needs to import from `./wasm/web/` (for local dev) or from `@mxdx/core/wasm/web/` (when installed as a dependency).

Read `packages/web-console/src/main.js` to find the WASM import and update accordingly.

**Step 4: Update vite.config.js**

Update the `optimizeDeps.exclude` path if it references the old WASM location.

**Step 5: Remove old WASM output directories**

```bash
rm -rf packages/core/wasm
rm -rf packages/web-console/wasm
```

**Step 6: Rebuild WASM with new paths**

```bash
wasm-pack build crates/mxdx-core-wasm --target nodejs --out-dir ../../packages/core/wasm/nodejs
wasm-pack build crates/mxdx-core-wasm --target web --out-dir ../../packages/core/wasm/web
```

**Step 7: Verify everything still works**

Run:
```bash
npm install
node --test packages/launcher/tests/runtime-unit.test.js
```
Expected: Tests pass with the new WASM path.

**Step 8: Update .gitignore**

Ensure `packages/core/wasm/` is in `.gitignore` (WASM is built, not committed):
```
packages/core/wasm/
packages/web-console/dist/
```

**Step 9: Update CI workflow WASM build commands**

In `.github/workflows/ci.yml`, update the `Build WASM` step:
```yaml
- name: Build WASM (nodejs)
  run: wasm-pack build crates/mxdx-core-wasm --target nodejs --out-dir ../../packages/core/wasm/nodejs
- name: Build WASM (web)
  run: wasm-pack build crates/mxdx-core-wasm --target web --out-dir ../../packages/core/wasm/web
```

**Step 10: Commit**

```bash
git add -A
git commit -m "refactor: restructure WASM to dual-target layout (nodejs + web)"
```

---

## Task 8: Create semantic-release configuration

**Files:**
- Create: `.releaserc.json`
- Create: `scripts/publish-crates.sh`

**Step 1: Create `.releaserc.json`**

```json
{
  "branches": ["main"],
  "verifyConditions": [
    "@semantic-release/github",
    "@semantic-release/git"
  ],
  "plugins": [
    "@semantic-release/commit-analyzer",
    "@semantic-release/release-notes-generator",
    [
      "@semantic-release/exec",
      {
        "prepareCmd": "node scripts/bump-versions.mjs ${nextRelease.version}",
        "publishCmd": "bash scripts/publish-crates.sh && bash scripts/publish-npm.sh"
      }
    ],
    [
      "@semantic-release/github",
      {
        "successComment": false,
        "failTitle": false
      }
    ],
    [
      "@semantic-release/git",
      {
        "assets": [
          "Cargo.toml",
          "crates/*/Cargo.toml",
          "packages/*/package.json",
          "Cargo.lock"
        ],
        "message": "chore(release): ${nextRelease.version} [skip ci]"
      }
    ]
  ]
}
```

**Step 2: Create version bump script**

Create `scripts/bump-versions.mjs`:

```javascript
#!/usr/bin/env node

// Bumps version across all Cargo.toml and package.json files
import { readFileSync, writeFileSync } from 'node:fs';
import { globSync } from 'node:fs';
import { execSync } from 'node:child_process';

const version = process.argv[2];
if (!version) {
  console.error('Usage: bump-versions.mjs <version>');
  process.exit(1);
}

// Bump workspace Cargo.toml version
const cargoRoot = 'Cargo.toml';
let cargo = readFileSync(cargoRoot, 'utf8');
cargo = cargo.replace(/^version = ".*"$/m, `version = "${version}"`);
writeFileSync(cargoRoot, cargo);

// Bump inter-crate dependency versions
const crateDirs = [
  'crates/mxdx-types', 'crates/mxdx-matrix', 'crates/mxdx-policy',
  'crates/mxdx-secrets', 'crates/mxdx-launcher', 'crates/mxdx-web',
  'crates/mxdx-core-wasm',
];
for (const dir of crateDirs) {
  const path = `${dir}/Cargo.toml`;
  let content = readFileSync(path, 'utf8');
  // Update mxdx-* dependency versions
  content = content.replace(
    /(mxdx-\w+\s*=\s*\{[^}]*version\s*=\s*)"[^"]*"/g,
    `$1"${version}"`
  );
  writeFileSync(path, content);
}

// Bump npm package versions
const npmDirs = [
  'packages/core', 'packages/launcher', 'packages/client',
  'packages/web-console', 'packages/mxdx',
];
for (const dir of npmDirs) {
  const path = `${dir}/package.json`;
  const pkg = JSON.parse(readFileSync(path, 'utf8'));
  pkg.version = version;
  // Update @mxdx/* dependency versions
  for (const depKey of ['dependencies', 'devDependencies', 'peerDependencies']) {
    if (pkg[depKey]) {
      for (const [name, ver] of Object.entries(pkg[depKey])) {
        if (name.startsWith('@mxdx/') || name === 'mxdx') {
          pkg[depKey][name] = version;
        }
      }
    }
  }
  writeFileSync(path, JSON.stringify(pkg, null, 2) + '\n');
}

console.log(`Bumped all versions to ${version}`);
```

**Step 3: Create crates publish script**

Create `scripts/publish-crates.sh`:

```bash
#!/bin/bash
set -euo pipefail

# Publish Rust crates in dependency order
# Waits between publishes for crates.io index to update

CRATES=(
  mxdx-types
  mxdx-matrix
  mxdx-policy
  mxdx-secrets
  mxdx-launcher
  mxdx-web
  mxdx-core-wasm
)

for crate in "${CRATES[@]}"; do
  echo "Publishing $crate..."
  cargo publish -p "$crate" --no-verify
  echo "Waiting for crates.io index update..."
  sleep 30
done

echo "All crates published."
```

**Step 4: Create npm publish script**

Create `scripts/publish-npm.sh`:

```bash
#!/bin/bash
set -euo pipefail

# Publish npm packages in dependency order

echo "Publishing @mxdx/core..."
cd packages/core && npm publish --provenance --access public && cd ../..

echo "Publishing @mxdx/launcher..."
cd packages/launcher && npm publish --provenance --access public && cd ../..

echo "Publishing @mxdx/client..."
cd packages/client && npm publish --provenance --access public && cd ../..

echo "Publishing @mxdx/web-console..."
cd packages/web-console && npm publish --provenance --access public && cd ../..

echo "Publishing mxdx..."
cd packages/mxdx && npm publish --provenance --access public && cd ../..

echo "All npm packages published."
```

**Step 5: Make scripts executable**

```bash
chmod +x scripts/publish-crates.sh scripts/publish-npm.sh
```

**Step 6: Commit**

```bash
git add .releaserc.json scripts/
git commit -m "chore: add semantic-release config and publish scripts"
```

---

## Task 9: Create release GitHub Actions workflow

**Files:**
- Create: `.github/workflows/release.yml`

**Step 1: Create the workflow**

Create `.github/workflows/release.yml`:

```yaml
name: Release

on:
  push:
    branches: [main]

permissions:
  contents: write
  id-token: write
  issues: write
  pull-requests: write

jobs:
  release:
    runs-on: ubuntu-latest
    steps:
      - uses: actions/checkout@v4
        with:
          fetch-depth: 0
          persist-credentials: false

      - uses: dtolnay/rust-toolchain@stable
        with:
          targets: wasm32-unknown-unknown

      - uses: Swatinem/rust-cache@v2

      - uses: actions/setup-node@v4
        with:
          node-version: 22
          registry-url: https://registry.npmjs.org

      - name: Install system deps
        run: sudo apt-get install -y libsqlite3-dev libssl-dev

      - name: Install wasm-pack
        run: curl https://rustwasm.github.io/wasm-pack/installer/init.sh -sSf | sh

      - name: Build workspace
        run: cargo build --workspace

      - name: Run Rust tests
        run: cargo test --workspace --lib

      - name: Build WASM (nodejs)
        run: wasm-pack build crates/mxdx-core-wasm --target nodejs --out-dir ../../packages/core/wasm/nodejs

      - name: Build WASM (web)
        run: wasm-pack build crates/mxdx-core-wasm --target web --out-dir ../../packages/core/wasm/web

      - name: Install npm deps
        run: npm install

      - name: Build web-console
        run: cd packages/web-console && npx vite build

      - name: Smoke test dispatcher
        run: |
          node packages/mxdx/bin/mxdx.js --help
          node packages/mxdx/bin/mxdx.js --version
          node packages/mxdx/bin/mxdx.js launcher --help
          node packages/mxdx/bin/mxdx.js client --help

      - name: Release
        env:
          GITHUB_TOKEN: ${{ secrets.GITHUB_TOKEN }}
          CARGO_REGISTRY_TOKEN: ${{ secrets.CARGO_REGISTRY_TOKEN }}
          NODE_AUTH_TOKEN: ${{ secrets.NPM_TOKEN }}
        run: npx semantic-release
```

**Step 2: Commit**

```bash
git add .github/workflows/release.yml
git commit -m "ci: add release workflow for crates.io and npm publishing"
```

---

## Task 10: Update CI workflow for new WASM paths

**Files:**
- Modify: `.github/workflows/ci.yml`

**Step 1: Update WASM build commands in ci.yml**

In the `npm-e2e` job, update the `Build WASM` step:

```yaml
      - name: Build WASM (nodejs)
        run: wasm-pack build crates/mxdx-core-wasm --target nodejs --out-dir ../../packages/core/wasm/nodejs
      - name: Build WASM (web)
        run: wasm-pack build crates/mxdx-core-wasm --target web --out-dir ../../packages/core/wasm/web
```

Same for the `npm-public-server` job.

**Step 2: Add npm pack smoke test job**

Add a new job to ci.yml:

```yaml
  npm-pack:
    needs: npm-e2e
    runs-on: ubuntu-latest
    steps:
      - uses: actions/checkout@v4
      - uses: dtolnay/rust-toolchain@stable
        with:
          targets: wasm32-unknown-unknown
      - uses: Swatinem/rust-cache@v2
      - uses: actions/setup-node@v4
        with:
          node-version: "22"
      - name: Install wasm-pack
        run: curl https://rustwasm.github.io/wasm-pack/installer/init.sh -sSf | sh
      - name: Install system deps
        run: sudo apt-get install -y libsqlite3-dev libssl-dev
      - name: Build WASM (nodejs)
        run: wasm-pack build crates/mxdx-core-wasm --target nodejs --out-dir ../../packages/core/wasm/nodejs
      - name: Build WASM (web)
        run: wasm-pack build crates/mxdx-core-wasm --target web --out-dir ../../packages/core/wasm/web
      - name: Install npm deps
        run: npm install
      - name: Build web-console
        run: cd packages/web-console && npx vite build
      - name: Pack all packages
        run: |
          cd packages/core && npm pack --dry-run && cd ../..
          cd packages/launcher && npm pack --dry-run && cd ../..
          cd packages/client && npm pack --dry-run && cd ../..
          cd packages/web-console && npm pack --dry-run && cd ../..
          cd packages/mxdx && npm pack --dry-run && cd ../..
      - name: Smoke test dispatcher
        run: |
          node packages/mxdx/bin/mxdx.js --help
          node packages/mxdx/bin/mxdx.js launcher --help
          node packages/mxdx/bin/mxdx.js client --help
```

**Step 3: Commit**

```bash
git add .github/workflows/ci.yml
git commit -m "ci: update WASM paths and add npm pack smoke test"
```

---

## Task 11: First publish — manual crates.io and npm

**This task is manual.** After all previous tasks are committed and pushed, perform the first publish manually to verify everything works before relying on CI automation.

**Step 1: Verify crates.io auth**

```bash
cargo login
# Paste your crates.io API token
```

**Step 2: Publish Rust crates in order**

```bash
cargo publish -p mxdx-types
sleep 30
cargo publish -p mxdx-matrix
sleep 30
cargo publish -p mxdx-policy
sleep 30
cargo publish -p mxdx-secrets
sleep 30
cargo publish -p mxdx-launcher
sleep 30
cargo publish -p mxdx-web
sleep 30
cargo publish -p mxdx-core-wasm
```

Wait 30s between each for the crates.io index to update.

**Step 3: Verify crates on crates.io**

Check each crate page:
- https://crates.io/crates/mxdx-types
- https://crates.io/crates/mxdx-matrix
- https://crates.io/crates/mxdx-policy
- https://crates.io/crates/mxdx-secrets
- https://crates.io/crates/mxdx-launcher
- https://crates.io/crates/mxdx-web
- https://crates.io/crates/mxdx-core-wasm

**Step 4: Build WASM and web-console for npm publish**

```bash
wasm-pack build crates/mxdx-core-wasm --target nodejs --out-dir ../../packages/core/wasm/nodejs
wasm-pack build crates/mxdx-core-wasm --target web --out-dir ../../packages/core/wasm/web
cd packages/web-console && npx vite build && cd ../..
```

**Step 5: Verify npm auth**

```bash
npm login
# Or set NPM_TOKEN env var
```

**Step 6: Publish npm packages in order**

```bash
cd packages/core && npm publish --access public && cd ../..
cd packages/launcher && npm publish --access public && cd ../..
cd packages/client && npm publish --access public && cd ../..
cd packages/web-console && npm publish --access public && cd ../..
cd packages/mxdx && npm publish --access public && cd ../..
```

**Step 7: Verify the full user flow**

```bash
# In a temp directory
mkdir /tmp/mxdx-test && cd /tmp/mxdx-test
npx -y mxdx --help
npx -y mxdx launcher --help
npx -y mxdx client --help
npx -y @mxdx/launcher --help
npx -y @mxdx/client --help
# Test mx alias
npx -y mxdx --help  # should also work as 'mx' after global install
```

**Step 8: Set up GitHub secrets for CI**

In the repo settings (https://github.com/Epiphytic/mxdx/settings/secrets/actions), add:
- `CARGO_REGISTRY_TOKEN` — crates.io API token
- `NPM_TOKEN` — npm access token with publish permission
