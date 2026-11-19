# Contributing

Thanks for helping improve HiyoDyna. Bug reports, feature ideas, documentation fixes, and code changes are welcome.

## Before you start

- Search existing issues and pull requests.
- Open an issue before large changes so the approach can be discussed.
- Keep each pull request focused on one concern.

## Reporting bugs

Include the Windows version, steps to reproduce, expected and actual behavior, and any logs or screenshots. Settings live in `%APPDATA%\HiyoDyna\settings.ini`; mention non-default values that affect the bug.

## Development

Requirements and build commands are in the [README](README.md). Run `.\scripts\check.ps1` before committing; CI runs the same formatting, Clippy, and build checks on Windows.

The app is Windows-only: Win32 + GDI, with no webview or JavaScript. Keep changes native and avoid new dependencies unless they are clearly justified.

## Code map

| Path | Responsibility |
| --- | --- |
| `src/main.rs` | Module wiring and fatal-error dialog |
| `src/window.rs` | Window creation, message loop, placement, animation timer, hover plumbing |
| `src/animation.rs` | Spring animation, hover spring, and staged reveal |
| `src/geometry.rs` | Continuous-corner rounded-rectangle geometry |
| `src/render.rs` | Software rendering, shadow, background, hover glow |
| `src/text.rs` | Bundled Inter loading, supersampled text masks, blending |
| `src/clock.rs` | Local time snapshot and formatting |
| `src/hover.rs` | Pointer checks and hover timing |
| `src/settings.rs` | INI settings, accent, seconds, and presets |
| `src/tray.rs` | Tray icon and context menus |
| `src/tray_icon.rs` | Custom tray icon pixels and HICON lifecycle |
| `src/autostart.rs` | HKCU Run-key autostart |
| `scripts/` | Per-user setup and uninstaller |

## Pull requests

- Explain the problem, the fix, and how you verified it.
- Add or update unit tests for platform-neutral logic when practical; UI changes need manual verification on Windows.
- Update documentation when behavior, settings, or defaults change.
- Use clear, English commit messages. Vietnamese is welcome in issues and pull requests with full diacritics.

## For AI agents

- Read [AGENTS.md](AGENTS.md) before changing code.
- Run `.\scripts\check.ps1`; do not report success without its output.
- Keep diffs minimal and preserve existing behavior unless the task asks otherwise.
- Verify UI changes on Windows; never commit build output, local settings, or private notes.

## Maintainers

Release steps are in [RELEASING.md](RELEASING.md).

## License

By contributing, you agree that your changes are licensed under the [GNU AGPL v3.0](LICENSE).
