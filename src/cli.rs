//! Non-interactive front-end, for cron jobs and CI.

use anyhow::Result;
use tokio::sync::mpsc;

use crate::config::{PasswordProvider, Settings};
use crate::event::TrackingEvent;
use crate::{keychain, tracking};

pub async fn run(settings: Settings) -> Result<()> {
    let config = settings.to_run_config(PasswordProvider::new(keychain::resolve_password))?;

    let (sink, mut events) = mpsc::unbounded_channel::<TrackingEvent>();
    let printer = tokio::spawn(async move {
        while let Some(event) = events.recv().await {
            println!("{}", event.line());
        }
    });

    let outcome = tracking::run_tracking(&config, &sink).await;

    // Dropping the sender ends the printer, so every queued line is flushed
    // before the summary is written.
    drop(sink);
    let _ = printer.await;

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
