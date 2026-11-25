use crate::animation::ShapeState;
use windows_sys::Win32::Foundation::{HWND, RECT};

fn covers_monitor(window: RECT, monitor: RECT) -> bool {
    window.left <= monitor.left
        && window.top <= monitor.top
        && window.right >= monitor.right
        && window.bottom >= monitor.bottom
}

#[derive(Default)]
pub(crate) struct Transition {
    pub hidden: bool,
    amount: f32,
}

impl Transition {
    pub fn advance(&mut self, dt: f32) {
        if !dt.is_finite() || dt <= 0.0 {
            return;
        }
        let goal = f32::from(self.hidden);
        if crate::animation::reduced_motion() {
            // Fullscreen retract lands instantly; no morph under reduced motion.
            self.amount = goal;
            return;
        }
        let response = if self.hidden { 0.06 } else { 0.085 };
        self.amount += (goal - self.amount) * (1.0 - (-dt.min(0.1) / response).exp());
        if (goal - self.amount).abs() < 0.002 {
            self.amount = goal;
        }
    }
    pub fn moving(&self) -> bool {
        self.amount != f32::from(self.hidden)
    }
    pub fn alpha(&self) -> f32 {
        1.0 - self.amount
    }
    pub fn shape(&self, state: ShapeState) -> ShapeState {
        ShapeState {
            width: state.width + (76.0 - state.width) * self.amount,
            height: state.height + (8.0 - state.height) * self.amount,
            radius: state.radius + (4.0 - state.radius) * self.amount,
        }
    }
}

pub(crate) fn foreground_covers_island(island: HWND) -> bool {
    use windows_sys::Win32::Graphics::Dwm::{DWMWA_EXTENDED_FRAME_BOUNDS, DwmGetWindowAttribute};
    use windows_sys::Win32::Graphics::Gdi::{
        GetMonitorInfoW, MONITOR_DEFAULTTONEAREST, MONITORINFO, MonitorFromWindow,
    };
    use windows_sys::Win32::System::Threading::GetCurrentProcessId;
    use windows_sys::Win32::UI::WindowsAndMessaging::*;
    unsafe {
        let foreground = GetForegroundWindow();
        if foreground.is_null()
            || foreground == island
            || foreground == GetDesktopWindow()
            || foreground == GetShellWindow()
            || IsIconic(foreground) != 0
            || IsWindowVisible(foreground) == 0
        {
            return false;
        }
        let mut process = 0;
        GetWindowThreadProcessId(foreground, &mut process);
        if process == GetCurrentProcessId() {
            return false;
        }
        let mut class = [0_u16; 64];
        let len = GetClassNameW(foreground, class.as_mut_ptr(), class.len() as i32);
        let class = String::from_utf16_lossy(&class[..len.max(0) as usize]);
        if matches!(
            class.as_str(),
            "Progman" | "WorkerW" | "Shell_TrayWnd" | "Shell_SecondaryTrayWnd"
        ) {
            return false;
        }
        // A captioned maximized app can extend beyond monitor bounds. Its
        // client still has a title bar and is not an immersive fullscreen app.
        if IsZoomed(foreground) != 0
            && GetWindowLongW(foreground, GWL_STYLE) as u32 & WS_CAPTION != 0
        {
            return false;
        }
        let monitor = MonitorFromWindow(island, MONITOR_DEFAULTTONEAREST);
        let mut info = MONITORINFO {
            cbSize: std::mem::size_of::<MONITORINFO>() as u32,
            ..Default::default()
        };
        if GetMonitorInfoW(monitor, &mut info) == 0 {
            return false;
        }
        let mut bounds = RECT::default();
        if DwmGetWindowAttribute(
            foreground,
            DWMWA_EXTENDED_FRAME_BOUNDS as u32,
            (&mut bounds as *mut RECT).cast(),
            std::mem::size_of::<RECT>() as u32,
        ) < 0
            && GetWindowRect(foreground, &mut bounds) == 0
        {
            return false;
        }
        covers_monitor(bounds, info.rcMonitor)
    }
}

#[cfg(test)]
pub(crate) mod tests {
    use super::*;

    #[test]
    fn fullscreen_uses_monitor_bounds_including_negative_coordinates() {
        let monitor = RECT {
            left: -1920,
            top: 0,
            right: 0,
            bottom: 1080,
        };
        assert!(covers_monitor(monitor, monitor));
        assert!(covers_monitor(
            RECT {
                left: -1921,
                top: -1,
                right: 1,
                bottom: 1081
            },
            monitor
        ));
        assert!(!covers_monitor(
            RECT {
                bottom: 1040,
                ..monitor
            },
            monitor
        ));
        assert!(!covers_monitor(
            RECT {
                left: 0,
                top: 0,
                right: 1920,
                bottom: 1080
            },
            monitor
        ));
    }

    #[test]
    fn fullscreen_retracts_and_reopens_without_changing_page_shape() {
        let original = ShapeState {
            height: 500.0,
            ..crate::animation::EXPANDED
        };
        let mut transition = Transition {
            hidden: true,
            ..Default::default()
        };
        transition.advance(1.0 / 60.0);
        assert!(transition.alpha() > 0.0 && transition.alpha() < 1.0);
        assert!(transition.shape(original).height < original.height);
        for _ in 0..90 {
            transition.advance(1.0 / 60.0);
        }
        assert!(!transition.moving());
        assert_eq!(transition.alpha(), 0.0);
        transition.hidden = false;
        transition.advance(1.0 / 60.0);
        assert!(transition.alpha() > 0.0 && transition.alpha() < 1.0);
        for _ in 0..120 {
            transition.advance(1.0 / 60.0);
        }
        assert!(!transition.moving());
        assert_eq!(transition.alpha(), 1.0);
        assert_eq!(transition.shape(original), original);
        transition.hidden = true;
        transition.advance(0.05);
        let before = transition.alpha();
        transition.hidden = false;
        transition.advance(0.01);
        assert!(
            transition.alpha() > before,
            "rapid reversal continues smoothly"
        );
    }

    #[test]
    #[ignore = "Child-process fixture for native fullscreen detection"]
    fn fullscreen_window_fixture() {
        if std::env::var_os("HIYODYNA_FULLSCREEN_FIXTURE").is_none() {
            return;
        }
        use std::io::Write;
        use windows_sys::Win32::UI::WindowsAndMessaging::*;
        unsafe {
            let class: Vec<_> = "STATIC\0".encode_utf16().collect();
            let title: Vec<_> = "HiyoDyna fullscreen test\0".encode_utf16().collect();
            let hwnd = CreateWindowExW(
                WS_EX_TOOLWINDOW,
                class.as_ptr(),
                title.as_ptr(),
                WS_POPUP | WS_VISIBLE,
                40,
                40,
                400,
                300,
                std::ptr::null_mut(),
                std::ptr::null_mut(),
                std::ptr::null_mut(),
                std::ptr::null(),
            );
            assert!(!hwnd.is_null());
            println!("FULLSCREEN_FIXTURE_HWND={}", hwnd as usize);
            std::io::stdout().flush().unwrap();
            let mut msg = MSG::default();
            while IsWindow(hwnd) != 0 && GetMessageW(&mut msg, std::ptr::null_mut(), 0, 0) > 0 {
                TranslateMessage(&msg);
                DispatchMessageW(&msg);
            }
        }
    }

    pub(crate) struct Fixture {
        child: std::process::Child,
        _stdout: std::io::BufReader<std::process::ChildStdout>,
        pub hwnd: HWND,
    }
    impl Drop for Fixture {
        fn drop(&mut self) {
            unsafe {
                windows_sys::Win32::UI::WindowsAndMessaging::PostMessageW(
                    self.hwnd,
                    windows_sys::Win32::UI::WindowsAndMessaging::WM_CLOSE,
                    0,
                    0,
                );
            }
            let _ = self.child.wait();
        }
    }
    impl Fixture {
        pub fn new() -> Self {
            use std::io::{BufRead, BufReader};
            use std::process::{Command, Stdio};
            let mut child = Command::new(std::env::current_exe().unwrap())
                .args([
                    "--exact",
                    "fullscreen::tests::fullscreen_window_fixture",
                    "--ignored",
                    "--nocapture",
                ])
                .env("HIYODYNA_FULLSCREEN_FIXTURE", "1")
                .stdout(Stdio::piped())
                .spawn()
                .unwrap();
            let mut reader = BufReader::new(child.stdout.take().unwrap());
            let hwnd = loop {
                let mut line = String::new();
                assert!(
                    reader.read_line(&mut line).unwrap() > 0,
                    "fullscreen fixture exited early"
                );
                if let Some((_, value)) = line.split_once("FULLSCREEN_FIXTURE_HWND=") {
                    break value.trim().parse::<usize>().unwrap() as HWND;
                }
            };
            Self {
                child,
                _stdout: reader,
                hwnd,
            }
        }
    }

    #[test]
    #[ignore = "Opens a separate process window to verify fullscreen, maximized and restored states"]
    fn native_fullscreen_detection() {
        use windows_sys::Win32::Graphics::Gdi::*;
        use windows_sys::Win32::UI::WindowsAndMessaging::*;
        let fixture = Fixture::new();
        let hwnd = fixture.hwnd;
        unsafe {
            let island = GetDesktopWindow();
            let mut info = MONITORINFO {
                cbSize: std::mem::size_of::<MONITORINFO>() as u32,
                ..Default::default()
            };
            assert_ne!(
                GetMonitorInfoW(
                    MonitorFromWindow(island, MONITOR_DEFAULTTOPRIMARY),
                    &mut info
                ),
                0
            );
            let r = info.rcMonitor;
            assert_ne!(
                SetWindowPos(
                    hwnd,
                    HWND_TOP,
                    r.left,
                    r.top,
                    r.right - r.left,
                    r.bottom - r.top,
                    SWP_SHOWWINDOW
                ),
                0
            );
            SetForegroundWindow(hwnd);
            for _ in 0..40 {
                if GetForegroundWindow() == hwnd {
                    break;
                }
                std::thread::sleep(std::time::Duration::from_millis(25));
            }
            assert_eq!(
                GetForegroundWindow(),
                hwnd,
                "test fixture must own foreground"
            );
            windows_sys::Win32::Graphics::Dwm::DwmFlush();
            assert!(
                foreground_covers_island(island),
                "borderless monitor coverage hides Island"
            );
            SetWindowLongPtrW(hwnd, GWL_STYLE, (WS_OVERLAPPEDWINDOW | WS_VISIBLE) as isize);
            SetWindowPos(
                hwnd,
                std::ptr::null_mut(),
                0,
                0,
                0,
                0,
                SWP_NOMOVE | SWP_NOSIZE | SWP_NOZORDER | SWP_FRAMECHANGED,
            );
            ShowWindow(hwnd, SW_MAXIMIZE);
            assert!(
                !foreground_covers_island(island),
                "normal maximize keeps Island visible"
            );
            ShowWindow(hwnd, SW_RESTORE);
            SetWindowLongPtrW(hwnd, GWL_STYLE, (WS_POPUP | WS_VISIBLE) as isize);
            SetWindowPos(
                hwnd,
                std::ptr::null_mut(),
                r.left + 40,
                r.top + 40,
                400,
                300,
                SWP_NOZORDER | SWP_FRAMECHANGED,
            );
            windows_sys::Win32::Graphics::Dwm::DwmFlush();
            assert!(
                !foreground_covers_island(island),
                "leaving fullscreen restores Island"
            );
        }
    }
}
