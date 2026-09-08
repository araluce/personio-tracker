# Install

```sh
git clone https://github.com/araluce/personio-tracker
cd personio-tracker
```

Then one of three routes, all installing the same `personio-tracker` binary.
**Pick one** — two copies on a PATH is one too many.

```sh
make install PREFIX="$HOME/.local"   # no sudo; needs ~/.local/bin on your PATH
sudo make install                    # /usr/local/bin
cargo install --path .               # ~/.cargo/bin, and cargo remembers it
```

## Requirements

| Need | Detail |
| --- | --- |
| A current stable Rust | the crate is edition 2024, developed and tested on 1.98. Build-time only: the binary is self-contained |
| A Chromium-based browser | discovered, not bundled — see below |
| Nothing from your package manager | no OpenSSL, no `pkg-config`, no `-dev` packages. The dependency tree is pure Rust plus each platform's own APIs, which is why CI builds it on a bare Ubuntu runner with no install step |

## Where it runs

| System | State | Detail |
| --- | --- | --- |
| **macOS** | verified | where it is developed. The password goes in the login keychain, via Security.framework |
| **Linux** | verified by CI | all 140 tests pass on `ubuntu-latest`, the browser-driving ones included. The password goes to the Secret Service over D-Bus — gnome-keyring, KWallet — through `zbus`, which is pure Rust, so no `libdbus` to install |
| **Windows** | untested | the browser paths and the Credential Manager backend are both in the tree and it should build, but nobody has run it. Reports welcome |

Where no Secret Service is running — a headless box, a container — the password
lookup fails rather than blocking and `PERSONIO_PASSWORD` takes over. See
[Authentication](authentication.md#when-there-is-no-keychain).

The browser is looked for in this order, and `--paths` prints which one won:

1. `PERSONIO_CHROME_PATH`, if it points at a file
2. Playwright's cache — `~/Library/Caches/ms-playwright` on macOS,
   `~/.cache/ms-playwright` on Linux — so an existing `playwright install` is
   reused rather than duplicated
3. The system browser:
   - macOS: Google Chrome, Chromium, Microsoft Edge under `/Applications`
   - Linux: `/usr/bin/google-chrome`, `/usr/bin/chromium`,
     `/usr/bin/chromium-browser`
   - Windows: Chrome under `Program Files`

## Uninstall

Which command depends on how it went in. Note the **package** name, not the
command name, for the cargo route:

```sh
make uninstall                      # and PREFIX=… if you used one
cargo uninstall personio-tracker-tui
```

## Packaging

`make install` honours `PREFIX` and `DESTDIR`, so a distro package can drive it
directly:

```sh
make install DESTDIR="$pkgdir" PREFIX=/usr
```

The release build runs `--locked`, so it resolves exactly what `Cargo.lock`
pins rather than whatever is newest today. `make` on its own lists every
target.

Two portability details, in case they matter to a packager:

- The install step uses `install -d` and `install -m 755`, not GNU-only
  `install -D`: macOS ships the BSD `install`.
- Nothing else is installed. No man page, no completions, no unit files. The
  [scheduling](scheduling.md) doc has a LaunchAgent plist to copy if you want
  one.

## Where its files live

```sh
personio-tracker --paths
```

Three files, none of them in the repo:

| File | Holds | If you delete it |
| --- | --- | --- |
| `settings.json` | email, company, employee id, work slots, browser visibility | the app starts on its defaults |
| `session.json` | cookies and `localStorage` from the last successful login | the next run logs in again |
| `calendar.json` | what each day of each visited month turned out to be | the grid is blank until the next run |

The directory is named after the **package**, `personio-tracker-tui`, and the
rename of the command did not move it — an existing set is found unchanged.

- macOS: everything under `~/Library/Application Support/com.personiotrack.personio-tracker-tui/`
- Linux: XDG, so `~/.config/…` for the settings and `~/.local/share/…` for the
  session and the day record

## Verify

```sh
personio-tracker --version
personio-tracker --paths      # resolves the same from any directory
```

- [ ] the command runs from a directory other than the repo
- [ ] `--paths` names a Chromium binary rather than "not found"
- [ ] only one `personio-tracker` on your PATH (`which -a personio-tracker`)

## Next

[Configuration](configuration.md) — the three fields it cannot run without.
