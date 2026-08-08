//! Device-key-encrypted local vault for SSH credentials.
//!
//! Secrets (passwords, key passphrases) are stored in `~/.verve/ssh_vault.bin`,
//! an encrypted file sealed with a device-specific key. The device key itself
//! lives in `~/.verve/device.key` (created once, permission `0600` on Unix).
//!
//! This design mirrors commercial SSH clients (e.g. Termius): no master
//! password to remember and **no OS keychain authorization prompt**. The
//! device key + vault are unlocked automatically at runtime; security comes
//! from filesystem permissions (per-user isolation) plus authenticated
//! encryption (AES-256-GCM).
//!
//! # Vault file format
//!
//! ```text
//! [12-byte nonce][AES-256-GCM ciphertext + GCM tag]
//! ```
//!
//! The plaintext inside the ciphertext is a JSON-serialized [`SecretMap`].

use std::fs;

use aes_gcm::aead::rand_core::RngCore;
use aes_gcm::aead::{Aead, KeyInit, OsRng as AeadOsRng};
use aes_gcm::{Aes256Gcm, KeyInit as _, Nonce};
use serde::{Deserialize, Serialize};
use zeroize::Zeroize;

/// The nonce length for AES-256-GCM (also written at the start of the file).
const NONCE_LEN: usize = 12;
/// The derived key length (AES-256 = 32 bytes).
pub const KEY_LEN: usize = 32;

/// The encrypted vault data stored on disk (bincode-serialized).
///
/// Only the nonce is stored in cleartext — there is no salt because the key
/// is no longer derived from a password; it is the raw device key.
#[derive(Serialize, Deserialize)]
struct VaultFile {
    /// AES-GCM nonce (stored in cleartext — unique per encryption).
    nonce: Vec<u8>,
    /// The encrypted ciphertext (includes the GCM auth tag).
    ciphertext: Vec<u8>,
}

/// The path to the vault file: `~/.verve/ssh_vault.bin`.
pub fn vault_path() -> Option<std::path::PathBuf> {
    crate::state::persistence::data_dir()
        .ok()
        .map(|d| d.join("ssh_vault.bin"))
}

/// The path to the device key: `~/.verve/device.key`.
pub fn device_key_path() -> Option<std::path::PathBuf> {
    crate::state::persistence::data_dir()
        .ok()
        .map(|d| d.join("device.key"))
}

/// Encrypt a plaintext byte slice with a raw 32-byte key.
/// Returns the serialized vault file bytes (`[nonce][ciphertext+tag]`).
pub fn encrypt(key: &[u8; KEY_LEN], plaintext: &[u8]) -> Result<Vec<u8>, String> {
    let cipher = Aes256Gcm::new_from_slice(key).map_err(|e| format!("AES key: {e}"))?;

    // Generate a fresh random nonce per encryption (forward secrecy).
    let mut nonce_bytes = vec![0u8; NONCE_LEN];
    AeadOsRng.fill_bytes(&mut nonce_bytes);
    let nonce = Nonce::from_slice(&nonce_bytes);

    let ciphertext = cipher
        .encrypt(nonce, plaintext)
        .map_err(|e| format!("AES-GCM encrypt: {e}"))?;

    let vault = VaultFile {
        nonce: nonce_bytes,
        ciphertext,
    };

    bincode::serialize(&vault).map_err(|e| format!("serialize: {e}"))
}

/// Decrypt vault file bytes with a raw 32-byte key.
pub fn decrypt(key: &[u8; KEY_LEN], data: &[u8]) -> Result<Vec<u8>, String> {
    let vault: VaultFile =
        bincode::deserialize(data).map_err(|e| format!("deserialize vault: {e}"))?;

    let cipher = Aes256Gcm::new_from_slice(key).map_err(|e| format!("AES key: {e}"))?;
    let nonce = Nonce::from_slice(&vault.nonce);
    let plaintext = cipher
        .decrypt(nonce, vault.ciphertext.as_ref())
        .map_err(|_| "解密失败：设备密钥不匹配或数据已损坏".to_string())?;

    Ok(plaintext)
}

/// The in-memory secrets map stored inside the vault.
/// Maps credential keys (`"{host_id}:{kind}"`) → secret values.
pub type SecretMap = std::collections::HashMap<String, String>;

/// Load and decrypt the vault's secret map with the device key. Returns an
/// empty map if the vault file doesn't exist yet (first run).
pub fn load_vault(key: &[u8; KEY_LEN]) -> Result<SecretMap, String> {
    let Some(path) = vault_path() else {
        return Ok(SecretMap::new());
    };
    match fs::read(&path) {
        Ok(data) => {
            let plaintext = decrypt(key, &data)?;
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
pub fn save_vault(key: &[u8; KEY_LEN], secrets: &SecretMap) -> Result<(), String> {
    let Some(path) = vault_path() else {
        return Err("vault path unavailable".into());
    };
    let plaintext = serde_json::to_vec(secrets).map_err(|e| format!("serialize: {e}"))?;
    let encrypted = encrypt(key, &plaintext)?;
    fs::write(&path, encrypted).map_err(|e| format!("write vault: {e}"))
}

/// Check whether a vault file exists.
pub fn vault_exists() -> bool {
    vault_path().map(|p| p.exists()).unwrap_or(false)
}

/// Load the device key from `~/.verve/device.key`, creating it on first run.
///
/// On first run a random 32-byte key is generated and written with permission
/// `0600` on Unix so other users on the machine cannot read it. The returned
/// key is the caller's responsibility; pass it to [`zeroize`] when done.
pub fn load_or_create_device_key() -> Result<[u8; KEY_LEN], String> {
    let Some(path) = device_key_path() else {
        return Err("device key path unavailable".into());
    };
    // Fast path: the key already exists — read it.
    if let Ok(bytes) = fs::read(&path) {
        if bytes.len() == KEY_LEN {
            let mut key = [0u8; KEY_LEN];
            key.copy_from_slice(&bytes);
            return Ok(key);
        }
        // Wrong length — treat as corrupt, fall through to regenerate.
        log::warn!(
            "device.key has unexpected length {}; regenerating",
            bytes.len()
        );
    }
    // Slow path: first run (or corrupt key) — generate and persist.
    let mut key = [0u8; KEY_LEN];
    AeadOsRng.fill_bytes(&mut key);
    write_secret_file(&path, &key)?;
    Ok(key)
}

/// Write a file and tighten its permissions to `0600` on Unix (`0700` for
/// directories). Falls back to plain `write` on non-Unix (Windows relies on
/// the user-profile NTFS ACL).
pub(crate) fn write_secret_file(path: &std::path::Path, data: &[u8]) -> Result<(), String> {
    fs::write(path, data).map_err(|e| format!("write {}: {e}", path.display()))?;
    restrict_file_permissions(path);
    Ok(())
}

/// Restrict a file to `0600` on Unix (owner read/write only). No-op elsewhere.
pub(crate) fn restrict_file_permissions(path: &std::path::Path) {
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt as _;
        if let Err(e) = fs::set_permissions(path, fs::Permissions::from_mode(0o600)) {
            log::warn!("chmod 0600 {}: {e}", path.display());
        }
    }
    #[cfg(not(unix))]
    {
        let _ = path;
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn encrypt_decrypt_round_trip() {
        let mut key = [0u8; KEY_LEN];
        AeadOsRng.fill_bytes(&mut key);
        let data = b"{\"host1:password\":\"s3cret\",\"host2:password\":\"hunter2\"}";
        let encrypted = encrypt(&key, data).unwrap();
        let decrypted = decrypt(&key, &encrypted).unwrap();
        assert_eq!(decrypted.as_slice(), data);
        key.zeroize();
    }

    #[test]
    fn wrong_key_fails() {
        let mut key = [0u8; KEY_LEN];
        AeadOsRng.fill_bytes(&mut key);
        let encrypted = encrypt(&key, b"secret data").unwrap();
        let mut wrong_key = [0u8; KEY_LEN];
        AeadOsRng.fill_bytes(&mut wrong_key);
        let result = decrypt(&wrong_key, &encrypted);
        assert!(result.is_err());
        key.zeroize();
        wrong_key.zeroize();
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

        let mut key = [0u8; KEY_LEN];
        AeadOsRng.fill_bytes(&mut key);
        let mut map = SecretMap::new();
        map.insert("host1:password".into(), "s3cret".into());
        let result = save_vault(&key, &map);
        let loaded = load_vault(&key).unwrap();
        assert_eq!(
            loaded.get("host1:password").map(String::as_str),
            Some("s3cret")
        );

        let _ = std::fs::remove_dir_all(&tmp);
        result.unwrap();
        key.zeroize();
    }

    #[test]
    fn device_key_is_created_once_and_stable() {
        let tmp = std::env::temp_dir().join(format!(
            "verve-devicekey-test-{}",
            crate::state::models::new_id()
        ));
        std::fs::create_dir_all(&tmp).unwrap();
        let _guard = crate::state::persistence::set_thread_data_dir(tmp.clone());

        let key1 = load_or_create_device_key().unwrap();
        // Second call must return the SAME key (read, not regenerate).
        let key2 = load_or_create_device_key().unwrap();
        assert_eq!(key1.as_slice(), key2.as_slice());

        let _ = std::fs::remove_dir_all(&tmp);
        let mut k1 = key1;
        let mut k2 = key2;
        k1.zeroize();
        k2.zeroize();
    }
}
