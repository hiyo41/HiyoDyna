use std::fs;
use std::path::PathBuf;

use crate::animation::SpringTuning;
use crate::clock::ClockFormat;

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub enum DockAnchor {
    Left,
    #[default]
    Center,
    Right,
}

impl DockAnchor {
    fn as_str(self) -> &'static str {
        match self {
            Self::Left => "left",
            Self::Center => "center",
            Self::Right => "right",
        }
    }
}

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

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum AccentPreset {
    Ice,
    Amber,
    Mint,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum UpdateChannel {
    Build,
    Stable,
}

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub(crate) enum LyricsMode {
    #[default]
    Auto,
    Lines,
    Words,
}

#[derive(Clone, Copy, Debug, PartialEq)]
pub(crate) struct Settings {
    pub motion: MotionPreset,
    pub bounce: BouncePreset,
    pub shadow: bool,
    pub top_gap: f32,
    pub dock: DockAnchor,
    pub clock: ClockFormat,
    pub accent: AccentPreset,
    pub show_seconds: bool,
    pub update: UpdateChannel,
    pub lyrics: LyricsMode,
    pub lyric_offset_ms: i32,
    pub compact_lyrics: bool,
    pub language: crate::i18n::Language,
    pub shortcut: crate::hotkey::Shortcut,
}

impl Default for Settings {
    fn default() -> Self {
        Self {
            motion: MotionPreset::Normal,
            bounce: BouncePreset::Medium,
            shadow: true,
            top_gap: 0.0,
            dock: DockAnchor::Center,
            clock: ClockFormat::TwentyFourHour,
            accent: AccentPreset::Ice,
            show_seconds: true,
            update: UpdateChannel::Build,
            lyrics: LyricsMode::Auto,
            lyric_offset_ms: 0,
            compact_lyrics: true,
            language: crate::i18n::Language::System,
            shortcut: crate::hotkey::Shortcut::default(),
        }
    }
}

impl Settings {
    pub fn tuning(&self) -> SpringTuning {
        let (expand_response_seconds, collapse_response_seconds) = match self.motion {
            MotionPreset::Slow => (0.38, 0.42),
            MotionPreset::Normal => (0.30, 0.32),
            MotionPreset::Fast => (0.22, 0.24),
        };
        let (expand_damping, collapse_damping) = match self.bounce {
            BouncePreset::Low => (0.78, 0.85),
            BouncePreset::Medium => (0.62, 0.76),
            BouncePreset::High => (0.50, 0.64),
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

    pub fn accent_bgra(&self) -> [u8; 3] {
        self.accent.bgra()
    }

    #[cfg(test)]
    pub fn parse(text: &str) -> Self {
        Self::parse_with_default(text, UpdateChannel::Build)
    }

    pub fn parse_with_default(text: &str, update: UpdateChannel) -> Self {
        let mut settings = Self {
            update,
            ..Self::default()
        };

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
                "dock" => match value {
                    "left" => settings.dock = DockAnchor::Left,
                    "center" => settings.dock = DockAnchor::Center,
                    "right" => settings.dock = DockAnchor::Right,
                    _ => {}
                },
                "accent" => {
                    if let Some(accent) = AccentPreset::parse(value) {
                        settings.accent = accent;
                    }
                }
                "seconds" => match value {
                    "on" => settings.show_seconds = true,
                    "off" => settings.show_seconds = false,
                    _ => {}
                },
                "update" => {
                    if let Some(update) = UpdateChannel::parse(value) {
                        settings.update = update;
                    }
                }
                "lyrics" => match value {
                    "lines" => settings.lyrics = LyricsMode::Lines,
                    "words" => settings.lyrics = LyricsMode::Words,
                    "auto" => settings.lyrics = LyricsMode::Auto,
                    _ => {}
                },
                "lyric_offset_ms" => {
                    if let Ok(offset) = value.parse::<i32>() {
                        settings.lyric_offset_ms = offset.clamp(-10_000, 10_000);
                    }
                }
                "compact_lyrics" => match value {
                    "on" => settings.compact_lyrics = true,
                    "off" => settings.compact_lyrics = false,
                    _ => {}
                },
                "language" => {
                    if let Some(language) = crate::i18n::Language::parse(value) {
                        settings.language = language;
                    }
                }
                "shortcut" => {
                    if let Some(shortcut) = crate::hotkey::Shortcut::parse(value) {
                        settings.shortcut = shortcut;
                    }
                }
                _ => {}
            }
        }

        settings
    }

    pub fn serialize(&self) -> String {
        format!(
            "motion={}\nbounce={}\nshadow={}\ntop_gap={}\ndock={}\nclock={}\naccent={}\nseconds={}\nupdate={}\nlyrics={}\nlyric_offset_ms={}\ncompact_lyrics={}\nlanguage={}\nshortcut={}\n",
            self.motion.as_str(),
            self.bounce.as_str(),
            if self.shadow { "soft" } else { "off" },
            self.top_gap as i32,
            self.dock.as_str(),
            match self.clock {
                ClockFormat::TwentyFourHour => "24",
                ClockFormat::TwelveHour => "12",
            },
            self.accent.as_str(),
            if self.show_seconds { "on" } else { "off" },
            self.update.as_str(),
            match self.lyrics {
                LyricsMode::Auto => "auto",
                LyricsMode::Lines => "lines",
                LyricsMode::Words => "words",
            },
            self.lyric_offset_ms,
            if self.compact_lyrics { "on" } else { "off" },
            self.language.as_str(),
            self.shortcut.serialize()
        )
    }

    pub fn load() -> Self {
        let default_update = default_update_channel();
        config_path()
            .and_then(|path| fs::read_to_string(path).ok())
            .map(|text| Self::parse_with_default(&text, default_update))
            .unwrap_or_else(|| Self {
                update: default_update,
                ..Self::default()
            })
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

impl UpdateChannel {
    pub fn label(self) -> &'static str {
        match self {
            Self::Build => "Build",
            Self::Stable => "Stable",
        }
    }

    pub(crate) fn as_str(self) -> &'static str {
        match self {
            Self::Build => "build",
            Self::Stable => "stable",
        }
    }

    pub(crate) fn parse(value: &str) -> Option<Self> {
        match value {
            "build" => Some(Self::Build),
            "stable" => Some(Self::Stable),
            _ => None,
        }
    }
}

fn default_update_channel_from(has_install_json: bool) -> UpdateChannel {
    if has_install_json {
        UpdateChannel::Build
    } else {
        UpdateChannel::Stable
    }
}

fn default_update_channel() -> UpdateChannel {
    let installed = std::env::current_exe()
        .ok()
        .and_then(|exe| exe.parent().map(|dir| dir.join("install.json").exists()))
        .unwrap_or(false);
    default_update_channel_from(installed)
}

impl AccentPreset {
    pub fn bgra(self) -> [u8; 3] {
        match self {
            Self::Ice => crate::theme::ICE,
            Self::Amber => crate::theme::AMBER,
            Self::Mint => crate::theme::MINT,
        }
    }

    pub fn label(self) -> &'static str {
        match self {
            Self::Ice => "Ice",
            Self::Amber => "Amber",
            Self::Mint => "Mint",
        }
    }

    fn as_str(self) -> &'static str {
        match self {
            Self::Ice => "ice",
            Self::Amber => "amber",
            Self::Mint => "mint",
        }
    }

    fn parse(value: &str) -> Option<Self> {
        match value {
            "ice" => Some(Self::Ice),
            "amber" => Some(Self::Amber),
            "mint" => Some(Self::Mint),
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

pub(crate) fn config_path() -> Option<PathBuf> {
    std::env::var_os("APPDATA")
        .map(|base| PathBuf::from(base).join("HiyoDyna").join("settings.ini"))
}

#[cfg(test)]
mod tests {
    #[test]
    fn new_ui_preferences_default_and_survive_serialization() {
        let defaults = super::Settings::default().serialize();
        assert!(defaults.lines().any(|line| line == "compact_lyrics=on"));
        assert!(defaults.lines().any(|line| line == "language=system"));
        assert!(defaults.lines().any(|line| line == "shortcut=3:68"));
        let changed = super::Settings::parse("compact_lyrics=off\nlanguage=vi\nshortcut=6:37\n");
        let saved = changed.serialize();
        for expected in ["compact_lyrics=off", "language=vi", "shortcut=6:37"] {
            assert!(saved.lines().any(|line| line == expected), "{expected}");
        }
        assert_eq!(super::Settings::parse(&saved), changed);
        assert_eq!(
            super::Settings::parse("language=bad\nshortcut=0:68\ncompact_lyrics=bad"),
            super::Settings::default()
        );
    }
    #[test]
    fn dock_values_round_trip_and_old_files_default_center() {
        for (value, anchor) in [
            ("left", super::DockAnchor::Left),
            ("center", super::DockAnchor::Center),
            ("right", super::DockAnchor::Right),
        ] {
            let settings = super::Settings::parse(&format!("dock={value}\n"));
            assert_eq!(settings.dock, anchor);
            assert!(settings.serialize().contains(&format!("dock={value}\n")));
        }
        assert!(
            super::Settings::parse("clock=12\n")
                .serialize()
                .contains("dock=center\n")
        );
        assert!(
            super::Settings::parse("dock=invalid\n")
                .serialize()
                .contains("dock=center\n")
        );
    }

    #[test]
    fn lyrics_preferences_clamp_offsets_and_preserve_old_defaults() {
        let s = super::Settings::parse("lyrics=words\nlyric_offset_ms=999999\n");
        assert_eq!(s.lyrics, super::LyricsMode::Words);
        assert_eq!(s.lyric_offset_ms, 10000);
        let old = super::Settings::parse("clock=12\n");
        assert_eq!(old.lyric_offset_ms, 0);
        assert_eq!(old.lyrics, super::LyricsMode::Auto);
        assert_eq!(super::Settings::parse(&s.serialize()), s);
    }
    use super::{AccentPreset, BouncePreset, MotionPreset, Settings, UpdateChannel};
    use crate::clock::ClockFormat;

    #[test]
    fn defaults_are_normal_medium_soft_flush_and_24_hour() {
        let settings = Settings::default();

        assert_eq!(settings.motion, MotionPreset::Normal);
        assert_eq!(settings.bounce, BouncePreset::Medium);
        assert!(settings.shadow);
        assert_eq!(settings.top_gap, 0.0);
        assert_eq!(settings.dock, super::DockAnchor::Center);
        assert_eq!(super::DockAnchor::default(), super::DockAnchor::Center);
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
            dock: super::DockAnchor::Left,
            clock: ClockFormat::TwelveHour,
            accent: AccentPreset::Mint,
            show_seconds: false,
            update: UpdateChannel::Stable,
            lyrics: super::LyricsMode::Words,
            lyric_offset_ms: -250,
            ..Settings::default()
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

        assert_eq!(slow.expand_response_seconds, 0.38);
        assert_eq!(slow.collapse_response_seconds, 0.42);
        assert_eq!(normal.expand_response_seconds, 0.30);
        assert_eq!(normal.collapse_response_seconds, 0.32);
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

        assert_eq!(low.expand_damping, 0.78);
        assert_eq!(low.collapse_damping, 0.85);
        assert_eq!(medium.expand_damping, 0.62);
        assert_eq!(medium.collapse_damping, 0.76);
        assert_eq!(high.expand_damping, 0.50);
        assert_eq!(high.collapse_damping, 0.64);
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

    #[test]
    fn defaults_include_ice_accent_and_visible_seconds() {
        let settings = Settings::default();
        assert_eq!(settings.accent, AccentPreset::Ice);
        assert!(settings.show_seconds);
    }

    #[test]
    fn parses_and_serializes_accent_and_seconds() {
        let parsed = Settings::parse("accent=mint\nseconds=off\n");
        assert_eq!(parsed.accent, AccentPreset::Mint);
        assert!(!parsed.show_seconds);
        assert!(parsed.serialize().contains("accent=mint"));
        assert!(parsed.serialize().contains("seconds=off"));
    }

    #[test]
    fn parses_and_serializes_update_channel() {
        let parsed = Settings::parse("update=stable\n");
        assert_eq!(parsed.update, UpdateChannel::Stable);
        assert!(parsed.serialize().contains("update=stable"));
    }

    #[test]
    fn old_settings_files_use_the_supplied_update_default() {
        let parsed = Settings::parse_with_default("motion=fast\n", UpdateChannel::Stable);
        assert_eq!(parsed.update, UpdateChannel::Stable);
        assert_eq!(
            Settings::parse("motion=fast\n").update,
            UpdateChannel::Build
        );
    }

    #[test]
    fn setup_installs_default_to_build_and_portable_copies_to_stable() {
        assert_eq!(
            super::default_update_channel_from(true),
            UpdateChannel::Build
        );
        assert_eq!(
            super::default_update_channel_from(false),
            UpdateChannel::Stable
        );
    }

    #[test]
    fn old_files_default_new_fields() {
        let parsed = Settings::parse("motion=fast\nbounce=high\nshadow=off\ntop_gap=8\nclock=12\n");
        assert_eq!(parsed.accent, AccentPreset::Ice);
        assert!(parsed.show_seconds);
    }

    #[test]
    fn accent_presets_map_to_bgra() {
        assert_eq!(AccentPreset::Ice.bgra(), crate::theme::ICE);
        assert_eq!(AccentPreset::Amber.bgra(), crate::theme::AMBER);
        assert_eq!(AccentPreset::Mint.bgra(), crate::theme::MINT);
    }
}
