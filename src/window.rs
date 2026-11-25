use std::mem::size_of;
use std::ptr::{null, null_mut};
use std::sync::atomic::{AtomicBool, AtomicUsize, Ordering};
use std::time::{Duration, Instant};

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
use crate::canvas::{Canvas, MUTED, Painter, Rect};
use crate::clock::ClockSnapshot;
use crate::hover::{
    HOVER_GRACE_MILLISECONDS, HOVER_RECHECK_MILLISECONDS, HoverCommand, HoverTracker,
    pointer_inside_shape,
};
use crate::player::Player;
use crate::render::{RenderParams, render_frame_with_margin};
use crate::settings::DockAnchor;
use crate::settings::Settings;
use crate::text::{TextLayer, TextParams};
use crate::tray::{TRAY_CALLBACK_MESSAGE, Tray, TrayCommand};
use crate::update;

const CANVAS_WIDTH: f32 = 512.0;
const CANVAS_HEIGHT: f32 = 600.0;
const TIMER_ID_MEDIA: usize = 5;
const TIMER_ID_HOVER_GRACE: usize = 2;
const TIMER_ID_HOVER_RECHECK: usize = 3;
const TIMER_ID_CLOCK: usize = 4;
const TIMER_ID_UPDATE: usize = 6;
const ANIMATION_TIMER_RESOLUTION_MILLISECONDS: u32 = 1;
const CLOCK_TIMER_MILLISECONDS: u32 = 1000;
const TOP_MARGIN: f32 = 0.0;
const WM_APP_REFRESH_DISPLAY: u32 = WM_APP + 1;
const WM_APP_TRAY_MENU: u32 = WM_APP + 3;
const WM_APP_ANIMATION_FRAME: u32 = WM_APP + 4;
const WM_APP_CANCEL_SEEK: u32 = WM_APP + 5;
const WM_APP_QUICK_ACTION: u32 = WM_APP + 6;
const WM_APP_MENU_CLOSED: u32 = WM_APP + 7;
const HOTKEY_PASSTHROUGH: i32 = 73;

static ANIMATION_FRAME_PENDING: AtomicBool = AtomicBool::new(false);
static ANIMATION_FRAME_REQUESTED: AtomicBool = AtomicBool::new(false);
static NEXT_ANIMATION_TIMER_GENERATION: AtomicUsize = AtomicUsize::new(0x1000);
static ACTIVE_ANIMATION_TIMER_GENERATION: AtomicUsize = AtomicUsize::new(0);

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum ExitAction {
    Exit,
    Restart,
}

struct InstanceGuard(HANDLE);
struct HotkeyGuard {
    hwnd: HWND,
    id: i32,
    shortcut: crate::hotkey::Shortcut,
}
impl HotkeyGuard {
    /// Registers `shortcut`. NOTE: when `registration` short-circuits because
    /// `current` already owns the requested binding, the returned guard owns no
    /// OS registration; assigning it over `current` would unregister the live
    /// binding. Callers must check `shortcut` equality first.
    fn register(
        hwnd: HWND,
        shortcut: crate::hotkey::Shortcut,
        current: Option<&Self>,
    ) -> Option<Self> {
        use windows_sys::Win32::UI::Input::KeyboardAndMouse::{MOD_NOREPEAT, RegisterHotKey};
        let (id, shortcut) = crate::hotkey::registration(
            current.map(|c| (c.id, c.shortcut)),
            shortcut,
            |id, key| unsafe {
                RegisterHotKey(hwnd, id, key.modifiers | MOD_NOREPEAT, key.key) != 0
            },
        )?;
        Some(Self { hwnd, id, shortcut })
    }
}
impl Drop for HotkeyGuard {
    fn drop(&mut self) {
        unsafe {
            windows_sys::Win32::UI::Input::KeyboardAndMouse::UnregisterHotKey(self.hwnd, self.id);
        }
    }
}

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
        if update::apply_pending_update() {
            return Ok(());
        }

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
        let exit_action = result?;
        drop(_instance_guard);
        if exit_action == ExitAction::Restart
            && let Ok(executable) = std::env::current_exe()
        {
            let _ = std::process::Command::new(executable).spawn();
        }
        Ok(())
    }
}

/// Runs a fallible steady-state action. A transient failure is logged and the
/// overlay keeps running instead of exiting with the misleading start-up
/// message; start-up calls before the message loop keep using `?`.
fn tolerate<T>(context: &str, action: impl FnOnce() -> Result<T, String>) -> Option<T> {
    action()
        .inspect_err(|error| eprintln!("HiyoDyna {context}: {error}"))
        .ok()
}

fn run_loop(hwnd: HWND) -> Result<ExitAction, String> {
    let mut scale = window_scale(hwnd);
    let mut settings = Settings::load();
    crate::i18n::set_language(settings.language);
    let mut exit_action = ExitAction::Exit;
    let mut clock = ClockSnapshot::now();
    let mut surface = Surface::new(canvas_size(scale), scale)?;
    surface.hotkey = HotkeyGuard::register(hwnd, settings.shortcut, None);
    surface.shortcut_available = surface.hotkey.is_some();
    if !surface.shortcut_available {
        eprintln!(
            "HiyoDyna: {} is unavailable; use Settings or the tray menu to restore interaction",
            settings.shortcut.label()
        );
        crate::native_ui::notice(hwnd, "Global shortcut", &crate::i18n::shortcut_hint(None));
    }
    surface.music_height = player_shape(hwnd, scale).height;
    surface.access = Some(crate::accessibility::Host::new(hwnd));
    let mut animation = Animation::new_with_tuning(settings.tuning());
    let mut hover = HoverTracker::new();
    let mut position = primary_position(
        surface.size,
        settings.top_gap,
        settings.dock,
        animation.current.width * scale,
        scale,
    )?;
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
        if SetTimer(
            hwnd,
            TIMER_ID_UPDATE,
            update::UPDATE_INTERVAL_MILLISECONDS,
            None,
        ) == 0
        {
            return Err(last_error("SetTimer"));
        }
        if SetTimer(hwnd, TIMER_ID_MEDIA, 350, None) == 0 {
            return Err(last_error("SetTimer"));
        }
    }

    let mut last_frame = Instant::now();
    let mut hover_grace_deadline: Option<Instant> = None;
    let mut last_hover_poll = Instant::now();
    let mut animation_timer = AnimationTimer::new(hwnd);
    animation_timer.set_unlimited(settings.unlimited_fps)?;
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
    update::cleanup_old_binary();
    update::request_check(hwnd, settings.update, false);
    let mut message = MSG::default();
    let mut quick_menu: Option<crate::quick_menu::Menu> = None;
    loop {
        // At rest the message loop sleeps between input, display and clock messages.
        let received = unsafe { GetMessageW(&mut message, null_mut(), 0, 0) };
        if received == -1 {
            return Err(last_error("GetMessageW"));
        }
        if received == 0 {
            return Ok(exit_action);
        }

        if surface.fullscreen.hidden
            && message.hwnd == hwnd
            && matches!(
                message.message,
                WM_KEYDOWN
                    | WM_MOUSEWHEEL
                    | WM_LBUTTONDOWN
                    | WM_LBUTTONUP
                    | WM_RBUTTONUP
                    | WM_MOUSEMOVE
                    | WM_MOUSELEAVE
            )
        {
            continue;
        }

        match message.message {
            crate::lyric_search::MESSAGE if message.hwnd == hwnd => {
                for selection in crate::lyric_search::take_selections() {
                    if surface.player.worker.use_lyrics(
                        &selection.key,
                        selection.generation,
                        selection.lyrics,
                    ) {
                        surface.player.refresh();
                        surface.player.resume_lyrics();
                        last_frame = Instant::now();
                        animation_timer.ensure_started();
                    } else {
                        show_about(
                            hwnd,
                            "The playing track changed. Open Find lyrics again to choose lyrics for the current track.",
                        );
                    }
                }
            }
            crate::preferences::MESSAGE if message.hwnd == hwnd => {
                for command in crate::preferences::take_commands() {
                    tolerate("settings command", || {
                        apply_tray_command(
                            hwnd,
                            command,
                            &mut settings,
                            &mut animation,
                            &mut animation_timer,
                            &mut surface,
                            tray.as_mut(),
                            scale,
                            &mut position,
                            &clock,
                            &mut exit_action,
                        )
                    });
                }
                crate::preferences::update(&settings, surface.shortcut_available);
                resume_hover_tracking(hwnd, &mut hover, &animation);
                last_frame = Instant::now();
                animation_timer.ensure_started();
            }
            WM_HOTKEY
                if message.hwnd == hwnd
                    && (message.wParam == HOTKEY_PASSTHROUGH as usize || message.wParam == 74) =>
            {
                if crate::preferences::capturing_shortcut()
                    || surface
                        .hotkey
                        .as_ref()
                        .is_none_or(|binding| binding.id as usize != message.wParam)
                {
                    continue;
                }
                tolerate("passthrough", || {
                    surface.set_passthrough(hwnd, !surface.passthrough)
                });
                resume_hover_tracking(hwnd, &mut hover, &animation);
                last_frame = Instant::now();
                animation_timer.ensure_started();
            }
            WM_APP_MENU_CLOSED if message.hwnd == hwnd => {
                if quick_menu
                    .as_ref()
                    .is_some_and(|menu| menu.handle() as usize == message.wParam)
                {
                    quick_menu = None;
                    resume_hover_tracking(hwnd, &mut hover, &animation);
                }
            }
            WM_APP_QUICK_ACTION if message.hwnd == hwnd => {
                if let Some(action) = crate::quick_menu::action(message.wParam) {
                    quick_menu = None;
                    tolerate("quick action", || {
                        apply_quick_action(
                            hwnd,
                            action,
                            tray.as_mut(),
                            &mut settings,
                            &mut animation,
                            &mut animation_timer,
                            &mut surface,
                            scale,
                            &mut position,
                            &clock,
                            &mut exit_action,
                        )
                    });
                    resume_hover_tracking(hwnd, &mut hover, &animation);
                    last_frame = Instant::now();
                    animation_timer.ensure_started();
                }
            }
            crate::accessibility::MESSAGE if message.hwnd == hwnd => {
                let actions = surface
                    .access
                    .as_ref()
                    .map(|a| a.take_actions())
                    .unwrap_or_default();
                if surface.fullscreen.hidden {
                    continue;
                }
                for action in actions {
                    use crate::accessibility::Action;
                    let left = (surface.logical_width() - animation.current.width) * 0.5;
                    match action {
                        Action::Settings => {
                            tolerate("settings menu", || {
                                show_settings_menu(
                                    hwnd,
                                    tray.as_mut(),
                                    &mut settings,
                                    &mut animation,
                                    &mut surface,
                                    scale,
                                    &mut position,
                                    &clock,
                                    &mut exit_action,
                                )
                            });
                        }
                        Action::Music => {
                            surface.player.select_page(true);
                            animation.expand();
                        }
                        Action::Focus => unsafe {
                            SetForegroundWindow(hwnd);
                            windows_sys::Win32::UI::Input::KeyboardAndMouse::SetFocus(hwnd);
                        },
                        Action::Seek(seconds) => {
                            surface.player.seek_to(seconds);
                        }
                        Action::Click(x, y) => {
                            if animation.current.progress() < 0.9 {
                                animation.target = if surface.player.music {
                                    player_shape(hwnd, scale)
                                } else {
                                    crate::animation::EXPANDED
                                };
                            } else if surface.player.music
                                && crate::player::Player::compact_lyrics_rect(
                                    left,
                                    animation.current.width,
                                )
                                .contains(x, y)
                            {
                                tolerate("compact lyrics", || {
                                    apply_tray_command(
                                        hwnd,
                                        TrayCommand::CompactLyrics(!settings.compact_lyrics),
                                        &mut settings,
                                        &mut animation,
                                        &mut animation_timer,
                                        &mut surface,
                                        tray.as_mut(),
                                        scale,
                                        &mut position,
                                        &clock,
                                        &mut exit_action,
                                    )
                                });
                                crate::preferences::update(&settings, surface.shortcut_available);
                            } else if Rect::new(
                                left + animation.current.width - 52.0,
                                10.0,
                                36.0,
                                34.0,
                            )
                            .contains(x, y)
                            {
                                tolerate("settings menu", || {
                                    show_settings_menu(
                                        hwnd,
                                        tray.as_mut(),
                                        &mut settings,
                                        &mut animation,
                                        &mut surface,
                                        scale,
                                        &mut position,
                                        &clock,
                                        &mut exit_action,
                                    )
                                });
                            } else if let Some(music) = crate::player::page_hit(
                                x,
                                y,
                                left,
                                animation.current.width,
                                animation.current.height,
                            ) {
                                surface.player.select_page(music);
                            } else if surface
                                .player
                                .search_hit(x, y, left, animation.current.width)
                            {
                                crate::lyric_search::show(hwnd, &surface.player.snapshot);
                            } else if surface.player.music {
                                surface.player.click(x, y, left, animation.current.width);
                                surface.player.finish_seek();
                            }
                        }
                    }
                }
                animation.target = if surface.player.music {
                    player_shape(hwnd, scale)
                } else {
                    crate::animation::EXPANDED
                };
                last_frame = Instant::now();
                animation_timer.ensure_started();
            }
            WM_KEYDOWN if message.hwnd == hwnd => {
                if let Some(access) = &surface.access {
                    match message.wParam {
                        9 => access.focus_next(
                            unsafe {
                                windows_sys::Win32::UI::Input::KeyboardAndMouse::GetKeyState(16)
                            } < 0,
                        ),
                        13 | 32 => access.activate(),
                        27 => {
                            access.clear_focus();
                            unsafe {
                                windows_sys::Win32::UI::Input::KeyboardAndMouse::SetFocus(
                                    null_mut(),
                                );
                            }
                            surface.player.reset();
                            unsafe {
                                if windows_sys::Win32::UI::Input::KeyboardAndMouse::GetCapture()
                                    == hwnd
                                {
                                    windows_sys::Win32::UI::Input::KeyboardAndMouse::ReleaseCapture(
                                    );
                                }
                            }
                            animation.collapse();
                        }
                        33 => surface.player.wheel(false, false),
                        34 => surface.player.wheel(true, false),
                        38 => surface.player.browse_lines(false),
                        40 => surface.player.browse_lines(true),
                        37 | 39 => {
                            let delta = if message.wParam == 37 { -5.0 } else { 5.0 };
                            surface.player.seek_relative(delta);
                        }
                        _ => {}
                    }
                }
                surface.player.dirty = true;
                unsafe {
                    if !surface.player.dragging
                        && windows_sys::Win32::UI::Input::KeyboardAndMouse::GetCapture() == hwnd
                    {
                        windows_sys::Win32::UI::Input::KeyboardAndMouse::ReleaseCapture();
                    }
                }
                if message.wParam != 27 {
                    animation.target = if surface.player.music {
                        player_shape(hwnd, scale)
                    } else {
                        crate::animation::EXPANDED
                    };
                }
                last_frame = Instant::now();
                animation_timer.ensure_started();
            }
            WM_MOUSEWHEEL if message.hwnd == hwnd => {
                if animation.current.progress() > 0.9 {
                    let down = ((message.wParam >> 16) as u16 as i16) < 0;
                    let (x, y) = wheel_position(message.lParam, position, scale);
                    let left = (surface.logical_width() - animation.current.width) * 0.5;
                    surface.player.wheel_at(
                        down,
                        message.wParam & 0x0008 != 0,
                        x,
                        y,
                        left,
                        animation.current.width,
                    );
                    animation.target = if surface.player.music {
                        player_shape(hwnd, scale)
                    } else {
                        crate::animation::EXPANDED
                    };
                    resume_hover_tracking(hwnd, &mut hover, &animation);
                    last_frame = Instant::now();
                    animation_timer.ensure_started();
                }
            }
            WM_LBUTTONDOWN if message.hwnd == hwnd => {
                let (x, y) = mouse_position(message.lParam);
                let x = x / scale;
                let y = y / scale;
                let left = (surface.logical_width() - animation.current.width) * 0.5;
                if animation.current.progress() < 0.9 {
                    animation.target = if surface.player.music {
                        player_shape(hwnd, scale)
                    } else {
                        crate::animation::EXPANDED
                    };
                    last_frame = Instant::now();
                    animation_timer.ensure_started();
                } else if surface.player.music
                    && crate::player::Player::compact_lyrics_rect(left, animation.current.width)
                        .contains(x, y)
                {
                    tolerate("compact lyrics", || {
                        apply_tray_command(
                            hwnd,
                            TrayCommand::CompactLyrics(!settings.compact_lyrics),
                            &mut settings,
                            &mut animation,
                            &mut animation_timer,
                            &mut surface,
                            tray.as_mut(),
                            scale,
                            &mut position,
                            &clock,
                            &mut exit_action,
                        )
                    });
                    crate::preferences::update(&settings, surface.shortcut_available);
                    animation_timer.ensure_started();
                } else if animation.current.progress() > 0.9
                    && Rect::new(left + animation.current.width - 52.0, 10.0, 36.0, 34.0)
                        .contains(x, y)
                {
                    tolerate("settings menu", || {
                        show_settings_menu(
                            hwnd,
                            tray.as_mut(),
                            &mut settings,
                            &mut animation,
                            &mut surface,
                            scale,
                            &mut position,
                            &clock,
                            &mut exit_action,
                        )
                    });
                } else if animation.current.progress() > 0.9
                    && let Some(music) = crate::player::page_hit(
                        x,
                        y,
                        left,
                        animation.current.width,
                        animation.current.height,
                    )
                {
                    surface.player.select_page(music);
                    animation.target = if music {
                        player_shape(hwnd, scale)
                    } else {
                        crate::animation::EXPANDED
                    };
                    last_frame = Instant::now();
                    animation_timer.ensure_started();
                } else if animation.current.progress() > 0.9
                    && surface
                        .player
                        .search_hit(x, y, left, animation.current.width)
                {
                    crate::lyric_search::show(hwnd, &surface.player.snapshot);
                } else if surface.player.music {
                    surface.player.click(x, y, left, animation.current.width);
                    if surface.player.dragging {
                        unsafe {
                            windows_sys::Win32::UI::Input::KeyboardAndMouse::SetCapture(hwnd);
                        }
                    }
                    animation.target = if surface.player.music {
                        player_shape(hwnd, scale)
                    } else {
                        crate::animation::EXPANDED
                    };
                    last_frame = Instant::now();
                    animation_timer.ensure_started();
                }
            }
            WM_LBUTTONUP if message.hwnd == hwnd => {
                if surface.player.dragging {
                    let (x, _) = mouse_position(message.lParam);
                    surface.player.seek(
                        x / scale,
                        (surface.logical_width() - animation.current.width) * 0.5 + 28.0,
                        animation.current.width - 56.0,
                    );
                    surface.player.finish_seek();
                    surface.player.dirty = true;
                    last_frame = Instant::now();
                    animation_timer.ensure_started();
                    unsafe {
                        windows_sys::Win32::UI::Input::KeyboardAndMouse::ReleaseCapture();
                    }
                }
            }
            WM_APP_CANCEL_SEEK if message.hwnd == hwnd => {
                if unsafe { windows_sys::Win32::UI::Input::KeyboardAndMouse::GetCapture() } != hwnd
                {
                    surface.player.cancel_seek();
                    surface.player.dirty = true;
                    animation_timer.ensure_started();
                }
            }
            WM_MOUSEMOVE if message.hwnd == hwnd => {
                if surface.passthrough || surface.fullscreen.hidden {
                    continue;
                }
                track_mouse_leave(hwnd);
                let (x, y) = mouse_position(message.lParam);
                let inside = surface.player.dragging
                    || surface.hover_contains(
                        &animation,
                        position,
                        (x, y),
                        settings.top_gap <= 0.0,
                    );
                if surface.player.dragging {
                    surface.player.seek(
                        x / scale,
                        (surface.logical_width() - animation.current.width) * 0.5 + 28.0,
                        animation.current.width - 56.0,
                    );
                    animation_timer.ensure_started();
                }
                let command = hover.pointer_moved(inside);
                tolerate("hover update", || {
                    set_hover(
                        &mut animation,
                        &mut animation_timer,
                        &mut last_frame,
                        inside,
                    )
                });
                tolerate("hover command", || {
                    apply_hover_command(
                        hwnd,
                        command,
                        &mut animation,
                        &mut animation_timer,
                        &mut last_frame,
                        &mut hover_grace_deadline,
                    )
                });
            }
            WM_MOUSELEAVE if message.hwnd == hwnd => {
                if surface.player.dragging || surface.fullscreen.hidden {
                    continue;
                }
                let inside = pointer_position(hwnd).is_some_and(|point| {
                    surface.hover_contains(&animation, position, point, settings.top_gap <= 0.0)
                });
                let command = hover.pointer_moved(inside);
                tolerate("hover update", || {
                    set_hover(
                        &mut animation,
                        &mut animation_timer,
                        &mut last_frame,
                        inside,
                    )
                });
                tolerate("hover command", || {
                    apply_hover_command(
                        hwnd,
                        command,
                        &mut animation,
                        &mut animation_timer,
                        &mut last_frame,
                        &mut hover_grace_deadline,
                    )
                });
            }
            WM_RBUTTONUP if message.hwnd == hwnd => {
                drop(quick_menu.take());
                quick_menu = crate::quick_menu::show(
                    hwnd,
                    surface.player.music,
                    surface.player.browse.is_some(),
                    surface.passthrough,
                    surface
                        .shortcut_available
                        .then(|| settings.shortcut.label())
                        .as_deref(),
                );
            }
            message_id
                if message.hwnd == hwnd
                    && animation_timer.is_wakeup(message_id, message.wParam) =>
            {
                // Keep the wakeup pending while drawing; a tick that lands
                // during the frame requests exactly one follow-up frame.
                let posted_frame = message_id == WM_APP_ANIMATION_FRAME;
                let now = Instant::now();
                let mut presented = false;
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
                    if animation.target.height < crate::animation::EXPANDED.height {
                        surface.player.collapse();
                        animation.target = surface.compact_shape(&settings);
                    } else if surface.player.music {
                        animation.target = player_shape(hwnd, scale);
                    }
                    let moving = animation.advance(elapsed);
                    if let Some(next) = tolerate("island placement", || {
                        primary_position(
                            surface.size,
                            settings.top_gap,
                            settings.dock,
                            animation.current.width * scale,
                            scale,
                        )
                    }) {
                        position = next;
                    }
                    let draw_start = Instant::now();
                    // A Settings/Find lyrics scroll has the floor for its short
                    // motion. Advance the island state but defer drawing so the
                    // two surfaces do not compete for the same frame budget.
                    let present_start = if crate::native_ui::scroll_animating() && moving {
                        draw_start
                    } else {
                        surface.draw(animation.current, scale, &settings, &clock, animation.hover);
                        let present_start = Instant::now();
                        tolerate("island present", || surface.present(hwnd, position));
                        presented = true;
                        present_start
                    };
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
                    if presented && !animation_timer.unlimited_fps() {
                        crate::display::wait_for_vblank();
                    }
                    if !surface.needs_animation(moving, animation.current.progress() > 0.9) {
                        animation_timer.stop();
                        if trace_frames && trace_frame_count > 0 {
                            let wall_duration = trace_first_frame
                                .zip(trace_last_frame)
                                .map(|(first, last)| last.saturating_duration_since(first))
                                .unwrap_or_default();
                            eprintln!(
                                "animation refresh={}Hz interval={}ms unlimited={} elapsed_fps={:.1} wall_fps={} frames={} avg_dt_ms={:.2} max_dt_ms={:.2} avg_draw_ms={:.2} max_draw_ms={:.2} avg_present_ms={:.2} max_present_ms={:.2} frame_span_ms={:.2}",
                                animation_timer.refresh_hz(),
                                animation_timer.interval_millis(),
                                animation_timer.unlimited_fps(),
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
                    let follow_up = finish_animation_frame(
                        &ANIMATION_FRAME_PENDING,
                        &ANIMATION_FRAME_REQUESTED,
                    );
                    if follow_up {
                        if animation_timer.is_active() {
                            // A timer tick landed while this frame was drawing.
                            // Queue the follow-up now so the next compositor
                            // refresh is not missed.
                            let generation = animation_timer.generation;
                            if unsafe { PostMessageW(hwnd, WM_APP_ANIMATION_FRAME, generation, 0) }
                                == 0
                            {
                                ANIMATION_FRAME_PENDING.store(false, Ordering::Release);
                            }
                        } else {
                            ANIMATION_FRAME_PENDING.store(false, Ordering::Release);
                        }
                    }
                }
                // WM_TIMER is low priority and only generated for an empty
                // queue, so a saturated render stream can postpone the hover
                // recheck and grace for a long time. Drive both from the
                // frame loop while frames are flowing; the timers remain the
                // idle-period fallback.
                if !surface.passthrough && !surface.fullscreen.hidden && quick_menu.is_none() {
                    if now.saturating_duration_since(last_hover_poll)
                        >= Duration::from_millis(u64::from(HOVER_RECHECK_MILLISECONDS))
                    {
                        last_hover_poll = now;
                        let inside =
                            hover_inside(hwnd, &mut surface, &animation, position, &settings);
                        let command = hover.pointer_moved(inside);
                        tolerate("hover update", || {
                            set_hover(
                                &mut animation,
                                &mut animation_timer,
                                &mut last_frame,
                                inside,
                            )
                        });
                        tolerate("hover command", || {
                            apply_hover_command(
                                hwnd,
                                command,
                                &mut animation,
                                &mut animation_timer,
                                &mut last_frame,
                                &mut hover_grace_deadline,
                            )
                        });
                    }
                    if hover_grace_deadline.is_some_and(|deadline| now >= deadline) {
                        hover_grace_deadline = None;
                        let inside =
                            hover_inside(hwnd, &mut surface, &animation, position, &settings);
                        let command = hover.grace_expired(inside);
                        tolerate("hover update", || {
                            set_hover(
                                &mut animation,
                                &mut animation_timer,
                                &mut last_frame,
                                inside,
                            )
                        });
                        tolerate("hover command", || {
                            apply_hover_command(
                                hwnd,
                                command,
                                &mut animation,
                                &mut animation_timer,
                                &mut last_frame,
                                &mut hover_grace_deadline,
                            )
                        });
                    }
                }
            }
            WM_TIMER if message.hwnd == hwnd && message.wParam == TIMER_ID_HOVER_RECHECK => {
                if surface.passthrough || surface.fullscreen.hidden || quick_menu.is_some() {
                    continue;
                }
                if let Some((x, y)) = pointer_position(hwnd) {
                    let navigation = keyboard_navigation_active(surface.access.as_ref());
                    let inside = navigation
                        || surface.player.dragging
                        || surface.hover_contains(
                            &animation,
                            position,
                            (x, y),
                            settings.top_gap <= 0.0,
                        );
                    let command = hover.pointer_moved(inside);
                    tolerate("hover update", || {
                        set_hover(
                            &mut animation,
                            &mut animation_timer,
                            &mut last_frame,
                            inside,
                        )
                    });
                    tolerate("hover command", || {
                        apply_hover_command(
                            hwnd,
                            command,
                            &mut animation,
                            &mut animation_timer,
                            &mut last_frame,
                            &mut hover_grace_deadline,
                        )
                    });
                }
            }
            WM_TIMER if message.hwnd == hwnd && message.wParam == TIMER_ID_HOVER_GRACE => {
                unsafe { KillTimer(hwnd, TIMER_ID_HOVER_GRACE) };
                hover_grace_deadline = None;
                if surface.passthrough || surface.fullscreen.hidden || quick_menu.is_some() {
                    continue;
                }
                let navigation = keyboard_navigation_active(surface.access.as_ref());
                let inside = navigation
                    || surface.player.dragging
                    || pointer_position(hwnd)
                        .map(|(x, y)| {
                            surface.hover_contains(
                                &animation,
                                position,
                                (x, y),
                                settings.top_gap <= 0.0,
                            )
                        })
                        .unwrap_or(false);
                let command = hover.grace_expired(inside);
                tolerate("hover update", || {
                    set_hover(
                        &mut animation,
                        &mut animation_timer,
                        &mut last_frame,
                        inside,
                    )
                });
                tolerate("hover command", || {
                    apply_hover_command(
                        hwnd,
                        command,
                        &mut animation,
                        &mut animation_timer,
                        &mut last_frame,
                        &mut hover_grace_deadline,
                    )
                });
            }
            WM_TIMER if message.hwnd == hwnd && message.wParam == TIMER_ID_CLOCK => {
                if update::take_ready() && update::restart_for_update() {
                    update::mark_app_exiting();
                    return Ok(ExitAction::Exit);
                }
                if let Some(result) = update::take_status() {
                    show_update_status(&mut tray, result);
                }
                if update_clock(
                    &mut clock,
                    ClockSnapshot::now(),
                    animation_timer.is_active(),
                ) && surface.clock_visible(animation.current, &settings)
                {
                    surface.draw(animation.current, scale, &settings, &clock, animation.hover);
                    tolerate("island present", || surface.present(hwnd, position));
                }
            }
            WM_TIMER if message.hwnd == hwnd && message.wParam == TIMER_ID_UPDATE => {
                update::request_check(hwnd, settings.update, false);
            }
            WM_TIMER if message.hwnd == hwnd && message.wParam == TIMER_ID_MEDIA => {
                let hidden = crate::fullscreen::foreground_covers_island(hwnd);
                if hidden != surface.fullscreen.hidden {
                    tolerate("fullscreen retract", || {
                        surface.set_fullscreen(hwnd, hidden)
                    });
                    resume_hover_tracking(hwnd, &mut hover, &animation);
                    last_frame = Instant::now();
                    animation_timer.ensure_started();
                }
                let was_dragging = surface.player.dragging;
                surface.player.refresh();
                let page_changed = surface
                    .player
                    .update_selection(Instant::now(), settings.lyric_offset_ms);
                crate::lyric_search::track_changed(&surface.player.snapshot);
                if was_dragging && !surface.player.dragging {
                    unsafe {
                        windows_sys::Win32::UI::Input::KeyboardAndMouse::ReleaseCapture();
                    }
                }
                let collapsed = animation.target.height < crate::animation::EXPANDED.height;
                let mut resized = false;
                if collapsed {
                    let target = surface.compact_shape(&settings);
                    resized = target != animation.target;
                    animation.target = target;
                } else if page_changed {
                    animation.target = if surface.player.music {
                        player_shape(hwnd, scale)
                    } else {
                        crate::animation::EXPANDED
                    };
                    resized = true;
                }
                let visible = page_changed
                    || (surface.player.music && animation.current.progress() > 0.9)
                    || collapsed;
                let needs_frame = surface.player.needs_frame();
                // A failed re-arm would silently freeze the media poll and the
                // lyrics/progress with it; surface the failure the same way as
                // the other transient steady-state errors.
                if unsafe { SetTimer(hwnd, TIMER_ID_MEDIA, 100, None) } == 0 {
                    eprintln!("HiyoDyna media poll re-arm: {}", last_error("SetTimer"));
                }
                if visible
                    && !surface.fullscreen.hidden
                    && (needs_frame || resized)
                    && !animation_timer.is_active()
                {
                    last_frame = Instant::now();
                    animation_timer.ensure_started();
                }
            }
            WM_APP_REFRESH_DISPLAY if message.hwnd == hwnd => {
                scale = window_scale(hwnd);
                tolerate("surface resize", || {
                    surface.resize(canvas_size(scale), scale)
                });
                surface.music_height = player_shape(hwnd, scale).height;
                animation.target =
                    surface.display_target(animation.target, &settings, player_shape(hwnd, scale));
                // The panel may have changed to a different refresh rate.
                animation_timer.stop();
                last_frame = Instant::now();
                animation_timer.ensure_started();
                if let Some(next) = tolerate("island placement", || {
                    primary_position(
                        surface.size,
                        settings.top_gap,
                        settings.dock,
                        animation.current.width * scale,
                        scale,
                    )
                }) {
                    position = next;
                }
                surface.draw(animation.current, scale, &settings, &clock, animation.hover);
                tolerate("island present", || surface.present(hwnd, position));
            }
            WM_APP_TRAY_MENU if message.hwnd == hwnd => {
                let event = (message.lParam as u32) & 0xFFFF;
                if event == WM_RBUTTONUP || event == WM_CONTEXTMENU {
                    drop(quick_menu.take());
                    quick_menu = crate::quick_menu::show(
                        hwnd,
                        surface.player.music,
                        surface.player.browse.is_some(),
                        surface.passthrough,
                        surface
                            .shortcut_available
                            .then(|| settings.shortcut.label())
                            .as_deref(),
                    );
                }
            }
            update::WM_APP_UPDATE_READY if message.hwnd == hwnd => {
                if update::take_ready() && update::restart_for_update() {
                    update::mark_app_exiting();
                    return Ok(ExitAction::Exit);
                }
            }
            update::WM_APP_UPDATE_STATUS if message.hwnd == hwnd => {
                if message.wParam != 0 {
                    show_update_status(&mut tray, message.lParam);
                }
            }
            WM_CLOSE if message.hwnd == hwnd => {
                update::mark_app_exiting();
                return Ok(exit_action);
            }
            _ => unsafe {
                if crate::preferences::translate_message(&message)
                    || crate::lyric_search::translate_message(&message)
                    || crate::native_ui::translate_message(&message)
                {
                    continue;
                }
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
        WM_APP_ANIMATION_FRAME => {
            // Modal loops (menus, message boxes) dispatch posted frames here instead of
            // through the main loop, so the frame flags must be released here too.
            ANIMATION_FRAME_PENDING.store(false, Ordering::Release);
            ANIMATION_FRAME_REQUESTED.store(false, Ordering::Release);
            0
        }
        crate::update::WM_APP_UPDATE_READY => 0,
        crate::update::WM_APP_UPDATE_STATUS => {
            if wparam != 0 {
                crate::update::store_status(lparam);
            }
            0
        }
        WM_MOUSEACTIVATE => MA_NOACTIVATE as LRESULT,
        WM_COMMAND if lparam == 0 && crate::quick_menu::action(wparam & 0xFFFF).is_some() => {
            unsafe {
                PostMessageW(hwnd, WM_APP_QUICK_ACTION, wparam & 0xFFFF, 0);
            }
            0
        }
        WM_UNINITMENUPOPUP => {
            unsafe {
                PostMessageW(hwnd, WM_APP_MENU_CLOSED, wparam, 0);
            }
            0
        }
        WM_NCHITTEST
            if unsafe { GetWindowLongPtrW(hwnd, GWL_EXSTYLE) } as u32 & WS_EX_TRANSPARENT != 0 =>
        {
            HTTRANSPARENT as LRESULT
        }
        WM_CAPTURECHANGED => {
            unsafe {
                PostMessageW(hwnd, WM_APP_CANCEL_SEEK, 0, 0);
            }
            0
        }
        WM_ERASEBKGND => 1,
        WM_CLOSE => {
            crate::update::mark_app_exiting();
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

struct PageFade {
    value: f32,
    music: bool,
    updated: Instant,
}
impl PageFade {
    fn new(now: Instant) -> Self {
        Self {
            value: 0.0,
            music: false,
            updated: now,
        }
    }
    fn advance(&mut self, music: bool, now: Instant) -> f32 {
        let elapsed = now
            .saturating_duration_since(self.updated)
            .as_secs_f32()
            .min(0.1);
        self.updated = now;
        let goal = if music { 1.0 } else { 0.0 };
        if crate::animation::reduced_motion() {
            // Page switches land instantly; no crossfade under reduced motion.
            self.music = music;
            self.value = goal;
            return self.value;
        }
        if music != self.music {
            self.music = music;
            return self.value;
        }
        self.value += (goal - self.value) * (1.0 - (-elapsed / 0.055).exp());
        if (goal - self.value).abs() < 0.002 {
            self.value = goal;
        }
        self.value
    }
    fn moving(&self) -> bool {
        self.value != if self.music { 1.0 } else { 0.0 }
    }
    fn snap_to_music(&mut self) {
        self.value = 1.0;
        self.music = true;
    }
}

struct HoverEntry {
    shape: ShapeState,
    origin: POINT,
    attached: bool,
}

struct Surface {
    dc: HDC,
    bitmap: HBITMAP,
    previous_bitmap: HGDIOBJ,
    bits: *mut u8,
    size: (u32, u32),
    text: TextLayer,
    page_fade: PageFade,
    compact_fade: Option<PageFade>,
    music_height: f32,
    painter: Painter,
    player: Player,
    access: Option<crate::accessibility::Host>,
    background: Vec<u8>,
    background_params: Option<RenderParams>,
    passthrough: bool,
    visibility: f32,
    visibility_updated: Instant,
    fullscreen: crate::fullscreen::Transition,
    shortcut_available: bool,
    hotkey: Option<HotkeyGuard>,
    compact_active: bool,
    compact: crate::compact::Presentation,
    hover_entry: Option<HoverEntry>,
    cover_glow: crate::compact_glow::CoverGlow,
}

impl Surface {
    fn hover_contains(
        &mut self,
        animation: &Animation,
        origin: POINT,
        point: (f32, f32),
        attached: bool,
    ) -> bool {
        let scale = self.painter.scale;
        if self.fullscreen.hidden {
            return false;
        }
        let inside =
            pointer_inside_current_shape(animation, scale, self.size, point.0, point.1, attached);
        if animation.target.height < crate::animation::EXPANDED.height {
            self.hover_entry = inside.then_some(HoverEntry {
                shape: scale_shape(animation.current, scale),
                origin,
                attached,
            });
            return inside;
        }
        // Expansion may narrow a long compact sentence. Keep its entry footprint
        // in screen coordinates, including while edge docking moves the window.
        let inside = inside
            || self.hover_entry.as_ref().is_some_and(|entry| {
                pointer_inside_shape(
                    self.size,
                    entry.shape,
                    TOP_MARGIN * scale,
                    point.0 + (origin.x - entry.origin.x) as f32,
                    point.1 + (origin.y - entry.origin.y) as f32,
                    entry.attached,
                )
            });
        if !inside {
            self.hover_entry = None;
        }
        inside
    }
    fn logical_width(&self) -> f32 {
        self.size.0 as f32 / self.painter.scale
    }
    fn needs_animation(&self, shape_moving: bool, expanded: bool) -> bool {
        if self.fullscreen.hidden && !self.fullscreen.moving() {
            return false;
        }
        shape_moving
            || self.fullscreen.moving()
            || self.page_fade.moving()
            || (!expanded && self.compact_fade.as_ref().is_some_and(PageFade::moving))
            || (expanded && self.player.music && self.player.needs_frame())
            || (!expanded && self.compact_active && self.player.needs_frame())
            || (!expanded && self.compact.moving(Instant::now()))
            || (self.visibility - if self.passthrough { 0.16 } else { 1.0 }).abs() > 0.002
    }
    fn compact_shape(&mut self, settings: &Settings) -> ShapeState {
        let lyric = if self.player.music && settings.compact_lyrics {
            self.player
                .compact_lyric(Instant::now(), settings.lyric_offset_ms)
        } else {
            None
        };
        let width = self.logical_width();
        let mut shape = crate::compact::shape_at(
            &mut self.painter,
            lyric.map(|(line, _)| line),
            lyric.map_or(0.0, |(_, at)| at),
            compact_max_width(width),
        );
        if lyric.is_none() && !self.player.snapshot.source.is_empty() {
            shape.width += crate::compact::SOURCE_SPACE;
        }
        shape
    }
    fn clock_visible(&self, state: ShapeState, settings: &Settings) -> bool {
        !self.player.music
            || (state.compact_content_alpha() > 0.0
                && (!settings.compact_lyrics
                    || self
                        .player
                        .compact_lyric(Instant::now(), settings.lyric_offset_ms)
                        .is_none()))
    }
    fn display_target(
        &mut self,
        previous: ShapeState,
        settings: &Settings,
        music: ShapeState,
    ) -> ShapeState {
        if previous.height < crate::animation::EXPANDED.height {
            self.compact_shape(settings)
        } else if self.player.music {
            music
        } else {
            crate::animation::EXPANDED
        }
    }
    fn set_passthrough(&mut self, hwnd: HWND, enabled: bool) -> Result<(), String> {
        self.update_input_style(hwnd, enabled || self.fullscreen.hidden)?;
        self.passthrough = enabled;
        self.visibility_updated = Instant::now();
        Ok(())
    }
    fn set_fullscreen(&mut self, hwnd: HWND, hidden: bool) -> Result<(), String> {
        self.update_input_style(hwnd, hidden || self.passthrough)?;
        self.fullscreen.hidden = hidden;
        if let Some(access) = &self.access {
            access.set_hidden(hidden);
        }
        self.visibility_updated = Instant::now();
        Ok(())
    }
    fn update_input_style(&mut self, hwnd: HWND, enabled: bool) -> Result<(), String> {
        let style = unsafe { GetWindowLongPtrW(hwnd, GWL_EXSTYLE) };
        let next = if enabled {
            style | WS_EX_TRANSPARENT as isize
        } else {
            style & !(WS_EX_TRANSPARENT as isize)
        };
        unsafe {
            SetWindowLongPtrW(hwnd, GWL_EXSTYLE, next);
            if GetWindowLongPtrW(hwnd, GWL_EXSTYLE) != next {
                return Err(last_error("click-through window style"));
            }
            SetWindowPos(
                hwnd,
                null_mut(),
                0,
                0,
                0,
                0,
                SWP_NOMOVE | SWP_NOSIZE | SWP_NOZORDER | SWP_NOACTIVATE | SWP_FRAMECHANGED,
            );
        }
        if enabled {
            self.player.cancel_seek();
            if let Some(access) = &self.access {
                access.clear_focus();
            }
            unsafe {
                if windows_sys::Win32::UI::Input::KeyboardAndMouse::GetCapture() == hwnd {
                    windows_sys::Win32::UI::Input::KeyboardAndMouse::ReleaseCapture();
                }
                if keyboard_focused(hwnd) {
                    windows_sys::Win32::UI::Input::KeyboardAndMouse::SetFocus(null_mut());
                }
            }
        }
        Ok(())
    }
    fn new(size: (u32, u32), scale: f32) -> Result<Self, String> {
        let text = TextLayer::new(scale)?;
        let painter = Painter::new(scale)?;
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
                page_fade: PageFade::new(Instant::now()),
                compact_fade: None,
                music_height: 500.0,
                painter,
                player: Player::new(),
                access: None,
                background: vec![0; size.0 as usize * size.1 as usize * 4],
                background_params: None,
                passthrough: false,
                visibility: 1.0,
                visibility_updated: Instant::now(),
                fullscreen: crate::fullscreen::Transition::default(),
                shortcut_available: false,
                hotkey: None,
                compact_active: false,
                compact: crate::compact::Presentation::default(),
                hover_entry: None,
                cover_glow: crate::compact_glow::CoverGlow::default(),
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
        let now = Instant::now();
        // DESIGN.md: reduced motion removes the automatic spring, overshoot,
        // and halo. The shadow lift and rim brightening ride on hover too, so
        // zeroing it here keeps the resting look under reduced motion.
        let hover = if crate::animation::reduced_motion() {
            0.0
        } else {
            hover
        };
        let goal = if self.passthrough { 0.16 } else { 1.0 };
        let elapsed = now
            .saturating_duration_since(self.visibility_updated)
            .as_secs_f32()
            .min(0.1);
        self.visibility_updated = now;
        self.fullscreen.advance(elapsed);
        let state = self.fullscreen.shape(state);
        if crate::animation::reduced_motion() {
            self.visibility = goal;
        } else {
            self.visibility += (goal - self.visibility) * (1.0 - (-elapsed / 0.045).exp());
            if (goal - self.visibility).abs() < 0.002 {
                self.visibility = goal;
            }
        }
        // The player borrow must end before the mutable render phase below;
        // capture the small scalars and the rendered text here instead of
        // cloning the whole lyric line on every frame.
        let candidate = self.player.compact_lyric(now, settings.lyric_offset_ms);
        let compact_pulse = candidate.as_ref().map(|(line, at)| (line.start, *at));
        let compact_line_text = candidate.as_ref().map(|(line, _)| line.text());
        let compact_lyric = candidate.is_some() && self.player.music && settings.compact_lyrics;
        // Content remains current while hidden. Only invalid or replaced media
        // discards the presentation, so collapse never revives a stale line.
        if candidate.is_none()
            || !self
                .compact
                .matches(&self.player.snapshot.key, self.player.snapshot.generation)
        {
            self.compact = crate::compact::Presentation::default();
            if let Some(fade) = &mut self.compact_fade {
                fade.value = 0.0;
                fade.music = false;
            }
        }
        self.compact_active = compact_lyric;
        if let Some((line, at)) = &candidate {
            self.compact.update(
                &self.player.snapshot.key,
                self.player.snapshot.generation,
                line,
                *at,
                now,
            );
        }
        let fade = self.compact_fade.get_or_insert_with(|| {
            let mut fade = PageFade::new(now);
            fade.music = compact_lyric;
            fade.value = if fade.music { 1.0 } else { 0.0 };
            fade
        });
        // Prime the destination while compact content is fully hidden, before
        // the first compact frame of a collapse can reveal clock glyphs.
        if compact_lyric && state.compact_content_alpha() == 0.0 {
            fade.snap_to_music();
        }
        let compact_mix = fade.advance(compact_lyric, now);
        let length = self.size.0 as usize * self.size.1 as usize * 4;
        // SAFETY: The DIB allocated exactly size.0 * size.1 * 4 writable bytes and outlives self.
        let pixels = unsafe { std::slice::from_raw_parts_mut(self.bits, length) };
        let accent = settings.accent_bgra();
        let params = RenderParams {
            attached: settings.top_gap <= 0.0,
            state: scale_shape(state, scale),
            top_margin: TOP_MARGIN * scale,
            scale,
            shadow_opacity: settings.shadow_opacity(),
            hover,
            accent,
        };
        if self.background_params != Some(params) {
            render_frame_with_margin(&mut self.background, self.size, params);
            self.background_params = Some(params);
        }
        pixels.copy_from_slice(&self.background);
        self.cover_glow
            .update(self.player.snapshot.artwork.as_ref());
        self.cover_glow.advance(
            elapsed,
            compact_lyric && self.player.snapshot.playing && !self.fullscreen.hidden,
        );
        if compact_mix > 0.0 {
            self.cover_glow.draw(pixels, self.size, params);
        }
        self.text.update(clock, settings.clock);
        let music_mix = self.page_fade.advance(self.player.music, Instant::now());
        if compact_mix < 1.0 {
            self.text.draw_with_opacity(
                pixels,
                TextParams {
                    size: self.size,
                    state: ShapeState {
                        height: state.height.min(crate::animation::EXPANDED.height),
                        ..state
                    },
                    top_margin: TOP_MARGIN * scale,
                    scale,
                    accent,
                    show_seconds: settings.show_seconds,
                    hover,
                    compact_trailing_space: if self.player.snapshot.source.is_empty() {
                        0.0
                    } else {
                        crate::compact::SOURCE_SPACE
                    },
                },
                (1.0 - compact_mix) * (1.0 - music_mix * state.expanded_content_alpha()),
            );
        }
        let mut canvas = Canvas::new(pixels, self.size, &mut self.painter);
        let left = (self.size.0 as f32 / scale - state.width) * 0.5;
        canvas.clip = Rect::new(
            left + 12.0,
            8.0,
            (state.width - 24.0).max(0.0),
            (state.height - 16.0).max(0.0),
        );
        if compact_mix > 0.0 {
            // Countdown is hidden within the pulse window; only lyric text and
            // the note receive this opacity. Background and glow are already drawn.
            let pulse = compact_pulse.map_or(1.0, |(start, at)| {
                crate::compact::wait_pulse(
                    start - at,
                    self.player.snapshot.playing,
                    state.compact_content_alpha() > 0.0 && !self.fullscreen.hidden,
                )
            });
            canvas.opacity = state.compact_content_alpha() * compact_mix * pulse;
            self.compact.draw(
                &mut canvas,
                left,
                state,
                settings,
                now,
                compact_max_width(self.size.0 as f32 / scale),
            );
        }
        if state.progress() < 0.1 {
            self.player.compact_drawn();
        }
        canvas.opacity = state.expanded_content_alpha();
        if state.compact_content_alpha() > 0.0
            && compact_mix < 1.0
            && !self.player.snapshot.source.is_empty()
        {
            canvas.opacity = state.compact_content_alpha() * (1.0 - compact_mix);
            crate::compact::draw_source(
                &mut canvas,
                left + state.width,
                state.height,
                &self.player.snapshot.source,
            );
        }
        canvas.opacity = state.expanded_content_alpha();
        if music_mix > 0.0 {
            canvas.opacity *= music_mix;
            self.player.draw(
                &mut canvas,
                left,
                crate::animation::EXPANDED.width,
                self.music_height,
                settings,
            );
            canvas.opacity = state.expanded_content_alpha();
        }
        if music_mix < 1.0 && canvas.opacity > 0.0 {
            canvas.opacity *= 1.0 - music_mix;
            canvas.gear(
                Rect::new(left + state.width - 49.0, 12.0, 30.0, 30.0),
                MUTED,
            );
        }
        canvas.opacity = state.expanded_content_alpha();
        crate::player::draw_page_dots(&mut canvas, left, state.width, state.height, music_mix);
        if let Some(access) = &self.access {
            let mut nodes = if self.player.music && state.progress() > 0.9 {
                self.player.accessible_nodes(left, state.width)
            } else {
                let mut nodes = vec![];
                if state.progress() > 0.9 {
                    let mut gear = crate::accessibility::Node::button(
                        2,
                        crate::i18n::tr("Settings"),
                        Rect::new(left + state.width - 52.0, 10.0, 36.0, 34.0),
                        true,
                    );
                    gear.action = crate::accessibility::Action::Settings;
                    nodes.push(gear);
                }
                nodes
            };
            if let Some(toggle) = nodes.iter_mut().find(|node| node.id == 24) {
                toggle.name = crate::i18n::tr(if settings.compact_lyrics {
                    "Hide compact lyrics"
                } else {
                    "Show compact lyrics"
                })
                .into();
            }
            nodes.retain(|n| n.id != 20 && n.id != 21);
            let mut pages = crate::player::page_nodes(left, state.width, state.height);
            pages[1].action = crate::accessibility::Action::Music;
            if state.progress() > 0.9 {
                nodes.extend(pages);
            } else {
                let mut node = crate::accessibility::Node::button(
                    22,
                    if self.player.music {
                        crate::i18n::tr("Open music player").to_owned()
                    } else {
                        format!(
                            "{} — {}",
                            crate::i18n::tr("Open clock"),
                            crate::text::clock_hint(clock, settings.clock)
                        )
                    },
                    Rect::new(left, 0.0, state.width, state.height),
                    true,
                );
                node.action = crate::accessibility::Action::Click(
                    left + state.width * 0.5,
                    state.height * 0.5,
                );
                nodes.push(node);
            }
            if self.fullscreen.hidden {
                nodes.clear();
            }
            access.update(
                if let Some(lyrics) = &compact_line_text {
                    crate::i18n::format_copy("HiyoDyna lyrics: {lyrics}", &[("lyrics", lyrics)])
                } else if self.player.music && state.progress() > 0.9 {
                    crate::i18n::format_copy(
                        "HiyoDyna: {title} — {artist}",
                        &[
                            ("title", &self.player.snapshot.title),
                            ("artist", &self.player.snapshot.artist),
                        ],
                    )
                } else {
                    crate::i18n::tr("HiyoDyna clock").into()
                },
                nodes,
                scale,
            );
            if keyboard_focused_from_access(access)
                && let Some(rect) = access.focus_rect()
            {
                canvas.rect(
                    Rect::new(rect.x, rect.y + rect.h - 2.0, rect.w, 2.0),
                    1.0,
                    accent,
                    0.8,
                );
            }
        }
    }

    fn resize(&mut self, size: (u32, u32), scale: f32) -> Result<(), String> {
        let mut replacement = Self::new(size, scale)?;
        std::mem::swap(&mut replacement.player, &mut self.player);
        std::mem::swap(&mut replacement.access, &mut self.access);
        std::mem::swap(&mut replacement.page_fade, &mut self.page_fade);
        std::mem::swap(&mut replacement.compact_fade, &mut self.compact_fade);
        std::mem::swap(&mut replacement.compact, &mut self.compact);
        replacement.music_height = self.music_height;
        replacement.passthrough = self.passthrough;
        replacement.visibility = self.visibility;
        std::mem::swap(&mut replacement.fullscreen, &mut self.fullscreen);
        replacement.shortcut_available = self.shortcut_available;
        replacement.hotkey = self.hotkey.take();
        replacement.compact_active = self.compact_active;
        *self = replacement;
        Ok(())
    }

    fn present(&self, hwnd: HWND, position: POINT) -> Result<(), String> {
        if let Some(params) = &self.background_params {
            let shape = params.state;
            let left = position.x + ((self.size.0 as f32 - shape.width) * 0.5).round() as i32;
            let top = position.y + params.top_margin.round() as i32;
            crate::island_surface::remember_bounds(
                hwnd,
                windows_sys::Win32::Foundation::RECT {
                    left,
                    top,
                    right: left + shape.width.round() as i32,
                    bottom: top + shape.height.round() as i32,
                },
            );
        }
        if let Some(access) = &self.access {
            access.origin(position.x, position.y);
        }
        let size = SIZE {
            cx: self.size.0 as i32,
            cy: self.size.1 as i32,
        };
        let origin = POINT { x: 0, y: 0 };
        let blend = BLENDFUNCTION {
            BlendOp: AC_SRC_OVER as u8,
            BlendFlags: 0,
            SourceConstantAlpha: (self.visibility.clamp(0.0, 1.0) * self.fullscreen.alpha() * 255.0)
                .round() as u8,
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

fn keyboard_focused(hwnd: HWND) -> bool {
    unsafe { windows_sys::Win32::UI::Input::KeyboardAndMouse::GetFocus() == hwnd }
}
fn keyboard_focused_from_access(access: &crate::accessibility::Host) -> bool {
    access.focus_rect().is_some()
}
/// A closing menu or owned dialog can leave native keyboard focus on the
/// Island; `GetFocus` keeps reporting it for the thread even after the user
/// moves to another app. Only accessibility navigation tracks a focused item
/// and draws the focus ring, so that is the state that holds the Island open.
fn keyboard_navigation_active(access: Option<&crate::accessibility::Host>) -> bool {
    access.is_some_and(keyboard_focused_from_access)
}
fn hover_inside(
    hwnd: HWND,
    surface: &mut Surface,
    animation: &Animation,
    position: POINT,
    settings: &Settings,
) -> bool {
    if surface.passthrough || surface.fullscreen.hidden {
        return false;
    }
    if keyboard_navigation_active(surface.access.as_ref()) || surface.player.dragging {
        return true;
    }
    pointer_position(hwnd).is_some_and(|point| {
        surface.hover_contains(animation, position, point, settings.top_gap <= 0.0)
    })
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

fn primary_position(
    size: (u32, u32),
    top_gap: f32,
    dock: DockAnchor,
    shape_width: f32,
    scale: f32,
) -> Result<POINT, String> {
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
        x: docked_x(
            monitor.rcMonitor.left,
            monitor.rcMonitor.right - monitor.rcMonitor.left,
            size.0 as i32,
            (shape_width
                + if top_gap <= 0.0 {
                    24.0 * size.1 as f32 / CANVAS_HEIGHT
                } else {
                    0.0
                })
            .round() as i32,
            dock,
        ),
        // top_gap is a logical preference; like every other metric it must
        // scale with the display DPI or the gap shrinks on HiDPI panels.
        y: monitor.rcMonitor.top + (top_gap * scale).round() as i32,
    })
}

fn docked_x(
    left: i32,
    monitor_width: i32,
    canvas_width: i32,
    shape_width: i32,
    dock: DockAnchor,
) -> i32 {
    let padding = (canvas_width - shape_width) / 2;
    match dock {
        DockAnchor::Left => left - padding,
        DockAnchor::Center => centered_x(left, monitor_width, canvas_width),
        DockAnchor::Right => left + monitor_width - shape_width - padding,
    }
}

fn window_scale(hwnd: HWND) -> f32 {
    let dpi = unsafe { GetDpiForWindow(hwnd) };
    dpi.max(96) as f32 / 96.0
}

fn canvas_size(scale: f32) -> (u32, u32) {
    let mut info = MONITORINFO {
        cbSize: size_of::<MONITORINFO>() as u32,
        ..Default::default()
    };
    let width = unsafe {
        if GetMonitorInfoW(
            MonitorFromWindow(null_mut(), MONITOR_DEFAULTTOPRIMARY),
            &mut info,
        ) != 0
        {
            (info.rcMonitor.right - info.rcMonitor.left).max(1) as u32
        } else {
            (CANVAS_WIDTH * scale).round() as u32
        }
    };
    (width, (CANVAS_HEIGHT * scale).round() as u32)
}

fn compact_max_width(canvas_width: f32) -> f32 {
    // Leave room for the attached shoulders, shadow, and spring overshoot.
    ((canvas_width - 64.0) / crate::animation::OVERSHOOT_MAX).max(crate::animation::COMPACT.width)
}

fn player_shape(hwnd: HWND, scale: f32) -> ShapeState {
    let mut info = MONITORINFO {
        cbSize: size_of::<MONITORINFO>() as u32,
        ..Default::default()
    };
    let height = unsafe {
        if GetMonitorInfoW(MonitorFromWindow(hwnd, MONITOR_DEFAULTTOPRIMARY), &mut info) != 0 {
            ((info.rcWork.bottom - info.rcWork.top) as f32 / scale - 52.0).clamp(300.0, 500.0)
        } else {
            500.0
        }
    };
    ShapeState {
        width: 420.0,
        height,
        radius: 30.0,
    }
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
    attached: bool,
) -> bool {
    pointer_inside_shape(
        size,
        scale_shape(animation.current, scale),
        TOP_MARGIN * scale,
        x,
        y,
        attached,
    )
}

fn mouse_position(lparam: LPARAM) -> (f32, f32) {
    let packed = lparam as u32;
    let x = (packed as u16 as i16) as f32;
    let y = ((packed >> 16) as u16 as i16) as f32;
    (x, y)
}

fn wheel_position(lparam: LPARAM, origin: POINT, scale: f32) -> (f32, f32) {
    let (x, y) = mouse_position(lparam);
    ((x - origin.x as f32) / scale, (y - origin.y as f32) / scale)
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

fn resume_hover_tracking(hwnd: HWND, hover: &mut HoverTracker, animation: &Animation) {
    let expanded = animation.target.height >= crate::animation::EXPANDED.height;
    hover.synchronize(expanded);
    unsafe {
        KillTimer(hwnd, TIMER_ID_HOVER_GRACE);
        if expanded {
            SetTimer(
                hwnd,
                TIMER_ID_HOVER_RECHECK,
                HOVER_RECHECK_MILLISECONDS,
                None,
            );
        } else {
            KillTimer(hwnd, TIMER_ID_HOVER_RECHECK);
        }
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
    grace_deadline: &mut Option<Instant>,
) -> Result<(), String> {
    match command {
        HoverCommand::Expand => {
            *grace_deadline = None;
            if animation_timer.is_active() {
                animation.advance(last_frame.elapsed().as_secs_f32());
            }
            animation.expand();
            *last_frame = Instant::now();
            animation_timer.ensure_started();
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
            *grace_deadline = None;
            if animation_timer.is_active() {
                animation.advance(last_frame.elapsed().as_secs_f32());
            }
            animation.collapse();
            *last_frame = Instant::now();
            animation_timer.ensure_started();
            unsafe {
                KillTimer(hwnd, TIMER_ID_HOVER_GRACE);
                KillTimer(hwnd, TIMER_ID_HOVER_RECHECK);
            }
        }
        HoverCommand::ArmGrace => {
            // The animation frame loop also enforces this deadline because a
            // saturated render queue can postpone WM_TIMER for a long time.
            *grace_deadline =
                Some(Instant::now() + Duration::from_millis(u64::from(HOVER_GRACE_MILLISECONDS)));
            if unsafe { SetTimer(hwnd, TIMER_ID_HOVER_GRACE, HOVER_GRACE_MILLISECONDS, None) } == 0
            {
                return Err(last_error("SetTimer"));
            }
        }
        HoverCommand::CancelGrace => {
            *grace_deadline = None;
            unsafe {
                KillTimer(hwnd, TIMER_ID_HOVER_GRACE);
            }
        }
        HoverCommand::None => {}
    }
    Ok(())
}

fn queue_animation_frame(pending: &AtomicBool, requested: &AtomicBool) -> bool {
    if pending.swap(true, Ordering::AcqRel) {
        // A frame is already queued or drawing. Remember that another tick
        // landed so the handler can queue exactly one follow-up frame.
        requested.store(true, Ordering::Release);
        false
    } else {
        true
    }
}

fn finish_animation_frame(pending: &AtomicBool, requested: &AtomicBool) -> bool {
    pending.store(false, Ordering::Release);
    requested.swap(false, Ordering::AcqRel)
        && pending
            .compare_exchange(false, true, Ordering::AcqRel, Ordering::Acquire)
            .is_ok()
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
    if queue_animation_frame(&ANIMATION_FRAME_PENDING, &ANIMATION_FRAME_REQUESTED) {
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
    refresh_hz: u32,
    interval_millis: u32,
    period_active: bool,
    unlimited_fps: bool,
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
            refresh_hz: crate::display::FALLBACK_REFRESH_HZ,
            interval_millis: crate::display::timer_interval_millis(
                crate::display::FALLBACK_REFRESH_HZ,
            ),
            period_active: false,
            unlimited_fps: false,
        }
    }

    fn is_active(&self) -> bool {
        self.source.is_some()
    }

    fn is_wakeup(&self, message_id: u32, wparam: WPARAM) -> bool {
        is_animation_wakeup(self.source, self.generation, message_id, wparam)
    }

    fn refresh_hz(&self) -> u32 {
        self.refresh_hz
    }

    fn interval_millis(&self) -> u32 {
        self.interval_millis
    }

    fn unlimited_fps(&self) -> bool {
        self.unlimited_fps
    }

    fn set_unlimited(&mut self, unlimited: bool) -> Result<(), String> {
        if self.unlimited_fps == unlimited {
            return Ok(());
        }
        self.unlimited_fps = unlimited;
        if self.is_active() {
            self.stop();
            self.start()?;
        }
        Ok(())
    }

    fn start(&mut self) -> Result<(), String> {
        if self.is_active() {
            return Ok(());
        }

        // Re-query on every start so a display mode change cannot leave the
        // animation locked to a previous panel rate.
        self.refresh_hz = crate::display::refresh_hz(self.hwnd);
        self.interval_millis = if self.unlimited_fps {
            1
        } else {
            crate::display::timer_interval_millis(self.refresh_hz)
        };
        // A tick racing with the previous stop can leave the flags set; a new
        // animation must be able to post its first frame.
        ANIMATION_FRAME_PENDING.store(false, Ordering::Release);
        ANIMATION_FRAME_REQUESTED.store(false, Ordering::Release);

        let generation = NEXT_ANIMATION_TIMER_GENERATION
            .fetch_add(1, Ordering::Relaxed)
            .max(1);
        self.generation = generation;
        if unsafe { timeBeginPeriod(ANIMATION_TIMER_RESOLUTION_MILLISECONDS) } == 0 {
            self.period_active = true;
            ACTIVE_ANIMATION_TIMER_GENERATION.store(generation, Ordering::Release);
            let timer_id = unsafe {
                timeSetEvent(
                    self.interval_millis,
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
        }

        eprintln!("HiyoDyna high-resolution timer unavailable; using WM_TIMER fallback");
        if unsafe { SetTimer(self.hwnd, self.generation, self.interval_millis, None) } == 0 {
            self.generation = 0;
            self.end_period();
            return Err(last_error("SetTimer"));
        }
        self.source = Some(AnimationTimerSource::Window);
        Ok(())
    }

    /// Starts the animation timer in the steady state; a failure is logged and
    /// the next event retries instead of tearing the overlay down.
    fn ensure_started(&mut self) {
        if let Err(error) = self.start() {
            eprintln!("HiyoDyna animation timer unavailable: {error}");
        }
    }

    fn end_period(&mut self) {
        if self.period_active {
            unsafe { timeEndPeriod(ANIMATION_TIMER_RESOLUTION_MILLISECONDS) };
            self.period_active = false;
        }
    }

    fn stop(&mut self) {
        if let Some(source) = self.source.take() {
            match source {
                AnimationTimerSource::Multimedia(timer_id) => unsafe {
                    timeKillEvent(timer_id);
                },
                AnimationTimerSource::Window => unsafe {
                    KillTimer(self.hwnd, self.generation);
                },
            }
        }
        self.end_period();
        ACTIVE_ANIMATION_TIMER_GENERATION.store(0, Ordering::Release);
        ANIMATION_FRAME_PENDING.store(false, Ordering::Release);
        ANIMATION_FRAME_REQUESTED.store(false, Ordering::Release);
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
    _tray: Option<&mut Tray>,
    settings: &mut Settings,
    _animation: &mut Animation,
    surface: &mut Surface,
    _scale: f32,
    _position: &mut POINT,
    _clock: &ClockSnapshot,
    _exit_action: &mut ExitAction,
) -> Result<(), String> {
    crate::preferences::show(hwnd, settings, surface.shortcut_available)
}

#[allow(clippy::too_many_arguments)]
fn apply_quick_action(
    hwnd: HWND,
    action: crate::quick_menu::Action,
    tray: Option<&mut Tray>,
    settings: &mut Settings,
    animation: &mut Animation,
    animation_timer: &mut AnimationTimer,
    surface: &mut Surface,
    scale: f32,
    position: &mut POINT,
    clock: &ClockSnapshot,
    exit_action: &mut ExitAction,
) -> Result<(), String> {
    match action {
        crate::quick_menu::Action::Clock => {
            surface.player.select_page(false);
            animation.expand();
        }
        crate::quick_menu::Action::Music => {
            surface.player.select_page(true);
            animation.target = player_shape(hwnd, scale);
        }
        crate::quick_menu::Action::ResumeLyrics => {
            surface.player.resume_lyrics();
        }
        crate::quick_menu::Action::SearchLyrics => {
            crate::lyric_search::show(hwnd, &surface.player.snapshot);
        }
        crate::quick_menu::Action::TogglePassthrough => {
            surface.set_passthrough(hwnd, !surface.passthrough)?;
        }
        crate::quick_menu::Action::Settings => {
            crate::preferences::show(hwnd, settings, surface.shortcut_available)?;
        }
        crate::quick_menu::Action::Exit => {
            apply_tray_command(
                hwnd,
                TrayCommand::Exit,
                settings,
                animation,
                animation_timer,
                surface,
                tray,
                scale,
                position,
                clock,
                exit_action,
            )?;
        }
    }
    surface.draw(animation.current, scale, settings, clock, animation.hover);
    surface.present(hwnd, *position)?;
    Ok(())
}

#[allow(clippy::too_many_arguments)]
fn apply_tray_command(
    hwnd: HWND,
    command: TrayCommand,
    settings: &mut Settings,
    animation: &mut Animation,
    animation_timer: &mut AnimationTimer,
    surface: &mut Surface,
    mut tray: Option<&mut Tray>,
    scale: f32,
    position: &mut POINT,
    clock: &ClockSnapshot,
    exit_action: &mut ExitAction,
) -> Result<(), String> {
    match command {
        TrayCommand::Settings => {
            return crate::preferences::show(hwnd, settings, surface.shortcut_available);
        }
        TrayCommand::Dock(dock) => settings.dock = dock,
        TrayCommand::TogglePassthrough => {
            surface.set_passthrough(hwnd, !surface.passthrough)?;
            return Ok(());
        }
        TrayCommand::Lyrics(mode) => settings.lyrics = mode,
        TrayCommand::LyricOffset(offset) => settings.lyric_offset_ms = offset.clamp(-10000, 10000),
        TrayCommand::CompactLyrics(enabled) => settings.compact_lyrics = enabled,
        TrayCommand::Language(language) => {
            settings.language = language;
            crate::i18n::set_language(language);
            crate::lyric_search::refresh_language();
            crate::native_ui::refresh_language();
        }
        TrayCommand::Shortcut(shortcut) => {
            if surface
                .hotkey
                .as_ref()
                .is_none_or(|binding| binding.shortcut != shortcut)
            {
                if let Some(binding) =
                    HotkeyGuard::register(hwnd, shortcut, surface.hotkey.as_ref())
                {
                    surface.hotkey = Some(binding);
                    surface.shortcut_available = true;
                } else {
                    crate::preferences::shortcut_error();
                    return Ok(());
                }
            }
            settings.shortcut = shortcut;
        }
        TrayCommand::ClearLyricsCache => {
            crate::lyric_provider::clear_cache();
            surface.player.worker.command(
                crate::media::MediaCommand::RefreshLyrics,
                &surface.player.snapshot,
            );
            return Ok(());
        }
        TrayCommand::Update(channel) => settings.update = channel,
        TrayCommand::Motion(motion) => settings.motion = motion,
        TrayCommand::Bounce(bounce) => settings.bounce = bounce,
        TrayCommand::Shadow(enabled) => settings.shadow = enabled,
        TrayCommand::TopGap(gap) => settings.top_gap = gap,
        TrayCommand::Clock(format) => settings.clock = format,
        TrayCommand::Accent(accent) => settings.accent = accent,
        TrayCommand::ShowSeconds(enabled) => settings.show_seconds = enabled,
        TrayCommand::UnlimitedFps(enabled) => settings.unlimited_fps = enabled,
        TrayCommand::ToggleAutostart => {
            if let Err(error) = crate::autostart::toggle() {
                eprintln!("HiyoDyna autostart: {error}");
            }
            return Ok(());
        }
        TrayCommand::Restart => {
            *exit_action = ExitAction::Restart;
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
            let tag = update::applied_tag().unwrap_or_else(|| "local".to_string());
            show_about(
                hwnd,
                &format!(
                    "HiyoDyna {}\n\nUpdate: {} ({tag})\nSettings: {settings_path}\nUpdate log: {}",
                    env!("CARGO_PKG_VERSION"),
                    settings.update.label(),
                    update::log_path()
                ),
            );
            return Ok(());
        }
        TrayCommand::Reset => {
            let update = settings.update;
            let old_shortcut = settings.shortcut;
            let mut defaults = Settings {
                update,
                ..Settings::default()
            };
            let mut shortcut_conflict = false;
            if surface
                .hotkey
                .as_ref()
                .is_none_or(|binding| binding.shortcut != defaults.shortcut)
            {
                if let Some(binding) =
                    HotkeyGuard::register(hwnd, defaults.shortcut, surface.hotkey.as_ref())
                {
                    surface.hotkey = Some(binding);
                    surface.shortcut_available = true;
                } else {
                    defaults.shortcut = old_shortcut;
                    shortcut_conflict = true;
                }
            }
            *settings = defaults;
            crate::i18n::set_language(settings.language);
            crate::lyric_search::refresh_language();
            crate::native_ui::refresh_language();
            if shortcut_conflict {
                crate::preferences::shortcut_error();
            }
        }
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
    tolerate("unlimited fps switch", || {
        animation_timer.set_unlimited(settings.unlimited_fps)
    });
    animation.set_tuning(settings.tuning());
    if animation.target.height <= crate::animation::COMPACT.height + 1.0 {
        animation.target = surface.compact_shape(settings);
    }
    if let Ok(next) = primary_position(
        surface.size,
        settings.top_gap,
        settings.dock,
        animation.current.width * scale,
        scale,
    ) {
        *position = next;
    }
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

fn show_update_status(tray: &mut Option<Tray>, result: isize) {
    let (text, warning) = match result {
        0 => ("HiyoDyna is up to date.", false),
        1 => ("No release is available.", false),
        _ => ("The update check failed. See the update log.", true),
    };
    if let Some(tray) = tray.as_mut() {
        tray.notify("Update", text, warning);
    }
}

fn show_about(hwnd: HWND, text: &str) {
    crate::native_ui::notice(hwnd, "HiyoDyna", text);
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
    use std::sync::atomic::Ordering;
    use windows_sys::Win32::UI::WindowsAndMessaging::{WM_DISPLAYCHANGE, WM_DPICHANGED, WM_TIMER};

    #[test]
    fn page_fade_starts_at_current_page_and_reverses_without_jumping() {
        let now = std::time::Instant::now();
        let mut fade = super::PageFade::new(now);
        assert_eq!(fade.advance(true, now), 0.0);
        let halfway = fade.advance(true, now + std::time::Duration::from_millis(60));
        assert!(halfway > 0.3 && halfway < 0.9);
        assert_eq!(
            fade.advance(false, now + std::time::Duration::from_millis(61)),
            halfway
        );
        assert!(fade.advance(false, now + std::time::Duration::from_millis(120)) < halfway);
    }

    #[test]
    fn fallback_clock_remains_eligible_for_idle_clock_ticks_with_music_selected() {
        use super::*;
        let mut surface = Surface::new(canvas_size(1.0), 1.0).unwrap();
        surface.player.worker = crate::media::MediaWorker::test_worker();
        surface.player.select_page(true);
        surface.player.snapshot.source = "Spotify".into();
        for playing in [false, true] {
            surface.player.snapshot.playing = playing;
            assert!(
                surface.clock_visible(crate::animation::COMPACT, &Settings::default()),
                "A visible fallback clock must tick even when Music is selected and playing={playing}"
            );
        }
        assert!(!surface.clock_visible(crate::animation::EXPANDED, &Settings::default()));
        surface.player.select_page(false);
        assert!(surface.clock_visible(crate::animation::EXPANDED, &Settings::default()));
    }

    #[test]
    fn display_refresh_preserves_compact_and_expanded_targets_for_each_page() {
        use super::*;
        let mut surface = Surface::new(canvas_size(1.0), 1.0).unwrap();
        surface.player.worker = crate::media::MediaWorker::test_worker();
        let settings = Settings::default();
        let music = ShapeState {
            height: 500.0,
            ..crate::animation::EXPANDED
        };
        for selected in [false, true] {
            surface.player.select_page(selected);
            assert_eq!(
                surface
                    .display_target(crate::animation::COMPACT, &settings, music)
                    .height,
                crate::animation::COMPACT.height,
                "Display changes must preserve a collapsed page: music={selected}"
            );
            assert_eq!(
                surface.display_target(crate::animation::EXPANDED, &settings, music),
                if selected {
                    music
                } else {
                    crate::animation::EXPANDED
                }
            );
        }
    }

    #[test]
    fn compact_music_without_lyrics_draws_the_clock_instead_of_an_empty_capsule() {
        use super::*;
        for scale in [1.0, 1.25, 1.5, 2.0] {
            let mut surface = Surface::new(canvas_size(scale), scale).unwrap();
            surface.player.worker = crate::media::MediaWorker::test_worker();
            surface.player.music = true;
            surface.player.snapshot.source = "Spotify.exe".into();
            surface.page_fade.music = true;
            surface.page_fade.value = 1.0;
            let settings = Settings::default();
            let shape = surface.compact_shape(&settings);
            surface.draw(shape, scale, &settings, &ClockSnapshot::now(), 0.0);
            let pixels = unsafe {
                std::slice::from_raw_parts(
                    surface.bits,
                    (surface.size.0 * surface.size.1 * 4) as usize,
                )
            };
            assert!(
                pixels
                    .as_chunks::<4>()
                    .0
                    .iter()
                    .filter(|p| p[0] > 180 && p[1] > 180 && p[2] > 180)
                    .count()
                    > 20,
                "the fallback must contain readable clock glyphs at {scale}x"
            );
        }
    }

    #[test]
    fn switching_compact_content_preserves_the_outgoing_frame_at_the_boundary() {
        use super::*;
        let mut surface = Surface::new(canvas_size(1.0), 1.0).unwrap();
        surface.player.worker = crate::media::MediaWorker::test_worker();
        surface.player.snapshot.key = "fixture".into();
        surface.player.snapshot.source = "Spotify".into();
        surface.player.snapshot.lyrics =
            crate::lyrics::parse_lrc("[00:00]A clearly different lyric").map(std::sync::Arc::new);
        surface.player.snapshot.timeline = Some(crate::media::Timeline {
            position: 1.0,
            duration: 60.0,
            playing: false,
            rate: 1.0,
            sampled: Instant::now(),
        });
        surface.player.select_page(true);
        surface.page_fade.music = true;
        surface.page_fade.value = 1.0;
        let settings = Settings::default();
        let clock = ClockSnapshot::now();
        let shape = surface.compact_shape(&settings);
        surface.draw(shape, 1.0, &settings, &clock, 0.0);
        let bytes = (surface.size.0 * surface.size.1 * 4) as usize;
        let previous = unsafe { std::slice::from_raw_parts(surface.bits, bytes) }.to_vec();
        surface.player.select_page(false);
        surface.draw(shape, 1.0, &settings, &clock, 0.0);
        let next = unsafe { std::slice::from_raw_parts(surface.bits, bytes) };
        assert!(
            previous == next,
            "a content switch must begin with its visible outgoing frame"
        );
    }

    #[test]
    fn collapse_keeps_lyrics_selected_from_the_first_compact_frame() {
        use super::*;
        for (scale, dt) in [1.0, 1.25, 1.5, 2.0]
            .into_iter()
            .flat_map(|scale| [1.0 / 30.0, 1.0 / 120.0].map(|dt| (scale, dt)))
        {
            let mut surface = compact_icon_test_surface(scale, true);
            let settings = Settings::default();
            let clock = compact_icon_test_clock();
            let mut animation = Animation::new();
            animation.current = ShapeState {
                height: 500.0,
                ..crate::animation::EXPANDED
            };
            animation.target = animation.current;
            // Begin with the clock mix, as when lyrics arrive while expanded.
            surface.compact_fade = Some(PageFade::new(Instant::now()));
            surface.draw(animation.current, scale, &settings, &clock, 0.0);
            animation.target = surface.compact_shape(&settings);
            let mut compact_frames = 0;
            let mut previous_baseline = None;
            for frame in 0..120 {
                animation.advance(dt);
                surface.draw(animation.current, scale, &settings, &clock, 0.0);
                if animation.current.compact_content_alpha() > 0.0 {
                    compact_frames += 1;
                    assert_eq!(
                        surface.compact_fade.as_ref().unwrap().value,
                        1.0,
                        "clock leaked into compact lyrics at {scale}x, dt={dt}, frame {frame}"
                    );
                    let pixels = compact_test_pixels(&surface);
                    if compact_frames == 1 || animation.current == animation.target {
                        let different_clock = ClockSnapshot {
                            hour: 3,
                            minute: 7,
                            second: 11,
                            ..clock
                        };
                        surface.draw(animation.current, scale, &settings, &different_clock, 0.0);
                        assert_eq!(
                            pixels,
                            compact_test_pixels(&surface),
                            "clock glyphs must not affect a lyric frame at {scale}x, dt={dt}, frame {frame}"
                        );
                    }
                    let left = (surface.size.0 as f32 / scale - animation.current.width) * 0.5;
                    // The last occupied lyric row tracks the real raster baseline, not just shape height.
                    let glyph_left = ((left + 36.0) * scale).ceil() as u32;
                    let glyph_right =
                        ((left + animation.current.width - 14.0) * scale).ceil() as u32;
                    let glyph_bottom = ((animation.current.height - 8.0) * scale).ceil() as u32;
                    let baseline = ((8.0 * scale) as u32..glyph_bottom).rev().find(|y| {
                        (glyph_left..glyph_right).any(|x| {
                            let index = ((y * surface.size.0 + x) * 4) as usize;
                            pixels[index..index + 3]
                                .iter()
                                .zip(&surface.background[index..index + 3])
                                .any(|(ink, background)| ink > background)
                        })
                    });
                    if let Some(baseline) = baseline {
                        if let Some(previous) = previous_baseline {
                            assert!(
                                baseline <= previous,
                                "lyric baseline bobbed down at {scale}x, dt={dt}, frame {frame}: {previous} -> {baseline}"
                            );
                        }
                        previous_baseline = Some(baseline);
                    }
                }
                if animation.current == animation.target {
                    break;
                }
            }
            assert!(compact_frames > 0);
            assert!(
                previous_baseline.is_some(),
                "lyrics must be rasterized at {scale}x"
            );
            assert_eq!(animation.current, animation.target);
        }
    }

    #[test]
    fn collapse_tracks_lyric_changes_while_compact_content_is_hidden() {
        use super::*;
        for scale in [1.0, 1.25, 1.5, 2.0] {
            let mut surface = compact_icon_test_surface(scale, true);
            let settings = Settings::default();
            let clock = compact_icon_test_clock();
            surface.draw(crate::animation::EXPANDED, scale, &settings, &clock, 0.0);
            assert!(
                surface.compact.matches("compact-icon-fixture", 0),
                "expanded frames must retain a current compact lyric presentation"
            );
            surface.player.snapshot.generation += 1;
            surface.player.snapshot.lyrics =
                crate::lyrics::parse_lrc("[00:00]Replacement lyric").map(std::sync::Arc::new);
            surface.draw(crate::animation::EXPANDED, scale, &settings, &clock, 0.0);
            assert!(surface.compact.matches("compact-icon-fixture", 1));
            assert_eq!(surface.compact_fade.as_ref().unwrap().value, 1.0);
            surface.player.snapshot.lyrics = None;
            surface.draw(crate::animation::COMPACT, scale, &settings, &clock, 0.0);
            assert!(!surface.compact_active);
            assert_eq!(surface.compact_fade.as_ref().unwrap().value, 0.0);
            assert!(!surface.compact.matches("compact-icon-fixture", 1));
        }
    }

    #[test]
    fn pulse_dims_only_compact_lyric_and_note_pixels() {
        use super::*;
        for scale in [1.0, 1.25, 1.5, 2.0] {
            let mut surface = compact_icon_test_surface(scale, true);
            surface.player.snapshot.lyrics =
                crate::lyrics::parse_lrc("[00:10]Upcoming lyric").map(std::sync::Arc::new);
            // Freeze the sampled position so the native raster comparison is exact.
            surface.player.snapshot.timeline.as_mut().unwrap().position = 7.8;
            let settings = Settings::default();
            let clock = compact_icon_test_clock();
            let shape = surface.compact_shape(&settings);
            surface.player.snapshot.playing = false;
            surface.draw(shape, scale, &settings, &clock, 0.0);
            let bright = compact_test_pixels(&surface);
            surface.player.snapshot.playing = true;
            surface.draw(shape, scale, &settings, &clock, 0.0);
            let dim = compact_test_pixels(&surface);
            surface.player.snapshot.timeline.as_mut().unwrap().position = 8.6;
            surface.draw(shape, scale, &settings, &clock, 0.0);
            assert_eq!(
                bright,
                compact_test_pixels(&surface),
                "the full-brightness phase must restore the exact native frame at {scale}x"
            );
            let left = (surface.size.0 as f32 / scale - shape.width) * 0.5;
            let mut note_changes = 0;
            let mut lyric_changes = 0;
            for (index, (a, b)) in bright
                .as_chunks::<4>()
                .0
                .iter()
                .zip(dim.as_chunks::<4>().0)
                .enumerate()
            {
                assert_eq!(a[3], b[3], "capsule alpha must stay unchanged at {scale}x");
                if a == b {
                    continue;
                }
                let x = (index as u32 % surface.size.0) as f32 / scale - left;
                let y = (index as u32 / surface.size.0) as f32 / scale;
                assert!(
                    (9.0..=32.0).contains(&y) && (12.0..shape.width - 12.0).contains(&x),
                    "pulse changed background outside lyric/note bounds at {scale}x: {x}, {y}"
                );
                assert!(
                    b[..3]
                        .iter()
                        .zip(&a[..3])
                        .all(|(dim, bright)| dim <= bright)
                );
                if x < 32.0 {
                    note_changes += 1;
                } else {
                    lyric_changes += 1;
                }
            }
            assert!(note_changes > 10, "the music note must pulse at {scale}x");
            assert!(lyric_changes > 20, "the lyric text must pulse at {scale}x");
            // A visible countdown and an already active lyric retain full alpha.
            for position in [6.0, 10.0] {
                surface.player.snapshot.timeline.as_mut().unwrap().position = position;
                surface.player.snapshot.playing = false;
                surface.draw(shape, scale, &settings, &clock, 0.0);
                let paused = compact_test_pixels(&surface);
                surface.player.snapshot.playing = true;
                surface.draw(shape, scale, &settings, &clock, 0.0);
                assert_eq!(
                    paused,
                    compact_test_pixels(&surface),
                    "countdown and active lyric frames must keep full alpha at {scale}x"
                );
            }
        }
    }

    #[test]
    fn pulse_wait_reuses_playback_frames_and_paused_wait_is_idle() {
        use super::*;
        for scale in [1.0, 1.25, 1.5, 2.0] {
            let mut surface = compact_icon_test_surface(scale, true);
            surface.player.snapshot.lyrics =
                crate::lyrics::parse_lrc("[00:10]Upcoming lyric").map(std::sync::Arc::new);
            surface.player.snapshot.timeline.as_mut().unwrap().position = 7.8;
            let settings = Settings::default();
            let shape = surface.compact_shape(&settings);
            surface.draw(shape, scale, &settings, &compact_icon_test_clock(), 0.0);
            assert!(
                !surface.needs_animation(false, false),
                "paused wait must remain idle"
            );
            surface.player.snapshot.playing = true;
            surface.player.snapshot.timeline.as_mut().unwrap().playing = true;
            assert!(
                surface.needs_animation(false, false),
                "visible playing wait must request frames"
            );
            surface.fullscreen.hidden = true;
            for _ in 0..10 {
                surface.fullscreen.advance(0.1);
            }
            assert!(
                !surface.needs_animation(false, false),
                "hidden playback must remain idle"
            );
        }
    }

    fn compact_icon_test_surface(scale: f32, lyrics: bool) -> super::Surface {
        use super::*;
        let mut surface = Surface::new(canvas_size(scale), scale).unwrap();
        surface.player.worker = crate::media::MediaWorker::test_worker();
        surface.player.select_page(true);
        surface.page_fade.music = true;
        surface.page_fade.value = 1.0;
        surface.player.snapshot.key = "compact-icon-fixture".into();
        if lyrics {
            surface.player.snapshot.lyrics =
                crate::lyrics::parse_lrc("[00:00]Follow the music").map(std::sync::Arc::new);
            surface.player.snapshot.timeline = Some(crate::media::Timeline {
                position: 1.0,
                duration: 60.0,
                playing: false,
                rate: 1.0,
                sampled: Instant::now(),
            });
        }
        surface
    }

    fn compact_icon_test_clock() -> crate::clock::ClockSnapshot {
        crate::clock::ClockSnapshot {
            hour: 12,
            minute: 58,
            second: 58,
            weekday: 5,
            month: 11,
            day: 20,
        }
    }

    fn compact_test_pixels(surface: &super::Surface) -> Vec<u8> {
        unsafe {
            std::slice::from_raw_parts(surface.bits, (surface.size.0 * surface.size.1 * 4) as usize)
        }
        .to_vec()
    }

    fn compact_icon_test_preview(
        surface: &super::Surface,
        shape: crate::animation::ShapeState,
        scale: f32,
        name: &str,
    ) {
        let Some(directory) = std::env::var_os("HIYODYNA_PREVIEW_DIR") else {
            return;
        };
        let directory = std::path::PathBuf::from(directory);
        std::fs::create_dir_all(&directory).unwrap();
        let mut pixels = compact_test_pixels(surface);
        for pixel in pixels.as_chunks_mut::<4>().0 {
            pixel.swap(0, 2);
        }
        let image = image::RgbaImage::from_raw(surface.size.0, surface.size.1, pixels).unwrap();
        image::imageops::crop_imm(
            &image,
            (((surface.logical_width() - shape.width) * 0.5 - 8.0) * scale).floor() as u32,
            0,
            ((shape.width + 16.0) * scale).ceil() as u32,
            ((shape.height + 8.0) * scale).ceil() as u32,
        )
        .to_image()
        .save(directory.join(format!("compact-{name}-{}.png", (scale * 100.0) as u32)))
        .unwrap();
    }

    #[test]
    fn compact_lyrics_keep_the_music_note_for_spotify_at_all_scales() {
        use super::*;
        for scale in [1.0, 1.25, 1.5, 2.0] {
            let mut surface = compact_icon_test_surface(scale, true);
            let settings = Settings::default();
            let shape = surface.compact_shape(&settings);
            let clock = compact_icon_test_clock();
            let mut reference = None;
            for source in ["Media Player", "Spotify.exe", ""] {
                surface.player.snapshot.source = source.into();
                surface.draw(shape, scale, &settings, &clock, 0.0);
                compact_icon_test_preview(&surface, shape, scale, &format!("lyrics-{source}"));
                let pixels = compact_test_pixels(&surface);
                let left = (surface.logical_width() - shape.width) * 0.5;
                let icon: Vec<_> = ((8.0 * scale) as u32..(32.0 * scale) as u32)
                    .flat_map(|y| {
                        (((left + 12.0) * scale).ceil() as u32
                            ..((left + 31.0) * scale).floor() as u32)
                            .map(move |x| ((y * surface.size.0 + x) * 4) as usize)
                    })
                    .map(|i| <[u8; 4]>::try_from(&pixels[i..i + 4]).unwrap())
                    .collect();
                assert!(surface.compact_active);
                assert!(
                    icon.iter()
                        .any(|p| p[..3] == crate::theme::MUTED && p[3] == 255),
                    "Active lyrics need a visible muted music note for {source:?} at {scale}x"
                );
                assert!(
                    icon.iter().all(|p| i16::from(p[1]) - i16::from(p[0]) < 30),
                    "Spotify's green logo must not leak into active lyrics at {scale}x"
                );
                if let Some(reference) = &reference {
                    assert_eq!(
                        &icon, reference,
                        "The lyric icon must be source-independent"
                    );
                } else {
                    reference = Some(icon);
                }
            }
        }
    }

    #[test]
    fn compact_fallback_source_icon_is_inside_the_right_edge_at_all_scales() {
        use super::*;
        for scale in [1.0, 1.25, 1.5, 2.0] {
            let mut surface = compact_icon_test_surface(scale, false);
            let settings = Settings::default();
            surface.player.snapshot.source = "Spotify.exe".into();
            let shape = surface.compact_shape(&settings);
            surface.draw(shape, scale, &settings, &compact_icon_test_clock(), 0.0);
            compact_icon_test_preview(&surface, shape, scale, "fallback-spotify");
            let pixels = compact_test_pixels(&surface);
            let green_x: Vec<_> = pixels
                .as_chunks::<4>()
                .0
                .iter()
                .enumerate()
                .filter(|(_, p)| i16::from(p[1]) - i16::from(p[0]) > 50)
                .map(|(i, _)| (i as u32 % surface.size.0) as f32 / scale)
                .collect();
            let right = (surface.logical_width() + shape.width) * 0.5;
            assert!(
                !green_x.is_empty(),
                "Fallback source must be visible at {scale}x"
            );
            assert!(
                green_x
                    .iter()
                    .all(|x| *x >= right - 31.0 && *x < right - 12.0),
                "The Spotify logo belongs inside the RIGHT edge at {scale}x; got {:?}",
                (green_x.first(), green_x.last())
            );
            surface.player.snapshot.source.clear();
            surface.draw(shape, scale, &settings, &compact_icon_test_clock(), 0.0);
            assert!(
                compact_test_pixels(&surface)
                    .as_chunks::<4>()
                    .0
                    .iter()
                    .all(|p| i16::from(p[1]) - i16::from(p[0]) <= 50),
                "Clearing the source must remove its logo immediately"
            );
        }
    }

    #[test]
    fn compact_fallback_clock_reserves_right_icon_space_with_seconds_at_all_scales() {
        use super::*;
        for scale in [1.0, 1.25, 1.5, 2.0] {
            let mut surface = compact_icon_test_surface(scale, false);
            let clock = compact_icon_test_clock();
            for format in [
                crate::clock::ClockFormat::TwentyFourHour,
                crate::clock::ClockFormat::TwelveHour,
            ] {
                for show_seconds in [false, true] {
                    let settings = Settings {
                        clock: format,
                        show_seconds,
                        ..Settings::default()
                    };
                    surface.player.snapshot.source.clear();
                    let plain_shape = surface.compact_shape(&settings);
                    surface.draw(plain_shape, scale, &settings, &clock, 0.0);
                    let size = surface.size;
                    let clock_columns = |pixels: &[u8]| -> Vec<u32> {
                        (0..size.0)
                            .filter(|&x| {
                                (0..(40.0 * scale) as u32).any(|y| {
                                    let i = ((y * size.0 + x) * 4) as usize;
                                    pixels[i] > 160 && pixels[i + 1] > 160 && pixels[i + 2] > 160
                                })
                            })
                            .collect()
                    };
                    let plain_pixels = compact_test_pixels(&surface);
                    let plain = clock_columns(&plain_pixels);
                    surface.player.snapshot.source = "Spotify.exe".into();
                    let shape = surface.compact_shape(&settings);
                    surface.draw(shape, scale, &settings, &clock, 0.0);
                    compact_icon_test_preview(
                        &surface,
                        shape,
                        scale,
                        &format!("clock-{format:?}-seconds-{show_seconds}"),
                    );
                    let sourced = clock_columns(&compact_test_pixels(&surface));
                    assert!(
                        plain.len().abs_diff(sourced.len()) <= 3,
                        "Time and seconds must stay complete at {scale}x, seconds={show_seconds}, format={format:?}; plain={}, sourced={}, plain_edges={:?}, sourced_edges={:?}",
                        plain.len(),
                        sourced.len(),
                        (plain.first(), plain.last()),
                        (sourced.first(), sourced.last())
                    );
                    let icon_left = (surface.logical_width() + shape.width) * 0.5 - 30.0;
                    assert!(
                        *sourced.last().expect("Readable time and seconds") as f32 / scale + 0.5
                            <= icon_left,
                        "Time and seconds need a clear gap before the source icon at {scale}x; gap={}",
                        icon_left
                            - *sourced.last().expect("Readable time and seconds") as f32 / scale
                    );
                    assert!(
                        icon_left
                            - *sourced.last().expect("Readable time and seconds") as f32 / scale
                            <= 22.0,
                        "Time and seconds must not sit too far from the source icon at {scale}x; gap={}",
                        icon_left
                            - *sourced.last().expect("Readable time and seconds") as f32 / scale
                    );
                    surface.player.snapshot.source.clear();
                    surface.draw(plain_shape, scale, &settings, &clock, 0.0);
                    assert_eq!(
                        compact_test_pixels(&surface),
                        plain_pixels,
                        "Clearing the source must restore the centered clock and seconds"
                    );
                }
            }
        }
    }

    #[test]
    fn wheel_coordinates_use_screen_origin_and_display_scale() {
        let origin = windows_sys::Win32::Foundation::POINT { x: -1700, y: -120 };
        let packed = (((480_i16 as u16 as u32) << 16) | (-1400_i16 as u16 as u32)) as isize;
        assert_eq!(super::wheel_position(packed, origin, 1.5), (200.0, 400.0));
        let packed = (((0_i16 as u16 as u32) << 16) | (-1700_i16 as u16 as u32)) as isize;
        assert_eq!(super::wheel_position(packed, origin, 2.0), (0.0, 60.0));
    }

    #[test]
    fn full_sentence_shape_fits_scaled_canvas_and_keeps_centered_input() {
        use super::*;
        for scale in [1.0, 1.25, 1.5, 2.0] {
            let mut surface = Surface::new((1920, (600.0 * scale) as u32), scale).unwrap();
            surface.player.worker = crate::media::MediaWorker::test_worker();
            surface.player.snapshot.lyrics = crate::lyrics::parse_lrc(&format!(
                "[00:00]{}",
                "Ngàn vì sao ở trên cao lấp lánh đang ngắm nhìn đôi ta bên nhau đêm nay ".repeat(3)
            ))
            .map(std::sync::Arc::new);
            surface.player.select_page(true);
            surface.player.snapshot.timeline = Some(crate::media::Timeline {
                position: 1.0,
                duration: 200.0,
                playing: false,
                rate: 1.0,
                sampled: Instant::now(),
            });
            let shape = surface.compact_shape(&Settings::default());
            assert!(shape.width > 360.0);
            assert!(
                shape.width * crate::animation::OVERSHOOT_MAX + 2.0 * crate::render::SHADOW_EXTENT
                    <= surface.logical_width()
            );
            let center = surface.logical_width() * 0.5;
            let mut animation = Animation::new();
            animation.current = shape;
            assert!(pointer_inside_current_shape(
                &animation,
                scale,
                surface.size,
                center * scale,
                20.0 * scale,
                true
            ));
            surface.player.music = true;
            let left = (surface.logical_width() - 420.0) * 0.5;
            surface
                .player
                .wheel_at(true, false, center, 300.0, left, 420.0);
            assert!(surface.player.music && surface.player.browse.is_some());
        }
    }

    #[test]
    fn wide_compact_hover_entry_survives_narrower_expansion_and_grace() {
        use super::*;
        for scale in [1.0, 1.25, 1.5, 2.0] {
            for dock in [DockAnchor::Center, DockAnchor::Left, DockAnchor::Right] {
                let mut surface = Surface::new((1920, (600.0 * scale) as u32), scale).unwrap();
                surface.player.worker = crate::media::MediaWorker::test_worker();
                let mut animation = Animation::new();
                animation.current = ShapeState {
                    width: 600.0,
                    ..crate::animation::COMPACT
                };
                animation.target = animation.current;
                let origin_for = |width: f32| POINT {
                    x: docked_x(-1920, 1920, 1920, (width * scale) as i32, dock),
                    y: -100,
                };
                let origin = origin_for(animation.current.width);
                let side = if dock == DockAnchor::Right { -1.0 } else { 1.0 };
                let entry = (960.0 + side * 280.0 * scale, 20.0 * scale);
                let mut hover = HoverTracker::new();
                assert_eq!(
                    hover.pointer_moved(surface.hover_contains(&animation, origin, entry, true)),
                    HoverCommand::Expand
                );
                animation.expand();
                for _ in 0..600 {
                    animation.advance(1.0 / 120.0);
                }
                let expanded_origin = origin_for(animation.current.width);
                let stationary = (entry.0 + (origin.x - expanded_origin.x) as f32, entry.1);
                let inside = surface.hover_contains(&animation, expanded_origin, stationary, true);
                assert!(
                    inside,
                    "expansion must retain the stationary compact entry at {scale}x {dock:?}"
                );
                assert_eq!(hover.pointer_moved(inside), HoverCommand::None);
                assert_eq!(hover.grace_expired(inside), HoverCommand::None);
                let outside = surface.hover_contains(
                    &animation,
                    expanded_origin,
                    (stationary.0, 550.0 * scale),
                    true,
                );
                assert_eq!(hover.pointer_moved(outside), HoverCommand::ArmGrace);
                assert_eq!(hover.grace_expired(outside), HoverCommand::Collapse);
            }
        }
    }

    #[test]
    fn resizing_keeps_the_selected_page_visible() {
        let mut surface = super::Surface::new((512, 600), 1.0).unwrap();
        surface.player.music = true;
        surface.page_fade.music = true;
        surface.page_fade.value = 1.0;
        surface.resize((768, 900), 1.5).unwrap();
        assert!(surface.player.music);
        assert_eq!(surface.page_fade.value, 1.0);
        assert!(!surface.page_fade.moving());
    }

    #[test]
    fn settled_playback_keeps_animation_frames_until_paused_or_hidden() {
        let mut surface = super::Surface::new((512, 600), 1.0).unwrap();
        surface.player.music = true;
        surface.player.dirty = false;
        surface.page_fade.music = true;
        surface.page_fade.value = 1.0;
        surface.player.snapshot.timeline = Some(crate::media::Timeline {
            position: 1.0,
            duration: 20.0,
            playing: true,
            rate: 1.0,
            sampled: std::time::Instant::now(),
        });
        assert!(surface.needs_animation(false, true));
        assert!(!surface.needs_animation(false, false));
        surface.player.music = false;
        assert!(!surface.needs_animation(false, true));
        surface.player.music = true;
        surface.player.snapshot.timeline.as_mut().unwrap().playing = false;
        assert!(!surface.needs_animation(false, true));
        surface.player.browse_lines(true);
        assert!(surface.needs_animation(false, true));
    }

    #[test]
    #[ignore = "Writes compact clock and timed lyric previews using the native renderer"]
    fn compact_render_preview() {
        use super::*;
        use crate::lyrics::{LyricLine, LyricWord, Lyrics};
        let dir = std::path::PathBuf::from(
            std::env::var_os("HIYODYNA_PREVIEW_DIR").expect("preview directory"),
        );
        std::fs::create_dir_all(&dir).unwrap();
        let clock = ClockSnapshot::now();
        let settings = Settings::default();
        let words = LyricLine {
            start: 0.0,
            end: 4.0,
            word_sync: true,
            words: ["Follow ", "the ", "music ", "tonight"]
                .iter()
                .enumerate()
                .map(|(i, text)| LyricWord {
                    text: (*text).into(),
                    start: i as f64,
                    end: i as f64 + 1.0,
                })
                .collect(),
        };
        let lines = crate::lyrics::parse_lrc("[00:00]Hello\n[00:04]A very long lyric line that stays fully visible inside its compact capsule without losing the final words").unwrap();
        let reference =
            crate::lyrics::parse_lrc("[00:00]But now I know a perfect way to let you go").unwrap();
        let live_line = crate::lyrics::parse_lrc("[00:00]Muốn sáng tạo").unwrap();
        let upcoming = crate::lyrics::parse_lrc("[00:10]The music starts here").unwrap();
        let live_cover = std::env::var_os("HIYODYNA_COVER_FILE")
            .map(|path| std::sync::Arc::new(image::open(path).unwrap().into_rgba8()));
        let vietnamese =
            crate::lyrics::parse_lrc("[00:00]Ngàn vì sao ở trên cao lấp lánh đang ngắm nhìn")
                .unwrap();
        let long_words = LyricLine {
            start: 0.0,
            end: 10.0,
            word_sync: true,
            words: [
                "Ngàn ",
                "vì sao ",
                "ở trên ",
                "cao ",
                "lấp lánh ",
                "đang ",
                "ngắm nhìn ",
                "đôi ta ",
                "bên nhau ",
                "đêm nay",
            ]
            .iter()
            .enumerate()
            .map(|(i, text)| LyricWord {
                text: (*text).into(),
                start: i as f64,
                end: i as f64 + 1.0,
            })
            .collect(),
        };
        for scale in [1.0, 1.25, 1.5, 2.0] {
            let mut surface = Surface::new(canvas_size(scale), scale).unwrap();
            surface.player.worker = crate::media::MediaWorker::test_worker();
            for (name, line) in [
                ("clock", None),
                ("short", Some(lines.lines[0].clone())),
                ("long", Some(lines.lines[1].clone())),
                ("words", Some(words.clone())),
                ("reference", Some(reference.lines[0].clone())),
                ("cover-warm", Some(reference.lines[0].clone())),
                ("cover-cool", Some(reference.lines[0].clone())),
                ("cover-live", Some(live_line.lines[0].clone())),
                ("vietnamese", Some(vietnamese.lines[0].clone())),
                ("word-end", Some(long_words.clone())),
                ("upcoming", Some(upcoming.lines[0].clone())),
            ] {
                surface.compact = crate::compact::Presentation::default();
                surface.player.snapshot.artwork = match name {
                    "cover-live" => live_cover.clone(),
                    "cover-warm" | "cover-cool" | "upcoming" => Some(std::sync::Arc::new(
                        // Synthetic warm/cool cover gradients verify artwork-derived glow, not UI colors.
                        image::RgbaImage::from_fn(32, 32, |x, _| {
                            let t = x as f32 / 31.0;
                            let (left, right) = if name == "cover-warm" || name == "upcoming" {
                                ([190.0, 105.0, 85.0], [125.0, 86.0, 130.0])
                            } else {
                                ([45.0, 145.0, 205.0], [120.0, 90.0, 180.0])
                            };
                            image::Rgba([
                                (left[0] * (1.0 - t) + right[0] * t) as u8,
                                (left[1] * (1.0 - t) + right[1] * t) as u8,
                                (left[2] * (1.0 - t) + right[2] * t) as u8,
                                255,
                            ])
                        }),
                    )),
                    _ => None,
                };
                surface.player.snapshot.timeline = Some(crate::media::Timeline {
                    position: if name == "long" {
                        5.0
                    } else if name == "word-end" {
                        9.5
                    } else {
                        1.6
                    },
                    duration: 20.0,
                    playing: false,
                    rate: 1.0,
                    sampled: Instant::now(),
                });
                surface.player.snapshot.lyrics = line.map(|line| {
                    std::sync::Arc::new(Lyrics {
                        lines: vec![line],
                        ..Default::default()
                    })
                });
                surface.player.select_page(name != "clock");
                surface.compact_fade = None;
                let shape = surface.compact_shape(&settings);
                surface.draw(shape, scale, &settings, &clock, 0.0);
                let width = surface.size.0;
                let height = (64.0 * scale) as u32;
                let mut pixels = unsafe {
                    std::slice::from_raw_parts(surface.bits, (width * height * 4) as usize)
                }
                .to_vec();
                for pixel in pixels.as_chunks_mut::<4>().0 {
                    pixel.swap(0, 2);
                }
                image::save_buffer(
                    dir.join(format!("compact-{name}-{}.png", (scale * 100.0) as u32)),
                    &pixels,
                    width,
                    height,
                    image::ColorType::Rgba8,
                )
                .unwrap();
                assert_eq!(surface.compact_active, name != "clock");
            }
        }
    }

    #[test]
    #[ignore = "Writes bilingual Island polish fixtures at every supported DPI"]
    fn island_polish_render_preview() {
        use super::*;
        let dir = std::env::var_os("HIYODYNA_PREVIEW_DIR")
            .map(std::path::PathBuf::from)
            .expect("preview directory");
        std::fs::create_dir_all(&dir).unwrap();
        let old_language = crate::i18n::language();
        let clock = ClockSnapshot::now();
        let lyrics = std::sync::Arc::new(crate::lyrics::parse_ttml("<tt><p begin='1s' end='4s'><span begin='1s' end='2s'>Ngàn vì sao </span><span begin='2s' end='4s'>ở trên cao</span></p><p begin='12s' end='17s'><span begin='12s' end='14s'>Chờ em quay đi, </span><span begin='14s' end='17s'>quay về bên anh</span></p></tt>").unwrap());
        for language in [
            crate::i18n::Language::English,
            crate::i18n::Language::Vietnamese,
        ] {
            crate::i18n::set_language(language);
            for scale in [1.0, 1.25, 1.5, 2.0] {
                for name in [
                    "clock",
                    "spotify-fallback",
                    "music-fallback",
                    "manual-clock",
                    "compact-disabled",
                    "countdown-visible",
                    "countdown-at-three",
                    "countdown-exit",
                    "word-lyrics",
                    "glow-later",
                    "expanded-waiting",
                    "expanded-lyrics",
                    "expanded-unavailable",
                    "expanded-clock",
                ] {
                    let mut surface = Surface::new(canvas_size(scale), scale).unwrap();
                    surface.player.worker = crate::media::MediaWorker::test_worker();
                    let mut settings = Settings {
                        language,
                        ..Settings::default()
                    };
                    settings.compact_lyrics = name != "compact-disabled";
                    let source = match name {
                        "clock" => "",
                        "music-fallback" => "Media Player",
                        _ => "Spotify.exe",
                    };
                    let at = match name {
                        "countdown-visible" => 8.5,
                        "countdown-at-three" => 9.0,
                        "countdown-exit" => 9.1,
                        "expanded-waiting" => 8.0,
                        _ => 13.2,
                    };
                    let music = !matches!(name, "clock" | "manual-clock" | "expanded-clock");
                    surface.player.select_page(music);
                    surface.page_fade.music = music;
                    surface.page_fade.value = if music { 1.0 } else { 0.0 };
                    surface.player.snapshot = crate::media::MediaSnapshot {
                        source: source.into(),
                        key: "polish-fixture".into(),
                        title: "Ngủ Quên".into(),
                        artist: "Wren Evans".into(),
                        can_toggle: true,
                        can_seek: true,
                        can_next: true,
                        can_previous: true,
                        lyrics: (!matches!(
                            name,
                            "clock"
                                | "spotify-fallback"
                                | "music-fallback"
                                | "expanded-unavailable"
                        ))
                        .then(|| lyrics.clone()),
                        timeline: Some(crate::media::Timeline {
                            position: at,
                            duration: 154.0,
                            playing: false,
                            rate: 1.0,
                            sampled: Instant::now(),
                        }),
                        // Synthetic cover gradients keep Island previews independent of theme accents.
                        artwork: Some(std::sync::Arc::new(image::RgbaImage::from_fn(
                            64,
                            64,
                            |x, y| {
                                image::Rgba([40 + (x * 2) as u8, 75 + y as u8, 180 - x as u8, 255])
                            },
                        ))),
                        ..Default::default()
                    };
                    if name == "glow-later" {
                        surface.cover_glow.advance(9.0, true);
                    }
                    let shape = if name.starts_with("expanded-") {
                        ShapeState {
                            height: if music {
                                500.0
                            } else {
                                crate::animation::EXPANDED.height
                            },
                            ..crate::animation::EXPANDED
                        }
                    } else {
                        surface.compact_shape(&settings)
                    };
                    surface.draw(shape, scale, &settings, &clock, 0.0);
                    let height = ((shape.height + 24.0) * scale).ceil() as u32;
                    let width = surface.size.0;
                    let mut pixels = unsafe {
                        std::slice::from_raw_parts(surface.bits, (width * height * 4) as usize)
                    }
                    .to_vec();
                    for pixel in pixels.as_chunks_mut::<4>().0 {
                        pixel.swap(0, 2);
                    }
                    image::save_buffer(
                        dir.join(format!(
                            "polish-{}-{name}-{}.png",
                            language.as_str(),
                            (scale * 100.0) as u32
                        )),
                        &pixels,
                        width,
                        height,
                        image::ColorType::Rgba8,
                    )
                    .unwrap();
                }
            }
        }
        crate::i18n::set_language(old_language);
    }

    #[test]
    #[ignore = "Measures full native software rendering during page transitions in release mode"]
    fn transition_render_benchmark() {
        use super::*;
        for scale in [1.0, 1.5, 2.0] {
            let mut surface = Surface::new(canvas_size(scale), scale).unwrap();
            let settings = Settings::default();
            let clock = ClockSnapshot::now();
            let mut animation = Animation::new_with_tuning(settings.tuning());
            animation.current = crate::animation::EXPANDED;
            animation.target = ShapeState {
                height: 500.0,
                ..crate::animation::EXPANDED
            };
            surface.player.music = true;
            surface.player.snapshot = crate::media::MediaSnapshot {
                key: "benchmark".into(),
                title: "Blinding Lights".into(),
                artist: "The Weeknd".into(),
                lyrics: crate::lyrics::parse_lrc(
                    "[00:01]First line\n[00:02]Second line\n[00:03]Third line",
                )
                .map(std::sync::Arc::new),
                ..Default::default()
            };
            let mut samples = Vec::new();
            for _ in 0..45 {
                animation.advance(1.0 / 60.0);
                let started = Instant::now();
                surface.draw(animation.current, scale, &settings, &clock, 1.0);
                samples.push(started.elapsed().as_secs_f64() * 1000.0);
            }
            let average = samples.iter().sum::<f64>() / samples.len() as f64;
            samples.sort_by(f64::total_cmp);
            println!(
                "{scale}x full transition render: mean {average:.2} ms, p95 {:.2} ms, max {:.2} ms (excludes presentation)",
                samples[42], samples[44]
            );
            let mut phases = [0.0_f64; 3];
            for _ in 0..30 {
                let params = surface.background_params.unwrap();
                let start = Instant::now();
                render_frame_with_margin(&mut surface.background, surface.size, params);
                phases[0] += start.elapsed().as_secs_f64() * 1000.0;
                let mut pixels = surface.background.clone();
                let start = Instant::now();
                surface.text.draw_with_opacity(
                    &mut pixels,
                    TextParams {
                        size: surface.size,
                        state: crate::animation::EXPANDED,
                        top_margin: 0.0,
                        scale,
                        accent: settings.accent_bgra(),
                        show_seconds: true,
                        hover: 1.0,
                        compact_trailing_space: 0.0,
                    },
                    0.5,
                );
                phases[1] += start.elapsed().as_secs_f64() * 1000.0;
                let start = Instant::now();
                surface.player.draw(
                    &mut Canvas::new(&mut pixels, surface.size, &mut surface.painter),
                    (surface.size.0 as f32 / scale - 420.0) * 0.5,
                    420.0,
                    500.0,
                    &settings,
                );
                phases[2] += start.elapsed().as_secs_f64() * 1000.0;
            }
            println!(
                "{scale}x phases: background {:.2}, clock {:.2}, player {:.2} ms",
                phases[0] / 30.0,
                phases[1] / 30.0,
                phases[2] / 30.0
            );
        }
    }

    #[test]
    #[ignore = "Checks Reset conflict feedback using isolated settings and native hotkey registrations"]
    fn native_reset_shortcut_conflict_preserves_binding_and_explains_failure() {
        use super::*;
        let isolated = std::env::var_os("HIYODYNA_NATIVE_TEST_APPDATA")
            .expect("Set an isolated native-test APPDATA directory");
        assert_eq!(std::env::var_os("APPDATA"), Some(isolated));
        unsafe {
            let first = CreateWindowExW(
                0,
                wide("STATIC").as_ptr(),
                null(),
                WS_POPUP,
                0,
                0,
                20,
                20,
                null_mut(),
                null_mut(),
                GetModuleHandleW(null()),
                null(),
            );
            let owner = CreateWindowExW(
                WS_EX_LAYERED | WS_EX_TOOLWINDOW | WS_EX_NOACTIVATE,
                wide("STATIC").as_ptr(),
                null(),
                WS_POPUP,
                0,
                0,
                20,
                20,
                null_mut(),
                null_mut(),
                GetModuleHandleW(null()),
                null(),
            );
            assert!(!first.is_null() && !owner.is_null());
            struct Cleanup(HWND, HWND, crate::i18n::Language);
            impl Drop for Cleanup {
                fn drop(&mut self) {
                    unsafe {
                        let popup = FindWindowW(wide("HiyoDynaPreferences").as_ptr(), null());
                        if !popup.is_null() {
                            DestroyWindow(popup);
                        }
                        DestroyWindow(self.1);
                        DestroyWindow(self.0);
                    }
                    crate::i18n::set_language(self.2);
                }
            }
            let _cleanup = Cleanup(first, owner, crate::i18n::language());
            let _default_guard =
                HotkeyGuard::register(first, crate::hotkey::Shortcut::default(), None);
            let previous = (0x75..=0x7a)
                .find_map(|key| {
                    HotkeyGuard::register(
                        owner,
                        crate::hotkey::Shortcut { modifiers: 7, key },
                        None,
                    )
                })
                .expect("No test shortcut was available");
            let shortcut = previous.shortcut;
            let mut settings = Settings {
                shortcut,
                language: crate::i18n::Language::English,
                ..Settings::default()
            };
            crate::i18n::set_language(settings.language);
            let mut surface = Surface::new(canvas_size(1.0), 1.0).unwrap();
            surface.player.worker = crate::media::MediaWorker::test_worker();
            surface.hotkey = Some(previous);
            surface.shortcut_available = true;
            crate::preferences::show(owner, &settings, true).unwrap();
            let mut animation = Animation::new_with_tuning(settings.tuning());
            let mut animation_timer = AnimationTimer::new(owner);
            let mut position = POINT::default();
            let mut exit_action = ExitAction::Exit;
            apply_tray_command(
                owner,
                TrayCommand::Reset,
                &mut settings,
                &mut animation,
                &mut animation_timer,
                &mut surface,
                None,
                1.0,
                &mut position,
                &ClockSnapshot::now(),
                &mut exit_action,
            )
            .unwrap();
            assert_eq!(settings.shortcut, shortcut);
            assert_eq!(Settings::load().shortcut, shortcut);
            assert!(
                HotkeyGuard::register(first, shortcut, None).is_none(),
                "Reset must retain the previous effective binding after conflict"
            );
            let popup = FindWindowW(wide("HiyoDynaPreferences").as_ptr(), null());
            let feedback = FindWindowExW(
                popup,
                null_mut(),
                wide("STATIC").as_ptr(),
                wide(crate::i18n::tr(
                    "This shortcut is unavailable. Your previous shortcut is unchanged.",
                ))
                .as_ptr(),
            );
            assert!(
                !feedback.is_null(),
                "Reset must disclose the shortcut conflict in Settings"
            );
        }
    }

    #[test]
    #[ignore = "Uses isolated native hotkey registrations to verify real conflict rollback"]
    fn native_shortcut_conflict_keeps_the_previous_binding_registered() {
        use super::*;
        unsafe {
            let windows: Vec<HWND> = (0..3)
                .map(|_| {
                    CreateWindowExW(
                        0,
                        wide("STATIC").as_ptr(),
                        std::ptr::null(),
                        WS_POPUP,
                        0,
                        0,
                        20,
                        20,
                        null_mut(),
                        null_mut(),
                        GetModuleHandleW(std::ptr::null()),
                        std::ptr::null(),
                    )
                })
                .collect();
            struct Cleanup(Vec<HWND>);
            impl Drop for Cleanup {
                fn drop(&mut self) {
                    for hwnd in &self.0 {
                        unsafe {
                            DestroyWindow(*hwnd);
                        }
                    }
                }
            }
            assert!(windows.iter().all(|hwnd| !hwnd.is_null()));
            let cleanup = Cleanup(windows);
            let reserve = |hwnd| {
                (0x75..=0x7a).find_map(|key| {
                    HotkeyGuard::register(hwnd, crate::hotkey::Shortcut { modifiers: 7, key }, None)
                })
            };
            let first = reserve(cleanup.0[0]).expect("No test shortcut was available");
            let previous = reserve(cleanup.0[1]).expect("No second test shortcut was available");
            assert_ne!(first.shortcut, previous.shortcut);
            assert!(HotkeyGuard::register(cleanup.0[1], first.shortcut, Some(&previous)).is_none());
            assert!(
                HotkeyGuard::register(cleanup.0[2], previous.shortcut, None).is_none(),
                "A rejected replacement must leave the original native binding reserved"
            );
            let shortcut = previous.shortcut;
            drop(previous);
            let released = HotkeyGuard::register(cleanup.0[2], shortcut, None)
                .expect("Dropping the old guard must release only its own binding");
            drop(released);
            drop(first);
        }
    }

    #[test]
    #[ignore = "Opens a separate native Island and checks UI Automation; requires an interactive desktop"]
    fn native_island_smoke() {
        use super::*;
        unsafe {
            let class_name = wide("HiyoDynaSmoke");
            let instance = GetModuleHandleW(null());
            let class = WNDCLASSW {
                lpfnWndProc: Some(window_proc),
                hInstance: instance,
                lpszClassName: class_name.as_ptr(),
                hCursor: LoadCursorW(null_mut(), IDC_ARROW),
                ..Default::default()
            };
            assert_ne!(RegisterClassW(&class), 0);
            let hwnd = CreateWindowExW(
                WS_EX_LAYERED | WS_EX_TOOLWINDOW | WS_EX_NOACTIVATE | WS_EX_TOPMOST,
                class_name.as_ptr(),
                wide("HiyoDyna smoke").as_ptr(),
                WS_POPUP,
                0,
                0,
                560,
                600,
                null_mut(),
                null_mut(),
                instance,
                null(),
            );
            assert!(!hwnd.is_null());
            let handle = hwnd as usize;
            let driver = std::thread::spawn(move || {
                let hwnd = handle as HWND;
                struct Cleanup(HWND, POINT);
                impl Drop for Cleanup {
                    fn drop(&mut self) {
                        unsafe {
                            SetCursorPos(self.1.x, self.1.y);
                            PostMessageW(self.0, WM_CLOSE, 0, 0);
                        }
                    }
                }
                let mut old = POINT::default();
                GetCursorPos(&mut old);
                let _cleanup = Cleanup(hwnd, old);
                let dir = std::env::var_os("HIYODYNA_PREVIEW_DIR")
                    .map(std::path::PathBuf::from)
                    .expect("preview directory");
                std::fs::create_dir_all(&dir).unwrap();
                use windows::Win32::System::Com::{
                    CLSCTX_INPROC_SERVER, COINIT_MULTITHREADED, CoCreateInstance, CoInitializeEx,
                    CoUninitialize,
                };
                use windows::Win32::UI::Accessibility::{
                    CUIAutomation, IUIAutomation, IUIAutomationInvokePattern, TreeScope_Children,
                    UIA_InvokePatternId,
                };
                use windows::core::Interface;
                CoInitializeEx(None, COINIT_MULTITHREADED).ok().unwrap();
                let automation: IUIAutomation =
                    CoCreateInstance(&CUIAutomation, None, CLSCTX_INPROC_SERVER).unwrap();
                let root = automation
                    .ElementFromHandle(windows::Win32::Foundation::HWND(hwnd))
                    .unwrap();
                let find_named = |name: &str| {
                    let condition = automation
                        .CreatePropertyCondition(
                            windows::Win32::UI::Accessibility::UIA_NamePropertyId,
                            &windows::Win32::System::Variant::VARIANT::from(name),
                        )
                        .unwrap();
                    let start = Instant::now();
                    loop {
                        if let Ok(node) = root.FindFirst(TreeScope_Children, &condition) {
                            return node;
                        }
                        assert!(
                            start.elapsed().as_secs() < 12,
                            "UI Automation control missing: {name}"
                        );
                        std::thread::park_timeout(std::time::Duration::from_millis(50));
                    }
                };
                PostMessageW(hwnd, WM_KEYDOWN, 34, 0);
                let music = find_named("Music page");
                music.SetFocus().unwrap();
                let invoke: IUIAutomationInvokePattern = music
                    .GetCurrentPattern(UIA_InvokePatternId)
                    .unwrap()
                    .cast()
                    .unwrap();
                invoke.Invoke().unwrap();
                let page = find_named("Clock page");
                find_named("Settings");
                let start = Instant::now();
                let mut stable = 0;
                let mut diagnosed = false;
                while stable < 5 {
                    let dot_top = page.CurrentBoundingRectangle().unwrap().top as f32;
                    let expected_top = player_shape(hwnd, window_scale(hwnd)).height * 0.5 - 23.0;
                    let mut window_rect = windows_sys::Win32::Foundation::RECT::default();
                    GetWindowRect(hwnd, &mut window_rect);
                    stable =
                        if (dot_top - window_rect.top as f32 - expected_top * window_scale(hwnd))
                            .abs()
                            < 1.0
                        {
                            stable + 1
                        } else {
                            0
                        };
                    if !diagnosed && start.elapsed().as_secs() >= 2 {
                        diagnosed = true;
                        eprintln!(
                            "Island settling: dot_top={dot_top}, window_top={}, expected_top={expected_top}, scale={}, root={}",
                            window_rect.top,
                            window_scale(hwnd),
                            root.CurrentName().unwrap()
                        );
                        crate::preferences::tests::save_window(
                            hwnd,
                            &dir.join("island-settle-diagnostic.png"),
                        );
                    }
                    assert!(
                        start.elapsed().as_secs() < 12,
                        "player animation did not settle"
                    );
                    std::thread::park_timeout(std::time::Duration::from_millis(50));
                }
                crate::preferences::tests::save_window(hwnd, &dir.join("island-native.png"));
                let mut lyric_point = POINT {
                    x: (canvas_size(window_scale(hwnd)).0 as f32 * 0.5) as i32,
                    y: (player_shape(hwnd, window_scale(hwnd)).height * 0.75 * window_scale(hwnd))
                        as i32,
                };
                assert_ne!(
                    windows_sys::Win32::Graphics::Gdi::ClientToScreen(hwnd, &mut lyric_point),
                    0
                );
                assert_ne!(SetCursorPos(lyric_point.x, lyric_point.y), 0);
                use windows_sys::Win32::UI::Input::KeyboardAndMouse::MOUSEEVENTF_WHEEL;
                for delta in [120_i32, -120] {
                    let input = INPUT {
                        r#type: INPUT_MOUSE,
                        Anonymous: INPUT_0 {
                            mi: MOUSEINPUT {
                                mouseData: delta as u32,
                                dwFlags: MOUSEEVENTF_WHEEL,
                                ..Default::default()
                            },
                        },
                    };
                    assert_eq!(SendInput(1, &input, size_of::<INPUT>() as i32), 1);
                    let started = Instant::now();
                    while started.elapsed().as_millis() < 500 {
                        assert!(
                            root.CurrentName().unwrap() != "HiyoDyna clock",
                            "wheel over lyrics must not navigate to Clock"
                        );
                        let bounds = page.CurrentBoundingRectangle().unwrap();
                        assert!(
                            bounds.bottom > bounds.top,
                            "player must remain expanded while scrolling lyrics"
                        );
                        std::thread::park_timeout(std::time::Duration::from_millis(20));
                    }
                }
                PostMessageW(hwnd, WM_MOUSEWHEEL, ((120_usize) << 16) | 0x0008, 0);
                std::thread::park_timeout(std::time::Duration::from_millis(100));
                assert!(
                    root.CurrentName().unwrap() != "HiyoDyna clock",
                    "Ctrl+wheel must browse without changing pages"
                );
                let nodes = root
                    .FindAll(
                        TreeScope_Children,
                        &automation.CreateTrueCondition().unwrap(),
                    )
                    .unwrap();
                let mut found = false;
                let mut settings = None;
                let mut source_picker = None;
                for i in 0..nodes.Length().unwrap() {
                    let node = nodes.GetElement(i).unwrap();
                    let name = node.CurrentName().unwrap().to_string();
                    println!("Island UIA: {name}");
                    found |= name == "Clock page";
                    if name == "Settings" {
                        settings = Some(node);
                    } else if name == "Choose media source" {
                        source_picker = Some(node);
                    }
                }
                assert!(found, "player accessibility fragments must be exposed");
                if let Some(picker) = source_picker {
                    let invoke_node =
                        |node: &windows::Win32::UI::Accessibility::IUIAutomationElement| {
                            let pattern: IUIAutomationInvokePattern = node
                                .GetCurrentPattern(UIA_InvokePatternId)
                                .unwrap()
                                .cast()
                                .unwrap();
                            pattern.Invoke().unwrap();
                        };
                    invoke_node(&picker);
                    invoke_node(&find_named("Clock page"));
                    let started = Instant::now();
                    while root.CurrentName().unwrap() != "HiyoDyna clock" {
                        assert!(
                            started.elapsed().as_secs() < 5,
                            "Clock dot must leave the source picker"
                        );
                        std::thread::park_timeout(std::time::Duration::from_millis(20));
                    }
                    PostMessageW(hwnd, WM_MOUSEWHEEL, (-120_i16 as u16 as usize) << 16, 0);
                    let started = Instant::now();
                    while root.CurrentName().unwrap() == "HiyoDyna clock" {
                        assert!(
                            started.elapsed().as_secs() < 5,
                            "Hidden picker must not consume page scrolling"
                        );
                        std::thread::park_timeout(std::time::Duration::from_millis(20));
                    }
                }
                PostMessageW(hwnd, WM_RBUTTONUP, 0, 0);
                let started = Instant::now();
                let menu = loop {
                    let candidate = FindWindowW(wide("#32768").as_ptr(), null());
                    let mut pid = 0;
                    GetWindowThreadProcessId(candidate, &mut pid);
                    if !candidate.is_null() && pid == std::process::id() {
                        break candidate;
                    }
                    assert!(
                        started.elapsed().as_secs() < 3,
                        "right-click must open a quick menu"
                    );
                    std::thread::park_timeout(std::time::Duration::from_millis(20));
                };
                assert!(
                    FindWindowW(wide("HiyoDynaPreferences").as_ptr(), null()).is_null(),
                    "right-click must not open Settings directly"
                );
                let handle = SendMessageW(menu, 0x01E1, 0, 0) as HMENU;
                assert!(!handle.is_null());
                assert_eq!(GetMenuItemCount(handle), 9);
                let mut info = MENUINFO {
                    cbSize: std::mem::size_of::<MENUINFO>() as u32,
                    fMask: MIM_STYLE,
                    ..Default::default()
                };
                assert_ne!(GetMenuInfo(handle, &mut info), 0);
                assert_ne!(
                    info.dwStyle & MNS_MODELESS,
                    0,
                    "menu must keep the main message loop running"
                );
                let mut item = windows_sys::Win32::Foundation::RECT::default();
                assert_ne!(GetMenuItemRect(hwnd, handle, 0, &mut item), 0);
                let point = POINT {
                    x: (item.left + item.right) / 2,
                    y: (item.top + item.bottom) / 2,
                };
                struct RestoreCursor(POINT);
                impl Drop for RestoreCursor {
                    fn drop(&mut self) {
                        unsafe {
                            SetCursorPos(self.0.x, self.0.y);
                        }
                    }
                }
                let mut saved_cursor = POINT::default();
                assert_ne!(GetCursorPos(&mut saved_cursor), 0);
                let _restore_cursor = RestoreCursor(saved_cursor);
                assert_ne!(SetCursorPos(point.x, point.y), 0);
                let started = Instant::now();
                while WindowFromPoint(point) != menu || IsWindowVisible(menu) == 0 {
                    assert!(
                        started.elapsed().as_secs() < 3,
                        "native menu must own the click point before input"
                    );
                    std::thread::park_timeout(std::time::Duration::from_millis(10));
                }
                use windows_sys::Win32::UI::Input::KeyboardAndMouse::{
                    INPUT, INPUT_0, INPUT_MOUSE, MOUSEEVENTF_LEFTDOWN, MOUSEEVENTF_LEFTUP,
                    MOUSEINPUT, SendInput,
                };
                let inputs = [MOUSEEVENTF_LEFTDOWN, MOUSEEVENTF_LEFTUP].map(|dw_flags| INPUT {
                    r#type: INPUT_MOUSE,
                    Anonymous: INPUT_0 {
                        mi: MOUSEINPUT {
                            dwFlags: dw_flags,
                            ..Default::default()
                        },
                    },
                });
                let sent = SendInput(2, inputs.as_ptr(), size_of::<INPUT>() as i32);
                assert_eq!(
                    sent,
                    2,
                    "native mouse injection error {}",
                    windows_sys::Win32::Foundation::GetLastError()
                );
                let started = Instant::now();
                while root.CurrentName().unwrap() != "HiyoDyna clock" {
                    assert!(
                        started.elapsed().as_secs() < 3,
                        "selecting Clock in the native menu must switch pages"
                    );
                    std::thread::park_timeout(std::time::Duration::from_millis(20));
                }
                PostMessageW(hwnd, WM_RBUTTONUP, 0, 0);
                let started = Instant::now();
                loop {
                    let candidate = FindWindowW(wide("#32768").as_ptr(), null());
                    let mut pid = 0;
                    GetWindowThreadProcessId(candidate, &mut pid);
                    if !candidate.is_null() && pid == std::process::id() {
                        break;
                    }
                    assert!(
                        started.elapsed().as_secs() < 3,
                        "quick menu must reopen after selecting an action"
                    );
                    std::thread::park_timeout(std::time::Duration::from_millis(20));
                }
                PostMessageW(hwnd, WM_HOTKEY, HOTKEY_PASSTHROUGH as usize, 0);
                let started = Instant::now();
                while GetWindowLongPtrW(hwnd, GWL_EXSTYLE) as u32 & WS_EX_TRANSPARENT == 0 {
                    assert!(
                        started.elapsed().as_secs() < 3,
                        "shortcut must work while the quick menu is open"
                    );
                    std::thread::park_timeout(std::time::Duration::from_millis(20));
                }
                SendMessageW(hwnd, WM_CANCELMODE, 0, 0);
                PostMessageW(hwnd, WM_HOTKEY, HOTKEY_PASSTHROUGH as usize, 0);
                let started = Instant::now();
                while GetWindowLongPtrW(hwnd, GWL_EXSTYLE) as u32 & WS_EX_TRANSPARENT != 0 {
                    assert!(
                        started.elapsed().as_secs() < 3,
                        "shortcut must restore normal interaction"
                    );
                    std::thread::park_timeout(std::time::Duration::from_millis(20));
                }
                let invoke: IUIAutomationInvokePattern = settings
                    .unwrap()
                    .GetCurrentPattern(UIA_InvokePatternId)
                    .unwrap()
                    .cast()
                    .unwrap();
                invoke.Invoke().unwrap();
                std::thread::park_timeout(std::time::Duration::from_millis(700));
                let popup = FindWindowW(wide("HiyoDynaPreferences").as_ptr(), null());
                assert!(!popup.is_null(), "gear must open settings");
                assert_ne!(
                    GetWindowLongW(popup, GWL_STYLE) as u32 & WS_CAPTION,
                    0,
                    "settings must have a native caption"
                );
                assert_ne!(
                    GetWindowLongW(popup, GWL_STYLE) as u32 & WS_THICKFRAME,
                    0,
                    "settings must resize"
                );
                PostMessageW(hwnd, WM_HOTKEY, HOTKEY_PASSTHROUGH as usize, 0);
                let started = Instant::now();
                while GetWindowLongPtrW(hwnd, GWL_EXSTYLE) as u32 & WS_EX_TRANSPARENT == 0 {
                    assert!(
                        started.elapsed().as_secs() < 3,
                        "shortcut must enable click-through"
                    );
                    std::thread::park_timeout(std::time::Duration::from_millis(20));
                }
                assert_eq!(
                    SendMessageW(hwnd, WM_NCHITTEST, 0, 0),
                    HTTRANSPARENT as isize
                );
                std::thread::park_timeout(std::time::Duration::from_millis(250));
                assert_ne!(
                    IsWindowVisible(popup),
                    0,
                    "modeless settings must stay visible"
                );
                crate::preferences::tests::save_window(hwnd, &dir.join("island-click-through.png"));
                PostMessageW(hwnd, WM_HOTKEY, HOTKEY_PASSTHROUGH as usize, 0);
                let started = Instant::now();
                while GetWindowLongPtrW(hwnd, GWL_EXSTYLE) as u32 & WS_EX_TRANSPARENT != 0 {
                    assert!(
                        started.elapsed().as_secs() < 3,
                        "shortcut must restore interaction"
                    );
                    std::thread::park_timeout(std::time::Duration::from_millis(20));
                }
                PostMessageW(popup, WM_KEYDOWN, 27, 0);
                PostMessageW(hwnd, WM_KEYDOWN, 33, 0);
                find_named("Settings");
                let fixture = crate::fullscreen::tests::Fixture::new();
                use windows_sys::Win32::Graphics::Gdi::{
                    GetMonitorInfoW, MONITOR_DEFAULTTONEAREST, MONITORINFO, MonitorFromWindow,
                };
                let mut monitor = MONITORINFO {
                    cbSize: size_of::<MONITORINFO>() as u32,
                    ..Default::default()
                };
                assert_ne!(
                    GetMonitorInfoW(
                        MonitorFromWindow(hwnd, MONITOR_DEFAULTTONEAREST),
                        &mut monitor
                    ),
                    0
                );
                let bounds = monitor.rcMonitor;
                SetForegroundWindow(fixture.hwnd);
                assert_ne!(SetCursorPos(bounds.right - 30, bounds.bottom - 30), 0);
                // Queue a pointer-outside check immediately before fullscreen detection,
                // reproducing entry while a collapse grace period is pending.
                assert_ne!(
                    SetWindowPos(
                        fixture.hwnd,
                        HWND_TOP,
                        bounds.left,
                        bounds.top,
                        bounds.right - bounds.left,
                        bounds.bottom - bounds.top,
                        SWP_SHOWWINDOW
                    ),
                    0
                );
                SetForegroundWindow(fixture.hwnd);
                let started = Instant::now();
                while GetForegroundWindow() != fixture.hwnd {
                    assert!(
                        started.elapsed().as_secs() < 3,
                        "fullscreen fixture must own foreground"
                    );
                    std::thread::park_timeout(std::time::Duration::from_millis(10));
                }
                PostMessageW(hwnd, WM_TIMER, TIMER_ID_HOVER_RECHECK, 0);
                PostMessageW(hwnd, WM_TIMER, TIMER_ID_MEDIA, 0);
                let started = Instant::now();
                while GetWindowLongPtrW(hwnd, GWL_EXSTYLE) as u32 & WS_EX_TRANSPARENT == 0 {
                    assert!(
                        started.elapsed().as_secs() < 3,
                        "fullscreen must disable Island interaction"
                    );
                    std::thread::park_timeout(std::time::Duration::from_millis(10));
                }
                assert!(
                    !root.CurrentIsKeyboardFocusable().unwrap().as_bool(),
                    "hidden Island must not advertise keyboard focus"
                );
                // Check effective native focus below, including any UIA host fallback.
                // Direct provider error contracts are covered by accessibility unit tests.
                let _ = root.SetFocus();
                let _ = music.SetFocus();
                PostMessageW(hwnd, WM_KEYDOWN, 34, 0);
                PostMessageW(hwnd, WM_TIMER, TIMER_ID_HOVER_GRACE, 0);
                std::thread::park_timeout(std::time::Duration::from_millis(400));
                assert_eq!(
                    GetForegroundWindow(),
                    fixture.hwnd,
                    "hidden actions must not steal fullscreen focus"
                );
                assert!(
                    !root.CurrentHasKeyboardFocus().unwrap().as_bool(),
                    "hidden Island must not acquire keyboard focus"
                );
                assert_ne!(
                    SetWindowPos(
                        fixture.hwnd,
                        null_mut(),
                        bounds.left + 40,
                        bounds.top + 80,
                        400,
                        300,
                        SWP_NOZORDER
                    ),
                    0
                );
                let started = Instant::now();
                while GetWindowLongPtrW(hwnd, GWL_EXSTYLE) as u32 & WS_EX_TRANSPARENT != 0 {
                    assert!(
                        started.elapsed().as_secs() < 3,
                        "leaving fullscreen must restore interaction"
                    );
                    std::thread::park_timeout(std::time::Duration::from_millis(10));
                }
                // Let the reveal finish before testing compact state: the reveal's
                // temporary narrow shape also exposes a single compact fragment.
                std::thread::park_timeout(std::time::Duration::from_millis(900));
                let started = Instant::now();
                loop {
                    let children = root
                        .FindAll(
                            TreeScope_Children,
                            &automation.CreateTrueCondition().unwrap(),
                        )
                        .unwrap();
                    if children.Length().unwrap() == 1 {
                        break;
                    }
                    assert!(
                        started.elapsed().as_secs() < 4,
                        "restored Island must rearm pending collapse without pointer reentry"
                    );
                    std::thread::park_timeout(std::time::Duration::from_millis(20));
                }
                assert!(
                    root.CurrentIsKeyboardFocusable().unwrap().as_bool(),
                    "restored Island must support accessibility focus"
                );
                let mut center = POINT {
                    x: (canvas_size(window_scale(hwnd)).0 / 2) as i32,
                    y: (12.0 * window_scale(hwnd)) as i32,
                };
                let client = center;
                assert_ne!(
                    windows_sys::Win32::Graphics::Gdi::ClientToScreen(hwnd, &mut center),
                    0
                );
                assert_ne!(SetCursorPos(center.x, center.y), 0);
                PostMessageW(
                    hwnd,
                    WM_MOUSEMOVE,
                    0,
                    ((client.y as u32) << 16 | client.x as u32) as isize,
                );
                find_named("Settings");
                assert_eq!(
                    root.CurrentName().unwrap(),
                    "HiyoDyna clock",
                    "hidden key input must preserve the selected page"
                );
                drop(fixture);
                CoUninitialize();
            });
            let result = run_loop(hwnd);
            driver.join().unwrap();
            result.unwrap();
            if IsWindow(hwnd) != 0 {
                DestroyWindow(hwnd);
            }
            UnregisterClassW(class_name.as_ptr(), instance);
        }
    }

    #[test]
    fn centers_window_inside_primary_monitor_bounds() {
        assert_eq!(centered_x(0, 1920, 560), 680);
        assert_eq!(centered_x(-1920, 1920, 560), -1240);
    }

    #[test]
    fn docking_keeps_visible_edges_fixed_while_lyrics_change_width() {
        use crate::settings::DockAnchor;
        for width in [148, 220, 360, 420] {
            let padding = (512 - width) / 2;
            let left = super::docked_x(-1920, 1920, 512, width, DockAnchor::Left);
            assert_eq!(left + padding, -1920);
            let right = super::docked_x(-1920, 1920, 512, width, DockAnchor::Right);
            assert_eq!(right + padding + width, 0);
            assert_eq!(
                super::docked_x(-1920, 1920, 512, width, DockAnchor::Center),
                -1216
            );
        }
    }

    #[test]
    fn scales_geometry() {
        let scaled = scale_shape(COMPACT, 1.5);

        assert_eq!(scaled.width, 222.0);
        assert_eq!(scaled.height, 60.0);
        assert_eq!(scaled.radius, 30.0);
    }

    #[test]
    fn modal_dispatch_clears_the_pending_animation_frame() {
        super::ANIMATION_FRAME_PENDING.store(true, Ordering::Release);
        super::ANIMATION_FRAME_REQUESTED.store(true, Ordering::Release);

        // SAFETY: this branch only clears atomics and does not touch the null HWND.
        unsafe {
            super::window_proc(std::ptr::null_mut(), super::WM_APP_ANIMATION_FRAME, 0, 0);
        }

        assert!(!super::ANIMATION_FRAME_PENDING.load(Ordering::Acquire));
        assert!(!super::ANIMATION_FRAME_REQUESTED.load(Ordering::Acquire));
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
    fn animation_timer_follows_the_display_interval() {
        use std::time::{Duration, Instant};

        for (interval_ms, expected_frames) in [(16_u64, 62), (7, 142)] {
            let start = Instant::now();
            let mut last_frame = start;
            let mut frames = 0;
            let mut total_elapsed = 0.0_f32;
            for ms in (interval_ms..=1000).step_by(interval_ms as usize) {
                let now = start + Duration::from_millis(ms);
                let elapsed = super::take_animation_frame(now, last_frame)
                    .expect("each timer tick must produce a frame");
                frames += 1;
                total_elapsed += elapsed;
                last_frame = now;
            }

            assert_eq!(frames, expected_frames, "{interval_ms} ms ticks");
            assert!(
                (0.9..=1.01).contains(&total_elapsed),
                "{interval_ms} ms ticks advanced {total_elapsed} s"
            );
        }
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
    fn unlimited_fps_can_be_toggled_while_idle() {
        let mut timer = super::AnimationTimer::new(std::ptr::null_mut());
        assert!(!timer.unlimited_fps());
        timer.set_unlimited(true).unwrap();
        assert!(timer.unlimited_fps());
        timer.set_unlimited(true).unwrap();
        assert!(timer.unlimited_fps());
        timer.set_unlimited(false).unwrap();
        assert!(!timer.unlimited_fps());
    }

    #[test]
    fn animation_timer_coalesces_wakeups_while_the_ui_is_busy() {
        use std::sync::atomic::AtomicBool;

        let pending = AtomicBool::new(false);
        let requested = AtomicBool::new(false);
        assert!(super::queue_animation_frame(&pending, &requested));
        assert!(!super::queue_animation_frame(&pending, &requested));
        assert!(requested.load(std::sync::atomic::Ordering::Acquire));
        assert!(super::finish_animation_frame(&pending, &requested));
        assert!(!requested.load(std::sync::atomic::Ordering::Acquire));
        assert!(!super::finish_animation_frame(&pending, &requested));
        assert!(super::queue_animation_frame(&pending, &requested));
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
