use crate::canvas::{Canvas, DIM, MUTED, Rect, WHITE};
use crate::i18n::{format_copy, tr};
use crate::media::{MediaCommand, MediaSnapshot, MediaWorker};
use crate::settings::{LyricsMode, Settings};
use std::time::Instant;

const LYRIC_SIZE: f32 = 20.0;
const LYRIC_ROW: f32 = 26.0;
const LYRIC_FADE: f32 = 16.0;

pub(crate) struct Player {
    pub worker: MediaWorker,
    pub music: bool,
    selection: crate::presentation::Selection,
    pub browse: Option<usize>,
    key: String,
    lines: Vec<(Rect, f64)>,
    pub dragging: bool,
    source_picker: bool,
    smooth_line: f32,
    browse_offset: f32,
    smooth_offset: f32,
    pub snapshot: MediaSnapshot,
    pub dirty: bool,
    following: bool,
    layout: Vec<LineLayout>,
    layout_key: (usize, u32),
    layout_lyrics: Option<std::sync::Arc<crate::lyrics::Lyrics>>,
    source_scroll: usize,
    height: f32,
    seek_preview: Option<SeekPreview>,
    last_lyric_frame: Instant,
    anchor_line: usize,
    last_lyric_position: Option<f64>,
    artwork_shown: Option<usize>,
    artwork_fade_started: Option<Instant>,
}

/// Crossfade window when one artwork replaces another. The first artwork of
/// a session does not fade: the page fade already reveals the whole panel.
const ARTWORK_FADE_SECONDS: f32 = 0.22;

struct SeekPreview {
    seconds: f64,
    sampled: Instant,
    key: String,
    generation: u64,
}

struct LineLayout {
    text: String,
    fragments: Vec<String>,
    y: f32,
    height: f32,
}

struct LyricView {
    anchor: usize,
    waiting: Option<(f64, f64)>,
}
fn lyric_emphasis(line: &crate::lyrics::LyricLine, at: f64) -> f32 {
    (((at - line.start) / 0.12).clamp(0.0, 1.0)
        * (1.0 - (at - line.end).max(0.0) / 0.18).clamp(0.0, 1.0)) as f32
}
fn lyric_view(lyrics: &crate::lyrics::Lyrics, at: f64, timed: bool) -> LyricView {
    if !timed {
        return LyricView {
            anchor: 0,
            waiting: None,
        };
    }
    let active = lyrics.active(at);
    let previous = lyrics.lines.iter().rposition(|line| line.start <= at);
    let next = lyrics.lines.iter().position(|line| line.start > at);
    let waiting = if active.is_none() {
        next.and_then(|next| {
            let start = lyrics
                .lines
                .iter()
                .filter(|line| line.start <= at)
                .map(|line| line.end)
                .fold(0.0_f64, f64::max);
            let end = lyrics.lines[next].start;
            (end - start >= 3.0 && at >= start && at < end).then_some((start, end))
        })
    } else {
        None
    };
    LyricView {
        anchor: active.or(previous).unwrap_or(0),
        waiting,
    }
}

impl Player {
    fn compact(&self) -> bool {
        self.height < 400.0
    }
    fn controls_y(&self) -> f32 {
        if self.compact() { 110.0 } else { 144.0 }
    }
    fn progress_y(&self) -> f32 {
        if self.compact() { 174.0 } else { 209.0 }
    }
    fn resume_y(&self) -> f32 {
        if self.compact() { 178.0 } else { 226.0 }
    }
    fn lyrics_top(&self) -> f32 {
        if self.compact() { 204.0 } else { 254.0 }
    }
    fn lyric_viewport(&self, left: f32, width: f32) -> Rect {
        Rect::new(
            left + 24.0,
            self.lyrics_top(),
            (width - 48.0).max(0.0),
            (self.height - 49.0 - self.lyrics_top()).max(0.0),
        )
    }
    pub fn in_lyrics(&self, y: f32) -> bool {
        y >= self.lyrics_top() && y < self.height - 49.0
    }
    fn search_rect(&self, left: f32, width: f32) -> Rect {
        Rect::new(left + width - 128.0, self.height - 37.0, 100.0, 24.0)
    }
    pub fn search_hit(&self, x: f32, y: f32, left: f32, width: f32) -> bool {
        self.music
            && !self.snapshot.key.is_empty()
            && !self.source_picker
            && self.search_rect(left, width).contains(x, y)
    }
    pub fn new() -> Self {
        Self::with_worker(MediaWorker::start())
    }
    fn with_worker(worker: MediaWorker) -> Self {
        Self {
            worker,
            music: false,
            selection: crate::presentation::Selection::default(),
            browse: None,
            key: String::new(),
            lines: Vec::new(),
            dragging: false,
            source_picker: false,
            smooth_line: 0.0,
            browse_offset: 0.0,
            smooth_offset: 0.0,
            snapshot: MediaSnapshot::default(),
            dirty: true,
            following: false,
            layout: Vec::new(),
            layout_key: (0, 0),
            layout_lyrics: None,
            source_scroll: 0,
            height: 500.0,
            seek_preview: None,
            last_lyric_frame: Instant::now(),
            anchor_line: 0,
            last_lyric_position: None,
            artwork_shown: None,
            artwork_fade_started: None,
        }
    }
    pub fn refresh(&mut self) {
        let next = self.worker.snapshot();
        self.reconcile_seek(&next, Instant::now());
        self.dirty |= !same_visual(&self.snapshot, &next);
        self.snapshot = next;
        self.clamp_sources();
        if self.key != self.snapshot.key {
            self.key = self.snapshot.key.clone();
            self.browse = None;
            self.smooth_line = 0.0;
            self.browse_offset = 0.0;
            self.smooth_offset = 0.0;
            self.last_lyric_position = None;
            self.dragging = false;
        }
    }
    pub fn reset(&mut self) {
        self.cancel_seek();
        self.browse = None;
        self.source_picker = false;
        self.dragging = false;
        self.following = false;
        self.browse_offset = 0.0;
        self.smooth_offset = 0.0;
    }
    pub fn collapse(&mut self) {
        self.cancel_seek();
        self.source_picker = false;
        self.dragging = false;
    }
    pub fn select_page(&mut self, music: bool) {
        if self.music != music {
            self.cancel_seek();
            self.source_picker = false;
            self.browse = None;
            self.browse_offset = 0.0;
            self.dragging = false;
        }
        self.music = music;
        self.selection.select(music, Instant::now());
        self.dirty = true;
    }
    pub fn update_selection(&mut self, now: Instant, offset: i32) -> bool {
        let ready = self.compact_lyric(now, offset).is_some();
        let changed =
            self.selection
                .update(self.snapshot.playing, &self.snapshot.source, ready, now);
        if changed {
            self.cancel_seek();
            self.source_picker = false;
            self.music = self.selection.music();
            self.dirty = true;
        }
        changed
    }
    pub fn compact_lyrics_rect(left: f32, width: f32) -> Rect {
        Rect::new(left + width - 292.0, 12.0, 153.0, 30.0)
    }
    fn source_capacity(&self) -> usize {
        ((self.height - 93.0) / 36.0).floor().max(0.0) as usize + 1
    }
    fn clamp_sources(&mut self) {
        self.source_scroll = self.source_scroll.min(
            self.snapshot
                .sources
                .len()
                .saturating_sub(self.source_capacity()),
        );
    }
    pub fn wheel_at(&mut self, down: bool, control: bool, x: f32, y: f32, left: f32, width: f32) {
        if !control
            && self.music
            && !self.source_picker
            && self.lyric_viewport(left, width).contains(x, y)
        {
            self.browse_lines(down);
        } else {
            self.wheel(down, control);
        }
    }
    pub fn wheel(&mut self, down: bool, control: bool) {
        if control {
            if self.music && !self.source_picker {
                self.browse_lines(down);
            }
            return;
        }
        self.dirty = true;
        if self.music && self.source_picker {
            let visible = self.source_capacity();
            self.source_scroll = if down {
                (self.source_scroll + 1).min(self.snapshot.sources.len().saturating_sub(visible))
            } else {
                self.source_scroll.saturating_sub(1)
            };
            return;
        }
        self.select_page(down);
    }
    pub fn browse_lines(&mut self, down: bool) {
        if self.source_picker {
            self.wheel(down, false);
            return;
        }
        if !self.music {
            return;
        }
        self.dirty = true;
        let count = self.snapshot.lyrics.as_ref().map_or(0, |l| l.lines.len());
        let current = self
            .browse
            .unwrap_or(self.anchor_line)
            .min(count.saturating_sub(1));
        if count > 0 {
            let viewport_height = self.lyric_viewport(0.0, 420.0).h;
            let max_offset = |index: usize| {
                self.layout.get(index).map_or(0.0, |layout| {
                    (layout.fragments.len() as f32 * LYRIC_ROW - viewport_height).max(0.0)
                })
            };
            if self.browse.is_none() {
                self.browse_offset = self.smooth_offset.min(max_offset(current));
            }
            let previous_offset = self.browse_offset;
            let target = if down && self.browse_offset < max_offset(current) {
                self.browse_offset = (self.browse_offset + LYRIC_ROW).min(max_offset(current));
                current
            } else if !down && self.browse_offset > 0.0 {
                self.browse_offset = (self.browse_offset - LYRIC_ROW).max(0.0);
                current
            } else if down {
                self.browse_offset = if current + 1 < count {
                    0.0
                } else {
                    self.browse_offset
                };
                (current + 1).min(count - 1)
            } else {
                self.browse_offset = if current > 0 {
                    max_offset(current - 1)
                } else {
                    0.0
                };
                current.saturating_sub(1)
            };
            if (target != current || previous_offset != self.browse_offset) && !self.following {
                self.last_lyric_frame = Instant::now();
            }
            self.browse = Some(target);
            self.following =
                self.smooth_line != target as f32 || self.smooth_offset != self.browse_offset;
        }
    }
    fn position(&self) -> f64 {
        self.position_at(Instant::now())
    }
    fn position_at(&self, now: Instant) -> f64 {
        if let Some(preview) = &self.seek_preview {
            return self.preview_position(preview, now);
        }
        self.snapshot
            .timeline
            .as_ref()
            .and_then(|t| t.position_at(now))
            .unwrap_or(0.0)
    }
    pub fn compact_lyric(
        &self,
        now: Instant,
        offset_ms: i32,
    ) -> Option<(&crate::lyrics::LyricLine, f64)> {
        self.snapshot.timeline.as_ref()?.position_at(now)?;
        let lyrics = self.snapshot.lyrics.as_ref()?;
        if lyrics.instrumental {
            return None;
        }
        let at = self.position_at(now) + f64::from(offset_ms) / 1000.0;
        let i = lyrics
            .active(at)
            .or_else(|| {
                lyrics
                    .lines
                    .iter()
                    .position(|line| line.start > at && line.end > line.start)
            })
            .or_else(|| {
                lyrics
                    .lines
                    .iter()
                    .rposition(|line| line.start <= at && line.end > line.start)
            })?;
        let line = &lyrics.lines[i];
        (line.end > line.start).then_some((line, at))
    }
    fn preview_position(&self, preview: &SeekPreview, now: Instant) -> f64 {
        let elapsed = if !self.dragging && self.snapshot.playing {
            now.saturating_duration_since(preview.sampled).as_secs_f64()
                * self.snapshot.timeline.as_ref().map_or(1.0, |t| t.rate)
        } else {
            0.0
        };
        (preview.seconds + elapsed).clamp(
            0.0,
            self.snapshot
                .timeline
                .as_ref()
                .map_or(preview.seconds, |t| t.duration),
        )
    }
    fn reconcile_seek(&mut self, next: &MediaSnapshot, now: Instant) {
        if let Some(preview) = &self.seek_preview {
            let changed = preview.key != next.key || preview.generation != next.generation;
            let acknowledged = !self.dragging
                && next.timeline.as_ref().is_some_and(|t| {
                    t.sampled >= preview.sampled
                        && t.position_at(now).is_some_and(|position| {
                            (position - self.preview_position(preview, now)).abs() < 0.75
                        })
                });
            let expired = !self.dragging
                && now.saturating_duration_since(preview.sampled).as_secs_f64() > 2.0;
            if changed {
                self.dragging = false;
            }
            if changed || acknowledged || expired {
                self.seek_preview = None;
                self.dirty = true;
            }
        }
    }
    pub fn seek_to(&mut self, seconds: f64) {
        self.set_seek_preview(seconds, true);
    }
    fn set_seek_preview(&mut self, seconds: f64, commit: bool) {
        if self.snapshot.can_seek
            && let Some(t) = &self.snapshot.timeline
            && seconds.is_finite()
        {
            let seconds = seconds.clamp(0.0, t.duration);
            self.seek_preview = Some(SeekPreview {
                seconds,
                sampled: Instant::now(),
                key: self.snapshot.key.clone(),
                generation: self.snapshot.generation,
            });
            self.dirty = true;
            if commit {
                self.worker
                    .command(MediaCommand::Seek(seconds), &self.snapshot);
            }
        }
    }
    pub fn seek_relative(&mut self, delta: f64) {
        self.seek_to(self.position() + delta);
    }
    pub fn seek(&mut self, x: f32, left: f32, width: f32) {
        if width > 0.0
            && let Some(t) = &self.snapshot.timeline
        {
            self.set_seek_preview(
                ((f64::from(x) - f64::from(left)) / f64::from(width)).clamp(0.0, 1.0) * t.duration,
                !self.dragging,
            );
        }
    }
    pub fn finish_seek(&mut self) {
        if !self.dragging {
            return;
        }
        self.dragging = false;
        if let Some(preview) = &mut self.seek_preview {
            if preview.key == self.snapshot.key && preview.generation == self.snapshot.generation {
                preview.sampled = Instant::now();
                self.worker
                    .command(MediaCommand::Seek(preview.seconds), &self.snapshot);
            } else {
                self.seek_preview = None;
            }
        }
        self.dirty = true;
    }
    pub fn cancel_seek(&mut self) {
        if self.dragging {
            self.seek_preview = None;
            self.dirty = true;
        }
        self.dragging = false;
    }
    pub fn click(&mut self, x: f32, y: f32, left: f32, width: f32) -> bool {
        self.dirty = true;
        if Rect::new(left + width - 132.0, 12.0, 76.0, 28.0).contains(x, y) {
            self.clamp_sources();
            self.source_picker = !self.source_picker;
            return true;
        }
        if self.source_picker {
            for (i, source) in self
                .snapshot
                .sources
                .iter()
                .skip(self.source_scroll)
                .enumerate()
            {
                if 51.0 + i as f32 * 36.0 + 28.0 > self.height - 14.0 {
                    break;
                }
                if Rect::new(left + 20.0, 48.0 + i as f32 * 36.0, width - 40.0, 34.0).contains(x, y)
                {
                    self.worker
                        .command(MediaCommand::Source(source.clone()), &self.snapshot);
                    self.source_picker = false;
                    return true;
                }
            }
            self.source_picker = false;
            return true;
        }
        if self.browse.is_some()
            && Rect::new(left + width - 120.0, self.resume_y(), 100.0, 26.0).contains(x, y)
        {
            self.resume_lyrics();
            return true;
        }
        if Rect::new(left + 24.0, self.progress_y() - 11.0, width - 48.0, 24.0).contains(x, y) {
            self.dragging = self.snapshot.can_seek && self.snapshot.timeline.is_some();
            self.seek(x, left + 28.0, width - 56.0);
            return true;
        }
        for (index, command, enabled) in [
            (0, MediaCommand::Previous, self.snapshot.can_previous),
            (1, MediaCommand::Toggle, self.snapshot.can_toggle),
            (2, MediaCommand::Next, self.snapshot.can_next),
        ] {
            if Rect::new(
                left + width * 0.5 - 78.0 + index as f32 * 56.0,
                self.controls_y(),
                44.0,
                40.0,
            )
            .contains(x, y)
                && enabled
            {
                self.worker.command(command, &self.snapshot);
                return true;
            }
        }
        if self.snapshot.can_seek
            && self.snapshot.timeline.is_some()
            && self.in_lyrics(y)
            && let Some((_, time)) = self.lines.iter().find(|(r, _)| r.contains(x, y))
        {
            self.seek_to(*time);
            return true;
        }
        false
    }
    pub fn draw(
        &mut self,
        c: &mut Canvas<'_>,
        left: f32,
        width: f32,
        height: f32,
        settings: &Settings,
    ) {
        self.draw_at(c, left, width, height, settings, Instant::now());
    }
    fn draw_at(
        &mut self,
        c: &mut Canvas<'_>,
        left: f32,
        width: f32,
        height: f32,
        settings: &Settings,
        now: Instant,
    ) {
        self.dirty = false;
        self.following = false;
        self.height = height;
        self.clamp_sources();
        let controls_y = self.controls_y();
        let progress_y = self.progress_y();
        let resume_y = self.resume_y();
        let short = self.compact();
        let art_size = if short { 46.0 } else { 68.0 };
        let card = Rect::new(
            left + 18.0,
            if short { 46.0 } else { 50.0 },
            width - 36.0,
            art_size + if short { 12.0 } else { 16.0 },
        );
        let artwork_rect = Rect::new(
            card.x + 10.0,
            card.y + (card.h - art_size) * 0.5,
            art_size,
            art_size,
        );
        let title_height = if short { 24.0 } else { 28.0 };
        let artist_height = if short { 18.0 } else { 22.0 };
        let metadata_gap = if short { 2.0 } else { 6.0 };
        let metadata_top = card.y + (card.h - title_height - metadata_gap - artist_height) * 0.5;
        let metadata_left = artwork_rect.x + artwork_rect.w + 16.0;
        let metadata_width = (card.x + card.w - 10.0 - metadata_left).max(0.0);
        let accent = settings.accent_bgra();
        let s = &self.snapshot;
        c.text(
            tr("NOW PLAYING"),
            Rect::new(left + 28.0, 17.0, 130.0, 22.0),
            10.0,
            true,
            DIM,
        );
        c.gear(Rect::new(left + width - 49.0, 12.0, 30.0, 30.0), MUTED);
        let toggle = Self::compact_lyrics_rect(left, width);
        c.rect(
            toggle,
            15.0,
            WHITE,
            if settings.compact_lyrics { 0.11 } else { 0.045 },
        );
        c.rect(
            Rect::new(toggle.x + 12.0, toggle.y + 11.0, 8.0, 8.0),
            4.0,
            if settings.compact_lyrics { accent } else { DIM },
            1.0,
        );
        c.ellipsis(
            crate::i18n::tr("Compact lyrics"),
            Rect::new(toggle.x + 28.0, toggle.y, toggle.w - 36.0, toggle.h),
            10.0,
            true,
            if settings.compact_lyrics {
                WHITE
            } else {
                MUTED
            },
        );
        if s.key.is_empty() {
            c.rect(
                Rect::new(left + width * 0.5 - 32.0, 95.0, 64.0, 64.0),
                22.0,
                accent,
                0.08,
            );
            c.text(
                "♪",
                Rect::new(left + width * 0.5 - 13.0, 106.0, 36.0, 40.0),
                30.0,
                false,
                accent,
            );
            c.text(
                if s.unavailable {
                    tr("Media unavailable")
                } else {
                    tr("Your music, right here")
                },
                Rect::new(left + 40.0, 188.0, width - 80.0, 30.0),
                22.0,
                true,
                WHITE,
            );
            c.text(
                tr("Play a song in Spotify or another media app."),
                Rect::new(left + 40.0, 226.0, width - 80.0, 26.0),
                12.0,
                false,
                MUTED,
            );
            c.text(
                tr("Controls and lyrics appear automatically."),
                Rect::new(left + 40.0, 252.0, width - 80.0, 26.0),
                12.0,
                false,
                DIM,
            );
            return;
        }
        // Crossfade between artworks. Pointer identity is safe here: the
        // Arc clone cached in `layout_lyrics`/the snapshot keeps each image
        // alive, so a matching address always means the same image.
        let artwork_key = s
            .artwork
            .as_ref()
            .map(|art| std::sync::Arc::as_ptr(art) as usize);
        if artwork_key != self.artwork_shown {
            self.artwork_fade_started = match (self.artwork_shown, artwork_key) {
                (Some(_), Some(_)) if !crate::animation::reduced_motion() => Some(now),
                _ => None,
            };
            self.artwork_shown = artwork_key;
        }
        let artwork_alpha = match self.artwork_fade_started {
            Some(started) => {
                let elapsed = now.saturating_duration_since(started).as_secs_f32();
                if elapsed >= ARTWORK_FADE_SECONDS {
                    self.artwork_fade_started = None;
                    1.0
                } else {
                    (elapsed / ARTWORK_FADE_SECONDS).min(1.0)
                }
            }
            None => 1.0,
        };
        if let Some(art) = &s.artwork {
            let opacity = c.opacity;
            c.opacity *= artwork_alpha;
            c.artwork(art, artwork_rect, 14.0);
            c.opacity = opacity;
        } else {
            c.rect(artwork_rect, 14.0, accent, 0.14);
            c.text(
                "♪",
                Rect::new(
                    artwork_rect.x + art_size * 0.33,
                    artwork_rect.y + art_size * 0.25,
                    40.0,
                    30.0,
                ),
                27.0,
                false,
                accent,
            );
        }
        c.ellipsis(
            &s.title,
            Rect::new(metadata_left, metadata_top, metadata_width, title_height),
            18.0,
            true,
            WHITE,
        );
        c.ellipsis(
            &s.artist,
            Rect::new(
                metadata_left,
                metadata_top + title_height + metadata_gap,
                metadata_width,
                artist_height,
            ),
            12.0,
            false,
            MUTED,
        );
        let source = source_label(&s.source);
        c.ellipsis(
            &source,
            Rect::new(left + width - 126.0, 18.0, 67.0, 20.0),
            10.0,
            false,
            accent,
        );
        let playing = s.playing;
        for (i, enabled) in [(0, s.can_previous), (1, s.can_toggle), (2, s.can_next)] {
            let rect = Rect::new(
                left + width * 0.5 - 78.0 + i as f32 * 56.0,
                controls_y,
                44.0,
                40.0,
            );
            c.rect(rect, 20.0, WHITE, if i == 1 { 1.0 } else { 0.0 });
            c.transport(
                rect,
                i,
                playing,
                if !enabled {
                    DIM
                } else if i == 1 {
                    crate::theme::INVERSE
                } else {
                    WHITE
                },
            );
        }
        let pos = self.position_at(now);
        c.rect(
            Rect::new(left + 28.0, progress_y, width - 56.0, 3.0),
            1.5,
            WHITE,
            0.12,
        );
        if let Some(t) = &s.timeline {
            let progress = (pos / t.duration).clamp(0.0, 1.0) as f32;
            c.rect(
                Rect::new(left + 28.0, progress_y, (width - 56.0) * progress, 3.0),
                1.5,
                accent,
                1.0,
            );
            let diameter = if self.dragging { 11.0 } else { 8.0 };
            c.rect(
                Rect::new(
                    left + 28.0 + (width - 56.0) * progress - diameter * 0.5,
                    progress_y + 1.5 - diameter * 0.5,
                    diameter,
                    diameter,
                ),
                diameter * 0.5,
                WHITE,
                if s.can_seek { 1.0 } else { 0.35 },
            );
            c.text(
                &time(pos),
                Rect::new(left + 28.0, progress_y - 23.0, 70.0, 17.0),
                10.0,
                false,
                MUTED,
            );
            c.text(
                &time(t.duration),
                Rect::new(left + width - 65.0, progress_y - 23.0, 46.0, 17.0),
                10.0,
                false,
                DIM,
            );
        } else {
            c.text(
                tr("Timing unavailable"),
                Rect::new(left + 28.0, progress_y - 23.0, width - 56.0, 17.0),
                10.0,
                false,
                DIM,
            );
        }
        self.lines.clear();
        if self.browse.is_some() {
            c.text(
                tr("Resume lyrics"),
                Rect::new(left + width - 118.0, resume_y + 2.0, 100.0, 22.0),
                11.0,
                true,
                accent,
            );
        }
        let old_clip = c.clip;
        c.clip = old_clip.intersect(self.lyric_viewport(left, width));
        let edge_fade = LyricEdgeFade::capture(c);
        if let Some(lyrics) = &s.lyrics
            && s.timeline
                .as_ref()
                .and_then(|t| t.position_at(now))
                .is_some()
        {
            let layout_key = (std::sync::Arc::as_ptr(lyrics) as usize, width.to_bits());
            let layout_changed = self.layout_key != layout_key
                || !self
                    .layout_lyrics
                    .as_ref()
                    .is_some_and(|cached| std::sync::Arc::ptr_eq(cached, lyrics));
            if layout_changed {
                self.layout_key = layout_key;
                self.layout_lyrics = Some(lyrics.clone());
                self.layout.clear();
                let mut y = 0.0;
                for line in &lyrics.lines {
                    let text = line.text();
                    let fragments = wrap(&text, c, width - 58.0, LYRIC_SIZE);
                    let height = fragments.len().max(1) as f32 * LYRIC_ROW + 4.0;
                    self.layout.push(LineLayout {
                        text,
                        fragments,
                        y,
                        height,
                    });
                    y += height;
                }
            }
            let timed = s.timeline.is_some() && lyrics.lines.iter().any(|l| l.end > l.start);
            let at = pos + settings.lyric_offset_ms as f64 / 1000.0;
            let view = lyric_view(lyrics, at, timed);
            self.anchor_line = view.anchor;
            let target = self
                .browse
                .unwrap_or(view.anchor)
                .min(self.layout.len().saturating_sub(1));
            let max_offset = self.layout.get(target).map_or(0.0, |layout| {
                (layout.fragments.len() as f32 * LYRIC_ROW - c.clip.h).max(0.0)
            });
            self.browse_offset = self.browse_offset.min(max_offset);
            let target_offset = if self.browse.is_some() {
                self.browse_offset
            } else if max_offset > 0.0
                && settings.lyrics != LyricsMode::Lines
                && lyrics
                    .lines
                    .get(target)
                    .is_some_and(|line| line.has_word_sync())
            {
                let row =
                    active_fragment(&self.layout[target], &lyrics.lines[target], at, c.painter);
                (row as f32 * LYRIC_ROW - (c.clip.h - LYRIC_ROW) * 0.5).clamp(0.0, max_offset)
            } else {
                0.0
            };
            let elapsed = now
                .saturating_duration_since(self.last_lyric_frame)
                .as_secs_f32()
                .min(0.1);
            self.last_lyric_frame = now;
            let jumped = self.last_lyric_position.is_some_and(|previous| {
                (at - previous).abs()
                    > f64::from(elapsed) * s.timeline.as_ref().map_or(1.0, |t| t.rate.abs()) + 0.75
            });
            self.last_lyric_position = Some(at);
            if layout_changed
                || jumped
                || self.seek_preview.is_some()
                || crate::animation::reduced_motion()
            {
                // Reduced motion: the lyric view follows the position without
                // the eased scroll.
                self.smooth_line = target as f32;
                self.smooth_offset = target_offset;
            } else {
                self.smooth_line +=
                    (target as f32 - self.smooth_line) * (1.0 - (-elapsed / 0.09).exp());
                self.smooth_offset +=
                    (target_offset - self.smooth_offset) * (1.0 - (-elapsed / 0.09).exp());
            }
            if (self.smooth_line - target as f32).abs() < 0.01 {
                self.smooth_line = target as f32;
            }
            if (self.smooth_offset - target_offset).abs() < 0.1 {
                self.smooth_offset = target_offset;
            }
            self.following =
                self.smooth_line != target as f32 || self.smooth_offset != target_offset;
            let selected_height = layout_height(&self.layout, self.smooth_line);
            let waiting_space = if self.browse.is_none() && view.waiting.is_some() {
                34.0_f32.min((c.clip.h - LYRIC_ROW).max(0.0))
            } else {
                0.0
            };
            let center_y = c.clip.y
                + waiting_space
                + ((c.clip.h - waiting_space - selected_height) * 0.5).clamp(0.0, 10.0);
            let scroll_y = layout_offset(&self.layout, self.smooth_line) + self.smooth_offset;
            for (i, line) in lyrics.lines.iter().enumerate() {
                let layout = &self.layout[i];
                let y = center_y + layout.y - scroll_y;
                if y + layout.height < c.clip.y || y > c.clip.y + c.clip.h {
                    continue;
                }
                let emphasis = if timed { lyric_emphasis(line, at) } else { 0.0 };
                let color = std::array::from_fn(|channel| {
                    (f32::from(DIM[channel])
                        + (f32::from(WHITE[channel]) - f32::from(DIM[channel])) * emphasis)
                        .round() as u8
                });
                let text = &layout.text;
                let fragments = &layout.fragments;
                let mut offset = 0_usize;
                for (row, fragment) in fragments.iter().enumerate() {
                    let rect = Rect::new(
                        left + 29.0,
                        y + row as f32 * LYRIC_ROW,
                        width - 58.0,
                        LYRIC_ROW,
                    );
                    c.text(fragment, rect, LYRIC_SIZE, true, color);
                    if emphasis > 0.0
                        && line.has_word_sync()
                        && settings.lyrics != LyricsMode::Lines
                    {
                        let fragment_end = offset + fragment.len();
                        let mut word_offset = 0;
                        for word in &line.words {
                            let word_end = word_offset + word.text.len();
                            let begin = word_offset.max(offset);
                            let end = word_end.min(fragment_end);
                            if begin < end
                                && text.is_char_boundary(begin)
                                && text.is_char_boundary(end)
                            {
                                let begin_x =
                                    c.painter.width(&text[offset..begin], LYRIC_SIZE, true);
                                let end_x = c.painter.width(&text[offset..end], LYRIC_SIZE, true);
                                let fill = if at >= word.end {
                                    1.0
                                } else if at <= word.start {
                                    0.0
                                } else {
                                    ((at - word.start) / (word.end - word.start).max(0.001)) as f32
                                };
                                let fill = fragment_fill(
                                    fill,
                                    c.painter.width(&word.text, LYRIC_SIZE, true),
                                    c.painter.width(&text[word_offset..begin], LYRIC_SIZE, true),
                                    end_x - begin_x,
                                );
                                let opacity = c.opacity;
                                let clip = c.clip;
                                c.clip = clip.intersect(Rect::new(
                                    rect.x + begin_x,
                                    rect.y,
                                    (end_x - begin_x) * fill,
                                    rect.h,
                                ));
                                c.opacity *= emphasis;
                                // Reuse the positioned full-fragment mask, including kerning
                                // and diacritics, and reveal only this word's advance span.
                                c.text(fragment, rect, LYRIC_SIZE, true, accent);
                                c.opacity = opacity;
                                c.clip = clip;
                            }
                            word_offset = word_end;
                        }
                    }
                    offset += fragment.len();
                }
                if timed && line.end > line.start {
                    let top = y.max(c.clip.y);
                    let bottom = (y + layout.height - 3.0).min(c.clip.y + c.clip.h);
                    if bottom > top {
                        self.lines.push((
                            Rect::new(left + 24.0, top, width - 48.0, bottom - top),
                            line.start,
                        ));
                    }
                }
            }
            if lyrics.instrumental {
                c.text(
                    tr("Instrumental"),
                    Rect::new(left + 32.0, c.clip.y, width - 64.0, 30.0),
                    22.0,
                    true,
                    MUTED,
                );
            } else if self.browse.is_none()
                && let Some((start, end)) = view.waiting
            {
                let progress = ((at - start) / (end - start)).clamp(0.0, 1.0) as f32;
                for dot in 0..3 {
                    let fill = (progress * 3.0 - dot as f32).clamp(0.0, 1.0);
                    c.rect(
                        Rect::new(left + 32.0 + dot as f32 * 16.0, c.clip.y + 18.0, 7.0, 7.0),
                        3.5,
                        accent,
                        0.25 + 0.75 * fill,
                    );
                }
            }
            edge_fade.apply(c);
            c.clip = old_clip;
            let mode = if timed && lyrics.has_words() && settings.lyrics != LyricsMode::Lines {
                tr("Words")
            } else if timed {
                tr("Lines")
            } else {
                tr("Text")
            };
            c.ellipsis(
                &format!(
                    "{mode} · {}{}",
                    lyrics.source,
                    if lyrics.credits.is_empty() {
                        String::new()
                    } else {
                        format!(" · {}", lyrics.credits)
                    }
                ),
                Rect::new(left + 28.0, height - 35.0, (width - 164.0).max(0.0), 20.0),
                9.0,
                false,
                DIM,
            );
        } else {
            c.text(
                tr(lyric_status(s, now)),
                Rect::new(left + 29.0, c.clip.y, width - 58.0, 32.0),
                22.0,
                true,
                MUTED,
            );
            c.text(
                if s.loading {
                    "AMLL TTML DB · LRCLIB"
                } else {
                    tr("Enjoy the music. Controls are ready.")
                },
                Rect::new(left + 29.0, c.clip.y + 43.0, width - 58.0, 22.0),
                11.0,
                false,
                DIM,
            );
            edge_fade.apply(c);
            c.clip = old_clip;
        }
        if self.music && !self.source_picker {
            let rect = self.search_rect(left, width);
            c.rect(rect, rect.h * 0.5, WHITE, 0.07);
            c.text(
                tr("Find lyrics"),
                Rect::new(rect.x + 12.0, rect.y, rect.w - 24.0, rect.h),
                10.0,
                true,
                MUTED,
            );
        }
        if self.source_picker {
            c.rect(
                Rect::new(
                    left + 18.0,
                    46.0,
                    width - 36.0,
                    (self.snapshot.sources.len() as f32 * 36.0 + 8.0).min(height - 62.0),
                ),
                14.0,
                crate::theme::SURFACE_RAISED,
                1.0,
            );
            for (i, source) in self
                .snapshot
                .sources
                .iter()
                .skip(self.source_scroll)
                .enumerate()
            {
                if 51.0 + i as f32 * 36.0 + 28.0 > height - 14.0 {
                    break;
                }
                c.ellipsis(
                    &source_label(source),
                    Rect::new(left + 30.0, 51.0 + i as f32 * 36.0, width - 60.0, 28.0),
                    12.0,
                    true,
                    if source == &self.snapshot.source {
                        accent
                    } else {
                        WHITE
                    },
                );
            }
        }
    }

    pub fn needs_frame(&self) -> bool {
        self.dirty
            || self.following
            || self.seek_preview.is_some()
            || self.artwork_fade_started.is_some()
            || self.snapshot.timeline.as_ref().is_some_and(|t| t.playing)
    }
    pub fn compact_drawn(&mut self) {
        self.dirty = false;
    }
    pub fn resume_lyrics(&mut self) {
        self.select_page(true);
        self.browse = None;
        self.browse_offset = 0.0;
        if !self.following {
            self.last_lyric_frame = Instant::now();
        }
        self.following = self.smooth_line != self.anchor_line as f32;
        self.dirty = true;
    }

    pub fn accessible_nodes(&self, left: f32, width: f32) -> Vec<crate::accessibility::Node> {
        use crate::accessibility::Node;
        let mut nodes = page_nodes(left, width, self.height);
        nodes.extend([Node::button(
            2,
            tr("Settings"),
            Rect::new(left + width - 52.0, 10.0, 36.0, 34.0),
            true,
        )]);
        if self.music {
            nodes.push(Node::button(
                24,
                tr("Compact lyrics"),
                Self::compact_lyrics_rect(left, width),
                true,
            ));
        }
        if self.snapshot.key.is_empty() {
            return nodes;
        }
        nodes.push(Node::button(
            3,
            tr("Choose media source"),
            Rect::new(left + width - 132.0, 12.0, 76.0, 28.0),
            true,
        ));
        if self.source_picker {
            for (i, source) in self
                .snapshot
                .sources
                .iter()
                .skip(self.source_scroll)
                .enumerate()
            {
                if 51.0 + i as f32 * 36.0 + 28.0 > self.height - 14.0 {
                    break;
                }
                nodes.push(Node::button(
                    100 + (self.source_scroll + i) as u32,
                    format_copy("Use {source}", &[("source", &source_label(source))]),
                    Rect::new(left + 20.0, 48.0 + i as f32 * 36.0, width - 40.0, 34.0),
                    true,
                ));
            }
            return nodes;
        }
        for (i, name, enabled) in [
            (0, tr("Previous track"), self.snapshot.can_previous),
            (
                1,
                if self.snapshot.playing {
                    tr("Pause")
                } else {
                    tr("Play")
                },
                self.snapshot.can_toggle,
            ),
            (2, tr("Next track"), self.snapshot.can_next),
        ] {
            nodes.push(Node::button(
                4 + i as u32,
                name,
                Rect::new(
                    left + width * 0.5 - 78.0 + i as f32 * 56.0,
                    self.controls_y(),
                    44.0,
                    40.0,
                ),
                enabled,
            ));
        }
        if let Some(t) = &self.snapshot.timeline {
            let mut slider = Node::button(
                7,
                tr("Playback position in seconds"),
                Rect::new(left + 24.0, self.progress_y() - 11.0, width - 48.0, 24.0),
                self.snapshot.can_seek,
            );
            slider.range = Some((self.position(), t.duration));
            nodes.push(slider);
        }
        if self.browse.is_some() {
            nodes.push(Node::button(
                8,
                tr("Resume following lyrics"),
                Rect::new(left + width - 120.0, self.resume_y(), 100.0, 26.0),
                true,
            ));
        }
        if self.music {
            nodes.push(Node::button(
                23,
                tr("Find lyrics"),
                self.search_rect(left, width),
                true,
            ));
        }
        nodes
    }
}

struct LyricEdgeFade {
    rows: Vec<(usize, f32)>,
    background: Vec<u8>,
    row_bytes: usize,
}

impl LyricEdgeFade {
    fn capture(c: &Canvas<'_>) -> Self {
        let scale = c.painter.scale;
        let left = (c.clip.x * scale).floor().clamp(0.0, c.size.0 as f32) as usize;
        let right = ((c.clip.x + c.clip.w) * scale)
            .ceil()
            .clamp(left as f32, c.size.0 as f32) as usize;
        let top = (c.clip.y * scale).floor().clamp(0.0, c.size.1 as f32) as usize;
        let bottom = ((c.clip.y + c.clip.h) * scale)
            .ceil()
            .clamp(top as f32, c.size.1 as f32) as usize;
        let row_bytes = (right - left) * 4;
        let mut rows = Vec::new();
        let mut background = Vec::new();
        for y in top..bottom {
            let logical_y = (y as f32 + 0.5) / scale;
            let edge = (logical_y - c.clip.y).min(c.clip.y + c.clip.h - logical_y);
            if edge >= LYRIC_FADE {
                continue;
            }
            let t = (edge / LYRIC_FADE).clamp(0.0, 1.0);
            let opacity = t * t * (3.0 - 2.0 * t);
            let start = (y * c.size.0 as usize + left) * 4;
            rows.push((start, opacity));
            background.extend_from_slice(&c.pixels[start..start + row_bytes]);
        }
        Self {
            rows,
            background,
            row_bytes,
        }
    }

    fn apply(self, c: &mut Canvas<'_>) {
        for ((start, opacity), original) in self
            .rows
            .into_iter()
            .zip(self.background.chunks_exact(self.row_bytes.max(1)))
        {
            for (pixel, &background) in c.pixels[start..start + self.row_bytes]
                .iter_mut()
                .zip(original)
            {
                // Interpolate premultiplied BGRA back to the pre-lyric pixels;
                // the shape, controls and any existing content remain intact.
                *pixel = (f32::from(background)
                    + (f32::from(*pixel) - f32::from(background)) * opacity)
                    .round() as u8;
            }
        }
    }
}

pub(crate) fn page_nodes(left: f32, width: f32, height: f32) -> Vec<crate::accessibility::Node> {
    [(20, "Clock page"), (21, "Music page")]
        .into_iter()
        .enumerate()
        .map(|(i, (id, name))| {
            crate::accessibility::Node::button(
                id,
                tr(name),
                Rect::new(
                    left + width - 28.0,
                    height * 0.5 - 23.0 + i as f32 * 24.0,
                    26.0,
                    22.0,
                ),
                true,
            )
        })
        .collect()
}

pub(crate) fn page_hit(x: f32, y: f32, left: f32, width: f32, height: f32) -> Option<bool> {
    page_nodes(left, width, height)
        .into_iter()
        .find(|n| n.rect.contains(x, y))
        .map(|n| n.id == 21)
}

pub(crate) fn draw_page_dots(c: &mut Canvas<'_>, left: f32, width: f32, height: f32, music: f32) {
    for i in 0..2 {
        let active = if i == 0 { 1.0 - music } else { music };
        c.rect(
            Rect::new(
                left + width - 18.0,
                height * 0.5 - 14.0 + i as f32 * 24.0,
                6.0,
                6.0,
            ),
            3.0,
            WHITE,
            0.22 + active * 0.78,
        );
    }
}

fn lyric_status(snapshot: &MediaSnapshot, now: Instant) -> &'static str {
    if snapshot.unavailable || snapshot.source.is_empty() {
        "No media session"
    } else if snapshot.loading {
        "Finding your lyrics…"
    } else if snapshot
        .timeline
        .as_ref()
        .and_then(|timeline| timeline.position_at(now))
        .is_none()
    {
        "Playback timing unavailable"
    } else {
        "No synced lyrics found"
    }
}

fn same_visual(a: &MediaSnapshot, b: &MediaSnapshot) -> bool {
    a.generation == b.generation
        && a.key == b.key
        && a.sources == b.sources
        && a.loading == b.loading
        && a.unavailable == b.unavailable
        && a.can_toggle == b.can_toggle
        && a.can_previous == b.can_previous
        && a.can_next == b.can_next
        && a.can_seek == b.can_seek
        && a.playing == b.playing
        && a.lyrics.as_ref().map(std::sync::Arc::as_ptr)
            == b.lyrics.as_ref().map(std::sync::Arc::as_ptr)
        && a.artwork.as_ref().map(std::sync::Arc::as_ptr)
            == b.artwork.as_ref().map(std::sync::Arc::as_ptr)
        && match (&a.timeline, &b.timeline) {
            (Some(a), Some(b)) => {
                a.position == b.position
                    && a.duration == b.duration
                    && a.playing == b.playing
                    && a.rate == b.rate
            }
            (None, None) => true,
            _ => false,
        }
}

fn layout_offset(layout: &[LineLayout], position: f32) -> f32 {
    let i = position.max(0.0) as usize;
    layout.get(i).map_or(0.0, |line| {
        line.y
            + (layout
                .get(i + 1)
                .map_or(line.height, |next| next.y - line.y))
                * position.fract()
    })
}

fn layout_height(layout: &[LineLayout], position: f32) -> f32 {
    let i = position.max(0.0) as usize;
    layout.get(i).map_or(30.0, |line| {
        line.height
            + (layout.get(i + 1).map_or(line.height, |next| next.height) - line.height)
                * position.fract()
    })
}

fn fragment_fill(fill: f32, word_width: f32, before: f32, part_width: f32) -> f32 {
    if part_width <= 0.0 {
        return 0.0;
    }
    ((fill * word_width - before) / part_width).clamp(0.0, 1.0)
}

fn active_fragment(
    layout: &LineLayout,
    line: &crate::lyrics::LyricLine,
    at: f64,
    painter: &mut crate::canvas::Painter,
) -> usize {
    let mut begin = 0;
    let Some((word, begin)) = line
        .words
        .iter()
        .filter_map(|word| {
            let start = begin;
            begin += word.text.len();
            (at >= word.start).then_some((word, start))
        })
        .last()
    else {
        return 0;
    };
    let end = begin + word.text.len();
    let progress = ((at - word.start) / (word.end - word.start).max(0.001)).clamp(0.0, 1.0) as f32;
    let cursor = painter.width(&word.text, LYRIC_SIZE, true) * progress;
    let mut bytes = 0;
    for (row, fragment) in layout.fragments.iter().enumerate() {
        bytes += fragment.len();
        if bytes > begin
            && (bytes >= end
                || cursor <= painter.width(&layout.text[begin..bytes.min(end)], LYRIC_SIZE, true))
        {
            return row;
        }
    }
    layout.fragments.len().saturating_sub(1)
}

fn source_label(source: &str) -> String {
    if source.to_lowercase().contains("spotify") {
        "Spotify".into()
    } else {
        source
            .split('!')
            .next()
            .unwrap_or(source)
            .trim_end_matches(".exe")
            .to_owned()
    }
}
fn time(seconds: f64) -> String {
    let seconds = seconds.max(0.0) as u64;
    format!("{}:{:02}", seconds / 60, seconds % 60)
}
fn wrap(text: &str, c: &mut Canvas<'_>, width: f32, size: f32) -> Vec<String> {
    let mut lines = Vec::new();
    let mut remainder = text;
    while !remainder.is_empty() {
        let boundaries: Vec<_> = remainder
            .char_indices()
            .map(|(i, _)| i)
            .chain(Some(remainder.len()))
            .collect();
        let mut low = 1;
        let mut high = boundaries.len() - 1;
        while low < high {
            let mid = (low + high).div_ceil(2);
            if c.painter.width(&remainder[..boundaries[mid]], size, true) <= width {
                low = mid;
            } else {
                high = mid - 1;
            }
        }
        let mut end = boundaries[low];
        if end < remainder.len()
            && let Some((i, ch)) = remainder[..end]
                .char_indices()
                .rev()
                .find(|(_, ch)| ch.is_whitespace())
            && i > 0
        {
            end = i + ch.len_utf8();
        }
        lines.push(remainder[..end].to_owned());
        remainder = &remainder[end..];
    }
    lines
}

#[cfg(test)]
mod tests {
    #[test]
    fn missing_timeline_does_not_render_a_fabricated_active_lyric() {
        let mut player = player_at_gap(1.5);
        player.snapshot.timeline = None;
        draw_test_player(&mut player, 500.0);
        assert!(
            player.layout.is_empty(),
            "timed lyric highlighting requires a usable timeline"
        );
        assert!(player.lines.is_empty());
    }
    #[test]
    fn paused_timed_lyrics_keep_the_music_page_across_pause_and_resume() {
        let mut player = player_at_gap(1.5);
        let now = Instant::now();
        player.snapshot.source = "Spotify.exe".into();
        player.select_page(true);
        player.snapshot.playing = false;
        player.update_selection(now, 0);
        player.update_selection(now + std::time::Duration::from_secs(11), 0);
        assert!(
            player.music,
            "paused timed lyrics must stay visible instead of returning to the clock"
        );
        player.snapshot.playing = true;
        assert!(player.compact_lyric(now, 0).is_some());
        player.update_selection(now + std::time::Duration::from_secs(12), 0);
        assert!(player.music);
    }

    #[test]
    fn resume_lyrics_exits_an_explicit_clock_override() {
        let mut player = player_at_gap(1.5);
        player.snapshot.playing = true;
        player.select_page(false);
        player.resume_lyrics();
        player.update_selection(Instant::now(), 0);
        assert!(
            player.music,
            "Resume following lyrics should restore the music surface"
        );
    }

    #[test]
    fn lyric_status_distinguishes_loading_missing_timing_and_missing_match() {
        let mut player = player_at_gap(1.5);
        let now = Instant::now();
        player.snapshot.source = "Spotify.exe".into();
        player.snapshot.loading = true;
        assert_eq!(lyric_status(&player.snapshot, now), "Finding your lyrics…");
        player.snapshot.loading = false;
        player.snapshot.timeline = None;
        assert_eq!(
            lyric_status(&player.snapshot, now),
            "Playback timing unavailable"
        );
        player.snapshot.timeline = Some(crate::media::Timeline {
            position: 1.0,
            duration: 30.0,
            playing: false,
            rate: 1.0,
            sampled: now,
        });
        player.snapshot.lyrics = None;
        assert_eq!(
            lyric_status(&player.snapshot, now),
            "No synced lyrics found"
        );
        player.snapshot.source.clear();
        assert_eq!(lyric_status(&player.snapshot, now), "No media session");
    }
    #[test]
    #[ignore = "Reads the live production media pipeline without playback commands; use isolated LOCALAPPDATA"]
    fn live_pipeline_diagnostic() {
        let mut player = super::Player::new();
        let settings = crate::settings::Settings::load();
        let start = std::time::Instant::now();
        let mut previous = String::new();
        let mut found = false;
        while start.elapsed() < std::time::Duration::from_secs(50) {
            player.refresh();
            let now = std::time::Instant::now();
            player.update_selection(now, settings.lyric_offset_ms);
            let s = &player.snapshot;
            let status = format!(
                "source={} title={:?} artist={:?} gen={} playing={} timeline={:?} loading={} lyrics={:?} music={} compact={} enabled={}",
                s.source,
                s.title,
                s.artist,
                s.generation,
                s.playing,
                s.timeline
                    .as_ref()
                    .map(|t| (t.position_at(now), t.duration)),
                s.loading,
                s.lyrics
                    .as_ref()
                    .map(|l| (l.lines.len(), l.is_synchronized(), l.instrumental)),
                player.music,
                player
                    .compact_lyric(now, settings.lyric_offset_ms)
                    .is_some(),
                settings.compact_lyrics
            );
            if status != previous {
                eprintln!("{:.1}s {status}", start.elapsed().as_secs_f32());
                previous = status;
            }
            found |= !s.source.is_empty();
            if found && s.playing && !s.loading && s.lyrics.is_some() {
                break;
            }
            std::thread::sleep(std::time::Duration::from_millis(500));
        }
        assert!(found, "No media session found during the diagnostic");
    }
    #[test]
    fn collapse_and_escape_preserve_manual_selection_across_later_lyric_updates() {
        let mut player = player_at_gap(1.5);
        let now = Instant::now();
        player.snapshot.playing = true;
        assert!(player.update_selection(now, 0));
        assert!(player.music);
        player.select_page(false);
        player.collapse();
        player.reset();
        player.snapshot.source = "Another player".into();
        player.update_selection(now + std::time::Duration::from_secs(30), 0);
        assert!(!player.music);
        player.select_page(true);
        player.collapse();
        player.reset();
        assert!(player.music);
    }
    #[test]
    fn instrumental_cache_entries_never_become_compact_lyrics() {
        let mut player = player_at_gap(1.5);
        let mut lyrics = player.snapshot.lyrics.as_ref().unwrap().as_ref().clone();
        lyrics.instrumental = true;
        player.snapshot.lyrics = Some(std::sync::Arc::new(lyrics));
        assert!(player.compact_lyric(Instant::now(), 0).is_none());
    }
    #[test]
    fn compact_header_toggle_has_a_unique_accessible_id_and_separate_hit_target() {
        let mut player = seekable_player();
        player.music = true;
        let rect = Player::compact_lyrics_rect(0.0, 420.0);
        let nodes = player.accessible_nodes(0.0, 420.0);
        for id in [2, 3] {
            let other = nodes.iter().find(|node| node.id == id).unwrap().rect;
            assert!(
                rect.intersect(other).w == 0.0 || rect.intersect(other).h == 0.0,
                "the compact toggle must not overlap header action {id}"
            );
        }
        assert!(
            nodes.iter().any(|node| node.id == 24),
            "the compact toggle must have its own accessible action"
        );
        let mut ids = std::collections::HashSet::new();
        assert!(nodes.iter().all(|node| ids.insert(node.id)));
    }
    #[test]
    fn browsing_keeps_live_line_and_word_highlights() {
        for word_sync in [false, true] {
            let mut player = seekable_player();
            player.music = true;
            player.snapshot.lyrics = crate::lyrics::parse_ttml("<tt><p begin='0s' end='8s'><span begin='0s' end='4s'>First </span><span begin='4s' end='8s'>sentence</span></p><p begin='8s' end='12s'>Next sentence</p></tt>").map(std::sync::Arc::new);
            std::sync::Arc::make_mut(player.snapshot.lyrics.as_mut().unwrap()).lines[0].word_sync =
                word_sync;
            player.snapshot.timeline.as_mut().unwrap().position = 2.0;
            let viewport_pixels = |pixels: Vec<u8>| pixels[254 * 420 * 4..451 * 420 * 4].to_vec();
            let following = viewport_pixels(draw_test_player(&mut player, 500.0));
            player.browse_lines(false);
            assert_eq!(player.browse, Some(0));
            assert!(
                following == viewport_pixels(draw_test_player(&mut player, 500.0)),
                "showing Resume lyrics must not dim the active line or clear word progress"
            );
            player.snapshot.timeline.as_mut().unwrap().position = 6.0;
            let later = viewport_pixels(draw_test_player(&mut player, 500.0));
            if word_sync {
                assert_ne!(
                    following, later,
                    "word progress must continue while browsing"
                );
            }
            player.snapshot.timeline.as_mut().unwrap().position = 9.0;
            let next = viewport_pixels(draw_test_player(&mut player, 500.0));
            assert_ne!(
                later, next,
                "the next line must become active while browsing"
            );
            assert_eq!(
                player.browse,
                Some(0),
                "playback must not move the manual viewport"
            );
            player.resume_lyrics();
            assert!(player.browse.is_none());
        }
    }

    #[test]
    fn short_viewport_follows_real_word_fragments_and_resets_on_backward_seek() {
        let mut player = seekable_player();
        player.music = true;
        let text =
            "Ngàn vì sao ở trên cao lấp lánh đang ngắm nhìn đôi ta bên nhau đêm nay ".repeat(3);
        let words: Vec<_> = text
            .split_inclusive(' ')
            .enumerate()
            .map(|(i, text)| crate::lyrics::LyricWord {
                text: text.into(),
                start: i as f64,
                end: i as f64 + 1.0,
            })
            .collect();
        let end = words.len() as f64;
        player.snapshot.lyrics = Some(std::sync::Arc::new(crate::lyrics::Lyrics {
            lines: vec![crate::lyrics::LyricLine {
                start: 0.0,
                end,
                words,
                word_sync: true,
            }],
            ..Default::default()
        }));
        player.snapshot.timeline.as_mut().unwrap().position = end - 0.1;
        draw_test_player(&mut player, 300.0);
        assert!(
            player.smooth_offset > 0.0,
            "active final words must be visible without manual browsing"
        );
        let paused = player.smooth_offset;
        draw_test_player(&mut player, 300.0);
        assert_eq!(player.smooth_offset, paused);
        player.snapshot.timeline.as_mut().unwrap().position = 0.0;
        draw_test_player(&mut player, 300.0);
        assert_eq!(player.smooth_offset, 0.0);
    }

    #[test]
    fn hover_collapse_preserves_browsing_and_selected_page() {
        let mut player = seekable_player();
        player.music = true;
        player.browse = Some(3);
        player.source_picker = true;
        player.dragging = true;
        player.collapse();
        assert_eq!(player.browse, Some(3));
        assert!(player.music);
        assert!(!player.dragging && !player.source_picker);
        player.resume_lyrics();
        assert!(player.browse.is_none());
    }

    #[test]
    fn minimum_height_can_browse_every_fragment_of_the_only_or_final_line() {
        for prefix in ["", "[00:00]First\n"] {
            let mut player = seekable_player();
            player.music = true;
            player.snapshot.lyrics = Some(std::sync::Arc::new(
                crate::lyrics::parse_lrc(&format!(
                    "{prefix}[00:01]{}",
                    "Ngàn vì sao ở trên cao lấp lánh đang ngắm nhìn đôi ta bên nhau đêm nay "
                        .repeat(3)
                ))
                .unwrap(),
            ));
            player.snapshot.timeline.as_mut().unwrap().position = 2.0;
            draw_test_player(&mut player, 300.0);
            let target = player.anchor_line;
            assert!(player.layout[target].fragments.len() >= 3);
            let before = draw_test_player(&mut player, 300.0);
            for _ in 0..20 {
                player.browse_lines(true);
                draw_test_player(&mut player, 300.0);
            }
            for _ in 0..20 {
                player.last_lyric_frame = Instant::now() - std::time::Duration::from_millis(100);
                draw_test_player(&mut player, 300.0);
            }
            assert_eq!(player.browse, Some(target));
            assert!(
                player.browse_offset > 0.0,
                "wheel must reveal fragments inside the final logical line"
            );
            let viewport = player.lyric_viewport(0.0, 420.0);
            let fragment_bottom = viewport.y
                + player.layout[target].fragments.len() as f32 * LYRIC_ROW
                - player.smooth_offset;
            assert!(
                fragment_bottom <= viewport.y + viewport.h + 1.0,
                "final fragment must fit inside viewport"
            );
            assert_ne!(before, draw_test_player(&mut player, 300.0));
            for _ in 0..40 {
                player.browse_lines(false);
            }
            assert_eq!(player.browse, Some(0));
            assert_eq!(player.browse_offset, 0.0);
        }
    }

    use super::*;

    #[test]
    fn find_lyrics_label_fits_both_languages_at_every_dpi() {
        let player = seekable_player();
        for scale in [1.0, 1.25, 1.5, 2.0] {
            let mut painter = crate::canvas::Painter::new(scale).unwrap();
            for language in [
                crate::i18n::Language::English,
                crate::i18n::Language::Vietnamese,
            ] {
                let label = crate::i18n::tr_in(language, "Find lyrics");
                let available = player.search_rect(0.0, 420.0).w - 24.0;
                let measured = painter.width(label, 10.0, true);
                assert!(
                    measured <= available,
                    "Find lyrics is clipped at {scale}x in {language:?}: {measured} > {available}"
                );
            }
        }
    }

    #[test]
    fn find_lyrics_hit_and_accessibility_share_the_visible_footer_button() {
        use crate::accessibility::Action;
        let mut player = seekable_player();
        player.music = true;
        for height in [300.0, 500.0] {
            draw_test_player(&mut player, height);
            let nodes = player.accessible_nodes(50.0, 420.0);
            let button = nodes
                .iter()
                .find(|node| node.id == 23)
                .expect("Find lyrics must be accessible");
            assert_eq!(button.name, "Find lyrics");
            assert!(button.enabled);
            assert_eq!(
                (button.rect.x, button.rect.y, button.rect.w, button.rect.h),
                (342.0, height - 37.0, 100.0, 24.0)
            );
            let Action::Click(x, y) = button.action else {
                panic!("search uses the normal click action")
            };
            assert!(player.search_hit(x, y, 50.0, 420.0));
            for (x, y, expected) in [
                (342.0, height - 37.0, true),
                (441.9, height - 13.1, true),
                (341.9, height - 25.0, false),
                (442.0, height - 25.0, false),
                (400.0, height - 13.0, false),
                (400.0, height - 38.0, false),
                (455.0, height * 0.5 + 12.0, false),
            ] {
                assert_eq!(
                    player.search_hit(x, y, 50.0, 420.0),
                    expected,
                    "point {x},{y}"
                );
            }
        }
        player.music = false;
        let without_button = raster_player(&mut player, &Settings::default(), 1.0);
        player.music = true;
        let with_button = raster_player(&mut player, &Settings::default(), 1.0);
        let mut changed = 0;
        for y in 0..500 {
            for x in 0..420 {
                let i = (y * 420 + x) * 4;
                if with_button[i..i + 4] != without_button[i..i + 4] {
                    changed += 1;
                    assert!(
                        player.search_hit(x as f32, y as f32, 0.0, 420.0),
                        "visible search pixels must match its hit target"
                    );
                }
            }
        }
        assert!(
            changed > 1000,
            "the search button must be drawn in the footer"
        );
        for (music, key, picker) in [
            (false, "track", false),
            (true, "", false),
            (true, "track", true),
        ] {
            player.music = music;
            player.snapshot.key = key.into();
            player.source_picker = picker;
            assert!(!player.search_hit(400.0, 475.0, 50.0, 420.0));
            assert!(
                !player
                    .accessible_nodes(50.0, 420.0)
                    .iter()
                    .any(|node| node.id == 23 && node.enabled)
            );
        }
    }

    #[test]
    fn pointer_wheel_browses_at_both_lyric_bounds_without_leaving_music() {
        let mut player = player_at_gap(1.5);
        player.music = true;
        draw_test_player(&mut player, 500.0);
        for (down, expected) in [(false, 0), (true, 1), (true, 1), (false, 0)] {
            player.wheel_at(down, false, 100.0, 330.0, 0.0, 420.0);
            assert!(player.music, "lyric wheel must keep Music at either bound");
            assert_eq!(player.browse, Some(expected));
        }
        player.snapshot.lyrics = None;
        player.wheel_at(false, false, 100.0, 330.0, 0.0, 420.0);
        assert!(
            player.music,
            "an empty lyric viewport must not switch pages"
        );
    }

    #[test]
    fn pointer_wheel_outside_viewport_retains_page_navigation() {
        for (x, y) in [
            (23.0, 330.0),
            (396.0, 330.0),
            (100.0, 100.0),
            (100.0, 451.0),
        ] {
            let mut player = player_at_gap(1.5);
            player.music = true;
            draw_test_player(&mut player, 500.0);
            player.wheel_at(false, false, x + 50.0, y, 50.0, 420.0);
            assert!(!player.music, "outside point ({x}, {y}) must route pages");
            player.wheel_at(true, false, x + 50.0, y, 50.0, 420.0);
            assert!(player.music);
        }
    }

    #[test]
    fn pointer_wheel_keeps_control_and_source_picker_routing() {
        let mut player = player_at_gap(1.5);
        player.music = true;
        player.wheel_at(true, true, 0.0, 0.0, 0.0, 420.0);
        assert_eq!(player.browse, Some(1));
        player.source_picker = true;
        player.snapshot.sources = (0..30).map(|i| format!("Source {i}")).collect();
        player.wheel_at(true, false, 100.0, 330.0, 0.0, 420.0);
        assert_eq!(player.source_scroll, 1);
        player.wheel_at(true, true, 100.0, 330.0, 0.0, 420.0);
        assert_eq!(player.source_scroll, 1);
        assert!(player.music);
        assert_eq!(player.browse, Some(1));
    }

    fn raster_player(player: &mut Player, settings: &Settings, scale: f32) -> Vec<u8> {
        let size = ((420.0 * scale) as u32, (500.0 * scale) as u32);
        let mut painter = crate::canvas::Painter::new(scale).unwrap();
        let mut pixels = vec![0; size.0 as usize * size.1 as usize * 4];
        player.draw_at(
            &mut Canvas::new(&mut pixels, size, &mut painter),
            0.0,
            420.0,
            500.0,
            settings,
            player.snapshot.timeline.as_ref().unwrap().sampled,
        );
        pixels
    }

    #[test]
    fn word_highlight_never_extends_the_original_fragment_glyphs() {
        for scale in [1.0, 1.25, 1.5, 2.0] {
            for text in [
                "<span begin='1s' end='2s'>A</span><span begin='2s' end='9s'>VẤN </span><span begin='2s' end='9s'>đường về vẫn nhớ một người yêu dấu</span>",
                "<span begin='1s' end='2s'>T</span><span begin='2s' end='9s'>ôi </span><span begin='2s' end='9s'>nghe những giấc mơ dịu dàng trong đêm</span>",
            ] {
                let mut player = seekable_player();
                player.snapshot.lyrics = crate::lyrics::parse_ttml(&format!(
                    "<tt><p begin='1s' end='10s'>{text}</p></tt>"
                ))
                .map(std::sync::Arc::new);
                let words = raster_player(&mut player, &Settings::default(), scale);
                let lines = raster_player(
                    &mut player,
                    &Settings {
                        lyrics: LyricsMode::Lines,
                        ..Settings::default()
                    },
                    scale,
                );
                assert!(player.layout[0].fragments.len() > 1, "fixture must wrap");
                assert_eq!(player.layout[0].fragments.concat(), player.layout[0].text);
                let width = (420.0 * scale) as usize;
                let mut accented = 0;
                for y in (player.lyrics_top() * scale).ceil() as usize..(451.0 * scale) as usize {
                    for x in 24 * width / 420..396 * width / 420 {
                        let i = (y * width + x) * 4;
                        if words[i..i + 3] != lines[i..i + 3] {
                            accented += 1;
                            assert_ne!(
                                lines[i + 3],
                                0,
                                "highlight extends glyph at {x},{y}, scale {scale}"
                            );
                        }
                    }
                }
                assert!(accented > 100, "fixture must contain highlighted pixels");
            }
        }
    }

    #[test]
    fn expanded_lyric_anchor_follows_timeline_without_a_large_empty_gap() {
        let mut player = player_at_gap(1.5);
        draw_test_player(&mut player, 500.0);
        let first = player.lines.iter().find(|(_, at)| *at == 1.0).unwrap().0;
        assert!(
            first.y - player.progress_y() <= 55.0,
            "timeline-to-lyric gap is {}",
            first.y - player.progress_y()
        );
    }

    #[test]
    fn lyric_viewport_edges_fade_the_raster_without_crossing_footer() {
        let mut player = seekable_player();
        player.snapshot.lyrics = crate::lyrics::parse_lrc(
            "[00:01]First line\n[00:02]Second line\n[00:03]Third line\n[00:04]Fourth line\n[00:05]Fifth line\n[00:06]Sixth line\n[00:07]Seventh line\n[00:08]Eighth line\n[00:09]Ninth gyp line\n[00:10]Last line"
        ).map(std::sync::Arc::new);
        player.browse = Some(3);
        raster_player(&mut player, &Settings::default(), 1.0);
        // Sample a scroll transition so glyphs intersect both fade boundaries.
        // Pin the interpolation clock and last position: otherwise the draw path
        // eases smooth_line toward the browse target using wall-clock time, and a
        // slow CI frame shifts the sampled glyphs out of the edge band.
        player.last_lyric_frame = Instant::now();
        player.last_lyric_position = Some(5.0);
        player.smooth_line = 2.65;
        let pixels = raster_player(&mut player, &Settings::default(), 1.0);
        assert!(
            (player.smooth_line - 2.65).abs() < 0.01,
            "sampled frame must keep the requested scroll position"
        );
        let top = player.lyrics_top() as usize;
        let edge: Vec<_> = (top..top + 6)
            .flat_map(|y| (24..396).map(move |x| (y * 420 + x) * 4 + 3))
            .map(|i| pixels[i])
            .collect();
        assert!(
            edge.iter().any(|&alpha| alpha > 0),
            "fixture must cross the top edge"
        );
        assert!(
            edge.iter().all(|&alpha| alpha <= 128),
            "top edge must fade lyric coverage"
        );
        let bottom: Vec<_> = (445..451)
            .flat_map(|y| (24..396).map(move |x| (y * 420 + x) * 4 + 3))
            .map(|i| pixels[i])
            .collect();
        assert!(
            bottom.iter().any(|&alpha| alpha > 0),
            "fixture must cross the bottom edge"
        );
        assert!(
            bottom.iter().all(|&alpha| alpha <= 128),
            "bottom edge must fade lyric coverage"
        );
        assert!(
            (451..463).all(|y| (24..396).all(|x| pixels[(y * 420 + x) * 4 + 3] == 0)),
            "lyrics must stop before footer"
        );
    }

    #[test]
    fn wrapped_word_fills_first_fragment_before_second() {
        assert_eq!(fragment_fill(0.5, 100.0, 0.0, 50.0), 1.0);
        assert_eq!(fragment_fill(0.5, 100.0, 50.0, 50.0), 0.0);
        assert_eq!(fragment_fill(0.75, 100.0, 50.0, 50.0), 0.5);
    }

    #[test]
    fn waiting_only_appears_in_long_gaps_with_upcoming_lyrics() {
        let lyrics=crate::lyrics::parse_ttml("<tt><p begin='5s' end='6s'>First</p><p begin='6.5s' end='7s'>Second</p><p begin='12s' end='13s'>Last</p></tt>").unwrap();
        assert_eq!(lyric_view(&lyrics, 1.0, true).waiting, Some((0.0, 5.0)));
        assert!(lyric_view(&lyrics, 6.2, true).waiting.is_none());
        assert_eq!(lyric_view(&lyrics, 9.0, true).waiting, Some((7.0, 12.0)));
        let finished = lyric_view(&lyrics, 14.0, true);
        assert_eq!(finished.anchor, 2);
        assert!(finished.waiting.is_none());
        assert!(lyric_view(&lyrics, 9.0, false).waiting.is_none());
        let overlap=crate::lyrics::parse_ttml("<tt><p begin='1s' end='9s'>A</p><p begin='2s' end='3s'>B</p><p begin='11s' end='12s'>C</p></tt>").unwrap();
        assert!(lyric_view(&overlap, 9.1, true).waiting.is_none());
    }

    #[test]
    fn waiting_dots_are_bright_and_clear_of_the_lyric_edge_fade() {
        for scale in [1.0, 1.25, 1.5, 2.0] {
            let mut player = player_at_gap(3.5);
            player.music = true;
            let mut painter = crate::canvas::Painter::new(scale).unwrap();
            let size = ((420.0 * scale) as u32, (500.0 * scale) as u32);
            let mut pixels = vec![0; (size.0 * size.1 * 4) as usize];
            let settings = Settings::default();
            player.draw_at(
                &mut Canvas::new(&mut pixels, size, &mut painter),
                0.0,
                420.0,
                500.0,
                &settings,
                Instant::now(),
            );
            let x = (35.5 * scale) as usize;
            let y = ((254.0 + 21.5) * scale) as usize;
            let offset = (y * size.0 as usize + x) * 4;
            assert!(
                pixels[offset + 3] > 220,
                "waiting indicator must be opaque at {scale}x"
            );
            for channel in 0..3 {
                assert!(
                    (i16::from(pixels[offset + channel])
                        - i16::from(settings.accent_bgra()[channel]))
                    .abs()
                        <= 2
                );
            }
        }
    }

    #[test]
    fn lyric_highlight_fades_at_boundaries_on_the_playback_clock() {
        let lyrics = crate::lyrics::parse_lrc("[00:01]One\n[00:02]Two").unwrap();
        let line = &lyrics.lines[0];
        assert_eq!(lyric_emphasis(line, 0.9), 0.0);
        assert!((lyric_emphasis(line, 1.06) - 0.5).abs() < 0.001);
        assert_eq!(lyric_emphasis(line, 1.5), 1.0);
        assert!((lyric_emphasis(line, 2.09) - 0.5).abs() < 0.001);
        assert_eq!(lyric_emphasis(line, 2.3), 0.0);
    }

    fn player_at_gap(position: f64) -> Player {
        let mut player = seekable_player();
        player.snapshot.lyrics = crate::lyrics::parse_ttml(
            "<tt><p begin='1s' end='2s'>First</p><p begin='5s' end='6s'>Last</p></tt>",
        )
        .map(std::sync::Arc::new);
        player.snapshot.timeline.as_mut().unwrap().position = position;
        player.last_lyric_frame = Instant::now() - std::time::Duration::from_millis(16);
        player
    }

    #[test]
    fn finished_lyrics_keep_final_anchor_instead_of_returning_to_first() {
        let mut player = player_at_gap(7.0);
        player.smooth_line = 1.0;
        draw_test_player(&mut player, 500.0);
        assert_eq!(player.smooth_line, 1.0);
        assert!(!player.following);
    }

    #[test]
    fn compact_lyric_shows_upcoming_then_live_and_keeps_final_line() {
        let mut player = player_at_gap(0.5);
        let now = Instant::now();
        let (line, at) = player
            .compact_lyric(now, 0)
            .expect("pre-roll must retain a lyric capsule");
        assert_eq!(line.text(), "First");
        assert_eq!(line.start - at, 0.5);
        player.snapshot.timeline.as_mut().unwrap().position = 1.5;
        let (line, at) = player.compact_lyric(now, 0).unwrap();
        assert_eq!(line.text(), "First");
        assert!(at >= line.start && at < line.end);
        player.snapshot.timeline.as_mut().unwrap().position = 3.0;
        let (line, at) = player.compact_lyric(now, 0).unwrap();
        assert_eq!(line.text(), "Last");
        assert_eq!(line.start - at, 2.0);
        player.snapshot.timeline.as_mut().unwrap().position = 7.0;
        assert_eq!(player.compact_lyric(now, 0).unwrap().0.text(), "Last");
        player.snapshot.timeline = None;
        assert!(player.compact_lyric(now, 0).is_none());
    }

    #[test]
    fn drag_seek_previews_locally_and_commits_only_the_final_position() {
        let mut player = seekable_player();
        player.worker = MediaWorker::test_worker();
        player.dragging = true;
        for x in [10.0, 30.0, 80.0] {
            player.seek(x, 0.0, 100.0);
            assert_eq!(player.worker.take_test_seek(), None);
        }
        assert_eq!(player.position(), 160.0);
        player.finish_seek();
        assert_eq!(player.worker.take_test_seek(), Some(160.0));
        player.finish_seek();
        assert_eq!(player.worker.take_test_seek(), None);
        player.seek_to(25.0);
        assert_eq!(player.worker.take_test_seek(), Some(25.0));
    }

    #[test]
    fn canceled_drag_rolls_back_without_seeking_another_track() {
        let mut player = seekable_player();
        player.worker = MediaWorker::test_worker();
        player.dragging = true;
        player.seek(80.0, 0.0, 100.0);
        player.cancel_seek();
        assert!(player.seek_preview.is_none());
        assert_eq!(player.worker.take_test_seek(), None);
        player.dragging = true;
        player.seek(80.0, 0.0, 100.0);
        player.snapshot.generation += 1;
        player.finish_seek();
        assert!(player.seek_preview.is_none());
        assert_eq!(player.worker.take_test_seek(), None);
    }

    #[test]
    fn generation_change_cancels_drag_before_mouse_release_on_same_track_key() {
        let mut player = seekable_player();
        player.worker = MediaWorker::test_worker();
        player.dragging = true;
        player.seek(80.0, 0.0, 100.0);
        let mut next = player.snapshot.clone();
        next.generation += 1;
        player.reconcile_seek(&next, Instant::now());

        assert!(!player.dragging);
        assert!(player.seek_preview.is_none());
        player.snapshot = next;
        // Mouse release updates the final pointer position only during an active drag.
        if player.dragging {
            player.seek(90.0, 0.0, 100.0);
        }
        player.finish_seek();
        assert_eq!(player.worker.take_test_seek(), None);
    }

    #[test]
    fn track_key_change_cancels_drag_before_mouse_release() {
        let mut player = seekable_player();
        player.worker = MediaWorker::test_worker();
        player.dragging = true;
        player.seek(80.0, 0.0, 100.0);
        let mut next = player.snapshot.clone();
        next.key = "another track".into();
        player.reconcile_seek(&next, Instant::now());

        assert!(!player.dragging);
        assert!(player.seek_preview.is_none());
        player.snapshot = next;
        if player.dragging {
            player.seek(90.0, 0.0, 100.0);
        }
        player.finish_seek();
        assert_eq!(player.worker.take_test_seek(), None);
    }

    #[test]
    fn ordinary_same_track_refresh_keeps_drag_and_preview_until_release() {
        let mut player = seekable_player();
        player.worker = MediaWorker::test_worker();
        player.dragging = true;
        player.seek(80.0, 0.0, 100.0);
        let issued = player.seek_preview.as_ref().unwrap().sampled;
        let mut next = player.snapshot.clone();
        next.timeline.as_mut().unwrap().position = 160.0;
        next.timeline.as_mut().unwrap().sampled = issued + std::time::Duration::from_secs(3);
        player.reconcile_seek(&next, issued + std::time::Duration::from_secs(3));

        assert!(player.dragging);
        assert!(player.seek_preview.is_some());
        assert_eq!(player.worker.take_test_seek(), None);
        player.snapshot = next;
        player.seek(90.0, 0.0, 100.0);
        player.finish_seek();
        assert_eq!(player.worker.take_test_seek(), Some(180.0));
    }

    #[test]
    fn gap_keeps_previous_line_until_next_lyric_starts() {
        let mut player = player_at_gap(2.1);
        draw_test_player(&mut player, 500.0);
        assert_eq!(player.smooth_line, 0.0);
    }

    #[test]
    fn control_wheel_browses_lyrics_without_changing_page() {
        let mut player = player_at_gap(1.5);
        player.music = true;
        draw_test_player(&mut player, 500.0);
        player.wheel(true, true);
        assert_eq!(player.browse, Some(1));
        assert!(player.music);
        player.wheel(false, true);
        assert_eq!(player.browse, Some(0));
        assert!(player.music);
        player.wheel(false, true);
        assert_eq!(player.browse, Some(0));
        assert!(player.music);
        player.select_page(false);
        player.wheel(true, true);
        assert!(!player.music);
    }

    #[test]
    fn browsing_paused_lyrics_starts_smoothly_after_idle() {
        let mut player = player_at_gap(1.5);
        player.music = true;
        draw_test_player(&mut player, 500.0);
        player.last_lyric_frame = Instant::now() - std::time::Duration::from_secs(2);
        player.wheel(true, true);
        draw_test_player(&mut player, 500.0);
        assert!(
            player.smooth_line < 0.3,
            "idle time must not advance a new scroll animation"
        );
    }

    #[test]
    fn repeated_browse_input_keeps_the_running_scroll_clock() {
        let mut player = player_at_gap(1.5);
        player.music = true;
        draw_test_player(&mut player, 500.0);
        player.wheel(true, true);
        draw_test_player(&mut player, 500.0);
        assert!(player.following);
        let previous_frame = Instant::now() - std::time::Duration::from_millis(16);
        player.last_lyric_frame = previous_frame;
        player.wheel(true, true);
        assert_eq!(
            player.last_lyric_frame, previous_frame,
            "boundary input must not restart animation"
        );
        player.wheel(false, true);
        assert_eq!(
            player.last_lyric_frame, previous_frame,
            "direction changes must preserve elapsed frame time"
        );
    }

    fn seekable_player() -> Player {
        let mut player = Player::with_worker(MediaWorker::test_worker());
        player.snapshot.key = "track".into();
        player.snapshot.can_seek = true;
        player.snapshot.timeline = Some(crate::media::Timeline {
            position: 5.0,
            duration: 200.0,
            playing: false,
            rate: 1.0,
            sampled: Instant::now(),
        });
        player
    }

    #[test]
    fn dragging_seek_updates_display_before_worker_snapshot() {
        let mut player = seekable_player();
        player.dragging = true;
        player.seek(200.0, 0.0, 400.0);
        assert_eq!(player.position(), 100.0);
        player.seek(320.0, 0.0, 400.0);
        assert!((player.position() - 160.0).abs() < 0.0001);
        assert!(player.needs_frame());
    }

    #[test]
    fn repeated_keyboard_seeks_accumulate_before_worker_updates() {
        let mut player = seekable_player();
        player.seek_relative(5.0);
        player.seek_relative(5.0);
        assert_eq!(player.position(), 15.0);
    }

    #[test]
    fn single_line_lyrics_have_compact_spacing() {
        let mut player = player_at_gap(1.5);
        player.snapshot.key = "track".into();
        player.snapshot.lyrics =
            crate::lyrics::parse_lrc("[00:01]First\n[00:02]Second").map(std::sync::Arc::new);
        draw_test_player(&mut player, 500.0);
        assert!(player.layout[1].y - player.layout[0].y <= 30.0);
        assert!(player.layout[0].height >= 25.0);
    }

    #[test]
    fn stale_timeline_does_not_snap_released_seek_back() {
        let mut player = seekable_player();
        player.seek(200.0, 0.0, 400.0);
        let issued = player.seek_preview.as_ref().unwrap().sampled;
        let mut next = player.snapshot.clone();
        player.reconcile_seek(&next, issued + std::time::Duration::from_millis(300));
        assert!((player.position() - 100.0).abs() < 0.001);
        next.timeline.as_mut().unwrap().position = 100.0;
        next.timeline.as_mut().unwrap().sampled = issued + std::time::Duration::from_millis(350);
        player.reconcile_seek(&next, issued + std::time::Duration::from_millis(350));
        assert!(player.seek_preview.is_none());
    }

    #[test]
    fn seek_preview_ends_on_track_change_or_failed_acknowledgement() {
        let mut player = seekable_player();
        player.seek_to(100.0);
        let mut next = player.snapshot.clone();
        next.generation += 1;
        player.reconcile_seek(&next, Instant::now());
        assert!(player.seek_preview.is_none());
        player.seek_to(100.0);
        let issued = player.seek_preview.as_ref().unwrap().sampled;
        player.dirty = false;
        player.reconcile_seek(
            &player.snapshot.clone(),
            issued + std::time::Duration::from_secs(3),
        );
        assert!(player.seek_preview.is_none());
        assert!(player.needs_frame());
    }

    #[test]
    fn wheel_up_switches_to_clock_even_over_later_lyrics() {
        let mut player = Player::with_worker(MediaWorker::test_worker());
        player.music = true;
        player.browse = Some(12);
        player.wheel(false, false);
        assert!(!player.music);
        assert!(player.browse.is_none());
    }

    #[test]
    fn leaving_picker_for_clock_does_not_trap_page_navigation() {
        let mut player = Player::with_worker(MediaWorker::test_worker());
        player.music = true;
        player.source_picker = true;
        player.select_page(false);
        player.wheel(true, false);
        assert!(player.music);
        assert!(!player.source_picker);
    }

    #[test]
    fn player_navigation_uses_page_dots_instead_of_back() {
        let player = Player::with_worker(MediaWorker::test_worker());
        let nodes = player.accessible_nodes(0.0, 420.0);
        assert!(nodes.iter().any(|n| n.name == "Clock page"));
        assert!(nodes.iter().any(|n| n.name == "Music page"));
        assert!(!nodes.iter().any(|n| n.name.contains("Back")));
    }

    fn draw_test_player(player: &mut Player, height: f32) -> Vec<u8> {
        let mut painter = crate::canvas::Painter::new(1.0).unwrap();
        let mut pixels = vec![0; 420 * height as usize * 4];
        let mut canvas = Canvas::new(&mut pixels, (420, height as u32), &mut painter);
        player.draw(&mut canvas, 0.0, 420.0, height, &Settings::default());
        pixels
    }

    fn alignment_player() -> Player {
        let mut player = Player::with_worker(MediaWorker::test_worker());
        player.music = true;
        player.snapshot = MediaSnapshot {
            key: "alignment".into(),
            title: "Header".into(),
            artist: "Artist".into(),
            source: "Spotify.exe".into(),
            // Saturated red artwork distinguishes cover pixels from UI text in alignment tests.
            artwork: Some(std::sync::Arc::new(image::RgbaImage::from_pixel(
                32,
                32,
                image::Rgba([255, 0, 0, 255]),
            ))),
            can_toggle: true,
            can_previous: true,
            can_next: true,
            ..Default::default()
        };
        player
    }

    #[test]
    fn source_picker_uses_the_shared_raised_surface() {
        let mut player = alignment_player();
        player.source_picker = true;
        player.snapshot.sources = vec!["Spotify.exe".into()];
        let pixels = draw_test_player(&mut player, 500.0);
        let sample = (64 * 420 + 22) * 4;
        assert_eq!(&pixels[sample..sample + 3], &crate::theme::SURFACE_RAISED);
        assert_eq!(pixels[sample + 3], 255);
    }

    #[test]
    fn track_card_centers_cover_and_metadata_above_transport_at_each_dpi() {
        for height in [300.0, 360.0, 500.0] {
            for scale in [1.0, 1.25, 1.5, 2.0] {
                let size = ((420.0 * scale) as u32, (height * scale) as u32);
                let mut painter = crate::canvas::Painter::new(scale).unwrap();
                let mut pixels = vec![0; (size.0 * size.1 * 4) as usize];
                let mut player = alignment_player();
                player.draw(
                    &mut Canvas::new(&mut pixels, size, &mut painter),
                    0.0,
                    420.0,
                    height,
                    &Settings::default(),
                );
                let card = if height < 400.0 {
                    ((46.0 * scale) as u32, (104.0 * scale) as u32 - 1)
                } else {
                    ((50.0 * scale) as u32, (134.0 * scale) as u32 - 1)
                };
                let mut cover = (u32::MAX, 0);
                let mut text = (u32::MAX, 0);
                for y in (44.0 * scale) as u32..(140.0 * scale) as u32 {
                    for x in (18.0 * scale) as u32..(400.0 * scale) as u32 {
                        let i = ((y * size.0 + x) * 4) as usize;
                        let p = &pixels[i..i + 4];
                        let bounds =
                            if x < (100.0 * scale) as u32 && p[2] > 240 && p[0] < 5 && p[1] < 5 {
                                Some(&mut cover)
                            } else if x >= (100.0 * scale) as u32
                                && y < ((if height < 400.0 { 104.0 } else { 134.0 }) * scale) as u32
                                && p[0] > 80
                            {
                                Some(&mut text)
                            } else {
                                None
                            };
                        if let Some(bounds) = bounds {
                            bounds.0 = bounds.0.min(y);
                            bounds.1 = bounds.1.max(y);
                        }
                    }
                }
                assert!(
                    cover.0 < cover.1 && text.0 < text.1,
                    "artwork and metadata must be visible"
                );
                let context = format!(
                    "height={height}, scale={scale}, card={card:?}, cover={cover:?}, text={text:?}"
                );
                assert!(
                    cover.0 > card.0 && cover.1 < card.1,
                    "artwork needs padding above and below: {context}"
                );
                let center = |bounds: (u32, u32)| (bounds.0 + bounds.1) as f32 * 0.5;
                assert!(
                    (center(cover) - center(card)).abs() <= scale,
                    "artwork must be vertically centered: {context}"
                );
                assert!(
                    (center(text) - center(card)).abs() <= 3.0 * scale,
                    "title/artist group must be vertically centered: {context}"
                );
                let controls_top = if height < 400.0 { 110.0 } else { 144.0 };
                assert!(
                    (card.1 as f32) < (controls_top - 3.0) * scale,
                    "card must not overlap transport controls: {context}"
                );
            }
        }
    }

    #[test]
    #[ignore = "Writes native player card alignment fixtures"]
    fn player_alignment_render_preview() {
        let dir = std::env::var_os("HIYODYNA_PREVIEW_DIR")
            .map(std::path::PathBuf::from)
            .expect("preview directory");
        std::fs::create_dir_all(&dir).unwrap();
        let live_cover = std::env::var_os("HIYODYNA_COVER_FILE")
            .map(|path| std::sync::Arc::new(image::open(path).unwrap().into_rgba8()));
        for height in [300.0, 360.0, 500.0] {
            for scale in [1.0, 1.25, 1.5, 2.0] {
                let size = ((460.0 * scale) as u32, ((height + 30.0) * scale) as u32);
                let mut painter = crate::canvas::Painter::new(scale).unwrap();
                let mut pixels = vec![0; (size.0 * size.1 * 4) as usize];
                crate::render::render_frame_with_margin(
                    &mut pixels,
                    size,
                    crate::render::RenderParams {
                        state: crate::animation::ShapeState {
                            width: 420.0 * scale,
                            height: height * scale,
                            radius: 30.0 * scale,
                        },
                        top_margin: 0.0,
                        attached: true,
                        scale,
                        shadow_opacity: 0.0,
                        hover: 0.0,
                        accent: Settings::default().accent_bgra(),
                    },
                );
                let mut player = alignment_player();
                player.snapshot.title = "Ngủ Quên".into();
                player.snapshot.artist = "Wren Evans".into();
                if live_cover.is_some() {
                    player.snapshot.artwork = live_cover.clone();
                }
                player.snapshot.timeline = Some(crate::media::Timeline {
                    position: 58.0,
                    duration: 154.0,
                    playing: false,
                    rate: 1.0,
                    sampled: Instant::now(),
                });
                player.snapshot.lyrics = crate::lyrics::parse_lrc("[00:57]Chờ em quay đi, quay về\n[01:02]Không đoán ra tâm tình\n[01:07]Ngồi đây một mình đến khi\n[01:12]Ai-ai-ai gọi anh thức dậy?\n[01:17]Mở mắt, níu lấy tay nhưng sao em không còn đây?").map(std::sync::Arc::new);
                let mut canvas = Canvas::new(&mut pixels, size, &mut painter);
                player.draw(&mut canvas, 20.0, 420.0, height, &Settings::default());
                draw_page_dots(&mut canvas, 20.0, 420.0, height, 1.0);
                for pixel in pixels.as_chunks_mut::<4>().0 {
                    pixel.swap(0, 2);
                }
                image::save_buffer(
                    dir.join(format!("player-alignment-{height}-{scale}.png")),
                    &pixels,
                    size.0,
                    size.1,
                    image::ColorType::Rgba8,
                )
                .unwrap();
            }
        }
    }

    #[test]
    fn replaced_lyrics_cannot_reuse_stale_layout_even_with_same_address_key() {
        let mut player = player_at_gap(1.5);
        player.snapshot.key = "track".into();
        player.snapshot.lyrics = crate::lyrics::parse_lrc("[00:01]Old").map(std::sync::Arc::new);
        draw_test_player(&mut player, 500.0);
        let replacement =
            std::sync::Arc::new(crate::lyrics::parse_lrc("[00:01]New\n[00:02]Second").unwrap());
        // Model an allocator reusing the old address after an intervening empty snapshot.
        player.layout_key = (
            std::sync::Arc::as_ptr(&replacement) as usize,
            420.0_f32.to_bits(),
        );
        player.snapshot.lyrics = Some(replacement);
        draw_test_player(&mut player, 500.0);
        assert_eq!(player.layout[0].text, "New");
        assert_eq!(player.layout.len(), 2);
    }

    #[test]
    fn minimum_player_height_keeps_selected_timed_lyric_visible() {
        let mut player = Player::with_worker(MediaWorker::test_worker());
        player.snapshot.key = "track".into();
        player.snapshot.lyrics =
            crate::lyrics::parse_lrc("[00:01]Active lyric\n[00:10]Next").map(std::sync::Arc::new);
        player.snapshot.timeline = Some(crate::media::Timeline {
            position: 2.0,
            duration: 30.0,
            playing: false,
            rate: 1.0,
            sampled: Instant::now(),
        });
        draw_test_player(&mut player, 300.0);
        assert!(
            player
                .lines
                .iter()
                .any(|(rect, time)| *time == 1.0 && rect.h >= 25.0)
        );
    }

    #[test]
    fn shrinking_sources_keeps_remaining_picker_choices_visible() {
        let mut player = Player::with_worker(MediaWorker::test_worker());
        player.snapshot.key = "track".into();
        player.source_picker = true;
        player.source_scroll = 20;
        player.snapshot.sources = vec!["Spotify.exe".into()];
        draw_test_player(&mut player, 500.0);
        assert!(
            player
                .accessible_nodes(0.0, 420.0)
                .iter()
                .any(|n| n.name == "Use Spotify")
        );
    }

    #[test]
    fn playing_without_timeline_exposes_pause_action() {
        let mut player = Player::with_worker(MediaWorker::test_worker());
        player.snapshot.key = "track".into();
        player.snapshot.playing = true;
        assert!(
            player
                .accessible_nodes(0.0, 420.0)
                .iter()
                .any(|n| n.name == "Pause")
        );
    }

    #[test]
    fn compact_resume_click_never_seeks_in_overlap() {
        let mut player = Player::with_worker(MediaWorker::test_worker());
        player.height = 300.0;
        player.music = true;
        player.browse = Some(1);
        assert!(player.click(330.0, 180.0, 0.0, 420.0));
        assert!(player.browse.is_none());
        assert!(!player.dragging);
    }
    #[test]
    fn keyboard_browses_lines_and_wheel_switches_pages() {
        let mut player = Player::with_worker(MediaWorker::test_worker());
        player.music = true;
        player.snapshot.lyrics =
            crate::lyrics::parse_lrc("[00:01]First\n[00:02]Second\n[00:03]Third")
                .map(std::sync::Arc::new);
        player.browse_lines(true);
        assert_eq!(player.browse, Some(1));
        player.browse_lines(false);
        assert_eq!(player.browse, Some(0));
        player.wheel(false, false);
        assert!(!player.music);
    }
    #[test]
    fn wraps_unicode_without_dropping_bytes() {
        let mut painter = crate::canvas::Painter::new(1.0).unwrap();
        let mut pixels = vec![0; 320 * 200 * 4];
        let mut c = Canvas::new(&mut pixels, (320, 200), &mut painter);
        let text = "Một dòng hát 日本語 🎵";
        let lines = wrap(text, &mut c, 90.0, 21.0);
        assert!(lines.len() > 1);
        assert_eq!(lines.concat(), text);
    }

    #[test]
    fn stable_paused_snapshot_does_not_request_continuous_frames() {
        let mut player = Player::with_worker(MediaWorker::test_worker());
        player.dirty = false;
        assert!(!player.needs_frame());
        player.wheel(true, false);
        assert!(player.needs_frame());
    }

    #[test]
    fn word_masks_keep_spaces_and_a_consistent_baseline() {
        let mut painter = crate::canvas::Painter::new(1.0).unwrap();
        assert!(painter.width("Hello ", 21.0, true) > painter.width("Hello", 21.0, true));
        assert!(painter.width(" ", 21.0, true) > 0.0);
    }

    #[test]
    #[ignore = "Writes native renderer previews and measures release rendering"]
    fn render_preview() {
        let dir = std::env::var_os("HIYODYNA_PREVIEW_DIR")
            .map(std::path::PathBuf::from)
            .expect("preview directory");
        std::fs::create_dir_all(&dir).unwrap();
        let mut player = Player::with_worker(MediaWorker::test_worker());
        player.music = true;
        player.snapshot=MediaSnapshot{key:"preview".into(),title:"Blinding Lights".into(),artist:"The Weeknd".into(),source:"Spotify.exe".into(),sources:vec!["Spotify.exe".into(),"Music.exe".into()],can_toggle:true,can_next:true,can_previous:true,can_seek:true,
            timeline:Some(crate::media::Timeline{position:31.0,duration:200.0,playing:false,rate:1.0,sampled:Instant::now()}),
            lyrics:crate::lyrics::parse_ttml("<tt><body><p begin='27s' end='29s'>I've been tryna call</p><p begin='30s' end='34s'><span begin='30s' end='30.5s'>I've </span><span begin='30.5s' end='31s'>been </span><span begin='31s' end='32s'>on my own </span><span begin='32s' end='34s'>for long enough</span></p><p begin='35s' end='38s'>Maybe you can show me how to love</p><p begin='39s' end='41s'>Maybe</p></body></tt>").map(|mut l|{l.source="AMLL TTML DB".into();std::sync::Arc::new(l)}),..Default::default()};
        player.smooth_line = 1.0;
        for scale in [1.0, 1.25, 1.5, 2.0] {
            let size = ((520.0 * scale) as u32, (580.0 * scale) as u32);
            let mut pixels = vec![0; size.0 as usize * size.1 as usize * 4];
            let mut background = pixels.clone();
            let mut painter = crate::canvas::Painter::new(scale).unwrap();
            let base_start = Instant::now();
            crate::render::render_frame_with_margin(
                &mut background,
                size,
                crate::render::RenderParams {
                    state: crate::animation::ShapeState {
                        width: 420.0 * scale,
                        height: 500.0 * scale,
                        radius: 30.0 * scale,
                    },
                    top_margin: 0.0,
                    scale,
                    attached: true,
                    shadow_opacity: 0.22,
                    hover: 1.0,
                    accent: Settings::default().accent_bgra(),
                },
            );
            let base_ms = base_start.elapsed().as_secs_f64() * 1000.0;
            let start = Instant::now();
            for _ in 0..30 {
                pixels.copy_from_slice(&background);
                player.draw(
                    &mut Canvas::new(&mut pixels, size, &mut painter),
                    50.0,
                    420.0,
                    500.0,
                    &Settings::default(),
                );
            }
            println!(
                "{scale}x: cached background {base_ms:.2} ms, content {:.2} ms/frame including cold text",
                start.elapsed().as_secs_f64() * 1000.0 / 30.0
            );
            for p in pixels.as_chunks_mut::<4>().0 {
                p.swap(0, 2);
            }
            image::save_buffer(
                dir.join(format!("player-{scale}.png")),
                &pixels,
                size.0,
                size.1,
                image::ColorType::Rgba8,
            )
            .unwrap();
            if scale == 1.0 {
                for (label, position) in [
                    ("intro", 20.0),
                    ("word", 31.5),
                    ("short-gap", 34.5),
                    ("finished", 45.0),
                ] {
                    player.snapshot.timeline.as_mut().unwrap().position = position;
                    pixels.copy_from_slice(&background);
                    player.draw(
                        &mut Canvas::new(&mut pixels, size, &mut painter),
                        50.0,
                        420.0,
                        500.0,
                        &Settings::default(),
                    );
                    for pixel in pixels.as_chunks_mut::<4>().0 {
                        pixel.swap(0, 2);
                    }
                    image::save_buffer(
                        dir.join(format!("lyrics-{label}.png")),
                        &pixels,
                        size.0,
                        size.1,
                        image::ColorType::Rgba8,
                    )
                    .unwrap();
                }
                player.snapshot.timeline.as_mut().unwrap().position = 31.0;
            }
        }
        for scale in [1.0, 1.25, 1.5, 2.0] {
            let size = ((520.0 * scale) as u32, (580.0 * scale) as u32);
            let mut painter = crate::canvas::Painter::new(scale).unwrap();
            let mut background = vec![0; size.0 as usize * size.1 as usize * 4];
            crate::render::render_frame_with_margin(
                &mut background,
                size,
                crate::render::RenderParams {
                    state: crate::animation::ShapeState {
                        width: 420.0 * scale,
                        height: 500.0 * scale,
                        radius: 30.0 * scale,
                    },
                    top_margin: 0.0,
                    scale,
                    attached: true,
                    shadow_opacity: 0.22,
                    hover: 1.0,
                    accent: Settings::default().accent_bgra(),
                },
            );
            for (label, position, lines, browse, seek) in [
                ("vietnamese-words", 12.7, false, None, None),
                ("vietnamese-lines", 12.7, true, None, None),
                ("vietnamese-short", 22.0, false, None, None),
                ("vietnamese-browse", 12.7, false, Some(5), None),
                ("vietnamese-browse-active", 12.7, false, Some(2), None),
                ("vietnamese-browse-active-later", 15.5, false, Some(2), None),
                ("vietnamese-seek", 12.7, false, None, Some(22.0)),
            ] {
                let mut player = seekable_player();
                player.music = true;
                player.snapshot.title = "Vietnamese lyric preview".into();
                player.snapshot.artist = "Paused software rendering".into();
                player.snapshot.source = "Spotify.exe".into();
                player.snapshot.can_toggle = true;
                player.snapshot.can_previous = true;
                player.snapshot.can_next = true;
                player.snapshot.timeline.as_mut().unwrap().position = position;
                player.snapshot.lyrics = crate::lyrics::parse_ttml(
                    "<tt><p begin='0s' end='5s'>Mình vẫn ở đây</p><p begin='5s' end='10s'>Lắng nghe tiếng mưa rơi</p><p begin='10s' end='20s'><span begin='10s' end='11s'>Tôi </span><span begin='11s' end='12s'>nghe </span><span begin='12s' end='14s'>những giấc mơ </span><span begin='14s' end='17s'>dịu dàng trong đêm, </span><span begin='17s' end='20s'>đường về vẫn nhớ một người yêu dấu</span></p><p begin='20s' end='25s'>Một đời thương nhớ</p><p begin='25s' end='30s'>Bình yên ở lại</p><p begin='30s' end='35s'>Ngày mai nắng sẽ lên</p><p begin='35s' end='40s'>Mình đi qua những mùa xa</p><p begin='40s' end='45s'>Vẫn nhớ tiếng em cười</p><p begin='45s' end='50s'>Giữ lấy những điều dịu dàng</p></tt>"
                ).map(|mut lyrics| { lyrics.source = "Software fixture".into(); std::sync::Arc::new(lyrics) });
                player.browse = browse;
                if let Some(seconds) = seek {
                    player.seek_to(seconds);
                }
                let settings = Settings {
                    lyrics: if lines {
                        LyricsMode::Lines
                    } else {
                        Settings::default().lyrics
                    },
                    ..Settings::default()
                };
                let mut pixels = background.clone();
                let now = player.snapshot.timeline.as_ref().unwrap().sampled;
                player.draw_at(
                    &mut Canvas::new(&mut pixels, size, &mut painter),
                    50.0,
                    420.0,
                    500.0,
                    &settings,
                    now,
                );
                draw_page_dots(
                    &mut Canvas::new(&mut pixels, size, &mut painter),
                    50.0,
                    420.0,
                    500.0,
                    1.0,
                );
                for pixel in pixels.as_chunks_mut::<4>().0 {
                    pixel.swap(0, 2);
                }
                image::save_buffer(
                    dir.join(format!("{label}-{scale}.png")),
                    &pixels,
                    size.0,
                    size.1,
                    image::ColorType::Rgba8,
                )
                .unwrap();
            }
        }
    }
}
