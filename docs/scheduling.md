# Scheduling

Running it daily without you. On macOS prefer **launchd**; the keychain is the
reason.

## Two things a scheduler must be told

| Because it has no… | Give it… |
| --- | --- |
| terminal | `--cli`. The UI exits 1 with `could not take over the terminal` |
| useful PATH | the absolute path to the binary — `which personio-tracker`, since it depends on how you installed it. Neither `~/.cargo/bin` nor `~/.local/bin` is on the PATH a scheduled job inherits |

`--headless` on top of that is belt and braces: it applies to that run alone and
is never written to the settings file, so a scheduled job cannot change what
your UI shows.

The run exits non-zero when it fails, so a wrapper can notice.

## The keychain catch

**A `cron` job does not run inside the logged-in GUI session.** The login
keychain is usually out of reach from there, and there is no way to answer its
authorisation dialog from a job nobody is watching.

Most days that costs nothing, because of how
[authentication](authentication.md) works: the password is only read when a run
has to log in, and a run replaying its saved session never asks for it.

When the session does expire, the run ends with `Login required but no password
is available` and a non-zero exit. Two ways out:

| Fix | Trade-off |
| --- | --- |
| Start the UI once and let it log in | Manual, but nothing is stored in plain text |
| `PERSONIO_PASSWORD` in the job's environment | Unattended forever, at the price of the password in plain text |
| Use a **LaunchAgent** instead of cron | Runs inside the session, so the keychain is reachable and it logs in by itself |

## launchd — macOS

Runs inside the logged-in session, and catches up on a missed run once the
machine wakes, which `cron` does not.

`~/Library/LaunchAgents/com.personiotrack.daily.plist`:

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
  <dict>
    <key>Hour</key><integer>9</integer>
    <key>Minute</key><integer>0</integer>
  </dict>
  <key>StandardOutPath</key>
  <string>/Users/YOU/Library/Logs/personio-tracker.log</string>
  <key>StandardErrorPath</key>
  <string>/Users/YOU/Library/Logs/personio-tracker.log</string>
</dict>
</plist>
```

```sh
launchctl bootstrap gui/$(id -u) ~/Library/LaunchAgents/com.personiotrack.daily.plist
launchctl kickstart -p gui/$(id -u)/com.personiotrack.daily   # run it now
launchctl print gui/$(id -u)/com.personiotrack.daily          # inspect it
launchctl bootout gui/$(id -u)/com.personiotrack.daily        # remove it
```

`ProgramArguments` takes a real path — `launchd` does not expand `~` or `$HOME`.

## cron

```sh
which personio-tracker    # the path to use below
crontab -e                # opens $EDITOR
```

Add one line — daily at 09:00 — then save. There is nothing to reload:

```cron
0 9 * * * /usr/local/bin/personio-tracker --cli --headless >> "$HOME/Library/Logs/personio-tracker.log" 2>&1
```

```sh
crontab -l                # confirm it took
```

Saving writes the whole crontab, so leave any `SHELL=` or `PATH=` lines already
in there alone. A `PATH` that happens to contain the binary's directory would
make the absolute path unnecessary — use it anyway, so the job does not depend
on a line someone may edit later.

For a script, or to skip the editor:

```sh
(crontab -l; echo '0 9 * * * /usr/local/bin/personio-tracker --cli --headless >> "$HOME/Library/Logs/personio-tracker.log" 2>&1') | crontab -
```

If the job appears to run and do nothing at all, check whether `cron` has Full
Disk Access in System Settings › Privacy & Security.

## Linux

Both routes work. `cron` there is the ordinary choice, or a systemd user timer
if you want the session — `systemctl --user` units run inside the user session,
so the Secret Service is reachable the way a LaunchAgent's keychain is.

Where there is no Secret Service at all — a headless box, a container — the
password lookup fails rather than blocking, and `PERSONIO_PASSWORD` takes over.
See [Authentication → When there is no keychain](authentication.md#when-there-is-no-keychain).

## Verify

```sh
# what the scheduler will run, run by hand first
/full/path/to/personio-tracker --cli --headless; echo "exit=$?"
```

- [ ] it completes by hand, from a directory other than the repo
- [ ] the log file gets written where you pointed it
- [ ] `exit=0`
- [ ] after the scheduled run, the [month grid](month-grid.md) shows the day

## Next

[Month grid](month-grid.md) — scheduled runs keep it current too, so the UI
shows the month without you ever pressing `t`.
