#!/usr/bin/env node
// Lint: verify every file in packages/e2e-tests/tests/ contains at least one subprocess call.
//
// File-level heuristic: if no spawn/execFile/spawnRustBinary token appears anywhere in the
// file, the file is flagged as a likely misclassified integration test.
// Per ADR 2026-04-29 req 28. Runs warn-only (exit 0) for 5 business days after introduction;
// T-2.8 flips to blocking by removing continue-on-error from CI and passing --blocking here.
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

// Anchor on call syntax to reduce false positives from comments and strings.
const SUBPROCESS_PATTERN = /\bspawn\s*\(|\bexecFile\s*\(|\bspawnSync\s*\(|\bexec\s*\(|\bspawnRustBinary\s*\(/;

let violations = [];
let totalFiles = 0;

for (const file of fs.readdirSync(testDir).sort()) {
  if (!file.endsWith('.test.js')) continue;
  totalFiles++;

  const filePath = path.join(testDir, file);
  const content = fs.readFileSync(filePath, 'utf8');

  if (!SUBPROCESS_PATTERN.test(content)) {
    violations.push({ file, line: 1 });
  }
}

if (violations.length === 0) {
  console.log(`e2e-subprocess-lint: PASS — all ${totalFiles} files contain subprocess calls`);
  process.exit(0);
}

console.log(`e2e-subprocess-lint: ${violations.length} file(s) lack subprocess calls (misclassified as E2E):`);
for (const v of violations) {
  console.log(`  WARN  ${v.file}:${v.line}  — no spawn/execFile/spawnSync call found`);
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
