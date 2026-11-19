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

#[cfg(test)]
mod tests {
    use super::rounded_rect_distance;
    use crate::animation::{COMPACT, EXPANDED};

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
