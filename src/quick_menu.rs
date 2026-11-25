use std::cell::Cell;
use std::mem::size_of;
use std::time::{Duration, Instant};
use windows_sys::Win32::Foundation::{HWND, POINT, RECT};
use windows_sys::Win32::UI::Input::KeyboardAndMouse::VK_ESCAPE;
use windows_sys::Win32::UI::WindowsAndMessaging::*;

use crate::i18n::{self, Language};

const FIRST_COMMAND: usize = 0x6001;
/// How long a just-opened modeless menu may take to create its popup window.
const WINDOW_DISCOVERY_GRACE: Duration = Duration::from_millis(250);
const ACTIONS: [Action; 7] = [
    Action::Clock,
    Action::Music,
    Action::ResumeLyrics,
    Action::SearchLyrics,
    Action::TogglePassthrough,
    Action::Settings,
    Action::Exit,
];

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum Action {
    Clock,
    Music,
    ResumeLyrics,
    SearchLyrics,
    TogglePassthrough,
    Settings,
    Exit,
}

/// Keep alive until the owner processes the queued WM_UNINITMENUPOPUP notification.
pub(crate) struct Menu {
    menu: HMENU,
    window: Cell<HWND>,
    shown_at: Instant,
}

impl Menu {
    pub(crate) fn handle(&self) -> HMENU {
        self.menu
    }

    /// The popup window is created by the menu manager after tracking starts.
    /// Resolve it lazily so a slow first frame cannot make the menu look closed.
    fn window(&self) -> Option<HWND> {
        let mut window = self.window.get();
        if window.is_null() {
            window = find_menu_window();
            self.window.set(window);
        }
        (!window.is_null()).then_some(window)
    }

    pub(crate) fn is_open(&self) -> bool {
        self.window().map_or(
            self.shown_at.elapsed() < WINDOW_DISCOVERY_GRACE,
            |window| unsafe { IsWindow(window) != 0 && IsWindowVisible(window) != 0 },
        )
    }

    /// A click inside the popup belongs to the menu; a click anywhere else
    /// must dismiss it. Layered owners such as the Island cannot become the
    /// foreground window, so the menu manager never sees that outside click.
    pub(crate) fn contains(&self, point: POINT) -> bool {
        let Some(window) = self.window() else {
            return false;
        };
        let mut rect = RECT::default();
        unsafe {
            GetWindowRect(window, &mut rect) != 0
                && rect.left <= point.x
                && point.x < rect.right
                && rect.top <= point.y
                && point.y < rect.bottom
        }
    }

    pub(crate) fn dismiss(&self) {
        if let Some(window) = self.window() {
            // A modeless menu closes on Escape. WM_CANCELMODE on the owner is
            // ignored while the menu is modeless.
            unsafe {
                PostMessageW(window, WM_KEYDOWN, VK_ESCAPE as usize, 0);
            }
        }
    }
}

fn find_menu_window() -> HWND {
    let class = wide("#32768");
    let process = std::process::id();
    let mut window: HWND = std::ptr::null_mut();
    loop {
        window = unsafe {
            FindWindowExW(
                std::ptr::null_mut(),
                window,
                class.as_ptr(),
                std::ptr::null(),
            )
        };
        if window.is_null() {
            return window;
        }
        let mut owner = 0_u32;
        unsafe {
            GetWindowThreadProcessId(window, &mut owner);
        }
        if owner == process && unsafe { IsWindowVisible(window) } != 0 {
            return window;
        }
    }
}

fn wide(text: &str) -> Vec<u16> {
    text.encode_utf16().chain(Some(0)).collect()
}

impl Drop for Menu {
    fn drop(&mut self) {
        unsafe {
            DestroyMenu(self.menu);
        }
    }
}

pub(crate) fn action(command: usize) -> Option<Action> {
    command
        .checked_sub(FIRST_COMMAND)
        .and_then(|index| ACTIONS.get(index).copied())
}

fn menu_labels_in(language: Language, passthrough: bool, shortcut: Option<&str>) -> [String; 7] {
    let mut labels = [
        "Clock",
        "Music player",
        "Resume following lyrics",
        "Find lyrics…",
        if passthrough {
            "Restore Island interaction"
        } else {
            "Fade and click through"
        },
        "Settings…",
        "Exit",
    ]
    .map(|label| i18n::tr_in(language, label).to_string());
    if !passthrough && let Some(shortcut) = shortcut.filter(|value| !value.is_empty()) {
        labels[4].push('\t');
        labels[4].push_str(shortcut);
    }
    labels
}

fn create(music: bool, browsing: bool, passthrough: bool, shortcut: Option<&str>) -> Option<Menu> {
    create_in(
        music,
        browsing,
        passthrough,
        shortcut,
        i18n::resolved_language(),
    )
}

fn create_in(
    music: bool,
    browsing: bool,
    passthrough: bool,
    shortcut: Option<&str>,
    language: Language,
) -> Option<Menu> {
    let labels = menu_labels_in(language, passthrough, shortcut);
    unsafe {
        let menu = CreatePopupMenu();
        if menu.is_null() {
            return None;
        }
        let menu = Menu {
            menu,
            window: Cell::new(std::ptr::null_mut()),
            shown_at: Instant::now(),
        };
        // Modeless tracking returns to the application loop. Keep WM_COMMAND
        // delivery so the owner can queue actions without borrowing UI state.
        let info = MENUINFO {
            cbSize: size_of::<MENUINFO>() as u32,
            fMask: MIM_STYLE,
            dwStyle: MNS_MODELESS,
            ..Default::default()
        };
        if SetMenuInfo(menu.handle(), &info) == 0 {
            return None;
        }
        for (i, label) in labels.iter().enumerate() {
            if (i == 4 || i == 6)
                && AppendMenuW(menu.handle(), MF_SEPARATOR, 0, std::ptr::null()) == 0
            {
                return None;
            }
            let checked = (i == 0 && !music) || (i == 1 && music) || (i == 4 && passthrough);
            let flags = MF_STRING
                | if checked { MF_CHECKED } else { 0 }
                | if i == 2 && music && !browsing {
                    MF_GRAYED
                } else {
                    0
                };
            let label: Vec<u16> = label.encode_utf16().chain(Some(0)).collect();
            if AppendMenuW(menu.handle(), flags, FIRST_COMMAND + i, label.as_ptr()) == 0 {
                return None;
            }
        }
        Some(menu)
    }
}

pub(crate) fn show(
    hwnd: HWND,
    music: bool,
    browsing: bool,
    passthrough: bool,
    shortcut: Option<&str>,
) -> Option<Menu> {
    let mut menu = create(music, browsing, passthrough, shortcut)?;
    unsafe {
        let mut cursor = POINT::default();
        if GetCursorPos(&mut cursor) == 0 {
            return None;
        }
        SetForegroundWindow(hwnd);
        if TrackPopupMenuEx(
            menu.handle(),
            TPM_RIGHTBUTTON,
            cursor.x,
            cursor.y,
            hwnd,
            std::ptr::null(),
        ) == 0
        {
            return None;
        }
        PostMessageW(hwnd, WM_NULL, 0, 0);
        menu.shown_at = Instant::now();
        menu.window.set(find_menu_window());
    }
    Some(menu)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn menu_copy_uses_the_selected_language_and_effective_shortcut() {
        let labels = menu_labels_in(Language::Vietnamese, false, Some("Ctrl+Alt+D"));
        assert_eq!(labels[0], "Đồng hồ");
        assert_eq!(labels[1], "Trình phát nhạc");
        assert_eq!(labels[2], "Tiếp tục theo dõi lời đang phát");
        assert_eq!(labels[3], "Tìm lời bài hát…");
        assert_eq!(labels[4], "Làm mờ và nhấp xuyên qua\tCtrl+Alt+D");
        assert_eq!(labels[5], "Cài đặt…");
        assert_eq!(labels[6], "Thoát");
        assert_eq!(
            menu_labels_in(Language::English, false, Some("Ctrl+Shift+F9"))[4],
            "Fade and click through\tCtrl+Shift+F9"
        );
    }

    #[test]
    fn unregistered_shortcuts_are_absent_and_restore_remains_discoverable() {
        assert_eq!(
            menu_labels_in(Language::Vietnamese, false, None)[4],
            "Làm mờ và nhấp xuyên qua"
        );
        for shortcut in [None, Some("Ctrl+Alt+D")] {
            assert_eq!(
                menu_labels_in(Language::Vietnamese, true, shortcut)[4],
                "Khôi phục tương tác với Island"
            );
        }
    }

    #[test]
    fn native_menu_preserves_vietnamese_and_the_effective_shortcut_as_utf16() {
        let menu = create_in(
            false,
            false,
            false,
            Some("Ctrl+Alt+D"),
            Language::Vietnamese,
        )
        .unwrap();
        let label = |command| {
            let mut buffer = [0_u16; 128];
            let length = unsafe {
                GetMenuStringW(
                    menu.handle(),
                    command,
                    buffer.as_mut_ptr(),
                    buffer.len() as i32,
                    MF_BYCOMMAND,
                )
            };
            assert!(length > 0);
            String::from_utf16(&buffer[..length as usize]).unwrap()
        };
        assert_eq!(label(0x6001), "Đồng hồ");
        assert_eq!(label(0x6005), "Làm mờ và nhấp xuyên qua\tCtrl+Alt+D");
        assert_eq!(label(0x6006), "Cài đặt…");
    }

    #[test]
    fn native_menu_is_modeless_and_keeps_command_notifications() {
        let menu = create(false, false, false, Some("Ctrl+Alt+D")).unwrap();
        let mut info = MENUINFO {
            cbSize: size_of::<MENUINFO>() as u32,
            fMask: MIM_STYLE,
            ..Default::default()
        };
        unsafe {
            assert_ne!(GetMenuInfo(menu.handle(), &mut info), 0);
        }
        assert_ne!(info.dwStyle & MNS_MODELESS, 0);
        assert_eq!(info.dwStyle & MNS_NOTIFYBYPOS, 0);
    }

    #[test]
    fn native_items_dispatch_search_and_existing_actions_without_capturing_other_commands() {
        let menu = create(false, false, false, None).unwrap();
        let expected = [
            (0, 0x6001, Action::Clock),
            (1, 0x6002, Action::Music),
            (2, 0x6003, Action::ResumeLyrics),
            (3, 0x6004, Action::SearchLyrics),
            (5, 0x6005, Action::TogglePassthrough),
            (6, 0x6006, Action::Settings),
            (8, 0x6007, Action::Exit),
        ];
        unsafe {
            assert_eq!(GetMenuItemCount(menu.handle()), 9);
            for (position, command, expected_action) in expected {
                assert_eq!(GetMenuItemID(menu.handle(), position), command);
                assert_eq!(action(command as usize), Some(expected_action));
            }
            for position in [4, 7] {
                assert_ne!(
                    GetMenuState(menu.handle(), position, MF_BYPOSITION) & MF_SEPARATOR,
                    0
                );
            }
        }
        for command in [0, 1, 0x6000, 0x6008, usize::MAX] {
            assert_eq!(action(command), None);
        }
    }

    #[test]
    fn native_checks_and_resume_availability_follow_current_state() {
        for (music, browsing, passthrough) in [
            (false, false, false),
            (true, true, true),
            (true, false, false),
        ] {
            let menu = create(music, browsing, passthrough, Some("Ctrl+Alt+D")).unwrap();
            unsafe {
                for (command, checked) in [(0x6001, !music), (0x6002, music), (0x6005, passthrough)]
                {
                    assert_eq!(
                        GetMenuState(menu.handle(), command, MF_BYCOMMAND) & MF_CHECKED != 0,
                        checked
                    );
                }
                assert_eq!(
                    GetMenuState(menu.handle(), 0x6003, MF_BYCOMMAND) & (MF_GRAYED | MF_DISABLED)
                        != 0,
                    music && !browsing
                );
            }
        }
    }

    #[test]
    fn menu_ownership_keeps_native_handle_alive_until_drop() {
        let menu = create(false, false, false, Some("Ctrl+Alt+D")).unwrap();
        let handle = menu.handle();
        unsafe {
            assert_ne!(IsMenu(handle), 0);
        }
        drop(menu);
        unsafe {
            assert_eq!(IsMenu(handle), 0);
        }
    }
}
