# HiyoDyna

**English** | [Tiếng Việt](README_vn.md)

[![CI](https://github.com/hiyo41/HiyoDyna/actions/workflows/ci.yml/badge.svg)](https://github.com/hiyo41/HiyoDyna/actions/workflows/ci.yml)

A native Rust Dynamic Island-style overlay for Windows. The current build provides a live clock capsule that expands on hover, with more system features to come.

Website: [hiyo41.github.io/HiyoDyna](https://hiyo41.github.io/HiyoDyna/)

## Current features

- Spring-animated capsule that expands on hover and collapses after a short delay
- Live clock with 24-hour or 12-hour format, smaller seconds, and an accent indicator
- Bundled Inter typography, squircle corners, soft shadow, and a hover glow
- Expanded panel adds the weekday and date above the larger time
- Native Win32 + software rendering — no webview, HTML, or JavaScript
- Tray icon and right-click menu for Motion, Bounce, Shadow, Accent, Seconds, Position, Clock, and Update
- In-app updater with Build and Stable channels, SHA-256 verification, and startup plus 30-minute checks
- Optional Start with Windows, plus restart, settings folder, and about actions
- Settings persist to `%APPDATA%\HiyoDyna\settings.ini`
- Flush to the top edge by default, centered, absent from the taskbar, and never activates when clicked

## Requirements

- Windows 10 or 11
- Rust stable with edition 2024 support when building from source

## Install

### Install a release

1. Download `HiyoDyna-<tag>-windows-x64.zip` from the [Releases page](https://github.com/hiyo41/HiyoDyna/releases).
2. Extract it anywhere.
3. Run `HiyoDyna.exe`.

No installer or admin rights. Windows 10/11 x64 only. Settings live in `%APPDATA%\HiyoDyna\settings.ini`.

### Install from source

```powershell
powershell.exe -NoProfile -ExecutionPolicy Bypass -File .\scripts\setup.ps1
```

The per-user install goes to `%LOCALAPPDATA%\Programs\HiyoDyna`. Setup creates Desktop and Start Menu shortcuts, an uninstall entry, and Start with Windows. Use `-NoAutostart` to skip autostart.

This installer builds the checkout locally and needs Rust. Release downloads run the packaged executable and update through the in-app updater.

## Auto-update

HiyoDyna updates itself from GitHub. It checks at startup and every 30 minutes, and the tray **Update** menu can switch channels or check now.

- **Build** follows pre-release builds from `main` (`pre-N`).
- **Stable** follows official releases; until the first official release exists it uses the newest pre-release.
- Downloads are verified with SHA-256 before they are staged and applied.
- Log: `%LOCALAPPDATA%\HiyoDyna\auto-update.log`

The updater runs inside HiyoDyna. It starts a short-lived helper only while replacing the executable; there is no resident watcher and no Startup shortcut for updates.

## Uninstall

### Uninstall a release

Turn off Start with Windows from the tray, exit HiyoDyna, and delete the extracted folder. Delete `%APPDATA%\HiyoDyna` as well to remove your settings.

### Uninstall a source install

Use **Settings > Apps** or run:

```powershell
powershell.exe -NoProfile -ExecutionPolicy Bypass -File "$env:LOCALAPPDATA\Programs\HiyoDyna\uninstall.ps1"
```

Add `-KeepSettings` to keep `%APPDATA%\HiyoDyna` and `%LOCALAPPDATA%\HiyoDyna`.

## Run

```powershell
cargo run --release
```

Hover over the capsule to expand the clock panel; move the pointer away and it collapses after a short delay. Left-click does not toggle the panel. Right-click the capsule or the tray icon for Motion, Bounce, Shadow, Accent, Seconds, Position, Clock, and Update; the same menu has Restart, Open settings folder, About, and Exit.

The Position preset can add an 8 or 16 px gap below the top edge. The clock uses 24-hour time by default and can be switched to 12-hour time from the menu.

## Build

```powershell
cargo build --release
```

The executable is written to `target\release\hiyodyna.exe`.

## Releases

Download builds from the [Releases page](https://github.com/hiyo41/HiyoDyna/releases). Each release includes a zip with the executable, both READMEs, and the license, plus the raw executable and its SHA-256 for the in-app updater.

Pre-releases are test builds from `main` and feed the Build channel. Official releases feed the Stable channel and include a full changelog.

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
