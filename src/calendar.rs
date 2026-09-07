//! What became of each day, kept between runs.
//!
//! The timesheet lives at Personio; this is only a local record of what the
//! last runs saw, so the grid can be drawn the moment the app opens instead of
//! after a browser round-trip. It is a cache, not a source of truth: anything
//! missing or stale is simply repainted by the next run.

use std::collections::BTreeMap;
use std::fmt;
use std::fs;
use std::path::{Path, PathBuf};

use anyhow::{Context, Result};
use chrono::{Datelike, Local, NaiveDate};
use serde::{Deserialize, Serialize};

use crate::config::calendar_path;
use crate::event::{DaySlot, SkipKind, TrackingEvent};

/// What became of one day, as Personio's own row flags report it.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "kebab-case")]
pub enum DayState {
    /// A run filled this day in.
    Tracked,
    /// It already had hours when a run reached it.
    AlreadyRegistered,
    /// An absence: holiday, sick leave, anything Personio marks as off.
    OffDay,
    /// A public holiday.
    Holiday,
    Weekend,
}

impl DayState {
    fn of_skip(kind: SkipKind) -> Self {
        match kind {
            SkipKind::Weekend => Self::Weekend,
            SkipKind::Holiday => Self::Holiday,
            SkipKind::OffDay => Self::OffDay,
        }
    }
}

/// A month, as it is keyed in the file: `2026-09`.
///
/// Ordered by year and then month, which is also the order the zero-padded
/// keys sort in — so the file's first key is always its oldest month.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord)]
pub struct MonthKey {
    pub year: i32,
    pub month: u32,
}

impl MonthKey {
    pub fn current() -> Self {
        Self::of(Local::now().date_naive())
    }

    pub fn of(date: NaiveDate) -> Self {
        Self {
            year: date.year(),
            month: date.month(),
        }
    }

    /// The month before this one, rolling the year over at January.
    pub fn previous(self) -> Self {
        if self.month == 1 {
            Self {
                year: self.year - 1,
                month: 12,
            }
        } else {
            Self {
                year: self.year,
                month: self.month - 1,
            }
        }
    }

    fn next(self) -> Self {
        if self.month == 12 {
            Self {
                year: self.year + 1,
                month: 1,
            }
        } else {
            Self {
                year: self.year,
                month: self.month + 1,
            }
        }
    }

    /// Reads a key back. Returns `None` for anything that is not a month, so
    /// a hand-edited file cannot take the app down.
    pub fn parse(raw: &str) -> Option<Self> {
        let (year, month) = raw.split_once('-')?;
        let year = year.parse().ok()?;
        let month = month.parse().ok()?;

        (1..=12).contains(&month).then_some(Self { year, month })
    }

    /// Whole months from `earlier` up to this one, and zero when `earlier` is
    /// not actually earlier.
    pub fn months_since(self, earlier: Self) -> usize {
        let months = (self.year - earlier.year) * 12 + self.month as i32 - earlier.month as i32;
        months.max(0) as usize
    }

    pub fn day(self, day: u32) -> Option<NaiveDate> {
        NaiveDate::from_ymd_opt(self.year, self.month, day)
    }

    /// How many days the month has, leap years included.
    pub fn length(self) -> u32 {
        let next = self.next();
        NaiveDate::from_ymd_opt(next.year, next.month, 1)
            .and_then(|first| first.pred_opt())
            .map_or(31, |last| last.day())
    }

    /// Month and year spelled out, for a pane title.
    pub fn label(self) -> String {
        self.day(1)
            .map_or_else(|| self.to_string(), |date| date.format("%B %Y").to_string())
    }
}

impl fmt::Display for MonthKey {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(formatter, "{:04}-{:02}", self.year, self.month)
    }
}

/// Day outcomes by month, sorted so the file stays diffable.
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct Calendar {
    #[serde(default)]
    months: BTreeMap<String, BTreeMap<u32, DayState>>,
}

impl Calendar {
    /// Reads the record, falling back to an empty one. A missing file is the
    /// normal first-run case, and a corrupt one is not worth refusing to
    /// start over: the next run rewrites what it sees.
    pub fn load() -> Self {
        calendar_path()
            .ok()
            .and_then(|path| Self::read(&path))
            .unwrap_or_default()
    }

    fn read(path: &Path) -> Option<Self> {
        let raw = fs::read_to_string(path).ok()?;
        serde_json::from_str(&raw).ok()
    }

    pub fn save(&self) -> Result<PathBuf> {
        self.save_to(&calendar_path()?)
    }

    fn save_to(&self, path: &Path) -> Result<PathBuf> {
        if let Some(parent) = path.parent() {
            fs::create_dir_all(parent)
                .with_context(|| format!("creating data dir {}", parent.display()))?;
        }
        let body = serde_json::to_string_pretty(self)?;
        fs::write(path, body).with_context(|| format!("writing {}", path.display()))?;
        Ok(path.to_path_buf())
    }

    pub fn set(&mut self, month: MonthKey, day: u32, state: DayState) {
        self.months
            .entry(month.to_string())
            .or_default()
            .insert(day, state);
    }

    pub fn state(&self, month: MonthKey, day: u32) -> Option<DayState> {
        self.months.get(&month.to_string())?.get(&day).copied()
    }

    /// The furthest back the record reaches, for capping how far the grid can
    /// be paged.
    pub fn oldest_month(&self) -> Option<MonthKey> {
        self.months
            .iter()
            .filter(|(_, days)| !days.is_empty())
            .filter_map(|(key, _)| MonthKey::parse(key))
            .min()
    }

    /// Whether anything at all is known about a month, so the UI can say
    /// "nothing tracked yet" rather than draw an all-blank grid.
    pub fn has_month(&self, month: MonthKey) -> bool {
        self.months
            .get(&month.to_string())
            .is_some_and(|days| !days.is_empty())
    }
}

/// Folds a run's events into calendar state, whichever front-end is driving.
///
/// Both front-ends record: a `--cli` run from `cron` must leave the same trail
/// as pressing `t`, or the grid would go blank for anyone driving it from a
/// scheduler.
#[derive(Debug, Clone)]
pub struct Recorder {
    calendar: Calendar,
    /// The month a run's first timesheet shows.
    opens_on: MonthKey,
    /// The month the walk is currently in. Only a fallback: a row that came
    /// with its own date does not need it.
    cursor: Option<MonthKey>,
    /// How many rows that month listed, which is what makes a row's position
    /// usable as a day number.
    rows: Option<usize>,
    /// Days that could not be placed at all. Counted rather than ignored: a
    /// grid quietly missing half a month is indistinguishable from a broken
    /// one, so a run has to be able to say so.
    unplaced: usize,
}

impl Recorder {
    pub fn new(calendar: Calendar) -> Self {
        Self::opening_on(calendar, MonthKey::current())
    }

    /// `opens_on` is the month a run's first timesheet shows — today's, for a
    /// real run. Injectable so the placement rules can be exercised against a
    /// month whose length is known rather than whichever one it is today.
    pub fn opening_on(calendar: Calendar, opens_on: MonthKey) -> Self {
        Self {
            calendar,
            opens_on,
            cursor: None,
            rows: None,
            unplaced: 0,
        }
    }

    pub fn calendar(&self) -> &Calendar {
        &self.calendar
    }

    /// Days this recorder could not place on any date.
    pub fn unplaced(&self) -> usize {
        self.unplaced
    }

    pub fn apply(&mut self, event: &TrackingEvent) {
        match event {
            // A run opens on the current month and only ever walks backwards.
            TrackingEvent::MonthTracking { row_count } => {
                self.cursor = Some(self.cursor.unwrap_or(self.opens_on));
                self.rows = Some(*row_count);
            }
            TrackingEvent::MonthPrevious => {
                self.cursor = self.cursor.map(MonthKey::previous);
            }
            TrackingEvent::DayTracked { slot, .. } => self.record(*slot, DayState::Tracked),
            TrackingEvent::DayAlreadyRegistered { slot, .. } => {
                self.record(*slot, DayState::AlreadyRegistered);
            }
            TrackingEvent::DaySkipped { slot, kind, .. } => {
                self.record(*slot, DayState::of_skip(*kind));
            }
            _ => {}
        }
    }

    /// Places one day, from the best source the row could offer.
    ///
    /// Anything that cannot be placed is counted, never guessed at: one wrong
    /// offset would recolour a whole month, and a wrong grid is worse than an
    /// incomplete one.
    fn record(&mut self, slot: DaySlot, state: DayState) {
        let placed = match slot {
            DaySlot::Dated(date) => {
                self.calendar.set(MonthKey::of(date), date.day(), state);
                true
            }
            DaySlot::DayOfMonth(day) => self.place(day, state),
            // A row's position is only a day number if the month is listed
            // one row per day — which is checkable, so it is checked.
            DaySlot::Row(index) => {
                let day = u32::try_from(index + 1).unwrap_or(u32::MAX);
                let listed_day_for_row = self
                    .cursor
                    .zip(self.rows)
                    .is_some_and(|(month, rows)| rows as u64 == u64::from(month.length()));

                listed_day_for_row && self.place(day, state)
            }
        };

        if !placed {
            self.unplaced += 1;
        }
    }

    /// Puts a day number in the month being walked, if a month is being
    /// walked at all and the day exists in it.
    fn place(&mut self, day: u32, state: DayState) -> bool {
        let Some(month) = self
            .cursor
            .filter(|month| (1..=month.length()).contains(&day))
        else {
            return false;
        };

        self.calendar.set(month, day, state);
        true
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn date(year: i32, month: u32, day: u32) -> NaiveDate {
        NaiveDate::from_ymd_opt(year, month, day).unwrap()
    }

    fn september() -> MonthKey {
        MonthKey {
            year: 2026,
            month: 9,
        }
    }

    #[test]
    fn a_month_key_is_the_file_key() {
        assert_eq!(september().to_string(), "2026-09");
        assert_eq!(
            MonthKey {
                year: 2026,
                month: 12
            }
            .to_string(),
            "2026-12"
        );
    }

    #[test]
    fn stepping_back_rolls_the_year_over() {
        assert_eq!(
            september().previous(),
            MonthKey {
                year: 2026,
                month: 8
            }
        );
        assert_eq!(
            MonthKey {
                year: 2026,
                month: 1
            }
            .previous(),
            MonthKey {
                year: 2025,
                month: 12
            }
        );
    }

    #[test]
    fn month_lengths_account_for_leap_years() {
        assert_eq!(september().length(), 30);
        assert_eq!(
            MonthKey {
                year: 2026,
                month: 2
            }
            .length(),
            28
        );
        assert_eq!(
            MonthKey {
                year: 2024,
                month: 2
            }
            .length(),
            29
        );
        assert_eq!(
            MonthKey {
                year: 2026,
                month: 12
            }
            .length(),
            31
        );
    }

    #[test]
    fn a_dated_row_carries_its_own_month() {
        let mut recorder = Recorder::new(Calendar::default());
        recorder.apply(&TrackingEvent::DayTracked {
            slot: DaySlot::Dated(date(2026, 4, 30)),
            label: "30".into(),
        });

        let april = MonthKey {
            year: 2026,
            month: 4,
        };
        assert_eq!(
            recorder.calendar().state(april, 30),
            Some(DayState::Tracked)
        );
    }

    #[test]
    fn every_skip_reason_gets_its_own_state() {
        let mut recorder = Recorder::new(Calendar::default());
        for (day, kind, expected) in [
            (1, SkipKind::Weekend, DayState::Weekend),
            (2, SkipKind::Holiday, DayState::Holiday),
            (3, SkipKind::OffDay, DayState::OffDay),
        ] {
            recorder.apply(&TrackingEvent::DaySkipped {
                slot: DaySlot::Dated(date(2026, 9, day)),
                label: day.to_string(),
                kind,
            });
            assert_eq!(recorder.calendar().state(september(), day), Some(expected));
        }
    }

    /// Without a date on the row, the month has to come from the walk: the
    /// run opens on the current month and steps back from there.
    #[test]
    fn an_undated_row_lands_in_the_month_being_walked() {
        let mut recorder = Recorder::new(Calendar::default());
        let current = MonthKey::current();

        recorder.apply(&TrackingEvent::MonthTracking { row_count: 30 });
        recorder.apply(&TrackingEvent::DayTracked {
            slot: DaySlot::DayOfMonth(4),
            label: "4".into(),
        });
        recorder.apply(&TrackingEvent::MonthPrevious);
        recorder.apply(&TrackingEvent::MonthTracking { row_count: 31 });
        recorder.apply(&TrackingEvent::DayTracked {
            slot: DaySlot::DayOfMonth(9),
            label: "9".into(),
        });

        assert_eq!(
            recorder.calendar().state(current, 4),
            Some(DayState::Tracked)
        );
        assert_eq!(
            recorder.calendar().state(current.previous(), 9),
            Some(DayState::Tracked)
        );
    }

    /// The bug this whole chain exists for: a run whose rows offered no
    /// readable date and no readable day number left the grid completely
    /// empty, and said nothing about it.
    #[test]
    fn a_month_of_unreadable_rows_still_fills_the_grid() {
        let september = MonthKey {
            year: 2026,
            month: 9,
        };
        let mut recorder = Recorder::opening_on(Calendar::default(), september);
        recorder.apply(&TrackingEvent::MonthTracking { row_count: 30 });

        for index in 0..30 {
            recorder.apply(&TrackingEvent::DayAlreadyRegistered {
                slot: DaySlot::Row(index),
                label: "unknown-day".into(),
            });
        }

        assert_eq!(recorder.unplaced(), 0);
        for day in 1..=30 {
            assert_eq!(
                recorder.calendar().state(september, day),
                Some(DayState::AlreadyRegistered),
                "day {day}"
            );
        }
    }

    /// A row's position is a day number only once the month has been shown
    /// to list one row per day.
    #[test]
    fn a_positional_row_is_placed_when_the_month_is_listed_day_for_row() {
        let current = MonthKey::current();
        let mut recorder = Recorder::new(Calendar::default());
        recorder.apply(&TrackingEvent::MonthTracking {
            row_count: current.length() as usize,
        });
        recorder.apply(&TrackingEvent::DayTracked {
            slot: DaySlot::Row(3),
            label: "unknown-day".into(),
        });

        assert_eq!(
            recorder.calendar().state(current, 4),
            Some(DayState::Tracked)
        );
        assert_eq!(recorder.unplaced(), 0);
    }

    /// Painting the wrong days is worse than painting none: a partial listing
    /// says nothing about which day row 4 is.
    #[test]
    fn a_positional_row_is_dropped_when_the_listing_is_short() {
        let current = MonthKey::current();
        let mut recorder = Recorder::new(Calendar::default());
        recorder.apply(&TrackingEvent::MonthTracking { row_count: 5 });
        recorder.apply(&TrackingEvent::DayTracked {
            slot: DaySlot::Row(3),
            label: "unknown-day".into(),
        });

        assert!(!recorder.calendar().has_month(current));
        assert_eq!(recorder.unplaced(), 1);
    }

    /// The count is what turns "the grid looks wrong" into something a run
    /// can actually report.
    #[test]
    fn unplaced_days_are_counted_rather_than_ignored() {
        let mut recorder = Recorder::new(Calendar::default());
        recorder.apply(&TrackingEvent::MonthTracking { row_count: 5 });

        for index in 0..3 {
            recorder.apply(&TrackingEvent::DaySkipped {
                slot: DaySlot::Row(index),
                label: "unknown-day".into(),
                kind: SkipKind::Weekend,
            });
        }

        assert_eq!(recorder.unplaced(), 3);
    }

    #[test]
    fn a_day_number_the_month_does_not_have_is_not_placed() {
        let february = MonthKey {
            year: 2026,
            month: 2,
        };
        let mut recorder = Recorder::opening_on(Calendar::default(), february);
        recorder.apply(&TrackingEvent::MonthTracking { row_count: 28 });

        for day in [0, 29, 31] {
            recorder.apply(&TrackingEvent::DayTracked {
                slot: DaySlot::DayOfMonth(day),
                label: day.to_string(),
            });
            assert_eq!(recorder.calendar().state(february, day), None, "{day}");
        }

        assert_eq!(recorder.unplaced(), 3);
    }

    /// A day whose row arrives before any month does: there is nowhere to
    /// put it, and guessing the current month could be a month out.
    #[test]
    fn an_undated_row_outside_a_month_is_dropped() {
        let mut recorder = Recorder::new(Calendar::default());
        recorder.apply(&TrackingEvent::DayTracked {
            slot: DaySlot::DayOfMonth(4),
            label: "4".into(),
        });

        assert!(!recorder.calendar().has_month(MonthKey::current()));
        assert_eq!(recorder.unplaced(), 1);
    }

    #[test]
    fn a_later_run_overwrites_what_an_earlier_one_recorded() {
        let mut calendar = Calendar::default();
        calendar.set(september(), 4, DayState::OffDay);
        calendar.set(september(), 4, DayState::Tracked);

        assert_eq!(calendar.state(september(), 4), Some(DayState::Tracked));
    }

    #[test]
    fn an_unknown_day_has_no_state() {
        let calendar = Calendar::default();

        assert_eq!(calendar.state(september(), 4), None);
        assert!(!calendar.has_month(september()));
    }

    #[test]
    fn a_saved_calendar_reads_back_unchanged() {
        let directory = std::env::temp_dir().join(format!(
            "personio-calendar-{}-{}",
            std::process::id(),
            line!()
        ));
        let path = directory.join("calendar.json");

        let mut calendar = Calendar::default();
        calendar.set(september(), 4, DayState::Tracked);
        calendar.set(september(), 5, DayState::Weekend);
        calendar.set(september().previous(), 1, DayState::Holiday);
        calendar.save_to(&path).unwrap();

        assert_eq!(Calendar::read(&path), Some(calendar));
        let _ = fs::remove_dir_all(&directory);
    }

    #[test]
    fn a_corrupt_file_reads_as_no_record_rather_than_an_error() {
        let directory = std::env::temp_dir().join(format!(
            "personio-calendar-{}-{}",
            std::process::id(),
            line!()
        ));
        fs::create_dir_all(&directory).unwrap();
        let path = directory.join("calendar.json");
        fs::write(&path, "{ not json").unwrap();

        assert_eq!(Calendar::read(&path), None);
        assert_eq!(Calendar::read(&directory.join("missing.json")), None);
        let _ = fs::remove_dir_all(&directory);
    }

    #[test]
    fn day_states_are_stored_under_stable_names() {
        let mut calendar = Calendar::default();
        calendar.set(september(), 1, DayState::AlreadyRegistered);
        calendar.set(september(), 2, DayState::OffDay);

        let body = serde_json::to_string(&calendar).unwrap();
        assert!(body.contains(r#""2026-09""#), "{body}");
        assert!(body.contains(r#""already-registered""#), "{body}");
        assert!(body.contains(r#""off-day""#), "{body}");
    }

    #[test]
    fn a_month_key_reads_back_from_its_file_key() {
        assert_eq!(MonthKey::parse("2026-09"), Some(september()));
        assert_eq!(
            MonthKey::parse("2025-12"),
            Some(MonthKey {
                year: 2025,
                month: 12
            })
        );
    }

    /// A hand-edited file must not take the app down.
    #[test]
    fn junk_keys_do_not_parse_as_months() {
        for raw in [
            "",
            "2026",
            "2026-13",
            "2026-00",
            "2026-ab",
            "not-a-month",
            "-",
        ] {
            assert_eq!(MonthKey::parse(raw), None, "{raw:?}");
        }
    }

    #[test]
    fn the_distance_between_months_counts_whole_months() {
        assert_eq!(september().months_since(september()), 0);
        assert_eq!(september().months_since(september().previous()), 1);
        assert_eq!(
            september().months_since(MonthKey {
                year: 2025,
                month: 9
            }),
            12
        );
    }

    /// Paging must not run off into months that were never walked.
    #[test]
    fn a_month_in_the_future_is_no_distance_at_all() {
        assert_eq!(
            september().months_since(MonthKey {
                year: 2027,
                month: 1
            }),
            0
        );
    }

    #[test]
    fn the_oldest_recorded_month_is_how_far_back_the_record_reaches() {
        let mut calendar = Calendar::default();
        assert_eq!(calendar.oldest_month(), None);

        calendar.set(september(), 1, DayState::Tracked);
        calendar.set(september().previous().previous(), 1, DayState::Tracked);
        calendar.set(
            MonthKey {
                year: 2027,
                month: 3,
            },
            1,
            DayState::Tracked,
        );

        assert_eq!(
            calendar.oldest_month(),
            Some(september().previous().previous())
        );
    }
}
