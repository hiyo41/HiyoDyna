//! Shared native utility-window frame. Native child buttons retain keyboard/UIA behavior.

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum Hit {
    Client,
    Caption,
    Left,
    Right,
    Top,
    Bottom,
    TopLeft,
    TopRight,
    BottomLeft,
    BottomRight,
}

fn hit_test(
    x: f32,
    y: f32,
    width: f32,
    height: f32,
    resizable: bool,
    maximized: bool,
    child: bool,
) -> Hit {
    if resizable && !maximized {
        let left = x < 6.0;
        let right = x >= width - 6.0;
        let top = y < 6.0;
        let bottom = y >= height - 6.0;
        match (left, right, top, bottom) {
            (true, _, true, _) => return Hit::TopLeft,
            (_, true, true, _) => return Hit::TopRight,
            (true, _, _, true) => return Hit::BottomLeft,
            (_, true, _, true) => return Hit::BottomRight,
            (true, _, _, _) => return Hit::Left,
            (_, true, _, _) => return Hit::Right,
            (_, _, true, _) => return Hit::Top,
            (_, _, _, true) => return Hit::Bottom,
            _ => {}
        }
    }
    if !child && (6.0..54.0).contains(&y) && x >= 6.0 && x < width - 112.0 {
        Hit::Caption
    } else {
        Hit::Client
    }
}

#[cfg(windows)]
mod native {
    use super::*;
    use crate::canvas::{Canvas, Painter, Rect, WHITE};
    use crate::i18n::tr;
    use crate::native_ui::{self, ButtonStyle};
    use std::cell::RefCell;
    use std::ptr::{null, null_mut};
    use std::time::Instant;
    use windows_sys::Win32::Foundation::{HWND, LPARAM, LRESULT, POINT, RECT, WPARAM};
    use windows_sys::Win32::Graphics::Gdi::*;
    use windows_sys::Win32::UI::Controls::{DRAWITEMSTRUCT, ODS_FOCUS, ODS_SELECTED};
    use windows_sys::Win32::UI::Shell::{DefSubclassProc, RemoveWindowSubclass, SetWindowSubclass};
    use windows_sys::Win32::UI::WindowsAndMessaging::*;

    pub(crate) const MINIMIZE: i32 = 0x7f10;
    pub(crate) const MAXIMIZE: i32 = 0x7f11;
    pub(crate) const CLOSE: i32 = 0x7f12;
    const SUBCLASS: usize = 0x4844;
    const MOTION: usize = 0x4953;

    struct Transition {
        animation: crate::animation::Animation,
        from: RECT,
        to: RECT,
        updated: Instant,
    }

    #[derive(Default)]
    struct Motion {
        restore: Option<RECT>,
        transition: Option<Transition>,
    }

    struct Chrome {
        resizable: bool,
        minimize: bool,
        painter: RefCell<Painter>,
        motion: RefCell<Motion>,
        return_focus: HWND,
    }

    fn wide(text: &str) -> Vec<u16> {
        text.encode_utf16().chain(Some(0)).collect()
    }

    pub(crate) fn is_button(id: i32) -> bool {
        matches!(id, MINIMIZE | MAXIMIZE | CLOSE)
    }

    pub(crate) unsafe fn install(
        hwnd: HWND,
        resizable: bool,
        _minimize: bool,
    ) -> Result<(), String> {
        unsafe {
            let scale = windows_sys::Win32::UI::HiDpi::GetDpiForWindow(hwnd).max(96) as f32 / 96.0;
            let chrome = Box::new(Chrome {
                resizable,
                minimize: false,
                painter: RefCell::new(Painter::new(scale)?),
                motion: RefCell::new(Motion::default()),
                return_focus: windows_sys::Win32::UI::Input::KeyboardAndMouse::GetFocus(),
            });
            let ptr = Box::into_raw(chrome);
            if SetWindowSubclass(hwnd, Some(frame_proc), SUBCLASS, ptr as usize) == 0 {
                drop(Box::from_raw(ptr));
                return Err("Could not create utility window chrome".into());
            }
            native_ui::style_window(hwnd);
            let ex = GetWindowLongW(hwnd, GWL_EXSTYLE) as u32;
            SetWindowLongW(
                hwnd,
                GWL_EXSTYLE,
                ((ex & !WS_EX_APPWINDOW) | WS_EX_TOOLWINDOW) as i32,
            );
            let style = GetWindowLongW(hwnd, GWL_STYLE) as u32;
            SetWindowLongW(
                hwnd,
                GWL_STYLE,
                ((style & !(WS_CAPTION | WS_MINIMIZEBOX | WS_MAXIMIZEBOX)) | WS_POPUP) as i32,
            );
            for (id, enabled) in [(MAXIMIZE, resizable), (CLOSE, true)] {
                if !enabled {
                    continue;
                }
                let button = CreateWindowExW(
                    0,
                    wide("BUTTON").as_ptr(),
                    null(),
                    WS_CHILD | WS_VISIBLE | WS_TABSTOP | BS_OWNERDRAW as u32,
                    0,
                    0,
                    1,
                    1,
                    hwnd,
                    id as usize as *mut _,
                    null_mut(),
                    null(),
                );
                if button.is_null() {
                    return Err("Could not create utility title controls".into());
                }
                SetWindowSubclass(button, Some(native_ui::hover_proc), 2, 0);
            }
            refresh(hwnd);
            SetWindowPos(
                hwnd,
                null_mut(),
                0,
                0,
                0,
                0,
                SWP_NOMOVE | SWP_NOSIZE | SWP_NOZORDER | SWP_NOACTIVATE | SWP_FRAMECHANGED,
            );
            Ok(())
        }
    }

    pub(crate) unsafe fn refresh(hwnd: HWND) {
        unsafe {
            for (id, name) in [
                (MINIMIZE, "Minimize"),
                (
                    MAXIMIZE,
                    if GetPropW(hwnd, wide("HiyoDynaExpanded").as_ptr()).is_null() {
                        "Expand"
                    } else {
                        "Restore"
                    },
                ),
                (CLOSE, "Close"),
            ] {
                let button = GetDlgItem(hwnd, id);
                if !button.is_null() {
                    SetWindowTextW(button, wide(tr(name)).as_ptr());
                    InvalidateRect(button, null(), 0);
                }
            }
        }
    }

    unsafe fn arrange(hwnd: HWND, chrome: &Chrome) {
        unsafe {
            let scale = windows_sys::Win32::UI::HiDpi::GetDpiForWindow(hwnd).max(96) as f32 / 96.0;
            let mut rect = RECT::default();
            GetClientRect(hwnd, &mut rect);
            let mut right = rect.right as f32 / scale - 12.0;
            for (id, present) in [
                (CLOSE, true),
                (MAXIMIZE, chrome.resizable),
                (MINIMIZE, chrome.minimize),
            ] {
                if !present {
                    continue;
                }
                right -= 32.0;
                SetWindowPos(
                    GetDlgItem(hwnd, id),
                    null_mut(),
                    (right * scale).round() as i32,
                    (14.0 * scale).round() as i32,
                    (32.0 * scale).round() as i32,
                    (30.0 * scale).round() as i32,
                    SWP_NOZORDER | SWP_NOACTIVATE,
                );
            }
            let mut window = RECT::default();
            GetWindowRect(hwnd, &mut window);
            let region = if IsZoomed(hwnd) != 0 {
                null_mut()
            } else {
                CreateRoundRectRgn(
                    0,
                    0,
                    window.right - window.left + 1,
                    window.bottom - window.top + 1,
                    (2.0 * crate::island_surface::radius(
                        window.right as f32 / scale - window.left as f32 / scale,
                        window.bottom as f32 / scale - window.top as f32 / scale,
                    ) * scale)
                        .round() as i32,
                    (2.0 * crate::animation::EXPANDED.radius * scale).round() as i32,
                )
            };
            if SetWindowRgn(hwnd, region, 1) == 0 && !region.is_null() {
                DeleteObject(region);
            }
            refresh(hwnd);
        }
    }

    unsafe extern "system" fn frame_proc(
        hwnd: HWND,
        message: u32,
        wparam: WPARAM,
        lparam: LPARAM,
        _id: usize,
        data: usize,
    ) -> LRESULT {
        unsafe {
            let chrome = &*(data as *const Chrome);
            match message {
                WM_NCCALCSIZE => {
                    if IsZoomed(hwnd) != 0 && lparam != 0 {
                        let rect = if wparam != 0 {
                            &mut (*(lparam as *mut NCCALCSIZE_PARAMS)).rgrc[0]
                        } else {
                            &mut *(lparam as *mut RECT)
                        };
                        let mut info = MONITORINFO {
                            cbSize: std::mem::size_of::<MONITORINFO>() as u32,
                            ..Default::default()
                        };
                        if GetMonitorInfoW(
                            MonitorFromWindow(hwnd, MONITOR_DEFAULTTONEAREST),
                            &mut info,
                        ) != 0
                        {
                            rect.left = rect.left.max(info.rcWork.left);
                            rect.top = rect.top.max(info.rcWork.top);
                            rect.right = rect.right.min(info.rcWork.right);
                            rect.bottom = rect.bottom.min(info.rcWork.bottom);
                        }
                    }
                    0
                }
                WM_NCHITTEST => {
                    let mut point = POINT {
                        x: lparam as u16 as i16 as i32,
                        y: (lparam >> 16) as u16 as i16 as i32,
                    };
                    ScreenToClient(hwnd, &mut point);
                    let mut rect = RECT::default();
                    GetClientRect(hwnd, &mut rect);
                    let scale =
                        windows_sys::Win32::UI::HiDpi::GetDpiForWindow(hwnd).max(96) as f32 / 96.0;
                    let child = ChildWindowFromPointEx(hwnd, point, CWP_SKIPINVISIBLE);
                    (match hit_test(
                        point.x as f32 / scale,
                        point.y as f32 / scale,
                        rect.right as f32 / scale,
                        rect.bottom as f32 / scale,
                        chrome.resizable,
                        IsZoomed(hwnd) != 0,
                        !child.is_null() && child != hwnd,
                    ) {
                        Hit::Client => HTCLIENT,
                        Hit::Caption => HTCAPTION,
                        Hit::Left => HTLEFT,
                        Hit::Right => HTRIGHT,
                        Hit::Top => HTTOP,
                        Hit::Bottom => HTBOTTOM,
                        Hit::TopLeft => HTTOPLEFT,
                        Hit::TopRight => HTTOPRIGHT,
                        Hit::BottomLeft => HTBOTTOMLEFT,
                        Hit::BottomRight => HTBOTTOMRIGHT,
                    }) as isize
                }
                WM_NCLBUTTONDBLCLK if wparam == HTCAPTION as usize && !chrome.resizable => 0,
                WM_COMMAND if is_button((wparam & 0xffff) as i32) => {
                    if (wparam & 0xffff) as i32 == MAXIMIZE {
                        toggle(hwnd, chrome);
                        return 0;
                    }
                    let command = match (wparam & 0xffff) as i32 {
                        MINIMIZE => SC_MINIMIZE,
                        MAXIMIZE => {
                            if IsZoomed(hwnd) != 0 {
                                SC_RESTORE
                            } else {
                                SC_MAXIMIZE
                            }
                        }
                        _ => SC_CLOSE,
                    };
                    PostMessageW(hwnd, WM_SYSCOMMAND, command as usize, 0);
                    0
                }
                WM_DRAWITEM
                    if lparam != 0
                        && is_button((*(lparam as *const DRAWITEMSTRUCT)).CtlID as i32) =>
                {
                    let item = &*(lparam as *const DRAWITEMSTRUCT);
                    let size = (
                        (item.rcItem.right - item.rcItem.left).max(0) as u32,
                        (item.rcItem.bottom - item.rcItem.top).max(0) as u32,
                    );
                    let mut pixels = vec![0; (size.0 * size.1 * 4) as usize];
                    let hovered = native_ui::pointer_inside(item.hwndItem);
                    {
                        let mut painter = chrome.painter.borrow_mut();
                        let mut c = Canvas::new(&mut pixels, size, &mut painter);
                        native_ui::button(
                            &mut c,
                            "",
                            ButtonStyle {
                                focused: item.itemState & ODS_FOCUS != 0,
                                pressed: item.itemState & ODS_SELECTED != 0,
                                hovered,
                                ..Default::default()
                            },
                        );
                        let center = (c.clip.w * 0.5, c.clip.h * 0.5);
                        let r = Rect::new(center.0 - 4.0, center.1 - 4.0, 8.0, 8.0);
                        match item.CtlID as i32 {
                            MINIMIZE => {
                                c.rect(Rect::new(r.x, center.1 + 3.0, 8.0, 1.0), 0.0, WHITE, 1.0)
                            }
                            MAXIMIZE => {
                                if IsZoomed(hwnd) != 0 {
                                    c.rect(
                                        Rect::new(r.x + 2.0, r.y - 2.0, 8.0, 1.0),
                                        0.0,
                                        WHITE,
                                        1.0,
                                    );
                                }
                                for edge in [
                                    Rect::new(r.x, r.y, 8.0, 1.0),
                                    Rect::new(r.x, r.y + 7.0, 8.0, 1.0),
                                    Rect::new(r.x, r.y, 1.0, 8.0),
                                    Rect::new(r.x + 7.0, r.y, 1.0, 8.0),
                                ] {
                                    c.rect(edge, 0.0, WHITE, 1.0);
                                }
                            }
                            _ => {
                                for i in 0..9 {
                                    let p = i as f32;
                                    for x in [r.x + p, r.x + 8.0 - p] {
                                        c.rect(Rect::new(x, r.y + p, 1.2, 1.2), 0.6, WHITE, 1.0);
                                    }
                                }
                            }
                        }
                    }
                    native_ui::blit(item.hDC, &pixels, size);
                    1
                }
                WM_SIZE => {
                    arrange(hwnd, chrome);
                    DefSubclassProc(hwnd, message, wparam, lparam)
                }
                WM_TIMER if wparam == MOTION => {
                    advance(hwnd, chrome);
                    0
                }
                WM_SYSCOMMAND
                    if (wparam as u32 & 0xfff0) == SC_MAXIMIZE
                        || (wparam as u32 & 0xfff0) == SC_RESTORE =>
                {
                    toggle(hwnd, chrome);
                    0
                }
                WM_SYSCOMMAND if (wparam as u32 & 0xfff0) == SC_MINIMIZE => 0,
                WM_DPICHANGED => {
                    let scale = (wparam & 0xffff).max(96) as f32 / 96.0;
                    if let Ok(painter) = Painter::new(scale) {
                        *chrome.painter.borrow_mut() = painter;
                    }
                    DefSubclassProc(hwnd, message, wparam, lparam)
                }
                WM_GETMINMAXINFO => {
                    let result = DefSubclassProc(hwnd, message, wparam, lparam);
                    let mut info = MONITORINFO {
                        cbSize: std::mem::size_of::<MONITORINFO>() as u32,
                        ..Default::default()
                    };
                    if GetMonitorInfoW(MonitorFromWindow(hwnd, MONITOR_DEFAULTTONEAREST), &mut info)
                        != 0
                    {
                        let bounds = &mut *(lparam as *mut MINMAXINFO);
                        bounds.ptMaxPosition = POINT {
                            x: info.rcWork.left - info.rcMonitor.left,
                            y: info.rcWork.top - info.rcMonitor.top,
                        };
                        bounds.ptMaxSize = POINT {
                            x: info.rcWork.right - info.rcWork.left,
                            y: info.rcWork.bottom - info.rcWork.top,
                        };
                        bounds.ptMinTrackSize.x = bounds.ptMinTrackSize.x.min(bounds.ptMaxSize.x);
                        bounds.ptMinTrackSize.y = bounds.ptMinTrackSize.y.min(bounds.ptMaxSize.y);
                    }
                    result
                }
                WM_NCDESTROY => {
                    let owner = GetWindow(hwnd, GW_OWNER);
                    let restore_focus = GetForegroundWindow() == hwnd
                        && !owner.is_null()
                        && IsWindow(owner) != 0
                        && IsWindowVisible(owner) != 0
                        && GetWindowLongW(owner, GWL_EXSTYLE) as u32 & WS_EX_NOACTIVATE == 0;
                    let focus = chrome.return_focus;
                    KillTimer(hwnd, MOTION);
                    RemovePropW(hwnd, wide("HiyoDynaExpanded").as_ptr());
                    RemoveWindowSubclass(hwnd, Some(frame_proc), SUBCLASS);
                    drop(Box::from_raw(data as *mut Chrome));
                    let result = DefSubclassProc(hwnd, message, wparam, lparam);
                    if restore_focus {
                        SetForegroundWindow(owner);
                        if IsWindow(focus) != 0 && (focus == owner || IsChild(owner, focus) != 0) {
                            windows_sys::Win32::UI::Input::KeyboardAndMouse::SetFocus(focus);
                        }
                    }
                    result
                }
                _ => DefSubclassProc(hwnd, message, wparam, lparam),
            }
        }
    }

    unsafe fn toggle(hwnd: HWND, chrome: &Chrome) {
        unsafe {
            if !chrome.resizable {
                return;
            }
            let mut current = RECT::default();
            GetWindowRect(hwnd, &mut current);
            let work = crate::island_surface::work_area(current);
            let scale = windows_sys::Win32::UI::HiDpi::GetDpiForWindow(hwnd).max(96) as f32 / 96.0;
            let to = {
                let mut motion = chrome.motion.borrow_mut();
                if let Some(restore) = motion.restore.take() {
                    RemovePropW(hwnd, wide("HiyoDynaExpanded").as_ptr());
                    crate::island_surface::fit_rect(restore, work)
                } else {
                    motion.restore = Some(current);
                    SetPropW(
                        hwnd,
                        wide("HiyoDynaExpanded").as_ptr(),
                        std::ptr::dangling_mut(),
                    );
                    let width = (920.0 * scale).round() as i32;
                    let height = (760.0 * scale).round() as i32;
                    let left = current.left + (current.right - current.left - width) / 2;
                    let top = current.top;
                    crate::island_surface::fit_rect(
                        RECT {
                            left,
                            top,
                            right: left + width,
                            bottom: top + height,
                        },
                        work,
                    )
                }
            };
            let mut animation = crate::animation::Animation::new_with_tuning(
                crate::settings::Settings::load().tuning(),
            );
            animation.current = crate::animation::ShapeState {
                width: (current.right - current.left) as f32,
                height: (current.bottom - current.top) as f32,
                radius: 30.0 * scale,
            };
            animation.target = crate::animation::ShapeState {
                width: (to.right - to.left) as f32,
                height: (to.bottom - to.top) as f32,
                radius: 30.0 * scale,
            };
            chrome.motion.borrow_mut().transition = Some(Transition {
                animation,
                from: current,
                to,
                updated: Instant::now(),
            });
            SetTimer(hwnd, MOTION, 15, None);
            refresh(hwnd);
        }
    }

    unsafe fn advance(hwnd: HWND, chrome: &Chrome) {
        unsafe {
            let next = {
                let mut motion = chrome.motion.borrow_mut();
                let Some(t) = &mut motion.transition else {
                    return;
                };
                let now = Instant::now();
                // A minimum step also makes synthetic timer delivery deterministic.
                let dt = now
                    .duration_since(t.updated)
                    .as_secs_f32()
                    .clamp(0.015, 0.1);
                t.updated = now;
                let moving = t.animation.advance(dt);
                let dw = (t.to.right - t.to.left) - (t.from.right - t.from.left);
                let dh = (t.to.bottom - t.to.top) - (t.from.bottom - t.from.top);
                let progress = if dw.abs() >= dh.abs() && dw != 0 {
                    (t.animation.current.width - (t.from.right - t.from.left) as f32) / dw as f32
                } else if dh != 0 {
                    (t.animation.current.height - (t.from.bottom - t.from.top) as f32) / dh as f32
                } else {
                    1.0
                };
                let left =
                    t.from.left + ((t.to.left - t.from.left) as f32 * progress).round() as i32;
                let top = t.from.top + ((t.to.top - t.from.top) as f32 * progress).round() as i32;
                let rect = if moving {
                    crate::island_surface::fit_rect(
                        RECT {
                            left,
                            top,
                            right: left + t.animation.current.width.round() as i32,
                            bottom: top + t.animation.current.height.round() as i32,
                        },
                        crate::island_surface::work_area(t.to),
                    )
                } else {
                    t.to
                };
                if !moving {
                    motion.transition = None;
                }
                (rect, moving)
            };
            let (r, moving) = next;
            SetWindowPos(
                hwnd,
                null_mut(),
                r.left,
                r.top,
                r.right - r.left,
                r.bottom - r.top,
                SWP_NOZORDER | SWP_NOACTIVATE | SWP_NOCOPYBITS,
            );
            if !moving {
                KillTimer(hwnd, MOTION);
            }
        }
    }
}

#[cfg(windows)]
pub(crate) use native::{CLOSE, MAXIMIZE, MINIMIZE, install, is_button, refresh};

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    #[ignore = "Creates native tool hosts and checks real child controls; run serially"]
    fn island_surface_keeps_native_edit_without_taskbar_or_minimize() {
        use std::ptr::{null, null_mut};
        use windows_sys::Win32::UI::WindowsAndMessaging::*;
        unsafe {
            let wide = |s: &str| s.encode_utf16().chain(Some(0)).collect::<Vec<_>>();
            let owner = CreateWindowExW(
                0,
                wide("STATIC").as_ptr(),
                null(),
                WS_POPUP,
                0,
                0,
                148,
                40,
                null_mut(),
                null_mut(),
                null_mut(),
                null(),
            );
            let host = CreateWindowExW(
                WS_EX_APPWINDOW,
                wide("STATIC").as_ptr(),
                null(),
                WS_OVERLAPPEDWINDOW,
                100,
                100,
                500,
                400,
                owner,
                null_mut(),
                null_mut(),
                null(),
            );
            assert!(!host.is_null());
            install(host, true, true).unwrap();
            let edit = CreateWindowExW(
                0,
                wide("EDIT").as_ptr(),
                wide("Retained input").as_ptr(),
                WS_CHILD | WS_VISIBLE | WS_TABSTOP,
                24,
                72,
                200,
                28,
                host,
                31usize as *mut _,
                null_mut(),
                null(),
            );
            let ex = GetWindowLongW(host, GWL_EXSTYLE) as u32;
            assert_eq!(
                ex & WS_EX_APPWINDOW,
                0,
                "an Island tool must not request a taskbar entry"
            );
            assert_ne!(ex & WS_EX_TOOLWINDOW, 0);
            assert_eq!(GetWindow(host, GW_OWNER), owner);
            assert!(GetDlgItem(host, MINIMIZE).is_null());
            SendMessageW(host, WM_COMMAND, MAXIMIZE as usize, 0);
            for _ in 0..160 {
                SendMessageW(host, WM_TIMER, 0x4953, 0);
            }
            SendMessageW(host, WM_COMMAND, MAXIMIZE as usize, 0);
            for _ in 0..160 {
                SendMessageW(host, WM_TIMER, 0x4953, 0);
            }
            let mut text = [0u16; 64];
            let len = GetWindowTextW(edit, text.as_mut_ptr(), text.len() as i32);
            assert_eq!(
                String::from_utf16_lossy(&text[..len as usize]),
                "Retained input"
            );
            assert_eq!(GetParent(edit), host);
            let mut rect = windows_sys::Win32::Foundation::RECT::default();
            GetWindowRect(host, &mut rect);
            assert_eq!(
                (
                    rect.left,
                    rect.top,
                    rect.right - rect.left,
                    rect.bottom - rect.top
                ),
                (100, 100, 500, 400)
            );
            DestroyWindow(host);
            DestroyWindow(owner);
        }
    }

    #[test]
    fn frame_hit_targets_leave_title_buttons_and_body_interactive() {
        assert_eq!(
            hit_test(50.0, 20.0, 500.0, 400.0, true, false, false),
            Hit::Caption
        );
        assert_eq!(
            hit_test(480.0, 25.0, 500.0, 400.0, true, false, false),
            Hit::Client
        );
        assert_eq!(
            hit_test(150.0, 200.0, 500.0, 400.0, true, false, false),
            Hit::Client
        );
        assert_eq!(
            hit_test(2.0, 2.0, 500.0, 400.0, true, false, false),
            Hit::TopLeft
        );
        assert_eq!(
            hit_test(498.0, 398.0, 500.0, 400.0, true, false, false),
            Hit::BottomRight
        );
    }

    #[test]
    fn fixed_and_maximized_frames_never_report_resize_edges() {
        for (resizable, maximized) in [(false, false), (true, true)] {
            assert_eq!(
                hit_test(2.0, 200.0, 500.0, 400.0, resizable, maximized, false),
                Hit::Client
            );
            assert_eq!(
                hit_test(200.0, 398.0, 500.0, 400.0, resizable, maximized, false),
                Hit::Client
            );
        }
    }

    #[test]
    fn header_children_are_interactive_inside_the_caption() {
        assert_eq!(
            hit_test(220.0, 25.0, 360.0, 400.0, true, false, true),
            Hit::Client
        );
        assert_eq!(
            hit_test(220.0, 25.0, 360.0, 400.0, true, false, false),
            Hit::Caption
        );
    }
}
