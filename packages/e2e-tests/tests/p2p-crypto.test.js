import { describe, it } from 'node:test';
import assert from 'node:assert/strict';
import { generateSessionKey, createP2PCrypto } from '../../../packages/core/p2p-crypto.js';

describe('P2PCrypto', () => {
  it('generates a 256-bit session key as base64', async () => {
    const key = await generateSessionKey();
    assert.equal(typeof key, 'string');
    // base64 of 32 bytes = 44 chars (with padding)
    assert.equal(key.length, 44);
    // Decode to verify it's 32 bytes
    const decoded = Uint8Array.from(atob(key), c => c.charCodeAt(0));
    assert.equal(decoded.length, 32);
  });

  it('encrypts and decrypts roundtrip', async () => {
    const key = await generateSessionKey();
    const crypto = await createP2PCrypto(key);
    const plaintext = 'hello terminal data';
    const ciphertext = await crypto.encrypt(plaintext);
    assert.notEqual(ciphertext, plaintext, 'ciphertext must differ from plaintext');
    const decrypted = await crypto.decrypt(ciphertext);
    assert.equal(decrypted, plaintext);
  });

  it('produces different ciphertexts for same plaintext (random IV)', async () => {
    const key = await generateSessionKey();
    const crypto = await createP2PCrypto(key);
    const ct1 = await crypto.encrypt('same data');
    const ct2 = await crypto.encrypt('same data');
    assert.notEqual(ct1, ct2, 'each encryption should use a unique IV');
  });

  it('fails to decrypt with wrong key', async () => {
    const key1 = await generateSessionKey();
    const key2 = await generateSessionKey();
    const crypto1 = await createP2PCrypto(key1);
    const crypto2 = await createP2PCrypto(key2);
    const ciphertext = await crypto1.encrypt('secret');
    await assert.rejects(
      () => crypto2.decrypt(ciphertext),
      'decryption with wrong key should fail',
    );
  });

  it('detects tampered ciphertext (GCM authentication)', async () => {
    const key = await generateSessionKey();
    const crypto = await createP2PCrypto(key);
    const ciphertext = await crypto.encrypt('important data');
    // Tamper with the ciphertext
    const parsed = JSON.parse(ciphertext);
    const bytes = Uint8Array.from(atob(parsed.c), c => c.charCodeAt(0));
    bytes[0] ^= 0xff; // flip a byte
    let binary = '';
    for (let i = 0; i < bytes.length; i++) binary += String.fromCharCode(bytes[i]);
    parsed.c = btoa(binary);
    await assert.rejects(
      () => crypto.decrypt(JSON.stringify(parsed)),
      'tampered ciphertext should be rejected by GCM',
    );
  });

  it('rejects invalid key length', async () => {
    await assert.rejects(
      () => createP2PCrypto(btoa('tooshort')),
      /Invalid session key length/,
    );
  });

  it('handles large payloads (terminal output)', async () => {
    const key = await generateSessionKey();
    const crypto = await createP2PCrypto(key);
    const largeData = 'x'.repeat(50000); // 50KB of terminal output
    const ciphertext = await crypto.encrypt(largeData);
    const decrypted = await crypto.decrypt(ciphertext);
    assert.equal(decrypted, largeData);
  });

  it('two peers with same key can cross-decrypt', async () => {
    const sessionKey = await generateSessionKey();
    const peerA = await createP2PCrypto(sessionKey);
    const peerB = await createP2PCrypto(sessionKey);
    const fromA = await peerA.encrypt('from A');
    const fromB = await peerB.encrypt('from B');
    assert.equal(await peerB.decrypt(fromA), 'from A');
    assert.equal(await peerA.decrypt(fromB), 'from B');
  });
});
