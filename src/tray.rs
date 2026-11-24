use crate::clock::ClockFormat;
use crate::settings::{
    AccentPreset, BouncePreset, DockAnchor, LyricsMode, MotionPreset, UpdateChannel,
};
use crate::tray_icon::TrayIcon;
use std::mem::size_of;
use windows_sys::Win32::Foundation::HWND;
use windows_sys::Win32::UI::Shell::{
    NIF_ICON, NIF_INFO, NIF_MESSAGE, NIF_TIP, NIIF_INFO, NIIF_WARNING, NIM_ADD, NIM_DELETE,
    NIM_MODIFY, NOTIFYICONDATAW, Shell_NotifyIconW,
};
use windows_sys::Win32::UI::WindowsAndMessaging::WM_APP;

pub(crate) const TRAY_CALLBACK_MESSAGE: u32 = WM_APP + 2;

#[derive(Clone, Copy, Debug, PartialEq)]
pub(crate) enum TrayCommand {
    Settings,
    Motion(MotionPreset),
    Bounce(BouncePreset),
    Shadow(bool),
    TopGap(f32),
    Dock(DockAnchor),
    TogglePassthrough,
    Clock(ClockFormat),
    Accent(AccentPreset),
    ShowSeconds(bool),
    UnlimitedFps(bool),
    Lyrics(LyricsMode),
    LyricOffset(i32),
    CompactLyrics(bool),
    Language(crate::i18n::Language),
    Shortcut(crate::hotkey::Shortcut),
    ClearLyricsCache,
    Update(UpdateChannel),
    ToggleAutostart,
    Restart,
    OpenSettingsFolder,
    About,
    Reset,
    Exit,
}

pub(crate) struct Tray {
    data: NOTIFYICONDATAW,
    _icon: TrayIcon,
}
impl Tray {
    pub fn create(hwnd: HWND, accent: AccentPreset) -> Result<Self, String> {
        let icon = TrayIcon::create(32, accent.bgra())
            .or_else(TrayIcon::system)
            .ok_or("could not create the tray icon")?;
        let mut data: NOTIFYICONDATAW = unsafe { std::mem::zeroed() };
        data.cbSize = size_of::<NOTIFYICONDATAW>() as u32;
        data.hWnd = hwnd;
        data.uID = 1;
        data.uFlags = NIF_ICON | NIF_MESSAGE | NIF_TIP;
        data.uCallbackMessage = TRAY_CALLBACK_MESSAGE;
        data.hIcon = icon.handle();
        let tip: Vec<u16> = "HiyoDyna".encode_utf16().chain(Some(0)).collect();
        data.szTip[..tip.len()].copy_from_slice(&tip);
        if unsafe { Shell_NotifyIconW(NIM_ADD, &data) } == 0 {
            return Err("Shell_NotifyIconW(NIM_ADD) failed".into());
        }
        Ok(Self { data, _icon: icon })
    }
    pub fn set_accent(&mut self, accent: AccentPreset) {
        if let Some(icon) = TrayIcon::create(32, accent.bgra()) {
            self.data.hIcon = icon.handle();
            self._icon = icon;
            unsafe {
                Shell_NotifyIconW(NIM_MODIFY, &self.data);
            }
        }
    }

    pub(crate) fn notify(&mut self, title: &str, text: &str, warning: bool) {
        self.data.uFlags = NIF_INFO;
        self.data.dwInfoFlags = if warning { NIIF_WARNING } else { NIIF_INFO };
        copy_wide(&mut self.data.szInfoTitle, title);
        copy_wide(&mut self.data.szInfo, text);
        unsafe {
            Shell_NotifyIconW(NIM_MODIFY, &self.data);
        }
        self.data.uFlags = NIF_ICON | NIF_MESSAGE | NIF_TIP;
    }
}
impl Drop for Tray {
    fn drop(&mut self) {
        unsafe {
            Shell_NotifyIconW(NIM_DELETE, &self.data);
        }
    }
}

fn copy_wide<const N: usize>(target: &mut [u16; N], text: &str) {
    target.fill(0);
    // Reserve the last slot for the NUL terminator: text that exactly fills
    // the buffer would otherwise leave the field unterminated.
    for (slot, value) in target[..N - 1].iter_mut().zip(text.encode_utf16()) {
        *slot = value;
    }
}
