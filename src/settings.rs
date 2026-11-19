use std::fs;
use std::path::PathBuf;

use crate::animation::SpringTuning;
use crate::clock::ClockFormat;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum MotionPreset {
    Slow,
    Normal,
    Fast,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum BouncePreset {
    Low,
    Medium,
    High,
}

#[derive(Clone, Copy, Debug, PartialEq)]
pub(crate) struct Settings {
    pub motion: MotionPreset,
    pub bounce: BouncePreset,
    pub shadow: bool,
    pub top_gap: f32,
    pub clock: ClockFormat,
}

impl Default for Settings {
    fn default() -> Self {
        Self {
            motion: MotionPreset::Normal,
            bounce: BouncePreset::Medium,
            shadow: true,
            top_gap: 0.0,
            clock: ClockFormat::TwentyFourHour,
        }
    }
}

impl Settings {
    pub fn tuning(&self) -> SpringTuning {
        let (expand_response_seconds, collapse_response_seconds) = match self.motion {
            MotionPreset::Slow => (0.36, 0.40),
            MotionPreset::Normal => (0.28, 0.30),
            MotionPreset::Fast => (0.22, 0.24),
        };
        let (expand_damping, collapse_damping) = match self.bounce {
            BouncePreset::Low => (0.75, 0.82),
            BouncePreset::Medium => (0.55, 0.68),
            BouncePreset::High => (0.50, 0.62),
        };
        SpringTuning {
            expand_response_seconds,
            expand_damping,
            collapse_response_seconds,
            collapse_damping,
        }
    }

    pub fn shadow_opacity(&self) -> f32 {
        if self.shadow { 0.22 } else { 0.0 }
    }

    pub fn parse(text: &str) -> Self {
        let mut settings = Self::default();

        for line in text.lines() {
            let Some((key, value)) = line.split_once('=') else {
                continue;
            };
            let value = value.trim();
            match key.trim() {
                "motion" => {
                    if let Some(motion) = MotionPreset::parse(value) {
                        settings.motion = motion;
                    }
                }
                "bounce" => {
                    if let Some(bounce) = BouncePreset::parse(value) {
                        settings.bounce = bounce;
                    }
                }
                "shadow" => match value {
                    "soft" => settings.shadow = true,
                    "off" => settings.shadow = false,
                    _ => {}
                },
                "top_gap" => {
                    if let Some(gap) = parse_top_gap(value) {
                        settings.top_gap = gap;
                    }
                }
                "clock" => match value {
                    "24" => settings.clock = ClockFormat::TwentyFourHour,
                    "12" => settings.clock = ClockFormat::TwelveHour,
                    _ => {}
                },
                _ => {}
            }
        }

        settings
    }

    pub fn serialize(&self) -> String {
        format!(
            "motion={}\nbounce={}\nshadow={}\ntop_gap={}\nclock={}\n",
            self.motion.as_str(),
            self.bounce.as_str(),
            if self.shadow { "soft" } else { "off" },
            self.top_gap as i32,
            match self.clock {
                ClockFormat::TwentyFourHour => "24",
                ClockFormat::TwelveHour => "12",
            }
        )
    }

    pub fn load() -> Self {
        config_path()
            .and_then(|path| fs::read_to_string(path).ok())
            .map(|text| Self::parse(&text))
            .unwrap_or_default()
    }

    pub fn save(&self) {
        let Some(path) = config_path() else {
            return;
        };
        if let Some(parent) = path.parent() {
            let _ = fs::create_dir_all(parent);
        }
        let _ = fs::write(path, self.serialize());
    }
}

impl MotionPreset {
    fn as_str(self) -> &'static str {
        match self {
            Self::Slow => "slow",
            Self::Normal => "normal",
            Self::Fast => "fast",
        }
    }

    fn parse(value: &str) -> Option<Self> {
        match value {
            "slow" => Some(Self::Slow),
            "normal" => Some(Self::Normal),
            "fast" => Some(Self::Fast),
            _ => None,
        }
    }
}

impl BouncePreset {
    fn as_str(self) -> &'static str {
        match self {
            Self::Low => "low",
            Self::Medium => "medium",
            Self::High => "high",
        }
    }

    fn parse(value: &str) -> Option<Self> {
        match value {
            "low" => Some(Self::Low),
            "medium" => Some(Self::Medium),
            "high" => Some(Self::High),
            _ => None,
        }
    }
}

fn parse_top_gap(value: &str) -> Option<f32> {
    match value {
        "0" => Some(0.0),
        "8" => Some(8.0),
        "16" => Some(16.0),
        _ => None,
    }
}

fn config_path() -> Option<PathBuf> {
    std::env::var_os("APPDATA")
        .map(|base| PathBuf::from(base).join("HiyoDyna").join("settings.ini"))
}

#[cfg(test)]
mod tests {
    use super::{BouncePreset, MotionPreset, Settings};
    use crate::clock::ClockFormat;

    #[test]
    fn defaults_are_normal_medium_soft_flush_and_24_hour() {
        let settings = Settings::default();

        assert_eq!(settings.motion, MotionPreset::Normal);
        assert_eq!(settings.bounce, BouncePreset::Medium);
        assert!(settings.shadow);
        assert_eq!(settings.top_gap, 0.0);
        assert_eq!(settings.clock, ClockFormat::TwentyFourHour);
    }

    #[test]
    fn parses_saved_values() {
        let settings =
            Settings::parse("motion=fast\nbounce=high\nshadow=off\ntop_gap=8\nclock=12\n");

        assert_eq!(settings.motion, MotionPreset::Fast);
        assert_eq!(settings.bounce, BouncePreset::High);
        assert!(!settings.shadow);
        assert_eq!(settings.top_gap, 8.0);
        assert_eq!(settings.clock, ClockFormat::TwelveHour);
    }

    #[test]
    fn ignores_unknown_and_bad_values() {
        let settings = Settings::parse(
            "motion=turbo\nbounce=high\nshadow=maybe\ntop_gap=5\nclock=13\nnonsense\n",
        );

        assert_eq!(settings.motion, MotionPreset::Normal);
        assert_eq!(settings.bounce, BouncePreset::High);
        assert!(settings.shadow);
        assert_eq!(settings.top_gap, 0.0);
        assert_eq!(settings.clock, ClockFormat::TwentyFourHour);
    }

    #[test]
    fn serializes_and_parses_round_trip() {
        let original = Settings {
            motion: MotionPreset::Fast,
            bounce: BouncePreset::High,
            shadow: false,
            top_gap: 16.0,
            clock: ClockFormat::TwelveHour,
        };

        let parsed = Settings::parse(&original.serialize());

        assert_eq!(parsed, original);
    }

    #[test]
    fn old_settings_files_default_to_24_hour() {
        let settings = Settings::parse("motion=slow\nbounce=low\nshadow=soft\ntop_gap=0\n");

        assert_eq!(settings.clock, ClockFormat::TwentyFourHour);
    }

    #[test]
    fn motion_presets_map_to_spring_responses() {
        let slow = Settings {
            motion: MotionPreset::Slow,
            ..Settings::default()
        }
        .tuning();
        let normal = Settings::default().tuning();
        let fast = Settings {
            motion: MotionPreset::Fast,
            ..Settings::default()
        }
        .tuning();

        assert_eq!(slow.expand_response_seconds, 0.36);
        assert_eq!(slow.collapse_response_seconds, 0.40);
        assert_eq!(normal.expand_response_seconds, 0.28);
        assert_eq!(normal.collapse_response_seconds, 0.30);
        assert_eq!(fast.expand_response_seconds, 0.22);
        assert_eq!(fast.collapse_response_seconds, 0.24);
    }

    #[test]
    fn bounce_presets_map_to_damping() {
        let low = Settings {
            bounce: BouncePreset::Low,
            ..Settings::default()
        }
        .tuning();
        let medium = Settings::default().tuning();
        let high = Settings {
            bounce: BouncePreset::High,
            ..Settings::default()
        }
        .tuning();

        assert_eq!(low.expand_damping, 0.75);
        assert_eq!(low.collapse_damping, 0.82);
        assert_eq!(medium.expand_damping, 0.55);
        assert_eq!(medium.collapse_damping, 0.68);
        assert_eq!(high.expand_damping, 0.50);
        assert_eq!(high.collapse_damping, 0.62);
    }

    #[test]
    fn shadow_off_removes_the_shadow() {
        assert_eq!(Settings::default().shadow_opacity(), 0.22);
        assert_eq!(
            Settings {
                shadow: false,
                ..Settings::default()
            }
            .shadow_opacity(),
            0.0
        );
    }
}
