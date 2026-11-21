use crate::animation::ShapeState;

pub(crate) fn rounded_rect_distance(x: f32, y: f32, state: ShapeState, center: (f32, f32)) -> f32 {
    let half_width = state.width.max(0.0) * 0.5;
    let half_height = state.height.max(0.0) * 0.5;
    let radius = state.radius.max(0.0).min(half_width.min(half_height));
    let qx = (x - center.0).abs() - (half_width - radius);
    let qy = (y - center.1).abs() - (half_height - radius);
    let outside = (qx.max(0.0).powi(3) + qy.max(0.0).powi(3)).powf(1.0 / 3.0);
    let inside = qx.max(qy).min(0.0);

    outside + inside - radius
}

pub(crate) fn island_distance(
    x: f32,
    y: f32,
    state: ShapeState,
    center: (f32, f32),
    attached: bool,
) -> f32 {
    if !attached {
        return rounded_rect_distance(x, y, state, center);
    }
    let geometry = IslandDistance::new(state, true);
    geometry.distance(geometry.column(x, center.0), geometry.row(y, center.1))
}

/// Keeps prepared rounded-rectangle axes and limits circular shoulder work to the top band.
pub(crate) struct IslandDistance {
    rounded: RoundedRectDistance,
    half_height: f32,
    attached: bool,
    shoulder: f32,
}

#[derive(Clone, Copy)]
pub(crate) struct IslandRow {
    pub q: f32,
    rounded: DistanceAxis,
    top: f32,
}

impl IslandDistance {
    pub fn new(state: ShapeState, attached: bool) -> Self {
        let rounded = RoundedRectDistance::new(state);
        // 8 logical pixels for compact, 12 for expanded; scales with the shape's DPI.
        let shoulder = if attached { rounded.radius * 0.4 } else { 0.0 };
        Self {
            rounded,
            half_height: state.height.max(0.0) * 0.5,
            attached,
            shoulder,
        }
    }

    pub fn horizontal_extension(&self) -> f32 {
        self.shoulder
    }

    pub fn column(&self, x: f32, center: f32) -> DistanceAxis {
        self.rounded.column(x, center)
    }

    pub fn row(&self, y: f32, center: f32) -> IslandRow {
        let rounded = self
            .rounded
            .row(if self.attached { y.max(center) } else { y }, center);
        let top = y - (center - self.half_height);
        IslandRow {
            // Shoulder rows cannot use the renderer's repeating straight-side shadow cache.
            q: if self.attached && top < self.shoulder {
                1.0
            } else {
                rounded.q
            },
            rounded,
            top,
        }
    }

    pub fn distance(&self, column: DistanceAxis, row: IslandRow) -> f32 {
        if !self.attached {
            return self.rounded.distance(column, row.rounded);
        }
        let side = if row.top < self.shoulder {
            let side = column.q - self.rounded.radius;
            let dx = side - self.shoulder;
            let dy = row.top - self.shoulder;
            // Keep the inner arc of a circle centered outside the body, making the
            // shoulder concave. Beyond its endpoint, measure to the top corner.
            if dx > 0.0 {
                (dx * dx + row.top * row.top).sqrt()
            } else {
                self.shoulder - (dx * dx + dy * dy).sqrt()
            }
        } else {
            self.rounded.distance(column, row.rounded)
        };
        side.max(-row.top)
    }
}

/// Separates the original distance formula into reusable row and column terms.
pub(crate) struct RoundedRectDistance {
    half_core_width: f32,
    half_core_height: f32,
    radius: f32,
}

#[derive(Clone, Copy)]
pub(crate) struct DistanceAxis {
    pub q: f32,
    cube: f32,
    outside: f32,
}

impl DistanceAxis {
    fn new(q: f32) -> Self {
        let cube = q.max(0.0).powi(3);
        Self {
            q,
            cube,
            // Keep powf rather than substituting q: its f32 rounding affects edge pixels.
            outside: if q > 0.0 { cube.powf(1.0 / 3.0) } else { 0.0 },
        }
    }
}

impl RoundedRectDistance {
    pub fn new(state: ShapeState) -> Self {
        let half_width = state.width.max(0.0) * 0.5;
        let half_height = state.height.max(0.0) * 0.5;
        let radius = state.radius.max(0.0).min(half_width.min(half_height));
        Self {
            half_core_width: half_width - radius,
            half_core_height: half_height - radius,
            radius,
        }
    }

    pub fn column(&self, x: f32, center: f32) -> DistanceAxis {
        DistanceAxis::new((x - center).abs() - self.half_core_width)
    }

    pub fn row(&self, y: f32, center: f32) -> DistanceAxis {
        DistanceAxis::new((y - center).abs() - self.half_core_height)
    }

    pub fn distance(&self, column: DistanceAxis, row: DistanceAxis) -> f32 {
        let outside = if column.q <= 0.0 {
            row.outside
        } else if row.q <= 0.0 {
            column.outside
        } else {
            (column.cube + row.cube).powf(1.0 / 3.0)
        };
        outside + column.q.max(row.q).min(0.0) - self.radius
    }
}

#[cfg(test)]
mod tests {
    use super::{island_distance, rounded_rect_distance};
    use crate::animation::{COMPACT, EXPANDED, ShapeState};

    #[test]
    fn attached_shoulders_are_wider_without_changing_detached_bounds() {
        for scale in [1.0, 1.25, 1.5, 2.0] {
            for base in [COMPACT, EXPANDED] {
                let state = ShapeState {
                    width: base.width * scale,
                    height: base.height * scale,
                    radius: base.radius * scale,
                };
                assert!(
                    (super::IslandDistance::new(state, true).horizontal_extension()
                        - base.radius * scale * 0.4)
                        .abs()
                        < 0.001
                );
                assert_eq!(
                    super::IslandDistance::new(state, false).horizontal_extension(),
                    0.0
                );
            }
        }
    }

    #[test]
    fn attached_shoulders_are_outward_concave_bounded_and_symmetric() {
        for scale in [1.0, 1.25, 1.5, 2.0] {
            for base in [
                COMPACT,
                EXPANDED,
                ShapeState {
                    width: 360.0,
                    ..COMPACT
                },
            ] {
                let state = ShapeState {
                    width: base.width * scale,
                    height: base.height * scale,
                    radius: base.radius * scale,
                };
                let center = (300.0 * scale, state.height * 0.5);
                let shoulder = base.radius * 0.4 * scale;
                for direction in [-1.0, 1.0] {
                    let x = center.0 + direction * (state.width * 0.5 + shoulder * 0.5);
                    assert!(island_distance(x, shoulder * 0.1, state, center, true) < 0.0);
                    // A convex corner or a straight bevel would still fill this point.
                    assert!(island_distance(x, shoulder * 0.5, state, center, true) > 0.0);
                    assert!(island_distance(x, shoulder, state, center, true) > 0.0);
                    let limit = center.0 + direction * (state.width * 0.5 + 14.0 * scale);
                    assert!(island_distance(limit, 0.0, state, center, true) > 0.0);
                }
                assert_eq!(island_distance(center.0, 0.0, state, center, true), 0.0);
                assert!(island_distance(center.0, -scale, state, center, true) > 0.0);
                assert!(island_distance(center.0, state.height + scale, state, center, true) > 0.0);
            }
        }
    }

    #[test]
    fn detached_distance_and_attached_lower_half_preserve_rounded_geometry() {
        for state in [COMPACT, EXPANDED] {
            let center = (217.25, 12.0 + state.height * 0.5);
            for y in -20..200 {
                for x in -20..460 {
                    let (x, y) = (x as f32 + 0.25, y as f32 + 0.75);
                    let expected = rounded_rect_distance(x, y, state, center);
                    assert_eq!(
                        island_distance(x, y, state, center, false).to_bits(),
                        expected.to_bits()
                    );
                    if y >= center.1 {
                        assert_eq!(
                            island_distance(x, y, state, center, true).to_bits(),
                            expected.to_bits()
                        );
                    }
                }
            }
        }
    }

    #[test]
    fn prepared_axes_preserve_scalar_distance_bits() {
        for state in [
            COMPACT,
            EXPANDED,
            crate::animation::ShapeState {
                width: 747.37,
                height: 635.73,
                radius: 57.91,
            },
            crate::animation::ShapeState {
                width: 31.25,
                height: 12.75,
                radius: 0.0,
            },
        ] {
            let prepared = super::RoundedRectDistance::new(state);
            let center = (17.25, -3.75);
            for y in -90..90 {
                let y = y as f32 * 4.25;
                let row = prepared.row(y, center.1);
                for x in -100..100 {
                    let x = x as f32 * 4.75;
                    let actual = prepared.distance(prepared.column(x, center.0), row);
                    let expected = rounded_rect_distance(x, y, state, center);
                    assert_eq!(
                        actual.to_bits(),
                        expected.to_bits(),
                        "{state:?} at ({x}, {y})"
                    );
                }
            }
        }
    }

    #[test]
    fn capsule_distance_is_inside_at_center_and_outside_past_edge() {
        assert!(rounded_rect_distance(0.0, 0.0, COMPACT, (0.0, 0.0)) < 0.0);
        assert!(rounded_rect_distance(93.0, 0.0, COMPACT, (0.0, 0.0)) > 0.0);
    }

    #[test]
    fn expanded_panel_distance_is_negative_inside_and_near_zero_at_edge() {
        assert!(rounded_rect_distance(0.0, 0.0, EXPANDED, (0.0, 0.0)) < 0.0);
        assert!(rounded_rect_distance(210.0, 0.0, EXPANDED, (0.0, 0.0)).abs() < 0.01);
    }

    #[test]
    fn distance_is_symmetric_around_the_shape_center() {
        let left = rounded_rect_distance(-70.0, 12.0, EXPANDED, (0.0, 0.0));
        let right = rounded_rect_distance(70.0, 12.0, EXPANDED, (0.0, 0.0));

        assert!((left - right).abs() < 0.001);
    }

    #[test]
    fn corner_is_fuller_than_a_circle() {
        let state = EXPANDED;
        let half_w = state.width * 0.5;
        let half_h = state.height * 0.5;
        let r = state.radius;
        let qx = 22.0;
        let qy = 22.0;
        let x = (half_w - r) + qx;
        let y = (half_h - r) + qy;
        let circle = (qx * qx + qy * qy).sqrt() - r;

        assert!(circle > 0.0);
        assert!(rounded_rect_distance(x, y, state, (0.0, 0.0)) < 0.0);
    }
}
