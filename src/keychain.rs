//! OS keychain access for the Personio password.
//!
//! Service and account names match the Electron app's `keytar` entry, so a
//! password saved there is picked up here without re-entering it.

use anyhow::{Context, Result};
use keyring::Entry;

const SERVICE_NAME: &str = "personio-track";
const ACCOUNT_NAME: &str = "personio-password";

fn entry() -> Result<Entry> {
    Entry::new(SERVICE_NAME, ACCOUNT_NAME).context("opening the OS keychain entry")
}

/// Returns the stored password, or `None` when there is nothing stored and when
/// the platform keychain is unavailable. Callers fall back to `PERSONIO_PASSWORD`.
pub fn get_password() -> Option<String> {
    let stored = entry().ok()?.get_password().ok()?;
    if stored.trim().is_empty() {
        None
    } else {
        Some(stored)
    }
}

pub fn set_password(password: &str) -> Result<()> {
    entry()?
        .set_password(password)
        .context("writing the password to the OS keychain")
}

/// Resolves the password to use for a run: keychain first, environment second.
pub fn resolve_password() -> Option<String> {
    get_password().or_else(|| {
        std::env::var("PERSONIO_PASSWORD")
            .ok()
            .filter(|value| !value.trim().is_empty())
    })
}
