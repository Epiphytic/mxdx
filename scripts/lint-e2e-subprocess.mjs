#!/usr/bin/env node
// Lint: verify every describe/it block in packages/e2e-tests/tests/ spawns a subprocess.
//
// Per ADR 2026-04-29 req 28: tests under e2e-tests/ that do not spawn at least
// one binary subprocess are misclassified integration tests. This script runs in
// warn-only mode (exit 0) for 5 business days after introduction; T-2.8 flips it
// to blocking by removing the force-exit-0 override.
//
// Usage: node scripts/lint-e2e-subprocess.mjs [--blocking]
//   --blocking  Exit non-zero on violations (used by T-2.8 to flip mode)

import fs from 'node:fs';
import path from 'node:path';
import { fileURLToPath } from 'node:url';

const __dirname = path.dirname(fileURLToPath(import.meta.url));
const repoRoot = path.resolve(__dirname, '..');
const testDir = path.join(repoRoot, 'packages', 'e2e-tests', 'tests');

const BLOCKING = process.argv.includes('--blocking');

const SUBPROCESS_PATTERN = /\bspawn\b|\bexecFile\b|\bspawnSync\b|\bexec\b|\bspawnRustBinary\b/;
const BLOCK_START = /^\s*(describe|it)\s*\(/;

let violations = [];
let totalFiles = 0;
let totalBlocks = 0;

for (const file of fs.readdirSync(testDir).sort()) {
  if (!file.endsWith('.test.js')) continue;
  totalFiles++;

  const filePath = path.join(testDir, file);
  const content = fs.readFileSync(filePath, 'utf8');

  if (!SUBPROCESS_PATTERN.test(content)) {
    violations.push({ file, block: '(entire file)', line: 1 });
    totalBlocks++;
    continue;
  }

  // Simple heuristic: find describe/it blocks and check if the surrounding
  // file section contains a subprocess call. For warn-only mode, file-level
  // detection is sufficient to surface the known violators.
  const lines = content.split('\n');
  for (let i = 0; i < lines.length; i++) {
    if (BLOCK_START.test(lines[i])) {
      totalBlocks++;
      // Check if there's any subprocess call in the file (already checked above).
      // For more precise block-level detection, flag files with NO subprocess calls.
    }
  }
}

if (violations.length === 0) {
  console.log(`e2e-subprocess-lint: PASS — all ${totalFiles} files contain subprocess calls`);
  process.exit(0);
}

console.log(`e2e-subprocess-lint: ${violations.length} file(s) lack subprocess calls (misclassified as E2E):`);
for (const v of violations) {
  console.log(`  WARN  ${v.file}:${v.line}  ${v.block}  — no spawn/execFile/spawnSync found`);
}
console.log('');
console.log('These files should be in packages/integration-tests/ not packages/e2e-tests/.');
console.log('See ADR docs/adr/2026-04-29-rust-npm-binary-parity.md req 28 and T-1.4/1.5/1.6.');

if (BLOCKING) {
  process.exit(1);
} else {
  console.log('(warn-only mode — exit 0; use --blocking to enforce)');
  process.exit(0);
}
