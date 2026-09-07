//! Non-interactive front-end, for cron jobs and CI.

use anyhow::Result;
use tokio::sync::mpsc;

use crate::calendar::{Calendar, Recorder};
use crate::config::{PasswordProvider, Settings};
use crate::event::TrackingEvent;
use crate::{keychain, tracking};

pub async fn run(settings: Settings, show_browser: Option<bool>) -> Result<()> {
    let config = settings.to_run_config(
        PasswordProvider::new(keychain::resolve_password),
        show_browser,
    )?;

    let (sink, mut events) = mpsc::unbounded_channel::<TrackingEvent>();

    // An unattended run has to leave the same trail as the UI, or the month
    // grid would go blank for anyone driving this from a scheduler.
    let mut recorder = Recorder::new(Calendar::load());
    let printer = tokio::spawn(async move {
        while let Some(event) = events.recv().await {
            println!("{}", event.line());
            recorder.apply(&event);
        }
        recorder
    });

    let outcome = tracking::run_tracking(&config, &sink).await;

    // Dropping the sender ends the printer, so every queued line is flushed
    // before the summary is written.
    drop(sink);

    // Written before the run's own failure is raised: a run that died halfway
    // still resolved days, and throwing them away would blank the grid.
    if let Ok(recorder) = printer.await {
        let unplaced = recorder.unplaced();
        if unplaced > 0 {
            eprintln!("{unplaced} day(s) could not be placed on the month grid");
        }
        if let Err(error) = recorder.calendar().save() {
            eprintln!("Could not save the day record: {error:#}");
        }
    }

    let summary = outcome?;

    println!();
    println!("=== Tracking summary ===");
    println!("Tracked days:             {}", summary.tracked_days.len());
    println!("Skipped days:             {}", summary.skipped_days.len());
    println!(
        "Already registered days:  {}",
        summary.already_registered_days.len()
    );
    println!("Errors:                   {}", summary.errors.len());
    println!("Months visited:           {}", summary.months_visited);

    Ok(())
}
