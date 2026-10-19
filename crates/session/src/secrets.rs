//! Stored passwords and key passphrases: the desktop Secret Service through `oo7`'s D-Bus client,
//! never a file. [`SecretStore`] is the closed set of backends; [`Secrets`] is the one handle the
//! session runtime owns: it connects on first use (so a profile that never stores anything never
//! talks to the keyring) and runs every call on the session runtime, whichever executor awaits it.
//!
//! Failure is visible and never silent: a missing, locked or refusing Secret Service is an
//! `Err(SecretError)` whose text the caller shows, and the caller asks the user instead. There
//! is no fallback storage. Secrets exist only as [`SecretString`]; the one place the text is
//! exposed is the `oo7` call that stores it.

use std::sync::Arc;
use std::time::Duration;

use fogu_core::SecretKey;
use oo7::dbus::{Collection, Service};
use secrecy::{ExposeSecret as _, SecretString};
use tokio::runtime::Handle;
use tokio::sync::OnceCell;

use crate::prompt::{PROMPT_TEXT_MAX_CHARS, sanitize_remote};

/// How long the connection to the Secret Service may take.
const DETECT_TIMEOUT: Duration = Duration::from_secs(2);
/// Shown when the Secret Service can't be reached (the cause goes to the log).
const NOT_RUNNING: &str = "no Secret Service answers (is KWallet or GNOME Keyring running?)";
/// How long one keyring call may take, including the user answering an unlock dialog.
const CALL_TIMEOUT: Duration = Duration::from_secs(60);

/// Why a stored secret could not be read, written or removed (sanitized, never a secret).
#[derive(Clone, Debug, PartialEq, Eq, thiserror::Error)]
#[error("{0}")]
pub struct SecretError(String);

impl SecretError {
    fn new(text: impl std::fmt::Display) -> Self {
        Self(sanitize_remote(
            &text.to_string(),
            PROMPT_TEXT_MAX_CHARS,
            false,
        ))
    }
}

/// Where secrets are kept.
#[derive(Debug)]
pub enum SecretStore {
    /// The user's default Secret Service collection (`gnome-keyring`, `KWallet`, `KeePassXC`…).
    SecretService(Collection),
    /// No store; the text says why (shown to the user).
    Disabled(String),
    /// Tests only.
    #[cfg(any(test, feature = "test-store"))]
    InMemory(std::sync::Mutex<std::collections::BTreeMap<String, SecretString>>),
}

impl SecretStore {
    /// Connects to the Secret Service (≤ 2 s), else [`Self::Disabled`] with the reason.
    pub async fn detect() -> Self {
        if cfg!(target_os = "macos") {
            return Self::Disabled("stored passwords are not supported on macOS yet".to_owned());
        }
        let connect = async {
            let service = Service::new().await?;
            service.default_collection().await
        };
        match tokio::time::timeout(DETECT_TIMEOUT, connect).await {
            Ok(Ok(collection)) => Self::SecretService(collection),
            Ok(Err(err)) => {
                tracing::warn!(error = %err, "Secret Service unavailable");
                Self::Disabled(NOT_RUNNING.to_owned())
            }
            Err(_elapsed) => {
                tracing::warn!("Secret Service did not answer");
                Self::Disabled(NOT_RUNNING.to_owned())
            }
        }
    }

    /// The test store.
    #[cfg(any(test, feature = "test-store"))]
    #[must_use]
    pub fn in_memory() -> Self {
        Self::InMemory(std::sync::Mutex::default())
    }

    /// The stored secret, if any.
    pub async fn get(&self, key: &SecretKey) -> Result<Option<SecretString>, SecretError> {
        match self {
            Self::SecretService(collection) => {
                let Some(item) = find(collection, key).await?.into_iter().next() else {
                    return Ok(None);
                };
                let secret = bounded(item.secret()).await?;
                let text = std::str::from_utf8(secret.as_bytes())
                    .map_err(|_| SecretError::new("the stored secret is not text"))?;
                Ok(Some(SecretString::from(text)))
            }
            Self::Disabled(reason) => Err(SecretError::new(reason)),
            #[cfg(any(test, feature = "test-store"))]
            Self::InMemory(map) => Ok(map.lock().ok().and_then(|m| m.get(&id(key)).cloned())),
        }
    }

    /// Whether a secret is stored (nothing is read out of the keyring).
    pub async fn exists(&self, key: &SecretKey) -> Result<bool, SecretError> {
        match self {
            Self::SecretService(collection) => Ok(!find(collection, key).await?.is_empty()),
            Self::Disabled(reason) => Err(SecretError::new(reason)),
            #[cfg(any(test, feature = "test-store"))]
            Self::InMemory(map) => Ok(map.lock().is_ok_and(|m| m.contains_key(&id(key)))),
        }
    }

    /// Stores `value`, replacing what `key` held.
    pub async fn set(&self, key: &SecretKey, value: &SecretString) -> Result<(), SecretError> {
        match self {
            Self::SecretService(collection) => {
                unlock(collection).await?;
                let secret = oo7::Secret::text(value.expose_secret());
                bounded(collection.create_item(
                    &key.label(),
                    &key.attributes(),
                    secret,
                    true,
                    None,
                ))
                .await?;
                Ok(())
            }
            Self::Disabled(reason) => Err(SecretError::new(reason)),
            #[cfg(any(test, feature = "test-store"))]
            Self::InMemory(map) => {
                if let Ok(mut map) = map.lock() {
                    map.insert(id(key), value.clone());
                }
                Ok(())
            }
        }
    }

    /// Removes the secret (nothing stored is not an error).
    pub async fn delete(&self, key: &SecretKey) -> Result<(), SecretError> {
        match self {
            Self::SecretService(collection) => {
                for item in find(collection, key).await? {
                    bounded(item.delete(None)).await?;
                }
                Ok(())
            }
            Self::Disabled(reason) => Err(SecretError::new(reason)),
            #[cfg(any(test, feature = "test-store"))]
            Self::InMemory(map) => {
                if let Ok(mut map) = map.lock() {
                    map.remove(&id(key));
                }
                Ok(())
            }
        }
    }
}

#[cfg(any(test, feature = "test-store"))]
fn id(key: &SecretKey) -> String {
    format!("{:?}", key.attributes())
}

/// Awaits a keyring call under [`CALL_TIMEOUT`].
async fn bounded<T>(
    call: impl Future<Output = Result<T, oo7::dbus::Error>>,
) -> Result<T, SecretError> {
    match tokio::time::timeout(CALL_TIMEOUT, call).await {
        Ok(result) => result.map_err(SecretError::new),
        Err(_elapsed) => Err(SecretError::new(
            "the Secret Service did not answer in time",
        )),
    }
}

/// Unlocks the collection (the desktop shows its unlock dialog) or says it stays locked.
async fn unlock(collection: &Collection) -> Result<(), SecretError> {
    const LOCKED: &str = "the keyring is locked and was not unlocked";
    if bounded(collection.is_locked()).await? {
        let unlocked = bounded(collection.unlock(None)).await;
        if unlocked.is_err() || bounded(collection.is_locked()).await? {
            return Err(SecretError::new(LOCKED));
        }
    }
    Ok(())
}

/// The items stored under `key`.
async fn find(
    collection: &Collection,
    key: &SecretKey,
) -> Result<Vec<oo7::dbus::Item>, SecretError> {
    unlock(collection).await?;
    bounded(collection.search_items(&key.attributes())).await
}

/// The session runtime's secret store: connected on first use, called on the session runtime.
#[derive(Clone, Debug)]
pub struct Secrets {
    rt: Handle,
    store: Arc<OnceCell<SecretStore>>,
}

impl Secrets {
    /// The desktop Secret Service, connected when first needed.
    #[must_use]
    pub fn new(rt: Handle) -> Self {
        Self {
            rt,
            store: Arc::new(OnceCell::new()),
        }
    }

    /// A fixed `store` (tests, `Disabled`).
    #[must_use]
    pub fn with_store(rt: Handle, store: SecretStore) -> Self {
        Self {
            rt,
            store: Arc::new(OnceCell::new_with(Some(store))),
        }
    }

    /// Runs `fut` on the session runtime (zbus needs it), whichever executor awaits the result.
    async fn spawn<T: Send + 'static>(
        &self,
        fut: impl Future<Output = Result<T, SecretError>> + Send + 'static,
    ) -> Result<T, SecretError> {
        self.rt
            .spawn(fut)
            .await
            .unwrap_or_else(|_| Err(SecretError::new("the secret store task failed")))
    }

    /// See [`SecretStore::get`].
    pub async fn get(&self, key: &SecretKey) -> Result<Option<SecretString>, SecretError> {
        let (store, key) = (Arc::clone(&self.store), key.clone());
        let fut = async move { store.get_or_init(SecretStore::detect).await.get(&key).await };
        self.spawn(fut).await
    }

    /// See [`SecretStore::exists`].
    pub async fn exists(&self, key: &SecretKey) -> Result<bool, SecretError> {
        let (store, key) = (Arc::clone(&self.store), key.clone());
        let fut = async move {
            store
                .get_or_init(SecretStore::detect)
                .await
                .exists(&key)
                .await
        };
        self.spawn(fut).await
    }

    /// See [`SecretStore::set`].
    pub async fn set(&self, key: &SecretKey, value: SecretString) -> Result<(), SecretError> {
        let (store, key) = (Arc::clone(&self.store), key.clone());
        let fut = async move {
            let store = store.get_or_init(SecretStore::detect).await;
            store.set(&key, &value).await
        };
        self.spawn(fut).await
    }

    /// See [`SecretStore::delete`].
    pub async fn delete(&self, key: &SecretKey) -> Result<(), SecretError> {
        let (store, key) = (Arc::clone(&self.store), key.clone());
        let fut = async move {
            store
                .get_or_init(SecretStore::detect)
                .await
                .delete(&key)
                .await
        };
        self.spawn(fut).await
    }
}

#[cfg(test)]
mod tests {
    #![expect(clippy::unwrap_used, reason = "tests")]

    use super::*;

    fn key(user: &str) -> SecretKey {
        SecretKey::SshPassword {
            user: user.into(),
            host: "h".into(),
            port: 22,
        }
    }

    #[tokio::test]
    async fn in_memory_round_trip_and_disabled_errors() {
        let store = SecretStore::in_memory();
        assert_eq!(
            store
                .get(&key("a"))
                .await
                .unwrap()
                .map(|s| s.expose_secret().to_owned()),
            None
        );
        store
            .set(&key("a"), &SecretString::from("one"))
            .await
            .unwrap();
        store
            .set(&key("a"), &SecretString::from("two"))
            .await
            .unwrap();
        assert!(store.exists(&key("a")).await.unwrap());
        assert!(!store.exists(&key("b")).await.unwrap());
        let got = store.get(&key("a")).await.unwrap().unwrap();
        assert_eq!(got.expose_secret(), "two");
        store.delete(&key("a")).await.unwrap();
        assert!(!store.exists(&key("a")).await.unwrap());

        let off = SecretStore::Disabled("no bus".into());
        let err = off.get(&key("a")).await.unwrap_err();
        assert_eq!(err.to_string(), "no bus");
        assert!(off.set(&key("a"), &SecretString::from("x")).await.is_err());
    }

    #[tokio::test]
    async fn debug_of_the_handle_and_errors_never_shows_secrets() {
        let store = SecretStore::in_memory();
        store
            .set(&key("a"), &SecretString::from("hunter2"))
            .await
            .unwrap();
        let secrets = Secrets::with_store(Handle::current(), store);
        assert!(!format!("{secrets:?}").contains("hunter2"));
        let got = secrets.get(&key("a")).await.unwrap();
        assert!(!format!("{got:?}").contains("hunter2"));
    }

    /// Against the real Secret Service of the session bus; needs `FOGU_TEST_SECRET_SERVICE=1`
    /// and an unlocked keyring.
    #[tokio::test]
    async fn live_secret_service_round_trip() {
        if std::env::var_os("FOGU_TEST_SECRET_SERVICE").is_none() {
            return;
        }
        let store = SecretStore::detect().await;
        assert!(matches!(store, SecretStore::SecretService(_)), "{store:?}");
        let key = SecretKey::KeyPassphrase {
            key_fingerprint: "fogu-test-live-item".into(),
        };
        store.set(&key, &SecretString::from("first")).await.unwrap();
        store
            .set(&key, &SecretString::from("päss wörd"))
            .await
            .unwrap();
        assert!(store.exists(&key).await.unwrap());
        let got = store.get(&key).await.unwrap().unwrap();
        assert_eq!(got.expose_secret(), "päss wörd");
        store.delete(&key).await.unwrap();
        assert!(!store.exists(&key).await.unwrap());
        assert!(store.get(&key).await.unwrap().is_none());
    }
}
