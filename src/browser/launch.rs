//! Chromium discovery and launch.

use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicU64, Ordering};
use std::time::{Duration, SystemTime, UNIX_EPOCH};

use anyhow::{Context, Result, anyhow};
use chromiumoxide::browser::{Browser, BrowserConfig};
use chromiumoxide::handler::viewport::Viewport;
use futures::StreamExt;
use tokio::task::JoinHandle;

/// chromiumoxide defaults to an 800x600 viewport, which pushes the Personio
/// timesheet into its narrow layout and breaks the row selectors. Playwright
/// defaults to 1280x720; we go a little wider so the two-slot editor fits.
const VIEWPORT_WIDTH: u32 = 1440;
const VIEWPORT_HEIGHT: u32 = 900;

/// Personio is a heavy SPA; the default request timeout is too optimistic.
const REQUEST_TIMEOUT: Duration = Duration::from_secs(60);
const LAUNCH_TIMEOUT: Duration = Duration::from_secs(60);

/// A running browser plus the background task driving its CDP handler.
///
/// The handler stream *must* be polled for any command to resolve, so it is
/// spawned here and joined on [`BrowserHandle::close`].
pub struct BrowserHandle {
    browser: Browser,
    handler_task: JoinHandle<()>,
    profile_dir: PathBuf,
}

impl BrowserHandle {
    pub fn browser(&self) -> &Browser {
        &self.browser
    }

    /// Closes the browser, stops the handler task and removes the throwaway
    /// profile. Errors are swallowed because this runs on the teardown path,
    /// where the run outcome matters more than a clean shutdown.
    pub async fn close(mut self) {
        let _ = self.browser.close().await;
        let _ = self.browser.wait().await;
        self.handler_task.abort();
        let _ = self.handler_task.await;
        let _ = std::fs::remove_dir_all(&self.profile_dir);
    }
}

/// Builds a profile directory that no other process can be using.
///
/// chromiumoxide otherwise reuses a single fixed directory, and Chrome's
/// `ProcessSingleton` aborts the launch when it finds another instance's
/// `SingletonLock` there. That breaks two real cases: a second run started
/// while one is in flight, and — worse — every run after one that was killed,
/// because the stale lock is never cleaned up.
fn unique_profile_dir() -> PathBuf {
    static COUNTER: AtomicU64 = AtomicU64::new(0);

    let nanos = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|elapsed| elapsed.as_nanos())
        .unwrap_or(0);

    std::env::temp_dir().join(format!(
        "personio-tracker-{}-{}-{}",
        std::process::id(),
        nanos,
        COUNTER.fetch_add(1, Ordering::Relaxed)
    ))
}

pub async fn launch(show_browser: bool) -> Result<BrowserHandle> {
    let profile_dir = unique_profile_dir();

    let mut builder = BrowserConfig::builder()
        .user_data_dir(&profile_dir)
        .viewport(Some(Viewport {
            width: VIEWPORT_WIDTH,
            height: VIEWPORT_HEIGHT,
            ..Viewport::default()
        }))
        .window_size(VIEWPORT_WIDTH, VIEWPORT_HEIGHT)
        .request_timeout(REQUEST_TIMEOUT)
        .launch_timeout(LAUNCH_TIMEOUT)
        .arg("--disable-notifications")
        .arg("--disable-blink-features=AutomationControlled");

    builder = if show_browser {
        builder.with_head()
    } else {
        // The new headless mode shares Chrome's real rendering path, which
        // keeps the DOM identical to what we see in headful runs. The old
        // mode is chromiumoxide's default and renders differently often
        // enough to matter for selector stability.
        builder.new_headless_mode()
    };

    if let Some(executable) = find_chrome() {
        builder = builder.chrome_executable(executable);
    }

    let config = builder
        .build()
        .map_err(|error| anyhow!("invalid browser configuration: {error}"))?;

    let (browser, mut handler) = Browser::launch(config).await.context(
        "launching Chromium (if no browser is installed, \
         install Google Chrome or set PERSONIO_CHROME_PATH)",
    )?;

    let handler_task = tokio::spawn(async move {
        while let Some(event) = handler.next().await {
            if event.is_err() {
                break;
            }
        }
    });

    Ok(BrowserHandle {
        browser,
        handler_task,
        profile_dir,
    })
}

/// Resolves a Chromium binary, in order of preference:
/// explicit override, Playwright's browser cache, then system Chrome.
/// Returning `None` lets chromiumoxide fall back to its own detection.
pub fn find_chrome() -> Option<PathBuf> {
    if let Ok(explicit) = std::env::var("PERSONIO_CHROME_PATH") {
        let path = PathBuf::from(explicit);
        if path.is_file() {
            return Some(path);
        }
    }

    playwright_chromium().or_else(system_chrome)
}

/// Reuses the Chromium that `playwright install` already downloaded, so users
/// coming from the Node app do not need a second browser on disk.
fn playwright_chromium() -> Option<PathBuf> {
    let cache = playwright_cache_dir()?;
    let mut revisions: Vec<PathBuf> = std::fs::read_dir(&cache)
        .ok()?
        .filter_map(Result::ok)
        .map(|entry| entry.path())
        .filter(|path| {
            path.file_name()
                .and_then(|name| name.to_str())
                .is_some_and(|name| name.starts_with("chromium-"))
        })
        .collect();

    // Revision directories are `chromium-<number>`; the highest is the newest.
    revisions.sort_by_key(|path| {
        path.file_name()
            .and_then(|name| name.to_str())
            .and_then(|name| name.trim_start_matches("chromium-").parse::<u64>().ok())
            .unwrap_or(0)
    });

    revisions
        .iter()
        .rev()
        .find_map(|revision| chromium_binary_in(revision).filter(|candidate| candidate.is_file()))
}

fn playwright_cache_dir() -> Option<PathBuf> {
    if let Ok(explicit) = std::env::var("PLAYWRIGHT_BROWSERS_PATH") {
        let path = PathBuf::from(explicit);
        if path.is_dir() {
            return Some(path);
        }
    }

    let home = PathBuf::from(std::env::var("HOME").ok()?);
    let candidates = [
        home.join("Library/Caches/ms-playwright"),
        home.join(".cache/ms-playwright"),
    ];

    candidates.into_iter().find(|path| path.is_dir())
}

/// Playwright's layout differs per platform, and on macOS the app bundle name
/// has changed across revisions, so the bundle is discovered rather than guessed.
fn chromium_binary_in(revision: &Path) -> Option<PathBuf> {
    if cfg!(target_os = "macos") {
        for arch_dir in ["chrome-mac-arm64", "chrome-mac"] {
            let root = revision.join(arch_dir);
            let Ok(entries) = std::fs::read_dir(&root) else {
                continue;
            };

            for entry in entries.filter_map(Result::ok) {
                let path = entry.path();
                if path.extension().and_then(|value| value.to_str()) != Some("app") {
                    continue;
                }

                let stem = path.file_stem()?.to_str()?.to_string();
                let binary = path.join("Contents/MacOS").join(&stem);
                if binary.is_file() {
                    return Some(binary);
                }
            }
        }
        return None;
    }

    if cfg!(target_os = "windows") {
        return Some(revision.join("chrome-win/chrome.exe"));
    }

    Some(revision.join("chrome-linux/chrome"))
}

fn system_chrome() -> Option<PathBuf> {
    let candidates: &[&str] = if cfg!(target_os = "macos") {
        &[
            "/Applications/Google Chrome.app/Contents/MacOS/Google Chrome",
            "/Applications/Chromium.app/Contents/MacOS/Chromium",
            "/Applications/Microsoft Edge.app/Contents/MacOS/Microsoft Edge",
        ]
    } else if cfg!(target_os = "windows") {
        &[
            r"C:\Program Files\Google\Chrome\Application\chrome.exe",
            r"C:\Program Files (x86)\Google\Chrome\Application\chrome.exe",
        ]
    } else {
        &[
            "/usr/bin/google-chrome",
            "/usr/bin/chromium",
            "/usr/bin/chromium-browser",
        ]
    };

    candidates
        .iter()
        .map(PathBuf::from)
        .find(|path| path.is_file())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn profile_dirs_are_unique_per_call() {
        let dirs: Vec<PathBuf> = (0..64).map(|_| unique_profile_dir()).collect();
        let mut deduped = dirs.clone();
        deduped.sort();
        deduped.dedup();

        assert_eq!(deduped.len(), dirs.len(), "profile dirs collided: {dirs:?}");
    }

    #[test]
    fn profile_dirs_live_under_the_temp_dir() {
        assert!(unique_profile_dir().starts_with(std::env::temp_dir()));
    }

    #[tokio::test]
    async fn closing_removes_the_throwaway_profile() {
        let handle = launch(false).await.expect("launching headless Chromium");
        let profile = handle.profile_dir.clone();
        assert!(profile.is_dir(), "Chromium should have created {profile:?}");

        handle.close().await;

        assert!(!profile.exists(), "{profile:?} should have been cleaned up");
    }

    #[tokio::test]
    async fn two_browsers_can_run_at_the_same_time() {
        // Regression: a shared profile directory made Chrome's ProcessSingleton
        // abort the second launch.
        let (first, second) = tokio::join!(launch(false), launch(false));
        let first = first.expect("first browser");
        let second = second.expect("second browser");

        assert_ne!(first.profile_dir, second.profile_dir);

        first.close().await;
        second.close().await;
    }
}
