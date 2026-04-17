#!/usr/bin/env node
// Regenerate the committed cross-language AES-GCM vector fixture at
// `crates/mxdx-p2p/tests/fixtures/crypto-vectors.json` by running the Rust
// ignored test `generate_vectors` under the `vector-gen` feature.
//
// AES-GCM is deterministic given (key, iv, plaintext), so the generator is
// pinned to a constant key + constant IVs, and the output is byte-stable
// across machines. This script exists so the fixture can be refreshed from
// npm tooling after an intentional update (e.g., adding a new vector).
//
// Usage:
//   node packages/e2e-tests/scripts/regenerate-p2p-vectors.mjs

import { spawnSync } from 'node:child_process';
import { existsSync, readFileSync } from 'node:fs';
import { fileURLToPath } from 'node:url';
import { dirname, resolve } from 'node:path';

const __dirname = dirname(fileURLToPath(import.meta.url));
const REPO_ROOT = resolve(__dirname, '../../..');
const FIXTURE = resolve(
  REPO_ROOT,
  'crates/mxdx-p2p/tests/fixtures/crypto-vectors.json',
);

const before = existsSync(FIXTURE) ? readFileSync(FIXTURE, 'utf8') : null;

const args = [
  'test',
  '-p', 'mxdx-p2p',
  '--features', 'vector-gen',
  '--test', 'crypto_vectors',
  '--',
  '--ignored',
  'generate_vectors',
  '--exact',
  '--nocapture',
];

console.log(`[regen] cargo ${args.join(' ')}`);
const result = spawnSync('cargo', args, {
  cwd: REPO_ROOT,
  stdio: 'inherit',
});
if (result.status !== 0) {
  console.error('[regen] cargo test failed');
  process.exit(result.status ?? 1);
}

if (!existsSync(FIXTURE)) {
  console.error(`[regen] fixture not produced at ${FIXTURE}`);
  process.exit(2);
}
const after = readFileSync(FIXTURE, 'utf8');
if (before !== null && before !== after) {
  console.log('[regen] fixture updated — commit the diff');
} else if (before === null) {
  console.log('[regen] fixture created');
} else {
  console.log('[regen] fixture unchanged');
}
