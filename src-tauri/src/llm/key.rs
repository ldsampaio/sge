//! LLM API-key custody (Phase 39, Plan 39-02 — LLM-02).
//!
//! The provider API key lives ONLY in the OS keyring (Secret Service on
//! Linux) under a dedicated entry, separate from the mail credentials and
//! the server-config entries in [`crate::creds`]. The key is stored as a
//! plain string via `set_password` (no JSON blob), and is exposed to the
//! rest of the app as presence-only: [`has_llm_api_key`] returns `bool`.
//! There is deliberately no getter returning the secret for convenience,
//! no `Debug` impl on anything holding key material, and no key field on
//! [`crate::llm::LlmSettings`] (T-39-01).
//!
//! Threading: every keyring call blocks the calling thread, so the Tauri
//! command layer MUST run these helpers inside `spawn_blocking`, never on
//! a runtime thread. The helpers themselves perform no locking and no
//! thread-hopping — they are thin wrappers over the entry.
//!
//! This lane performs zero network I/O: no `reqwest::Client`
//! construction, no `.send()`.

use std::sync::Mutex;
use thiserror::Error;

/// Keyring account slot for the LLM provider API key. NEVER CHANGES:
/// saved entries must survive upgrades, so this string is stable forever.
pub const KEYRING_LLM_API_KEY: &str = "sge-llm-api-key";

/// Keyring service id, shared with the mail-credential entries.
const KEYRING_SERVICE: &str = "sge";

#[derive(Debug, Error)]
pub enum LlmKeyError {
    #[error("could not {op} the LLM API key in the OS keyring ({detail})")]
    StoreUnavailable { op: &'static str, detail: String },
}

/// Secret-slot backing store. The app uses [`KeyringLlmKeyStore`]; unit
/// tests use [`MemoryLlmKeyStore`] (one instance per secret slot) where
/// no Secret Service exists.
pub trait LlmKeyStore: Send {
    fn save_secret(&self, secret: &str) -> Result<(), LlmKeyError>;
    fn load_secret(&self) -> Result<Option<String>, LlmKeyError>;
    fn clear_secret(&self) -> Result<(), LlmKeyError>;
}

fn unavailable(op: &'static str, e: keyring::Error) -> LlmKeyError {
    LlmKeyError::StoreUnavailable {
        op,
        detail: e.to_string(),
    }
}

/// OS-keyring store for the LLM API key (Secret Service via
/// `keyring sync-secret-service`).
pub struct KeyringLlmKeyStore {
    service: String,
    account: String,
}

impl KeyringLlmKeyStore {
    pub fn new() -> Self {
        KeyringLlmKeyStore {
            service: KEYRING_SERVICE.to_string(),
            account: KEYRING_LLM_API_KEY.to_string(),
        }
    }

    #[cfg(test)]
    fn with_ids(service: &str, account: &str) -> Self {
        KeyringLlmKeyStore {
            service: service.to_string(),
            account: account.to_string(),
        }
    }

    fn entry(&self) -> Result<keyring::Entry, LlmKeyError> {
        keyring::Entry::new(&self.service, &self.account).map_err(|e| unavailable("open", e))
    }
}

impl Default for KeyringLlmKeyStore {
    fn default() -> Self {
        Self::new()
    }
}

impl LlmKeyStore for KeyringLlmKeyStore {
    fn save_secret(&self, secret: &str) -> Result<(), LlmKeyError> {
        self.entry()?
            .set_password(secret)
            .map_err(|e| unavailable("save", e))
    }

    fn load_secret(&self) -> Result<Option<String>, LlmKeyError> {
        match self.entry()?.get_password() {
            Ok(secret) => Ok(Some(secret)),
            Err(keyring::Error::NoEntry) => Ok(None),
            Err(e) => Err(unavailable("load", e)),
        }
    }

    fn clear_secret(&self) -> Result<(), LlmKeyError> {
        match self.entry()?.delete_credential() {
            Ok(()) | Err(keyring::Error::NoEntry) => Ok(()),
            Err(e) => Err(unavailable("clear", e)),
        }
    }
}

/// In-memory secret slot: the keyring-mocked equivalent for unit tests.
/// One instance holds exactly one secret slot; use a separate instance
/// per slot under test. Holds no `Debug` impl by design so key material
/// can never leak through formatting.
pub struct MemoryLlmKeyStore {
    inner: Mutex<Option<String>>,
}

impl MemoryLlmKeyStore {
    pub fn new() -> Self {
        MemoryLlmKeyStore {
            inner: Mutex::new(None),
        }
    }
}

impl Default for MemoryLlmKeyStore {
    fn default() -> Self {
        Self::new()
    }
}

impl LlmKeyStore for MemoryLlmKeyStore {
    fn save_secret(&self, secret: &str) -> Result<(), LlmKeyError> {
        *self.inner.lock().expect("memory key store lock") = Some(secret.to_string());
        Ok(())
    }

    fn load_secret(&self) -> Result<Option<String>, LlmKeyError> {
        Ok(self.inner.lock().expect("memory key store lock").clone())
    }

    fn clear_secret(&self) -> Result<(), LlmKeyError> {
        *self.inner.lock().expect("memory key store lock") = None;
        Ok(())
    }
}

/// Persist the LLM provider API key as a plain string in the slot.
pub fn save_llm_api_key(store: &impl LlmKeyStore, key: &str) -> Result<(), LlmKeyError> {
    store.save_secret(key)
}

/// Load the LLM provider API key. Returns `Ok(None)` when no entry
/// exists yet — absence is a normal state, not an error.
pub fn load_llm_api_key(store: &impl LlmKeyStore) -> Result<Option<String>, LlmKeyError> {
    store.load_secret()
}

/// Presence-only probe: `true` when a key entry exists. Keyring errors
/// surface as `false` (fail-closed: the settings UI prompts for a key
/// rather than crashing on a locked keyring).
pub fn has_llm_api_key(store: &impl LlmKeyStore) -> bool {
    store.load_secret().map(|v| v.is_some()).unwrap_or(false)
}

/// Remove the LLM API-key entry. Succeeds when no entry exists
/// (`NoEntry`-tolerant, matching the [`crate::creds`] clear contract).
pub fn clear_llm_api_key(store: &impl LlmKeyStore) -> Result<(), LlmKeyError> {
    store.clear_secret()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn save_load_roundtrip_reports_presence() {
        let store = MemoryLlmKeyStore::new();
        assert!(!has_llm_api_key(&store), "absent entry must report false");
        assert_eq!(load_llm_api_key(&store).unwrap(), None);
        let saved = "sk-test-llm-custody-probe-key";
        save_llm_api_key(&store, saved).unwrap();
        assert!(has_llm_api_key(&store), "saved entry must report true");
        assert_eq!(load_llm_api_key(&store).unwrap().as_deref(), Some(saved));
        clear_llm_api_key(&store).unwrap();
        assert!(!has_llm_api_key(&store), "cleared entry must report false");
        assert_eq!(load_llm_api_key(&store).unwrap(), None);
    }

    #[test]
    fn clear_when_absent_succeeds() {
        let store = MemoryLlmKeyStore::new();
        clear_llm_api_key(&store).expect("clear with no entry must succeed");
        assert!(!has_llm_api_key(&store));
    }

    #[test]
    fn double_save_leaves_one_working_entry() {
        // LLM-02 idempotency probe: saving the same key twice leaves one
        // working entry holding the same value.
        let store = MemoryLlmKeyStore::new();
        let saved = "sk-test-llm-idempotent-key";
        save_llm_api_key(&store, saved).unwrap();
        save_llm_api_key(&store, saved).unwrap();
        assert_eq!(
            load_llm_api_key(&store).unwrap().as_deref(),
            Some(saved),
            "double-save must keep the identical value"
        );
        assert!(has_llm_api_key(&store));
    }

    #[test]
    fn error_strings_name_operation_not_secret() {
        // T-39-01: error text names the keyring operation and the backend
        // reason only — never key material.
        let err = unavailable("save", keyring::Error::NoEntry);
        let msg = err.to_string();
        assert!(msg.contains("save"), "error must name the operation: {msg}");
        assert!(
            msg.contains("LLM API key"),
            "error must name the slot in plain language: {msg}"
        );
        assert!(!msg.contains("sk-test"), "error must not carry secrets");
    }

    /// Live Secret Service roundtrip. Ignored by default (needs an unlocked
    /// login keyring); run explicitly to prove the real path:
    /// `cargo test -p sge llm::key -- --ignored`.
    /// Uses a distinctive test-only account and cleans up after itself.
    #[test]
    #[ignore]
    fn live_llm_keyring_roundtrip() {
        let store = KeyringLlmKeyStore::with_ids(KEYRING_SERVICE, "sge-test-llm-key");
        clear_llm_api_key(&store).unwrap();
        assert!(!has_llm_api_key(&store));
        let saved = "sk-test-live-llm-key";
        save_llm_api_key(&store, saved).unwrap();
        assert!(has_llm_api_key(&store));
        assert_eq!(load_llm_api_key(&store).unwrap().as_deref(), Some(saved));
        clear_llm_api_key(&store).unwrap();
        assert!(!has_llm_api_key(&store));
    }
}
