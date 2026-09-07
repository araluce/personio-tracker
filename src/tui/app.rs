//! TUI state and input handling.

use std::time::{Duration, Instant};

use crossterm::event::{KeyCode, KeyEvent, KeyEventKind, KeyModifiers};
use tokio::task::JoinHandle;

use crate::calendar::{Calendar, MonthKey, Recorder};
use crate::config::Settings;
use crate::event::{Severity, Summary, TrackingEvent};

const TOAST_LIFETIME: Duration = Duration::from_secs(4);
const MAX_LOG_LINES: usize = 2000;

/// Which pane reacts to navigation keys.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Pane {
    Log,
    Config,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Mode {
    Normal,
    Editing,
    Help,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Status {
    Idle,
    Running,
    Finished,
    Failed(String),
}

/// The editable configuration rows, in display order.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Field {
    Email,
    Company,
    EmployeeId,
    FirstSlotStart,
    FirstSlotEnd,
    SecondSlotStart,
    SecondSlotEnd,
    ShowBrowser,
    Password,
}

impl Field {
    pub const ALL: [Self; 9] = [
        Self::Email,
        Self::Company,
        Self::EmployeeId,
        Self::FirstSlotStart,
        Self::FirstSlotEnd,
        Self::SecondSlotStart,
        Self::SecondSlotEnd,
        Self::ShowBrowser,
        Self::Password,
    ];

    pub fn label(self) -> &'static str {
        match self {
            Self::Email => "Email",
            Self::Company => "Company",
            Self::EmployeeId => "Employee ID",
            Self::FirstSlotStart => "Slot 1 start",
            Self::FirstSlotEnd => "Slot 1 end",
            Self::SecondSlotStart => "Slot 2 start",
            Self::SecondSlotEnd => "Slot 2 end",
            Self::ShowBrowser => "Show browser",
            Self::Password => "Password",
        }
    }

    /// Where the value comes from, for the fields whose source is not
    /// obvious from the label. Shown under the configuration pane while the
    /// field is selected.
    pub fn hint(self) -> Option<&'static str> {
        match self {
            Self::Company => {
                Some("The subdomain of your Personio URL: acme.app.personio.com → acme")
            }
            Self::EmployeeId => {
                Some("The number ending your Personio attendance URL: /attendance/employee/1234")
            }
            _ => None,
        }
    }

    /// A toggle and the write-only password field are not free-text.
    fn is_text(self) -> bool {
        !matches!(self, Self::ShowBrowser)
    }

    fn is_secret(self) -> bool {
        matches!(self, Self::Password)
    }
}

#[derive(Debug, Clone)]
pub struct LogLine {
    pub timestamp: String,
    pub text: String,
    pub severity: Severity,
}

#[derive(Debug, Clone)]
pub struct Toast {
    pub text: String,
    pub is_error: bool,
    shown_at: Instant,
}

pub struct App {
    pub settings: Settings,
    /// Last persisted copy, so the header can flag unsaved edits.
    saved_settings: Settings,
    pub status: Status,
    pub pane: Pane,
    pub mode: Mode,
    pub field_index: usize,
    pub edit_buffer: String,
    pub log: Vec<LogLine>,
    pub log_scroll: u16,
    pub log_follow: bool,
    pub summary: Option<Summary>,
    /// What is known about each day, from earlier runs and this one.
    recorder: Recorder,
    /// How many months back the grid is showing. Kept as a distance rather
    /// than a month so that leaving the app open past midnight on the 1st
    /// cannot strand the view on a month that is no longer the current one.
    month_offset: usize,
    pub toast: Option<Toast>,
    /// Advances once per tick; the only input the running animations take, so
    /// rendering stays a pure function of state.
    pub tick: u64,
    pub keychain_has_password: bool,
    pub should_quit: bool,
    /// Present while a run is in flight, so it can be aborted on force-quit.
    pub run_task: Option<JoinHandle<()>>,
}

impl App {
    /// `keychain_has_password` is passed in rather than read here: the
    /// platform keychain lookup can take seconds on macOS, and a constructor
    /// that blocks on I/O makes the whole UI untestable.
    pub fn new(settings: Settings, keychain_has_password: bool, calendar: Calendar) -> Self {
        Self {
            saved_settings: settings.clone(),
            recorder: Recorder::new(calendar),
            month_offset: 0,
            settings,
            status: Status::Idle,
            pane: Pane::Config,
            mode: Mode::Normal,
            field_index: 0,
            edit_buffer: String::new(),
            log: Vec::new(),
            log_scroll: 0,
            log_follow: true,
            summary: None,
            toast: None,
            tick: 0,
            keychain_has_password,
            should_quit: false,
            run_task: None,
        }
    }

    pub fn is_running(&self) -> bool {
        self.status == Status::Running
    }

    pub fn has_unsaved_changes(&self) -> bool {
        self.settings != self.saved_settings
    }

    pub fn selected_field(&self) -> Field {
        Field::ALL[self.field_index.min(Field::ALL.len() - 1)]
    }

    /// Value shown next to a field label. The password is never read back from
    /// the keychain — only whether one is stored.
    pub fn field_display(&self, field: Field) -> String {
        match field {
            Field::Email => self.settings.personio_email.clone(),
            Field::Company => self.settings.personio_company.clone(),
            Field::EmployeeId => self.settings.employee_id.clone(),
            Field::FirstSlotStart => self.settings.start_time_first_slot.clone(),
            Field::FirstSlotEnd => self.settings.end_time_first_slot.clone(),
            Field::SecondSlotStart => self.settings.start_time_second_slot.clone(),
            Field::SecondSlotEnd => self.settings.end_time_second_slot.clone(),
            Field::ShowBrowser => if self.settings.show_browser {
                "on"
            } else {
                "off"
            }
            .to_string(),
            Field::Password => if self.keychain_has_password {
                "•••••• saved"
            } else {
                "not set"
            }
            .to_string(),
        }
    }

    fn field_value(&self, field: Field) -> String {
        match field {
            Field::Password => String::new(),
            _ => self.field_display(field),
        }
    }

    fn set_field_value(&mut self, field: Field, value: String) {
        let trimmed = value.trim().to_string();
        match field {
            Field::Email => self.settings.personio_email = trimmed,
            Field::Company => self.settings.personio_company = trimmed,
            Field::EmployeeId => self.settings.employee_id = trimmed,
            Field::FirstSlotStart => self.settings.start_time_first_slot = trimmed,
            Field::FirstSlotEnd => self.settings.end_time_first_slot = trimmed,
            Field::SecondSlotStart => self.settings.start_time_second_slot = trimmed,
            Field::SecondSlotEnd => self.settings.end_time_second_slot = trimmed,
            Field::ShowBrowser | Field::Password => {}
        }
    }

    pub fn push_log(&mut self, text: String, severity: Severity) {
        self.log.push(LogLine {
            timestamp: chrono::Local::now().format("%H:%M:%S").to_string(),
            text,
            severity,
        });

        if self.log.len() > MAX_LOG_LINES {
            self.log.drain(..self.log.len() - MAX_LOG_LINES);
        }
    }

    pub fn toast(&mut self, text: impl Into<String>, is_error: bool) {
        self.toast = Some(Toast {
            text: text.into(),
            is_error,
            shown_at: Instant::now(),
        });
    }

    /// One beat of the UI clock: move the animations on and retire a stale
    /// toast. Wraps rather than saturates, so a long-lived session keeps
    /// animating instead of freezing on a maxed-out counter.
    pub fn on_tick(&mut self) {
        self.tick = self.tick.wrapping_add(1);
        self.expire_toast();
    }

    fn expire_toast(&mut self) {
        if self
            .toast
            .as_ref()
            .is_some_and(|toast| toast.shown_at.elapsed() > TOAST_LIFETIME)
        {
            self.toast = None;
        }
    }

    /// Every day the runs have resolved, for the calendar grid.
    pub fn calendar(&self) -> &Calendar {
        self.recorder.calendar()
    }

    /// Days the last run could not place on the grid.
    pub fn unplaced_days(&self) -> usize {
        self.recorder.unplaced()
    }

    /// The month the grid is showing.
    pub fn viewed_month(&self) -> MonthKey {
        let mut month = MonthKey::current();
        for _ in 0..self.month_offset {
            month = month.previous();
        }
        month
    }

    /// Snaps the grid back to the current month, which is where a run starts.
    pub fn show_current_month(&mut self) {
        self.month_offset = 0;
    }

    /// How far back paging may go: as far as the record reaches, and no
    /// further — there is nothing to look at in months nobody ever walked.
    fn oldest_offset(&self) -> usize {
        self.calendar()
            .oldest_month()
            .map_or(0, |oldest| MonthKey::current().months_since(oldest))
    }

    /// Folds a tracking event into the log, summary, status and calendar.
    pub fn apply_tracking_event(&mut self, event: TrackingEvent) {
        self.push_log(event.line(), event.severity());

        self.recorder.apply(&event);

        match event {
            TrackingEvent::SessionFinish(summary) => {
                self.summary = Some(*summary);
                self.status = Status::Finished;
                self.run_task = None;
            }
            TrackingEvent::SessionError(error) => {
                self.status = Status::Failed(error);
                self.run_task = None;
            }
            _ => {}
        }
    }

    pub fn clear_log(&mut self) {
        self.log.clear();
        self.log_scroll = 0;
        self.log_follow = true;
    }

    /// Translates a key press into an intent. Returns `Some` when the caller
    /// has work to do that needs the async runtime.
    pub fn handle_key(&mut self, key: KeyEvent) -> Option<Action> {
        if key.kind != KeyEventKind::Press {
            return None;
        }

        if key.modifiers.contains(KeyModifiers::CONTROL) && matches!(key.code, KeyCode::Char('c')) {
            return Some(Action::ForceQuit);
        }

        match self.mode {
            Mode::Editing => self.handle_editing_key(key),
            Mode::Help => {
                self.mode = Mode::Normal;
                None
            }
            Mode::Normal => self.handle_normal_key(key),
        }
    }

    fn handle_editing_key(&mut self, key: KeyEvent) -> Option<Action> {
        match key.code {
            KeyCode::Esc => {
                self.mode = Mode::Normal;
                self.edit_buffer.clear();
                None
            }
            KeyCode::Enter => {
                self.mode = Mode::Normal;
                let field = self.selected_field();
                let value = std::mem::take(&mut self.edit_buffer);

                if field.is_secret() {
                    return Some(Action::SavePassword(value));
                }

                self.set_field_value(field, value);
                None
            }
            KeyCode::Backspace => {
                self.edit_buffer.pop();
                None
            }
            KeyCode::Char(c) => {
                self.edit_buffer.push(c);
                None
            }
            _ => None,
        }
    }

    fn handle_normal_key(&mut self, key: KeyEvent) -> Option<Action> {
        match key.code {
            KeyCode::Char('Q') => Some(Action::ForceQuit),
            KeyCode::Char('q') | KeyCode::Esc => {
                if self.is_running() {
                    self.toast("Run in progress — press Q to force quit", true);
                    None
                } else {
                    self.should_quit = true;
                    None
                }
            }
            KeyCode::Char('?') => {
                self.mode = Mode::Help;
                None
            }
            KeyCode::Tab | KeyCode::BackTab => {
                self.pane = match self.pane {
                    Pane::Log => Pane::Config,
                    Pane::Config => Pane::Log,
                };
                None
            }
            KeyCode::Char('t') => {
                if self.is_running() {
                    self.toast("A run is already in progress", true);
                    None
                } else {
                    Some(Action::StartTracking)
                }
            }
            KeyCode::Char('w') => Some(Action::SaveSettings),
            KeyCode::Char('c') => {
                self.clear_log();
                None
            }
            KeyCode::Char('j') | KeyCode::Down => {
                self.move_down();
                None
            }
            KeyCode::Char('k') | KeyCode::Up => {
                self.move_up();
                None
            }
            KeyCode::Char('h') | KeyCode::Left => {
                self.month_offset = (self.month_offset + 1).min(self.oldest_offset());
                None
            }
            KeyCode::Char('l') | KeyCode::Right => {
                self.month_offset = self.month_offset.saturating_sub(1);
                None
            }
            KeyCode::Char('g') => {
                if self.pane == Pane::Log {
                    self.log_scroll = 0;
                    self.log_follow = false;
                } else {
                    self.field_index = 0;
                }
                None
            }
            KeyCode::Char('G') => {
                if self.pane == Pane::Log {
                    self.log_follow = true;
                } else {
                    self.field_index = Field::ALL.len() - 1;
                }
                None
            }
            KeyCode::Enter | KeyCode::Char(' ') => self.activate_field(),
            _ => None,
        }
    }

    fn activate_field(&mut self) -> Option<Action> {
        if self.pane != Pane::Config {
            return None;
        }

        if self.is_running() {
            self.toast("Settings are locked while a run is in progress", true);
            return None;
        }

        let field = self.selected_field();

        if field == Field::ShowBrowser {
            self.settings.show_browser = !self.settings.show_browser;
            return None;
        }

        if field.is_text() {
            self.mode = Mode::Editing;
            self.edit_buffer = self.field_value(field);
        }

        None
    }

    fn move_down(&mut self) {
        match self.pane {
            Pane::Log => {
                self.log_follow = false;
                self.log_scroll = self.log_scroll.saturating_add(1);
            }
            Pane::Config => {
                self.field_index = (self.field_index + 1) % Field::ALL.len();
            }
        }
    }

    fn move_up(&mut self) {
        match self.pane {
            Pane::Log => {
                self.log_follow = false;
                self.log_scroll = self.log_scroll.saturating_sub(1);
            }
            Pane::Config => {
                self.field_index = if self.field_index == 0 {
                    Field::ALL.len() - 1
                } else {
                    self.field_index - 1
                };
            }
        }
    }

    pub fn mark_settings_saved(&mut self) {
        self.saved_settings = self.settings.clone();
    }
}

/// Work the event loop performs on the app's behalf.
#[derive(Debug, Clone)]
pub enum Action {
    StartTracking,
    SaveSettings,
    SavePassword(String),
    ForceQuit,
}

/// Whether the edit line should be masked.
pub fn edit_is_secret(field: Field) -> bool {
    field.is_secret()
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::calendar::DayState;

    fn key(code: KeyCode) -> KeyEvent {
        KeyEvent::new(code, KeyModifiers::NONE)
    }

    fn app() -> App {
        App::new(Settings::default(), false, Calendar::default())
    }

    #[test]
    fn tab_cycles_between_panes() {
        let mut app = app();
        assert_eq!(app.pane, Pane::Config);

        app.handle_key(key(KeyCode::Tab));
        assert_eq!(app.pane, Pane::Log);

        app.handle_key(key(KeyCode::Tab));
        assert_eq!(app.pane, Pane::Config);
    }

    #[test]
    fn field_navigation_wraps_in_both_directions() {
        let mut app = app();

        app.handle_key(key(KeyCode::Char('k')));
        assert_eq!(app.selected_field(), Field::Password);

        app.handle_key(key(KeyCode::Char('j')));
        assert_eq!(app.selected_field(), Field::Email);
    }

    #[test]
    fn editing_a_field_commits_on_enter_and_reverts_on_escape() {
        let mut app = app();
        app.handle_key(key(KeyCode::Enter));
        assert_eq!(app.mode, Mode::Editing);

        for c in "me@acme.com".chars() {
            app.handle_key(key(KeyCode::Char(c)));
        }
        app.handle_key(key(KeyCode::Enter));

        assert_eq!(app.mode, Mode::Normal);
        assert_eq!(app.settings.personio_email, "me@acme.com");

        app.handle_key(key(KeyCode::Enter));
        app.handle_key(key(KeyCode::Backspace));
        app.handle_key(key(KeyCode::Esc));

        assert_eq!(app.settings.personio_email, "me@acme.com");
    }

    #[test]
    fn space_toggles_the_show_browser_flag() {
        let mut app = app();
        app.field_index = Field::ALL
            .iter()
            .position(|field| *field == Field::ShowBrowser)
            .unwrap();

        assert!(app.settings.show_browser);
        app.handle_key(key(KeyCode::Char(' ')));
        assert!(!app.settings.show_browser);
    }

    #[test]
    fn the_password_field_never_prefills_the_edit_buffer() {
        let mut app = app();
        app.keychain_has_password = true;
        app.field_index = Field::ALL
            .iter()
            .position(|field| *field == Field::Password)
            .unwrap();

        app.handle_key(key(KeyCode::Enter));

        assert_eq!(app.mode, Mode::Editing);
        assert!(app.edit_buffer.is_empty());
    }

    #[test]
    fn committing_the_password_asks_the_loop_to_store_it() {
        let mut app = app();
        app.field_index = Field::ALL
            .iter()
            .position(|field| *field == Field::Password)
            .unwrap();
        app.handle_key(key(KeyCode::Enter));
        app.handle_key(key(KeyCode::Char('x')));

        let action = app.handle_key(key(KeyCode::Enter));

        assert!(matches!(action, Some(Action::SavePassword(value)) if value == "x"));
    }

    #[test]
    fn quitting_is_refused_mid_run_but_force_quit_is_not() {
        let mut app = app();
        app.status = Status::Running;

        app.handle_key(key(KeyCode::Char('q')));
        assert!(!app.should_quit);
        assert!(app.toast.is_some());

        let action = app.handle_key(key(KeyCode::Char('Q')));
        assert!(matches!(action, Some(Action::ForceQuit)));
    }

    #[test]
    fn a_second_run_is_refused_while_one_is_in_flight() {
        let mut app = app();
        app.status = Status::Running;

        assert!(app.handle_key(key(KeyCode::Char('t'))).is_none());
    }

    #[test]
    fn settings_are_locked_while_running() {
        let mut app = app();
        app.status = Status::Running;

        app.handle_key(key(KeyCode::Enter));

        assert_eq!(app.mode, Mode::Normal);
        assert!(app.toast.is_some());
    }

    #[test]
    fn unsaved_changes_are_detected_and_cleared_on_save() {
        let mut app = app();
        assert!(!app.has_unsaved_changes());

        app.settings.employee_id = "42".into();
        assert!(app.has_unsaved_changes());

        app.mark_settings_saved();
        assert!(!app.has_unsaved_changes());
    }

    #[test]
    fn finishing_a_run_stores_the_summary_and_clears_running() {
        let mut app = app();
        app.status = Status::Running;

        let summary = Summary {
            tracked_days: vec!["3".into()],
            months_visited: 1,
            ..Summary::default()
        };
        app.apply_tracking_event(TrackingEvent::SessionFinish(Box::new(summary)));

        assert_eq!(app.status, Status::Finished);
        assert_eq!(app.summary.as_ref().unwrap().tracked_days.len(), 1);
    }

    #[test]
    fn a_session_error_surfaces_as_a_failed_status() {
        let mut app = app();
        app.status = Status::Running;

        app.apply_tracking_event(TrackingEvent::SessionError("boom".into()));

        assert_eq!(app.status, Status::Failed("boom".into()));
    }

    #[test]
    fn the_log_is_capped() {
        let mut app = app();
        for i in 0..MAX_LOG_LINES + 50 {
            app.push_log(format!("line {i}"), Severity::Info);
        }

        assert_eq!(app.log.len(), MAX_LOG_LINES);
        assert_eq!(
            app.log.last().unwrap().text,
            format!("line {}", MAX_LOG_LINES + 49)
        );
    }

    #[test]
    fn scrolling_the_log_stops_following_and_g_resumes_it() {
        let mut app = app();
        app.pane = Pane::Log;

        app.handle_key(key(KeyCode::Char('k')));
        assert!(!app.log_follow);

        app.handle_key(key(KeyCode::Char('G')));
        assert!(app.log_follow);
    }

    /// A record reaching back two months is two pages of paging.
    fn app_with_a_record() -> App {
        let current = MonthKey::current();
        let mut calendar = Calendar::default();
        calendar.set(current, 1, DayState::Tracked);
        calendar.set(current.previous().previous(), 1, DayState::Tracked);

        App::new(Settings::default(), false, calendar)
    }

    #[test]
    fn the_grid_opens_on_the_current_month() {
        assert_eq!(app().viewed_month(), MonthKey::current());
    }

    #[test]
    fn h_and_l_page_the_month_grid() {
        let mut app = app_with_a_record();
        let current = MonthKey::current();

        app.handle_key(key(KeyCode::Char('h')));
        assert_eq!(app.viewed_month(), current.previous());

        app.handle_key(key(KeyCode::Char('h')));
        assert_eq!(app.viewed_month(), current.previous().previous());

        app.handle_key(key(KeyCode::Char('l')));
        assert_eq!(app.viewed_month(), current.previous());
    }

    /// There is nothing to see past the record, in either direction: the
    /// future has not happened and older months were never walked.
    #[test]
    fn paging_stops_at_both_ends_of_the_record() {
        let mut app = app_with_a_record();
        let current = MonthKey::current();

        for _ in 0..5 {
            app.handle_key(key(KeyCode::Char('h')));
        }
        assert_eq!(app.viewed_month(), current.previous().previous());

        for _ in 0..5 {
            app.handle_key(key(KeyCode::Char('l')));
        }
        assert_eq!(app.viewed_month(), current);
    }

    #[test]
    fn with_no_record_there_is_nowhere_to_page_to() {
        let mut app = app();
        app.handle_key(key(KeyCode::Char('h')));

        assert_eq!(app.viewed_month(), MonthKey::current());
    }

    #[test]
    fn a_run_snaps_the_grid_back_to_the_current_month() {
        let mut app = app_with_a_record();
        app.handle_key(key(KeyCode::Char('h')));
        app.show_current_month();

        assert_eq!(app.viewed_month(), MonthKey::current());
    }

    /// `h` is a letter first: paging must not eat what is being typed.
    #[test]
    fn h_and_l_type_themselves_while_a_field_is_being_edited() {
        let mut app = app_with_a_record();
        app.mode = Mode::Editing;
        app.edit_buffer.clear();

        app.handle_key(key(KeyCode::Char('h')));
        app.handle_key(key(KeyCode::Char('l')));

        assert_eq!(app.edit_buffer, "hl");
        assert_eq!(app.viewed_month(), MonthKey::current());
    }
}
