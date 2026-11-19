use std::f32::consts::PI;

const EXPAND_DAMPING: f32 = 0.55;
const EXPAND_RESPONSE_SECONDS: f32 = 0.28;
const COLLAPSE_DAMPING: f32 = 0.68;
const COLLAPSE_RESPONSE_SECONDS: f32 = 0.30;
pub(crate) const OVERSHOOT_MIN: f32 = 0.92;
pub(crate) const OVERSHOOT_MAX: f32 = 1.08;
const MAX_FRAME_DELTA_SECONDS: f32 = 0.1;
const MAX_SUBSTEP_SECONDS: f32 = 1.0 / 240.0;

#[derive(Clone, Copy, Debug, PartialEq)]
pub(crate) struct SpringTuning {
    pub expand_response_seconds: f32,
    pub expand_damping: f32,
    pub collapse_response_seconds: f32,
    pub collapse_damping: f32,
}

impl Default for SpringTuning {
    fn default() -> Self {
        Self {
            expand_response_seconds: EXPAND_RESPONSE_SECONDS,
            expand_damping: EXPAND_DAMPING,
            collapse_response_seconds: COLLAPSE_RESPONSE_SECONDS,
            collapse_damping: COLLAPSE_DAMPING,
        }
    }
}

pub(crate) const COMPACT: ShapeState = ShapeState {
    width: 184.0,
    height: 54.0,
    radius: 27.0,
    content_opacity: 0.0,
};

pub(crate) const EXPANDED: ShapeState = ShapeState {
    width: 420.0,
    height: 150.0,
    radius: 30.0,
    content_opacity: 1.0,
};

#[derive(Clone, Copy, Debug, PartialEq)]
pub(crate) struct ShapeState {
    pub width: f32,
    pub height: f32,
    pub radius: f32,
    pub content_opacity: f32,
}

#[derive(Clone, Copy, Debug, Default, PartialEq)]
pub(crate) struct ShapeVelocity {
    pub width: f32,
    pub height: f32,
    pub radius: f32,
    pub content_opacity: f32,
}

pub(crate) struct Animation {
    pub current: ShapeState,
    pub velocity: ShapeVelocity,
    pub target: ShapeState,
    tuning: SpringTuning,
}

impl Animation {
    #[cfg(test)]
    pub fn new() -> Self {
        Self::new_with_tuning(SpringTuning::default())
    }

    pub fn new_with_tuning(tuning: SpringTuning) -> Self {
        Self {
            current: COMPACT,
            velocity: ShapeVelocity::default(),
            target: COMPACT,
            tuning,
        }
    }

    pub fn set_tuning(&mut self, tuning: SpringTuning) {
        self.tuning = tuning;
    }

    pub fn expand(&mut self) {
        self.target = EXPANDED;
    }

    pub fn collapse(&mut self) {
        self.target = COMPACT;
    }

    /// Advances the spring by elapsed wall-clock time and returns whether it is moving.
    pub fn advance(&mut self, dt_seconds: f32) -> bool {
        if !dt_seconds.is_finite() || dt_seconds <= 0.0 {
            return self.is_active();
        }

        let (damping, response) = if self.target == EXPANDED {
            (
                self.tuning.expand_damping,
                self.tuning.expand_response_seconds,
            )
        } else {
            (
                self.tuning.collapse_damping,
                self.tuning.collapse_response_seconds,
            )
        };
        let omega = 2.0 * PI / response;

        let mut remaining = dt_seconds.min(MAX_FRAME_DELTA_SECONDS);
        while remaining > 0.0 {
            let step = remaining.min(MAX_SUBSTEP_SECONDS);
            self.integrate_axis(step, 0, omega, damping);
            self.integrate_axis(step, 1, omega, damping);
            self.integrate_axis(step, 2, omega, damping);
            self.integrate_axis(step, 3, omega, damping);
            remaining -= step;
        }

        if self.is_settled() {
            self.current = self.target;
            self.velocity = ShapeVelocity::default();
            false
        } else {
            true
        }
    }

    fn integrate_axis(&mut self, dt: f32, axis: usize, omega: f32, damping: f32) {
        let current_height = self.current.height;
        let (position, velocity, target, min, max) = match axis {
            0 => (
                &mut self.current.width,
                &mut self.velocity.width,
                self.target.width,
                COMPACT.width * OVERSHOOT_MIN,
                EXPANDED.width * OVERSHOOT_MAX,
            ),
            1 => (
                &mut self.current.height,
                &mut self.velocity.height,
                self.target.height,
                COMPACT.height * OVERSHOOT_MIN,
                EXPANDED.height * OVERSHOOT_MAX,
            ),
            2 => (
                &mut self.current.radius,
                &mut self.velocity.radius,
                self.target.radius,
                COMPACT.radius * OVERSHOOT_MIN,
                EXPANDED.radius * OVERSHOOT_MAX,
            ),
            _ => (
                &mut self.current.content_opacity,
                &mut self.velocity.content_opacity,
                self.target.content_opacity,
                0.0,
                1.0,
            ),
        };

        let acceleration =
            -2.0 * damping * omega * *velocity - omega * omega * (*position - target);
        *velocity += acceleration * dt;
        *position += *velocity * dt;

        if *position < min {
            *position = min;
            if *velocity < 0.0 {
                *velocity = 0.0;
            }
        } else if *position > max {
            *position = max;
            if *velocity > 0.0 {
                *velocity = 0.0;
            }
        }

        if axis == 2 {
            let max_radius = (current_height / 2.0).min(max);
            if *position > max_radius {
                *position = max_radius;
                if *velocity > 0.0 {
                    *velocity = 0.0;
                }
            }
        }
    }

    fn is_active(&self) -> bool {
        !self.is_settled()
    }

    fn is_settled(&self) -> bool {
        const POSITION_EPSILON: f32 = 0.03;
        const VELOCITY_EPSILON: f32 = 0.05;

        (self.current.width - self.target.width).abs() < POSITION_EPSILON
            && (self.current.height - self.target.height).abs() < POSITION_EPSILON
            && (self.current.radius - self.target.radius).abs() < POSITION_EPSILON
            && (self.current.content_opacity - self.target.content_opacity).abs() < POSITION_EPSILON
            && self.velocity.width.abs() < VELOCITY_EPSILON
            && self.velocity.height.abs() < VELOCITY_EPSILON
            && self.velocity.radius.abs() < VELOCITY_EPSILON
            && self.velocity.content_opacity.abs() < VELOCITY_EPSILON
    }
}

#[cfg(test)]
mod tests {
    use super::{Animation, COMPACT, EXPANDED, ShapeVelocity, SpringTuning};

    #[test]
    fn starts_in_the_compact_capsule_state() {
        let animation = Animation::new();

        assert_eq!(animation.current, COMPACT);
        assert_eq!(animation.target, COMPACT);
    }

    #[test]
    fn custom_tuning_changes_the_response_speed() {
        let mut default = Animation::new();
        let mut fast = Animation::new_with_tuning(SpringTuning {
            expand_response_seconds: 0.12,
            ..SpringTuning::default()
        });
        default.expand();
        fast.expand();
        default.advance(0.08);
        fast.advance(0.08);

        assert!(fast.current.width > default.current.width);
    }

    #[test]
    fn spring_advancement_is_consistent_at_30fps_and_120fps() {
        let mut at_30_fps = Animation::new();
        let mut at_120_fps = Animation::new();
        at_30_fps.expand();
        at_120_fps.expand();

        at_30_fps.advance(1.0 / 30.0);
        for _ in 0..4 {
            at_120_fps.advance(1.0 / 120.0);
        }

        assert!((at_30_fps.current.width - at_120_fps.current.width).abs() < 0.01);
        assert!((at_30_fps.current.height - at_120_fps.current.height).abs() < 0.01);
        assert!(
            (at_30_fps.current.content_opacity - at_120_fps.current.content_opacity).abs() < 0.001
        );
    }

    #[test]
    fn clamps_a_long_frame_to_one_tenth_of_a_second() {
        let mut long_frame = Animation::new();
        let mut clamped_frame = Animation::new();
        long_frame.expand();
        clamped_frame.expand();

        long_frame.advance(0.5);
        clamped_frame.advance(0.1);

        assert_eq!(long_frame.current, clamped_frame.current);
        assert_eq!(long_frame.velocity, clamped_frame.velocity);
    }

    #[test]
    fn expand_reaches_the_panel_before_250_milliseconds() {
        let mut animation = Animation::new();
        animation.expand();

        let mut crossed_at = None;
        for frame in 1..=120 {
            animation.advance(1.0 / 120.0);
            if animation.current.width >= EXPANDED.width {
                crossed_at = Some(frame as f32 / 120.0);
                break;
            }
        }

        let crossed_at = crossed_at.expect("spring should reach the target width");
        assert!(crossed_at < 0.25, "took {crossed_at}s");
    }

    #[test]
    fn expanding_overshoots_the_panel_width_then_settles() {
        let mut animation = Animation::new();
        animation.expand();

        let mut peak = 0.0_f32;
        for _ in 0..180 {
            animation.advance(1.0 / 120.0);
            peak = peak.max(animation.current.width);
        }

        assert!(peak > EXPANDED.width + 5.0, "peak was {peak}");
        assert!((animation.current.width - EXPANDED.width).abs() < 0.01);
        assert!(animation.velocity.width.abs() < 0.01);
    }

    #[test]
    fn collapsing_dips_below_the_capsule_width_then_settles() {
        let mut animation = Animation::new();
        animation.expand();
        for _ in 0..180 {
            animation.advance(1.0 / 120.0);
        }

        animation.collapse();
        let mut dip = f32::INFINITY;
        for _ in 0..180 {
            animation.advance(1.0 / 120.0);
            dip = dip.min(animation.current.width);
        }

        assert!(dip < COMPACT.width - 3.0, "dip was {dip}");
        assert!((animation.current.width - COMPACT.width).abs() < 0.01);
    }

    #[test]
    fn reversing_mid_transition_preserves_position_and_velocity() {
        let mut animation = Animation::new();
        animation.expand();
        animation.advance(0.1);

        let position = animation.current;
        let velocity = animation.velocity;
        animation.collapse();

        assert_eq!(animation.current, position);
        assert_eq!(animation.velocity, velocity);
        assert_eq!(animation.target, COMPACT);
    }

    #[test]
    fn shape_values_stay_within_soft_bounds_and_radius_fits() {
        let mut animation = Animation::new();

        for _ in 0..2 {
            animation.expand();
            for _ in 0..180 {
                animation.advance(1.0 / 120.0);
                assert!(
                    (COMPACT.width * 0.92..=EXPANDED.width * 1.08)
                        .contains(&animation.current.width)
                );
                assert!(
                    (COMPACT.height * 0.92..=EXPANDED.height * 1.08)
                        .contains(&animation.current.height)
                );
                assert!(animation.current.radius >= COMPACT.radius * 0.92);
                assert!(animation.current.radius <= animation.current.height / 2.0 + 0.001);
            }

            animation.collapse();
            for _ in 0..180 {
                animation.advance(1.0 / 120.0);
                assert!(
                    (COMPACT.width * 0.92..=EXPANDED.width * 1.08)
                        .contains(&animation.current.width)
                );
                assert!(
                    (COMPACT.height * 0.92..=EXPANDED.height * 1.08)
                        .contains(&animation.current.height)
                );
                assert!(animation.current.radius >= COMPACT.radius * 0.92);
                assert!(animation.current.radius <= animation.current.height / 2.0 + 0.001);
            }
        }
    }

    #[test]
    fn settles_at_target_and_reports_idle() {
        let mut animation = Animation::new();
        animation.expand();

        let mut active = true;
        for _ in 0..600 {
            active = animation.advance(1.0 / 60.0);
            if !active {
                break;
            }
        }

        assert!(!active);
        assert_eq!(animation.current, EXPANDED);
        assert_eq!(animation.velocity, ShapeVelocity::default());
    }
}
