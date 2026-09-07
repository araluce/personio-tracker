//! Session persistence — the equivalent of Playwright's `storageState`.
//!
//! Playwright saves cookies *and* per-origin `localStorage` into a JSON file
//! and replays both into a fresh context. CDP gives us cookies directly, but
//! `localStorage` can only be touched from a page already on the origin, so
//! restoring it is a two-step dance: cookies before navigating, storage after.

use std::collections::BTreeMap;
use std::path::Path;

use anyhow::{Context, Result};
use chromiumoxide::browser::Browser;
use chromiumoxide::cdp::browser_protocol::network::{CookieParam, CookieSameSite, TimeSinceEpoch};
use chromiumoxide::page::Page;
use serde::{Deserialize, Serialize};

use super::locator::eval;

#[derive(Debug, Clone, Default, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct SessionState {
    #[serde(default)]
    pub cookies: Vec<StoredCookie>,
    #[serde(default)]
    pub local_storage: BTreeMap<String, String>,
}

impl SessionState {
    pub fn is_empty(&self) -> bool {
        self.cookies.is_empty() && self.local_storage.is_empty()
    }
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct StoredCookie {
    pub name: String,
    pub value: String,
    pub domain: String,
    pub path: String,
    /// Seconds since the epoch; `-1` (or any non-positive value) marks a
    /// session cookie, which is restored without an expiry.
    #[serde(default)]
    pub expires: f64,
    #[serde(default)]
    pub http_only: bool,
    #[serde(default)]
    pub secure: bool,
    #[serde(default)]
    pub same_site: Option<String>,
}

impl StoredCookie {
    fn to_param(&self) -> Result<CookieParam> {
        let mut builder = CookieParam::builder()
            .name(&self.name)
            .value(&self.value)
            .domain(&self.domain)
            .path(&self.path)
            .http_only(self.http_only)
            .secure(self.secure);

        if self.expires > 0.0 {
            builder = builder.expires(TimeSinceEpoch::new(self.expires));
        }

        if let Some(same_site) = self.same_site.as_deref().and_then(parse_same_site) {
            builder = builder.same_site(same_site);
        }

        builder
            .build()
            .map_err(|error| anyhow::anyhow!("rebuilding cookie {:?}: {error}", self.name))
    }
}

fn parse_same_site(value: &str) -> Option<CookieSameSite> {
    match value {
        "Strict" => Some(CookieSameSite::Strict),
        "Lax" => Some(CookieSameSite::Lax),
        "None" => Some(CookieSameSite::None),
        _ => None,
    }
}

pub fn load(path: &Path) -> Option<SessionState> {
    let raw = std::fs::read_to_string(path).ok()?;
    let state: SessionState = serde_json::from_str(&raw).ok()?;
    (!state.is_empty()).then_some(state)
}

/// Snapshots cookies and the current origin's `localStorage`.
pub async fn save(browser: &Browser, page: &Page, path: &Path) -> Result<()> {
    let cookies = browser
        .get_cookies()
        .await
        .context("reading cookies from the browser")?
        .into_iter()
        .map(|cookie| StoredCookie {
            name: cookie.name,
            value: cookie.value,
            domain: cookie.domain,
            path: cookie.path,
            expires: cookie.expires,
            http_only: cookie.http_only,
            secure: cookie.secure,
            same_site: cookie.same_site.map(|value| value.as_ref().to_string()),
        })
        .collect();

    // A failure here is not fatal: cookies alone are usually enough to keep the
    // session alive, and we would rather store a partial state than none.
    let local_storage = read_local_storage(page).await.unwrap_or_default();

    let state = SessionState {
        cookies,
        local_storage,
    };

    if let Some(parent) = path.parent() {
        std::fs::create_dir_all(parent)
            .with_context(|| format!("creating session dir {}", parent.display()))?;
    }

    std::fs::write(path, serde_json::to_string_pretty(&state)?)
        .with_context(|| format!("writing session file {}", path.display()))?;

    Ok(())
}

/// Replays cookies into a freshly launched browser. Must run before the first
/// navigation so the very first request is already authenticated.
pub async fn restore_cookies(browser: &Browser, state: &SessionState) -> Result<usize> {
    if state.cookies.is_empty() {
        return Ok(0);
    }

    let params: Vec<CookieParam> = state
        .cookies
        .iter()
        .filter_map(|cookie| cookie.to_param().ok())
        .collect();

    let restored = params.len();
    browser
        .set_cookies(params)
        .await
        .context("restoring cookies into the browser")?;

    Ok(restored)
}

/// Replays `localStorage` once a page is on the target origin.
pub async fn restore_local_storage(page: &Page, state: &SessionState) -> Result<usize> {
    if state.local_storage.is_empty() {
        return Ok(0);
    }

    let payload = serde_json::to_string(&state.local_storage)?;
    let expression = format!(
        "(() => {{ \
           const data = {payload}; \
           let written = 0; \
           for (const key of Object.keys(data)) {{ \
             try {{ window.localStorage.setItem(key, data[key]); written += 1; }} catch (e) {{}} \
           }} \
           return written; \
         }})()"
    );

    eval::<usize>(page, &expression)
        .await
        .context("restoring localStorage")
}

async fn read_local_storage(page: &Page) -> Result<BTreeMap<String, String>> {
    let expression = "(() => { \
         const out = {}; \
         try { \
           for (let i = 0; i < window.localStorage.length; i += 1) { \
             const key = window.localStorage.key(i); \
             if (key !== null) out[key] = window.localStorage.getItem(key); \
           } \
         } catch (e) {} \
         return out; \
       })()";

    eval(page, expression).await
}

#[cfg(test)]
mod tests {
    use super::*;

    fn cookie(expires: f64, same_site: Option<&str>) -> StoredCookie {
        StoredCookie {
            name: "sid".into(),
            value: "abc".into(),
            domain: ".app.personio.com".into(),
            path: "/".into(),
            expires,
            http_only: true,
            secure: true,
            same_site: same_site.map(str::to_string),
        }
    }

    #[test]
    fn session_cookies_are_restored_without_an_expiry() {
        let param = cookie(-1.0, None).to_param().unwrap();
        assert!(param.expires.is_none());

        let param = cookie(1893456000.0, None).to_param().unwrap();
        assert_eq!(
            param.expires.as_ref().map(|value| *value.inner()),
            Some(1893456000.0)
        );
    }

    #[test]
    fn unknown_same_site_values_are_dropped_instead_of_failing() {
        assert!(
            cookie(-1.0, Some("Nonsense"))
                .to_param()
                .unwrap()
                .same_site
                .is_none()
        );
        assert_eq!(
            cookie(-1.0, Some("Lax")).to_param().unwrap().same_site,
            Some(CookieSameSite::Lax)
        );
    }

    #[test]
    fn state_round_trips_through_json() {
        let mut local_storage = BTreeMap::new();
        local_storage.insert("token".to_string(), "value".to_string());
        let state = SessionState {
            cookies: vec![cookie(-1.0, Some("Lax"))],
            local_storage,
        };

        let raw = serde_json::to_string(&state).unwrap();
        let parsed: SessionState = serde_json::from_str(&raw).unwrap();

        assert_eq!(parsed.cookies.len(), 1);
        assert_eq!(
            parsed.local_storage.get("token").map(String::as_str),
            Some("value")
        );
        assert!(!parsed.is_empty());
    }

    #[test]
    fn empty_state_is_reported_as_empty() {
        assert!(SessionState::default().is_empty());
    }
}
