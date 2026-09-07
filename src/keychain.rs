//! OS keychain access for the Personio password.
//!
//! Service and account names match the Electron app's `keytar` entry, so a
//! password saved there is picked up here without re-entering it.
//!
//! Reading the *secret* is what makes macOS put up its authorisation dialog;
//! reading an item's *attributes* does not. Everything here is built around
//! that difference: the secret is asked for once per process, at the moment a
//! run actually needs it, and never merely to find out whether one exists.

use std::collections::HashMap;
use std::sync::{Mutex, MutexGuard, PoisonError};

use anyhow::{Context, Result};
use keyring::Entry;

const SERVICE_NAME: &str = "personio-track";
const ACCOUNT_NAME: &str = "personio-password";

static CACHE: PasswordCache = PasswordCache::new();

fn entry() -> Result<Entry> {
    Entry::new(SERVICE_NAME, ACCOUNT_NAME).context("opening the OS keychain entry")
}

/// Whether a password is stored, without reading it.
///
/// A search matches on the item's attributes and never decrypts the secret, so
/// the OS answers it without a dialog. Calling `get_password` for this instead
/// would put one up on every start-up, before anything has been tracked.
pub fn has_password() -> bool {
    // Also what initialises the credential store the search reads from.
    if Entry::store_status().is_err() {
        return false;
    }

    let query = HashMap::from([("service", SERVICE_NAME), ("user", ACCOUNT_NAME)]);
    keyring_core::Entry::search(&query).is_ok_and(|found| !found.is_empty())
}

pub fn set_password(password: &str) -> Result<()> {
    entry()?
        .set_password(password)
        .context("writing the password to the OS keychain")?;

    // The value is already in hand, so the next run must not go back to the
    // OS — and its dialog — for something this process just wrote.
    CACHE.store(Some(password.to_string()));
    Ok(())
}

/// Resolves the password to use for a run: keychain first, environment second.
///
/// The keychain is consulted once per process. Later runs reuse that answer,
/// so a session that tracks five times still authorises once.
pub fn resolve_password() -> Option<String> {
    if let Some(resolved) = CACHE.get() {
        return resolved;
    }

    match read_password() {
        Stored::Known(stored) => {
            let resolved = stored.or_else(password_from_env);
            CACHE.store(resolved.clone());
            resolved
        }
        // Deliberately not cached: a mis-clicked Deny or a locked keychain
        // would otherwise become a permanent "no password" until restart.
        Stored::Unavailable => password_from_env(),
    }
}

/// What the OS had to say when asked for the secret.
enum Stored {
    /// It answered: either with a password, or that there is none.
    Known(Option<String>),
    /// It could not answer — dialog denied, keychain locked, no store.
    Unavailable,
}

fn read_password() -> Stored {
    let Ok(entry) = entry() else {
        return Stored::Unavailable;
    };

    match entry.get_password() {
        Ok(stored) if stored.trim().is_empty() => Stored::Known(None),
        Ok(stored) => Stored::Known(Some(stored)),
        Err(keyring::Error::NoEntry) => Stored::Known(None),
        Err(_) => Stored::Unavailable,
    }
}

fn password_from_env() -> Option<String> {
    std::env::var("PERSONIO_PASSWORD")
        .ok()
        .filter(|value| !value.trim().is_empty())
}

/// Remembers the first resolution for the life of the process. The outer
/// `Option` is "has it been resolved", the inner one "was there a password".
struct PasswordCache {
    resolved: Mutex<Option<Option<String>>>,
}

impl PasswordCache {
    const fn new() -> Self {
        Self {
            resolved: Mutex::new(None),
        }
    }

    fn get(&self) -> Option<Option<String>> {
        self.lock().clone()
    }

    fn store(&self, resolved: Option<String>) {
        *self.lock() = Some(resolved);
    }

    /// A poisoned lock still holds a usable value, and losing keychain access
    /// for the rest of the session is a worse outcome than a stale read.
    fn lock(&self) -> MutexGuard<'_, Option<Option<String>>> {
        self.resolved.lock().unwrap_or_else(PoisonError::into_inner)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::cell::Cell;

    #[test]
    fn an_unresolved_cache_holds_nothing() {
        assert_eq!(PasswordCache::new().get(), None);
    }

    #[test]
    fn a_resolved_password_is_handed_back_without_reading_again() {
        let cache = PasswordCache::new();
        cache.store(Some("secret".to_string()));

        assert_eq!(cache.get(), Some(Some("secret".to_string())));
    }

    #[test]
    fn the_absence_of_a_password_is_remembered_too() {
        let cache = PasswordCache::new();
        cache.store(None);

        // Some(None), not None: asked and answered "nothing stored".
        assert_eq!(cache.get(), Some(None));
    }

    #[test]
    fn storing_again_replaces_what_was_remembered() {
        let cache = PasswordCache::new();
        cache.store(Some("old".to_string()));
        cache.store(Some("new".to_string()));

        assert_eq!(cache.get(), Some(Some("new".to_string())));
    }

    /// The whole point of the cache: the caller reads the OS at most once.
    #[test]
    fn a_cached_resolution_short_circuits_the_read() {
        let cache = PasswordCache::new();
        let reads = Cell::new(0);
        let resolve = || match cache.get() {
            Some(resolved) => resolved,
            None => {
                reads.set(reads.get() + 1);
                let read = Some("secret".to_string());
                cache.store(read.clone());
                read
            }
        };

        assert_eq!(resolve(), Some("secret".to_string()));
        assert_eq!(resolve(), Some("secret".to_string()));
        assert_eq!(reads.get(), 1);
    }
}
