# HiyoDyna

A native Dynamic Island-style clock overlay for Windows, written in Rust and drawn with Win32 and GDI.

## Platform

Windows 10/11 x64 desktop. No web runtime. The release is portable, needs no admin rights, and an optional source setup installer is available for developers.

## Who it is for

Windows users who want a compact clock, media controls, and synchronized lyrics at the top of the screen without another browser process. Developers evaluating whether a native overlay can replace an Electron or webview widget.

## Jobs

- Glance at the time, date, and seconds without leaving the current app.
- Control playback and read synchronized lyrics without leaving the current app.
- Tune motion, bounce, accent, clock format, seconds, position, shadow, lyrics mode, and timing offset.
- Install from a portable executable, or build and install with the optional source setup.
- Trust that an update came from GitHub and matches its SHA-256.
- Remove the app cleanly by deleting one executable.

## Differentiators

- Under 2 MB, one native process, no webview or Electron.
- A real spring animation, three accents, a native Settings window with categories, and a Find lyrics window.
- Synchronized lyrics from AMLL TTML DB, LRCLIB, and NetEase, with no account or API key.
- AGPL-3.0 source, portable release, optional source setup, optional Start with Windows.
- Build and Stable update channels.

## Constraints

- Windows 10/11 x64 only; the overlay uses the primary display. Media integration needs Windows 10 version 1809 or later.
- The executable is unsigned, so SmartScreen may appear.
- Current downloads are pre-releases until the first official release.
- The site is written in English; `README_vn.md` is the Vietnamese translation.
- "Dynamic Island" is Apple's term; describe the app as Dynamic Island-style and state that it is not affiliated with Apple.

## Website

- Surface: product landing page. Mode: Persuade.
- Primary action: download the current Windows build.
- Secondary actions: inspect the source, understand the native architecture, try the settings.
- Success: a visitor understands the product in one screen and reaches a verified release without installing anything else.
- Non-goals: docs site, blog, CMS, accounts, analytics, newsletter, custom domain, Vietnamese translation of the page.
