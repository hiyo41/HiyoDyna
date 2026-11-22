use crate::canvas::{Canvas, MUTED, Painter, Rect, WHITE};
use crate::clock::ClockFormat;
use crate::hotkey::Shortcut;
use crate::i18n::{Language, tr};
use crate::native_ui::{FramePump, SmoothScroll, WindowSurface, blit, trace_start, trace_ui};
use crate::settings::{
    AccentPreset, BouncePreset, DockAnchor, LyricsMode, MotionPreset, Settings, UpdateChannel,
};
use crate::tray::TrayCommand;
use std::cell::{Cell, RefCell};
use std::mem::size_of;
use std::ptr::{null, null_mut};
use std::time::Instant;
use windows_sys::Win32::Foundation::{HWND, LPARAM, LRESULT, POINT, RECT, WPARAM};
use windows_sys::Win32::Graphics::Gdi::*;
use windows_sys::Win32::System::LibraryLoader::GetModuleHandleW;
use windows_sys::Win32::UI::Controls::{
    DRAWITEMSTRUCT, ODA_DRAWENTIRE, ODS_FOCUS, ODS_HOTLIGHT, ODS_SELECTED, ODT_BUTTON,
};
use windows_sys::Win32::UI::Input::KeyboardAndMouse::{GetFocus, GetKeyState, SetFocus, VK_SHIFT};
use windows_sys::Win32::UI::Shell::{DefSubclassProc, RemoveWindowSubclass, SetWindowSubclass};
use windows_sys::Win32::UI::WindowsAndMessaging::*;

const COMMAND: u32 = WM_APP + 40;
const RELAYOUT: u32 = WM_APP + 41;
pub(crate) const MESSAGE: u32 = WM_APP + 42;
const REMEMBER_FOCUS: u32 = WM_APP + 43;
const WIDTH: f32 = 360.0;
const CONTENT: f32 = 1275.0;
const CONTENT_TOP: f32 = 68.0;
const CONTENT_BOTTOM: f32 = 62.0;
const WINDOW_STYLE: u32 = WS_OVERLAPPEDWINDOW | WS_CLIPCHILDREN;
const UPDATE_POLL: usize = 0x5544;
const SCROLL_FRAME: u32 = WM_APP + 44;

#[derive(Clone, Copy)]
struct SettingsLayout {
    column: f32,
    wide: bool,
    zoom: f32,
}

impl SettingsLayout {
    fn new(width: f32) -> Self {
        let wide = width >= 760.0;
        let column = if wide { (width - 16.0) * 0.5 } else { width };
        let zoom = ((column - 48.0) / (WIDTH - 48.0)).clamp(0.85, 1.25);
        Self { column, wide, zoom }
    }

    fn rect(self, rect: Rect) -> Rect {
        let second = self.wide && rect.y >= 627.0;
        let stretch = (self.column - 48.0).max(1.0) / (WIDTH - 48.0);
        Rect::new(
            24.0 + (rect.x - 24.0) * stretch + if second { self.column + 16.0 } else { 0.0 },
            (rect.y - if second { 627.0 } else { 0.0 }) * self.zoom,
            rect.w * stretch,
            rect.h * self.zoom,
        )
    }

    fn content_height(self) -> f32 {
        (if self.wide { CONTENT - 627.0 } else { CONTENT }) * self.zoom
    }

    fn text_scale(self) -> f32 {
        self.zoom
    }

    fn max_scroll(self, height: f32) -> f32 {
        (self.content_height() - (height - CONTENT_TOP - CONTENT_BOTTOM).max(0.0)).max(0.0)
    }
}

thread_local! {
    static WINDOW: Cell<HWND> = const { Cell::new(null_mut()) };
    static COMMANDS: RefCell<Vec<TrayCommand>> = const { RefCell::new(Vec::new()) };
}

struct Control {
    rect: Rect,
    label: String,
    command: TrayCommand,
    selected: bool,
    hwnd: HWND,
    action: Option<LocalAction>,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum LocalAction {
    CaptureShortcut,
    ApplyShortcut,
    CancelShortcut,
    DefaultShortcut,
    CheckUpdate,
    OpenRelease,
    ShortcutStatus,
    UpdateStatus,
}

#[derive(Default)]
struct ShortcutCapture {
    active: bool,
    pending: Option<Shortcut>,
    feedback: &'static str,
}

impl ShortcutCapture {
    fn begin(&mut self, current: Shortcut) {
        self.active = true;
        self.pending = Some(current);
        self.feedback = "Press a shortcut with at least one modifier.";
    }
    fn candidate(&mut self, shortcut: Option<Shortcut>) {
        self.pending = shortcut.filter(|shortcut| {
            shortcut.is_valid()
                && shortcut.key != 0x7b // F12 is reserved for the debugger.
                && !(shortcut.key == 0x73 && shortcut.modifiers & 1 != 0) // Alt+F4.
                && !(shortcut.key == 0x2e && shortcut.modifiers & 3 == 3) // Secure attention.
                && !(shortcut.key == 0x4c && shortcut.modifiers & 8 != 0) // Workstation lock.
        });
        self.feedback = if self.pending.is_some() {
            "Choose Apply to use this shortcut."
        } else {
            "Use a modifier and a supported key."
        };
    }
    fn cancel(&mut self) {
        self.active = false;
        self.pending = None;
        self.feedback = "";
    }
}

impl Control {
    fn is_choice(&self) -> bool {
        matches!(
            self.command,
            TrayCommand::Accent(_)
                | TrayCommand::Shadow(_)
                | TrayCommand::Motion(_)
                | TrayCommand::Bounce(_)
                | TrayCommand::Clock(_)
                | TrayCommand::ShowSeconds(_)
                | TrayCommand::TopGap(_)
                | TrayCommand::Dock(_)
                | TrayCommand::Lyrics(_)
                | TrayCommand::CompactLyrics(_)
                | TrayCommand::Language(_)
                | TrayCommand::Update(_)
                | TrayCommand::ToggleAutostart
        )
    }
    fn accessible_name(&self) -> String {
        let group = match self.command {
            TrayCommand::Accent(_) => "Accent",
            TrayCommand::Shadow(_) => "Soft shadow",
            TrayCommand::Motion(_) => "Motion speed",
            TrayCommand::Bounce(_) => "Bounce",
            TrayCommand::Clock(_) => "Time format",
            TrayCommand::ShowSeconds(_) => "Show seconds",
            TrayCommand::TopGap(_) => "Top gap",
            TrayCommand::Dock(_) => "Dock position",
            TrayCommand::Lyrics(_) => "Lyrics mode",
            TrayCommand::CompactLyrics(_) => "Compact lyrics",
            TrayCommand::Language(_) => "Language",
            TrayCommand::Update(_) => "Update channel",
            TrayCommand::ToggleAutostart => "Start with Windows",
            TrayCommand::LyricOffset(_) => "Lyrics timing offset",
            _ => "",
        };
        if group.is_empty() {
            self.label.clone()
        } else {
            format!("{}: {}", tr(group), self.label)
        }
    }
}

unsafe extern "system" fn button_proc(
    hwnd: HWND,
    message: u32,
    wparam: WPARAM,
    lparam: LPARAM,
    _id: usize,
    _data: usize,
) -> LRESULT {
    unsafe {
        match message {
            WM_PAINT => {
                let mut paint = PAINTSTRUCT::default();
                let dc = BeginPaint(hwnd, &mut paint);
                draw_button(hwnd, dc);
                EndPaint(hwnd, &paint);
                0
            }
            WM_PRINTCLIENT => {
                draw_button(hwnd, wparam as HDC);
                0
            }
            WM_ERASEBKGND => 1,
            BM_SETCHECK | BM_SETSTATE | BM_SETSTYLE | WM_SETTEXT | WM_SETFOCUS | WM_KILLFOCUS
            | WM_ENABLE | WM_UPDATEUISTATE => {
                if message == WM_SETFOCUS {
                    PostMessageW(GetParent(hwnd), REMEMBER_FOCUS, hwnd as usize, 0);
                }
                // BUTTON can paint synchronously from these messages, bypassing
                // WM_PAINT. Keep its check/pressed state and accessibility events,
                // but defer every visual update to our buffered painter. Test the
                // style, not IsWindowVisible: the popup can itself still be hidden.
                let visible = GetWindowLongW(hwnd, GWL_STYLE) as u32 & WS_VISIBLE != 0;
                if visible {
                    DefSubclassProc(hwnd, WM_SETREDRAW, 0, 0);
                }
                let result = DefSubclassProc(hwnd, message, wparam, lparam);
                if visible {
                    DefSubclassProc(hwnd, WM_SETREDRAW, 1, 0);
                }
                InvalidateRect(hwnd, null(), 0);
                result
            }
            WM_NCDESTROY => {
                RemoveWindowSubclass(hwnd, Some(button_proc), 1);
                DefSubclassProc(hwnd, message, wparam, lparam)
            }
            _ => DefSubclassProc(hwnd, message, wparam, lparam),
        }
    }
}

unsafe fn draw_button(hwnd: HWND, dc: HDC) {
    unsafe {
        let mut rect = RECT::default();
        GetClientRect(hwnd, &mut rect);
        let pressed = SendMessageW(hwnd, BM_GETSTATE, 0, 0) as u32;
        let item = DRAWITEMSTRUCT {
            CtlType: ODT_BUTTON,
            CtlID: GetDlgCtrlID(hwnd) as u32,
            itemAction: ODA_DRAWENTIRE,
            itemState: if pressed & 4 != 0 { ODS_SELECTED } else { 0 }
                | if GetFocus() == hwnd { ODS_FOCUS } else { 0 }
                | if crate::native_ui::pointer_inside(hwnd) {
                    ODS_HOTLIGHT
                } else {
                    0
                },
            hwndItem: hwnd,
            hDC: dc,
            rcItem: rect,
            ..Default::default()
        };
        SendMessageW(
            GetParent(hwnd),
            WM_DRAWITEM,
            item.CtlID as usize,
            (&item as *const DRAWITEMSTRUCT) as isize,
        );
    }
}

fn reveal_control(state: &mut State, index: usize) {
    let layout = SettingsLayout::new(state.width);
    let rect = layout.rect(state.controls[index].rect);
    let visible = (state.height - CONTENT_TOP - CONTENT_BOTTOM).max(0.0);
    if rect.y < state.scroll {
        state.scroll = rect.y;
    }
    if rect.y + rect.h > state.scroll + visible {
        state.scroll = rect.y + rect.h - visible;
    }
    state.scroll = state.scroll.clamp(0.0, layout.max_scroll(state.height));
    state.scroll_motion.jump(state.scroll);
    unsafe {
        state.frame_pump.stop();
    }
}

struct State {
    painter: Painter,
    surface: WindowSurface,
    frame_pump: FramePump,
    settings: Settings,
    controls: Vec<Control>,
    scale: f32,
    height: f32,
    width: f32,
    scroll: f32,
    scroll_motion: SmoothScroll,
    about: bool,
    background: Option<Background>,
    content: Option<ContentBackground>,
    button_rasters: Vec<ButtonRaster>,
    shortcut: ShortcutCapture,
    shortcut_available: bool,
    updates: crate::update::CheckSession,
}

struct Background {
    key: ((u32, u32), u32, u32, u32, bool),
    pixels: Vec<u8>,
}

struct ContentBackground {
    scale: u32,
    language: Language,
    size: (u32, u32),
    pixels: Vec<u8>,
}

#[derive(PartialEq, Eq)]
struct ButtonRasterKey {
    id: u32,
    size: (u32, u32),
    scale: u32,
    text_scale: u32,
    label: String,
    accent: [u8; 3],
    item_state: u32,
    selected: bool,
    enabled: bool,
    status: bool,
    destructive: bool,
}

struct ButtonRaster {
    key: ButtonRasterKey,
    pixels: std::sync::Arc<[u8]>,
}

fn button_raster(state: &mut State, item: &DRAWITEMSTRUCT, enabled: bool) -> std::sync::Arc<[u8]> {
    let control = state
        .controls
        .get((item.CtlID as usize).saturating_sub(100));
    let (label, selected) = match item.CtlID {
        1 => (if state.about { "‹" } else { "×" }, false),
        2 => (tr("Restart"), false),
        3 => (tr("Exit"), false),
        _ => control.map_or(("", false), |c| (c.label.as_str(), c.selected)),
    };
    let key = ButtonRasterKey {
        id: item.CtlID,
        size: (
            (item.rcItem.right - item.rcItem.left).max(0) as u32,
            (item.rcItem.bottom - item.rcItem.top).max(0) as u32,
        ),
        scale: state.scale.to_bits(),
        text_scale: SettingsLayout::new(state.width).text_scale().to_bits(),
        label: label.into(),
        accent: state.settings.accent_bgra(),
        item_state: item.itemState,
        selected,
        enabled,
        status: control.is_some_and(|c| {
            matches!(
                c.action,
                Some(LocalAction::ShortcutStatus | LocalAction::UpdateStatus)
            )
        }),
        destructive: item.CtlID == 3 || control.is_some_and(|c| c.command == TrayCommand::Reset),
    };
    if let Some(raster) = state.button_rasters.iter().find(|raster| raster.key == key) {
        return raster.pixels.clone();
    }
    let mut pixels = vec![0; key.size.0 as usize * key.size.1 as usize * 4];
    let mut canvas = Canvas::new(&mut pixels, key.size, &mut state.painter);
    if key.status {
        canvas.rect(canvas.clip, 0.0, crate::native_ui::BACKGROUND, 1.0);
        let bounds = canvas.clip;
        crate::native_ui::wrapped_text(
            &mut canvas,
            &key.label,
            bounds,
            11.0 * f32::from_bits(key.text_scale),
            MUTED,
        );
    } else {
        crate::native_ui::button(
            &mut canvas,
            &key.label,
            crate::native_ui::ButtonStyle {
                accent: key.accent,
                background: crate::native_ui::BACKGROUND,
                selected: key.selected,
                enabled: key.enabled,
                focused: key.item_state & ODS_FOCUS != 0,
                pressed: key.item_state & ODS_SELECTED != 0,
                hovered: key.item_state & ODS_HOTLIGHT != 0,
                destructive: key.destructive,
                text_scale: f32::from_bits(key.text_scale),
            },
        );
    }
    let pixels: std::sync::Arc<[u8]> = pixels.into();
    // Keep only the latest image for each native control. Repeated interaction
    // and resize variants cannot accumulate additional retained bitmaps.
    state
        .button_rasters
        .retain(|raster| raster.key.id != key.id);
    state.button_rasters.push(ButtonRaster {
        key,
        pixels: pixels.clone(),
    });
    pixels
}

fn copy_scrolled_content(state: &mut State, pixels: &mut [u8], size: (u32, u32)) {
    let scale = state.scale;
    let language = crate::i18n::language();
    let layout = SettingsLayout::new(state.width);
    let content = state.content.get_or_insert_with(|| ContentBackground {
        scale: 0,
        language,
        size: (0, 0),
        pixels: Vec::new(),
    });
    let desired_size = (
        (state.width * scale).round() as u32,
        ((CONTENT_TOP + layout.content_height()) * scale).ceil() as u32,
    );
    if content.scale != scale.to_bits()
        || content.language != language
        || content.size != desired_size
    {
        content.size = (desired_size.0, desired_size.1);
        content.pixels = [8, 8, 8, 255].repeat(content.size.0 as usize * content.size.1 as usize);
        let mut c = Canvas::new(&mut content.pixels, content.size, &mut state.painter);
        draw_settings_content(&mut c);
        content.scale = scale.to_bits();
        content.language = language;
    }
    let first = (CONTENT_TOP * scale).ceil().max(0.0) as usize;
    let last = ((state.height - CONTENT_BOTTOM) * scale)
        .floor()
        .max(first as f32)
        .min(size.1 as f32) as usize;
    let scroll = (state.scroll * scale).round().max(0.0) as usize;
    let width = size.0.min(content.size.0) as usize * 4;
    for y in first.min(size.1 as usize)..last {
        let source_y = y + scroll;
        if source_y >= content.size.1 as usize {
            break;
        }
        let source = source_y * content.size.0 as usize * 4;
        let target = y * size.0 as usize * 4;
        pixels[target..target + width].copy_from_slice(&content.pixels[source..source + width]);
    }
}

fn draw_settings_content(c: &mut Canvas<'_>) {
    let layout = SettingsLayout::new(c.clip.w);
    let map = |rect: Rect| {
        let mut rect = layout.rect(rect);
        rect.y += CONTENT_TOP;
        rect
    };
    for (y, h, title, detail) in [
        (0.0, 110.0, "Appearance", "Color & depth"),
        (117.0, 110.0, "Motion", "Pace & spring"),
        (234.0, 187.0, "Clock & position", "Time & placement"),
        (428.0, 192.0, "Music & lyrics", "Timing & display"),
        (627.0, 645.0, "Application", "Startup & updates"),
    ] {
        // Keep the separator in the section's column, including its last edge.
        let mut separator = map(Rect::new(26.0, y, 308.0, 1.0));
        separator.y += h * layout.zoom;
        c.rect(separator, 0.0, WHITE, 0.06);
        c.text(
            tr(title),
            map(Rect::new(32.0, 9.0 + y, 290.0, 18.0)),
            12.0 * layout.text_scale(),
            true,
            WHITE,
        );
        c.text(
            tr(detail),
            map(Rect::new(207.0, 9.0 + y, 115.0, 18.0)),
            9.0 * layout.text_scale(),
            false,
            MUTED,
        );
    }
    for (y, label) in [
        (34.0, "Accent"),
        (71.0, "Soft shadow"),
        (151.0, "Speed"),
        (188.0, "Bounce"),
        (268.0, "Time format"),
        (305.0, "Seconds"),
        (342.0, "Top gap"),
        (379.0, "Dock position"),
        (462.0, "Lyrics"),
        (499.0, "Timing offset"),
        (539.0, "Compact lyrics"),
        (658.0, "Start with Windows"),
        (702.0, "Shortcut"),
        (882.0, "Language"),
        (952.0, "Updates"),
    ] {
        let width = if matches!(y, 539.0 | 658.0) {
            207.0
        } else if matches!(y, 702.0 | 882.0 | 952.0) {
            287.0
        } else {
            99.0
        };
        c.text(
            tr(label),
            map(Rect::new(32.0, y, width, 29.0)),
            11.0 * layout.text_scale(),
            false,
            MUTED,
        );
    }
}

struct ControlUpdate {
    hwnd: HWND,
    name: Option<String>,
    checked: Option<bool>,
}

fn update_controls(
    state: &mut State,
    settings: Settings,
    autostart: bool,
    focus: HWND,
) -> Vec<ControlUpdate> {
    let accent_changed = state.settings.accent != settings.accent;
    let language_changed = state.settings.language != settings.language;
    state.updates.set_channel(settings.update);
    if language_changed {
        state.background = None;
    }
    if state.shortcut.feedback == "Applying shortcut..."
        && state.shortcut.pending == Some(settings.shortcut)
    {
        state.shortcut.feedback = "Shortcut updated.";
        state.shortcut.pending = None;
    }
    state.settings = settings;
    let mut updates: Vec<_> = state
        .controls
        .iter_mut()
        .zip(controls_with_shortcut(
            &settings,
            autostart,
            state.shortcut_available,
        ))
        .filter_map(|(old, new)| {
            if old.action.is_some() {
                return None;
            }
            let label_changed = old.label != new.label || language_changed;
            let check_changed = old.selected != new.selected;
            old.label = new.label;
            old.command = new.command;
            old.selected = new.selected;
            let repaint = label_changed
                || check_changed
                || (accent_changed && (old.selected || old.hwnd == focus));
            repaint.then(|| ControlUpdate {
                hwnd: old.hwnd,
                name: label_changed.then(|| old.accessible_name()),
                checked: (check_changed && old.is_choice()).then_some(old.selected),
            })
        })
        .collect();
    if accent_changed && !focus.is_null() && !state.controls.iter().any(|c| c.hwnd == focus) {
        updates.push(ControlUpdate {
            hwnd: focus,
            name: None,
            checked: None,
        });
    }
    updates
}

struct Popup {
    state: RefCell<State>,
    suggested: Cell<Option<RECT>>,
    owner: HWND,
    focus: Cell<HWND>,
}

fn wide(text: &str) -> Vec<u16> {
    text.encode_utf16().chain(Some(0)).collect()
}

#[cfg(test)]
fn controls(settings: &Settings) -> Vec<Control> {
    controls_with_autostart(settings, crate::autostart::is_enabled())
}

fn controls_with_shortcut(settings: &Settings, autostart: bool, available: bool) -> Vec<Control> {
    let mut controls = controls_with_autostart(settings, autostart);
    if !available
        && let Some(action) = controls
            .iter_mut()
            .find(|c| c.command == TrayCommand::TogglePassthrough)
    {
        action.label = tr("Fade and click through").into();
    }
    controls
}

fn controls_with_autostart(settings: &Settings, autostart: bool) -> Vec<Control> {
    let mut out = Vec::new();
    let mut add = |x, y, w, label: &str, command, selected| {
        out.push(Control {
            rect: Rect::new(x, y, w, 29.0),
            label: tr(label).into(),
            command,
            selected,
            hwnd: null_mut(),
            action: None,
        })
    };
    for (i, preset) in [AccentPreset::Ice, AccentPreset::Amber, AccentPreset::Mint]
        .into_iter()
        .enumerate()
    {
        add(
            132.0 + i as f32 * 64.0,
            34.0,
            59.0,
            preset.label(),
            TrayCommand::Accent(preset),
            settings.accent == preset,
        );
    }
    add(
        242.0,
        71.0,
        77.0,
        if settings.shadow { "On" } else { "Off" },
        TrayCommand::Shadow(!settings.shadow),
        settings.shadow,
    );
    for (i, preset) in [MotionPreset::Slow, MotionPreset::Normal, MotionPreset::Fast]
        .into_iter()
        .enumerate()
    {
        add(
            132.0 + i as f32 * 64.0,
            151.0,
            59.0,
            ["Slow", "Normal", "Fast"][i],
            TrayCommand::Motion(preset),
            settings.motion == preset,
        );
    }
    for (i, preset) in [BouncePreset::Low, BouncePreset::Medium, BouncePreset::High]
        .into_iter()
        .enumerate()
    {
        add(
            132.0 + i as f32 * 64.0,
            188.0,
            59.0,
            ["Low", "Medium", "High"][i],
            TrayCommand::Bounce(preset),
            settings.bounce == preset,
        );
    }
    for (i, format) in [ClockFormat::TwentyFourHour, ClockFormat::TwelveHour]
        .into_iter()
        .enumerate()
    {
        add(
            166.0 + i as f32 * 80.0,
            268.0,
            74.0,
            ["24 hour", "12 hour"][i],
            TrayCommand::Clock(format),
            settings.clock == format,
        );
    }
    add(
        242.0,
        305.0,
        77.0,
        if settings.show_seconds { "On" } else { "Off" },
        TrayCommand::ShowSeconds(!settings.show_seconds),
        settings.show_seconds,
    );
    for (i, gap) in [0.0, 8.0, 16.0].into_iter().enumerate() {
        add(
            132.0 + i as f32 * 64.0,
            342.0,
            59.0,
            ["0 px", "8 px", "16 px"][i],
            TrayCommand::TopGap(gap),
            settings.top_gap == gap,
        );
    }
    for (i, mode) in [LyricsMode::Auto, LyricsMode::Lines, LyricsMode::Words]
        .into_iter()
        .enumerate()
    {
        add(
            132.0 + i as f32 * 64.0,
            422.0,
            59.0,
            ["Auto", "Lines", "Words"][i],
            TrayCommand::Lyrics(mode),
            settings.lyrics == mode,
        );
    }
    add(
        132.0,
        459.0,
        40.0,
        "−",
        TrayCommand::LyricOffset((settings.lyric_offset_ms - 100).max(-10000)),
        false,
    );
    add(
        277.0,
        459.0,
        42.0,
        "+",
        TrayCommand::LyricOffset((settings.lyric_offset_ms + 100).min(10000)),
        false,
    );
    add(
        174.0,
        459.0,
        100.0,
        &tr("{offset}s Reset").replace(
            "{offset}",
            &format!("{:+.1}", settings.lyric_offset_ms as f32 / 1000.0),
        ),
        TrayCommand::LyricOffset(0),
        settings.lyric_offset_ms != 0,
    );
    add(
        32.0,
        500.0,
        287.0,
        "Clear lyrics cache",
        TrayCommand::ClearLyricsCache,
        false,
    );
    add(
        242.0,
        580.0,
        77.0,
        if autostart { "On" } else { "Off" },
        TrayCommand::ToggleAutostart,
        autostart,
    );
    for (i, channel) in [UpdateChannel::Build, UpdateChannel::Stable]
        .into_iter()
        .enumerate()
    {
        add(
            166.0 + i as f32 * 80.0,
            617.0,
            74.0,
            channel.label(),
            TrayCommand::Update(channel),
            settings.update == channel,
        );
    }
    add(
        32.0,
        662.0,
        137.0,
        "Settings folder",
        TrayCommand::OpenSettingsFolder,
        false,
    );
    add(
        177.0,
        662.0,
        142.0,
        "About HiyoDyna",
        TrayCommand::About,
        false,
    );
    add(
        32.0,
        702.0,
        287.0,
        "Reset to defaults",
        TrayCommand::Reset,
        false,
    );
    // Insert the placement row below Top gap and leave the remaining sections
    // in their original keyboard order.
    for control in &mut out {
        if control.rect.y >= 388.0 {
            control.rect.y += 40.0;
        }
    }
    let placement = out
        .iter()
        .position(|c| c.rect.y >= 428.0)
        .unwrap_or(out.len());
    for (i, anchor) in [DockAnchor::Left, DockAnchor::Center, DockAnchor::Right]
        .into_iter()
        .enumerate()
    {
        out.insert(
            placement + i,
            Control {
                rect: Rect::new(132.0 + i as f32 * 64.0, 379.0, 59.0, 29.0),
                label: tr(["Left", "Center", "Right"][i]).into(),
                command: TrayCommand::Dock(anchor),
                selected: settings.dock == anchor,
                hwnd: null_mut(),
                action: None,
            },
        );
    }
    out.push(Control {
        rect: Rect::new(32.0, 782.0, 287.0, 29.0),
        label: tr("Fade and click through ({shortcut})")
            .replace("{shortcut}", &settings.shortcut.label()),
        command: TrayCommand::TogglePassthrough,
        selected: false,
        hwnd: null_mut(),
        action: None,
    });
    // Keep the existing groups and keyboard ordering; only extend their content.
    for control in &mut out {
        if control.rect.y == 540.0 {
            control.rect.y = 578.0;
        }
        match control.command {
            TrayCommand::ToggleAutostart => control.rect.y = 658.0,
            TrayCommand::Update(_) => control.rect.y = 976.0,
            TrayCommand::OpenSettingsFolder | TrayCommand::About => control.rect.y = 1155.0,
            TrayCommand::Reset => control.rect.y = 1195.0,
            TrayCommand::TogglePassthrough => control.rect.y = 1235.0,
            _ => {}
        }
    }
    out.push(Control {
        rect: Rect::new(242.0, 539.0, 77.0, 29.0),
        label: tr(if settings.compact_lyrics { "On" } else { "Off" }).into(),
        command: TrayCommand::CompactLyrics(!settings.compact_lyrics),
        selected: settings.compact_lyrics,
        hwnd: null_mut(),
        action: None,
    });
    for (i, language) in [Language::System, Language::English, Language::Vietnamese]
        .into_iter()
        .enumerate()
    {
        out.push(Control {
            rect: Rect::new(32.0 + i as f32 * 97.0, 906.0, 93.0, 29.0),
            label: match language {
                Language::System => tr("System"),
                Language::English => "English",
                Language::Vietnamese => "Tiếng Việt",
            }
            .into(),
            command: TrayCommand::Language(language),
            selected: settings.language == language,
            hwnd: null_mut(),
            action: None,
        });
    }
    for (action, x, y, w, label) in [
        (
            LocalAction::CaptureShortcut,
            32.0,
            726.0,
            287.0,
            settings.shortcut.label(),
        ),
        (
            LocalAction::ApplyShortcut,
            32.0,
            763.0,
            137.0,
            tr("Apply").into(),
        ),
        (
            LocalAction::CancelShortcut,
            177.0,
            763.0,
            142.0,
            tr("Cancel").into(),
        ),
        (
            LocalAction::DefaultShortcut,
            32.0,
            800.0,
            287.0,
            tr("Restore default shortcut").into(),
        ),
        (
            LocalAction::CheckUpdate,
            32.0,
            1013.0,
            287.0,
            tr("Check for updates").into(),
        ),
        (
            LocalAction::OpenRelease,
            32.0,
            1110.0,
            287.0,
            tr("Open release page").into(),
        ),
        (
            LocalAction::ShortcutStatus,
            32.0,
            837.0,
            287.0,
            String::new(),
        ),
        (
            LocalAction::UpdateStatus,
            32.0,
            1050.0,
            287.0,
            String::new(),
        ),
    ] {
        let status = matches!(
            action,
            LocalAction::ShortcutStatus | LocalAction::UpdateStatus
        );
        out.push(Control {
            rect: Rect::new(
                x,
                y,
                w,
                if action == LocalAction::ShortcutStatus {
                    38.0
                } else if status {
                    55.0
                } else {
                    29.0
                },
            ),
            label,
            command: TrayCommand::Settings,
            selected: false,
            hwnd: null_mut(),
            action: Some(action),
        });
    }
    out.sort_by(|a, b| {
        a.rect
            .y
            .total_cmp(&b.rect.y)
            .then(a.rect.x.total_cmp(&b.rect.x))
    });
    out
}

fn clamp_anchor(anchor: POINT, work: RECT, width: i32, height: i32) -> POINT {
    POINT {
        x: anchor
            .x
            .clamp(work.left, (work.right - width).max(work.left)),
        y: anchor
            .y
            .clamp(work.top, (work.bottom - height).max(work.top)),
    }
}

struct PopupGeometry {
    scale: f32,
    origin: POINT,
    width_px: i32,
    height_px: i32,
}

fn popup_geometry(anchor: POINT, work: RECT, dpi: u32) -> PopupGeometry {
    let scale = if dpi == 0 { 96 } else { dpi } as f32 / 96.0;
    let available = (work.bottom - work.top - 24).max(1);
    let height = 760.0_f32.min(available as f32 / scale);
    let width_px = ((WIDTH * scale) as i32).min((work.right - work.left).max(1));
    let height_px = (height * scale) as i32;
    PopupGeometry {
        scale,
        origin: clamp_anchor(anchor, work, width_px, height_px),
        width_px,
        height_px,
    }
}

pub(crate) fn show(
    owner: HWND,
    settings: &Settings,
    shortcut_available: bool,
) -> Result<(), String> {
    unsafe {
        let existing = WINDOW.with(Cell::get);
        if !existing.is_null() && IsWindow(existing) != 0 {
            update(settings, shortcut_available);
            ShowWindow(
                existing,
                if IsIconic(existing) != 0 {
                    SW_RESTORE
                } else {
                    SW_SHOW
                },
            );
            SetForegroundWindow(existing);
            return Ok(());
        }
        let mut anchor = POINT::default();
        GetCursorPos(&mut anchor);
        let class_name = wide("HiyoDynaPreferences");
        let instance = GetModuleHandleW(null());
        let class = WNDCLASSW {
            style: CS_HREDRAW | CS_VREDRAW,
            lpfnWndProc: Some(proc),
            hInstance: instance,
            lpszClassName: class_name.as_ptr(),
            hCursor: LoadCursorW(null_mut(), IDC_ARROW),
            ..Default::default()
        };
        RegisterClassW(&class);
        let hwnd = CreateWindowExW(
            WS_EX_TOOLWINDOW | WS_EX_CONTROLPARENT,
            class_name.as_ptr(),
            wide(tr("HiyoDyna Settings")).as_ptr(),
            WINDOW_STYLE,
            anchor.x,
            anchor.y,
            1,
            1,
            owner,
            null_mut(),
            instance,
            null(),
        );
        if hwnd.is_null() {
            UnregisterClassW(class_name.as_ptr(), instance);
            return Err("Could not create settings window".into());
        }
        // Creating on the cursor's monitor establishes this window's effective DPI,
        // independently of the owner's monitor in our per-monitor-aware process.
        let dpi = windows_sys::Win32::UI::HiDpi::GetDpiForWindow(hwnd);
        let scale = if dpi == 0 { 96 } else { dpi } as f32 / 96.0;
        let painter = match Painter::new(scale) {
            Ok(painter) => painter,
            Err(error) => {
                DestroyWindow(hwnd);
                UnregisterClassW(class_name.as_ptr(), instance);
                return Err(error);
            }
        };
        let Some(surface) = WindowSurface::new() else {
            DestroyWindow(hwnd);
            return Err("Could not create Settings surface".into());
        };
        let popup = Box::new(Popup {
            state: RefCell::new(State {
                painter,
                surface,
                frame_pump: FramePump::new(SCROLL_FRAME),
                settings: *settings,
                controls: controls_with_shortcut(
                    settings,
                    crate::autostart::is_enabled(),
                    shortcut_available,
                ),
                scale,
                height: 760.0,
                width: WIDTH,
                scroll: 0.0,
                scroll_motion: SmoothScroll::default(),
                about: false,
                background: None,
                content: None,
                button_rasters: Vec::new(),
                shortcut: ShortcutCapture::default(),
                shortcut_available,
                updates: crate::update::CheckSession::new(settings.update),
            }),
            suggested: Cell::new(None),
            owner,
            focus: Cell::new(null_mut()),
        });
        let ptr = Box::into_raw(popup);
        let state = &(*ptr).state;
        SetWindowLongPtrW(hwnd, GWLP_USERDATA, ptr as isize);
        WINDOW.with(|window| window.set(hwnd));
        if let Err(error) = crate::window_chrome::install(hwnd, true, true) {
            DestroyWindow(hwnd);
            return Err(error);
        }
        let buttons: Vec<_> = state
            .borrow()
            .controls
            .iter()
            .map(|control| {
                (
                    control.accessible_name(),
                    control.is_choice(),
                    control.selected,
                    matches!(
                        control.action,
                        Some(LocalAction::ShortcutStatus | LocalAction::UpdateStatus)
                    ),
                )
            })
            .collect();
        for (i, (name, choice, selected, status)) in buttons.into_iter().enumerate() {
            let button = CreateWindowExW(
                0,
                wide(if status { "STATIC" } else { "BUTTON" }).as_ptr(),
                wide(&name).as_ptr(),
                WS_CHILD
                    | if status { 0x0d } else { WS_TABSTOP }
                    | if status {
                        0
                    } else if choice {
                        BS_CHECKBOX as u32 | BS_PUSHLIKE as u32
                    } else {
                        BS_PUSHBUTTON as u32
                    },
                0,
                0,
                1,
                1,
                hwnd,
                (i + 100) as *mut _,
                instance,
                null(),
            );
            state.borrow_mut().controls[i].hwnd = button;
            if !status {
                SetWindowSubclass(button, Some(button_proc), 1, 0);
                SetWindowSubclass(button, Some(crate::native_ui::hover_proc), 2, 0);
            }
            if choice {
                SendMessageW(button, BM_SETCHECK, usize::from(selected), 0);
            }
        }
        for (id, label) in [(1, "Close"), (2, "Restart"), (3, "Exit")] {
            let button = CreateWindowExW(
                0,
                wide("BUTTON").as_ptr(),
                wide(tr(label)).as_ptr(),
                WS_CHILD | WS_VISIBLE | WS_TABSTOP | BS_OWNERDRAW as u32,
                0,
                0,
                1,
                1,
                hwnd,
                id as *mut _,
                instance,
                null(),
            );
            SetWindowSubclass(button, Some(button_proc), 1, 0);
            SetWindowSubclass(button, Some(crate::native_ui::hover_proc), 2, 0);
        }
        if let Err(error) = initial_layout(hwnd, state, anchor) {
            DestroyWindow(hwnd);
            UnregisterClassW(class_name.as_ptr(), instance);
            return Err(error);
        }
        refresh_local(hwnd, state);
        ShowWindow(hwnd, SW_SHOW);
        SetTimer(hwnd, UPDATE_POLL, 100, None);
        SetForegroundWindow(hwnd);
        SetFocus(GetDlgItem(hwnd, crate::window_chrome::CLOSE));
        Ok(())
    }
}

pub(crate) fn take_commands() -> Vec<TrayCommand> {
    COMMANDS.with(|commands| std::mem::take(&mut *commands.borrow_mut()))
}

pub(crate) fn update(settings: &Settings, shortcut_available: bool) {
    unsafe {
        let hwnd = WINDOW.with(Cell::get);
        let ptr = GetWindowLongPtrW(hwnd, GWLP_USERDATA) as *const Popup;
        if ptr.is_null() {
            return;
        }
        (*ptr).state.borrow_mut().shortcut_available = shortcut_available;
        let updates = update_controls(
            &mut (*ptr).state.borrow_mut(),
            *settings,
            crate::autostart::is_enabled(),
            GetFocus(),
        );
        for update in updates {
            if let Some(name) = update.name {
                SetWindowTextW(update.hwnd, wide(&name).as_ptr());
            }
            if let Some(checked) = update.checked {
                SendMessageW(update.hwnd, BM_SETCHECK, usize::from(checked), 0);
            }
            InvalidateRect(update.hwnd, null(), 0);
        }
        refresh_local(hwnd, &(*ptr).state);
        SetWindowTextW(hwnd, wide(tr("HiyoDyna Settings")).as_ptr());
        for (id, key) in [(2, "Restart"), (3, "Exit")] {
            SetWindowTextW(GetDlgItem(hwnd, id), wide(tr(key)).as_ptr());
        }
        let about = (*ptr).state.borrow().about;
        SetWindowTextW(
            GetDlgItem(hwnd, 1),
            wide(tr(if about { "Back to settings" } else { "Close" })).as_ptr(),
        );
        crate::window_chrome::refresh(hwnd);
        InvalidateRect(hwnd, null(), 0);
        crate::native_ui::refresh_language();
    }
}

pub(crate) fn capturing_shortcut() -> bool {
    unsafe {
        let hwnd = WINDOW.with(Cell::get);
        if hwnd.is_null() {
            return false;
        }
        let ptr = GetWindowLongPtrW(hwnd, GWLP_USERDATA) as *const Popup;
        !ptr.is_null() && (*ptr).state.borrow().shortcut.active
    }
}

pub(crate) fn shortcut_error() {
    unsafe {
        let hwnd = WINDOW.with(Cell::get);
        if hwnd.is_null() {
            return;
        }
        let ptr = GetWindowLongPtrW(hwnd, GWLP_USERDATA) as *const Popup;
        if ptr.is_null() {
            return;
        }
        {
            let mut state = (*ptr).state.borrow_mut();
            state.shortcut.cancel();
            state.shortcut.feedback =
                "This shortcut is unavailable. Your previous shortcut is unchanged.";
        }
        refresh_local(hwnd, &(*ptr).state);
    }
}

fn update_summary(status: &crate::update::CheckStatus) -> String {
    use crate::update::CheckStatus;
    match status {
        CheckStatus::Idle => tr("Check for a published release.").into(),
        CheckStatus::Checking => tr("Checking for updates...").into(),
        CheckStatus::UpToDate { tag, .. } => tr("Up to date: {tag}").replace("{tag}", tag),
        CheckStatus::Available { tag, .. } => tr("Update available: {tag}").replace("{tag}", tag),
        CheckStatus::LatestPublished { tag, .. } => {
            tr("Latest published release: {tag}").replace("{tag}", tag)
        }
        CheckStatus::NoReleases => tr("No published release.").into(),
        CheckStatus::Error(_) => tr("Unable to check for updates. Please try again.").into(),
    }
}

fn release_url(status: &crate::update::CheckStatus) -> Option<&str> {
    use crate::update::CheckStatus;
    match status {
        CheckStatus::UpToDate { url, .. }
        | CheckStatus::Available { url, .. }
        | CheckStatus::LatestPublished { url, .. } => Some(url),
        _ => None,
    }
}

unsafe fn refresh_local(hwnd: HWND, state: &RefCell<State>) {
    let updates = {
        let mut state = state.borrow_mut();
        let pending = state.shortcut.pending.map(Shortcut::label);
        let binding = state.settings.shortcut.label();
        let active = state.shortcut.active;
        let feedback = tr(state.shortcut.feedback).to_owned();
        let checking = matches!(state.updates.status(), crate::update::CheckStatus::Checking);
        let retry = matches!(state.updates.status(), crate::update::CheckStatus::Error(_));
        let summary = update_summary(state.updates.status());
        let can_open = release_url(state.updates.status()).is_some();
        state
            .controls
            .iter_mut()
            .filter_map(|control| {
                let action = control.action?;
                let (label, enabled) = match action {
                    LocalAction::CaptureShortcut => (
                        if active {
                            tr("Shortcut: {shortcut}")
                                .replace("{shortcut}", pending.as_deref().unwrap_or("…"))
                        } else {
                            tr("Change shortcut: {shortcut}").replace("{shortcut}", &binding)
                        },
                        true,
                    ),
                    LocalAction::ApplyShortcut => (tr("Apply").into(), active && pending.is_some()),
                    LocalAction::CancelShortcut => (tr("Cancel").into(), active),
                    LocalAction::DefaultShortcut => (tr("Restore default shortcut").into(), true),
                    LocalAction::CheckUpdate => (
                        tr(if checking {
                            "Checking..."
                        } else if retry {
                            "Retry"
                        } else {
                            "Check for updates"
                        })
                        .into(),
                        !checking,
                    ),
                    LocalAction::OpenRelease => (tr("Open release page").into(), can_open),
                    LocalAction::ShortcutStatus => (feedback.clone(), true),
                    LocalAction::UpdateStatus => (summary.clone(), true),
                };
                control.label = label;
                Some((control.hwnd, control.accessible_name(), enabled))
            })
            .collect::<Vec<_>>()
    };
    unsafe {
        for (control, name, enabled) in updates {
            if control.is_null() {
                continue;
            }
            SetWindowTextW(control, wide(&name).as_ptr());
            windows_sys::Win32::UI::Input::KeyboardAndMouse::EnableWindow(
                control,
                i32::from(enabled),
            );
            InvalidateRect(control, null(), 0);
        }
        InvalidateRect(hwnd, null(), 0);
    }
}

// Called by the application's ordinary message loop before Translate/Dispatch.
pub(crate) fn translate_message(message: &MSG) -> bool {
    unsafe {
        let hwnd = WINDOW.with(Cell::get);
        if hwnd.is_null() || (message.hwnd != hwnd && IsChild(hwnd, message.hwnd) == 0) {
            return false;
        }
        let ptr = GetWindowLongPtrW(hwnd, GWLP_USERDATA) as *const Popup;
        if ptr.is_null() {
            return false;
        }
        let state = &(*ptr).state;
        #[cfg(test)]
        if message.message == WM_APP + 99 {
            tests::KEYBOARD_DONE.store(true, std::sync::atomic::Ordering::Release);
            return true;
        }
        if state.borrow().shortcut.active && matches!(message.message, WM_KEYDOWN | WM_SYSKEYDOWN) {
            let key = message.wParam as u32;
            if key == 27 {
                state.borrow_mut().shortcut.cancel();
                refresh_local(hwnd, state);
                return true;
            }
            if key != 9 && key != 13 {
                if !matches!(key, 0x10..=0x12 | 0x5b..=0x5c | 0xa0..=0xa5) {
                    state
                        .borrow_mut()
                        .shortcut
                        .candidate(Shortcut::from_key(key));
                    refresh_local(hwnd, state);
                }
                return true;
            }
        }
        if state.borrow().shortcut.active
            && matches!(
                message.message,
                WM_CHAR | WM_SYSCHAR | WM_KEYUP | WM_SYSKEYUP
            )
        {
            return true;
        }
        if message.message == WM_KEYDOWN && message.wParam == 9 && !state.borrow().about {
            let mut order = [
                crate::window_chrome::MINIMIZE,
                crate::window_chrome::MAXIMIZE,
                crate::window_chrome::CLOSE,
            ]
            .map(|id| GetDlgItem(hwnd, id))
            .to_vec();
            order.extend(
                state
                    .borrow()
                    .controls
                    .iter()
                    .filter(|c| {
                        !matches!(
                            c.action,
                            Some(LocalAction::ShortcutStatus | LocalAction::UpdateStatus)
                        )
                    })
                    .map(|c| c.hwnd),
            );
            order.extend([GetDlgItem(hwnd, 2), GetDlgItem(hwnd, 3)]);
            order.retain(|control| {
                !control.is_null()
                    && windows_sys::Win32::UI::Input::KeyboardAndMouse::IsWindowEnabled(*control)
                        != 0
            });
            let current = order.iter().position(|c| *c == GetFocus());
            let next = match (current, GetKeyState(VK_SHIFT as i32) < 0) {
                (Some(i), true) => (i + order.len() - 1) % order.len(),
                (Some(i), false) => (i + 1) % order.len(),
                (None, true) => order.len() - 1,
                (None, false) => 0,
            };
            let index = state
                .borrow()
                .controls
                .iter()
                .position(|c| c.hwnd == order[next]);
            if let Some(index) = index {
                reveal_control(&mut state.borrow_mut(), index);
                position_controls(hwnd, state);
                InvalidateRect(hwnd, null(), 0);
            }
            SetFocus(order[next]);
            return true;
        }
        if message.message == WM_KEYDOWN && message.wParam == 13 && IsChild(hwnd, GetFocus()) != 0 {
            let id = GetDlgCtrlID(GetFocus());
            PostMessageW(
                hwnd,
                if crate::window_chrome::is_button(id) {
                    WM_COMMAND
                } else {
                    COMMAND
                },
                id as usize,
                0,
            );
            return true;
        }
        if message.message == WM_KEYDOWN && message.wParam == 27 {
            PostMessageW(hwnd, COMMAND, 1, 0);
            return true;
        }
        if message.message == WM_KEYDOWN
            && matches!(message.wParam, 33..=36)
            && !state.borrow().about
        {
            let target = {
                let state = state.borrow();
                match message.wParam {
                    33 => state.scroll_motion.target() - 180.0,
                    34 => state.scroll_motion.target() + 180.0,
                    35 => CONTENT,
                    _ => 0.0,
                }
            };
            queue_scroll(hwnd, state, target);
            return true;
        }
        if message.message == WM_MOUSEWHEEL {
            if !state.borrow().about {
                let delta = (message.wParam >> 16) as u16 as i16;
                let target = state.borrow().scroll_motion.target() - delta as f32 / 120.0 * 60.0;
                queue_scroll(hwnd, state, target);
            }
            return true;
        }
        IsDialogMessageW(hwnd, message) != 0
    }
}

unsafe fn queue_scroll(hwnd: HWND, state: &RefCell<State>, target: f32) {
    {
        let mut state = state.borrow_mut();
        let was_moving = state.scroll_motion.moving();
        if !was_moving && state.scroll_motion.offset() != state.scroll {
            let scroll = state.scroll;
            state.scroll_motion.jump(scroll);
        }
        let max = SettingsLayout::new(state.width).max_scroll(state.height);
        state.scroll_motion.scroll_to(target, max, Instant::now());
        unsafe {
            if state.scroll_motion.moving() {
                state.frame_pump.start(hwnd);
                if !state.frame_pump.is_active() {
                    let target = state.scroll_motion.target();
                    state.scroll_motion.jump(target);
                }
            } else {
                state.frame_pump.stop();
            }
        }
    }
    if !state.borrow().scroll_motion.moving() {
        unsafe {
            advance_scroll(hwnd, state, Instant::now());
        }
    }
}

unsafe fn advance_scroll(hwnd: HWND, state: &RefCell<State>, now: Instant) {
    let (changed, moving) = {
        let mut state = state.borrow_mut();
        state.scroll_motion.tick(now);
        let changed = state.scroll != state.scroll_motion.offset();
        state.scroll = state.scroll_motion.offset();
        (changed, state.scroll_motion.moving())
    };
    unsafe {
        if changed {
            position_controls(hwnd, state);
        }
        if !moving {
            state.borrow_mut().frame_pump.stop();
        }
    }
}

unsafe fn client_outer_size(width: i32, height: i32, dpi: u32) -> (i32, i32) {
    let _ = dpi;
    // The shared frame makes the entire outer rectangle client space.
    (width, height)
}

unsafe fn initial_layout(hwnd: HWND, state: &RefCell<State>, anchor: POINT) -> Result<(), String> {
    unsafe {
        let owner = GetWindow(hwnd, GW_OWNER);
        if !owner.is_null() {
            crate::island_surface::place(hwnd, owner, (420.0, 640.0));
            return reflow(hwnd, state);
        }
        let monitor = MonitorFromWindow(hwnd, MONITOR_DEFAULTTONEAREST);
        let mut info = MONITORINFO {
            cbSize: size_of::<MONITORINFO>() as u32,
            ..Default::default()
        };
        if GetMonitorInfoW(monitor, &mut info) == 0 {
            return Err("Could not read settings monitor work area".into());
        }
        let geometry = popup_geometry(
            anchor,
            info.rcWork,
            windows_sys::Win32::UI::HiDpi::GetDpiForWindow(hwnd),
        );
        let dpi = (geometry.scale * 96.0) as u32;
        let (width, height) = client_outer_size(geometry.width_px, geometry.height_px, dpi);
        let width = width.min(info.rcWork.right - info.rcWork.left);
        let height = height.min(info.rcWork.bottom - info.rcWork.top);
        let origin = clamp_anchor(geometry.origin, info.rcWork, width, height);
        // Native calls can synchronously reenter proc; no State borrow crosses them.
        SetWindowPos(
            hwnd,
            null_mut(),
            origin.x,
            origin.y,
            width,
            height,
            SWP_NOZORDER | SWP_NOACTIVATE,
        );
        reflow(hwnd, state)
    }
}

unsafe fn reflow(hwnd: HWND, state: &RefCell<State>) -> Result<(), String> {
    unsafe {
        let mut rect = RECT::default();
        GetClientRect(hwnd, &mut rect);
        let dpi = windows_sys::Win32::UI::HiDpi::GetDpiForWindow(hwnd);
        let scale = dpi.max(96) as f32 / 96.0;
        let painter = if state.borrow().scale != scale {
            Some(Painter::new(scale)?)
        } else {
            None
        };
        let focus = GetFocus();
        {
            let mut state = state.borrow_mut();
            state.frame_pump.stop();
            state
                .surface
                .resize(rect.right.max(0) as u32, rect.bottom.max(0) as u32);
            if let Some(painter) = painter {
                state.painter = painter;
            }
            state.scale = scale;
            state.width = rect.right.max(0) as f32 / scale;
            state.height = rect.bottom.max(0) as f32 / scale;
            state.scroll = state.scroll.clamp(
                0.0,
                SettingsLayout::new(state.width).max_scroll(state.height),
            );
            let scroll = state.scroll;
            state.scroll_motion.jump(scroll);
            if let Some(index) = state.controls.iter().position(|c| c.hwnd == focus) {
                reveal_control(&mut state, index);
            }
        }
        position_controls(hwnd, state);
        InvalidateRect(hwnd, null(), 0);
        Ok(())
    }
}

unsafe fn position_controls(hwnd: HWND, state: &RefCell<State>) {
    let started = trace_start();
    let (positions, scale, height) = {
        let state = state.borrow();
        let positions: Vec<_> = state
            .controls
            .iter()
            .map(|control| {
                let rect = SettingsLayout::new(state.width).rect(control.rect);
                let y = CONTENT_TOP + rect.y - state.scroll;
                let visible =
                    !state.about && y + rect.h > CONTENT_TOP && y < state.height - CONTENT_BOTTOM;
                (control.hwnd, rect.x, y, rect.w, rect.h, visible)
            })
            .collect();
        (positions, state.scale, state.height)
    };
    let width = state.borrow().width;
    let footer_width = ((width - 56.0) * 0.5).max(1.0);
    let about = state.borrow().about;
    unsafe {
        // One scroll is one frame. Child moves must never copy pixels from the
        // previous background or become visible before the whole layout is ready.
        let visible_window = IsWindowVisible(hwnd) != 0;
        if visible_window {
            SendMessageW(hwnd, WM_SETREDRAW, 0, 0);
        }
        for (button, x, y, w, h, visible) in positions {
            if button.is_null() {
                continue;
            }
            if !visible {
                // Offscreen controls retain their native identity for keyboard
                // navigation; lay them out when they re-enter the viewport.
                if GetWindowLongW(button, GWL_STYLE) as u32 & WS_VISIBLE != 0 {
                    ShowWindow(button, SW_HIDE);
                }
                continue;
            }
            let left = (x * scale).round() as i32;
            let top = (y * scale).round() as i32;
            let width = ((x + w) * scale).round() as i32 - left;
            let height_px = ((y + h) * scale).round() as i32 - top;
            SetWindowPos(
                button,
                null_mut(),
                left,
                top,
                width,
                height_px,
                SWP_NOZORDER | SWP_NOACTIVATE | SWP_NOCOPYBITS | SWP_NOREDRAW,
            );
            // Child windows must share the software background's viewport clip.
            // A native region also clips hit testing and WS_CLIPCHILDREN, avoiding
            // missing whole rows or controls drawing across the fixed chrome.
            let clip_top = ((CONTENT_TOP * scale).ceil() as i32 - top).clamp(0, height_px);
            let clip_bottom = (((height - CONTENT_BOTTOM) * scale).floor() as i32 - top)
                .clamp(clip_top, height_px);
            let region = CreateRectRgn(0, clip_top, width, clip_bottom);
            if SetWindowRgn(button, region, 0) == 0 {
                DeleteObject(region);
            }
            if GetWindowLongW(button, GWL_STYLE) as u32 & WS_VISIBLE == 0 {
                ShowWindow(button, SW_SHOWNA);
            }
            InvalidateRect(button, null(), 0);
        }
        for (id, x, y, w) in [
            (1, 210.0, 16.0, 32.0),
            (2, 24.0, height - 46.0, footer_width),
            (3, 32.0 + footer_width, height - 46.0, footer_width),
        ] {
            let button = GetDlgItem(hwnd, id);
            SetWindowPos(
                button,
                null_mut(),
                (x * scale).round() as i32,
                (y * scale) as i32,
                (w * scale) as i32,
                (30.0 * scale) as i32,
                SWP_NOZORDER | SWP_NOACTIVATE | SWP_NOCOPYBITS | SWP_NOREDRAW,
            );
            InvalidateRect(button, null(), 0);
            if id == 1 {
                ShowWindow(button, if about { SW_SHOWNA } else { SW_HIDE });
            }
        }
        if visible_window {
            SendMessageW(hwnd, WM_SETREDRAW, 1, 0);
            RedrawWindow(
                hwnd,
                null(),
                null_mut(),
                RDW_INVALIDATE | RDW_ALLCHILDREN | RDW_UPDATENOW,
            );
        }
    }
    trace_ui("settings", "reposition", started);
}

unsafe extern "system" fn proc(
    hwnd: HWND,
    message: u32,
    wparam: WPARAM,
    lparam: LPARAM,
) -> LRESULT {
    unsafe {
        if message == WM_NCDESTROY {
            KillTimer(hwnd, UPDATE_POLL);
            let ptr = GetWindowLongPtrW(hwnd, GWLP_USERDATA) as *mut Popup;
            SetWindowLongPtrW(hwnd, GWLP_USERDATA, 0);
            WINDOW.with(|window| {
                if window.get() == hwnd {
                    window.set(null_mut());
                }
            });
            if !ptr.is_null() {
                drop(Box::from_raw(ptr));
            }
            return DefWindowProcW(hwnd, message, wparam, lparam);
        }
        let ptr = GetWindowLongPtrW(hwnd, GWLP_USERDATA) as *const Popup;
        if ptr.is_null() {
            return DefWindowProcW(hwnd, message, wparam, lparam);
        }
        match message {
            WM_GETMINMAXINFO => {
                let scale =
                    windows_sys::Win32::UI::HiDpi::GetDpiForWindow(hwnd).max(96) as f32 / 96.0;
                let (x, y) = client_outer_size(
                    (WIDTH * scale).round() as i32,
                    (300.0 * scale).round() as i32,
                    (scale * 96.0) as u32,
                );
                (*(lparam as *mut MINMAXINFO)).ptMinTrackSize = POINT { x, y };
                0
            }
            WM_SIZE if wparam != SIZE_MINIMIZED as usize => {
                // Reflow never changes the user's outer rectangle.
                let _ = reflow(hwnd, &(*ptr).state);
                0
            }
            WM_DPICHANGED => {
                // Copy the temporary suggested rectangle and defer native calls and
                // State mutation until the message loop has no outstanding borrow.
                if lparam != 0 {
                    (*ptr).suggested.set(Some(*(lparam as *const RECT)));
                }
                PostMessageW(hwnd, RELAYOUT, 0, 0);
                0
            }
            RELAYOUT => {
                if let Some(mut rect) = (*ptr).suggested.take() {
                    let mut info = MONITORINFO {
                        cbSize: size_of::<MONITORINFO>() as u32,
                        ..Default::default()
                    };
                    if GetMonitorInfoW(MonitorFromRect(&rect, MONITOR_DEFAULTTONEAREST), &mut info)
                        != 0
                    {
                        let width = (rect.right - rect.left)
                            .min(info.rcWork.right - info.rcWork.left)
                            .max(1);
                        let height = (rect.bottom - rect.top)
                            .min(info.rcWork.bottom - info.rcWork.top)
                            .max(1);
                        let origin = clamp_anchor(
                            POINT {
                                x: rect.left,
                                y: rect.top,
                            },
                            info.rcWork,
                            width,
                            height,
                        );
                        rect = RECT {
                            left: origin.x,
                            top: origin.y,
                            right: origin.x + width,
                            bottom: origin.y + height,
                        };
                    }
                    SetWindowPos(
                        hwnd,
                        null_mut(),
                        rect.left,
                        rect.top,
                        rect.right - rect.left,
                        rect.bottom - rect.top,
                        SWP_NOZORDER | SWP_NOACTIVATE,
                    );
                }
                let _ = reflow(hwnd, &(*ptr).state);
                0
            }
            WM_COMMAND => {
                if wparam >> 16 == BN_CLICKED as usize {
                    PostMessageW(hwnd, COMMAND, wparam & 0xffff, 0);
                }
                0
            }
            WM_TIMER if wparam == UPDATE_POLL => {
                let changed = (*ptr).state.borrow_mut().updates.poll();
                if changed {
                    refresh_local(hwnd, &(*ptr).state);
                }
                0
            }
            _ if (message == SCROLL_FRAME || message == WM_TIMER)
                && (*ptr).state.borrow().frame_pump.is_frame(message, wparam) =>
            {
                let now = Instant::now();
                if (*ptr).state.borrow_mut().frame_pump.should_draw(now) {
                    advance_scroll(hwnd, &(*ptr).state, now);
                }
                0
            }
            COMMAND => {
                let state = &(*ptr).state;
                let id = wparam;
                if id == 1 && !state.borrow().about {
                    DestroyWindow(hwnd);
                    return 0;
                }
                let action = state
                    .borrow()
                    .controls
                    .get(id.saturating_sub(100))
                    .and_then(|c| c.action);
                if let Some(action) = action {
                    let mut send = None;
                    let mut open = None;
                    {
                        let mut state = state.borrow_mut();
                        match action {
                            LocalAction::CaptureShortcut => {
                                let current = state.settings.shortcut;
                                state.shortcut.begin(current);
                            }
                            LocalAction::ApplyShortcut => {
                                if state.shortcut.active {
                                    send = state.shortcut.pending.map(TrayCommand::Shortcut);
                                    if send.is_some() {
                                        state.shortcut.active = false;
                                        state.shortcut.feedback = "Applying shortcut...";
                                    }
                                }
                            }
                            LocalAction::CancelShortcut => state.shortcut.cancel(),
                            LocalAction::DefaultShortcut => {
                                state.shortcut.begin(Shortcut::default());
                                state.shortcut.feedback = "Choose Apply to use this shortcut.";
                            }
                            LocalAction::CheckUpdate => {
                                state.updates.start();
                            }
                            LocalAction::OpenRelease => {
                                open = release_url(state.updates.status()).map(str::to_owned);
                            }
                            LocalAction::ShortcutStatus | LocalAction::UpdateStatus => {}
                        }
                    }
                    refresh_local(hwnd, state);
                    if let Some(url) = open {
                        windows_sys::Win32::UI::Shell::ShellExecuteW(
                            hwnd,
                            wide("open").as_ptr(),
                            wide(&url).as_ptr(),
                            null(),
                            null(),
                            SW_SHOWNORMAL,
                        );
                    }
                    if let Some(command) = send {
                        COMMANDS.with(|commands| commands.borrow_mut().push(command));
                        PostMessageW((*ptr).owner, MESSAGE, 0, 0);
                    }
                    return 0;
                }
                let command = match id {
                    1 => Some(TrayCommand::About),
                    2 => Some(TrayCommand::Restart),
                    3 => Some(TrayCommand::Exit),
                    _ => state
                        .borrow()
                        .controls
                        .get(id.saturating_sub(100))
                        .map(|c| c.command),
                };
                if let Some(command) = command {
                    if command == TrayCommand::About {
                        let about = !state.borrow().about;
                        {
                            let mut state = state.borrow_mut();
                            state.about = about;
                            let scroll = state.scroll;
                            state.scroll_motion.jump(scroll);
                        }
                        state.borrow_mut().frame_pump.stop();
                        SetWindowTextW(
                            GetDlgItem(hwnd, 1),
                            wide(tr(if about { "Back to settings" } else { "Close" })).as_ptr(),
                        );
                        position_controls(hwnd, state);
                        InvalidateRect(hwnd, null(), 0);
                    } else {
                        COMMANDS.with(|commands| commands.borrow_mut().push(command));
                        PostMessageW((*ptr).owner, MESSAGE, 0, 0);
                    }
                }
                0
            }
            WM_ACTIVATE if wparam as u16 == WA_INACTIVE as u16 => {
                let focus = GetFocus();
                if IsChild(hwnd, focus) != 0 {
                    (*ptr).focus.set(focus);
                }
                0
            }
            REMEMBER_FOCUS => {
                (*ptr).focus.set(wparam as HWND);
                0
            }
            WM_SETFOCUS => {
                let focus = (*ptr).focus.get();
                SetFocus(
                    if !focus.is_null() && IsChild(hwnd, focus) != 0 && IsWindowVisible(focus) != 0
                    {
                        focus
                    } else {
                        GetDlgItem(hwnd, crate::window_chrome::CLOSE)
                    },
                );
                0
            }
            WM_ERASEBKGND => 1,
            WM_PAINT => {
                let started = trace_start();
                let mut ps = PAINTSTRUCT::default();
                let dc = BeginPaint(hwnd, &mut ps);
                let mut borrowed = (*ptr).state.borrow_mut();
                let state = &mut *borrowed;
                let mut client = RECT::default();
                GetClientRect(hwnd, &mut client);
                let size = (client.right.max(0) as u32, client.bottom.max(0) as u32);
                state.width = size.0 as f32 / state.scale;
                state.height = size.1 as f32 / state.scale;
                let key = (
                    size,
                    state.scale.to_bits(),
                    state.height.to_bits(),
                    state.scroll.to_bits(),
                    state.about,
                );
                let pixels = match state.background.take() {
                    Some(background) if background.key == key => background.pixels,
                    _ => {
                        let content_size = size;
                        let mut pixels = [8, 8, 8, 255]
                            .repeat(content_size.0 as usize * content_size.1 as usize);
                        if !state.about {
                            copy_scrolled_content(state, &mut pixels, content_size);
                        }
                        let mut c = Canvas::new(&mut pixels, content_size, &mut state.painter);
                        c.text(
                            if state.about {
                                "HiyoDyna"
                            } else {
                                tr("Settings")
                            },
                            Rect::new(24.0, 16.0, 170.0, 25.0),
                            22.0,
                            true,
                            WHITE,
                        );
                        c.text(
                            tr(if state.about {
                                "ABOUT"
                            } else {
                                "Appearance, motion and playback"
                            }),
                            Rect::new(25.0, 42.0, 200.0, 16.0),
                            10.0,
                            false,
                            MUTED,
                        );
                        let old_clip = c.clip;
                        c.clip = Rect::new(
                            0.0,
                            CONTENT_TOP,
                            state.width,
                            (state.height - CONTENT_TOP - CONTENT_BOTTOM).max(0.0),
                        );
                        if state.about {
                            c.text(
                                tr("A little space. More possibility."),
                                Rect::new(26.0, 97.0, 310.0, 28.0),
                                17.0,
                                true,
                                WHITE,
                            );
                            c.text(
                                concat!("HiyoDyna ", env!("CARGO_PKG_VERSION")),
                                Rect::new(26.0, 144.0, 310.0, 24.0),
                                12.0,
                                false,
                                MUTED,
                            );
                            for (i, line) in [
                                "Built with Rust and native Windows APIs.",
                                "Lyrics by AMLL TTML DB and LRCLIB.",
                                "Inspired by Spicy Lyrics.",
                                "AGPL-3.0 · github.com/hiyo41/HiyoDyna",
                                "Scroll settings to explore all controls.",
                                "Press Escape or Close to return.",
                            ]
                            .iter()
                            .enumerate()
                            {
                                c.text(
                                    tr(line),
                                    Rect::new(26.0, 188.0 + i as f32 * 28.0, 310.0, 24.0),
                                    11.0,
                                    false,
                                    MUTED,
                                );
                            }
                        }
                        c.clip = old_clip;
                        if !state.about {
                            let visible = (state.height - CONTENT_TOP - CONTENT_BOTTOM).max(0.0);
                            let content_height = SettingsLayout::new(state.width).content_height();
                            let thumb = (visible * visible / content_height).min(visible);
                            c.rect(
                                Rect::new(
                                    state.width - 14.0,
                                    68.0 + state.scroll / content_height * visible,
                                    2.0,
                                    thumb,
                                ),
                                1.0,
                                MUTED,
                                0.22,
                            );
                        }
                        c.rect(
                            Rect::new(22.0, state.height - 59.0, state.width - 44.0, 1.0),
                            0.0,
                            WHITE,
                            0.07,
                        );
                        pixels
                    }
                };
                trace_ui("settings", "draw", started);
                let present_started = trace_start();
                if state.surface.resize(size.0, size.1) {
                    state.surface.pixels_mut().copy_from_slice(&pixels);
                    state.surface.present(dc, ps.rcPaint);
                }
                state.background = Some(Background { key, pixels });
                drop(borrowed);
                EndPaint(hwnd, &ps);
                trace_ui("settings", "present", present_started);
                0
            }
            WM_DRAWITEM => {
                let item = &*(lparam as *const DRAWITEMSTRUCT);
                let size = (
                    (item.rcItem.right - item.rcItem.left) as u32,
                    (item.rcItem.bottom - item.rcItem.top) as u32,
                );
                let enabled =
                    windows_sys::Win32::UI::Input::KeyboardAndMouse::IsWindowEnabled(item.hwndItem)
                        != 0;
                let pixels = button_raster(&mut (*ptr).state.borrow_mut(), item, enabled);
                // WM_DRAWITEM can receive a cached/print DC without the child's
                // window region. Enforce the region on every owner-draw blit,
                // intersecting the caller's existing clip and restoring it after.
                let saved = SaveDC(item.hDC);
                if saved == 0 {
                    return 1;
                }
                let region = CreateRectRgn(0, 0, 0, 0);
                if !item.hwndItem.is_null() && GetWindowRgn(item.hwndItem, region) != ERROR {
                    ExtSelectClipRgn(item.hDC, region, RGN_AND);
                }
                DeleteObject(region);
                blit(item.hDC, &pixels, size);
                RestoreDC(item.hDC, saved);
                1
            }
            WM_CLOSE => {
                DestroyWindow(hwnd);
                0
            }
            _ => DefWindowProcW(hwnd, message, wparam, lparam),
        }
    }
}

#[cfg(test)]
pub(crate) mod tests {
    use super::*;
    pub(super) static KEYBOARD_DONE: std::sync::atomic::AtomicBool =
        std::sync::atomic::AtomicBool::new(false);

    #[test]
    fn expanded_settings_reflow_groups_and_reduce_scrolling() {
        let layout = SettingsLayout::new(920.0);
        let left = layout.rect(Rect::new(32.0, 34.0, 287.0, 29.0));
        let right = layout.rect(Rect::new(32.0, 658.0, 287.0, 29.0));
        assert!(right.x > 460.0, "Application belongs in the second column");
        assert!(
            right.y < 50.0,
            "Application starts at the top when expanded"
        );
        assert!(left.w > 350.0, "controls should grow with the content area");
        assert!(
            layout.content_height() < 850.0,
            "wide layout should need less scrolling"
        );
        assert!((1.0..=1.25).contains(&layout.text_scale()));
        let controls = controls_with_autostart(&Settings::default(), false);
        for (i, control) in controls.iter().enumerate() {
            let r = layout.rect(control.rect);
            assert!(r.x >= 20.0 && r.x + r.w <= 900.0);
            for other in &controls[i + 1..] {
                let overlap = r.intersect(layout.rect(other.rect));
                assert!(
                    overlap.w < 0.1 || overlap.h < 0.1,
                    "controls overlap after reflow"
                );
            }
        }
    }

    #[test]
    #[ignore = "Writes Settings renderer previews across languages, layouts and DPI; run serially"]
    fn responsive_settings_render_matrix() {
        let dir = std::path::PathBuf::from(
            std::env::var_os("HIYODYNA_PREVIEW_DIR").expect("preview directory"),
        );
        let old = crate::i18n::language();
        for language in [Language::English, Language::Vietnamese] {
            crate::i18n::set_language(language);
            for scale in [1.0, 1.25, 1.5, 2.0] {
                for (name, width, height) in [("normal", 420.0, 640.0), ("wide", 920.0, 760.0)] {
                    let popup = HiddenPopup::new(scale);
                    unsafe {
                        SetWindowPos(
                            popup.hwnd,
                            null_mut(),
                            0,
                            0,
                            (width * scale) as i32,
                            (height * scale) as i32,
                            SWP_NOMOVE | SWP_NOZORDER | SWP_NOACTIVATE,
                        );
                    }
                    popup.paint();
                    let mut pixels = popup.pixels();
                    let size = ((width * scale) as u32, (height * scale) as u32);
                    let layout = SettingsLayout::new(width);
                    let mut state = popup.popup.state.borrow_mut();
                    for index in 0..state.controls.len() {
                        let r = layout.rect(state.controls[index].rect);
                        let left = (r.x * scale).round() as usize;
                        let top = ((CONTENT_TOP + r.y) * scale).round() as usize;
                        let w = ((r.x + r.w) * scale).round() as usize - left;
                        let h = ((CONTENT_TOP + r.y + r.h) * scale).round() as usize - top;
                        let item = DRAWITEMSTRUCT {
                            CtlID: index as u32 + 100,
                            rcItem: RECT {
                                left: 0,
                                top: 0,
                                right: w as i32,
                                bottom: h as i32,
                            },
                            ..Default::default()
                        };
                        let raster = button_raster(&mut state, &item, true);
                        for row in 0..h {
                            let y = top + row;
                            if y >= ((height - CONTENT_BOTTOM) * scale).floor() as usize {
                                break;
                            }
                            assert!(left + w <= size.0 as usize);
                            let dst = (y * size.0 as usize + left) * 4;
                            pixels[dst..dst + w * 4]
                                .copy_from_slice(&raster[row * w * 4..(row + 1) * w * 4]);
                        }
                    }
                    for p in pixels.as_chunks_mut::<4>().0 {
                        p.swap(0, 2);
                    }
                    image::RgbaImage::from_raw(size.0, size.1, pixels)
                        .unwrap()
                        .save(dir.join(format!(
                            "settings-responsive-{name}-{}-{}.png",
                            language.as_str(),
                            (scale * 100.0) as u32
                        )))
                        .unwrap();
                }
            }
        }
        crate::i18n::set_language(old);
    }

    pub(crate) struct NativeTestKeyboard([u8; 256]);

    impl NativeTestKeyboard {
        pub(crate) unsafe fn new(reverse: bool) -> Self {
            unsafe {
                use windows_sys::Win32::UI::Input::KeyboardAndMouse::{
                    GetKeyboardState, SetKeyboardState, VK_CONTROL, VK_LCONTROL, VK_LMENU,
                    VK_LSHIFT, VK_MENU, VK_RCONTROL, VK_RMENU, VK_RSHIFT, VK_SHIFT,
                };
                let mut saved = [0_u8; 256];
                assert_ne!(GetKeyboardState(saved.as_mut_ptr()), 0);
                let guard = Self(saved);
                let mut keyboard = saved;
                // Synthetic Tab messages have no matching OS modifier events.
                // Specify their direction only on this thread and restore it.
                for key in [
                    VK_SHIFT,
                    VK_LSHIFT,
                    VK_RSHIFT,
                    VK_CONTROL,
                    VK_LCONTROL,
                    VK_RCONTROL,
                    VK_MENU,
                    VK_LMENU,
                    VK_RMENU,
                ] {
                    keyboard[key as usize] &= 0x7f;
                }
                if reverse {
                    keyboard[VK_SHIFT as usize] |= 0x80;
                    keyboard[VK_LSHIFT as usize] |= 0x80;
                }
                assert_ne!(SetKeyboardState(keyboard.as_ptr()), 0);
                guard
            }
        }
    }

    impl Drop for NativeTestKeyboard {
        fn drop(&mut self) {
            unsafe {
                windows_sys::Win32::UI::Input::KeyboardAndMouse::SetKeyboardState(self.0.as_ptr());
            }
        }
    }

    #[test]
    fn scroll_repaints_reuse_button_pixels_and_refresh_changed_states() {
        let mut state = state_for_test(Settings::default(), 1.0);
        let mut item = DRAWITEMSTRUCT {
            CtlType: ODT_BUTTON,
            CtlID: 100,
            rcItem: RECT {
                left: 0,
                top: 0,
                right: 100,
                bottom: 29,
            },
            ..Default::default()
        };
        let first = button_raster(&mut state, &item, true);
        state.scroll = 25.0;
        let scrolled = button_raster(&mut state, &item, true);
        assert!(
            std::sync::Arc::ptr_eq(&first, &scrolled),
            "moving an unchanged native button must reuse its finished pixels"
        );
        item.itemState = ODS_FOCUS;
        let focused = button_raster(&mut state, &item, true);
        assert_ne!(
            first, focused,
            "keyboard focus must invalidate the button image"
        );
        item.itemState = ODS_HOTLIGHT;
        let hovered = button_raster(&mut state, &item, true);
        assert_ne!(
            first, hovered,
            "pointer hover must invalidate the button image"
        );
        let disabled = button_raster(&mut state, &item, false);
        assert_ne!(
            hovered, disabled,
            "disabled text must not retain enabled pixels"
        );
        item.itemState = 0;
        state.controls[0].label = "Different label".into();
        let relabeled = button_raster(&mut state, &item, true);
        assert_ne!(
            first, relabeled,
            "translated or updated labels must invalidate the button image"
        );
        state.controls[0].selected = !state.controls[0].selected;
        let selected = button_raster(&mut state, &item, true);
        assert_ne!(
            relabeled, selected,
            "selection must invalidate the button image"
        );
        for width in 100..180 {
            item.rcItem.right = width;
            assert_eq!(
                button_raster(&mut state, &item, true).len(),
                width as usize * 29 * 4
            );
        }
        assert_eq!(
            state.button_rasters.len(),
            1,
            "resize and interaction variants must replace the previous image for that control"
        );
    }

    #[test]
    fn utility_preferences_include_compact_language_shortcut_and_manual_update_controls() {
        let settings = Settings::default();
        let controls = controls_with_autostart(&settings, false);
        assert!(
            controls
                .iter()
                .any(|c| c.command == TrayCommand::CompactLyrics(false))
        );
        assert_eq!(
            controls
                .iter()
                .filter(|c| matches!(c.command, TrayCommand::Language(_)))
                .count(),
            3
        );
        assert!(
            controls
                .iter()
                .any(|c| c.action == Some(LocalAction::CaptureShortcut))
        );
        assert!(
            controls
                .iter()
                .any(|c| c.action == Some(LocalAction::CheckUpdate))
        );
        for control in controls {
            assert!(control.rect.y >= 0.0 && control.rect.y + control.rect.h <= CONTENT);
        }
    }

    #[test]
    fn unavailable_shortcut_is_omitted_from_settings_action_hint() {
        let settings = Settings::default();
        for available in [false, true] {
            let controls = controls_with_shortcut(&settings, false, available);
            let action = controls
                .iter()
                .find(|c| c.command == TrayCommand::TogglePassthrough)
                .unwrap();
            assert_eq!(
                action.label.contains(&settings.shortcut.label()),
                available,
                "Settings must advertise only an effective global shortcut"
            );
            assert!(!action.label.is_empty());
        }
    }

    #[test]
    fn shortcut_capture_keeps_effective_binding_until_apply_and_cancel_restores_it() {
        let original = crate::hotkey::Shortcut::default();
        let mut capture = ShortcutCapture::default();
        capture.begin(original);
        let next = crate::hotkey::Shortcut::parse("6:75").unwrap();
        capture.candidate(Some(next));
        assert!(capture.active);
        assert_eq!(capture.pending, Some(next));
        capture.cancel();
        assert!(!capture.active);
        assert_eq!(capture.pending, None);
        assert_eq!(original, crate::hotkey::Shortcut::default());
    }

    #[test]
    fn shortcut_capture_rejects_reserved_chords_without_losing_capture() {
        let mut capture = ShortcutCapture::default();
        capture.begin(Shortcut::default());
        for shortcut in [
            Shortcut {
                modifiers: 1,
                key: 0x73,
            },
            Shortcut {
                modifiers: 3,
                key: 0x2e,
            },
            Shortcut {
                modifiers: 8,
                key: 0x4c,
            },
            Shortcut {
                modifiers: 3,
                key: 0x7b,
            },
        ] {
            capture.candidate(Some(shortcut));
            assert!(capture.active);
            assert_eq!(
                capture.pending,
                None,
                "reserved chord: {}",
                shortcut.label()
            );
        }
    }

    #[test]
    fn utility_copy_has_english_and_vietnamese_labels_without_mutating_global_language() {
        for key in [
            "Compact lyrics",
            "Language",
            "System",
            "Shortcut",
            "Apply",
            "Cancel",
            "Restore default shortcut",
            "Check for updates",
            "Checking...",
            "Retry",
            "Open release page",
            "Minimize",
            "Maximize",
            "Restore",
            "Close",
        ] {
            let english = crate::i18n::tr_in(Language::English, key);
            let vietnamese = crate::i18n::tr_in(Language::Vietnamese, key);
            assert_eq!(english, key);
            assert!(!vietnamese.is_empty());
            assert_ne!(
                english, vietnamese,
                "Missing Vietnamese utility label: {key}"
            );
        }
    }

    #[test]
    fn settings_row_labels_fit_both_languages_at_each_dpi() {
        for scale in [1.0, 1.25, 1.5, 2.0] {
            let mut painter = Painter::new(scale).unwrap();
            for language in [Language::English, Language::Vietnamese] {
                for key in [
                    "Accent",
                    "Soft shadow",
                    "Speed",
                    "Bounce",
                    "Time format",
                    "Seconds",
                    "Top gap",
                    "Dock position",
                    "Lyrics",
                    "Timing offset",
                ] {
                    let label = crate::i18n::tr_in(language, key);
                    assert!(
                        painter.width(label, 11.0, false) <= 99.0,
                        "Settings row label is clipped at {scale}x in {language:?}: {label}"
                    );
                }
            }
        }
    }

    fn state_for_test(settings: Settings, scale: f32) -> State {
        let mut controls = controls_with_autostart(&settings, false);
        for (index, control) in controls.iter_mut().enumerate() {
            control.hwnd = (index + 100) as HWND;
        }
        State {
            painter: Painter::new(scale).unwrap(),
            surface: WindowSurface::new().unwrap(),
            frame_pump: FramePump::new(SCROLL_FRAME),
            settings,
            controls,
            scale,
            height: 760.0,
            width: WIDTH,
            scroll: 0.0,
            scroll_motion: SmoothScroll::default(),
            about: false,
            background: None,
            content: None,
            button_rasters: Vec::new(),
            shortcut: ShortcutCapture::default(),
            shortcut_available: true,
            updates: crate::update::CheckSession::new(settings.update),
        }
    }

    struct HiddenPopup {
        hwnd: HWND,
        popup: Box<Popup>,
    }

    impl HiddenPopup {
        fn new(scale: f32) -> Self {
            unsafe {
                let hwnd = CreateWindowExW(
                    0,
                    wide("STATIC").as_ptr(),
                    null(),
                    WS_POPUP,
                    0,
                    0,
                    (WIDTH * scale) as i32,
                    (760.0 * scale) as i32,
                    null_mut(),
                    null_mut(),
                    GetModuleHandleW(null()),
                    null(),
                );
                assert!(!hwnd.is_null());
                let popup = Box::new(Popup {
                    state: RefCell::new(state_for_test(Settings::default(), scale)),
                    suggested: Cell::new(None),
                    owner: null_mut(),
                    focus: Cell::new(null_mut()),
                });
                SetWindowLongPtrW(hwnd, GWLP_USERDATA, (&*popup as *const Popup) as isize);
                Self { hwnd, popup }
            }
        }

        fn paint(&self) {
            unsafe {
                InvalidateRect(self.hwnd, null(), 0);
                proc(self.hwnd, WM_PAINT, 0, 0);
            }
        }

        fn pixels(&self) -> Vec<u8> {
            self.popup
                .state
                .borrow()
                .background
                .as_ref()
                .unwrap()
                .pixels
                .clone()
        }
    }

    #[test]
    fn native_window_style_does_not_enable_composited_painting() {
        unsafe {
            let class = wide("HiyoDynaSettingsStyleTest");
            let instance = GetModuleHandleW(null());
            RegisterClassW(&WNDCLASSW {
                lpfnWndProc: Some(DefWindowProcW),
                hInstance: instance,
                lpszClassName: class.as_ptr(),
                ..Default::default()
            });
            let hwnd = CreateWindowExW(
                0,
                class.as_ptr(),
                null(),
                WS_POPUP,
                0,
                0,
                64,
                32,
                null_mut(),
                null_mut(),
                instance,
                null(),
            );
            assert!(!hwnd.is_null());
            crate::native_ui::style_window(hwnd);
            let style = GetWindowLongPtrW(hwnd, GWL_EXSTYLE);
            DestroyWindow(hwnd);
            assert_eq!(style & WS_EX_COMPOSITED as isize, 0);
        }
    }

    #[test]
    fn paint_presents_the_persistent_surface_once() {
        let popup = HiddenPopup::new(1.0);
        popup.paint();
        let address = {
            let mut state = popup.popup.state.borrow_mut();
            assert_eq!(state.surface.presents, 1);
            state.surface.pixels_mut().as_ptr()
        };
        popup.paint();
        let mut state = popup.popup.state.borrow_mut();
        assert_eq!(state.surface.presents, 2);
        assert_eq!(state.surface.pixels_mut().as_ptr(), address);
    }

    impl Drop for HiddenPopup {
        fn drop(&mut self) {
            unsafe {
                DestroyWindow(self.hwnd);
            }
        }
    }

    #[test]
    fn wheel_input_keeps_the_current_frame_until_scroll_animation_advances() {
        let popup = HiddenPopup::new(1.0);
        WINDOW.with(|window| window.set(popup.hwnd));
        let handled = translate_message(&MSG {
            hwnd: popup.hwnd,
            message: WM_MOUSEWHEEL,
            wParam: (-120_i16 as u16 as usize) << 16,
            ..Default::default()
        });
        WINDOW.with(|window| window.set(null_mut()));
        assert!(handled);
        assert_eq!(
            popup.popup.state.borrow().scroll,
            0.0,
            "wheel input must schedule smooth motion instead of jumping one notch"
        );
        let now = Instant::now();
        unsafe {
            advance_scroll(
                popup.hwnd,
                &popup.popup.state,
                now + std::time::Duration::from_millis(30),
            );
        }
        let middle = popup.popup.state.borrow().scroll;
        assert!(middle > 0.0 && middle < 60.0);
        unsafe {
            advance_scroll(
                popup.hwnd,
                &popup.popup.state,
                now + std::time::Duration::from_secs(1),
            );
        }
        let state = popup.popup.state.borrow();
        assert_eq!(state.scroll, 60.0);
        assert!(!state.scroll_motion.moving());
        assert!(!state.frame_pump.is_active());
    }

    #[test]
    #[ignore = "Measures a warmed 760x680 Settings scroll frame without foreground interaction"]
    fn persistent_surface_scroll_frame_timing() {
        let popup = HiddenPopup::new(1.0);
        unsafe {
            SetWindowPos(
                popup.hwnd,
                null_mut(),
                0,
                0,
                760,
                680,
                SWP_NOMOVE | SWP_NOZORDER | SWP_NOACTIVATE,
            );
            // Renderer fixtures use placeholder handles. Give this timing probe
            // real native children so repositioning never targets those values.
            let instance = GetModuleHandleW(null());
            for (index, control) in popup
                .popup
                .state
                .borrow_mut()
                .controls
                .iter_mut()
                .enumerate()
            {
                control.hwnd = CreateWindowExW(
                    0,
                    wide("BUTTON").as_ptr(),
                    wide(&control.label).as_ptr(),
                    WS_CHILD | WS_VISIBLE | BS_OWNERDRAW as u32,
                    0,
                    0,
                    1,
                    1,
                    popup.hwnd,
                    (index + 100) as *mut _,
                    instance,
                    null(),
                );
                assert!(!control.hwnd.is_null());
            }
            for id in 1..=3 {
                assert!(
                    !CreateWindowExW(
                        0,
                        wide("BUTTON").as_ptr(),
                        null(),
                        WS_CHILD | WS_VISIBLE | BS_OWNERDRAW as u32,
                        0,
                        0,
                        1,
                        1,
                        popup.hwnd,
                        id as *mut _,
                        instance,
                        null()
                    )
                    .is_null()
                );
            }
            position_controls(popup.hwnd, &popup.popup.state);
        }
        popup.paint();
        popup.popup.state.borrow_mut().scroll = 40.0;
        let started = Instant::now();
        unsafe {
            position_controls(popup.hwnd, &popup.popup.state);
        }
        popup.paint();
        let elapsed = started.elapsed();
        eprintln!(
            "settings 760x680 scroll frame={:.3}ms",
            elapsed.as_secs_f64() * 1000.0
        );
        assert!(
            elapsed < std::time::Duration::from_millis(12),
            "{elapsed:?}"
        );
    }

    #[test]
    #[ignore = "Measures Settings software background scrolling without foreground interaction"]
    fn settings_scroll_render_benchmark() {
        for scale in [1.0, 1.25, 1.5, 2.0] {
            let popup = HiddenPopup::new(scale);
            popup.paint();
            let mut samples = Vec::new();
            for frame in 0..48 {
                popup.popup.state.borrow_mut().scroll = (frame % 12) as f32 * 44.25;
                let start = std::time::Instant::now();
                popup.paint();
                samples.push(start.elapsed().as_secs_f64() * 1000.0);
            }
            samples.sort_by(f64::total_cmp);
            eprintln!(
                "settings-scroll {}% mean={:.3}ms p95={:.3}ms max={:.3}ms",
                scale * 100.0,
                samples.iter().sum::<f64>() / samples.len() as f64,
                samples[45],
                samples[47],
            );
        }
    }

    #[test]
    fn selected_controls_keep_their_selection_when_keyboard_focused() {
        let scale = 1.5;
        let mut painter = Painter::new(scale).unwrap();
        let size = (89, 44);
        let mut normal = vec![0; (size.0 * size.1 * 4) as usize];
        let mut focused = normal.clone();
        let accent = Settings::default().accent_bgra();
        crate::native_ui::button(
            &mut Canvas::new(&mut normal, size, &mut painter),
            "Ice",
            crate::native_ui::ButtonStyle {
                accent,
                selected: true,
                ..Default::default()
            },
        );
        crate::native_ui::button(
            &mut Canvas::new(&mut focused, size, &mut painter),
            "Ice",
            crate::native_ui::ButtonStyle {
                accent,
                selected: true,
                focused: true,
                ..Default::default()
            },
        );
        let center = ((15 * size.0 + 15) * 4) as usize;
        assert_eq!(
            normal[center..center + 3],
            focused[center..center + 3],
            "focus must not replace the selected surface"
        );
        assert_ne!(
            normal, focused,
            "keyboard focus needs its own visible outline"
        );
    }

    #[test]
    fn printed_button_pixels_respect_partial_viewport_and_preserve_caller_clip() {
        unsafe extern "system" fn parent_proc(
            hwnd: HWND,
            message: u32,
            wparam: WPARAM,
            lparam: LPARAM,
            _id: usize,
            _data: usize,
        ) -> LRESULT {
            unsafe {
                if message == WM_DRAWITEM {
                    return proc(hwnd, message, wparam, lparam);
                }
                DefSubclassProc(hwnd, message, wparam, lparam)
            }
        }
        for (scale, height, scroll, y, width, rows, first, last) in [
            (1.0, 760.0, 44.0, 34.0, 59, 29, 10, 29),
            (1.0, 300.0, 0.0, 151.0, 59, 29, 0, 19),
            (1.25, 760.0, 44.0, 34.0, 74, 36, 12, 36),
            (1.25, 300.0, 0.0, 151.0, 74, 36, 0, 23),
            (1.5, 760.0, 44.0, 34.0, 89, 44, 15, 44),
            (1.5, 300.0, 0.0, 151.0, 89, 43, 0, 28),
            (2.0, 760.0, 44.0, 34.0, 118, 58, 20, 58),
            (2.0, 300.0, 0.0, 151.0, 118, 58, 0, 38),
        ] {
            let popup = HiddenPopup::new(scale);
            unsafe {
                SetWindowSubclass(popup.hwnd, Some(parent_proc), 2, 0);
                let button = CreateWindowExW(
                    0,
                    wide("BUTTON").as_ptr(),
                    wide("Accent: Ice").as_ptr(),
                    WS_CHILD | BS_CHECKBOX as u32 | BS_PUSHLIKE as u32,
                    0,
                    0,
                    59,
                    29,
                    popup.hwnd,
                    100 as *mut _,
                    GetModuleHandleW(null()),
                    null(),
                );
                assert!(!button.is_null());
                SetWindowSubclass(button, Some(button_proc), 1, 0);
                {
                    let mut state = popup.popup.state.borrow_mut();
                    for control in &mut state.controls {
                        control.hwnd = null_mut();
                    }
                    state.controls[0].hwnd = button;
                    state.controls[0].rect.y = y;
                    state.height = height;
                    state.scroll = scroll;
                }
                position_controls(popup.hwnd, &popup.popup.state);
                SendMessageW(button, BM_SETSTATE, 1, 0);
                let region = CreateRectRgn(0, 0, 0, 0);
                assert_ne!(GetWindowRgn(button, region), ERROR);
                let mut bounds = RECT::default();
                GetRgnBox(region, &mut bounds);
                DeleteObject(region);
                assert_eq!(
                    (bounds.top, bounds.bottom),
                    (first, last),
                    "state changes must preserve native clipping"
                );
                let dc = CreateCompatibleDC(null_mut());
                let info = BITMAPINFO {
                    bmiHeader: BITMAPINFOHEADER {
                        biSize: size_of::<BITMAPINFOHEADER>() as u32,
                        biWidth: width,
                        biHeight: -rows,
                        biPlanes: 1,
                        biBitCount: 32,
                        biCompression: BI_RGB,
                        ..Default::default()
                    },
                    ..Default::default()
                };
                let mut bits = null_mut();
                let bitmap = CreateDIBSection(dc, &info, DIB_RGB_COLORS, &mut bits, null_mut(), 0);
                assert!(!bitmap.is_null());
                let previous = SelectObject(dc, bitmap);
                let pixels =
                    std::slice::from_raw_parts_mut(bits.cast::<u8>(), (width * rows * 4) as usize);
                pixels.fill(203);
                IntersectClipRect(dc, 3, 0, width - 3, rows);
                SendMessageW(button, WM_PRINTCLIENT, dc as usize, PRF_CLIENT as isize);
                GdiFlush();
                let mut outside_changed = 0;
                let mut inside_changed = 0;
                for row in 0..rows {
                    for column in 0..width {
                        let index = ((row * width + column) * 4) as usize;
                        let changed = pixels[index..index + 4] != [203; 4];
                        if (first..last).contains(&row) && (3..width - 3).contains(&column) {
                            inside_changed += usize::from(changed);
                        } else {
                            outside_changed += usize::from(changed);
                        }
                    }
                }
                let mut clip = RECT::default();
                GetClipBox(dc, &mut clip);
                SelectObject(dc, previous);
                DeleteObject(bitmap);
                DeleteDC(dc);
                assert_eq!(
                    (clip.left, clip.top, clip.right, clip.bottom),
                    (3, 0, width - 3, rows),
                    "drawing must restore the caller's clip"
                );
                assert_eq!(
                    outside_changed, 0,
                    "owner drawing must not write into header/footer pixels"
                );
                assert!(inside_changed > 0, "visible button pixels must be painted");
            }
        }
    }

    #[test]
    fn native_window_destruction_releases_singleton_state() {
        unsafe {
            let class = wide("HiyoDynaPreferencesLifecycleTest");
            let instance = GetModuleHandleW(null());
            RegisterClassW(&WNDCLASSW {
                lpfnWndProc: Some(proc),
                hInstance: instance,
                lpszClassName: class.as_ptr(),
                ..Default::default()
            });
            let hwnd = CreateWindowExW(
                0,
                class.as_ptr(),
                null(),
                WS_OVERLAPPED,
                0,
                0,
                360,
                400,
                null_mut(),
                null_mut(),
                instance,
                null(),
            );
            assert!(!hwnd.is_null());
            let popup = Box::new(Popup {
                state: RefCell::new(state_for_test(Settings::default(), 1.0)),
                suggested: Cell::new(None),
                owner: null_mut(),
                focus: Cell::new(null_mut()),
            });
            SetWindowLongPtrW(hwnd, GWLP_USERDATA, Box::into_raw(popup) as isize);
            WINDOW.with(|window| window.set(hwnd));
            assert_ne!(DestroyWindow(hwnd), 0);
            assert!(WINDOW.with(Cell::get).is_null());
            assert_eq!(IsWindow(hwnd), 0);
            update(&Settings::default(), true);
            UnregisterClassW(class.as_ptr(), instance);
        }
    }

    #[test]
    fn partially_scrolled_buttons_are_clipped_to_content_viewport() {
        let popup = HiddenPopup::new(1.0);
        unsafe {
            let button = CreateWindowExW(
                0,
                wide("BUTTON").as_ptr(),
                wide("Accent: Ice").as_ptr(),
                WS_CHILD,
                0,
                0,
                59,
                29,
                popup.hwnd,
                100 as *mut _,
                GetModuleHandleW(null()),
                null(),
            );
            assert!(!button.is_null());
            {
                let mut state = popup.popup.state.borrow_mut();
                for control in &mut state.controls {
                    control.hwnd = null_mut();
                }
                state.controls[0].hwnd = button;
                state.scroll = 44.0;
            }
            position_controls(popup.hwnd, &popup.popup.state);
            assert_ne!(
                GetWindowLongW(button, GWL_STYLE) as u32 & WS_VISIBLE,
                0,
                "partially visible control must remain painted"
            );
            let region = CreateRectRgn(0, 0, 0, 0);
            assert_ne!(GetWindowRgn(button, region), ERROR);
            let mut bounds = RECT::default();
            GetRgnBox(region, &mut bounds);
            DeleteObject(region);
            assert_eq!(
                (bounds.left, bounds.top, bounds.right, bounds.bottom),
                (0, 10, 59, 29)
            );
        }
    }

    #[test]
    fn commands_queue_without_borrowing_parent_state() {
        let popup = HiddenPopup::new(1.0);
        let index = popup
            .popup
            .state
            .borrow()
            .controls
            .iter()
            .position(|c| c.command == TrayCommand::TogglePassthrough)
            .unwrap();
        unsafe {
            proc(popup.hwnd, COMMAND, index + 100, 0);
        }
        assert_eq!(take_commands(), [TrayCommand::TogglePassthrough]);
        assert!(take_commands().is_empty());
    }

    #[test]
    fn scrolled_background_cannot_overpaint_header_or_footer_at_fractional_dpi() {
        for scale in [1.0, 1.25, 1.5, 2.0] {
            let popup = HiddenPopup::new(scale);
            popup.paint();
            let before = popup.pixels();
            popup.popup.state.borrow_mut().scroll = 137.0;
            popup.paint();
            let after = popup.pixels();
            let row = (WIDTH * scale) as usize * 4;
            let top = (CONTENT_TOP * scale).floor() as usize * row;
            let bottom = ((760.0 - CONTENT_BOTTOM) * scale).ceil() as usize * row;
            assert!(before[..top] == after[..top], "header at scale {scale}");
            assert!(
                before[bottom..] == after[bottom..],
                "footer at scale {scale}"
            );
            assert_ne!(before[top..bottom], after[top..bottom]);
        }
    }

    #[test]
    fn minimum_track_size_uses_full_client_custom_frame() {
        let popup = HiddenPopup::new(1.0);
        unsafe {
            let mut info = MINMAXINFO::default();
            proc(
                popup.hwnd,
                WM_GETMINMAXINFO,
                0,
                (&mut info as *mut MINMAXINFO) as isize,
            );
            assert_eq!(info.ptMinTrackSize.x, 360);
            assert_eq!(info.ptMinTrackSize.y, 300);
        }
    }

    #[test]
    fn losing_activation_keeps_settings_open() {
        let popup = HiddenPopup::new(1.0);
        unsafe {
            proc(popup.hwnd, WM_ACTIVATE, WA_INACTIVE as usize, 0);
            let mut message = MSG::default();
            assert_eq!(
                PeekMessageW(&mut message, popup.hwnd, WM_CLOSE, WM_CLOSE, PM_REMOVE),
                0
            );
            assert_ne!(IsWindow(popup.hwnd), 0);
        }
    }

    #[test]
    fn resize_reflow_preserves_user_outer_geometry_and_clamps_scroll() {
        let popup = HiddenPopup::new(1.0);
        popup.popup.state.borrow_mut().scroll = CONTENT;
        unsafe {
            SetWindowPos(
                popup.hwnd,
                null_mut(),
                0,
                0,
                521,
                420,
                SWP_NOMOVE | SWP_NOZORDER | SWP_NOACTIVATE,
            );
            let mut before = RECT::default();
            GetWindowRect(popup.hwnd, &mut before);
            proc(popup.hwnd, WM_SIZE, SIZE_RESTORED as usize, 0);
            let mut after = RECT::default();
            GetWindowRect(popup.hwnd, &mut after);
            assert_eq!(
                (before.left, before.top, before.right, before.bottom),
                (after.left, after.top, after.right, after.bottom)
            );
        }
        let state = popup.popup.state.borrow();
        assert_eq!((state.width, state.height), (521.0, 420.0));
        assert_eq!(
            state.scroll,
            CONTENT.min(SettingsLayout::new(state.width).max_scroll(420.0))
        );
    }

    #[test]
    fn background_covers_actual_resized_client() {
        let popup = HiddenPopup::new(1.0);
        unsafe {
            SetWindowPos(
                popup.hwnd,
                null_mut(),
                0,
                0,
                520,
                420,
                SWP_NOMOVE | SWP_NOZORDER | SWP_NOACTIVATE,
            );
        }
        popup.paint();
        let pixels = popup.pixels();
        assert_eq!(pixels.len(), 520 * 420 * 4);
        assert_eq!(&pixels[(419 * 520 + 519) * 4..], &[8, 8, 8, 255]);
    }

    #[test]
    fn background_cache_tracks_scroll_about_size_and_dpi() {
        let popup = HiddenPopup::new(1.0);
        popup.paint();
        let original = popup.pixels();
        let buffer = popup
            .popup
            .state
            .borrow()
            .background
            .as_ref()
            .unwrap()
            .pixels
            .as_ptr();
        popup.paint();
        assert_eq!(
            popup
                .popup
                .state
                .borrow()
                .background
                .as_ref()
                .unwrap()
                .pixels
                .as_ptr(),
            buffer,
            "unchanged background should reuse its software bitmap"
        );
        assert_eq!(popup.pixels(), original);
        popup.popup.state.borrow_mut().scroll = 72.0;
        popup.paint();
        let scrolled = popup.pixels();
        assert_ne!(scrolled, original);
        popup.popup.state.borrow_mut().about = true;
        popup.paint();
        assert_ne!(popup.pixels(), scrolled);
        {
            let mut state = popup.popup.state.borrow_mut();
            state.height = 400.0;
            state.scale = 2.0;
            state.painter = Painter::new(2.0).unwrap();
        }
        unsafe {
            SetWindowPos(
                popup.hwnd,
                null_mut(),
                0,
                0,
                720,
                800,
                SWP_NOMOVE | SWP_NOZORDER | SWP_NOACTIVATE,
            );
        }
        popup.paint();
        assert_eq!(popup.pixels().len(), 720 * 800 * 4);
    }

    #[test]
    fn changing_motion_updates_only_the_two_changed_checkboxes() {
        let mut state = state_for_test(Settings::default(), 1.0);
        let settings = Settings {
            motion: MotionPreset::Fast,
            ..state.settings
        };
        let updates = update_controls(&mut state, settings, false, null_mut());
        assert_eq!(updates.len(), 2);
        assert!(updates.iter().all(|update| update.name.is_none()));
        assert_eq!(
            updates.iter().map(|u| u.checked).collect::<Vec<_>>(),
            [Some(false), Some(true)]
        );
        assert!(update_controls(&mut state, settings, false, null_mut()).is_empty());
    }

    #[test]
    fn timing_offset_refreshes_commands_without_repainting_unchanged_buttons() {
        let mut state = state_for_test(Settings::default(), 1.0);
        let settings = Settings {
            lyric_offset_ms: 100,
            ..state.settings
        };
        let updates = update_controls(&mut state, settings, false, null_mut());
        assert_eq!(updates.len(), 1);
        assert!(updates[0].name.as_ref().unwrap().contains("+0.1s Reset"));
        assert!(
            state
                .controls
                .iter()
                .any(|c| c.label == "+" && c.command == TrayCommand::LyricOffset(200))
        );
        assert!(
            state
                .controls
                .iter()
                .any(|c| c.label == "−" && c.command == TrayCommand::LyricOffset(0))
        );
    }

    #[test]
    fn changing_accent_repaints_selected_and_focused_controls() {
        let mut state = state_for_test(Settings::default(), 1.0);
        let focus = state.controls.last().unwrap().hwnd;
        let settings = Settings {
            accent: AccentPreset::Mint,
            ..state.settings
        };
        let updates = update_controls(&mut state, settings, false, focus);
        assert!(updates.iter().any(|u| u.hwnd == focus));
        for control in &state.controls {
            if control.selected {
                assert!(updates.iter().any(|u| u.hwnd == control.hwnd));
            }
        }
        assert!(
            !updates.iter().any(|u| u.hwnd == state.controls[1].hwnd),
            "unchosen Amber should not repaint"
        );
    }

    #[test]
    #[ignore = "Measures software painting and GDI memory copies; run with --release --nocapture"]
    fn settings_paint_performance() {
        use std::time::Instant;
        unsafe {
            for scale in [1.0, 2.0] {
                let popup = HiddenPopup::new(scale);
                let hwnd = popup.hwnd;
                let dc = CreateCompatibleDC(null_mut());
                let window_dc = GetDC(hwnd);
                let bitmap = CreateCompatibleBitmap(
                    window_dc,
                    (WIDTH * scale) as i32,
                    (760.0 * scale) as i32,
                );
                ReleaseDC(hwnd, window_dc);
                let old = SelectObject(dc, bitmap);
                let paint = || {
                    popup.paint();
                    let state = popup.popup.state.borrow();
                    let background = state.background.as_ref().unwrap();
                    // The hidden window DC is clipped out. Copy to a real memory
                    // bitmap too, so cached timings include transferring pixels.
                    blit(dc, &background.pixels, background.key.0);
                    GdiFlush();
                };
                paint();
                let started = Instant::now();
                for _ in 0..20 {
                    paint();
                }
                let background_ms = started.elapsed().as_secs_f64() * 1000.0 / 20.0;
                let started = Instant::now();
                for i in 0..10 {
                    popup.popup.state.borrow_mut().scroll = i as f32;
                    paint();
                }
                let changed_background_ms = started.elapsed().as_secs_f64() * 1000.0 / 10.0;
                let item = DRAWITEMSTRUCT {
                    CtlType: ODT_BUTTON,
                    CtlID: 100,
                    hDC: dc,
                    rcItem: RECT {
                        left: 0,
                        top: 0,
                        right: (59.0 * scale) as i32,
                        bottom: (29.0 * scale) as i32,
                    },
                    ..Default::default()
                };
                let draw = || {
                    proc(
                        hwnd,
                        WM_DRAWITEM,
                        100,
                        (&item as *const DRAWITEMSTRUCT) as isize,
                    );
                    GdiFlush();
                };
                draw();
                let started = Instant::now();
                for _ in 0..100 {
                    draw();
                }
                let button_ms = started.elapsed().as_secs_f64() * 1000.0 / 100.0;
                println!(
                    "settings scale={scale}: cached background={background_ms:.3} ms, changed background={changed_background_ms:.3} ms, button={button_ms:.3} ms"
                );
                SelectObject(dc, old);
                DeleteObject(bitmap);
                DeleteDC(dc);
            }
        }
    }

    #[test]
    fn native_state_changes_do_not_use_the_default_button_painter() {
        unsafe extern "system" fn parent_proc(
            hwnd: HWND,
            message: u32,
            wparam: WPARAM,
            lparam: LPARAM,
        ) -> LRESULT {
            unsafe {
                if message == WM_CTLCOLORBTN {
                    let count = GetWindowLongPtrW(hwnd, GWLP_USERDATA)
                        as *const Cell<(usize, usize, usize)>;
                    let (native, custom, clicks) = (*count).get();
                    (*count).set((native + 1, custom, clicks));
                    return GetStockObject(BLACK_BRUSH) as isize;
                }
                if message == WM_DRAWITEM {
                    let count = GetWindowLongPtrW(hwnd, GWLP_USERDATA)
                        as *const Cell<(usize, usize, usize)>;
                    let (native, custom, clicks) = (*count).get();
                    (*count).set((native, custom + 1, clicks));
                    return 1;
                }
                if message == WM_COMMAND && wparam >> 16 == BN_CLICKED as usize {
                    let count = GetWindowLongPtrW(hwnd, GWLP_USERDATA)
                        as *const Cell<(usize, usize, usize)>;
                    let (native, custom, clicks) = (*count).get();
                    (*count).set((native, custom, clicks + 1));
                    return 0;
                }
                DefWindowProcW(hwnd, message, wparam, lparam)
            }
        }
        unsafe {
            let class = wide("HiyoDynaPreferencesPaintTest");
            let instance = GetModuleHandleW(null());
            RegisterClassW(&WNDCLASSW {
                lpfnWndProc: Some(parent_proc),
                hInstance: instance,
                lpszClassName: class.as_ptr(),
                ..Default::default()
            });
            // Never show the test windows or take focus from the desktop owner.
            let parent = CreateWindowExW(
                0,
                class.as_ptr(),
                null(),
                WS_POPUP,
                0,
                0,
                100,
                100,
                null_mut(),
                null_mut(),
                instance,
                null(),
            );
            assert!(!parent.is_null());
            let count = Cell::new((0_usize, 0_usize, 0_usize));
            SetWindowLongPtrW(
                parent,
                GWLP_USERDATA,
                (&count as *const Cell<(usize, usize, usize)>) as isize,
            );
            let button = CreateWindowExW(
                0,
                wide("BUTTON").as_ptr(),
                wide("Accent: Ice").as_ptr(),
                WS_CHILD | WS_VISIBLE | BS_CHECKBOX as u32 | BS_PUSHLIKE as u32,
                0,
                0,
                59,
                29,
                parent,
                100 as *mut _,
                instance,
                null(),
            );
            assert!(!button.is_null());
            SetWindowSubclass(button, Some(button_proc), 1, 0);
            SendMessageW(button, BM_SETCHECK, 1, 0);
            assert_eq!(SendMessageW(button, BM_GETCHECK, 0, 0), 1);
            SendMessageW(button, BM_SETSTATE, 1, 0);
            assert_ne!(SendMessageW(button, BM_GETSTATE, 0, 0) & 4, 0);
            SendMessageW(button, BM_SETSTATE, 0, 0);
            SetWindowTextW(button, wide("Accent: Mint").as_ptr());
            SendMessageW(button, BM_CLICK, 0, 0);
            assert_eq!(SendMessageW(button, BM_GETCHECK, 0, 0), 1);
            assert_ne!(GetWindowLongW(button, GWL_STYLE) as u32 & WS_VISIBLE, 0);
            assert_ne!(
                SendMessageW(button, WM_GETDLGCODE, 0, 0) & DLGC_BUTTON as isize,
                0
            );
            ShowWindow(button, SW_HIDE);
            SendMessageW(button, BM_SETCHECK, 0, 0);
            SetWindowTextW(button, wide("Accent: Ice").as_ptr());
            assert_eq!(GetWindowLongW(button, GWL_STYLE) as u32 & WS_VISIBLE, 0);
            let dc = CreateCompatibleDC(null_mut());
            SendMessageW(button, WM_PRINTCLIENT, dc as usize, PRF_CLIENT as isize);
            DeleteDC(dc);
            let paints = count.get();
            DestroyWindow(parent);
            UnregisterClassW(class.as_ptr(), instance);
            assert_eq!(
                paints,
                (0, 1, 1),
                "custom drawing must preserve native click notifications"
            );
        }
    }

    #[test]
    #[ignore = "Creates a native settings window and writes screenshots; requires an interactive desktop"]
    fn native_popup_smoke() {
        let driver = std::thread::spawn(|| unsafe {
            let mut hwnd = null_mut();
            for _ in 0..100 {
                let candidate = FindWindowW(wide("HiyoDynaPreferences").as_ptr(), null());
                let mut pid = 0;
                GetWindowThreadProcessId(candidate, &mut pid);
                if pid == std::process::id() {
                    hwnd = candidate;
                    break;
                }
                std::thread::park_timeout(std::time::Duration::from_millis(50));
            }
            assert!(!hwnd.is_null(), "popup window was not created");
            struct CloseOnPanic(HWND);
            impl Drop for CloseOnPanic {
                fn drop(&mut self) {
                    if std::thread::panicking() {
                        unsafe {
                            PostMessageW(self.0, WM_CLOSE, 0, 0);
                        }
                    }
                }
            }
            let _close_on_panic = CloseOnPanic(hwnd);
            // Desktop captures must show this test window above any running island.
            assert_ne!(
                SetWindowPos(
                    hwnd,
                    HWND_TOPMOST,
                    0,
                    0,
                    0,
                    0,
                    SWP_NOMOVE | SWP_NOSIZE | SWP_NOACTIVATE,
                ),
                0
            );
            std::thread::park_timeout(std::time::Duration::from_millis(500));
            let dir = std::env::var_os("HIYODYNA_PREVIEW_DIR")
                .map(std::path::PathBuf::from)
                .expect("preview directory");
            std::fs::create_dir_all(&dir).unwrap();
            save_window(hwnd, &dir.join("settings-top.png"));
            let about = controls(&Settings::default())
                .iter()
                .position(|c| c.command == TrayCommand::About)
                .unwrap();
            PostMessageW(hwnd, COMMAND, 100 + about, 0);
            std::thread::park_timeout(std::time::Duration::from_millis(150));
            save_window(hwnd, &dir.join("settings-about.png"));
            PostMessageW(hwnd, COMMAND, 1, 0);
            std::thread::park_timeout(std::time::Duration::from_millis(150));
            let style = GetWindowLongW(hwnd, GWL_STYLE) as u32;
            assert_eq!(style & (WS_CAPTION | WS_MINIMIZEBOX | WS_MAXIMIZEBOX), 0);
            assert_ne!(style & WS_POPUP, 0);
            assert_ne!(style & WS_THICKFRAME, 0);
            let ex_style = GetWindowLongW(hwnd, GWL_EXSTYLE) as u32;
            assert_eq!(ex_style & WS_EX_APPWINDOW, 0);
            assert_ne!(ex_style & WS_EX_TOOLWINDOW, 0);
            SendMessageW(hwnd, WM_ACTIVATE, WA_INACTIVE as usize, 0);
            std::thread::park_timeout(std::time::Duration::from_millis(100));
            assert_ne!(IsWindow(hwnd), 0, "settings must survive deactivation");
            let scale = windows_sys::Win32::UI::HiDpi::GetDpiForWindow(hwnd).max(96) as f32 / 96.0;
            let (width, height) = client_outer_size(
                (520.0 * scale) as i32,
                (420.0 * scale) as i32,
                (scale * 96.0) as u32,
            );
            SetWindowPos(
                hwnd,
                null_mut(),
                0,
                0,
                width,
                height,
                SWP_NOMOVE | SWP_NOZORDER | SWP_NOACTIVATE,
            );
            std::thread::park_timeout(std::time::Duration::from_millis(200));
            save_window(hwnd, &dir.join("settings-wide.png"));
            let (width, height) = client_outer_size(
                (360.0 * scale) as i32,
                (300.0 * scale) as i32,
                (scale * 96.0) as u32,
            );
            SetWindowPos(
                hwnd,
                null_mut(),
                0,
                0,
                width,
                height,
                SWP_NOMOVE | SWP_NOZORDER | SWP_NOACTIVATE,
            );
            std::thread::park_timeout(std::time::Duration::from_millis(200));
            save_window(hwnd, &dir.join("settings-minimum.png"));
            assert_eq!(SendMessageW(GetDlgItem(hwnd, 100), BM_GETCHECK, 0, 0), 1);
            let mut name = [0_u16; 100];
            GetWindowTextW(GetDlgItem(hwnd, 100), name.as_mut_ptr(), 100);
            assert!(String::from_utf16_lossy(&name).starts_with("Accent: Ice"));
            let last = GetDlgItem(hwnd, 100 + controls(&Settings::default()).len() as i32 - 1);
            // Frame buttons and disabled actions change the number of Tab stops.
            // Observe traversal reaching the last action instead of assuming a
            // fixed number of key presses leaves focus there without wrapping.
            for _ in 0..controls(&Settings::default()).len() + 6 {
                KEYBOARD_DONE.store(false, std::sync::atomic::Ordering::Release);
                PostMessageW(hwnd, WM_KEYDOWN, 9, 0);
                PostMessageW(hwnd, WM_APP + 99, 0, 0);
                for _ in 0..200 {
                    if KEYBOARD_DONE.load(std::sync::atomic::Ordering::Acquire) {
                        break;
                    }
                    std::thread::park_timeout(std::time::Duration::from_millis(10));
                }
                assert!(KEYBOARD_DONE.load(std::sync::atomic::Ordering::Acquire));
                if IsWindowVisible(last) != 0 {
                    break;
                }
            }
            std::thread::park_timeout(std::time::Duration::from_millis(200));
            save_window(hwnd, &dir.join("settings-keyboard-bottom.png"));
            assert_ne!(IsWindowVisible(last), 0, "Tab must reveal the final action");
            PostMessageW(hwnd, WM_KEYDOWN, 27, 0);
        });
        let settings = Settings::default();
        show(null_mut(), &settings, true).unwrap();
        let first = WINDOW.with(Cell::get);
        show(null_mut(), &settings, true).unwrap();
        assert_eq!(
            WINDOW.with(Cell::get),
            first,
            "opening settings twice must activate the existing window"
        );
        unsafe {
            let mut message = MSG::default();
            while !WINDOW.with(Cell::get).is_null()
                && GetMessageW(&mut message, null_mut(), 0, 0) > 0
            {
                let tab = message.message == WM_KEYDOWN && message.wParam == 9;
                let _caller_shift = tab.then(|| NativeTestKeyboard::new(true));
                {
                    let _plain_tab = tab.then(|| NativeTestKeyboard::new(false));
                    if !translate_message(&message) {
                        TranslateMessage(&message);
                        DispatchMessageW(&message);
                    }
                }
                if tab {
                    assert!(
                        GetKeyState(VK_SHIFT as i32) < 0,
                        "synthetic plain Tab must restore the caller's held Shift state"
                    );
                }
            }
        }
        driver.join().unwrap();
    }

    #[test]
    #[ignore = "Captures both utility languages and checks real custom-frame controls serially"]
    fn native_settings_polish_preview() {
        unsafe {
            let old_language = crate::i18n::language();
            struct Cleanup(crate::i18n::Language);
            impl Drop for Cleanup {
                fn drop(&mut self) {
                    unsafe {
                        let hwnd = WINDOW.with(Cell::get);
                        if !hwnd.is_null() {
                            DestroyWindow(hwnd);
                        }
                    }
                    crate::i18n::set_language(self.0);
                }
            }
            let _cleanup = Cleanup(old_language);
            let dir = std::path::PathBuf::from(
                std::env::var_os("HIYODYNA_PREVIEW_DIR").expect("preview directory"),
            );
            crate::i18n::set_language(Language::English);
            let mut settings = Settings {
                language: Language::English,
                ..Settings::default()
            };
            show(null_mut(), &settings, true).unwrap();
            let hwnd = WINDOW.with(Cell::get);
            let ptr = GetWindowLongPtrW(hwnd, GWLP_USERDATA) as *const Popup;
            let state = &(*ptr).state;
            let scale = state.borrow().scale;
            SetWindowPos(
                hwnd,
                HWND_TOPMOST,
                40,
                40,
                (WIDTH * scale) as i32,
                (600.0 * scale) as i32,
                SWP_NOACTIVATE,
            );
            let hit = |x: f32, y: f32| {
                let mut point = POINT {
                    x: (x * scale) as i32,
                    y: (y * scale) as i32,
                };
                ClientToScreen(hwnd, &mut point);
                SendMessageW(
                    hwnd,
                    WM_NCHITTEST,
                    0,
                    ((point.y as u16 as u32) << 16 | point.x as u16 as u32) as isize,
                )
            };
            assert_eq!(hit(24.0, 24.0), HTCAPTION as isize);
            assert_eq!(hit(2.0, 2.0), HTTOPLEFT as isize);
            assert_eq!(hit(WIDTH - 28.0, 28.0), HTCLIENT as isize);
            SendMessageW(hwnd, WM_SYSCOMMAND, SC_MAXIMIZE as usize, 0);
            for _ in 0..160 {
                SendMessageW(hwnd, WM_TIMER, 0x4953, 0);
            }
            assert_eq!(IsZoomed(hwnd), 0, "Island expansion is work-area bounded");
            assert!(GetDlgItem(hwnd, crate::window_chrome::MINIMIZE).is_null());
            assert_eq!(
                GetWindowLongW(hwnd, GWL_EXSTYLE) as u32 & WS_EX_APPWINDOW,
                0
            );
            let mut bounds = RECT::default();
            GetClientRect(hwnd, &mut bounds);
            let mut client_origin = POINT::default();
            ClientToScreen(hwnd, &mut client_origin);
            bounds.right += client_origin.x;
            bounds.bottom += client_origin.y;
            bounds.left = client_origin.x;
            bounds.top = client_origin.y;
            let mut monitor = MONITORINFO {
                cbSize: size_of::<MONITORINFO>() as u32,
                ..Default::default()
            };
            GetMonitorInfoW(
                MonitorFromWindow(hwnd, MONITOR_DEFAULTTONEAREST),
                &mut monitor,
            );
            assert!(
                bounds.left >= monitor.rcWork.left
                    && bounds.top >= monitor.rcWork.top
                    && bounds.right <= monitor.rcWork.right
                    && bounds.bottom <= monitor.rcWork.bottom,
                "Maximized client bounds ({}, {}, {}, {}) must fit work area ({}, {}, {}, {})",
                bounds.left,
                bounds.top,
                bounds.right,
                bounds.bottom,
                monitor.rcWork.left,
                monitor.rcWork.top,
                monitor.rcWork.right,
                monitor.rcWork.bottom
            );
            SendMessageW(hwnd, WM_SYSCOMMAND, SC_RESTORE as usize, 0);
            for _ in 0..160 {
                SendMessageW(hwnd, WM_TIMER, 0x4953, 0);
            }
            assert_eq!(IsZoomed(hwnd), 0);
            SendMessageW(hwnd, WM_SYSCOMMAND, SC_MINIMIZE as usize, 0);
            assert_eq!(IsIconic(hwnd), 0, "tool surfaces have no minimize state");
            for language in [Language::English, Language::Vietnamese] {
                settings.language = language;
                let focus = GetDlgItem(hwnd, crate::window_chrome::CLOSE);
                SetFocus(focus);
                let scroll = state.borrow().scroll;
                crate::i18n::set_language(language);
                update(&settings, true);
                assert_eq!(GetFocus(), focus);
                assert_eq!(state.borrow().scroll, scroll);
                SendMessageW(hwnd, WM_COMMAND, crate::window_chrome::MAXIMIZE as usize, 0);
                for _ in 0..160 {
                    SendMessageW(hwnd, WM_TIMER, 0x4953, 0);
                }
                assert_eq!(GetFocus(), focus);
                save_window(
                    hwnd,
                    &dir.join(format!("settings-island-wide-{}.png", language.as_str())),
                );
                SendMessageW(hwnd, WM_COMMAND, crate::window_chrome::MAXIMIZE as usize, 0);
                for _ in 0..160 {
                    SendMessageW(hwnd, WM_TIMER, 0x4953, 0);
                }
                let mut title = [0; 128];
                let len = GetWindowTextW(hwnd, title.as_mut_ptr(), title.len() as i32);
                assert_eq!(
                    String::from_utf16_lossy(&title[..len as usize]),
                    tr("HiyoDyna Settings")
                );
                for (name, offset) in [
                    ("top", 0.0_f32),
                    ("lyrics", 500.0),
                    ("shortcut-updates", 900.0),
                ] {
                    {
                        let mut state = state.borrow_mut();
                        state.scroll =
                            offset.min(SettingsLayout::new(state.width).max_scroll(state.height));
                    }
                    position_controls(hwnd, state);
                    RedrawWindow(
                        hwnd,
                        null(),
                        null_mut(),
                        RDW_INVALIDATE | RDW_ALLCHILDREN | RDW_UPDATENOW,
                    );
                    windows_sys::Win32::Graphics::Dwm::DwmFlush();
                    save_window(
                        hwnd,
                        &dir.join(format!("settings-{}-{name}.png", language.as_str())),
                    );
                }
                let capture = state
                    .borrow()
                    .controls
                    .iter()
                    .position(|c| c.action == Some(LocalAction::CaptureShortcut))
                    .unwrap();
                proc(hwnd, COMMAND, 100 + capture, 0);
                assert!(capturing_shortcut());
                assert!(translate_message(&MSG {
                    hwnd,
                    message: WM_KEYDOWN,
                    wParam: 27,
                    ..Default::default()
                }));
                assert!(!capturing_shortcut());
                let about = state
                    .borrow()
                    .controls
                    .iter()
                    .position(|c| c.command == TrayCommand::About)
                    .unwrap();
                proc(hwnd, COMMAND, 100 + about, 0);
                RedrawWindow(
                    hwnd,
                    null(),
                    null_mut(),
                    RDW_INVALIDATE | RDW_ALLCHILDREN | RDW_UPDATENOW,
                );
                windows_sys::Win32::Graphics::Dwm::DwmFlush();
                save_window(
                    hwnd,
                    &dir.join(format!("settings-{}-about.png", language.as_str())),
                );
                proc(hwnd, COMMAND, 1, 0);
            }
        }
    }

    #[test]
    #[ignore = "Verifies an open notice leaves Settings commands for the application dispatcher"]
    fn native_notice_preserves_live_settings_dispatch() {
        unsafe {
            let old_language = crate::i18n::language();
            crate::i18n::set_language(Language::English);
            let owner = CreateWindowExW(
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
            struct Cleanup(HWND, Language);
            impl Drop for Cleanup {
                fn drop(&mut self) {
                    unsafe {
                        let notice = FindWindowW(wide("HiyoDynaNotice").as_ptr(), null());
                        if !notice.is_null() {
                            DestroyWindow(notice);
                        }
                        let settings = WINDOW.with(Cell::get);
                        if !settings.is_null() {
                            DestroyWindow(settings);
                        }
                        DestroyWindow(self.0);
                    }
                    crate::i18n::set_language(self.1);
                }
            }
            let _cleanup = Cleanup(owner, old_language);
            let mut settings = Settings {
                language: Language::English,
                ..Settings::default()
            };
            show(owner, &settings, true).unwrap();
            let hwnd = WINDOW.with(Cell::get);
            let id = 100
                + controls(&settings)
                    .iter()
                    .position(|control| {
                        control.command == TrayCommand::Language(Language::Vietnamese)
                    })
                    .unwrap();
            let handle = hwnd as usize;
            let (release, wait) = std::sync::mpsc::channel();
            let driver = std::thread::spawn(move || {
                let start = std::time::Instant::now();
                let notice = loop {
                    let notice = FindWindowW(wide("HiyoDynaNotice").as_ptr(), null());
                    if !notice.is_null() && IsWindowVisible(notice) != 0 {
                        break notice;
                    }
                    if start.elapsed() > std::time::Duration::from_secs(5) {
                        return;
                    }
                    std::thread::park_timeout(std::time::Duration::from_millis(10));
                };
                PostMessageW(handle as HWND, COMMAND, id, 0);
                let _ = wait.recv_timeout(std::time::Duration::from_secs(2));
                PostMessageW(notice, WM_CLOSE, 0, 0);
            });
            crate::native_ui::notice(
                owner,
                "HiyoDyna",
                "Shortcut unavailable. Use the tray menu to restore Island interaction.",
            );
            let notice = FindWindowW(wide("HiyoDynaNotice").as_ptr(), null());
            let remained_open = !notice.is_null() && IsWindowVisible(notice) != 0;
            let start = std::time::Instant::now();
            let mut applied = 0;
            while applied == 0 && start.elapsed() < std::time::Duration::from_secs(3) {
                let mut message = MSG::default();
                if PeekMessageW(&mut message, null_mut(), 0, 0, PM_REMOVE) != 0 {
                    if message.hwnd == owner && message.message == MESSAGE {
                        for command in take_commands() {
                            assert_eq!(command, TrayCommand::Language(Language::Vietnamese));
                            settings.language = Language::Vietnamese;
                            crate::i18n::set_language(settings.language);
                            update(&settings, true);
                            applied += 1;
                        }
                    } else if !translate_message(&message) {
                        TranslateMessage(&message);
                        DispatchMessageW(&message);
                    }
                } else {
                    std::thread::park_timeout(std::time::Duration::from_millis(5));
                }
            }
            let _ = release.send(());
            driver.join().unwrap();
            assert!(
                remained_open,
                "A notice must return to the application dispatcher while it stays visible"
            );
            assert_eq!(
                applied, 1,
                "A Settings action must apply exactly once while the notice is open"
            );
            assert!(take_commands().is_empty());
            assert_ne!(IsWindow(notice), 0);
        }
    }

    pub(crate) unsafe fn save_window(hwnd: HWND, path: &std::path::Path) {
        if let Some(parent) = path.parent() {
            std::fs::create_dir_all(parent).unwrap();
        }
        let (pixels, w, h) = unsafe { capture_window(hwnd) };
        image::save_buffer(path, &pixels, w, h, image::ColorType::Rgba8).unwrap();
    }

    unsafe fn capture_window(hwnd: HWND) -> (Vec<u8>, u32, u32) {
        unsafe {
            let mut rect = RECT::default();
            GetClientRect(hwnd, &mut rect);
            let w = rect.right as u32;
            let h = rect.bottom as u32;
            // Read ordinary windows from their drawing surface so an unrelated
            // desktop overlay cannot corrupt an immediate-frame comparison.
            // Layered windows require desktop capture instead.
            let layered = GetWindowLongPtrW(hwnd, GWL_EXSTYLE) as u32 & WS_EX_LAYERED != 0;
            let mut origin = POINT::default();
            ClientToScreen(hwnd, &mut origin);
            let source_window = if layered { null_mut() } else { hwnd };
            let source = GetDC(source_window);
            let dc = CreateCompatibleDC(source);
            let info = BITMAPINFO {
                bmiHeader: BITMAPINFOHEADER {
                    biSize: size_of::<BITMAPINFOHEADER>() as u32,
                    biWidth: w as i32,
                    biHeight: -(h as i32),
                    biPlanes: 1,
                    biBitCount: 32,
                    biCompression: BI_RGB,
                    ..Default::default()
                },
                ..Default::default()
            };
            let mut bits = null_mut();
            let bitmap = CreateDIBSection(dc, &info, DIB_RGB_COLORS, &mut bits, null_mut(), 0);
            assert!(!bitmap.is_null());
            let previous = SelectObject(dc, bitmap);
            assert_ne!(
                BitBlt(
                    dc,
                    0,
                    0,
                    w as i32,
                    h as i32,
                    source,
                    if layered { origin.x } else { 0 },
                    if layered { origin.y } else { 0 },
                    SRCCOPY
                ),
                0
            );
            GdiFlush();
            let mut pixels =
                std::slice::from_raw_parts(bits.cast::<u8>(), w as usize * h as usize * 4).to_vec();
            for p in pixels.as_chunks_mut::<4>().0 {
                p.swap(0, 2);
                p[3] = 255;
            }
            // The rounded region exposes desktop pixels in its corners. They
            // are not part of the application and may change between captures.
            let region = CreateRectRgn(0, 0, 0, 0);
            if GetWindowRgn(hwnd, region) > 0 {
                let mut window = RECT::default();
                GetWindowRect(hwnd, &mut window);
                for (index, pixel) in pixels.as_chunks_mut::<4>().0.iter_mut().enumerate() {
                    let x = index as i32 % w as i32 + origin.x - window.left;
                    let y = index as i32 / w as i32 + origin.y - window.top;
                    if PtInRegion(region, x, y) == 0 {
                        *pixel = [0; 4];
                    }
                }
            }
            DeleteObject(region);
            SelectObject(dc, previous);
            DeleteObject(bitmap);
            DeleteDC(dc);
            ReleaseDC(source_window, source);
            (pixels, w, h)
        }
    }

    #[test]
    #[ignore = "Checks immediate native wheel frames against clean redraws on the interactive desktop"]
    fn native_scroll_frames_are_complete() {
        unsafe {
            show(null_mut(), &Settings::default(), true).unwrap();
            let hwnd = WINDOW.with(Cell::get);
            struct Cleanup(HWND);
            impl Drop for Cleanup {
                fn drop(&mut self) {
                    unsafe {
                        DestroyWindow(self.0);
                    }
                }
            }
            let _cleanup = Cleanup(hwnd);
            let (w, h) = client_outer_size(360, 420, 96);
            SetWindowPos(hwnd, HWND_TOPMOST, 40, 40, w, h, SWP_NOACTIVATE);
            SetFocus(hwnd);
            RedrawWindow(
                hwnd,
                null(),
                null_mut(),
                RDW_INVALIDATE | RDW_ALLCHILDREN | RDW_UPDATENOW,
            );
            let mut frame_times = Vec::new();
            for (index, delta) in [-120_i16, -120, -240, 120, -120, 240, 120, -360]
                .into_iter()
                .enumerate()
            {
                let msg = MSG {
                    hwnd,
                    message: WM_MOUSEWHEEL,
                    wParam: (delta as u16 as usize) << 16,
                    ..Default::default()
                };
                assert!(translate_message(&msg));
                // Exercise an animation frame without pumping queued WM_PAINT:
                // the timer must present one complete frame before returning.
                std::thread::sleep(std::time::Duration::from_millis(20));
                let ptr = GetWindowLongPtrW(hwnd, GWLP_USERDATA) as *const Popup;
                let draw_started = Instant::now();
                advance_scroll(hwnd, &(*ptr).state, Instant::now());
                frame_times.push(draw_started.elapsed().as_secs_f64() * 1000.0);
                windows_sys::Win32::Graphics::Dwm::DwmFlush();
                let immediate = capture_window(hwnd).0;
                RedrawWindow(
                    hwnd,
                    null(),
                    null_mut(),
                    RDW_INVALIDATE | RDW_ALLCHILDREN | RDW_UPDATENOW,
                );
                windows_sys::Win32::Graphics::Dwm::DwmFlush();
                let clean = capture_window(hwnd).0;
                let changed = immediate
                    .as_chunks::<4>()
                    .0
                    .iter()
                    .zip(clean.as_chunks::<4>().0)
                    .filter(|(a, b)| a != b)
                    .count();
                if changed != 0 {
                    let mut rect = RECT::default();
                    GetClientRect(hwnd, &mut rect);
                    let locations: Vec<_> = immediate
                        .as_chunks::<4>()
                        .0
                        .iter()
                        .zip(clean.as_chunks::<4>().0)
                        .enumerate()
                        .filter_map(|(pixel, (a, b))| {
                            (a != b).then_some((
                                pixel % rect.right as usize,
                                pixel / rect.right as usize,
                                *a,
                                *b,
                            ))
                        })
                        .take(20)
                        .collect();
                    eprintln!("Changed pixels: {locations:?}");
                    if let Some(dir) = std::env::var_os("HIYODYNA_PREVIEW_DIR") {
                        let dir = std::path::PathBuf::from(dir);
                        image::save_buffer(
                            dir.join(format!("scroll-{index}-immediate.png")),
                            &immediate,
                            rect.right as u32,
                            rect.bottom as u32,
                            image::ColorType::Rgba8,
                        )
                        .unwrap();
                        image::save_buffer(
                            dir.join(format!("scroll-{index}-clean.png")),
                            &clean,
                            rect.right as u32,
                            rect.bottom as u32,
                            image::ColorType::Rgba8,
                        )
                        .unwrap();
                    }
                }
                assert_eq!(
                    changed, 0,
                    "wheel frame {index} contains stale or displaced pixels"
                );
                if let Some(dir) = std::env::var_os("HIYODYNA_PREVIEW_DIR") {
                    save_window(
                        hwnd,
                        &std::path::PathBuf::from(dir)
                            .join(format!("settings-smooth-frame-{index}.png")),
                    );
                }
            }
            eprintln!(
                "settings native scroll frame mean={:.3}ms max={:.3}ms (layout and synchronous repaint, excluding DWM presentation)",
                frame_times.iter().sum::<f64>() / frame_times.len() as f64,
                frame_times.iter().copied().fold(0.0, f64::max),
            );
            for key in [34, 35, 33, 36] {
                assert!(translate_message(&MSG {
                    hwnd,
                    message: WM_KEYDOWN,
                    wParam: key,
                    ..Default::default()
                }));
                windows_sys::Win32::Graphics::Dwm::DwmFlush();
                let immediate = capture_window(hwnd).0;
                RedrawWindow(
                    hwnd,
                    null(),
                    null_mut(),
                    RDW_INVALIDATE | RDW_ALLCHILDREN | RDW_UPDATENOW,
                );
                windows_sys::Win32::Graphics::Dwm::DwmFlush();
                assert_eq!(
                    immediate,
                    capture_window(hwnd).0,
                    "keyboard scroll frame {key} must be complete"
                );
            }
        }
    }
    #[test]
    fn popup_stays_inside_negative_origin_monitor() {
        let p = clamp_anchor(
            POINT { x: -10, y: 980 },
            RECT {
                left: -1920,
                top: 0,
                right: 0,
                bottom: 1040,
            },
            360,
            700,
        );
        assert_eq!((p.x, p.y), (-360, 340));
    }

    #[test]
    fn popup_geometry_converts_destination_dpi_and_clamps_work_area() {
        let work = RECT {
            left: -1920,
            top: 0,
            right: 0,
            bottom: 1040,
        };
        let anchor = POINT { x: -10, y: 980 };
        let normal = popup_geometry(anchor, work, 96);
        assert_eq!(
            (normal.scale, normal.width_px, normal.height_px),
            (1.0, 360, 760)
        );
        assert_eq!((normal.origin.x, normal.origin.y), (-360, 280));

        let scaled = popup_geometry(anchor, work, 192);
        assert_eq!(
            (scaled.scale, scaled.width_px, scaled.height_px),
            (2.0, 720, 1016)
        );
        assert_eq!(scaled.height_px as f32 / scaled.scale, 508.0);
        assert_eq!((scaled.origin.x, scaled.origin.y), (-720, 24));

        // A minimum logical height must not overflow a short, high-DPI work area.
        let short = popup_geometry(
            anchor,
            RECT {
                bottom: 300,
                ..work
            },
            192,
        );
        assert_eq!(short.height_px, 276);
        assert_eq!(short.height_px as f32 / short.scale, 138.0);
        assert_eq!(short.origin.y, 24);
    }
    #[test]
    fn controls_preserve_lyrics_and_updater_choices() {
        let settings = Settings {
            lyrics: LyricsMode::Words,
            lyric_offset_ms: 10000,
            ..Settings::default()
        };
        let controls = controls(&settings);
        assert!(
            controls
                .iter()
                .any(|c| c.selected && c.command == TrayCommand::Lyrics(LyricsMode::Words))
        );
        assert!(
            controls
                .iter()
                .any(|c| c.command == TrayCommand::LyricOffset(10000))
        );
        assert!(
            controls
                .iter()
                .any(|c| c.command == TrayCommand::Update(UpdateChannel::Stable))
        );
    }

    #[test]
    fn keyboard_reveals_last_control_on_short_work_area() {
        let settings = Settings::default();
        let mut state = State {
            painter: Painter::new(1.0).unwrap(),
            surface: WindowSurface::new().unwrap(),
            frame_pump: FramePump::new(SCROLL_FRAME),
            settings,
            controls: controls(&settings),
            scale: 1.0,
            height: 400.0,
            width: WIDTH,
            scroll: 0.0,
            scroll_motion: SmoothScroll::default(),
            about: false,
            background: None,
            content: None,
            button_rasters: Vec::new(),
            shortcut: ShortcutCapture::default(),
            shortcut_available: true,
            updates: crate::update::CheckSession::new(settings.update),
        };
        let index = state.controls.len() - 1;
        reveal_control(&mut state, index);
        let rect = state.controls[index].rect;
        assert!(rect.y >= state.scroll);
        assert!(CONTENT_TOP + rect.y - state.scroll + rect.h <= state.height - CONTENT_BOTTOM);
        reveal_control(&mut state, 0);
        assert!(state.controls[0].rect.y >= state.scroll);
    }
}
