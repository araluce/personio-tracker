//! Persisted settings and the resolved configuration a tracking run needs.
//!
//! The on-disk format is intentionally identical to the Electron app's
//! `settings.json` (same camelCase keys), so an existing file can be copied
//! over without conversion. Unknown keys such as `launchAtLogin` are ignored.

use std::fs;
use std::path::PathBuf;

use anyhow::{Context, Result, bail};
use directories::ProjectDirs;
use serde::{Deserialize, Serialize};

const QUALIFIER: &str = "com";
const ORGANIZATION: &str = "personiotrack";
const APPLICATION: &str = "personio-tracker-tui";
const SETTINGS_FILE: &str = "settings.json";
const SESSION_FILE: &str = "session.json";

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
        let mut settings = Self::read_file().unwrap_or_default();
        settings.apply_env_fallbacks();
        settings
    }

    fn read_file() -> Option<Self> {
        let path = settings_path().ok()?;
        let raw = fs::read_to_string(path).ok()?;
        serde_json::from_str(&raw).ok()
    }

    fn apply_env_fallbacks(&mut self) {
        fill_blank(&mut self.personio_email, "PERSONIO_EMAIL");
        fill_blank(&mut self.personio_company, "PERSONIO_COMPANY");
        fill_blank(&mut self.employee_id, "EMPLOYEE_ID");
        fill_blank(&mut self.start_time_first_slot, "START_TIME_FIRST_SLOT");
        fill_blank(&mut self.end_time_first_slot, "END_TIME_FIRST_SLOT");
        fill_blank(&mut self.start_time_second_slot, "START_TIME_SECOND_SLOT");
        fill_blank(&mut self.end_time_second_slot, "END_TIME_SECOND_SLOT");

        if let Ok(value) = std::env::var("SHOW_BROWSER") {
            self.show_browser = value != "false";
        }
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

    /// Turns settings plus the resolved password into a runnable configuration.
    pub fn to_run_config(&self, password: Option<String>) -> Result<RunConfig> {
        let config = RunConfig {
            personio_email: self.personio_email.trim().to_string(),
            personio_password: password.filter(|value| !value.trim().is_empty()),
            personio_company: self.personio_company.trim().to_string(),
            employee_id: self.employee_id.trim().to_string(),
            show_browser: self.show_browser,
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

/// Everything a tracking run needs, already validated.
#[derive(Debug, Clone)]
pub struct RunConfig {
    pub personio_email: String,
    pub personio_password: Option<String>,
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

#[cfg(test)]
mod tests {
    use super::*;

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
        let settings = Settings {
            personio_email: "a@b.com".into(),
            personio_company: "acme".into(),
            employee_id: "42".into(),
            ..Settings::default()
        };
        let config = settings.to_run_config(None).unwrap();

        assert_eq!(config.base_url(), "https://acme.app.personio.com");
        assert_eq!(config.home_url(), "https://acme.app.personio.com/");
        assert_eq!(
            config.attendance_url(),
            "https://acme.app.personio.com/attendance/employee/42"
        );
    }

    #[test]
    fn validation_reports_every_missing_field() {
        let error = Settings::default().to_run_config(None).unwrap_err();
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
}
