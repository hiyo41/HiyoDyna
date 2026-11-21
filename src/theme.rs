// The complete shared palette is consumed incrementally by native UI components.
#![allow(dead_code)]

pub(crate) const BG: [u8; 3] = [10, 11, 13];
pub(crate) const SURFACE: [u8; 3] = [22, 24, 28];
pub(crate) const SURFACE_RAISED: [u8; 3] = [30, 33, 39];
pub(crate) const BORDER: [u8; 3] = [42, 46, 54];
pub(crate) const BORDER_STRONG: [u8; 3] = [60, 66, 76];
pub(crate) const TEXT: [u8; 3] = [248, 248, 248];
pub(crate) const MUTED: [u8; 3] = [168, 174, 184];
pub(crate) const DIM: [u8; 3] = [112, 118, 128];
pub(crate) const INVERSE: [u8; 3] = [10, 11, 13];
pub(crate) const ICE: [u8; 3] = [245, 200, 169];
pub(crate) const AMBER: [u8; 3] = [168, 201, 246];
pub(crate) const MINT: [u8; 3] = [200, 232, 167];
pub(crate) const LAVENDER: [u8; 3] = [253, 181, 196];
pub(crate) const SUCCESS: [u8; 3] = MINT;
pub(crate) const WARNING: [u8; 3] = AMBER;
pub(crate) const DANGER: [u8; 3] = [160, 169, 242];

/// Packs BGR bytes into Win32 COLORREF (0x00BBGGRR), with red in the low byte.
/// For example, [0x11, 0x22, 0x33] becomes 0x00112233.
pub(crate) fn colorref(color: [u8; 3]) -> u32 {
    u32::from(color[2]) | (u32::from(color[1]) << 8) | (u32::from(color[0]) << 16)
}

pub(crate) fn mix(a: [u8; 3], b: [u8; 3], amount: f32) -> [u8; 3] {
    let amount = amount.clamp(0.0, 1.0);
    std::array::from_fn(|channel| {
        (a[channel] as f32 + (b[channel] as f32 - a[channel] as f32) * amount).round() as u8
    })
}

pub(crate) fn contrast_ratio(a: [u8; 3], b: [u8; 3]) -> f32 {
    fn luminance(color: [u8; 3]) -> f32 {
        let linear = color.map(|channel| {
            let value = channel as f32 / 255.0;
            if value <= 0.04045 {
                value / 12.92
            } else {
                ((value + 0.055) / 1.055).powf(2.4)
            }
        });
        0.2126 * linear[2] + 0.7152 * linear[1] + 0.0722 * linear[0]
    }

    let a = luminance(a);
    let b = luminance(b);
    (a.max(b) + 0.05) / (a.min(b) + 0.05)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn tokens_have_exact_bgr_values() {
        assert_eq!(BG, [10, 11, 13]);
        assert_eq!(SURFACE, [22, 24, 28]);
        assert_eq!(SURFACE_RAISED, [30, 33, 39]);
        assert_eq!(BORDER, [42, 46, 54]);
        assert_eq!(BORDER_STRONG, [60, 66, 76]);
        assert_eq!(TEXT, [248, 248, 248]);
        assert_eq!(MUTED, [168, 174, 184]);
        assert_eq!(DIM, [112, 118, 128]);
        assert_eq!(INVERSE, [10, 11, 13]);
        assert_eq!(ICE, [245, 200, 169]);
        assert_eq!(AMBER, [168, 201, 246]);
        assert_eq!(MINT, [200, 232, 167]);
        assert_eq!(LAVENDER, [253, 181, 196]);
        assert_eq!(SUCCESS, [200, 232, 167]);
        assert_eq!(WARNING, [168, 201, 246]);
        assert_eq!(DANGER, [160, 169, 242]);
    }

    #[test]
    fn colorref_packs_bgr_channels_in_win32_order() {
        assert_eq!(colorref([0x11, 0x22, 0x33]), 0x00112233);
    }

    #[test]
    fn mix_preserves_endpoints_and_interpolates_channels() {
        let a = [10, 20, 30];
        let b = [110, 220, 230];
        assert_eq!(mix(a, b, 0.0), a);
        assert_eq!(mix(a, b, 1.0), b);
        assert_eq!(mix(a, b, 0.5), [60, 120, 130]);
    }

    #[test]
    fn text_colors_meet_contrast_requirements() {
        assert!(contrast_ratio(TEXT, BG) >= 7.0);
        assert!(contrast_ratio(MUTED, SURFACE) >= 4.5);
    }

    #[test]
    fn contrast_uses_srgb_luminance_in_bgr_order() {
        assert!((contrast_ratio([0, 0, 0], [255, 255, 255]) - 21.0).abs() < 0.001);
        assert!((contrast_ratio([128, 128, 128], [0, 0, 0]) - 5.317).abs() < 0.001);
        assert!((contrast_ratio([0, 0, 255], [0, 0, 0]) - 5.252).abs() < 0.001);
        assert_eq!(contrast_ratio(BG, BG), 1.0);
        assert_eq!(contrast_ratio(TEXT, BG), contrast_ratio(BG, TEXT));
    }

    #[test]
    fn accents_are_pairwise_distinct() {
        let accents = [ICE, AMBER, MINT, LAVENDER];
        for (index, accent) in accents.iter().enumerate() {
            for other in &accents[index + 1..] {
                assert_ne!(accent, other);
            }
        }
    }
}
