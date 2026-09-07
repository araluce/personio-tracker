<p align="center">
  <img src="assets/logo.svg" alt="personio-tracker — fills your pending attendance days, from a terminal" width="760">
</p>

A terminal UI that fills in your pending Personio attendance days.

A Rust port of `personio-track`, an Electron tray app that drove Playwright.
Same behaviour, one 4 MB binary, no Node runtime and no bundled browser.

> **Unofficial.** Not affiliated with, endorsed by, or supported by Personio SE.
> It drives their web UI exactly as you would, which is also why a redesign on
> their side can break it.

```
 Personio Tracker │ ⠸ running
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
│                                            │╰────────────────────────────────╯
│                                            │╭ September 2026 ────────────────╮
│                                            ││      Mo Tu We Th Fr Sa Su      │
│                                            ││         ██ ██ ██ ██ ██ ██      │
│                                            ││      ██ ██ ██ ██ ██ ██ ██      │
│                                            ││      ██ ██ ██ ██ ██ ██ ██      │
│                                            ││      ██ ██ ██ ██ ██ ·· ··      │
│                                            ││      ·· ·· ··                  │
│                                            │╰────────────────────────────────╯
│                                            │╭ Summary ───────────────────────╮
│                                            ││No runs yet.                    │
╰━━━━───────────────────────────────────────━╯╰────────────────────────────────╯
 t track · w save · j/k move · h/l month · enter edit · ? help · q quit
```

## Quick path

```sh
make install PREFIX=$HOME/.local   # or: cargo install --path .
personio-tracker
```

1. Fill in **Email**, **Company** and **Employee ID** — `enter` edits the
   selected field. The last one is [not obvious](docs/configuration.md#employee-id);
   the app says where to find it under the field.
2. Press `w` to save, then `t` to run.
3. The first run opens a browser and logs in. Later runs replay that session,
   so they neither log in nor ask the OS for your password.

The grid under the configuration is the month at a glance, and it is drawn from
a local record: opening the app shows where the month stands before anything
runs.

## Run it daily

The point of the thing: never think about your timesheet again.

```sh
which personio-tracker    # the path depends on how you installed it
crontab -e                # opens $EDITOR; add the line below, save, done
```

```cron
0 9 * * * /usr/local/bin/personio-tracker --cli --headless >> "$HOME/Library/Logs/personio-tracker.log" 2>&1
```

Nothing to reload afterwards — `crontab -l` confirms it took. Saving writes the
whole crontab, so leave any `SHELL=` and `PATH=` lines already in there alone.

`--cli` because a scheduler has no terminal, the absolute path because it has no
useful PATH, and `--headless` because nobody is there to watch a browser open.
Scheduled runs keep the month grid current too, so the UI shows the month
without you ever pressing `t`.

Try it before waiting for 09:00, since a failing cron job fails quietly:

```sh
/usr/local/bin/personio-tracker --cli --headless; echo "exit=$?"
```

**On macOS, prefer a LaunchAgent.** A `cron` job runs outside the logged-in
session, where the keychain is out of reach — which only bites when the saved
session expires, but then it needs you. A LaunchAgent runs inside the session
and logs in by itself.

[Scheduling](docs/scheduling.md) has the plist to copy, the systemd option for
Linux, and what to do when a run reports `Login required`.

## Documentation

| To… | Read |
| --- | --- |
| install it, or find where its files live | [Install](docs/install.md) |
| set up email, company, employee id, work slots | [Configuration](docs/configuration.md) |
| understand the password and session handling | [Authentication](docs/authentication.md) |
| read the month grid and its colours | [Month grid](docs/month-grid.md) |
| know exactly what a run does to a timesheet | [Tracking](docs/tracking.md) |
| schedule it: cron, launchd, systemd | [Scheduling](docs/scheduling.md) |
| drive the UI: keys, panes, the log | [Interface](docs/interface.md) |
| work on the code | [Development](docs/development.md) |

## What it does

Walks the Personio timesheet from the current month backwards, registering the
configured two work slots on every trackable day that has no entry yet. It
stops at today's row (after filling it) and keeps stepping back a month while
that month still has fewer confirmed hours than its target.

Weekends, public holidays and absences are skipped, with the reason Personio
itself shows recorded in the log. [Tracking](docs/tracking.md) has the whole
walk, step by step.

## The one thing to know

**The selectors are Personio's, not ours.** This drives their real web UI, so a
redesign on their side breaks it. Everything that can break that way is listed
in [Tracking → When it breaks](docs/tracking.md#when-it-breaks), and every
selector lives in one file: `src/tracking/selectors.rs`.
