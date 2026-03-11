/**
 * P2P session encryption using AES-256-GCM via the Web Crypto API.
 *
 * Session key is exchanged via E2EE Matrix signaling (m.call.invite/answer),
 * so it's authenticated by the Megolm layer. This means:
 *   - Only the two Matrix-authenticated peers have the key
 *   - AES-GCM provides authenticated encryption (detects tampering)
 *   - No separate signing/verification is needed for peer identity
 *
 * Works in both browser and Node.js 22+ (both provide crypto.subtle).
 */

/**
 * Generate a random 256-bit session key and return it as a base64 string.
 * The key is exportable so it can be included in Matrix signaling events.
 */
export async function generateSessionKey() {
  const key = await crypto.subtle.generateKey(
    { name: 'AES-GCM', length: 256 },
    true, // extractable — needed to send via Matrix
    ['encrypt', 'decrypt'],
  );
  const raw = await crypto.subtle.exportKey('raw', key);
  return arrayToBase64(new Uint8Array(raw));
}

/**
 * Create a P2PCrypto instance from a base64-encoded session key.
 * The imported key is non-extractable for defense-in-depth.
 */
export async function createP2PCrypto(base64Key) {
  const raw = base64ToArray(base64Key);
  if (raw.length !== 32) {
    throw new Error(`Invalid session key length: ${raw.length} (expected 32)`);
  }
  const key = await crypto.subtle.importKey(
    'raw',
    raw,
    { name: 'AES-GCM' },
    false, // non-extractable once imported
    ['encrypt', 'decrypt'],
  );
  return new P2PCrypto(key);
}

export class P2PCrypto {
  #aesKey;

  constructor(aesKey) {
    this.#aesKey = aesKey;
  }

  /**
   * Encrypt plaintext string → JSON string with ciphertext + IV.
   * Uses a random 96-bit IV per frame (standard for AES-GCM).
   */
  async encrypt(plaintext) {
    const iv = crypto.getRandomValues(new Uint8Array(12));
    const encoded = new TextEncoder().encode(plaintext);
    const ciphertext = await crypto.subtle.encrypt(
      { name: 'AES-GCM', iv },
      this.#aesKey,
      encoded,
    );
    return JSON.stringify({
      c: arrayToBase64(new Uint8Array(ciphertext)),
      iv: arrayToBase64(iv),
    });
  }

  /**
   * Decrypt JSON string → plaintext string.
   * Returns null on decryption failure (tampered/wrong key).
   */
  async decrypt(ciphertextJson) {
    const { c, iv } = JSON.parse(ciphertextJson);
    const ciphertext = base64ToArray(c);
    const ivBytes = base64ToArray(iv);
    const plaintext = await crypto.subtle.decrypt(
      { name: 'AES-GCM', iv: ivBytes },
      this.#aesKey,
      ciphertext,
    );
    return new TextDecoder().decode(plaintext);
  }
}

// --- Base64 helpers (no Node.js Buffer dependency) ---

function arrayToBase64(bytes) {
  let binary = '';
  for (let i = 0; i < bytes.length; i++) {
    binary += String.fromCharCode(bytes[i]);
  }
  return btoa(binary);
}

function base64ToArray(b64) {
  const binary = atob(b64);
  const bytes = new Uint8Array(binary.length);
  for (let i = 0; i < binary.length; i++) {
    bytes[i] = binary.charCodeAt(i);
  }
  return bytes;
}
