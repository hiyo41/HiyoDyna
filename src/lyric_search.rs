use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::{Arc, Mutex};

#[cfg(windows)]
pub(crate) struct Selection {
    pub key: String,
    pub generation: u64,
    pub lyrics: Arc<crate::lyrics::Lyrics>,
}

#[derive(Clone, Debug, PartialEq, Eq)]
struct Target {
    key: String,
    generation: u64,
}

enum Completion<R, L> {
    Search(Result<Vec<R>, String>),
    Fetch(Result<L, String>),
    Warning(String),
}

struct Mailbox<R, L> {
    revision: AtomicU64,
    responses: Mutex<Vec<(u64, Completion<R, L>)>>,
}

struct Job<R, L> {
    revision: u64,
    mailbox: Arc<Mailbox<R, L>>,
}

impl<R, L> Job<R, L> {
    fn current(&self) -> bool {
        self.mailbox.revision.load(Ordering::Acquire) == self.revision
    }

    fn deliver(&self, response: Completion<R, L>) {
        let mut responses = self
            .mailbox
            .responses
            .lock()
            .expect("the worker cannot panic while holding the mailbox lock");
        if self.current() {
            responses.push((self.revision, response));
        }
    }
}

enum Event<L> {
    Results(usize),
    Error(String),
    Selected(Target, L),
    Warning(String),
}

struct Session<R, L> {
    target: Target,
    mailbox: Arc<Mailbox<R, L>>,
    results: Vec<R>,
}

impl<R, L> Session<R, L> {
    fn new(target: Target) -> Self {
        Self {
            target,
            mailbox: Arc::new(Mailbox {
                revision: AtomicU64::new(1),
                responses: Mutex::new(Vec::new()),
            }),
            results: Vec::new(),
        }
    }

    fn begin(&mut self) -> Job<R, L> {
        self.close();
        Job {
            revision: self.mailbox.revision.load(Ordering::Acquire),
            mailbox: self.mailbox.clone(),
        }
    }

    fn replace(&mut self, target: Target) {
        self.close();
        self.target = target;
        self.results.clear();
    }

    fn select(&mut self, index: usize) -> Option<(R, Job<R, L>)>
    where
        R: Clone,
    {
        if self.target.key.is_empty() {
            return None;
        }
        let result = self.results.get(index)?.clone();
        Some((result, self.begin()))
    }

    fn close(&self) {
        self.mailbox.revision.fetch_add(1, Ordering::AcqRel);
        self.mailbox
            .responses
            .lock()
            .expect("the worker cannot panic while holding the mailbox lock")
            .clear();
    }

    fn poll(&mut self) -> Vec<Event<L>> {
        let responses = std::mem::take(
            &mut *self
                .mailbox
                .responses
                .lock()
                .expect("the worker cannot panic while holding the mailbox lock"),
        );
        let revision = self.mailbox.revision.load(Ordering::Acquire);
        responses
            .into_iter()
            .filter(|(job, _)| *job == revision)
            .map(|(_, response)| match response {
                Completion::Search(Ok(results)) => {
                    self.results = results;
                    Event::Results(self.results.len())
                }
                Completion::Search(Err(error)) | Completion::Fetch(Err(error)) => {
                    Event::Error(error)
                }
                Completion::Fetch(Ok(lyrics)) => Event::Selected(self.target.clone(), lyrics),
                Completion::Warning(warning) => Event::Warning(warning),
            })
            .collect()
    }
}

impl<R, L> Drop for Session<R, L> {
    fn drop(&mut self) {
        self.close();
    }
}

#[cfg(windows)]
mod native {
    use super::*;
    use crate::canvas::{Canvas, MUTED, Painter, Rect, WHITE};
    use crate::i18n::tr;
    use crate::lyric_provider::{self, SearchResult};
    use crate::lyrics::Lyrics;
    use crate::media::MediaSnapshot;
    use crate::native_ui::{self, BACKGROUND, ButtonStyle, SURFACE, SmoothScroll};
    use std::cell::{Cell, RefCell};
    use std::collections::VecDeque;
    use std::ptr::{null, null_mut};
    use std::time::Instant;
    use windows_sys::Win32::Foundation::{HWND, LPARAM, LRESULT, RECT, WPARAM};
    use windows_sys::Win32::Graphics::Gdi::*;
    use windows_sys::Win32::System::LibraryLoader::GetModuleHandleW;
    use windows_sys::Win32::UI::Controls::{
        DRAWITEMSTRUCT, EM_SETLIMITTEXT, ODS_DISABLED, ODS_FOCUS, ODS_SELECTED, SetScrollInfo,
    };
    use windows_sys::Win32::UI::HiDpi::{GetDpiForWindow, GetSystemMetricsForDpi};
    use windows_sys::Win32::UI::Input::KeyboardAndMouse::{GetFocus, SetFocus};
    use windows_sys::Win32::UI::Shell::{DefSubclassProc, SetWindowSubclass};
    use windows_sys::Win32::UI::WindowsAndMessaging::*;

    pub(crate) const MESSAGE: u32 = WM_APP + 43;
    const TITLE: i32 = 101;
    const ARTIST: i32 = 102;
    const SEARCH: i32 = 103;
    const RESULTS: i32 = 104;
    const APPLY: i32 = 105;
    const CANCEL: i32 = 2;
    const TITLE_LABEL: i32 = 201;
    const ARTIST_LABEL: i32 = 202;
    const STATUS: i32 = 203;
    const RESULTS_VIEWPORT: i32 = 204;
    const ROW_HEIGHT: f32 = 64.0;
    const POLL: usize = 1;
    const SCROLL_FRAME: u32 = WM_APP + 45;
    // No WS_CAPTION: DWM otherwise keeps a native title bar over our custom header.
    const STYLE: u32 = WS_POPUP | WS_THICKFRAME | WS_SYSMENU | WS_CLIPCHILDREN;
    // SS_OWNERDRAW preserves a real static control and its accessible status text.
    const STATUS_STYLE: u32 = 0x0d;

    thread_local! {
        static WINDOW: Cell<HWND> = const { Cell::new(null_mut()) };
        static SELECTIONS: RefCell<Vec<Selection>> = const { RefCell::new(Vec::new()) };
    }

    struct State {
        owner: HWND,
        session: Session<SearchResult, Arc<Lyrics>>,
        suppress_edits: bool,
        fetching: bool,
        font: HFONT,
        painter: Painter,
        surface: native_ui::WindowSurface,
        frame_pump: native_ui::FramePump,
        loading_since: Option<Instant>,
        loading_elapsed: f32,
        results_placement: Option<ResultsPlacement>,
        results_surface: Option<native_ui::WindowSurface>,
        results_name: Option<ResultsName>,
        background: Option<((u32, u32), Vec<u8>)>,
        track: (String, String, String),
        status: String,
        status_source: String,
        status_count: Option<usize>,
        brush: HBRUSH,
        field_brush: HBRUSH,
        accent: [u8; 3],
        focus: HWND,
        results_scroll: SmoothScroll,
        results_syncing: bool,
        results_timer: bool,
        row_cache: VecDeque<RowRaster>,
    }

    #[derive(PartialEq, Eq)]
    struct RowKey {
        index: usize,
        size: (u32, u32),
        dpi: u32,
        provider: String,
        duration: u64,
        timing: String,
        selected: bool,
        focused: bool,
        accent: [u8; 3],
    }

    struct RowRaster {
        key: RowKey,
        pixels: Arc<[u8]>,
    }

    #[derive(Clone, Copy, PartialEq, Eq)]
    struct ResultsPlacement {
        offset: i32,
        row: i32,
        width: i32,
        height: i32,
        max: i32,
    }

    fn result_raster(
        state: &mut State,
        index: usize,
        size: (u32, u32),
        style: ButtonStyle,
    ) -> Arc<[u8]> {
        let result = state.session.results.get(index);
        let key = RowKey {
            index,
            size,
            dpi: state.painter.scale.to_bits(),
            provider: result.map_or_else(String::new, |r| r.source.clone()),
            duration: result.map_or(0, |r| r.duration.to_bits()),
            timing: result.map_or_else(String::new, |r| r.timing.clone()),
            selected: style.selected,
            focused: style.focused,
            accent: style.accent,
        };
        if let Some(position) = state.row_cache.iter().position(|row| row.key == key) {
            let row = state
                .row_cache
                .remove(position)
                .expect("row_cache position came from position()");
            let pixels = row.pixels.clone();
            state.row_cache.push_back(row);
            return pixels;
        }
        let mut pixels = vec![0; (size.0 * size.1 * 4) as usize];
        let mut c = Canvas::new(&mut pixels, size, &mut state.painter);
        if let Some(result) = state.session.results.get(index) {
            result_row(&mut c, result, style);
        } else {
            c.rect(c.clip, 0.0, BACKGROUND, 1.0);
        }
        let pixels: Arc<[u8]> = pixels.into();
        // Results are small, but DPI/resize and focus variants must not grow an
        // unbounded bitmap cache. Evict old entries before retaining a new one.
        let bytes = pixels.len();
        while !state.row_cache.is_empty()
            && (state.row_cache.len() >= 48
                || state
                    .row_cache
                    .iter()
                    .map(|row| row.pixels.len())
                    .sum::<usize>()
                    + bytes
                    > 12 * 1024 * 1024)
        {
            state.row_cache.pop_front();
        }
        if bytes <= 12 * 1024 * 1024 {
            state.row_cache.push_back(RowRaster {
                key,
                pixels: pixels.clone(),
            });
        }
        pixels
    }

    unsafe fn results(hwnd: HWND) -> HWND {
        unsafe {
            let viewport = GetDlgItem(hwnd, RESULTS_VIEWPORT);
            GetDlgItem(if viewport.is_null() { hwnd } else { viewport }, RESULTS)
        }
    }

    struct ResultsName {
        list: HWND,
        service: Option<windows::Win32::UI::Accessibility::IAccPropServices>,
        initialized: bool,
    }

    impl ResultsName {
        unsafe fn new(list: HWND) -> windows::core::Result<Self> {
            use windows::Win32::System::Com::{
                CLSCTX_INPROC_SERVER, COINIT_APARTMENTTHREADED, CoCreateInstance, CoInitializeEx,
            };
            use windows::Win32::UI::Accessibility::CAccPropServices;
            unsafe {
                let mut annotation = Self {
                    list,
                    service: None,
                    initialized: CoInitializeEx(None, COINIT_APARTMENTTHREADED).is_ok(),
                };
                annotation.service = Some(CoCreateInstance(
                    &CAccPropServices,
                    None,
                    CLSCTX_INPROC_SERVER,
                )?);
                Ok(annotation)
            }
        }

        unsafe fn update(&self, label: &str) -> windows::core::Result<()> {
            use windows::Win32::UI::Accessibility::{Name_Property_GUID, PROPID_ACC_NAME};
            unsafe {
                let name = wide(label);
                for property in [PROPID_ACC_NAME, Name_Property_GUID] {
                    self.service
                        .as_ref()
                        .expect("ResultsName::new initialized the accessibility service")
                        .SetHwndPropStr(
                            windows::Win32::Foundation::HWND(self.list),
                            OBJID_CLIENT as u32,
                            0,
                            property,
                            windows::core::PCWSTR(name.as_ptr()),
                        )?;
                }
                Ok(())
            }
        }
    }

    impl Drop for ResultsName {
        fn drop(&mut self) {
            use windows::Win32::UI::Accessibility::{Name_Property_GUID, PROPID_ACC_NAME};
            unsafe {
                if let Some(service) = self.service.take() {
                    if IsWindow(self.list) != 0 {
                        let _ = service.ClearHwndProps(
                            windows::Win32::Foundation::HWND(self.list),
                            OBJID_CLIENT as u32,
                            0,
                            &[PROPID_ACC_NAME, Name_Property_GUID],
                        );
                    }
                    drop(service);
                }
                if self.initialized {
                    windows::Win32::System::Com::CoUninitialize();
                }
            }
        }
    }

    unsafe fn name_results(hwnd: HWND, label: &str) -> bool {
        unsafe {
            let list = results(hwnd);
            SetWindowTextW(list, wide(label).as_ptr());
            // Annotate the native provider, keeping its COM apartment alive
            // until the search window closes; WM_GETTEXT alone is not a UIA name.
            let Some(existing) = access(hwnd, |state| state.results_name.take()) else {
                return false;
            };
            let annotation = match existing.map_or_else(|| ResultsName::new(list), Ok) {
                Ok(annotation) => annotation,
                Err(error) => {
                    eprintln!("Lyrics result accessible name: {error}");
                    return false;
                }
            };
            let result = annotation.update(label);
            access(hwnd, |state| state.results_name = Some(annotation));
            if let Err(error) = result {
                eprintln!("Lyrics result accessible name: {error}");
                return false;
            }
            true
        }
    }

    // Move the actual LISTBOX by the fractional-row remainder inside a clipping
    // parent. Native item rectangles, input and accessibility stay on one HWND;
    // no separately painted or manually hit-tested representation is needed.
    unsafe fn install_results_viewport(hwnd: HWND) -> bool {
        unsafe {
            let instance = GetModuleHandleW(null());
            let class_name = wide("HiyoDynaResultViewport");
            RegisterClassW(&WNDCLASSW {
                lpfnWndProc: Some(viewport_proc),
                hInstance: instance,
                lpszClassName: class_name.as_ptr(),
                hCursor: LoadCursorW(null_mut(), IDC_ARROW),
                ..Default::default()
            });
            let list = GetDlgItem(hwnd, RESULTS);
            let previous = GetWindow(list, GW_HWNDPREV);
            let viewport = CreateWindowExW(
                WS_EX_CONTROLPARENT,
                class_name.as_ptr(),
                null(),
                WS_CHILD | WS_VISIBLE | WS_CLIPCHILDREN | WS_VSCROLL,
                0,
                0,
                1,
                1,
                hwnd,
                RESULTS_VIEWPORT as usize as *mut _,
                instance,
                null(),
            );
            if viewport.is_null() {
                return false;
            }
            SetParent(list, viewport);
            if GetParent(list) != viewport {
                return false;
            }
            crate::dark_scrollbar::apply(viewport);
            crate::dark_scrollbar::apply(list);
            // Dialog traversal descends into control parents in sibling order.
            // Keep this parent at the original list's place before the actions.
            if SetWindowPos(
                viewport,
                previous,
                0,
                0,
                0,
                0,
                SWP_NOMOVE | SWP_NOSIZE | SWP_NOACTIVATE,
            ) == 0
            {
                return false;
            }
            SetWindowSubclass(list, Some(results_proc), 3, hwnd as usize) != 0
        }
    }

    unsafe fn results_metrics(hwnd: HWND) -> (HWND, i32, i32, f32) {
        unsafe {
            let viewport = GetDlgItem(hwnd, RESULTS_VIEWPORT);
            let mut rect = RECT::default();
            GetClientRect(viewport, &mut rect);
            let row = SendMessageW(results(hwnd), LB_GETITEMHEIGHT, 0, 0).max(1) as i32;
            let count = SendMessageW(results(hwnd), LB_GETCOUNT, 0, 0).max(0) as i32;
            (
                viewport,
                row,
                rect.bottom.max(0),
                (count * row - rect.bottom).max(0) as f32,
            )
        }
    }

    unsafe fn position_results(hwnd: HWND) {
        let started = native_ui::trace_start();
        unsafe {
            let (viewport, row, height, max) = results_metrics(hwnd);
            if viewport.is_null() {
                return;
            }
            let Some((offset, previous)) = access(hwnd, |state| {
                state.results_scroll.clamp(max);
                if !state.results_scroll.moving() {
                    if state.loading_since.is_none() {
                        state.frame_pump.stop();
                    }
                    state.results_timer = false;
                }
                state.results_syncing = true;
                (
                    state.results_scroll.offset().round() as i32,
                    state.results_placement,
                )
            }) else {
                return;
            };
            let mut rect = RECT::default();
            GetClientRect(viewport, &mut rect);
            let list = results(hwnd);
            let remainder = offset % row;
            let placement = ResultsPlacement {
                offset,
                row,
                width: rect.right.max(0),
                height,
                max: max as i32,
            };
            let geometry_changed = previous.is_none_or(|old| {
                old.width != placement.width || old.height != height || old.row != row
            });
            let moved = previous.is_none_or(|old| old.offset != offset) || geometry_changed;
            let top = SendMessageW(list, LB_GETTOPINDEX, 0, 0);
            if previous == Some(placement) && top == (offset / row) as isize {
                access(hwnd, |state| state.results_syncing = false);
                native_ui::trace_ui("lyric_search", "reposition", started);
                return;
            }
            let pending = GetUpdateRect(list, null_mut(), 0) != 0;
            let delta = offset - previous.map_or(offset, |old| old.offset);
            let mut reuse = moved && !geometry_changed && !pending && delta.abs() < height;
            if reuse {
                let dc = GetDC(list);
                reuse = access(hwnd, |state| {
                    let Some(surface) = state.results_surface.as_mut() else {
                        return false;
                    };
                    if surface.resize(placement.width as u32, height as u32) {
                        let top = previous
                            .expect("geometry_changed is false, so a previous placement exists")
                            .offset
                            % row;
                        surface.copy_from(
                            dc,
                            RECT {
                                left: 0,
                                top,
                                right: placement.width,
                                bottom: top + height,
                            },
                        )
                    } else {
                        false
                    }
                })
                .unwrap_or(false);
                ReleaseDC(list, dc);
            }
            // Keep the native scrollbar for its standard accessibility provider,
            // but clip its nonclient strip beyond the viewport's right edge.
            // LBS_DISABLENOSCROLL keeps this width stable for short/empty lists.
            let scrollbar_width = GetSystemMetricsForDpi(SM_CXVSCROLL, GetDpiForWindow(list));
            // The bottom edge stays at the viewport bottom. In particular,
            // native auto-scroll does not count an extra hidden complete row.
            if geometry_changed || previous.is_none_or(|old| old.offset % row != remainder) {
                SetWindowPos(
                    list,
                    null_mut(),
                    0,
                    -remainder,
                    rect.right.max(0) + scrollbar_width,
                    height + remainder,
                    SWP_NOZORDER | SWP_NOACTIVATE | SWP_NOREDRAW,
                );
            }
            if top != (offset / row) as isize {
                SendMessageW(list, LB_SETTOPINDEX, (offset / row) as usize, 0);
            }
            let info = SCROLLINFO {
                cbSize: std::mem::size_of::<SCROLLINFO>() as u32,
                fMask: SIF_RANGE | SIF_PAGE | SIF_POS | SIF_DISABLENOSCROLL,
                nMin: 0,
                nMax: max as i32 + height.saturating_sub(1),
                nPage: height as u32,
                nPos: offset,
                ..Default::default()
            };
            if previous.is_none_or(|old| {
                old.max != placement.max || old.height != height || old.offset != offset
            }) {
                SetScrollInfo(viewport, SB_VERT, &info, 0);
                RedrawWindow(
                    viewport,
                    null(),
                    null_mut(),
                    RDW_INVALIDATE | RDW_FRAME | RDW_UPDATENOW | RDW_NOCHILDREN,
                );
            }
            access(hwnd, |state| {
                state.results_syncing = false;
                state.results_placement = Some(placement);
            });
            if moved {
                let visible = RECT {
                    left: 0,
                    top: remainder,
                    right: placement.width,
                    bottom: height + remainder,
                };
                if !reuse {
                    InvalidateRect(list, &visible, 0);
                    UpdateWindow(list);
                } else {
                    // Native LB_SETTOPINDEX can dirty the entire list. Preserve
                    // the already drawn viewport and repaint only new pixels.
                    ValidateRect(list, null());
                    let dc = GetDC(list);
                    let restored = access(hwnd, |state| {
                        state.results_surface.as_ref().is_some_and(|surface| {
                            surface.copy_to(
                                dc,
                                RECT {
                                    left: 0,
                                    top: delta.max(0),
                                    right: placement.width,
                                    bottom: height + delta.min(0),
                                },
                                0,
                                remainder + (-delta).max(0),
                            )
                        })
                    })
                    .unwrap_or(false);
                    ReleaseDC(list, dc);
                    if !restored {
                        InvalidateRect(list, &visible, 0);
                        UpdateWindow(list);
                        return;
                    }
                    if let Some(dirty) = exposed_scroll_strip(visible, delta) {
                        InvalidateRect(list, &dirty, 0);
                        UpdateWindow(list);
                    }
                }
            }
        }
        native_ui::trace_ui("lyric_search", "reposition", started);
    }

    fn exposed_scroll_strip(visible: RECT, delta: i32) -> Option<RECT> {
        let height = (visible.bottom - visible.top).max(0);
        if delta == 0 || height == 0 {
            None
        } else if delta >= height || delta <= -height {
            Some(visible)
        } else if delta > 0 {
            Some(RECT {
                top: visible.bottom - delta,
                ..visible
            })
        } else {
            Some(RECT {
                bottom: visible.top - delta,
                ..visible
            })
        }
    }

    unsafe fn stop_results_timer(hwnd: HWND) {
        native_ui::set_scroll_animating(false);
        unsafe {
            access(hwnd, |state| {
                if state.loading_since.is_none() {
                    state.frame_pump.stop();
                }
                state.results_timer = false;
            });
        }
    }

    unsafe fn animate_results(hwnd: HWND) {
        unsafe {
            let start = access(hwnd, |state| {
                state.results_scroll.moving() && !state.results_timer
            })
            .unwrap_or(false);
            if start {
                let active = access(hwnd, |state| {
                    state.frame_pump.start(hwnd);
                    state.results_timer = state.frame_pump.is_active();
                    state.results_timer
                })
                .unwrap_or(false);
                if !active {
                    access(hwnd, |state| {
                        state.results_scroll.jump(state.results_scroll.target())
                    });
                    position_results(hwnd);
                }
            }
            if access(hwnd, |state| !state.results_scroll.moving()).unwrap_or(true) {
                stop_results_timer(hwnd);
            }
        }
    }

    unsafe fn tick_results(hwnd: HWND, now: Instant) {
        unsafe {
            let loading = access(hwnd, |state| {
                if let Some(started) = state.loading_since {
                    state.loading_elapsed = now.saturating_duration_since(started).as_secs_f32();
                    true
                } else {
                    false
                }
            })
            .unwrap_or(false);
            if loading {
                invalidate_loading(hwnd);
                // The loading band is a moving indicator; paint it in the frame
                // instead of leaving it to an idle message queue.
                UpdateWindow(hwnd);
            }
            let (changed, moving) = access(hwnd, |state| {
                let changed = state.results_scroll.tick(now);
                (changed, state.results_scroll.moving())
            })
            .unwrap_or((false, false));
            native_ui::set_scroll_animating(moving);
            // Wheel retargeting can already advance and settle SmoothScroll.
            // Apply its final offset before stopping, even if this tick is idle.
            if changed || !moving {
                position_results(hwnd);
            }
            if !moving {
                stop_results_timer(hwnd);
            }
        }
    }

    unsafe fn reveal_result(hwnd: HWND) {
        unsafe {
            let (_, row, height, max) = results_metrics(hwnd);
            let selected = SendMessageW(results(hwnd), LB_GETCURSEL, 0, 0);
            if selected < 0 {
                return;
            }
            access(hwnd, |state| {
                let offset = state.results_scroll.offset();
                let top = selected as f32 * row as f32;
                let next = if top < offset {
                    top
                } else if top + row as f32 > offset + height as f32 {
                    top + row as f32 - height as f32
                } else {
                    offset
                };
                state.results_scroll.jump(next.clamp(0.0, max));
            });
            stop_results_timer(hwnd);
            position_results(hwnd);
        }
    }

    unsafe fn wheel_results(hwnd: HWND, wparam: WPARAM, now: Instant) {
        unsafe {
            let (_, row, height, max) = results_metrics(hwnd);
            let mut lines = 3_u32;
            SystemParametersInfoW(
                SPI_GETWHEELSCROLLLINES,
                0,
                (&mut lines as *mut u32).cast(),
                0,
            );
            let pixels = if lines == u32::MAX {
                height.max(row) as f32
            } else {
                row as f32 * lines as f32
            };
            let delta = -f32::from((wparam >> 16) as i16) / 120.0 * pixels;
            access(hwnd, |state| {
                state.results_scroll.scroll_by(delta, max, now)
            });
            animate_results(hwnd);
        }
    }

    // Retain the native pixel scrollbar's hit testing and range, but suppress
    // its light system rendering. The list's native row scrollbar remains
    // clipped, retaining its UIA provider and row-to-pixel mapping.
    unsafe fn paint_viewport_scrollbar(hwnd: HWND, dc: HDC) {
        unsafe {
            let mut bar = SCROLLBARINFO {
                cbSize: std::mem::size_of::<SCROLLBARINFO>() as u32,
                ..Default::default()
            };
            if GetScrollBarInfo(hwnd, OBJID_VSCROLL, &mut bar) == 0 {
                return;
            }
            let mut outer = RECT::default();
            GetWindowRect(hwnd, &mut outer);
            OffsetRect(&mut bar.rcScrollBar, -outer.left, -outer.top);
            let brush = CreateSolidBrush(crate::theme::colorref(BACKGROUND));
            FillRect(dc, &bar.rcScrollBar, brush);
            DeleteObject(brush);
            let owner = GetParent(hwnd);
            let scrollable =
                access(owner, |s| s.results_placement.is_some_and(|p| p.max > 0)).unwrap_or(false);
            if scrollable && bar.xyThumbBottom > bar.xyThumbTop {
                let scale = access(owner, |s| s.painter.scale).unwrap_or(1.0);
                let inset = ((5.0 * scale).round() as i32)
                    .min((bar.rcScrollBar.right - bar.rcScrollBar.left) / 3);
                let brush = CreateSolidBrush(crate::theme::colorref(crate::theme::BORDER_STRONG));
                let old_brush = SelectObject(dc, brush);
                let old_pen = SelectObject(dc, GetStockObject(NULL_PEN));
                RoundRect(
                    dc,
                    bar.rcScrollBar.left + inset,
                    bar.rcScrollBar.top + bar.xyThumbTop,
                    bar.rcScrollBar.right - inset,
                    bar.rcScrollBar.top + bar.xyThumbBottom,
                    (6.0 * scale).round() as i32,
                    (6.0 * scale).round() as i32,
                );
                SelectObject(dc, old_pen);
                SelectObject(dc, old_brush);
                DeleteObject(brush);
            }
        }
    }

    unsafe extern "system" fn viewport_proc(
        hwnd: HWND,
        message: u32,
        wparam: WPARAM,
        lparam: LPARAM,
    ) -> LRESULT {
        unsafe {
            let owner = GetParent(hwnd);
            match message {
                WM_COMMAND | WM_DRAWITEM | WM_CTLCOLORLISTBOX | WM_MEASUREITEM => {
                    SendMessageW(owner, message, wparam, lparam)
                }
                WM_ERASEBKGND => 1,
                WM_NCPAINT => {
                    let dc = GetWindowDC(hwnd);
                    paint_viewport_scrollbar(hwnd, dc);
                    ReleaseDC(hwnd, dc);
                    0
                }
                WM_NCACTIVATE => 1,
                WM_PRINT => {
                    // Memory-DC child printing does not inherit HWND clipping.
                    // Keep the translated list and its native scrollbar inside
                    // the viewport client, just as ordinary window painting does.
                    let dc = wparam as HDC;
                    let saved = SaveDC(dc);
                    if saved == 0 {
                        return 0;
                    }
                    let mut client = RECT::default();
                    GetClientRect(hwnd, &mut client);
                    IntersectClipRect(dc, client.left, client.top, client.right, client.bottom);
                    let result =
                        DefWindowProcW(hwnd, message, wparam, lparam & !(PRF_NONCLIENT as isize));
                    RestoreDC(dc, saved);
                    if lparam & PRF_NONCLIENT as isize != 0 {
                        paint_viewport_scrollbar(hwnd, dc);
                    }
                    result
                }
                WM_MOUSEWHEEL => {
                    wheel_results(owner, wparam, Instant::now());
                    0
                }
                WM_VSCROLL => {
                    let (_, row, height, max) = results_metrics(owner);
                    let mut info = SCROLLINFO {
                        cbSize: std::mem::size_of::<SCROLLINFO>() as u32,
                        fMask: SIF_TRACKPOS,
                        ..Default::default()
                    };
                    GetScrollInfo(hwnd, SB_VERT, &mut info);
                    access(owner, |state| {
                        let offset = state.results_scroll.offset();
                        let target = match wparam as u16 as i32 {
                            SB_LINEUP => offset - row as f32,
                            SB_LINEDOWN => offset + row as f32,
                            SB_PAGEUP => offset - height as f32,
                            SB_PAGEDOWN => offset + height as f32,
                            SB_THUMBPOSITION | SB_THUMBTRACK => info.nTrackPos as f32,
                            SB_TOP => 0.0,
                            SB_BOTTOM => max,
                            _ => offset,
                        };
                        state.results_scroll.jump(target.clamp(0.0, max));
                    });
                    stop_results_timer(owner);
                    position_results(owner);
                    0
                }
                WM_SIZE => {
                    position_results(owner);
                    0
                }
                _ => DefWindowProcW(hwnd, message, wparam, lparam),
            }
        }
    }

    unsafe extern "system" fn results_proc(
        hwnd: HWND,
        message: u32,
        wparam: WPARAM,
        lparam: LPARAM,
        _id: usize,
        owner: usize,
    ) -> LRESULT {
        unsafe {
            let owner = owner as HWND;
            match message {
                WM_PRINT => {
                    // This native scrollbar is clipped out of the live viewport.
                    // Printing its nonclient area resets the inherited DC clip
                    // and leaks the upper arrow during fractional scrolling.
                    return DefSubclassProc(
                        hwnd,
                        message,
                        wparam,
                        lparam & !(PRF_NONCLIENT as isize),
                    );
                }
                WM_MOUSEWHEEL => {
                    wheel_results(owner, wparam, Instant::now());
                    return 0;
                }
                WM_VSCROLL => {
                    if matches!(wparam as u16 as i32, SB_THUMBPOSITION | SB_THUMBTRACK) {
                        // Native listbox clients (including UIA) pass a row in
                        // HIWORD. It is not the clipping viewport's pixel thumb
                        // track position; forwarding it there discards the row.
                        let (_, row, _, max) = results_metrics(owner);
                        let top = (wparam >> 16) as u16;
                        access(owner, |state| {
                            state
                                .results_scroll
                                .jump((f32::from(top) * row as f32).min(max));
                        });
                        stop_results_timer(owner);
                        position_results(owner);
                        return 0;
                    }
                    return SendMessageW(GetParent(hwnd), message, wparam, lparam);
                }
                _ => {}
            }
            let synced = access(owner, |state| state.results_syncing).unwrap_or(true);
            let keyboard = matches!(message, WM_KEYDOWN | WM_CHAR);
            let previous = if keyboard && !synced {
                Some((
                    SendMessageW(hwnd, LB_GETCURSEL, 0, 0),
                    SendMessageW(hwnd, LB_GETTOPINDEX, 0, 0),
                ))
            } else {
                None
            };
            let result = DefSubclassProc(hwnd, message, wparam, lparam);
            if !synced {
                match message {
                    WM_KEYDOWN | WM_CHAR => {
                        let current = (
                            SendMessageW(hwnd, LB_GETCURSEL, 0, 0),
                            SendMessageW(hwnd, LB_GETTOPINDEX, 0, 0),
                        );
                        let navigation = message == WM_KEYDOWN && (0x21..=0x28).contains(&wparam);
                        // Unmatched type-ahead and modifier keys are native
                        // no-ops. Revealing the unchanged caret here would undo
                        // a concurrent UIA/scrollbar scroll to another row.
                        if navigation || previous != Some(current) {
                            reveal_result(owner);
                        }
                    }
                    LB_SETCURSEL | LB_SETCARETINDEX => reveal_result(owner),
                    LB_SETTOPINDEX => {
                        let (_, row, _, max) = results_metrics(owner);
                        let top = SendMessageW(hwnd, LB_GETTOPINDEX, 0, 0).max(0);
                        access(owner, |state| {
                            state
                                .results_scroll
                                .jump((top as f32 * row as f32).min(max))
                        });
                        stop_results_timer(owner);
                        position_results(owner);
                    }
                    WM_MOUSEMOVE | WM_LBUTTONDOWN | WM_LBUTTONUP | WM_LBUTTONDBLCLK | WM_TIMER => {
                        let (_, row, _, max) = results_metrics(owner);
                        let top = SendMessageW(hwnd, LB_GETTOPINDEX, 0, 0).max(0);
                        let changed = access(owner, |state| {
                            let current = state.results_scroll.offset().round() as i32 / row;
                            if top as i32 == current {
                                return false;
                            }
                            state
                                .results_scroll
                                .jump((top as f32 * row as f32).min(max));
                            true
                        })
                        .unwrap_or(false);
                        if changed {
                            stop_results_timer(owner);
                            position_results(owner);
                        }
                    }
                    _ => {}
                }
            }
            result
        }
    }

    fn wide(text: &str) -> Vec<u16> {
        text.encode_utf16().chain(Some(0)).collect()
    }

    fn target(snapshot: &MediaSnapshot) -> Target {
        Target {
            key: snapshot.key.clone(),
            generation: snapshot.generation,
        }
    }

    // The closure must only read/mutate Rust state. No Win32 calls while borrowed:
    // control creation, text changes, and destruction can reenter the window proc.
    unsafe fn access<T>(hwnd: HWND, f: impl FnOnce(&mut State) -> T) -> Option<T> {
        unsafe {
            let ptr = GetWindowLongPtrW(hwnd, GWLP_USERDATA) as *const RefCell<State>;
            if ptr.is_null() {
                return None;
            }
            Some(f(&mut (*ptr).borrow_mut()))
        }
    }

    unsafe fn text(hwnd: HWND, id: i32, value: &str) {
        unsafe {
            if id == STATUS {
                status(hwnd, value, None);
                return;
            }
            SetWindowTextW(GetDlgItem(hwnd, id), wide(value).as_ptr());
            InvalidateRect(GetDlgItem(hwnd, id), null(), 0);
        }
    }

    unsafe fn status(hwnd: HWND, source: &str, count: Option<usize>) {
        let label = count.map_or_else(
            || tr(source).to_owned(),
            |count| crate::i18n::search_result_count(count, source.contains("providers could not")),
        );
        unsafe {
            access(hwnd, |state| {
                state.status = label.clone();
                state.status_source = source.into();
                state.status_count = count;
                let busy = source.starts_with("Searching") || source.starts_with("Fetching");
                if busy {
                    if state.loading_since.is_none() {
                        state.loading_since = Some(Instant::now());
                        state.loading_elapsed = 0.0;
                    }
                    state.frame_pump.start(hwnd);
                } else {
                    state.loading_since = None;
                    if !state.results_scroll.moving() {
                        state.frame_pump.stop();
                    }
                }
            });
            invalidate_loading(hwnd);
            SetWindowTextW(GetDlgItem(hwnd, STATUS), wide(&label).as_ptr());
            InvalidateRect(GetDlgItem(hwnd, STATUS), null(), 0);
        }
    }

    fn status_tint(source: &str, count: Option<usize>) -> [u8; 3] {
        use crate::theme;
        if source.starts_with("Searching") || source.starts_with("Fetching") {
            theme::ICE
        } else if source.contains("failed")
            || source.starts_with("Could not")
            || source.starts_with("Enter a song")
        {
            theme::DANGER
        } else if source == "Instrumental" {
            theme::LAVENDER
        } else if source.starts_with("No active") || source.starts_with("Play a media") {
            theme::DIM
        } else if source.starts_with("No synchronized") || source.contains("providers could not") {
            theme::AMBER
        } else if count.is_some_and(|n| n > 0) || source.starts_with("Choose Use") {
            theme::SUCCESS
        } else {
            theme::MUTED
        }
    }

    fn loading_band(layout: &SearchLayout) -> Rect {
        Rect::new(
            layout.search_card.x + 12.0,
            layout.search_card.y + layout.search_card.h - 3.0,
            (layout.search_card.w - 24.0).max(0.0),
            2.0,
        )
    }

    unsafe fn invalidate_loading(hwnd: HWND) {
        unsafe {
            let mut client = RECT::default();
            GetClientRect(hwnd, &mut client);
            let s = access(hwnd, |state| state.painter.scale).unwrap_or(1.0);
            let band = loading_band(&SearchLayout::new(
                client.right as f32 / s,
                client.bottom as f32 / s,
            ));
            let dirty = RECT {
                left: (band.x * s).floor() as i32,
                top: (band.y * s).floor() as i32,
                right: ((band.x + band.w) * s).ceil() as i32,
                bottom: ((band.y + band.h) * s).ceil() as i32,
            };
            InvalidateRect(hwnd, &dirty, 0);
        }
    }

    fn draw_loading(c: &mut Canvas<'_>, elapsed: f32, accent: [u8; 3]) {
        let band = loading_band(&SearchLayout::new(
            c.size.0 as f32 / c.painter.scale,
            c.size.1 as f32 / c.painter.scale,
        ));
        if band.w <= 0.0 {
            return;
        }
        let width = band.w * 0.25;
        let offset = (elapsed * 160.0) % band.w;
        c.rect(
            Rect::new(band.x + offset, band.y, width.min(band.w - offset), band.h),
            0.0,
            accent,
            1.0,
        );
        if offset + width > band.w {
            c.rect(
                Rect::new(band.x, band.y, offset + width - band.w, band.h),
                0.0,
                accent,
                1.0,
            );
        }
    }

    unsafe fn input(hwnd: HWND, id: i32) -> String {
        unsafe {
            let control = GetDlgItem(hwnd, id);
            let mut buffer = vec![0; GetWindowTextLengthW(control).max(0) as usize + 1];
            let len = GetWindowTextW(control, buffer.as_mut_ptr(), buffer.len() as i32);
            String::from_utf16_lossy(&buffer[..len.max(0) as usize])
                .trim()
                .to_owned()
        }
    }

    unsafe fn apply_enabled(hwnd: HWND, enabled: bool) {
        unsafe {
            let has_target =
                access(hwnd, |state| !state.session.target.key.is_empty()).unwrap_or(false);
            windows_sys::Win32::UI::Input::KeyboardAndMouse::EnableWindow(
                GetDlgItem(hwnd, APPLY),
                i32::from(enabled && has_target),
            );
        }
    }

    unsafe fn clear_results(hwnd: HWND) {
        unsafe {
            stop_results_timer(hwnd);
            access(hwnd, |state| {
                state.results_scroll.jump(0.0);
                state.row_cache.clear();
            });
            SendMessageW(results(hwnd), LB_RESETCONTENT, 0, 0);
            position_results(hwnd);
            apply_enabled(hwnd, false);
        }
    }

    unsafe fn reset(hwnd: HWND, snapshot: &MediaSnapshot) {
        unsafe {
            access(hwnd, |state| {
                state.session.replace(target(snapshot));
                state.fetching = false;
                state.suppress_edits = true;
                state.track = (
                    snapshot.title.clone(),
                    snapshot.artist.clone(),
                    snapshot.source.clone(),
                );
                state.background = None;
            });
            text(hwnd, TITLE, &snapshot.title);
            text(hwnd, ARTIST, &snapshot.artist);
            access(hwnd, |state| state.suppress_edits = false);
            clear_results(hwnd);
            text(
                hwnd,
                STATUS,
                if snapshot.key.is_empty() {
                    "No active media track. You can search; play a track before applying lyrics."
                } else {
                    "Enter a title and artist, then choose Search."
                },
            );
            InvalidateRect(hwnd, null(), 0);
        }
    }

    unsafe fn edited(hwnd: HWND) {
        unsafe {
            let changed = access(hwnd, |state| {
                if state.suppress_edits {
                    return false;
                }
                state.session.close();
                state.session.results.clear();
                state.fetching = false;
                true
            })
            .unwrap_or(false);
            if changed {
                clear_results(hwnd);
                text(
                    hwnd,
                    STATUS,
                    "Search again to find lyrics for these details.",
                );
            }
        }
    }

    unsafe fn search(hwnd: HWND) {
        unsafe {
            let title = input(hwnd, TITLE);
            let artist = input(hwnd, ARTIST);
            if title.is_empty() {
                text(hwnd, STATUS, "Enter a song title to search.");
                SetFocus(GetDlgItem(hwnd, TITLE));
                return;
            }
            let Some(job) = access(hwnd, |state| {
                state.fetching = false;
                state.session.results.clear();
                state.session.begin()
            }) else {
                return;
            };
            clear_results(hwnd);
            text(hwnd, STATUS, "Searching lyric providers...");
            let worker = std::thread::Builder::new()
                .name("lyric-search".into())
                .spawn(
                    move || match lyric_provider::search(&title, &artist, || job.current()) {
                        Ok(response) => {
                            job.deliver(Completion::Search(Ok(response.results)));
                            if !response.warnings.is_empty() {
                                job.deliver(Completion::Warning(response.warnings.join("; ")));
                            }
                        }
                        Err(error) => job.deliver(Completion::Search(Err(error))),
                    },
                );
            if let Err(error) = worker {
                eprintln!("Could not start lyric search: {error}");
                text(
                    hwnd,
                    STATUS,
                    "Could not start lyric search. Please try again.",
                );
            }
        }
    }

    unsafe fn fetch(hwnd: HWND) {
        unsafe {
            if !access(hwnd, |state| !state.session.target.key.is_empty()).unwrap_or(false) {
                text(
                    hwnd,
                    STATUS,
                    "Play a media track before applying lyrics. You can still search.",
                );
                return;
            }
            let index = SendMessageW(results(hwnd), LB_GETCURSEL, 0, 0);
            if index < 0 {
                return;
            }
            let work = access(hwnd, |state| {
                if state.fetching {
                    return None;
                }
                let work = state.session.select(index as usize)?;
                state.fetching = true;
                Some(work)
            })
            .flatten();
            let Some((result, job)) = work else {
                return;
            };
            apply_enabled(hwnd, false);
            text(hwnd, STATUS, "Fetching selected lyrics...");
            let worker = std::thread::Builder::new()
                .name("lyric-selection".into())
                .spawn(move || {
                    let response = lyric_provider::fetch(&result, || job.current()).map(Arc::new);
                    job.deliver(Completion::Fetch(response));
                });
            if let Err(error) = worker {
                access(hwnd, |state| state.fetching = false);
                apply_enabled(hwnd, true);
                eprintln!("Could not fetch lyrics: {error}");
                text(hwnd, STATUS, "Could not fetch lyrics. Please try again.");
            }
        }
    }

    fn candidate_label(result: &SearchResult) -> String {
        let duration = if result.duration.is_finite() && result.duration > 0.0 {
            let seconds = result.duration.round() as u64;
            format!("{}:{:02}", seconds / 60, seconds % 60)
        } else {
            tr("unknown duration").into()
        };
        let album = if result.album.trim().is_empty() {
            String::new()
        } else {
            format!(" | {}", result.album)
        };
        // Listbox rows are single-line even when remote metadata contains breaks.
        format!(
            "{} - {}{} | {} | {} | {}",
            result.title,
            result.artist,
            album,
            result.source,
            tr(&result.timing),
            duration
        )
        .replace(['\r', '\n', '\t'], " ")
    }

    unsafe fn poll(hwnd: HWND) {
        unsafe {
            let events = access(hwnd, |state| state.session.poll()).unwrap_or_default();
            for event in events {
                match event {
                    Event::Results(count) => {
                        let labels = access(hwnd, |state| {
                            state
                                .session
                                .results
                                .iter()
                                .map(candidate_label)
                                .collect::<Vec<_>>()
                        })
                        .unwrap_or_default();
                        clear_results(hwnd);
                        let list = results(hwnd);
                        SendMessageW(list, WM_SETREDRAW, 0, 0);
                        for label in labels {
                            let row = wide(&label);
                            SendMessageW(list, LB_ADDSTRING, 0, row.as_ptr() as isize);
                        }
                        SendMessageW(list, WM_SETREDRAW, 1, 0);
                        // Result replacement changes every row even when the
                        // viewport's rounded position and size stay unchanged.
                        InvalidateRect(list, null(), 0);
                        position_results(hwnd);
                        if count == 0 {
                            text(
                                hwnd,
                                STATUS,
                                "No synchronized lyrics found. Try a different title or artist.",
                            );
                        } else {
                            SendMessageW(list, LB_SETCURSEL, 0, 0);
                            apply_enabled(hwnd, true);
                            status(
                                hwnd,
                                "{count} result(s). Choose a match, then Use selected lyrics.",
                                Some(count),
                            );
                        }
                        // Results replace every row at once; present the list,
                        // status and action state as one complete frame.
                        RedrawWindow(
                            hwnd,
                            null(),
                            null_mut(),
                            RDW_INVALIDATE | RDW_UPDATENOW | RDW_ALLCHILDREN,
                        );
                    }
                    Event::Error(error) => {
                        let available = access(hwnd, |state| {
                            state.fetching = false;
                            !state.session.results.is_empty()
                        })
                        .unwrap_or(false);
                        apply_enabled(hwnd, available);
                        eprintln!("Lyrics request failed: {error}");
                        text(hwnd, STATUS, "Lyrics request failed. Please try again.");
                    }
                    Event::Warning(warning) => {
                        let count = access(hwnd, |state| state.session.results.len()).unwrap_or(0);
                        eprintln!("Lyrics search incomplete: {warning}");
                        status(
                            hwnd,
                            "{count} result(s); some providers could not be reached.",
                            Some(count),
                        );
                    }
                    Event::Selected(target, lyrics) => {
                        let owner = access(hwnd, |state| state.owner).unwrap_or(null_mut());
                        SELECTIONS.with(|selections| {
                            selections.borrow_mut().push(Selection {
                                key: target.key,
                                generation: target.generation,
                                lyrics,
                            })
                        });
                        // No state borrow spans destruction or owner notification.
                        DestroyWindow(hwnd);
                        if !owner.is_null() {
                            PostMessageW(owner, MESSAGE, 0, 0);
                        }
                        return;
                    }
                }
            }
        }
    }

    unsafe fn scale(hwnd: HWND) -> f64 {
        unsafe { f64::from(GetDpiForWindow(hwnd).max(96)) / 96.0 }
    }

    fn fit_window_rect(rect: RECT, work: RECT) -> RECT {
        crate::island_surface::fit_rect(rect, work)
    }

    unsafe fn work_area(rect: &RECT) -> RECT {
        unsafe {
            let monitor = MonitorFromRect(rect, MONITOR_DEFAULTTONEAREST);
            let mut info = MONITORINFO {
                cbSize: std::mem::size_of::<MONITORINFO>() as u32,
                ..Default::default()
            };
            if GetMonitorInfoW(monitor, &mut info) != 0 {
                info.rcWork
            } else {
                *rect
            }
        }
    }

    struct SearchLayout {
        header: Rect,
        track: Rect,
        search_card: Rect,
        results: Rect,
        footer: Rect,
    }

    impl SearchLayout {
        fn new(width: f32, height: f32) -> Self {
            let content_width = (width - 48.0).max(0.0);
            let search_height = if height < 500.0 { 88.0 } else { 104.0 };
            let footer_top = (height - 64.0).max(0.0);
            let results_top = 64.0 + search_height + 12.0;
            Self {
                header: Rect::new(24.0, 0.0, content_width, 64.0),
                // The metadata line overlaps the title buttons' vertical band.
                track: Rect::new(24.0, 36.0, (content_width - 72.0).max(0.0), 22.0),
                search_card: Rect::new(24.0, 64.0, content_width, search_height),
                results: Rect::new(
                    24.0,
                    results_top,
                    content_width,
                    (footer_top - results_top).max(0.0),
                ),
                footer: Rect::new(24.0, footer_top, content_width, 64.0),
            }
        }

        fn tagline_visible(&self) -> bool {
            self.search_card.h > 88.0
        }

        fn controls(&self) -> [(i32, Rect); 9] {
            let card = self.search_card;
            let field = ((card.w - 132.0) * 0.5).max(0.0);
            let artist_x = card.x + 24.0 + field;
            let footer = self.footer;
            [
                (
                    TITLE_LABEL,
                    Rect::new(card.x + 12.0, card.y + 8.0, field, 20.0),
                ),
                (TITLE, Rect::new(card.x + 12.0, card.y + 32.0, field, 28.0)),
                (ARTIST_LABEL, Rect::new(artist_x, card.y + 8.0, field, 20.0)),
                (ARTIST, Rect::new(artist_x, card.y + 32.0, field, 28.0)),
                (
                    SEARCH,
                    Rect::new(card.x + card.w - 96.0, card.y + 28.0, 84.0, 36.0),
                ),
                (RESULTS, self.results),
                (STATUS, Rect::new(footer.x, footer.y, footer.w, 24.0)),
                (
                    APPLY,
                    Rect::new(footer.x + footer.w - 276.0, footer.y + 24.0, 184.0, 36.0),
                ),
                (
                    CANCEL,
                    Rect::new(footer.x + footer.w - 84.0, footer.y + 24.0, 84.0, 36.0),
                ),
            ]
        }
    }

    fn draw_background(c: &mut Canvas<'_>, track: &(String, String, String)) {
        let rect = c.clip;
        let layout = SearchLayout::new(rect.w, rect.h);
        c.rect(rect, 0.0, BACKGROUND, 1.0);
        c.ellipsis(
            tr("Find lyrics"),
            Rect::new(
                layout.header.x,
                6.0,
                (layout.header.w - 102.0).max(0.0),
                28.0,
            ),
            22.0,
            true,
            WHITE,
        );
        let source_width = if track.2.is_empty() {
            0.0
        } else {
            (c.painter.width(&track.2, 11.0, false) + 16.0).min(layout.track.w * 0.35)
        };
        c.ellipsis(
            &format!("{} · {}", track.0, track.1).replace(['\r', '\n', '\t'], " "),
            Rect::new(
                layout.track.x,
                layout.track.y,
                (layout.track.w - source_width - 12.0).max(0.0),
                layout.track.h,
            ),
            11.0,
            false,
            MUTED,
        );
        if source_width > 0.0 {
            native_ui::chip(
                c,
                Rect::new(
                    layout.track.x + layout.track.w - source_width,
                    layout.track.y,
                    source_width,
                    layout.track.h,
                ),
                &track.2,
                crate::theme::ICE,
                1.0,
            );
        }
        native_ui::card(c, layout.search_card, 12.0, SURFACE, crate::theme::BORDER);
        if layout.tagline_visible() {
            c.ellipsis(
                tr("Find the right recording, then make it yours."),
                Rect::new(
                    layout.search_card.x + 12.0,
                    layout.search_card.y + 76.0,
                    layout.search_card.w - 24.0,
                    20.0,
                ),
                11.0,
                false,
                MUTED,
            );
        }
        c.rect(
            Rect::new(layout.footer.x, layout.footer.y, layout.footer.w, 1.0),
            0.0,
            WHITE,
            0.08,
        );
    }

    fn provider_tint(source: &str) -> [u8; 3] {
        match source {
            "AMLL TTML DB" | "AMLL" => crate::theme::LAVENDER,
            "LRCLIB" => crate::theme::MINT,
            "NetEase" => crate::theme::AMBER,
            _ => crate::theme::DIM,
        }
    }

    fn timing_tint(timing: &str) -> [u8; 3] {
        match timing {
            "Words" => crate::theme::SUCCESS,
            "Lines" => crate::theme::ICE,
            _ => crate::theme::DIM,
        }
    }

    fn result_row(c: &mut Canvas<'_>, result: &SearchResult, style: ButtonStyle) {
        let duration = if result.duration.is_finite() && result.duration > 0.0 {
            let seconds = result.duration.round() as u64;
            format!("{}:{:02}", seconds / 60, seconds % 60)
        } else {
            "—".into()
        };
        let detail = if result.album.is_empty() {
            result.artist.clone()
        } else {
            format!("{} · {}", result.artist, result.album)
        }
        .replace(['\r', '\n', '\t'], " ");
        native_ui::result_row(
            c,
            &result.title.replace(['\r', '\n', '\t'], " "),
            &detail,
            &[
                (&result.source, provider_tint(&result.source)),
                (&duration, crate::theme::MUTED),
                (tr(&result.timing), timing_tint(&result.timing)),
            ],
            style,
        );
    }

    unsafe fn resize_surface(hwnd: HWND) {
        unsafe {
            compose_background(hwnd);
        }
    }

    unsafe fn compose_background(hwnd: HWND) {
        let started = native_ui::trace_start();
        unsafe {
            let mut rect = RECT::default();
            GetClientRect(hwnd, &mut rect);
            let size = (rect.right.max(0) as u32, rect.bottom.max(0) as u32);
            access(hwnd, |state| {
                let pixels = match state.background.take() {
                    Some((cached, pixels)) if cached == size => pixels,
                    _ => {
                        let mut pixels = vec![0; (size.0 * size.1 * 4) as usize];
                        draw_background(
                            &mut Canvas::new(&mut pixels, size, &mut state.painter),
                            &state.track,
                        );
                        pixels
                    }
                };
                if state.surface.resize(size.0, size.1) {
                    // Restore the entire opaque client, even for a strip paint.
                    // Presentation still copies only the native dirty region.
                    state.surface.pixels_mut().copy_from_slice(&pixels);
                    if state.loading_since.is_some() {
                        draw_loading(
                            &mut Canvas::new(state.surface.pixels_mut(), size, &mut state.painter),
                            state.loading_elapsed,
                            state.accent,
                        );
                    }
                }
                state.background = Some((size, pixels));
            });
        }
        native_ui::trace_ui("lyric_search", "compose", started);
    }

    unsafe fn paint_background(hwnd: HWND, dc: HDC, dirty: RECT) {
        unsafe {
            compose_background(hwnd);
            let started = native_ui::trace_start();
            access(hwnd, |state| state.surface.present(dc, dirty));
            native_ui::trace_ui("lyric_search", "present", started);
        }
    }

    unsafe fn layout(hwnd: HWND) {
        unsafe {
            layout_scaled(hwnd, scale(hwnd));
        }
    }

    unsafe fn layout_scaled(hwnd: HWND, s: f64) {
        unsafe {
            resize_surface(hwnd);
            let mut rect = RECT::default();
            GetClientRect(hwnd, &mut rect);
            let width = (f64::from(rect.right) / s) as f32;
            let height = (f64::from(rect.bottom) / s) as f32;
            for (id, bounds) in SearchLayout::new(width, height).controls() {
                MoveWindow(
                    GetDlgItem(hwnd, if id == RESULTS { RESULTS_VIEWPORT } else { id }),
                    (f64::from(bounds.x) * s).round() as i32,
                    (f64::from(bounds.y) * s).round() as i32,
                    (f64::from(bounds.w) * s).round() as i32,
                    (f64::from(bounds.h) * s).round() as i32,
                    1,
                );
            }
            position_results(hwnd);
            InvalidateRect(hwnd, null(), 0);
        }
    }

    unsafe fn set_font(hwnd: HWND) {
        unsafe {
            set_font_scaled(hwnd, scale(hwnd));
        }
    }

    unsafe fn set_font_scaled(hwnd: HWND, dpi_scale: f64) {
        unsafe {
            let font = CreateFontW(
                (-13.0 * dpi_scale) as i32,
                0,
                0,
                0,
                FW_MEDIUM as i32,
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
            let old = access(hwnd, |state| std::mem::replace(&mut state.font, font))
                .unwrap_or(null_mut());
            let effective = if font.is_null() {
                GetStockObject(DEFAULT_GUI_FONT)
            } else {
                font
            };
            for id in [
                TITLE_LABEL,
                TITLE,
                ARTIST_LABEL,
                ARTIST,
                SEARCH,
                RESULTS,
                STATUS,
                APPLY,
                CANCEL,
            ] {
                let control = if id == RESULTS {
                    results(hwnd)
                } else {
                    GetDlgItem(hwnd, id)
                };
                SendMessageW(control, WM_SETFONT, effective as usize, 1);
            }
            if !old.is_null() {
                DeleteObject(old);
            }
            SendMessageW(
                results(hwnd),
                LB_SETITEMHEIGHT,
                0,
                (f64::from(ROW_HEIGHT) * dpi_scale).round() as isize,
            );
            let s = dpi_scale as f32;
            access(hwnd, |state| {
                if let Ok(painter) = Painter::new(s) {
                    state.painter = painter;
                }
                state.background = None;
                state.row_cache.clear();
                let offset = state.results_scroll.offset();
                state.results_scroll.jump(offset);
            });
            stop_results_timer(hwnd);
            position_results(hwnd);
        }
    }

    unsafe extern "system" fn proc(
        hwnd: HWND,
        message: u32,
        wparam: WPARAM,
        lparam: LPARAM,
    ) -> LRESULT {
        unsafe {
            match message {
                WM_ACTIVATE if wparam as u16 == WA_INACTIVE as u16 => {
                    let focus = GetFocus();
                    if IsChild(hwnd, focus) != 0 {
                        access(hwnd, |state| state.focus = focus);
                    }
                    DefWindowProcW(hwnd, message, wparam, lparam)
                }
                WM_SETFOCUS => {
                    if let Some(saved) = access(hwnd, |state| state.focus) {
                        let valid = !saved.is_null()
                            && IsChild(hwnd, saved) != 0
                            && IsWindowVisible(saved) != 0
                            && windows_sys::Win32::UI::Input::KeyboardAndMouse::IsWindowEnabled(
                                saved,
                            ) != 0;
                        SetFocus(if valid {
                            saved
                        } else {
                            GetDlgItem(hwnd, TITLE)
                        });
                    }
                    0
                }
                WM_ERASEBKGND => 1,
                WM_CTLCOLORSTATIC | WM_CTLCOLOREDIT | WM_CTLCOLORLISTBOX => {
                    let dc = wparam as HDC;
                    let field = message == WM_CTLCOLOREDIT;
                    let on_card =
                        field || matches!(GetDlgCtrlID(lparam as HWND), TITLE_LABEL | ARTIST_LABEL);
                    let color = if on_card { SURFACE } else { BACKGROUND };
                    SetBkColor(dc, crate::theme::colorref(color));
                    SetTextColor(
                        dc,
                        crate::theme::colorref(if field {
                            crate::theme::TEXT
                        } else {
                            crate::theme::MUTED
                        }),
                    );
                    access(hwnd, |state| {
                        if on_card {
                            state.field_brush
                        } else {
                            state.brush
                        }
                    })
                    .unwrap_or(null_mut()) as isize
                }
                WM_PAINT => {
                    let mut ps = PAINTSTRUCT::default();
                    let dc = BeginPaint(hwnd, &mut ps);
                    paint_background(hwnd, dc, ps.rcPaint);
                    EndPaint(hwnd, &ps);
                    0
                }
                WM_PRINTCLIENT => {
                    let mut rect = RECT::default();
                    GetClientRect(hwnd, &mut rect);
                    paint_background(hwnd, wparam as HDC, rect);
                    0
                }
                WM_DRAWITEM => {
                    let item = &*(lparam as *const DRAWITEMSTRUCT);
                    let hovered =
                        item.CtlID != RESULTS as u32 && native_ui::pointer_inside(item.hwndItem);
                    let size = (
                        (item.rcItem.right - item.rcItem.left).max(0) as u32,
                        (item.rcItem.bottom - item.rcItem.top).max(0) as u32,
                    );
                    let pixels = access(hwnd, |state| {
                        let style = ButtonStyle {
                            accent: state.accent,
                            selected: item.itemState & ODS_SELECTED != 0,
                            focused: item.itemState & ODS_FOCUS != 0,
                            hovered,
                            enabled: item.itemState & ODS_DISABLED == 0,
                            ..Default::default()
                        };
                        if item.CtlID == RESULTS as u32 {
                            return result_raster(state, item.itemID as usize, size, style);
                        }
                        let mut pixels = vec![0; (size.0 * size.1 * 4) as usize];
                        let mut c = Canvas::new(&mut pixels, size, &mut state.painter);
                        if item.CtlID == STATUS as u32 {
                            c.rect(c.clip, 0.0, BACKGROUND, 1.0);
                            let tone = status_tint(&state.status_source, state.status_count);
                            c.rect(Rect::new(0.0, 11.0, 5.0, 5.0), 2.5, tone, 1.0);
                            let width = c.clip.w - 18.0;
                            c.ellipsis(
                                &state.status,
                                Rect::new(16.0, 2.0, width, 20.0),
                                11.0,
                                false,
                                MUTED,
                            );
                        } else {
                            let label = match item.CtlID as i32 {
                                SEARCH => "Search",
                                APPLY => "Use selected lyrics",
                                _ => "Cancel",
                            };
                            native_ui::button(
                                &mut c,
                                tr(label),
                                ButtonStyle {
                                    background: if item.CtlID == SEARCH as u32 {
                                        SURFACE
                                    } else {
                                        BACKGROUND
                                    },
                                    selected: item.CtlID == SEARCH as u32
                                        || item.CtlID == APPLY as u32,
                                    pressed: item.itemState & ODS_SELECTED != 0,
                                    ..style
                                },
                            );
                        }
                        Arc::<[u8]>::from(pixels)
                    });
                    if let Some(pixels) = pixels {
                        let saved = SaveDC(item.hDC);
                        if saved != 0 {
                            IntersectClipRect(
                                item.hDC,
                                item.rcItem.left,
                                item.rcItem.top,
                                item.rcItem.right,
                                item.rcItem.bottom,
                            );
                            SetViewportOrgEx(
                                item.hDC,
                                item.rcItem.left,
                                item.rcItem.top,
                                null_mut(),
                            );
                            native_ui::blit(item.hDC, &pixels, size);
                            RestoreDC(item.hDC, saved);
                        }
                    }
                    1
                }
                WM_COMMAND => {
                    let id = (wparam & 0xffff) as i32;
                    let notification = (wparam >> 16) as u32;
                    match id {
                        TITLE | ARTIST if notification == EN_CHANGE => edited(hwnd),
                        SEARCH => search(hwnd),
                        APPLY => fetch(hwnd),
                        CANCEL => {
                            DestroyWindow(hwnd);
                        }
                        RESULTS if notification == LBN_DBLCLK => fetch(hwnd),
                        RESULTS if notification == LBN_SELCHANGE => {
                            access(hwnd, |state| {
                                if state.fetching {
                                    state.session.close();
                                }
                                state.fetching = false;
                            });
                            let selected = SendMessageW(results(hwnd), LB_GETCURSEL, 0, 0) >= 0;
                            apply_enabled(hwnd, selected);
                            text(
                                hwnd,
                                STATUS,
                                "Choose Use selected lyrics to apply this match.",
                            );
                        }
                        _ => {}
                    }
                    0
                }
                WM_TIMER if wparam == POLL => {
                    poll(hwnd);
                    0
                }
                _ if (message == SCROLL_FRAME || message == WM_TIMER)
                    && access(hwnd, |state| state.frame_pump.is_frame(message, wparam))
                        .unwrap_or(false) =>
                {
                    let now = Instant::now();
                    if access(hwnd, |state| state.frame_pump.should_draw(now)).unwrap_or(false) {
                        tick_results(hwnd, now);
                    }
                    0
                }
                WM_SIZE => {
                    layout(hwnd);
                    0
                }
                WM_GETMINMAXINFO => {
                    let info = &mut *(lparam as *mut MINMAXINFO);
                    let s = scale(hwnd);
                    let rect = RECT {
                        left: 0,
                        top: 0,
                        right: (520.0 * s).round() as i32,
                        bottom: (420.0 * s).round() as i32,
                    };
                    let mut current = RECT::default();
                    GetWindowRect(hwnd, &mut current);
                    let work = work_area(&current);
                    info.ptMinTrackSize.x = (rect.right - rect.left).min(work.right - work.left);
                    info.ptMinTrackSize.y = (rect.bottom - rect.top).min(work.bottom - work.top);
                    0
                }
                WM_DPICHANGED => {
                    let proposed = *(lparam as *const RECT);
                    let rect = fit_window_rect(proposed, work_area(&proposed));
                    set_font(hwnd);
                    SetWindowPos(
                        hwnd,
                        null_mut(),
                        rect.left,
                        rect.top,
                        rect.right - rect.left,
                        rect.bottom - rect.top,
                        SWP_NOZORDER | SWP_NOACTIVATE,
                    );
                    layout(hwnd);
                    crate::dark_scrollbar::apply(GetDlgItem(hwnd, RESULTS_VIEWPORT));
                    crate::dark_scrollbar::apply(results(hwnd));
                    0
                }
                WM_THEMECHANGED => {
                    // SetWindowTheme retains associations across theme changes.
                    // Reapply to children only, avoiding synchronous self-recursion.
                    crate::dark_scrollbar::apply(GetDlgItem(hwnd, RESULTS_VIEWPORT));
                    crate::dark_scrollbar::apply(results(hwnd));
                    DefWindowProcW(hwnd, message, wparam, lparam)
                }
                WM_CLOSE => {
                    DestroyWindow(hwnd);
                    0
                }
                WM_DESTROY => {
                    // Clear annotations before the native child HWND is destroyed.
                    drop(access(hwnd, |state| state.results_name.take()));
                    0
                }
                WM_NCDESTROY => {
                    KillTimer(hwnd, POLL);
                    native_ui::set_scroll_animating(false);
                    let ptr = GetWindowLongPtrW(hwnd, GWLP_USERDATA) as *mut RefCell<State>;
                    SetWindowLongPtrW(hwnd, GWLP_USERDATA, 0);
                    WINDOW.with(|window| {
                        if window.get() == hwnd {
                            window.set(null_mut());
                        }
                    });
                    if !ptr.is_null() {
                        let state = Box::from_raw(ptr).into_inner();
                        if !state.font.is_null() {
                            DeleteObject(state.font);
                        }
                        DeleteObject(state.brush);
                        DeleteObject(state.field_brush);
                        // Session drop only invalidates the mailbox; it never joins workers.
                        drop(state);
                    }
                    DefWindowProcW(hwnd, message, wparam, lparam)
                }
                _ => DefWindowProcW(hwnd, message, wparam, lparam),
            }
        }
    }

    pub(crate) fn show(owner: HWND, snapshot: &MediaSnapshot) {
        unsafe {
            let existing = WINDOW.with(Cell::get);
            if !existing.is_null() && IsWindow(existing) != 0 {
                let changed = access(existing, |state| {
                    state.owner = owner;
                    state.session.target != target(snapshot)
                })
                .unwrap_or(false);
                if changed {
                    reset(existing, snapshot);
                }
                ShowWindow(
                    existing,
                    if IsIconic(existing) != 0 {
                        SW_RESTORE
                    } else {
                        SW_SHOW
                    },
                );
                SetForegroundWindow(existing);
                return;
            }
            if let Some(hwnd) = create(owner, snapshot) {
                ShowWindow(hwnd, SW_SHOW);
                SetForegroundWindow(hwnd);
                SetFocus(GetDlgItem(hwnd, TITLE));
            }
        }
    }

    // Construct before activation so native fixtures can render the real host
    // offscreen without ever flashing a preview on the user's desktop.
    unsafe fn create(owner: HWND, snapshot: &MediaSnapshot) -> Option<HWND> {
        unsafe {
            let class_name = wide("HiyoDynaLyricSearch");
            let instance = GetModuleHandleW(null());
            let class = WNDCLASSW {
                lpfnWndProc: Some(proc),
                hInstance: instance,
                lpszClassName: class_name.as_ptr(),
                hCursor: LoadCursorW(null_mut(), IDC_ARROW),
                ..Default::default()
            };
            RegisterClassW(&class);
            let hwnd = CreateWindowExW(
                WS_EX_CONTROLPARENT | WS_EX_TOOLWINDOW,
                class_name.as_ptr(),
                wide(tr("Search lyrics - HiyoDyna")).as_ptr(),
                STYLE,
                CW_USEDEFAULT,
                CW_USEDEFAULT,
                760,
                520,
                owner,
                null_mut(),
                instance,
                null(),
            );
            if hwnd.is_null() {
                native_ui::notice(
                    owner,
                    "Search lyrics",
                    "Could not create the lyrics search window.",
                );
                return None;
            }
            let Ok(painter) = Painter::new(scale(hwnd) as f32) else {
                DestroyWindow(hwnd);
                return None;
            };
            let state = Box::new(RefCell::new(State {
                owner,
                session: Session::new(target(snapshot)),
                suppress_edits: true,
                fetching: false,
                font: null_mut(),
                painter,
                surface: match native_ui::WindowSurface::new() {
                    Some(surface) => surface,
                    None => {
                        DestroyWindow(hwnd);
                        return None;
                    }
                },
                frame_pump: native_ui::FramePump::new(SCROLL_FRAME),
                loading_since: None,
                loading_elapsed: 0.0,
                results_placement: None,
                results_surface: native_ui::WindowSurface::new(),
                results_name: None,
                background: None,
                track: (
                    snapshot.title.clone(),
                    snapshot.artist.clone(),
                    snapshot.source.clone(),
                ),
                status: String::new(),
                status_source: String::new(),
                status_count: None,
                brush: CreateSolidBrush(crate::theme::colorref(BACKGROUND)),
                field_brush: CreateSolidBrush(crate::theme::colorref(SURFACE)),
                accent: crate::settings::Settings::load().accent_bgra(),
                focus: null_mut(),
                results_scroll: SmoothScroll::default(),
                results_syncing: false,
                results_timer: false,
                row_cache: VecDeque::new(),
            }));
            SetWindowLongPtrW(hwnd, GWLP_USERDATA, Box::into_raw(state) as isize);
            WINDOW.with(|window| window.set(hwnd));
            if let Err(error) = crate::window_chrome::install(hwnd, true, true) {
                eprintln!("Lyrics search chrome: {error}");
                DestroyWindow(hwnd);
                native_ui::notice(
                    owner,
                    "Search lyrics",
                    "Could not create the lyrics search controls.",
                );
                return None;
            }
            for (id, class, label, style, extended) in [
                (TITLE_LABEL, "STATIC", "&Title", 0, 0),
                (TITLE, "EDIT", "", WS_TABSTOP | ES_AUTOHSCROLL as u32, 0),
                (ARTIST_LABEL, "STATIC", "&Artist", 0, 0),
                (ARTIST, "EDIT", "", WS_TABSTOP | ES_AUTOHSCROLL as u32, 0),
                (
                    SEARCH,
                    "BUTTON",
                    "&Search",
                    WS_TABSTOP | BS_OWNERDRAW as u32,
                    0,
                ),
                (
                    RESULTS,
                    "LISTBOX",
                    "",
                    WS_TABSTOP
                        | WS_VSCROLL
                        | LBS_DISABLENOSCROLL as u32
                        | LBS_OWNERDRAWFIXED as u32
                        | LBS_HASSTRINGS as u32
                        | LBS_NOTIFY as u32
                        | LBS_NOINTEGRALHEIGHT as u32,
                    0,
                ),
                (STATUS, "STATIC", "", STATUS_STYLE, 0),
                (
                    APPLY,
                    "BUTTON",
                    "&Use selected lyrics",
                    WS_TABSTOP | BS_OWNERDRAW as u32,
                    0,
                ),
                (
                    CANCEL,
                    "BUTTON",
                    "Cancel",
                    WS_TABSTOP | BS_OWNERDRAW as u32,
                    0,
                ),
            ] {
                let control = CreateWindowExW(
                    extended,
                    wide(class).as_ptr(),
                    wide(tr(label)).as_ptr(),
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
                    native_ui::notice(
                        owner,
                        "Search lyrics",
                        "Could not create the lyrics search controls.",
                    );
                    return None;
                }
                if matches!(id, SEARCH | APPLY | CANCEL) {
                    windows_sys::Win32::UI::Shell::SetWindowSubclass(
                        control,
                        Some(native_ui::hover_proc),
                        2,
                        0,
                    );
                }
            }
            if !install_results_viewport(hwnd) || !name_results(hwnd, tr("Search results")) {
                DestroyWindow(hwnd);
                native_ui::notice(
                    owner,
                    "Search lyrics",
                    "Could not create the lyrics search controls.",
                );
                return None;
            }
            SendMessageW(GetDlgItem(hwnd, TITLE), EM_SETLIMITTEXT, 512, 0);
            SendMessageW(GetDlgItem(hwnd, ARTIST), EM_SETLIMITTEXT, 512, 0);
            set_font(hwnd);
            reset(hwnd, snapshot);
            crate::island_surface::place(hwnd, owner, (640.0, 560.0));
            layout(hwnd);
            if SetTimer(hwnd, POLL, 100, None) == 0 {
                DestroyWindow(hwnd);
                native_ui::notice(
                    owner,
                    "Search lyrics",
                    "Could not start lyrics search updates.",
                );
                return None;
            }
            Some(hwnd)
        }
    }

    pub(crate) fn translate_message(message: &MSG) -> bool {
        unsafe {
            let hwnd = WINDOW.with(Cell::get);
            if hwnd.is_null() || (message.hwnd != hwnd && IsChild(hwnd, message.hwnd) == 0) {
                return false;
            }
            if message.message == WM_KEYDOWN {
                let command = match message.wParam {
                    27 => Some(CANCEL),
                    13 => Some(match GetDlgCtrlID(GetFocus()) {
                        RESULTS | APPLY => APPLY,
                        CANCEL => CANCEL,
                        id if crate::window_chrome::is_button(id) => id,
                        _ => SEARCH,
                    }),
                    _ => None,
                };
                if let Some(command) = command {
                    PostMessageW(hwnd, WM_COMMAND, command as usize, 0);
                    return true;
                }
            }
            IsDialogMessageW(hwnd, message) != 0
        }
    }

    pub(crate) fn take_selections() -> Vec<Selection> {
        SELECTIONS.with(|selections| std::mem::take(&mut *selections.borrow_mut()))
    }

    /// Relabel the existing HWNDs. Typed edits, list selection/top index and focus survive.
    pub(crate) fn refresh_language() {
        unsafe {
            let hwnd = WINDOW.with(Cell::get);
            if hwnd.is_null() || IsWindow(hwnd) == 0 {
                return;
            }
            let Some((source, count, labels, scroll)) = access(hwnd, |state| {
                state.background = None;
                state.row_cache.clear();
                state.results_syncing = true;
                (
                    state.status_source.clone(),
                    state.status_count,
                    state
                        .session
                        .results
                        .iter()
                        .map(candidate_label)
                        .collect::<Vec<_>>(),
                    std::mem::take(&mut state.results_scroll),
                )
            }) else {
                return;
            };
            SetWindowTextW(hwnd, wide(tr("Search lyrics - HiyoDyna")).as_ptr());
            for (id, key) in [
                (TITLE_LABEL, "&Title"),
                (ARTIST_LABEL, "&Artist"),
                (SEARCH, "&Search"),
                (APPLY, "&Use selected lyrics"),
                (CANCEL, "Cancel"),
            ] {
                SetWindowTextW(GetDlgItem(hwnd, id), wide(tr(key)).as_ptr());
                InvalidateRect(GetDlgItem(hwnd, id), null(), 0);
            }
            // Native EDIT accessible names come from the preceding translated STATIC;
            // WM_SETTEXT on those edits would replace the user's current search query.
            let list = results(hwnd);
            name_results(hwnd, tr("Search results"));
            let selected = SendMessageW(list, LB_GETCURSEL, 0, 0);
            let top = SendMessageW(list, LB_GETTOPINDEX, 0, 0);
            SendMessageW(list, WM_SETREDRAW, 0, 0);
            SendMessageW(list, LB_RESETCONTENT, 0, 0);
            for label in labels {
                SendMessageW(list, LB_ADDSTRING, 0, wide(&label).as_ptr() as isize);
            }
            if selected >= 0 {
                SendMessageW(list, LB_SETCURSEL, selected as usize, 0);
            }
            if top >= 0 {
                SendMessageW(list, LB_SETTOPINDEX, top as usize, 0);
            }
            SendMessageW(list, WM_SETREDRAW, 1, 0);
            access(hwnd, |state| {
                state.results_syncing = false;
                state.results_scroll = scroll;
            });
            position_results(hwnd);
            status(hwnd, &source, count);
            crate::window_chrome::refresh(hwnd);
            InvalidateRect(list, null(), 0);
            InvalidateRect(hwnd, null(), 0);
        }
    }

    pub(crate) fn track_changed(snapshot: &MediaSnapshot) {
        unsafe {
            let hwnd = WINDOW.with(Cell::get);
            if hwnd.is_null() {
                return;
            }
            if access(hwnd, |state| state.session.target != target(snapshot)).unwrap_or(false) {
                reset(hwnd, snapshot);
                if !snapshot.key.is_empty() {
                    text(
                        hwnd,
                        STATUS,
                        "Playback changed. Search again to choose lyrics for the current track.",
                    );
                }
            }
        }
    }

    #[cfg(test)]
    mod tests {
        use super::*;
        use crate::preferences::tests::NativeTestKeyboard;
        use std::time::{Duration, Instant};
        use windows_sys::Win32::UI::Input::KeyboardAndMouse::IsWindowEnabled;

        struct NativeRaster {
            dc: HDC,
            bitmap: HBITMAP,
            previous: HGDIOBJ,
            bits: *mut u8,
            size: (u32, u32),
        }

        impl NativeRaster {
            unsafe fn new(size: (u32, u32)) -> Self {
                unsafe {
                    let dc = CreateCompatibleDC(null_mut());
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
                    let mut bits = null_mut();
                    let bitmap =
                        CreateDIBSection(dc, &info, DIB_RGB_COLORS, &mut bits, null_mut(), 0);
                    assert!(!dc.is_null() && !bitmap.is_null());
                    let previous = SelectObject(dc, bitmap);
                    let mut raster = Self {
                        dc,
                        bitmap,
                        previous,
                        bits: bits.cast(),
                        size,
                    };
                    raster.pixels().fill(0);
                    raster
                }
            }

            fn pixels(&mut self) -> &mut [u8] {
                unsafe {
                    GdiFlush();
                    std::slice::from_raw_parts_mut(
                        self.bits,
                        (self.size.0 * self.size.1 * 4) as usize,
                    )
                }
            }
        }

        impl Drop for NativeRaster {
            fn drop(&mut self) {
                unsafe {
                    SelectObject(self.dc, self.previous);
                    DeleteObject(self.bitmap);
                    DeleteDC(self.dc);
                }
            }
        }

        #[test]
        fn native_control_colors_use_the_shared_palette() {
            use crate::theme;
            unsafe {
                let raster = NativeRaster::new((1, 1));
                for (message, text, background) in [
                    (WM_CTLCOLOREDIT, theme::TEXT, theme::SURFACE),
                    (WM_CTLCOLORSTATIC, theme::MUTED, theme::BG),
                    (WM_CTLCOLORLISTBOX, theme::MUTED, theme::BG),
                ] {
                    // Color messages only need a memory DC; no search window or media target is opened.
                    proc(null_mut(), message, raster.dc as usize, 0);
                    assert_eq!(GetTextColor(raster.dc), theme::colorref(text));
                    assert_eq!(GetBkColor(raster.dc), theme::colorref(background));
                }
            }
        }

        #[test]
        #[ignore = "Checks the real search host and edit retention through Island expansion; run serially"]
        fn native_search_is_owned_tool_surface_and_retains_edits() {
            unsafe {
                let owner = CreateWindowExW(
                    WS_EX_TOOLWINDOW,
                    wide("STATIC").as_ptr(),
                    null(),
                    WS_POPUP,
                    300,
                    0,
                    148,
                    40,
                    null_mut(),
                    null_mut(),
                    null_mut(),
                    null(),
                );
                show(owner, &MediaSnapshot::default());
                let hwnd = WINDOW.with(Cell::get);
                let edit = GetDlgItem(hwnd, TITLE);
                SetWindowTextW(edit, wide("A typed search query").as_ptr());
                let ex = GetWindowLongW(hwnd, GWL_EXSTYLE) as u32;
                assert_eq!(ex & WS_EX_APPWINDOW, 0);
                assert_ne!(ex & WS_EX_TOOLWINDOW, 0);
                assert_eq!(GetWindow(hwnd, GW_OWNER), owner);
                assert!(GetDlgItem(hwnd, crate::window_chrome::MINIMIZE).is_null());
                let mut placement = RECT::default();
                GetWindowRect(hwnd, &mut placement);
                let gap = (8.0 * scale(hwnd)).round() as i32;
                assert_eq!(
                    placement.top,
                    40 + gap,
                    "search should open next to the visible Island"
                );
                for _ in 0..2 {
                    SendMessageW(hwnd, WM_COMMAND, crate::window_chrome::MAXIMIZE as usize, 0);
                    for _ in 0..160 {
                        SendMessageW(hwnd, WM_TIMER, 0x4953, 0);
                    }
                    assert_eq!(GetDlgItem(hwnd, TITLE), edit);
                    let mut text = [0u16; 128];
                    let len = GetWindowTextW(edit, text.as_mut_ptr(), text.len() as i32);
                    assert_eq!(
                        String::from_utf16_lossy(&text[..len as usize]),
                        "A typed search query"
                    );
                }
                DestroyWindow(hwnd);
                DestroyWindow(owner);
            }
        }

        #[test]
        fn redesigned_search_reserves_flexible_results_and_compact_search_card() {
            for (width, height, results_top) in [(640.0, 560.0, 180.0), (520.0, 420.0, 164.0)] {
                let layout = SearchLayout::new(width, height);
                assert_eq!(layout.header.h, 64.0);
                assert!(
                    layout.track.x + layout.track.w <= width - 84.0,
                    "track metadata must clear the native title buttons"
                );
                assert_eq!(layout.footer.h, 64.0);
                assert_eq!(
                    layout.search_card.h,
                    if height == 560.0 { 104.0 } else { 88.0 }
                );
                assert_eq!(layout.tagline_visible(), height == 560.0);
                let controls = layout.controls();
                let results = controls.iter().find(|(id, _)| *id == RESULTS).unwrap().1;
                assert_eq!(results.y, results_top);
                assert_eq!(results.y + results.h, height - 64.0);
                assert!(results.h >= 160.0);
                for (id, bounds) in controls {
                    let region = match id {
                        RESULTS => layout.results,
                        STATUS | APPLY | CANCEL => layout.footer,
                        _ => layout.search_card,
                    };
                    assert!(bounds.x >= region.x && bounds.y >= region.y);
                    assert!(bounds.x + bounds.w <= region.x + region.w);
                    assert!(bounds.y + bounds.h <= region.y + region.h);
                    for dpi in [1.0, 2.0] {
                        assert!(bounds.x * dpi >= 0.0 && bounds.y * dpi >= 0.0);
                        assert!((bounds.x + bounds.w) * dpi <= width * dpi);
                        assert!((bounds.y + bounds.h) * dpi <= height * dpi);
                    }
                }
            }
        }

        #[test]
        fn redesigned_rows_use_raised_selection_and_three_pixel_accent() {
            let mut painter = Painter::new(1.0).unwrap();
            let mut pixels = vec![0; 440 * 64 * 4];
            result_row(
                &mut Canvas::new(&mut pixels, (440, 64), &mut painter),
                &SearchResult::fixture("Title", "Artist"),
                ButtonStyle {
                    selected: true,
                    accent: crate::theme::AMBER,
                    ..Default::default()
                },
            );
            let pixel = |x: usize, y: usize| &pixels[(y * 440 + x) * 4..(y * 440 + x) * 4 + 3];
            assert_eq!(pixel(430, 32), crate::theme::SURFACE_RAISED);
            for x in 3..6 {
                assert_eq!(pixel(x, 32), crate::theme::AMBER);
            }
        }

        #[test]
        fn redesigned_rows_render_provider_and_timing_palette() {
            use crate::theme;
            for (source, timing, provider_color, timing_color) in [
                ("AMLL TTML DB", "Words", theme::LAVENDER, theme::SUCCESS),
                ("LRCLIB", "Lines", theme::MINT, theme::ICE),
                ("NetEase", "Unknown", theme::AMBER, theme::DIM),
            ] {
                for scale in [1.0, 2.0] {
                    let mut painter = Painter::new(scale).unwrap();
                    let size = ((440.0 * scale) as u32, (64.0 * scale) as u32);
                    let mut pixels = vec![0; (size.0 * size.1 * 4) as usize];
                    let mut result =
                        SearchResult::fixture("A long bilingual recording title", "Artist");
                    result.source = source.into();
                    result.timing = timing.into();
                    result_row(
                        &mut Canvas::new(&mut pixels, size, &mut painter),
                        &result,
                        ButtonStyle::default(),
                    );
                    assert!(
                        pixels
                            .as_chunks::<4>()
                            .0
                            .iter()
                            .any(|p| p[..3] == provider_color)
                    );
                    assert!(
                        pixels
                            .as_chunks::<4>()
                            .0
                            .iter()
                            .any(|p| p[..3] == timing_color)
                    );
                }
            }
        }

        #[test]
        fn narrow_rows_drop_detail_before_metadata_chips_collide() {
            let mut painter = Painter::new(1.0).unwrap();
            let mut render = |artist: &str| {
                let mut pixels = vec![0; 320 * 64 * 4];
                let mut result = SearchResult::fixture("A long recording title", artist);
                result.source = "AMLL TTML DB".into();
                result.timing = "Unknown".into();
                result_row(
                    &mut Canvas::new(&mut pixels, (320, 64), &mut painter),
                    &result,
                    ButtonStyle::default(),
                );
                pixels
            };
            assert!(render("First artist") == render("Another artist"));
        }

        #[test]
        fn timing_chips_paint_the_complete_localized_label_at_every_dpi() {
            use crate::i18n::{Language, tr_in};
            for language in [Language::English, Language::Vietnamese] {
                for scale in [1.0, 1.25, 1.5, 2.0] {
                    for timing in ["Words", "Lines", "Unknown"] {
                        let label = tr_in(language, timing);
                        let mut painter = Painter::new(scale).unwrap();
                        let size = ((472.0 * scale) as u32, (64.0 * scale) as u32);
                        let mut actual = vec![0; (size.0 * size.1 * 4) as usize];
                        native_ui::result_row(
                            &mut Canvas::new(&mut actual, size, &mut painter),
                            "Title",
                            "Artist",
                            &[
                                ("AMLL TTML DB", crate::theme::LAVENDER),
                                ("3:20", crate::theme::MUTED),
                                (label, crate::theme::ICE),
                            ],
                            ButtonStyle::default(),
                        );
                        // A generously sized chip is an independent full-text
                        // reference. Count solid glyph pixels, not its padding.
                        let mut expected = vec![0; (size.0 * size.1 * 4) as usize];
                        let mut c = Canvas::new(&mut expected, size, &mut painter);
                        c.rect(c.clip, 0.0, BACKGROUND, 1.0);
                        native_ui::chip(
                            &mut c,
                            Rect::new(14.0, 36.0, 200.0, 22.0),
                            label,
                            crate::theme::ICE,
                            1.0,
                        );
                        let glyphs = |pixels: &[u8]| {
                            pixels
                                .as_chunks::<4>()
                                .0
                                .iter()
                                .filter(|p| p[..3] == crate::theme::ICE)
                                .count()
                        };
                        assert_eq!(
                            glyphs(&actual),
                            glyphs(&expected),
                            "{language:?} {timing} at {scale}x must not ellipsize"
                        );
                    }
                }
            }
        }

        // Memory-DC printing is independent of the desktop capture boundary.
        // Standard EDIT text needs an initialized offscreen host for previews.
        unsafe fn printed_window(hwnd: HWND) -> image::RgbaImage {
            unsafe {
                let mut outer = RECT::default();
                GetWindowRect(hwnd, &mut outer);
                let width = (outer.right - outer.left) as u32;
                let height = (outer.bottom - outer.top) as u32;
                let mut raster = NativeRaster::new((width, height));
                SendMessageW(
                    hwnd,
                    WM_PRINT,
                    raster.dc as usize,
                    (PRF_CLIENT | PRF_NONCLIENT | PRF_CHILDREN | PRF_ERASEBKGND) as isize,
                );
                let mut pixels = raster.pixels().to_vec();
                for pixel in pixels.as_chunks_mut::<4>().0 {
                    pixel.swap(0, 2);
                    pixel[3] = 255;
                }
                image::RgbaImage::from_raw(width, height, pixels).unwrap()
            }
        }

        #[test]
        fn search_button_print_keeps_card_background_outside_white_pill() {
            unsafe {
                let hwnd = create(null_mut(), &MediaSnapshot::default()).unwrap();
                let _windows = TestWindows {
                    owner: hwnd,
                    searches: Vec::new(),
                };
                for scale in [1.0, 2.0] {
                    set_font_scaled(hwnd, scale);
                    layout_scaled(hwnd, scale);
                    for (id, background) in
                        [(SEARCH, SURFACE), (APPLY, BACKGROUND), (CANCEL, BACKGROUND)]
                    {
                        let button = GetDlgItem(hwnd, id);
                        let mut rect = RECT::default();
                        GetClientRect(button, &mut rect);
                        let mut raster = NativeRaster::new((rect.right as u32, rect.bottom as u32));
                        SendMessageW(
                            button,
                            WM_PRINTCLIENT,
                            raster.dc as usize,
                            PRF_CLIENT as isize,
                        );
                        let pixels = raster.pixels();
                        for y in 0..rect.bottom {
                            for x in 0..rect.right {
                                if x == 0 || y == 0 || x == rect.right - 1 || y == rect.bottom - 1 {
                                    let i = ((y * rect.right + x) * 4) as usize;
                                    assert_eq!(
                                        pixels[i..i + 3],
                                        background,
                                        "button {id} outer pixel ({x}, {y}) at {scale}x must match its parent"
                                    );
                                }
                            }
                        }
                        if id == SEARCH {
                            let i = ((rect.bottom / 2 * rect.right + (10.0 * scale) as i32) * 4)
                                as usize;
                            assert_eq!(
                                pixels[i..i + 3],
                                WHITE,
                                "Search must retain its white pill"
                            );
                        }
                    }
                }
                assert_eq!(IsWindowVisible(hwnd), 0);
            }
        }

        #[test]
        fn hidden_two_hundred_percent_minimum_print_keeps_footer_and_opaque_client() {
            unsafe {
                let hwnd = create(null_mut(), &MediaSnapshot::default()).unwrap();
                let _windows = TestWindows {
                    owner: hwnd,
                    searches: Vec::new(),
                };
                SetWindowPos(
                    hwnd,
                    null_mut(),
                    -16000,
                    -16000,
                    1040,
                    840,
                    SWP_NOZORDER | SWP_NOACTIVATE,
                );
                set_font_scaled(hwnd, 2.0);
                layout_scaled(hwnd, 2.0);
                let mut client = RECT::default();
                GetClientRect(hwnd, &mut client);
                let mut outer = RECT::default();
                GetWindowRect(hwnd, &mut outer);
                let image = printed_window(hwnd);
                assert_eq!(
                    image.dimensions(),
                    (
                        (outer.right - outer.left) as u32,
                        (outer.bottom - outer.top) as u32
                    )
                );
                let rgb = |color: [u8; 3]| [color[2], color[1], color[0], 255];
                assert_eq!(
                    image.get_pixel(100, client.bottom as u32 - 2).0,
                    rgb(BACKGROUND),
                    "the entire hidden client must be cleared"
                );
                let layout =
                    SearchLayout::new(client.right as f32 / 2.0, client.bottom as f32 / 2.0);
                for id in [STATUS, APPLY, CANCEL] {
                    let mut actual = RECT::default();
                    GetWindowRect(GetDlgItem(hwnd, id), &mut actual);
                    MapWindowPoints(null_mut(), hwnd, (&mut actual as *mut RECT).cast(), 2);
                    assert!(
                        actual.left >= 0
                            && actual.top >= (layout.footer.y * 2.0) as i32
                            && actual.right <= client.right
                            && actual.bottom <= client.bottom,
                        "footer control {id} must fit the actual clamped client"
                    );
                }
                let cancel = layout
                    .controls()
                    .into_iter()
                    .find(|(id, _)| *id == CANCEL)
                    .unwrap()
                    .1;
                assert_eq!(
                    image
                        .get_pixel(
                            ((cancel.x + 12.0) * 2.0) as u32,
                            ((cancel.y + 18.0) * 2.0) as u32
                        )
                        .0,
                    rgb(SURFACE),
                    "Cancel must paint below the desktop edge"
                );
                assert!(SearchLayout::new(520.0, 420.0).results.h >= 160.0);
                assert!(
                    access(hwnd, |s| s
                        .surface
                        .pixels_mut()
                        .as_chunks::<4>()
                        .0
                        .iter()
                        .all(|p| p[3] == 255))
                    .unwrap(),
                    "the persistent client surface must be fully opaque"
                );
            }
        }

        #[test]
        fn narrow_search_actions_and_inputs_stay_inside_surface() {
            for (width, height) in [
                (320.0, 260.0),
                (400.0, 300.0),
                (520.0, 460.0),
                (920.0, 760.0),
            ] {
                for (_, r) in SearchLayout::new(width, height).controls() {
                    assert!(r.x >= 0.0 && r.y >= 0.0 && r.w >= 0.0 && r.h >= 0.0);
                    assert!(r.x + r.w <= width && r.y + r.h <= height);
                }
            }
        }

        #[test]
        fn search_layout_keeps_fields_results_status_and_actions_separate() {
            for (width, height) in [(504.0, 300.0), (504.0, 340.0), (744.0, 481.0)] {
                let layout = SearchLayout::new(width, height).controls();
                let rect = |id| layout.iter().find(|(key, _)| *key == id).unwrap().1;
                let title = rect(TITLE);
                let artist = rect(ARTIST);
                let results = rect(RESULTS);
                let status = rect(STATUS);
                let apply = rect(APPLY);
                assert!(title.y >= if height < 340.0 { 60.0 } else { 90.0 });
                assert!(title.x + title.w <= artist.x);
                assert!(results.y >= title.y + title.h + 28.0);
                assert!(results.y + results.h <= status.y);
                assert!(status.y + status.h <= apply.y);
                assert!(apply.y + apply.h <= height);
            }
        }

        #[test]
        fn search_window_fits_small_high_dpi_and_negative_origin_work_areas() {
            for work in [
                RECT {
                    left: 0,
                    top: 0,
                    right: 1366,
                    bottom: 728,
                },
                RECT {
                    left: -1366,
                    top: 30,
                    right: 0,
                    bottom: 728,
                },
            ] {
                let fitted = fit_window_rect(
                    RECT {
                        left: work.right - 400,
                        top: 100,
                        right: work.right - 400 + 1140,
                        bottom: 880,
                    },
                    work,
                );
                assert!(fitted.left >= work.left && fitted.top >= work.top);
                assert!(fitted.right <= work.right && fitted.bottom <= work.bottom);
                assert_eq!(fitted.bottom - fitted.top, work.bottom - work.top);
            }
        }

        #[test]
        fn result_rows_have_visible_selection_and_focus_at_every_dpi() {
            use crate::canvas::{Canvas, Painter};
            use crate::native_ui::ButtonStyle;
            for scale in [1.0, 1.25, 1.5, 2.0] {
                let size = ((440.0 * scale) as u32, (ROW_HEIGHT * scale) as u32);
                let mut painter = Painter::new(scale).unwrap();
                let render = |painter: &mut Painter, selected, focused| {
                    let mut pixels = vec![0; (size.0 * size.1 * 4) as usize];
                    result_row(
                        &mut Canvas::new(&mut pixels, size, painter),
                        &SearchResult::fixture("A song title", "Artist"),
                        ButtonStyle {
                            // Fixed accent fixture isolates selection/focus raster changes from the theme.
                            accent: [220, 190, 130],
                            selected,
                            focused,
                            ..Default::default()
                        },
                    );
                    pixels
                };
                let plain = render(&mut painter, false, false);
                let selected = render(&mut painter, true, false);
                let focused = render(&mut painter, true, true);
                assert_ne!(plain, selected);
                assert_ne!(selected, focused);
                for (x, y) in [(220.0, 2.0), (220.0, 61.0), (1.0, 32.0), (438.0, 32.0)] {
                    let index = (((y * scale) as u32 * size.0 + (x * scale) as u32) * 4) as usize;
                    assert_ne!(
                        &selected[index..index + 4],
                        &focused[index..index + 4],
                        "focus ring must cover all four edges at {scale} DPI scale"
                    );
                }
            }
        }

        #[test]
        #[ignore = "Software-only row benchmark; no windows, focus, or network"]
        fn search_row_raster_baseline() {
            let mut painter = Painter::new(1.25).unwrap();
            let size = (700, 73);
            let title = "Bản thu âm trực tiếp — A very long bilingual recording title ".repeat(3);
            let detail = "Nghệ sĩ · LRCLIB · Lines · 4:20 · A long album description ".repeat(3);
            let mut samples = Vec::new();
            for _ in 0..32 {
                let started = Instant::now();
                let mut pixels = vec![0; (size.0 * size.1 * 4) as usize];
                native_ui::result_row(
                    &mut Canvas::new(&mut pixels, size, &mut painter),
                    &title,
                    &detail,
                    &[
                        ("LRCLIB", crate::theme::MINT),
                        ("4:20", MUTED),
                        ("Lines", crate::theme::ICE),
                    ],
                    ButtonStyle::default(),
                );
                std::hint::black_box(pixels);
                samples.push(started.elapsed().as_micros());
            }
            samples.sort_unstable();
            eprintln!(
                "search baseline row raster: median={}us p95={}us (32 long bilingual rows, 125% DPI, debug)",
                samples[16], samples[30]
            );
        }

        // A hidden native fixture: never calls show(), SetFocus, ShowWindow, or
        // SetForegroundWindow, so it can run alongside other agents' desktop work.
        fn hidden_results() -> TestWindows {
            unsafe {
                let instance = GetModuleHandleW(null());
                let class_name = wide("HiyoDynaSearchScrollTest");
                RegisterClassW(&WNDCLASSW {
                    lpfnWndProc: Some(proc),
                    hInstance: instance,
                    lpszClassName: class_name.as_ptr(),
                    ..Default::default()
                });
                let hwnd = CreateWindowExW(
                    0,
                    class_name.as_ptr(),
                    null(),
                    WS_OVERLAPPEDWINDOW,
                    0,
                    0,
                    760,
                    520,
                    null_mut(),
                    null_mut(),
                    instance,
                    null(),
                );
                assert!(!hwnd.is_null());
                let state = State {
                    owner: null_mut(),
                    session: Session::new(Target {
                        key: "scroll-fixture".into(),
                        generation: 7,
                    }),
                    suppress_edits: false,
                    fetching: false,
                    font: null_mut(),
                    painter: Painter::new(scale(hwnd) as f32).unwrap(),
                    surface: native_ui::WindowSurface::new().unwrap(),
                    frame_pump: native_ui::FramePump::new(SCROLL_FRAME),
                    loading_since: None,
                    loading_elapsed: 0.0,
                    results_placement: None,
                    results_surface: native_ui::WindowSurface::new(),
                    results_name: None,
                    background: None,
                    track: (String::new(), String::new(), String::new()),
                    status: String::new(),
                    status_source: String::new(),
                    status_count: None,
                    // Black brushes and a fixed accent isolate the hidden viewport fixture from theme changes.
                    brush: CreateSolidBrush(0),
                    field_brush: CreateSolidBrush(0),
                    accent: [220, 190, 130],
                    focus: null_mut(),
                    results_scroll: SmoothScroll::default(),
                    results_syncing: false,
                    results_timer: false,
                    row_cache: VecDeque::new(),
                };
                SetWindowLongPtrW(
                    hwnd,
                    GWLP_USERDATA,
                    Box::into_raw(Box::new(RefCell::new(state))) as isize,
                );
                let list = CreateWindowExW(
                    0,
                    wide("LISTBOX").as_ptr(),
                    null(),
                    WS_CHILD
                        | WS_VISIBLE
                        | WS_VSCROLL
                        | LBS_DISABLENOSCROLL as u32
                        | LBS_OWNERDRAWFIXED as u32
                        | LBS_HASSTRINGS as u32
                        | LBS_NOTIFY as u32
                        | LBS_NOINTEGRALHEIGHT as u32,
                    0,
                    0,
                    560,
                    180,
                    hwnd,
                    RESULTS as usize as *mut _,
                    instance,
                    null(),
                );
                assert!(!list.is_null());
                SendMessageW(list, LB_SETITEMHEIGHT, 0, ROW_HEIGHT as isize);
                assert!(install_results_viewport(hwnd));
                assert!(name_results(hwnd, tr("Search results")));
                MoveWindow(GetDlgItem(hwnd, RESULTS_VIEWPORT), 0, 0, 560, 180, 0);
                let job = access(hwnd, |state| state.session.begin()).unwrap();
                job.deliver(Completion::Search(Ok((0..24)
                    .map(|i| {
                        SearchResult::fixture(&format!("Bản thu âm — Recording {i}"), "Nghệ sĩ")
                    })
                    .collect())));
                poll(hwnd);
                TestWindows {
                    owner: hwnd,
                    searches: Vec::new(),
                }
            }
        }

        unsafe fn displayed_row_top(hwnd: HWND, index: usize) -> i32 {
            unsafe {
                let list = results(hwnd);
                let mut rect = RECT::default();
                SendMessageW(
                    list,
                    LB_GETITEMRECT,
                    index,
                    (&mut rect as *mut RECT) as isize,
                );
                MapWindowPoints(list, hwnd, (&mut rect as *mut RECT).cast(), 2);
                rect.top
            }
        }

        #[test]
        fn native_window_style_does_not_enable_composited_painting() {
            let windows = hidden_results();
            unsafe {
                native_ui::style_window(windows.owner);
                assert_eq!(
                    GetWindowLongPtrW(windows.owner, GWL_EXSTYLE) & WS_EX_COMPOSITED as isize,
                    0
                );
            }
        }

        #[test]
        fn status_states_paint_distinct_theme_dots() {
            use crate::theme;
            unsafe {
                let windows = hidden_results();
                let dc = CreateCompatibleDC(null_mut());
                let info = BITMAPINFO {
                    bmiHeader: BITMAPINFOHEADER {
                        biSize: std::mem::size_of::<BITMAPINFOHEADER>() as u32,
                        biWidth: 440,
                        biHeight: -24,
                        biPlanes: 1,
                        biBitCount: 32,
                        biCompression: BI_RGB,
                        ..Default::default()
                    },
                    ..Default::default()
                };
                let mut bits = null_mut();
                let bitmap = CreateDIBSection(dc, &info, DIB_RGB_COLORS, &mut bits, null_mut(), 0);
                let old = SelectObject(dc, bitmap);
                for (source, count, color) in [
                    (
                        "Enter a title and artist, then choose Search.",
                        None,
                        theme::MUTED,
                    ),
                    ("Searching lyric providers...", None, theme::ICE),
                    (
                        "{count} result(s). Choose a match, then Use selected lyrics.",
                        Some(3),
                        theme::SUCCESS,
                    ),
                    (
                        "No synchronized lyrics found. Try a different title or artist.",
                        None,
                        theme::AMBER,
                    ),
                    (
                        "Lyrics request failed. Please try again.",
                        None,
                        theme::DANGER,
                    ),
                    ("Instrumental", None, theme::LAVENDER),
                    (
                        "No active media track. You can search; play a track before applying lyrics.",
                        None,
                        theme::DIM,
                    ),
                ] {
                    status(windows.owner, source, count);
                    let item = DRAWITEMSTRUCT {
                        CtlID: STATUS as u32,
                        hDC: dc,
                        rcItem: RECT {
                            left: 0,
                            top: 0,
                            right: 440,
                            bottom: 24,
                        },
                        ..Default::default()
                    };
                    SendMessageW(
                        windows.owner,
                        WM_DRAWITEM,
                        0,
                        (&item as *const DRAWITEMSTRUCT) as isize,
                    );
                    assert_eq!(GetPixel(dc, 2, 13), theme::colorref(color), "{source}");
                }
                SelectObject(dc, old);
                DeleteObject(bitmap);
                DeleteDC(dc);
            }
        }

        #[test]
        fn no_media_status_text_meets_aa_without_changing_its_dot() {
            unsafe {
                let windows = hidden_results();
                for source in [
                    "No active media track. You can search; play a track before applying lyrics.",
                    "Play a media track before applying lyrics.",
                ] {
                    status(windows.owner, source, None);
                    let mut raster = NativeRaster::new((440, 24));
                    let item = DRAWITEMSTRUCT {
                        CtlID: STATUS as u32,
                        hDC: raster.dc,
                        rcItem: RECT {
                            left: 0,
                            top: 0,
                            right: 440,
                            bottom: 24,
                        },
                        ..Default::default()
                    };
                    SendMessageW(
                        windows.owner,
                        WM_DRAWITEM,
                        0,
                        (&item as *const DRAWITEMSTRUCT) as isize,
                    );
                    let pixels = raster.pixels();
                    let foreground = pixels
                        .as_chunks::<4>()
                        .0
                        .iter()
                        .enumerate()
                        .filter(|(index, _)| index % 440 >= 16)
                        .map(|(_, pixel)| [pixel[0], pixel[1], pixel[2]])
                        .max_by_key(|pixel| {
                            pixel.iter().map(|&channel| u32::from(channel)).sum::<u32>()
                        })
                        .unwrap();
                    assert!(
                        crate::theme::contrast_ratio(foreground, BACKGROUND) >= 4.5,
                        "the actual no-media status text must meet AA: {foreground:?}"
                    );
                    assert_eq!(foreground, MUTED);
                    assert!(crate::theme::contrast_ratio(MUTED, BACKGROUND) >= 4.5);
                    assert_eq!(
                        GetPixel(raster.dc, 2, 13),
                        crate::theme::colorref(crate::theme::DIM)
                    );
                }
            }
        }

        #[test]
        fn loading_line_advances_and_pump_stops_when_search_settles() {
            unsafe {
                let windows = hidden_results();
                let hwnd = windows.owner;
                let now = Instant::now();
                status(hwnd, "Searching lyric providers...", None);
                assert!(access(hwnd, |s| s.frame_pump.is_active()).unwrap());
                let raster = || {
                    access(hwnd, |s| {
                        let mut pixels = vec![0; 640 * 560 * 4];
                        let mut painter = Painter::new(1.0).unwrap();
                        draw_loading(
                            &mut Canvas::new(&mut pixels, (640, 560), &mut painter),
                            s.loading_elapsed,
                            s.accent,
                        );
                        pixels
                    })
                    .unwrap()
                };
                let first = raster();
                tick_results(hwnd, now + Duration::from_millis(300));
                let second = raster();
                assert!(first != second, "loading line must advance on elapsed time");
                access(hwnd, |s| s.session.begin())
                    .unwrap()
                    .deliver(Completion::Search(Ok(Vec::new())));
                poll(hwnd);
                assert!(!access(hwnd, |s| s.frame_pump.is_active()).unwrap());
                status(hwnd, "Fetching selected lyrics...", None);
                assert!(access(hwnd, |s| s.frame_pump.is_active()).unwrap());
                status(hwnd, "Lyrics request failed. Please try again.", None);
                assert!(!access(hwnd, |s| s.frame_pump.is_active()).unwrap());
            }
        }

        #[test]
        fn results_have_a_localized_native_accessible_name() {
            use crate::i18n::{Language, tr_in};
            assert_eq!(tr_in(Language::English, "Search results"), "Search results");
            assert_eq!(
                tr_in(Language::Vietnamese, "Search results"),
                "Kết quả tìm kiếm"
            );
            unsafe {
                let windows = hidden_results();
                let list = results(windows.owner);
                let mut name = [0u16; 128];
                let len = GetWindowTextW(list, name.as_mut_ptr(), name.len() as i32);
                assert_eq!(
                    String::from_utf16_lossy(&name[..len as usize]),
                    tr("Search results")
                );
            }
        }

        #[test]
        fn scroll_noop_and_row_changes_only_update_required_native_geometry() {
            unsafe extern "system" fn count_moves(
                hwnd: HWND,
                message: u32,
                w: WPARAM,
                l: LPARAM,
                _: usize,
                data: usize,
            ) -> LRESULT {
                unsafe {
                    let counts = &*(data as *const (Cell<usize>, Cell<usize>));
                    if message == WM_WINDOWPOSCHANGING {
                        counts.0.set(counts.0.get() + 1);
                    }
                    if message == LB_SETTOPINDEX {
                        counts.1.set(counts.1.get() + 1);
                    }
                    DefSubclassProc(hwnd, message, w, l)
                }
            }
            unsafe {
                let windows = hidden_results();
                let hwnd = windows.owner;
                let list = results(hwnd);
                let counts = (Cell::new(0usize), Cell::new(0usize));
                SetWindowSubclass(list, Some(count_moves), 91, (&counts as *const _) as usize);
                InvalidateRect(list, null(), 0);
                SendMessageW(list, WM_PAINT, 0, 0);
                ValidateRect(hwnd, null());
                ValidateRect(GetDlgItem(hwnd, RESULTS_VIEWPORT), null());
                ValidateRect(list, null());
                assert_eq!(
                    GetUpdateRect(list, null_mut(), 0),
                    0,
                    "fixture starts clean"
                );
                access(hwnd, |s| s.results_scroll.jump(0.4));
                position_results(hwnd);
                assert_eq!(
                    counts.0.get(),
                    0,
                    "subpixel movement must not move the HWND"
                );
                assert_eq!(GetUpdateRect(list, null_mut(), 0), 0);
                access(hwnd, |s| s.results_scroll.jump(5.0));
                position_results(hwnd);
                assert_eq!(counts.0.get(), 1, "a changed remainder moves the HWND once");
                assert_eq!(counts.1.get(), 0, "same row must not send LB_SETTOPINDEX");
                ValidateRect(list, null());
                access(hwnd, |s| s.results_scroll.jump(68.0));
                position_results(hwnd);
                assert_eq!(counts.1.get(), 1);
                assert_eq!(counts.0.get(), 2);
                access(hwnd, |s| s.results_scroll.jump(132.0));
                position_results(hwnd);
                assert_eq!(
                    counts.0.get(),
                    2,
                    "a whole-row delta retains the fractional HWND position"
                );
                assert_eq!(
                    counts.1.get(),
                    2,
                    "row changes send exactly one LB_SETTOPINDEX"
                );
                windows_sys::Win32::UI::Shell::RemoveWindowSubclass(list, Some(count_moves), 91);
            }
        }

        #[test]
        fn scroll_strip_tracks_direction_remainders_and_large_jumps() {
            for (top, delta, expected) in [
                (0, 5, Some((175, 180))),
                (5, -5, Some((5, 10))),
                (4, 63, Some((121, 184))),
                (60, -8, Some((60, 68))),
                (0, 0, None),
                (4, 180, Some((4, 184))),
                (4, -240, Some((4, 184))),
            ] {
                let dirty = exposed_scroll_strip(
                    RECT {
                        left: 0,
                        top,
                        right: 560,
                        bottom: top + 180,
                    },
                    delta,
                );
                assert_eq!(
                    dirty.as_ref().map(|r| (r.top, r.bottom)),
                    expected,
                    "delta {delta}"
                );
                if let Some(dirty) = dirty {
                    assert_eq!((dirty.left, dirty.right), (0, 560));
                }
            }
            assert!(exposed_scroll_strip(RECT::default(), 5).is_none());
        }

        #[test]
        fn native_two_hundred_percent_controls_fit_regions_and_client() {
            unsafe {
                let windows = hidden_results();
                let hwnd = windows.owner;
                SetWindowLongW(hwnd, GWL_STYLE, (WS_POPUP | WS_CLIPCHILDREN) as i32);
                SetWindowPos(
                    hwnd,
                    null_mut(),
                    0,
                    0,
                    1040,
                    840,
                    SWP_NOZORDER | SWP_NOACTIVATE | SWP_FRAMECHANGED,
                );
                for (id, class) in [
                    (TITLE_LABEL, "STATIC"),
                    (TITLE, "EDIT"),
                    (ARTIST_LABEL, "STATIC"),
                    (ARTIST, "EDIT"),
                    (SEARCH, "BUTTON"),
                    (STATUS, "STATIC"),
                    (APPLY, "BUTTON"),
                    (CANCEL, "BUTTON"),
                ] {
                    assert!(
                        !CreateWindowExW(
                            0,
                            wide(class).as_ptr(),
                            null(),
                            WS_CHILD | WS_VISIBLE,
                            0,
                            0,
                            1,
                            1,
                            hwnd,
                            id as usize as *mut _,
                            GetModuleHandleW(null()),
                            null()
                        )
                        .is_null()
                    );
                }
                layout_scaled(hwnd, 2.0);
                let layout = SearchLayout::new(520.0, 420.0);
                for (id, assigned) in layout.controls() {
                    let control =
                        GetDlgItem(hwnd, if id == RESULTS { RESULTS_VIEWPORT } else { id });
                    let mut actual = RECT::default();
                    GetWindowRect(control, &mut actual);
                    MapWindowPoints(null_mut(), hwnd, (&mut actual as *mut RECT).cast(), 2);
                    assert_eq!(
                        actual.left,
                        (assigned.x * 2.0).round() as i32,
                        "control {id}"
                    );
                    assert_eq!(
                        actual.top,
                        (assigned.y * 2.0).round() as i32,
                        "control {id}"
                    );
                    assert_eq!(
                        actual.right - actual.left,
                        (assigned.w * 2.0).round() as i32
                    );
                    assert_eq!(
                        actual.bottom - actual.top,
                        (assigned.h * 2.0).round() as i32
                    );
                    let region = match id {
                        RESULTS => layout.results,
                        STATUS | APPLY | CANCEL => layout.footer,
                        _ => layout.search_card,
                    };
                    assert!(
                        actual.left >= (region.x * 2.0) as i32
                            && actual.top >= (region.y * 2.0) as i32
                    );
                    assert!(
                        actual.right <= ((region.x + region.w) * 2.0) as i32
                            && actual.bottom <= ((region.y + region.h) * 2.0) as i32
                    );
                    assert!(
                        actual.left >= 0
                            && actual.top >= 0
                            && actual.right <= 1040
                            && actual.bottom <= 840
                    );
                }
            }
        }

        #[test]
        fn hidden_viewport_print_renders_native_selected_rows() {
            unsafe {
                let windows = hidden_results();
                let viewport = GetDlgItem(windows.owner, RESULTS_VIEWPORT);
                let mut outer = RECT::default();
                GetWindowRect(viewport, &mut outer);
                let mut raster = NativeRaster::new((
                    (outer.right - outer.left) as u32,
                    (outer.bottom - outer.top) as u32,
                ));
                SendMessageW(
                    viewport,
                    WM_PRINT,
                    raster.dc as usize,
                    (PRF_CLIENT | PRF_NONCLIENT | PRF_CHILDREN | PRF_ERASEBKGND) as isize,
                );
                assert_eq!(
                    GetPixel(raster.dc, 280, 32),
                    crate::theme::colorref(crate::theme::SURFACE_RAISED),
                    "the native selected row must print into the memory DC"
                );
                assert!(
                    raster
                        .pixels()
                        .as_chunks::<4>()
                        .0
                        .iter()
                        .any(|p| p[..3] == WHITE),
                    "native owner-draw rows must print their titles"
                );
                assert_eq!(IsWindowVisible(windows.owner), 0);
            }
        }

        #[test]
        fn partial_background_paint_clears_the_entire_surface_opaquely() {
            unsafe {
                let windows = hidden_results();
                let hwnd = windows.owner;
                InvalidateRect(hwnd, null(), 0);
                SendMessageW(hwnd, WM_PAINT, 0, 0);
                access(hwnd, |s| s.surface.pixels_mut().fill(0));
                let dirty = RECT {
                    left: 30,
                    top: 70,
                    right: 40,
                    bottom: 80,
                };
                InvalidateRect(hwnd, &dirty, 0);
                SendMessageW(hwnd, WM_PAINT, 0, 0);
                access(hwnd, |s| {
                    let pixels = s.surface.pixels_mut();
                    assert!(pixels.as_chunks::<4>().0.iter().all(|p| p[3] == 255));
                    assert_eq!(
                        &pixels[pixels.len() - 4..],
                        &[BACKGROUND[0], BACKGROUND[1], BACKGROUND[2], 255]
                    );
                });
            }
        }

        #[test]
        #[ignore = "Hidden memory-only cached background benchmark; no activation or network"]
        fn search_background_compose_cached() {
            unsafe {
                let windows = hidden_results();
                let hwnd = windows.owner;
                SetWindowLongW(hwnd, GWL_STYLE, (WS_POPUP | WS_CLIPCHILDREN) as i32);
                for scale in [1.0, 2.0] {
                    access(hwnd, |s| {
                        s.painter = Painter::new(scale).unwrap();
                        s.background = None;
                        s.loading_since = Some(Instant::now());
                        s.loading_elapsed = 0.3;
                    });
                    SetWindowPos(
                        hwnd,
                        null_mut(),
                        -16000,
                        -16000,
                        (640.0 * scale) as i32,
                        (560.0 * scale) as i32,
                        SWP_NOZORDER | SWP_NOACTIVATE | SWP_FRAMECHANGED,
                    );
                    compose_background(hwnd);
                    let mut samples = Vec::new();
                    for _ in 0..64 {
                        let started = Instant::now();
                        compose_background(hwnd);
                        samples.push(started.elapsed().as_micros());
                    }
                    samples.sort_unstable();
                    eprintln!(
                        "search cached whole-client compose + loading overlay at {}%: median={}us p95={}us",
                        (scale * 100.0) as u32,
                        samples[32],
                        samples[60]
                    );
                    access(hwnd, |s| {
                        assert!(
                            s.surface
                                .pixels_mut()
                                .as_chunks::<4>()
                                .0
                                .iter()
                                .all(|p| p[3] == 255)
                        )
                    });
                }
                assert_eq!(IsWindowVisible(hwnd), 0);
            }
        }

        #[test]
        fn hidden_client_print_paints_the_complete_background() {
            unsafe {
                let windows = hidden_results();
                let mut rect = RECT::default();
                GetClientRect(windows.owner, &mut rect);
                let mut raster = NativeRaster::new((rect.right as u32, rect.bottom as u32));
                SendMessageW(
                    windows.owner,
                    WM_PRINTCLIENT,
                    raster.dc as usize,
                    PRF_CLIENT as isize,
                );
                assert_eq!(GetPixel(raster.dc, 30, 70), crate::theme::colorref(SURFACE));
                assert!(
                    raster
                        .pixels()
                        .as_chunks::<4>()
                        .0
                        .iter()
                        .all(|p| p[3] == 255)
                );
            }
        }

        #[test]
        fn hidden_scrolled_viewport_print_clips_native_children_to_its_bounds() {
            unsafe {
                let windows = hidden_results();
                access(windows.owner, |state| {
                    state.results_scroll.jump(6.0 * ROW_HEIGHT + 16.0)
                });
                position_results(windows.owner);
                let viewport = GetDlgItem(windows.owner, RESULTS_VIEWPORT);
                let mut outer = RECT::default();
                GetWindowRect(viewport, &mut outer);
                let width = outer.right - outer.left;
                let height = outer.bottom - outer.top;
                let mut raster = NativeRaster::new(((width + 40) as u32, (height + 40) as u32));
                let untouched = [17, 18, 19, 255];
                for pixel in raster.pixels().as_chunks_mut::<4>().0 {
                    pixel.copy_from_slice(&untouched);
                }
                SetViewportOrgEx(raster.dc, 20, 20, null_mut());
                SendMessageW(
                    viewport,
                    WM_PRINT,
                    raster.dc as usize,
                    (PRF_CLIENT | PRF_NONCLIENT | PRF_CHILDREN | PRF_ERASEBKGND) as isize,
                );
                let pixels = raster.pixels();
                let mut glyphs = 0;
                for y in 0..height + 40 {
                    for x in 0..width + 40 {
                        let i = ((y * (width + 40) + x) * 4) as usize;
                        if !(20..width + 20).contains(&x) || !(20..height + 20).contains(&y) {
                            assert_eq!(
                                pixels[i..i + 4],
                                untouched,
                                "native child print must not leak at ({x}, {y})"
                            );
                        } else if pixels[i..i + 3] == WHITE {
                            glyphs += 1;
                        }
                    }
                }
                assert!(glyphs > 0, "clipping must retain native owner-drawn rows");
            }
        }

        #[test]
        fn viewport_scrollbar_print_is_dark_and_has_a_themed_thumb() {
            unsafe {
                let windows = hidden_results();
                let viewport = GetDlgItem(windows.owner, RESULTS_VIEWPORT);
                let mut outer = RECT::default();
                let mut client = RECT::default();
                GetWindowRect(viewport, &mut outer);
                GetClientRect(viewport, &mut client);
                let width = outer.right - outer.left;
                let height = outer.bottom - outer.top;
                let mut bar = SCROLLBARINFO {
                    cbSize: std::mem::size_of::<SCROLLBARINFO>() as u32,
                    ..Default::default()
                };
                assert_ne!(GetScrollBarInfo(viewport, OBJID_VSCROLL, &mut bar), 0);
                let top = bar.rcScrollBar.top - outer.top + bar.xyThumbTop;
                let bottom = bar.rcScrollBar.top - outer.top + bar.xyThumbBottom;
                assert!(bottom > top);
                for scale in [1.0, 1.25, 1.5, 2.0] {
                    access(windows.owner, |s| s.painter = Painter::new(scale).unwrap());
                    let mut raster = NativeRaster::new((width as u32, height as u32));
                    SendMessageW(
                        viewport,
                        WM_PRINT,
                        raster.dc as usize,
                        (PRF_CLIENT | PRF_NONCLIENT | PRF_ERASEBKGND) as isize,
                    );
                    let pixels = raster.pixels();
                    let mut thumb_pixels = 0;
                    for y in 0..height {
                        for x in client.right..width {
                            let i = ((y * width + x) * 4) as usize;
                            assert!(
                                !pixels[i..i + 3].iter().all(|&v| v > 200),
                                "no white system track at {scale}x"
                            );
                            if (top..bottom).contains(&y)
                                && pixels[i..i + 3] == crate::theme::BORDER_STRONG
                            {
                                thumb_pixels += 1;
                            }
                        }
                    }
                    assert!(
                        thumb_pixels > 0,
                        "BORDER_STRONG thumb must occupy its native rectangle at {scale}x"
                    );
                }
            }
        }

        #[test]
        fn paint_presents_the_persistent_surface_once() {
            let windows = hidden_results();
            unsafe {
                let hwnd = windows.owner;
                InvalidateRect(hwnd, null(), 0);
                SendMessageW(hwnd, WM_PAINT, 0, 0);
                let address = access(hwnd, |state| {
                    assert_eq!(state.surface.presents, 1);
                    state.surface.pixels_mut().as_ptr()
                })
                .unwrap();
                InvalidateRect(hwnd, null(), 0);
                SendMessageW(hwnd, WM_PAINT, 0, 0);
                access(hwnd, |state| {
                    assert_eq!(state.surface.presents, 2);
                    assert_eq!(state.surface.pixels_mut().as_ptr(), address);
                });
            }
        }

        #[test]
        fn search_wheel_moves_through_pixels_before_reaching_a_row_boundary() {
            unsafe {
                let windows = hidden_results();
                let hwnd = windows.owner;
                let list = results(hwnd);
                let before = displayed_row_top(hwnd, 0);
                SendMessageW(list, WM_MOUSEWHEEL, (-120_i16 as u16 as usize) << 16, 0);
                assert_eq!(
                    displayed_row_top(hwnd, 0),
                    before,
                    "wheel input must set a target rather than jump a whole row"
                );
                assert!(access(hwnd, |state| state.results_scroll.moving()).unwrap());
                // Keep the real WM_MOUSEWHEEL route assertion above, but use
                // one explicit clock origin for interpolation. A descheduled
                // test thread must not turn its 8 ms frame into a 50 ms frame.
                access(hwnd, |state| state.results_scroll.jump(0.0));
                let now = Instant::now();
                wheel_results(hwnd, (-120_i16 as u16 as usize) << 16, now);
                tick_results(hwnd, now + Duration::from_millis(8));
                let moved = before - displayed_row_top(hwnd, 0);
                assert!(
                    moved > 0 && moved < ROW_HEIGHT as i32,
                    "first frame must move intermediate pixels, got {moved}"
                );
                tick_results(hwnd, now + Duration::from_secs(1));
                access(hwnd, |state| {
                    assert!(!state.results_scroll.moving());
                    assert!(!state.frame_pump.is_active());
                    assert!(!state.results_timer);
                });
            }
        }

        #[test]
        fn search_changed_results_cancel_wheel_motion_and_reset_native_selection() {
            unsafe {
                let windows = hidden_results();
                let hwnd = windows.owner;
                let list = results(hwnd);
                SendMessageW(list, WM_MOUSEWHEEL, (-120_i16 as u16 as usize) << 16, 0);
                tick_results(hwnd, Instant::now() + Duration::from_millis(8));
                access(hwnd, |state| state.session.begin())
                    .unwrap()
                    .deliver(Completion::Search(Ok(vec![SearchResult::fixture(
                        "New result",
                        "New artist",
                    )])));
                poll(hwnd);
                let before = displayed_row_top(hwnd, 0);
                tick_results(hwnd, Instant::now() + Duration::from_millis(8));
                assert_eq!(displayed_row_top(hwnd, 0), before);
                assert_eq!(SendMessageW(list, LB_GETCURSEL, 0, 0), 0);
                assert_eq!(SendMessageW(list, LB_GETTOPINDEX, 0, 0), 0);
            }
        }

        #[test]
        fn search_native_hit_mapping_and_keyboard_selection_follow_the_displayed_rows() {
            unsafe {
                let windows = hidden_results();
                let hwnd = windows.owner;
                let list = results(hwnd);
                access(hwnd, |state| {
                    state.results_scroll.jump(83.0);
                    state
                        .results_scroll
                        .scroll_to(257.0, 1356.0, Instant::now());
                });
                position_results(hwnd);
                assert!(access(hwnd, |state| state.results_scroll.moving()).unwrap());
                assert_eq!(displayed_row_top(hwnd, 2), 45);
                let mut point = windows_sys::Win32::Foundation::POINT { x: 20, y: 52 };
                MapWindowPoints(GetDlgItem(hwnd, RESULTS_VIEWPORT), list, &mut point, 1);
                let point = (point.y as u16 as u32) << 16 | point.x as u16 as u32;
                assert_eq!(
                    SendMessageW(list, LB_ITEMFROMPOINT, 0, point as isize) & 0xffff,
                    2
                );
                SendMessageW(list, LB_SETCURSEL, 2, 0);
                SendMessageW(list, WM_KEYDOWN, 0x28, 0);
                assert_eq!(SendMessageW(list, LB_GETCURSEL, 0, 0), 3);
                SendMessageW(list, WM_KEYDOWN, 0x23, 0);
                assert_eq!(SendMessageW(list, LB_GETCURSEL, 0, 0), 23);
                let top = displayed_row_top(hwnd, 23);
                assert!(
                    top >= 0 && top + ROW_HEIGHT as i32 <= 180,
                    "keyboard selection must remain visible"
                );
            }
        }

        #[test]
        fn search_relabel_preserves_fractional_viewport_and_pending_job() {
            unsafe {
                let windows = hidden_results();
                let hwnd = windows.owner;
                WINDOW.with(|window| window.set(hwnd));
                access(hwnd, |state| {
                    state.results_scroll.jump(25.0);
                    state
                        .results_scroll
                        .scroll_by(192.0, 1356.0, Instant::now());
                });
                position_results(hwnd);
                let pending = access(hwnd, |state| state.session.begin()).unwrap();
                let before = displayed_row_top(hwnd, 0);
                refresh_language();
                assert_eq!(
                    displayed_row_top(hwnd, 0),
                    before,
                    "relabeling must retain the partial top row"
                );
                assert!(access(hwnd, |state| state.results_scroll.moving()).unwrap());
                assert!(pending.current());
            }
        }

        #[test]
        fn search_native_tab_order_keeps_results_between_search_and_apply() {
            unsafe {
                let instance = GetModuleHandleW(null());
                let hwnd = CreateWindowExW(
                    0,
                    wide("STATIC").as_ptr(),
                    null(),
                    WS_OVERLAPPEDWINDOW,
                    0,
                    0,
                    760,
                    520,
                    null_mut(),
                    null_mut(),
                    instance,
                    null(),
                );
                assert!(!hwnd.is_null());
                let _windows = TestWindows {
                    owner: hwnd,
                    searches: Vec::new(),
                };
                let control = |class: &str, id: i32, tab: bool| {
                    let child = CreateWindowExW(
                        0,
                        wide(class).as_ptr(),
                        null(),
                        WS_CHILD | WS_VISIBLE | if tab { WS_TABSTOP } else { 0 },
                        0,
                        0,
                        100,
                        30,
                        hwnd,
                        id as usize as *mut _,
                        instance,
                        null(),
                    );
                    assert!(!child.is_null());
                    child
                };
                let search = control("BUTTON", SEARCH, true);
                let list = control("LISTBOX", RESULTS, true);
                control("STATIC", STATUS, false);
                let apply = control("BUTTON", APPLY, true);
                let cancel = control("BUTTON", CANCEL, true);
                assert_eq!(GetNextDlgTabItem(hwnd, search, 0), list);
                assert_eq!(GetNextDlgTabItem(hwnd, apply, 1), list);
                assert!(install_results_viewport(hwnd));
                for (from, next, previous) in [
                    (search, list, cancel),
                    (list, apply, search),
                    (apply, cancel, list),
                    (cancel, search, apply),
                ] {
                    assert_eq!(
                        GetNextDlgTabItem(hwnd, from, 0),
                        next,
                        "forward native Tab order from control {}",
                        GetDlgCtrlID(from)
                    );
                    assert_eq!(
                        GetNextDlgTabItem(hwnd, from, 1),
                        previous,
                        "reverse native Tab order from control {}",
                        GetDlgCtrlID(from)
                    );
                }
                windows_sys::Win32::UI::Input::KeyboardAndMouse::EnableWindow(apply, 0);
                assert_eq!(GetNextDlgTabItem(hwnd, list, 0), cancel);
                assert_eq!(GetNextDlgTabItem(hwnd, cancel, 1), list);
            }
        }

        #[test]
        fn search_resize_during_motion_cancels_without_reversing_native_rows() {
            unsafe {
                let windows = hidden_results();
                let hwnd = windows.owner;
                let now = Instant::now();
                access(hwnd, |state| {
                    state.results_scroll.scroll_to(1356.0, 1356.0, now)
                });
                tick_results(hwnd, now + Duration::from_millis(40));
                let offset = access(hwnd, |state| state.results_scroll.offset()).unwrap();
                let before = displayed_row_top(hwnd, 0);
                assert!((offset - 783.9375).abs() < 0.01);
                MoveWindow(GetDlgItem(hwnd, RESULTS_VIEWPORT), 0, 0, 560, 300, 0);
                assert_eq!(
                    access(hwnd, |state| state.results_scroll.offset()),
                    Some(offset)
                );
                assert!(
                    !access(hwnd, |state| state.results_scroll.moving()).unwrap(),
                    "shrinking bounds must cancel the old interpolation"
                );
                tick_results(hwnd, now + Duration::from_millis(41));
                assert_eq!(
                    displayed_row_top(hwnd, 0),
                    before,
                    "resize must not reverse the native viewport without reverse input"
                );
                let mut scrollbar = SCROLLINFO {
                    cbSize: std::mem::size_of::<SCROLLINFO>() as u32,
                    fMask: SIF_POS,
                    ..Default::default()
                };
                GetScrollInfo(GetDlgItem(hwnd, RESULTS_VIEWPORT), SB_VERT, &mut scrollbar);
                assert_eq!(scrollbar.nPos, offset.round() as i32);
            }
        }

        #[test]
        fn search_late_outward_wheel_applies_settled_native_viewport() {
            unsafe {
                let windows = hidden_results();
                let hwnd = windows.owner;
                let now = Instant::now();
                let outward = (-12000_i16 as u16 as usize) << 16;
                wheel_results(hwnd, outward, now);
                tick_results(hwnd, now + Duration::from_millis(20));
                let partial = displayed_row_top(hwnd, 0);
                assert!(partial < 0 && partial > -1356);
                wheel_results(hwnd, outward, now + Duration::from_secs(1));
                tick_results(hwnd, now + Duration::from_secs(1));
                assert_eq!(
                    access(hwnd, |state| state.results_scroll.offset()),
                    Some(1356.0)
                );
                assert_eq!(
                    displayed_row_top(hwnd, 0),
                    -1356,
                    "an already settled offset must still reach the native viewport"
                );
                let mut scrollbar = SCROLLINFO {
                    cbSize: std::mem::size_of::<SCROLLINFO>() as u32,
                    fMask: SIF_POS,
                    ..Default::default()
                };
                GetScrollInfo(GetDlgItem(hwnd, RESULTS_VIEWPORT), SB_VERT, &mut scrollbar);
                assert_eq!(scrollbar.nPos, 1356);
                assert!(!access(hwnd, |state| state.results_scroll.moving()).unwrap());
            }
        }

        #[test]
        fn search_unmatched_keyboard_input_preserves_native_scroll_position() {
            unsafe {
                let windows = hidden_results();
                let hwnd = windows.owner;
                let list = results(hwnd);
                SendMessageW(list, LB_SETTOPINDEX, 10, 0);
                assert_eq!(SendMessageW(list, LB_GETCURSEL, 0, 0), 0);
                for message in [WM_KEYDOWN, WM_CHAR] {
                    SendMessageW(
                        list,
                        message,
                        if message == WM_CHAR { 0x61 } else { 0x41 },
                        0,
                    );
                    assert_eq!(SendMessageW(list, LB_GETCURSEL, 0, 0), 0);
                    assert_eq!(
                        access(hwnd, |state| state.results_scroll.offset()),
                        Some(640.0),
                        "an unmatched key must not reveal an unchanged selection"
                    );
                    assert_eq!(displayed_row_top(hwnd, 0), -640);
                }
                // Home must still reveal the selected first row even when its
                // index was already zero before the native navigation message.
                SendMessageW(list, WM_KEYDOWN, 0x24, 0);
                assert_eq!(displayed_row_top(hwnd, 0), 0);
            }
        }

        #[test]
        fn search_native_scroll_thumb_position_uses_listbox_row_units() {
            unsafe {
                let windows = hidden_results();
                let hwnd = windows.owner;
                let list = results(hwnd);
                SendMessageW(list, WM_VSCROLL, (10 << 16) | SB_THUMBPOSITION as usize, 0);
                assert_eq!(
                    access(hwnd, |state| state.results_scroll.offset()),
                    Some(640.0)
                );
                assert_eq!(SendMessageW(list, LB_GETTOPINDEX, 0, 0), 10);
                assert_eq!(displayed_row_top(hwnd, 0), -640);
                let mut scrollbar = SCROLLINFO {
                    cbSize: std::mem::size_of::<SCROLLINFO>() as u32,
                    fMask: SIF_POS,
                    ..Default::default()
                };
                GetScrollInfo(GetDlgItem(hwnd, RESULTS_VIEWPORT), SB_VERT, &mut scrollbar);
                assert_eq!(scrollbar.nPos, 640);
                assert_eq!(SendMessageW(list, LB_GETCURSEL, 0, 0), 0);
            }
        }

        #[test]
        fn search_results_expose_only_viewport_scrollbar_across_native_updates() {
            unsafe {
                let windows = hidden_results();
                let hwnd = windows.owner;
                let list = results(hwnd);
                let viewport = GetDlgItem(hwnd, RESULTS_VIEWPORT);
                let mut samples = vec![("initial population", results_scrollbar_geometry(hwnd))];
                position_results(hwnd);
                samples.push(("repositioned", results_scrollbar_geometry(hwnd)));
                MoveWindow(viewport, 0, 0, 640, 300, 0);
                samples.push(("resized", results_scrollbar_geometry(hwnd)));
                SendMessageW(list, WM_VSCROLL, (10 << 16) | SB_THUMBPOSITION as usize, 0);
                samples.push(("native scrolled", results_scrollbar_geometry(hwnd)));
                access(hwnd, |state| state.results_scroll.jump(603.0));
                position_results(hwnd);
                samples.push(("fractional row", results_scrollbar_geometry(hwnd)));
                let job = access(hwnd, |state| state.session.begin()).unwrap();
                job.deliver(Completion::Search(Ok((0..40)
                    .map(|index| {
                        SearchResult::fixture(&format!("Replacement recording {index}"), "Artist")
                    })
                    .collect())));
                poll(hwnd);
                samples.push(("results replaced", results_scrollbar_geometry(hwnd)));
                clear_results(hwnd);
                samples.push(("results cleared", results_scrollbar_geometry(hwnd)));
                MoveWindow(viewport, 0, 0, 480, 400, 0);
                samples.push(("empty resized", results_scrollbar_geometry(hwnd)));
                let job = access(hwnd, |state| state.session.begin()).unwrap();
                job.deliver(Completion::Search(Ok(vec![SearchResult::fixture(
                    "Short result",
                    "Artist",
                )])));
                poll(hwnd);
                samples.push(("short results replaced", results_scrollbar_geometry(hwnd)));
                for (phase, geometry) in &samples {
                    eprintln!(
                        "search scrollbar {phase}: {geometry:?} (viewport width, native client right, exposed native bar, WS_VSCROLL, bar x-range, list outer width)"
                    );
                }
                for (
                    phase,
                    (width, client_right, overlap, style, (bar_left, bar_right), outer_width),
                ) in samples
                {
                    assert_eq!(
                        overlap, 0,
                        "{phase}: only the viewport scrollbar may occupy visible space"
                    );
                    assert_eq!(
                        client_right, width,
                        "{phase}: the native row client must exactly fill the viewport width"
                    );
                    assert_ne!(
                        style, 0,
                        "{phase}: the native scroll capability must remain present"
                    );
                    assert_eq!(
                        bar_left, width,
                        "{phase}: the native bar must start beyond the viewport clip"
                    );
                    assert_eq!(
                        bar_right, outer_width,
                        "{phase}: the native bar must occupy the listbox nonclient strip"
                    );
                    assert!(
                        bar_right > bar_left,
                        "{phase}: the native bar must retain positive width"
                    );
                }
            }
        }

        unsafe fn results_scrollbar_geometry(hwnd: HWND) -> (i32, i32, i32, u32, (i32, i32), i32) {
            unsafe {
                let viewport = GetDlgItem(hwnd, RESULTS_VIEWPORT);
                let list = results(hwnd);
                let mut clip = RECT::default();
                let mut client = RECT::default();
                let mut outer = RECT::default();
                GetClientRect(viewport, &mut clip);
                let mut visible_bar = SCROLLBARINFO {
                    cbSize: std::mem::size_of::<SCROLLBARINFO>() as u32,
                    ..Default::default()
                };
                assert_ne!(
                    GetScrollBarInfo(viewport, OBJID_VSCROLL, &mut visible_bar),
                    0
                );
                MapWindowPoints(
                    null_mut(),
                    viewport,
                    (&mut visible_bar.rcScrollBar as *mut RECT).cast(),
                    2,
                );
                assert_eq!(
                    visible_bar.rcScrollBar.left, clip.right,
                    "the viewport's own scrollbar must remain at its client edge"
                );
                assert!(visible_bar.rcScrollBar.right > visible_bar.rcScrollBar.left);
                GetClientRect(list, &mut client);
                MapWindowPoints(list, viewport, (&mut client as *mut RECT).cast(), 2);
                GetWindowRect(list, &mut outer);
                let mut bar = SCROLLBARINFO {
                    cbSize: std::mem::size_of::<SCROLLBARINFO>() as u32,
                    ..Default::default()
                };
                let present = GetScrollBarInfo(list, OBJID_VSCROLL, &mut bar) != 0;
                MapWindowPoints(
                    null_mut(),
                    viewport,
                    (&mut bar.rcScrollBar as *mut RECT).cast(),
                    2,
                );
                let style = GetWindowLongPtrW(list, GWL_STYLE) as u32 & WS_VSCROLL;
                let overlap = if present && style != 0 {
                    (bar.rcScrollBar.right.min(clip.right) - bar.rcScrollBar.left.max(0)).max(0)
                } else {
                    0
                };
                (
                    clip.right,
                    client.right,
                    overlap,
                    style,
                    (bar.rcScrollBar.left, bar.rcScrollBar.right),
                    outer.right - outer.left,
                )
            }
        }

        #[test]
        fn dark_scrollbar_applies_on_first_call_and_keeps_pressed_track_dark() {
            unsafe {
                use windows_sys::Win32::UI::Controls::{
                    CloseThemeData, DrawThemeBackground, OpenThemeData, SBP_LOWERTRACKVERT,
                    SCRBS_PRESSED,
                };
                let windows = hidden_results();
                // A new, never-opted-in control catches skipping SetWindowTheme
                // because of the undocumented opt-in's first-call return value.
                let control = CreateWindowExW(
                    0,
                    wide("STATIC").as_ptr(),
                    null(),
                    WS_CHILD | WS_VSCROLL,
                    0,
                    0,
                    100,
                    200,
                    windows.owner,
                    null_mut(),
                    GetModuleHandleW(null()),
                    null(),
                );
                assert!(!control.is_null()); // Owned/destroyed by TestWindows.
                let supported = crate::dark_scrollbar::initialize();
                assert_eq!(crate::dark_scrollbar::apply(control), supported);
                assert_eq!(crate::dark_scrollbar::apply(control), supported);
                if !supported {
                    return;
                }
                // Exercise native theme data, not our idle/print painter. This
                // still does NOT prove USER32's pixels during a live tracker loop.
                for control in [
                    control,
                    GetDlgItem(windows.owner, RESULTS_VIEWPORT),
                    results(windows.owner),
                ] {
                    let theme = OpenThemeData(control, wide("ScrollBar").as_ptr());
                    assert_ne!(theme, 0);
                    let mut raster = NativeRaster::new((16, 64));
                    let rect = RECT {
                        left: 0,
                        top: 0,
                        right: 16,
                        bottom: 64,
                    };
                    let drawn = DrawThemeBackground(
                        theme,
                        raster.dc,
                        SBP_LOWERTRACKVERT,
                        SCRBS_PRESSED,
                        &rect,
                        null(),
                    );
                    CloseThemeData(theme);
                    assert!(drawn >= 0);
                    let pixels = raster.pixels();
                    assert!(
                        pixels
                            .as_chunks::<4>()
                            .0
                            .iter()
                            .all(|pixel| { pixel[..3].iter().all(|&channel| channel < 160) }),
                        "native pressed-track theme must be dark, not the stock light theme"
                    );
                }
            }
        }

        #[test]
        fn search_native_accessibility_keeps_list_scroll_pattern() {
            unsafe {
                let windows = hidden_results();
                // Verify the native opt-in, not custom WM_PRINT pixels, which
                // cannot establish the tracker's appearance during a live drag.
                use windows_sys::Win32::System::LibraryLoader::GetProcAddress;
                let supported = crate::dark_scrollbar::initialize();
                assert_eq!(crate::dark_scrollbar::initialize(), supported);
                assert!(!crate::dark_scrollbar::apply(null_mut()));
                for control in [
                    GetDlgItem(windows.owner, RESULTS_VIEWPORT),
                    results(windows.owner),
                ] {
                    assert_ne!(GetWindowLongW(control, GWL_STYLE) as u32 & WS_VSCROLL, 0);
                    let geometry = || {
                        let mut bar = SCROLLBARINFO {
                            cbSize: std::mem::size_of::<SCROLLBARINFO>() as u32,
                            ..Default::default()
                        };
                        assert_ne!(GetScrollBarInfo(control, OBJID_VSCROLL, &mut bar), 0);
                        assert!(bar.rcScrollBar.right > bar.rcScrollBar.left);
                        assert!(bar.rcScrollBar.bottom > bar.rcScrollBar.top);
                        (
                            bar.rcScrollBar.left,
                            bar.rcScrollBar.top,
                            bar.rcScrollBar.right,
                            bar.rcScrollBar.bottom,
                            bar.dxyLineButton,
                            bar.xyThumbTop,
                            bar.xyThumbBottom,
                        )
                    };
                    let before = geometry();
                    assert_eq!(crate::dark_scrollbar::apply(control), supported);
                    assert_eq!(crate::dark_scrollbar::apply(control), supported);
                    assert_ne!(GetWindowLongW(control, GWL_STYLE) as u32 & WS_VSCROLL, 0);
                    assert_eq!(
                        geometry(),
                        before,
                        "idempotent dark theming must retain native geometry"
                    );
                    if supported {
                        let module = GetModuleHandleW(wide("uxtheme.dll").as_ptr());
                        if let Some(query) = GetProcAddress(module, 137_usize as *const u8) {
                            let allowed: unsafe extern "system" fn(HWND) -> bool =
                                std::mem::transmute(query);
                            assert!(
                                allowed(control),
                                "the native scrollbar window must opt into dark tracking"
                            );
                        }
                    }
                }
                SendMessageW(windows.owner, WM_THEMECHANGED, 0, 0);
                let list = results(windows.owner) as usize;
                for (pass, name) in [
                    tr("Search results"),
                    crate::i18n::tr_in(crate::i18n::Language::Vietnamese, "Search results"),
                ]
                .into_iter()
                .enumerate()
                {
                    if pass > 0 {
                        assert!(name_results(windows.owner, name));
                    }
                    let (send, receive) = std::sync::mpsc::channel();
                    let worker = std::thread::spawn(move || {
                        use windows::Win32::System::Com::{
                            CLSCTX_INPROC_SERVER, COINIT_MULTITHREADED, CoCreateInstance,
                            CoInitializeEx, CoUninitialize,
                        };
                        use windows::Win32::UI::Accessibility::{
                            CUIAutomation, IUIAutomation, IUIAutomationScrollPattern,
                            UIA_ScrollPatternId,
                        };
                        use windows::core::Interface;
                        // UIA clients must run off the owning window thread; that
                        // thread continues pumping native provider messages below.
                        let result = (|| -> windows::core::Result<(bool, String)> {
                            CoInitializeEx(None, COINIT_MULTITHREADED).ok()?;
                            struct ComGuard;
                            impl Drop for ComGuard {
                                fn drop(&mut self) {
                                    unsafe {
                                        CoUninitialize();
                                    }
                                }
                            }
                            let _com = ComGuard;
                            let automation: IUIAutomation =
                                CoCreateInstance(&CUIAutomation, None, CLSCTX_INPROC_SERVER)?;
                            let element = automation.ElementFromHandle(
                                windows::Win32::Foundation::HWND(list as HWND),
                            )?;
                            let pattern: IUIAutomationScrollPattern =
                                element.GetCurrentPattern(UIA_ScrollPatternId)?.cast()?;
                            let scrollable = pattern.CurrentVerticallyScrollable()?.as_bool();
                            pattern.SetScrollPercent(-1.0, 50.0)?;
                            Ok((scrollable, element.CurrentName()?.to_string()))
                        })();
                        send.send(result.map_err(|error| error.to_string()))
                            .unwrap();
                    });
                    let mut response = None;
                    assert!(
                        pump_until(Duration::from_secs(10), || {
                            response = receive.try_recv().ok();
                            response.is_some()
                        }),
                        "native UIA query timed out"
                    );
                    assert_eq!(
                        response.unwrap(),
                        Ok((true, name.to_owned())),
                        "the real listbox must retain its native UIA ScrollPattern and explicit name"
                    );
                    assert!(
                        access(windows.owner, |state| state.results_scroll.offset() > 0.0).unwrap()
                            && displayed_row_top(windows.owner, 0) < 0,
                        "UIA scrolling must move the actual pixel viewport: {}",
                        uia_scroll_snapshot(windows.owner)
                    );
                    let (width, client_right, overlap, style, _, _) =
                        results_scrollbar_geometry(windows.owner);
                    assert_eq!(overlap, 0, "UIA scrolling must not expose a second bar");
                    assert_eq!(client_right, width);
                    assert_ne!(style, 0);
                    worker.join().unwrap();
                }
            }
        }

        unsafe fn uia_scroll_snapshot(hwnd: HWND) -> String {
            unsafe {
                let list = results(hwnd);
                let viewport = GetDlgItem(hwnd, RESULTS_VIEWPORT);
                let top = SendMessageW(list, LB_GETTOPINDEX, 0, 0);
                let mut native = SCROLLINFO {
                    cbSize: std::mem::size_of::<SCROLLINFO>() as u32,
                    fMask: SIF_ALL,
                    ..Default::default()
                };
                let mut pixels = SCROLLINFO {
                    cbSize: std::mem::size_of::<SCROLLINFO>() as u32,
                    fMask: SIF_ALL,
                    ..Default::default()
                };
                GetScrollInfo(list, SB_VERT, &mut native);
                GetScrollInfo(viewport, SB_VERT, &mut pixels);
                format!(
                    "top={top}, state={:?}, row0_y={}, native_bar=({},{},{},{}), pixel_bar=({},{},{},{})",
                    access(hwnd, |state| (
                        state.results_scroll.offset(),
                        state.results_scroll.target(),
                        state.results_syncing
                    )),
                    displayed_row_top(hwnd, 0),
                    native.nPos,
                    native.nTrackPos,
                    native.nMax,
                    native.nPage,
                    pixels.nPos,
                    pixels.nTrackPos,
                    pixels.nMax,
                    pixels.nPage
                )
            }
        }

        #[test]
        fn search_row_cache_tracks_provider_duration_timing_width_and_dpi() {
            unsafe {
                let windows = hidden_results();
                access(windows.owner, |state| {
                    let first = result_raster(state, 0, (440, 64), ButtonStyle::default());
                    state.session.results[0].source = "NetEase".into();
                    let provider = result_raster(state, 0, (440, 64), ButtonStyle::default());
                    assert!(!Arc::ptr_eq(&first, &provider));
                    state.session.results[0].duration = 260.0;
                    let duration = result_raster(state, 0, (440, 64), ButtonStyle::default());
                    assert!(!Arc::ptr_eq(&provider, &duration));
                    state.session.results[0].timing = "Words".into();
                    let timing = result_raster(state, 0, (440, 64), ButtonStyle::default());
                    assert!(!Arc::ptr_eq(&duration, &timing));
                    let width = result_raster(state, 0, (400, 64), ButtonStyle::default());
                    assert!(!Arc::ptr_eq(&timing, &width));
                    state.painter = Painter::new(2.0).unwrap();
                    let dpi = result_raster(state, 0, (440, 64), ButtonStyle::default());
                    assert!(!Arc::ptr_eq(&timing, &dpi));
                });
            }
        }

        #[test]
        fn search_row_cache_reuses_pixels_and_invalidates_for_changed_results() {
            unsafe {
                let windows = hidden_results();
                let hwnd = windows.owner;
                let (first, second, selected) = access(hwnd, |state| {
                    let first = result_raster(state, 0, (440, 64), ButtonStyle::default());
                    let second = result_raster(state, 0, (440, 64), ButtonStyle::default());
                    let selected = result_raster(
                        state,
                        0,
                        (440, 64),
                        ButtonStyle {
                            selected: true,
                            ..Default::default()
                        },
                    );
                    (first, second, selected)
                })
                .unwrap();
                assert!(
                    Arc::ptr_eq(&first, &second),
                    "unchanged repaint must reuse the finished bitmap"
                );
                assert_ne!(
                    first, selected,
                    "selection must retain its visible native state"
                );
                access(hwnd, |state| state.session.begin())
                    .unwrap()
                    .deliver(Completion::Search(Ok(vec![SearchResult::fixture(
                        "A different recording",
                        "Another artist",
                    )])));
                poll(hwnd);
                let changed = access(hwnd, |state| {
                    result_raster(state, 0, (440, 64), ButtonStyle::default())
                })
                .unwrap();
                assert_ne!(
                    first, changed,
                    "new result at the same index must not reuse stale text"
                );
            }
        }

        #[test]
        #[ignore = "Hidden-window paired raster benchmark; no focus or network"]
        fn search_row_raster_cached() {
            unsafe {
                let windows = hidden_results();
                access(windows.owner, |state| {
                    state.painter = Painter::new(1.25).unwrap();
                    state.session.results[0].title =
                        "Bản thu âm trực tiếp — A very long bilingual recording title ".repeat(3);
                    state.session.results[0].artist =
                        "Nghệ sĩ · A long bilingual artist description ".repeat(3);
                    let mut cold = Vec::new();
                    for _ in 0..32 {
                        state.row_cache.clear();
                        let started = Instant::now();
                        std::hint::black_box(result_raster(
                            state,
                            0,
                            (700, 73),
                            ButtonStyle::default(),
                        ));
                        cold.push(started.elapsed().as_nanos());
                    }
                    let mut warm = Vec::new();
                    for _ in 0..1000 {
                        let started = Instant::now();
                        std::hint::black_box(result_raster(
                            state,
                            0,
                            (700, 73),
                            ButtonStyle::default(),
                        ));
                        warm.push(started.elapsed().as_nanos());
                    }
                    cold.sort_unstable();
                    warm.sort_unstable();
                    eprintln!(
                        "search paired row raster: cold median={}ns p95={}ns (32), cache-hit median={}ns p95={}ns (1000), 125% DPI, debug; excludes GDI blit",
                        cold[16], cold[30], warm[500], warm[950]
                    );
                });
            }
        }

        #[test]
        fn search_scroll_bounds_resize_and_keyboard_reveal_match_the_native_scrollbar() {
            unsafe {
                let windows = hidden_results();
                let hwnd = windows.owner;
                let list = results(hwnd);
                let viewport = GetDlgItem(hwnd, RESULTS_VIEWPORT);
                SendMessageW(viewport, WM_VSCROLL, SB_BOTTOM as usize, 0);
                let mut info = SCROLLINFO {
                    cbSize: std::mem::size_of::<SCROLLINFO>() as u32,
                    fMask: SIF_ALL,
                    ..Default::default()
                };
                GetScrollInfo(viewport, SB_VERT, &mut info);
                assert_eq!(info.nPos, 1356);
                assert_eq!(displayed_row_top(hwnd, 23), 116);
                SendMessageW(list, WM_MOUSEWHEEL, (-120_i16 as u16 as usize) << 16, 0);
                assert!(
                    !access(hwnd, |state| state.results_scroll.moving()).unwrap(),
                    "outward wheel input at the bottom must remain stopped"
                );
                MoveWindow(viewport, 0, 0, 560, 300, 0);
                GetScrollInfo(viewport, SB_VERT, &mut info);
                assert_eq!(info.nPos, 1236);
                assert_eq!(displayed_row_top(hwnd, 23), 236);
                SendMessageW(list, WM_KEYDOWN, 0x24, 0);
                assert_eq!(SendMessageW(list, LB_GETCURSEL, 0, 0), 0);
                assert_eq!(displayed_row_top(hwnd, 0), 0);
                assert!(!access(hwnd, |state| state.results_scroll.moving()).unwrap());
            }
        }

        #[test]
        #[ignore = "Foreground native selection/double-click/scroll preview; coordinator runs serially"]
        fn native_search_smooth_viewport_preview() {
            unsafe {
                let owner = CreateWindowExW(
                    0,
                    wide("STATIC").as_ptr(),
                    null(),
                    WS_POPUP,
                    0,
                    0,
                    100,
                    100,
                    null_mut(),
                    null_mut(),
                    GetModuleHandleW(null()),
                    null(),
                );
                let mut windows = TestWindows {
                    owner,
                    searches: Vec::new(),
                };
                show(
                    owner,
                    &MediaSnapshot {
                        key: "smooth-preview".into(),
                        generation: 42,
                        title: "Bản thu âm trực tiếp".into(),
                        artist: "Nghệ sĩ — Authentic artist".into(),
                        ..Default::default()
                    },
                );
                let hwnd = WINDOW.with(Cell::get);
                windows.searches.push(hwnd);
                let job = access(hwnd, |state| state.session.begin()).unwrap();
                job.deliver(Completion::Search(Ok((0..24).map(|i| {
                    SearchResult::fixture(&format!("Bản thu âm — Recording {i}: A long live bilingual recording title that must remain clipped"), "Nghệ sĩ — Authentic artist and a long collaboration label")
                }).collect())));
                poll(hwnd);
                let list = results(hwnd);
                let viewport = GetDlgItem(hwnd, RESULTS_VIEWPORT);
                SetFocus(list);
                if let Some(dir) = std::env::var_os("HIYODYNA_PREVIEW_DIR") {
                    let dir = std::path::PathBuf::from(dir);
                    RedrawWindow(
                        hwnd,
                        null(),
                        null_mut(),
                        RDW_INVALIDATE | RDW_ALLCHILDREN | RDW_UPDATENOW,
                    );
                    for offset in [5.0, 68.0, 60.0, 128.0, 121.0] {
                        access(hwnd, |s| s.results_scroll.jump(offset));
                        position_results(hwnd);
                        UpdateWindow(list);
                        let partial = dir.join("search-scroll-strip.png");
                        crate::preferences::tests::save_window(hwnd, &partial);
                        RedrawWindow(list, null(), null_mut(), RDW_INVALIDATE | RDW_UPDATENOW);
                        let full = dir.join("search-scroll-full.png");
                        crate::preferences::tests::save_window(hwnd, &full);
                        let partial = image::open(partial).unwrap().into_rgba8();
                        let full = image::open(full).unwrap().into_rgba8();
                        assert!(
                            partial == full,
                            "strip repaint must match a clean repaint at offset {offset}"
                        );
                    }
                    access(hwnd, |s| s.results_scroll.jump(0.0));
                    position_results(hwnd);
                }
                SendMessageW(list, WM_MOUSEWHEEL, (-120_i16 as u16 as usize) << 16, 0);
                assert_eq!(
                    access(hwnd, |state| state.results_scroll.offset()),
                    Some(0.0)
                );
                access(hwnd, |state| state.results_scroll.jump(0.0));
                let now = Instant::now();
                wheel_results(hwnd, (-120_i16 as u16 as usize) << 16, now);
                tick_results(hwnd, now + Duration::from_millis(12));
                assert!(access(hwnd, |state| state.results_scroll.moving()).unwrap());
                let mut point = windows_sys::Win32::Foundation::POINT { x: 20, y: 10 };
                MapWindowPoints(viewport, list, &mut point, 1);
                let hit = ((point.y as u16 as u32) << 16 | point.x as u16 as u32) as isize;
                let expected = SendMessageW(list, LB_ITEMFROMPOINT, 0, hit) & 0xffff;
                SendMessageW(list, WM_LBUTTONDOWN, 1, hit);
                SendMessageW(list, WM_LBUTTONUP, 0, hit);
                assert_eq!(SendMessageW(list, LB_GETCURSEL, 0, 0), expected);
                // Exercise real LBN_DBLCLK dispatch without making a provider request.
                access(hwnd, |state| state.session.target.key.clear());
                SendMessageW(list, WM_LBUTTONDBLCLK, 1, hit);
                SendMessageW(list, WM_LBUTTONUP, 0, hit);
                assert_eq!(
                    input(hwnd, STATUS),
                    tr("Play a media track before applying lyrics. You can still search.")
                );
                let (width, client_right, overlap, style, _, _) = results_scrollbar_geometry(hwnd);
                assert_eq!(
                    overlap, 0,
                    "the actual partial-row preview must expose only one scrollbar"
                );
                assert_eq!(client_right, width);
                assert_ne!(style, 0);
                let right_hit =
                    ((point.y as u16 as u32) << 16 | (width - 1) as u16 as u32) as isize;
                assert_eq!(
                    SendMessageW(list, LB_ITEMFROMPOINT, 0, right_hit),
                    expected,
                    "the rightmost visible column must remain inside the same native row"
                );
                if let Some(dir) = std::env::var_os("HIYODYNA_PREVIEW_DIR") {
                    RedrawWindow(
                        hwnd,
                        null(),
                        null_mut(),
                        RDW_INVALIDATE | RDW_ALLCHILDREN | RDW_UPDATENOW,
                    );
                    crate::preferences::tests::save_window(
                        hwnd,
                        &std::path::PathBuf::from(dir).join("search-smooth-partial-row.png"),
                    );
                }
                SendMessageW(viewport, WM_VSCROLL, SB_BOTTOM as usize, 0);
                let (_, _, _, max) = results_metrics(hwnd);
                assert_eq!(
                    access(hwnd, |state| state.results_scroll.offset()),
                    Some(max)
                );
                assert!(!access(hwnd, |state| state.results_scroll.moving()).unwrap());
                SendMessageW(list, WM_KEYDOWN, 0x24, 0);
                assert_eq!(SendMessageW(list, LB_GETCURSEL, 0, 0), 0);
                assert_eq!(
                    access(hwnd, |state| state.results_scroll.offset()),
                    Some(0.0)
                );
                assert_eq!(GetFocus(), list);
            }
        }

        // Track only this test's windows. The built-in STATIC owner never posts
        // WM_QUIT, and cleanup runs even if a provider assertion panics.
        struct TestWindows {
            owner: HWND,
            searches: Vec<HWND>,
        }

        impl Drop for TestWindows {
            fn drop(&mut self) {
                unsafe {
                    for &hwnd in self.searches.iter().rev() {
                        if IsWindow(hwnd) != 0 && GetWindow(hwnd, GW_OWNER) == self.owner {
                            DestroyWindow(hwnd);
                        }
                    }
                    if IsWindow(self.owner) != 0 {
                        DestroyWindow(self.owner);
                    }
                }
            }
        }

        fn pump_until(timeout: Duration, mut done: impl FnMut() -> bool) -> bool {
            let deadline = Instant::now() + timeout;
            loop {
                unsafe {
                    let mut message = MSG::default();
                    while PeekMessageW(&mut message, null_mut(), 0, 0, PM_REMOVE) != 0 {
                        if message.message == WM_QUIT {
                            // Preserve a pre-existing thread quit request; this
                            // test must neither swallow nor introduce one.
                            PostQuitMessage(message.wParam as i32);
                            panic!("Unexpected WM_QUIT during isolated lyric search smoke test");
                        }
                        if !translate_message(&message) {
                            TranslateMessage(&message);
                            DispatchMessageW(&message);
                        }
                    }
                }
                if done() {
                    return true;
                }
                if Instant::now() >= deadline {
                    return false;
                }
                std::thread::park_timeout(Duration::from_millis(10));
            }
        }

        fn key(hwnd: HWND, code: usize) -> MSG {
            MSG {
                hwnd,
                message: WM_KEYDOWN,
                wParam: code,
                ..Default::default()
            }
        }

        unsafe fn native_focus_snapshot(hwnd: HWND) -> String {
            unsafe {
                use windows_sys::Win32::UI::Input::KeyboardAndMouse::{
                    GetAsyncKeyState, GetKeyState, VK_CONTROL, VK_MENU, VK_SHIFT,
                };
                let focus = GetFocus();
                let next = GetNextDlgTabItem(hwnd, focus, 0);
                let previous = GetNextDlgTabItem(hwnd, focus, 1);
                format!(
                    "focus={focus:?}/{} parent={:?} next={next:?}/{} previous={previous:?}/{} title={:?} artist={:?} list={:?} foreground={:?} shift={:#x} async_shift={:#x} ctrl={:#x} alt={:#x}",
                    GetDlgCtrlID(focus),
                    GetParent(focus),
                    GetDlgCtrlID(next),
                    GetDlgCtrlID(previous),
                    GetDlgItem(hwnd, TITLE),
                    GetDlgItem(hwnd, ARTIST),
                    results(hwnd),
                    GetForegroundWindow(),
                    GetKeyState(VK_SHIFT as i32) as u16,
                    GetAsyncKeyState(VK_SHIFT as i32) as u16,
                    GetKeyState(VK_CONTROL as i32) as u16,
                    GetKeyState(VK_MENU as i32) as u16
                )
            }
        }

        unsafe fn native_tab(hwnd: HWND, from: HWND, to: HWND, reverse: bool) {
            unsafe {
                SetFocus(from);
                assert_eq!(
                    GetFocus(),
                    from,
                    "native Tab precondition: {}",
                    native_focus_snapshot(hwnd)
                );
                let _keyboard = NativeTestKeyboard::new(reverse);
                assert!(translate_message(&key(from, 9)));
                assert_eq!(
                    GetFocus(),
                    to,
                    "actual native Tab reverse={reverse}: {}",
                    native_focus_snapshot(hwnd)
                );
            }
        }

        #[test]
        #[ignore = "Creates a native search window; run serially with the desktop verification pass"]
        fn live_language_refresh_preserves_search_edits_focus_scroll_and_pending_job() {
            unsafe {
                let previous = crate::i18n::language();
                struct LanguageGuard(crate::i18n::Language);
                impl Drop for LanguageGuard {
                    fn drop(&mut self) {
                        crate::i18n::set_language(self.0);
                    }
                }
                let _language = LanguageGuard(previous);
                crate::i18n::set_language(crate::i18n::Language::English);
                let owner = CreateWindowExW(
                    0,
                    wide("STATIC").as_ptr(),
                    null(),
                    WS_POPUP,
                    0,
                    0,
                    100,
                    100,
                    null_mut(),
                    null_mut(),
                    GetModuleHandleW(null()),
                    null(),
                );
                let mut windows = TestWindows {
                    owner,
                    searches: Vec::new(),
                };
                show(
                    owner,
                    &MediaSnapshot {
                        key: "language-fixture".into(),
                        generation: 42,
                        ..Default::default()
                    },
                );
                let hwnd = WINDOW.with(Cell::get);
                windows.searches.push(hwnd);
                text(hwnd, TITLE, "Unchanged title Ω");
                text(hwnd, ARTIST, "Nghệ sĩ auténtico");
                let job = access(hwnd, |state| state.session.begin()).unwrap();
                job.deliver(Completion::Search(Ok((0..20)
                    .map(|i| SearchResult::fixture(&format!("Recording {i}"), "Authentic artist"))
                    .collect())));
                poll(hwnd);
                let pending = access(hwnd, |state| state.session.begin()).unwrap();
                let list = results(hwnd);
                SendMessageW(list, LB_SETCURSEL, 12, 0);
                SendMessageW(list, LB_SETTOPINDEX, 10, 0);
                let top = SendMessageW(list, LB_GETTOPINDEX, 0, 0);
                SetFocus(GetDlgItem(hwnd, ARTIST));
                crate::i18n::set_language(crate::i18n::Language::Vietnamese);
                refresh_language();
                assert_eq!(input(hwnd, TITLE), "Unchanged title Ω");
                assert_eq!(input(hwnd, ARTIST), "Nghệ sĩ auténtico");
                assert_eq!(GetFocus(), GetDlgItem(hwnd, ARTIST));
                assert_eq!(SendMessageW(list, LB_GETCURSEL, 0, 0), 12);
                assert_eq!(SendMessageW(list, LB_GETTOPINDEX, 0, 0), top);
                assert!(
                    pending.current(),
                    "Relabeling must not invalidate an in-flight request"
                );
                assert_eq!(input(hwnd, SEARCH), crate::i18n::tr("&Search"));
                assert_eq!(
                    access(hwnd, |state| state.session.target.generation),
                    Some(42)
                );
            }
        }

        #[test]
        #[ignore = "Creates native lyric search states and screenshots without network requests"]
        fn native_lyric_search_states_preview() {
            let previous = crate::i18n::language();
            struct LanguageGuard(crate::i18n::Language);
            impl Drop for LanguageGuard {
                fn drop(&mut self) {
                    crate::i18n::set_language(self.0);
                }
            }
            let _language = LanguageGuard(previous);
            for language in [
                crate::i18n::Language::English,
                crate::i18n::Language::Vietnamese,
            ] {
                crate::i18n::set_language(language);
                for preview_scale in [1.0, 1.25, 1.5, 2.0] {
                    if std::env::var("HIYODYNA_PREVIEW_CASE").is_ok_and(|case| {
                        case != format!("{}-{}", language.as_str(), (preview_scale * 100.0) as u32)
                    }) {
                        continue;
                    }
                    unsafe {
                        let owner = CreateWindowExW(
                            0,
                            wide("STATIC").as_ptr(),
                            null(),
                            WS_OVERLAPPEDWINDOW,
                            30,
                            30,
                            300,
                            180,
                            null_mut(),
                            null_mut(),
                            GetModuleHandleW(null()),
                            null(),
                        );
                        let mut windows = TestWindows {
                            owner,
                            searches: Vec::new(),
                        };
                        let snapshot = MediaSnapshot {
                            key: "preview".into(),
                            generation: 42,
                            title: "Blinding Lights".into(),
                            artist: "The Weeknd".into(),
                            ..Default::default()
                        };
                        let hwnd = create(owner, &snapshot).unwrap();
                        windows.searches.push(hwnd);
                        assert!(!hwnd.is_null());
                        // Synthetic DPI fixtures need a full-size memory-DC
                        // host, not this desktop's maximum tracking rectangle.
                        // Production hosts retain their work-area clamping;
                        // the separate real-host minimum fixture covers it.
                        let mut proposed = RECT::default();
                        GetWindowRect(hwnd, &mut proposed);
                        let dpi = (preview_scale * 96.0) as usize;
                        SendMessageW(
                            hwnd,
                            WM_DPICHANGED,
                            dpi | (dpi << 16),
                            (&proposed as *const RECT) as isize,
                        );
                        SetWindowLongW(hwnd, GWL_STYLE, (WS_POPUP | WS_CLIPCHILDREN) as i32);
                        let dc = GetDC(hwnd);
                        let font = SendMessageW(GetDlgItem(hwnd, TITLE), WM_GETFONT, 0, 0) as HFONT;
                        let previous = SelectObject(dc, font);
                        let mut face = [0_u16; 64];
                        let len = GetTextFaceW(dc, face.len() as i32, face.as_mut_ptr());
                        SelectObject(dc, previous);
                        ReleaseDC(hwnd, dc);
                        let face =
                            String::from_utf16_lossy(&face[..len.saturating_sub(1) as usize]);
                        assert!(
                            face.starts_with("Inter"),
                            "native input must use the bundled sans-serif font, got {face}"
                        );
                        SetWindowPos(
                            hwnd,
                            null_mut(),
                            -16000,
                            -16000,
                            0,
                            0,
                            SWP_NOSIZE | SWP_NOACTIVATE | SWP_NOZORDER | SWP_FRAMECHANGED,
                        );
                        let dir = std::path::PathBuf::from(
                            std::env::var_os("HIYODYNA_PREVIEW_DIR").expect("preview directory"),
                        );
                        std::fs::create_dir_all(&dir).unwrap();
                        let preview_size = Cell::new((640.0, 560.0));
                        let resize = |width: f64, height: f64| {
                            preview_size.set((width, height));
                            let mut client = RECT::default();
                            let mut outer = RECT::default();
                            GetClientRect(hwnd, &mut client);
                            GetWindowRect(hwnd, &mut outer);
                            SetWindowPos(
                                hwnd,
                                null_mut(),
                                0,
                                0,
                                (width * preview_scale).round() as i32 + outer.right
                                    - outer.left
                                    - client.right,
                                (height * preview_scale).round() as i32 + outer.bottom
                                    - outer.top
                                    - client.bottom,
                                SWP_NOMOVE | SWP_NOZORDER | SWP_NOACTIVATE,
                            );
                            if access(hwnd, |s| s.painter.scale != preview_scale as f32).unwrap() {
                                set_font_scaled(hwnd, preview_scale);
                            }
                            layout_scaled(hwnd, preview_scale);
                            for (id, x) in [
                                (crate::window_chrome::CLOSE, width - 44.0),
                                (crate::window_chrome::MAXIMIZE, width - 76.0),
                            ] {
                                MoveWindow(
                                    GetDlgItem(hwnd, id),
                                    (x * preview_scale).round() as i32,
                                    (14.0 * preview_scale).round() as i32,
                                    (32.0 * preview_scale).round() as i32,
                                    (30.0 * preview_scale).round() as i32,
                                    0,
                                );
                            }
                            GetClientRect(hwnd, &mut client);
                            assert_eq!(
                                (client.right, client.bottom),
                                (
                                    (width * preview_scale).round() as i32,
                                    (height * preview_scale).round() as i32
                                )
                            );
                        };
                        resize(640.0, 560.0);
                        // Standard EDIT printing needs an initialized shown
                        // host. Show it only after placing it beyond all screens.
                        ShowWindow(hwnd, SW_SHOWNOACTIVATE);
                        let mut offscreen = RECT::default();
                        GetWindowRect(hwnd, &mut offscreen);
                        assert!(
                            offscreen.right < -10000 && offscreen.bottom < -10000,
                            "the preview host must never enter the visible desktop"
                        );
                        let capture = |name: &str| {
                            // Activation/dialog messages use real desktop DPI.
                            // Reapply synthetic fixture geometry before capture.
                            let (width, height) = preview_size.get();
                            resize(width, height);
                            RedrawWindow(
                                hwnd,
                                null(),
                                null_mut(),
                                RDW_INVALIDATE | RDW_ALLCHILDREN | RDW_UPDATENOW,
                            );
                            let image = printed_window(hwnd);
                            image
                                .save(dir.join(format!(
                                    "lyrics-{}-{}-{name}.png",
                                    language.as_str(),
                                    (preview_scale * 100.0) as u32
                                )))
                                .unwrap();
                            for id in [TITLE, ARTIST] {
                                if input(hwnd, id).is_empty() {
                                    continue;
                                }
                                let mut bounds = RECT::default();
                                GetWindowRect(GetDlgItem(hwnd, id), &mut bounds);
                                MapWindowPoints(
                                    null_mut(),
                                    hwnd,
                                    (&mut bounds as *mut RECT).cast(),
                                    2,
                                );
                                let printed = (bounds.top..bounds.bottom).any(|y| {
                                    (bounds.left..bounds.right).any(|x| {
                                        image.get_pixel(x as u32, y as u32).0[..3]
                                            .iter()
                                            .all(|&v| v > 200)
                                    })
                                });
                                assert!(
                                    printed,
                                    "{language:?} at {preview_scale}x {name}: native edit {id} must print its current query"
                                );
                            }
                            image
                        };
                        assert_eq!(IsWindowEnabled(GetDlgItem(hwnd, APPLY)), 0);
                        capture("initial");
                        let list = results(hwnd);
                        native_tab(hwnd, GetDlgItem(hwnd, SEARCH), list, false);
                        native_tab(hwnd, list, GetDlgItem(hwnd, SEARCH), true);
                        native_tab(hwnd, list, GetDlgItem(hwnd, CANCEL), false);
                        native_tab(hwnd, GetDlgItem(hwnd, CANCEL), list, true);
                        {
                            let _held_shift = NativeTestKeyboard::new(true);
                            native_tab(
                                hwnd,
                                GetDlgItem(hwnd, TITLE),
                                GetDlgItem(hwnd, ARTIST),
                                false,
                            );
                            assert!(
                                windows_sys::Win32::UI::Input::KeyboardAndMouse::GetKeyState(
                                    windows_sys::Win32::UI::Input::KeyboardAndMouse::VK_SHIFT
                                        as i32
                                ) < 0
                            );
                        }
                        SendMessageW(hwnd, WM_ACTIVATE, WA_INACTIVE as usize, 0);
                        SetFocus(owner);
                        SendMessageW(hwnd, WM_SETFOCUS, 0, 0);
                        assert_eq!(GetFocus(), GetDlgItem(hwnd, ARTIST));
                        text(hwnd, STATUS, "Searching lyric providers...");
                        capture("loading");
                        tick_results(hwnd, Instant::now() + Duration::from_millis(300));
                        capture("loading-advanced");
                        let job = access(hwnd, |state| state.session.begin()).unwrap();
                        let mut matches = vec![
                            SearchResult::fixture("Blinding Lights", "The Weeknd"),
                            SearchResult::fixture("Blinding Lights (Live)", "The Weeknd"),
                            SearchResult::fixture(
                                "Blinding Lights — Alternate recording",
                                "The Weeknd",
                            ),
                        ];
                        matches[0].timing = "Words".into();
                        matches[1].source = "AMLL TTML DB".into();
                        matches[2].timing = "Unknown".into();
                        matches[2].source = "NetEase".into();
                        job.deliver(Completion::Search(Ok(matches)));
                        poll(hwnd);
                        let list = results(hwnd);
                        assert_eq!(SendMessageW(list, LB_GETCOUNT, 0, 0), 3);
                        assert_ne!(IsWindowEnabled(GetDlgItem(hwnd, APPLY)), 0);
                        for (from, to) in [
                            (GetDlgItem(hwnd, TITLE), GetDlgItem(hwnd, ARTIST)),
                            (GetDlgItem(hwnd, ARTIST), GetDlgItem(hwnd, SEARCH)),
                            (GetDlgItem(hwnd, SEARCH), list),
                            (list, GetDlgItem(hwnd, APPLY)),
                            (GetDlgItem(hwnd, APPLY), GetDlgItem(hwnd, CANCEL)),
                        ] {
                            native_tab(hwnd, from, to, false);
                            native_tab(hwnd, to, from, true);
                        }
                        SetFocus(list);
                        capture("selected");
                        text(hwnd, STATUS, "Fetching selected lyrics...");
                        apply_enabled(hwnd, false);
                        capture("fetching");
                        apply_enabled(hwnd, true);
                        let mut row = RECT::default();
                        SendMessageW(list, LB_GETITEMRECT, 0, (&mut row as *mut RECT) as isize);
                        assert_eq!(
                            row.bottom - row.top,
                            (f64::from(ROW_HEIGHT) * preview_scale).round() as i32
                        );
                        access(hwnd, |state| state.session.begin())
                            .unwrap()
                            .deliver(Completion::Fetch(Err("Preview provider failure".into())));
                        poll(hwnd);
                        capture("error");
                        status(hwnd, "Instrumental", None);
                        capture("instrumental");
                        access(hwnd, |state| state.session.begin())
                            .unwrap()
                            .deliver(Completion::Search(Ok(Vec::new())));
                        poll(hwnd);
                        capture("empty");
                        resize(520.0, 420.0);
                        capture("minimum");
                        reset(hwnd, &MediaSnapshot::default());
                        assert_eq!(SendMessageW(list, LB_GETCOUNT, 0, 0), 0);
                        assert_eq!(IsWindowEnabled(GetDlgItem(hwnd, APPLY)), 0);
                        assert_ne!(IsWindowEnabled(GetDlgItem(hwnd, SEARCH)), 0);
                        capture("no-track");
                        if preview_scale == 1.0 || preview_scale == 2.0 {
                            reset(hwnd, &snapshot);
                            resize(640.0, 420.0);
                            let matches = (1..=24)
                                .map(|index| {
                                    let mut result = SearchResult::fixture(
                                        &format!("Blinding Lights — Recording {index}"),
                                        "The Weeknd",
                                    );
                                    match index % 3 {
                                        0 => {
                                            result.source = "NetEase".into();
                                            result.timing = "Unknown".into();
                                        }
                                        1 => result.timing = "Words".into(),
                                        _ => result.source = "AMLL TTML DB".into(),
                                    }
                                    result
                                })
                                .collect();
                            access(hwnd, |state| state.session.begin())
                                .unwrap()
                                .deliver(Completion::Search(Ok(matches)));
                            poll(hwnd);
                            SendMessageW(list, LB_SETCURSEL, 7, 0);
                            SetFocus(list);
                            access(hwnd, |state| {
                                state
                                    .results_scroll
                                    .jump((6.0 * ROW_HEIGHT + 16.0) * preview_scale as f32);
                            });
                            position_results(hwnd);
                            let image = capture("scrolling");
                            assert_eq!(SendMessageW(list, LB_GETTOPINDEX, 0, 0), 6);
                            let viewport = GetDlgItem(hwnd, RESULTS_VIEWPORT);
                            let mut bar = SCROLLBARINFO {
                                cbSize: std::mem::size_of::<SCROLLBARINFO>() as u32,
                                ..Default::default()
                            };
                            assert_ne!(GetScrollBarInfo(viewport, OBJID_VSCROLL, &mut bar), 0);
                            assert!(bar.xyThumbBottom > bar.xyThumbTop);
                            MapWindowPoints(
                                null_mut(),
                                hwnd,
                                (&mut bar.rcScrollBar as *mut RECT).cast(),
                                2,
                            );
                            let rgb = |color: [u8; 3]| [color[2], color[1], color[0], 255];
                            let mut thumb_pixels = 0;
                            let mut track_pixels = 0;
                            // Also reject the clipped native list's upper arrow:
                            // its window starts above the viewport on fractional moves.
                            let clipped_top = bar.rcScrollBar.top - (16.0 * preview_scale) as i32;
                            for y in clipped_top..bar.rcScrollBar.bottom {
                                for x in bar.rcScrollBar.left..bar.rcScrollBar.right {
                                    let pixel = image.get_pixel(x as u32, y as u32).0;
                                    assert!(
                                        !pixel[..3].iter().all(|&value| value > 200),
                                        "{language:?} at {preview_scale}x: no white system pixels anywhere in the scrollbar strip ({x}, {y}) {pixel:?}"
                                    );
                                    if pixel == rgb(crate::theme::BORDER_STRONG)
                                        && (bar.rcScrollBar.top + bar.xyThumbTop
                                            ..bar.rcScrollBar.top + bar.xyThumbBottom)
                                            .contains(&y)
                                    {
                                        thumb_pixels += 1;
                                    }
                                    if pixel == rgb(BACKGROUND) {
                                        track_pixels += 1;
                                    }
                                }
                            }
                            assert!(
                                thumb_pixels > 0,
                                "the full-host preview must show the themed thumb"
                            );
                            assert!(
                                track_pixels > 0,
                                "the full-host preview must show the dark track"
                            );
                        }
                        GetWindowRect(hwnd, &mut offscreen);
                        assert!(
                            offscreen.right < -10000 && offscreen.bottom < -10000,
                            "memory-DC previews must remain outside the visible desktop"
                        );
                    }
                }
            }
        }

        #[test]
        #[ignore = "interactive desktop/public lyric endpoints"]
        fn native_lyric_search_smoke() {
            unsafe {
                assert!(
                    WINDOW.with(Cell::get).is_null(),
                    "Use an isolated test thread"
                );
                assert!(take_selections().is_empty());
                let owner = CreateWindowExW(
                    0,
                    wide("STATIC").as_ptr(),
                    wide("Lyric search smoke owner").as_ptr(),
                    WS_OVERLAPPEDWINDOW,
                    30,
                    30,
                    300,
                    180,
                    null_mut(),
                    null_mut(),
                    GetModuleHandleW(null()),
                    null(),
                );
                assert!(!owner.is_null(), "Could not create isolated owner");
                ShowWindow(owner, SW_SHOWNOACTIVATE);
                let mut windows = TestWindows {
                    owner,
                    searches: Vec::new(),
                };
                let snapshot = MediaSnapshot {
                    key: "test-track".into(),
                    generation: 41,
                    title: "Blinding Lights".into(),
                    artist: "The Weeknd".into(),
                    ..Default::default()
                };
                show(owner, &snapshot);
                let hwnd = WINDOW.with(Cell::get);
                windows.searches.push(hwnd);
                assert!(!hwnd.is_null(), "Search window was not created");
                assert_eq!(input(hwnd, TITLE), "Blinding Lights");
                assert_eq!(input(hwnd, ARTIST), "The Weeknd");
                let style = GetWindowLongW(hwnd, GWL_STYLE) as u32;
                assert_eq!(
                    style & WS_CAPTION,
                    0,
                    "search uses the custom client-area chrome"
                );
                assert_ne!(style & WS_THICKFRAME, 0);
                assert!(!GetDlgItem(hwnd, crate::window_chrome::MAXIMIZE).is_null());
                assert!(!GetDlgItem(hwnd, crate::window_chrome::CLOSE).is_null());
                assert_ne!(IsWindowEnabled(owner), 0, "Search must remain modeless");
                show(owner, &snapshot);
                assert_eq!(
                    WINDOW.with(Cell::get),
                    hwnd,
                    "Repeated show must reuse the window"
                );

                SetFocus(GetDlgItem(hwnd, TITLE));
                assert!(translate_message(&key(GetDlgItem(hwnd, TITLE), 9)));
                assert_eq!(
                    GetFocus(),
                    GetDlgItem(hwnd, ARTIST),
                    "Tab must reach the artist input"
                );
                SendMessageW(GetDlgItem(hwnd, SEARCH), BM_CLICK, 0, 0);
                assert_ne!(IsWindowEnabled(owner), 0);
                assert_eq!(input(hwnd, STATUS), "Searching lyric providers...");
                let list = results(hwnd);
                let searched = pump_until(Duration::from_secs(25), || {
                    SendMessageW(list, LB_GETCOUNT, 0, 0) > 0
                        || input(hwnd, STATUS).starts_with("Lyrics request failed:")
                        || input(hwnd, STATUS).starts_with("No lyrics found.")
                });
                let count = SendMessageW(list, LB_GETCOUNT, 0, 0);
                assert!(
                    searched && count > 0,
                    "Search did not produce candidates: {}",
                    input(hwnd, STATUS)
                );
                let (source, timing) = access(hwnd, |state| {
                    let result = &state.session.results[0];
                    (result.source.clone(), result.timing.clone())
                })
                .unwrap();
                assert!(!source.trim().is_empty());
                assert!(!timing.trim().is_empty());
                let length = SendMessageW(list, LB_GETTEXTLEN, 0, 0);
                assert!(length > 0, "First candidate must have a visible label");
                let mut row = vec![0_u16; length as usize + 1];
                assert_eq!(
                    SendMessageW(list, LB_GETTEXT, 0, row.as_mut_ptr() as isize),
                    length
                );
                let label = String::from_utf16_lossy(&row[..length as usize]);
                assert!(
                    label.contains(&source) && label.contains(&timing),
                    "Missing source/timing in {label}"
                );
                if let Some(dir) = std::env::var_os("HIYODYNA_PREVIEW_DIR") {
                    SetWindowPos(
                        hwnd,
                        HWND_TOPMOST,
                        0,
                        0,
                        0,
                        0,
                        SWP_NOMOVE | SWP_NOSIZE | SWP_NOACTIVATE,
                    );
                    RedrawWindow(
                        hwnd,
                        null(),
                        null_mut(),
                        RDW_INVALIDATE | RDW_ALLCHILDREN | RDW_UPDATENOW,
                    );
                    assert_ne!(IsWindowVisible(hwnd), 0);
                    let painted = Instant::now();
                    assert!(pump_until(Duration::from_secs(1), || painted.elapsed()
                        >= Duration::from_millis(150)));
                    crate::preferences::tests::save_window(
                        hwnd,
                        &std::path::PathBuf::from(dir).join("lyrics-search-results.png"),
                    );
                }

                SendMessageW(list, LB_SETCURSEL, 0, 0);
                assert_ne!(IsWindowEnabled(GetDlgItem(hwnd, APPLY)), 0);
                SendMessageW(GetDlgItem(hwnd, APPLY), BM_CLICK, 0, 0);
                let mut selections = Vec::new();
                let fetched = pump_until(Duration::from_secs(25), || {
                    selections.extend(take_selections());
                    !selections.is_empty()
                        || IsWindow(hwnd) == 0
                        || input(hwnd, STATUS).starts_with("Lyrics request failed:")
                });
                assert!(
                    fetched && selections.len() == 1,
                    "Fetch did not deliver one selection: {}",
                    input(hwnd, STATUS)
                );
                let selection = selections.pop().unwrap();
                assert_eq!(selection.key, "test-track");
                assert_eq!(selection.generation, 41);
                assert!(!selection.lyrics.source.trim().is_empty());
                assert!(
                    selection
                        .lyrics
                        .lines
                        .iter()
                        .any(|line| !line.text().trim().is_empty()),
                    "Selected recording must contain lyric text"
                );
                assert_eq!(IsWindow(hwnd), 0, "Successful selection must close search");
                assert_ne!(IsWindowEnabled(owner), 0);

                show(owner, &snapshot);
                let pending = WINDOW.with(Cell::get);
                windows.searches.push(pending);
                assert!(!pending.is_null());
                SendMessageW(GetDlgItem(pending, SEARCH), BM_CLICK, 0, 0);
                let (mailbox, revision) = access(pending, |state| {
                    let mailbox = state.session.mailbox.clone();
                    let revision = mailbox.revision.load(Ordering::Acquire);
                    (mailbox, revision)
                })
                .unwrap();
                show(owner, &snapshot);
                assert_eq!(WINDOW.with(Cell::get), pending);
                assert!(translate_message(&key(GetDlgItem(pending, TITLE), 27)));
                assert!(
                    pump_until(Duration::from_secs(2), || IsWindow(pending) == 0),
                    "Escape must close the modeless window"
                );
                assert_ne!(
                    mailbox.revision.load(Ordering::Acquire),
                    revision,
                    "Closing must invalidate the pending provider job"
                );
                assert!(
                    pump_until(Duration::from_secs(25), || Arc::strong_count(&mailbox) == 1),
                    "Cancelled provider worker did not finish within its request deadline"
                );
                assert!(
                    mailbox.responses.lock().unwrap().is_empty(),
                    "Closed window accepted a stale response"
                );
                assert!(
                    take_selections().is_empty(),
                    "Closed query must never apply lyrics"
                );
                assert!(WINDOW.with(Cell::get).is_null());
                assert_ne!(IsWindowEnabled(owner), 0);
            }
        }
    }
}

#[cfg(windows)]
pub(crate) use native::{
    MESSAGE, refresh_language, show, take_selections, track_changed, translate_message,
};

#[cfg(test)]
mod tests {
    use super::*;

    fn target(key: &str, generation: u64) -> Target {
        Target {
            key: key.into(),
            generation,
        }
    }

    #[test]
    fn newer_query_ignores_late_search_response() {
        let mut session = Session::<u32, String>::new(target("original", 7));
        let old = session.begin();
        let new = session.begin();
        new.deliver(Completion::Search(Ok(vec![20])));
        old.deliver(Completion::Search(Ok(vec![10])));
        let events = session.poll();
        assert_eq!(session.results, vec![20]);
        assert_eq!(events.len(), 1);
        assert!(!old.current());
        assert!(new.current());
    }

    #[test]
    fn replacement_cancels_search_and_fetch_even_for_same_key() {
        for (key, fetching) in [("same", false), ("same", true), ("different", true)] {
            let mut session = Session::<u32, String>::new(target("same", 7));
            let old = session.begin();
            session.replace(target(key, 8));
            old.deliver(if fetching {
                Completion::Fetch(Ok("wrong track".into()))
            } else {
                Completion::Search(Ok(vec![10]))
            });
            assert!(session.poll().is_empty());
            assert!(session.results.is_empty());
            assert!(!old.current());
        }
    }

    #[test]
    fn closed_window_ignores_worker_delivery() {
        let mut session = Session::<u32, String>::new(target("closed", 3));
        let pending = session.begin();
        session.close();
        pending.deliver(Completion::Fetch(Ok("late".into())));
        assert!(session.poll().is_empty());
        assert!(!pending.current());
    }

    #[test]
    fn newer_query_discards_already_queued_fetch_and_late_error() {
        let mut session = Session::<u32, String>::new(target("original", 7));
        let fetch = session.begin();
        fetch.deliver(Completion::Fetch(Ok("superseded".into())));
        let search = session.begin();
        fetch.deliver(Completion::Fetch(Err("stale error".into())));
        search.deliver(Completion::Search(Ok(vec![42])));
        assert!(matches!(&session.poll()[..], [Event::Results(1)]));
        assert_eq!(session.results, vec![42]);
    }

    #[test]
    fn worker_can_finish_after_session_is_dropped() {
        let mut session = Session::<u32, String>::new(target("original", 7));
        let job = session.begin();
        let (send, receive) = std::sync::mpsc::channel();
        let worker = std::thread::spawn(move || {
            receive.recv().unwrap();
            assert!(!job.current());
            job.deliver(Completion::Fetch(Ok("late".into())));
            assert!(job.mailbox.responses.lock().unwrap().is_empty());
        });
        drop(session);
        send.send(()).unwrap();
        worker.join().unwrap();
    }

    #[test]
    fn successful_fetch_keeps_original_track_identity() {
        let mut session = Session::<u32, String>::new(target("original track", 41));
        session.results = vec![99];
        let (selected, job) = session.select(0).unwrap();
        assert_eq!(selected, 99);
        job.deliver(Completion::Fetch(Ok("chosen lyrics".into())));
        let events = session.poll();
        assert!(matches!(&events[..], [Event::Selected(target, lyrics)]
            if target.key == "original track" && target.generation == 41 && lyrics == "chosen lyrics"));
    }

    #[test]
    fn search_without_media_target_can_show_results_but_cannot_apply() {
        let mut session = Session::<u32, String>::new(target("", 0));
        session.begin().deliver(Completion::Search(Ok(vec![99])));
        assert!(matches!(&session.poll()[..], [Event::Results(1)]));
        assert!(session.select(0).is_none());
    }

    #[test]
    fn network_error_is_distinct_from_empty_results() {
        let mut session = Session::<u32, String>::new(target("track", 1));
        session
            .begin()
            .deliver(Completion::Search(Err("Provider unavailable".into())));
        assert!(
            matches!(&session.poll()[..], [Event::Error(error)] if error == "Provider unavailable")
        );
        session.begin().deliver(Completion::Search(Ok(vec![])));
        assert!(matches!(&session.poll()[..], [Event::Results(0)]));
    }
}
