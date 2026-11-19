# HiyoDyna

**English** | [Tiếng Việt](README_vn.md)

[![CI](https://github.com/hiyo41/HiyoDyna/actions/workflows/ci.yml/badge.svg)](https://github.com/hiyo41/HiyoDyna/actions/workflows/ci.yml)

A native Rust Dynamic Island-style overlay for Windows. The current build provides a live clock capsule that expands on hover, with more system features to come.

## Current features

- Spring-animated capsule that expands on hover and collapses after a short delay
- Live clock with 24-hour or 12-hour format, smaller seconds, and a blue indicator
- Expanded panel adds the weekday and date above the larger time
- Native Win32 + software rendering — no webview, HTML, or JavaScript
- Tray icon and right-click menu for Motion, Bounce, Shadow, Position, and Clock presets
- Settings persist to `%APPDATA%\HiyoDyna\settings.ini`
- Flush to the top edge by default, centered, absent from the taskbar, and never activates when clicked

## Requirements

- Windows 10 or 11
- Rust stable with edition 2024 support

## Run

```powershell
cargo run --release
```

Hover over the capsule to expand the clock panel; move the pointer away and it collapses after a short delay. Left-click does not toggle the panel. Right-click the capsule or the tray icon for Motion, Bounce, Shadow, Position, and Clock presets; choose Exit in that menu to quit.

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
