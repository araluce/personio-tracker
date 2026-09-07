# Configuration

Everything is editable from the UI: select a field with `j`/`k`, press `enter`
to edit it or `space` to toggle it, then `w` to write the file. `--paths` says
where that file is.

## Fields

| Field | Meaning | Default |
| --- | --- | --- |
| Email | the account you log into Personio with | — |
| Company | the subdomain of your Personio URL | — |
| Employee ID | whose timesheet to fill — see [below](#employee-id) | — |
| Slot 1 start / end | the morning work block | 09:00 – 14:00 |
| Slot 2 start / end | the afternoon work block | 15:00 – 18:00 |
| Show browser | whether the Chromium window is visible | on |
| Password | write-only, and not kept in this file — see [Authentication](authentication.md) | — |

A run refuses to start without Email, Company and Employee ID, and names the
ones it is missing. Times must be `HH:MM` within range; `9:5` is accepted and
stored as `09:05`.

## Employee ID

The number Personio uses to address your timesheet — not your name, not your
email. Log in, open **Attendance** from your own profile, and read the last
segment of the URL:

```
https://acme.app.personio.com/attendance/employee/1234
        ^^^^ Company                              ^^^^ Employee ID
```

The same number appears in your profile URL (`/staff/details/1234`). The app
also prints this hint under the field while it is selected, along with one for
Company.

## The file

JSON, and deliberately the same shape as the Electron app's `settings.json`
(same camelCase keys), so an existing file can be copied across unchanged.
Unknown keys such as `launchAtLogin` are ignored rather than rejected.

```json
{
  "personioEmail": "me@acme.com",
  "personioCompany": "acme",
  "employeeId": "1234",
  "showBrowser": false,
  "startTimeFirstSlot": "09:00",
  "endTimeFirstSlot": "14:00",
  "startTimeSecondSlot": "15:00",
  "endTimeSecondSlot": "18:00"
}
```

## Environment

Any field the file leaves blank falls back to the environment, which is also
read from a `.env` in the working directory:

| Variable | Fills |
| --- | --- |
| `PERSONIO_EMAIL`, `PERSONIO_COMPANY` | account |
| `EMPLOYEE_ID` | whose timesheet to fill |
| `PERSONIO_PASSWORD` | the password, when the keychain has none |
| `SHOW_BROWSER` | `false` to run hidden |
| `START_TIME_FIRST_SLOT`, `END_TIME_FIRST_SLOT` | first work block |
| `START_TIME_SECOND_SLOT`, `END_TIME_SECOND_SLOT` | second work block |
| `PERSONIO_CHROME_PATH` | overrides browser discovery |

### Precedence

**The file always wins.** Editing in the UI is never silently overridden by a
stale `.env`, `SHOW_BROWSER` included: the environment only decides what the
file says nothing about.

That last one took some doing. Every text field has a blank value to test, so
"the file said nothing" is obvious. A `bool` does not — serde fills a missing
key with the default and the file's silence becomes indistinguishable from the
file's `false`. So the presence of `showBrowser` is read from the raw JSON
rather than from the deserialised struct.

### `--show` and `--headless`

These belong to the run they are given to. They are **never folded into the
settings**, which means:

- the configuration pane keeps showing what is stored, not what the flag forced
- `w` cannot persist them by accident
- each run reports the visibility it actually used in its first log line:
  `Starting tracking session (show browser: false)`

## Verify

- [ ] `w` shows "Settings saved to …" and the `● unsaved` marker clears
- [ ] the file at `--paths` holds what you typed
- [ ] reopening the app shows the same values

## Next

[Authentication](authentication.md) — where the password lives, and why a
normal run never asks for it.
