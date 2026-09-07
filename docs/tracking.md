# Tracking

What pressing `t` — or running `--cli` — actually does to a timesheet.

## The walk

1. Open Personio, replay the saved session, log in if it was refused.
   ([Authentication](authentication.md))
2. Open the attendance page for the configured Employee ID.
3. Walk the current month's rows from the top:
   - a day that cannot be tracked is **skipped**, with Personio's own reason
   - a day that already has hours is **left alone**
   - anything else gets both configured work slots registered
4. Stop at **today's row** — after filling it in, not before.
5. Step back a month while that month still has fewer confirmed hours than its
   target, and repeat from 3.

Nothing in the future is ever touched, which is why the [month
grid](month-grid.md) legitimately ends part-filled.

## Which days are skipped

From Personio's own row flags, never from the words it prints — those are
localised and would not survive a UI language change.

| Flag | Meaning in the grid |
| --- | --- |
| `data-is-weekend` | Weekend |
| `data-is-holiday` | Holiday, with the name Personio shows |
| `data-is-off-day` | An absence of any kind: vacation, sick leave, unpaid leave |

The log line carries the reason it shows you: `5 sept — Weekend: not
trackable`.

## Stepping back a month

The walk continues while a month has hours missing, comparing the confirmed
total against the target. Both are read from Personio's own widget.

Two things worth knowing:

- Totals parse both `158.5 h` and `158,5 h`. The original used
  `Number("158,5")`, which is `NaN`, and `NaN < NaN` is false — so the walk
  stopped early on a Spanish-formatted account.
- When the widget cannot be read at all, the run logs **No tracked month hours
  found** in yellow and stops walking back. That is a warning, not a failure:
  the check could not be made, so the run finishes where it is with `Errors 0`.

## The browser

Each run gets a **throwaway Chrome profile**. chromiumoxide otherwise reuses
one fixed directory, and Chrome's `ProcessSingleton` then refuses to start —
which also meant that after one killed run, every later run failed.

`Q` or `ctrl-c` kills the browser with the run. `q` is refused mid-run so a
half-filled day is not left behind by accident.

## When it breaks

**The selectors are Personio's, not ours.** This drives their real web UI. Every
selector lives in one `selectors` module in `src/tracking.rs`, which is the
first place to look when a run stops finding things.

| Risk | Detail |
| --- | --- |
| A Personio redesign | changes the selectors, and the run stops finding rows |
| Locale-dependent selectors | the previous-month button and the segmented time inputs are matched by `aria-label`. Spanish and English are both matched; another UI language needs a new entry |
| First error aborts the run | matching the original. The error lands in the summary and the log, and the day record still keeps what the run got through |

## Reading a run

```
14:14:55 Starting tracking session (show browser: false)
14:14:56 Restored saved session      ← the session file was replayed
14:15:00 Valid session               ← Personio accepted it; no login needed
14:15:02 Tracking month (30 rows)
14:15:02 1 sept: already registered  ← left alone
14:15:02 5 sept — Weekend: not trackable
14:15:02 7 sept: shift registered    ← filled in by this run
14:15:02 Reached today's row at index 6
14:15:14 Session finished — Tracked 1 / Skipped 2 / Already registered 5 / …
```

Colours in the log: green for something achieved, grey for a day left alone,
**yellow for a check that could not be made**, red for a failure.

## Verify

- [ ] the summary's counts add up to the rows the log walked
- [ ] no day past today was touched
- [ ] the [grid](month-grid.md) agrees with the log, day for day

## Next

[Month grid](month-grid.md) — how the run's findings are drawn and kept.
