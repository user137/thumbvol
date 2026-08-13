# thumbvol

*[Українська версія](README.uk.md)*

Lightweight thumb-wheel-to-volume daemon for Logitech MX Master mice (Windows & Linux) —
a tiny replacement for Logi Options/Options+ that does exactly one thing.

## Why

Logi Options/Options+ is heavy, buggy, and leaks memory — and that whole bundle exists for the
sake of one thing: the thumb wheel turning your volume up and down. `thumbvol` does only that.
Install it and forget about it: it autostarts with the OS, no background telemetry.

## What it does

- Side (thumb) wheel → system volume, with the native OS/DE volume OSD.
- Autostart with the OS (can be turned off in the config).
- **Windows:** the wheel event is swallowed, so it does *not* also side-scroll the focused
  window. A tray icon provides:
  - **Sensitivity** (submenu, 1–5) and **Invert direction** (checkbox) — applied immediately and
    written back to the config file, so they survive a restart;
  - **Reload** — re-read the config file from disk on demand;
  - **About**, **Exit**.
- **Linux:** the wheel event is **not** swallowed (it still behaves as a normal horizontal
  scroll, on top of changing the volume) — a deliberate v1 simplification, reasoning in
  [DECISIONS.md](DECISIONS.md) (D1). No tray yet — config file only (D8).

Everything else (DPI switching, gestures, per-app profiles, other buttons) is deliberately out of
scope for v1 — see [TASKS.md](TASKS.md).

## Install from a package (recommended)

Prebuilt packages — an MSI (Windows) and a `.deb` (Linux) — are published on the
[Releases](https://github.com/user137/thumbvol/releases) page for every tagged release.

- **Windows:** run the `.msi`. It installs per-user (no admin rights, no UAC prompt) into
  `%LocalAppData%\thumbvol\`. The final screen has a "Launch thumbvol" checkbox that starts it
  right away (the first run registers autostart itself).
- **Linux:** `sudo dpkg -i thumbvol_*.deb` (or `apt install ./thumbvol_*.deb` to pull in
  dependencies automatically). Installs to `/usr/bin/thumbvol`; `/dev/input`/`/dev/uinput`
  permissions are as described below.

**Before removing the package**, run `thumbvol --uninstall-autostart` — uninstalling (either the
MSI or `dpkg -r`) only removes the files, not the autostart entry: that's written by the app
itself on first run (`HKCU\...\Run` / `~/.config/autostart/thumbvol.desktop`), and the installer
has no record of it. Skip this and the OS will silently try to launch a binary that's no longer
there on the next login.

## Install from source

Requires [Rust](https://rustup.rs/) (stable).

```sh
git clone <this repo's URL>
cd thumbvol
cargo build --release
```

The binary is `target/release/thumbvol` (`.exe` on Windows, ~500 KB, no runtime dependencies).
Move it to a permanent location
(e.g. `%LOCALAPPDATA%\thumbvol\` on Windows or `~/.local/bin/` on Linux) **before** the first
run — autostart registers the exe's current path, and moving or deleting the file afterwards
silently breaks the autostart entry.

The first run registers autostart itself (if `general.autostart = true`, the default) and starts
listening to the thumb wheel. No console window on Windows.

### Linux: device permissions

You need read access to `/dev/input/eventN` (the mouse) and write access to `/dev/uinput`
(emulating the volume keys) — typically via the `input`/`uinput` groups:

```sh
sudo usermod -aG input,uinput "$USER"
# log out and back in for group membership to take effect
```

If device auto-detection (the first device reporting `REL_HWHEEL`) picks the wrong mouse, set
the path explicitly in the config (`[linux] device_path`, see below).

## Config (optional)

Copy [`config.example.toml`](config.example.toml), which documents every field. Location:

- Windows: `%APPDATA%\thumbvol\config.toml`
- Linux: `$XDG_CONFIG_HOME/thumbvol/config.toml` (typically `~/.config/thumbvol/config.toml`)

Without a file: defaults are sensitivity `2`, no inversion, autostart on.

Editing the config file by hand takes effect after a process restart, or after **Reload** from
the tray menu (Windows); on Linux, only a restart (no tray, no hot-reload). Invert and
Sensitivity changed from the Windows tray menu apply instantly, no Reload needed.

**Heads up:** changing Invert or Sensitivity from the tray menu rewrites the whole config file —
any comments (including ones copied from `config.example.toml`) are lost, only the values remain.
Keep your own copy of `config.example.toml` around as a reference if the comments matter to you.

Turn off autostart without touching the config file:

```sh
thumbvol --uninstall-autostart
```

## Development

```sh
cargo test --lib                           # unit tests
cargo clippy --all-targets -- -D warnings  # static analysis, must be clean
```

Architecture and non-obvious details: [CLAUDE.md](CLAUDE.md). Decisions and rejected
alternatives: [DECISIONS.md](DECISIONS.md).

## License

[MIT](LICENSE)
