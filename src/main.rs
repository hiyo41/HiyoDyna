#![cfg_attr(all(windows, not(debug_assertions)), windows_subsystem = "windows")]

#[cfg(windows)]
mod accessibility;
#[cfg(any(windows, test))]
mod animation;
#[cfg(any(windows, test))]
mod autostart;
#[cfg(windows)]
mod canvas;
#[cfg(any(windows, test))]
mod clock;
#[cfg(windows)]
mod compact;
mod compact_glow;
#[cfg(windows)]
mod dark_scrollbar;
#[cfg(windows)]
mod fullscreen;
#[cfg(any(windows, test))]
mod geometry;
mod hotkey;
#[cfg(any(windows, test))]
mod hover;
mod i18n;
#[cfg(windows)]
mod island_surface;
mod lyric_provider;
#[cfg(windows)]
mod lyric_search;
mod lyrics;
#[cfg(windows)]
mod media;
#[cfg(windows)]
mod native_ui;
#[cfg(any(windows, test))]
mod net;
#[cfg(windows)]
mod player;
#[cfg(windows)]
mod preferences;
mod presentation;
#[cfg(windows)]
mod quick_menu;
#[cfg(any(windows, test))]
mod render;
#[cfg(any(windows, test))]
mod settings;
#[cfg(any(windows, test))]
mod sha256;
#[cfg(any(windows, test))]
mod text;
mod theme;
#[cfg(any(windows, test))]
mod tray;
#[cfg(any(windows, test))]
mod tray_icon;
#[cfg(any(windows, test))]
mod update;
#[cfg(windows)]
mod window;
#[cfg(windows)]
mod window_chrome;

#[cfg(windows)]
fn main() {
    if update::is_apply_helper_invocation() {
        update::run_apply_helper();
        return;
    }
    dark_scrollbar::initialize();
    if let Err(error) = window::run() {
        eprintln!("HiyoDyna could not start: {error}");
        native_ui::notice_formatted(
            std::ptr::null_mut(),
            "HiyoDyna",
            "HiyoDyna could not start: {error}",
            &[("error", &error)],
        );
        native_ui::wait_for_notices();
        std::process::exit(1);
    }
}

#[cfg(all(not(windows), not(test)))]
compile_error!("HiyoDyna is a Windows-only application.");

#[cfg(all(not(windows), test))]
fn main() {}
