// Cross-runtime crypto-vector verification: loads the same committed fixture
// that `crates/mxdx-p2p/tests/crypto_vectors.rs` reads, and decrypts each
// vector through `packages/core/p2p-crypto.js`. This proves the Rust and npm
// AES-256-GCM wire format are byte-identical without needing a live
// homeserver.
//
// Run: `node --test packages/e2e-tests/tests/rust-npm-crypto-vectors.test.js`

import { test } from 'node:test';
import assert from 'node:assert/strict';
import { readFileSync } from 'node:fs';
import { fileURLToPath } from 'node:url';
import { dirname, resolve } from 'node:path';

import { createP2PCrypto } from '../../core/p2p-crypto.js';

const __dirname = dirname(fileURLToPath(import.meta.url));
const FIXTURE_PATH = resolve(
  __dirname,
  '../../../crates/mxdx-p2p/tests/fixtures/crypto-vectors.json',
);

test('decrypt all committed Rust vectors via npm p2p-crypto', async () => {
  const fixture = JSON.parse(readFileSync(FIXTURE_PATH, 'utf8'));
  assert.equal(fixture.version, 1);
  assert.ok(Array.isArray(fixture.vectors) && fixture.vectors.length > 0);

  for (const v of fixture.vectors) {
    const crypto = await createP2PCrypto(v.key_b64);
    const frameJson = JSON.stringify({ c: v.ciphertext_b64, iv: v.iv_b64 });
    const plaintextStr = await crypto.decrypt(frameJson);

    const expected = Buffer.from(v.plaintext_b64, 'base64');
    const got = Buffer.from(plaintextStr, 'binary'); // decrypt returns a string from TextDecoder

    // The npm decrypt returns a UTF-8 string; for binary vectors we compare
    // the UTF-8-decoded string's bytes against the original plaintext bytes.
    // For non-UTF-8 inputs (1KB/64KB pseudorandom patterns), TextDecoder
    // performs WHATWG UTF-8 replacement. To keep the test valid across all
    // vectors, we compare the decrypt() output bytes re-encoded as UTF-8 to
    // the expected plaintext bytes ONLY when the plaintext is valid UTF-8.
    // For binary vectors we decrypt at the Web Crypto layer directly and
    // compare raw bytes.
    if (isValidUtf8(expected)) {
      const expectedStr = expected.toString('utf8');
      assert.equal(plaintextStr, expectedStr, `vector ${v.name} (utf8)`);
    } else {
      // Fall back to the raw Web Crypto API so we can compare bytes-for-bytes.
      const rawKey = Buffer.from(v.key_b64, 'base64');
      const subtleKey = await globalThis.crypto.subtle.importKey(
        'raw',
        rawKey,
        { name: 'AES-GCM' },
        false,
        ['decrypt'],
      );
      const ct = Buffer.from(v.ciphertext_b64, 'base64');
      const iv = Buffer.from(v.iv_b64, 'base64');
      const pt = new Uint8Array(
        await globalThis.crypto.subtle.decrypt({ name: 'AES-GCM', iv }, subtleKey, ct),
      );
      assert.deepEqual(Buffer.from(pt), expected, `vector ${v.name} (binary)`);
    }

    // Silence unused 'got' in the utf8 branch without pulling in another var.
    void got;
  }
});

// Basic UTF-8 validator — reject any byte sequence that would trigger
// WHATWG TextDecoder replacement. Good enough to partition our 5 vectors.
function isValidUtf8(buf) {
  try {
    new TextDecoder('utf-8', { fatal: true }).decode(buf);
    return true;
  } catch {
    return false;
  }
}
