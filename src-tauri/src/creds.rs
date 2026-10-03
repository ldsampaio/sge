//! OS-keyring credential save slice (Phase 1, Plan 01-03 — CONN-03 save only).
//!
//! Remember-me persists username + password in the OS keyring (Secret Service
//! on Linux: GNOME Keyring / KWallet) under service `sge`. Nothing secret
//! ever touches SQLite, files, or logs. Auto-connect stays in Phase 5: this
//! module only saves/loads/clears on explicit user action.
//!
//! Every keyring call blocks the calling thread: all commands run them inside
//! `spawn_blocking` (STACK hard rule), never on a runtime thread.

use std::sync::Mutex;
use thiserror::Error;

/// Keyring service id. Never changes: saved entries must survive upgrades.
pub const KEYRING_SERVICE: &str = "sge";
/// Single-account slot for M1 (multi-account arrives with the accounts
/// table in Phase 2).
const KEYRING_ACCOUNT: &str = "sge-default-account";

#[derive(Debug, Error)]
pub enum CredsError {
    #[error(
        "could not access the OS keyring ({detail}) — continuing with a memory-only session is safe"
    )]
    StoreUnavailable { detail: String },
    #[error("saved credentials are unreadable ({detail})")]
    Corrupt { detail: String },
}

/// Username + password pair crossing the keyring boundary. Serialized as one
/// JSON blob in a single entry so save/load/clear stay atomic.
#[derive(Debug, Clone, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
pub struct SavedCredentials {
    pub username: String,
    pub password: String,
}

/// Credential backing store. The app uses [`KeyringStore`]; tests use
/// [`MemoryStore`] where no Secret Service exists.
pub trait CredentialStore: Send {
    fn save(&self, creds: &SavedCredentials) -> Result<(), CredsError>;
    fn load(&self) -> Result<Option<SavedCredentials>, CredsError>;
    fn clear(&self) -> Result<(), CredsError>;
}

fn encode(creds: &SavedCredentials) -> String {
    serde_json::to_string(creds).expect("SavedCredentials always serializes")
}

fn encode_server_config(cfg: &ServerConfig) -> String {
    serde_json::to_string(cfg).expect("ServerConfig always serializes")
}

fn decode(blob: &str) -> Result<SavedCredentials, CredsError> {
    serde_json::from_str(blob).map_err(|e| CredsError::Corrupt {
        detail: e.to_string(),
    })
}

fn decode_server_config(blob: &str) -> Result<ServerConfig, CredsError> {
    serde_json::from_str(blob).map_err(|e| CredsError::Corrupt {
        detail: e.to_string(),
    })
}

/// Server config persisted alongside credentials (written by
/// `connect_account` on first successful login, read by `start_sync`).
#[derive(Debug, Clone, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
pub struct ServerConfig {
    pub host: String,
    pub port: u16,
    pub security: String,
}

/// OS-keyring store (Secret Service via `keyring sync-secret-service`).
pub struct KeyringStore {
    service: String,
    account: String,
}

/// Separate keyring entry for server config (host/port/security) so
/// `start_sync` can reconnect without re-prompting the user.
const KEYRING_SERVER_CFG: &str = "sge-server-cfg";

impl KeyringStore {
    pub fn new() -> Self {
        KeyringStore {
            service: KEYRING_SERVICE.to_string(),
            account: KEYRING_ACCOUNT.to_string(),
        }
    }

    #[cfg(test)]
    fn with_ids(service: &str, account: &str) -> Self {
        KeyringStore {
            service: service.to_string(),
            account: account.to_string(),
        }
    }

    /// Persist server configuration (host, port, security mode).
    pub fn save_server_config(&self, cfg: &ServerConfig) -> Result<(), CredsError> {
        self.entry_for(KEYRING_SERVER_CFG)?
            .set_password(&encode_server_config(cfg))
            .map_err(unavailable)
    }

    /// Load server configuration. Returns `None` if no entry exists.
    pub fn load_server_config(&self) -> Result<Option<ServerConfig>, CredsError> {
        match self.entry_for(KEYRING_SERVER_CFG)?.get_password() {
            Ok(blob) => decode_server_config(&blob).map(Some),
            Err(keyring::Error::NoEntry) => Ok(None),
            Err(e) => Err(unavailable(e)),
        }
    }

    fn entry(&self) -> Result<keyring::Entry, CredsError> {
        keyring::Entry::new(&self.service, &self.account).map_err(|e| {
            CredsError::StoreUnavailable {
                detail: e.to_string(),
            }
        })
    }

    fn entry_for(&self, account: &str) -> Result<keyring::Entry, CredsError> {
        keyring::Entry::new(&self.service, account).map_err(|e| {
            CredsError::StoreUnavailable {
                detail: e.to_string(),
            }
        })
    }
}

impl Default for KeyringStore {
    fn default() -> Self {
        Self::new()
    }
}

fn unavailable(e: keyring::Error) -> CredsError {
    CredsError::StoreUnavailable {
        detail: e.to_string(),
    }
}

impl CredentialStore for KeyringStore {
    fn save(&self, creds: &SavedCredentials) -> Result<(), CredsError> {
        self.entry()?
            .set_password(&encode(creds))
            .map_err(unavailable)
    }

    fn load(&self) -> Result<Option<SavedCredentials>, CredsError> {
        match self.entry()?.get_password() {
            Ok(blob) => decode(&blob).map(Some),
            Err(keyring::Error::NoEntry) => Ok(None),
            Err(e) => Err(unavailable(e)),
        }
    }

    fn clear(&self) -> Result<(), CredsError> {
        match self.entry()?.delete_credential() {
            Ok(()) | Err(keyring::Error::NoEntry) => Ok(()),
            Err(e) => Err(unavailable(e)),
        }
    }
}

/// In-memory store: the keyring-mocked equivalent for unit tests and for
/// documenting the remember-me contract without a Secret Service.
pub struct MemoryStore {
    inner: Mutex<Option<String>>,
}

impl MemoryStore {
    pub fn new() -> Self {
        MemoryStore {
            inner: Mutex::new(None),
        }
    }
}

impl Default for MemoryStore {
    fn default() -> Self {
        Self::new()
    }
}

impl CredentialStore for MemoryStore {
    fn save(&self, creds: &SavedCredentials) -> Result<(), CredsError> {
        *self.inner.lock().expect("memory store lock") = Some(encode(creds));
        Ok(())
    }

    fn load(&self) -> Result<Option<SavedCredentials>, CredsError> {
        match self.inner.lock().expect("memory store lock").clone() {
            Some(blob) => decode(&blob).map(Some),
            None => Ok(None),
        }
    }

    fn clear(&self) -> Result<(), CredsError> {
        *self.inner.lock().expect("memory store lock") = None;
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn sample() -> SavedCredentials {
        SavedCredentials {
            username: "alice".to_string(),
            password: "s3cret-pw".to_string(),
        }
    }

    #[test]
    fn memory_save_load_roundtrip() {
        let store = MemoryStore::new();
        assert_eq!(store.load().unwrap(), None);
        store.save(&sample()).unwrap();
        assert_eq!(store.load().unwrap(), Some(sample()));
        store.clear().unwrap();
        assert_eq!(store.load().unwrap(), None);
    }

    #[test]
    fn corrupt_blob_is_corrupt_not_unavailable() {
        assert!(decode("not-json{{").is_err());
        let err = decode("not-json{{").unwrap_err();
        assert!(
            matches!(err, CredsError::Corrupt { .. }),
            "unexpected: {err}"
        );
    }

    #[test]
    fn keyring_error_maps_to_friendly_store_unavailable() {
        let err = unavailable(keyring::Error::NoEntry);
        let msg = err.to_string();
        assert!(matches!(err, CredsError::StoreUnavailable { .. }), "{msg}");
        assert!(msg.contains("memory-only"), "{msg}");
    }

    /// Live Secret Service roundtrip. Ignored by default (needs an unlocked
    /// login keyring); run explicitly to prove the real path:
    /// `cargo test -p sge creds:: -- --ignored`.
    /// Uses a distinctive test-only account and cleans up after itself.
    #[test]
    #[ignore]
    fn live_keyring_save_load_roundtrip() {
        let store = KeyringStore::with_ids(KEYRING_SERVICE, "sge-test-roundtrip");
        store.clear().unwrap();
        assert_eq!(store.load().unwrap(), None);
        store.save(&sample()).unwrap();
        assert_eq!(store.load().unwrap(), Some(sample()));
        store.clear().unwrap();
        assert_eq!(store.load().unwrap(), None);
    }
}
