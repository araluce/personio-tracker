# Scheduling

Running it every weekday without you. Use the scheduler your OS runs **inside
your login session**: a LaunchAgent on macOS, a systemd user timer on Linux, a
Task Scheduler task on Windows. Not `cron` — the keychain is the reason.

| OS | Use | Catches up a missed run |
| --- | --- | --- |
| macOS | [LaunchAgent](#macos--launchagent) | after sleep, once; not after power-off |
| Linux | [systemd user timer](#linux--systemd-user-timer) | after sleep or power-off, once |
| Windows | [Task Scheduler](#windows--task-scheduler) | once, with *start when available* |
| a box with no session at all | [cron + `PERSONIO_PASSWORD`](#cron-only-without-a-session) | no |

## Why not cron

**A `cron` job does not run inside the logged-in session.** The keychain
dialog belongs to that session, so a cron job can never be asked for access:
the lookup fails on the spot, the run ends with `Login required but no password
is available`, and the only trace is a line in a log nobody reads.

Most days that costs nothing, because of how
[authentication](authentication.md) works: the password is only read when a run
has to log in, and a run replaying its saved session never asks for it. Then
the session expires — after a reboot, typically — and every run from there on
fails until you notice.

The session schedulers below run where the keychain can answer. The first time
a run needs the password the OS asks once; allow it permanently and it does not
ask again.

> **macOS:** the binary is not signed, so the keychain remembers that
> permission for this exact build. After a reinstall it asks once more.

## Two things a scheduler must be told

| Because it has no… | Give it… |
| --- | --- |
| terminal | `--cli`. The UI exits 1 with `could not take over the terminal` |
| useful PATH | the absolute path to the binary — `which personio-tracker`, since it depends on how you installed it |

`--headless` on top of that is belt and braces: it applies to that run alone and
is never written to the settings file, so a scheduled job cannot change what
your UI shows.

The run exits non-zero when it fails, so a wrapper can notice.

## macOS — LaunchAgent

`~/Library/LaunchAgents/com.personiotrack.daily.plist`, Monday to Friday at
09:00:

```xml
<?xml version="1.0" encoding="UTF-8"?>
<!DOCTYPE plist PUBLIC "-//Apple//DTD PLIST 1.0//EN"
  "http://www.apple.com/DTDs/PropertyList-1.0.dtd">
<plist version="1.0">
<dict>
  <key>Label</key><string>com.personiotrack.daily</string>
  <key>ProgramArguments</key>
  <array>
    <string>/usr/local/bin/personio-tracker</string>
    <string>--cli</string>
    <string>--headless</string>
  </array>
  <key>StartCalendarInterval</key>
  <array>
    <dict><key>Weekday</key><integer>1</integer><key>Hour</key><integer>9</integer><key>Minute</key><integer>0</integer></dict>
    <dict><key>Weekday</key><integer>2</integer><key>Hour</key><integer>9</integer><key>Minute</key><integer>0</integer></dict>
    <dict><key>Weekday</key><integer>3</integer><key>Hour</key><integer>9</integer><key>Minute</key><integer>0</integer></dict>
    <dict><key>Weekday</key><integer>4</integer><key>Hour</key><integer>9</integer><key>Minute</key><integer>0</integer></dict>
    <dict><key>Weekday</key><integer>5</integer><key>Hour</key><integer>9</integer><key>Minute</key><integer>0</integer></dict>
  </array>
  <key>StandardOutPath</key>
  <string>/Users/YOU/Library/Logs/personio-tracker.log</string>
  <key>StandardErrorPath</key>
  <string>/Users/YOU/Library/Logs/personio-tracker.log</string>
</dict>
</plist>
```

`StartCalendarInterval` takes the same fields as a cron line — `Minute`,
`Hour`, `Day`, `Weekday` (0 is Sunday), `Month` — and a missing one means "any".
For every day, drop the array and keep a single `<dict>` with `Hour` and
`Minute`.

```sh
plutil -lint ~/Library/LaunchAgents/com.personiotrack.daily.plist
launchctl bootstrap gui/$(id -u) ~/Library/LaunchAgents/com.personiotrack.daily.plist
launchctl kickstart -p gui/$(id -u)/com.personiotrack.daily   # run it now
launchctl print gui/$(id -u)/com.personiotrack.daily          # inspect it
launchctl bootout gui/$(id -u)/com.personiotrack.daily        # remove it
```

`ProgramArguments` takes a real path — `launchd` does not expand `~` or `$HOME`.
The agent loads again by itself at every login.

A Mac asleep at 09:00 runs the job when it wakes, and several missed slots
collapse into one run — which is enough, because a run fills every pending day
of the current month. A Mac switched off at 09:00 skips it.

## Linux — systemd user timer

`systemctl --user` units run inside your session, so the Secret Service — GNOME
Keyring, KWallet — is reachable, unlocked at login.

`~/.config/systemd/user/personio-tracker.service`:

```ini
[Unit]
Description=personio-tracker run

[Service]
Type=oneshot
ExecStart=/usr/local/bin/personio-tracker --cli --headless
```

`~/.config/systemd/user/personio-tracker.timer`:

```ini
[Unit]
Description=personio-tracker, weekdays at 09:00

[Timer]
OnCalendar=Mon..Fri 09:00
Persistent=true

[Install]
WantedBy=timers.target
```

```sh
systemctl --user daemon-reload
systemctl --user enable --now personio-tracker.timer
systemctl --user start personio-tracker.service     # run it now
systemctl --user list-timers personio-tracker.timer # when it runs next
journalctl --user -u personio-tracker               # its output
systemctl --user disable --now personio-tracker.timer   # remove it
```

`Persistent=true` makes a run missed while the machine was asleep or off happen
once, as soon as it is back. Output goes to the journal; there is no log file
to point anywhere.

## Windows — Task Scheduler

> Untested, like the Windows build itself — see [Install](install.md).

The Credential Manager puts up no dialog, so a task running as you, while you
are logged on, reads the password with nothing to approve. In PowerShell:

```powershell
$exe      = 'C:\path\to\personio-tracker.exe'
$log      = "$env:LOCALAPPDATA\personio-tracker.log"
$action   = New-ScheduledTaskAction -Execute 'cmd.exe' `
              -Argument "/c `"`"$exe`" --cli --headless >> `"$log`" 2>&1`""
$trigger  = New-ScheduledTaskTrigger -Weekly -At 9am `
              -DaysOfWeek Monday,Tuesday,Wednesday,Thursday,Friday
$settings = New-ScheduledTaskSettingsSet -StartWhenAvailable
Register-ScheduledTask -TaskName 'personio-tracker' `
  -Action $action -Trigger $trigger -Settings $settings
```

`cmd.exe /c` is only there for the redirect: Task Scheduler keeps no output of
its own. `-StartWhenAvailable` is the catch-up — a run missed while the machine
was off happens once at the next chance. Registered without a principal, the
task runs as you and only while you are logged on.

```powershell
Start-ScheduledTask -TaskName 'personio-tracker'                    # run it now
Get-ScheduledTaskInfo -TaskName 'personio-tracker'                  # last result, next run
Unregister-ScheduledTask -TaskName 'personio-tracker' -Confirm:$false   # remove it
```

## cron, only without a session

A server or a container has no login session and no keychain to open. There
the password lookup fails rather than blocking, `PERSONIO_PASSWORD` takes over
(see [Authentication → When there is no keychain](authentication.md#when-there-is-no-keychain)),
and `cron` is as good as anything:

```cron
0 9 * * 1-5 /usr/local/bin/personio-tracker --cli --headless >> "$HOME/personio-tracker.log" 2>&1
```

The price is the password in plain text in the job's environment. On a machine
you log in to, use the session scheduler for your OS instead.

## Verify

```sh
# what the scheduler will run, run by hand first
/full/path/to/personio-tracker --cli --headless; echo "exit=$?"
```

- [ ] it completes by hand, from a directory other than the repo
- [ ] the scheduler's own "run it now" completes too, and answers the keychain
      prompt if there is one
- [ ] the output lands where you expect: the log file, or the journal
- [ ] after the first scheduled run, the [month grid](month-grid.md) shows the day

## Next

[Month grid](month-grid.md) — scheduled runs keep it current too, so the UI
shows the month without you ever pressing `t`.
