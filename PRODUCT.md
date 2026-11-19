# HiyoDyna

A native Dynamic Island-style clock overlay for Windows, written in Rust and drawn with Win32 and GDI.

## Platform

Windows 10/11 x64 desktop. No web runtime, no installer, and no admin rights for the release.

## Who it is for

Windows users who want a compact clock at the top of the screen without another browser process. Developers evaluating whether a native overlay can replace an Electron or webview widget.

## Jobs

- Glance at the time, date, and seconds without leaving the current app.
- Tune motion, bounce, accent, clock format, seconds, position, and shadow.
- Install from a portable zip and start it without an installer.
- Trust that an update came from GitHub and matches its SHA-256.
- Remove it cleanly by deleting a folder.

## Differentiators

- Under 2 MB, one native process, no webview or Electron.
- A real spring animation, three accents, and a tray menu instead of a settings window.
- AGPL-3.0 source, portable release, optional Start with Windows.
- Build and Stable update channels.

## Constraints

- Windows 10/11 x64 only; the overlay uses the primary display.
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
