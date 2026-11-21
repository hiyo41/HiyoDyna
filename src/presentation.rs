use std::time::{Duration, Instant};

#[derive(Default)]
pub(crate) struct Selection {
    music: bool,
    manual: bool,
    inactive_since: Option<Instant>,
    source: String,
}

impl Selection {
    pub fn select(&mut self, music: bool, now: Instant) {
        self.music = music;
        self.manual = !music;
        self.inactive_since = Some(now);
    }

    pub fn music(&self) -> bool {
        self.music
    }

    pub fn update(&mut self, playing: bool, source: &str, lyric_ready: bool, now: Instant) -> bool {
        let previous = self.music;
        if self.source != source {
            self.source = source.to_owned();
            self.inactive_since = None;
        }
        if playing {
            self.inactive_since = None;
            if !self.manual && lyric_ready {
                self.music = true;
            }
        } else if now.saturating_duration_since(*self.inactive_since.get_or_insert(now))
            >= Duration::from_secs(10)
        {
            self.music = false;
        }
        previous != self.music
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn playback_waits_for_timed_lyrics_then_automatically_selects_music() {
        let now = Instant::now();
        let mut selection = Selection::default();
        assert!(!selection.music());
        assert!(!selection.update(true, "Spotify", false, now));
        assert!(selection.update(true, "Spotify", true, now));
        assert!(selection.music());
    }

    #[test]
    fn manual_clock_survives_tracks_sources_and_resumed_playback() {
        let now = Instant::now();
        let mut selection = Selection::default();
        selection.update(true, "Spotify", true, now);
        selection.select(false, now);
        for (playing, source) in [(true, "Spotify"), (false, ""), (true, "Music")] {
            selection.update(playing, source, true, now + Duration::from_secs(30));
            assert!(!selection.music());
        }
    }

    #[test]
    fn continuous_pause_returns_to_clock_at_ten_seconds_without_snapshot_drift() {
        let now = Instant::now();
        let mut selection = Selection::default();
        selection.update(true, "Spotify", true, now);
        selection.update(false, "Spotify", true, now);
        for millis in [100, 9000, 9999] {
            selection.update(false, "Spotify", true, now + Duration::from_millis(millis));
            assert!(selection.music());
        }
        assert!(selection.update(false, "Spotify", true, now + Duration::from_secs(10)));
        assert!(!selection.music());
        assert!(!selection.update(false, "Spotify", true, now + Duration::from_secs(20)));
    }

    #[test]
    fn resuming_before_timeout_cancels_it_and_disappearance_starts_a_new_timer() {
        let now = Instant::now();
        let mut selection = Selection::default();
        selection.update(true, "Spotify", true, now);
        selection.update(false, "Spotify", true, now);
        selection.update(true, "Spotify", true, now + Duration::from_millis(9900));
        selection.update(true, "Spotify", true, now + Duration::from_secs(12));
        assert!(selection.music());
        selection.update(false, "", false, now + Duration::from_secs(12));
        selection.update(false, "", false, now + Duration::from_secs(21));
        assert!(selection.music());
        selection.update(false, "", false, now + Duration::from_secs(22));
        assert!(!selection.music());
    }

    #[test]
    fn explicit_music_after_timeout_receives_a_new_grace_period() {
        let now = Instant::now();
        let mut selection = Selection::default();
        selection.select(true, now);
        selection.update(false, "Spotify", false, now);
        selection.update(false, "Spotify", false, now + Duration::from_secs(10));
        assert!(!selection.music());
        selection.select(true, now + Duration::from_secs(15));
        selection.update(false, "Spotify", false, now + Duration::from_secs(24));
        assert!(selection.music());
        selection.update(false, "Spotify", false, now + Duration::from_secs(25));
        assert!(!selection.music());
        selection.update(true, "Spotify", true, now + Duration::from_secs(26));
        assert!(selection.music());
    }

    #[test]
    fn automatic_music_can_resume_after_inactivity_without_setting_the_manual_latch() {
        let now = Instant::now();
        let mut selection = Selection::default();
        selection.update(true, "Spotify", true, now);
        selection.update(false, "Spotify", true, now);
        selection.update(false, "Spotify", true, now + Duration::from_secs(10));
        assert!(!selection.music());
        assert!(selection.update(true, "Spotify", true, now + Duration::from_secs(11)));
        assert!(selection.music());
    }

    #[test]
    fn a_different_inactive_source_receives_its_own_grace_period() {
        let now = Instant::now();
        let mut selection = Selection::default();
        selection.update(true, "Spotify", true, now);
        selection.update(false, "Spotify", true, now);
        selection.update(false, "Music", false, now + Duration::from_secs(9));
        selection.update(false, "Music", false, now + Duration::from_secs(18));
        assert!(selection.music());
        selection.update(false, "Music", false, now + Duration::from_secs(19));
        assert!(!selection.music());
    }
}
