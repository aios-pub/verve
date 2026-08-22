//! Unified credential storage interface (device-key encrypted vault).
//!
//! Secrets (passwords, key passphrases) are stored in `~/.verve/ssh_vault.bin`,
//! an AES-256-GCM encrypted file sealed with a per-device key
//! (`~/.verve/device.key`). This mirrors the approach used by commercial SSH
//! clients (e.g. Termius): **no master password to remember and no OS
//! keychain authorization prompt**, while still keeping secrets encrypted at
//! rest and isolated per OS user via filesystem permissions.
//!
//! The vault lookup key for a secret is `"{host_id}:{kind}"`
//! (e.g. `"abc123:password"`, `"abc123:key_passphrase"`). Host configuration
//! files (`ssh_hosts.json`) store only non-sensitive data.
//!
//! # Caching
//!
//! The decrypted [`SecretMap`] is cached in process memory after the first
//! access (see [`VAULT_CACHE`]) so the vault file is decrypted at most once
//! per app run.

use std::sync::{Arc, Mutex};

use super::vault::{self, KEY_LEN, SecretMap};

/// In-memory cache of the decrypted vault (populated on first access).
/// Holding the decrypted secrets in memory avoids re-reading/decrypting the
/// vault on every SSH connection.
static VAULT_CACHE: Mutex<Option<Arc<SecretMap>>> = Mutex::new(None);

/// The vault lookup key for a secret: `"{host_id}:{kind}"`.
pub fn keyring_key(host_id: &str, kind: &str) -> String {
    format!("{host_id}:{kind}")
}

/// Return the cached decrypted vault, loading + decrypting it on first access.
///
/// This is the single entry point for reading the vault; callers should never
/// touch `VAULT_CACHE` directly.
fn vault_map() -> Result<Arc<SecretMap>, String> {
    let mut guard = VAULT_CACHE
        .lock()
        .map_err(|e| format!("vault cache lock: {e}"))?;
    if let Some(map) = guard.as_ref() {
        return Ok(Arc::clone(map));
    }
    let key = vault::load_or_create_device_key()?;
    let map = Arc::new(vault::load_vault(&key)?);
    zeroize_key(key);
    *guard = Some(Arc::clone(&map));
    Ok(map)
}

/// Replace the on-disk vault and refresh the in-memory cache atomically with
/// respect to this process.
fn save_and_cache(map: SecretMap) -> Result<(), String> {
    let key = vault::load_or_create_device_key()?;
    let result = vault::save_vault(&key, &map);
    zeroize_key(key);
    result?;
    let mut guard = VAULT_CACHE
        .lock()
        .map_err(|e| format!("vault cache lock: {e}"))?;
    *guard = Some(Arc::new(map));
    Ok(())
}

/// Zeroize a device key in place.
fn zeroize_key(mut key: [u8; KEY_LEN]) {
    use zeroize::Zeroize as _;
    key.zeroize();
}

/// Read-modify-write source for mutations: always decrypt from DISK, never
/// the process cache. Concurrent app instances (e.g. an old build and a new
/// one) share `~/.verve/ssh_vault.bin`; writing back a stale cached copy of
/// the whole vault would wipe keys stored by the other process in between.
fn vault_map_for_write() -> SecretMap {
    let key = match vault::load_or_create_device_key() {
        Ok(k) => k,
        Err(e) => {
            log::error!("vault device key: {e}");
            return SecretMap::new();
        }
    };
    let map = vault::load_vault(&key).unwrap_or_else(|e| {
        log::error!("vault read for write: {e}");
        SecretMap::new()
    });
    zeroize_key(key);
    map
}

/// Store a secret for `host_id` / `kind`. Overwrites any existing value.
pub fn store_secret(host_id: &str, kind: &str, secret: &str) -> Result<(), String> {
    let mut map = vault_map_for_write();
    map.insert(keyring_key(host_id, kind), secret.to_string());
    save_and_cache(map)
}

/// Load a secret for `host_id` / `kind`. Returns `None` if not stored.
pub fn load_secret(host_id: &str, kind: &str) -> Option<String> {
    let map = vault_map().ok()?;
    map.get(&keyring_key(host_id, kind)).cloned()
}

/// Delete a secret for `host_id` / `kind`. Silently succeeds if not found.
pub fn delete_secret(host_id: &str, kind: &str) -> Result<(), String> {
    let mut map = vault_map_for_write();
    map.remove(&keyring_key(host_id, kind));
    save_and_cache(map)
}

/// Drop the in-memory vault cache. Intended for tests; production callers do
/// not normally need this.
#[cfg(test)]
pub fn clear_cache() {
    if let Ok(mut guard) = VAULT_CACHE.lock() {
        *guard = None;
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Isolate data_dir() at a unique per-thread temp dir (no HOME mutation,
    /// no mutex) so this test can't race with sibling data-dir tests.
    fn with_temp_data_dir<F: FnOnce()>(f: F) {
        let tmp = std::env::temp_dir().join(format!(
            "verve-cred-test-{}",
            crate::state::models::new_id()
        ));
        std::fs::create_dir_all(&tmp).unwrap();
        let _guard = crate::state::persistence::set_thread_data_dir(tmp.clone());
        let result = std::panic::catch_unwind(std::panic::AssertUnwindSafe(f));
        let _ = std::fs::remove_dir_all(&tmp);
        if let Err(e) = result {
            std::panic::resume_unwind(e);
        }
    }

    #[test]
    fn store_load_delete_round_trip() {
        with_temp_data_dir(|| {
            store_secret("host1", "password", "s3cret").unwrap();
            assert_eq!(load_secret("host1", "password").as_deref(), Some("s3cret"));

            // Overwrite.
            store_secret("host1", "password", "new_pw").unwrap();
            assert_eq!(load_secret("host1", "password").as_deref(), Some("new_pw"));

            // Delete.
            delete_secret("host1", "password").unwrap();
            assert!(load_secret("host1", "password").is_none());
        });
    }

    #[test]
    fn missing_secret_returns_none() {
        with_temp_data_dir(|| {
            assert!(load_secret("nope", "password").is_none());
        });
    }
}
