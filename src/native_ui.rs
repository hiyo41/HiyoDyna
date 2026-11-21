use crate::canvas::{Canvas, DIM, MUTED, Rect, WHITE};

pub(crate) use crate::theme::{BG as BACKGROUND, SURFACE};

use std::sync::atomic::{AtomicBool, AtomicUsize, Ordering};
use std::time::{Duration, Instant};
use windows_sys::Win32::Foundation::{HWND, RECT, WPARAM};
use windows_sys::Win32::Graphics::Gdi::{HBITMAP, HDC, HGDIOBJ};

/// A persistent, top-down BGRA DIB selected into its own memory DC.
pub(crate) struct WindowSurface {
    dc: HDC,
    bitmap: HBITMAP,
    original: HGDIOBJ,
    pixels: *mut u8,
    size: (u32, u32),
    len: usize,
    #[cfg(test)]
    pub(crate) presents: usize,
}

impl WindowSurface {
    pub(crate) fn new() -> Option<Self> {
        let dc =
            unsafe { windows_sys::Win32::Graphics::Gdi::CreateCompatibleDC(std::ptr::null_mut()) };
        if dc.is_null() {
            return None;
        }
        Some(Self {
            dc,
            bitmap: std::ptr::null_mut(),
            original: std::ptr::null_mut(),
            pixels: std::ptr::null_mut(),
            size: (0, 0),
            len: 0,
            #[cfg(test)]
            presents: 0,
        })
    }

    pub(crate) fn resize(&mut self, width: u32, height: u32) -> bool {
        use windows_sys::Win32::Graphics::Gdi::*;
        if width == 0 || height == 0 || width > i32::MAX as u32 || height > i32::MAX as u32 {
            return false;
        }
        if self.size == (width, height) {
            return true;
        }
        let Some(len) = (width as usize)
            .checked_mul(height as usize)
            .and_then(|n| n.checked_mul(4))
            .filter(|len| *len <= isize::MAX as usize)
        else {
            return false;
        };
        let info = BITMAPINFO {
            bmiHeader: BITMAPINFOHEADER {
                biSize: std::mem::size_of::<BITMAPINFOHEADER>() as u32,
                biWidth: width as i32,
                biHeight: -(height as i32),
                biPlanes: 1,
                biBitCount: 32,
                biCompression: BI_RGB,
                ..Default::default()
            },
            ..Default::default()
        };
        unsafe {
            let mut bits = std::ptr::null_mut();
            let bitmap = CreateDIBSection(
                self.dc,
                &info,
                DIB_RGB_COLORS,
                &mut bits,
                std::ptr::null_mut(),
                0,
            );
            if bitmap.is_null() {
                return false;
            }
            let previous = SelectObject(self.dc, bitmap);
            if previous.is_null() || previous as isize == -1 {
                DeleteObject(bitmap);
                return false;
            }
            if self.original.is_null() {
                self.original = previous;
            }
            if !self.bitmap.is_null() {
                DeleteObject(self.bitmap);
            }
            self.bitmap = bitmap;
            self.pixels = bits.cast();
            self.size = (width, height);
            self.len = len;
            self.pixels_mut().fill(0);
        }
        true
    }

    pub(crate) fn pixels_mut(&mut self) -> &mut [u8] {
        if self.len == 0 {
            return &mut [];
        }
        // Finish any batched GDI reads before the software renderer writes again.
        unsafe {
            windows_sys::Win32::Graphics::Gdi::GdiFlush();
            std::slice::from_raw_parts_mut(self.pixels, self.len)
        }
    }

    pub(crate) unsafe fn present(&mut self, dc: HDC, dirty: RECT) {
        #[cfg(test)]
        {
            self.presents += 1;
        }
        use windows_sys::Win32::Graphics::Gdi::{BitBlt, SRCCOPY};
        let left = dirty.left.clamp(0, self.size.0 as i32);
        let top = dirty.top.clamp(0, self.size.1 as i32);
        let right = dirty.right.clamp(left, self.size.0 as i32);
        let bottom = dirty.bottom.clamp(top, self.size.1 as i32);
        if right > left && bottom > top {
            unsafe {
                BitBlt(
                    dc,
                    left,
                    top,
                    right - left,
                    bottom - top,
                    self.dc,
                    left,
                    top,
                    SRCCOPY,
                );
            }
        }
    }
}

impl Drop for WindowSurface {
    fn drop(&mut self) {
        use windows_sys::Win32::Graphics::Gdi::*;
        unsafe {
            if !self.original.is_null() {
                SelectObject(self.dc, self.original);
            }
            if !self.bitmap.is_null() {
                DeleteObject(self.bitmap);
            }
            DeleteDC(self.dc);
        }
    }
}

struct FrameWakeup {
    hwnd: HWND,
    message: u32,
    generation: usize,
    pending: AtomicBool,
}

enum FrameSource {
    Multimedia(u32),
    Window(usize),
}

pub(crate) struct FramePump {
    wakeup: Box<FrameWakeup>,
    source: Option<FrameSource>,
    last_draw: Option<Instant>,
}

unsafe extern "system" fn frame_callback(_: u32, _: u32, user: usize, _: usize, _: usize) {
    // TIME_KILL_SYNCHRONOUS keeps this allocation alive until callbacks finish.
    let wakeup = unsafe { &*(user as *const FrameWakeup) };
    if !wakeup.pending.swap(true, Ordering::AcqRel)
        && unsafe {
            windows_sys::Win32::UI::WindowsAndMessaging::PostMessageW(
                wakeup.hwnd,
                wakeup.message,
                wakeup.generation,
                0,
            )
        } == 0
    {
        wakeup.pending.store(false, Ordering::Release);
    }
}

impl FramePump {
    pub(crate) fn new(message: u32) -> Self {
        Self {
            wakeup: Box::new(FrameWakeup {
                hwnd: std::ptr::null_mut(),
                message,
                generation: 0,
                pending: AtomicBool::new(false),
            }),
            source: None,
            last_draw: None,
        }
    }

    pub(crate) fn is_active(&self) -> bool {
        self.source.is_some()
    }

    pub(crate) unsafe fn start(&mut self, hwnd: HWND) {
        use windows_sys::Win32::Media::*;
        use windows_sys::Win32::UI::WindowsAndMessaging::SetTimer;
        static NEXT: AtomicUsize = AtomicUsize::new(0x10000);
        if self.is_active() {
            return;
        }
        self.wakeup.hwnd = hwnd;
        self.wakeup.generation = NEXT.fetch_add(1, Ordering::Relaxed).max(1);
        unsafe {
            if timeBeginPeriod(1) == 0 {
                let timer = timeSetEvent(
                    16,
                    1,
                    Some(frame_callback),
                    (&*self.wakeup as *const FrameWakeup) as usize,
                    TIME_PERIODIC | TIME_CALLBACK_FUNCTION | TIME_KILL_SYNCHRONOUS,
                );
                if timer != 0 {
                    self.source = Some(FrameSource::Multimedia(timer));
                    return;
                }
                timeEndPeriod(1);
            }
            let timer = SetTimer(hwnd, self.wakeup.generation, 16, None);
            if timer != 0 {
                self.source = Some(FrameSource::Window(timer));
            } else {
                self.wakeup.generation = 0;
            }
        }
    }

    pub(crate) unsafe fn stop(&mut self) {
        use windows_sys::Win32::Media::{timeEndPeriod, timeKillEvent};
        unsafe {
            match self.source.take() {
                Some(FrameSource::Multimedia(timer)) => {
                    timeKillEvent(timer);
                    timeEndPeriod(1);
                }
                Some(FrameSource::Window(timer)) => {
                    windows_sys::Win32::UI::WindowsAndMessaging::KillTimer(self.wakeup.hwnd, timer);
                }
                None => {}
            }
        }
        self.wakeup.pending.store(false, Ordering::Release);
        self.wakeup.generation = 0;
    }

    pub(crate) fn is_frame(&self, message: u32, wparam: WPARAM) -> bool {
        (message == self.wakeup.message && wparam == self.wakeup.generation)
            || matches!(self.source, Some(FrameSource::Window(timer))
                if message == windows_sys::Win32::UI::WindowsAndMessaging::WM_TIMER && wparam == timer)
    }

    pub(crate) unsafe fn should_draw(&mut self, now: Instant) -> bool {
        self.wakeup.pending.store(false, Ordering::Release);
        if self
            .last_draw
            .is_some_and(|last| now.saturating_duration_since(last) < Duration::from_millis(8))
        {
            return false;
        }
        self.last_draw = Some(now);
        true
    }
}

impl Drop for FramePump {
    fn drop(&mut self) {
        unsafe {
            self.stop();
        }
    }
}

pub(crate) fn trace_start() -> Option<Instant> {
    static ENABLED: std::sync::OnceLock<bool> = std::sync::OnceLock::new();
    ENABLED
        .get_or_init(|| std::env::var_os("HIYODYNA_TRACE_UI").is_some())
        .then(Instant::now)
}

pub(crate) fn trace_ui(window: &str, phase: &str, started: Option<Instant>) {
    if let Some(started) = started {
        eprintln!(
            "HIYODYNA_TRACE_UI {window} {phase}={:.3}ms",
            started.elapsed().as_secs_f64() * 1000.0
        );
    }
}

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
        SetWindowLongPtrW(hwnd, GWL_EXSTYLE, style & !(WS_EX_COMPOSITED as isize));
        let dark = 1_i32;
        let dark_result = DwmSetWindowAttribute(
            hwnd,
            DWMWA_USE_IMMERSIVE_DARK_MODE as u32,
            (&dark as *const i32).cast(),
            4,
        );
        let corner = 2_u32;
        // DWMWA_WINDOW_CORNER_PREFERENCE / DWMWCP_ROUND on supported Windows.
        let corner_result = DwmSetWindowAttribute(hwnd, 33, (&corner as *const u32).cast(), 4);
        if trace_start().is_some() && (dark_result < 0 || corner_result < 0) {
            eprintln!("HIYODYNA_TRACE_UI DWM dark={dark_result:#x} corner={corner_result:#x}");
        }
    }
}

/// Apply group opacity after composing overlapping rounded fills.
fn composite_rects(c: &mut Canvas<'_>, shapes: &[(Rect, f32, [u8; 3])]) {
    if c.opacity <= 0.0 {
        return;
    }
    if c.opacity >= 1.0 {
        for &(rect, radius, color) in shapes {
            c.rect(rect, radius, color, 1.0);
        }
        return;
    }
    let scale = c.painter.scale;
    let bounds = shapes[0].0.intersect(c.clip);
    let left = (bounds.x * scale).floor().clamp(0.0, c.size.0 as f32) as u32;
    let top = (bounds.y * scale).floor().clamp(0.0, c.size.1 as f32) as u32;
    let right = ((bounds.x + bounds.w) * scale)
        .ceil()
        .clamp(0.0, c.size.0 as f32) as u32;
    let bottom = ((bounds.y + bounds.h) * scale)
        .ceil()
        .clamp(0.0, c.size.1 as f32) as u32;
    if left >= right || top >= bottom {
        return;
    }
    let size = (right - left, bottom - top);
    let origin = (left as f32 / scale, top as f32 / scale);
    let mut pixels = vec![0; size.0 as usize * size.1 as usize * 4];
    {
        let mut layer = Canvas::new(&mut pixels, size, c.painter);
        layer.clip = Rect::new(c.clip.x - origin.0, c.clip.y - origin.1, c.clip.w, c.clip.h);
        for &(rect, radius, color) in shapes {
            layer.rect(
                Rect::new(rect.x - origin.0, rect.y - origin.1, rect.w, rect.h),
                radius,
                color,
                1.0,
            );
        }
    }
    for y in 0..size.1 {
        for x in 0..size.0 {
            let source = ((y * size.0 + x) * 4) as usize;
            let destination = (((top + y) * c.size.0 + left + x) * 4) as usize;
            let inverse = 1.0 - pixels[source + 3] as f32 / 255.0 * c.opacity;
            for channel in 0..4 {
                // The layer already contains premultiplied channels. Scale
                // them once, without multiplying them by alpha a second time.
                let value = pixels[source + channel] as f32 * c.opacity
                    + c.pixels[destination + channel] as f32 * inverse;
                c.pixels[destination + channel] = (f64::from(value) + 0.5) as u8;
            }
        }
    }
}

/// Rounded card with a one-logical-pixel inset border (normally radius 14).
#[allow(dead_code)] // Shared primitives are adopted by the window rewrites.
pub(crate) fn card(c: &mut Canvas<'_>, rect: Rect, radius: f32, fill: [u8; 3], border: [u8; 3]) {
    if rect.w <= 0.0 || rect.h <= 0.0 {
        return;
    }
    let radius = radius.clamp(0.0, rect.w.min(rect.h) * 0.5);
    let shapes = [
        (rect, radius, border),
        (
            Rect::new(rect.x + 1.0, rect.y + 1.0, rect.w - 2.0, rect.h - 2.0),
            (radius - 1.0).max(0.0),
            fill,
        ),
    ];
    let count = if rect.w > 2.0 && rect.h > 2.0 { 2 } else { 1 };
    composite_rects(c, &shapes[..count]);
}

#[allow(dead_code)]
pub(crate) fn section_header(
    c: &mut Canvas<'_>,
    rect: Rect,
    title: &str,
    detail: &str,
    text_scale: f32,
) {
    if rect.w <= 0.0 || rect.h <= 0.0 || text_scale <= 0.0 {
        return;
    }
    let old_clip = c.clip;
    c.clip = old_clip.intersect(rect);
    let title_height = (22.0 * text_scale).min(rect.h);
    c.ellipsis(
        title,
        Rect::new(rect.x, rect.y, rect.w, title_height),
        14.0 * text_scale,
        true,
        crate::theme::TEXT,
    );
    let detail_y = rect.y + title_height + 4.0 * text_scale;
    if detail_y < rect.y + rect.h {
        c.ellipsis(
            detail,
            Rect::new(rect.x, detail_y, rect.w, rect.y + rect.h - detail_y),
            11.0 * text_scale,
            false,
            crate::theme::MUTED,
        );
    }
    c.clip = old_clip;
}

#[allow(dead_code)]
pub(crate) fn chip(c: &mut Canvas<'_>, rect: Rect, label: &str, tint: [u8; 3], text_scale: f32) {
    if rect.w <= 0.0 || rect.h <= 0.0 {
        return;
    }
    c.rect(rect, 999.0_f32.min(rect.w.min(rect.h) * 0.5), tint, 0.1);
    if rect.w <= 16.0 || text_scale <= 0.0 {
        return;
    }
    let available = Rect::new(rect.x + 8.0, rect.y, rect.w - 16.0, rect.h);
    let old_clip = c.clip;
    c.clip = old_clip.intersect(available);
    let size = 11.0 * text_scale;
    c.ellipsis(label, available, size, false, tint);
    c.clip = old_clip;
}

/// A 36x20 logical-pixel track, centered in the supplied control rectangle.
#[allow(dead_code)]
pub(crate) fn switch(c: &mut Canvas<'_>, rect: Rect, on: bool, accent: [u8; 3]) {
    if rect.w <= 0.0 || rect.h <= 0.0 {
        return;
    }
    let old_clip = c.clip;
    c.clip = old_clip.intersect(rect);
    let track = Rect::new(
        rect.x + (rect.w - 36.0) * 0.5,
        rect.y + (rect.h - 20.0) * 0.5,
        36.0,
        20.0,
    );
    composite_rects(
        c,
        &[
            (
                track,
                10.0,
                if on {
                    accent
                } else {
                    crate::theme::BORDER_STRONG
                },
            ),
            (
                Rect::new(
                    track.x + if on { 19.0 } else { 3.0 },
                    track.y + 3.0,
                    14.0,
                    14.0,
                ),
                7.0,
                crate::theme::TEXT,
            ),
        ],
    );
    c.clip = old_clip;
}

/// An inset 1.5-pixel rounded stroke (normally radius 10), preserving its interior.
#[allow(dead_code)]
pub(crate) fn focus_ring(c: &mut Canvas<'_>, rect: Rect, radius: f32, accent: [u8; 3]) {
    if rect.w <= 0.0 || rect.h <= 0.0 || c.opacity <= 0.0 {
        return;
    }
    // Canvas has filled rounded rectangles only. Subtract the inner coverage
    // before blending once so translucent surfaces keep their original pixels.
    let scale = c.painter.scale;
    let radius = radius.clamp(0.0, rect.w.min(rect.h) * 0.5);
    let bounds = rect.intersect(c.clip);
    let left = (bounds.x * scale).floor().max(0.0) as u32;
    let top = (bounds.y * scale).floor().max(0.0) as u32;
    let right = ((bounds.x + bounds.w) * scale)
        .ceil()
        .clamp(0.0, c.size.0 as f32) as u32;
    let bottom = ((bounds.y + bounds.h) * scale)
        .ceil()
        .clamp(0.0, c.size.1 as f32) as u32;
    let inset = 1.5_f32.min(rect.w.min(rect.h) * 0.5);
    for y in top..bottom {
        for x in left..right {
            let px = (x as f32 + 0.5) / scale - rect.x - rect.w * 0.5;
            let py = (y as f32 + 0.5) / scale - rect.y - rect.h * 0.5;
            let coverage = |inset: f32| {
                let r = (radius - inset).max(0.0);
                let dx = px.abs() - rect.w * 0.5 + inset + r;
                let dy = py.abs() - rect.h * 0.5 + inset + r;
                let distance = dx.max(0.0).hypot(dy.max(0.0)) + dx.max(dy).min(0.0) - r;
                (0.5 - distance * scale).clamp(0.0, 1.0)
            };
            let inner = if rect.w > inset * 2.0 && rect.h > inset * 2.0 {
                coverage(inset)
            } else {
                0.0
            };
            let alpha = (coverage(0.0) - inner).max(0.0) * c.opacity;
            if alpha > 0.0 {
                let index = ((y * c.size.0 + x) * 4) as usize;
                crate::render::blend_pixel(&mut c.pixels[index..index + 4], accent, alpha);
            }
        }
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

    fn raster(scale: f32, draw: impl FnOnce(&mut Canvas<'_>)) -> (Vec<u8>, (u32, u32)) {
        let size = ((160.0 * scale) as u32, (100.0 * scale) as u32);
        let mut pixels = vec![0; size.0 as usize * size.1 as usize * 4];
        let mut painter = crate::canvas::Painter::new(scale).unwrap();
        draw(&mut Canvas::new(&mut pixels, size, &mut painter));
        (pixels, size)
    }

    fn pixel_at(pixels: &[u8], size: (u32, u32), scale: f32, x: f32, y: f32) -> &[u8] {
        let index = (((y * scale) as u32 * size.0 + (x * scale) as u32) * 4) as usize;
        &pixels[index..index + 4]
    }

    fn assert_ink_inside(pixels: &[u8], size: (u32, u32), scale: f32, rect: Rect) {
        assert!(pixels.as_chunks::<4>().0.iter().any(|p| p[3] != 0));
        for (index, pixel) in pixels.as_chunks::<4>().0.iter().enumerate() {
            if pixel[3] != 0 {
                let x = (index as u32 % size.0) as f32 / scale;
                let y = (index as u32 / size.0) as f32 / scale;
                assert!(
                    rect.contains(x, y),
                    "ink outside bounds at ({x}, {y}), scale {scale}"
                );
                assert!(pixel[..3].iter().all(|channel| *channel <= pixel[3]));
            }
        }
    }

    #[test]
    fn card_applies_opacity_once_over_transparent_and_translucent_pixels() {
        for scale in [1.0, 1.25, 1.5, 2.0] {
            for (background, expected) in [
                ([0, 0, 0, 0], [11, 12, 14, 128]),
                ([20, 40, 60, 100], [21, 32, 44, 178]),
            ] {
                let (pixels, size) = raster(scale, |c| {
                    for pixel in c.pixels.as_chunks_mut::<4>().0 {
                        pixel.copy_from_slice(&background);
                    }
                    c.opacity = 0.5;
                    card(
                        c,
                        Rect::new(12.0, 12.0, 100.0, 60.0),
                        14.0,
                        crate::theme::SURFACE,
                        crate::theme::BORDER,
                    );
                });
                assert_eq!(
                    pixel_at(&pixels, size, scale, 60.0, 40.0),
                    expected,
                    "card interior at {scale}x, background {background:?}"
                );
                assert_eq!(
                    pixel_at(&pixels, size, scale, 60.0, 12.0),
                    [
                        21 + background[0] / 2,
                        23 + background[1] / 2,
                        27 + background[2] / 2,
                        expected[3]
                    ]
                );
                assert_eq!(pixel_at(&pixels, size, scale, 0.0, 0.0), background);
            }
        }
    }

    #[test]
    fn switch_applies_opacity_once_to_knob_and_track() {
        for scale in [1.0, 1.25, 1.5, 2.0] {
            for on in [false, true] {
                for (background, expected_knob) in [
                    ([0, 0, 0, 0], [124, 124, 124, 128]),
                    ([20, 40, 60, 100], [134, 144, 154, 178]),
                ] {
                    let (pixels, size) = raster(scale, |c| {
                        for pixel in c.pixels.as_chunks_mut::<4>().0 {
                            pixel.copy_from_slice(&background);
                        }
                        c.opacity = 0.5;
                        switch(c, Rect::new(12.0, 12.0, 36.0, 20.0), on, crate::theme::ICE);
                    });
                    let knob_x = if on { 38.0 } else { 22.0 };
                    assert_eq!(
                        pixel_at(&pixels, size, scale, knob_x, 22.0),
                        expected_knob,
                        "knob at {scale}x, on {on}, background {background:?}"
                    );
                    let track = if on { [123, 100, 85] } else { [30, 33, 38] };
                    assert_eq!(
                        pixel_at(&pixels, size, scale, 30.0, 14.0),
                        [
                            track[0] + background[0] / 2,
                            track[1] + background[1] / 2,
                            track[2] + background[2] / 2,
                            expected_knob[3]
                        ]
                    );
                    assert_eq!(pixel_at(&pixels, size, scale, 0.0, 0.0), background);
                }
            }
        }
    }

    #[test]
    fn card_keeps_fill_and_border_inside_rounded_bounds() {
        for scale in [1.0, 1.25, 1.5, 2.0] {
            let rect = Rect::new(12.0, 12.0, 100.0, 60.0);
            let (pixels, size) = raster(scale, |c| {
                card(c, rect, 14.0, crate::theme::SURFACE, crate::theme::BORDER);
            });
            assert_ink_inside(&pixels, size, scale, rect);
            assert_eq!(
                pixel_at(&pixels, size, scale, 60.0, 40.0),
                &[22, 24, 28, 255]
            );
            assert_eq!(
                pixel_at(&pixels, size, scale, 60.0, 12.0),
                &[42, 46, 54, 255]
            );
            assert_eq!(pixel_at(&pixels, size, scale, 12.0, 12.0), &[0; 4]);
        }
    }

    #[test]
    fn chip_label_respects_eight_pixel_padding_and_uses_an_ellipsis() {
        for scale in [1.0, 1.25, 1.5, 2.0] {
            let rect = Rect::new(12.0, 12.0, 72.0, 28.0);
            let (label, size) = raster(scale, |c| {
                chip(c, rect, "WWWWWWWWWW", crate::theme::ICE, 1.0)
            });
            let (blank, _) = raster(scale, |c| chip(c, rect, "", crate::theme::ICE, 1.0));
            let mut ink = false;
            for (index, (a, b)) in label
                .as_chunks::<4>()
                .0
                .iter()
                .zip(blank.as_chunks::<4>().0)
                .enumerate()
            {
                if a != b {
                    ink = true;
                    let x = (index as u32 % size.0) as f32 / scale;
                    assert!((20.0..76.0).contains(&x), "chip padding lost at {scale}x");
                }
            }
            assert!(ink);
            assert_ink_inside(&label, size, scale, rect);
            let (narrow, _) = raster(scale, |c| {
                chip(
                    c,
                    Rect::new(12.0, 12.0, 28.0, 28.0),
                    "WWWWWW",
                    crate::theme::ICE,
                    1.0,
                )
            });
            let (ellipsis, _) = raster(scale, |c| {
                chip(
                    c,
                    Rect::new(12.0, 12.0, 28.0, 28.0),
                    "…",
                    crate::theme::ICE,
                    1.0,
                )
            });
            assert_eq!(
                narrow, ellipsis,
                "narrow chip must show an ellipsis at {scale}x"
            );
            let (narrow_blank, _) = raster(scale, |c| {
                chip(
                    c,
                    Rect::new(12.0, 12.0, 28.0, 28.0),
                    "",
                    crate::theme::ICE,
                    1.0,
                )
            });
            assert!(narrow != narrow_blank, "ellipsis must remain visible");
            assert_eq!(pixel_at(&blank, size, scale, 40.0, 24.0), &[25, 20, 17, 26]);
        }
    }

    #[test]
    fn switch_changes_track_color_and_moves_its_fourteen_pixel_knob() {
        for scale in [1.0, 1.25, 1.5, 2.0] {
            let rect = Rect::new(12.0, 12.0, 36.0, 20.0);
            let (on, size) = raster(scale, |c| switch(c, rect, true, crate::theme::ICE));
            let (off, _) = raster(scale, |c| switch(c, rect, false, crate::theme::ICE));
            assert_ink_inside(&on, size, scale, rect);
            assert_ink_inside(&off, size, scale, rect);
            assert_eq!(
                pixel_at(&on, size, scale, 30.0, 14.0),
                &[245, 200, 169, 255]
            );
            assert_eq!(pixel_at(&off, size, scale, 30.0, 14.0), &[60, 66, 76, 255]);
            assert_eq!(
                pixel_at(&off, size, scale, 22.0, 22.0),
                &[248, 248, 248, 255]
            );
            assert_eq!(
                pixel_at(&on, size, scale, 38.0, 22.0),
                &[248, 248, 248, 255]
            );
            assert_ne!(
                pixel_at(&on, size, scale, 22.0, 22.0),
                pixel_at(&off, size, scale, 22.0, 22.0)
            );
        }
    }

    #[test]
    fn focus_ring_preserves_interior_and_draws_all_four_edges() {
        for scale in [1.0, 1.25, 1.5, 2.0] {
            let rect = Rect::new(12.0, 12.0, 100.0, 60.0);
            let (pixels, size) = raster(scale, |c| focus_ring(c, rect, 10.0, crate::theme::ICE));
            assert_ink_inside(&pixels, size, scale, rect);
            for (x, y) in [(60.0, 12.0), (60.0, 71.0), (12.0, 40.0), (111.0, 40.0)] {
                assert!(
                    pixel_at(&pixels, size, scale, x, y)[3] > 0,
                    "missing edge at {scale}x"
                );
            }
            assert_eq!(pixel_at(&pixels, size, scale, 60.0, 40.0), &[0; 4]);
            assert_eq!(pixel_at(&pixels, size, scale, 12.0, 12.0), &[0; 4]);
            assert!(
                pixels
                    .as_chunks::<4>()
                    .0
                    .iter()
                    .any(|p| p[3] > 0 && p[3] < 255)
            );
            let (over_fill, _) = raster(scale, |c| {
                c.rect(rect, 0.0, crate::theme::SURFACE, 1.0);
                focus_ring(c, rect, 10.0, crate::theme::ICE);
            });
            assert_eq!(
                pixel_at(&over_fill, size, scale, 60.0, 40.0),
                &[22, 24, 28, 255]
            );
        }
    }

    #[test]
    fn section_header_clips_both_lines_and_truncates_to_ellipsis() {
        for scale in [1.0, 1.25, 1.5, 2.0] {
            let rect = Rect::new(12.0, 12.0, 14.0, 44.0);
            let (long, size) = raster(scale, |c| {
                section_header(c, rect, "WWWWWWWW", "WWWWWWWW", 1.0)
            });
            let (ellipsis, _) = raster(scale, |c| section_header(c, rect, "…", "…", 1.0));
            assert_eq!(long, ellipsis, "both lines must truncate at {scale}x");
            assert_ink_inside(&long, size, scale, rect);
            for (top, bottom) in [(12.0, 34.0), (38.0, 56.0)] {
                assert!(
                    long.as_chunks::<4>()
                        .0
                        .iter()
                        .enumerate()
                        .any(|(index, pixel)| {
                            let y = (index as u32 / size.0) as f32 / scale;
                            pixel[3] != 0 && (top..bottom).contains(&y)
                        }),
                    "each header line must remain visible at {scale}x"
                );
            }
            let shallow = Rect::new(12.0, 12.0, 100.0, 8.0);
            let (pixels, size) = raster(scale, |c| {
                section_header(c, shallow, "Tall glyphs", "Detail", 2.0)
            });
            assert_ink_inside(&pixels, size, scale, shallow);
        }
    }

    #[test]
    fn surface_reuses_storage_and_zeroes_resized_pixels() {
        let mut surface = WindowSurface::new().unwrap();
        assert!(surface.resize(64, 32));
        assert_eq!(surface.pixels_mut().len(), 8192);
        surface.pixels_mut()[0] = 123;
        let address = surface.pixels_mut().as_ptr();
        assert!(surface.resize(64, 32));
        assert_eq!(surface.pixels_mut().as_ptr(), address);
        assert_eq!(surface.pixels_mut()[0], 123);
        assert!(surface.resize(32, 16));
        assert!(surface.pixels_mut().iter().all(|byte| *byte == 0));
        assert!(!surface.resize(u32::MAX, 32));
        assert_eq!(surface.pixels_mut().len(), 2048);
    }

    #[test]
    fn surface_presents_only_the_dirty_rectangle_from_top_down_pixels() {
        let mut source = WindowSurface::new().unwrap();
        let mut destination = WindowSurface::new().unwrap();
        assert!(source.resize(4, 4));
        assert!(destination.resize(4, 4));
        source.pixels_mut().fill(255);
        source.pixels_mut()[20..24].copy_from_slice(&[10, 20, 30, 0]);
        unsafe {
            source.present(
                destination.dc,
                RECT {
                    left: 1,
                    top: 1,
                    right: 3,
                    bottom: 2,
                },
            );
        }
        let pixels = destination.pixels_mut();
        assert_eq!(&pixels[20..24], &[10, 20, 30, 0]);
        assert_eq!(&pixels[24..28], &[255; 4]);
        assert!(pixels[..20].iter().all(|byte| *byte == 0));
        assert!(pixels[28..].iter().all(|byte| *byte == 0));
    }

    #[test]
    fn frame_pump_accepts_only_its_message_and_limits_draw_frequency() {
        use std::time::{Duration, Instant};
        use windows_sys::Win32::UI::WindowsAndMessaging::{WM_APP, WM_TIMER};
        let mut pump = FramePump::new(WM_APP + 9);
        assert!(pump.is_frame(WM_APP + 9, 0));
        assert!(!pump.is_frame(WM_APP + 10, 0));
        assert!(!pump.is_frame(WM_TIMER, 0));
        let now = Instant::now();
        unsafe {
            assert!(pump.should_draw(now));
            assert!(!pump.should_draw(now + Duration::from_millis(7)));
            assert!(pump.should_draw(now + Duration::from_millis(9)));
        }
    }

    #[test]
    fn frame_pump_coalesces_posts_and_rejects_stale_wakeups_after_stop() {
        use std::ptr::{null, null_mut};
        use windows_sys::Win32::UI::WindowsAndMessaging::*;
        unsafe {
            let class: Vec<u16> = "STATIC".encode_utf16().chain(Some(0)).collect();
            let hwnd = CreateWindowExW(
                0,
                class.as_ptr(),
                null(),
                WS_POPUP,
                0,
                0,
                1,
                1,
                null_mut(),
                null_mut(),
                null_mut(),
                null(),
            );
            assert!(!hwnd.is_null());
            let mut pump = FramePump::new(WM_APP + 9);
            pump.start(hwnd);
            assert!(pump.is_active());
            let token = pump.wakeup.generation;
            pump.stop();
            assert!(!pump.is_active());
            assert!(!pump.is_frame(WM_APP + 9, token));
            // Drain any real timer post, then exercise the same callback twice.
            let mut message = MSG::default();
            while PeekMessageW(&mut message, hwnd, WM_APP + 9, WM_APP + 9, PM_REMOVE) != 0 {}
            let user = (&*pump.wakeup as *const FrameWakeup) as usize;
            frame_callback(0, 0, user, 0, 0);
            frame_callback(0, 0, user, 0, 0);
            assert_ne!(
                PeekMessageW(&mut message, hwnd, WM_APP + 9, WM_APP + 9, PM_REMOVE),
                0
            );
            assert_eq!(
                PeekMessageW(&mut message, hwnd, WM_APP + 9, WM_APP + 9, PM_REMOVE),
                0
            );
            assert!(pump.should_draw(Instant::now()));
            frame_callback(0, 0, user, 0, 0);
            assert_ne!(
                PeekMessageW(&mut message, hwnd, WM_APP + 9, WM_APP + 9, PM_REMOVE),
                0
            );
            pump.stop();
            // Exercise the real window-timer teardown path without requiring
            // multimedia-timer allocation to fail on this machine.
            let fallback = SetTimer(hwnd, 0x20000, 16, None);
            assert_ne!(fallback, 0);
            pump.source = Some(FrameSource::Window(fallback));
            assert!(pump.is_frame(WM_TIMER, fallback));
            assert!(!pump.is_frame(WM_TIMER, fallback + 1));
            pump.stop();
            assert_eq!(KillTimer(hwnd, fallback), 0);
            pump.start(hwnd);
            assert_ne!(pump.wakeup.generation, token);
            drop(pump);
            DestroyWindow(hwnd);
        }
    }

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
