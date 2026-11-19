use crate::animation::ShapeState;
use crate::geometry::rounded_rect_distance;

const SHADOW_OFFSET_Y: f32 = 3.0;
const SHADOW_SIGMA: f32 = 3.0;
const SHADOW_MAX_DISTANCE: f32 = SHADOW_SIGMA * 3.0;
const DEFAULT_SHADOW_OPACITY: f32 = 0.22;
const HALO_SIGMA: f32 = 6.0;
const HALO_MAX_DISTANCE: f32 = HALO_SIGMA * 3.0;
const HALO_OPACITY: f32 = 0.06;
const ICE_ACCENT: [u8; 3] = [255, 199, 143];
const BAYER_4X4: [[f32; 4]; 4] = [
    [0.0, 8.0, 2.0, 10.0],
    [12.0, 4.0, 14.0, 6.0],
    [3.0, 11.0, 1.0, 9.0],
    [15.0, 7.0, 13.0, 5.0],
];
#[cfg(test)]
pub(crate) const SHADOW_EXTENT: f32 = SHADOW_OFFSET_Y + 1.5 + (SHADOW_SIGMA + 1.5) * 3.0 + 1.0;

#[derive(Clone, Copy, Debug, PartialEq)]
pub(crate) struct RenderParams {
    pub state: ShapeState,
    pub top_margin: f32,
    pub scale: f32,
    pub shadow_opacity: f32,
    pub hover: f32,
    pub accent: [u8; 3],
}

/// Draws a centered shape into a top-down BGRA premultiplied-alpha buffer.
#[allow(dead_code)] // Public platform-neutral rendering entry point; the window uses a top-margin variant.
pub(crate) fn render_frame(pixels: &mut [u8], size: (u32, u32), state: ShapeState) {
    render_frame_with_margin(
        pixels,
        size,
        RenderParams {
            state,
            top_margin: size.1 as f32 * 0.5 - state.height * 0.5,
            scale: 1.0,
            shadow_opacity: DEFAULT_SHADOW_OPACITY,
            hover: 0.0,
            accent: ICE_ACCENT,
        },
    );
}

pub(crate) fn render_frame_with_margin(pixels: &mut [u8], size: (u32, u32), params: RenderParams) {
    let Some(expected_len) = (size.0 as usize)
        .checked_mul(size.1 as usize)
        .and_then(|pixel_count| pixel_count.checked_mul(4))
    else {
        return;
    };
    if pixels.len() != expected_len || size.0 == 0 || size.1 == 0 {
        return;
    }

    pixels.fill(0);
    let state = params.state;
    let scale = params.scale;
    let hover = params.hover.clamp(0.0, 1.0);
    let center = (size.0 as f32 * 0.5, params.top_margin + state.height * 0.5);

    let shadow_offset = (SHADOW_OFFSET_Y + 1.5 * hover) * scale;
    let shadow_sigma = (SHADOW_SIGMA + 1.5 * hover) * scale;
    let shadow_max_distance = (SHADOW_MAX_DISTANCE + 4.5 * hover) * scale;
    let shadow_opacity = (params.shadow_opacity * (1.0 + 0.36 * hover)).clamp(0.0, 1.0);
    let halo_max_distance = HALO_MAX_DISTANCE * scale;
    let reach = shadow_max_distance.max(halo_max_distance);
    let left = (center.0 - state.width * 0.5 - reach - 1.0)
        .floor()
        .max(0.0) as u32;
    let right = (center.0 + state.width * 0.5 + reach + 1.0)
        .ceil()
        .min(size.0 as f32) as u32;
    let top = (center.1 - state.height * 0.5 - reach - 1.0)
        .floor()
        .max(0.0) as u32;
    let bottom = (center.1 + state.height * 0.5 + shadow_offset + reach + 1.0)
        .ceil()
        .min(size.1 as f32) as u32;

    for y in top..bottom {
        for x in left..right {
            let px = x as f32 + 0.5;
            let py = y as f32 + 0.5;
            let index = ((y * size.0 + x) * 4) as usize;

            let distance = rounded_rect_distance(px, py, state, center);
            if distance > 0.0 {
                if shadow_opacity > 0.0 {
                    let shadow_distance =
                        rounded_rect_distance(px, py - shadow_offset, state, center);
                    if shadow_distance < shadow_max_distance {
                        // Keep the offset silhouette itself, then soften its outer edge.
                        // Clipping to the front shape above leaves only the visible cast shadow.
                        let falloff_distance = shadow_distance.max(0.0);
                        let normalized = falloff_distance / shadow_sigma;
                        let alpha = (-0.5 * normalized * normalized).exp() * shadow_opacity;
                        blend_pixel(&mut pixels[index..index + 4], [0, 0, 0], alpha);
                    }
                }

                if hover > 0.0 && distance < halo_max_distance {
                    let normalized = distance / (HALO_SIGMA * scale);
                    let alpha = (-0.5 * normalized * normalized).exp() * HALO_OPACITY * hover;
                    blend_pixel(&mut pixels[index..index + 4], params.accent, alpha);
                }
            }

            let coverage = (0.5 - distance).clamp(0.0, 1.0);
            if coverage > 0.0 {
                let vertical = ((py - (center.1 - state.height * 0.5)) / state.height.max(1.0))
                    .clamp(0.0, 1.0);
                let lift = hover * 4.0;
                let dither = dither_offset(x, y);
                let blue = (28.0 - 8.0 * vertical + lift + dither).round();
                let green = (27.0 - 8.0 * vertical + lift + dither).round();
                let red = (25.0 - 7.0 * vertical + lift + dither).round();
                blend_pixel(
                    &mut pixels[index..index + 4],
                    [
                        blue.clamp(0.0, 255.0) as u8,
                        green.clamp(0.0, 255.0) as u8,
                        red.clamp(0.0, 255.0) as u8,
                    ],
                    coverage,
                );

                let edge = (1.0 - (distance + 0.55).abs() / 0.65).clamp(0.0, 1.0);
                if edge > 0.0 {
                    let top_weight = 1.0 - 0.65 * vertical;
                    let rim_alpha = (0.10 + 0.05 * hover) * top_weight;
                    blend_pixel(
                        &mut pixels[index..index + 4],
                        [255, 255, 255],
                        edge * coverage * rim_alpha,
                    );
                }
            }
        }
    }
}

fn dither_offset(x: u32, y: u32) -> f32 {
    BAYER_4X4[(y % 4) as usize][(x % 4) as usize] / 16.0 - 0.5
}

pub(crate) fn blend_pixel(destination: &mut [u8], color_bgra: [u8; 3], alpha: f32) {
    let alpha = alpha.clamp(0.0, 1.0);
    let inverse_alpha = 1.0 - alpha;
    for channel in 0..3 {
        destination[channel] = (color_bgra[channel] as f32 * alpha
            + destination[channel] as f32 * inverse_alpha)
            .round()
            .clamp(0.0, 255.0) as u8;
    }
    destination[3] = (alpha * 255.0 + destination[3] as f32 * inverse_alpha)
        .round()
        .clamp(0.0, 255.0) as u8;
}

#[cfg(test)]
mod tests {
    use super::{RenderParams, render_frame, render_frame_with_margin};
    use crate::animation::{COMPACT, EXPANDED};

    const ICE: [u8; 3] = [255, 199, 143];

    fn params(state: crate::animation::ShapeState, top_margin: f32, hover: f32) -> RenderParams {
        RenderParams {
            state,
            top_margin,
            scale: 1.0,
            shadow_opacity: 0.22,
            hover,
            accent: ICE,
        }
    }

    #[test]
    fn renders_the_requested_bgra_buffer_size() {
        let size = (64, 32);
        let mut pixels = vec![0_u8; size.0 as usize * size.1 as usize * 4];

        render_frame(&mut pixels, size, COMPACT);

        assert_eq!(pixels.len(), 64 * 32 * 4);
    }

    #[test]
    fn leaves_the_transparent_corner_outside_the_shape() {
        let size = (256, 128);
        let mut pixels = vec![0_u8; size.0 as usize * size.1 as usize * 4];

        render_frame(&mut pixels, size, COMPACT);

        assert_eq!(&pixels[0..4], &[0, 0, 0, 0]);
    }

    #[test]
    fn fills_the_shape_center_with_opaque_color() {
        let size = (256, 128);
        let mut pixels = vec![0_u8; size.0 as usize * size.1 as usize * 4];
        let center = ((size.1 / 2 * size.0 + size.0 / 2) * 4) as usize;

        render_frame(&mut pixels, size, COMPACT);

        assert!(pixels[center + 3] > 240);
        assert!(pixels[center] < 40);
        assert!(pixels[center + 1] < 40);
        assert!(pixels[center + 2] < 40);
    }

    #[test]
    fn keeps_rgb_channels_premultiplied_by_alpha() {
        let size = (256, 128);
        let mut pixels = vec![0_u8; size.0 as usize * size.1 as usize * 4];

        render_frame(&mut pixels, size, COMPACT);

        for pixel in pixels.as_chunks::<4>().0 {
            assert!(pixel[0] <= pixel[3]);
            assert!(pixel[1] <= pixel[3]);
            assert!(pixel[2] <= pixel[3]);
        }
    }

    #[test]
    fn keeps_compact_and_expanded_shapes_anchored_to_the_same_top_margin() {
        let size = (512, 220);
        for state in [COMPACT, EXPANDED] {
            let mut pixels = vec![0_u8; size.0 as usize * size.1 as usize * 4];
            render_frame_with_margin(&mut pixels, size, params(state, 12.0, 0.0));

            let first_visible_row = (0..size.1)
                .find(|&y| (0..size.0).any(|x| pixels[((y * size.0 + x) * 4 + 3) as usize] > 240))
                .expect("rendered shape should have opaque pixels");
            assert_eq!(first_visible_row, 12);
        }
    }

    #[test]
    fn shape_touches_the_top_when_margin_is_zero() {
        let size = (512, 220);
        let mut pixels = vec![0_u8; size.0 as usize * size.1 as usize * 4];

        render_frame_with_margin(&mut pixels, size, params(COMPACT, 0.0, 0.0));

        let first_visible_row = (0..size.1)
            .find(|&y| (0..size.0).any(|x| pixels[((y * size.0 + x) * 4 + 3) as usize] > 240))
            .expect("rendered shape should have opaque pixels");
        assert_eq!(first_visible_row, 0);
    }

    #[test]
    fn shadow_scales_proportionally_at_150_percent_dpi() {
        fn shadow_alpha(scale: f32) -> u8 {
            let size = (384, 192);
            let scaled = crate::animation::ShapeState {
                width: COMPACT.width * scale,
                height: COMPACT.height * scale,
                radius: COMPACT.radius * scale,
                content_opacity: COMPACT.content_opacity,
            };
            let top_margin = 12.0 * scale;
            let y = (top_margin + scaled.height + 4.0 * scale).floor() as u32;
            let mut pixels = vec![0_u8; size.0 as usize * size.1 as usize * 4];

            render_frame_with_margin(
                &mut pixels,
                size,
                RenderParams {
                    state: scaled,
                    top_margin,
                    scale,
                    shadow_opacity: 0.22,
                    hover: 0.0,
                    accent: ICE,
                },
            );

            pixels[((y * size.0 + size.0 / 2) * 4 + 3) as usize]
        }

        let at_100_percent = shadow_alpha(1.0);
        let at_150_percent = shadow_alpha(1.5);

        assert!((45..=60).contains(&at_100_percent));
        assert!((45..=60).contains(&at_150_percent));
    }

    #[test]
    fn draws_the_offset_shadow_just_below_the_capsule_edge() {
        let size = (256, 128);
        let mut pixels = vec![0_u8; size.0 as usize * size.1 as usize * 4];
        let y = 70;

        render_frame_with_margin(&mut pixels, size, params(COMPACT, 12.0, 0.0));

        let alpha = pixels[((y * size.0 + size.0 / 2) * 4 + 3) as usize];
        assert!(
            alpha > 35,
            "expected a visible drop shadow, got alpha {alpha}"
        );
    }

    #[test]
    fn shadow_reach_stays_tight_below_the_shape() {
        let size = (256, 128);
        let mut pixels = vec![0_u8; size.0 as usize * size.1 as usize * 4];
        let y = 12 + 54 + 15;

        render_frame_with_margin(&mut pixels, size, params(COMPACT, 12.0, 0.0));

        let alpha = pixels[((y * size.0 + size.0 / 2) * 4 + 3) as usize];
        assert_eq!(alpha, 0, "expected no wide halo, got alpha {alpha}");
    }

    #[test]
    fn shadow_can_be_disabled() {
        let size = (256, 128);
        let mut pixels = vec![0_u8; size.0 as usize * size.1 as usize * 4];
        let y = 70;

        render_frame_with_margin(
            &mut pixels,
            size,
            RenderParams {
                state: COMPACT,
                top_margin: 12.0,
                scale: 1.0,
                shadow_opacity: 0.0,
                hover: 0.0,
                accent: ICE,
            },
        );

        let alpha = pixels[((y * size.0 + size.0 / 2) * 4 + 3) as usize];
        assert_eq!(alpha, 0);
    }

    #[test]
    fn hover_deepens_the_shadow() {
        fn alpha(hover: f32) -> u8 {
            let size = (256, 128);
            let mut pixels = vec![0_u8; size.0 as usize * size.1 as usize * 4];
            render_frame_with_margin(&mut pixels, size, params(COMPACT, 12.0, hover));
            pixels[((70 * size.0 + size.0 / 2) * 4 + 3) as usize]
        }

        assert!(alpha(1.0) > alpha(0.0));
    }

    #[test]
    fn accent_halo_only_exists_while_hovered() {
        fn alpha(hover: f32) -> u8 {
            let size = (256, 128);
            let mut pixels = vec![0_u8; size.0 as usize * size.1 as usize * 4];
            render_frame_with_margin(
                &mut pixels,
                size,
                RenderParams {
                    state: COMPACT,
                    top_margin: 12.0,
                    scale: 1.0,
                    shadow_opacity: 0.0,
                    hover,
                    accent: ICE,
                },
            );
            let x = size.0 / 2 - COMPACT.width as u32 / 2 - 4;
            pixels[((50 * size.0 + x) * 4 + 3) as usize]
        }

        assert_eq!(alpha(0.0), 0);
        assert!(alpha(1.0) > 0);
    }

    #[test]
    fn overshoot_fits_the_canvas_at_150_percent_dpi() {
        let scale = 1.5;
        let size = (768, 336);
        let state = crate::animation::ShapeState {
            width: EXPANDED.width * 1.08 * scale,
            height: EXPANDED.height * 1.08 * scale,
            radius: EXPANDED.radius * 1.08 * scale,
            content_opacity: 1.0,
        };
        let mut pixels = vec![0_u8; size.0 as usize * size.1 as usize * 4];

        render_frame_with_margin(
            &mut pixels,
            size,
            RenderParams {
                state,
                top_margin: 0.0,
                scale,
                shadow_opacity: 0.22,
                hover: 0.0,
                accent: ICE,
            },
        );

        for y in 0..size.1 {
            assert_eq!(pixels[((y * size.0) * 4 + 3) as usize], 0, "left edge {y}");
            assert_eq!(
                pixels[((y * size.0 + size.0 - 1) * 4 + 3) as usize],
                0,
                "right edge {y}"
            );
        }
        for x in 0..size.0 {
            assert_eq!(
                pixels[(((size.1 - 1) * size.0 + x) * 4 + 3) as usize],
                0,
                "bottom edge {x}"
            );
        }
    }

    #[test]
    fn ignores_a_buffer_that_is_not_large_enough() {
        let mut pixels = vec![7_u8; 8];

        render_frame(&mut pixels, (64, 32), COMPACT);

        assert_eq!(pixels, vec![7_u8; 8]);
    }
}
