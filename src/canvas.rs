use crate::render::blend_pixel;
use crate::text::{Mask, TextLayer};
use std::collections::HashMap;

pub(crate) const WHITE: [u8; 3] = [248, 248, 248];
pub(crate) const MUTED: [u8; 3] = [164, 164, 168];
pub(crate) const DIM: [u8; 3] = [112, 112, 118];

#[derive(Clone, Copy, Debug, Default)]
pub(crate) struct Rect {
    pub x: f32,
    pub y: f32,
    pub w: f32,
    pub h: f32,
}

impl Rect {
    pub fn intersect(self, other: Self) -> Self {
        let x = self.x.max(other.x);
        let y = self.y.max(other.y);
        Self::new(
            x,
            y,
            ((self.x + self.w).min(other.x + other.w) - x).max(0.0),
            ((self.y + self.h).min(other.y + other.h) - y).max(0.0),
        )
    }
    pub fn new(x: f32, y: f32, w: f32, h: f32) -> Self {
        Self { x, y, w, h }
    }
    pub fn contains(self, x: f32, y: f32) -> bool {
        x >= self.x && y >= self.y && x < self.x + self.w && y < self.y + self.h
    }
}

pub(crate) struct Painter {
    text: TextLayer,
    masks: HashMap<(String, u32, bool), Mask>,
    widths: HashMap<(String, u32, bool), f32>,
    artwork_columns: Vec<(usize, f32)>,
    pub scale: f32,
}
impl Painter {
    pub fn new(scale: f32) -> Result<Self, String> {
        Ok(Self {
            text: TextLayer::new(scale)?,
            masks: HashMap::new(),
            widths: HashMap::new(),
            artwork_columns: Vec::new(),
            scale,
        })
    }
    fn mask(&mut self, text: &str, size: f32, bold: bool) -> Option<&Mask> {
        let key = (text.to_owned(), size.to_bits(), bold);
        if !self.masks.contains_key(&key) {
            if self.masks.len() > 800 {
                self.masks.clear();
            }
            self.masks
                .insert(key.clone(), self.text.label_mask(text, size, bold)?);
        }
        self.masks.get(&key)
    }
    pub fn width(&mut self, text: &str, size: f32, bold: bool) -> f32 {
        let key = (text.to_owned(), size.to_bits(), bold);
        if let Some(width) = self.widths.get(&key) {
            return *width;
        }
        let width = self.text.label_advance(text, size, bold);
        if self.widths.len() > 4000 {
            self.widths.clear();
        }
        self.widths.insert(key, width);
        width
    }
}

pub(crate) struct Canvas<'a> {
    pub pixels: &'a mut [u8],
    pub size: (u32, u32),
    pub painter: &'a mut Painter,
    pub clip: Rect,
    pub opacity: f32,
}
impl<'a> Canvas<'a> {
    pub fn new(pixels: &'a mut [u8], size: (u32, u32), painter: &'a mut Painter) -> Self {
        let clip = Rect::new(
            0.0,
            0.0,
            size.0 as f32 / painter.scale,
            size.1 as f32 / painter.scale,
        );
        Self {
            pixels,
            size,
            painter,
            clip,
            opacity: 1.0,
        }
    }
    pub fn rect(&mut self, rect: Rect, radius: f32, color: [u8; 3], alpha: f32) {
        if alpha * self.opacity <= 0.0 {
            return;
        }
        let s = self.painter.scale;
        let left = (rect.x.max(self.clip.x) * s).floor().max(0.0) as u32;
        let top = (rect.y.max(self.clip.y) * s).floor().max(0.0) as u32;
        let right = ((rect.x + rect.w).min(self.clip.x + self.clip.w) * s)
            .ceil()
            .min(self.size.0 as f32)
            .max(0.0) as u32;
        let bottom = ((rect.y + rect.h).min(self.clip.y + self.clip.h) * s)
            .ceil()
            .min(self.size.1 as f32)
            .max(0.0) as u32;
        let fill_alpha = alpha * self.opacity;
        // Large translucent fills reuse the same blend for thousands of pixels.
        // Preserve the scalar rounding exactly, but compute it once per byte value.
        let lookup: Option<[[u8; 256]; 4]> =
            ((right.saturating_sub(left) as usize) * (bottom.saturating_sub(top) as usize) > 4096
                && fill_alpha > 0.0
                && fill_alpha < 1.0)
                .then(|| {
                    std::array::from_fn(|channel| {
                        let source = if channel == 3 { 255 } else { color[channel] };
                        std::array::from_fn(|value| {
                            let blended =
                                source as f32 * fill_alpha + value as f32 * (1.0 - fill_alpha);
                            (f64::from(blended) + 0.5) as u8
                        })
                    })
                });
        for y in top..bottom {
            let dy = ((y as f32 + 0.5) / s - rect.y - rect.h * 0.5).abs() - rect.h * 0.5 + radius;
            // Keep a full pixel of margin around the solid span. Only edge and
            // corner pixels need the signed distance and varying coverage.
            let inset = if dy <= 0.0 { 0.0 } else { radius.max(0.0) };
            let solid_left =
                (((rect.x + inset) * s + 1.0).ceil().max(0.0) as u32).clamp(left.min(right), right);
            let solid_right = (((rect.x + rect.w - inset) * s - 1.0).floor().max(0.0) as u32)
                .clamp(solid_left, right);
            let solid = (dy - radius) * s <= -0.5 && solid_left < solid_right;
            let row_start = y as usize * self.size.0 as usize * 4;
            if solid {
                let span = &mut self.pixels
                    [row_start + solid_left as usize * 4..row_start + solid_right as usize * 4];
                if let Some(table) = &lookup {
                    for pixel in span.as_chunks_mut::<4>().0 {
                        for channel in 0..4 {
                            pixel[channel] = table[channel][pixel[channel] as usize];
                        }
                    }
                } else {
                    blend_span(span, color, fill_alpha);
                }
            }
            let edge_ranges = if solid {
                [left..solid_left, solid_right..right]
            } else {
                [left..right, right..right]
            };
            for range in edge_ranges {
                for x in range {
                    let dx = ((x as f32 + 0.5) / s - rect.x - rect.w * 0.5).abs() - rect.w * 0.5
                        + radius;
                    let coverage = rounded_coverage(dx, dy, radius, s);
                    if coverage > 0.0 {
                        let i = row_start + x as usize * 4;
                        blend_pixel(
                            &mut self.pixels[i..i + 4],
                            color,
                            coverage * alpha * self.opacity,
                        );
                    }
                }
            }
        }
    }
    pub fn text(&mut self, text: &str, rect: Rect, size: f32, bold: bool, color: [u8; 3]) {
        self.text_fill(text, rect, size, bold, color, 1.0);
    }
    pub fn text_fill(
        &mut self,
        text: &str,
        rect: Rect,
        size: f32,
        bold: bool,
        color: [u8; 3],
        fill: f32,
    ) {
        if self.opacity <= 0.0 || fill <= 0.0 {
            return;
        }
        let s = self.painter.scale;
        let (clip_left, clip_right) = pixel_clip(self.clip.x, self.clip.w, s, 0.0, self.size.0);
        let (clip_top, clip_bottom) = pixel_clip(self.clip.y, self.clip.h, s, 0.0, self.size.1);
        if clip_left >= clip_right || clip_top >= clip_bottom {
            return;
        }
        let Some(mask) = self.painter.mask(text, size, bold) else {
            return;
        };
        let start_x = (rect.x * s).round() as i32;
        let start_y = ((rect.y + (rect.h - mask.height as f32 / s) * 0.5) * s).round() as i32;
        let max_x = start_x
            + ((mask.width as f32 * fill.clamp(0.0, 1.0)).ceil() as i32).min((rect.w * s) as i32);
        let left = start_x.max(clip_left as i32);
        let right = max_x.min(clip_right as i32);
        let top = start_y.max(clip_top as i32);
        let bottom = (start_y + mask.height as i32).min(clip_bottom as i32);
        if left >= right || top >= bottom {
            return;
        }
        let width = (right - left) as usize;
        for y in top..bottom {
            let source = ((y - start_y) as u32 * mask.width + (left - start_x) as u32) as usize;
            let destination = (y as usize * self.size.0 as usize + left as usize) * 4;
            let row = &mut self.pixels[destination..destination + width * 4];
            for (pixel, &coverage) in row
                .as_chunks_mut::<4>()
                .0
                .iter_mut()
                .zip(&mask.coverage[source..source + width])
            {
                if coverage != 0 {
                    blend_pixel(pixel, color, coverage as f32 / 255.0 * self.opacity);
                }
            }
        }
    }
    pub fn ellipsis(&mut self, text: &str, rect: Rect, size: f32, bold: bool, color: [u8; 3]) {
        if self.painter.width(text, size, bold) <= rect.w {
            self.text(text, rect, size, bold, color);
            return;
        }
        let mut text = text.to_owned();
        while !text.is_empty() {
            text.pop();
            if self.painter.width(&format!("{text}…"), size, bold) <= rect.w {
                break;
            }
        }
        self.text(&format!("{text}…"), rect, size, bold, color);
    }
    pub fn gear(&mut self, rect: Rect, color: [u8; 3]) {
        let cx = rect.x + rect.w * 0.5;
        let cy = rect.y + rect.h * 0.5;
        for i in 0..8 {
            let a = i as f32 * std::f32::consts::FRAC_PI_4;
            self.rect(
                Rect::new(cx + a.cos() * 7.0 - 1.8, cy + a.sin() * 7.0 - 1.8, 3.6, 3.6),
                1.0,
                color,
                1.0,
            );
        }
        self.rect(Rect::new(cx - 6.0, cy - 6.0, 12.0, 12.0), 6.0, color, 1.0);
        self.rect(Rect::new(cx - 2.7, cy - 2.7, 5.4, 5.4), 2.7, [8, 8, 8], 1.0);
    }
    pub fn transport(&mut self, rect: Rect, kind: i32, playing: bool, color: [u8; 3]) {
        let cx = rect.x + rect.w * 0.5;
        let cy = rect.y + rect.h * 0.5;
        if kind == 1 && playing {
            self.rect(Rect::new(cx - 6.0, cy - 7.0, 4.0, 14.0), 1.0, color, 1.0);
            self.rect(Rect::new(cx + 2.0, cy - 7.0, 4.0, 14.0), 1.0, color, 1.0);
        } else {
            let direction = if kind == 0 { -1.0 } else { 1.0 };
            let scale = self.painter.scale;
            let bounds = Rect::new(cx - 8.0, cy - 8.0, 16.0, 16.0).intersect(self.clip);
            let (left, right) = pixel_clip(bounds.x, bounds.w, scale, 0.5, self.size.0);
            let (top, bottom) = pixel_clip(bounds.y, bounds.h, scale, 0.5, self.size.1);
            for y in top..bottom {
                let dy = ((y as f32 + 0.5) / scale - cy).abs();
                for x in left..right {
                    let dx = ((x as f32 + 0.5) / scale - cx) * direction + 4.0;
                    // Rasterize the triangle as one shape. Independently antialiased
                    // horizontal strips leave translucent seams at fractional DPI.
                    let distance = (-dx).max((dx + 1.5 * dy - 10.5) / 3.25_f32.sqrt());
                    let coverage = (0.5 - distance * scale).clamp(0.0, 1.0);
                    if coverage > 0.0 {
                        let offset = ((y * self.size.0 + x) * 4) as usize;
                        blend_pixel(
                            &mut self.pixels[offset..offset + 4],
                            color,
                            coverage * self.opacity,
                        );
                    }
                }
            }
            if kind != 1 {
                self.rect(
                    Rect::new(cx + direction * 8.0 - 1.0, cy - 7.0, 2.0, 14.0),
                    0.7,
                    color,
                    1.0,
                );
            }
        }
    }
    pub fn artwork(&mut self, image: &image::RgbaImage, rect: Rect, radius: f32) {
        if self.opacity <= 0.0 || image.width() == 0 || image.height() == 0 {
            return;
        }
        let s = self.painter.scale;
        let (clip_left, clip_right) = pixel_clip(self.clip.x, self.clip.w, s, 0.5, self.size.0);
        let (clip_top, clip_bottom) = pixel_clip(self.clip.y, self.clip.h, s, 0.5, self.size.1);
        let left = ((rect.x * s).max(0.0) as u32).max(clip_left);
        let right = (((rect.x + rect.w) * s).min(self.size.0 as f32) as u32).min(clip_right);
        let top = ((rect.y * s).max(0.0) as u32).max(clip_top);
        let bottom = (((rect.y + rect.h) * s).min(self.size.1 as f32) as u32).min(clip_bottom);
        if left >= right || top >= bottom {
            return;
        }
        // Reuse the storage and calculate horizontal sampling/geometry once per
        // column, rather than repeating both for every row of the artwork.
        let columns = &mut self.painter.artwork_columns;
        columns.clear();
        columns.extend((left..right).map(|x| {
            let px = (x as f32 + 0.5) / s;
            let ix = ((px - rect.x) / rect.w * image.width() as f32)
                .clamp(0.0, (image.width() - 1) as f32) as usize;
            let dx = (px - rect.x - rect.w * 0.5).abs() - rect.w * 0.5 + radius;
            (ix * 4, dx)
        }));
        let source = image.as_raw();
        let width = (right - left) as usize;
        for y in top..bottom {
            let py = (y as f32 + 0.5) / s;
            let dy = (py - rect.y - rect.h * 0.5).abs() - rect.h * 0.5 + radius;
            let iy = ((py - rect.y) / rect.h * image.height() as f32)
                .clamp(0.0, (image.height() - 1) as f32) as usize;
            let source_row =
                &source[iy * image.width() as usize * 4..(iy + 1) * image.width() as usize * 4];
            let destination = (y as usize * self.size.0 as usize + left as usize) * 4;
            for (pixel, &(ix, dx)) in self.pixels[destination..destination + width * 4]
                .as_chunks_mut::<4>()
                .0
                .iter_mut()
                .zip(columns.iter())
            {
                let coverage = rounded_coverage(dx, dy, radius, s);
                let alpha = coverage * source_row[ix + 3] as f32 / 255.0 * self.opacity;
                if alpha >= 1.0 {
                    pixel.copy_from_slice(&[
                        source_row[ix + 2],
                        source_row[ix + 1],
                        source_row[ix],
                        255,
                    ]);
                } else if alpha > 0.0 {
                    blend_pixel(
                        pixel,
                        [source_row[ix + 2], source_row[ix + 1], source_row[ix]],
                        alpha,
                    );
                }
            }
        }
    }
}

// Text clips sample pixel origins; artwork clips sample pixel centers. Adjust the
// endpoints using the same division as the original test so fractional DPI does
// not change which boundary pixels belong to a clip.
fn pixel_clip(origin: f32, extent: f32, scale: f32, offset: f32, limit: u32) -> (u32, u32) {
    let end = origin + extent;
    let mut first = ((origin * scale - offset).floor().max(0.0) as u32).min(limit);
    let mut last = (((end * scale - offset).ceil().max(0.0) as u32).saturating_add(1)).min(limit);
    while first < last && (first as f32 + offset) / scale < origin {
        first += 1;
    }
    while last > first && (last as f32 - 1.0 + offset) / scale >= end {
        last -= 1;
    }
    (first, last)
}

fn rounded_coverage(dx: f32, dy: f32, radius: f32, scale: f32) -> f32 {
    let distance = if dx > 0.0 && dy > 0.0 {
        dx.hypot(dy) - radius
    } else {
        dx.max(dy) - radius
    };
    (0.5 - distance * scale).clamp(0.0, 1.0)
}

fn blend_span(pixels: &mut [u8], color: [u8; 3], alpha: f32) {
    let alpha = alpha.clamp(0.0, 1.0);
    if alpha <= 0.0 {
        return;
    }
    if alpha >= 1.0 {
        for pixel in pixels.as_chunks_mut::<4>().0 {
            pixel.copy_from_slice(&[color[0], color[1], color[2], 255]);
        }
        return;
    }
    let inverse = 1.0 - alpha;
    let source = [
        color[0] as f32 * alpha,
        color[1] as f32 * alpha,
        color[2] as f32 * alpha,
        255.0 * alpha,
    ];
    for pixel in pixels.as_chunks_mut::<4>().0 {
        for channel in 0..4 {
            // Widen before adding half so a value just below 0.5 cannot round
            // up to 1.0 during the addition. The blend itself remains f32.
            let value = source[channel] + pixel[channel] as f32 * inverse;
            pixel[channel] = (f64::from(value) + 0.5) as u8;
        }
    }
}

#[cfg(test)]
mod tests {
    use super::{Canvas, Painter, Rect};
    use crate::render::blend_pixel;
    use std::hint::black_box;
    use std::time::{Duration, Instant};

    fn measure(mut draw: impl FnMut(), frames: u32) -> Duration {
        draw();
        let start = Instant::now();
        for _ in 0..frames {
            draw();
        }
        start.elapsed() / frames
    }

    #[test]
    #[ignore = "Measures native canvas hot paths in release mode"]
    fn canvas_render_benchmark() {
        for scale in [1.0, 1.5, 2.0] {
            let size = ((520.0 * scale) as u32, (580.0 * scale) as u32);
            let mut pixels = vec![0; size.0 as usize * size.1 as usize * 4];
            let mut painter = Painter::new(scale).unwrap();
            let mut c = Canvas::new(&mut pixels, size, &mut painter);
            let art = image::RgbaImage::from_fn(1024, 1024, |x, y| {
                image::Rgba([x as u8, y as u8, (x ^ y) as u8, 255])
            });
            let rect = measure(
                || c.rect(Rect::new(50.0, 10.0, 420.0, 500.0), 30.0, [30, 25, 20], 0.8),
                30,
            );
            let artwork = measure(
                || c.artwork(&art, Rect::new(75.0, 50.0, 180.0, 180.0), 14.0),
                30,
            );
            let text = measure(
                || {
                    c.text(
                        "Maybe you can show me how to love",
                        Rect::new(75.0, 300.0, 370.0, 40.0),
                        21.0,
                        true,
                        [248; 3],
                    )
                },
                100,
            );
            c.clip = Rect::new(75.0, 300.0, 90.0, 40.0);
            let long = "Maybe you can show me how to love ".repeat(40);
            let clipped_text = measure(
                || {
                    c.text_fill(
                        &long,
                        Rect::new(75.0, 300.0, 370.0, 40.0),
                        21.0,
                        true,
                        [248; 3],
                        0.1,
                    )
                },
                100,
            );
            black_box(&c.pixels);
            println!(
                "{scale}x: rect {rect:?}, artwork {artwork:?}, text {text:?}, clipped long text {clipped_text:?}"
            );
        }
    }

    #[test]
    #[ignore = "Checks that clipping work stays bounded by visible text in release mode"]
    fn clipped_text_cost_is_independent_of_hidden_mask_width() {
        let mut painter = Painter::new(1.0).unwrap();
        let mut pixels = vec![0; 320 * 100 * 4];
        let mut c = Canvas::new(&mut pixels, (320, 100), &mut painter);
        c.clip = Rect::new(0.0, 0.0, 60.0, 100.0);
        let rect = Rect::new(0.0, 0.0, 300.0, 40.0);
        let short = "Maybe you can show me how to love ";
        let long = short.repeat(80);
        let short_time = measure(|| c.text(short, rect, 21.0, true, [248; 3]), 200);
        let long_time = measure(|| c.text(&long, rect, 21.0, true, [248; 3]), 200);
        black_box(&c.pixels);
        println!("clipped text: short {short_time:?}, long {long_time:?}");
        assert!(
            long_time < short_time * 4 + Duration::from_micros(50),
            "hidden mask pixels must not be scanned: short {short_time:?}, long {long_time:?}"
        );
    }

    #[test]
    fn text_clipping_and_fill_preserve_cached_glyph_coverage() {
        for scale in [1.0, 1.25, 1.5, 2.0] {
            let mut painter = Painter::new(scale).unwrap();
            let size = (97, 61);
            let rect = Rect::new(-7.3, -5.2, 58.3, 43.1);
            let clip = Rect::new(2.3, 3.7, 42.4, 29.5);
            for fill in [0.0, 0.13, 0.5, 1.0] {
                let mut actual = vec![37; size.0 as usize * size.1 as usize * 4];
                let mut expected = actual.clone();
                let mask = painter.mask("Maybe on my own ", 21.0, true).unwrap();
                let mask_pointer = mask.coverage.as_ptr();
                let start_x = (rect.x * scale).round() as i32;
                let start_y =
                    ((rect.y + (rect.h - mask.height as f32 / scale) * 0.5) * scale).round() as i32;
                let max_x = start_x
                    + ((mask.width as f32 * fill).ceil() as i32).min((rect.w * scale) as i32);
                for my in 0..mask.height {
                    for mx in 0..mask.width {
                        let x = start_x + mx as i32;
                        let y = start_y + my as i32;
                        if x >= 0
                            && y >= 0
                            && x < max_x
                            && x < size.0 as i32
                            && y < size.1 as i32
                            && clip.contains(x as f32 / scale, y as f32 / scale)
                        {
                            let i = ((y as u32 * size.0 + x as u32) * 4) as usize;
                            blend_pixel(
                                &mut expected[i..i + 4],
                                [230, 140, 70],
                                mask.coverage[(my * mask.width + mx) as usize] as f32 / 255.0
                                    * 0.73,
                            );
                        }
                    }
                }
                let mut c = Canvas::new(&mut actual, size, &mut painter);
                c.clip = clip;
                c.opacity = 0.73;
                c.text_fill("Maybe on my own ", rect, 21.0, true, [230, 140, 70], fill);
                assert_eq!(actual, expected, "scale {scale}, fill {fill}");
                assert_eq!(
                    painter
                        .mask("Maybe on my own ", 21.0, true)
                        .unwrap()
                        .coverage
                        .as_ptr(),
                    mask_pointer,
                    "drawing must reuse the cached mask"
                );
            }
        }
    }

    #[test]
    fn rounded_primitives_preserve_antialiasing_and_premultiplied_alpha() {
        for scale in [1.0, 1.25, 1.5, 2.0] {
            let mut painter = Painter::new(scale).unwrap();
            let size = (97, 61);
            let clip = Rect::new(2.3, 3.7, 42.4, 29.5);
            let rect = Rect::new(-2.3, -1.7, 53.2, 39.4);
            for radius in [0.0, 3.7, 19.7] {
                for opacity in [0.0, 0.73, 1.0] {
                    let mut actual = vec![37; size.0 as usize * size.1 as usize * 4];
                    let mut expected = actual.clone();
                    let bounds = rect.intersect(clip);
                    for y in (bounds.y * scale).floor().max(0.0) as u32
                        ..((bounds.y + bounds.h) * scale).ceil().min(size.1 as f32) as u32
                    {
                        for x in (bounds.x * scale).floor().max(0.0) as u32
                            ..((bounds.x + bounds.w) * scale).ceil().min(size.0 as f32) as u32
                        {
                            let dx = ((x as f32 + 0.5) / scale - rect.x - rect.w * 0.5).abs()
                                - rect.w * 0.5
                                + radius;
                            let dy = ((y as f32 + 0.5) / scale - rect.y - rect.h * 0.5).abs()
                                - rect.h * 0.5
                                + radius;
                            let coverage = (0.5
                                - (dx.max(0.0).hypot(dy.max(0.0)) + dx.max(dy).min(0.0) - radius)
                                    * scale)
                                .clamp(0.0, 1.0);
                            let i = ((y * size.0 + x) * 4) as usize;
                            blend_pixel(
                                &mut expected[i..i + 4],
                                [230, 140, 70],
                                coverage * opacity,
                            );
                        }
                    }
                    let mut c = Canvas::new(&mut actual, size, &mut painter);
                    c.clip = clip;
                    c.opacity = opacity;
                    c.rect(rect, radius, [230, 140, 70], 1.0);
                    assert_eq!(
                        actual, expected,
                        "scale {scale}, radius {radius}, opacity {opacity}"
                    );
                }
            }
        }
    }

    #[test]
    fn solid_spans_match_per_pixel_blending_for_all_channel_values() {
        let initial: Vec<u8> = (0..=255).flat_map(|channel| [channel; 4]).collect();
        for source in 0..=255 {
            for alpha in [
                0.0,
                0.04,
                0.08,
                0.11,
                0.14,
                f32::from_bits(0.5_f32.to_bits() - 1),
                0.5,
                0.73,
                0.8,
                1.0,
            ] {
                let color = [source, 255 - source, source];
                let mut expected = initial.clone();
                let mut actual = initial.clone();
                for pixel in expected.as_chunks_mut::<4>().0 {
                    blend_pixel(pixel, color, alpha);
                }
                super::blend_span(&mut actual, color, alpha);
                assert_eq!(actual, expected, "source {source}, alpha {alpha}");
            }
        }
    }

    #[test]
    fn large_translucent_fills_match_scalar_blending() {
        let size = (256, 24);
        let mut painter = Painter::new(1.0).unwrap();
        let initial: Vec<u8> = (0..size.0 * size.1)
            .flat_map(|value| [value as u8; 4])
            .collect();
        for color in [[0, 0, 0], [248, 164, 112], [255, 255, 255]] {
            for alpha in [
                0.04,
                0.14,
                f32::from_bits(0.5_f32.to_bits() - 1),
                0.5,
                0.73,
                1.0,
            ] {
                for opacity in [0.16, 0.73, 1.0] {
                    let mut expected = initial.clone();
                    for pixel in expected.as_chunks_mut::<4>().0 {
                        blend_pixel(pixel, color, alpha * opacity);
                    }
                    let mut actual = initial.clone();
                    let mut canvas = Canvas::new(&mut actual, size, &mut painter);
                    canvas.opacity = opacity;
                    canvas.rect(Rect::new(0.0, 0.0, 256.0, 24.0), 0.0, color, alpha);
                    assert_eq!(
                        actual, expected,
                        "color {color:?}, alpha {alpha}, opacity {opacity}"
                    );
                }
            }
        }
    }

    #[test]
    fn transport_triangles_have_no_interior_seams_at_fractional_dpi() {
        for scale in [1.0, 1.25, 1.5, 2.0] {
            let size = ((52.0 * scale) as u32, (48.0 * scale) as u32);
            let mut painter = Painter::new(scale).unwrap();
            for kind in [0, 1, 2] {
                let mut pixels = vec![0; size.0 as usize * size.1 as usize * 4];
                let mut canvas = Canvas::new(&mut pixels, size, &mut painter);
                canvas.transport(Rect::new(4.0, 3.0, 44.0, 40.0), kind, false, [255; 3]);
                let direction = if kind == 0 { -1.0 } else { 1.0 };
                for y in 0..size.1 {
                    for x in 0..size.0 {
                        let dx = ((x as f32 + 0.5) / scale - 26.0) * direction + 4.0;
                        let dy = ((y as f32 + 0.5) / scale - 23.0).abs();
                        if dx > 1.5 && dx + 1.5 * dy < 8.0 {
                            let offset = ((y * size.0 + x) * 4) as usize;
                            assert_eq!(
                                &pixels[offset..offset + 4],
                                &[255; 4],
                                "solid triangle interior at {scale}x, kind {kind}, pixel ({x},{y})"
                            );
                        }
                    }
                }
            }
        }
    }

    #[test]
    fn artwork_preserves_sampling_alpha_and_fractional_clipping() {
        let image = image::RgbaImage::from_fn(13, 9, |x, y| {
            image::Rgba([x as u8 * 17, y as u8 * 23, 91, ((x + y) * 11) as u8])
        });
        for scale in [1.0, 1.25, 1.5, 2.0] {
            let mut painter = Painter::new(scale).unwrap();
            let size = (97, 61);
            let clip = Rect::new(2.3, 3.7, 42.4, 29.5);
            let rect = Rect::new(-2.3, -1.7, 53.2, 39.4);
            let radius = 13.7;
            let mut actual = vec![37; size.0 as usize * size.1 as usize * 4];
            let mut expected = actual.clone();
            for y in (rect.y * scale).max(0.0) as u32
                ..((rect.y + rect.h) * scale).min(size.1 as f32) as u32
            {
                for x in (rect.x * scale).max(0.0) as u32
                    ..((rect.x + rect.w) * scale).min(size.0 as f32) as u32
                {
                    let px = (x as f32 + 0.5) / scale;
                    let py = (y as f32 + 0.5) / scale;
                    if !clip.contains(px, py) {
                        continue;
                    }
                    let dx = (px - rect.x - rect.w * 0.5).abs() - rect.w * 0.5 + radius;
                    let dy = (py - rect.y - rect.h * 0.5).abs() - rect.h * 0.5 + radius;
                    let coverage = (0.5
                        - (dx.max(0.0).hypot(dy.max(0.0)) + dx.max(dy).min(0.0) - radius) * scale)
                        .clamp(0.0, 1.0);
                    let ix = ((px - rect.x) / rect.w * image.width() as f32)
                        .clamp(0.0, (image.width() - 1) as f32) as u32;
                    let iy = ((py - rect.y) / rect.h * image.height() as f32)
                        .clamp(0.0, (image.height() - 1) as f32)
                        as u32;
                    let p = image.get_pixel(ix, iy).0;
                    let i = ((y * size.0 + x) * 4) as usize;
                    blend_pixel(
                        &mut expected[i..i + 4],
                        [p[2], p[1], p[0]],
                        coverage * p[3] as f32 / 255.0 * 0.73,
                    );
                }
            }
            let mut c = Canvas::new(&mut actual, size, &mut painter);
            c.clip = clip;
            c.opacity = 0.73;
            c.artwork(&image, rect, radius);
            assert_eq!(actual, expected, "scale {scale}");
        }
    }
}
