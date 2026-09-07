//! Getting in: the cookie banner, the two-step login form, and waiting for
//! Personio to accept the session.
//!
//! A run that replays a saved session never reaches [`login`], which is what
//! keeps the OS keychain out of the way of an ordinary run.

use std::time::Duration;

use anyhow::{Context, Result, bail};
use chromiumoxide::browser::Browser;
use chromiumoxide::page::Page;
use tokio::time::sleep;

use super::{SETTLE_AFTER_COOKIES, selectors};
use crate::browser::locator::{DEFAULT_TIMEOUT, Locator, SHORT_TIMEOUT, eval, js_string};
use crate::browser::session;
use crate::config::RunConfig;
use crate::event::{EventSink, TrackingEvent, emit};

pub(super) async fn accept_cookies_if_present(page: &Page, sink: &EventSink) -> Result<()> {
    let Some(button) = crate::browser::nth_by_xpath(page, selectors::ACCEPT_COOKIES_XPATH).await
    else {
        return Ok(());
    };

    emit(sink, TrackingEvent::CookiesAccepting);
    button.click().await.context("accepting cookies")?;
    sleep(SETTLE_AFTER_COOKIES).await;
    Ok(())
}

pub(super) async fn login(
    browser: &Browser,
    page: &Page,
    config: &RunConfig,
    sink: &EventSink,
) -> Result<()> {
    let username = Locator::new(page, selectors::USERNAME_INPUT);
    if !username.is_visible(SHORT_TIMEOUT).await {
        emit(sink, TrackingEvent::AuthSessionValid);
        return Ok(());
    }

    emit(sink, TrackingEvent::AuthLoginStart);

    // First and only point in a run that needs the password: a replayed
    // session returns above, without ever reaching the keychain. Resolving on
    // a blocking thread because the OS dialog sits there for as long as the
    // user takes, and a parked runtime worker would freeze the UI with it.
    let provider = config.personio_password.clone();
    let password = tokio::task::spawn_blocking(move || provider.resolve())
        .await
        .context("resolving the password")?;

    let Some(password) = password else {
        bail!(
            "Login required but no password is available. \
             Store one from the Password field in the UI, \
             or set PERSONIO_PASSWORD."
        );
    };

    fill_input(page, selectors::USERNAME_INPUT, &config.personio_email).await?;
    Locator::new(page, selectors::SUBMIT_BUTTON)
        .click(DEFAULT_TIMEOUT)
        .await?;

    // Personio splits login across two steps, so the password field only
    // appears after the email has been submitted.
    Locator::new(page, selectors::PASSWORD_INPUT)
        .wait_visible(DEFAULT_TIMEOUT)
        .await
        .context("waiting for the password field")?;

    fill_input(page, selectors::PASSWORD_INPUT, &password).await?;
    Locator::new(page, selectors::SUBMIT_BUTTON)
        .click(DEFAULT_TIMEOUT)
        .await?;

    wait_for_login(page, config).await?;

    session::save(browser, page, &config.session_file)
        .await
        .context("saving the session")?;
    emit(sink, TrackingEvent::AuthSessionSaved);

    accept_cookies_if_present(page, sink).await
}

/// Login is complete once we are back on the Personio origin with no
/// credential fields left on the page.
///
/// The original waited for the URL to match the base URL exactly. Checking
/// that the inputs are gone as well avoids a false positive when Personio
/// bounces through an interstitial that shares the origin.
async fn wait_for_login(page: &Page, config: &RunConfig) -> Result<()> {
    let base_url = config.base_url();
    let deadline = tokio::time::Instant::now() + Duration::from_secs(30);

    loop {
        let on_origin = page
            .url()
            .await
            .ok()
            .flatten()
            .is_some_and(|url| url.starts_with(&base_url));

        if on_origin {
            let credentials_gone = Locator::new(page, selectors::USERNAME_INPUT).count().await == 0
                && Locator::new(page, selectors::PASSWORD_INPUT).count().await == 0;
            if credentials_gone {
                return Ok(());
            }
        }

        if tokio::time::Instant::now() >= deadline {
            bail!("login did not complete within 30s — check the credentials");
        }

        sleep(Duration::from_millis(250)).await;
    }
}

/// Clears a text input and types into it.
///
/// Playwright's `fill()` sets the value and fires the events React expects.
/// Here the field is focused and emptied through JS, then typed into with real
/// key events so a controlled React input keeps its state in sync.
async fn fill_input(page: &Page, selector: &str, value: &str) -> Result<()> {
    let element = Locator::new(page, selector)
        .wait_visible(DEFAULT_TIMEOUT)
        .await?;

    let expression = format!(
        "(() => {{ \
           const el = document.querySelector({}); \
           if (!el) return false; \
           el.focus(); \
           el.value = ''; \
           el.dispatchEvent(new Event('input', {{ bubbles: true }})); \
           return true; \
         }})()",
        js_string(selector)
    );

    let focused: bool = eval(page, &expression).await?;
    if !focused {
        bail!("could not focus input {selector:?}");
    }

    element
        .type_str(value)
        .await
        .with_context(|| format!("typing into {selector:?}"))?;
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_cookie_selector_is_xpath_not_playwright_pseudo_css() {
        assert!(!selectors::ACCEPT_COOKIES_XPATH.contains(":has-text"));
        assert!(selectors::ACCEPT_COOKIES_XPATH.starts_with("//button"));
    }
}
