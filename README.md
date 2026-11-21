# HiyoDyna

**English** | [Tiếng Việt](README_vn.md)

[![CI](https://github.com/hiyo41/HiyoDyna/actions/workflows/ci.yml/badge.svg)](https://github.com/hiyo41/HiyoDyna/actions/workflows/ci.yml)

A native Rust Dynamic Island-style overlay for Windows with a live clock, media controls, and synchronized lyrics.

## Current features

- Spring-animated capsule that expands on hover and collapses after a short delay
- Live clock with 24-hour or 12-hour format, smaller seconds, and an accent indicator
- Bundled Inter typography, concave shoulders at the screen edge, rounded lower corners, and a soft shadow
- Expanded panel adds the weekday and date above the larger time
- Native Win32 + software rendering — no webview, HTML, or JavaScript
- Tray icon and right-click menu for Motion, Bounce, Shadow, Accent, Seconds, Position, Clock, and Update
- In-app updater with Build and Stable channels, SHA-256 verification, and startup plus 30-minute checks
- Resizable native Settings and lyric-search windows with shared custom chrome, plus live English/Vietnamese switching
- Scroll from the expanded clock to a player with artwork, source selection, seeking, and synchronized lyrics
- Optional Start with Windows, plus restart, settings folder, and about actions
- Settings persist to `%APPDATA%\HiyoDyna\settings.ini`
- Compact line lyrics are white, with a soft lower glow sampled from the cover; resizing stays centered by default, with optional left and right docking
- Configurable Ctrl+Alt+D shortcut temporarily fades the Island and lets clicks pass through
- Persistent compact-lyrics toggle, preserved manual page choice, and a Clock return after ten continuous seconds without playback
 - Manual published-release checks in Settings alongside automatic staged download and installation
- Retracts automatically over another app's fullscreen window and returns when fullscreen ends
- Flush to the top edge by default, absent from the taskbar, and never activates when clicked

## Requirements

- Windows 10 or 11; media integration requires Windows 10 version 1809 or later
- Rust stable with edition 2024 support when building from source

## Install

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

Use **Settings > Apps** or run:

```powershell
powershell.exe -NoProfile -ExecutionPolicy Bypass -File "$env:LOCALAPPDATA\Programs\HiyoDyna\uninstall.ps1"
```

Add `-KeepSettings` to keep `%APPDATA%\HiyoDyna` and `%LOCALAPPDATA%\HiyoDyna`.

## Run

```powershell
cargo run --release
```

Hover over the capsule to expand the clock. Scroll down/up or use the two dots at the right center to switch between clock and player; the active page's dot is brighter. Play music in Spotify desktop or another app that publishes a Windows media session. HiyoDyna prefers Spotify automatically when its session is available; a manual source choice stays selected until that source disappears. No Spotify login is required. Dragging the white seek thumb previews the position immediately and sends the final seek when released. Moving away collapses the Island after a short delay; dragging keeps it open. The collapsed capsule displays the current timed lyric and adapts its width to the text, with a clock fallback when timing is unavailable. Before the next lyric starts, it shows the upcoming line in muted text, retaining the note icon and cover glow. A baseline-aligned countdown appears only while the wait exceeds three seconds.

Right-click the Island for quick actions: clock, player, resume lyrics, find lyrics, fade/click-through, Settings, and exit. The tray also provides Settings and interaction recovery. Click the expanded gear to open Settings directly. The Settings window can be moved, resized, minimized, and left open while using other apps; changes apply immediately. Tab/Shift+Tab reveals each control, Enter/Space activates, Page Up/Down scrolls, and Escape closes the window.

Settings offers left, center, or right docking and an optional 8 or 16 px top gap. Ctrl+Alt+D is the default shortcut for temporary fading and click-through; capture and apply another combination in Settings. A conflict preserves the previous binding and shows an explanation. Use the tray's **Restore Island interaction** action if the shortcut is unavailable. Fullscreen apps covering the Island's monitor automatically retract and fade it; leaving fullscreen restores it with the selected page and click-through preference retained. Ordinary maximized windows keep it visible. The clock defaults to 24-hour time and also supports 12-hour time.

The Island starts on Clock and can select usable synchronized lyrics automatically until you choose a page manually. Collapse and Escape preserve that choice. After ten continuous seconds without playback it returns to Clock; resuming sooner cancels the timer. Compact lyrics default to enabled and can be changed in Settings or the expanded player. When disabled or unavailable, the compact view shows a clock with the active source icon. Settings also offers System, English, or Vietnamese, applied immediately to open application windows. **Check for updates** checks the selected Build/Stable channel off the UI thread; it reports published releases and opens a release page only when requested. It does not download, install, or replace the app.

Lyrics come from the free public [AMLL TTML DB](https://github.com/amll-dev/amll-ttml-db), [LRCLIB](https://lrclib.net), and [NetEase](https://music.163.com), with contributor credits when provided. No account or API key is required. Lyrics require genuine timestamps: word timing takes priority, with line timing as the fallback. Untimed text is not applied; instrumental recordings display a status. Coverage and service availability vary. Unsupported media timelines disable synchronized following; playback controls remain available. Scroll over the lyric area, use Ctrl+wheel, or press Up/Down to browse lyrics; visible lines keep their live playback highlighting. Choose **Resume lyrics** to follow again, or click a timed line to seek when supported. Wheel input outside the lyric area switches clock/player pages. Compact lyrics display the complete sentence without ellipsis; the capsule grows within the display width and reduces the text size when necessary to fit. Pause indicators follow playback only during long gaps before an upcoming line; completed lyrics retain their final position. The source name opens the scrollable session picker. Timing offset and mode are in Settings.

Choose **Find lyrics** in the player footer or Island quick menu to search by an editable title and artist. The native search window lists recordings with their provider, duration, and available timing: **Words**, **Lines**, or **Unknown** until fetched. Known untimed results are filtered, and a fetched recording must contain synchronized lyrics before it can be applied. Select a recording and choose **Use selected lyrics**. Your choice stays active until the track changes or lyrics are explicitly refreshed; changing the query or track cancels obsolete requests. Searching runs in the background and leaves media controls available. Manual choices apply to the current playback session and are not saved to the automatic-match cache.

Lyrics cache lives in `%LOCALAPPDATA%\HiyoDyna\lyrics`, bounded to 128 entries. Successful matches expire after seven days; misses after five minutes. **Clear lyrics cache** removes only this cache. The visual treatment is inspired by [Spicy Lyrics](https://github.com/Spikerko/spicy-lyrics); HiyoDyna has no Spicetify or companion service dependency.

The Island exposes named controls and a seek slider through Windows UI Automation. When focused through accessibility tools, Tab moves between controls, Enter/Space activates, Page Up/Down switches pages, Up/Down browses lyrics or sources, Left/Right seeks five seconds when supported, and Escape returns to the capsule.

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
