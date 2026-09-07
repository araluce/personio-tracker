//! Tracking events and run summary.
//!
//! Mirrors the event vocabulary emitted by the original Node implementation so
//! both front-ends (TUI and plain CLI) can render the same run narrative.

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
    DayTracked(String),
    DaySkipped(String),
    DayAlreadyRegistered(String),
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
            Self::DayTracked(day) => format!("{day}: shift registered"),
            Self::DaySkipped(day) => format!("{day}: not trackable"),
            Self::DayAlreadyRegistered(day) => format!("{day}: already registered"),
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
            Self::DayTracked(_) | Self::SessionFinish(_) | Self::AuthSessionSaved => Severity::Good,
            Self::DaySkipped(_) | Self::DayAlreadyRegistered(_) => Severity::Muted,
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
