# Month grid

One cell per day of the month, coloured by what became of it.

```
╭ September 2026 ────────────────╮
│      Mo Tu We Th Fr Sa Su      │
│         ██ ██ ██ ██ ██ ██      │   the 1st sits under its own weekday
│      ██ ██ ██ ██ ██ ██ ██      │
│      ██ ██ ██ ██ ██ ██ ██      │
│      ██ ██ ██ ██ ██ ·· ··      │   ·· nothing known yet
│      ·· ·· ··                  │
╰────────────────────────────────╯
```

| Colour | Meaning |
| --- | --- |
| bright green | tracked by a run |
| green | already registered before a run reached it |
| light blue | an absence or day off |
| magenta | a public holiday |
| grey `██` | weekend |
| grey `··` | nothing known about that day yet |

Today's cell is underlined, and `?` lists the colours without leaving the app.
Weekends fall in the last two columns, so the shape of a month reads at a
glance.

**It is drawn from a local record, not from Personio.** Opening the app shows
where the month stands before anything runs; `t` repaints it live as the run
resolves each day.

## Paging

`h` and `l` page back and forward through the months a run has walked, and stop
where the record does — there is nothing to show past it in either direction.
Pressing `t` snaps back to the current month, which is where a walk starts.

The view is held as a distance from the current month rather than as a month, so
leaving the app open past midnight on the 1st cannot strand it on a month that
is no longer current.

## The day record

`calendar.json`, next to the session file (`--paths`). Keyed by month and day,
sorted so the file stays diffable:

```json
{
  "months": {
    "2026-09": {
      "1": "already-registered",
      "5": "weekend",
      "7": "tracked"
    }
  }
}
```

**It is a cache, never a source of truth.** The timesheet lives at Personio;
this only exists so the grid can be drawn without a browser round-trip. A
missing or unreadable file reads as "no record" — the pane title says so — and
the next run rewrites whatever it sees. Deleting it costs nothing.

It is written when a run ends, by `--cli` runs as much as by the UI, so an
unattended [scheduled](scheduling.md) job keeps it current too. A run killed
half-way writes nothing, and loses nothing that matters: the next run reads the
same days back from Personio.

Every month a run walks through is recorded, not just the current one, so
paging back has something to show.

## How a day is placed

In this order, stopping at the first that answers:

| Source | Why here |
| --- | --- |
| a full date in the printed label | so a label that *is* a date isn't scanned for digits — `2026-09-12` would read as the 9th |
| the day number in the printed label | **the trustworthy one.** It is what Personio shows and what the log repeats back, so the grid cannot disagree with either. Localised, so `lun 12` and `12 sept.` both place their row |
| the row's `datetime` / `data-date` attributes | a fallback only, for a row that prints no readable day |
| the row's position in the timesheet | last resort, and only once the month has turned out to list exactly one row per day |

A row that still cannot be placed is **counted and reported** when the run
ends — `3 day(s) could not be placed on the month grid` — never guessed at. One
wrong offset would recolour a whole month, and a grid that is quietly wrong is
worse than one that is visibly incomplete.

### Why the label beats a whole date

It looks backwards: an attribute carrying a full date is more information than
a printed day number. It was tried that way, and on a real account those
attributes came back **a day behind the label beside them** — which moved every
cell one column to the left and filed 1 September under August.

Two independent causes, both real:

- The attribute can be an *instant* rather than a day. Personio writes local
  midnight as UTC, so in Madrid the 1st of September arrives as
  `2026-08-31T22:00:00Z`. Reading the text before the `T` calls that the 31st of
  August. Such a value is now moved into local time before its day is taken.
- Even then it disagreed. The label did not.

So the label leads, and the attributes are kept for rows that print nothing
readable.

## Troubleshooting

| Symptom | Meaning |
| --- | --- |
| Title says `· no record` | nothing is known about that month yet — press `t`, or page to a month a run has walked |
| The grid disagrees with the log | should be impossible now; both come from the printed day. Report it with the log lines |
| `N day(s) could not be placed` | those rows offered no readable day and their month was not listed one row per day |
| The grid is a day out | the bug above, in a new form. The log's day numbers are the reference |

## Verify

- [ ] the weekend cells sit under `Sa` and `Su`
- [ ] the 1st sits under the weekday it really falls on
- [ ] today's cell is underlined
- [ ] the run's log lines and the coloured cells agree, day for day

## Next

[Scheduling](scheduling.md) — keeping the record current without opening the UI.
