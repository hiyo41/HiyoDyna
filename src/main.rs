#![cfg_attr(all(windows, not(debug_assertions)), windows_subsystem = "windows")]

#[cfg(any(windows, test))]
mod animation;
#[cfg(any(windows, test))]
mod autostart;
#[cfg(any(windows, test))]
mod clock;
#[cfg(any(windows, test))]
mod geometry;
#[cfg(any(windows, test))]
mod hover;
#[cfg(any(windows, test))]
mod net;
#[cfg(any(windows, test))]
mod render;
#[cfg(any(windows, test))]
mod settings;
#[cfg(any(windows, test))]
mod sha256;
#[cfg(any(windows, test))]
mod text;
#[cfg(any(windows, test))]
mod tray;
#[cfg(any(windows, test))]
mod tray_icon;
#[cfg(windows)]
mod window;

#[cfg(windows)]
fn main() {
    if let Err(error) = window::run() {
        eprintln!("HiyoDyna could not start: {error}");
        let message: Vec<u16> = format!("HiyoDyna could not start: {error}")
            .encode_utf16()
            .chain(Some(0))
            .collect();
        let title: Vec<u16> = "HiyoDyna".encode_utf16().chain(Some(0)).collect();
        unsafe {
            windows_sys::Win32::UI::WindowsAndMessaging::MessageBoxW(
                std::ptr::null_mut(),
                message.as_ptr(),
                title.as_ptr(),
                windows_sys::Win32::UI::WindowsAndMessaging::MB_OK
                    | windows_sys::Win32::UI::WindowsAndMessaging::MB_ICONERROR,
            );
        }
        std::process::exit(1);
    }
}

#[cfg(all(not(windows), not(test)))]
compile_error!("HiyoDyna is a Windows-only application.");

#[cfg(all(not(windows), test))]
fn main() {}
