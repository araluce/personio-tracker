//! Tracking events and run summary.
//!
//! Mirrors the event vocabulary emitted by the original Node implementation so
//! both front-ends (TUI and plain CLI) can render the same run narrative.

use chrono::NaiveDate;
use serde::{Deserialize, Serialize};
use tokio::sync::mpsc::UnboundedSender;

/// Aggregated outcome of a tracking run.
#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct Summary {
    pub tracked_days: Vec<String>,
    pub skipped_days: Vec<String>,
    pub already_registered_days: Vec<String>,
    pub errors: Vec<String>,
    pub months_visited: usize,
}

impl Summary {
    pub fn headline(&self) -> String {
        format!(
            "Tracked {} / Skipped {} / Already registered {} / Errors {} / Months {}",
            self.tracked_days.len(),
            self.skipped_days.len(),
            self.already_registered_days.len(),
            self.errors.len(),
            self.months_visited,
        )
    }
}

/// Which calendar day a timesheet row belongs to, and how sure the tracker is.
///
/// The variants are kept apart rather than collapsed into one day number
/// because they are not equally trustworthy, and a day painted onto the wrong
/// date is worse than a day left unpainted.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum DaySlot {
    /// A machine-readable date off the row itself, so month and day are
    /// certain.
    Dated(NaiveDate),
    /// The day number the row prints; the month comes from the walk.
    DayOfMonth(u32),
    /// Only the row's position in the timesheet. Placeable only if the month
    /// turns out to be listed one row per day, which the recorder checks.
    Row(usize),
}

/// Why Personio refuses a day, taken from its own row flags rather than from
/// the wording it shows — which is localised, and would not survive a
/// different UI language.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum SkipKind {
    Weekend,
    Holiday,
    /// An absence of any kind: vacation, sick leave, unpaid leave.
    OffDay,
}

/// A single step of progress during a tracking run.
#[derive(Debug, Clone)]
pub enum TrackingEvent {
    SessionStart {
        show_browser: bool,
    },
    Navigation(String),
    CookiesAccepting,
    AuthSessionValid,
    AuthLoginStart,
    AuthSessionSaved,
    AuthSessionRestored,
    MonthTracking {
        row_count: usize,
    },
    MonthPrevious,
    MonthPreviousMissing,
    MonthReachedToday {
        index: usize,
    },
    DayTracked {
        slot: DaySlot,
        label: String,
    },
    DaySkipped {
        slot: DaySlot,
        label: String,
        kind: SkipKind,
    },
    DayAlreadyRegistered {
        slot: DaySlot,
        label: String,
    },
    HoursPendingCheck {
        confirmed: f64,
        target: f64,
        pending: bool,
    },
    HoursPendingCheckMissing(String),
    SessionFinish(Box<Summary>),
    SessionError(String),
}

impl TrackingEvent {
    /// Single-line rendering used by both the TUI log pane and the CLI output.
    pub fn line(&self) -> String {
        match self {
            Self::SessionStart { show_browser } => {
                format!("Starting tracking session (show browser: {show_browser})")
            }
            Self::Navigation(message) => message.clone(),
            Self::CookiesAccepting => "Accepting cookies".to_string(),
            Self::AuthSessionValid => "Valid session".to_string(),
            Self::AuthLoginStart => "Logging in".to_string(),
            Self::AuthSessionSaved => "Session saved".to_string(),
            Self::AuthSessionRestored => "Restored saved session".to_string(),
            Self::MonthTracking { row_count } => format!("Tracking month ({row_count} rows)"),
            Self::MonthPrevious => "Going to previous month".to_string(),
            Self::MonthPreviousMissing => "No previous month found".to_string(),
            Self::MonthReachedToday { index } => format!("Reached today's row at index {index}"),
            Self::DayTracked { label, .. } => format!("{label}: shift registered"),
            Self::DaySkipped { label, .. } => format!("{label}: not trackable"),
            Self::DayAlreadyRegistered { label, .. } => {
                format!("{label}: already registered")
            }
            Self::HoursPendingCheck {
                confirmed,
                target,
                pending,
            } => format!("Hours confirmed={confirmed} target={target} pending={pending}"),
            Self::HoursPendingCheckMissing(message) => message.clone(),
            Self::SessionFinish(summary) => format!("Session finished — {}", summary.headline()),
            Self::SessionError(error) => format!("Session error: {error}"),
        }
    }

    /// Severity hint so front-ends can colour the line without re-matching.
    pub fn severity(&self) -> Severity {
        match self {
            Self::SessionError(_) | Self::HoursPendingCheckMissing(_) => Severity::Error,
            Self::DayTracked { .. } | Self::SessionFinish(_) | Self::AuthSessionSaved => {
                Severity::Good
            }
            Self::DaySkipped { .. } | Self::DayAlreadyRegistered { .. } => Severity::Muted,
            _ => Severity::Info,
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Severity {
    Info,
    Good,
    Muted,
    Error,
}

/// Channel the tracking engine pushes progress into.
pub type EventSink = UnboundedSender<TrackingEvent>;

/// Emits an event, ignoring the error when the receiver is already gone.
pub fn emit(sink: &EventSink, event: TrackingEvent) {
    let _ = sink.send(event);
}
