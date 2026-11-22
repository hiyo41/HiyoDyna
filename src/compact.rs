use crate::animation::{COMPACT, ShapeState};
use crate::canvas::{Canvas, MUTED, Painter, Rect, WHITE};
use crate::lyrics::LyricLine;
use crate::settings::{LyricsMode, Settings};
use crate::theme;

const SIZE: f32 = 13.0;
pub(crate) const SOURCE_SPACE: f32 = 13.0;

pub(crate) fn wait_pulse(remaining: f64, playing: bool, visible: bool) -> f32 {
    if playing && visible && remaining > 0.0 && remaining <= 3.0 {
        (0.8 + 0.2 * (std::f64::consts::TAU * (3.0 - remaining) / 1.6).cos()) as f32
    } else {
        1.0
    }
}

#[derive(Clone)]
struct Frame {
    line: LyricLine,
    text: String,
    at: f64,
}

struct Layer {
    frame: Frame,
    opacity: f32,
    offset: f32,
}

#[derive(Default)]
pub(crate) struct Presentation {
    track: String,
    generation: u64,
    current: Option<Frame>,
    previous: Vec<Layer>,
    changed: Option<std::time::Instant>,
    updated: Option<std::time::Instant>,
}

impl Presentation {
    pub fn matches(&self, track: &str, generation: u64) -> bool {
        self.current.is_some() && self.track == track && self.generation == generation
    }
    pub fn update(
        &mut self,
        track: &str,
        generation: u64,
        line: &LyricLine,
        at: f64,
        now: std::time::Instant,
    ) {
        let text = line.text();
        let same_track = self.track == track && self.generation == generation;
        let changed = !same_track
            || self.current.as_ref().is_none_or(|current| {
                current.line.start != line.start
                    || current.text != text
                    || current.line.end != line.end
            });
        if changed {
            self.previous = if same_track
                && self.updated.is_some_and(|updated| {
                    now.saturating_duration_since(updated).as_secs_f32() < 0.5
                }) {
                self.layers(now)
            } else {
                Vec::new()
            };
            self.changed = Some(now);
            self.current = Some(Frame {
                line: line.clone(),
                text,
                at,
            });
            self.track = track.into();
            self.generation = generation;
        } else if let Some(current) = &mut self.current {
            if current.line.word_sync != line.word_sync
                || current.line.words.len() != line.words.len()
                || current
                    .line
                    .words
                    .iter()
                    .zip(&line.words)
                    .any(|(a, b)| a.text != b.text || a.start != b.start || a.end != b.end)
            {
                current.line.clone_from(line);
                self.previous.clear();
            }
            if at < current.at - 0.1 || at - current.at > 0.75 {
                self.previous.clear();
            }
            current.at = at;
        }
        self.updated = Some(now);
        if !self.moving(now) {
            self.previous.clear();
        }
    }

    pub fn moving(&self, now: std::time::Instant) -> bool {
        !self.previous.is_empty()
            && self
                .changed
                .is_some_and(|changed| now.saturating_duration_since(changed).as_secs_f32() < 0.16)
    }

    fn mix(&self, now: std::time::Instant) -> f32 {
        if self.previous.is_empty() {
            return 1.0;
        }
        let mix = self.changed.map_or(1.0, |changed| {
            (now.saturating_duration_since(changed).as_secs_f32() / 0.16).clamp(0.0, 1.0)
        });
        mix * mix * (3.0 - 2.0 * mix)
    }

    fn layers(&self, now: std::time::Instant) -> Vec<Layer> {
        let mix = self.mix(now);
        let mut layers: Vec<_> = self
            .previous
            .iter()
            .filter_map(|layer| {
                let opacity = layer.opacity * (1.0 - mix);
                (opacity > 0.001).then(|| Layer {
                    frame: layer.frame.clone(),
                    opacity,
                    offset: layer.offset - 3.0 * mix,
                })
            })
            .collect();
        if let Some(current) = &self.current
            && mix > 0.0
        {
            layers.push(Layer {
                frame: current.clone(),
                opacity: mix,
                offset: 3.0 * (1.0 - mix),
            });
        }
        layers
    }

    pub fn draw(
        &self,
        c: &mut Canvas<'_>,
        left: f32,
        state: ShapeState,
        settings: &Settings,
        now: std::time::Instant,
        max_width: f32,
    ) {
        let opacity = c.opacity;
        let mix = self.mix(now);
        for previous in &self.previous {
            c.opacity = opacity * previous.opacity * (1.0 - mix);
            draw_text(
                c,
                (left, previous.offset - 3.0 * mix),
                state,
                (&previous.frame.line, previous.frame.at),
                settings,
                max_width,
                false,
            );
        }
        if let Some(current) = &self.current {
            c.opacity = opacity * mix;
            draw_text(
                c,
                (left, 3.0 * (1.0 - mix)),
                state,
                (&current.line, current.at),
                settings,
                max_width,
                true,
            );
        }
        c.opacity = opacity;
        draw_note(c, left, state.height);
    }
}

#[cfg(test)]
pub(crate) fn shape(painter: &mut Painter, line: Option<&LyricLine>, max_width: f32) -> ShapeState {
    shape_at(painter, line, f64::INFINITY, max_width)
}

pub(crate) fn shape_at(
    painter: &mut Painter,
    line: Option<&LyricLine>,
    at: f64,
    max_width: f32,
) -> ShapeState {
    ShapeState {
        width: line.map_or(COMPACT.width, |line| {
            (painter.width(&line.text(), SIZE, true).ceil()
                + 50.0
                + countdown_width(painter, line, at))
            .clamp(COMPACT.width, max_width.max(COMPACT.width))
        }),
        ..COMPACT
    }
}

fn countdown_label(remaining: f64) -> String {
    let seconds = remaining.clamp(0.0, 86_400.0).ceil() as u64;
    format!("{}:{:02}", seconds / 60, seconds % 60)
}

fn countdown_width(painter: &mut Painter, line: &LyricLine, at: f64) -> f32 {
    let remaining = line.start - at;
    if remaining > 2.8 {
        // Reserve for the longest value so second ticks never resize the capsule.
        let reserved = painter
            .width(&countdown_label(line.start), SIZE, true)
            .ceil()
            + 16.0;
        let t = ((remaining - 2.8) / 0.2).clamp(0.0, 1.0) as f32;
        reserved * t * t * (3.0 - 2.0 * t)
    } else {
        0.0
    }
}

#[cfg(test)]
fn draw(
    c: &mut Canvas<'_>,
    left: f32,
    state: ShapeState,
    line: &LyricLine,
    at: f64,
    settings: &Settings,
) {
    draw_text(
        c,
        (left, 0.0),
        state,
        (line, at),
        settings,
        state.width,
        true,
    );
    draw_note(c, left, state.height);
}

fn draw_text(
    c: &mut Canvas<'_>,
    position: (f32, f32),
    state: ShapeState,
    lyric: (&LyricLine, f64),
    settings: &Settings,
    max_width: f32,
    countdown: bool,
) {
    let (left, vertical_offset) = position;
    let (line, at) = lyric;
    let old_clip = c.clip;
    let text = line.text();
    let upcoming = at < line.start;
    let countdown_width = countdown_width(c.painter, line, at);
    let layout = shape_at(c.painter, Some(line), at, max_width);
    let size = fitted_size(c.painter, &text, layout.width - 50.0 - countdown_width);
    if countdown && line.start - at > 3.0 {
        let opacity = c.opacity;
        c.opacity *= ((line.start - at - 3.0) / 0.18).clamp(0.0, 1.0) as f32;
        c.text(
            &countdown_label(line.start - at),
            Rect::new(
                left + state.width - countdown_width - 2.0,
                (state.height - 22.0) * 0.5 + vertical_offset,
                (countdown_width - 12.0).max(0.0),
                22.0,
            ),
            size,
            true,
            MUTED,
        );
        c.opacity = opacity;
    }
    let rect = Rect::new(
        left + 36.0,
        (state.height - 22.0) * 0.5 + vertical_offset,
        (state.width - 50.0 - countdown_width).max(0.0),
        22.0,
    );
    c.clip = c.clip.intersect(rect);
    let bounds = c.clip;
    // Fit against the sentence's destination, never the spring's intermediate
    // width. Otherwise each frame changes the font and makes lyrics pulse.
    let full_width = c.painter.width(&text, size, true);
    let word_sync = !upcoming && line.has_word_sync() && settings.lyrics != LyricsMode::Lines;
    let text_rect = Rect::new(
        rect.x + ((rect.w - full_width) * 0.5).max(0.0),
        rect.y,
        full_width.max(rect.w),
        rect.h,
    );
    c.text(
        &text,
        text_rect,
        size,
        true,
        if upcoming {
            MUTED
        } else if word_sync {
            theme::mix(WHITE, MUTED, 0.325)
        } else {
            WHITE
        },
    );
    if word_sync {
        let mut offset = 0;
        for word in &line.words {
            let end = offset + word.text.len();
            let before = c.painter.width(&text[..offset], size, true);
            let after = c.painter.width(&text[..end], size, true);
            let fill = if at >= word.end {
                1.0
            } else if at <= word.start {
                0.0
            } else {
                ((at - word.start) / (word.end - word.start)) as f32
            };
            c.clip = bounds.intersect(Rect::new(
                text_rect.x + before,
                rect.y,
                (after - before) * fill,
                rect.h,
            ));
            c.text(&text, text_rect, size, true, WHITE);
            offset = end;
        }
    }
    c.clip = old_clip;
}

fn fitted_size(painter: &mut Painter, text: &str, width: f32) -> f32 {
    let step = 0.25 / painter.scale;
    let mut size = SIZE;
    while size > step {
        let measured = painter.width(text, size, true).ceil();
        if measured <= width {
            break;
        }
        size = (size * (width / measured) - step).max(step);
    }
    size
}

fn draw_note(c: &mut Canvas<'_>, left: f32, height: f32) {
    let y = height * 0.5 - 7.0;
    for (x, y, w, h, r) in [
        (18.0, y + 1.0, 1.8, 11.0, 0.7),
        (26.0, y - 1.0, 1.8, 11.0, 0.7),
        (19.0, y, 8.0, 2.5, 0.7),
        (13.5, y + 9.0, 6.0, 4.5, 2.25),
        (21.5, y + 7.0, 6.0, 4.5, 2.25),
    ] {
        c.rect(Rect::new(left + x, y, w, h), r, MUTED, 1.0);
    }
}

pub(crate) fn draw_source(c: &mut Canvas<'_>, right: f32, height: f32, source: &str) {
    let left = right - 43.0;
    if !source.to_ascii_lowercase().contains("spotify") {
        draw_note(c, left, height);
        return;
    }
    let x = left + 13.0;
    let y = height * 0.5 - 8.0;
    // Spotify's source glyph retains its brand color rather than becoming a theme accent.
    c.rect(Rect::new(x, y, 17.0, 17.0), 8.5, [84, 196, 60], 1.0);
    for row in 0..3 {
        let width = 11.0 - row as f32;
        for step in 0..22 {
            let t = step as f32 / 21.0;
            let curve = (t - 0.42).powi(2) * 3.0;
            c.rect(
                Rect::new(
                    x + 3.0 + t * width,
                    y + 4.0 + row as f32 * 3.2 + curve,
                    1.2,
                    1.2,
                ),
                0.6,
                theme::INVERSE,
                1.0,
            );
        }
    }
}

#[cfg(test)]
mod tests {
    #[test]
    fn note_uses_the_shared_muted_text_color() {
        let mut painter = crate::canvas::Painter::new(1.0).unwrap();
        let mut pixels = vec![0; 60 * 40 * 4];
        super::draw_note(
            &mut crate::canvas::Canvas::new(&mut pixels, (60, 40), &mut painter),
            0.0,
            40.0,
        );
        let center = (24 * 60 + 16) * 4;
        assert_eq!(&pixels[center..center + 3], &crate::theme::MUTED);
        assert_eq!(pixels[center + 3], 255);
    }

    #[test]
    fn pulse_boundaries_and_playback_visibility_gates() {
        for (remaining, playing, visible, expected) in [
            (3.0, true, true, 1.0),
            (2.2, true, true, 0.6),
            (2.2, false, true, 1.0),
            (2.2, true, false, 1.0),
            (4.0, true, true, 1.0),
            (0.0, true, true, 1.0),
            (-0.1, true, true, 1.0),
        ] {
            assert_eq!(super::wait_pulse(remaining, playing, visible), expected);
        }
    }

    #[test]
    fn pulse_has_a_continuous_bounded_one_point_six_second_cycle() {
        for (elapsed, expected) in [(0.0, 1.0), (0.4, 0.8), (0.8, 0.6), (1.2, 0.8), (1.6, 1.0)] {
            assert!((super::wait_pulse(3.0 - elapsed, true, true) - expected).abs() < 0.00001);
        }
        let mut previous = 1.0;
        for step in 0..1600 {
            let remaining = 3.0 - step as f64 / 1000.0;
            let pulse = super::wait_pulse(remaining, true, true);
            assert!((0.6..=1.0).contains(&pulse));
            assert!((pulse - previous).abs() < 0.001);
            if remaining > 1.6 {
                assert!((pulse - super::wait_pulse(remaining - 1.6, true, true)).abs() < 0.00001);
            }
            previous = pulse;
        }
    }

    #[test]
    fn collapse_line_change_preserves_one_continuous_composition() {
        use std::time::{Duration, Instant};
        let now = Instant::now();
        let lyrics = crate::lyrics::parse_lrc("[00:00]First line\n[00:04]Second line").unwrap();
        let mut presentation = super::Presentation::default();
        // Keep receiving snapshots during the expanded page, then change line
        // exactly when collapse begins.
        for frame in 0..60 {
            presentation.update(
                "track",
                1,
                &lyrics.lines[0],
                3.9,
                now + Duration::from_millis(frame * 16),
            );
        }
        let collapse = now + Duration::from_millis(960);
        for frame in 0..60 {
            let when = collapse + Duration::from_millis(frame * 16);
            presentation.update(
                "track",
                1,
                &lyrics.lines[1],
                4.0 + frame as f64 * 0.016,
                when,
            );
            let layers = presentation.layers(when);
            assert!((layers.iter().map(|layer| layer.opacity).sum::<f32>() - 1.0).abs() < 0.001);
            for start in [0.0, 4.0] {
                assert!(
                    layers
                        .iter()
                        .filter(|layer| layer.frame.line.start == start)
                        .count()
                        <= 1,
                    "collapse must not duplicate a lyric layer"
                );
            }
            if frame == 0 {
                assert_eq!(layers.len(), 1);
                assert_eq!(layers[0].frame.line.start, 0.0);
                assert_eq!(layers[0].offset, 0.0);
            }
            if frame >= 10 {
                assert_eq!(layers.len(), 1);
                assert_eq!(layers[0].frame.line.start, 4.0);
                assert_eq!(layers[0].offset, 0.0);
            }
        }
    }

    #[test]
    fn countdown_space_retracts_continuously_after_the_numeric_cutoff() {
        let lyrics =
            crate::lyrics::parse_lrc("[00:10]A sufficiently long upcoming sentence").unwrap();
        let line = &lyrics.lines[0];
        let mut painter = crate::canvas::Painter::new(1.0).unwrap();
        let widths: Vec<_> = [6.999, 7.0, 7.05, 7.1, 7.2]
            .iter()
            .map(|at| super::shape_at(&mut painter, Some(line), *at, 700.0).width)
            .collect();
        assert!(
            (widths[0] - widths[1]).abs() < 0.01,
            "removing the number must not snap the reserved space"
        );
        assert!(widths[1] > widths[2] && widths[2] > widths[3] && widths[3] > widths[4]);
        assert_eq!(
            widths[4],
            super::shape_at(&mut painter, Some(line), 10.0, 700.0).width
        );
    }

    #[test]
    fn interrupted_line_transition_starts_with_the_current_composition() {
        use std::time::{Duration, Instant};
        let now = Instant::now();
        let lyrics =
            crate::lyrics::parse_lrc("[00:00]First line\n[00:04]Second line\n[00:08]Third line")
                .unwrap();
        let mut presentation = super::Presentation::default();
        let paint = |p: &super::Presentation, when| {
            let mut painter = crate::canvas::Painter::new(1.0).unwrap();
            let mut pixels = vec![0; 400 * 60 * 4];
            p.draw(
                &mut crate::canvas::Canvas::new(&mut pixels, (400, 60), &mut painter),
                20.0,
                crate::animation::ShapeState {
                    width: 360.0,
                    ..crate::animation::COMPACT
                },
                &crate::settings::Settings::default(),
                when,
                360.0,
            );
            pixels
        };
        presentation.update("track", 1, &lyrics.lines[0], 1.0, now);
        presentation.update(
            "track",
            1,
            &lyrics.lines[1],
            4.0,
            now + Duration::from_millis(16),
        );
        let halfway = now + Duration::from_millis(80);
        let before = paint(&presentation, halfway);
        presentation.update("track", 1, &lyrics.lines[2], 8.0, halfway);
        assert_eq!(
            before,
            paint(&presentation, halfway),
            "interrupting a fade must preserve its visible layers"
        );
        assert!(!presentation.moving(halfway + Duration::from_millis(200)));
    }
    #[test]
    fn upcoming_countdown_rounds_up_to_the_next_second() {
        assert_eq!(super::countdown_label(4.01), "0:05");
        assert_eq!(super::countdown_label(4.0), "0:04");
        assert_eq!(super::countdown_label(0.2), "0:01");
    }

    #[test]
    fn seeking_to_an_active_line_hides_outgoing_countdown_on_the_first_frame() {
        use std::time::{Duration, Instant};
        let now = Instant::now();
        let lyrics = crate::lyrics::parse_lrc("[00:10]Hi\n[00:20]Now").unwrap();
        for scale in [1.0, 1.25, 1.5, 2.0] {
            let mut presentation = super::Presentation::default();
            let mut painter = crate::canvas::Painter::new(scale).unwrap();
            let size = ((400.0 * scale) as u32, (60.0 * scale) as u32);
            let state = super::shape_at(&mut painter, Some(&lyrics.lines[0]), 1.0, 360.0);
            let reserved = super::countdown_width(&mut painter, &lyrics.lines[0], 1.0);
            let mut paint = |presentation: &super::Presentation, when| {
                let mut pixels = vec![0; (size.0 * size.1 * 4) as usize];
                presentation.draw(
                    &mut crate::canvas::Canvas::new(&mut pixels, size, &mut painter),
                    20.0,
                    state,
                    &crate::settings::Settings::default(),
                    when,
                    360.0,
                );
                pixels
            };
            let has_countdown = |pixels: &[u8]| {
                (0..size.1).any(|y| {
                    (((20.0 + state.width - reserved - 2.0) * scale).ceil() as u32
                        ..((20.0 + state.width - 14.0) * scale).ceil() as u32)
                        .any(|x| pixels[((y * size.0 + x) * 4 + 3) as usize] > 0)
                })
            };
            presentation.update("track", 1, &lyrics.lines[0], 1.0, now);
            assert!(has_countdown(&paint(&presentation, now)));
            let sought = now + Duration::from_millis(16);
            presentation.update("track", 1, &lyrics.lines[1], 21.0, sought);
            assert!(
                !has_countdown(&paint(&presentation, sought)),
                "An outgoing layer must not retain numeric pixels after a seek at {scale}x"
            );
        }
    }

    #[test]
    fn countdown_is_hidden_for_waits_of_three_seconds_or_less() {
        let line = crate::lyrics::parse_lrc("[00:10]Waiting for the next lyric")
            .unwrap()
            .lines
            .remove(0);
        let mut painter = crate::canvas::Painter::new(1.0).unwrap();
        let live = super::shape_at(&mut painter, Some(&line), 10.0, 600.0).width;
        for at in [7.0, 7.1, 7.2, 9.99] {
            let state = super::shape_at(&mut painter, Some(&line), at, 600.0);
            let reserved = super::countdown_width(&mut painter, &line, at);
            let mut pixels = vec![0; 640 * 60 * 4];
            super::draw(
                &mut crate::canvas::Canvas::new(&mut pixels, (640, 60), &mut painter),
                10.0,
                state,
                &line,
                at,
                &crate::settings::Settings::default(),
            );
            let right = (10.0 + state.width - 2.0).floor() as usize;
            let left = (10.0 + state.width - reserved - 2.0).ceil() as usize;
            assert!(
                (8..32).all(|y| (left..right).all(|x| pixels[(y * 640 + x) * 4 + 3] == 0)),
                "a wait of {} seconds must not draw a numeric label",
                line.start - at
            );
        }
        assert_eq!(
            super::shape_at(&mut painter, Some(&line), 7.2, 600.0).width,
            live
        );
        assert!(super::shape_at(&mut painter, Some(&line), 6.99, 600.0).width > live);
    }

    #[test]
    fn upcoming_text_is_muted_and_keeps_note_and_countdown_at_all_scales() {
        for scale in [1.0, 1.25, 1.5, 2.0] {
            let lyrics =
                crate::lyrics::parse_ttml("<tt><p begin='10s' end='14s'>Hi</p></tt>").unwrap();
            let line = &lyrics.lines[0];
            let mut painter = crate::canvas::Painter::new(scale).unwrap();
            let state = super::shape_at(&mut painter, Some(line), 6.0, 360.0);
            let size = ((400.0 * scale) as u32, (60.0 * scale) as u32);
            let mut pixels = vec![0; (size.0 * size.1 * 4) as usize];
            super::draw(
                &mut crate::canvas::Canvas::new(&mut pixels, size, &mut painter),
                20.0,
                state,
                line,
                6.0,
                &crate::settings::Settings::default(),
            );
            let visible = |left: f32, right: f32| {
                (0..size.1).any(|y| {
                    ((left * scale) as u32..(right * scale) as u32)
                        .any(|x| pixels[((y * size.0 + x) * 4 + 3) as usize] > 100)
                })
            };
            assert!(visible(33.0, 48.0), "note at {scale}x");
            assert!(visible(56.0, 100.0), "upcoming lyric at {scale}x");
            assert!(
                visible(20.0 + state.width - 50.0, 20.0 + state.width - 14.0),
                "countdown at {scale}x"
            );
            assert!(
                pixels
                    .as_chunks::<4>()
                    .0
                    .iter()
                    .all(|pixel| pixel[0] <= crate::canvas::MUTED[0]),
                "pre-roll must not highlight words"
            );
            let gap = 20.0 + state.width - 14.0 - super::countdown_width(&mut painter, line, 6.0);
            assert!(
                (0..size.1).all(|y| (((gap + 1.0) * scale).ceil() as u32
                    ..((gap + 10.0) * scale).floor() as u32)
                    .all(|x| pixels[((y * size.0 + x) * 4 + 3) as usize] == 0)),
                "countdown needs a clear gap after the lyric at {scale}x"
            );
        }
    }

    #[test]
    fn line_lyrics_are_as_white_as_completed_word_lyrics() {
        let mut line = long_words();
        let words = render(&line, 10.0);
        let brightest = |pixels: &[u8]| {
            pixels
                .as_chunks::<4>()
                .0
                .iter()
                .map(|pixel| pixel[0])
                .max()
                .unwrap()
        };
        line.word_sync = false;
        assert_eq!(
            brightest(&render(&line, 0.0)),
            brightest(&words),
            "line timing must start white, without waiting for word progress"
        );
        let mut painter = crate::canvas::Painter::new(1.0).unwrap();
        let state = super::shape(&mut painter, Some(&line), 360.0);
        line.word_sync = true;
        let mut pixels = vec![0; 400 * 60 * 4];
        let mut canvas = crate::canvas::Canvas::new(&mut pixels, (400, 60), &mut painter);
        super::draw(
            &mut canvas,
            20.0,
            state,
            &line,
            0.0,
            &crate::settings::Settings {
                lyrics: crate::settings::LyricsMode::Lines,
                ..Default::default()
            },
        );
        assert_eq!(
            brightest(&pixels),
            brightest(&words),
            "the Lines preference must also start white for word-timed sources"
        );
    }

    #[test]
    fn compact_sentence_font_does_not_pump_during_width_animation() {
        let lyrics =
            crate::lyrics::parse_lrc("[00:00]HHHH HHHH HHHH HHHH HHHH HHHH HHHH HHHH").unwrap();
        let now = std::time::Instant::now();
        let mut presentation = super::Presentation::default();
        presentation.update("track", 1, &lyrics.lines[0], 0.0, now);
        for scale in [1.0, 1.25, 1.5, 2.0] {
            let mut painter = crate::canvas::Painter::new(scale).unwrap();
            let size = ((1200.0 * scale) as u32, (60.0 * scale) as u32);
            let mut heights = Vec::new();
            for width in [148.0, 220.0, 360.0, 600.0, 360.0, 220.0, 148.0] {
                let mut pixels = vec![0; (size.0 * size.1 * 4) as usize];
                let mut canvas = crate::canvas::Canvas::new(&mut pixels, size, &mut painter);
                presentation.draw(
                    &mut canvas,
                    20.0,
                    crate::animation::ShapeState {
                        width,
                        ..crate::animation::COMPACT
                    },
                    &crate::settings::Settings::default(),
                    now,
                    1100.0,
                );
                let occupied: Vec<_> = (0..size.1)
                    .filter(|&y| {
                        ((60.0 * scale) as u32..size.0)
                            .any(|x| pixels[((y * size.0 + x) * 4 + 3) as usize] > 0)
                    })
                    .collect();
                heights.push(occupied.last().unwrap() - occupied.first().unwrap() + 1);
            }
            assert!(
                heights.iter().all(|height| *height == heights[3]),
                "the same lyric must retain its font height while the capsule resizes at {scale}x: {heights:?}"
            );
        }
    }

    #[test]
    #[ignore = "Writes compact sentence resize frames for visual inspection"]
    fn compact_resize_render_preview() {
        use crate::animation::{Animation, COMPACT};
        use crate::canvas::{Canvas, Painter};
        use crate::settings::Settings;
        use std::time::{Duration, Instant};
        let Some(dir) = std::env::var_os("HIYODYNA_PREVIEW_DIR") else {
            return;
        };
        let dir = std::path::PathBuf::from(dir);
        std::fs::create_dir_all(&dir).unwrap();
        let settings = Settings::parse("motion=slow\nbounce=high");
        let lyrics = crate::lyrics::parse_lrc("[00:00]Bên nhau\n[00:04]Ngàn vì sao ở trên cao lấp lánh đang ngắm nhìn đôi ta bên nhau đêm nay").unwrap();
        for scale in [1.0, 1.25, 1.5, 2.0] {
            let mut painter = Painter::new(scale).unwrap();
            let size = ((1000.0 * scale) as u32, (60.0 * scale) as u32);
            let max_width = 900.0;
            let mut animation = Animation::new_with_tuning(settings.tuning());
            animation.current = super::shape(&mut painter, Some(&lyrics.lines[0]), max_width);
            animation.target = animation.current;
            let mut presentation = super::Presentation::default();
            let start = Instant::now();
            presentation.update("track", 1, &lyrics.lines[0], 1.0, start);
            for (direction, line) in [("grow", &lyrics.lines[1]), ("shrink", &lyrics.lines[0])] {
                let mut sheet = image::RgbaImage::new(size.0, size.1 * 12);
                let change = start + Duration::from_secs(if direction == "grow" { 1 } else { 4 });
                // Match a continuously painted preceding line so the crossfade is active.
                presentation.update(
                    "track",
                    1,
                    &presentation.current.as_ref().unwrap().line.clone(),
                    1.0,
                    change - Duration::from_millis(8),
                );
                animation.target = super::shape(&mut painter, Some(line), max_width);
                for frame in 0..240 {
                    let now = change + Duration::from_secs_f32(frame as f32 / 120.0);
                    presentation.update("track", 1, line, 4.0, now);
                    animation.advance(1.0 / 120.0);
                    if frame >= 120 || frame % 10 != 0 {
                        continue;
                    }
                    let mut pixels = vec![0; (size.0 * size.1 * 4) as usize];
                    crate::render::render_frame_with_margin(
                        &mut pixels,
                        size,
                        crate::render::RenderParams {
                            state: crate::animation::ShapeState {
                                width: animation.current.width * scale,
                                height: animation.current.height * scale,
                                radius: animation.current.radius * scale,
                            },
                            top_margin: 0.0,
                            scale,
                            shadow_opacity: 0.0,
                            hover: 0.0,
                            accent: settings.accent_bgra(),
                            attached: true,
                        },
                    );
                    let mut canvas = Canvas::new(&mut pixels, size, &mut painter);
                    presentation.draw(
                        &mut canvas,
                        (1000.0 - animation.current.width) * 0.5,
                        animation.current,
                        &settings,
                        now,
                        max_width,
                    );
                    for pixel in pixels.as_chunks_mut::<4>().0 {
                        pixel.swap(0, 2);
                    }
                    let image = image::RgbaImage::from_raw(size.0, size.1, pixels).unwrap();
                    image::imageops::replace(&mut sheet, &image, 0, i64::from(frame / 10 * size.1));
                }
                sheet
                    .save(dir.join(format!(
                        "compact-resize-{direction}-{}.png",
                        (scale * 100.0) as u32
                    )))
                    .unwrap();
                assert_eq!(animation.current, animation.target);
                assert_eq!(animation.current.height, COMPACT.height);
            }
        }
    }

    #[test]
    fn compact_grows_to_show_the_whole_sentence() {
        let line = long_words();
        let mut painter = crate::canvas::Painter::new(1.0).unwrap();
        let state = super::shape(&mut painter, Some(&line), 1200.0);
        assert!(
            state.width >= painter.width(&line.text(), super::SIZE, true) + 50.0,
            "the full sentence must fit instead of stopping at 360px"
        );
    }

    #[test]
    fn narrow_compact_shows_the_end_of_line_only_and_word_lyrics() {
        for scale in [1.0, 1.25, 1.5, 2.0] {
            for word_sync in [false, true] {
                let render_tail = |tail: &str| {
                    let mut line = long_words();
                    line.word_sync = word_sync;
                    line.words.last_mut().unwrap().text = tail.into();
                    let mut painter = crate::canvas::Painter::new(scale).unwrap();
                    let size = ((400.0 * scale) as u32, (60.0 * scale) as u32);
                    let mut pixels = vec![0; (size.0 * size.1 * 4) as usize];
                    let mut canvas = crate::canvas::Canvas::new(&mut pixels, size, &mut painter);
                    super::draw(
                        &mut canvas,
                        20.0,
                        crate::animation::ShapeState {
                            width: 360.0,
                            ..crate::animation::COMPACT
                        },
                        &line,
                        0.0,
                        &crate::settings::Settings::default(),
                    );
                    pixels
                };
                assert!(
                    render_tail("MMMM") != render_tail("WWWW"),
                    "even the last word must be drawn on a narrow surface at {scale}x"
                );
            }
        }
    }

    #[test]
    fn manual_timing_replacement_updates_the_current_compact_line() {
        let now = std::time::Instant::now();
        let mut presentation = super::Presentation::default();
        let mut line = long_words();
        line.word_sync = false;
        presentation.update("track", 1, &line, 9.5, now);
        line.word_sync = true;
        presentation.update("track", 1, &line, 9.5, now);
        assert!(
            presentation.current.as_ref().unwrap().line.has_word_sync(),
            "manual word timing must take effect even when the displayed sentence is unchanged"
        );
        line.words[9].start = 9.2;
        presentation.update("track", 1, &line, 9.5, now);
        assert_eq!(
            presentation.current.as_ref().unwrap().line.words[9].start,
            9.2
        );
        assert!(!presentation.moving(now));
    }

    #[test]
    fn line_transition_preserves_previous_frame_then_finishes() {
        use std::time::{Duration, Instant};
        let now = Instant::now();
        let mut presentation = super::Presentation::default();
        let lyrics = crate::lyrics::parse_lrc("[00:00]First line\n[00:04]Second line").unwrap();
        presentation.update("track", 1, &lyrics.lines[0], 1.0, now);
        let paint = |p: &super::Presentation, when| {
            let mut painter = crate::canvas::Painter::new(1.0).unwrap();
            let mut pixels = vec![0; 400 * 60 * 4];
            let mut canvas = crate::canvas::Canvas::new(&mut pixels, (400, 60), &mut painter);
            p.draw(
                &mut canvas,
                20.0,
                crate::animation::ShapeState {
                    width: 360.0,
                    ..crate::animation::COMPACT
                },
                &crate::settings::Settings::default(),
                when,
                360.0,
            );
            pixels
        };
        let old = paint(&presentation, now);
        presentation.update(
            "track",
            1,
            &lyrics.lines[1],
            4.0,
            now + Duration::from_millis(16),
        );
        assert_eq!(
            old,
            paint(&presentation, now + Duration::from_millis(16)),
            "line change starts with previous frame"
        );
        assert!(presentation.moving(now + Duration::from_millis(60)));
        let halfway = paint(&presentation, now + Duration::from_millis(96));
        for y in 10..30 {
            for x in 32..49 {
                let pixel = (y * 400 + x) * 4;
                assert_eq!(
                    &old[pixel..pixel + 4],
                    &halfway[pixel..pixel + 4],
                    "music note must not pulse during a lyric crossfade"
                );
            }
        }
        assert!(!presentation.moving(now + Duration::from_millis(200)));
        assert_ne!(old, paint(&presentation, now + Duration::from_millis(200)));
        presentation.update(
            "another track",
            2,
            &lyrics.lines[0],
            1.0,
            now + Duration::from_millis(210),
        );
        assert!(
            !presentation.moving(now + Duration::from_millis(210)),
            "do not crossfade across tracks"
        );
    }

    fn long_words() -> crate::lyrics::LyricLine {
        crate::lyrics::LyricLine {
            start: 0.0,
            end: 10.0,
            word_sync: true,
            words: [
                "But now ",
                "I know ",
                "a perfect ",
                "way to ",
                "let you go ",
                "through ",
                "the night ",
                "and follow ",
                "the music ",
                "tonight",
            ]
            .iter()
            .enumerate()
            .map(|(i, text)| crate::lyrics::LyricWord {
                text: (*text).into(),
                start: i as f64,
                end: i as f64 + 1.0,
            })
            .collect(),
        }
    }

    fn render(line: &crate::lyrics::LyricLine, at: f64) -> Vec<u8> {
        let mut painter = crate::canvas::Painter::new(1.0).unwrap();
        let state = super::shape(&mut painter, Some(line), 360.0);
        let mut pixels = vec![0; 400 * 60 * 4];
        let mut canvas = crate::canvas::Canvas::new(&mut pixels, (400, 60), &mut painter);
        super::draw(
            &mut canvas,
            20.0,
            state,
            line,
            at,
            &crate::settings::Settings::default(),
        );
        pixels
    }

    #[test]
    fn full_word_lyrics_keep_all_glyphs_visible_during_playback_and_seek() {
        let line = long_words();
        let alpha = |pixels: Vec<u8>| {
            pixels
                .as_chunks::<4>()
                .0
                .iter()
                .map(|p| p[3] > 0)
                .collect::<Vec<_>>()
        };
        let beginning = alpha(render(&line, 0.0));
        let ending = alpha(render(&line, 9.5));
        assert_eq!(
            beginning, ending,
            "the whole sentence stays visible while highlighting progresses"
        );
        assert_eq!(
            beginning,
            alpha(render(&line, 0.0)),
            "backward seeks reset the text position"
        );
        assert_eq!(
            ending,
            alpha(render(&line, 9.5)),
            "paused playback must produce the same glyph positions"
        );
    }

    #[test]
    fn line_timing_keeps_a_stable_complete_sentence() {
        let mut line = long_words();
        line.word_sync = false;
        assert_eq!(render(&line, 0.0), render(&line, 9.5));
    }

    #[test]
    fn highlighted_short_lyrics_keep_the_same_glyph_bounds() {
        use crate::lyrics::{LyricLine, LyricWord};
        let line = LyricLine {
            start: 0.0,
            end: 10.0,
            word_sync: true,
            words: ["First ", "second ", "third ", "fourth"]
                .iter()
                .enumerate()
                .map(|(i, text)| LyricWord {
                    text: (*text).into(),
                    start: i as f64,
                    end: i as f64 + 1.0,
                })
                .collect(),
        };
        let mut painter = crate::canvas::Painter::new(1.0).unwrap();
        let state = super::shape(&mut painter, Some(&line), 360.0);
        let render = |painter: &mut crate::canvas::Painter, at| {
            let mut pixels = vec![0; 400 * 60 * 4];
            let mut canvas = crate::canvas::Canvas::new(&mut pixels, (400, 60), painter);
            super::draw(
                &mut canvas,
                20.0,
                state,
                &line,
                at,
                &crate::settings::Settings::default(),
            );
            pixels
        };
        let waiting = render(&mut painter, 0.0);
        let finished = render(&mut painter, 10.0);
        for (before, after) in waiting
            .as_chunks::<4>()
            .0
            .iter()
            .zip(finished.as_chunks::<4>().0)
        {
            assert!(
                before[3] != 0 || after[3] == 0,
                "word fill must not add glyphs beyond the displayed prefix"
            );
        }
    }

    #[test]
    fn capsule_sizes_follow_unicode_lyrics_and_fit_available_display() {
        let lyrics=crate::lyrics::parse_lrc("[00:01]Hi\n[00:02]Một câu hát vừa dài hơn\n[00:03]A very long lyric line that must stay bounded on a small display instead of covering everything").unwrap();
        let mut painter = crate::canvas::Painter::new(1.0).unwrap();
        let short = super::shape(&mut painter, Some(&lyrics.lines[0]), 600.0);
        let medium = super::shape(&mut painter, Some(&lyrics.lines[1]), 600.0);
        let long = super::shape(&mut painter, Some(&lyrics.lines[2]), 600.0);
        assert!(short.width < medium.width);
        assert!(medium.width < long.width);
        assert!(long.width <= 600.0);
        assert_eq!(long.height, crate::animation::COMPACT.height);
    }
}
