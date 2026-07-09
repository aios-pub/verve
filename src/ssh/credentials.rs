//! Unified credential storage interface.
//!
//! Secrets (passwords, key passphrases) are stored via one of two backends:
//!
//! - **Keychain** (default): OS-native secure storage via the `keyring` crate.
//!   macOS Keychain / Windows Credential Manager / Linux libsecret.
//! - **Vault**: encrypted local file (`ssh_vault.bin`) using AES-256-GCM +
//!   Argon2. Used when keychain is unavailable or the user explicitly chose it.
//!
//! The caller decides which backend to use via [`CredentialSource`]. The
//! keyring `Entry` key is `"{host_id}:{kind}"` (e.g. `"abc123:password"`).

use std::sync::{Arc, Mutex};

use super::models::CredentialSource;
use super::vault;

/// The service name used for keyring entries.
const KEYRING_SERVICE: &str = "verve.ssh";

/// In-memory cache of the decrypted vault (populated on unlock). The vault
/// master password is NOT stored here — only the decrypted secret map.
static VAULT_CACHE: Mutex<Option<Arc<vault::SecretMap>>> = Mutex::new(None);

/// The service prefix for keyring entries.
pub fn keyring_key(host_id: &str, kind: &str) -> String {
    format!("{host_id}:{kind}")
}

/// Store a secret in the OS keychain.
pub fn store_keychain(host_id: &str, kind: &str, secret: &str) -> Result<(), String> {
    let key = keyring_key(host_id, kind);
    let entry = keyring::Entry::new(KEYRING_SERVICE, &key)
        .map_err(|e| format!("keyring entry: {e}"))?;
    entry
        .set_password(secret)
        .map_err(|e| format!("keyring store: {e}"))
}

/// Load a secret from the OS keychain.
pub fn load_keychain(host_id: &str, kind: &str) -> Result<String, String> {
    let key = keyring_key(host_id, kind);
    let entry = keyring::Entry::new(KEYRING_SERVICE, &key)
        .map_err(|e| format!("keyring entry: {e}"))?;
    entry
        .get_password()
        .map_err(|e| format!("keyring load: {e}"))
}

/// Delete a secret from the OS keychain (silently succeeds if not found).
pub fn delete_keychain(host_id: &str, kind: &str) -> Result<(), String> {
    let key = keyring_key(host_id, kind);
    let entry = keyring::Entry::new(KEYRING_SERVICE, &key)
        .map_err(|e| format!("keyring entry: {e}"))?;
    let _ = entry.delete_credential();
    Ok(())
}

/// Unlock the encrypted vault with a master password, caching the decrypted
/// secret map in memory.
pub fn unlock_vault(master_pw: &str) -> Result<(), String> {
    let map = vault::load_vault(master_pw)?;
    let mut guard = VAULT_CACHE.lock().unwrap();
    *guard = Some(Arc::new(map));
    Ok(())
}

/// Lock the vault (clear the in-memory cache).
pub fn lock_vault() {
    let mut guard = VAULT_CACHE.lock().unwrap();
    *guard = None;
}

/// Whether the vault is currently unlocked.
pub fn vault_is_unlocked() -> bool {
    VAULT_CACHE.lock().unwrap().is_some()
}

/// Store a secret in the vault (requires prior `unlock_vault`).
pub fn store_vault(host_id: &str, kind: &str, secret: &str, master_pw: &str) -> Result<(), String> {
    let mut map = vault::load_vault(master_pw)?;
    map.insert(keyring_key(host_id, kind), secret.to_string());
    vault::save_vault(master_pw, &map)?;
    // Update cache.
    let mut guard = VAULT_CACHE.lock().unwrap();
    *guard = Some(Arc::new(map));
    Ok(())
}

/// Load a secret from the vault (requires prior `unlock_vault`).
pub fn load_vault_secret(host_id: &str, kind: &str) -> Result<String, String> {
    let guard = VAULT_CACHE.lock().unwrap();
    guard
        .as_ref()
        .and_then(|map| map.get(&keyring_key(host_id, kind)).cloned())
        .ok_or_else(|| "vault locked or secret not found".to_string())
}

/// Delete a secret from the vault.
pub fn delete_vault_secret(host_id: &str, kind: &str, master_pw: &str) -> Result<(), String> {
    let mut map = vault::load_vault(master_pw)?;
    map.remove(&keyring_key(host_id, kind));
    vault::save_vault(master_pw, &map)?;
    let mut guard = VAULT_CACHE.lock().unwrap();
    *guard = Some(Arc::new(map));
    Ok(())
}

// ---- Unified interface (source-agnostic) ----

/// Store a secret using the specified backend.
pub fn store_secret(
    host_id: &str,
    kind: &str,
    secret: &str,
    source: CredentialSource,
    master_pw: Option<&str>,
) -> Result<(), String> {
    match source {
        CredentialSource::Keychain => store_keychain(host_id, kind, secret),
        CredentialSource::Vault => {
            let pw = master_pw.ok_or("vault requires master password")?;
            store_vault(host_id, kind, secret, pw)
        }
    }
}

/// Load a secret from the specified backend.
pub fn load_secret(
    host_id: &str,
    kind: &str,
    source: CredentialSource,
    _master_pw: Option<&str>,
) -> Result<String, String> {
    match source {
        CredentialSource::Keychain => load_keychain(host_id, kind),
        CredentialSource::Vault => load_vault_secret(host_id, kind),
    }
}

/// Delete a secret from both backends (best-effort).
pub fn delete_secret(host_id: &str, kind: &str) -> Result<(), String> {
    let _ = delete_keychain(host_id, kind);
    // Vault deletion needs master_pw — skip here (cleanup happens on host delete).
    Ok(())
}

/// Whether the OS keychain is available on this platform.
pub fn keychain_available() -> bool {
    // The keyring crate is compiled per-platform; if it links, the keychain
    // backend is available. We do a probe entry to check at runtime.
    keyring::Entry::new(KEYRING_SERVICE, "__probe__").is_ok()
}
