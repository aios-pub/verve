//! Encrypted local vault for SSH credentials.
//!
//! When the OS keychain is unavailable (or the user prefers a master-password
//! vault), secrets are stored in `~/.verve/ssh_vault.bin`. The file format is:
//!
//! ```text
//! [16-byte salt] [12-byte nonce] [AES-256-GCM ciphertext + tag]
//! ```
//!
//! The encryption key is derived from the user's master password using Argon2id
//! (memory-hard KDF). All secrets are serialized as JSON inside the ciphertext.

use std::fs;
use std::path::PathBuf;

use aes_gcm::aead::rand_core::RngCore;
use aes_gcm::aead::{Aead, KeyInit, OsRng as AeadOsRng};
use aes_gcm::{Aes256Gcm, KeyInit as _, Nonce};
use argon2::Argon2;
use serde::{Deserialize, Serialize};
use zeroize::Zeroize;

/// The salt length for Argon2 key derivation.
const SALT_LEN: usize = 16;
/// The nonce length for AES-256-GCM.
const NONCE_LEN: usize = 12;
/// The derived key length (AES-256 = 32 bytes).
const KEY_LEN: usize = 32;

/// The encrypted vault data stored on disk.
#[derive(Serialize, Deserialize)]
struct VaultFile {
    /// Argon2 salt (stored in cleartext — it's not secret).
    salt: Vec<u8>,
    /// AES-GCM nonce (stored in cleartext — unique per encryption).
    nonce: Vec<u8>,
    /// The encrypted ciphertext (includes the GCM auth tag).
    ciphertext: Vec<u8>,
}

/// The path to the vault file: `~/.verve/ssh_vault.bin`.
pub fn vault_path() -> Option<PathBuf> {
    crate::state::persistence::data_dir()
        .ok()
        .map(|d| d.join("ssh_vault.bin"))
}

/// Derive a 256-bit key from a master password + salt using Argon2id.
fn derive_key(master_pw: &str, salt: &[u8]) -> Result<[u8; KEY_LEN], argon2::Error> {
    let mut key = [0u8; KEY_LEN];
    Argon2::default().hash_password_into(master_pw.as_bytes(), salt, &mut key)?;
    Ok(key)
}

/// Encrypt a plaintext byte slice with the given master password.
/// Returns the serialized vault file bytes.
pub fn encrypt(master_pw: &str, plaintext: &[u8]) -> Result<Vec<u8>, String> {
    // Generate a random salt (new per encryption for forward secrecy).
    let mut salt = vec![0u8; SALT_LEN];
    AeadOsRng.fill_bytes(&mut salt);

    let key = derive_key(master_pw, &salt).map_err(|e| format!("Argon2: {e}"))?;
    let cipher = Aes256Gcm::new_from_slice(&key).map_err(|e| format!("AES key: {e}"))?;

    // Generate a random nonce.
    let mut nonce_bytes = vec![0u8; NONCE_LEN];
    AeadOsRng.fill_bytes(&mut nonce_bytes);
    let nonce = Nonce::from_slice(&nonce_bytes);

    let ciphertext = cipher
        .encrypt(nonce, plaintext)
        .map_err(|e| format!("AES-GCM encrypt: {e}"))?;

    let vault = VaultFile {
        salt,
        nonce: nonce_bytes,
        ciphertext,
    };

    let mut key_arr = key;
    key_arr.zeroize();

    bincode::serialize(&vault).map_err(|e| format!("serialize: {e}"))
}

/// Decrypt vault file bytes with the given master password.
pub fn decrypt(master_pw: &str, data: &[u8]) -> Result<Vec<u8>, String> {
    let vault: VaultFile =
        bincode::deserialize(data).map_err(|e| format!("deserialize vault: {e}"))?;

    let key = derive_key(master_pw, &vault.salt).map_err(|e| format!("Argon2: {e}"))?;
    let cipher = Aes256Gcm::new_from_slice(&key).map_err(|e| format!("AES key: {e}"))?;

    let nonce = Nonce::from_slice(&vault.nonce);
    let plaintext = cipher
        .decrypt(nonce, vault.ciphertext.as_ref())
        .map_err(|_| "解密失败：主密码错误或数据已损坏".to_string())?;

    let mut key_arr = key;
    key_arr.zeroize();

    Ok(plaintext)
}

/// The in-memory secrets map stored inside the vault.
/// Maps credential keys → secret values.
pub type SecretMap = std::collections::HashMap<String, String>;

/// Load and decrypt the vault's secret map. Returns an empty map if the vault
/// file doesn't exist yet.
pub fn load_vault(master_pw: &str) -> Result<SecretMap, String> {
    let Some(path) = vault_path() else {
        return Ok(SecretMap::new());
    };
    match fs::read(&path) {
        Ok(data) => {
            let plaintext = decrypt(master_pw, &data)?;
            if plaintext.is_empty() {
                Ok(SecretMap::new())
            } else {
                serde_json::from_slice(&plaintext).map_err(|e| format!("parse vault JSON: {e}"))
            }
        }
        Err(_) => Ok(SecretMap::new()),
    }
}

/// Encrypt and save the secret map to the vault file.
pub fn save_vault(master_pw: &str, secrets: &SecretMap) -> Result<(), String> {
    let Some(path) = vault_path() else {
        return Err("vault path unavailable".into());
    };
    let plaintext = serde_json::to_vec(secrets).map_err(|e| format!("serialize: {e}"))?;
    let encrypted = encrypt(master_pw, &plaintext)?;
    fs::write(&path, encrypted).map_err(|e| format!("write vault: {e}"))
}

/// Check whether a vault file exists.
pub fn vault_exists() -> bool {
    vault_path().map(|p| p.exists()).unwrap_or(false)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn encrypt_decrypt_round_trip() {
        let pw = "my_master_password_123!";
        let data = b"{\"host1:password\":\"s3cret\",\"host2:password\":\"hunter2\"}";
        let encrypted = encrypt(pw, data).unwrap();
        let decrypted = decrypt(pw, &encrypted).unwrap();
        assert_eq!(decrypted.as_slice(), data);
    }

    #[test]
    fn wrong_password_fails() {
        let encrypted = encrypt("correct_pw", b"secret data").unwrap();
        let result = decrypt("wrong_pw", &encrypted);
        assert!(result.is_err());
    }

    #[test]
    fn vault_load_save_round_trip() {
        // Isolate data_dir() at a unique per-thread temp dir (no HOME mutation,
        // no mutex) so this test can't race with sibling data-dir tests.
        let tmp = std::env::temp_dir().join(format!(
            "verve-vault-test-{}",
            crate::state::models::new_id()
        ));
        std::fs::create_dir_all(&tmp).unwrap();
        let _guard = crate::state::persistence::set_thread_data_dir(tmp.clone());
        let pw = "master123";
        let mut map = SecretMap::new();
        map.insert("host1:password".into(), "s3cret".into());
        let result = save_vault(pw, &map);

        let _ = std::fs::remove_dir_all(&tmp);
        result.unwrap();
    }
}
