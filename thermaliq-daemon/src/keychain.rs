//! The user's own decision-model key (SPEC §7.2), in the OS keychain and nowhere else. A key
//! has no `Debug` or `Display`, so it can't reach a log by accident.

use std::io;

const MAX_KEY_LEN: usize = 1024;

/// Whose key it is, which fixes where it may ever be sent.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Account {
    OpenRouter,
    TypeSafe,
}

impl Account {
    pub const fn name(self) -> &'static str {
        match self {
            Self::OpenRouter => "openrouter",
            Self::TypeSafe => "typesafe",
        }
    }

    pub fn from_name(name: &str) -> Option<Self> {
        [Self::OpenRouter, Self::TypeSafe]
            .into_iter()
            .find(|a| a.name() == name)
    }

    pub const fn url(self) -> &'static str {
        match self {
            Self::OpenRouter => "https://openrouter.ai/api/v1/systemone",
            Self::TypeSafe => "https://api.typesafe.ai/v1/systemone",
        }
    }

    /// OpenRouter is pinned to the version the spike measured. TypeSafe's docs name only
    /// `jev-latest`.
    pub const fn model(self) -> &'static str {
        match self {
            Self::OpenRouter => "typesafe/jev-1.14",
            Self::TypeSafe => "jev-latest",
        }
    }
}

pub struct Key {
    pub account: Account,
    secret: String,
}

impl Key {
    pub fn new(account: Account, secret: &str) -> Option<Self> {
        let ok = !secret.is_empty()
            && secret.len() <= MAX_KEY_LEN
            && !secret.chars().any(|c| c.is_whitespace() || c.is_control());
        ok.then(|| Self {
            account,
            secret: secret.into(),
        })
    }

    pub fn secret(&self) -> &str {
        &self.secret
    }

    fn encode(&self) -> String {
        format!("{}\n{}", self.account.name(), self.secret)
    }

    fn decode(stored: &str) -> Option<Self> {
        let (account, secret) = stored.split_once('\n')?;
        Self::new(Account::from_name(account)?, secret)
    }
}

pub trait KeyStore: Send + Sync {
    fn get(&self) -> io::Result<Option<Key>>;
    fn set(&self, key: &Key) -> io::Result<()>;
    fn delete(&self) -> io::Result<()>;
}

/// Windows Credential Manager, under whichever account the daemon runs as: SYSTEM for the
/// service, so the key is set through the API, not in the user's own vault.
pub struct OsKeychain {
    #[cfg_attr(not(windows), allow(dead_code))]
    target: String,
}

impl Default for OsKeychain {
    fn default() -> Self {
        Self::named("ThermalIQ/decision-key")
    }
}

impl OsKeychain {
    pub fn named(target: &str) -> Self {
        Self {
            target: target.into(),
        }
    }
}

#[cfg(windows)]
impl KeyStore for OsKeychain {
    fn get(&self) -> io::Result<Option<Key>> {
        Ok(crate::win::cred_read(&self.target)?.and_then(|s| Key::decode(&s)))
    }
    fn set(&self, key: &Key) -> io::Result<()> {
        crate::win::cred_write(&self.target, &key.encode())
    }
    fn delete(&self) -> io::Result<()> {
        crate::win::cred_delete(&self.target)
    }
}

/// v2.0 is Windows-only (SPEC §10): elsewhere nothing is stored, and saying so is the honest
/// answer (S8).
#[cfg(not(windows))]
impl KeyStore for OsKeychain {
    fn get(&self) -> io::Result<Option<Key>> {
        Ok(None)
    }
    fn set(&self, _: &Key) -> io::Result<()> {
        Err(io::Error::other("no OS keychain on this platform"))
    }
    fn delete(&self) -> io::Result<()> {
        Ok(())
    }
}

#[cfg(test)]
#[derive(Default)]
pub struct MemoryKeys(std::sync::Mutex<Option<String>>);

#[cfg(test)]
impl KeyStore for MemoryKeys {
    fn get(&self) -> io::Result<Option<Key>> {
        let stored = self
            .0
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner);
        Ok(stored.as_deref().and_then(Key::decode))
    }
    fn set(&self, key: &Key) -> io::Result<()> {
        *self
            .0
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner) = Some(key.encode());
        Ok(())
    }
    fn delete(&self) -> io::Result<()> {
        *self
            .0
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner) = None;
        Ok(())
    }
}

#[cfg(test)]
#[allow(clippy::unwrap_used)]
mod tests {
    use super::*;

    #[test]
    fn a_key_round_trips_through_its_stored_form() {
        for account in [Account::OpenRouter, Account::TypeSafe] {
            let k = Key::new(account, "sk-abc").unwrap();
            let back = Key::decode(&k.encode()).unwrap();
            assert_eq!((back.account, back.secret()), (account, "sk-abc"));
        }
    }

    #[test]
    fn garbage_in_the_store_is_no_key() {
        for bad in ["", "openrouter", "nope\nsk-abc", "openrouter\n"] {
            assert!(Key::decode(bad).is_none(), "{bad:?}");
        }
    }

    #[test]
    fn a_key_must_look_like_one() {
        assert!(Key::new(Account::OpenRouter, "").is_none());
        assert!(Key::new(Account::OpenRouter, "two\nlines").is_none());
        assert!(Key::new(Account::OpenRouter, "has space").is_none());
        assert!(Key::new(Account::OpenRouter, &"x".repeat(2000)).is_none());
    }

    #[test]
    fn accounts_only_ever_name_https_endpoints() {
        for a in [Account::OpenRouter, Account::TypeSafe] {
            assert!(a.url().starts_with("https://"), "{}", a.url());
            assert_eq!(Account::from_name(a.name()), Some(a));
        }
        assert_eq!(Account::from_name("evil"), None);
    }

    #[cfg(windows)]
    #[test]
    fn the_windows_keychain_stores_replaces_and_deletes() {
        let store = OsKeychain::named("ThermalIQ/test-keychain-roundtrip");
        let _ = store.delete();
        assert!(store.get().unwrap().is_none());
        // ERROR_NO_SUCH_LOGON_SESSION: a logon with no credential vault, as on some CI runners
        if let Err(e) = store.set(&Key::new(Account::OpenRouter, "first").unwrap()) {
            assert_eq!(e.raw_os_error(), Some(1312), "{e}");
            eprintln!("skipped: no credential vault in this logon session");
            return;
        }
        store
            .set(&Key::new(Account::TypeSafe, "second").unwrap())
            .unwrap();
        let k = store.get().unwrap().unwrap();
        assert_eq!((k.account, k.secret()), (Account::TypeSafe, "second"));
        store.delete().unwrap();
        assert!(store.get().unwrap().is_none());
        store.delete().unwrap(); // nothing to delete is not an error
    }

    #[cfg(not(windows))]
    #[test]
    fn without_a_keychain_storing_a_key_fails_rather_than_pretends() {
        let store = OsKeychain::default();
        assert!(store.get().unwrap().is_none());
        assert!(store
            .set(&Key::new(Account::OpenRouter, "k").unwrap())
            .is_err());
    }
}
