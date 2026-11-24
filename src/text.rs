use std::mem::{MaybeUninit, size_of};
use std::path::PathBuf;
use std::ptr::{null, null_mut};

use windows_sys::Win32::Foundation::SIZE;
use windows_sys::Win32::Graphics::Gdi::{
    ANTIALIASED_QUALITY, AddFontResourceExW, BI_RGB, BITMAPINFO, BITMAPINFOHEADER,
    CLIP_DEFAULT_PRECIS, CreateCompatibleDC, CreateDIBSection, CreateFontW, DEFAULT_CHARSET,
    DEFAULT_PITCH, DIB_RGB_COLORS, DeleteDC, DeleteObject, FF_DONTCARE, FR_NOT_ENUM, FR_PRIVATE,
    FW_SEMIBOLD, GdiFlush, GetTextExtentPoint32W, GetTextFaceW, GetTextMetricsW, HBITMAP, HDC,
    HFONT, HGDIOBJ, OUT_TT_PRECIS, RemoveFontResourceExW, SelectObject, SetBkMode,
    SetTextCharacterExtra, SetTextColor, TEXTMETRICW, TRANSPARENT, TextOutW,
};

use crate::animation::ShapeState;
use crate::clock::{ClockFormat, ClockSnapshot};
use crate::geometry::rounded_rect_distance;
use crate::i18n::{self, Language};
use crate::render::blend_pixel;
use crate::theme;

const TEXT_SUPERSAMPLE: usize = 3;
const SUPERSAMPLE_SCALE: f32 = TEXT_SUPERSAMPLE as f32;
const COMPACT_TIME_PX: f32 = 21.0;
const EXPANDED_TIME_PX: f32 = 50.0;
const COMPACT_SUFFIX_PX: f32 = 9.0;
const EXPANDED_SUFFIX_PX: f32 = 14.0;
const COMPACT_SECONDS_PX: f32 = 10.0;
const EXPANDED_SECONDS_PX: f32 = 15.0;
const DATE_PX: f32 = 13.0;
const DIGIT_GAP_PX: f32 = 2.0;
const MERIDIEM_GAP_PX: f32 = 6.0;
const DATE_GAP_PX: f32 = 9.0;
const COMPACT_SECONDS_GAP_PX: f32 = 4.0;
const EXPANDED_SECONDS_GAP_PX: f32 = 6.0;
const DIVIDER_GAP_PX: f32 = 12.0;
const DIVIDER_WIDTH_PX: f32 = 28.0;
const DIVIDER_HEIGHT_PX: f32 = 2.0;
const DOT_PX: f32 = 4.0;
const DOT_GAP_PX: f32 = 6.0;
const DATE_TRACKING_EM: f32 = 0.1;
const SCRATCH_ORIGIN_PX: f32 = 8.0;
const SCRATCH_DESIGN: (f32, f32) = (280.0, 90.0);
const EMBEDDED_FONTS: [(&str, &[u8]); 3] = [
    (
        "Inter-Medium.ttf",
        include_bytes!("../assets/fonts/Inter-Medium.ttf"),
    ),
    (
        "Inter-SemiBold.ttf",
        include_bytes!("../assets/fonts/Inter-SemiBold.ttf"),
    ),
    (
        "InterDisplay-SemiBold.ttf",
        include_bytes!("../assets/fonts/InterDisplay-SemiBold.ttf"),
    ),
];

pub(crate) fn clock_date_in(clock: &ClockSnapshot, language: Language) -> String {
    if i18n::resolve_language(language) != Language::Vietnamese {
        return clock.date_text();
    }
    const WEEKDAYS: [&str; 7] = [
        "Sunday",
        "Monday",
        "Tuesday",
        "Wednesday",
        "Thursday",
        "Friday",
        "Saturday",
    ];
    i18n::format_copy_in(
        Language::Vietnamese,
        "{weekday} · {month} {day}",
        &[
            (
                "weekday",
                i18n::tr_in(Language::Vietnamese, WEEKDAYS[clock.weekday as usize % 7]),
            ),
            ("month", &clock.month.clamp(1, 12).to_string()),
            ("day", &clock.day.to_string()),
        ],
    )
}

pub(crate) fn clock_meridiem_in(
    clock: &ClockSnapshot,
    format: ClockFormat,
    language: Language,
) -> Option<&'static str> {
    clock
        .meridiem(format)
        .map(|suffix| i18n::tr_in(language, suffix))
}

pub(crate) fn clock_hint(clock: &ClockSnapshot, format: ClockFormat) -> String {
    clock_hint_in(clock, format, i18n::resolved_language())
}

pub(crate) fn clock_hint_in(
    clock: &ClockSnapshot,
    format: ClockFormat,
    language: Language,
) -> String {
    let mut hint = format!(
        "{} · {}",
        clock_date_in(clock, language),
        clock.time_text(format)
    );
    if let Some(suffix) = clock_meridiem_in(clock, format, language) {
        hint.push(' ');
        hint.push_str(suffix);
    }
    hint
}

pub(crate) struct Mask {
    pub width: u32,
    pub height: u32,
    advance: u32,
    pub coverage: Vec<u8>,
}

struct FontResources {
    paths: Vec<Vec<u16>>,
}

impl Drop for FontResources {
    fn drop(&mut self) {
        for path in &self.paths {
            // SAFETY: path is a NUL-terminated wide string previously loaded with the same flags.
            unsafe {
                RemoveFontResourceExW(path.as_ptr(), FR_PRIVATE | FR_NOT_ENUM, null());
            }
        }
    }
}

struct Fonts {
    compact_time: HFONT,
    compact_suffix: HFONT,
    expanded_time: HFONT,
    expanded_suffix: HFONT,
    compact_seconds: HFONT,
    expanded_seconds: HFONT,
    date: HFONT,
}

impl Fonts {
    fn all(&self) -> [HFONT; 7] {
        [
            self.compact_time,
            self.compact_suffix,
            self.expanded_time,
            self.expanded_suffix,
            self.compact_seconds,
            self.expanded_seconds,
            self.date,
        ]
    }
}

#[derive(Clone, Copy, Debug, PartialEq)]
pub(crate) struct TextRect {
    pub x: f32,
    pub y: f32,
    pub width: f32,
    pub height: f32,
}

impl TextRect {
    fn center_y(self) -> f32 {
        self.y + self.height * 0.5
    }
}

#[derive(Clone, Copy, Debug, PartialEq)]
pub(crate) struct CompactLayout {
    pub dot: TextRect,
    pub time: TextRect,
    pub seconds: Option<TextRect>,
}

#[derive(Clone, Copy, Debug, PartialEq)]
pub(crate) struct ExpandedLayout {
    pub date: Option<TextRect>,
    pub time: TextRect,
    pub seconds: Option<TextRect>,
    pub divider: TextRect,
}

pub(crate) fn compact_layout(
    canvas_width: f32,
    center_y: f32,
    time: (f32, f32),
    seconds: Option<(f32, f32)>,
    scale: f32,
) -> CompactLayout {
    let dot = DOT_PX * scale;
    let dot_gap = DOT_GAP_PX * scale;
    let seconds_gap = COMPACT_SECONDS_GAP_PX * scale;
    let group_width = dot + dot_gap + time.0 + seconds.map_or(0.0, |value| seconds_gap + value.0);
    let start = canvas_width * 0.5 - group_width * 0.5;
    let dot_rect = TextRect {
        x: start,
        y: center_y - dot * 0.5,
        width: dot,
        height: dot,
    };
    let time_rect = TextRect {
        x: start + dot + dot_gap,
        y: center_y - time.1 * 0.5,
        width: time.0,
        height: time.1,
    };
    let seconds_rect = seconds.map(|(width, height)| TextRect {
        x: time_rect.x + time.0 + seconds_gap,
        y: center_y - height * 0.5,
        width,
        height,
    });

    CompactLayout {
        dot: dot_rect,
        time: time_rect,
        seconds: seconds_rect,
    }
}

pub(crate) fn expanded_layout(
    canvas_width: f32,
    top: f32,
    shape_height: f32,
    date: Option<(f32, f32)>,
    time: (f32, f32),
    seconds: Option<(f32, f32)>,
    scale: f32,
) -> ExpandedLayout {
    let date_gap = DATE_GAP_PX * scale;
    let divider_gap = DIVIDER_GAP_PX * scale;
    let divider_width = DIVIDER_WIDTH_PX * scale;
    let divider_height = DIVIDER_HEIGHT_PX * scale;
    let seconds_gap = EXPANDED_SECONDS_GAP_PX * scale;
    let date_height = date.map_or(0.0, |value| value.1 + date_gap);
    let block_height = date_height + time.1 + divider_gap + divider_height;
    let block_top = top + (shape_height - block_height).max(0.0) * 0.5;

    let date_rect = date.map(|(width, height)| TextRect {
        x: canvas_width * 0.5 - width * 0.5,
        y: block_top,
        width,
        height,
    });
    let row_y = block_top + date_height;
    let row_width = time.0 + seconds.map_or(0.0, |value| seconds_gap + value.0);
    let row_left = canvas_width * 0.5 - row_width * 0.5;
    let time_rect = TextRect {
        x: row_left,
        y: row_y,
        width: time.0,
        height: time.1,
    };
    let seconds_rect = seconds.map(|(width, height)| TextRect {
        x: time_rect.x + time.0 + seconds_gap,
        y: row_y + time.1 * 0.10,
        width,
        height,
    });
    let divider_rect = TextRect {
        x: canvas_width * 0.5 - divider_width * 0.5,
        y: row_y + time.1 + divider_gap,
        width: divider_width,
        height: divider_height,
    };

    ExpandedLayout {
        date: date_rect,
        time: time_rect,
        seconds: seconds_rect,
        divider: divider_rect,
    }
}

#[derive(Clone, Copy, Debug, PartialEq)]
pub(crate) struct TextParams {
    pub size: (u32, u32),
    pub state: ShapeState,
    pub top_margin: f32,
    pub scale: f32,
    pub accent: [u8; 3],
    pub show_seconds: bool,
    pub hover: f32,
    pub compact_trailing_space: f32,
}

pub(crate) struct TextLayer {
    dc: HDC,
    bitmap: HBITMAP,
    previous_bitmap: HGDIOBJ,
    previous_font: HGDIOBJ,
    bits: *mut u8,
    scratch_size: (u32, u32),
    origin: i32,
    scale: f32,
    fonts: Fonts,
    _font_resources: Option<FontResources>,
    key: Option<(ClockSnapshot, ClockFormat, Language)>,
    compact: Option<Mask>,
    expanded: Option<Mask>,
    compact_seconds: Option<Mask>,
    expanded_seconds: Option<Mask>,
    date: Option<Mask>,
}

impl TextLayer {
    pub(crate) fn label_advance(&self, text: &str, px: f32, bold: bool) -> f32 {
        let face = wide("Inter");
        unsafe {
            let font = CreateFontW(
                -(px * self.scale * SUPERSAMPLE_SCALE).round() as i32,
                0,
                0,
                0,
                if bold { 600 } else { 400 },
                0,
                0,
                0,
                DEFAULT_CHARSET as u32,
                OUT_TT_PRECIS as u32,
                CLIP_DEFAULT_PRECIS as u32,
                ANTIALIASED_QUALITY as u32,
                (DEFAULT_PITCH | FF_DONTCARE) as u32,
                face.as_ptr(),
            );
            if font.is_null() {
                return 0.0;
            }
            let previous = SelectObject(self.dc, font);
            let width = text_extent(self.dc, text).0 as f32 / (SUPERSAMPLE_SCALE * self.scale);
            SelectObject(self.dc, previous);
            DeleteObject(font);
            width
        }
    }
    pub(crate) fn label_mask(&mut self, text: &str, px: f32, bold: bool) -> Option<Mask> {
        let face = wide("Inter");
        let font = unsafe {
            CreateFontW(
                -(px * self.scale * SUPERSAMPLE_SCALE).round() as i32,
                0,
                0,
                0,
                if bold { 600 } else { 400 },
                0,
                0,
                0,
                DEFAULT_CHARSET as u32,
                OUT_TT_PRECIS as u32,
                CLIP_DEFAULT_PRECIS as u32,
                ANTIALIASED_QUALITY as u32,
                (DEFAULT_PITCH | FF_DONTCARE) as u32,
                face.as_ptr(),
            )
        };
        if font.is_null() {
            return None;
        }
        let previous = unsafe { SelectObject(self.dc, font) };
        let (width, height) = text_extent(self.dc, text);
        let result = if self.ensure_scratch((
            (width + self.origin * 2).max(1) as u32,
            (height + self.origin * 2).max(1) as u32,
        )) {
            self.clear();
            draw_text(self.dc, text, self.origin, self.origin);
            // Keep the measured cell, including whitespace and baseline, so independently
            // colored word fragments occupy the same positions as the complete line.
            unsafe {
                GdiFlush();
            }
            let pixels = unsafe {
                std::slice::from_raw_parts(
                    self.bits,
                    self.scratch_size.0 as usize * self.scratch_size.1 as usize * 4,
                )
            };
            let w = (width.max(1) as usize).div_ceil(TEXT_SUPERSAMPLE);
            let h = (height.max(1) as usize).div_ceil(TEXT_SUPERSAMPLE);
            let mut coverage = vec![0; w * h];
            for y in 0..h {
                for x in 0..w {
                    let mut sum = 0_u32;
                    for sy in 0..TEXT_SUPERSAMPLE {
                        for sx in 0..TEXT_SUPERSAMPLE {
                            let ix = self.origin as usize + x * TEXT_SUPERSAMPLE + sx;
                            let iy = self.origin as usize + y * TEXT_SUPERSAMPLE + sy;
                            let i = (iy * self.scratch_size.0 as usize + ix) * 4;
                            sum += u32::from(pixels[i].max(pixels[i + 1]).max(pixels[i + 2]));
                        }
                    }
                    coverage[y * w + x] =
                        (sum / (TEXT_SUPERSAMPLE * TEXT_SUPERSAMPLE) as u32) as u8;
                }
            }
            Some(Mask {
                width: w as u32,
                height: h as u32,
                advance: w as u32,
                coverage,
            })
        } else {
            None
        };
        unsafe {
            SelectObject(self.dc, previous);
            DeleteObject(font);
        }
        result
    }

    pub(crate) fn new(scale: f32) -> Result<Self, String> {
        let scale = scale.max(0.5);
        let scratch_size = (
            supersampled(SCRATCH_DESIGN.0, scale),
            supersampled(SCRATCH_DESIGN.1, scale),
        );
        let origin = (SCRATCH_ORIGIN_PX * scale * SUPERSAMPLE_SCALE).round() as i32;

        // SAFETY: The memory DC, DIB, fonts, and resources are owned by the returned TextLayer.
        unsafe {
            let dc = CreateCompatibleDC(null_mut());
            if dc.is_null() {
                return Err("CreateCompatibleDC failed for text".to_string());
            }
            let Some((bitmap, bits)) = create_scratch_bitmap(dc, scratch_size) else {
                DeleteDC(dc);
                return Err("CreateDIBSection failed for text".to_string());
            };
            let previous_bitmap = SelectObject(dc, bitmap);
            if previous_bitmap.is_null() || previous_bitmap as isize == -1 {
                DeleteObject(bitmap);
                DeleteDC(dc);
                return Err("SelectObject failed for text".to_string());
            }

            SetBkMode(dc, TRANSPARENT as i32);
            // GDI renders a full-white coverage mask; UI colors are applied during blending.
            SetTextColor(dc, 0x00FF_FFFF);

            let (fonts, font_resources) = match create_fonts(dc, scale) {
                Ok(value) => value,
                Err(error) => {
                    SelectObject(dc, previous_bitmap);
                    DeleteObject(bitmap);
                    DeleteDC(dc);
                    return Err(error);
                }
            };
            let previous_font = SelectObject(dc, fonts.compact_time);

            Ok(Self {
                dc,
                bitmap,
                previous_bitmap,
                previous_font,
                bits,
                scratch_size,
                origin,
                scale,
                fonts,
                _font_resources: font_resources,
                key: None,
                compact: None,
                expanded: None,
                compact_seconds: None,
                expanded_seconds: None,
                date: None,
            })
        }
    }

    pub(crate) fn update(&mut self, clock: &ClockSnapshot, format: ClockFormat) {
        self.update_in(clock, format, i18n::resolved_language());
    }

    fn update_in(&mut self, clock: &ClockSnapshot, format: ClockFormat, language: Language) {
        if self.key.is_none_or(|(previous, old_format, old_language)| {
            (previous.hour, previous.minute, old_format, old_language)
                != (clock.hour, clock.minute, format, language)
        }) {
            let time = clock.time_text(format);
            let meridiem = clock_meridiem_in(clock, format, language);
            self.clear();
            self.compact = self.draw_time(
                &time,
                meridiem,
                self.fonts.compact_time,
                self.fonts.compact_suffix,
            );
            self.clear();
            self.expanded = self.draw_time(
                &time,
                meridiem,
                self.fonts.expanded_time,
                self.fonts.expanded_suffix,
            );
        }
        if self
            .key
            .is_none_or(|(previous, _, _)| previous.second != clock.second)
        {
            let seconds = clock.seconds_text();
            self.clear();
            self.compact_seconds = self.draw_seconds(&seconds, self.fonts.compact_seconds);
            self.clear();
            self.expanded_seconds = self.draw_seconds(&seconds, self.fonts.expanded_seconds);
        }
        if self.key.is_none_or(|(previous, _, old_language)| {
            (previous.weekday, previous.month, previous.day, old_language)
                != (clock.weekday, clock.month, clock.day, language)
        }) {
            self.clear();
            self.date = self.draw_date(&clock_date_in(clock, language).to_uppercase());
        }
        self.key = Some((*clock, format, language));
    }

    #[cfg(test)]
    pub(crate) fn draw(&self, pixels: &mut [u8], params: TextParams) {
        self.draw_with_opacity(pixels, params, 1.0);
    }

    pub(crate) fn draw_with_opacity(&self, pixels: &mut [u8], params: TextParams, opacity: f32) {
        let opacity = opacity.clamp(0.0, 1.0);
        if opacity <= 0.0 {
            return;
        }
        let shape = scale_shape(params.state, params.scale);
        let center = (
            params.size.0 as f32 * 0.5,
            params.top_margin + shape.height * 0.5,
        );
        let compact_alpha = params.state.compact_content_alpha().clamp(0.0, 1.0) * opacity;
        let compact_center_y =
            params.top_margin + shape.height * 0.5 + params.state.compact_lift() * params.scale;

        if compact_alpha > 0.0
            && let Some(mask) = &self.compact
        {
            let seconds = params
                .show_seconds
                .then_some(self.compact_seconds.as_ref())
                .flatten();
            let layout = compact_layout(
                params.size.0 as f32 - params.compact_trailing_space * params.scale,
                compact_center_y,
                (mask.width as f32, mask.height as f32),
                seconds.map(|value| (value.advance as f32, value.height as f32)),
                params.scale,
            );
            let dot_center = (layout.dot.x + layout.dot.width * 0.5, layout.dot.center_y());
            if params.hover > 0.0 {
                fill_circle(
                    pixels,
                    params.size,
                    dot_center,
                    layout.dot.width * 0.95,
                    compact_alpha * params.hover * 0.08,
                    params.accent,
                    shape,
                    center,
                );
            }
            fill_circle(
                pixels,
                params.size,
                dot_center,
                layout.dot.width * 0.5,
                compact_alpha,
                params.accent,
                shape,
                center,
            );
            blend_mask(
                pixels,
                params.size,
                mask,
                layout.time.x,
                layout.time.y,
                compact_alpha,
                theme::TEXT,
                shape,
                center,
            );
            if let (Some(seconds), Some(rect)) = (seconds, layout.seconds) {
                blend_mask(
                    pixels,
                    params.size,
                    seconds,
                    rect.x + (rect.width - seconds.width as f32) * 0.5,
                    rect.y,
                    compact_alpha,
                    theme::MUTED,
                    shape,
                    center,
                );
            }
        }

        let expanded_alpha = params.state.expanded_content_alpha().clamp(0.0, 1.0) * opacity;
        if expanded_alpha > 0.0
            && let Some(mask) = &self.expanded
        {
            let date = self.date.as_ref();
            let seconds = params
                .show_seconds
                .then_some(self.expanded_seconds.as_ref())
                .flatten();
            let content_scale = params.state.expanded_scale();
            let lift = params.state.expanded_lift() * params.scale;
            let layout = expanded_layout(
                params.size.0 as f32,
                params.top_margin + lift,
                shape.height,
                date.map(|value| (value.width as f32, value.height as f32)),
                (mask.width as f32, mask.height as f32),
                seconds.map(|value| (value.advance as f32, value.height as f32)),
                params.scale,
            );
            if let (Some(date), Some(rect)) = (date, layout.date) {
                blend_mask_scaled(
                    pixels,
                    params.size,
                    date,
                    rect.x,
                    rect.y,
                    expanded_alpha,
                    theme::mix(theme::DIM, theme::MUTED, 0.75),
                    shape,
                    center,
                    content_scale,
                );
            }
            blend_mask_scaled(
                pixels,
                params.size,
                mask,
                layout.time.x,
                layout.time.y,
                expanded_alpha,
                theme::TEXT,
                shape,
                center,
                content_scale,
            );
            if let (Some(seconds), Some(rect)) = (seconds, layout.seconds) {
                blend_mask_scaled(
                    pixels,
                    params.size,
                    seconds,
                    rect.x + (rect.width - seconds.width as f32) * 0.5,
                    rect.y,
                    expanded_alpha,
                    theme::MUTED,
                    shape,
                    center,
                    content_scale,
                );
            }
            fill_rect(
                pixels,
                params.size,
                (layout.divider.x, layout.divider.y),
                (layout.divider.width, layout.divider.height),
                expanded_alpha,
                params.accent,
                shape,
                center,
            );
        }
    }

    #[cfg(test)]
    pub(crate) fn scratch_size(&self) -> (u32, u32) {
        self.scratch_size
    }

    fn clear(&mut self) {
        let length = self.scratch_size.0 as usize * self.scratch_size.1 as usize * 4;
        // SAFETY: The DIB allocated exactly this many bytes and is owned by self.
        unsafe { std::ptr::write_bytes(self.bits, 0, length) };
    }

    fn ensure_scratch(&mut self, required: (u32, u32)) -> bool {
        if required.0 <= self.scratch_size.0 && required.1 <= self.scratch_size.1 {
            return true;
        }
        let width = required
            .0
            .max((self.scratch_size.0 as f32 * 1.15) as u32)
            .max(1);
        let height = required
            .1
            .max((self.scratch_size.1 as f32 * 1.15) as u32)
            .max(1);
        // SAFETY: The new DIB replaces the old one in the same live DC.
        unsafe {
            let Some((bitmap, bits)) = create_scratch_bitmap(self.dc, (width, height)) else {
                return false;
            };
            let previous = SelectObject(self.dc, bitmap);
            if previous.is_null() || previous as isize == -1 {
                DeleteObject(bitmap);
                return false;
            }
            DeleteObject(self.bitmap);
            self.bitmap = bitmap;
            self.bits = bits;
            self.scratch_size = (width, height);
        }
        true
    }

    fn draw_time(
        &mut self,
        time: &str,
        meridiem: Option<&'static str>,
        time_font: HFONT,
        suffix_font: HFONT,
    ) -> Option<Mask> {
        let gap = (DIGIT_GAP_PX * self.scale * SUPERSAMPLE_SCALE).round() as i32;
        // SAFETY: dc is a live memory DC and every string pointer is valid for the call.
        unsafe {
            SelectObject(self.dc, time_font);
            let metrics = text_metrics(self.dc);
            let digit_width = ('0'..='9')
                .map(|digit| text_extent(self.dc, &digit.to_string()).0)
                .max()
                .unwrap_or(1);
            let colon_width = text_extent(self.dc, ":").0;

            let mut glyphs = Vec::with_capacity(time.chars().count());
            let mut required_width = self.origin;
            for character in time.chars() {
                let glyph = character.to_string();
                let (glyph_width, _) = text_extent(self.dc, &glyph);
                let cell = if character == ':' {
                    (colon_width as f32 * 1.2).round() as i32
                } else {
                    digit_width + gap
                };
                required_width += cell.max(glyph_width);
                glyphs.push((glyph, glyph_width, cell));
            }
            required_width += self.origin;

            let mut suffix_height = 0;
            if let Some(meridiem) = meridiem {
                SelectObject(self.dc, suffix_font);
                let suffix_metrics = text_metrics(self.dc);
                suffix_height = suffix_metrics.tmHeight;
                required_width += (MERIDIEM_GAP_PX * self.scale * SUPERSAMPLE_SCALE).round() as i32
                    + text_extent(self.dc, meridiem).0;
            }
            let required_height = self.origin + metrics.tmHeight.max(suffix_height) + self.origin;
            if !self.ensure_scratch((required_width.max(1) as u32, required_height.max(1) as u32)) {
                return None;
            }

            SelectObject(self.dc, time_font);
            let mut x = self.origin;
            for (glyph, glyph_width, cell) in glyphs {
                draw_text(self.dc, &glyph, x + (cell - glyph_width) / 2, self.origin);
                x += cell.max(glyph_width);
            }

            if let Some(meridiem) = meridiem {
                SelectObject(self.dc, suffix_font);
                let suffix_metrics = text_metrics(self.dc);
                x += (MERIDIEM_GAP_PX * self.scale * SUPERSAMPLE_SCALE).round() as i32;
                let baseline = self.origin + metrics.tmAscent;
                let suffix_y = baseline - suffix_metrics.tmAscent;
                draw_text(self.dc, meridiem, x, suffix_y);
            }
        }
        self.extract_mask()
    }

    fn draw_date(&mut self, date: &str) -> Option<Mask> {
        // SAFETY: dc is live and the text pointer is valid for the duration of TextOutW.
        unsafe {
            SelectObject(self.dc, self.fonts.date);
            let tracking =
                (DATE_TRACKING_EM * DATE_PX * self.scale * SUPERSAMPLE_SCALE).round() as i32;
            SetTextCharacterExtra(self.dc, tracking);
            let (width, _) = text_extent(self.dc, date);
            let height = text_metrics(self.dc).tmHeight;
            let ready = self.ensure_scratch((
                (self.origin + width + self.origin).max(1) as u32,
                (self.origin + height + self.origin).max(1) as u32,
            ));
            if !ready {
                SetTextCharacterExtra(self.dc, 0);
                return None;
            }
            draw_text(self.dc, date, self.origin, self.origin);
            SetTextCharacterExtra(self.dc, 0);
        }
        self.extract_mask()
    }

    fn draw_seconds(&mut self, seconds: &str, font: HFONT) -> Option<Mask> {
        // SAFETY: dc is a live memory DC and every string pointer is valid for the call.
        let advance = unsafe {
            SelectObject(self.dc, font);
            let cell = ('0'..='9')
                .map(|digit| text_extent(self.dc, &digit.to_string()).0)
                .max()?;
            let height = text_metrics(self.dc).tmHeight;
            let required_width = self.origin + cell * seconds.chars().count() as i32 + self.origin;
            if !self.ensure_scratch((
                required_width.max(1) as u32,
                (self.origin + height + self.origin).max(1) as u32,
            )) {
                return None;
            }
            for (index, digit) in seconds.chars().enumerate() {
                let glyph = digit.to_string();
                let width = text_extent(self.dc, &glyph).0;
                draw_text(
                    self.dc,
                    &glyph,
                    self.origin + index as i32 * cell + (cell - width) / 2,
                    self.origin,
                );
            }
            ((cell * seconds.chars().count() as i32) as f32 / SUPERSAMPLE_SCALE).ceil() as u32
        };
        let mut mask = self.extract_mask()?;
        mask.advance = advance.max(mask.width);
        Some(mask)
    }

    fn extract_mask(&self) -> Option<Mask> {
        let width = self.scratch_size.0 as usize;
        let height = self.scratch_size.1 as usize;
        // SAFETY: Flush queued GDI writes before reading our mapped DIB.
        unsafe { GdiFlush() };
        let pixels = unsafe { std::slice::from_raw_parts(self.bits, width * height * 4) };
        let mut min_x = width;
        let mut min_y = height;
        let mut max_x = 0_usize;
        let mut max_y = 0_usize;

        for y in 0..height {
            for x in 0..width {
                let index = (y * width + x) * 4;
                let coverage = pixels[index].max(pixels[index + 1]).max(pixels[index + 2]);
                if coverage > 0 {
                    min_x = min_x.min(x);
                    max_x = max_x.max(x);
                    min_y = min_y.min(y);
                    max_y = max_y.max(y);
                }
            }
        }
        if min_x > max_x {
            return None;
        }

        self.downsample(pixels, width, height, (min_x, min_y, max_x, max_y))
    }

    fn downsample(
        &self,
        pixels: &[u8],
        stride: usize,
        height: usize,
        bounds: (usize, usize, usize, usize),
    ) -> Option<Mask> {
        let (min_x, min_y, max_x, max_y) = bounds;
        let left = min_x / TEXT_SUPERSAMPLE;
        let top = min_y / TEXT_SUPERSAMPLE;
        let right = ((max_x + 1).div_ceil(TEXT_SUPERSAMPLE)).min(stride / TEXT_SUPERSAMPLE);
        let bottom = ((max_y + 1).div_ceil(TEXT_SUPERSAMPLE)).min(height / TEXT_SUPERSAMPLE);
        if left >= right || top >= bottom {
            return None;
        }
        let width = right - left;
        let sample_count = (TEXT_SUPERSAMPLE * TEXT_SUPERSAMPLE) as u32;
        let mut coverage = vec![0_u8; width * (bottom - top)];
        for row in 0..bottom - top {
            for column in 0..width {
                let mut sum = 0_u32;
                for sy in 0..TEXT_SUPERSAMPLE {
                    let source_y = (top + row) * TEXT_SUPERSAMPLE + sy;
                    for sx in 0..TEXT_SUPERSAMPLE {
                        let source_x = (left + column) * TEXT_SUPERSAMPLE + sx;
                        let index = (source_y * stride + source_x) * 4;
                        sum += pixels[index].max(pixels[index + 1]).max(pixels[index + 2]) as u32;
                    }
                }
                coverage[row * width + column] = (sum / sample_count) as u8;
            }
        }

        let first_column =
            (0..width).find(|&x| (0..bottom - top).any(|y| coverage[y * width + x] > 0))?;
        let last_column = (0..width)
            .rev()
            .find(|&x| (0..bottom - top).any(|y| coverage[y * width + x] > 0))?;
        let first_row =
            (0..bottom - top).find(|&y| (0..width).any(|x| coverage[y * width + x] > 0))?;
        let last_row = (0..bottom - top)
            .rev()
            .find(|&y| (0..width).any(|x| coverage[y * width + x] > 0))?;
        let trimmed_width = last_column - first_column + 1;
        let trimmed_height = last_row - first_row + 1;
        let mut trimmed = Vec::with_capacity(trimmed_width * trimmed_height);
        for y in first_row..=last_row {
            for x in first_column..=last_column {
                trimmed.push(coverage[y * width + x]);
            }
        }

        Some(Mask {
            width: trimmed_width as u32,
            height: trimmed_height as u32,
            advance: trimmed_width as u32,
            coverage: trimmed,
        })
    }
}

impl Drop for TextLayer {
    fn drop(&mut self) {
        // SAFETY: Restore the original GDI objects before deleting owned fonts, the
        // bitmap, and the memory DC.
        unsafe {
            SelectObject(self.dc, self.previous_font);
            for font in self.fonts.all() {
                DeleteObject(font);
            }
            SelectObject(self.dc, self.previous_bitmap);
            DeleteObject(self.bitmap);
            DeleteDC(self.dc);
        }
    }
}

fn supersampled(design_px: f32, scale: f32) -> u32 {
    (design_px * scale * SUPERSAMPLE_SCALE).ceil().max(1.0) as u32
}

unsafe fn create_scratch_bitmap(dc: HDC, size: (u32, u32)) -> Option<(HBITMAP, *mut u8)> {
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
    let mut bits = MaybeUninit::uninit();
    // SAFETY: info describes a valid 32-bit top-down DIB. On success, the call
    // initializes `bits` with a pointer into the DIB.
    let bitmap =
        unsafe { CreateDIBSection(dc, &info, DIB_RGB_COLORS, bits.as_mut_ptr(), null_mut(), 0) };
    if bitmap.is_null() {
        return None;
    }
    // SAFETY: A successful CreateDIBSection call initializes the out parameter.
    let bits = unsafe { bits.assume_init() };
    if bits.is_null() {
        // SAFETY: bitmap was just created and is not selected anywhere.
        unsafe { DeleteObject(bitmap) };
        return None;
    }
    Some((bitmap, bits.cast()))
}

fn install_fonts() -> Option<FontResources> {
    let directory = PathBuf::from(std::env::var_os("LOCALAPPDATA")?)
        .join("HiyoDyna")
        .join("fonts");
    std::fs::create_dir_all(&directory).ok()?;
    let mut paths = Vec::with_capacity(EMBEDDED_FONTS.len());
    for (name, data) in EMBEDDED_FONTS {
        let path = directory.join(name);
        let needs_write = std::fs::metadata(&path)
            .map(|metadata| metadata.len() != data.len() as u64)
            .unwrap_or(true);
        if needs_write && std::fs::write(&path, data).is_err() {
            continue;
        }
        let wide_path = wide(&path.to_string_lossy());
        // SAFETY: wide_path is a NUL-terminated wide string for the duration of the call.
        let added =
            unsafe { AddFontResourceExW(wide_path.as_ptr(), FR_PRIVATE | FR_NOT_ENUM, null()) };
        if added > 0 {
            paths.push(wide_path);
        }
    }
    if paths.is_empty() {
        None
    } else {
        Some(FontResources { paths })
    }
}

fn create_fonts(dc: HDC, scale: f32) -> Result<(Fonts, Option<FontResources>), String> {
    let resources = install_fonts();
    if resources.is_some()
        && let Some(fonts) = build_fonts(dc, scale, true)
    {
        return Ok((fonts, resources));
    }
    let fonts = build_fonts(dc, scale, false).ok_or("CreateFontW failed for text")?;
    Ok((fonts, None))
}

fn build_fonts(dc: HDC, scale: f32, inter: bool) -> Option<Fonts> {
    let time_weight = if inter { 400 } else { FW_SEMIBOLD as i32 };
    let secondary_weight = 400;
    let medium: &[&str] = if inter {
        &["Inter Medium", "Inter"]
    } else {
        &["Segoe UI"]
    };
    let semibold: &[&str] = if inter {
        &["Inter SemiBold", "Inter"]
    } else {
        &["Segoe UI"]
    };
    let display: &[&str] = if inter {
        &[
            "Inter Display SemiBold",
            "Inter Display",
            "Inter SemiBold",
            "Inter",
        ]
    } else {
        &["Segoe UI"]
    };
    let size = |design_px: f32| design_px * scale * SUPERSAMPLE_SCALE;
    let specs: [(f32, i32, &[&str]); 7] = [
        (size(COMPACT_TIME_PX), time_weight, semibold),
        (size(COMPACT_SUFFIX_PX), time_weight, medium),
        (size(EXPANDED_TIME_PX), time_weight, display),
        (size(EXPANDED_SUFFIX_PX), time_weight, medium),
        (size(COMPACT_SECONDS_PX), secondary_weight, medium),
        (size(EXPANDED_SECONDS_PX), secondary_weight, medium),
        (size(DATE_PX), secondary_weight, medium),
    ];
    let mut handles = Vec::with_capacity(specs.len());
    for (physical_px, weight, candidates) in specs {
        let Some(font) = pick_font(dc, physical_px, weight, candidates, inter) else {
            // SAFETY: every handle was created here and none are selected.
            unsafe {
                for font in handles {
                    DeleteObject(font);
                }
            }
            return None;
        };
        handles.push(font);
    }

    Some(Fonts {
        compact_time: handles[0],
        compact_suffix: handles[1],
        expanded_time: handles[2],
        expanded_suffix: handles[3],
        compact_seconds: handles[4],
        expanded_seconds: handles[5],
        date: handles[6],
    })
}

fn pick_font(
    dc: HDC,
    physical_px: f32,
    weight: i32,
    candidates: &[&str],
    inter: bool,
) -> Option<HFONT> {
    for name in candidates {
        let font = create_font(physical_px, weight, &wide(name));
        if font.is_null() {
            continue;
        }
        // SAFETY: dc is live and font was just created.
        unsafe {
            let previous = SelectObject(dc, font);
            let face = selected_face(dc);
            SelectObject(dc, previous);
            let matches = if inter {
                face.to_ascii_lowercase().contains("inter")
            } else {
                !face.is_empty()
            };
            if matches {
                return Some(font);
            }
            DeleteObject(font);
        }
    }
    None
}

fn selected_face(dc: HDC) -> String {
    let mut buffer = [0_u16; 64];
    // SAFETY: dc is live and buffer is writable for its full length.
    let length = unsafe { GetTextFaceW(dc, buffer.len() as i32, buffer.as_mut_ptr()) };
    if length <= 0 {
        return String::new();
    }
    let length = (length as usize).min(buffer.len());
    let end = buffer[..length]
        .iter()
        .position(|&value| value == 0)
        .unwrap_or(length);
    String::from_utf16_lossy(&buffer[..end])
}

fn create_font(physical_px: f32, weight: i32, face: &[u16]) -> HFONT {
    // In the memory DC's default MM_TEXT mode, a logical unit is one pixel.
    let logical_height = -physical_px.round().max(1.0) as i32;
    // SAFETY: face is a NUL-terminated wide string.
    unsafe {
        CreateFontW(
            logical_height,
            0,
            0,
            0,
            weight,
            0,
            0,
            0,
            DEFAULT_CHARSET as u32,
            OUT_TT_PRECIS as u32,
            CLIP_DEFAULT_PRECIS as u32,
            ANTIALIASED_QUALITY as u32,
            DEFAULT_PITCH as u32 | FF_DONTCARE as u32,
            face.as_ptr(),
        )
    }
}

fn text_extent(dc: HDC, text: &str) -> (i32, i32) {
    let wide: Vec<u16> = text.encode_utf16().collect();
    let mut size = SIZE { cx: 0, cy: 0 };
    // SAFETY: dc is live and wide contains the exact UTF-16 length passed.
    unsafe {
        GetTextExtentPoint32W(dc, wide.as_ptr(), wide.len() as i32, &mut size);
    }
    (size.cx, size.cy)
}

fn text_metrics(dc: HDC) -> TEXTMETRICW {
    let mut metrics = TEXTMETRICW::default();
    // SAFETY: dc is live and selects a valid font.
    unsafe {
        GetTextMetricsW(dc, &mut metrics);
    }
    metrics
}

fn draw_text(dc: HDC, text: &str, x: i32, y: i32) {
    let wide: Vec<u16> = text.encode_utf16().collect();
    // SAFETY: dc is live and wide contains the exact UTF-16 length passed.
    unsafe {
        TextOutW(dc, x, y, wide.as_ptr(), wide.len() as i32);
    }
}

fn scale_shape(state: ShapeState, scale: f32) -> ShapeState {
    ShapeState {
        width: state.width * scale,
        height: state.height * scale,
        radius: state.radius * scale,
    }
}

#[allow(clippy::too_many_arguments)]
fn blend_mask(
    pixels: &mut [u8],
    size: (u32, u32),
    mask: &Mask,
    x: f32,
    y: f32,
    opacity: f32,
    color_bgra: [u8; 3],
    state: ShapeState,
    center: (f32, f32),
) {
    blend_mask_scaled(
        pixels, size, mask, x, y, opacity, color_bgra, state, center, 1.0,
    );
}

#[allow(clippy::too_many_arguments)]
fn blend_mask_scaled(
    pixels: &mut [u8],
    size: (u32, u32),
    mask: &Mask,
    x: f32,
    y: f32,
    opacity: f32,
    color_bgra: [u8; 3],
    state: ShapeState,
    center: (f32, f32),
    mask_scale: f32,
) {
    if opacity <= 0.0 || mask.width == 0 || mask.height == 0 {
        return;
    }
    let canvas_width = size.0 as i32;
    let canvas_height = size.1 as i32;
    let mask_scale = if mask_scale.is_finite() && mask_scale > 0.0 {
        mask_scale
    } else {
        1.0
    };

    if (mask_scale - 1.0).abs() < 0.001 {
        let base_x = x.round() as i32;
        let base_y = y.round() as i32;
        for row in 0..mask.height as i32 {
            let py = base_y + row;
            if py < 0 || py >= canvas_height {
                continue;
            }
            for column in 0..mask.width as i32 {
                let px = base_x + column;
                if px < 0 || px >= canvas_width {
                    continue;
                }
                let coverage = mask.coverage[(row * mask.width as i32 + column) as usize];
                if coverage == 0 {
                    continue;
                }
                let distance =
                    rounded_rect_distance(px as f32 + 0.5, py as f32 + 0.5, state, center);
                let alpha = (coverage as f32 / 255.0) * opacity * (0.5 - distance).clamp(0.0, 1.0);
                if alpha > 0.0 {
                    blend_pixel_at(pixels, size, px, py, color_bgra, alpha);
                }
            }
        }
        return;
    }

    let destination_width = mask.width as f32 * mask_scale;
    let destination_height = mask.height as f32 * mask_scale;
    let left = (x + mask.width as f32 * 0.5 - destination_width * 0.5)
        .floor()
        .max(0.0) as i32;
    let right = (x + mask.width as f32 * 0.5 + destination_width * 0.5)
        .ceil()
        .min(canvas_width as f32) as i32;
    let top = (y + mask.height as f32 * 0.5 - destination_height * 0.5)
        .floor()
        .max(0.0) as i32;
    let bottom = (y + mask.height as f32 * 0.5 + destination_height * 0.5)
        .ceil()
        .min(canvas_height as f32) as i32;
    let destination_left = x + mask.width as f32 * 0.5 - destination_width * 0.5;
    let destination_top = y + mask.height as f32 * 0.5 - destination_height * 0.5;

    for py in top..bottom {
        for px in left..right {
            let u = (px as f32 + 0.5 - destination_left) / mask_scale - 0.5;
            let v = (py as f32 + 0.5 - destination_top) / mask_scale - 0.5;
            if u < -0.5 || v < -0.5 || u > mask.width as f32 - 0.5 || v > mask.height as f32 - 0.5 {
                continue;
            }
            let x0 = u.floor() as i32;
            let y0 = v.floor() as i32;
            let tx = u - x0 as f32;
            let ty = v - y0 as f32;
            let coverage = sample(mask, x0, y0) as f32 * (1.0 - tx) * (1.0 - ty)
                + sample(mask, x0 + 1, y0) as f32 * tx * (1.0 - ty)
                + sample(mask, x0, y0 + 1) as f32 * (1.0 - tx) * ty
                + sample(mask, x0 + 1, y0 + 1) as f32 * tx * ty;
            let distance = rounded_rect_distance(px as f32 + 0.5, py as f32 + 0.5, state, center);
            let alpha = (coverage / 255.0) * opacity * (0.5 - distance).clamp(0.0, 1.0);
            if alpha > 0.0 {
                blend_pixel_at(pixels, size, px, py, color_bgra, alpha);
            }
        }
    }
}

fn sample(mask: &Mask, x: i32, y: i32) -> u8 {
    if x < 0 || y < 0 || x >= mask.width as i32 || y >= mask.height as i32 {
        0
    } else {
        mask.coverage[(y * mask.width as i32 + x) as usize]
    }
}

#[allow(clippy::too_many_arguments)]
fn fill_circle(
    pixels: &mut [u8],
    size: (u32, u32),
    center_point: (f32, f32),
    radius: f32,
    opacity: f32,
    color: [u8; 3],
    state: ShapeState,
    shape_center: (f32, f32),
) {
    let left = (center_point.0 - radius).floor().max(0.0) as i32;
    let right = (center_point.0 + radius).ceil().min(size.0 as f32) as i32;
    let top = (center_point.1 - radius).floor().max(0.0) as i32;
    let bottom = (center_point.1 + radius).ceil().min(size.1 as f32) as i32;
    for y in top..bottom {
        for x in left..right {
            let distance = (x as f32 + 0.5 - center_point.0).hypot(y as f32 + 0.5 - center_point.1);
            let coverage = (radius + 0.5 - distance).clamp(0.0, 1.0)
                * (0.5
                    - rounded_rect_distance(x as f32 + 0.5, y as f32 + 0.5, state, shape_center))
                .clamp(0.0, 1.0);
            if coverage > 0.0 {
                blend_pixel_at(pixels, size, x, y, color, coverage * opacity);
            }
        }
    }
}

#[allow(clippy::too_many_arguments)]
fn fill_rect(
    pixels: &mut [u8],
    size: (u32, u32),
    origin: (f32, f32),
    extent: (f32, f32),
    opacity: f32,
    color: [u8; 3],
    state: ShapeState,
    shape_center: (f32, f32),
) {
    let left = origin.0.round().max(0.0) as i32;
    let right = (origin.0.round() + extent.0.round()).min(size.0 as f32) as i32;
    let top = origin.1.round().max(0.0) as i32;
    let bottom = (origin.1.round() + extent.1.round().max(1.0)).min(size.1 as f32) as i32;
    for y in top..bottom {
        for x in left..right {
            let coverage = (0.5
                - rounded_rect_distance(x as f32 + 0.5, y as f32 + 0.5, state, shape_center))
            .clamp(0.0, 1.0);
            if coverage > 0.0 {
                blend_pixel_at(pixels, size, x, y, color, coverage * opacity);
            }
        }
    }
}

fn blend_pixel_at(pixels: &mut [u8], size: (u32, u32), x: i32, y: i32, color: [u8; 3], alpha: f32) {
    let index = ((y * size.0 as i32 + x) * 4) as usize;
    blend_pixel(&mut pixels[index..index + 4], color, alpha);
}

fn wide(text: &str) -> Vec<u16> {
    text.encode_utf16().chain(Some(0)).collect()
}

#[cfg(test)]
mod tests {
    use super::{Mask, TextLayer, TextParams, blend_mask, compact_layout, expanded_layout};
    use crate::animation::{COMPACT, EXPANDED};
    use crate::clock::{ClockFormat, ClockSnapshot};
    use crate::i18n::Language;

    use crate::theme::ICE;

    fn snapshot() -> ClockSnapshot {
        snapshot_with_second(17)
    }

    fn snapshot_with_second(second: u16) -> ClockSnapshot {
        ClockSnapshot {
            hour: 21,
            minute: 41,
            second,
            weekday: 3,
            month: 11,
            day: 18,
        }
    }

    #[test]
    fn vietnamese_clock_uses_day_month_order_and_localized_meridiem() {
        assert_eq!(
            super::clock_date_in(&snapshot(), Language::Vietnamese),
            "Thứ tư · 18 tháng 11"
        );
        assert_eq!(
            super::clock_date_in(&snapshot(), Language::English),
            "Wednesday · November 18"
        );
        let sunday = ClockSnapshot {
            weekday: 0,
            month: 1,
            day: 1,
            hour: 0,
            ..snapshot()
        };
        assert_eq!(
            super::clock_date_in(&sunday, Language::Vietnamese),
            "Chủ nhật · 1 tháng 1"
        );
        assert_eq!(
            super::clock_meridiem_in(&sunday, ClockFormat::TwelveHour, Language::Vietnamese),
            Some("SA")
        );
        assert_eq!(
            super::clock_meridiem_in(&snapshot(), ClockFormat::TwelveHour, Language::Vietnamese),
            Some("CH")
        );
        assert_eq!(
            super::clock_meridiem_in(
                &snapshot(),
                ClockFormat::TwentyFourHour,
                Language::Vietnamese
            ),
            None
        );
        assert_eq!(
            super::clock_meridiem_in(&snapshot(), ClockFormat::TwelveHour, Language::English),
            Some("PM")
        );
    }

    #[test]
    fn clock_hint_contains_the_localized_date_and_unambiguous_time() {
        assert_eq!(
            super::clock_hint_in(&snapshot(), ClockFormat::TwelveHour, Language::Vietnamese),
            "Thứ tư · 18 tháng 11 · 9:41 CH"
        );
        assert_eq!(
            super::clock_hint_in(&snapshot(), ClockFormat::TwentyFourHour, Language::English),
            "Wednesday · November 18 · 21:41"
        );
        let invalid = ClockSnapshot {
            weekday: 9,
            month: 99,
            ..snapshot()
        };
        assert_eq!(
            super::clock_date_in(&invalid, Language::Vietnamese),
            "Thứ ba · 18 tháng 12"
        );
    }

    #[test]
    fn language_switch_refreshes_date_and_suffix_without_a_clock_tick() {
        for scale in [1.0, 1.25, 1.5, 2.0] {
            let mut layer = layer(scale);
            layer.update_in(&snapshot(), ClockFormat::TwelveHour, Language::English);
            let english_date = layer.date.as_ref().unwrap().coverage.clone();
            let english_time = layer.compact.as_ref().unwrap().coverage.clone();
            let seconds = layer.compact_seconds.as_ref().unwrap().coverage.clone();
            layer.update_in(&snapshot(), ClockFormat::TwelveHour, Language::Vietnamese);
            assert!(
                layer.date.as_ref().unwrap().coverage != english_date,
                "date at {scale}x"
            );
            assert!(
                layer.compact.as_ref().unwrap().coverage != english_time,
                "suffix at {scale}x"
            );
            assert_eq!(layer.compact_seconds.as_ref().unwrap().coverage, seconds);
            layer.update_in(&snapshot(), ClockFormat::TwelveHour, Language::English);
            assert_eq!(layer.date.as_ref().unwrap().coverage, english_date);
            assert_eq!(layer.compact.as_ref().unwrap().coverage, english_time);
        }
    }

    #[test]
    fn vietnamese_clock_masks_fit_the_existing_capsule_at_each_dpi() {
        for scale in [1.0, 1.25, 1.5, 2.0] {
            let mut layer = layer(scale);
            for format in [ClockFormat::TwentyFourHour, ClockFormat::TwelveHour] {
                for hour in [0, 9, 12, 21, 23] {
                    for weekday in 0..7 {
                        let clock = ClockSnapshot {
                            hour,
                            weekday,
                            month: 12,
                            day: 31,
                            ..snapshot()
                        };
                        layer.update_in(&clock, format, Language::Vietnamese);
                        let time = layer.compact.as_ref().unwrap();
                        let seconds = layer.compact_seconds.as_ref().unwrap();
                        let layout = compact_layout(
                            148.0 * scale,
                            20.0 * scale,
                            (time.width as f32, time.height as f32),
                            Some((seconds.advance as f32, seconds.height as f32)),
                            scale,
                        );
                        assert!(layout.dot.x >= 12.0 * scale, "{layout:?}");
                        let seconds = layout.seconds.unwrap();
                        assert!(seconds.x + seconds.width <= 136.0 * scale, "{layout:?}");
                        assert!(layout.time.y >= 8.0 * scale, "{layout:?}");
                        assert!(
                            layout.time.y + layout.time.height <= 32.0 * scale,
                            "{layout:?}"
                        );
                        let date = layer.date.as_ref().unwrap();
                        assert!(date.width as f32 <= (EXPANDED.width - 24.0) * scale);
                    }
                }
            }
        }
    }

    #[test]
    fn bundled_date_font_contains_all_vietnamese_clock_accents() {
        use windows_sys::Win32::Graphics::Gdi::{
            GGI_MARK_NONEXISTING_GLYPHS, GetGlyphIndicesW, SelectObject,
        };
        let layer = layer(1.0);
        let text: Vec<u16> = "CHỦ NHẬT THỨ HAI BA TƯ NĂM SÁU BẢY THÁNG"
            .encode_utf16()
            .collect();
        let mut glyphs = vec![0_u16; text.len()];
        unsafe {
            let previous = SelectObject(layer.dc, layer.fonts.date);
            let count = GetGlyphIndicesW(
                layer.dc,
                text.as_ptr(),
                text.len() as i32,
                glyphs.as_mut_ptr(),
                GGI_MARK_NONEXISTING_GLYPHS,
            );
            SelectObject(layer.dc, previous);
            assert_ne!(count, u32::MAX);
        }
        assert!(
            !glyphs.contains(&u16::MAX),
            "clock accents must have glyphs in the bundled font"
        );
    }

    fn layer(scale: f32) -> TextLayer {
        TextLayer::new(scale).expect("text layer should initialize")
    }

    fn params(
        size: (u32, u32),
        state: crate::animation::ShapeState,
        show_seconds: bool,
    ) -> TextParams {
        TextParams {
            size,
            state,
            top_margin: 0.0,
            scale: 1.0,
            accent: ICE,
            show_seconds,
            hover: 0.0,
            compact_trailing_space: 0.0,
        }
    }

    fn mask_touches_all_edges(mask: &Mask) -> bool {
        let width = mask.width as usize;
        let height = mask.height as usize;
        (0..width).any(|x| mask.coverage[x] > 0)
            && (0..width).any(|x| mask.coverage[(height - 1) * width + x] > 0)
            && (0..height).any(|y| mask.coverage[y * width] > 0)
            && (0..height).any(|y| mask.coverage[y * width + width - 1] > 0)
    }

    #[test]
    fn compact_clock_with_seconds_fits_the_small_capsule_at_each_display_scale() {
        for scale in [1.0, 1.5, 2.0] {
            let mut layer = layer(scale);
            for format in [ClockFormat::TwentyFourHour, ClockFormat::TwelveHour] {
                for hour in [0, 12, 20, 23] {
                    layer.update(&ClockSnapshot { hour, ..snapshot() }, format);
                    let time = layer.compact.as_ref().unwrap();
                    let seconds = layer.compact_seconds.as_ref().unwrap();
                    let layout = compact_layout(
                        148.0 * scale,
                        20.0 * scale,
                        (time.width as f32, time.height as f32),
                        Some((seconds.advance as f32, seconds.height as f32)),
                        scale,
                    );
                    assert!(
                        layout.dot.x >= 12.0 * scale,
                        "{format:?} at {scale}: {layout:?}"
                    );
                    let seconds = layout.seconds.unwrap();
                    assert!(seconds.x + seconds.width <= 136.0 * scale);
                    assert!(layout.time.y >= 8.0 * scale);
                    assert!(layout.time.y + layout.time.height <= 32.0 * scale);
                }
            }
        }
    }

    #[test]
    fn bundled_inter_fonts_are_selected() {
        let layer = layer(1.0);
        // SAFETY: dc is live and the fonts are owned by the layer.
        unsafe {
            for font in [layer.fonts.compact_time, layer.fonts.expanded_time] {
                let previous = windows_sys::Win32::Graphics::Gdi::SelectObject(layer.dc, font);
                let face = super::selected_face(layer.dc);
                windows_sys::Win32::Graphics::Gdi::SelectObject(layer.dc, previous);
                assert!(
                    face.to_ascii_lowercase().contains("inter"),
                    "expected bundled Inter, got {face}"
                );
            }
        }
    }

    #[test]
    fn renders_compact_and_expanded_clock_masks() {
        let mut layer = layer(1.0);
        layer.update(&snapshot(), ClockFormat::TwentyFourHour);

        let compact = layer.compact.as_ref().expect("compact time should render");
        let expanded = layer
            .expanded
            .as_ref()
            .expect("expanded time should render");
        assert!(compact.width > 10 && compact.height > 5);
        assert!(expanded.width > compact.width);
        assert!(mask_touches_all_edges(compact));
        assert!(mask_touches_all_edges(expanded));
        assert!(layer.compact_seconds.is_some());
        assert!(layer.expanded_seconds.is_some());
        assert!(layer.date.is_some());
        assert!(mask_touches_all_edges(
            layer.compact_seconds.as_ref().unwrap()
        ));
    }

    #[test]
    fn changing_only_seconds_changes_the_rendered_clock() {
        let size = (512, 224);
        let mut layer = layer(1.0);
        for state in [COMPACT, EXPANDED] {
            let mut first = vec![0_u8; size.0 as usize * size.1 as usize * 4];
            let mut next = first.clone();
            layer.update(&snapshot_with_second(11), ClockFormat::TwentyFourHour);
            layer.draw(&mut first, params(size, state, true));
            layer.update(&snapshot_with_second(58), ClockFormat::TwentyFourHour);
            layer.draw(&mut next, params(size, state, true));

            assert!(first != next, "a second rollover should refresh the clock");
            let bright_glyph = |pixel: &[u8; 4]| pixel[..3].iter().all(|&channel| channel > 230);
            let (first, _) = first.as_chunks::<4>();
            let (next, _) = next.as_chunks::<4>();
            assert!(first.iter().any(bright_glyph));
            assert!(
                first
                    .iter()
                    .zip(next)
                    .all(|(a, b)| { bright_glyph(a) == bright_glyph(b) }),
                "the main time must not move when the seconds change"
            );
        }
    }

    #[test]
    fn text_masks_scale_with_the_display_scale() {
        let mut regular = layer(1.0);
        let mut scaled = layer(1.5);
        let snapshot = snapshot();
        regular.update(&snapshot, ClockFormat::TwentyFourHour);
        scaled.update(&snapshot, ClockFormat::TwentyFourHour);
        let regular_time = regular.expanded.as_ref().unwrap();
        let scaled_time = scaled.expanded.as_ref().unwrap();
        let regular_seconds = regular.expanded_seconds.as_ref().unwrap();
        let scaled_seconds = scaled.expanded_seconds.as_ref().unwrap();

        assert!(scaled_time.width as f32 >= regular_time.width as f32 * 1.35);
        assert!(scaled_time.height as f32 >= regular_time.height as f32 * 1.35);
        assert!(scaled_seconds.width as f32 >= regular_seconds.width as f32 * 1.35);
        assert!(scaled_seconds.height as f32 >= regular_seconds.height as f32 * 1.35);
    }

    #[test]
    fn text_masks_scale_at_200_percent_dpi() {
        let mut regular = layer(1.0);
        let mut doubled = layer(2.0);
        let snapshot = snapshot();
        regular.update(&snapshot, ClockFormat::TwentyFourHour);
        doubled.update(&snapshot, ClockFormat::TwentyFourHour);

        let regular_time = regular.expanded.as_ref().unwrap();
        let doubled_time = doubled.expanded.as_ref().unwrap();
        assert!(doubled_time.width as f32 >= regular_time.width as f32 * 1.8);
        assert!(doubled_time.height as f32 >= regular_time.height as f32 * 1.8);
    }

    #[test]
    fn scratch_starts_at_design_size_and_grows() {
        let mut layer = layer(1.0);
        let initial = layer.scratch_size();
        assert!(initial.0 >= 280 * 3);
        assert!(initial.1 >= 90 * 3);
        assert!(layer.ensure_scratch((initial.0 + 100, initial.1 + 100)));
        let grown = layer.scratch_size();
        assert!(grown.0 > initial.0);
        assert!(grown.1 > initial.1);
    }

    #[test]
    fn compact_layout_centers_seconds_on_the_time_ink() {
        let layout = compact_layout(512.0, 100.0, (80.0, 24.0), Some((20.0, 14.0)), 1.0);
        let time = layout.time;
        let seconds = layout.seconds.expect("seconds should be present");
        assert_eq!(seconds.center_y(), time.center_y());
        assert!(layout.dot.x < time.x);
        assert!(seconds.x >= time.x + time.width);
        let left = layout.dot.x;
        let right = seconds.x + seconds.width;
        assert!(((left + right) * 0.5 - 256.0).abs() < 0.01);
    }

    #[test]
    fn compact_layout_drops_seconds_when_hidden() {
        let layout = compact_layout(512.0, 100.0, (80.0, 24.0), None, 1.0);
        assert!(layout.seconds.is_none());
        let right = layout.time.x + layout.time.width;
        let left = layout.dot.x;
        assert!(((left + right) * 0.5 - 256.0).abs() < 0.01);
    }

    #[test]
    fn expanded_layout_keeps_seconds_and_divider_clear() {
        let layout = expanded_layout(
            512.0,
            0.0,
            150.0,
            Some((195.0, 13.0)),
            (300.0, 50.0),
            Some((20.0, 15.0)),
            1.0,
        );
        let time = layout.time;
        let seconds = layout.seconds.expect("seconds should be present");
        assert!(seconds.x >= time.x + time.width);
        assert!(layout.divider.y >= time.y + time.height);
        let date = layout.date.expect("date should be present");
        let top = date.y;
        let bottom = layout.divider.y + layout.divider.height;
        assert!(((top + bottom) * 0.5 - 75.0).abs() < 0.01);
    }

    #[test]
    fn twenty_four_and_twelve_hour_rows_do_not_overlap() {
        let mut layer = layer(1.0);
        for format in [ClockFormat::TwentyFourHour, ClockFormat::TwelveHour] {
            layer.update(&snapshot(), format);
            let time = layer.expanded.as_ref().unwrap();
            let seconds = layer.expanded_seconds.as_ref().unwrap();
            let layout = expanded_layout(
                512.0,
                0.0,
                150.0,
                None,
                (time.width as f32, time.height as f32),
                Some((seconds.advance as f32, seconds.height as f32)),
                1.0,
            );
            assert!(
                layout.seconds.unwrap().x >= layout.time.x + layout.time.width,
                "seconds must clear the time mask in {format:?}"
            );
        }
    }

    #[test]
    fn twelve_hour_mode_adds_a_meridiem_suffix() {
        let mut layer = layer(1.0);
        layer.update(&snapshot(), ClockFormat::TwelveHour);
        let twelve_hour_width = layer.compact.as_ref().unwrap().width;

        layer.update(&snapshot(), ClockFormat::TwentyFourHour);
        let twenty_four_hour_width = layer.compact.as_ref().unwrap().width;

        assert!(twelve_hour_width > twenty_four_hour_width);
    }

    #[test]
    fn mask_blending_clips_to_the_rounded_shape() {
        // A full-white mask fixture isolates geometric clipping from application colors.
        let size = (20, 20);
        let mut pixels = vec![0_u8; size.0 as usize * size.1 as usize * 4];
        let mask = Mask {
            width: 10,
            height: 10,
            advance: 10,
            coverage: vec![255; 100],
        };
        let shape = crate::animation::ShapeState {
            width: 10.0,
            height: 10.0,
            radius: 5.0,
        };

        blend_mask(
            &mut pixels,
            size,
            &mask,
            5.0,
            5.0,
            1.0,
            [255, 255, 255],
            shape,
            (10.0, 10.0),
        );

        let corner = ((5 * size.0 + 5) * 4) as usize;
        let center = ((10 * size.0 + 10) * 4) as usize;
        assert_eq!(
            pixels[corner + 3],
            0,
            "rounded corner must remain transparent"
        );
        assert!(
            pixels[center + 3] > 0,
            "the mask interior must still be drawn"
        );
    }

    #[test]
    fn clock_uses_shared_primary_secondary_and_date_colors() {
        let mut layer = layer(1.0);
        layer.update(&snapshot(), ClockFormat::TwentyFourHour);
        let size = (512, 224);
        let mut pixels = vec![0; (size.0 * size.1 * 4) as usize];
        layer.draw(&mut pixels, params(size, EXPANDED, true));
        for color in [
            crate::theme::TEXT,
            crate::theme::MUTED,
            crate::theme::mix(crate::theme::DIM, crate::theme::MUTED, 0.75),
        ] {
            assert!(
                pixels
                    .as_chunks::<4>()
                    .0
                    .iter()
                    .any(|pixel| { pixel[..3] == color && pixel[3] == 255 }),
                "the clock must render its shared text role: {color:?}"
            );
        }
    }

    #[test]
    fn draws_bright_clock_pixels_inside_the_expanded_panel() {
        let size = (512, 224);
        let mut pixels = vec![0_u8; size.0 as usize * size.1 as usize * 4];
        for pixel in pixels.chunks_mut(4) {
            // A fixed opaque gray fixture makes blended clock ink easy to distinguish.
            pixel.copy_from_slice(&[20, 20, 20, 255]);
        }
        let mut layer = layer(1.0);
        layer.update(&snapshot(), ClockFormat::TwentyFourHour);

        layer.draw(&mut pixels, params(size, EXPANDED, true));

        let bright = pixels.chunks(4).filter(|pixel| pixel[0] > 200).count();
        assert!(bright > 100, "expected clock pixels, got {bright}");
    }

    #[test]
    fn draws_the_compact_clock_while_collapsed() {
        let size = (512, 224);
        let mut pixels = vec![0_u8; size.0 as usize * size.1 as usize * 4];
        for pixel in pixels.chunks_mut(4) {
            // A fixed opaque gray fixture makes blended clock ink easy to distinguish.
            pixel.copy_from_slice(&[20, 20, 20, 255]);
        }
        let mut layer = layer(1.0);
        layer.update(&snapshot(), ClockFormat::TwentyFourHour);

        layer.draw(&mut pixels, params(size, COMPACT, true));

        let bright = pixels.chunks(4).filter(|pixel| pixel[0] > 200).count();
        assert!(bright > 50, "expected compact clock pixels, got {bright}");
    }

    #[test]
    fn clock_page_opacity_fades_all_content_without_moving_the_layout() {
        let size = (512, 224);
        let mut layer = layer(1.0);
        layer.update(&snapshot(), ClockFormat::TwentyFourHour);
        for state in [COMPACT, EXPANDED] {
            let mut normal = vec![0; size.0 as usize * size.1 as usize * 4];
            layer.draw(&mut normal, params(size, state, true));
            for opacity in [0.0, 0.5, 1.0] {
                let mut faded = vec![0; normal.len()];
                layer.draw_with_opacity(&mut faded, params(size, state, true), opacity);
                for (original, actual) in normal
                    .as_chunks::<4>()
                    .0
                    .iter()
                    .zip(faded.as_chunks::<4>().0)
                {
                    for channel in 0..4 {
                        let expected = (original[channel] as f32 * opacity).round() as u8;
                        assert!(
                            actual[channel].abs_diff(expected) <= u8::from(opacity == 0.5),
                            "state {state:?}, opacity {opacity}: expected {expected}, got {}",
                            actual[channel]
                        );
                    }
                }
            }
        }
    }

    #[test]
    fn hidden_seconds_leave_the_compact_clock_centered() {
        let size = (512, 224);
        let mut layer = layer(1.0);
        layer.update(&snapshot(), ClockFormat::TwentyFourHour);
        let time = layer.compact.as_ref().unwrap();
        let seconds = layer.compact_seconds.as_ref().unwrap();

        let with_seconds = compact_layout(
            size.0 as f32,
            100.0,
            (time.width as f32, time.height as f32),
            Some((seconds.advance as f32, seconds.height as f32)),
            1.0,
        );
        let without_seconds = compact_layout(
            size.0 as f32,
            100.0,
            (time.width as f32, time.height as f32),
            None,
            1.0,
        );

        assert!(with_seconds.seconds.unwrap().x > with_seconds.time.x);
        assert_eq!(without_seconds.seconds, None);
    }
}
