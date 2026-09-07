//! Auto-waiting element helpers on top of raw CDP.
//!
//! Playwright's `Locator` is lazy: it re-resolves the selector on every call,
//! and every action implicitly waits for the element to be actionable. Raw CDP
//! gives us neither — `find_element` fails immediately when nothing matches,
//! and an `Element` holds a `NodeId` that goes stale as soon as the page
//! re-renders. Both matter here: clicking a timesheet row opens an editor that
//! re-renders the whole month.
//!
//! So every helper in this module resolves the selector from the document at
//! call time and, where Playwright would wait, polls until a deadline.

use std::time::Duration;

use anyhow::{Context, Result, bail};
use chromiumoxide::element::Element;
use chromiumoxide::page::Page;
use serde::de::DeserializeOwned;
use tokio::time::{Instant, sleep};

/// Matches Playwright's default action timeout.
pub const DEFAULT_TIMEOUT: Duration = Duration::from_secs(15);
/// Used where the original code passed an explicit short timeout.
pub const SHORT_TIMEOUT: Duration = Duration::from_secs(3);

const POLL_INTERVAL: Duration = Duration::from_millis(150);

/// A lazily-resolved CSS selector bound to a page.
pub struct Locator<'a> {
    page: &'a Page,
    selector: String,
}

impl<'a> Locator<'a> {
    pub fn new(page: &'a Page, selector: impl Into<String>) -> Self {
        Self {
            page,
            selector: selector.into(),
        }
    }

    /// Number of matches right now, without waiting. A failed query counts as
    /// zero so callers can treat "missing" and "absent" alike, the way
    /// `locator.count()` does.
    pub async fn count(&self) -> usize {
        eval(
            self.page,
            &format!(
                "document.querySelectorAll({}).length",
                js_string(&self.selector)
            ),
        )
        .await
        .unwrap_or(0)
    }

    /// Resolves the nth match into a live element, re-reading the document so
    /// the returned `NodeId` is fresh.
    pub async fn nth(&self, index: usize) -> Option<Element> {
        let elements = self.page.find_elements(self.selector.clone()).await.ok()?;
        elements.into_iter().nth(index)
    }

    pub async fn first(&self) -> Option<Element> {
        self.nth(0).await
    }

    /// Text of the first match, or `None` when nothing matches.
    ///
    /// Reading through JS keeps this to a single round-trip and avoids
    /// resolving an element only to discover it is not there — the original
    /// implementation had to guard `textContent()` behind a `count()` check for
    /// exactly this reason.
    pub async fn first_text(&self) -> Option<String> {
        let expression = format!(
            "(() => {{ const el = document.querySelector({}); \
             return el ? el.textContent : null; }})()",
            js_string(&self.selector)
        );

        eval::<Option<String>>(self.page, &expression)
            .await
            .ok()
            .flatten()
            .map(|text| text.trim().to_string())
            .filter(|text| !text.is_empty())
    }

    /// True when a match becomes visible before the timeout elapses.
    ///
    /// "Visible" follows Playwright: a non-empty bounding box and a computed
    /// `visibility` other than `hidden`.
    pub async fn is_visible(&self, timeout: Duration) -> bool {
        self.wait_until_visible(timeout).await.is_ok()
    }

    /// Waits for a visible match and returns it.
    pub async fn wait_visible(&self, timeout: Duration) -> Result<Element> {
        self.wait_until_visible(timeout).await?;
        self.first().await.with_context(|| {
            format!(
                "selector {:?} vanished after becoming visible",
                self.selector
            )
        })
    }

    async fn wait_until_visible(&self, timeout: Duration) -> Result<()> {
        let expression = format!(
            "(() => {{ \
               const el = document.querySelector({}); \
               if (!el) return false; \
               if (getComputedStyle(el).visibility === 'hidden') return false; \
               const rect = el.getBoundingClientRect(); \
               return rect.width > 0 && rect.height > 0; \
             }})()",
            js_string(&self.selector)
        );

        poll(timeout, &self.selector, || async {
            eval::<bool>(self.page, &expression).await.unwrap_or(false)
        })
        .await
    }

    /// Waits for the element to be visible, then clicks it.
    ///
    /// The clicked element is returned because keyboard input has to be sent
    /// through an `Element` — chromiumoxide keeps `type_str`/`press_key` off
    /// the public `Page` — and it is almost always the element just focused.
    pub async fn click(&self, timeout: Duration) -> Result<Element> {
        let element = self.wait_visible(timeout).await?;
        element
            .click()
            .await
            .with_context(|| format!("clicking {:?}", self.selector))?;
        Ok(element)
    }

    /// Waits for visibility, scrolls the element into view and returns it.
    pub async fn wait_in_view(&self, timeout: Duration) -> Result<Element> {
        let element = self.wait_visible(timeout).await?;
        element
            .scroll_into_view()
            .await
            .with_context(|| format!("scrolling {:?} into view", self.selector))?;
        Ok(element)
    }
}

/// Resolves a single element by 1-based XPath index without materialising its
/// siblings. Used for timesheet rows, where resolving all ~31 of them on every
/// iteration would cost hundreds of round-trips.
pub async fn nth_by_xpath(page: &Page, xpath: &str) -> Option<Element> {
    page.find_xpaths(xpath.to_string())
        .await
        .ok()?
        .into_iter()
        .next()
}

/// Evaluates an expression and deserialises its result.
pub async fn eval<T: DeserializeOwned>(page: &Page, expression: &str) -> Result<T> {
    let result = page
        .evaluate_expression(expression)
        .await
        .context("evaluating page expression")?;

    result
        .into_value::<T>()
        .context("unexpected shape returned by page expression")
}

/// Quotes a string for safe interpolation into a JS expression. Selectors here
/// contain double quotes (`button[aria-label="…"]`), so this is not optional.
pub fn js_string(value: &str) -> String {
    serde_json::to_string(value).expect("strings are always serialisable")
}

/// Polls `check` until it returns true or the timeout elapses.
async fn poll<F, Fut>(timeout: Duration, what: &str, mut check: F) -> Result<()>
where
    F: FnMut() -> Fut,
    Fut: Future<Output = bool>,
{
    let deadline = Instant::now() + timeout;

    loop {
        if check().await {
            return Ok(());
        }

        if Instant::now() >= deadline {
            bail!("timed out after {timeout:?} waiting for {what:?}");
        }

        sleep(POLL_INTERVAL).await;
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::browser::launch;

    /// Loads a fixture page in a real headless Chromium.
    ///
    /// These tests exercise the part of the port with no Playwright safety net
    /// underneath it: selector escaping, the visibility definition, and the
    /// polling that stands in for auto-waiting.
    async fn fixture(html: &str) -> (launch::BrowserHandle, chromiumoxide::Page) {
        let handle = launch::launch(false)
            .await
            .expect("launching headless Chromium");
        let page = handle
            .browser()
            .new_page("about:blank")
            .await
            .expect("opening a page");
        page.set_content(html).await.expect("setting content");
        (handle, page)
    }

    #[tokio::test]
    async fn counts_present_and_absent_selectors() {
        let (handle, page) = fixture("<div class='row'>a</div><div class='row'>b</div>").await;

        assert_eq!(Locator::new(&page, ".row").count().await, 2);
        assert_eq!(Locator::new(&page, ".missing").count().await, 0);

        handle.close().await;
    }

    #[tokio::test]
    async fn visibility_matches_playwrights_definition() {
        let (handle, page) = fixture(
            "<p id='shown'>here</p>\
             <p id='none' style='display:none'>hidden</p>\
             <p id='hidden' style='visibility:hidden'>hidden</p>\
             <p id='empty' style='width:0;height:0;overflow:hidden'></p>",
        )
        .await;

        assert!(
            Locator::new(&page, "#shown")
                .is_visible(SHORT_TIMEOUT)
                .await
        );

        for selector in ["#none", "#hidden", "#empty"] {
            assert!(
                !Locator::new(&page, selector)
                    .is_visible(Duration::from_millis(300))
                    .await,
                "{selector} should not count as visible"
            );
        }

        handle.close().await;
    }

    #[tokio::test]
    async fn waiting_gives_up_instead_of_hanging_on_a_missing_element() {
        let (handle, page) = fixture("<p>nothing to see</p>").await;

        let started = std::time::Instant::now();
        let outcome = Locator::new(&page, "#never")
            .wait_visible(Duration::from_millis(400))
            .await;

        assert!(outcome.is_err());
        assert!(started.elapsed() < Duration::from_secs(5));

        handle.close().await;
    }

    #[tokio::test]
    async fn waiting_picks_up_an_element_that_appears_late() {
        // The whole reason this layer exists: Personio renders its editor
        // after the click, so a plain `find_element` would lose the race.
        let (handle, page) = fixture(
            "<script>setTimeout(() => { \
               const el = document.createElement('button'); \
               el.id = 'late'; \
               el.textContent = 'Save'; \
               document.body.appendChild(el); \
             }, 600);</script>",
        )
        .await;

        assert!(Locator::new(&page, "#late").count().await == 0);
        assert!(
            Locator::new(&page, "#late")
                .is_visible(Duration::from_secs(5))
                .await
        );

        handle.close().await;
    }

    #[tokio::test]
    async fn selectors_containing_quotes_are_escaped_for_the_page() {
        // `button[aria-label="Ir al mes anterior"]` would end the JS string
        // literal early without escaping, and every query would throw.
        let (handle, page) = fixture(
            "<button aria-label=\"Ir al mes anterior\">prev</button>\
             <span aria-label=\"horas\">09</span>",
        )
        .await;

        let selector = r#"button[aria-label="Ir al mes anterior"]"#;
        assert_eq!(Locator::new(&page, selector).count().await, 1);
        assert!(
            Locator::new(&page, selector)
                .is_visible(SHORT_TIMEOUT)
                .await
        );

        let union = r#"span[aria-label="horas"], span[aria-label="hours"]"#;
        assert_eq!(
            Locator::new(&page, union).first_text().await.as_deref(),
            Some("09")
        );

        handle.close().await;
    }

    #[tokio::test]
    async fn text_is_trimmed_and_absent_text_is_none() {
        let (handle, page) = fixture("<em id='t'>  158,5 h \n </em><em id='blank'>   </em>").await;

        assert_eq!(
            Locator::new(&page, "#t").first_text().await.as_deref(),
            Some("158,5 h")
        );
        assert_eq!(Locator::new(&page, "#blank").first_text().await, None);
        assert_eq!(Locator::new(&page, "#gone").first_text().await, None);

        handle.close().await;
    }

    #[tokio::test]
    async fn xpath_indexing_resolves_one_row_out_of_many() {
        let (handle, page) = fixture(
            "<div role='row' data-test-id='timesheet-timecard' data-date='2026-09-01'>1</div>\
             <div role='row' data-test-id='timesheet-timecard' data-date='2026-09-02'>2</div>\
             <div role='row' data-test-id='timesheet-timecard' data-date='2026-09-03'>3</div>",
        )
        .await;

        let base = r#"//*[@role="row"][@data-test-id="timesheet-timecard"]"#;
        let second = nth_by_xpath(&page, &format!("({base})[2]"))
            .await
            .expect("second row");

        assert_eq!(
            second.attribute("data-date").await.unwrap().as_deref(),
            Some("2026-09-02")
        );
        assert!(nth_by_xpath(&page, &format!("({base})[9]")).await.is_none());

        handle.close().await;
    }

    #[tokio::test]
    async fn clicking_returns_the_element_so_keystrokes_reach_it() {
        // chromiumoxide keeps `type_str` off the public `Page`, so every
        // keystroke has to be dispatched through an element.
        let (handle, page) = fixture("<input id='who' type='text' />").await;

        let element = Locator::new(&page, "#who")
            .click(DEFAULT_TIMEOUT)
            .await
            .expect("clicking the input");
        element.type_str("09").await.expect("typing");

        let value: String = eval(&page, "document.querySelector('#who').value")
            .await
            .unwrap();
        assert_eq!(value, "09");

        handle.close().await;
    }
}
