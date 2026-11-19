use crate::animation::ShapeState;
use crate::geometry::rounded_rect_distance;

pub(crate) const HOVER_GRACE_MILLISECONDS: u32 = 120;
pub(crate) const HOVER_RECHECK_MILLISECONDS: u32 = 200;

pub(crate) fn pointer_inside_shape(
    canvas_size: (u32, u32),
    state: ShapeState,
    top_margin: f32,
    x: f32,
    y: f32,
) -> bool {
    let center = (canvas_size.0 as f32 * 0.5, top_margin + state.height * 0.5);
    rounded_rect_distance(x, y, state, center) <= 1.0
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum HoverCommand {
    None,
    Expand,
    CancelGrace,
    ArmGrace,
    Collapse,
}

#[derive(Debug, Default)]
pub(crate) struct HoverTracker {
    expanded: bool,
    grace_armed: bool,
}

impl HoverTracker {
    pub fn new() -> Self {
        Self::default()
    }

    pub fn pointer_moved(&mut self, inside_shape: bool) -> HoverCommand {
        if inside_shape {
            if self.grace_armed {
                self.grace_armed = false;
                HoverCommand::CancelGrace
            } else if !self.expanded {
                self.expanded = true;
                HoverCommand::Expand
            } else {
                HoverCommand::None
            }
        } else if self.expanded && !self.grace_armed {
            self.grace_armed = true;
            HoverCommand::ArmGrace
        } else {
            HoverCommand::None
        }
    }

    pub fn grace_expired(&mut self, inside_shape: bool) -> HoverCommand {
        if !self.grace_armed {
            return HoverCommand::None;
        }
        self.grace_armed = false;
        if inside_shape || !self.expanded {
            return HoverCommand::None;
        }
        self.expanded = false;
        HoverCommand::Collapse
    }
}

#[cfg(test)]
mod tests {
    use super::{HoverCommand, HoverTracker, pointer_inside_shape};
    use crate::animation::{COMPACT, EXPANDED};

    const CANVAS: (u32, u32) = (512, 224);
    const TOP_MARGIN: f32 = 12.0;

    #[test]
    fn compact_hit_test_matches_the_capsule_body_not_the_shadow() {
        let center = (CANVAS.0 as f32 / 2.0, TOP_MARGIN + COMPACT.height / 2.0);
        assert!(pointer_inside_shape(
            CANVAS, COMPACT, TOP_MARGIN, center.0, center.1
        ));
        assert!(pointer_inside_shape(
            CANVAS,
            COMPACT,
            TOP_MARGIN,
            center.0 + COMPACT.width / 2.0,
            center.1
        ));
        assert!(!pointer_inside_shape(
            CANVAS,
            COMPACT,
            TOP_MARGIN,
            center.0 + COMPACT.width / 2.0 + 4.0,
            center.1
        ));
        assert!(!pointer_inside_shape(
            CANVAS,
            COMPACT,
            TOP_MARGIN,
            center.0,
            center.1 + COMPACT.height / 2.0 + 4.0
        ));
    }

    #[test]
    fn expanded_hit_test_uses_the_full_panel() {
        let center = (CANVAS.0 as f32 / 2.0, TOP_MARGIN + EXPANDED.height / 2.0);
        assert!(pointer_inside_shape(
            CANVAS, EXPANDED, TOP_MARGIN, center.0, center.1
        ));
        assert!(pointer_inside_shape(
            CANVAS,
            EXPANDED,
            TOP_MARGIN,
            center.0 + EXPANDED.width / 2.0 - 1.0,
            center.1
        ));
        assert!(!pointer_inside_shape(
            CANVAS,
            EXPANDED,
            TOP_MARGIN,
            center.0 + EXPANDED.width / 2.0 + 4.0,
            center.1
        ));
    }

    #[test]
    fn hit_test_scales_with_dpi() {
        let scale = 1.5_f32;
        let scaled = crate::animation::ShapeState {
            width: COMPACT.width * scale,
            height: COMPACT.height * scale,
            radius: COMPACT.radius * scale,
        };
        let canvas = (768, 336);
        let margin = TOP_MARGIN * scale;
        let center = (canvas.0 as f32 / 2.0, margin + scaled.height / 2.0);

        assert!(pointer_inside_shape(
            canvas, scaled, margin, center.0, center.1
        ));
        assert!(!pointer_inside_shape(
            canvas,
            scaled,
            margin,
            center.0 + scaled.width / 2.0 + 4.0,
            center.1
        ));
    }

    #[test]
    fn pointer_entering_expands_once() {
        let mut hover = HoverTracker::new();
        assert_eq!(hover.pointer_moved(true), HoverCommand::Expand);
        assert_eq!(hover.pointer_moved(true), HoverCommand::None);
    }

    #[test]
    fn leaving_arms_a_single_grace_period() {
        let mut hover = HoverTracker::new();
        hover.pointer_moved(true);
        assert_eq!(hover.pointer_moved(false), HoverCommand::ArmGrace);
        assert_eq!(hover.pointer_moved(false), HoverCommand::None);
    }

    #[test]
    fn reentering_cancels_the_grace_period_and_keeps_the_panel_open() {
        let mut hover = HoverTracker::new();
        hover.pointer_moved(true);
        hover.pointer_moved(false);

        assert_eq!(hover.pointer_moved(true), HoverCommand::CancelGrace);
        assert_eq!(hover.pointer_moved(true), HoverCommand::None);
        assert_eq!(hover.grace_expired(false), HoverCommand::None);
    }

    #[test]
    fn grace_expiry_collapses_only_when_still_outside() {
        let mut hover = HoverTracker::new();
        hover.pointer_moved(true);
        hover.pointer_moved(false);
        assert_eq!(hover.grace_expired(true), HoverCommand::None);

        hover.pointer_moved(false);
        assert_eq!(hover.grace_expired(false), HoverCommand::Collapse);
        assert_eq!(hover.pointer_moved(false), HoverCommand::None);
    }

    #[test]
    fn entering_after_a_collapse_expands_again() {
        let mut hover = HoverTracker::new();
        hover.pointer_moved(true);
        hover.pointer_moved(false);
        hover.grace_expired(false);

        assert_eq!(hover.pointer_moved(true), HoverCommand::Expand);
    }
}
