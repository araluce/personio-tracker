# personio-tracker-tui

A terminal UI that fills in your pending Personio attendance days.

A Rust port of [`personio-track`](../../scripts/personio-track), which was an
Electron tray app driving Playwright. Same behaviour, one 3.8 MB binary, no
Node runtime and no bundled browser.

```
 Personio Tracker │ idle
╭ Live log ──────────────────────────────────╮╭ Configuration ─────────────────╮
│14:22:01 Starting tracking session          ││▸ Email         me@acme.com     │
│14:22:04 Valid session                      ││  Company       acme            │
│14:22:06 Opening Time Tracking              ││  Employee ID   1234            │
│14:22:09 Tracking month (30 rows)           ││  Slot 1 start  09:00           │
│14:22:09 1 — Weekend: not trackable         ││  Slot 1 end    14:00           │
│14:22:10 3: already registered              ││  Slot 2 start  15:00           │
│14:22:22 4: shift registered                ││  Slot 2 end    18:00           │
│14:22:34 5: shift registered                ││  Show browser  on              │
│14:22:35 Reached today's row at index 4     ││  Password      •••••• saved    │
╰────────────────────────────────────────────╯╰────────────────────────────────╯
```

## What it does

Walks the Personio timesheet from the current month backwards, registering the
configured two work slots on every trackable day that has no entry yet. It
stops at today's row (after filling it) and keeps stepping back a month while
that month still has fewer confirmed hours than its target.

Weekends, public holidays and absences are skipped, with the reason Personio
itself shows recorded in the log.

## Requirements

- Rust 1.98+ to build.
- A Chromium-based browser. Discovery order:
  1. `PERSONIO_CHROME_PATH`
  2. Playwright's cache (`~/Library/Caches/ms-playwright`, `~/.cache/ms-playwright`)
     — so an existing `playwright install` is reused
  3. System Chrome / Chromium / Edge

  `--paths` prints which one was picked.

## Install

```sh
cargo install --path .
```

## Usage

```sh
personio-tracker-tui              # terminal UI
personio-tracker-tui --cli        # run once, print to stdout (for cron)
personio-tracker-tui --headless   # hide the browser for this run
personio-tracker-tui --show       # show the browser for this run
personio-tracker-tui --paths      # settings, session and browser locations
```

### Keys

| Key          | Action                                    |
| ------------ | ----------------------------------------- |
| `t`          | start a run                               |
| `w`          | save settings to disk                     |
| `tab`        | switch between the log and configuration  |
| `j` / `k`    | move field, or scroll the log             |
| `g` / `G`    | jump to top / follow the log tail         |
| `enter`      | edit the selected field                   |
| `space`      | toggle the selected flag                  |
| `c`          | clear the log                             |
| `?`          | help                                      |
| `q`          | quit (refused mid-run)                    |
| `Q`/`ctrl-c` | force quit, killing the browser           |

## Configuration

Settings are stored as JSON — run `--paths` to find the file. The format is
identical to the Electron app's `settings.json`, so an existing file can be
copied across unchanged.

Any field left blank falls back to the environment, which is also read from a
`.env` in the working directory:

| Variable                                        | Purpose                    |
| ----------------------------------------------- | -------------------------- |
| `PERSONIO_EMAIL`, `PERSONIO_COMPANY`            | account                    |
| `EMPLOYEE_ID`                                   | whose timesheet to fill    |
| `PERSONIO_PASSWORD`                             | fallback if no keychain    |
| `SHOW_BROWSER`                                  | `false` to run hidden      |
| `START_TIME_FIRST_SLOT`, `END_TIME_FIRST_SLOT`  | first work block           |
| `START_TIME_SECOND_SLOT`, `END_TIME_SECOND_SLOT`| second work block          |
| `PERSONIO_CHROME_PATH`                          | override browser discovery |

A value in the settings file always wins over the environment, so editing in
the UI is never silently overridden by a stale `.env`.

### Password

Read from the OS keychain first (service `personio-track`, account
`personio-password` — the same entry the Electron app used, so it carries
over), falling back to `PERSONIO_PASSWORD`.

To store a new one, select the `Password` field and press `enter`. It is
written to the keychain and never read back into the UI — the field only ever
shows whether one is stored.

### Session

Cookies and `localStorage` are saved after a successful login and replayed on
the next run, so logging in is a once-in-a-while event. Delete the session file
(`--paths`) to force a fresh login.

## Notes and known constraints

- **Selectors are Personio's, not ours.** The tracker drives Personio's real
  web UI, so a redesign on their side breaks it. Every selector lives in one
  `selectors` module in `src/tracking.rs`.
- **Some selectors are locale-dependent.** The previous-month button and the
  segmented time inputs are matched by `aria-label`. Spanish and English
  labels are both matched; another UI language needs a new entry.
- **A run aborts on the first unexpected error**, matching the original. The
  error lands in the summary and the log.

## Differences from the Electron version

Deliberate:

- No tray icon and no launch-at-login. A TUI is started when you want it; use
  `--cli` from `cron` or a `launchd` job for unattended runs.
- Each run gets a throwaway Chrome profile. chromiumoxide otherwise reuses one
  fixed directory, and Chrome's `ProcessSingleton` then refuses to start —
  which also meant every run after a killed one would fail.
- One `Runtime.evaluate` reads a whole timesheet row instead of six separate
  DOM round-trips.
- Hour totals parse both `158.5 h` and `158,5 h`. The original used
  `Number("158,5")`, which is `NaN`, and `NaN < NaN` is false — so the walk
  back through previous months stopped early on a Spanish-formatted account.
- The cookie banner is matched by XPath. The original used Playwright's
  `:has-text()` pseudo-class, which is not valid CSS and throws inside
  `querySelectorAll`.

## Development

```sh
cargo test      # 57 tests, ~2s (8 of them drive a real headless Chromium)
cargo clippy --all-targets
cargo fmt
```

The browser-facing tests in `src/browser/locator.rs` are the ones worth reading
first: Playwright's auto-waiting `Locator` has no equivalent in Rust, so
`src/browser/locator.rs` reimplements it on raw CDP and those tests pin down
the behaviour it has to match.
