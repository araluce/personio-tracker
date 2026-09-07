//! Terminal front-end: event loop plus rendering.

mod app;
mod ui;

use std::time::Duration;

use anyhow::{Context, Result};
use crossterm::event::{Event, EventStream};
use futures::StreamExt;
use tokio::sync::mpsc;

use crate::config::{PasswordProvider, Settings};
use crate::event::{EventSink, Severity};
use crate::{keychain, tracking};

use app::{Action, App, Status};

const TICK: Duration = Duration::from_millis(250);

/// Takes over the terminal, runs the loop, and always restores the terminal —
/// including on a panic, which `ratatui::restore` handles via its hook.
pub async fn run(settings: Settings) -> Result<()> {
    let mut terminal = ratatui::try_init()
        .context("could not take over the terminal — for a non-interactive run, use --cli")?;
    let result = event_loop(&mut terminal, settings).await;
    ratatui::restore();
    result
}

async fn event_loop(terminal: &mut ratatui::DefaultTerminal, settings: Settings) -> Result<()> {
    let mut app = App::new(settings, keychain::has_password());
    let (sink, mut events) = mpsc::unbounded_channel();
    let mut input = EventStream::new();
    let mut ticker = tokio::time::interval(TICK);

    loop {
        terminal
            .draw(|frame| ui::render(frame, &app))
            .context("drawing the terminal")?;

        if app.should_quit {
            break;
        }

        tokio::select! {
            maybe_input = input.next() => {
                match maybe_input {
                    Some(Ok(Event::Key(key))) => {
                        if let Some(action) = app.handle_key(key) {
                            perform(&mut app, action, &sink).await;
                        }
                    }
                    // Resize and focus events only need a redraw, which the
                    // top of the loop already does.
                    Some(Ok(_)) => {}
                    Some(Err(error)) => return Err(error).context("reading terminal input"),
                    None => break,
                }
            }
            Some(event) = events.recv() => {
                app.apply_tracking_event(event);
            }
            _ = ticker.tick() => {
                app.expire_toast();
            }
        }
    }

    // Leaving with a run in flight would orphan Chromium; aborting the task
    // drops the browser handle, which kills the child process.
    if let Some(task) = app.run_task.take() {
        task.abort();
        let _ = task.await;
    }

    Ok(())
}

async fn perform(app: &mut App, action: Action, sink: &EventSink) {
    match action {
        Action::StartTracking => start_tracking(app, sink),
        Action::SaveSettings => match app.settings.save() {
            Ok(path) => {
                app.mark_settings_saved();
                app.toast(format!("Settings saved to {}", path.display()), false);
            }
            Err(error) => app.toast(format!("Could not save settings: {error:#}"), true),
        },
        Action::SavePassword(password) => {
            if password.trim().is_empty() {
                app.toast("Password unchanged (nothing typed)", true);
                return;
            }

            match keychain::set_password(&password) {
                Ok(()) => {
                    app.keychain_has_password = true;
                    app.toast("Password saved to the OS keychain", false);
                }
                Err(error) => app.toast(format!("Keychain unavailable: {error:#}"), true),
            }
        }
        Action::ForceQuit => {
            if let Some(task) = app.run_task.take() {
                task.abort();
                let _ = task.await;
            }
            app.should_quit = true;
        }
    }
}

fn start_tracking(app: &mut App, sink: &EventSink) {
    // Passed as a source, not a value: pressing `t` must not reach the
    // keychain, only a run that discovers it has to log in.
    let password = PasswordProvider::new(keychain::resolve_password);

    let config = match app.settings.to_run_config(password) {
        Ok(config) => config,
        Err(error) => {
            let message = format!("{error:#}");
            app.push_log(message.clone(), Severity::Error);
            app.status = Status::Failed(message.clone());
            app.toast(message, true);
            return;
        }
    };

    app.clear_log();
    app.summary = None;
    app.status = Status::Running;

    let sink = sink.clone();
    app.run_task = Some(tokio::spawn(async move {
        // The run reports its own outcome through the sink, including failures,
        // so the returned error is intentionally dropped here.
        let _ = tracking::run_tracking(&config, &sink).await;
    }));
}
