use std::f32::consts::PI;

const EXPAND_DAMPING: f32 = 0.55;
const EXPAND_RESPONSE_SECONDS: f32 = 0.28;
const COLLAPSE_DAMPING: f32 = 0.68;
const COLLAPSE_RESPONSE_SECONDS: f32 = 0.30;
const HOVER_RESPONSE_SECONDS: f32 = 0.16;
const HOVER_DAMPING: f32 = 1.0;
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
    width: 148.0,
    height: 40.0,
    radius: 20.0,
};

pub(crate) const EXPANDED: ShapeState = ShapeState {
    width: 420.0,
    height: 150.0,
    radius: 30.0,
};

#[derive(Clone, Copy, Debug, PartialEq)]
pub(crate) struct ShapeState {
    pub width: f32,
    pub height: f32,
    pub radius: f32,
}

impl ShapeState {
    pub fn progress(self) -> f32 {
        ((self.height - COMPACT.height) / (EXPANDED.height - COMPACT.height)).clamp(0.0, 1.0)
    }

    pub fn compact_content_alpha(self) -> f32 {
        1.0 - smoothstep(0.10, 0.42, self.progress())
    }

    pub fn expanded_content_alpha(self) -> f32 {
        smoothstep(0.50, 0.90, self.progress())
    }

    pub fn compact_lift(self) -> f32 {
        -(1.0 - self.compact_content_alpha()) * 7.0
    }

    pub fn expanded_lift(self) -> f32 {
        (1.0 - self.expanded_content_alpha()) * 10.0
    }

    pub fn expanded_scale(self) -> f32 {
        0.96 + 0.04 * self.expanded_content_alpha()
    }
}

fn smoothstep(edge0: f32, edge1: f32, value: f32) -> f32 {
    let t = ((value - edge0) / (edge1 - edge0)).clamp(0.0, 1.0);
    t * t * (3.0 - 2.0 * t)
}

#[derive(Clone, Copy, Debug, Default, PartialEq)]
pub(crate) struct ShapeVelocity {
    pub width: f32,
    pub height: f32,
    pub radius: f32,
}

pub(crate) struct Animation {
    pub current: ShapeState,
    pub velocity: ShapeVelocity,
    pub target: ShapeState,
    pub hover: f32,
    hover_velocity: f32,
    hover_target: f32,
    tuning: SpringTuning,
    previous_target: ShapeState,
    page_shrink: bool,
    compact_resize: bool,
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
            hover: 0.0,
            hover_velocity: 0.0,
            hover_target: 0.0,
            tuning,
            previous_target: COMPACT,
            page_shrink: false,
            compact_resize: false,
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

    pub fn set_hover(&mut self, inside: bool) -> bool {
        let target = if inside { 1.0 } else { 0.0 };
        if (self.hover_target - target).abs() < f32::EPSILON {
            return false;
        }
        self.hover_target = target;
        true
    }

    /// Advances the spring by elapsed wall-clock time and returns whether it is moving.
    pub fn advance(&mut self, dt_seconds: f32) -> bool {
        if !dt_seconds.is_finite() || dt_seconds <= 0.0 {
            return self.is_active();
        }

        if self.target != self.previous_target {
            self.compact_resize = self.target.height == COMPACT.height
                && self.previous_target.height == COMPACT.height
                && (self.current.height - COMPACT.height).abs() < 0.03;
            self.page_shrink =
                self.target.height >= EXPANDED.height && self.target.height < self.current.height;
            self.previous_target = self.target;
        }

        let (damping, response) = if self.target.height >= EXPANDED.height {
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
            remaining -= step;
        }
        self.integrate_hover(dt_seconds);

        if self.is_settled() {
            self.current = self.target;
            self.velocity = ShapeVelocity::default();
            self.hover = self.hover_target;
            self.hover_velocity = 0.0;
            false
        } else {
            true
        }
    }

    fn integrate_hover(&mut self, dt_seconds: f32) {
        let omega = 2.0 * PI / HOVER_RESPONSE_SECONDS;
        let mut remaining = dt_seconds.min(MAX_FRAME_DELTA_SECONDS);
        while remaining > 0.0 {
            let step = remaining.min(MAX_SUBSTEP_SECONDS);
            let acceleration = -2.0 * HOVER_DAMPING * omega * self.hover_velocity
                - omega * omega * (self.hover - self.hover_target);
            self.hover_velocity += acceleration * step;
            self.hover += self.hover_velocity * step;
            if self.hover < 0.0 {
                self.hover = 0.0;
                if self.hover_velocity < 0.0 {
                    self.hover_velocity = 0.0;
                }
            } else if self.hover > 1.0 {
                self.hover = 1.0;
                if self.hover_velocity > 0.0 {
                    self.hover_velocity = 0.0;
                }
            }
            remaining -= step;
        }
    }

    fn integrate_axis(&mut self, dt: f32, axis: usize, omega: f32, damping: f32) {
        let current_width = self.current.width;
        let current_height = self.current.height;
        let (position, velocity, target, min, max) = match axis {
            0 => (
                &mut self.current.width,
                &mut self.velocity.width,
                self.target.width,
                COMPACT.width * OVERSHOOT_MIN,
                (EXPANDED.width.max(self.target.width) * OVERSHOOT_MAX).max(current_width),
            ),
            1 => (
                &mut self.current.height,
                &mut self.velocity.height,
                self.target.height,
                COMPACT.height * OVERSHOOT_MIN,
                (EXPANDED.height.max(self.target.height) * OVERSHOOT_MAX).max(current_height),
            ),
            _ => (
                &mut self.current.radius,
                &mut self.velocity.radius,
                self.target.radius,
                COMPACT.radius * OVERSHOOT_MIN,
                EXPANDED.radius * OVERSHOOT_MAX,
            ),
        };

        // Shrinking pages and collapse height must approach their destination
        // without undershooting or rebounding the compact lyric baseline.
        let collapse_height = axis == 1 && self.target.height == COMPACT.height;
        let monotonic_shrink = (self.page_shrink || collapse_height) && *position >= target;
        let compact_resize = axis == 0 && self.compact_resize;
        let previous = *position;
        let damping = if monotonic_shrink || compact_resize {
            damping.max(1.0)
        } else {
            damping
        };
        let acceleration =
            -2.0 * damping * omega * *velocity - omega * omega * (*position - target);
        *velocity += acceleration * dt;
        *position += *velocity * dt;

        if monotonic_shrink {
            let clamped = position.clamp(target, previous);
            if *position != clamped {
                *position = clamped;
                *velocity = 0.0;
            }
        }

        if compact_resize {
            let clamped = position.clamp(previous.min(target), previous.max(target));
            if *position != clamped {
                *position = clamped;
                *velocity = 0.0;
            }
        }

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
        const HOVER_EPSILON: f32 = 0.001;
        const HOVER_VELOCITY_EPSILON: f32 = 0.01;

        (self.current.width - self.target.width).abs() < POSITION_EPSILON
            && (self.current.height - self.target.height).abs() < POSITION_EPSILON
            && (self.current.radius - self.target.radius).abs() < POSITION_EPSILON
            && self.velocity.width.abs() < VELOCITY_EPSILON
            && self.velocity.height.abs() < VELOCITY_EPSILON
            && self.velocity.radius.abs() < VELOCITY_EPSILON
            && (self.hover - self.hover_target).abs() < HOVER_EPSILON
            && self.hover_velocity.abs() < HOVER_VELOCITY_EPSILON
    }
}

#[cfg(test)]
mod tests {
    #[test]
    fn collapse_height_and_lyric_baseline_never_undershoot_or_rebound() {
        use crate::settings::{BouncePreset, MotionPreset, Settings};
        for motion in [MotionPreset::Slow, MotionPreset::Normal, MotionPreset::Fast] {
            for bounce in [BouncePreset::Low, BouncePreset::Medium, BouncePreset::High] {
                for dt in [1.0 / 30.0, 1.0 / 120.0] {
                    let settings = Settings {
                        motion,
                        bounce,
                        ..Settings::default()
                    };
                    let mut animation = Animation::new_with_tuning(settings.tuning());
                    animation.target = ShapeState {
                        height: 500.0,
                        ..EXPANDED
                    };
                    for _ in 0..600 {
                        animation.advance(dt);
                    }
                    animation.collapse();
                    let mut previous_baseline = (animation.current.height - 22.0) * 0.5;
                    for frame in 0..600 {
                        animation.advance(dt);
                        let baseline = (animation.current.height - 22.0) * 0.5;
                        assert!(
                            baseline >= 9.0 && baseline <= previous_baseline,
                            "collapse baseline dipped or rebounded on frame {frame}: {previous_baseline} -> {baseline}, {motion:?}/{bounce:?}"
                        );
                        previous_baseline = baseline;
                    }
                    assert_eq!(animation.current, COMPACT);
                }
            }
        }
    }

    #[test]
    fn compact_sentence_width_changes_do_not_rebound() {
        use crate::settings::{BouncePreset, MotionPreset, Settings};
        for motion in [MotionPreset::Slow, MotionPreset::Normal, MotionPreset::Fast] {
            for bounce in [BouncePreset::Low, BouncePreset::Medium, BouncePreset::High] {
                let settings = Settings {
                    motion,
                    bounce,
                    ..Settings::default()
                };
                let mut animation = Animation::new_with_tuning(settings.tuning());
                for width in [700.0, 220.0, 1100.0, 148.0] {
                    animation.target = ShapeState { width, ..COMPACT };
                    for _ in 0..240 {
                        let previous = animation.current.width;
                        animation.advance(1.0 / 120.0);
                        assert!(
                            (previous.min(width)..=previous.max(width))
                                .contains(&animation.current.width),
                            "compact resize rebounded: {previous} -> {} toward {width}, {motion:?}/{bounce:?}",
                            animation.current.width
                        );
                    }
                    assert_eq!(animation.current, animation.target);
                }
            }
        }
    }

    #[test]
    fn display_sized_compact_width_settles_and_can_expand_then_return() {
        let mut animation = Animation::new();
        let compact = ShapeState {
            width: 1100.0,
            ..COMPACT
        };
        for target in [compact, EXPANDED, compact] {
            animation.target = target;
            let mut moving = true;
            for _ in 0..600 {
                moving = animation.advance(1.0 / 120.0);
            }
            assert!(
                !moving,
                "display-sized compact widths must settle instead of continuously rendering"
            );
            assert_eq!(animation.current, target);
        }
    }

    use super::{Animation, COMPACT, EXPANDED, ShapeState, ShapeVelocity, SpringTuning};

    #[test]
    fn player_to_clock_shrinks_monotonically_without_crossing_the_destination() {
        for dt in [1.0 / 30.0, 1.0 / 120.0] {
            let mut animation = Animation::new();
            animation.current = ShapeState {
                width: 440.0,
                height: 500.0,
                radius: 32.0,
            };
            animation.expand();
            for frame in 0..360 {
                let previous = animation.current;
                animation.advance(dt);
                if frame == 0 {
                    assert!(animation.current.height < previous.height);
                }
                for (current, previous, destination) in [
                    (animation.current.height, previous.height, 150.0),
                    (animation.current.width, previous.width, 420.0),
                    (animation.current.radius, previous.radius, 30.0),
                ] {
                    assert!(current >= destination, "undershot {destination}: {current}");
                    assert!(
                        current <= previous,
                        "rebounded from {previous} to {current}"
                    );
                }
            }
            assert_eq!(animation.current, EXPANDED);
        }
    }

    #[test]
    fn interrupted_page_shrink_returns_to_clock_without_a_deep_dip() {
        let mut animation = Animation::new();
        let player = ShapeState {
            height: 500.0,
            ..EXPANDED
        };
        animation.current = player;
        animation.expand();
        animation.advance(0.1);
        animation.target = player;
        let before_reversal = animation.current;
        animation.advance(1.0 / 120.0);
        assert!((animation.current.height - before_reversal.height).abs() < 30.0);
        animation.advance(0.1);
        animation.expand();
        for _ in 0..360 {
            animation.advance(1.0 / 120.0);
            assert!(animation.current.height >= 150.0);
            assert!(animation.current.width >= 420.0);
            assert!(animation.current.radius >= 30.0);
            assert!(animation.current.height <= 540.0);
        }
        assert_eq!(animation.current, EXPANDED);
    }

    #[test]
    fn page_shrink_honors_the_selected_response_speed() {
        let mut regular = Animation::new();
        let mut fast = Animation::new_with_tuning(SpringTuning {
            expand_response_seconds: 0.12,
            ..SpringTuning::default()
        });
        for animation in [&mut regular, &mut fast] {
            animation.current = ShapeState {
                height: 500.0,
                ..EXPANDED
            };
            animation.expand();
            animation.advance(0.08);
            assert!(animation.current.height >= 150.0);
        }
        assert!(fast.current.height < regular.current.height);
    }

    #[test]
    fn compact_lyrics_width_retargets_and_settles_at_capsule_height() {
        let mut animation = Animation::new();
        for width in [360.0, 220.0, 148.0] {
            animation.target = ShapeState { width, ..COMPACT };
            for _ in 0..360 {
                animation.advance(1.0 / 120.0);
                assert_eq!(animation.current.height, 40.0);
                assert_eq!(animation.current.radius, 20.0);
                assert!(animation.current.width <= 453.6);
            }
            assert_eq!(animation.current.width, width);
        }
    }

    #[test]
    fn taller_player_settles_and_collapses_without_snapping() {
        let mut animation = Animation::new();
        animation.target = super::ShapeState {
            height: 500.0,
            ..EXPANDED
        };
        for _ in 0..300 {
            animation.advance(1.0 / 120.0);
        }
        assert_eq!(animation.current.height, 500.0);
        animation.collapse();
        animation.advance(1.0 / 120.0);
        assert!(animation.current.height > 450.0);
        for _ in 0..300 {
            animation.advance(1.0 / 120.0);
        }
        assert_eq!(animation.current, COMPACT);
    }

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

    #[test]
    fn reveal_windows_never_overlap() {
        for i in 0..=100 {
            let p = i as f32 / 100.0;
            let state = super::ShapeState {
                width: COMPACT.width,
                height: COMPACT.height + p * (EXPANDED.height - COMPACT.height),
                radius: COMPACT.radius,
            };
            assert_eq!(
                state.compact_content_alpha() * state.expanded_content_alpha(),
                0.0
            );
        }
    }

    #[test]
    fn reveal_reaches_both_ends() {
        assert_eq!(COMPACT.compact_content_alpha(), 1.0);
        assert_eq!(COMPACT.expanded_content_alpha(), 0.0);
        assert_eq!(EXPANDED.compact_content_alpha(), 0.0);
        assert_eq!(EXPANDED.expanded_content_alpha(), 1.0);
    }

    #[test]
    fn hover_target_change_is_reported_once_and_settles() {
        let mut animation = Animation::new();
        assert!(animation.set_hover(true));
        assert!(!animation.set_hover(true));
        for _ in 0..120 {
            animation.advance(1.0 / 120.0);
        }
        assert!((animation.hover - 1.0).abs() < 0.01);
    }
}
