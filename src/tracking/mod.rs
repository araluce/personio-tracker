//! Personio attendance automation: the walk itself.
//!
//! A port of the original Node/Playwright implementation. Behaviour is kept
//! deliberately identical: walk the current month's timesheet top to bottom,
//! fill every trackable day that has no entry yet, stop once today's row has
//! been handled, then step back a month while hours are still missing.
//!
//! Notable differences forced by the move off Playwright are called out inline.
//!
//! The parts that are not the walk live next door, so that a reader after one
//! of them does not have to read the others:
//!
//! | module | holds |
//! |---|---|
//! | [`selectors`] | every Personio selector, the part a redesign breaks |
//! | [`auth`] | the cookie banner, the login form, waiting to be let in |
//! | [`day`] | one timesheet row: reading it, and working out its date |

mod auth;
mod day;
mod selectors;

use std::time::Duration;

use anyhow::{Context, Result};
use chromiumoxide::browser::Browser;
use chromiumoxide::page::Page;
use tokio::time::sleep;

use self::auth::{accept_cookies_if_present, login};
use self::day::{DayRow, read_row};
use crate::browser::locator::{DEFAULT_TIMEOUT, Locator, SHORT_TIMEOUT};
use crate::browser::{launch, session};
use crate::config::{RunConfig, parse_time};
use crate::event::{EventSink, Summary, TrackingEvent, emit};

/// How long the original waited after each interaction for Personio's SPA to
/// settle. Kept as named constants rather than bare numbers.
const SETTLE_AFTER_NAVIGATION: Duration = Duration::from_secs(2);
const SETTLE_AFTER_ROW_CLICK: Duration = Duration::from_secs(2);
const SETTLE_AFTER_SAVE: Duration = Duration::from_secs(2);
const SETTLE_AFTER_TIME_INPUT: Duration = Duration::from_secs(1);
const SETTLE_AFTER_COOKIES: Duration = Duration::from_secs(1);

/// How long a login may take, and how often it is checked. Personio's
/// two-step form redirects a few times before it settles.
pub(super) const LOGIN_TIMEOUT: Duration = Duration::from_secs(30);
pub(super) const LOGIN_POLL_INTERVAL: Duration = Duration::from_millis(250);

/// The month-hours widget renders late; not finding it is not an error.
const HOURS_WIDGET_TIMEOUT: Duration = Duration::from_secs(10);

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

    if widget.wait_in_view(HOURS_WIDGET_TIMEOUT).await.is_err() {
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
}
