use serde::{Deserialize, Serialize};

#[derive(Clone, Debug, Default, Serialize, Deserialize)]
pub(crate) struct Lyrics {
    pub lines: Vec<LyricLine>,
    pub source: String,
    pub credits: String,
    pub instrumental: bool,
}

#[derive(Clone, Debug, Serialize, Deserialize)]
pub(crate) struct LyricLine {
    pub start: f64,
    pub end: f64,
    pub words: Vec<LyricWord>,
    pub word_sync: bool,
}

#[derive(Clone, Debug, Serialize, Deserialize)]
pub(crate) struct LyricWord {
    pub text: String,
    pub start: f64,
    pub end: f64,
}

const MAX_BYTES: usize = 2_000_000;
const MAX_LINES: usize = 3000;

impl LyricLine {
    pub fn text(&self) -> String {
        self.words.iter().map(|w| w.text.as_str()).collect()
    }

    fn valid_word_times(&self) -> bool {
        !self.words.is_empty()
            && self.words.iter().all(|w| {
                w.start.is_finite()
                    && w.end.is_finite()
                    && w.start >= self.start
                    && w.end <= self.end
                    && w.end > w.start
            })
            && self.words.windows(2).all(|w| w[0].start <= w[1].start)
    }

    pub fn has_word_sync(&self) -> bool {
        if !self.word_sync || !self.valid_word_times() {
            return false;
        }
        let mut words = self
            .words
            .iter()
            .filter(|word| !word.text.trim().is_empty());
        let Some(first) = words.next() else {
            return false;
        };
        let Some(second) = words.next() else {
            return first.text.split_whitespace().count() == 1;
        };
        std::iter::once(second)
            .chain(words)
            .any(|word| word.start != first.start || word.end != first.end)
    }
}

impl Lyrics {
    pub fn is_synchronized(&self) -> bool {
        !self.lines.is_empty()
            && self.lines.iter().all(|line| {
                line.start.is_finite()
                    && line.end.is_finite()
                    && line.start >= 0.0
                    && line.end > line.start
                    && line.end <= 86_400.0
                    && !line.text().trim().is_empty()
            })
            && self
                .lines
                .windows(2)
                .all(|lines| lines[0].start <= lines[1].start)
    }

    pub fn active(&self, position: f64) -> Option<usize> {
        self.lines
            .iter()
            .enumerate()
            .rev()
            .find(|(_, l)| position >= l.start && position < l.end)
            .map(|(i, _)| i)
    }

    pub fn has_words(&self) -> bool {
        self.lines.iter().any(LyricLine::has_word_sync)
    }

    fn finish(mut self) -> Option<Self> {
        self.lines.retain(|l| {
            !l.words.is_empty()
                && l.start.is_finite()
                && l.end.is_finite()
                && l.start >= 0.0
                && l.end > l.start
                && l.end <= 86_400.0
                && !l.text().trim().is_empty()
        });
        if self.lines.len() > MAX_LINES {
            return None;
        }
        self.lines.sort_by(|a, b| a.start.total_cmp(&b.start));
        for line in &mut self.lines {
            line.word_sync = line.has_word_sync();
            if !line.valid_word_times() {
                // Keep the supplied text and line timing when word timing is unusable.
                for word in &mut line.words {
                    word.start = line.start;
                    word.end = line.end;
                }
            }
        }
        (!self.lines.is_empty() || self.instrumental).then_some(self)
    }
}

fn timestamp(value: &str) -> Option<f64> {
    let value = value.trim();
    let result = if let Some(ms) = value.strip_suffix("ms") {
        ms.parse::<f64>().ok()? / 1000.0
    } else if let Some(s) = value.strip_suffix('s') {
        s.parse().ok()?
    } else {
        let mut parts = value.split(':');
        let mut time = parts.next()?.parse::<f64>().ok()?;
        for part in parts {
            time = time * 60.0 + part.parse::<f64>().ok()?;
        }
        time
    };
    (result.is_finite() && (0.0..=86_400.0).contains(&result)).then_some(result)
}

fn auxiliary(node: roxmltree::Node<'_, '_>) -> bool {
    node.attributes().any(|a| {
        a.name() == "role" && matches!(a.value(), "x-translation" | "x-roman" | "x-romanization")
    })
}

fn timed_text(
    node: roxmltree::Node<'_, '_>,
    inherited: (f64, f64),
    owner: Option<roxmltree::NodeId>,
    words: &mut Vec<LyricWord>,
    explicit: &mut bool,
    last_owner: &mut Option<roxmltree::NodeId>,
) {
    if auxiliary(node) {
        return;
    }
    let begin = node
        .attribute("begin")
        .map(|value| timestamp(value).unwrap_or(f64::NAN))
        .unwrap_or(inherited.0);
    let end = node
        .attribute("end")
        .map(|value| timestamp(value).unwrap_or(f64::NAN))
        .or_else(|| {
            node.attribute("dur")
                .map(|d| begin + timestamp(d).unwrap_or(f64::NAN))
        })
        .unwrap_or(inherited.1);
    let owner = if node.has_tag_name("span") && node.attribute("begin").is_some() {
        Some(node.id())
    } else {
        owner
    };
    for child in node.children() {
        if let Some(text) = child.text().filter(|_| child.is_text()) {
            if !text.is_empty() {
                if text.trim().is_empty() {
                    // Whitespace belongs to the preceding word, not the paragraph clock.
                    // Normalize it with both neighboring characters available below.
                    if let Some(last) = words.last_mut() {
                        last.text.push_str(text);
                    }
                    continue;
                }
                if owner.is_none() {
                    *explicit = false;
                }
                if let Some(last) = words
                    .last_mut()
                    .filter(|w| *last_owner == owner && w.start == begin && w.end == end)
                {
                    last.text.push_str(text);
                } else {
                    words.push(LyricWord {
                        text: text.to_owned(),
                        start: begin,
                        end,
                    });
                }
                *last_owner = owner;
            }
        } else if child.is_element() {
            timed_text(child, (begin, end), owner, words, explicit, last_owner);
        }
    }
}

fn normalize_ttml_space(words: &mut [LyricWord]) {
    fn cjk(c: char) -> bool {
        matches!(c, '\u{1100}'..='\u{11ff}' | '\u{2e80}'..='\u{a4cf}'
            | '\u{ac00}'..='\u{d7af}' | '\u{f900}'..='\u{faff}'
            | '\u{fe30}'..='\u{fe4f}' | '\u{ff00}'..='\u{ffef}'
            | '\u{20000}'..='\u{323af}')
    }

    let raw: Vec<_> = words
        .iter_mut()
        .map(|word| std::mem::take(&mut word.text))
        .collect();
    let mut previous = None;
    let mut whitespace: Option<(usize, bool)> = None;
    for (index, text) in raw.iter().enumerate() {
        for c in text.chars() {
            if matches!(c, ' ' | '\t' | '\n' | '\r') {
                let pending = whitespace.get_or_insert((index, false));
                pending.1 |= matches!(c, '\n' | '\r');
                continue;
            }
            if let Some((owner, line_break)) = whitespace.take()
                && let Some(previous) = previous
                // TTML collapses whitespace and suppresses line breaks between CJK characters.
                && !(line_break && cjk(previous) && cjk(c))
            {
                words[owner].text.push(' ');
            }
            words[index].text.push(c);
            previous = Some(c);
        }
    }
    // Pending trailing whitespace and leading whitespace have no paragraph content on both sides.
}

pub(crate) fn parse_ttml(text: &str) -> Option<Lyrics> {
    if text.len() > MAX_BYTES {
        return None;
    }
    let doc = roxmltree::Document::parse_with_options(
        text,
        roxmltree::ParsingOptions {
            nodes_limit: 80_000,
            ..Default::default()
        },
    )
    .ok()?;
    let mut result = Lyrics::default();
    if doc
        .descendants()
        .any(|node| node.ancestors().take(65).count() > 64)
    {
        return None;
    }
    for p in doc.descendants().filter(|n| n.has_tag_name("p")) {
        let start = timestamp(p.attribute("begin")?)?;
        let end = p
            .attribute("end")
            .and_then(timestamp)
            .or_else(|| p.attribute("dur").and_then(timestamp).map(|d| start + d))?;
        let mut words = Vec::new();
        let mut word_sync = true;
        timed_text(p, (start, end), None, &mut words, &mut word_sync, &mut None);
        normalize_ttml_space(&mut words);
        result.lines.push(LyricLine {
            start,
            end,
            words,
            word_sync,
        });
    }
    let substantive_lines = result
        .lines
        .iter()
        .filter(|line| !line.text().trim().is_empty())
        .count();
    result
        .finish()
        .filter(|lyrics| lyrics.lines.len() == substantive_lines)
}

pub(crate) fn parse_lrc(text: &str) -> Option<Lyrics> {
    if text.len() > MAX_BYTES {
        return None;
    }
    let offset = text
        .lines()
        .find_map(|l| {
            l.trim()
                .strip_prefix("[offset:")?
                .strip_suffix(']')?
                .parse::<f64>()
                .ok()
                .filter(|offset| offset.is_finite())
        })
        .unwrap_or(0.0)
        / 1000.0;
    let mut result = Lyrics::default();
    for line in text.lines() {
        let mut remainder = line.trim();
        let mut times = Vec::new();
        while let Some(rest) = remainder.strip_prefix('[') {
            let Some((tag, after)) = rest.split_once(']') else {
                break;
            };
            if tag.contains(':')
                && let Some(t) = timestamp(tag)
            {
                times.push((t + offset).max(0.0));
            }
            remainder = after;
        }
        for start in times {
            result.lines.push(LyricLine {
                start,
                end: start + 8.0,
                word_sync: false,
                words: vec![LyricWord {
                    text: remainder.trim().to_owned(),
                    start,
                    end: start + 8.0,
                }],
            });
        }
    }
    result.lines.sort_by(|a, b| a.start.total_cmp(&b.start));
    let mut next_start = None;
    let mut group_start = None;
    for line in result.lines.iter_mut().rev() {
        if group_start != Some(line.start) {
            next_start = group_start;
            group_start = Some(line.start);
        }
        if let Some(end) = next_start {
            line.end = end;
            line.words[0].end = end;
        }
    }
    result.finish()
}

#[derive(Deserialize)]
struct LyricsFile {
    #[serde(default)]
    lines: Vec<FileLine>,
}
#[derive(Deserialize)]
struct FileLine {
    text: String,
    start_ms: f64,
    end_ms: f64,
    #[serde(default)]
    words: Vec<FileWord>,
}
#[derive(Deserialize)]
struct FileWord {
    text: String,
    start_ms: f64,
    end_ms: Option<f64>,
}

pub(crate) fn parse_lyricsfile(text: &str) -> Option<Lyrics> {
    if text.len() > MAX_BYTES {
        return None;
    }
    let file: LyricsFile = serde_yaml::from_str(text).ok()?;
    let substantive_lines = file
        .lines
        .iter()
        .filter(|line| !line.text.trim().is_empty())
        .count();
    let lines = file
        .lines
        .into_iter()
        .map(|line| {
            let word_sync = !line.words.is_empty()
                && line
                    .words
                    .iter()
                    .map(|w| w.text.as_str())
                    .collect::<String>()
                    .trim()
                    == line.text.trim();
            let words = if word_sync {
                line.words
                    .iter()
                    .enumerate()
                    .map(|(i, w)| LyricWord {
                        text: w.text.clone(),
                        start: w.start_ms / 1000.0,
                        end: w.end_ms.unwrap_or_else(|| {
                            line.words
                                .get(i + 1)
                                .map_or(line.end_ms, |next| next.start_ms)
                        }) / 1000.0,
                    })
                    .collect()
            } else {
                vec![LyricWord {
                    text: line.text.clone(),
                    start: line.start_ms / 1000.0,
                    end: line.end_ms / 1000.0,
                }]
            };
            let mut parsed = LyricLine {
                start: line.start_ms / 1000.0,
                end: line.end_ms / 1000.0,
                words,
                word_sync,
            };
            if !parsed.has_word_sync() {
                parsed.words = vec![LyricWord {
                    text: line.text,
                    start: parsed.start,
                    end: parsed.end,
                }];
                parsed.word_sync = false;
            }
            parsed
        })
        .collect();
    Lyrics {
        lines,
        ..Default::default()
    }
    .finish()
    // A surviving word-timed fragment must not outrank a complete fallback source.
    .filter(|lyrics| lyrics.lines.len() == substantive_lines)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn ttml_rejects_dropped_substantive_paragraphs() {
        for invalid in [
            "<p begin='5' end='5'>Lost line</p>",
            "<p begin='6' end='5'>Lost line</p>",
            "<p begin='86400' dur='2'>Lost line</p>",
        ] {
            let text = format!(
                "<tt><p begin='1' end='3'><span begin='1' end='2'>Hello </span><span begin='2' end='3'>world</span></p>{invalid}</tt>"
            );
            assert!(
                parse_ttml(&text).is_none(),
                "must not publish truncated synchronized lyrics: {invalid}"
            );
        }
        let lyrics =
            parse_ttml("<tt><p begin='1' end='3'>Complete line</p><p begin='5' end='5'> </p></tt>")
                .unwrap();
        assert_eq!(lyrics.lines.len(), 1);
        assert_eq!(lyrics.lines[0].text(), "Complete line");
    }

    #[test]
    fn ttml_preserves_nested_words_and_ignores_translations() {
        let lyrics = parse_ttml(r#"<tt xmlns:ttm="http://www.w3.org/ns/ttml#metadata"><body><div><p begin="00:01.000" end="00:04.000"><span begin="1s" end="2s">Hello &amp; </span><span begin="2s" end="4s"><span>world</span></span><span ttm:role="x-translation">wrong text</span></p></div></body></tt>"#).unwrap();
        assert_eq!(lyrics.lines.len(), 1);
        assert!(lyrics.lines[0].word_sync);
        assert_eq!(
            lyrics.lines[0]
                .words
                .iter()
                .map(|w| w.text.as_str())
                .collect::<String>(),
            "Hello & world"
        );
        assert_eq!(lyrics.lines[0].words[1].start, 2.0);
        assert_eq!(lyrics.lines[0].end, 4.0);
    }

    #[test]
    fn lrc_supports_repeated_timestamps_and_signed_offset() {
        let lyrics =
            parse_lrc("[offset:-500]\n[00:01.00][00:03.00]One line\n[00:05.00]Next").unwrap();
        assert_eq!(lyrics.lines.len(), 3);
        assert_eq!(
            lyrics.lines.iter().map(|l| l.start).collect::<Vec<_>>(),
            vec![0.5, 2.5, 4.5]
        );
        assert!(!lyrics.lines[0].word_sync);
    }

    #[test]
    fn lyricsfile_uses_real_word_starts_and_line_end() {
        let lyrics = parse_lyricsfile("version: '1.0'\nlines:\n- text: Hello world\n  start_ms: 1000\n  end_ms: 4000\n  words:\n  - text: 'Hello '\n    start_ms: 1100\n  - text: world\n    start_ms: 2300\n").unwrap();
        assert!(lyrics.lines[0].word_sync);
        assert_eq!(lyrics.lines[0].words[0].end, 2.3);
        assert_eq!(lyrics.lines[0].words[1].end, 4.0);
    }

    #[test]
    fn rejects_malformed_or_oversized_lyrics() {
        assert!(parse_ttml("<tt><p>").is_none());
        assert!(parse_ttml(&" ".repeat(2_100_000)).is_none());
        assert!(parse_lrc("[bad]not synchronized").is_none());
        assert!(
            parse_ttml(&format!(
                "<tt><p begin='1s' end='2s'>{}nested{}</p></tt>",
                "<span>".repeat(100),
                "</span>".repeat(100)
            ))
            .is_none()
        );
    }

    #[test]
    fn pauses_between_lines_do_not_keep_previous_line_active() {
        let lyrics = parse_ttml(
            "<tt><body><p begin='1s' end='2s'>First</p><p begin='5s' end='7s'>Next</p></body></tt>",
        )
        .unwrap();
        assert_eq!(lyrics.active(1.5), Some(0));
        assert_eq!(lyrics.active(3.0), None);
        assert_eq!(lyrics.active(5.5), Some(1));
        assert!(!lyrics.has_words());
    }

    #[test]
    fn ttml_spaces_use_word_timing_and_equal_timed_spans_stay_separate() {
        let lyrics = parse_ttml("<tt><p begin='27.173' end='28.516'>\n  <span begin='27.173' end='27.407'>I've</span> <span begin='27.407' end='27.510'>been</span> <span begin='27.510' end='27.899'>tryna</span> <span begin='27.899' end='28.516'>call</span>\n</p><p begin='30' end='32'><span begin='30' end='32'>同</span><span begin='30' end='32'>時</span></p></tt>").unwrap();
        let line = &lyrics.lines[0];
        assert_eq!(line.text(), "I've been tryna call");
        assert_eq!(line.words.len(), 4);
        assert_eq!(line.words[0].end, 27.407);
        assert_eq!(line.words[1].start, 27.407);
        assert!(line.word_sync);
        assert_eq!(lyrics.lines[1].words.len(), 2);
        assert_eq!(lyrics.lines[1].text(), "同時");
        assert!(!lyrics.lines[1].word_sync);
    }

    #[test]
    fn ttml_invalid_word_timing_falls_back_without_losing_text() {
        for spans in [
            "<span begin='1' end='1'>You</span> <span begin='0.9' end='2'>can</span>",
            "<span begin='1' end='2'>You</span> <span begin='NaN' end='3'>can</span>",
            "<span begin='2' end='3'>You</span> <span begin='1' end='2'>can</span>",
            "<span begin='1' end='2'>You</span> <span begin='2' end='5'>can</span>",
            "<span begin='1' end='4'>You can</span>",
            "You <span begin='2' end='3'>can</span>",
        ] {
            let lyrics = parse_ttml(&format!("<tt><p begin='1' end='4'>{spans}</p></tt>")).unwrap();
            assert_eq!(lyrics.lines[0].text(), "You can");
            assert!(!lyrics.has_words(), "{spans}");
            assert_eq!(lyrics.active(2.5), Some(0));
        }
    }

    #[test]
    fn lyricsfile_preserves_explicit_gaps_and_falls_back_for_invalid_words() {
        for words in [
            "[{text: 'Hello ', start_ms: 900}, {text: world, start_ms: 2300}]",
            "[{text: 'Hello ', start_ms: 2300}, {text: world, start_ms: 1100}]",
            "[{text: 'Hello ', start_ms: 1100, end_ms: 5000}, {text: world, start_ms: 2300}]",
            "[{text: 'Hello ', start_ms: .nan}, {text: world, start_ms: 2300}]",
            "[{text: Hello, start_ms: 1100}]",
        ] {
            let lyrics = parse_lyricsfile(&format!("lines:\n- text: Hello world\n  start_ms: 1000\n  end_ms: 4000\n  words: {words}\n- text: Next\n  start_ms: 6000\n  end_ms: 7000\n")).unwrap();
            assert_eq!(lyrics.lines[0].text(), "Hello world");
            assert!(!lyrics.has_words(), "{words}");
            assert_eq!(lyrics.active(5.0), None);
        }
    }

    #[test]
    fn lrc_empty_markers_end_lines_without_becoming_lyrics() {
        let lyrics = parse_lrc("[00:13.13]Yeah\n[00:16.56]\n[00:27.16]Next\n[00:30.00]").unwrap();
        assert_eq!(lyrics.lines.len(), 2);
        assert_eq!(lyrics.lines[0].end, 16.56);
        assert_eq!(lyrics.lines[1].end, 30.0);
        assert_eq!(lyrics.active(20.0), None);
        assert!(!lyrics.has_words());
    }

    #[test]
    fn lrc_duplicate_times_preserve_each_line() {
        let lyrics = parse_lrc("[00:01.00]First\n[00:01.00]Second\n[00:04.00]Next").unwrap();
        assert_eq!(lyrics.lines.len(), 3);
        assert_eq!(lyrics.lines[0].text(), "First");
        assert_eq!(lyrics.lines[0].end, 4.0);
        assert_eq!(lyrics.lines[1].end, 4.0);
    }

    #[test]
    fn ttml_nested_timed_containers_do_not_time_spacing_as_words() {
        let lyrics = parse_ttml("<tt><p begin='1' end='4'><span begin='1' end='4'><span begin='1' end='2'><span>Hello</span></span> <span begin='2' end='4'>world</span></span></p></tt>").unwrap();
        assert_eq!(lyrics.lines[0].text(), "Hello world");
        assert_eq!(lyrics.lines[0].words.len(), 2);
        assert!(lyrics.has_words());
    }

    #[test]
    fn lrc_nonfinite_offset_does_not_replace_real_timestamps() {
        for offset in ["NaN", "inf", "-inf"] {
            let lyrics = parse_lrc(&format!(
                "[offset:{offset}]\n[00:01.00]First\n[00:04.00]Next"
            ))
            .unwrap();
            assert_eq!(lyrics.lines[0].start, 1.0);
            assert_eq!(lyrics.lines[0].end, 4.0);
        }
    }

    #[test]
    fn ttml_normalizes_interspan_line_breaks_and_trims_paragraph_edges() {
        for (first, second, expected) in [
            ("Hello", "world", "Hello world"),
            ("你", "好", "你好"),
            ("世", "界", "世界"),
        ] {
            let lyrics = parse_ttml(&format!("<tt><p begin='1' end='4'>\n  <span begin='1' end='2'>{first}</span>\n  <span begin='2' end='4'>{second}</span>\n</p></tt>")).unwrap();
            assert_eq!(lyrics.lines[0].text(), expected);
            assert_eq!(lyrics.lines[0].words.len(), 2);
            assert_eq!(lyrics.lines[0].words[0].end, 2.0);
            assert!(lyrics.has_words());
        }
    }

    #[test]
    fn empty_words_do_not_turn_a_single_timed_phrase_into_word_sync() {
        for empty in ["", " "] {
            let line = LyricLine {
                start: 1.0,
                end: 4.0,
                word_sync: true,
                words: vec![
                    LyricWord {
                        text: "Hello world".into(),
                        start: 1.0,
                        end: 3.0,
                    },
                    LyricWord {
                        text: empty.into(),
                        start: 3.0,
                        end: 4.0,
                    },
                ],
            };
            assert!(!line.has_word_sync(), "{empty:?}");
            let lyrics = parse_lyricsfile(&format!("lines:\n- text: Hello world\n  start_ms: 1000\n  end_ms: 4000\n  words: [{{text: 'Hello world', start_ms: 1000, end_ms: 3000}}, {{text: '{empty}', start_ms: 3000, end_ms: 4000}}]")).unwrap();
            assert!(!lyrics.has_words(), "{empty:?}");
            assert_eq!(lyrics.lines[0].text(), "Hello world");
        }
    }

    #[test]
    fn substantive_word_detection_preserves_single_words_and_equal_timed_spans() {
        let single = parse_lyricsfile("lines:\n- text: Hello\n  start_ms: 1000\n  end_ms: 4000\n  words: [{text: Hello, start_ms: 1000, end_ms: 3000}, {text: ' ', start_ms: 3000, end_ms: 4000}]").unwrap();
        assert!(single.has_words());
        assert_eq!(single.lines[0].text(), "Hello ");
        let syllables = parse_ttml("<tt><p begin='1' end='4'><span begin='1' end='2'>Hel</span><span begin='1' end='2'>lo</span> <span begin='2' end='4'>world</span></p></tt>").unwrap();
        assert!(syllables.has_words());
        assert_eq!(syllables.lines[0].words.len(), 3);
        assert_eq!(syllables.lines[0].text(), "Hello world");
        assert_eq!(syllables.lines[0].words[1].text, "lo ");
        assert_eq!(syllables.lines[0].words[1].start, 1.0);
        assert_eq!(syllables.lines[0].words[1].end, 2.0);
    }
}
