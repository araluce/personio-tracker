//! Persisted settings and the resolved configuration a tracking run needs.
//!
//! The on-disk format is intentionally identical to the Electron app's
//! `settings.json` (same camelCase keys), so an existing file can be copied
//! over without conversion. Unknown keys such as `launchAtLogin` are ignored.

use std::fmt;
use std::fs;
use std::path::PathBuf;
use std::sync::Arc;

use anyhow::{Context, Result, bail};
use directories::ProjectDirs;
use serde::{Deserialize, Serialize};

const QUALIFIER: &str = "com";
const ORGANIZATION: &str = "personiotrack";
const APPLICATION: &str = "personio-tracker-tui";
const SETTINGS_FILE: &str = "settings.json";
const SESSION_FILE: &str = "session.json";
const CALENDAR_FILE: &str = "calendar.json";

/// As the on-disk (and Electron) format spells it.
const SHOW_BROWSER_KEY: &str = "showBrowser";

const DEFAULT_START_FIRST: &str = "09:00";
const DEFAULT_END_FIRST: &str = "14:00";
const DEFAULT_START_SECOND: &str = "15:00";
const DEFAULT_END_SECOND: &str = "18:00";

/// User-editable settings, persisted as JSON.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(default, rename_all = "camelCase")]
pub struct Settings {
    pub personio_email: String,
    pub personio_company: String,
    pub employee_id: String,
    pub show_browser: bool,
    pub start_time_first_slot: String,
    pub end_time_first_slot: String,
    pub start_time_second_slot: String,
    pub end_time_second_slot: String,
}

impl Default for Settings {
    fn default() -> Self {
        Self {
            personio_email: String::new(),
            personio_company: String::new(),
            employee_id: String::new(),
            show_browser: true,
            start_time_first_slot: DEFAULT_START_FIRST.to_string(),
            end_time_first_slot: DEFAULT_END_FIRST.to_string(),
            start_time_second_slot: DEFAULT_START_SECOND.to_string(),
            end_time_second_slot: DEFAULT_END_SECOND.to_string(),
        }
    }
}

impl Settings {
    /// Reads settings from disk, then fills every blank field from the
    /// environment. Values already present in the file win, so editing in the
    /// TUI always takes precedence over a stale `.env`.
    pub fn load() -> Self {
        let stored = Self::read_file();
        let from_file = stored.as_ref().and_then(|stored| stored.show_browser);

        let mut settings = stored.map_or_else(Self::default, |stored| stored.settings);
        settings.apply_env_fallbacks();
        settings.show_browser = resolve_show_browser(from_file, std::env::var("SHOW_BROWSER").ok());
        settings
    }

    fn read_file() -> Option<Stored> {
        let path = settings_path().ok()?;
        let raw = fs::read_to_string(path).ok()?;

        Some(Stored {
            settings: serde_json::from_str(&raw).ok()?,
            show_browser: show_browser_in(&raw),
        })
    }

    fn apply_env_fallbacks(&mut self) {
        fill_blank(&mut self.personio_email, "PERSONIO_EMAIL");
        fill_blank(&mut self.personio_company, "PERSONIO_COMPANY");
        fill_blank(&mut self.employee_id, "EMPLOYEE_ID");
        fill_blank(&mut self.start_time_first_slot, "START_TIME_FIRST_SLOT");
        fill_blank(&mut self.end_time_first_slot, "END_TIME_FIRST_SLOT");
        fill_blank(&mut self.start_time_second_slot, "START_TIME_SECOND_SLOT");
        fill_blank(&mut self.end_time_second_slot, "END_TIME_SECOND_SLOT");
    }

    pub fn save(&self) -> Result<PathBuf> {
        let path = settings_path()?;
        if let Some(parent) = path.parent() {
            fs::create_dir_all(parent)
                .with_context(|| format!("creating config dir {}", parent.display()))?;
        }
        let body = serde_json::to_string_pretty(self)?;
        fs::write(&path, body).with_context(|| format!("writing {}", path.display()))?;
        Ok(path)
    }

    /// Turns settings plus a password source into a runnable configuration.
    ///
    /// `show_browser` is the `--show` / `--headless` override, which belongs to
    /// the run and not to the settings: `None` uses what is stored.
    pub fn to_run_config(
        &self,
        password: PasswordProvider,
        show_browser: Option<bool>,
    ) -> Result<RunConfig> {
        let config = RunConfig {
            personio_email: self.personio_email.trim().to_string(),
            personio_password: password,
            personio_company: self.personio_company.trim().to_string(),
            employee_id: self.employee_id.trim().to_string(),
            show_browser: show_browser.unwrap_or(self.show_browser),
            session_file: session_path()?,
            start_time_first_slot: non_blank(&self.start_time_first_slot, DEFAULT_START_FIRST),
            end_time_first_slot: non_blank(&self.end_time_first_slot, DEFAULT_END_FIRST),
            start_time_second_slot: non_blank(&self.start_time_second_slot, DEFAULT_START_SECOND),
            end_time_second_slot: non_blank(&self.end_time_second_slot, DEFAULT_END_SECOND),
        };
        config.validate()?;
        Ok(config)
    }
}

/// Where a run gets its password, resolved only if it turns out to need one.
///
/// Resolving is what makes the OS put up its keychain dialog, and a run that
/// replays a saved session never logs in — so it must never ask. The provider
/// defers that decision to the one place that knows: the login step itself.
#[derive(Clone)]
pub struct PasswordProvider(Arc<dyn Fn() -> Option<String> + Send + Sync>);

impl PasswordProvider {
    pub fn new(resolve: impl Fn() -> Option<String> + Send + Sync + 'static) -> Self {
        Self(Arc::new(resolve))
    }

    /// Asks the source for the password. Blank is the same as absent: a
    /// half-filled `.env` should read as "no password", not as an empty one.
    pub fn resolve(&self) -> Option<String> {
        (self.0)().filter(|value| !value.trim().is_empty())
    }
}

impl fmt::Debug for PasswordProvider {
    /// Deliberately opaque: `RunConfig` derives `Debug`, and a config that
    /// ends up in a log line or an error chain must not carry the password.
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter.write_str("PasswordProvider")
    }
}

/// Everything a tracking run needs, already validated.
#[derive(Debug, Clone)]
pub struct RunConfig {
    pub personio_email: String,
    pub personio_password: PasswordProvider,
    pub personio_company: String,
    pub employee_id: String,
    pub show_browser: bool,
    pub session_file: PathBuf,
    pub start_time_first_slot: String,
    pub end_time_first_slot: String,
    pub start_time_second_slot: String,
    pub end_time_second_slot: String,
}

impl RunConfig {
    fn validate(&self) -> Result<()> {
        let mut missing = Vec::new();
        if self.personio_email.is_empty() {
            missing.push("email");
        }
        if self.personio_company.is_empty() {
            missing.push("company");
        }
        if self.employee_id.is_empty() {
            missing.push("employee id");
        }

        if !missing.is_empty() {
            bail!("Missing configuration: {}", missing.join(", "));
        }

        for (label, value) in [
            ("start first slot", &self.start_time_first_slot),
            ("end first slot", &self.end_time_first_slot),
            ("start second slot", &self.start_time_second_slot),
            ("end second slot", &self.end_time_second_slot),
        ] {
            parse_time(value).with_context(|| format!("Invalid {label} time: {value:?}"))?;
        }

        Ok(())
    }

    /// Personio origin without a trailing slash.
    pub fn base_url(&self) -> String {
        format!("https://{}.app.personio.com", self.personio_company)
    }

    /// Landing page URL, used to detect a completed login.
    pub fn home_url(&self) -> String {
        format!("{}/", self.base_url())
    }

    pub fn attendance_url(&self) -> String {
        format!(
            "{}/attendance/employee/{}",
            self.base_url(),
            self.employee_id
        )
    }
}

/// Splits `HH:MM` into its two components, rejecting anything out of range.
pub fn parse_time(value: &str) -> Result<(String, String)> {
    let (hours, minutes) = value
        .trim()
        .split_once(':')
        .with_context(|| format!("expected HH:MM, got {value:?}"))?;

    let parsed_hours: u32 = hours.trim().parse().context("hours are not a number")?;
    let parsed_minutes: u32 = minutes.trim().parse().context("minutes are not a number")?;

    if parsed_hours > 23 || parsed_minutes > 59 {
        bail!("time out of range: {value:?}");
    }

    Ok((format!("{parsed_hours:02}"), format!("{parsed_minutes:02}")))
}

/// A settings file, plus what it had to say about the one field that cannot
/// be blank.
struct Stored {
    settings: Settings,
    show_browser: Option<bool>,
}

/// What a settings file says about `showBrowser`, if it says anything.
///
/// Read from the raw JSON rather than from the deserialised struct because a
/// `bool` cannot express "absent": serde fills a missing key with the default
/// and the two become indistinguishable.
fn show_browser_in(raw: &str) -> Option<bool> {
    serde_json::from_str::<serde_json::Value>(raw)
        .ok()
        .and_then(|value| value.get(SHOW_BROWSER_KEY)?.as_bool())
}

/// Whether to show the browser, from what the file said and what the
/// environment says.
///
/// The file wins, like every other field: `fill_blank` leaves a stored value
/// alone and only fills a blank one. A `bool` has no blank to test, which is
/// how the environment came to overwrite this one on every start-up — so the
/// toggle in the UI could be saved and still never stick.
fn resolve_show_browser(from_file: Option<bool>, from_env: Option<String>) -> bool {
    if let Some(stored) = from_file {
        return stored;
    }

    from_env.map_or_else(
        || Settings::default().show_browser,
        |value| value.trim() != "false",
    )
}

fn fill_blank(target: &mut String, key: &str) {
    if !target.trim().is_empty() {
        return;
    }
    if let Ok(value) = std::env::var(key)
        && !value.trim().is_empty()
    {
        *target = value;
    }
}

fn non_blank(value: &str, fallback: &str) -> String {
    if value.trim().is_empty() {
        fallback.to_string()
    } else {
        value.trim().to_string()
    }
}

fn project_dirs() -> Result<ProjectDirs> {
    ProjectDirs::from(QUALIFIER, ORGANIZATION, APPLICATION)
        .context("could not resolve a platform config directory")
}

pub fn settings_path() -> Result<PathBuf> {
    Ok(project_dirs()?.config_dir().join(SETTINGS_FILE))
}

pub fn session_path() -> Result<PathBuf> {
    Ok(project_dirs()?.data_dir().join(SESSION_FILE))
}

pub fn calendar_path() -> Result<PathBuf> {
    Ok(project_dirs()?.data_dir().join(CALENDAR_FILE))
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::atomic::{AtomicUsize, Ordering};

    /// The minimum that passes validation, so a test can say what it is about.
    fn settings() -> Settings {
        Settings {
            personio_email: "a@b.com".into(),
            personio_company: "acme".into(),
            employee_id: "42".into(),
            ..Settings::default()
        }
    }

    #[test]
    fn parses_padded_times() {
        assert_eq!(parse_time("9:5").unwrap(), ("09".into(), "05".into()));
        assert_eq!(parse_time("18:00").unwrap(), ("18".into(), "00".into()));
    }

    #[test]
    fn rejects_invalid_times() {
        for value in ["", "9", "24:00", "10:60", "aa:bb", "10-00"] {
            assert!(parse_time(value).is_err(), "{value:?} should be rejected");
        }
    }

    #[test]
    fn builds_urls_without_double_slashes() {
        let config = settings()
            .to_run_config(PasswordProvider::new(|| None), None)
            .unwrap();

        assert_eq!(config.base_url(), "https://acme.app.personio.com");
        assert_eq!(config.home_url(), "https://acme.app.personio.com/");
        assert_eq!(
            config.attendance_url(),
            "https://acme.app.personio.com/attendance/employee/42"
        );
    }

    #[test]
    fn validation_reports_every_missing_field() {
        let error = Settings::default()
            .to_run_config(PasswordProvider::new(|| None), None)
            .unwrap_err();
        let message = error.to_string();

        assert!(message.contains("email"), "{message}");
        assert!(message.contains("company"), "{message}");
        assert!(message.contains("employee id"), "{message}");
    }

    #[test]
    fn on_disk_format_matches_the_electron_app() {
        let json = serde_json::to_string(&Settings::default()).unwrap();
        assert!(json.contains("\"personioEmail\""), "{json}");
        assert!(json.contains("\"showBrowser\""), "{json}");
        assert!(json.contains("\"startTimeFirstSlot\""), "{json}");
    }

    #[test]
    fn ignores_unknown_keys_from_older_settings_files() {
        let raw = r#"{"personioEmail":"a@b.com","launchAtLogin":true}"#;
        let settings: Settings = serde_json::from_str(raw).unwrap();

        assert_eq!(settings.personio_email, "a@b.com");
        assert_eq!(settings.start_time_first_slot, DEFAULT_START_FIRST);
        assert!(settings.show_browser);
    }

    /// The whole point of the provider: building a run config must not reach
    /// the password source, or pressing `t` would prompt the keychain before
    /// the browser has even tried the saved session.
    #[test]
    fn building_a_run_config_never_resolves_the_password() {
        let asked = Arc::new(AtomicUsize::new(0));
        let counter = Arc::clone(&asked);
        let provider = PasswordProvider::new(move || {
            counter.fetch_add(1, Ordering::SeqCst);
            Some("hunter2".to_string())
        });

        let config = settings().to_run_config(provider, None).unwrap();
        assert_eq!(asked.load(Ordering::SeqCst), 0);

        assert_eq!(
            config.personio_password.resolve().as_deref(),
            Some("hunter2")
        );
        assert_eq!(asked.load(Ordering::SeqCst), 1);
    }

    #[test]
    fn a_blank_password_resolves_to_none() {
        assert_eq!(PasswordProvider::new(|| Some("   ".into())).resolve(), None);
        assert_eq!(
            PasswordProvider::new(|| Some(String::new())).resolve(),
            None
        );
        assert_eq!(PasswordProvider::new(|| None).resolve(), None);
    }

    #[test]
    fn a_debugged_config_does_not_leak_the_password() {
        let config = settings()
            .to_run_config(PasswordProvider::new(|| Some("hunter2".into())), None)
            .unwrap();

        assert!(!format!("{config:?}").contains("hunter2"));
    }

    /// The README promises the file always wins over the environment. A bool
    /// has no blank value for `fill_blank` to test, so this one field used to
    /// be the exception — saved as `off`, back to `on` on the next start-up.
    #[test]
    fn a_stored_browser_setting_wins_over_the_environment() {
        assert!(!resolve_show_browser(Some(false), Some("true".into())));
        assert!(!resolve_show_browser(Some(false), Some("anything".into())));
        assert!(resolve_show_browser(Some(true), Some("false".into())));
    }

    #[test]
    fn the_environment_decides_only_what_the_file_left_out() {
        assert!(!resolve_show_browser(None, Some("false".into())));
        assert!(!resolve_show_browser(None, Some(" false ".into())));
        assert!(resolve_show_browser(None, Some("true".into())));
    }

    #[test]
    fn with_neither_the_browser_is_shown() {
        assert_eq!(
            resolve_show_browser(None, None),
            Settings::default().show_browser
        );
    }

    /// "Absent" and "false" have to stay distinguishable, which the
    /// deserialised struct cannot express.
    #[test]
    fn a_file_that_says_nothing_about_the_browser_is_not_a_file_that_says_no() {
        assert_eq!(show_browser_in(r#"{"showBrowser": false}"#), Some(false));
        assert_eq!(show_browser_in(r#"{"showBrowser": true}"#), Some(true));
        assert_eq!(show_browser_in(r#"{"personioEmail": "a@b.com"}"#), None);
        assert_eq!(show_browser_in("{}"), None);
        assert_eq!(show_browser_in(r#"{"showBrowser": "off"}"#), None);
        assert_eq!(show_browser_in("{ not json"), None);
    }

    /// The whole round trip, on a real file: saved as off, read back as off.
    #[test]
    fn a_browser_setting_saved_as_off_reads_back_as_off() {
        let settings = Settings {
            show_browser: false,
            ..settings()
        };
        let raw = serde_json::to_string_pretty(&settings).unwrap();

        assert_eq!(show_browser_in(&raw), Some(false));
        assert!(!resolve_show_browser(
            show_browser_in(&raw),
            Some("true".into())
        ));
    }

    /// `--show` and `--headless` are documented as applying to one run, so
    /// they must not reach the settings: the pane would show them as stored
    /// and `w` would write them to disk.
    #[test]
    fn a_visibility_override_applies_to_the_run_and_not_to_the_settings() {
        let stored = Settings {
            show_browser: false,
            ..settings()
        };

        let forced = stored
            .to_run_config(PasswordProvider::new(|| None), Some(true))
            .unwrap();
        assert!(forced.show_browser);
        assert!(!stored.show_browser, "the settings are left alone");

        let hidden = stored
            .to_run_config(PasswordProvider::new(|| None), Some(false))
            .unwrap();
        assert!(!hidden.show_browser);
    }

    #[test]
    fn without_an_override_the_run_uses_what_is_stored() {
        for stored in [true, false] {
            let settings = Settings {
                show_browser: stored,
                ..settings()
            };
            let config = settings
                .to_run_config(PasswordProvider::new(|| None), None)
                .unwrap();

            assert_eq!(config.show_browser, stored);
        }
    }
}
