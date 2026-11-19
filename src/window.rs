use std::mem::size_of;
use std::ptr::{null, null_mut};
use std::sync::atomic::{AtomicBool, AtomicUsize, Ordering};
use std::time::Instant;

use windows_sys::Win32::Foundation::{
    CloseHandle, ERROR_ALREADY_EXISTS, GetLastError, HANDLE, HWND, LPARAM, LRESULT, POINT, SIZE,
    WPARAM,
};
use windows_sys::Win32::Graphics::Gdi::{
    AC_SRC_ALPHA, AC_SRC_OVER, BI_RGB, BITMAPINFO, BITMAPINFOHEADER, BLENDFUNCTION,
    CreateCompatibleDC, CreateDIBSection, DIB_RGB_COLORS, DeleteDC, DeleteObject, GetMonitorInfoW,
    HBITMAP, HDC, HGDIOBJ, MONITOR_DEFAULTTOPRIMARY, MONITORINFO, MonitorFromWindow,
    ScreenToClient, SelectObject,
};
use windows_sys::Win32::Media::{
    TIME_CALLBACK_FUNCTION, TIME_KILL_SYNCHRONOUS, TIME_PERIODIC, timeBeginPeriod, timeEndPeriod,
    timeKillEvent, timeSetEvent,
};
use windows_sys::Win32::System::LibraryLoader::GetModuleHandleW;
use windows_sys::Win32::System::Threading::CreateMutexW;
use windows_sys::Win32::UI::Controls::WM_MOUSELEAVE;
use windows_sys::Win32::UI::HiDpi::{
    DPI_AWARENESS_CONTEXT_PER_MONITOR_AWARE_V2, GetDpiForWindow, SetProcessDpiAwarenessContext,
};
use windows_sys::Win32::UI::Input::KeyboardAndMouse::{
    TME_LEAVE, TRACKMOUSEEVENT, TrackMouseEvent,
};
use windows_sys::Win32::UI::WindowsAndMessaging::*;

use crate::animation::{Animation, ShapeState};
use crate::clock::ClockSnapshot;
use crate::hover::{
    HOVER_GRACE_MILLISECONDS, HOVER_RECHECK_MILLISECONDS, HoverCommand, HoverTracker,
    pointer_inside_shape,
};
use crate::render::{RenderParams, render_frame_with_margin};
use crate::settings::Settings;
use crate::text::{TextLayer, TextParams};
use crate::tray::{TRAY_CALLBACK_MESSAGE, Tray, TrayCommand};

const CANVAS_WIDTH: f32 = 512.0;
const CANVAS_HEIGHT: f32 = 224.0;
const TIMER_ID_HOVER_GRACE: usize = 2;
const TIMER_ID_HOVER_RECHECK: usize = 3;
const TIMER_ID_CLOCK: usize = 4;
const ANIMATION_TIMER_MILLISECONDS: u32 = 16;
const ANIMATION_TIMER_RESOLUTION_MILLISECONDS: u32 = 1;
const CLOCK_TIMER_MILLISECONDS: u32 = 1000;
const TOP_MARGIN: f32 = 0.0;
const WM_APP_REFRESH_DISPLAY: u32 = WM_APP + 1;
const WM_APP_TRAY_MENU: u32 = WM_APP + 3;
const WM_APP_ANIMATION_FRAME: u32 = WM_APP + 4;

static ANIMATION_FRAME_PENDING: AtomicBool = AtomicBool::new(false);
static NEXT_ANIMATION_TIMER_GENERATION: AtomicUsize = AtomicUsize::new(0x1000);
static ACTIVE_ANIMATION_TIMER_GENERATION: AtomicUsize = AtomicUsize::new(0);

struct InstanceGuard(HANDLE);

impl Drop for InstanceGuard {
    fn drop(&mut self) {
        // SAFETY: The handle came from CreateMutexW and is owned by this guard.
        unsafe { CloseHandle(self.0) };
    }
}

pub(crate) fn run() -> Result<(), String> {
    // SAFETY: All HWND/GDI handles remain owned by this thread until the loop exits.
    unsafe {
        SetProcessDpiAwarenessContext(DPI_AWARENESS_CONTEXT_PER_MONITOR_AWARE_V2);
        let mutex_name = wide("Local\\HiyoDynaInstance");
        let instance_mutex = CreateMutexW(null(), 0, mutex_name.as_ptr());
        if instance_mutex.is_null() {
            return Err(last_error("CreateMutexW"));
        }
        if GetLastError() == ERROR_ALREADY_EXISTS {
            CloseHandle(instance_mutex);
            return Ok(());
        }
        let _instance_guard = InstanceGuard(instance_mutex);

        let instance = GetModuleHandleW(null());
        if instance.is_null() {
            return Err(last_error("GetModuleHandleW"));
        }

        let class_name = wide("HiyoDyna");
        let title = wide("HiyoDyna");
        let class = WNDCLASSW {
            lpfnWndProc: Some(window_proc),
            hInstance: instance,
            lpszClassName: class_name.as_ptr(),
            hCursor: LoadCursorW(null_mut(), IDC_ARROW),
            ..Default::default()
        };
        if RegisterClassW(&class) == 0 {
            return Err(last_error("RegisterClassW"));
        }

        let hwnd = CreateWindowExW(
            WS_EX_LAYERED | WS_EX_TOOLWINDOW | WS_EX_NOACTIVATE | WS_EX_TOPMOST,
            class_name.as_ptr(),
            title.as_ptr(),
            WS_POPUP,
            0,
            0,
            CANVAS_WIDTH as i32,
            CANVAS_HEIGHT as i32,
            null_mut(),
            null_mut(),
            instance,
            null(),
        );
        if hwnd.is_null() {
            let error = last_error("CreateWindowExW");
            UnregisterClassW(class_name.as_ptr(), instance);
            return Err(error);
        }

        let result = run_loop(hwnd);
        if IsWindow(hwnd) != 0 {
            DestroyWindow(hwnd);
        }
        UnregisterClassW(class_name.as_ptr(), instance);
        let restart_requested = result?;
        drop(_instance_guard);
        if restart_requested && let Ok(executable) = std::env::current_exe() {
            let _ = std::process::Command::new(executable).spawn();
        }
        Ok(())
    }
}

fn run_loop(hwnd: HWND) -> Result<bool, String> {
    let mut scale = window_scale(hwnd);
    let mut settings = Settings::load();
    let mut restart_requested = false;
    let mut clock = ClockSnapshot::now();
    let mut surface = Surface::new(canvas_size(scale), scale)?;
    let mut animation = Animation::new_with_tuning(settings.tuning());
    let mut hover = HoverTracker::new();
    let mut position = primary_position(surface.size, settings.top_gap)?;
    surface.draw(animation.current, scale, &settings, &clock, animation.hover);
    surface.present(hwnd, position)?;

    // SAFETY: hwnd is live, owned by this thread, and the surface has been initialized.
    unsafe {
        if SetWindowPos(
            hwnd,
            HWND_TOPMOST,
            position.x,
            position.y,
            surface.size.0 as i32,
            surface.size.1 as i32,
            SWP_NOACTIVATE | SWP_SHOWWINDOW,
        ) == 0
        {
            return Err(last_error("SetWindowPos"));
        }
        if SetTimer(hwnd, TIMER_ID_CLOCK, CLOCK_TIMER_MILLISECONDS, None) == 0 {
            return Err(last_error("SetTimer"));
        }
    }

    let mut last_frame = Instant::now();
    let mut animation_timer = AnimationTimer::new(hwnd);
    let trace_frames = std::env::var_os("HIYODYNA_TRACE_FRAMES").is_some();
    let mut trace_first_frame = None;
    let mut trace_last_frame = None;
    let mut trace_frame_count = 0_u32;
    let mut trace_elapsed_ms = 0.0_f64;
    let mut trace_max_dt_ms = 0.0_f64;
    let mut trace_total_draw_ms = 0.0_f64;
    let mut trace_max_draw_ms = 0.0_f64;
    let mut trace_total_present_ms = 0.0_f64;
    let mut trace_max_present_ms = 0.0_f64;
    let mut tray = match Tray::create(hwnd, settings.accent) {
        Ok(tray) => Some(tray),
        Err(error) => {
            eprintln!("HiyoDyna tray unavailable: {error}");
            None
        }
    };
    let mut message = MSG::default();
    loop {
        // At rest the message loop sleeps between input, display and clock messages.
        let received = unsafe { GetMessageW(&mut message, null_mut(), 0, 0) };
        if received == -1 {
            return Err(last_error("GetMessageW"));
        }
        if received == 0 {
            return Ok(restart_requested);
        }

        match message.message {
            WM_MOUSEMOVE if message.hwnd == hwnd => {
                track_mouse_leave(hwnd);
                let (x, y) = mouse_position(message.lParam);
                let inside = pointer_inside_current_shape(&animation, scale, surface.size, x, y);
                let command = hover.pointer_moved(inside);
                set_hover(
                    &mut animation,
                    &mut animation_timer,
                    &mut last_frame,
                    inside,
                )?;
                apply_hover_command(
                    hwnd,
                    command,
                    &mut animation,
                    &mut animation_timer,
                    &mut last_frame,
                )?;
            }
            WM_MOUSELEAVE if message.hwnd == hwnd => {
                let command = hover.pointer_moved(false);
                set_hover(&mut animation, &mut animation_timer, &mut last_frame, false)?;
                apply_hover_command(
                    hwnd,
                    command,
                    &mut animation,
                    &mut animation_timer,
                    &mut last_frame,
                )?;
            }
            WM_RBUTTONUP if message.hwnd == hwnd => {
                show_settings_menu(
                    hwnd,
                    tray.as_mut(),
                    &mut settings,
                    &mut animation,
                    &mut surface,
                    scale,
                    &mut position,
                    &clock,
                    &mut restart_requested,
                )?;
            }
            message_id
                if message.hwnd == hwnd
                    && animation_timer.is_wakeup(message_id, message.wParam) =>
            {
                // Keep the wakeup pending while drawing so the timer can't queue a stale frame.
                let posted_frame = message_id == WM_APP_ANIMATION_FRAME;
                let now = Instant::now();
                if animation_timer.is_active()
                    && let Some(elapsed) = take_animation_frame(now, last_frame)
                {
                    if trace_frames {
                        trace_first_frame.get_or_insert(now);
                        trace_last_frame = Some(now);
                        trace_elapsed_ms += f64::from(elapsed) * 1000.0;
                        trace_max_dt_ms = trace_max_dt_ms.max(f64::from(elapsed) * 1000.0);
                        trace_frame_count += 1;
                    }
                    last_frame = now;
                    let moving = animation.advance(elapsed);
                    let draw_start = Instant::now();
                    surface.draw(animation.current, scale, &settings, &clock, animation.hover);
                    let present_start = Instant::now();
                    surface.present(hwnd, position)?;
                    if trace_frames {
                        trace_max_draw_ms = trace_max_draw_ms
                            .max(present_start.duration_since(draw_start).as_secs_f64() * 1000.0);
                        trace_total_draw_ms +=
                            present_start.duration_since(draw_start).as_secs_f64() * 1000.0;
                        trace_max_present_ms = trace_max_present_ms.max(
                            Instant::now().duration_since(present_start).as_secs_f64() * 1000.0,
                        );
                        trace_total_present_ms +=
                            Instant::now().duration_since(present_start).as_secs_f64() * 1000.0;
                    }
                    if !moving {
                        animation_timer.stop();
                        if trace_frames && trace_frame_count > 0 {
                            let wall_duration = trace_first_frame
                                .zip(trace_last_frame)
                                .map(|(first, last)| last.saturating_duration_since(first))
                                .unwrap_or_default();
                            eprintln!(
                                "animation elapsed_fps={:.1} wall_fps={} frames={} avg_dt_ms={:.2} max_dt_ms={:.2} avg_draw_ms={:.2} max_draw_ms={:.2} avg_present_ms={:.2} max_present_ms={:.2} frame_span_ms={:.2}",
                                1000.0 / (trace_elapsed_ms / f64::from(trace_frame_count)),
                                format_frames_per_second(trace_frame_count, wall_duration),
                                trace_frame_count,
                                trace_elapsed_ms / f64::from(trace_frame_count),
                                trace_max_dt_ms,
                                trace_total_draw_ms / f64::from(trace_frame_count),
                                trace_max_draw_ms,
                                trace_total_present_ms / f64::from(trace_frame_count),
                                trace_max_present_ms,
                                wall_duration.as_secs_f64() * 1000.0,
                            );
                            trace_first_frame = None;
                            trace_last_frame = None;
                            trace_frame_count = 0;
                            trace_elapsed_ms = 0.0;
                            trace_max_dt_ms = 0.0;
                            trace_total_draw_ms = 0.0;
                            trace_max_draw_ms = 0.0;
                            trace_total_present_ms = 0.0;
                            trace_max_present_ms = 0.0;
                        }
                    }
                }
                if posted_frame {
                    ANIMATION_FRAME_PENDING.store(false, Ordering::Release);
                }
            }
            WM_TIMER if message.hwnd == hwnd && message.wParam == TIMER_ID_HOVER_RECHECK => {
                if let Some((x, y)) = pointer_position(hwnd) {
                    let inside =
                        pointer_inside_current_shape(&animation, scale, surface.size, x, y);
                    let command = hover.pointer_moved(inside);
                    set_hover(
                        &mut animation,
                        &mut animation_timer,
                        &mut last_frame,
                        inside,
                    )?;
                    apply_hover_command(
                        hwnd,
                        command,
                        &mut animation,
                        &mut animation_timer,
                        &mut last_frame,
                    )?;
                }
            }
            WM_TIMER if message.hwnd == hwnd && message.wParam == TIMER_ID_HOVER_GRACE => {
                unsafe { KillTimer(hwnd, TIMER_ID_HOVER_GRACE) };
                let inside = pointer_position(hwnd)
                    .map(|(x, y)| {
                        pointer_inside_current_shape(&animation, scale, surface.size, x, y)
                    })
                    .unwrap_or(false);
                let command = hover.grace_expired(inside);
                set_hover(
                    &mut animation,
                    &mut animation_timer,
                    &mut last_frame,
                    inside,
                )?;
                apply_hover_command(
                    hwnd,
                    command,
                    &mut animation,
                    &mut animation_timer,
                    &mut last_frame,
                )?;
            }
            WM_TIMER if message.hwnd == hwnd && message.wParam == TIMER_ID_CLOCK => {
                if update_clock(
                    &mut clock,
                    ClockSnapshot::now(),
                    animation_timer.is_active(),
                ) {
                    surface.draw(animation.current, scale, &settings, &clock, animation.hover);
                    surface.present(hwnd, position)?;
                }
            }
            WM_APP_REFRESH_DISPLAY if message.hwnd == hwnd => {
                scale = window_scale(hwnd);
                surface = Surface::new(canvas_size(scale), scale)?;
                position = primary_position(surface.size, settings.top_gap)?;
                surface.draw(animation.current, scale, &settings, &clock, animation.hover);
                surface.present(hwnd, position)?;
            }
            WM_APP_TRAY_MENU if message.hwnd == hwnd => {
                let event = (message.lParam as u32) & 0xFFFF;
                if event == WM_RBUTTONUP || event == WM_CONTEXTMENU {
                    show_settings_menu(
                        hwnd,
                        tray.as_mut(),
                        &mut settings,
                        &mut animation,
                        &mut surface,
                        scale,
                        &mut position,
                        &clock,
                        &mut restart_requested,
                    )?;
                }
            }
            _ => unsafe {
                TranslateMessage(&message);
                DispatchMessageW(&message);
            },
        }
    }
}

unsafe extern "system" fn window_proc(
    hwnd: HWND,
    message: u32,
    wparam: WPARAM,
    lparam: LPARAM,
) -> LRESULT {
    // No Rust state or allocation crosses this callback, including synchronous Win32 reentry.
    match message {
        message if refresh_message_for(message).is_some() => {
            // DPI/display notifications are sent directly to WndProc, not queued for GetMessageW.
            unsafe { PostMessageW(hwnd, WM_APP_REFRESH_DISPLAY, 0, 0) };
            0
        }
        TRAY_CALLBACK_MESSAGE => {
            unsafe { PostMessageW(hwnd, WM_APP_TRAY_MENU, wparam, lparam) };
            0
        }
        WM_MOUSEACTIVATE => MA_NOACTIVATE as LRESULT,
        WM_ERASEBKGND => 1,
        WM_CLOSE => {
            unsafe { DestroyWindow(hwnd) };
            0
        }
        WM_DESTROY => {
            unsafe { PostQuitMessage(0) };
            0
        }
        _ => unsafe { DefWindowProcW(hwnd, message, wparam, lparam) },
    }
}

struct Surface {
    dc: HDC,
    bitmap: HBITMAP,
    previous_bitmap: HGDIOBJ,
    bits: *mut u8,
    size: (u32, u32),
    text: TextLayer,
}

impl Surface {
    fn new(size: (u32, u32), scale: f32) -> Result<Self, String> {
        let text = TextLayer::new(scale)?;
        // SAFETY: The DIB is top-down, 32-bit BGRA. Handles are cleaned up on every exit path.
        unsafe {
            let dc = CreateCompatibleDC(null_mut());
            if dc.is_null() {
                return Err(last_error("CreateCompatibleDC"));
            }
            let info = BITMAPINFO {
                bmiHeader: BITMAPINFOHEADER {
                    biSize: size_of::<BITMAPINFOHEADER>() as u32,
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
            let bitmap = CreateDIBSection(dc, &info, DIB_RGB_COLORS, &mut bits, null_mut(), 0);
            if bitmap.is_null() || bits.is_null() {
                let error = last_error("CreateDIBSection");
                if !bitmap.is_null() {
                    DeleteObject(bitmap);
                }
                DeleteDC(dc);
                return Err(error);
            }
            let previous_bitmap = SelectObject(dc, bitmap);
            if previous_bitmap.is_null() || previous_bitmap as isize == -1 {
                let error = last_error("SelectObject");
                DeleteObject(bitmap);
                DeleteDC(dc);
                return Err(error);
            }
            Ok(Self {
                dc,
                bitmap,
                previous_bitmap,
                bits: bits.cast(),
                size,
                text,
            })
        }
    }

    fn draw(
        &mut self,
        state: ShapeState,
        scale: f32,
        settings: &Settings,
        clock: &ClockSnapshot,
        hover: f32,
    ) {
        let length = self.size.0 as usize * self.size.1 as usize * 4;
        // SAFETY: The DIB allocated exactly size.0 * size.1 * 4 writable bytes and outlives self.
        let pixels = unsafe { std::slice::from_raw_parts_mut(self.bits, length) };
        let accent = settings.accent_bgra();
        render_frame_with_margin(
            pixels,
            self.size,
            RenderParams {
                state: scale_shape(state, scale),
                top_margin: TOP_MARGIN * scale,
                scale,
                shadow_opacity: settings.shadow_opacity(),
                hover,
                accent,
            },
        );
        self.text.update(clock, settings.clock);
        self.text.draw(
            pixels,
            TextParams {
                size: self.size,
                state,
                top_margin: TOP_MARGIN * scale,
                scale,
                accent,
                show_seconds: settings.show_seconds,
                hover,
            },
        );
    }

    fn present(&self, hwnd: HWND, position: POINT) -> Result<(), String> {
        let size = SIZE {
            cx: self.size.0 as i32,
            cy: self.size.1 as i32,
        };
        let origin = POINT { x: 0, y: 0 };
        let blend = BLENDFUNCTION {
            BlendOp: AC_SRC_OVER as u8,
            BlendFlags: 0,
            SourceConstantAlpha: 255,
            AlphaFormat: AC_SRC_ALPHA as u8,
        };
        // SAFETY: The source DC contains the selected, initialized premultiplied BGRA DIB.
        if unsafe {
            UpdateLayeredWindow(
                hwnd,
                null_mut(),
                &position,
                &size,
                self.dc,
                &origin,
                0,
                &blend,
                ULW_ALPHA,
            )
        } == 0
        {
            return Err(last_error("UpdateLayeredWindow"));
        }
        Ok(())
    }
}

impl Drop for Surface {
    fn drop(&mut self) {
        // SAFETY: Restore the original bitmap before deleting the owned bitmap and memory DC.
        unsafe {
            SelectObject(self.dc, self.previous_bitmap);
            DeleteObject(self.bitmap);
            DeleteDC(self.dc);
        }
    }
}

fn primary_position(size: (u32, u32), top_gap: f32) -> Result<POINT, String> {
    let mut monitor = MONITORINFO {
        cbSize: size_of::<MONITORINFO>() as u32,
        ..Default::default()
    };
    // SAFETY: A null HWND with DEFAULTTOPRIMARY selects the primary display.
    unsafe {
        let handle = MonitorFromWindow(null_mut(), MONITOR_DEFAULTTOPRIMARY);
        if GetMonitorInfoW(handle, &mut monitor) == 0 {
            return Err(last_error("GetMonitorInfoW"));
        }
    }
    Ok(POINT {
        x: centered_x(
            monitor.rcMonitor.left,
            monitor.rcMonitor.right - monitor.rcMonitor.left,
            size.0 as i32,
        ),
        y: monitor.rcMonitor.top + top_gap.round() as i32,
    })
}

fn window_scale(hwnd: HWND) -> f32 {
    let dpi = unsafe { GetDpiForWindow(hwnd) };
    dpi.max(96) as f32 / 96.0
}

fn canvas_size(scale: f32) -> (u32, u32) {
    (
        (CANVAS_WIDTH * scale).round() as u32,
        (CANVAS_HEIGHT * scale).round() as u32,
    )
}

fn centered_x(left: i32, monitor_width: i32, window_width: i32) -> i32 {
    left + (monitor_width - window_width) / 2
}

fn refresh_message_for(message: u32) -> Option<u32> {
    matches!(message, WM_DPICHANGED | WM_DISPLAYCHANGE).then_some(WM_APP_REFRESH_DISPLAY)
}

fn scale_shape(state: ShapeState, scale: f32) -> ShapeState {
    ShapeState {
        width: state.width * scale,
        height: state.height * scale,
        radius: state.radius * scale,
    }
}

fn pointer_inside_current_shape(
    animation: &Animation,
    scale: f32,
    size: (u32, u32),
    x: f32,
    y: f32,
) -> bool {
    pointer_inside_shape(
        size,
        scale_shape(animation.current, scale),
        TOP_MARGIN * scale,
        x,
        y,
    )
}

fn mouse_position(lparam: LPARAM) -> (f32, f32) {
    let packed = lparam as u32;
    let x = (packed as u16 as i16) as f32;
    let y = ((packed >> 16) as u16 as i16) as f32;
    (x, y)
}

fn pointer_position(hwnd: HWND) -> Option<(f32, f32)> {
    let mut point = POINT { x: 0, y: 0 };
    // SAFETY: hwnd is live and point is a valid writable POINT.
    unsafe {
        if GetCursorPos(&mut point) == 0 {
            return None;
        }
        if ScreenToClient(hwnd, &mut point) == 0 {
            return None;
        }
    }
    Some((point.x as f32, point.y as f32))
}

fn track_mouse_leave(hwnd: HWND) {
    let mut event = TRACKMOUSEEVENT {
        cbSize: size_of::<TRACKMOUSEEVENT>() as u32,
        dwFlags: TME_LEAVE,
        hwndTrack: hwnd,
        dwHoverTime: 0,
    };
    // SAFETY: event is fully initialized and hwnd is live.
    unsafe {
        TrackMouseEvent(&mut event);
    }
}

fn set_hover(
    animation: &mut Animation,
    animation_timer: &mut AnimationTimer,
    last_frame: &mut Instant,
    inside: bool,
) -> Result<(), String> {
    if !animation.set_hover(inside) {
        return Ok(());
    }
    if animation_timer.is_active() {
        animation.advance(last_frame.elapsed().as_secs_f32());
    }
    *last_frame = Instant::now();
    animation_timer.start()
}

fn apply_hover_command(
    hwnd: HWND,
    command: HoverCommand,
    animation: &mut Animation,
    animation_timer: &mut AnimationTimer,
    last_frame: &mut Instant,
) -> Result<(), String> {
    match command {
        HoverCommand::Expand => {
            if animation_timer.is_active() {
                animation.advance(last_frame.elapsed().as_secs_f32());
            }
            animation.expand();
            *last_frame = Instant::now();
            animation_timer.start()?;
            unsafe {
                KillTimer(hwnd, TIMER_ID_HOVER_GRACE);
            }
            if unsafe {
                SetTimer(
                    hwnd,
                    TIMER_ID_HOVER_RECHECK,
                    HOVER_RECHECK_MILLISECONDS,
                    None,
                )
            } == 0
            {
                return Err(last_error("SetTimer"));
            }
        }
        HoverCommand::Collapse => {
            if animation_timer.is_active() {
                animation.advance(last_frame.elapsed().as_secs_f32());
            }
            animation.collapse();
            *last_frame = Instant::now();
            animation_timer.start()?;
            unsafe {
                KillTimer(hwnd, TIMER_ID_HOVER_GRACE);
                KillTimer(hwnd, TIMER_ID_HOVER_RECHECK);
            }
        }
        HoverCommand::ArmGrace => {
            if unsafe { SetTimer(hwnd, TIMER_ID_HOVER_GRACE, HOVER_GRACE_MILLISECONDS, None) } == 0
            {
                return Err(last_error("SetTimer"));
            }
        }
        HoverCommand::CancelGrace => unsafe {
            KillTimer(hwnd, TIMER_ID_HOVER_GRACE);
        },
        HoverCommand::None => {}
    }
    Ok(())
}

fn queue_animation_frame(pending: &AtomicBool) -> bool {
    !pending.swap(true, Ordering::AcqRel)
}

unsafe extern "system" fn animation_timer_callback(
    _timer_id: u32,
    _message: u32,
    user: usize,
    _dw1: usize,
    _dw2: usize,
) {
    let generation = ACTIVE_ANIMATION_TIMER_GENERATION.load(Ordering::Acquire);
    if generation == 0 {
        return;
    }
    if queue_animation_frame(&ANIMATION_FRAME_PENDING) {
        let hwnd = user as HWND;
        if unsafe { PostMessageW(hwnd, WM_APP_ANIMATION_FRAME, generation, 0) } == 0 {
            ANIMATION_FRAME_PENDING.store(false, Ordering::Release);
        }
    }
}

struct AnimationTimer {
    hwnd: HWND,
    generation: usize,
    source: Option<AnimationTimerSource>,
}

#[derive(Clone, Copy)]
enum AnimationTimerSource {
    Multimedia(u32),
    Window,
}

impl AnimationTimer {
    fn new(hwnd: HWND) -> Self {
        Self {
            hwnd,
            generation: 0,
            source: None,
        }
    }

    fn is_active(&self) -> bool {
        self.source.is_some()
    }

    fn is_wakeup(&self, message_id: u32, wparam: WPARAM) -> bool {
        is_animation_wakeup(self.source, self.generation, message_id, wparam)
    }

    fn start(&mut self) -> Result<(), String> {
        if self.is_active() {
            return Ok(());
        }

        let generation = NEXT_ANIMATION_TIMER_GENERATION
            .fetch_add(1, Ordering::Relaxed)
            .max(1);
        self.generation = generation;
        if unsafe { timeBeginPeriod(ANIMATION_TIMER_RESOLUTION_MILLISECONDS) } == 0 {
            ACTIVE_ANIMATION_TIMER_GENERATION.store(generation, Ordering::Release);
            let timer_id = unsafe {
                timeSetEvent(
                    ANIMATION_TIMER_MILLISECONDS,
                    ANIMATION_TIMER_RESOLUTION_MILLISECONDS,
                    Some(animation_timer_callback),
                    self.hwnd as usize,
                    TIME_PERIODIC | TIME_CALLBACK_FUNCTION | TIME_KILL_SYNCHRONOUS,
                )
            };
            if timer_id != 0 {
                self.source = Some(AnimationTimerSource::Multimedia(timer_id));
                return Ok(());
            }
            ACTIVE_ANIMATION_TIMER_GENERATION.store(0, Ordering::Release);
            unsafe { timeEndPeriod(ANIMATION_TIMER_RESOLUTION_MILLISECONDS) };
        }

        eprintln!("HiyoDyna high-resolution timer unavailable; using WM_TIMER fallback");
        if unsafe {
            SetTimer(
                self.hwnd,
                self.generation,
                ANIMATION_TIMER_MILLISECONDS,
                None,
            )
        } == 0
        {
            self.generation = 0;
            return Err(last_error("SetTimer"));
        }
        self.source = Some(AnimationTimerSource::Window);
        Ok(())
    }

    fn stop(&mut self) {
        let Some(source) = self.source.take() else {
            return;
        };

        match source {
            AnimationTimerSource::Multimedia(timer_id) => unsafe {
                timeKillEvent(timer_id);
                timeEndPeriod(ANIMATION_TIMER_RESOLUTION_MILLISECONDS);
            },
            AnimationTimerSource::Window => unsafe {
                KillTimer(self.hwnd, self.generation);
            },
        }
        ACTIVE_ANIMATION_TIMER_GENERATION.store(0, Ordering::Release);
        ANIMATION_FRAME_PENDING.store(false, Ordering::Release);
        self.generation = 0;
    }
}

impl Drop for AnimationTimer {
    fn drop(&mut self) {
        self.stop();
    }
}

fn take_animation_frame(now: Instant, last_frame: Instant) -> Option<f32> {
    let elapsed = now.saturating_duration_since(last_frame);
    (!elapsed.is_zero()).then_some(elapsed.as_secs_f32())
}

fn frames_per_second(frame_count: u32, elapsed: std::time::Duration) -> Option<f64> {
    let intervals = frame_count.checked_sub(1)?;
    let seconds = elapsed.as_secs_f64();
    (seconds > 0.0).then_some(f64::from(intervals) / seconds)
}

fn format_frames_per_second(frame_count: u32, elapsed: std::time::Duration) -> String {
    frames_per_second(frame_count, elapsed)
        .map(|fps| format!("{fps:.1}"))
        .unwrap_or_else(|| "n/a".to_owned())
}

fn is_animation_wakeup(
    source: Option<AnimationTimerSource>,
    generation: usize,
    message_id: u32,
    wparam: WPARAM,
) -> bool {
    match source {
        Some(AnimationTimerSource::Multimedia(_)) => {
            message_id == WM_APP_ANIMATION_FRAME && wparam == generation
        }
        Some(AnimationTimerSource::Window) => message_id == WM_TIMER && wparam == generation,
        None => false,
    }
}

fn update_clock(clock: &mut ClockSnapshot, updated: ClockSnapshot, ticking: bool) -> bool {
    if *clock == updated {
        return false;
    }
    *clock = updated;
    !ticking
}

#[allow(clippy::too_many_arguments)]
fn show_settings_menu(
    hwnd: HWND,
    tray: Option<&mut Tray>,
    settings: &mut Settings,
    animation: &mut Animation,
    surface: &mut Surface,
    scale: f32,
    position: &mut POINT,
    clock: &ClockSnapshot,
    restart_requested: &mut bool,
) -> Result<(), String> {
    let Some(tray) = tray else {
        return Ok(());
    };
    let Some(command) = tray.show_menu(settings) else {
        return Ok(());
    };
    apply_tray_command(
        hwnd,
        command,
        settings,
        animation,
        surface,
        Some(tray),
        scale,
        position,
        clock,
        restart_requested,
    )
}

#[allow(clippy::too_many_arguments)]
fn apply_tray_command(
    hwnd: HWND,
    command: TrayCommand,
    settings: &mut Settings,
    animation: &mut Animation,
    surface: &mut Surface,
    mut tray: Option<&mut Tray>,
    scale: f32,
    position: &mut POINT,
    clock: &ClockSnapshot,
    restart_requested: &mut bool,
) -> Result<(), String> {
    match command {
        TrayCommand::Motion(motion) => settings.motion = motion,
        TrayCommand::Bounce(bounce) => settings.bounce = bounce,
        TrayCommand::Shadow(enabled) => settings.shadow = enabled,
        TrayCommand::TopGap(gap) => settings.top_gap = gap,
        TrayCommand::Clock(format) => settings.clock = format,
        TrayCommand::Accent(accent) => settings.accent = accent,
        TrayCommand::ShowSeconds(enabled) => settings.show_seconds = enabled,
        TrayCommand::ToggleAutostart => {
            if let Err(error) = crate::autostart::toggle() {
                eprintln!("HiyoDyna autostart: {error}");
            }
            return Ok(());
        }
        TrayCommand::Restart => {
            *restart_requested = true;
            unsafe { PostMessageW(hwnd, WM_CLOSE, 0, 0) };
            return Ok(());
        }
        TrayCommand::OpenSettingsFolder => {
            if let Some(path) = crate::settings::config_path()
                && let Some(directory) = path.parent()
            {
                let _ = std::fs::create_dir_all(directory);
                let _ = std::process::Command::new("explorer")
                    .arg(directory)
                    .spawn();
            }
            return Ok(());
        }
        TrayCommand::About => {
            let settings_path = crate::settings::config_path()
                .map(|path| path.display().to_string())
                .unwrap_or_else(|| "unavailable".to_string());
            let log_path = std::env::var_os("LOCALAPPDATA")
                .map(|base| {
                    std::path::PathBuf::from(base)
                        .join("HiyoDyna")
                        .join("auto-update.log")
                        .display()
                        .to_string()
                })
                .unwrap_or_else(|| "unavailable".to_string());
            show_about(
                hwnd,
                &format!(
                    "HiyoDyna {}\n\nSettings: {settings_path}\nUpdate log: {log_path}",
                    env!("CARGO_PKG_VERSION")
                ),
            );
            return Ok(());
        }
        TrayCommand::Reset => *settings = Settings::default(),
        TrayCommand::Exit => {
            unsafe { PostMessageW(hwnd, WM_CLOSE, 0, 0) };
            return Ok(());
        }
    }

    if matches!(command, TrayCommand::Accent(_) | TrayCommand::Reset)
        && let Some(tray) = tray.as_mut()
    {
        tray.set_accent(settings.accent);
    }
    animation.set_tuning(settings.tuning());
    *position = primary_position(surface.size, settings.top_gap)?;
    if unsafe {
        SetWindowPos(
            hwnd,
            HWND_TOPMOST,
            position.x,
            position.y,
            surface.size.0 as i32,
            surface.size.1 as i32,
            SWP_NOACTIVATE | SWP_SHOWWINDOW,
        )
    } == 0
    {
        return Err(last_error("SetWindowPos"));
    }
    surface.draw(animation.current, scale, settings, clock, animation.hover);
    surface.present(hwnd, *position)?;
    settings.save();
    Ok(())
}

fn show_about(hwnd: HWND, text: &str) {
    let message: Vec<u16> = text.encode_utf16().chain(Some(0)).collect();
    let title = wide("HiyoDyna");
    // SAFETY: hwnd is live and both wide strings are NUL-terminated.
    unsafe {
        MessageBoxW(
            hwnd,
            message.as_ptr(),
            title.as_ptr(),
            MB_OK | MB_ICONINFORMATION,
        );
    }
}

fn last_error(operation: &str) -> String {
    format!("{operation} failed (Windows error {})", unsafe {
        GetLastError()
    })
}

fn wide(text: &str) -> Vec<u16> {
    text.encode_utf16().chain(Some(0)).collect()
}

#[cfg(test)]
mod tests {
    use super::{WM_APP_REFRESH_DISPLAY, centered_x, refresh_message_for, scale_shape};
    use crate::animation::COMPACT;
    use windows_sys::Win32::UI::WindowsAndMessaging::{WM_DISPLAYCHANGE, WM_DPICHANGED, WM_TIMER};

    #[test]
    fn centers_window_inside_primary_monitor_bounds() {
        assert_eq!(centered_x(0, 1920, 560), 680);
        assert_eq!(centered_x(-1920, 1920, 560), -1240);
    }

    #[test]
    fn scales_geometry() {
        let scaled = scale_shape(COMPACT, 1.5);

        assert_eq!(scaled.width, 276.0);
        assert_eq!(scaled.height, 81.0);
        assert_eq!(scaled.radius, 40.5);
    }

    #[test]
    fn routes_sent_dpi_and_display_changes_to_the_message_loop() {
        assert_eq!(
            refresh_message_for(WM_DPICHANGED),
            Some(WM_APP_REFRESH_DISPLAY)
        );
        assert_eq!(
            refresh_message_for(WM_DISPLAYCHANGE),
            Some(WM_APP_REFRESH_DISPLAY)
        );
        assert_eq!(refresh_message_for(WM_TIMER), None);
    }

    #[test]
    fn mouse_position_decodes_client_coordinates() {
        let lparam = ((32_i32 << 16) | 16_i32) as isize;
        assert_eq!(super::mouse_position(lparam), (16.0, 32.0));

        let negative = (((-32_i32) << 16) | (-16_i32 & 0xFFFF)) as isize;
        assert_eq!(super::mouse_position(negative), (-16.0, -32.0));
    }

    #[test]
    fn overshoot_bounds_fit_inside_the_canvas() {
        let max_width = crate::animation::EXPANDED.width * crate::animation::OVERSHOOT_MAX;
        let max_height = crate::animation::EXPANDED.height * crate::animation::OVERSHOOT_MAX;
        let shadow = crate::render::SHADOW_EXTENT;

        assert!(max_width + 2.0 * shadow <= super::CANVAS_WIDTH);
        assert!(max_height + 2.0 * shadow <= super::CANVAS_HEIGHT);
    }

    #[test]
    fn animation_timer_delivers_every_sixteen_millisecond_tick() {
        use std::time::{Duration, Instant};

        let start = Instant::now();
        assert_eq!(super::ANIMATION_TIMER_MILLISECONDS, 16);
        let mut last_frame = start;
        let mut frames = 0;
        let mut total_elapsed = 0.0_f32;
        for ms in (16..=1000).step_by(16) {
            let now = start + Duration::from_millis(ms);
            let elapsed = super::take_animation_frame(now, last_frame)
                .expect("each timer tick must produce a frame");
            frames += 1;
            total_elapsed += elapsed;
            last_frame = now;
        }

        assert_eq!(frames, 62);
        assert!((0.98..=1.0).contains(&total_elapsed));
    }

    #[test]
    fn coarse_windows_timer_ticks_are_not_filtered_by_a_second_frame_gate() {
        use std::time::{Duration, Instant};

        let start = Instant::now();
        let mut last_frame = start;
        let mut frames = Vec::new();
        for tick_index in 1..=64 {
            let now = start + Duration::from_nanos(15_625_000 * tick_index);
            let elapsed = super::take_animation_frame(now, last_frame)
                .expect("coarse timer tick must not be discarded by a frame gate");
            frames.push(elapsed);
            last_frame = now;
        }

        assert_eq!(frames.len(), 64);
        assert!(
            frames
                .iter()
                .all(|elapsed| (0.015..=0.016).contains(elapsed))
        );
    }

    #[test]
    fn late_and_stalled_ticks_keep_measured_elapsed_time() {
        use std::time::{Duration, Instant};

        let start = Instant::now();
        let late = start + Duration::from_millis(48);
        assert_eq!(super::take_animation_frame(late, start), Some(0.048));

        let resumed = late + Duration::from_millis(250);
        assert_eq!(
            super::take_animation_frame(resumed, late),
            Some(0.250),
            "a stall advances from actual elapsed time rather than replaying missed frames"
        );
    }

    #[test]
    fn animation_wall_fps_uses_wall_clock_between_rendered_frames() {
        use std::time::Duration;

        assert_eq!(
            super::format_frames_per_second(16, Duration::from_millis(240)),
            "62.5"
        );
        assert_eq!(super::format_frames_per_second(1, Duration::ZERO), "n/a");
    }

    #[test]
    fn animation_timer_coalesces_wakeups_while_the_ui_is_busy() {
        use std::sync::atomic::AtomicBool;

        let pending = AtomicBool::new(false);
        assert!(super::queue_animation_frame(&pending));
        assert!(!super::queue_animation_frame(&pending));
        pending.store(false, std::sync::atomic::Ordering::Release);
        assert!(super::queue_animation_frame(&pending));
    }

    #[test]
    fn animation_timer_ignores_stale_wakeup_messages() {
        let multimedia = Some(super::AnimationTimerSource::Multimedia(1));
        assert!(super::is_animation_wakeup(
            multimedia,
            42,
            super::WM_APP_ANIMATION_FRAME,
            42,
        ));
        assert!(!super::is_animation_wakeup(
            multimedia,
            42,
            super::WM_APP_ANIMATION_FRAME,
            41,
        ));
        assert!(!super::is_animation_wakeup(
            multimedia,
            42,
            super::WM_TIMER,
            42,
        ));

        let fallback = Some(super::AnimationTimerSource::Window);
        assert!(super::is_animation_wakeup(
            fallback,
            42,
            super::WM_TIMER,
            42,
        ));
        assert!(!super::is_animation_wakeup(
            fallback,
            42,
            super::WM_APP_ANIMATION_FRAME,
            42,
        ));
        assert!(!super::is_animation_wakeup(
            fallback,
            42,
            super::WM_TIMER,
            41,
        ));
    }

    #[test]
    fn a_clock_tick_redraws_only_if_the_time_changed_and_animation_is_idle() {
        use crate::clock::ClockSnapshot;

        let first = ClockSnapshot {
            hour: 21,
            minute: 41,
            second: 17,
            weekday: 3,
            month: 11,
            day: 18,
        };
        let next = ClockSnapshot {
            second: 18,
            ..first
        };
        let mut displayed = first;
        assert!(!super::update_clock(&mut displayed, first, false));
        assert!(!super::update_clock(&mut displayed, next, true));
        assert_eq!(
            displayed, next,
            "the animation frame must receive the latest second"
        );
        assert!(super::update_clock(&mut displayed, first, false));
        assert!(!super::update_clock(&mut displayed, first, false));
    }
}
