//! Personio Tracker — a terminal UI that fills in pending attendance days.

mod browser;
mod cli;
mod config;
mod event;
mod keychain;
mod tracking;
mod tui;

use anyhow::Result;

use config::Settings;

const HELP: &str = "\
personio-tracker-tui — fill pending Personio attendance days

USAGE:
    personio-tracker-tui [OPTIONS]

OPTIONS:
    (none)         Launch the terminal UI
    --cli          Run once and print progress to stdout, for cron jobs
    --headless     Force the browser to stay hidden for this run
    --show         Force the browser to be visible for this run
    --paths        Print the settings and session file locations
    -h, --help     Print this help
    -V, --version  Print the version

CONFIGURATION:
    Settings live in a JSON file (see --paths) and are editable from the UI.
    Any blank field falls back to the environment, which is also read from a
    .env file in the working directory:
      PERSONIO_EMAIL, PERSONIO_COMPANY, EMPLOYEE_ID, PERSONIO_PASSWORD,
      SHOW_BROWSER, START_TIME_FIRST_SLOT, END_TIME_FIRST_SLOT,
      START_TIME_SECOND_SLOT, END_TIME_SECOND_SLOT

    The password is read from the OS keychain first (service `personio-track`),
    falling back to PERSONIO_PASSWORD.

    PERSONIO_CHROME_PATH overrides browser detection.
";

#[derive(Debug, PartialEq, Eq)]
enum Command {
    Tui,
    Cli,
    Help,
    Version,
    Paths,
}

#[derive(Debug, PartialEq, Eq)]
struct Args {
    command: Command,
    /// Overrides the persisted `showBrowser` flag for this run only.
    show_browser: Option<bool>,
}

fn parse_args<I: IntoIterator<Item = String>>(raw: I) -> Result<Args> {
    let mut command = Command::Tui;
    let mut show_browser = None;

    for argument in raw {
        match argument.as_str() {
            "--cli" => command = Command::Cli,
            "--paths" => command = Command::Paths,
            "-h" | "--help" => command = Command::Help,
            "-V" | "--version" => command = Command::Version,
            "--headless" => show_browser = Some(false),
            "--show" => show_browser = Some(true),
            other => anyhow::bail!("unknown argument {other:?} — try --help"),
        }
    }

    Ok(Args {
        command,
        show_browser,
    })
}

#[tokio::main]
async fn main() -> Result<()> {
    // A local .env is a convenience, not a requirement.
    let _ = dotenvy::dotenv();

    let args = parse_args(std::env::args().skip(1))?;

    match args.command {
        Command::Help => {
            print!("{HELP}");
            return Ok(());
        }
        Command::Version => {
            println!("{} {}", env!("CARGO_PKG_NAME"), env!("CARGO_PKG_VERSION"));
            return Ok(());
        }
        Command::Paths => {
            println!("settings: {}", config::settings_path()?.display());
            println!("session:  {}", config::session_path()?.display());
            println!(
                "chrome:   {}",
                browser::launch::find_chrome()
                    .map(|path| path.display().to_string())
                    .unwrap_or_else(|| "not found (chromiumoxide will auto-detect)".to_string())
            );
            return Ok(());
        }
        Command::Tui | Command::Cli => {}
    }

    let mut settings = Settings::load();
    if let Some(show_browser) = args.show_browser {
        settings.show_browser = show_browser;
    }

    match args.command {
        Command::Cli => cli::run(settings).await,
        _ => tui::run(settings).await,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn parse(args: &[&str]) -> Result<Args> {
        parse_args(args.iter().map(|value| value.to_string()))
    }

    #[test]
    fn defaults_to_the_terminal_ui() {
        let args = parse(&[]).unwrap();
        assert_eq!(args.command, Command::Tui);
        assert_eq!(args.show_browser, None);
    }

    #[test]
    fn recognises_every_documented_flag() {
        assert_eq!(parse(&["--cli"]).unwrap().command, Command::Cli);
        assert_eq!(parse(&["--paths"]).unwrap().command, Command::Paths);
        assert_eq!(parse(&["-h"]).unwrap().command, Command::Help);
        assert_eq!(parse(&["--help"]).unwrap().command, Command::Help);
        assert_eq!(parse(&["-V"]).unwrap().command, Command::Version);
    }

    #[test]
    fn browser_visibility_can_be_forced_either_way() {
        assert_eq!(parse(&["--headless"]).unwrap().show_browser, Some(false));
        assert_eq!(parse(&["--show"]).unwrap().show_browser, Some(true));
    }

    #[test]
    fn the_last_visibility_flag_wins() {
        assert_eq!(
            parse(&["--show", "--headless"]).unwrap().show_browser,
            Some(false)
        );
    }

    #[test]
    fn unknown_arguments_are_rejected_with_a_hint() {
        let error = parse(&["--nope"]).unwrap_err().to_string();
        assert!(error.contains("--nope"), "{error}");
        assert!(error.contains("--help"), "{error}");
    }
}
