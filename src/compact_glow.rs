use std::sync::Arc;

use crate::animation::{COMPACT, ShapeState};
use crate::geometry::IslandDistance;
use crate::render::RenderParams;

#[derive(Default)]
pub(crate) struct CoverGlow {
    artwork: Option<Arc<image::RgbaImage>>,
    palette: [[f32; 4]; 8],
    phase: f32,
}

impl CoverGlow {
    pub fn advance(&mut self, elapsed: f32, playing: bool) {
        // The glow's slow color sweep is ambient animation; hold it still
        // when the OS asks for reduced motion (DESIGN.md motion policy).
        if crate::animation::reduced_motion() || !playing || !elapsed.is_finite() {
            return;
        }
        self.phase = (self.phase + elapsed.max(0.0) * 0.32) % std::f32::consts::TAU;
    }
    pub fn update(&mut self, artwork: Option<&Arc<image::RgbaImage>>) {
        if self
            .artwork
            .as_ref()
            .zip(artwork)
            .is_some_and(|(old, new)| Arc::ptr_eq(old, new))
        {
            return;
        }
        self.artwork = artwork.cloned();
        self.palette = [[0.0; 4]; 8];
        let Some(artwork) = artwork.filter(|art| art.width() > 0 && art.height() > 0) else {
            return;
        };
        // Keep colors separate from black borders, white branding and transparent
        // padding until after extraction; averaging them first hides dark covers.
        let thumbnail = image::imageops::resize(
            artwork.as_ref(),
            32,
            32,
            image::imageops::FilterType::Nearest,
        );
        let Some(global) = cover_color(&thumbnail, 0..32, false) else {
            return;
        };
        for (column, color) in self.palette.iter_mut().enumerate() {
            let x = column as u32 * 4;
            let local = cover_color(&thumbnail, x.saturating_sub(8)..(x + 12).min(32), true)
                .unwrap_or(global);
            // Let the global hue carry across black margins while retaining a
            // broad horizontal variation from the artwork itself.
            *color = std::array::from_fn(|channel| global[channel] * 0.55 + local[channel] * 0.45);
        }
    }

    pub fn draw(&self, pixels: &mut [u8], size: (u32, u32), params: RenderParams) {
        let logical = ShapeState {
            height: params.state.height / params.scale,
            ..COMPACT
        };
        let opacity = logical.compact_content_alpha();
        if opacity <= 0.0
            || self.palette.iter().all(|color| color[3] == 0.0)
            || pixels.len() != size.0 as usize * size.1 as usize * 4
        {
            return;
        }
        let shape = params.state;
        let center = (size.0 as f32 * 0.5, params.top_margin + shape.height * 0.5);
        let geometry = IslandDistance::new(shape, params.attached);
        let left = (center.0 - shape.width * 0.5).floor().max(0.0) as u32;
        let right = (center.0 + shape.width * 0.5).ceil().min(size.0 as f32) as u32;
        let top = (params.top_margin + shape.height * 0.30).floor().max(0.0) as u32;
        let bottom = (params.top_margin + shape.height).ceil().min(size.1 as f32) as u32;
        for y in top..bottom {
            let vertical = ((y as f32 + 0.5 - params.top_margin) / shape.height - 0.30) / 0.70;
            let fade = vertical.clamp(0.0, 1.0).powf(1.2) * 0.82 * opacity;
            let row = geometry.row(y as f32 + 0.5, center.1);
            for x in left..right {
                let column = geometry.column(x as f32 + 0.5, center.0);
                let coverage = (0.5 - geometry.distance(column, row)).clamp(0.0, 1.0);
                if coverage == 0.0 {
                    continue;
                }
                let along = ((x as f32 + 0.5 - center.0) / shape.width + 0.5).clamp(0.0, 1.0);
                let position =
                    (along + 0.075 * (self.phase + along * 4.0).sin()).clamp(0.0, 1.0) * 7.0;
                let index = position.floor() as usize;
                let next = (index + 1).min(7);
                let mix = position - index as f32;
                let color: [f32; 4] = std::array::from_fn(|channel| {
                    self.palette[index][channel] * (1.0 - mix) + self.palette[next][channel] * mix
                });
                let pixel = &mut pixels[((y * size.0 + x) * 4) as usize..][..4];
                let alpha = f32::from(pixel[3]) / 255.0;
                for channel in 0..3 {
                    pixel[channel] = (f32::from(pixel[channel])
                        + (color[channel] * alpha - f32::from(pixel[channel]))
                            * fade
                            * coverage
                            * color[3]
                            * (1.04 + 0.04 * (self.phase * 0.7 - along * 8.0).cos()))
                    .round()
                    .clamp(0.0, f32::from(pixel[3])) as u8;
                }
            }
        }
    }
}

fn cover_color(
    image: &image::RgbaImage,
    columns: std::ops::Range<u32>,
    chromatic_only: bool,
) -> Option<[f32; 4]> {
    let mut chromatic = [0.0; 3];
    let mut neutral = [0.0; 3];
    let mut color_weight = 0.0;
    let mut neutral_weight = 0.0;
    let mut opacity = 0.0_f32;
    for x in columns {
        for y in 0..image.height() {
            let pixel = image.get_pixel(x, y).0;
            let alpha = f32::from(pixel[3]) / 255.0;
            if alpha == 0.0 {
                continue;
            }
            opacity = opacity.max(alpha);
            let high = f32::from(
                *pixel[..3]
                    .iter()
                    .max()
                    .expect("RGB pixels have three channels"),
            );
            let low = f32::from(
                *pixel[..3]
                    .iter()
                    .min()
                    .expect("RGB pixels have three channels"),
            );
            let saturation = (high - low) / high.max(1.0);
            let weight = if high >= 12.0 && saturation >= 0.08 {
                alpha * saturation.powi(2)
            } else {
                0.0
            };
            for channel in 0..3 {
                chromatic[channel] += f32::from(pixel[2 - channel]) * weight;
                neutral[channel] += f32::from(pixel[2 - channel]) * alpha;
            }
            color_weight += weight;
            neutral_weight += alpha;
        }
    }
    if opacity == 0.0 || (chromatic_only && color_weight == 0.0) {
        return None;
    }
    let mut color = if color_weight > 0.0 {
        chromatic.map(|channel| channel / color_weight)
    } else {
        neutral.map(|channel| channel / neutral_weight)
    };
    let peak = color.into_iter().fold(0.0_f32, f32::max);
    // Lift the extracted color rather than its blend opacity. Dark artwork must
    // remain visible over black, without inventing a hue for grayscale covers.
    let target = if color_weight > 0.0 { 200.0 } else { 120.0 };
    if peak > 0.0 {
        let lift = (target / peak).max(1.0);
        color = color.map(|channel| (channel * lift).min(255.0));
    } else {
        color = [target; 3];
    }
    Some([color[0], color[1], color[2], opacity])
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::animation::ShapeState;
    use crate::render::{RenderParams, render_frame_with_margin};
    use std::sync::Arc;

    #[test]
    fn glow_moves_during_playback_and_freezes_while_paused_without_changing_alpha() {
        let mut glow = CoverGlow::default();
        // Warm/cool artwork fixtures verify hue sampling independently of UI accents.
        let artwork = Arc::new(image::RgbaImage::from_fn(32, 32, |x, _| {
            image::Rgba(if x < 16 {
                [220, 70, 40, 255]
            } else {
                [40, 100, 220, 255]
            })
        }));
        glow.update(Some(&artwork));
        let size = (440, 70);
        let params = RenderParams {
            state: ShapeState {
                width: 360.0,
                height: 40.0,
                radius: 20.0,
            },
            top_margin: 0.0,
            attached: true,
            scale: 1.0,
            shadow_opacity: 0.0,
            hover: 0.0,
            accent: crate::theme::ICE,
        };
        let mut base = vec![0; (size.0 * size.1 * 4) as usize];
        render_frame_with_margin(&mut base, size, params);
        let render = |glow: &CoverGlow| {
            let mut pixels = base.clone();
            glow.draw(&mut pixels, size, params);
            pixels
        };
        let first = render(&glow);
        glow.advance(1.0, true);
        let moved = render(&glow);
        assert_ne!(first, moved);
        glow.advance(4.0, false);
        assert_eq!(moved, render(&glow));
        assert!(
            base.as_chunks::<4>()
                .0
                .iter()
                .zip(moved.as_chunks::<4>().0.iter())
                .all(|(a, b)| a[3] == b[3])
        );
    }

    #[test]
    fn neutral_covers_stay_neutral_and_preserve_artwork_opacity() {
        // Grayscale artwork fixtures cover brightness and alpha boundaries without a theme hue.
        let mut glow = CoverGlow::default();
        for level in [0, 20, 128, 255] {
            let artwork = Arc::new(image::RgbaImage::from_pixel(
                32,
                32,
                image::Rgba([level, level, level, 128]),
            ));
            glow.update(Some(&artwork));
            for color in glow.palette {
                assert_eq!(color[0], color[1]);
                assert_eq!(color[1], color[2]);
                assert!(color[0] >= 119.0 && color[0] <= 255.0);
                assert!((color[3] - 128.0 / 255.0).abs() < 0.001);
            }
        }
    }

    #[test]
    fn dark_cover_color_survives_black_white_and_transparent_regions() {
        // Dark blue, white, black and transparent artwork fixtures reproduce the SMTC thumbnail failure.
        let artwork = Arc::new(image::RgbaImage::from_fn(32, 32, |x, y| {
            image::Rgba(if y >= 28 {
                [255, 255, 255, 0]
            } else if y >= 24 {
                [245, 245, 245, 255]
            } else if (12..20).contains(&x) && (4..20).contains(&y) {
                [12, 25, 40, 255]
            } else {
                [2, 2, 2, 255]
            })
        }));
        let mut glow = CoverGlow::default();
        glow.update(Some(&artwork));
        for color in glow.palette {
            assert!(
                color[0] >= 180.0,
                "lift the dark cover's blue enough to remain visible: {color:?}"
            );
            assert!(
                color[0] > color[2] * 1.8,
                "black and white regions must not wash out the cover hue: {color:?}"
            );
            assert!(
                color[3] >= 0.95,
                "transparent padding must not dim the extracted glow: {color:?}"
            );
        }
        for scale in [1.0, 1.25, 1.5, 2.0] {
            let params = RenderParams {
                state: ShapeState {
                    width: 148.0 * scale,
                    height: 40.0 * scale,
                    radius: 16.0 * scale,
                },
                top_margin: 0.0,
                attached: true,
                scale,
                shadow_opacity: 0.0,
                hover: 0.0,
                accent: crate::theme::ICE,
            };
            let size = ((220.0 * scale) as u32, (60.0 * scale) as u32);
            let mut pixels = vec![0; (size.0 * size.1 * 4) as usize];
            render_frame_with_margin(&mut pixels, size, params);
            glow.draw(&mut pixels, size, params);
            let lower = (((35.0 * scale) as u32 * size.0 + (110.0 * scale) as u32) * 4) as usize;
            assert!(
                pixels[lower] >= 110 && pixels[lower] > pixels[lower + 2] + 50,
                "a short compact line must have a visible colored lower edge at {scale}x: {:?}",
                &pixels[lower..lower + 4]
            );
        }
    }

    #[test]
    #[ignore = "Prints the palette of a locally supplied cover for diagnostics"]
    fn live_cover_palette_diagnostic() {
        let path = std::env::var_os("HIYODYNA_COVER_FILE").expect("HIYODYNA_COVER_FILE");
        let artwork = Arc::new(image::open(path).unwrap().into_rgba8());
        let mut glow = CoverGlow::default();
        glow.update(Some(&artwork));
        println!("live cover palette BGRA: {:?}", glow.palette);
    }

    #[test]
    fn cover_glow_tracks_artwork_and_preserves_shape_alpha_at_each_dpi() {
        // Fixed warm, cool and transparent artwork fixtures exercise cover-derived glow.
        let warm = Arc::new(image::RgbaImage::from_pixel(
            8,
            8,
            image::Rgba([230, 70, 30, 255]),
        ));
        let cool = Arc::new(image::RgbaImage::from_pixel(
            8,
            8,
            image::Rgba([30, 70, 230, 255]),
        ));
        let transparent = Arc::new(image::RgbaImage::from_pixel(
            8,
            8,
            image::Rgba([255, 0, 0, 0]),
        ));
        for scale in [1.0, 1.25, 1.5, 2.0] {
            let size = ((440.0 * scale) as u32, (70.0 * scale) as u32);
            let params = RenderParams {
                state: ShapeState {
                    width: 360.0 * scale,
                    height: 40.0 * scale,
                    radius: 20.0 * scale,
                },
                top_margin: 0.0,
                attached: true,
                scale,
                shadow_opacity: 0.22,
                hover: 0.0,
                accent: crate::theme::ICE,
            };
            let mut base = vec![0; (size.0 * size.1 * 4) as usize];
            render_frame_with_margin(&mut base, size, params);
            let mut glow = CoverGlow::default();
            let mut tinted = base.clone();
            glow.draw(&mut tinted, size, params);
            assert_eq!(base, tinted, "missing cover must leave the capsule alone");
            glow.update(Some(&warm));
            glow.draw(&mut tinted, size, params);
            let index =
                |x: f32, y: f32| (((y * scale) as u32 * size.0 + (x * scale) as u32) * 4) as usize;
            let lower = index(220.0, 35.0);
            assert!(
                tinted[lower + 2] > tinted[lower] + 30,
                "warm cover must produce a visible warm lower glow"
            );
            let upper = index(220.0, 10.0);
            assert_eq!(
                &base[upper..upper + 4],
                &tinted[upper..upper + 4],
                "keep the text background dark"
            );
            for (before, after) in base
                .as_chunks::<4>()
                .0
                .iter()
                .zip(tinted.as_chunks::<4>().0)
            {
                assert_eq!(
                    before[3], after[3],
                    "glow must not alter silhouette/shadow alpha"
                );
                if before[3] == 0 {
                    assert_eq!(before, after);
                }
            }
            glow.update(Some(&cool));
            let mut blue = base.clone();
            glow.draw(&mut blue, size, params);
            assert!(
                blue[lower] > blue[lower + 2] + 30,
                "new artwork must replace the previous palette"
            );
            for artwork in [Some(&transparent), None] {
                glow.update(artwork);
                let mut cleared = base.clone();
                glow.draw(&mut cleared, size, params);
                assert_eq!(
                    cleared, base,
                    "missing or transparent replacement must clear stale colors"
                );
            }
            glow.update(Some(&warm));
            let mut expanded = base.clone();
            glow.draw(
                &mut expanded,
                size,
                RenderParams {
                    state: ShapeState {
                        height: 150.0 * scale,
                        ..params.state
                    },
                    ..params
                },
            );
            assert_eq!(expanded, base, "compact glow must disappear when expanded");
        }
    }
}
