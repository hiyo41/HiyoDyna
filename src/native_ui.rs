use crate::canvas::{Canvas, DIM, MUTED, Rect, WHITE};

pub(crate) const BACKGROUND: [u8; 3] = [8, 8, 8];
pub(crate) const SURFACE: [u8; 3] = [25, 25, 25];

/// Bounded, elapsed-time scrolling. Input changes the target; the frame timer
/// advances the viewport, so bursts of wheel messages share the same repaint.
#[derive(Clone, Copy, Debug, Default)]
pub(crate) struct SmoothScroll {
    offset: f32,
    target: f32,
    start: f32,
    started: Option<std::time::Instant>,
}

impl SmoothScroll {
    pub(crate) fn offset(&self) -> f32 {
        self.offset
    }

    pub(crate) fn target(&self) -> f32 {
        self.target
    }

    pub(crate) fn moving(&self) -> bool {
        self.started.is_some()
    }

    pub(crate) fn jump(&mut self, offset: f32) {
        self.offset = offset;
        self.target = offset;
        self.start = offset;
        self.started = None;
    }

    pub(crate) fn clamp(&mut self, max: f32) {
        let max = max.max(0.0);
        let offset = self.offset.clamp(0.0, max);
        if offset != self.offset
            || self.target != self.target.clamp(0.0, max)
            || self.start != self.start.clamp(0.0, max)
            || self.offset == self.target
        {
            // A shorter range invalidates the old interpolation curve. Keep
            // its current visible position rather than recomputing it backward.
            self.jump(offset);
        }
    }

    pub(crate) fn scroll_by(&mut self, delta: f32, max: f32, now: std::time::Instant) {
        self.scroll_to(self.target + delta, max, now);
    }

    pub(crate) fn scroll_to(&mut self, target: f32, max: f32, now: std::time::Instant) {
        self.tick(now);
        self.clamp(max);
        let target = target.clamp(0.0, max.max(0.0));
        if target == self.target {
            return;
        }
        self.start = self.offset;
        self.target = target;
        self.started = (self.start != self.target).then_some(now);
    }

    pub(crate) fn tick(&mut self, now: std::time::Instant) -> bool {
        let Some(started) = self.started else {
            return false;
        };
        let old = self.offset;
        let progress = (now.saturating_duration_since(started).as_secs_f32() / 0.160).min(1.0);
        if progress >= 1.0 {
            self.jump(self.target);
        } else {
            let eased = 1.0 - (1.0 - progress).powi(3);
            self.offset = self.start + (self.target - self.start) * eased;
        }
        self.offset != old
    }
}

struct NoticeCopy {
    template: String,
    values: Vec<(String, String)>,
}

impl NoticeCopy {
    fn render(&self) -> String {
        let values = self
            .values
            .iter()
            .map(|(key, value)| (key.as_str(), value.as_str()))
            .collect::<Vec<_>>();
        crate::i18n::format_copy(&self.template, &values)
    }
}

pub(crate) unsafe fn style_window(hwnd: windows_sys::Win32::Foundation::HWND) {
    use windows_sys::Win32::Graphics::Dwm::{DWMWA_USE_IMMERSIVE_DARK_MODE, DwmSetWindowAttribute};
    unsafe {
        use windows_sys::Win32::UI::WindowsAndMessaging::*;
        let style = GetWindowLongPtrW(hwnd, GWL_EXSTYLE);
        SetWindowLongPtrW(hwnd, GWL_EXSTYLE, style | WS_EX_COMPOSITED as isize);
        let dark = 1_i32;
        DwmSetWindowAttribute(
            hwnd,
            DWMWA_USE_IMMERSIVE_DARK_MODE as u32,
            (&dark as *const i32).cast(),
            4,
        );
        let corner = 2_u32;
        // DWMWA_WINDOW_CORNER_PREFERENCE / DWMWCP_ROUND on supported Windows.
        DwmSetWindowAttribute(hwnd, 33, (&corner as *const u32).cast(), 4);
    }
}

pub(crate) struct ButtonStyle {
    pub text_scale: f32,
    pub accent: [u8; 3],
    pub selected: bool,
    pub focused: bool,
    pub pressed: bool,
    pub hovered: bool,
    pub enabled: bool,
    pub destructive: bool,
}

impl Default for ButtonStyle {
    fn default() -> Self {
        Self {
            text_scale: 1.0,
            accent: WHITE,
            selected: false,
            focused: false,
            pressed: false,
            hovered: false,
            enabled: true,
            destructive: false,
        }
    }
}

pub(crate) fn button(c: &mut Canvas<'_>, label: &str, style: ButtonStyle) {
    let rect = c.clip;
    c.rect(rect, 0.0, BACKGROUND, 1.0);
    let inner = Rect::new(2.0, 2.0, (rect.w - 4.0).max(0.0), (rect.h - 4.0).max(0.0));
    if style.focused {
        c.rect(rect, rect.h * 0.5, style.accent, 0.9);
    }
    c.rect(
        inner,
        inner.h * 0.5,
        if style.selected && style.enabled {
            WHITE
        } else {
            SURFACE
        },
        1.0,
    );
    if style.enabled && (style.pressed || style.hovered) {
        c.rect(
            inner,
            inner.h * 0.5,
            WHITE,
            if style.pressed { 0.14 } else { 0.08 },
        );
    }
    let color = if !style.enabled {
        DIM
    } else if style.selected {
        BACKGROUND
    } else if style.destructive {
        [154, 164, 244]
    } else {
        WHITE
    };
    let label = label.replace('&', "");
    let text_size = 11.0 * style.text_scale;
    let available = (rect.w - 10.0).max(0.0);
    let mut lines = vec![label.as_str()];
    if c.painter.width(&label, text_size, style.selected) > available {
        let mut best = None;
        for (index, ch) in label.char_indices().filter(|(_, ch)| ch.is_whitespace()) {
            let (left, right) = label.split_at(index);
            let (left, right) = (left.trim(), right.trim_start_matches(ch));
            if left.is_empty() || right.is_empty() {
                continue;
            }
            let a = c.painter.width(left, text_size, style.selected);
            let b = c.painter.width(right, text_size, style.selected);
            if a.max(b) <= available && best.is_none_or(|(_, _, balance)| (a - b).abs() < balance) {
                best = Some((left, right, (a - b).abs()));
            }
        }
        if let Some((left, right, _)) = best {
            lines = vec![left, right];
        }
    }
    let line_height = rect.h / lines.len() as f32;
    for (index, line) in lines.into_iter().enumerate() {
        let width = c.painter.width(line, text_size, style.selected);
        c.ellipsis(
            line,
            Rect::new(
                ((rect.w - width) * 0.5).max(5.0),
                index as f32 * line_height,
                available,
                line_height,
            ),
            text_size,
            style.selected,
            color,
        );
    }
}

/// Wrap without reducing the text size; used by native accessible status controls.
pub(crate) fn wrapped_text(c: &mut Canvas<'_>, text: &str, rect: Rect, size: f32, color: [u8; 3]) {
    let mut y = rect.y;
    let height = size * 1.65;
    for paragraph in text.split('\n') {
        let mut line = String::new();
        for word in paragraph.split_whitespace() {
            let candidate = if line.is_empty() {
                word.to_owned()
            } else {
                format!("{line} {word}")
            };
            if !line.is_empty() && c.painter.width(&candidate, size, false) > rect.w {
                c.ellipsis(
                    &line,
                    Rect::new(rect.x, y, rect.w, height),
                    size,
                    false,
                    color,
                );
                y += height;
                line.clear();
            }
            if y + height > rect.y + rect.h {
                return;
            }
            if !line.is_empty() {
                line.push(' ');
            }
            line.push_str(word);
        }
        if y + height > rect.y + rect.h {
            return;
        }
        if !line.is_empty() {
            c.ellipsis(
                &line,
                Rect::new(rect.x, y, rect.w, height),
                size,
                false,
                color,
            );
        }
        y += height;
    }
}

pub(crate) unsafe fn pointer_inside(hwnd: windows_sys::Win32::Foundation::HWND) -> bool {
    use windows_sys::Win32::Foundation::{POINT, RECT};
    use windows_sys::Win32::Graphics::Gdi::ScreenToClient;
    use windows_sys::Win32::UI::WindowsAndMessaging::{GetClientRect, GetCursorPos};
    unsafe {
        let mut point = POINT::default();
        let mut rect = RECT::default();
        GetCursorPos(&mut point) != 0
            && ScreenToClient(hwnd, &mut point) != 0
            && GetClientRect(hwnd, &mut rect) != 0
            && point.x >= rect.left
            && point.x < rect.right
            && point.y >= rect.top
            && point.y < rect.bottom
    }
}

/// Request one repaint on hover entry/exit, keeping idle controls event-driven.
pub(crate) unsafe extern "system" fn hover_proc(
    hwnd: windows_sys::Win32::Foundation::HWND,
    message: u32,
    wparam: usize,
    lparam: isize,
    _id: usize,
    _data: usize,
) -> isize {
    use windows_sys::Win32::Graphics::Gdi::InvalidateRect;
    use windows_sys::Win32::UI::Controls::WM_MOUSELEAVE;
    use windows_sys::Win32::UI::Input::KeyboardAndMouse::{
        TME_LEAVE, TME_QUERY, TRACKMOUSEEVENT, TrackMouseEvent,
    };
    use windows_sys::Win32::UI::Shell::{DefSubclassProc, RemoveWindowSubclass};
    use windows_sys::Win32::UI::WindowsAndMessaging::*;
    unsafe {
        if message == WM_MOUSEMOVE {
            let mut track = TRACKMOUSEEVENT {
                cbSize: std::mem::size_of::<TRACKMOUSEEVENT>() as u32,
                dwFlags: TME_QUERY,
                hwndTrack: hwnd,
                dwHoverTime: 0,
            };
            TrackMouseEvent(&mut track);
            if track.dwFlags & TME_LEAVE == 0 {
                track.dwFlags = TME_LEAVE;
                track.hwndTrack = hwnd;
                TrackMouseEvent(&mut track);
                InvalidateRect(hwnd, std::ptr::null(), 0);
            }
        } else if message == WM_MOUSELEAVE {
            InvalidateRect(hwnd, std::ptr::null(), 0);
        } else if message == WM_NCDESTROY {
            RemoveWindowSubclass(hwnd, Some(hover_proc), 2);
        }
        DefSubclassProc(hwnd, message, wparam, lparam)
    }
}

/// Two-line native list row. The LISTBOX retains navigation and accessibility.
pub(crate) fn result_row(c: &mut Canvas<'_>, title: &str, detail: &str, style: ButtonStyle) {
    let rect = c.clip;
    c.rect(rect, 0.0, BACKGROUND, 1.0);
    let row = Rect::new(0.0, 2.0, rect.w, (rect.h - 4.0).max(0.0));
    if style.focused {
        c.rect(row, 14.0, WHITE, 0.55);
    }
    let row = Rect::new(1.0, 3.0, rect.w - 2.0, (rect.h - 6.0).max(0.0));
    c.rect(
        row,
        13.0,
        if style.selected {
            [29, 29, 29]
        } else {
            BACKGROUND
        },
        1.0,
    );
    if style.selected {
        c.rect(row, 13.0, style.accent, 0.04);
        c.rect(
            Rect::new(3.0, 18.0, 3.0, (rect.h - 36.0).max(0.0)),
            1.5,
            style.accent,
            1.0,
        );
    }
    c.ellipsis(
        title,
        Rect::new(14.0, 7.0, rect.w - 28.0, 22.0),
        13.0,
        true,
        WHITE,
    );
    c.ellipsis(
        detail,
        Rect::new(14.0, 30.0, rect.w - 28.0, 18.0),
        10.0,
        false,
        MUTED,
    );
}

pub(crate) unsafe fn blit(
    dc: windows_sys::Win32::Graphics::Gdi::HDC,
    pixels: &[u8],
    size: (u32, u32),
) {
    use windows_sys::Win32::Graphics::Gdi::*;
    let info = BITMAPINFO {
        bmiHeader: BITMAPINFOHEADER {
            biSize: std::mem::size_of::<BITMAPINFOHEADER>() as u32,
            biWidth: size.0 as i32,
            biHeight: -(size.1 as i32),
            biPlanes: 1,
            biBitCount: 32,
            biCompression: BI_RGB,
            ..Default::default()
        },
        ..Default::default()
    };
    unsafe {
        StretchDIBits(
            dc,
            0,
            0,
            size.0 as i32,
            size.1 as i32,
            0,
            0,
            size.0 as i32,
            size.1 as i32,
            pixels.as_ptr().cast(),
            &info,
            DIB_RGB_COLORS,
            SRCCOPY,
        );
    }
}

mod notices {
    use super::*;
    use crate::canvas::Painter;
    use crate::i18n::tr;
    use std::cell::{Cell, RefCell};
    use std::ptr::{null, null_mut};
    use windows_sys::Win32::Foundation::{HWND, LPARAM, LRESULT, RECT, WPARAM};
    use windows_sys::Win32::Graphics::Gdi::*;
    use windows_sys::Win32::System::LibraryLoader::GetModuleHandleW;
    use windows_sys::Win32::UI::Controls::{DRAWITEMSTRUCT, ODS_FOCUS, ODS_SELECTED};
    use windows_sys::Win32::UI::Input::KeyboardAndMouse::{GetFocus, SetFocus};
    use windows_sys::Win32::UI::WindowsAndMessaging::*;

    thread_local! { static WINDOWS: RefCell<Vec<HWND>> = const { RefCell::new(Vec::new()) }; }

    struct Notice {
        title: String,
        message: NoticeCopy,
        painter: RefCell<Painter>,
        font: Cell<HFONT>,
        brush: HBRUSH,
    }

    impl Drop for Notice {
        fn drop(&mut self) {
            unsafe {
                if !self.font.get().is_null() {
                    DeleteObject(self.font.get());
                }
                if !self.brush.is_null() {
                    DeleteObject(self.brush);
                }
            }
        }
    }

    fn wide(text: &str) -> Vec<u16> {
        text.encode_utf16().chain(Some(0)).collect()
    }

    unsafe fn relabel(hwnd: HWND) {
        unsafe {
            let ptr = GetWindowLongPtrW(hwnd, GWLP_USERDATA) as *const Notice;
            if ptr.is_null() {
                return;
            }
            let title = tr(&(*ptr).title).to_owned();
            let message = (*ptr).message.render();
            let body = GetDlgItem(hwnd, 2);
            let mut start = 0_u32;
            let mut end = 0_u32;
            SendMessageW(
                body,
                windows_sys::Win32::UI::Controls::EM_GETSEL,
                (&mut start as *mut u32) as usize,
                (&mut end as *mut u32) as isize,
            );
            let first_line = SendMessageW(
                body,
                windows_sys::Win32::UI::Controls::EM_GETFIRSTVISIBLELINE,
                0,
                0,
            );
            SetWindowTextW(hwnd, wide(&title).as_ptr());
            SetWindowTextW(
                body,
                wide(&message.replace("\r\n", "\n").replace('\n', "\r\n")).as_ptr(),
            );
            SendMessageW(
                body,
                windows_sys::Win32::UI::Controls::EM_SETSEL,
                start as usize,
                end as isize,
            );
            let current_line = SendMessageW(
                body,
                windows_sys::Win32::UI::Controls::EM_GETFIRSTVISIBLELINE,
                0,
                0,
            );
            SendMessageW(
                body,
                windows_sys::Win32::UI::Controls::EM_LINESCROLL,
                0,
                first_line - current_line,
            );
            SetWindowTextW(GetDlgItem(hwnd, 1), wide(tr("Close")).as_ptr());
            crate::window_chrome::refresh(hwnd);
            RedrawWindow(hwnd, null(), null_mut(), RDW_INVALIDATE | RDW_ALLCHILDREN);
        }
    }

    pub(crate) fn refresh_language() {
        let windows = WINDOWS.with(|windows| windows.borrow().clone());
        for hwnd in windows {
            unsafe {
                if IsWindow(hwnd) != 0 {
                    relabel(hwnd);
                }
            }
        }
    }

    unsafe fn layout(hwnd: HWND) {
        unsafe {
            let mut rect = RECT::default();
            GetClientRect(hwnd, &mut rect);
            let scale = windows_sys::Win32::UI::HiDpi::GetDpiForWindow(hwnd).max(96) as f32 / 96.0;
            let padding = (24.0 * scale).round() as i32;
            MoveWindow(
                GetDlgItem(hwnd, 2),
                padding,
                (68.0 * scale).round() as i32,
                (rect.right - padding * 2).max(1),
                (rect.bottom - (140.0 * scale).round() as i32).max(1),
                1,
            );
            MoveWindow(
                GetDlgItem(hwnd, 1),
                (rect.right - (144.0 * scale).round() as i32).max(0),
                (rect.bottom - (54.0 * scale).round() as i32).max(0),
                (120.0 * scale).round() as i32,
                (32.0 * scale).round() as i32,
                1,
            );
            InvalidateRect(hwnd, null(), 0);
        }
    }

    unsafe fn set_font(hwnd: HWND) {
        unsafe {
            let ptr = GetWindowLongPtrW(hwnd, GWLP_USERDATA) as *const Notice;
            if ptr.is_null() {
                return;
            }
            let scale = windows_sys::Win32::UI::HiDpi::GetDpiForWindow(hwnd).max(96) as f32 / 96.0;
            let font = CreateFontW(
                (-12.0 * scale).round() as i32,
                0,
                0,
                0,
                FW_NORMAL as i32,
                0,
                0,
                0,
                DEFAULT_CHARSET.into(),
                OUT_DEFAULT_PRECIS.into(),
                CLIP_DEFAULT_PRECIS.into(),
                CLEARTYPE_QUALITY.into(),
                (DEFAULT_PITCH | FF_DONTCARE).into(),
                wide("Inter Medium").as_ptr(),
            );
            let old = (*ptr).font.replace(font);
            let effective = if font.is_null() {
                GetStockObject(DEFAULT_GUI_FONT)
            } else {
                font
            };
            SendMessageW(GetDlgItem(hwnd, 2), WM_SETFONT, effective as usize, 1);
            if !old.is_null() {
                DeleteObject(old);
            }
        }
    }

    unsafe extern "system" fn proc(
        hwnd: HWND,
        message: u32,
        wparam: WPARAM,
        lparam: LPARAM,
    ) -> LRESULT {
        unsafe {
            let ptr = GetWindowLongPtrW(hwnd, GWLP_USERDATA) as *const Notice;
            if message == WM_NCDESTROY {
                SetWindowLongPtrW(hwnd, GWLP_USERDATA, 0);
                WINDOWS.with(|windows| windows.borrow_mut().retain(|candidate| *candidate != hwnd));
                if !ptr.is_null() {
                    drop(Box::from_raw(ptr as *mut Notice));
                }
                return DefWindowProcW(hwnd, message, wparam, lparam);
            }
            if ptr.is_null() {
                return DefWindowProcW(hwnd, message, wparam, lparam);
            }
            match message {
                WM_ERASEBKGND => 1,
                WM_CTLCOLORSTATIC | WM_CTLCOLOREDIT => {
                    let dc = wparam as HDC;
                    SetTextColor(dc, 0x00f8f8f8);
                    SetBkColor(dc, 0x00080808);
                    (*ptr).brush as isize
                }
                WM_PAINT => {
                    let mut ps = PAINTSTRUCT::default();
                    let dc = BeginPaint(hwnd, &mut ps);
                    let mut rect = RECT::default();
                    GetClientRect(hwnd, &mut rect);
                    let size = (rect.right.max(0) as u32, rect.bottom.max(0) as u32);
                    let mut pixels = vec![0; (size.0 * size.1 * 4) as usize];
                    {
                        let mut painter = (*ptr).painter.borrow_mut();
                        let mut c = Canvas::new(&mut pixels, size, &mut painter);
                        c.rect(c.clip, 0.0, BACKGROUND, 1.0);
                        let width = c.clip.w;
                        c.ellipsis(
                            tr(&(*ptr).title),
                            Rect::new(24.0, 16.0, width - 84.0, 30.0),
                            20.0,
                            true,
                            WHITE,
                        );
                    }
                    blit(dc, &pixels, size);
                    EndPaint(hwnd, &ps);
                    0
                }
                WM_DRAWITEM => {
                    let item = &*(lparam as *const DRAWITEMSTRUCT);
                    let size = (
                        (item.rcItem.right - item.rcItem.left).max(0) as u32,
                        (item.rcItem.bottom - item.rcItem.top).max(0) as u32,
                    );
                    let hovered = pointer_inside(item.hwndItem);
                    let mut pixels = vec![0; (size.0 * size.1 * 4) as usize];
                    {
                        let mut painter = (*ptr).painter.borrow_mut();
                        let mut c = Canvas::new(&mut pixels, size, &mut painter);
                        button(
                            &mut c,
                            tr("Close"),
                            ButtonStyle {
                                focused: item.itemState & ODS_FOCUS != 0,
                                pressed: item.itemState & ODS_SELECTED != 0,
                                hovered,
                                ..Default::default()
                            },
                        );
                    }
                    blit(item.hDC, &pixels, size);
                    1
                }
                WM_SIZE => {
                    layout(hwnd);
                    0
                }
                WM_DPICHANGED => {
                    let scale = (wparam & 0xffff).max(96) as f32 / 96.0;
                    if let Ok(painter) = Painter::new(scale) {
                        *(*ptr).painter.borrow_mut() = painter;
                    }
                    set_font(hwnd);
                    let rect = *(lparam as *const RECT);
                    let mut monitor = MONITORINFO {
                        cbSize: std::mem::size_of::<MONITORINFO>() as u32,
                        ..Default::default()
                    };
                    GetMonitorInfoW(
                        MonitorFromRect(&rect, MONITOR_DEFAULTTONEAREST),
                        &mut monitor,
                    );
                    let width = (rect.right - rect.left)
                        .min(monitor.rcWork.right - monitor.rcWork.left)
                        .max(1);
                    let height = (rect.bottom - rect.top)
                        .min(monitor.rcWork.bottom - monitor.rcWork.top)
                        .max(1);
                    let x = rect.left.clamp(
                        monitor.rcWork.left,
                        (monitor.rcWork.right - width).max(monitor.rcWork.left),
                    );
                    let y = rect.top.clamp(
                        monitor.rcWork.top,
                        (monitor.rcWork.bottom - height).max(monitor.rcWork.top),
                    );
                    SetWindowPos(
                        hwnd,
                        null_mut(),
                        x,
                        y,
                        width,
                        height,
                        SWP_NOZORDER | SWP_NOACTIVATE,
                    );
                    layout(hwnd);
                    0
                }
                WM_COMMAND if wparam & 0xffff == 1 => {
                    DestroyWindow(hwnd);
                    0
                }
                WM_CLOSE => {
                    DestroyWindow(hwnd);
                    0
                }
                _ => DefWindowProcW(hwnd, message, wparam, lparam),
            }
        }
    }

    /// Native translated notice, dispatched by the application's normal message loop.
    pub(crate) fn notice(owner: HWND, title: &str, message: &str) {
        notice_formatted(owner, title, message, &[]);
    }

    /// Keep template and metadata separate so an open About/error notice can relabel live.
    pub(crate) fn notice_formatted(
        owner: HWND,
        title: &str,
        template: &str,
        values: &[(&str, &str)],
    ) {
        let copy = NoticeCopy {
            template: template.into(),
            values: values
                .iter()
                .map(|(key, value)| ((*key).into(), (*value).into()))
                .collect(),
        };
        let message = copy.render();
        unsafe {
            let instance = GetModuleHandleW(null());
            let class = wide("HiyoDynaNotice");
            RegisterClassW(&WNDCLASSW {
                lpfnWndProc: Some(proc),
                hInstance: instance,
                lpszClassName: class.as_ptr(),
                hCursor: LoadCursorW(null_mut(), IDC_ARROW),
                ..Default::default()
            });
            let hwnd = CreateWindowExW(
                WS_EX_CONTROLPARENT | WS_EX_TOOLWINDOW,
                class.as_ptr(),
                wide(tr(title)).as_ptr(),
                WS_OVERLAPPED | WS_CAPTION | WS_SYSMENU | WS_CLIPCHILDREN,
                CW_USEDEFAULT,
                CW_USEDEFAULT,
                520,
                330,
                owner,
                null_mut(),
                instance,
                null(),
            );
            if hwnd.is_null() {
                eprintln!("{title}: {message}");
                return;
            }
            let scale = windows_sys::Win32::UI::HiDpi::GetDpiForWindow(hwnd).max(96) as f32 / 96.0;
            let Ok(painter) = Painter::new(scale) else {
                DestroyWindow(hwnd);
                return;
            };
            SetWindowLongPtrW(
                hwnd,
                GWLP_USERDATA,
                Box::into_raw(Box::new(Notice {
                    title: title.into(),
                    message: copy,
                    painter: RefCell::new(painter),
                    font: Cell::new(null_mut()),
                    brush: CreateSolidBrush(0x00080808),
                })) as isize,
            );
            if crate::window_chrome::install(hwnd, false, false).is_err() {
                DestroyWindow(hwnd);
                return;
            }
            for (id, class, label, style) in [
                (
                    2,
                    "EDIT",
                    message.as_str(),
                    WS_TABSTOP
                        | WS_VSCROLL
                        | ES_MULTILINE as u32
                        | ES_READONLY as u32
                        | ES_AUTOVSCROLL as u32,
                ),
                (1, "BUTTON", tr("Close"), WS_TABSTOP | BS_OWNERDRAW as u32),
            ] {
                let control = CreateWindowExW(
                    0,
                    wide(class).as_ptr(),
                    wide(&label.replace("\r\n", "\n").replace('\n', "\r\n")).as_ptr(),
                    WS_CHILD | WS_VISIBLE | style,
                    0,
                    0,
                    1,
                    1,
                    hwnd,
                    id as usize as *mut _,
                    instance,
                    null(),
                );
                if control.is_null() {
                    DestroyWindow(hwnd);
                    return;
                }
                if id == 1 {
                    windows_sys::Win32::UI::Shell::SetWindowSubclass(
                        control,
                        Some(hover_proc),
                        2,
                        0,
                    );
                }
            }
            WINDOWS.with(|windows| windows.borrow_mut().push(hwnd));
            set_font(hwnd);
            crate::island_surface::place(hwnd, owner, (520.0, 330.0));
            layout(hwnd);
            ShowWindow(hwnd, SW_SHOW);
            SetForegroundWindow(hwnd);
            SetFocus(GetDlgItem(hwnd, 1));
        }
    }

    pub(crate) fn translate_message(message: &MSG) -> bool {
        let windows = WINDOWS.with(|windows| windows.borrow().clone());
        unsafe {
            for hwnd in windows {
                if message.hwnd != hwnd && IsChild(hwnd, message.hwnd) == 0 {
                    continue;
                }
                if message.message == WM_KEYDOWN && matches!(message.wParam, 13 | 27) {
                    let focused = GetDlgCtrlID(GetFocus());
                    if message.wParam == 13 && crate::window_chrome::is_button(focused) {
                        SendMessageW(hwnd, WM_COMMAND, focused as usize, 0);
                    } else {
                        DestroyWindow(hwnd);
                    }
                    return true;
                }
                return IsDialogMessageW(hwnd, message) != 0;
            }
        }
        false
    }

    /// Only for startup failure, when no application dispatcher remains active.
    pub(crate) fn wait_for_notices() {
        unsafe {
            let mut message = MSG::default();
            while WINDOWS.with(|windows| !windows.borrow().is_empty()) {
                let result = GetMessageW(&mut message, null_mut(), 0, 0);
                if result <= 0 {
                    if result == 0 {
                        PostQuitMessage(message.wParam as i32);
                    }
                    break;
                }
                if !translate_message(&message) {
                    TranslateMessage(&message);
                    DispatchMessageW(&message);
                }
            }
        }
    }
}

pub(crate) use notices::{
    notice, notice_formatted, refresh_language, translate_message, wait_for_notices,
};

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn smooth_scroll_has_intermediate_frames_and_finishes_exactly() {
        use std::time::{Duration, Instant};
        let now = Instant::now();
        let mut scroll = SmoothScroll::default();
        scroll.scroll_by(60.0, 500.0, now);
        assert_eq!(scroll.offset(), 0.0);
        assert_eq!(scroll.target(), 60.0);
        assert!(scroll.moving());
        assert!(scroll.tick(now + Duration::from_millis(30)));
        assert!(scroll.offset() > 0.0 && scroll.offset() < 60.0);
        scroll.tick(now + Duration::from_millis(500));
        assert_eq!(scroll.offset(), 60.0);
        assert!(!scroll.moving());
        assert!(!scroll.tick(now + Duration::from_secs(5)));
    }

    #[test]
    fn smooth_scroll_accumulates_reverses_and_clamps_without_overshoot() {
        use std::time::{Duration, Instant};
        let now = Instant::now();
        let mut scroll = SmoothScroll::default();
        scroll.scroll_by(60.0, 100.0, now);
        scroll.scroll_by(60.0, 100.0, now);
        assert_eq!(scroll.target(), 100.0);
        scroll.tick(now + Duration::from_millis(40));
        let turning = scroll.offset();
        scroll.scroll_by(-200.0, 100.0, now + Duration::from_millis(40));
        assert_eq!(scroll.offset(), turning);
        assert_eq!(scroll.target(), 0.0);
        scroll.tick(now + Duration::from_millis(60));
        assert!(scroll.offset() > 0.0 && scroll.offset() < turning);
        scroll.tick(now + Duration::from_secs(1));
        assert_eq!(scroll.offset(), 0.0);
        scroll.jump(90.0);
        scroll.scroll_to(100.0, 100.0, now + Duration::from_secs(2));
        scroll.clamp(20.0);
        assert_eq!(scroll.offset(), 20.0);
        assert_eq!(scroll.target(), 20.0);
        assert!(!scroll.moving());
    }

    #[test]
    fn smooth_scroll_depends_on_elapsed_time_instead_of_timer_frequency() {
        use std::time::{Duration, Instant};
        let now = Instant::now();
        let mut frequent = SmoothScroll::default();
        let mut delayed = SmoothScroll::default();
        frequent.scroll_by(180.0, 500.0, now);
        delayed.scroll_by(180.0, 500.0, now);
        for millis in [10, 20, 30, 40, 50, 60, 70, 80] {
            frequent.tick(now + Duration::from_millis(millis));
        }
        delayed.tick(now + Duration::from_millis(80));
        assert_eq!(frequent.offset(), delayed.offset());
        frequent.jump(25.0);
        assert_eq!(frequent.target(), 25.0);
        assert!(!frequent.moving());
    }

    #[test]
    fn smooth_scroll_range_reduction_preserves_the_visible_position_without_reversing() {
        use std::time::{Duration, Instant};
        let now = Instant::now();
        let mut scroll = SmoothScroll::default();
        scroll.scroll_to(1212.0, 1212.0, now);
        scroll.tick(now + Duration::from_millis(40));
        assert_eq!(scroll.offset(), 700.6875);
        scroll.clamp(1212.0);
        assert!(
            scroll.moving(),
            "an unchanged layout must retain smooth motion"
        );
        scroll.clamp(1092.0);
        assert_eq!(
            scroll.offset(),
            700.6875,
            "resizing must retain a still-visible position"
        );
        scroll.tick(now + Duration::from_millis(41));
        assert_eq!(
            scroll.offset(),
            700.6875,
            "a shorter range must cancel at the current frame instead of reversing its interpolation"
        );
        assert!(!scroll.moving());
    }

    #[test]
    #[ignore = "Captures custom About notices in both languages on the interactive desktop"]
    fn native_notice_polish_preview() {
        use std::ptr::{null, null_mut};
        use windows_sys::Win32::UI::WindowsAndMessaging::*;
        let old_language = crate::i18n::language();
        struct LanguageGuard(crate::i18n::Language);
        impl Drop for LanguageGuard {
            fn drop(&mut self) {
                crate::i18n::set_language(self.0);
            }
        }
        let _language = LanguageGuard(old_language);
        let dir = std::path::PathBuf::from(
            std::env::var_os("HIYODYNA_PREVIEW_DIR").expect("preview directory"),
        );
        for language in [
            crate::i18n::Language::English,
            crate::i18n::Language::Vietnamese,
        ] {
            crate::i18n::set_language(language);
            let path = dir.join(format!("notice-{}-about.png", language.as_str()));
            let driver = std::thread::spawn(move || unsafe {
                let class: Vec<u16> = "HiyoDynaNotice".encode_utf16().chain(Some(0)).collect();
                let start = std::time::Instant::now();
                let hwnd = loop {
                    let hwnd = FindWindowW(class.as_ptr(), null());
                    let mut pid = 0;
                    GetWindowThreadProcessId(hwnd, &mut pid);
                    if pid == std::process::id()
                        && IsWindowVisible(hwnd) != 0
                        && !GetDlgItem(hwnd, 1).is_null()
                    {
                        break hwnd;
                    }
                    if start.elapsed() >= std::time::Duration::from_secs(10) {
                        return false;
                    }
                    std::thread::park_timeout(std::time::Duration::from_millis(20));
                };
                struct Close(windows_sys::Win32::Foundation::HWND);
                impl Drop for Close {
                    fn drop(&mut self) {
                        unsafe {
                            PostMessageW(self.0, WM_CLOSE, 0, 0);
                        }
                    }
                }
                let _close = Close(hwnd);
                SetWindowPos(
                    hwnd,
                    HWND_TOPMOST,
                    80,
                    80,
                    0,
                    0,
                    SWP_NOSIZE | SWP_NOACTIVATE,
                );
                windows_sys::Win32::Graphics::Gdi::RedrawWindow(
                    hwnd,
                    null(),
                    null_mut(),
                    windows_sys::Win32::Graphics::Gdi::RDW_INVALIDATE
                        | windows_sys::Win32::Graphics::Gdi::RDW_ALLCHILDREN
                        | windows_sys::Win32::Graphics::Gdi::RDW_UPDATENOW,
                );
                windows_sys::Win32::Graphics::Dwm::DwmFlush();
                crate::preferences::tests::save_window(hwnd, &path);
                true
            });
            notice_formatted(
                null_mut(),
                "HiyoDyna",
                "HiyoDyna {version}\n\nSettings: {settings}\nUpdate log: {log}",
                &[
                    ("version", env!("CARGO_PKG_VERSION")),
                    ("settings", "C:\\Fixture\\settings.ini"),
                    ("log", "C:\\Fixture\\update.log"),
                ],
            );
            wait_for_notices();
            assert!(driver.join().unwrap(), "The native notice did not appear");
        }
    }

    #[test]
    fn notice_copy_substitutes_metadata_without_parsing_or_translating_its_braces() {
        let copy = NoticeCopy {
            template: "Release: {tag}\nPublished: {published}".into(),
            values: vec![
                ("tag".into(), "v1-{published}".into()),
                ("published".into(), "2026-11-20".into()),
            ],
        };
        let rendered = copy.render();
        assert!(rendered.contains("v1-{published}"));
        assert!(rendered.ends_with("2026-11-20"));
    }

    #[test]
    fn long_vietnamese_buttons_keep_all_glyphs_at_every_dpi() {
        use crate::canvas::Painter;
        for scale in [1.0, 1.25, 1.5, 2.0] {
            let mut painter = Painter::new(scale).unwrap();
            let ink = |painter: &mut Painter, width: f32| {
                let size = (
                    (width * scale).round() as u32,
                    (29.0 * scale).round() as u32,
                );
                let mut pixels = vec![0; size.0 as usize * size.1 as usize * 4];
                button(
                    &mut Canvas::new(&mut pixels, size, painter),
                    "Bình thường",
                    ButtonStyle::default(),
                );
                pixels
                    .as_chunks::<4>()
                    .0
                    .iter()
                    .map(|pixel| u64::from(pixel[0].saturating_sub(SURFACE[0])))
                    .sum::<u64>()
            };
            let full = ink(&mut painter, 220.0);
            let narrow = ink(&mut painter, 59.0);
            assert!(
                narrow as f64 >= full as f64 * 0.90,
                "Vietnamese button lost glyphs at scale {scale}: {narrow}/{full}"
            );
        }
    }
}
