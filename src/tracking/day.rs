//! One timesheet row: what Personio says about a day, and which day it is.
//!
//! Reading a row is a single `Runtime.evaluate` rather than six DOM
//! round-trips, and the page-side script lives in [`row_expression`] so it can
//! be tested: a syntax error there breaks every row read in a run, not one.

use anyhow::{Context, Result};
use chromiumoxide::page::Page;
use chrono::{DateTime, Local, NaiveDate, NaiveDateTime};
use serde::Deserialize;

use super::selectors;
use crate::browser::locator::{eval, js_string};
use crate::event::{DaySlot, SkipKind};

/// Everything the tracker needs to know about one timesheet row.
#[derive(Debug, Clone, Deserialize)]
pub(super) struct DayRow {
    pub(super) label: String,
    /// The `datetime` attribute of the row's `<time>` element — the one field
    /// HTML defines as a machine-readable date.
    #[serde(default)]
    pub(super) day_iso: String,
    /// Personio's `data-date`, when the row carries one.
    ///
    /// Both are defaulted rather than required: a page that stops emitting
    /// one should cost the grid some precision, never abort the run.
    #[serde(default)]
    pub(super) date: String,
    pub(super) off_day: bool,
    pub(super) weekend: bool,
    pub(super) holiday: bool,
    pub(super) today: bool,
    pub(super) registered: bool,
    pub(super) holiday_name: String,
}

impl DayRow {
    pub(super) fn trackable(&self) -> bool {
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
    pub(super) fn slot(&self, index: usize) -> DaySlot {
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
    pub(super) fn skip_kind(&self) -> SkipKind {
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
    pub(super) fn skip_reason(&self) -> &str {
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
pub(super) async fn read_row(page: &Page, index: usize) -> Result<Option<DayRow>> {
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

#[cfg(test)]
mod tests {
    use super::*;
    use crate::event::TrackingEvent;
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
}
