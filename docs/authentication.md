# Authentication

**The short version: log in once, and the password is rarely touched again.**

A successful login is saved as a session and replayed on the next run. A run
that replays its session never logs in, and a run that never logs in never
reads the password — so it never raises the OS keychain dialog either.

```
run ──> replay saved session ──> valid? ──yes──> track
                                   │
                                   no
                                   ▼
                            read the password ──> log in ──> save session
                            (keychain, then env)
```

## Storing a password

Select the **Password** field and press `enter`. It is written to the OS
keychain and never read back into the UI — the field only ever shows whether
one is stored, never a value.

That indicator costs nothing: it asks the keychain whether the item exists,
which is answered from the item's attributes and needs no authorisation. Asking
for the *secret* is what raises a dialog, and the app only does that when a run
has to log in.

| Platform | Where it goes |
| --- | --- |
| macOS | the login keychain, service `personio-track`, account `personio-password` |
| Linux | the Secret Service over D-Bus — gnome-keyring, KWallet |
| Windows | the Credential Manager |

The macOS service and account names match the Electron app's `keytar` entry, so
a password saved there carries over with nothing to do.

## When there is no keychain

A headless box or a container has no Secret Service. The lookup **fails rather
than blocking**, and `PERSONIO_PASSWORD` takes over. That is the case
[scheduling](scheduling.md) is built around.

A denied dialog or a locked keychain is deliberately *not* remembered as "no
password": a mis-clicked Deny would otherwise become a permanent failure until
the app restarts.

## macOS authorisation, in detail

The keychain is read at most once per process, so a session that tracks five
times authorises once. When a dialog does appear, **Always Allow** ends it for
good — the grant is tied to the binary, so rebuilding it asks once more.

If you see two dialogs for one read, that is macOS updating the item's access
list, not the app reading twice.

## The session

Cookies and `localStorage` are saved after a successful login and replayed on
the next run, which is what makes logging in a once-in-a-while event.

`localStorage` is origin-scoped, so it can only be replayed once the browser is
already on the Personio origin — and only a reload makes the app read it. That
is why the log shows `Restored saved session` before `Valid session`: the first
is the replay, the second is Personio accepting it.

To force a fresh login, delete the session file (`--paths`).

## Troubleshooting

| Symptom | Meaning |
| --- | --- |
| `Login required but no password is available` | the session expired and neither the keychain nor `PERSONIO_PASSWORD` could supply one |
| A keychain dialog on a run that used to be silent | the session expired, so this run had to log in |
| A keychain dialog on every run | the grant is not being remembered — answer **Always Allow**, and expect one more prompt after a rebuild |
| The password field says `not set` but you saved one | the keychain is unreachable, not empty; check that a Secret Service is running on Linux |

## Verify

- [ ] the Password field reads `•••••• saved`
- [ ] a second `t` in the same session raises no dialog
- [ ] deleting `session.json` makes the next run log in again

## Next

[Tracking](tracking.md) — what the run does once it is in.
