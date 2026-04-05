//! AES-256-GCM encrypted file-based keychain backend.
//!
//! Wire format (matches npm `credentials.js` exactly):
//! ```text
//! [IV (16 bytes)][AuthTag (16 bytes)][Ciphertext]
//! ```
//! Stored as base64 in `{config_dir}/{sanitized_key}.enc`.
//!
//! Key derivation: `SHA256(hostname:uid:mxdx-credential-store)` where `uid` is
//! the numeric Unix UID (matching Node.js `os.userInfo().uid`) or the username
//! on non-Unix platforms.

use std::path::PathBuf;

use aes_gcm::{
    aead::{Aead, KeyInit, OsRng, generic_array::typenum::U16},
    aes::Aes256,
    AesGcm, AeadCore, Nonce,
};
use anyhow::{Context, Result};
use base64::Engine;
use sha2::{Digest, Sha256};

use crate::identity::KeychainBackend;

/// AES-256-GCM with 16-byte (128-bit) nonce, matching Node.js crypto's default IV size.
type Aes256Gcm16 = AesGcm<Aes256, U16>;

/// File-based keychain backend with AES-256-GCM encryption at rest.
pub struct FileKeychain {
    config_dir: PathBuf,
    key: [u8; 32],
}

impl FileKeychain {
    /// Create a new `FileKeychain` with the default config directory (`~/.config/mxdx`)
    /// and a key derived from `SHA256(hostname:uid:mxdx-credential-store)`.
    ///
    /// If the `MXDX_KEYCHAIN_DIR` environment variable is set, uses that directory
    /// instead of the default. This is useful for test isolation.
    pub fn new() -> Result<Self> {
        let config_dir = if let Ok(dir) = std::env::var("MXDX_KEYCHAIN_DIR") {
            PathBuf::from(dir)
        } else {
            dirs::config_dir()
                .unwrap_or_else(|| PathBuf::from("~/.config"))
                .join("mxdx")
        };
        let key = derive_key()?;
        Ok(Self { config_dir, key })
    }

    /// Create a `FileKeychain` with an explicit config directory and key.
    /// Useful for testing.
    pub fn with_dir_and_key(config_dir: PathBuf, key: [u8; 32]) -> Self {
        Self { config_dir, key }
    }

    fn file_path(&self, key: &str) -> PathBuf {
        let sanitized = sanitize_key(key);
        self.config_dir.join(format!("{sanitized}.enc"))
    }

    fn ensure_dir(&self) -> Result<()> {
        #[cfg(unix)]
        {
            use std::os::unix::fs::DirBuilderExt;
            std::fs::DirBuilder::new()
                .recursive(true)
                .mode(0o700)
                .create(&self.config_dir)
                .context("failed to create keychain config directory")?;
        }
        #[cfg(not(unix))]
        {
            std::fs::create_dir_all(&self.config_dir)
                .context("failed to create keychain config directory")?;
        }
        Ok(())
    }
}

impl KeychainBackend for FileKeychain {
    fn get(&self, key: &str) -> Result<Option<Vec<u8>>> {
        let path = self.file_path(key);
        if !path.exists() {
            return Ok(None);
        }
        let encoded = std::fs::read_to_string(&path)
            .with_context(|| format!("failed to read keychain file: {}", path.display()))?;
        let plaintext = decrypt(&encoded.trim(), &self.key)?;
        Ok(Some(plaintext.into_bytes()))
    }

    fn set(&self, key: &str, value: &[u8]) -> Result<()> {
        self.ensure_dir()?;
        let plaintext =
            std::str::from_utf8(value).context("file keychain value must be valid UTF-8")?;
        let encoded = encrypt(plaintext, &self.key)?;
        let path = self.file_path(key);
        std::fs::write(&path, &encoded)
            .with_context(|| format!("failed to write keychain file: {}", path.display()))?;
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            std::fs::set_permissions(&path, std::fs::Permissions::from_mode(0o600))
                .context("failed to set file permissions to 0o600")?;
        }
        Ok(())
    }

    fn delete(&self, key: &str) -> Result<()> {
        let path = self.file_path(key);
        if path.exists() {
            std::fs::remove_file(&path)
                .with_context(|| format!("failed to delete keychain file: {}", path.display()))?;
        }
        Ok(())
    }
}

// ---------------------------------------------------------------------------
// Key derivation — must match npm's credentials.js exactly
// ---------------------------------------------------------------------------

/// Derive the encryption key: `SHA256(hostname:uid:mxdx-credential-store)`.
///
/// On Unix, `uid` is the numeric user ID (matching `os.userInfo().uid` in Node.js).
/// On non-Unix, `uid` is the username string.
fn derive_key() -> Result<[u8; 32]> {
    let host = hostname::get()
        .context("failed to get hostname")?
        .to_string_lossy()
        .to_string();
    let uid = get_uid();
    let material = format!("{host}:{uid}:mxdx-credential-store");
    let hash = Sha256::digest(material.as_bytes());
    let mut key = [0u8; 32];
    key.copy_from_slice(&hash);
    Ok(key)
}

/// Derive a key from explicit material (for testing).
#[cfg(test)]
fn derive_key_from_material(material: &str) -> [u8; 32] {
    let hash = Sha256::digest(material.as_bytes());
    let mut key = [0u8; 32];
    key.copy_from_slice(&hash);
    key
}

#[cfg(unix)]
fn get_uid() -> String {
    // SAFETY: libc::getuid() is always safe to call, returns u32.
    unsafe { libc::getuid() }.to_string()
}

#[cfg(not(unix))]
fn get_uid() -> String {
    whoami::username()
}

// ---------------------------------------------------------------------------
// AES-256-GCM encrypt/decrypt matching npm wire format
// ---------------------------------------------------------------------------

/// Encrypt plaintext to `base64(IV || AuthTag || Ciphertext)`.
fn encrypt(plaintext: &str, key: &[u8; 32]) -> Result<String> {
    let cipher = Aes256Gcm16::new_from_slice(key).context("invalid AES key")?;
    let nonce = Aes256Gcm16::generate_nonce(&mut OsRng);

    // aes-gcm crate appends the tag to the ciphertext
    let ciphertext_with_tag = cipher
        .encrypt(&nonce, plaintext.as_bytes())
        .map_err(|e| anyhow::anyhow!("AES-GCM encryption failed: {e}"))?;

    // The crate returns ciphertext || tag (tag is last 16 bytes)
    let ct_len = ciphertext_with_tag.len() - 16;
    let ciphertext = &ciphertext_with_tag[..ct_len];
    let tag = &ciphertext_with_tag[ct_len..];

    // npm wire format: IV (16) || AuthTag (16) || Ciphertext
    let mut wire = Vec::with_capacity(16 + 16 + ciphertext.len());
    wire.extend_from_slice(nonce.as_slice());
    wire.extend_from_slice(tag);
    wire.extend_from_slice(ciphertext);

    Ok(base64::engine::general_purpose::STANDARD.encode(&wire))
}

/// Decrypt from `base64(IV || AuthTag || Ciphertext)`.
fn decrypt(encoded: &str, key: &[u8; 32]) -> Result<String> {
    let wire = base64::engine::general_purpose::STANDARD
        .decode(encoded)
        .context("invalid base64 in keychain file")?;

    if wire.len() < 32 {
        anyhow::bail!("keychain file too short (expected at least 32 bytes for IV + AuthTag)");
    }

    let iv = &wire[..16];
    let tag = &wire[16..32];
    let ciphertext = &wire[32..];

    let cipher = Aes256Gcm16::new_from_slice(key).context("invalid AES key")?;
    let nonce = Nonce::from_slice(iv);

    // Reconstruct ciphertext || tag format that aes-gcm expects
    let mut ct_with_tag = Vec::with_capacity(ciphertext.len() + 16);
    ct_with_tag.extend_from_slice(ciphertext);
    ct_with_tag.extend_from_slice(tag);

    let plaintext = cipher
        .decrypt(nonce, ct_with_tag.as_ref())
        .map_err(|e| anyhow::anyhow!("AES-GCM decryption failed: {e}"))?;

    String::from_utf8(plaintext).context("decrypted value is not valid UTF-8")
}

// ---------------------------------------------------------------------------
// Key sanitization — matches npm's `key.replace(/[^a-zA-Z0-9._-]/g, '_')`
// ---------------------------------------------------------------------------

fn sanitize_key(key: &str) -> String {
    key.chars()
        .map(|c| {
            if c.is_ascii_alphanumeric() || c == '.' || c == '_' || c == '-' {
                c
            } else {
                '_'
            }
        })
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;
    use tempfile::TempDir;

    fn test_keychain(dir: &std::path::Path) -> FileKeychain {
        let key = derive_key_from_material("testhost:1000:mxdx-credential-store");
        FileKeychain::with_dir_and_key(dir.to_path_buf(), key)
    }

    #[test]
    fn test_file_keychain_roundtrip() {
        let tmp = TempDir::new().unwrap();
        let kc = test_keychain(tmp.path());

        kc.set("test-key", b"hello-world").unwrap();
        let val = kc.get("test-key").unwrap();
        assert_eq!(val, Some(b"hello-world".to_vec()));
    }

    #[test]
    fn test_file_keychain_delete() {
        let tmp = TempDir::new().unwrap();
        let kc = test_keychain(tmp.path());

        kc.set("del-key", b"to-delete").unwrap();
        kc.delete("del-key").unwrap();
        let val = kc.get("del-key").unwrap();
        assert_eq!(val, None);
    }

    #[test]
    fn test_file_keychain_overwrite() {
        let tmp = TempDir::new().unwrap();
        let kc = test_keychain(tmp.path());

        kc.set("ow-key", b"first").unwrap();
        kc.set("ow-key", b"second").unwrap();
        let val = kc.get("ow-key").unwrap();
        assert_eq!(val, Some(b"second".to_vec()));
    }

    #[test]
    fn test_file_keychain_get_nonexistent() {
        let tmp = TempDir::new().unwrap();
        let kc = test_keychain(tmp.path());

        let val = kc.get("no-such-key").unwrap();
        assert_eq!(val, None);
    }

    #[cfg(unix)]
    #[test]
    fn test_file_keychain_permissions() {
        use std::os::unix::fs::PermissionsExt;

        let tmp = TempDir::new().unwrap();
        let kc = test_keychain(tmp.path());

        kc.set("perm-key", b"secret").unwrap();

        // Check file permissions
        let file_path = tmp.path().join("perm-key.enc");
        let file_perms = std::fs::metadata(&file_path).unwrap().permissions().mode() & 0o777;
        assert_eq!(file_perms, 0o600, "file should be 0o600");

        // Check directory permissions — since we use TempDir, the dir already exists.
        // The ensure_dir won't set perms on an existing dir, so we test with a subdirectory.
        let sub_dir = tmp.path().join("sub");
        let kc2 = FileKeychain::with_dir_and_key(
            sub_dir.clone(),
            derive_key_from_material("testhost:1000:mxdx-credential-store"),
        );
        kc2.set("sub-key", b"val").unwrap();
        let dir_perms = std::fs::metadata(&sub_dir).unwrap().permissions().mode() & 0o777;
        assert_eq!(dir_perms, 0o700, "directory should be 0o700");
    }

    #[test]
    fn test_file_keychain_key_sanitization() {
        assert_eq!(
            sanitize_key("mxdx:alice@matrix.org:session"),
            "mxdx_alice_matrix.org_session"
        );
    }

    #[test]
    fn test_encrypt_decrypt_roundtrip() {
        let key = derive_key_from_material("test:0:mxdx-credential-store");
        let plaintext = "hello, encrypted world!";
        let encoded = encrypt(plaintext, &key).unwrap();
        let decrypted = decrypt(&encoded, &key).unwrap();
        assert_eq!(decrypted, plaintext);
    }

    #[test]
    fn test_decrypt_with_wrong_key_fails() {
        let key1 = derive_key_from_material("host1:1000:mxdx-credential-store");
        let key2 = derive_key_from_material("host2:1000:mxdx-credential-store");
        let encoded = encrypt("secret", &key1).unwrap();
        assert!(decrypt(&encoded, &key2).is_err());
    }
}
