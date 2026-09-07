# Interface

Four panes and a footer. The footer always names the keys that apply to what
you are doing.

```
 Personio Tracker │ ⠸ running   ● unsaved      ← status, and edits not yet written
╭ Live log ───────────────╮╭ Configuration ───╮
│ what the run is doing   ││ the settings      │
│                         │╰───────────────────╯
│                         │╭ September 2026 ──╮
│                         ││ the month grid    │
│                         │╰───────────────────╯
│                         │╭ Summary ─────────╮
│                         ││ the last run      │
╰━━━━─────────────────────╯╰───────────────────╯
 t track · w save · j/k move · h/l month · enter edit · ? help · q quit
```

## Keys

| Key | Action |
| --- | --- |
| `t` | start a run |
| `w` | save settings to disk |
| `tab` | switch between the log and the configuration |
| `j` / `k` | move field, or scroll the log |
| `g` / `G` | jump to top / follow the log tail |
| `h` / `l` | page the [month grid](month-grid.md) back and forward |
| `enter` | edit the selected field |
| `space` | toggle the selected flag |
| `c` | clear the log |
| `?` | help, including the grid's colour legend |
| `q` | quit — refused mid-run |
| `Q` / `ctrl-c` | force quit, killing the browser |

The footer carries the handful you reach for; `?` carries all of them. `h` and
`l` are letters first: while a field is being edited they type themselves.

## Command line

```sh
personio-tracker              # terminal UI
personio-tracker --cli        # run once, print to stdout — for cron
personio-tracker --headless   # hide the browser for this run
personio-tracker --show       # show the browser for this run
personio-tracker --paths      # settings, session, day record and browser locations
personio-tracker --help
personio-tracker --version
```

`--show` and `--headless` apply to the run they are given to and are never
written to the settings file. See
[Configuration](configuration.md#--show-and---headless).

The UI needs a terminal: without one it exits 1 with `could not take over the
terminal — for a non-interactive run, use --cli`.

## While a run is in flight

The screen says "working" rather than looking hung, because Personio spends
whole seconds on a single click:

- a spinner beside the status
- a highlight travelling along the foot of the log pane
- the [month grid](month-grid.md) filling in as each day resolves

Only a run pays for that: the redraw clock runs at 80 ms while tracking and
250 ms when idle, so an idle app is not burning a laptop battery to animate
nothing.

## The log

| Colour | Means |
| --- | --- |
| green | something achieved — a day registered, the session saved |
| grey | a day left alone: already registered, or not trackable |
| yellow | a check the run could not make and worked around |
| red | a failure |

`g` jumps to the top, `G` follows the tail, `j`/`k` scroll when the log pane has
focus, and `c` clears it. A new run clears it for you.

## Settings and unsaved edits

Editing a field marks the header `● unsaved` until `w` writes the file. Quitting
does not warn about unsaved edits — the marker is the only warning — and
settings are locked while a run is in progress.

## Next

[Month grid](month-grid.md) — the one pane worth explaining on its own.
