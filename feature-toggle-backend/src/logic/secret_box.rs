//! Encryption of secrets at rest (SSO client secrets).
//!
//! Values are sealed with AES-256-GCM under a key read once from the
//! `FLUXGATE_ENCRYPTION_KEY` environment variable (base64, 32 bytes). Each value
//! gets a fresh random 96-bit nonce and is stored as `base64(nonce || ciphertext)`,
//! where the ciphertext carries the GCM authentication tag. Decryption verifies the
//! tag, so a tampered value or a wrong key is an error, never garbage plaintext.
//!
//! Neither plaintext nor key material appears in error values or logs.

use aes_gcm::aead::{Aead, AeadCore, KeyInit, OsRng};
use aes_gcm::{Aes256Gcm, Key, Nonce};
use base64::Engine;
use base64::engine::general_purpose::STANDARD;
use std::sync::OnceLock;

/// Environment variable holding the base64 encoded 32 byte encryption key.
pub const ENCRYPTION_KEY_ENV: &str = "FLUXGATE_ENCRYPTION_KEY";

const KEY_LEN: usize = 32;
const NONCE_LEN: usize = 12;
const TAG_LEN: usize = 16;

#[derive(Debug, Clone, PartialEq, Eq, thiserror::Error)]
pub enum SecretBoxError {
    #[error("{ENCRYPTION_KEY_ENV} is not set")]
    KeyMissing,
    #[error("{ENCRYPTION_KEY_ENV} must be base64 encoding of exactly 32 bytes")]
    KeyInvalid,
    #[error("encrypted value is malformed")]
    InvalidCiphertext,
    #[error("failed to decrypt value: wrong key or tampered data")]
    DecryptionFailed,
    #[error("failed to encrypt value")]
    EncryptionFailed,
}

/// AES-256-GCM cipher bound to one key.
pub struct SecretBox {
    cipher: Aes256Gcm,
}

impl std::fmt::Debug for SecretBox {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("SecretBox").finish_non_exhaustive()
    }
}

impl SecretBox {
    /// Builds a cipher from a base64 encoded 32 byte key.
    pub fn from_base64_key(key: &str) -> Result<Self, SecretBoxError> {
        let bytes = STANDARD
            .decode(key.trim())
            .map_err(|_| SecretBoxError::KeyInvalid)?;
        if bytes.len() != KEY_LEN {
            return Err(SecretBoxError::KeyInvalid);
        }
        Ok(Self {
            cipher: Aes256Gcm::new(Key::<Aes256Gcm>::from_slice(&bytes)),
        })
    }

    /// Builds a cipher from an optional key value, as read from the environment.
    pub fn from_key_value(value: Option<&str>) -> Result<Self, SecretBoxError> {
        match value {
            Some(v) if !v.trim().is_empty() => Self::from_base64_key(v),
            _ => Err(SecretBoxError::KeyMissing),
        }
    }

    /// Encrypts `plaintext` with a fresh random nonce; returns `base64(nonce || ciphertext)`.
    pub fn encrypt(&self, plaintext: &str) -> Result<String, SecretBoxError> {
        let nonce = Aes256Gcm::generate_nonce(&mut OsRng);
        let ciphertext = self
            .cipher
            .encrypt(&nonce, plaintext.as_bytes())
            .map_err(|_| SecretBoxError::EncryptionFailed)?;
        let mut sealed = Vec::with_capacity(NONCE_LEN + ciphertext.len());
        sealed.extend_from_slice(&nonce);
        sealed.extend_from_slice(&ciphertext);
        Ok(STANDARD.encode(sealed))
    }

    /// Decrypts a value produced by [`SecretBox::encrypt`], verifying its authentication tag.
    pub fn decrypt(&self, sealed: &str) -> Result<String, SecretBoxError> {
        let bytes = STANDARD
            .decode(sealed.trim())
            .map_err(|_| SecretBoxError::InvalidCiphertext)?;
        if bytes.len() < NONCE_LEN + TAG_LEN {
            return Err(SecretBoxError::InvalidCiphertext);
        }
        let (nonce, ciphertext) = bytes.split_at(NONCE_LEN);
        let plaintext = self
            .cipher
            .decrypt(Nonce::from_slice(nonce), ciphertext)
            .map_err(|_| SecretBoxError::DecryptionFailed)?;
        String::from_utf8(plaintext).map_err(|_| SecretBoxError::DecryptionFailed)
    }
}

fn global() -> Result<&'static SecretBox, SecretBoxError> {
    static BOX: OnceLock<Result<SecretBox, SecretBoxError>> = OnceLock::new();
    BOX.get_or_init(|| SecretBox::from_key_value(std::env::var(ENCRYPTION_KEY_ENV).ok().as_deref()))
        .as_ref()
        .map_err(Clone::clone)
}

/// Whether `FLUXGATE_ENCRYPTION_KEY` is set to a valid key.
pub fn is_configured() -> bool {
    global().is_ok()
}

/// Encrypts with the key from `FLUXGATE_ENCRYPTION_KEY` (loaded once).
pub fn encrypt(plaintext: &str) -> Result<String, SecretBoxError> {
    global()?.encrypt(plaintext)
}

/// Decrypts with the key from `FLUXGATE_ENCRYPTION_KEY` (loaded once).
pub fn decrypt(sealed: &str) -> Result<String, SecretBoxError> {
    global()?.decrypt(sealed)
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::collections::HashSet;

    fn key(byte: u8) -> String {
        STANDARD.encode([byte; KEY_LEN])
    }

    fn secret_box(byte: u8) -> SecretBox {
        SecretBox::from_base64_key(&key(byte)).unwrap()
    }

    #[test]
    fn round_trip() {
        let sb = secret_box(1);
        for plaintext in ["", "s3cr3t-client-secret", "unicode: ключ 🔑"] {
            let sealed = sb.encrypt(plaintext).unwrap();
            assert_ne!(sealed, plaintext);
            assert_eq!(sb.decrypt(&sealed).unwrap(), plaintext);
        }
    }

    #[test]
    fn sealed_value_is_base64_of_nonce_then_ciphertext_with_tag() {
        let sealed = secret_box(1).encrypt("abc").unwrap();
        let bytes = STANDARD.decode(sealed).unwrap();
        assert_eq!(bytes.len(), NONCE_LEN + 3 + TAG_LEN);
    }

    #[test]
    fn wrong_key_fails() {
        let sealed = secret_box(1).encrypt("secret").unwrap();
        assert_eq!(
            secret_box(2).decrypt(&sealed),
            Err(SecretBoxError::DecryptionFailed)
        );
    }

    #[test]
    fn tampered_ciphertext_fails() {
        let sb = secret_box(1);
        let sealed = sb.encrypt("secret").unwrap();
        let bytes = STANDARD.decode(&sealed).unwrap();
        // Flip one bit in the nonce, the ciphertext body and the tag in turn.
        for index in [0, NONCE_LEN, bytes.len() - 1] {
            let mut tampered = bytes.clone();
            tampered[index] ^= 0x01;
            assert_eq!(
                sb.decrypt(&STANDARD.encode(tampered)),
                Err(SecretBoxError::DecryptionFailed),
                "flipping byte {index} must be detected"
            );
        }
    }

    #[test]
    fn truncated_or_malformed_input_fails() {
        let sb = secret_box(1);
        assert_eq!(
            sb.decrypt("not base64 !!!"),
            Err(SecretBoxError::InvalidCiphertext)
        );
        assert_eq!(sb.decrypt(""), Err(SecretBoxError::InvalidCiphertext));
        assert_eq!(
            sb.decrypt(&STANDARD.encode([0u8; NONCE_LEN + TAG_LEN - 1])),
            Err(SecretBoxError::InvalidCiphertext)
        );
    }

    #[test]
    fn nonce_is_unique_per_value() {
        let sb = secret_box(1);
        let sealed: Vec<String> = (0..200)
            .map(|_| sb.encrypt("same plaintext").unwrap())
            .collect();
        let distinct: HashSet<&String> = sealed.iter().collect();
        assert_eq!(
            distinct.len(),
            sealed.len(),
            "same plaintext must seal differently"
        );
        let nonces: HashSet<Vec<u8>> = sealed
            .iter()
            .map(|s| STANDARD.decode(s).unwrap()[..NONCE_LEN].to_vec())
            .collect();
        assert_eq!(nonces.len(), sealed.len());
    }

    #[test]
    fn missing_key_is_explicit_error() {
        assert_eq!(
            SecretBox::from_key_value(None).unwrap_err(),
            SecretBoxError::KeyMissing
        );
        assert_eq!(
            SecretBox::from_key_value(Some("  ")).unwrap_err(),
            SecretBoxError::KeyMissing
        );
    }

    #[test]
    fn invalid_key_is_explicit_error() {
        assert_eq!(
            SecretBox::from_base64_key("%%% not base64").unwrap_err(),
            SecretBoxError::KeyInvalid
        );
        // Valid base64 but 16 and 33 bytes.
        assert_eq!(
            SecretBox::from_base64_key(&STANDARD.encode([7u8; 16])).unwrap_err(),
            SecretBoxError::KeyInvalid
        );
        assert_eq!(
            SecretBox::from_base64_key(&STANDARD.encode([7u8; 33])).unwrap_err(),
            SecretBoxError::KeyInvalid
        );
    }

    #[test]
    fn key_with_surrounding_whitespace_is_accepted() {
        let sealed = secret_box(1).encrypt("x").unwrap();
        let padded = SecretBox::from_key_value(Some(&format!("  {}\n", key(1)))).unwrap();
        assert_eq!(padded.decrypt(&sealed).unwrap(), "x");
    }

    #[test]
    fn errors_do_not_leak_key_material() {
        let message = SecretBox::from_base64_key(&key(9)[..10])
            .unwrap_err()
            .to_string();
        assert!(!message.contains(&key(9)[..10]));
    }
}
