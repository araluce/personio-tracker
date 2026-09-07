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
use chrono::{DateTime, Local, NaiveDate, NaiveDateTime};
use serde::Deserialize;
use tokio::time::sleep;

use crate::browser::locator::{DEFAULT_TIMEOUT, Locator, SHORT_TIMEOUT, eval, js_string};
use crate::browser::{launch, session};
use crate::config::{RunConfig, parse_time};
use crate::event::{DaySlot, EventSink, SkipKind, Summary, TrackingEvent, emit};

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
    /// The `datetime` attribute of the row's `<time>` element — the one field
    /// HTML defines as a machine-readable date.
    #[serde(default)]
    day_iso: String,
    /// Personio's `data-date`, when the row carries one.
    ///
    /// Both are defaulted rather than required: a page that stops emitting
    /// one should cost the grid some precision, never abort the run.
    #[serde(default)]
    date: String,
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

    /// Which calendar day this row is, from the most trustworthy source that
    /// answers.
    ///
    /// The printed label comes first, and deliberately so. It is what Personio
    /// shows the user and what the run log repeats back, so a grid built from
    /// it can never disagree with either. The row's date attributes look like
    /// better evidence — a whole date, not just a day — but on a real account
    /// they came back a day behind the label they sat next to, which put every
    /// cell one column to the left and the 1st into the previous month. They
    /// are kept as a fallback for a row that prints no readable day, and no
    /// higher than that.
    ///
    /// Last of all, the row's own position, which the recorder will only trust
    /// once it has checked that the month is listed one row per day.
    fn slot(&self, index: usize) -> DaySlot {
        // A label that is itself a date, before digits get read out of it:
        // `2026-09-01` would otherwise scan as the 9th.
        if let Some(date) = parse_date_attribute(&self.label) {
            return DaySlot::Dated(date);
        }

        if let Some(day) = day_of_month(&self.label) {
            return DaySlot::DayOfMonth(day);
        }

        for raw in [&self.day_iso, &self.date] {
            if let Some(date) = parse_date_attribute(raw) {
                return DaySlot::Dated(date);
            }
        }

        DaySlot::Row(index)
    }

    /// Which of Personio's flags is keeping the day off limits, checked in the
    /// same order as `trackable`.
    fn skip_kind(&self) -> SkipKind {
        if self.weekend {
            SkipKind::Weekend
        } else if self.holiday {
            SkipKind::Holiday
        } else {
            SkipKind::OffDay
        }
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

/// Parses the calendar day out of a date attribute.
///
/// A bare `2026-09-01` is simply that day. A timestamp with an offset is an
/// *instant*, not a day, and Personio writes local midnight as one: in Madrid
/// the 1st of September starts at `2026-08-31T22:00:00Z`. Reading the text
/// before the `T` would call that the 31st of August and shift the entire
/// grid back by a day, so the instant is moved into local time first.
fn parse_date_attribute(raw: &str) -> Option<NaiveDate> {
    let raw = raw.trim();

    // No instant to place: a plain date is already a day.
    if let Ok(date) = NaiveDate::parse_from_str(raw, "%Y-%m-%d") {
        return Some(date);
    }

    // An offset makes it an instant, and its day is whichever day it is here.
    if let Ok(moment) = DateTime::parse_from_rfc3339(raw) {
        return Some(moment.with_timezone(&Local).date_naive());
    }

    // No offset: already a wall-clock reading, so its own date is the day.
    ["%Y-%m-%dT%H:%M:%S%.f", "%Y-%m-%d %H:%M:%S%.f"]
        .into_iter()
        .find_map(|format| NaiveDateTime::parse_from_str(raw, format).ok())
        .map(|moment| moment.date())
}

/// The first day-of-month number in a label, so a printed `lun 1` or `1 sept.`
/// still places its row.
fn day_of_month(label: &str) -> Option<u32> {
    label
        .split(|character: char| !character.is_ascii_digit())
        .filter(|group| !group.is_empty())
        .filter_map(|group| group.parse::<u32>().ok())
        .find(|day| (1..=31).contains(day))
}

/// Reads one row's state in a single round-trip.
///
/// The original issued six separate `getAttribute`/`count`/`textContent` calls
/// per row, which is what made an earlier version of the summary crawl. Doing
/// it in one `Runtime.evaluate` also sidesteps stale `NodeId`s: the row is
/// looked up fresh inside the page each time.
async fn read_row(page: &Page, index: usize) -> Result<Option<DayRow>> {
    eval(page, &row_expression(index))
        .await
        .with_context(|| format!("reading timesheet row {index}"))
}

/// The page-side reader for one row.
///
/// Built here rather than inline so it can be inspected in a test: a syntax
/// error in it would not break one row, it would break every read in the run.
fn row_expression(index: usize) -> String {
    format!(
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
           const stamp = row.querySelector({day_label}); \
           const day_iso = stamp ? (stamp.getAttribute('datetime') || '').trim() : ''; \
           return {{ \
             label: text({day_label}) || day_iso || date || 'unknown-day', \
             day_iso, \
             date, \
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
    )
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
        emit(
            sink,
            TrackingEvent::DaySkipped {
                slot: row.slot(index),
                label,
                kind: row.skip_kind(),
            },
        );
        return Ok(());
    }

    if row.registered {
        summary.already_registered_days.push(row.label.clone());
        emit(
            sink,
            TrackingEvent::DayAlreadyRegistered {
                slot: row.slot(index),
                label: row.label.clone(),
            },
        );
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
    emit(
        sink,
        TrackingEvent::DayTracked {
            slot: row.slot(index),
            label: row.label.clone(),
        },
    );
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
    use chrono::{TimeZone, Utc};

    fn row() -> DayRow {
        DayRow {
            label: "12".into(),
            day_iso: "2026-09-12".into(),
            date: String::new(),
            off_day: false,
            weekend: false,
            holiday: false,
            today: false,
            registered: false,
            holiday_name: String::new(),
        }
    }

    fn september(day: u32) -> DaySlot {
        DaySlot::Dated(NaiveDate::from_ymd_opt(2026, 9, day).unwrap())
    }

    /// The bug this ordering exists for: the row's date attribute disagreed
    /// with the day the row printed, and the printed day was the right one.
    #[test]
    fn the_printed_day_wins_over_a_date_attribute_that_disagrees() {
        let mut candidate = row();
        candidate.label = "1 sept".into();
        // What a real account sent for its 1 September row.
        candidate.day_iso = "2026-08-31T22:00:00.000Z".into();
        candidate.date = "2026-08-31".into();

        assert_eq!(candidate.slot(0), DaySlot::DayOfMonth(1));
    }

    /// A row that prints nothing readable still has the attributes to fall
    /// back on, in order.
    #[test]
    fn date_attributes_are_used_when_no_day_is_printed() {
        let mut candidate = row();
        candidate.label = "unknown-day".into();
        assert_eq!(candidate.slot(11), september(12));

        candidate.day_iso = String::new();
        candidate.date = "2026-09-12".into();
        assert_eq!(candidate.slot(11), september(12));
    }

    /// A wall-clock reading carries no offset, so its own date is the day.
    #[test]
    fn a_date_with_a_local_time_after_it_still_parses() {
        let mut candidate = row();
        candidate.label = "unknown-day".into();
        for stamp in [
            "2026-09-12T00:00:00",
            "2026-09-12T09:30:00.500",
            "2026-09-12 00:00:00",
        ] {
            candidate.day_iso = stamp.into();
            assert_eq!(candidate.slot(11), september(12), "{stamp:?}");
        }
    }

    /// The bug that shifted the whole grid back by a day: Personio writes
    /// local midnight as a UTC instant, and in any zone ahead of UTC that
    /// instant's UTC date is the day before.
    #[test]
    fn a_timestamp_is_the_day_it_is_here_not_the_day_it_is_in_utc() {
        for (year, month, day) in [(2026, 9, 1), (2026, 1, 1), (2026, 6, 15)] {
            let expected = NaiveDate::from_ymd_opt(year, month, day).unwrap();
            let local_midnight = Local
                .from_local_datetime(&expected.and_hms_opt(0, 0, 0).unwrap())
                .single()
                .expect("midnight exists on these days");

            for stamp in [
                local_midnight.with_timezone(&Utc).to_rfc3339(),
                local_midnight.to_rfc3339(),
            ] {
                assert_eq!(
                    parse_date_attribute(&stamp),
                    Some(expected),
                    "{stamp:?} should be {expected}"
                );
            }
        }
    }

    /// The reported run, end to end: seven rows whose `<time>` elements carry
    /// local midnight as a UTC instant. Every one of them used to land on the
    /// day before, which put the 1st into the previous month.
    #[test]
    fn the_reported_september_run_places_every_row_on_its_own_day() {
        let mut candidate = row();

        for day in 1..=7u32 {
            let expected = NaiveDate::from_ymd_opt(2026, 9, day).unwrap();
            let local_midnight = Local
                .from_local_datetime(&expected.and_hms_opt(0, 0, 0).unwrap())
                .single()
                .unwrap();

            // Both of the things the page actually sent for that row.
            candidate.day_iso = local_midnight.with_timezone(&Utc).to_rfc3339();
            candidate.date = expected.pred_opt().unwrap().to_string();
            candidate.label = format!("{day} sept");

            assert_eq!(
                candidate.slot((day - 1) as usize),
                DaySlot::DayOfMonth(day),
                "row for {day} sept"
            );
        }
    }

    /// And the same rows, once the recorder has placed them: the run reported
    /// days 1 to 7 of September, so that is where they have to land.
    #[test]
    fn a_day_number_lands_in_the_month_being_walked() {
        use crate::calendar::{Calendar, DayState, MonthKey, Recorder};

        let september = MonthKey {
            year: 2026,
            month: 9,
        };
        let mut recorder = Recorder::opening_on(Calendar::default(), september);
        recorder.apply(&TrackingEvent::MonthTracking { row_count: 30 });

        for day in 1..=7 {
            recorder.apply(&TrackingEvent::DayAlreadyRegistered {
                slot: DaySlot::DayOfMonth(day),
                label: format!("{day} sept"),
            });
        }

        assert_eq!(recorder.unplaced(), 0);
        for day in 1..=7 {
            assert_eq!(
                recorder.calendar().state(september, day),
                Some(DayState::AlreadyRegistered),
                "day {day}"
            );
        }
    }

    #[test]
    fn an_attribute_that_is_not_a_date_parses_as_nothing() {
        for raw in ["", "   ", "not-a-date", "2026", "2026-13-01", "12 sept."] {
            assert_eq!(parse_date_attribute(raw), None, "{raw:?}");
        }
    }

    /// The printed label is localised: a weekday or a month name around the
    /// number must not stop the row being placed.
    #[test]
    fn a_day_number_is_read_out_of_whatever_the_label_prints() {
        let mut candidate = row();
        candidate.day_iso = String::new();
        candidate.date = String::new();

        for label in ["12", " 12 ", "lun 12", "12 sept.", "Mon 12", "12/09/2026"] {
            candidate.label = label.into();
            assert_eq!(candidate.slot(11), DaySlot::DayOfMonth(12), "{label:?}");
        }
    }

    /// Falling back to the row's position, which the recorder only trusts
    /// after checking the month is listed one row per day.
    #[test]
    fn a_row_with_nothing_readable_falls_back_to_its_position() {
        let mut candidate = row();
        candidate.day_iso = String::new();

        for label in ["unknown-day", "", "-", "no digits here"] {
            candidate.label = label.into();
            assert_eq!(candidate.slot(11), DaySlot::Row(11), "{label:?}");
        }
    }

    /// `2026-09-12` must not scan as the 9th.
    #[test]
    fn a_label_that_is_itself_a_date_is_read_as_one() {
        let mut candidate = row();
        candidate.day_iso = String::new();
        candidate.date = String::new();
        candidate.label = "2026-09-12".into();

        assert_eq!(candidate.slot(11), september(12));
    }

    #[test]
    fn day_numbers_outside_a_month_are_not_day_numbers() {
        assert_eq!(day_of_month("0"), None);
        assert_eq!(day_of_month("32"), None);
        assert_eq!(day_of_month("2026"), None);
        // The year is skipped over, the day is found.
        assert_eq!(day_of_month("2026 12"), Some(12));
    }

    #[test]
    fn the_skip_kind_follows_the_same_order_as_trackable() {
        let mut candidate = row();
        candidate.off_day = true;
        assert_eq!(candidate.skip_kind(), SkipKind::OffDay);

        candidate.holiday = true;
        assert_eq!(candidate.skip_kind(), SkipKind::Holiday);

        candidate.weekend = true;
        assert_eq!(candidate.skip_kind(), SkipKind::Weekend);
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
            "day_iso": "2026-09-03",
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
        // The printed day, not the attribute beside it.
        assert_eq!(parsed.slot(2), DaySlot::DayOfMonth(3));
    }

    /// A row without either date attribute still has to parse, or one
    /// Personio change would abort every run instead of costing the grid
    /// some colour.
    #[test]
    fn a_payload_with_no_date_still_deserialises() {
        let raw = r#"{
            "label": "3",
            "off_day": false,
            "weekend": false,
            "holiday": false,
            "today": false,
            "registered": false,
            "holiday_name": ""
        }"#;

        let parsed: DayRow = serde_json::from_str(raw).unwrap();
        assert!(parsed.trackable());
        assert_eq!(parsed.slot(2), DaySlot::DayOfMonth(3));
    }

    /// Not a snapshot of the whole script — just that every piece the Rust
    /// side deserialises is actually emitted, and quoted.
    #[test]
    fn the_row_expression_asks_for_every_field_it_deserialises() {
        let expression = row_expression(4);

        assert!(expression.contains("rows[4]"), "{expression}");
        for field in [
            "label:",
            "day_iso,",
            "date,",
            "off_day:",
            "weekend:",
            "holiday:",
            "today:",
            "registered:",
            "holiday_name:",
        ] {
            assert!(expression.contains(field), "{field} missing: {expression}");
        }

        // The `<time>` element is read for its machine-readable date.
        assert!(
            expression.contains("getAttribute('datetime')"),
            "{expression}"
        );
        assert!(
            expression.contains(&js_string(selectors::DAY_LABEL)),
            "{expression}"
        );
    }

    /// Balanced braces and parens: an unbalanced script throws inside the
    /// page and takes every row read down with it.
    #[test]
    fn the_row_expression_is_balanced() {
        let expression = row_expression(0);
        for (open, close) in [('{', '}'), ('(', ')')] {
            let depth = expression.chars().fold(0i32, |depth, character| {
                if character == open {
                    depth + 1
                } else if character == close {
                    depth - 1
                } else {
                    depth
                }
            });
            assert_eq!(depth, 0, "unbalanced {open}{close}: {expression}");
        }
    }

    #[test]
    fn the_cookie_selector_is_xpath_not_playwright_pseudo_css() {
        assert!(!selectors::ACCEPT_COOKIES_XPATH.contains(":has-text"));
        assert!(selectors::ACCEPT_COOKIES_XPATH.starts_with("//button"));
    }
}
