# HiyoDyna

**English** | [Tiếng Việt](README_vn.md)

[![CI](https://github.com/hiyo41/HiyoDyna/actions/workflows/ci.yml/badge.svg)](https://github.com/hiyo41/HiyoDyna/actions/workflows/ci.yml)

A native Rust Dynamic Island-style overlay for Windows. The current build provides a live clock capsule that expands on hover, with more system features to come.

## Current features

- Spring-animated capsule that expands on hover and collapses after a short delay
- Live clock with 24-hour or 12-hour format, smaller seconds, and an accent indicator
- Bundled Inter typography, squircle corners, soft shadow, and a hover glow
- Expanded panel adds the weekday and date above the larger time
- Native Win32 + software rendering — no webview, HTML, or JavaScript
- Tray icon and right-click menu for Motion, Bounce, Shadow, Accent, Seconds, Position, and Clock
- Optional Start with Windows, plus restart, settings folder, and about actions
- Settings persist to `%APPDATA%\HiyoDyna\settings.ini`
- Flush to the top edge by default, centered, absent from the taskbar, and never activates when clicked

## Requirements

- Windows 10 or 11
- Rust stable with edition 2024 support for building and for the auto-update watcher

## Install

```powershell
.\scripts\setup.ps1
```

The per-user install goes to `%LOCALAPPDATA%\Programs\HiyoDyna`. Setup also creates a Start Menu shortcut, an uninstall entry, and a hidden watcher in the Startup folder. Use `-NoAutostart` or `-NoWatcher` to skip either one.

This installer is for a local checkout and needs Rust to rebuild. Release downloads run the packaged executable without the local watcher.

## Auto-update

The watcher fingerprints `src\**`, `assets\**`, `Cargo.toml`, and `Cargo.lock`. When they change, it rebuilds the release binary and restarts the installed app. Changes made while it was off are picked up on its next start.

- Log: `%LOCALAPPDATA%\HiyoDyna\auto-update.log`
- Pause: create `%LOCALAPPDATA%\HiyoDyna\pause`; delete it to resume
- A failed build leaves the installed app running and waits for the next edit

## Uninstall

Use **Settings > Apps** or run:

```powershell
%LOCALAPPDATA%\Programs\HiyoDyna\uninstall.ps1
```

Add `-KeepSettings` to keep `%APPDATA%\HiyoDyna` and `%LOCALAPPDATA%\HiyoDyna`.

## Run

```powershell
cargo run --release
```

Hover over the capsule to expand the clock panel; move the pointer away and it collapses after a short delay. Left-click does not toggle the panel. Right-click the capsule or the tray icon for Motion, Bounce, Shadow, Accent, Seconds, Position, and Clock presets; the same menu has Restart, Open settings folder, About, and Exit.

The Position preset can add an 8 or 16 px gap below the top edge. The clock uses 24-hour time by default and can be switched to 12-hour time from the menu.

## Build

```powershell
cargo build --release
```

The executable is written to `target\release\hiyodyna.exe`.

## Releases

Download builds from the [Releases page](https://github.com/hiyo41/HiyoDyna/releases). Each zip contains the executable, both READMEs, and the license.

Pre-releases are test builds from `main`. Official releases are stable and include a full changelog.

Found a problem? [Open an issue](https://github.com/hiyo41/HiyoDyna/issues).

## Development

```powershell
.\scripts\check.ps1
```

CI runs the same checks on `windows-latest`.

## Contributing

Contributions are welcome. See [CONTRIBUTING.md](CONTRIBUTING.md).

## License

GNU Affero General Public License v3.0 — see [LICENSE](LICENSE).

Bundled Inter fonts are licensed under the SIL Open Font License; see [assets/fonts/OFL.txt](assets/fonts/OFL.txt).
