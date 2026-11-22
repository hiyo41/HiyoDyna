//! Native scrollbar themes cover USER32 tracking draws as well as idle painting.
// Recipe/order: https://github.com/reupen/foobar2000-sdk-unmodified/blob/5fb30e4cf97d03edafc214febfb5e30597015b38/libPPUI/DarkMode.cpp
// Versioned ABIs: https://github.com/ysc3839/win32-darkmode/blob/master/win32-darkmode/DarkMode.h
// AllowDarkModeForWindow is ordinal 133, NOT 104 (RefreshImmersiveColorPolicyState).
// Ordinal 135 is AllowDarkModeForApp(bool) on 1809 and SetPreferredAppMode on 1903+.

use std::ptr::{null, null_mut};
use std::sync::OnceLock;
use windows_sys::Win32::Foundation::{FreeLibrary, HWND};
use windows_sys::Win32::System::LibraryLoader::{
    GetModuleHandleW, GetProcAddress, LOAD_LIBRARY_SEARCH_SYSTEM32, LoadLibraryExW,
};
use windows_sys::Win32::UI::WindowsAndMessaging::IsWindow;

type AllowWindow = unsafe extern "system" fn(HWND, bool) -> bool;
type SetTheme = unsafe extern "system" fn(HWND, *const u16, *const u16) -> i32;

struct DarkMode {
    allow_window: AllowWindow,
    set_theme: SetTheme,
}

static DARK_MODE: OnceLock<Option<DarkMode>> = OnceLock::new();

fn wide(value: &str) -> Vec<u16> {
    value.encode_utf16().chain([0]).collect()
}

impl DarkMode {
    unsafe fn load() -> Result<Self, &'static str> {
        unsafe {
            let ntdll = GetModuleHandleW(wide("ntdll.dll").as_ptr());
            if ntdll.is_null() {
                return Err("ntdll unavailable");
            }
            let version = GetProcAddress(ntdll, c"RtlGetNtVersionNumbers".as_ptr().cast())
                .ok_or("RtlGetNtVersionNumbers unavailable")?;
            let version: unsafe extern "system" fn(*mut u32, *mut u32, *mut u32) =
                std::mem::transmute(version);
            let (mut major, mut minor, mut build) = (0, 0, 0);
            version(&mut major, &mut minor, &mut build);
            build &= 0x0fff_ffff;
            if major != 10 || minor != 0 || build < 17763 {
                return Err("requires Windows 10 1809 or later");
            }
            let module = LoadLibraryExW(
                wide("uxtheme.dll").as_ptr(),
                null_mut(),
                LOAD_LIBRARY_SEARCH_SYSTEM32,
            );
            if module.is_null() {
                return Err("uxtheme unavailable");
            }
            let result = (|| {
                let allow = GetProcAddress(module, 133_usize as *const u8)
                    .ok_or("uxtheme ordinal 133 unavailable")?;
                let app = GetProcAddress(module, 135_usize as *const u8)
                    .ok_or("uxtheme ordinal 135 unavailable")?;
                let theme = GetProcAddress(module, c"SetWindowTheme".as_ptr().cast())
                    .ok_or("SetWindowTheme unavailable")?;
                let allow_window: AllowWindow = std::mem::transmute(allow);
                let set_theme: SetTheme = std::mem::transmute(theme);
                if build >= 18362 {
                    let set_app: unsafe extern "system" fn(i32) -> i32 = std::mem::transmute(app);
                    set_app(2); // PreferredAppMode::ForceDark, before any app HWND.
                } else {
                    // 1809 has no ForceDark enum. Opt in using its boolean ABI;
                    // the explicit DarkMode_Explorer window theme selects dark.
                    let allow_app: unsafe extern "system" fn(bool) -> bool =
                        std::mem::transmute(app);
                    allow_app(true);
                }
                Ok(Self {
                    allow_window,
                    set_theme,
                })
            })();
            if result.is_err() {
                FreeLibrary(module);
            }
            // On success retain the system DLL for the process lifetime: cached
            // function pointers must never outlive their module's load reference.
            result
        }
    }
}

/// Process-wide opt-in, once at startup; also supports isolated hidden fixtures.
pub(crate) fn initialize() -> bool {
    DARK_MODE
        .get_or_init(|| match unsafe { DarkMode::load() } {
            Ok(api) => Some(api),
            Err(reason) => {
                eprintln!("Native dark scrollbar disabled: {reason}");
                None
            }
        })
        .is_some()
}

/// Keep native input/geometry/providers; theme the tracker instead of repainting it.
pub(crate) unsafe fn apply(hwnd: HWND) -> bool {
    unsafe {
        if hwnd.is_null() || IsWindow(hwnd) == 0 || !initialize() {
            return false;
        }
        let Some(Some(api)) = DARK_MODE.get() else {
            return false;
        };
        // This order matters. SetWindowTheme sends WM_THEMECHANGED synchronously;
        // callers must not hold a mutable app-state borrow during this call.
        // The reference recipe does not treat this undocumented return as an
        // HRESULT. A fresh HWND can return false while ordinal 137 confirms it
        // has opted in; skipping SetWindowTheme here would miss the first use.
        (api.allow_window)(hwnd, true);
        let result = (api.set_theme)(hwnd, wide("DarkMode_Explorer").as_ptr(), null());
        if result < 0 {
            eprintln!("Native dark scrollbar: SetWindowTheme failed ({result:#x})");
            return false;
        }
        true
    }
}
