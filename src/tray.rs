use std::mem::size_of;
use std::ptr::null_mut;

use windows_sys::Win32::Foundation::{HWND, POINT};
use windows_sys::Win32::UI::Shell::{
    NIF_ICON, NIF_MESSAGE, NIF_TIP, NIM_ADD, NIM_DELETE, NIM_MODIFY, NOTIFYICONDATAW,
    Shell_NotifyIconW,
};
use windows_sys::Win32::UI::WindowsAndMessaging::{
    AppendMenuW, CreatePopupMenu, DestroyMenu, GetCursorPos, MF_CHECKED, MF_POPUP, MF_SEPARATOR,
    MF_STRING, PostMessageW, SetForegroundWindow, TPM_NONOTIFY, TPM_RETURNCMD, TPM_RIGHTBUTTON,
    TrackPopupMenu, WM_APP,
};

use crate::clock::ClockFormat;
use crate::settings::{AccentPreset, BouncePreset, MotionPreset, Settings};
use crate::tray_icon::TrayIcon;

pub(crate) const TRAY_CALLBACK_MESSAGE: u32 = WM_APP + 2;
const TRAY_ICON_ID: u32 = 1;
const CMD_MOTION_SLOW: usize = 100;
const CMD_MOTION_NORMAL: usize = 101;
const CMD_MOTION_FAST: usize = 102;
const CMD_BOUNCE_LOW: usize = 110;
const CMD_BOUNCE_MEDIUM: usize = 111;
const CMD_BOUNCE_HIGH: usize = 112;
const CMD_SHADOW_OFF: usize = 120;
const CMD_SHADOW_SOFT: usize = 121;
const CMD_TOP_GAP_0: usize = 130;
const CMD_TOP_GAP_8: usize = 131;
const CMD_TOP_GAP_16: usize = 132;
const CMD_CLOCK_24: usize = 150;
const CMD_CLOCK_12: usize = 151;
const CMD_ACCENT_ICE: usize = 160;
const CMD_ACCENT_AMBER: usize = 161;
const CMD_ACCENT_MINT: usize = 162;
const CMD_SECONDS_ON: usize = 170;
const CMD_SECONDS_OFF: usize = 171;
const CMD_AUTOSTART: usize = 180;
const CMD_RESET: usize = 140;
const CMD_EXIT: usize = 141;
const CMD_RESTART: usize = 190;
const CMD_OPEN_SETTINGS: usize = 191;
const CMD_ABOUT: usize = 192;

#[derive(Clone, Copy, Debug, PartialEq)]
pub(crate) enum TrayCommand {
    Motion(MotionPreset),
    Bounce(BouncePreset),
    Shadow(bool),
    TopGap(f32),
    Clock(ClockFormat),
    Accent(AccentPreset),
    ShowSeconds(bool),
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
    pub(crate) fn create(hwnd: HWND, accent: AccentPreset) -> Result<Self, String> {
        let icon = TrayIcon::create(32, accent.bgra())
            .or_else(TrayIcon::system)
            .ok_or("could not create the tray icon")?;

        let mut data: NOTIFYICONDATAW = unsafe { std::mem::zeroed() };
        data.cbSize = size_of::<NOTIFYICONDATAW>() as u32;
        data.hWnd = hwnd;
        data.uID = TRAY_ICON_ID;
        data.uFlags = NIF_ICON | NIF_MESSAGE | NIF_TIP;
        data.uCallbackMessage = TRAY_CALLBACK_MESSAGE;
        data.hIcon = icon.handle();
        let tip: Vec<u16> = "HiyoDyna".encode_utf16().chain(Some(0)).collect();
        let tip_len = tip.len().min(data.szTip.len());
        data.szTip[..tip_len].copy_from_slice(&tip[..tip_len]);

        if unsafe { Shell_NotifyIconW(NIM_ADD, &data) } == 0 {
            return Err("Shell_NotifyIconW(NIM_ADD) failed".to_string());
        }

        Ok(Self { data, _icon: icon })
    }

    pub(crate) fn set_accent(&mut self, accent: AccentPreset) {
        let Some(icon) = TrayIcon::create(32, accent.bgra()) else {
            return;
        };
        self.data.hIcon = icon.handle();
        self._icon = icon;
        unsafe {
            Shell_NotifyIconW(NIM_MODIFY, &self.data);
        }
    }

    pub(crate) fn show_menu(&self, settings: &Settings) -> Option<TrayCommand> {
        let menu = unsafe { CreatePopupMenu() };
        if menu.is_null() {
            return None;
        }

        let motion = unsafe { CreatePopupMenu() };
        let bounce = unsafe { CreatePopupMenu() };
        let shadow = unsafe { CreatePopupMenu() };
        let position = unsafe { CreatePopupMenu() };
        let clock = unsafe { CreatePopupMenu() };
        let accent = unsafe { CreatePopupMenu() };
        let seconds = unsafe { CreatePopupMenu() };
        if motion.is_null()
            || bounce.is_null()
            || shadow.is_null()
            || position.is_null()
            || clock.is_null()
            || accent.is_null()
            || seconds.is_null()
        {
            unsafe {
                DestroyMenu(menu);
                if !motion.is_null() {
                    DestroyMenu(motion);
                }
                if !bounce.is_null() {
                    DestroyMenu(bounce);
                }
                if !shadow.is_null() {
                    DestroyMenu(shadow);
                }
                if !position.is_null() {
                    DestroyMenu(position);
                }
                if !clock.is_null() {
                    DestroyMenu(clock);
                }
                if !accent.is_null() {
                    DestroyMenu(accent);
                }
                if !seconds.is_null() {
                    DestroyMenu(seconds);
                }
            }
            return None;
        }

        append_checked(
            motion,
            CMD_MOTION_SLOW,
            "Slow",
            settings.motion == MotionPreset::Slow,
        );
        append_checked(
            motion,
            CMD_MOTION_NORMAL,
            "Normal",
            settings.motion == MotionPreset::Normal,
        );
        append_checked(
            motion,
            CMD_MOTION_FAST,
            "Fast",
            settings.motion == MotionPreset::Fast,
        );
        append_checked(
            bounce,
            CMD_BOUNCE_LOW,
            "Low",
            settings.bounce == BouncePreset::Low,
        );
        append_checked(
            bounce,
            CMD_BOUNCE_MEDIUM,
            "Medium",
            settings.bounce == BouncePreset::Medium,
        );
        append_checked(
            bounce,
            CMD_BOUNCE_HIGH,
            "High",
            settings.bounce == BouncePreset::High,
        );
        append_checked(shadow, CMD_SHADOW_OFF, "Off", !settings.shadow);
        append_checked(shadow, CMD_SHADOW_SOFT, "Soft", settings.shadow);
        append_checked(position, CMD_TOP_GAP_0, "Top edge", settings.top_gap == 0.0);
        append_checked(position, CMD_TOP_GAP_8, "8 px gap", settings.top_gap == 8.0);
        append_checked(
            position,
            CMD_TOP_GAP_16,
            "16 px gap",
            settings.top_gap == 16.0,
        );
        append_checked(
            clock,
            CMD_CLOCK_24,
            "24-hour",
            settings.clock == ClockFormat::TwentyFourHour,
        );
        append_checked(
            clock,
            CMD_CLOCK_12,
            "12-hour",
            settings.clock == ClockFormat::TwelveHour,
        );
        append_checked(
            accent,
            CMD_ACCENT_ICE,
            AccentPreset::Ice.label(),
            settings.accent == AccentPreset::Ice,
        );
        append_checked(
            accent,
            CMD_ACCENT_AMBER,
            AccentPreset::Amber.label(),
            settings.accent == AccentPreset::Amber,
        );
        append_checked(
            accent,
            CMD_ACCENT_MINT,
            AccentPreset::Mint.label(),
            settings.accent == AccentPreset::Mint,
        );
        append_checked(seconds, CMD_SECONDS_ON, "On", settings.show_seconds);
        append_checked(seconds, CMD_SECONDS_OFF, "Off", !settings.show_seconds);

        append_submenu(menu, motion, "Motion");
        append_submenu(menu, bounce, "Bounce");
        append_submenu(menu, shadow, "Shadow");
        append_submenu(menu, accent, "Accent");
        append_submenu(menu, seconds, "Seconds");
        append_submenu(menu, position, "Position");
        append_submenu(menu, clock, "Clock");
        append_item(menu, MF_SEPARATOR, 0, "");
        append_checked(
            menu,
            CMD_AUTOSTART,
            "Start with Windows",
            crate::autostart::is_enabled(),
        );
        append_item(menu, MF_STRING, CMD_RESTART, "Restart");
        append_item(menu, MF_STRING, CMD_OPEN_SETTINGS, "Open settings folder");
        append_item(menu, MF_STRING, CMD_ABOUT, "About");
        append_item(menu, MF_SEPARATOR, 0, "");
        append_item(menu, MF_STRING, CMD_RESET, "Reset to defaults");
        append_item(menu, MF_STRING, CMD_EXIT, "Exit");

        let mut point = POINT { x: 0, y: 0 };
        unsafe {
            GetCursorPos(&mut point);
            SetForegroundWindow(self.data.hWnd);
        }
        let command = unsafe {
            TrackPopupMenu(
                menu,
                TPM_RIGHTBUTTON | TPM_RETURNCMD | TPM_NONOTIFY,
                point.x,
                point.y,
                0,
                self.data.hWnd,
                null_mut(),
            )
        };
        unsafe {
            DestroyMenu(menu);
            // Docs: a menu owned by a window without activation needs this nudge so it
            // dismisses when the user clicks elsewhere.
            PostMessageW(self.data.hWnd, 0, 0, 0);
        }

        command_from_id(command as usize)
    }
}

impl Drop for Tray {
    fn drop(&mut self) {
        unsafe {
            Shell_NotifyIconW(NIM_DELETE, &self.data);
        }
    }
}

fn command_from_id(id: usize) -> Option<TrayCommand> {
    match id {
        CMD_MOTION_SLOW => Some(TrayCommand::Motion(MotionPreset::Slow)),
        CMD_MOTION_NORMAL => Some(TrayCommand::Motion(MotionPreset::Normal)),
        CMD_MOTION_FAST => Some(TrayCommand::Motion(MotionPreset::Fast)),
        CMD_BOUNCE_LOW => Some(TrayCommand::Bounce(BouncePreset::Low)),
        CMD_BOUNCE_MEDIUM => Some(TrayCommand::Bounce(BouncePreset::Medium)),
        CMD_BOUNCE_HIGH => Some(TrayCommand::Bounce(BouncePreset::High)),
        CMD_SHADOW_OFF => Some(TrayCommand::Shadow(false)),
        CMD_SHADOW_SOFT => Some(TrayCommand::Shadow(true)),
        CMD_TOP_GAP_0 => Some(TrayCommand::TopGap(0.0)),
        CMD_TOP_GAP_8 => Some(TrayCommand::TopGap(8.0)),
        CMD_TOP_GAP_16 => Some(TrayCommand::TopGap(16.0)),
        CMD_CLOCK_24 => Some(TrayCommand::Clock(ClockFormat::TwentyFourHour)),
        CMD_CLOCK_12 => Some(TrayCommand::Clock(ClockFormat::TwelveHour)),
        CMD_ACCENT_ICE => Some(TrayCommand::Accent(AccentPreset::Ice)),
        CMD_ACCENT_AMBER => Some(TrayCommand::Accent(AccentPreset::Amber)),
        CMD_ACCENT_MINT => Some(TrayCommand::Accent(AccentPreset::Mint)),
        CMD_SECONDS_ON => Some(TrayCommand::ShowSeconds(true)),
        CMD_SECONDS_OFF => Some(TrayCommand::ShowSeconds(false)),
        CMD_AUTOSTART => Some(TrayCommand::ToggleAutostart),
        CMD_RESTART => Some(TrayCommand::Restart),
        CMD_OPEN_SETTINGS => Some(TrayCommand::OpenSettingsFolder),
        CMD_ABOUT => Some(TrayCommand::About),
        CMD_RESET => Some(TrayCommand::Reset),
        CMD_EXIT => Some(TrayCommand::Exit),
        _ => None,
    }
}

fn append_checked(
    menu: windows_sys::Win32::UI::WindowsAndMessaging::HMENU,
    id: usize,
    text: &str,
    checked: bool,
) {
    let flags = if checked {
        MF_STRING | MF_CHECKED
    } else {
        MF_STRING
    };
    append_item(menu, flags, id, text);
}

fn append_submenu(
    menu: windows_sys::Win32::UI::WindowsAndMessaging::HMENU,
    submenu: windows_sys::Win32::UI::WindowsAndMessaging::HMENU,
    text: &str,
) {
    let label: Vec<u16> = text.encode_utf16().chain(Some(0)).collect();
    unsafe {
        AppendMenuW(menu, MF_POPUP, submenu as usize, label.as_ptr());
    }
}

fn append_item(
    menu: windows_sys::Win32::UI::WindowsAndMessaging::HMENU,
    flags: windows_sys::Win32::UI::WindowsAndMessaging::MENU_ITEM_FLAGS,
    id: usize,
    text: &str,
) {
    let label: Vec<u16> = text.encode_utf16().chain(Some(0)).collect();
    let pointer = if flags == MF_SEPARATOR {
        std::ptr::null()
    } else {
        label.as_ptr()
    };
    unsafe {
        AppendMenuW(menu, flags, id, pointer);
    }
}

#[cfg(test)]
mod tests {
    use super::{TrayCommand, command_from_id};
    use crate::clock::ClockFormat;
    use crate::settings::{AccentPreset, BouncePreset, MotionPreset};

    #[test]
    fn maps_motion_menu_ids() {
        assert_eq!(
            command_from_id(100),
            Some(TrayCommand::Motion(MotionPreset::Slow))
        );
        assert_eq!(
            command_from_id(101),
            Some(TrayCommand::Motion(MotionPreset::Normal))
        );
        assert_eq!(
            command_from_id(102),
            Some(TrayCommand::Motion(MotionPreset::Fast))
        );
    }

    #[test]
    fn maps_bounce_and_shadow_menu_ids() {
        assert_eq!(
            command_from_id(110),
            Some(TrayCommand::Bounce(BouncePreset::Low))
        );
        assert_eq!(
            command_from_id(112),
            Some(TrayCommand::Bounce(BouncePreset::High))
        );
        assert_eq!(command_from_id(120), Some(TrayCommand::Shadow(false)));
        assert_eq!(command_from_id(121), Some(TrayCommand::Shadow(true)));
    }

    #[test]
    fn maps_position_reset_and_exit_menu_ids() {
        assert_eq!(command_from_id(130), Some(TrayCommand::TopGap(0.0)));
        assert_eq!(command_from_id(131), Some(TrayCommand::TopGap(8.0)));
        assert_eq!(command_from_id(132), Some(TrayCommand::TopGap(16.0)));
        assert_eq!(
            command_from_id(150),
            Some(TrayCommand::Clock(ClockFormat::TwentyFourHour))
        );
        assert_eq!(
            command_from_id(151),
            Some(TrayCommand::Clock(ClockFormat::TwelveHour))
        );
        assert_eq!(command_from_id(140), Some(TrayCommand::Reset));
        assert_eq!(command_from_id(141), Some(TrayCommand::Exit));
        assert_eq!(command_from_id(999), None);
    }

    #[test]
    fn maps_accent_seconds_and_utility_menu_ids() {
        assert_eq!(
            command_from_id(160),
            Some(TrayCommand::Accent(AccentPreset::Ice))
        );
        assert_eq!(
            command_from_id(161),
            Some(TrayCommand::Accent(AccentPreset::Amber))
        );
        assert_eq!(
            command_from_id(162),
            Some(TrayCommand::Accent(AccentPreset::Mint))
        );
        assert_eq!(command_from_id(170), Some(TrayCommand::ShowSeconds(true)));
        assert_eq!(command_from_id(171), Some(TrayCommand::ShowSeconds(false)));
        assert_eq!(command_from_id(180), Some(TrayCommand::ToggleAutostart));
        assert_eq!(command_from_id(190), Some(TrayCommand::Restart));
        assert_eq!(command_from_id(191), Some(TrayCommand::OpenSettingsFolder));
        assert_eq!(command_from_id(192), Some(TrayCommand::About));
    }
}
