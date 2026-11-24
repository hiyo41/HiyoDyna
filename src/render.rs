use crate::animation::ShapeState;
use crate::geometry::{DistanceAxis, IslandDistance};

const SHADOW_OFFSET_Y: f32 = 3.0;
const SHADOW_SIGMA: f32 = 3.0;
const SHADOW_MAX_DISTANCE: f32 = SHADOW_SIGMA * 3.0;
const DEFAULT_SHADOW_OPACITY: f32 = 0.22;
const HALO_SIGMA: f32 = 6.0;
const HALO_MAX_DISTANCE: f32 = HALO_SIGMA * 3.0;
const HALO_OPACITY: f32 = 0.06;
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
    pub attached: bool,
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
            attached: false,
            scale: 1.0,
            shadow_opacity: DEFAULT_SHADOW_OPACITY,
            hover: 0.0,
            accent: crate::theme::ICE,
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
    let geometry = IslandDistance::new(state, params.attached);
    let extension = geometry.horizontal_extension();
    let left = (center.0 - state.width * 0.5 - extension - reach - 1.0)
        .floor()
        .max(0.0) as u32;
    let right = (center.0 + state.width * 0.5 + extension + reach + 1.0)
        .ceil()
        .max(0.0)
        .min(size.0 as f32) as u32;
    let top = (center.1 - state.height * 0.5 - reach - 1.0)
        .floor()
        .max(0.0) as u32;
    let bottom = (center.1 + state.height * 0.5 + shadow_offset + reach + 1.0)
        .ceil()
        .max(0.0)
        .min(size.1 as f32) as u32;

    let columns: Vec<_> = (left..right)
        .map(|x| geometry.column(x as f32 + 0.5, center.0))
        .collect();
    if columns.is_empty() {
        return;
    }
    let core_start = columns.iter().position(|column| column.q <= 0.0);
    let core_end = columns
        .iter()
        .rposition(|column| column.q <= 0.0)
        .map(|x| x + 1);
    let middle = columns.len() / 2;
    let center_row = geometry.row(center.1, center.1);
    let exterior =
        |column: &DistanceAxis| column.q > 0.0 && geometry.distance(*column, center_row) >= 0.5;
    let exterior_left = columns[..middle].partition_point(exterior);
    let exterior_right = middle + columns[middle..].partition_point(|column| !exterior(column));
    let mut straight_pixels = vec![0; columns.len() * 4];
    let mut straight_pixels_ready = false;
    let mirrors: Vec<_> = columns
        .iter()
        .enumerate()
        .map(|(index, column)| {
            let mirrored_x = size.0 - 1 - (left + index as u32);
            mirrored_x
                .checked_sub(left)
                .map(|x| x as usize)
                .filter(|&other| other < index && columns[other].q.to_bits() == column.q.to_bits())
        })
        .collect();
    let mut mirrorable = vec![false; columns.len()];

    for y in top..bottom {
        mirrorable.fill(false);
        let py = y as f32 + 0.5;
        let row = geometry.row(py, center.1);
        let shadow_row = geometry.row(py - shadow_offset, center.1);
        let vertical =
            ((py - (center.1 - state.height * 0.5)) / state.height.max(1.0)).clamp(0.0, 1.0);
        let lift = hover * 4.0;
        // The dithered neutral gradient is algorithmic shading, not a fixed UI palette.
        let colors: [[u8; 3]; 4] = std::array::from_fn(|x| {
            let dither = dither_offset(x as u32, y);
            [
                (9.0 - 3.0 * vertical + lift + dither)
                    .round()
                    .clamp(0.0, 255.0) as u8,
                (9.0 - 3.0 * vertical + lift + dither)
                    .round()
                    .clamp(0.0, 255.0) as u8,
                (9.0 - 3.0 * vertical + lift + dither)
                    .round()
                    .clamp(0.0, 255.0) as u8,
            ]
        });
        let rim_alpha = (0.055 + 0.025 * hover) * (1.0 - 0.65 * vertical);
        let shade = |pixel: &mut [u8], column: DistanceAxis, x: u32| {
            let distance = geometry.distance(column, row);
            if distance > 0.0 {
                if shadow_opacity > 0.0 {
                    let shadow_distance = geometry.distance(column, shadow_row);
                    if shadow_distance < shadow_max_distance {
                        // Keep the offset silhouette itself, then soften its outer edge.
                        // Clipping to the front shape above leaves only the visible cast shadow.
                        let falloff_distance = shadow_distance.max(0.0);
                        let normalized = falloff_distance / shadow_sigma;
                        let alpha = (-0.5 * normalized * normalized).exp() * shadow_opacity;
                        // Pure black is the exact cast-shadow compositing endpoint.
                        blend_pixel(pixel, [0, 0, 0], alpha);
                    }
                }

                if hover > 0.0 && distance < halo_max_distance {
                    let normalized = distance / (HALO_SIGMA * scale);
                    let alpha = (-0.5 * normalized * normalized).exp() * HALO_OPACITY * hover;
                    blend_pixel(pixel, params.accent, alpha);
                }
            }

            let coverage = (0.5 - distance).clamp(0.0, 1.0);
            if coverage > 0.0 {
                blend_pixel(pixel, colors[(x % 4) as usize], coverage);

                let edge = (1.0 - (distance + 0.55).abs() / 0.65).clamp(0.0, 1.0);
                if edge > 0.0 {
                    // Pure white is the exact algorithmic rim-light compositing endpoint.
                    blend_pixel(pixel, [255, 255, 255], edge * coverage * rim_alpha);
                }
            }
            coverage <= 0.0
        };

        // The rim ends at -1.2. A conservative -1.25 threshold also guarantees full coverage.
        let opaque = |column: &DistanceAxis| geometry.distance(*column, row) <= -1.25;
        let span = if opaque(&columns[middle]) {
            let start = columns[..middle].partition_point(|column| !opaque(column));
            let end = middle + columns[middle..].partition_point(opaque);
            Some((start, end, true))
        } else if row.q >= 0.0
            && (shadow_row.q >= 0.0
                || shadow_opacity == 0.0
                || geometry.distance(columns[middle], row) <= 0.0)
        {
            core_start
                .zip(core_end)
                .map(|(start, end)| (start, end, false))
        } else {
            None
        };
        let row_start = ((y * size.0 + left) * 4) as usize;
        let output = &mut pixels[row_start..row_start + columns.len() * 4];
        let (start, end, opaque) = span.unwrap_or((columns.len(), columns.len(), false));
        // In this band both silhouettes have identical column distances. Exterior pixels
        // have no gradient or rim, so their fully blended shadow/halo bytes repeat exactly.
        let (copy_left, copy_right) = if row.q <= 0.0 && shadow_row.q <= 0.0 {
            if !straight_pixels_ready {
                for index in (0..exterior_left).chain(exterior_right..columns.len()) {
                    shade(
                        &mut straight_pixels[index * 4..index * 4 + 4],
                        columns[index],
                        left + index as u32,
                    );
                }
                straight_pixels_ready = true;
            }
            output[..exterior_left * 4].copy_from_slice(&straight_pixels[..exterior_left * 4]);
            output[exterior_right * 4..].copy_from_slice(&straight_pixels[exterior_right * 4..]);
            (exterior_left, exterior_right)
        } else {
            (0, columns.len())
        };
        for (offset, column) in columns[copy_left..start.min(copy_right)].iter().enumerate() {
            let index = copy_left + offset;
            mirrorable[index] = shade(
                &mut output[index * 4..index * 4 + 4],
                *column,
                left + index as u32,
            );
        }
        if start < end {
            let first_x = left + start as u32;
            let mut pattern = [0; 16];
            for (offset, pixel) in pattern.as_chunks_mut::<4>().0.iter_mut().enumerate() {
                let x = first_x + offset as u32;
                if opaque {
                    pixel[..3].copy_from_slice(&colors[(x % 4) as usize]);
                    pixel[3] = 255;
                } else {
                    shade(pixel, columns[start], x);
                }
            }
            fill_pattern(&mut output[start * 4..end * 4], &pattern);
        }
        let right_start = end.max(copy_left).min(copy_right);
        for (offset, column) in columns[right_start..copy_right].iter().enumerate() {
            let index = right_start + offset;
            if let Some(other) = mirrors[index].filter(|&other| mirrorable[other]) {
                // Only zero-coverage pixels are symmetric: shape colors retain their Bayer phase.
                output.copy_within(other * 4..other * 4 + 4, index * 4);
            } else {
                shade(
                    &mut output[index * 4..index * 4 + 4],
                    *column,
                    left + index as u32,
                );
            }
        }
    }
}

fn fill_pattern(pixels: &mut [u8], pattern: &[u8; 16]) {
    let (chunks, remainder) = pixels.as_chunks_mut::<16>();
    for chunk in chunks {
        chunk.copy_from_slice(pattern);
    }
    remainder.copy_from_slice(&pattern[..remainder.len()]);
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

    use crate::theme::ICE;

    #[test]
    fn attached_top_has_continuous_body_and_outward_concave_shoulders_at_each_dpi() {
        for scale in [1.0, 1.25, 1.5, 2.0] {
            for base in [COMPACT, EXPANDED] {
                let state = crate::animation::ShapeState {
                    width: base.width * scale,
                    height: base.height * scale,
                    radius: base.radius * scale,
                };
                let size = ((512.0 * scale) as u32, (592.0 * scale) as u32);
                let mut pixels = vec![0; size.0 as usize * size.1 as usize * 4];
                let mut options = params(state, 0.0, 0.0);
                options.attached = true;
                options.scale = scale;
                options.shadow_opacity = 0.0;
                render_frame_with_margin(&mut pixels, size, options);
                let alpha = |x: u32, y: u32| pixels[((y * size.0 + x) * 4 + 3) as usize];
                let left = (size.0 as f32 * 0.5 - state.width * 0.5).ceil() as u32;
                let right = (size.0 as f32 * 0.5 + state.width * 0.5).floor() as u32;
                for x in left..right {
                    assert_eq!(
                        alpha(x, 0),
                        255,
                        "top continuity at {scale}x, {base:?}, x={x}"
                    );
                }
                assert!(
                    alpha(right + scale as u32, 0) > 0,
                    "outward shoulder at {scale}x"
                );
                assert_eq!(alpha(right + (13.0 * scale) as u32, 0), 0);
                assert_eq!(alpha(right + scale as u32, (12.0 * scale) as u32), 0);
                assert_eq!(
                    alpha(right - 1, state.height as u32 - 1),
                    0,
                    "rounded bottom"
                );
            }
        }
    }

    #[test]
    fn rendered_body_and_hover_share_the_attached_and_detached_silhouettes() {
        for scale in [1.0, 1.25, 1.5, 2.0] {
            for base in [
                COMPACT,
                EXPANDED,
                crate::animation::ShapeState {
                    width: 360.0,
                    ..COMPACT
                },
            ] {
                let state = crate::animation::ShapeState {
                    width: base.width * scale,
                    height: base.height * scale,
                    radius: base.radius * scale,
                };
                let size = ((512.0 * scale) as u32, (224.0 * scale) as u32);
                for (attached, margin) in [(true, 0.0), (false, 0.0), (false, 12.0 * scale)] {
                    let mut options = params(state, margin, 0.0);
                    options.attached = attached;
                    options.scale = scale;
                    options.shadow_opacity = 0.0;
                    let mut pixels = vec![0; size.0 as usize * size.1 as usize * 4];
                    render_frame_with_margin(&mut pixels, size, options);
                    for y in 0..size.1 {
                        for x in 0..size.0 {
                            let alpha = pixels[((y * size.0 + x) * 4 + 3) as usize];
                            let hit = crate::hover::pointer_inside_shape(
                                size,
                                state,
                                margin,
                                x as f32 + 0.5,
                                y as f32 + 0.5,
                                attached,
                            );
                            // Sub-byte coverage can round to transparent; all visible
                            // body pixels must hit, and every hit must have coverage.
                            assert!(
                                alpha == 0 || hit,
                                "visible pixel misses: {options:?}, ({x}, {y})"
                            );
                            if hit && alpha == 0 {
                                let center = (size.0 as f32 * 0.5, margin + state.height * 0.5);
                                let distance = crate::geometry::island_distance(
                                    x as f32 + 0.5,
                                    y as f32 + 0.5,
                                    state,
                                    center,
                                    attached,
                                );
                                assert!(
                                    distance > 0.498,
                                    "transparent interior hit: {options:?}, ({x}, {y})"
                                );
                            }
                        }
                    }
                    let edge_x = (size.0 as f32 * 0.5 + state.width * 0.5 - 1.0) as u32;
                    let top_y = margin as u32;
                    let alpha = pixels[((top_y * size.0 + edge_x) * 4 + 3) as usize];
                    assert_eq!(
                        alpha == 255,
                        attached,
                        "explicit attachment controls top corners"
                    );
                }
            }
        }
    }

    // Frozen scalar renderer: keep its arithmetic independent of the optimized paths.
    fn reference_frame(pixels: &mut [u8], size: (u32, u32), params: RenderParams) {
        let state = params.state;
        let scale = params.scale;
        let hover = params.hover.clamp(0.0, 1.0);
        let center = (size.0 as f32 * 0.5, params.top_margin + state.height * 0.5);
        let shadow_offset = (super::SHADOW_OFFSET_Y + 1.5 * hover) * scale;
        let shadow_sigma = (super::SHADOW_SIGMA + 1.5 * hover) * scale;
        let shadow_max_distance = (super::SHADOW_MAX_DISTANCE + 4.5 * hover) * scale;
        let shadow_opacity = (params.shadow_opacity * (1.0 + 0.36 * hover)).clamp(0.0, 1.0);
        let halo_max_distance = super::HALO_MAX_DISTANCE * scale;
        let reach = shadow_max_distance.max(halo_max_distance);
        let radius = state
            .radius
            .max(0.0)
            .min(state.width.max(0.0).min(state.height.max(0.0)) * 0.5);
        let extension = if params.attached { radius * 0.4 } else { 0.0 };
        let left = (center.0 - state.width * 0.5 - extension - reach - 1.0)
            .floor()
            .max(0.0) as u32;
        let right = (center.0 + state.width * 0.5 + extension + reach + 1.0)
            .ceil()
            .min(size.0 as f32) as u32;
        let top = (center.1 - state.height * 0.5 - reach - 1.0)
            .floor()
            .max(0.0) as u32;
        let bottom = (center.1 + state.height * 0.5 + shadow_offset + reach + 1.0)
            .ceil()
            .min(size.1 as f32) as u32;
        pixels.fill(0);
        for y in top..bottom {
            for x in left..right {
                let px = x as f32 + 0.5;
                let py = y as f32 + 0.5;
                let index = ((y * size.0 + x) * 4) as usize;
                let distance = reference_distance(px, py, state, center, params.attached);
                if distance > 0.0 {
                    if shadow_opacity > 0.0 {
                        let shadow_distance = reference_distance(
                            px,
                            py - shadow_offset,
                            state,
                            center,
                            params.attached,
                        );
                        if shadow_distance < shadow_max_distance {
                            let normalized = shadow_distance.max(0.0) / shadow_sigma;
                            let alpha = (-0.5 * normalized * normalized).exp() * shadow_opacity;
                            // Frozen scalar fixture retains the exact black shadow endpoint.
                            super::blend_pixel(&mut pixels[index..index + 4], [0, 0, 0], alpha);
                        }
                    }
                    if hover > 0.0 && distance < halo_max_distance {
                        let normalized = distance / (super::HALO_SIGMA * scale);
                        let alpha =
                            (-0.5 * normalized * normalized).exp() * super::HALO_OPACITY * hover;
                        super::blend_pixel(&mut pixels[index..index + 4], params.accent, alpha);
                    }
                }
                let coverage = (0.5 - distance).clamp(0.0, 1.0);
                if coverage > 0.0 {
                    let vertical = ((py - (center.1 - state.height * 0.5)) / state.height.max(1.0))
                        .clamp(0.0, 1.0);
                    let lift = hover * 4.0;
                    let dither = super::BAYER_4X4[(y % 4) as usize][(x % 4) as usize] / 16.0 - 0.5;
                    // Frozen gradient arithmetic checks optimized shading byte for byte.
                    let blue = (9.0 - 3.0 * vertical + lift + dither).round();
                    let green = (9.0 - 3.0 * vertical + lift + dither).round();
                    let red = (9.0 - 3.0 * vertical + lift + dither).round();
                    super::blend_pixel(
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
                        let rim_alpha = (0.055 + 0.025 * hover) * top_weight;
                        // Frozen scalar fixture retains the exact white rim-light endpoint.
                        super::blend_pixel(
                            &mut pixels[index..index + 4],
                            [255, 255, 255],
                            edge * coverage * rim_alpha,
                        );
                    }
                }
            }
        }
    }

    fn reference_distance(
        x: f32,
        y: f32,
        state: crate::animation::ShapeState,
        center: (f32, f32),
        attached: bool,
    ) -> f32 {
        let half_width = state.width.max(0.0) * 0.5;
        let half_height = state.height.max(0.0) * 0.5;
        let radius = state.radius.max(0.0).min(half_width.min(half_height));
        let qx = (x - center.0).abs() - (half_width - radius);
        let qy =
            (if attached { y.max(center.1) } else { y } - center.1).abs() - (half_height - radius);
        let outside = (qx.max(0.0).powi(3) + qy.max(0.0).powi(3)).powf(1.0 / 3.0);
        let rounded = outside + qx.max(qy).min(0.0) - radius;
        if !attached {
            return rounded;
        }
        let top = y - (center.1 - half_height);
        let shoulder = radius * 0.4;
        let side = if top < shoulder {
            let dx = qx - radius - shoulder;
            let dy = top - shoulder;
            if dx > 0.0 {
                (dx * dx + top * top).sqrt()
            } else {
                shoulder - (dx * dx + dy * dy).sqrt()
            }
        } else {
            rounded
        };
        side.max(-top)
    }

    #[test]
    fn matches_scalar_reference_for_animated_shapes_dpi_and_hover() {
        use crate::animation::ShapeState;
        let states = [
            COMPACT,
            EXPANDED,
            ShapeState {
                width: 420.0,
                height: 500.0,
                radius: 30.0,
            },
            ShapeState {
                width: 373.37,
                height: 317.73,
                radius: 28.91,
            },
            ShapeState {
                width: 453.6,
                height: 540.0,
                radius: 32.4,
            },
            ShapeState {
                width: 93.7,
                height: 47.3,
                radius: 100.0,
            },
            ShapeState {
                width: 31.25,
                height: 12.75,
                radius: 0.0,
            },
        ];
        for scale in [1.0, 1.25, 1.5, 2.0] {
            let size = ((512.0 * scale) as u32 + 1, (592.0 * scale) as u32 + 1);
            let mut actual = vec![91; size.0 as usize * size.1 as usize * 4];
            let mut expected = actual.clone();
            for state in states {
                for (hover, shadow_opacity, top_margin, attached) in [
                    (0.0, 0.22, 12.0, false),
                    (0.37, 0.0, 0.0, false),
                    (1.0, 0.47, -3.25, false),
                    (0.0, 0.22, 0.0, true),
                    (0.37, 0.0, 0.0, true),
                    (1.0, 0.47, -3.25, true),
                ] {
                    let mut params = params(
                        ShapeState {
                            width: state.width * scale,
                            height: state.height * scale,
                            radius: state.radius * scale,
                        },
                        top_margin * scale,
                        hover,
                    );
                    params.scale = scale;
                    params.attached = attached;
                    params.shadow_opacity = shadow_opacity;
                    // Asymmetric fixture channels expose color-order bugs in the renderer.
                    params.accent = [31, 203, 117];
                    render_frame_with_margin(&mut actual, size, params);
                    reference_frame(&mut expected, size, params);
                    if let Some(index) = actual.iter().zip(&expected).position(|(a, b)| a != b) {
                        panic!(
                            "pixel ({}, {}), channel {}: {} != {}; {params:?}",
                            index / 4 % size.0 as usize,
                            index / 4 / size.0 as usize,
                            index % 4,
                            actual[index],
                            expected[index]
                        );
                    }
                }
            }
        }
    }

    #[test]
    fn matches_scalar_reference_for_clipped_and_degenerate_shapes() {
        use crate::animation::ShapeState;
        for size in [(1, 1), (13, 11), (127, 93), (128, 94)] {
            for (width, height, radius, margin) in [
                (0.0, 0.0, 0.0, 0.0),
                (90.75, 0.0, 0.0, 12.75),
                (0.0, 44.25, 40.0, -4.5),
                (109.75, 67.25, 0.125, -13.25),
                (109.75, 67.25, 1.0, 0.125),
                (90.75, 44.25, 100.0, -7.75),
                (270.75, 144.25, 14.0, -37.75),
                (11.75, 1.75, 0.5, 4.125),
            ] {
                for (hover, attached) in [
                    (-0.25, false),
                    (0.125, false),
                    (1.25, false),
                    (-0.25, true),
                    (0.125, true),
                    (1.25, true),
                ] {
                    let params = RenderParams {
                        state: ShapeState {
                            width,
                            height,
                            radius,
                        },
                        top_margin: margin,
                        attached,
                        scale: 1.5,
                        shadow_opacity: 0.22,
                        hover,
                        // A second asymmetric fixture exercises clipping and blend rounding.
                        accent: [149, 19, 251],
                    };
                    let mut actual = vec![91; size.0 as usize * size.1 as usize * 4];
                    let mut expected = actual.clone();
                    render_frame_with_margin(&mut actual, size, params);
                    reference_frame(&mut expected, size, params);
                    assert!(actual == expected, "{size:?}: {params:?}");
                }
            }
        }
    }

    #[test]
    #[ignore = "release-mode background benchmark; run with --release --ignored --nocapture"]
    fn benchmark_animated_backgrounds() {
        use std::{hint::black_box, time::Instant};
        for (name, height, attached) in [
            ("clock", 150.0, false),
            ("player", 500.0, false),
            ("attached clock", 150.0, true),
            ("attached player", 500.0, true),
        ] {
            for scale in [1.0, 1.25, 1.5, 2.0] {
                let size = ((512.0 * scale) as u32, (592.0 * scale) as u32);
                let frames: Vec<_> = [0.72, 0.85, 0.93, 0.98, 1.0, 1.04, 1.08, 1.02, 1.0]
                    .into_iter()
                    .map(|factor| RenderParams {
                        state: crate::animation::ShapeState {
                            width: 420.0 * factor * scale,
                            height: height * factor * scale,
                            radius: 30.0 * factor * scale,
                        },
                        scale,
                        top_margin: if attached { 0.0 } else { 12.0 * scale },
                        attached,
                        hover: 0.5,
                        shadow_opacity: 0.22,
                        accent: ICE,
                    })
                    .collect();
                let mut pixels = vec![0; size.0 as usize * size.1 as usize * 4];
                let mut timings = [0.0; 2];
                for (slot, render) in [reference_frame, render_frame_with_margin]
                    .into_iter()
                    .enumerate()
                {
                    render(&mut pixels, size, frames[0]);
                    let start = Instant::now();
                    for &params in &frames {
                        render(black_box(&mut pixels), black_box(size), black_box(params));
                        black_box(&pixels);
                    }
                    timings[slot] = start.elapsed().as_secs_f64() * 1000.0 / frames.len() as f64;
                }
                eprintln!(
                    "{name} {scale}x: scalar {:.3} ms/frame, optimized {:.3} ms/frame ({:.1}x)",
                    timings[0],
                    timings[1],
                    timings[0] / timings[1]
                );
                if name.ends_with("player") && scale == 2.0 {
                    assert!(
                        timings[1] < timings[0] / 30.0,
                        "large animated background must be at least thirty times faster than scalar rendering"
                    );
                }
            }
        }
    }

    fn params(state: crate::animation::ShapeState, top_margin: f32, hover: f32) -> RenderParams {
        RenderParams {
            state,
            top_margin,
            attached: false,
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
                    attached: false,
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
        let y = 12 + COMPACT.height as u32 + 4;

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
        let y = 12 + COMPACT.height as u32 + 15;

        render_frame_with_margin(&mut pixels, size, params(COMPACT, 12.0, 0.0));

        let alpha = pixels[((y * size.0 + size.0 / 2) * 4 + 3) as usize];
        assert_eq!(alpha, 0, "expected no wide halo, got alpha {alpha}");
    }

    #[test]
    fn shadow_can_be_disabled() {
        let size = (256, 128);
        let mut pixels = vec![0_u8; size.0 as usize * size.1 as usize * 4];
        let y = 12 + COMPACT.height as u32 + 4;

        render_frame_with_margin(
            &mut pixels,
            size,
            RenderParams {
                state: COMPACT,
                top_margin: 12.0,
                attached: false,
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
            let y = 12 + COMPACT.height as u32 + 4;
            pixels[((y * size.0 + size.0 / 2) * 4 + 3) as usize]
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
                    attached: false,
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
        };
        let mut pixels = vec![0_u8; size.0 as usize * size.1 as usize * 4];

        render_frame_with_margin(
            &mut pixels,
            size,
            RenderParams {
                state,
                top_margin: 0.0,
                attached: false,
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
