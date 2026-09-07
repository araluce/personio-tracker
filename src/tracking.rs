//! Personio attendance automation.
//!
//! A port of the original Node/Playwright implementation. Behaviour is kept
//! deliberately identical: walk the current month's timesheet top to bottom,
//! fill every trackable day that has no entry yet, stop once today's row has
//! been handled, then step back a month while hours are still missing.
//!
//! Notable differences forced by the move off Playwright are called out inline.

use std::time::Duration;

use anyhow::{Context, Result, bail};
use chromiumoxide::browser::Browser;
use chromiumoxide::page::Page;
use serde::Deserialize;
use tokio::time::sleep;

use crate::browser::locator::{DEFAULT_TIMEOUT, Locator, SHORT_TIMEOUT, eval, js_string};
use crate::browser::{launch, session};
use crate::config::{RunConfig, parse_time};
use crate::event::{EventSink, Summary, TrackingEvent, emit};

/// Personio selectors, kept together because they are the part most likely to
/// break when Personio ships a redesign.
mod selectors {
    /// Timesheet day rows.
    pub const ROWS: &str = r#"[role="row"][data-test-id="timesheet-timecard"]"#;
    /// XPath form of `ROWS`, used to resolve a single row without
    /// materialising its ~30 siblings.
    pub const ROWS_XPATH: &str = r#"//*[@role="row"][@data-test-id="timesheet-timecard"]"#;
    /// Present on a row that already has a registered time range.
    pub const REGISTERED_RANGE: &str = r#"[data-test-id="range-cell-time"]"#;
    /// The visible day number inside a row.
    pub const DAY_LABEL: &str = ".DayCell-module__dateGrid___gi8vf time";
    /// Holiday or absence name shown instead of a time range.
    pub const HOLIDAY_NAME: &str = ".DayRangeCell-module__holidayName___ATMOE";

    pub const USERNAME_INPUT: &str = r#"input[name="username"]"#;
    pub const PASSWORD_INPUT: &str = r#"input[name="password"]"#;
    pub const SUBMIT_BUTTON: &str = r#"button[type="submit"]"#;

    pub const FIRST_SLOT_START: &str = r#"[data-test-id="periods.0.start"]"#;
    pub const FIRST_SLOT_END: &str = r#"[data-test-id="periods.0.end"]"#;
    /// Personio inserts the break as period 1, so the second work block is
    /// period 2 rather than period 1.
    pub const SECOND_SLOT_START: &str = r#"[data-test-id="periods.2.start"]"#;
    pub const SECOND_SLOT_END: &str = r#"[data-test-id="periods.2.end"]"#;
    pub const ADD_WORK_BUTTON: &str = r#"[data-test-id="timecard-add-work"]"#;
    pub const SAVE_BUTTON: &str = r#"[data-test-id="timecard-save-button"]"#;

    /// Segmented time inputs are labelled in the account's UI language. Both
    /// spellings are matched as a union so the selector keeps working on a
    /// Spanish or English account.
    pub const HOURS_SEGMENT: &str = r#"span[aria-label="horas"], span[aria-label="hours"]"#;
    pub const MINUTES_SEGMENT: &str = r#"span[aria-label="minutos"], span[aria-label="minutes"]"#;

    pub const MONTH_HOURS_WIDGET: &str = r#"[data-test-id="widget-time-duration"]"#;
    pub const CONFIRMED_HOURS: &str =
        r#"[data-test-id="tracked-hours-widget-confirmed-time-duration"] span span"#;
    pub const TARGET_HOURS: &str =
        r#"[data-test-id="tracked-hours-widget-target-time-duration"] span span"#;

    /// `button[aria-label="Ir al mes anterior"]`, plus the English label.
    pub const PREV_MONTH_BUTTON: &str =
        r#"button[aria-label="Ir al mes anterior"], button[aria-label="Go to previous month"]"#;

    /// The cookie banner button is matched by text. The original used
    /// Playwright's `:has-text()` pseudo-class, which is not valid CSS and
    /// would throw inside `querySelectorAll`, so this is an XPath instead.
    pub const ACCEPT_COOKIES_XPATH: &str =
        r#"//button[contains(., "Accept All") or contains(., "Aceptar todo")]"#;
}

/// How long the original waited after each interaction for Personio's SPA to
/// settle. Kept as named constants rather than bare numbers.
const SETTLE_AFTER_NAVIGATION: Duration = Duration::from_secs(2);
const SETTLE_AFTER_ROW_CLICK: Duration = Duration::from_secs(2);
const SETTLE_AFTER_SAVE: Duration = Duration::from_secs(2);
const SETTLE_AFTER_TIME_INPUT: Duration = Duration::from_secs(1);
const SETTLE_AFTER_COOKIES: Duration = Duration::from_secs(1);

/// Runs a full tracking session, reporting progress through `sink`.
pub async fn run_tracking(config: &RunConfig, sink: &EventSink) -> Result<Summary> {
    let mut summary = Summary::default();

    match run_inner(config, sink, &mut summary).await {
        Ok(()) => {
            emit(
                sink,
                TrackingEvent::SessionFinish(Box::new(summary.clone())),
            );
            Ok(summary)
        }
        Err(error) => {
            let message = format!("{error:#}");
            summary.errors.push(message.clone());
            emit(sink, TrackingEvent::SessionError(message));
            Err(error)
        }
    }
}

async fn run_inner(config: &RunConfig, sink: &EventSink, summary: &mut Summary) -> Result<()> {
    emit(
        sink,
        TrackingEvent::SessionStart {
            show_browser: config.show_browser,
        },
    );

    let handle = launch(config.show_browser).await?;
    let result = drive(handle.browser(), config, sink, summary).await;
    handle.close().await;
    result
}

async fn drive(
    browser: &Browser,
    config: &RunConfig,
    sink: &EventSink,
    summary: &mut Summary,
) -> Result<()> {
    let saved_session = session::load(&config.session_file);

    // Cookies have to land before the first request, otherwise Personio
    // answers with the login page and the saved session is wasted.
    if let Some(state) = &saved_session {
        session::restore_cookies(browser, state).await?;
    }

    let page = browser
        .new_page("about:blank")
        .await
        .context("opening a browser page")?;

    emit(
        sink,
        TrackingEvent::Navigation("Opening Personio".to_string()),
    );
    navigate(&page, &config.home_url()).await?;

    // `localStorage` is origin-scoped, so it can only be replayed now that we
    // are on the Personio origin — and only a reload makes the app read it.
    if let Some(state) = &saved_session {
        if session::restore_local_storage(&page, state)
            .await
            .unwrap_or(0)
            > 0
        {
            page.reload()
                .await
                .context("reloading after restoring localStorage")?;
            sleep(SETTLE_AFTER_NAVIGATION).await;
        }
        emit(sink, TrackingEvent::AuthSessionRestored);
    }

    accept_cookies_if_present(&page, sink).await?;
    login(browser, &page, config, sink).await?;

    emit(
        sink,
        TrackingEvent::Navigation("Opening Time Tracking".to_string()),
    );
    navigate(&page, &config.attendance_url()).await?;

    accept_cookies_if_present(&page, sink).await?;
    sleep(SETTLE_AFTER_NAVIGATION).await;

    summary.months_visited += 1;
    track_month(&page, config, sink, summary).await?;

    while goto_prev_month(&page, sink).await? && pending_hours(&page, sink).await? {
        summary.months_visited += 1;
        track_month(&page, config, sink, summary).await?;
    }

    Ok(())
}

async fn navigate(page: &Page, url: &str) -> Result<()> {
    page.goto(url)
        .await
        .with_context(|| format!("navigating to {url}"))?;
    page.wait_for_navigation()
        .await
        .with_context(|| format!("waiting for {url} to load"))?;
    Ok(())
}

async fn accept_cookies_if_present(page: &Page, sink: &EventSink) -> Result<()> {
    let Some(button) = crate::browser::nth_by_xpath(page, selectors::ACCEPT_COOKIES_XPATH).await
    else {
        return Ok(());
    };

    emit(sink, TrackingEvent::CookiesAccepting);
    button.click().await.context("accepting cookies")?;
    sleep(SETTLE_AFTER_COOKIES).await;
    Ok(())
}

async fn login(browser: &Browser, page: &Page, config: &RunConfig, sink: &EventSink) -> Result<()> {
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

async fn track_month(
    page: &Page,
    config: &RunConfig,
    sink: &EventSink,
    summary: &mut Summary,
) -> Result<()> {
    let rows = Locator::new(page, selectors::ROWS);
    rows.wait_visible(DEFAULT_TIMEOUT)
        .await
        .context("waiting for the timesheet rows")?;

    let row_count = rows.count().await;
    emit(sink, TrackingEvent::MonthTracking { row_count });

    for index in 0..row_count {
        let Some(row) = read_row(page, index).await? else {
            // The month re-rendered and shrank underneath us; nothing left.
            break;
        };

        fill_date_slot(page, index, &row, config, sink, summary).await?;

        // Today is tracked first and only then ends the month, matching the
        // original: the current day must be registered, not skipped.
        if row.today {
            emit(sink, TrackingEvent::MonthReachedToday { index });
            break;
        }
    }

    Ok(())
}

/// Everything the tracker needs to know about one timesheet row.
#[derive(Debug, Clone, Deserialize)]
struct DayRow {
    label: String,
    off_day: bool,
    weekend: bool,
    holiday: bool,
    today: bool,
    registered: bool,
    holiday_name: String,
}

impl DayRow {
    fn trackable(&self) -> bool {
        !self.off_day && !self.weekend && !self.holiday
    }

    /// Human-readable reason a row was skipped, preferring the name Personio
    /// itself shows (a public holiday or an absence type).
    fn skip_reason(&self) -> &str {
        if !self.holiday_name.is_empty() {
            return &self.holiday_name;
        }
        if self.weekend {
            return "Weekend";
        }
        if self.holiday {
            return "Holiday";
        }
        if self.off_day {
            return "Off day";
        }
        "Not trackable"
    }
}

/// Reads one row's state in a single round-trip.
///
/// The original issued six separate `getAttribute`/`count`/`textContent` calls
/// per row, which is what made an earlier version of the summary crawl. Doing
/// it in one `Runtime.evaluate` also sidesteps stale `NodeId`s: the row is
/// looked up fresh inside the page each time.
async fn read_row(page: &Page, index: usize) -> Result<Option<DayRow>> {
    let expression = format!(
        "(() => {{ \
           const rows = document.querySelectorAll({rows}); \
           const row = rows[{index}]; \
           if (!row) return null; \
           const text = (sel) => {{ \
             const el = row.querySelector(sel); \
             return el && el.textContent ? el.textContent.trim() : ''; \
           }}; \
           const flag = (name) => row.getAttribute(name) === 'true'; \
           const date = (row.getAttribute('data-date') || '').trim(); \
           return {{ \
             label: text({day_label}) || date || 'unknown-day', \
             off_day: flag('data-is-off-day'), \
             weekend: flag('data-is-weekend'), \
             holiday: flag('data-is-holiday'), \
             today: flag('data-is-today'), \
             registered: row.querySelectorAll({registered}).length > 0, \
             holiday_name: text({holiday_name}) \
           }}; \
         }})()",
        rows = js_string(selectors::ROWS),
        day_label = js_string(selectors::DAY_LABEL),
        registered = js_string(selectors::REGISTERED_RANGE),
        holiday_name = js_string(selectors::HOLIDAY_NAME),
    );

    eval(page, &expression)
        .await
        .with_context(|| format!("reading timesheet row {index}"))
}

async fn fill_date_slot(
    page: &Page,
    index: usize,
    row: &DayRow,
    config: &RunConfig,
    sink: &EventSink,
    summary: &mut Summary,
) -> Result<()> {
    if !row.trackable() {
        let label = format!("{} — {}", row.label, row.skip_reason());
        summary.skipped_days.push(label.clone());
        emit(sink, TrackingEvent::DaySkipped(label));
        return Ok(());
    }

    if row.registered {
        summary.already_registered_days.push(row.label.clone());
        emit(sink, TrackingEvent::DayAlreadyRegistered(row.label.clone()));
        return Ok(());
    }

    // Resolved by index at click time: the previous save re-rendered the month,
    // so any element captured earlier would point at a detached node.
    let xpath = format!("({})[{}]", selectors::ROWS_XPATH, index + 1);
    let element = crate::browser::nth_by_xpath(page, &xpath)
        .await
        .with_context(|| format!("resolving timesheet row {index} for editing"))?;

    element
        .click()
        .await
        .with_context(|| format!("opening the editor for {}", row.label))?;
    sleep(SETTLE_AFTER_ROW_CLICK).await;

    fill_time_input(
        page,
        selectors::FIRST_SLOT_START,
        &config.start_time_first_slot,
    )
    .await?;
    fill_time_input(page, selectors::FIRST_SLOT_END, &config.end_time_first_slot).await?;

    Locator::new(page, selectors::ADD_WORK_BUTTON)
        .click(DEFAULT_TIMEOUT)
        .await
        .context("adding the second work block")?;

    fill_time_input(
        page,
        selectors::SECOND_SLOT_START,
        &config.start_time_second_slot,
    )
    .await?;
    fill_time_input(
        page,
        selectors::SECOND_SLOT_END,
        &config.end_time_second_slot,
    )
    .await?;

    let save_button = Locator::new(page, selectors::SAVE_BUTTON)
        .click(DEFAULT_TIMEOUT)
        .await
        .context("saving the timecard")?;
    save_button
        .press_key("Enter")
        .await
        .context("confirming the save")?;
    sleep(SETTLE_AFTER_SAVE).await;

    summary.tracked_days.push(row.label.clone());
    emit(sink, TrackingEvent::DayTracked(row.label.clone()));
    Ok(())
}

/// Fills one segmented `HH:MM` control: click the hours segment, type, tab to
/// the minutes segment, type again.
async fn fill_time_input(page: &Page, input_selector: &str, time: &str) -> Result<()> {
    let (hours, minutes) = parse_time(time)?;

    let hours_segment = format!("{input_selector} :is({})", selectors::HOURS_SEGMENT);
    let minutes_segment = format!("{input_selector} :is({})", selectors::MINUTES_SEGMENT);

    let hours_field = Locator::new(page, &hours_segment)
        .click(DEFAULT_TIMEOUT)
        .await
        .with_context(|| format!("focusing the hours segment of {input_selector}"))?;
    hours_field.type_str(&hours).await.context("typing hours")?;
    hours_field
        .press_key("Tab")
        .await
        .context("tabbing to minutes")?;

    let minutes_field = Locator::new(page, &minutes_segment)
        .click(DEFAULT_TIMEOUT)
        .await
        .with_context(|| format!("focusing the minutes segment of {input_selector}"))?;
    minutes_field
        .type_str(&minutes)
        .await
        .context("typing minutes")?;

    sleep(SETTLE_AFTER_TIME_INPUT).await;
    Ok(())
}

/// True when the visible month still has fewer confirmed hours than its target.
async fn pending_hours(page: &Page, sink: &EventSink) -> Result<bool> {
    let widget = Locator::new(page, selectors::MONTH_HOURS_WIDGET);

    if widget.wait_in_view(Duration::from_secs(10)).await.is_err() {
        let count = widget.count().await;
        emit(
            sink,
            TrackingEvent::HoursPendingCheckMissing(format!(
                "No tracked month hours found (count={count})"
            )),
        );
        return Ok(false);
    }

    let confirmed = read_hours(page, selectors::CONFIRMED_HOURS).await;
    let target = read_hours(page, selectors::TARGET_HOURS).await;

    let (Some(confirmed), Some(target)) = (confirmed, target) else {
        emit(
            sink,
            TrackingEvent::HoursPendingCheckMissing(
                "Could not read the confirmed/target hours".to_string(),
            ),
        );
        return Ok(false);
    };

    let pending = confirmed < target;
    emit(
        sink,
        TrackingEvent::HoursPendingCheck {
            confirmed,
            target,
            pending,
        },
    );

    Ok(pending)
}

async fn read_hours(page: &Page, selector: &str) -> Option<f64> {
    let text = Locator::new(page, selector).first_text().await?;
    parse_hours(&text)
}

/// Parses Personio's hour labels, e.g. `"160 h"` or `"158,5 h"`.
///
/// The original used `Number(text.replace('h',''))`, which yields `NaN` on a
/// comma decimal separator — and `NaN < NaN` is false, so the month walk would
/// stop early on a Spanish-formatted account. Both separators are handled here.
fn parse_hours(text: &str) -> Option<f64> {
    let cleaned: String = text
        .chars()
        .filter(|c| c.is_ascii_digit() || *c == ',' || *c == '.' || *c == '-')
        .collect();

    cleaned.replace(',', ".").parse().ok()
}

async fn goto_prev_month(page: &Page, sink: &EventSink) -> Result<bool> {
    let button = Locator::new(page, selectors::PREV_MONTH_BUTTON);

    if !button.is_visible(SHORT_TIMEOUT).await {
        emit(sink, TrackingEvent::MonthPreviousMissing);
        return Ok(false);
    }

    emit(sink, TrackingEvent::MonthPrevious);
    button.click(DEFAULT_TIMEOUT).await?;
    sleep(SETTLE_AFTER_NAVIGATION).await;
    Ok(true)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn row() -> DayRow {
        DayRow {
            label: "12".into(),
            off_day: false,
            weekend: false,
            holiday: false,
            today: false,
            registered: false,
            holiday_name: String::new(),
        }
    }

    #[test]
    fn a_plain_weekday_is_trackable() {
        assert!(row().trackable());
    }

    #[test]
    fn off_days_weekends_and_holidays_are_not_trackable() {
        for mutate in [
            (|r: &mut DayRow| r.off_day = true) as fn(&mut DayRow),
            |r: &mut DayRow| r.weekend = true,
            |r: &mut DayRow| r.holiday = true,
        ] {
            let mut candidate = row();
            mutate(&mut candidate);
            assert!(!candidate.trackable());
        }
    }

    #[test]
    fn personios_own_label_wins_over_the_generic_reason() {
        let mut candidate = row();
        candidate.holiday = true;
        candidate.holiday_name = "Sant Joan".into();

        assert_eq!(candidate.skip_reason(), "Sant Joan");
    }

    #[test]
    fn skip_reason_priority_matches_the_original() {
        let mut candidate = row();
        candidate.weekend = true;
        candidate.holiday = true;
        candidate.off_day = true;
        assert_eq!(candidate.skip_reason(), "Weekend");

        let mut candidate = row();
        candidate.holiday = true;
        candidate.off_day = true;
        assert_eq!(candidate.skip_reason(), "Holiday");

        let mut candidate = row();
        candidate.off_day = true;
        assert_eq!(candidate.skip_reason(), "Off day");

        assert_eq!(row().skip_reason(), "Not trackable");
    }

    #[test]
    fn parses_both_decimal_separators() {
        assert_eq!(parse_hours("160 h"), Some(160.0));
        assert_eq!(parse_hours("158,5 h"), Some(158.5));
        assert_eq!(parse_hours("158.5h"), Some(158.5));
        assert_eq!(parse_hours(" 0 h "), Some(0.0));
    }

    #[test]
    fn unparseable_hours_yield_none_instead_of_nan() {
        for text in ["", "h", "--", "n/a"] {
            assert_eq!(parse_hours(text), None, "{text:?} should not parse");
        }
    }

    #[test]
    fn row_metadata_deserialises_from_the_page_payload() {
        let raw = r#"{
            "label": "3",
            "off_day": false,
            "weekend": true,
            "holiday": false,
            "today": false,
            "registered": false,
            "holiday_name": ""
        }"#;

        let parsed: DayRow = serde_json::from_str(raw).unwrap();
        assert_eq!(parsed.label, "3");
        assert!(!parsed.trackable());
        assert_eq!(parsed.skip_reason(), "Weekend");
    }

    #[test]
    fn the_cookie_selector_is_xpath_not_playwright_pseudo_css() {
        assert!(!selectors::ACCEPT_COOKIES_XPATH.contains(":has-text"));
        assert!(selectors::ACCEPT_COOKIES_XPATH.starts_with("//button"));
    }
}
