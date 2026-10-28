//! Where the Gemini API key lives: the operating system's credential store and nowhere else.
//!
//! The key is never written to SQLite, a file, a log or the repository, and it is never sent to
//! the interface (which only learns whether one is configured). [`ApiKey`] has no `Debug`, `Display`
//! or `Serialize` that could print it by accident.

use std::sync::Mutex;

use crate::error::{Error, Result};

/// Longest key accepted (real keys are far shorter; this only stops absurd pastes).
const MAX_KEY_LEN: usize = 256;

/// A validated API key. Printing it, even with `{:?}`, shows nothing of its value.
pub struct ApiKey(String);

impl ApiKey {
    /// Trims the paste (a trailing newline is the usual accident) and refuses anything that is
    /// not printable ASCII without spaces, since the key travels in an HTTP header.
    pub fn parse(raw: &str) -> Result<Self> {
        let key = raw.trim();
        let problem = if key.is_empty() {
            Some("the key is empty")
        } else if key.len() > MAX_KEY_LEN {
            Some("the key is too long")
        } else if !key.bytes().all(|b| b.is_ascii_graphic()) {
            Some("the key must be printable characters without spaces")
        } else {
            None
        };
        problem.map_or_else(
            || Ok(Self(key.to_owned())),
            |detail| {
                Err(Error::InvalidConfig {
                    what: "API key",
                    detail: detail.to_owned(),
                })
            },
        )
    }

    /// The key itself, for the one place that sends it (the request header).
    #[must_use]
    pub fn expose(&self) -> &str {
        &self.0
    }
}

impl std::fmt::Debug for ApiKey {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str("ApiKey(<redacted>)")
    }
}

pub trait SecretStore: Send + Sync {
    /// The stored key, or `None` when none was saved.
    fn get(&self) -> Result<Option<ApiKey>>;

    /// Saves the key, replacing any previous one.
    fn set(&self, key: &ApiKey) -> Result<()>;

    /// Removes the key. Removing a key that is not there is not an error.
    fn clear(&self) -> Result<()>;

    fn is_configured(&self) -> Result<bool> {
        Ok(self.get()?.is_some())
    }
}

/// A key held in memory only: for tests, and as the stand-in on systems without a credential
/// store this build supports.
#[derive(Default)]
pub struct MemorySecretStore(Mutex<Option<String>>);

impl MemorySecretStore {
    fn slot(&self) -> std::sync::MutexGuard<'_, Option<String>> {
        self.0
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
    }
}

impl SecretStore for MemorySecretStore {
    fn get(&self) -> Result<Option<ApiKey>> {
        Ok(self.slot().clone().map(ApiKey))
    }

    fn set(&self, key: &ApiKey) -> Result<()> {
        *self.slot() = Some(key.0.clone());
        Ok(())
    }

    fn clear(&self) -> Result<()> {
        *self.slot() = None;
        Ok(())
    }
}

#[cfg(windows)]
pub use windows::WindowsCredentialStore;

#[cfg(windows)]
mod windows {
    use std::collections::HashMap;

    use keyring_core::{Entry, api::CredentialStoreApi};

    use super::{ApiKey, SecretStore};
    use crate::error::{Error, Result};

    /// The credential name inside the Windows Credential Manager.
    const USER: &str = "gemini-api-key";

    /// The Windows Credential Manager. The entry is stored with `Local` persistence: it stays on
    /// this PC and does not roam with a domain profile (the store's default would).
    pub struct WindowsCredentialStore {
        entry: Entry,
    }

    fn failure(error: &keyring_core::Error) -> Error {
        // The variants that carry the secret's bytes are described without them.
        let detail = match error {
            keyring_core::Error::BadEncoding(_) => "the stored key is not valid text".to_owned(),
            keyring_core::Error::BadDataFormat(..) => "the stored key is malformed".to_owned(),
            other => other.to_string(),
        };
        Error::SecretStore(detail)
    }

    impl WindowsCredentialStore {
        pub fn new(service: &str) -> Result<Self> {
            let store = windows_native_keyring_store::Store::new().map_err(|e| failure(&e))?;
            let modifiers = HashMap::from([("persistence", "Local")]);
            let entry = store
                .build(service, USER, Some(&modifiers))
                .map_err(|e| failure(&e))?;
            Ok(Self { entry })
        }
    }

    #[cfg(test)]
    impl WindowsCredentialStore {
        /// The credential's attributes as the Credential Manager reports them.
        pub fn attributes(&self) -> Result<HashMap<String, String>> {
            self.entry.get_attributes().map_err(|e| failure(&e))
        }
    }

    impl SecretStore for WindowsCredentialStore {
        fn get(&self) -> Result<Option<ApiKey>> {
            match self.entry.get_password() {
                Ok(text) => Ok(Some(ApiKey(text))),
                Err(keyring_core::Error::NoEntry) => Ok(None),
                Err(other) => Err(failure(&other)),
            }
        }

        fn set(&self, key: &ApiKey) -> Result<()> {
            self.entry
                .set_password(key.expose())
                .map_err(|e| failure(&e))
        }

        fn clear(&self) -> Result<()> {
            match self.entry.delete_credential() {
                Ok(()) | Err(keyring_core::Error::NoEntry) => Ok(()),
                Err(other) => Err(failure(&other)),
            }
        }
    }
}

#[cfg(test)]
mod tests;
