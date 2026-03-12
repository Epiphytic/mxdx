#!/usr/bin/env node

// Bumps version across all Cargo.toml and package.json files
import { readFileSync, writeFileSync } from 'node:fs';

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
      for (const [name] of Object.entries(pkg[depKey])) {
        if (name.startsWith('@mxdx/') || name === 'mxdx') {
          pkg[depKey][name] = version;
        }
      }
    }
  }
  writeFileSync(path, JSON.stringify(pkg, null, 2) + '\n');
}

console.log(`Bumped all versions to ${version}`);
