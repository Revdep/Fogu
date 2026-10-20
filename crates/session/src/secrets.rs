//! Stored passwords and key passphrases: the desktop Secret Service through `oo7`'s D-Bus client
//! (the login Keychain on macOS), never a file. [`SecretStore`] is the closed set of backends; [`Secrets`] is the one handle the
//! session runtime owns: it connects on first use (so a profile that never stores anything never
//! talks to the keyring) and runs every call on the session runtime, whichever executor awaits it.
//!
//! Failure is visible and never silent: a missing, locked or refusing Secret Service is an
//! `Err(SecretError)` whose text the caller shows, and the caller asks the user instead. There
//! is no fallback storage. Secrets exist only as [`SecretString`]; the one place the text is
//! exposed is the `oo7` call that stores it.

use std::sync::Arc;
#[cfg(not(target_os = "macos"))]
use std::time::Duration;

use fogu_core::SecretKey;
#[cfg(not(target_os = "macos"))]
use oo7::dbus::{Collection, Service};
#[cfg(not(target_os = "macos"))]
use secrecy::ExposeSecret as _;
use secrecy::SecretString;
use tokio::runtime::Handle;
use tokio::sync::OnceCell;

use crate::prompt::{PROMPT_TEXT_MAX_CHARS, sanitize_remote};

/// How long the connection to the Secret Service may take.
#[cfg(not(target_os = "macos"))]
const DETECT_TIMEOUT: Duration = Duration::from_secs(2);
/// Shown when the Secret Service can't be reached (the cause goes to the log).
#[cfg(not(target_os = "macos"))]
const NOT_RUNNING: &str = "no Secret Service answers (is KWallet or GNOME Keyring running?)";
/// How long one keyring call may take, including the user answering an unlock dialog.
#[cfg(not(target_os = "macos"))]
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
    #[cfg(not(target_os = "macos"))]
    SecretService(Collection),
    /// The login Keychain (macOS): generic passwords under service `fogu`.
    #[cfg(target_os = "macos")]
    Keychain,
    /// No store; the text says why (shown to the user).
    Disabled(String),
    /// Tests only.
    #[cfg(any(test, feature = "test-store"))]
    InMemory(std::sync::Mutex<std::collections::BTreeMap<String, SecretString>>),
}

impl SecretStore {
    /// The login Keychain.
    #[cfg(target_os = "macos")]
    #[expect(
        clippy::unused_async,
        clippy::unused_async_trait_impl,
        reason = "same signature as the Secret Service's"
    )]
    pub async fn detect() -> Self {
        Self::Keychain
    }

    /// Connects to the Secret Service (≤ 2 s), else [`Self::Disabled`] with the reason.
    #[cfg(not(target_os = "macos"))]
    pub async fn detect() -> Self {
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
            #[cfg(not(target_os = "macos"))]
            Self::SecretService(collection) => {
                let Some(item) = find(collection, key).await?.into_iter().next() else {
                    return Ok(None);
                };
                let secret = bounded(item.secret()).await?;
                let text = std::str::from_utf8(secret.as_bytes())
                    .map_err(|_| SecretError::new("the stored secret is not text"))?;
                Ok(Some(SecretString::from(text)))
            }
            #[cfg(target_os = "macos")]
            Self::Keychain => keychain::get(key).await,
            Self::Disabled(reason) => Err(SecretError::new(reason)),
            #[cfg(any(test, feature = "test-store"))]
            Self::InMemory(map) => Ok(map.lock().ok().and_then(|m| m.get(&id(key)).cloned())),
        }
    }

    /// Whether a secret is stored (nothing is read out of the keyring).
    pub async fn exists(&self, key: &SecretKey) -> Result<bool, SecretError> {
        match self {
            #[cfg(not(target_os = "macos"))]
            Self::SecretService(collection) => Ok(!find(collection, key).await?.is_empty()),
            #[cfg(target_os = "macos")]
            Self::Keychain => keychain::exists(key).await,
            Self::Disabled(reason) => Err(SecretError::new(reason)),
            #[cfg(any(test, feature = "test-store"))]
            Self::InMemory(map) => Ok(map.lock().is_ok_and(|m| m.contains_key(&id(key)))),
        }
    }

    /// Stores `value`, replacing what `key` held.
    pub async fn set(&self, key: &SecretKey, value: &SecretString) -> Result<(), SecretError> {
        match self {
            #[cfg(not(target_os = "macos"))]
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
            #[cfg(target_os = "macos")]
            Self::Keychain => keychain::set(key, value).await,
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
            #[cfg(not(target_os = "macos"))]
            Self::SecretService(collection) => {
                for item in find(collection, key).await? {
                    bounded(item.delete(None)).await?;
                }
                Ok(())
            }
            #[cfg(target_os = "macos")]
            Self::Keychain => keychain::delete(key).await,
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

/// The login Keychain through Security.framework's generic passwords.
#[cfg(target_os = "macos")]
mod keychain {
    use std::time::Duration;

    use fogu_core::SecretKey;
    use secrecy::{ExposeSecret as _, SecretString};
    use security_framework::item::{ItemClass, ItemSearchOptions, ItemUpdateOptions, update_item};
    use security_framework::passwords::{self, PasswordOptions};

    use super::SecretError;

    const SERVICE: &str = "fogu";
    /// How long one call may take, including the user answering an access dialog.
    const CALL_TIMEOUT: Duration = Duration::from_secs(60);
    /// `errSecItemNotFound`.
    const NOT_FOUND: i32 = -25_300;

    /// The item's account: the key's lookup attributes as JSON, which is deterministic (sorted
    /// map) and injective (escaped), unlike joining `k=v` pairs of unrestricted strings.
    pub(super) fn account(key: &SecretKey) -> String {
        serde_json::to_string(&key.attributes()).unwrap_or_default()
    }

    /// Runs a Security.framework call off the runtime: it blocks, and an access dialog waits
    /// for the user.
    async fn blocking<T: Send + 'static>(
        call: impl FnOnce() -> Result<T, SecretError> + Send + 'static,
    ) -> Result<T, SecretError> {
        within(CALL_TIMEOUT, call).await
    }

    /// [`blocking`] under `limit`.
    pub(super) async fn within<T: Send + 'static>(
        limit: Duration,
        call: impl FnOnce() -> Result<T, SecretError> + Send + 'static,
    ) -> Result<T, SecretError> {
        match tokio::time::timeout(limit, tokio::task::spawn_blocking(call)).await {
            Ok(Ok(result)) => result,
            Ok(Err(_)) => Err(SecretError::new("the Keychain call failed")),
            Err(_elapsed) => Err(SecretError::new("the Keychain did not answer in time")),
        }
    }

    pub(super) async fn get(key: &SecretKey) -> Result<Option<SecretString>, SecretError> {
        let account = account(key);
        blocking(
            move || match passwords::get_generic_password(SERVICE, &account) {
                Ok(bytes) => String::from_utf8(bytes)
                    .map(|text| Some(SecretString::from(text)))
                    .map_err(|_| SecretError::new("the stored secret is not text")),
                Err(err) if err.code() == NOT_FOUND => Ok(None),
                Err(err) => Err(SecretError::new(err)),
            },
        )
        .await
    }

    /// Looks the item up without its data: no access dialog, the secret is never read.
    pub(super) async fn exists(key: &SecretKey) -> Result<bool, SecretError> {
        let account = account(key);
        blocking(move || {
            let found = ItemSearchOptions::new()
                .class(ItemClass::generic_password())
                .service(SERVICE)
                .account(&account)
                .search();
            match found {
                Ok(_) => Ok(true),
                Err(err) if err.code() == NOT_FOUND => Ok(false),
                Err(err) => Err(SecretError::new(err)),
            }
        })
        .await
    }

    pub(super) async fn set(key: &SecretKey, value: &SecretString) -> Result<(), SecretError> {
        let (account, label, value) = (account(key), key.label(), value.clone());
        blocking(move || {
            let store = || {
                let mut options = PasswordOptions::new_generic_password(SERVICE, &account);
                options.set_label(&label);
                passwords::set_generic_password_options(value.expose_secret().as_bytes(), options)
            };
            match store() {
                // The update half matches on the label too: an item under another label (the
                // host's case, a rename in Keychain Access) gets ours first, in place, so a
                // failure never loses the stored secret.
                Err(err) if err.code() == NOT_FOUND => {
                    let mut item = ItemSearchOptions::new();
                    item.class(ItemClass::generic_password())
                        .service(SERVICE)
                        .account(&account);
                    let mut relabel = ItemUpdateOptions::new();
                    relabel.set_label(&label);
                    update_item(&item, &relabel)
                        .and_then(|()| store())
                        .map_err(SecretError::new)
                }
                stored => stored.map_err(SecretError::new),
            }
        })
        .await
    }

    pub(super) async fn delete(key: &SecretKey) -> Result<(), SecretError> {
        let account = account(key);
        blocking(
            move || match passwords::delete_generic_password(SERVICE, &account) {
                Err(err) if err.code() != NOT_FOUND => Err(SecretError::new(err)),
                _ => Ok(()),
            },
        )
        .await
    }
}

#[cfg(any(test, feature = "test-store"))]
fn id(key: &SecretKey) -> String {
    format!("{:?}", key.attributes())
}

/// Awaits a keyring call under [`CALL_TIMEOUT`].
#[cfg(not(target_os = "macos"))]
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
#[cfg(not(target_os = "macos"))]
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
#[cfg(not(target_os = "macos"))]
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

    #[cfg(target_os = "macos")]
    use secrecy::ExposeSecret as _;

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

    #[cfg(target_os = "macos")]
    #[test]
    fn keychain_accounts_are_distinct_for_tricky_names() {
        let script = |profile: &str, name: &str| SecretKey::LoginScript {
            profile_id: fogu_core::ProfileId(profile.into()),
            name: name.into(),
        };
        let a = keychain::account(&script("p;name=x", "y"));
        let b = keychain::account(&script("p", "x;name=y"));
        let c = keychain::account(&script("p\"", "y"));
        assert_ne!(a, b);
        assert_ne!(a, c);
        assert_eq!(a, keychain::account(&script("p;name=x", "y")));
        assert!(a.contains("\"application\":\"fogu\""));
    }

    /// A Keychain call nobody answers (an access dialog left open) gives up instead of hanging.
    #[cfg(target_os = "macos")]
    #[tokio::test]
    async fn keychain_calls_give_up_after_their_timeout() {
        let started = std::time::Instant::now();
        let got = keychain::within(std::time::Duration::from_millis(100), || {
            std::thread::sleep(std::time::Duration::from_secs(2));
            Ok(())
        })
        .await;
        assert!(got.is_err());
        assert!(started.elapsed() < std::time::Duration::from_secs(1));
    }

    /// Against the user's login Keychain; needs `FOGU_TEST_KEYCHAIN=1`.
    #[cfg(target_os = "macos")]
    #[tokio::test]
    async fn live_keychain_round_trip() {
        if std::env::var_os("FOGU_TEST_KEYCHAIN").is_none() {
            return;
        }
        let store = SecretStore::detect().await;
        assert!(matches!(store, SecretStore::Keychain), "{store:?}");
        let key = SecretKey::KeyPassphrase {
            key_fingerprint: "fogu-test-live-item".into(),
        };
        store.delete(&key).await.unwrap();
        assert!(!store.exists(&key).await.unwrap());
        store.set(&key, &SecretString::from("first")).await.unwrap();
        store
            .set(&key, &SecretString::from("päss wörd"))
            .await
            .unwrap();
        let got = store.get(&key).await.unwrap().unwrap();
        assert_eq!(got.expose_secret(), "päss wörd");
        store.delete(&key).await.unwrap();
        assert!(store.get(&key).await.unwrap().is_none());
    }

    /// Replacing an item whose label differs (the host's case, or renamed in Keychain Access)
    /// still stores the new value. Needs `FOGU_TEST_KEYCHAIN=1`.
    #[cfg(target_os = "macos")]
    #[tokio::test]
    async fn live_keychain_set_replaces_an_item_with_another_label() {
        if std::env::var_os("FOGU_TEST_KEYCHAIN").is_none() {
            return;
        }
        let key = |host: &str| SecretKey::SshPassword {
            user: "fogu-test".into(),
            host: host.into(),
            port: 22,
        };
        let (upper, lower) = (key("Label-Test.example"), key("label-test.example"));
        assert_eq!(keychain::account(&upper), keychain::account(&lower));
        assert_ne!(upper.label(), lower.label());
        let store = SecretStore::Keychain;
        store.delete(&upper).await.unwrap();
        store.set(&upper, &SecretString::from("one")).await.unwrap();
        let replaced = store.set(&lower, &SecretString::from("two")).await;
        let got = store.get(&upper).await.unwrap();
        store.delete(&upper).await.unwrap();
        assert_eq!(replaced, Ok(()));
        assert_eq!(
            got.map(|s| s.expose_secret().to_owned()).as_deref(),
            Some("two")
        );
    }

    /// That replacement updates the item in place (same creation date): deleting and adding it
    /// again would lose the old secret if the add then failed. Needs `FOGU_TEST_KEYCHAIN=1`.
    #[cfg(target_os = "macos")]
    #[tokio::test]
    async fn live_keychain_set_keeps_the_item_when_the_label_changes() {
        if std::env::var_os("FOGU_TEST_KEYCHAIN").is_none() {
            return;
        }
        let key = |host: &str| SecretKey::SshPassword {
            user: "fogu-test".into(),
            host: host.into(),
            port: 22,
        };
        let (upper, lower) = (key("Keep-Item.example"), key("keep-item.example"));
        let account = keychain::account(&upper);
        // `security` prints the attributes (never the secret, so no access dialog).
        let created = || {
            let out = std::process::Command::new("/usr/bin/security")
                .args(["find-generic-password", "-s", "fogu", "-a", &account])
                .output()
                .unwrap();
            String::from_utf8_lossy(&out.stdout)
                .lines()
                .find(|l| l.contains("\"cdat\""))
                .map(str::to_owned)
        };
        let store = SecretStore::Keychain;
        store.delete(&upper).await.unwrap();
        store.set(&upper, &SecretString::from("one")).await.unwrap();
        let before = created();
        tokio::time::sleep(std::time::Duration::from_millis(1100)).await;
        store.set(&lower, &SecretString::from("two")).await.unwrap();
        let after = created();
        let got = store.get(&upper).await.unwrap();
        store.delete(&upper).await.unwrap();
        assert!(before.is_some());
        assert_eq!(before, after, "the item was recreated");
        assert_eq!(
            got.map(|s| s.expose_secret().to_owned()).as_deref(),
            Some("two")
        );
    }

    /// An item whose access list trusts no program (as after a rebuild changes Fogu's
    /// signature): `exists` (the profile editor's "stored" marks) must answer without the
    /// Keychain access dialog, which only reading the secret itself needs. Needs
    /// `FOGU_TEST_KEYCHAIN=1`.
    #[cfg(target_os = "macos")]
    #[tokio::test(flavor = "multi_thread")]
    async fn live_keychain_exists_needs_no_access_dialog() {
        if std::env::var_os("FOGU_TEST_KEYCHAIN").is_none() {
            return;
        }
        let key = SecretKey::KeyPassphrase {
            key_fingerprint: "fogu-test-acl-item".into(),
        };
        let account = keychain::account(&key);
        let security = |args: &[&str]| {
            std::process::Command::new("/usr/bin/security")
                .args(args)
                .stderr(std::process::Stdio::null())
                .status()
                .is_ok_and(|s| s.success())
        };
        let item = ["-s", "fogu", "-a", &account];
        let _stale = security(&[&["delete-generic-password"][..], &item].concat());
        assert!(security(
            &[&["add-generic-password"][..], &item, &["-w", "x", "-T", ""]].concat()
        ));
        let found = tokio::time::timeout(
            std::time::Duration::from_secs(3),
            SecretStore::Keychain.exists(&key),
        )
        .await;
        let _cleanup = security(&[&["delete-generic-password"][..], &item].concat());
        assert_eq!(found, Ok(Ok(true)), "blocked on the access dialog?");
    }

    /// Against the real Secret Service of the session bus; needs `FOGU_TEST_SECRET_SERVICE=1`
    /// and an unlocked keyring.
    #[cfg(not(target_os = "macos"))]
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
