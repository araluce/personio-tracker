//! Personio selectors, kept together because they are the part most
//! likely to break when Personio ships a redesign.
//!
//! They stay inside `tracking` rather than moving to the crate root: what
//! breaks when one of these stops matching is the walk next door.
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
