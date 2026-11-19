use std::mem::size_of;
use std::ptr::null_mut;

use windows_sys::Win32::Foundation::SIZE;
use windows_sys::Win32::Graphics::Gdi::{
    ANTIALIASED_QUALITY, BI_RGB, BITMAPINFO, BITMAPINFOHEADER, CLIP_DEFAULT_PRECIS,
    CreateCompatibleDC, CreateDIBSection, CreateFontW, DEFAULT_CHARSET, DEFAULT_PITCH,
    DIB_RGB_COLORS, DeleteDC, DeleteObject, FF_DONTCARE, FW_SEMIBOLD, GdiFlush,
    GetTextExtentPoint32W, GetTextMetricsW, HBITMAP, HDC, HFONT, HGDIOBJ, OUT_TT_PRECIS,
    SelectObject, SetBkMode, SetTextColor, TEXTMETRICW, TRANSPARENT, TextOutW,
};

use crate::animation::ShapeState;
use crate::clock::{ClockFormat, ClockSnapshot};
use crate::geometry::rounded_rect_distance;
use crate::render::blend_pixel;

const COMPACT_TIME_PX: f32 = 26.0;
const EXPANDED_TIME_PX: f32 = 48.0;
const COMPACT_SUFFIX_PX: f32 = 11.0;
const EXPANDED_SUFFIX_PX: f32 = 14.0;
const DATE_PX: f32 = 15.0;
const COMPACT_SECONDS_PX: f32 = 11.0;
const EXPANDED_SECONDS_PX: f32 = 14.0;
const DIGIT_GAP_PX: f32 = 2.0;
const MERIDIEM_GAP_PX: f32 = 6.0;
const DATE_GAP_PX: f32 = 7.0;
const SECONDS_GAP_PX: f32 = 4.0;
const ACCENT_COLOR: [u8; 3] = [228, 177, 122];
const SCRATCH_ORIGIN: i32 = 8;
const TEXT_COLOR: [u8; 3] = [255, 255, 255];
const DATE_COLOR: [u8; 3] = [200, 194, 188];

struct Mask {
    width: u32,
    height: u32,
    coverage: Vec<u8>,
}

pub(crate) struct TextLayer {
    dc: HDC,
    bitmap: HBITMAP,
    previous_bitmap: HGDIOBJ,
    previous_font: HGDIOBJ,
    bits: *mut u8,
    size: (u32, u32),
    compact_font: HFONT,
    compact_suffix_font: HFONT,
    expanded_font: HFONT,
    expanded_suffix_font: HFONT,
    compact_seconds_font: HFONT,
    expanded_seconds_font: HFONT,
    date_font: HFONT,
    key: Option<(ClockSnapshot, ClockFormat)>,
    compact: Option<Mask>,
    expanded: Option<Mask>,
    compact_seconds: Option<Mask>,
    expanded_seconds: Option<Mask>,
    date: Option<Mask>,
}

impl TextLayer {
    pub(crate) fn new(size: (u32, u32), scale: f32) -> Result<Self, String> {
        // SAFETY: This memory DC and its DIB are owned by the returned TextLayer.
        unsafe {
            let dc = CreateCompatibleDC(null_mut());
            if dc.is_null() {
                return Err("CreateCompatibleDC failed for text".to_string());
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
                if !bitmap.is_null() {
                    DeleteObject(bitmap);
                }
                DeleteDC(dc);
                return Err("CreateDIBSection failed for text".to_string());
            }
            let previous_bitmap = SelectObject(dc, bitmap);
            if previous_bitmap.is_null() || previous_bitmap as isize == -1 {
                DeleteObject(bitmap);
                DeleteDC(dc);
                return Err("SelectObject failed for text".to_string());
            }

            SetBkMode(dc, TRANSPARENT as i32);
            SetTextColor(dc, 0x00FF_FFFF);

            let face = wide("Segoe UI Variable Display");
            let text_face = wide("Segoe UI Variable Text");
            let mut fonts = Vec::with_capacity(7);
            let specs: [(f32, i32, &[u16]); 7] = [
                (COMPACT_TIME_PX, FW_SEMIBOLD as i32, face.as_slice()),
                (COMPACT_SUFFIX_PX, FW_SEMIBOLD as i32, text_face.as_slice()),
                (EXPANDED_TIME_PX, FW_SEMIBOLD as i32, face.as_slice()),
                (EXPANDED_SUFFIX_PX, FW_SEMIBOLD as i32, text_face.as_slice()),
                (COMPACT_SECONDS_PX, 400, text_face.as_slice()),
                (EXPANDED_SECONDS_PX, 400, text_face.as_slice()),
                (DATE_PX, 400, text_face.as_slice()),
            ];
            for (design_px, weight, face) in specs {
                let font = create_font(design_px * scale, weight, face);
                if font.is_null() {
                    for font in fonts {
                        DeleteObject(font);
                    }
                    SelectObject(dc, previous_bitmap);
                    DeleteObject(bitmap);
                    DeleteDC(dc);
                    return Err("CreateFontW failed for text".to_string());
                }
                fonts.push(font);
            }
            let previous_font = SelectObject(dc, fonts[0]);

            Ok(Self {
                dc,
                bitmap,
                previous_bitmap,
                previous_font,
                bits: bits.cast(),
                size,
                compact_font: fonts[0],
                compact_suffix_font: fonts[1],
                expanded_font: fonts[2],
                expanded_suffix_font: fonts[3],
                compact_seconds_font: fonts[4],
                expanded_seconds_font: fonts[5],
                date_font: fonts[6],
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
        if self.key.is_none_or(|(previous, old_format)| {
            (previous.hour, previous.minute, old_format) != (clock.hour, clock.minute, format)
        }) {
            let time = clock.time_text(format);
            let meridiem = clock.meridiem(format);
            self.clear();
            self.compact =
                self.draw_time(&time, meridiem, self.compact_font, self.compact_suffix_font);
            self.clear();
            self.expanded = self.draw_time(
                &time,
                meridiem,
                self.expanded_font,
                self.expanded_suffix_font,
            );
        }
        if self
            .key
            .is_none_or(|(previous, _)| previous.second != clock.second)
        {
            let seconds = clock.seconds_text();
            self.clear();
            self.compact_seconds = self.draw_seconds(&seconds, self.compact_seconds_font);
            self.clear();
            self.expanded_seconds = self.draw_seconds(&seconds, self.expanded_seconds_font);
        }
        if self.key.is_none_or(|(previous, _)| {
            (previous.weekday, previous.month, previous.day)
                != (clock.weekday, clock.month, clock.day)
        }) {
            self.clear();
            self.date = self.draw_date(&clock.date_text().to_uppercase());
        }
        self.key = Some((*clock, format));
    }

    pub(crate) fn draw(
        &self,
        pixels: &mut [u8],
        size: (u32, u32),
        state: ShapeState,
        top_margin: f32,
        scale: f32,
    ) {
        let content = state.content_opacity.clamp(0.0, 1.0);
        let top = top_margin;
        let center = (size.0 as f32 * 0.5, top + state.height * 0.5);

        if content < 1.0
            && let Some(mask) = &self.compact
        {
            let seconds = self.compact_seconds.as_ref();
            let seconds_gap = seconds.map_or(0.0, |_| SECONDS_GAP_PX * scale);
            let indicator = 4.0 * scale;
            let indicator_gap = 7.0 * scale;
            let seconds_width = seconds.map_or(0.0, |value| value.width as f32);
            let group_width =
                indicator + indicator_gap + mask.width as f32 + seconds_gap + seconds_width;
            let start = size.0 as f32 * 0.5 - group_width * 0.5;
            let x = start + indicator + indicator_gap;
            let y = top + state.height * 0.5 - mask.height as f32 * 0.5;
            blend_mask(
                pixels,
                size,
                mask,
                x,
                y,
                1.0 - content,
                TEXT_COLOR,
                state,
                center,
            );
            fill_circle(
                pixels,
                size,
                (start + indicator * 0.5, top + state.height * 0.5),
                indicator * 0.5,
                1.0 - content,
                ACCENT_COLOR,
                state,
                center,
            );
            if let Some(seconds) = seconds {
                let seconds_y = top + state.height * 0.5 - seconds.height as f32 * 0.5;
                blend_mask(
                    pixels,
                    size,
                    seconds,
                    x + mask.width as f32 + seconds_gap,
                    seconds_y,
                    1.0 - content,
                    DATE_COLOR,
                    state,
                    center,
                );
            }
        }

        if content > 0.0
            && let Some(mask) = &self.expanded
        {
            let date = self.date.as_ref();
            let seconds = self.expanded_seconds.as_ref();
            let date_gap = DATE_GAP_PX * scale;
            let seconds_gap = SECONDS_GAP_PX * scale;
            let date_height = date.map_or(0.0, |label| label.height as f32 + date_gap);
            let seconds_height = seconds.map_or(0.0, |label| label.height as f32 + seconds_gap);
            let divider_gap = 5.0 * scale;
            let divider_height = 1.0 * scale;
            let block_height =
                date_height + mask.height as f32 + divider_gap + divider_height + seconds_height;
            let mut y = top + (state.height - block_height).max(0.0) * 0.5;
            if let Some(date) = date {
                let x = size.0 as f32 * 0.5 - date.width as f32 * 0.5;
                blend_mask(pixels, size, date, x, y, content, DATE_COLOR, state, center);
                y += date.height as f32 + date_gap;
            }
            let x = size.0 as f32 * 0.5 - mask.width as f32 * 0.5;
            blend_mask(pixels, size, mask, x, y, content, TEXT_COLOR, state, center);
            y += mask.height as f32 + divider_gap;
            fill_rect(
                pixels,
                size,
                (size.0 as f32 * 0.5 - 15.0 * scale, y),
                (30.0 * scale, divider_height),
                content,
                ACCENT_COLOR,
                state,
                center,
            );
            if let Some(seconds) = seconds {
                let x = size.0 as f32 * 0.5 - seconds.width as f32 * 0.5;
                y += divider_height + seconds_gap;
                blend_mask(
                    pixels, size, seconds, x, y, content, DATE_COLOR, state, center,
                );
            }
        }
    }

    fn clear(&mut self) {
        let length = self.size.0 as usize * self.size.1 as usize * 4;
        // SAFETY: The DIB allocated exactly this many bytes and is owned by self.
        unsafe { std::ptr::write_bytes(self.bits, 0, length) };
    }

    fn draw_time(
        &mut self,
        time: &str,
        meridiem: Option<&'static str>,
        time_font: HFONT,
        suffix_font: HFONT,
    ) -> Option<Mask> {
        // SAFETY: dc is a live memory DC and every string pointer is valid for the call.
        unsafe {
            SelectObject(self.dc, time_font);
            let metrics = text_metrics(self.dc);
            let digit_width = ('0'..='9')
                .map(|digit| text_extent(self.dc, &digit.to_string()).0)
                .max()
                .unwrap_or(1);
            let colon_width = text_extent(self.dc, ":").0;

            let mut x = SCRATCH_ORIGIN;
            for character in time.chars() {
                let glyph = character.to_string();
                let (glyph_width, _) = text_extent(self.dc, &glyph);
                let cell = if character == ':' {
                    (colon_width as f32 * 1.2).round() as i32
                } else {
                    digit_width + DIGIT_GAP_PX as i32
                };
                draw_text(
                    self.dc,
                    &glyph,
                    x + (cell - glyph_width) / 2,
                    SCRATCH_ORIGIN,
                );
                x += cell.max(glyph_width);
            }

            if let Some(meridiem) = meridiem {
                SelectObject(self.dc, suffix_font);
                let suffix_metrics = text_metrics(self.dc);
                x += MERIDIEM_GAP_PX as i32;
                let baseline = SCRATCH_ORIGIN + metrics.tmAscent;
                let suffix_y = baseline - suffix_metrics.tmAscent;
                draw_text(self.dc, meridiem, x, suffix_y);
            }
        }
        self.extract_mask()
    }

    fn draw_date(&mut self, date: &str) -> Option<Mask> {
        // SAFETY: dc is live and the text pointer is valid for the duration of TextOutW.
        unsafe {
            SelectObject(self.dc, self.date_font);
            draw_text(self.dc, date, SCRATCH_ORIGIN, SCRATCH_ORIGIN);
        }
        self.extract_mask()
    }

    fn draw_seconds(&self, seconds: &str, font: HFONT) -> Option<Mask> {
        // Use fixed-width digit cells and keep the font's full height in the mask.
        unsafe { SelectObject(self.dc, font) };
        let cell = ('0'..='9')
            .map(|digit| text_extent(self.dc, &digit.to_string()).0)
            .max()?;
        let height = text_metrics(self.dc).tmHeight;
        for (index, digit) in seconds.chars().enumerate() {
            let glyph = digit.to_string();
            let width = text_extent(self.dc, &glyph).0;
            draw_text(
                self.dc,
                &glyph,
                SCRATCH_ORIGIN + index as i32 * cell + (cell - width) / 2,
                SCRATCH_ORIGIN,
            );
        }
        let start = SCRATCH_ORIGIN as usize;
        self.read_mask(start, start, (2 * cell) as usize, height as usize)
    }

    fn extract_mask(&self) -> Option<Mask> {
        let width = self.size.0 as usize;
        let height = self.size.1 as usize;
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

        self.read_mask(min_x, min_y, max_x - min_x + 1, max_y - min_y + 1)
    }

    fn read_mask(&self, left: usize, top: usize, width: usize, height: usize) -> Option<Mask> {
        let stride = self.size.0 as usize;
        if width == 0
            || height == 0
            || width > stride.saturating_sub(left)
            || height > (self.size.1 as usize).saturating_sub(top)
        {
            return None;
        }
        // SAFETY: Flush queued writes; the DIB contains stride * height_of_surface * 4 bytes.
        unsafe { GdiFlush() };
        let pixels =
            unsafe { std::slice::from_raw_parts(self.bits, stride * self.size.1 as usize * 4) };
        let mut coverage = Vec::with_capacity(width * height);
        for y in top..top + height {
            for x in left..left + width {
                let index = (y * stride + x) * 4;
                coverage.push(pixels[index].max(pixels[index + 1]).max(pixels[index + 2]));
            }
        }

        Some(Mask {
            width: width as u32,
            height: height as u32,
            coverage,
        })
    }
}

impl Drop for TextLayer {
    fn drop(&mut self) {
        // SAFETY: Restore the original GDI objects before deleting owned fonts, the
        // bitmap, and the memory DC.
        unsafe {
            SelectObject(self.dc, self.previous_font);
            for font in [
                self.compact_font,
                self.compact_suffix_font,
                self.expanded_font,
                self.expanded_suffix_font,
                self.compact_seconds_font,
                self.expanded_seconds_font,
                self.date_font,
            ] {
                DeleteObject(font);
            }
            SelectObject(self.dc, self.previous_bitmap);
            DeleteObject(self.bitmap);
            DeleteDC(self.dc);
        }
    }
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
    if opacity <= 0.0 || mask.width == 0 || mask.height == 0 {
        return;
    }
    let canvas_width = size.0 as i32;
    let canvas_height = size.1 as i32;
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
            let distance = rounded_rect_distance(px as f32 + 0.5, py as f32 + 0.5, state, center);
            let alpha = (coverage as f32 / 255.0) * opacity * (0.5 - distance).clamp(0.0, 1.0);
            if alpha > 0.0 {
                blend_pixel_at(pixels, size, px, py, color_bgra, alpha);
            }
        }
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
    use super::{Mask, TextLayer, blend_mask};
    use crate::animation::{COMPACT, EXPANDED, ShapeState};
    use crate::clock::{ClockFormat, ClockSnapshot};

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
    fn renders_compact_and_expanded_clock_masks() {
        let mut layer = TextLayer::new((512, 224), 1.0).expect("text layer should initialize");
        layer.update(&snapshot(), ClockFormat::TwentyFourHour);

        let compact = layer.compact.as_ref().expect("compact time should render");
        let expanded = layer
            .expanded
            .as_ref()
            .expect("expanded time should render");
        assert!(compact.width > 10 && compact.height > 5);
        assert!(expanded.width > compact.width);
        assert!(layer.compact_seconds.is_some());
        assert!(layer.expanded_seconds.is_some());
        assert!(layer.date.is_some());
    }

    #[test]
    fn changing_only_seconds_changes_the_rendered_clock() {
        let size = (512, 224);
        let mut layer = TextLayer::new(size, 1.0).expect("text layer should initialize");
        for state in [COMPACT, EXPANDED] {
            let mut first = vec![0_u8; size.0 as usize * size.1 as usize * 4];
            let mut next = first.clone();
            layer.update(&snapshot_with_second(11), ClockFormat::TwentyFourHour);
            layer.draw(&mut first, size, state, 0.0, 1.0);
            layer.update(&snapshot_with_second(58), ClockFormat::TwentyFourHour);
            layer.draw(&mut next, size, state, 0.0, 1.0);

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
        let regular = TextLayer::new((512, 224), 1.0).expect("text layer should initialize");
        let scaled = TextLayer::new((768, 336), 1.5).expect("scaled text layer should initialize");
        let snapshot = snapshot();
        let mut regular = regular;
        let mut scaled = scaled;
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
    fn mask_blending_clips_to_the_rounded_shape() {
        let size = (20, 20);
        let mut pixels = vec![0_u8; size.0 as usize * size.1 as usize * 4];
        let mask = Mask {
            width: 10,
            height: 10,
            coverage: vec![255; 100],
        };
        let shape = ShapeState {
            width: 10.0,
            height: 10.0,
            radius: 5.0,
            content_opacity: 1.0,
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
    fn draws_bright_clock_pixels_inside_the_expanded_panel() {
        let size = (512, 224);
        let mut pixels = vec![0_u8; size.0 as usize * size.1 as usize * 4];
        for pixel in pixels.chunks_mut(4) {
            pixel.copy_from_slice(&[20, 20, 20, 255]);
        }
        let mut layer = TextLayer::new(size, 1.0).expect("text layer should initialize");
        layer.update(&snapshot(), ClockFormat::TwentyFourHour);

        layer.draw(&mut pixels, size, EXPANDED, 0.0, 1.0);

        let bright = pixels.chunks(4).filter(|pixel| pixel[0] > 200).count();
        assert!(bright > 100, "expected clock pixels, got {bright}");
    }

    #[test]
    fn draws_the_compact_clock_while_collapsed() {
        let size = (512, 224);
        let mut pixels = vec![0_u8; size.0 as usize * size.1 as usize * 4];
        for pixel in pixels.chunks_mut(4) {
            pixel.copy_from_slice(&[20, 20, 20, 255]);
        }
        let mut layer = TextLayer::new(size, 1.0).expect("text layer should initialize");
        layer.update(&snapshot(), ClockFormat::TwentyFourHour);

        layer.draw(&mut pixels, size, COMPACT, 0.0, 1.0);

        let bright = pixels.chunks(4).filter(|pixel| pixel[0] > 200).count();
        assert!(bright > 50, "expected compact clock pixels, got {bright}");
    }

    #[test]
    fn twelve_hour_mode_adds_a_meridiem_suffix() {
        let mut layer = TextLayer::new((512, 224), 1.0).expect("text layer should initialize");
        layer.update(&snapshot(), ClockFormat::TwelveHour);
        let twelve_hour_width = layer.compact.as_ref().unwrap().width;

        layer.update(&snapshot(), ClockFormat::TwentyFourHour);
        let twenty_four_hour_width = layer.compact.as_ref().unwrap().width;

        assert!(twelve_hour_width > 0 && twenty_four_hour_width > 0);
    }
}
