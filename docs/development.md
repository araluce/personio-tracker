# Development

```sh
make test     # 140 tests, ~3s — 10 of them drive a real headless Chromium
make lint     # rustfmt --check, then clippy with warnings denied
make fmt
make build    # release
```

## Where things live

| Module | Holds |
| --- | --- |
| `src/tracking/mod.rs` | the walk: open the month, fill its days, step back |
| `src/tracking/day.rs` | one timesheet row: reading it, and which day it is |
| `src/tracking/auth.rs` | cookie banner, login form, waiting to be let in |
| `src/tracking/selectors.rs` | **every Personio selector** — one file, on purpose |
| `src/browser/locator.rs` | an auto-waiting `Locator` on raw CDP |
| `src/browser/launch.rs` | browser discovery and the throwaway profile |
| `src/browser/session.rs` | saving and replaying cookies and `localStorage` |
| `src/calendar.rs` | the day record and the `Recorder` that folds events into it |
| `src/config.rs` | settings, environment precedence, `PasswordProvider` |
| `src/keychain.rs` | OS keychain, and the once-per-process password cache |
| `src/event.rs` | the event vocabulary both front-ends render |
| `src/tui/` | state (`app.rs`), rendering (`ui.rs`), event loop (`mod.rs`) |
| `src/cli.rs` | the `--cli` front-end |

Test counts, as a map of where the risk is thought to be:

```
32  tui::ui          22  tracking::day  22  calendar
21  tui::app         16  config          8  browser::locator
 5  keychain          4  browser::launch 4  browser::session
 2  tracking          1  tracking::auth
```

## Read these first

**`src/browser/locator.rs`.** Playwright's auto-waiting `Locator` has no
equivalent in Rust, so this reimplements it on raw CDP, and its tests pin down
the behaviour it has to match. They drive a real headless Chromium.

**`src/event.rs`.** One event vocabulary, two front-ends. The TUI and `--cli`
render the same run narrative because neither invents its own: a front-end that
needs new information adds it to an event rather than reaching into the tracker.

## The rules this code follows

Each of these was learned by getting it wrong first.

| Rule | Why |
| --- | --- |
| Rendering is a pure function of state | `ui.rs` never mutates `App`. Animations take a tick counter, nothing else |
| The page script lives in `day.rs::row_expression()`, not inline | a syntax error there breaks *every* row read in a run, so it is built somewhere it can be tested |
| A day that cannot be placed is counted, not guessed | one wrong offset recolours a whole month. See [Month grid](month-grid.md#how-a-day-is-placed) |
| The keychain is read only when a login actually needs it | reading it raises an OS dialog. See [Authentication](authentication.md) |
| Settings beat the environment, always | including `showBrowser`, which needs the raw JSON to tell "absent" from "false" |
| A cache must never break the main job | a missing or corrupt day record reads as "no record"; a missing date attribute costs the grid precision, not the run |

## Testing notes

Things that have bitten, in this codebase specifically:

- **A regression test that does not fail against its own bug is not a test.**
  Several here were verified by temporarily reverting the fix and confirming
  they fail. Do that on a copy: reverting production code in a working tree
  someone else is building from will ship them the bug.
- **Do not build an expectation from the function under test.** The grid
  alignment test once computed its expected column by calling `leading_blanks`,
  so it passed with that function stubbed to `0`. Expectations come from
  `chrono`, or from the calendar, never from the code being checked.
- **A `Buffer` column is a cell, not a byte.** Pane borders (`│ ╭ ─`) are three
  bytes each, so `str::find` lands far to the right of the column it names. The
  ui tests carry a `column_of` helper for this.
- **A `Buffer` cell holds one grapheme.** A two-column `▇▇` cell is two cells
  whose symbol is `▇`.
- **Date fixtures are built from `Local`.** A hardcoded UTC string would pass in
  UTC and fail in Madrid, or worse, the other way round.
- **No test mutates the process environment.** It is `unsafe` in edition 2024
  and racy across parallel tests, so environment-dependent logic is a pure
  function taking what it needs (`resolve_show_browser`).

## Differences from the Electron original

Deliberate:

- No tray icon and no launch-at-login. A TUI is started when you want it; use
  [scheduling](scheduling.md) for unattended runs.
- Each run gets a throwaway Chrome profile, because chromiumoxide otherwise
  reuses one fixed directory and Chrome's `ProcessSingleton` refuses to start —
  which also meant every run after a killed one failed.
- One `Runtime.evaluate` reads a whole timesheet row instead of six separate DOM
  round-trips.
- Hour totals parse both `158.5 h` and `158,5 h`. The original used
  `Number("158,5")`, which is `NaN`, and `NaN < NaN` is false — so the walk back
  stopped early on a Spanish-formatted account.
- The cookie banner is matched by XPath. The original used Playwright's
  `:has-text()` pseudo-class, which is not valid CSS and throws inside
  `querySelectorAll`.
- The password is resolved lazily and cached per process, so a session that
  tracks five times authorises once instead of five times.

## Before opening a PR

- [ ] `make lint` and `make test` pass
- [ ] a new behaviour has a test that fails without it — verified, not assumed
- [ ] a new selector went into `tracking/selectors.rs`, not inline
- [ ] the affected doc under `docs/` says the new truth
