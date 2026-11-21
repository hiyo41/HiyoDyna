use std::io::Read;
use std::path::PathBuf;
use std::sync::{Mutex, OnceLock};
use std::time::{Duration, SystemTime};

use crate::lyrics::{LyricLine, LyricWord, Lyrics, parse_lrc, parse_lyricsfile, parse_ttml};
use reqwest::blocking::Client;
use serde_json::Value;

fn normalize(text: &str) -> String {
    text.chars()
        .flat_map(char::to_lowercase)
        .filter(|c| c.is_alphanumeric())
        .collect()
}

fn matches_recording(title: &str, artist: &str, other_title: &str, other_artist: &str) -> bool {
    normalize(title) == normalize(other_title) && normalize(artist) == normalize(other_artist)
}

fn cache_dir() -> Option<PathBuf> {
    std::env::var_os("LOCALAPPDATA").map(|p| PathBuf::from(p).join("HiyoDyna").join("lyrics"))
}

pub(crate) fn clear_cache() {
    if let Some(dir) = cache_dir()
        && let Ok(entries) = std::fs::read_dir(dir)
    {
        for entry in entries
            .flatten()
            .filter(|e| e.path().extension().is_some_and(|x| x == "json"))
        {
            let _ = std::fs::remove_file(entry.path());
        }
    }
}

fn cache_path(key: &str) -> Option<PathBuf> {
    // Stable FNV-1a key, independent of process hash randomization.
    let hash = key.bytes().fold(0xcbf29ce484222325_u64, |h, b| {
        (h ^ u64::from(b)).wrapping_mul(0x100000001b3)
    });
    // Parsed timing semantics changed; leave old cache files untouched and refetch.
    Some(cache_dir()?.join(format!("v2-{hash:016x}.json")))
}

static COOLDOWN: OnceLock<Mutex<[Option<SystemTime>; 3]>> = OnceLock::new();

#[derive(Clone, Copy, Debug, Eq, Hash, PartialEq)]
enum Provider {
    Amll = 0,
    Lrclib = 1,
    Netease = 2,
}

impl Provider {
    fn name(self) -> &'static str {
        match self {
            Self::Amll => "AMLL TTML DB",
            Self::Lrclib => "LRCLIB",
            Self::Netease => "NetEase",
        }
    }
}

#[derive(Clone, Debug)]
pub(crate) struct SearchResult {
    pub title: String,
    pub artist: String,
    pub album: String,
    /// Seconds; zero when the provider's search response omits duration.
    pub duration: f64,
    pub source: String,
    /// Verified timing when included in search; otherwise "Unknown" until fetched.
    pub timing: String,
    provider: Provider,
    id: u64,
    lyrics: Option<Lyrics>,
}

#[cfg(test)]
impl SearchResult {
    pub(crate) fn fixture(title: &str, artist: &str) -> Self {
        Self {
            title: title.into(),
            artist: artist.into(),
            album: "Native renderer preview".into(),
            duration: 200.0,
            source: "LRCLIB".into(),
            timing: "Lines".into(),
            provider: Provider::Lrclib,
            id: 1,
            lyrics: None,
        }
    }
}

fn client() -> Result<Client, String> {
    Client::builder()
        .timeout(Duration::from_secs(8))
        .connect_timeout(Duration::from_secs(4))
        .user_agent(concat!(
            "HiyoDyna v",
            env!("CARGO_PKG_VERSION"),
            " (https://github.com/hiyo41/HiyoDyna)"
        ))
        .build()
        .map_err(|e| e.to_string())
}

fn check_current(current: &impl Fn() -> bool) -> Result<(), String> {
    if current() {
        Ok(())
    } else {
        Err("Lyric request cancelled".into())
    }
}

fn read_json(mut reader: impl Read, current: &impl Fn() -> bool) -> Result<Value, String> {
    let mut body = Vec::new();
    let mut buffer = [0; 8192];
    loop {
        check_current(current)?;
        let len = reader.read(&mut buffer).map_err(|e| e.to_string())?;
        if len == 0 {
            break;
        }
        if body.len() + len > 2_000_000 {
            return Err("Provider response exceeds 2 MB".into());
        }
        body.extend_from_slice(&buffer[..len]);
    }
    check_current(current)?;
    serde_json::from_slice(&body).map_err(|_| "Invalid provider JSON".into())
}

fn get(client: &Client, url: &str, provider: usize, current: &impl Fn() -> bool) -> Option<Value> {
    request(client, url, provider, current).ok()
}

fn request(
    client: &Client,
    url: &str,
    provider: usize,
    current: &impl Fn() -> bool,
) -> Result<Value, String> {
    check_current(current)?;
    let cooldown = COOLDOWN.get_or_init(|| Mutex::new([None; 3]));
    if cooldown.lock().map_err(|_| "Provider state unavailable")?[provider]
        .is_some_and(|until| until > SystemTime::now())
    {
        return Err("Provider temporarily rate limited".into());
    }
    let response = client.get(url).send().map_err(|e| e.to_string())?;
    check_current(current)?;
    if response.status().as_u16() == 429 || response.status().as_u16() == 503 {
        let delay = response
            .headers()
            .get("retry-after")
            .and_then(|v| v.to_str().ok())
            .and_then(|v| v.parse::<u64>().ok())
            .unwrap_or(60)
            .max(1);
        cooldown.lock().map_err(|_| "Provider state unavailable")?[provider] =
            SystemTime::now().checked_add(Duration::from_secs(delay.saturating_add(1)));
        return Err(format!("Provider returned HTTP {}", response.status()));
    }
    if !response.status().is_success() {
        return Err(format!("Provider returned HTTP {}", response.status()));
    }
    if response.content_length().is_some_and(|len| len > 2_000_000) {
        return Err("Provider response exceeds 2 MB".into());
    }
    read_json(response, current)
}

fn url(base: &str, pairs: &[(&str, String)]) -> String {
    let mut url = reqwest::Url::parse(base).expect("constant provider URL");
    url.query_pairs_mut()
        .extend_pairs(pairs.iter().map(|(k, v)| (*k, v.as_str())));
    url.into()
}

fn joined_names(value: &Value) -> String {
    value
        .as_array()
        .map(|names| {
            names
                .iter()
                .filter_map(|v| v.as_str().or_else(|| v["name"].as_str()))
                .collect::<Vec<_>>()
                .join(", ")
        })
        .unwrap_or_default()
}

fn first_name(value: &Value) -> String {
    value
        .as_array()
        .and_then(|a| a.first())
        .and_then(Value::as_str)
        .unwrap_or_default()
        .into()
}

fn timing(lyrics: &Lyrics) -> &'static str {
    match quality(lyrics).0 {
        3 => "Words",
        2 => "Lines",
        _ if !lyrics.lines.is_empty() => "Text",
        _ => "Unknown",
    }
}

fn search_results(provider: Provider, data: &Value) -> Result<Vec<SearchResult>, String> {
    let items = match provider {
        Provider::Amll if data["status"].as_u64() == Some(200) => data["data"]["items"].as_array(),
        Provider::Lrclib => data.as_array(),
        Provider::Netease if data["code"].as_u64() == Some(200) => {
            if data["result"]["songCount"].as_u64() == Some(0) {
                return Ok(Vec::new());
            }
            data["result"]["songs"].as_array()
        }
        _ => None,
    }
    .ok_or_else(|| "Invalid search response or provider unavailable".to_string())?;
    Ok(items
        .iter()
        .take(100)
        .filter_map(|item| {
            let id = item["id"].as_u64()?;
            let (title, artist, album, duration, lyrics) = match provider {
                Provider::Amll => (
                    first_name(&item["musicNames"]),
                    joined_names(&item["artistNames"]),
                    first_name(&item["albumNames"]),
                    0.0,
                    None,
                ),
                Provider::Lrclib => (
                    item["trackName"].as_str()?.into(),
                    item["artistName"].as_str().unwrap_or_default().into(),
                    item["albumName"].as_str().unwrap_or_default().into(),
                    item["duration"].as_f64().unwrap_or_default(),
                    lrclib_lyrics(item),
                ),
                Provider::Netease => (
                    item["name"].as_str()?.into(),
                    joined_names(&item["artists"]),
                    item["album"]["name"].as_str().unwrap_or_default().into(),
                    item["duration"].as_f64().unwrap_or_default() / 1000.0,
                    None,
                ),
            };
            if title.trim().is_empty() {
                return None;
            }
            let timing = lyrics.as_ref().map(timing).unwrap_or("Unknown").into();
            Some(SearchResult {
                title,
                artist,
                album,
                duration: if duration.is_finite() && duration > 0.0 {
                    duration
                } else {
                    0.0
                },
                source: provider.name().into(),
                timing,
                provider,
                id,
                lyrics,
            })
        })
        .collect())
}

fn rank_results(mut results: Vec<SearchResult>, title: &str, artist: &str) -> Vec<SearchResult> {
    results.retain(|result| {
        result
            .lyrics
            .as_ref()
            .is_none_or(|lyrics| quality(lyrics).0 > 0)
    });
    let rank = |result: &SearchResult| {
        let exact_title = normalize(&result.title) == normalize(title);
        let exact_artist =
            !artist.trim().is_empty() && normalize(&result.artist) == normalize(artist);
        let timing = match result.timing.as_str() {
            "Words" => 3,
            "Lines" => 2,
            "Text" => 1,
            _ => 0,
        };
        (u8::from(exact_title) * 2 + u8::from(exact_artist), timing)
    };
    results.sort_by(|a, b| {
        rank(b)
            .cmp(&rank(a))
            .then_with(|| a.source.cmp(&b.source))
            .then_with(|| a.title.cmp(&b.title))
            .then_with(|| a.artist.cmp(&b.artist))
            .then_with(|| a.album.cmp(&b.album))
            .then_with(|| a.id.cmp(&b.id))
    });
    let mut seen = std::collections::HashSet::new();
    results.retain(|result| seen.insert((result.provider, result.id)));
    results.truncate(24);
    results
}

fn search_url(provider: Provider, title: &str, artist: &str) -> String {
    match provider {
        Provider::Amll => url(
            "https://api.amll.dev/v1/lyrics/search",
            &[
                ("musicName", title.into()),
                ("artistName", artist.into()),
                ("pageSize", "8".into()),
            ],
        ),
        Provider::Lrclib => {
            let pairs: Vec<_> = [("track_name", title), ("artist_name", artist)]
                .into_iter()
                .filter(|(_, value)| !value.is_empty())
                .map(|(key, value)| (key, value.into()))
                .collect();
            url("https://lrclib.net/api/search", &pairs)
        }
        // Public endpoints on NetEase's own host, without cookies, keys, or a proxy.
        Provider::Netease => url(
            "https://music.163.com/api/search/get",
            &[
                ("s", format!("{title} {artist}").trim().into()),
                ("type", "1".into()),
                ("offset", "0".into()),
                ("limit", "8".into()),
            ],
        ),
    }
}

#[derive(Debug)]
pub(crate) struct SearchResponse {
    pub results: Vec<SearchResult>,
    pub warnings: Vec<String>,
}

fn finish_search(
    results: Vec<SearchResult>,
    errors: Vec<String>,
    successes: usize,
) -> Result<SearchResponse, String> {
    if successes == 0 {
        Err(format!("All lyric providers failed: {}", errors.join("; ")))
    } else {
        Ok(SearchResponse {
            results,
            warnings: errors,
        })
    }
}

/// Explicit searches bypass the automatic match/miss cache and retain alternative versions.
/// A successful empty provider response is distinct from all providers being unavailable.
pub(crate) fn search(
    title: &str,
    artist: &str,
    current: impl Fn() -> bool + Sync,
) -> Result<SearchResponse, String> {
    check_current(&current)?;
    let (title, artist) = (title.trim(), artist.trim());
    if title.is_empty() && artist.is_empty() {
        return Err("Enter a song title or artist".into());
    }
    if title.len() > 500 || artist.len() > 500 {
        return Err("Search title or artist is too long".into());
    }
    let client = client()?;
    let mut results = Vec::new();
    let mut errors = Vec::new();
    let mut successes = 0;
    let outcomes = std::thread::scope(|scope| {
        let client = &client;
        let current = &current;
        let handles = [Provider::Amll, Provider::Lrclib, Provider::Netease].map(|provider| {
            (
                provider,
                scope.spawn(move || {
                    request(
                        client,
                        &search_url(provider, title, artist),
                        provider as usize,
                        current,
                    )
                    .and_then(|data| search_results(provider, &data))
                }),
            )
        });
        handles.map(|(provider, handle)| {
            (
                provider,
                handle
                    .join()
                    .unwrap_or_else(|_| Err("Provider search interrupted".into())),
            )
        })
    });
    for (provider, outcome) in outcomes {
        match outcome {
            Ok(items) => {
                successes += 1;
                // Keep space for each provider, including less common recordings.
                results.extend(rank_results(items, title, artist).into_iter().take(8));
            }
            Err(error) => errors.push(format!("{}: {error}", provider.name())),
        }
    }
    check_current(&current)?;
    finish_search(rank_results(results, title, artist), errors, successes)
}

fn provider_lyrics(provider: Provider, data: &Value) -> Option<Lyrics> {
    let mut lyrics = match provider {
        Provider::Amll if data["status"].as_u64() == Some(200) => {
            data["data"]["lyrics"].as_str().and_then(parse_ttml)
        }
        Provider::Lrclib => lrclib_lyrics(data),
        Provider::Netease
            if data["code"].as_u64() == Some(200)
                && data["uncollected"].as_bool() != Some(true) =>
        {
            if data["nolyric"].as_bool() == Some(true) {
                Some(Lyrics {
                    instrumental: true,
                    ..Default::default()
                })
            } else {
                let text = data["lrc"]["lyric"].as_str()?;
                parse_lrc(text).or_else(|| {
                    // Untimed lyrics may also be returned; preserve text without inventing timestamps.
                    if text.trim_start().starts_with('[') {
                        None
                    } else {
                        lrclib_lyrics(&serde_json::json!({"plainLyrics": text}))
                    }
                })
            }
        }
        _ => None,
    }?;
    lyrics.source = provider.name().into();
    lyrics.credits = match provider {
        Provider::Amll => joined_names(&data["data"]["authorUsernames"]),
        Provider::Netease => data["lyricUser"]["nickname"]
            .as_str()
            .unwrap_or_default()
            .into(),
        Provider::Lrclib => String::new(),
    };
    Some(lyrics)
}

fn fetch_with(
    client: &Client,
    result: &SearchResult,
    current: &impl Fn() -> bool,
) -> Result<Lyrics, String> {
    check_current(current)?;
    if let Some(mut lyrics) = result.lyrics.clone() {
        lyrics.source = result.source.clone();
        check_current(current)?;
        return require_sync(lyrics);
    }
    let url = match result.provider {
        Provider::Amll => format!("https://api.amll.dev/v1/lyrics/get?id={}", result.id),
        Provider::Lrclib => format!("https://lrclib.net/api/get/{}", result.id),
        Provider::Netease => format!(
            "https://music.163.com/api/song/lyric?id={}&lv=-1&kv=-1&tv=-1",
            result.id
        ),
    };
    let data = request(client, &url, result.provider as usize, current)?;
    let lyrics = provider_lyrics(result.provider, &data).ok_or_else(|| {
        format!(
            "{} returned no usable lyrics for this recording",
            result.source
        )
    })?;
    check_current(current)?;
    require_sync(lyrics)
}

fn require_sync(lyrics: Lyrics) -> Result<Lyrics, String> {
    if quality(&lyrics).0 > 0 {
        Ok(lyrics)
    } else {
        Err("No synchronized lyrics for this recording. Try another match.".into())
    }
}

/// The user explicitly chooses a recording, so do not apply the playing track's metadata filter.
/// Manual choices never overwrite the automatic match cache.
pub(crate) fn fetch(result: &SearchResult, current: impl Fn() -> bool) -> Result<Lyrics, String> {
    check_current(&current)?;
    fetch_with(&client()?, result, &current)
}

fn automatic_match(result: &SearchResult, title: &str, artist: &str, duration: f64) -> bool {
    !title.trim().is_empty()
        && !artist.trim().is_empty()
        && matches_recording(title, artist, &result.title, &result.artist)
        && (duration <= 0.0 || (result.duration > 0.0 && (result.duration - duration).abs() <= 2.0))
}

fn lrclib_lyrics(data: &Value) -> Option<Lyrics> {
    if data["instrumental"].as_bool() == Some(true) {
        Some(Lyrics {
            instrumental: true,
            ..Default::default()
        })
    } else {
        let mut best = data["lyricsfile"].as_str().and_then(parse_lyricsfile);
        if let Some(lyrics) = data["syncedLyrics"].as_str().and_then(parse_lrc) {
            choose_lyrics(&mut best, lyrics);
        }
        if best.is_none() {
            best = data["plainLyrics"]
                .as_str()
                .filter(|text| !text.trim().is_empty())
                .map(|text| Lyrics {
                    lines: text
                        .lines()
                        .take(3000)
                        .map(|line| LyricLine {
                            start: 0.0,
                            end: 0.0,
                            word_sync: false,
                            words: vec![LyricWord {
                                text: line.into(),
                                start: 0.0,
                                end: 0.0,
                            }],
                        })
                        .collect(),
                    ..Default::default()
                });
        }
        best
    }
}

fn quality(lyrics: &Lyrics) -> (u8, f64, usize) {
    if !lyrics.is_synchronized() {
        return (u8::from(lyrics.instrumental), 0.0, 0);
    }
    let words = lyrics
        .lines
        .iter()
        .filter(|line| line.has_word_sync())
        .count();
    let timed = lyrics
        .lines
        .iter()
        .filter(|line| line.start.is_finite() && line.end.is_finite() && line.end > line.start)
        .count();
    if words > 0 {
        (3, words as f64 / lyrics.lines.len() as f64, timed)
    } else if timed > 0 {
        (2, 0.0, timed)
    } else {
        (0, 0.0, 0)
    }
}

fn choose_lyrics(best: &mut Option<Lyrics>, candidate: Lyrics) {
    if quality(&candidate).0 > 0
        && best
            .as_ref()
            .is_none_or(|best| quality(&candidate) > quality(best))
    {
        *best = Some(candidate);
    }
}

fn needs_fallback(best: Option<&Lyrics>) -> bool {
    !best.is_some_and(|lyrics| {
        !lyrics.lines.is_empty() && lyrics.lines.iter().all(LyricLine::has_word_sync)
    })
}

pub(crate) fn resolve(
    title: &str,
    artist: &str,
    album: &str,
    duration: f64,
    current: impl Fn() -> bool,
) -> Option<Lyrics> {
    if !current() {
        return None;
    }
    // Old caches can contain plain text or a lower-quality automatic choice.
    let key = format!("sync-v1\n{title}\n{artist}\n{album}\n{}", duration.round());
    let path = cache_path(&key);
    if let Some(path) = &path
        && let Ok(meta) = path.metadata()
        && meta.len() <= 2_000_000
        && meta
            .modified()
            .ok()
            .and_then(|t| t.elapsed().ok())
            .is_some_and(|age| age < Duration::from_secs(7 * 86400))
        && let Ok(bytes) = std::fs::read(path)
        && let Ok(lyrics) = serde_json::from_slice::<Option<Lyrics>>(&bytes)
        && lyrics.as_ref().is_none_or(|lyrics| quality(lyrics).0 > 0)
    {
        // Retry misses promptly after outages; successful metadata matches live for a week.
        if lyrics.is_some()
            || meta
                .modified()
                .ok()
                .and_then(|t| t.elapsed().ok())
                .is_some_and(|age| age < Duration::from_secs(300))
        {
            return lyrics;
        }
    }
    let client = client().ok()?;
    let mut best = None;
    let search = url(
        "https://api.amll.dev/v1/lyrics/search",
        &[
            ("musicName", title.into()),
            ("artistName", artist.into()),
            ("pageSize", "8".into()),
        ],
    );
    if let Some(data) = get(&client, &search, 0, &current)
        && let Some(items) = data["data"]["items"].as_array()
    {
        let matches: Vec<_> = items
            .iter()
            .filter(|item| {
                item["musicNames"].as_array().is_some_and(|names| {
                    names
                        .iter()
                        .any(|n| normalize(n.as_str().unwrap_or_default()) == normalize(title))
                }) && item["artistNames"].as_array().is_some_and(|names| {
                    names
                        .iter()
                        .any(|n| normalize(n.as_str().unwrap_or_default()) == normalize(artist))
                })
            })
            .collect();
        if matches.len() == 1 {
            let item = matches[0];
            if let Some(id) = item["id"].as_u64()
                && let Some(data) = get(
                    &client,
                    &format!("https://api.amll.dev/v1/lyrics/get?id={id}"),
                    0,
                    &current,
                )
                && let Some(mut lyrics) = data["data"]["lyrics"].as_str().and_then(parse_ttml)
                && (duration <= 0.0 || lyrics.lines.iter().all(|l| l.end <= duration + 3.0))
            {
                lyrics.source = "AMLL TTML DB".into();
                lyrics.credits = data["data"]["authorUsernames"]
                    .as_array()
                    .map(|a| {
                        a.iter()
                            .filter_map(Value::as_str)
                            .collect::<Vec<_>>()
                            .join(", ")
                    })
                    .unwrap_or_default();
                choose_lyrics(&mut best, lyrics);
            }
        }
    }
    if needs_fallback(best.as_ref()) {
        let mut pairs = vec![("track_name", title.into()), ("artist_name", artist.into())];
        if !album.is_empty() {
            pairs.push(("album_name", album.into()));
        }
        if (1.0..=3600.0).contains(&duration) {
            pairs.push(("duration", duration.round().to_string()));
        }
        if let Some(data) = get(
            &client,
            &url("https://lrclib.net/api/get", &pairs),
            1,
            &current,
        ) && matches_recording(
            title,
            artist,
            data["trackName"].as_str().unwrap_or_default(),
            data["artistName"].as_str().unwrap_or_default(),
        ) && (duration <= 0.0
            || data["duration"]
                .as_f64()
                .is_some_and(|d| (d - duration).abs() <= 2.0))
        {
            let result = lrclib_lyrics(&data);
            if let Some(mut result) = result {
                result.source = "LRCLIB".into();
                choose_lyrics(&mut best, result);
            }
        }
    }
    if needs_fallback(best.as_ref())
        && let Some(data) = get(
            &client,
            &search_url(Provider::Netease, title, artist),
            Provider::Netease as usize,
            &current,
        )
        && let Ok(mut results) = search_results(Provider::Netease, &data)
    {
        results.retain(|result| automatic_match(result, title, artist, duration));
        results.sort_by_key(|result| (normalize(&result.album) != normalize(album), result.id));
        for result in results.into_iter().take(2) {
            if let Ok(lyrics) = fetch_with(&client, &result, &current) {
                choose_lyrics(&mut best, lyrics);
            }
            if !current() || !needs_fallback(best.as_ref()) {
                break;
            }
        }
    }
    if !current() {
        return None;
    }
    if let Some(path) = path {
        if let Some(dir) = path.parent() {
            let _ = std::fs::create_dir_all(dir);
            if let Ok(entries) = std::fs::read_dir(dir) {
                let mut entries: Vec<_> = entries
                    .flatten()
                    .filter(|e| e.path().extension().is_some_and(|x| x == "json"))
                    .collect();
                entries.sort_by_key(|e| e.metadata().and_then(|m| m.modified()).ok());
                let excess = entries.len().saturating_sub(127);
                for entry in entries.into_iter().take(excess) {
                    let _ = std::fs::remove_file(entry.path());
                }
            }
        }
        if let Ok(bytes) = serde_json::to_vec(&best)
            && bytes.len() <= 2_000_000
        {
            let _ = std::fs::write(path, bytes);
        }
    }
    best
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn synchronized_selection_rejects_plain_text_in_automatic_and_manual_paths() {
        let data = serde_json::json!({"plainLyrics": "No timestamps\nAnother line"});
        let plain = lrclib_lyrics(&data).unwrap();
        let mut best = None;
        choose_lyrics(&mut best, plain.clone());
        assert!(
            best.is_none(),
            "automatic selection requires synchronization"
        );
        let result = SearchResult {
            title: "Song".into(),
            artist: "Singer".into(),
            album: String::new(),
            duration: 120.0,
            source: "LRCLIB".into(),
            timing: "Text".into(),
            provider: Provider::Lrclib,
            id: 1,
            lyrics: Some(plain),
        };
        assert!(
            fetch(&result, || true)
                .unwrap_err()
                .contains("synchronized")
        );
        assert!(rank_results(vec![result], "Song", "Singer").is_empty());
    }

    #[test]
    fn synchronized_selection_prefers_words_then_lines_over_instrumental_status() {
        let line = parse_lrc("[00:01]A real timed line").unwrap();
        let words = parse_ttml(r#"<tt><body><p begin="1s" end="5s"><span begin="1s" end="2s">Real </span><span begin="2s" end="5s">words</span></p></body></tt>"#).unwrap();
        let instrumental = Lyrics {
            instrumental: true,
            ..Default::default()
        };
        let mut best = Some(instrumental.clone());
        choose_lyrics(&mut best, line.clone());
        assert_eq!(best.as_ref().unwrap().lines.len(), 1);
        choose_lyrics(&mut best, words);
        assert!(best.as_ref().unwrap().has_words());
        choose_lyrics(&mut best, line);
        choose_lyrics(&mut best, instrumental);
        assert!(best.as_ref().unwrap().has_words());
    }

    #[test]
    fn manual_search_keeps_versions_and_exposes_duration_without_auto_accepting_them() {
        let data = serde_json::json!({"code": 200, "result": {"songs": [
            {"id": 1432459359_u64, "name": "Blinding Lights", "duration": 200040,
             "artists": [{"name": "The Weeknd"}], "album": {"name": "After Hours"}},
            {"id": 2028721586_u64, "name": "Blinding Lights (Live)", "duration": 253365,
             "artists": [{"name": "The Weeknd"}], "album": {"name": "Live At SoFi Stadium"}},
            {"id": 3, "name": "Blinding Lights", "duration": 230000,
             "artists": [{"name": "The Weeknd"}], "album": {"name": "Alternate"}},
            {"id": 4, "name": "Blinding Lights", "duration": 200000,
             "artists": [{"name": "Other Singer"}], "album": {"name": "Cover"}}
        ]}});
        let results = search_results(Provider::Netease, &data).unwrap();
        assert_eq!(results.len(), 4);
        assert_eq!(results[0].duration, 200.04);
        assert_eq!(results[0].album, "After Hours");
        assert_eq!(results[0].source, "NetEase");
        assert_eq!(results[0].timing, "Unknown");
        assert!(automatic_match(
            &results[0],
            "Blinding Lights",
            "The Weeknd",
            200.0
        ));
        for result in &results[1..] {
            assert!(!automatic_match(
                result,
                "Blinding Lights",
                "The Weeknd",
                200.0
            ));
        }
    }

    #[test]
    fn netease_lyric_fixture_uses_lines_and_rejects_missing_lyric_placeholder() {
        let lyrics = provider_lyrics(Provider::Netease, &serde_json::json!({
            "code": 200, "lrc": {"version": 11, "lyric": "[00:13.937]Yeah\n[00:27.311]I've been tryna call\n[00:30.028]"},
            "lyricUser": {"nickname": "Contributor"}
        })).unwrap();
        assert_eq!(lyrics.source, "NetEase");
        assert_eq!(lyrics.credits, "Contributor");
        assert_eq!(lyrics.lines[0].start, 13.937);
        assert_eq!(lyrics.lines[1].text(), "I've been tryna call");
        assert!(!lyrics.has_words());
        assert!(
            provider_lyrics(
                Provider::Netease,
                &serde_json::json!({
                    "code": 200, "uncollected": true, "lrc": {"lyric": "[00:00.00]暂无歌词"}
                })
            )
            .is_none()
        );
    }

    #[test]
    fn manual_selection_fetches_selected_recording_and_cancels_before_network() {
        let results = search_results(
            Provider::Lrclib,
            &serde_json::json!([
                {"id": 7, "trackName": "Different Song (Live)", "artistName": "Another Artist",
                 "albumName": "Live", "duration": 240.0, "syncedLyrics": "[00:01]Selected\n[00:04]"}
            ]),
        )
        .unwrap();
        let lyrics = fetch(&results[0], || true).unwrap();
        assert_eq!(lyrics.lines[0].text(), "Selected");
        assert_eq!(results[0].timing, "Lines");
        assert!(fetch(&results[0], || false).unwrap_err().contains("cancel"));
        assert!(
            search("Title", "Artist", || false)
                .unwrap_err()
                .contains("cancel")
        );
    }

    #[test]
    fn bounded_json_rejects_large_and_cancelled_responses() {
        assert!(read_json(&b"{\"ok\":true}"[..], &|| true).is_ok());
        assert!(read_json(&vec![b' '; 2_000_001][..], &|| true).is_err());
        assert!(
            read_json(&b"{}"[..], &|| false)
                .unwrap_err()
                .contains("cancel")
        );
        let calls = std::cell::Cell::new(0);
        assert!(
            read_json(&vec![b' '; 20_000][..], &|| {
                calls.set(calls.get() + 1);
                calls.get() < 2
            })
            .unwrap_err()
            .contains("cancel")
        );
    }

    #[test]
    fn manual_search_deduplicates_caps_and_ranks_exact_recordings() {
        let items: Vec<_> = (1..=40)
            .map(|id| {
                serde_json::json!({
                    "id": id, "trackName": if id == 40 { "Requested" } else { "Other" },
                    "artistName": "Singer", "duration": 100.0, "syncedLyrics": "[00:01]Text"
                })
            })
            .collect();
        let mut results = search_results(Provider::Lrclib, &Value::Array(items)).unwrap();
        results.push(results[0].clone());
        let results = rank_results(results, "Requested", "Singer");
        assert_eq!(results.len(), 24);
        assert_eq!(results[0].title, "Requested");
        assert_eq!(results[0].timing, "Lines");
        let ids: std::collections::HashSet<_> =
            results.iter().map(|r| (r.provider, r.id)).collect();
        assert_eq!(ids.len(), results.len());
    }

    #[test]
    fn search_distinguishes_empty_success_from_provider_failure() {
        assert!(
            search_results(
                Provider::Netease,
                &serde_json::json!({"code": 200, "result": {"songCount": 0}})
            )
            .unwrap()
            .is_empty()
        );
        assert!(search_results(Provider::Netease, &serde_json::json!({"code": 301})).is_err());
        assert!(search_results(Provider::Amll, &serde_json::json!({"status": 503})).is_err());
        assert!(
            search_results(
                Provider::Lrclib,
                &serde_json::json!({"error": "unavailable"})
            )
            .is_err()
        );
        assert!(
            finish_search(vec![], vec!["NetEase: unavailable".into()], 1)
                .unwrap()
                .results
                .is_empty()
        );
        assert!(
            finish_search(vec![], vec!["NetEase: unavailable".into()], 0)
                .unwrap_err()
                .contains("NetEase")
        );
    }

    #[test]
    fn partial_outages_are_preserved_with_empty_or_usable_results() {
        for results in [vec![], search_results(Provider::Lrclib, &serde_json::json!([{"id":1,"trackName":"Title","artistName":"Artist","duration":120.0,"plainLyrics":"Words"}])).unwrap()] {
            let count = results.len();
            let response = finish_search(results, vec!["NetEase: unavailable".into()], 1).unwrap();
            assert_eq!(response.results.len(), count);
            assert_eq!(response.warnings, vec!["NetEase: unavailable"]);
        }
    }

    #[test]
    fn amll_manual_search_preserves_locator_and_reports_unknown_timing() {
        let results = search_results(
            Provider::Amll,
            &serde_json::json!({
                "status": 200, "data": {"items": [{"id": 8269588720193923_u64,
                    "musicNames": ["Blinding Lights"], "artistNames": ["The Weeknd"],
                    "albumNames": ["Blinding Lights"], "authorUsernames": ["cybaka520"]}]}
            }),
        )
        .unwrap();
        assert_eq!(results[0].id, 8269588720193923);
        assert_eq!(results[0].title, "Blinding Lights");
        assert_eq!(results[0].artist, "The Weeknd");
        assert_eq!(results[0].duration, 0.0);
        assert_eq!(results[0].timing, "Unknown");
    }

    #[test]
    fn netease_text_fallback_and_instrumental_do_not_invent_word_timing() {
        let text = provider_lyrics(
            Provider::Netease,
            &serde_json::json!({
                "code": 200, "lrc": {"lyric": "First line\nSecond line"}
            }),
        )
        .unwrap();
        assert_eq!(timing(&text), "Text");
        assert!(!text.has_words());
        let instrumental = provider_lyrics(
            Provider::Netease,
            &serde_json::json!({
                "code": 200, "nolyric": true
            }),
        )
        .unwrap();
        assert!(instrumental.instrumental);
        let mut best = Some(text);
        choose_lyrics(&mut best, instrumental);
        assert!(best.unwrap().instrumental);
    }

    #[test]
    #[ignore = "Requires NetEase's public search and lyric endpoints; does not write cache"]
    fn netease_read_only_smoke() {
        let client = client().unwrap();
        let search_url = search_url(Provider::Netease, "Blinding Lights", "The Weeknd");
        let data = request(&client, &search_url, Provider::Netease as usize, &|| true).unwrap();
        let results = search_results(Provider::Netease, &data).unwrap();
        let result = results
            .iter()
            .find(|r| {
                r.album == "After Hours"
                    && automatic_match(r, "Blinding Lights", "The Weeknd", 200.0)
            })
            .unwrap();
        let lyrics = fetch_with(&client, result, &|| true).unwrap();
        assert!(lyrics.lines.len() > 10);
        assert_eq!(lyrics.source, "NetEase");
        assert_eq!(timing(&lyrics), "Lines");
        println!(
            "{search_url}\nhttps://music.163.com/api/song/lyric?id={}&lv=-1&kv=-1&tv=-1\n{}: {} by {}, {:.3} seconds, {} lines, timing {}",
            result.id,
            result.source,
            result.title,
            result.artist,
            result.duration,
            lyrics.lines.len(),
            timing(&lyrics)
        );
    }

    #[test]
    #[ignore = "Requires all public search endpoints; does not write cache"]
    fn manual_search_read_only_smoke() {
        let results = search("Blinding Lights", "The Weeknd", || true)
            .unwrap()
            .results;
        assert!(!results.is_empty());
        assert!(results.len() <= 24);
        for source in ["AMLL TTML DB", "LRCLIB", "NetEase"] {
            let found = results.iter().find(|r| r.source == source).expect(source);
            let lyrics = fetch(found, || true).unwrap();
            assert!(!lyrics.lines.is_empty());
            println!(
                "{}: {} search results; {} lyric lines; {}",
                source,
                results.iter().filter(|r| r.source == source).count(),
                lyrics.lines.len(),
                timing(&lyrics)
            );
        }
    }

    #[test]
    fn provider_checks_partial_word_timing_before_stopping_fallback() {
        let lyrics = parse_ttml("<tt><p begin='1' end='3'><span begin='1' end='2'>Hello </span><span begin='2' end='3'>world</span></p><p begin='5' end='6'>Next line</p></tt>").unwrap();
        assert!(needs_fallback(Some(&lyrics)));
    }

    #[test]
    fn provider_prefers_more_complete_word_timing() {
        let partial = parse_ttml("<tt><p begin='1' end='3'><span begin='1' end='2'>Hello </span><span begin='2' end='3'>world</span></p><p begin='5' end='6'>Next line</p></tt>").unwrap();
        let complete = parse_lyricsfile("lines:\n- text: Hello world\n  start_ms: 1000\n  end_ms: 3000\n  words: [{text: 'Hello ', start_ms: 1000}, {text: world, start_ms: 2000}]\n- text: Next line\n  start_ms: 5000\n  end_ms: 6000\n  words: [{text: 'Next ', start_ms: 5000}, {text: line, start_ms: 5500}]").unwrap();
        let mut best = Some(complete);
        choose_lyrics(&mut best, partial);
        let best = best.unwrap();
        assert!(best.lines.iter().all(LyricLine::has_word_sync));
        assert!(!needs_fallback(Some(&best)));
    }

    #[test]
    fn lrclib_uses_line_sync_when_lyricsfile_is_only_partially_valid() {
        let lyrics = lrclib_lyrics(&serde_json::json!({
            "lyricsfile": "lines:\n- text: First\n  start_ms: 1000\n  end_ms: 3000\n- text: Lost line\n  start_ms: 5000\n  end_ms: 0",
            "syncedLyrics": "[00:01.00]First\n[00:05.00]Lost line\n[00:07.00]",
            "plainLyrics": "First\nLost line"
        })).unwrap();
        assert_eq!(lyrics.lines.len(), 2);
        assert_eq!(lyrics.lines[1].text(), "Lost line");
        assert!(!lyrics.has_words());
    }

    #[test]
    fn lrclib_keeps_complete_lrc_when_partial_lyricsfile_has_real_word_timing() {
        let lyrics = lrclib_lyrics(&serde_json::json!({
            "lyricsfile": "lines:\n- text: Hello world\n  start_ms: 1000\n  end_ms: 4000\n  words: [{text: 'Hello ', start_ms: 1000, end_ms: 2000}, {text: world, start_ms: 2000, end_ms: 4000}]\n- text: Lost line\n  start_ms: 5000\n  end_ms: 0",
            "syncedLyrics": "[00:01.00]Hello world\n[00:05.00]Lost line\n[00:07.00]",
            "plainLyrics": "Hello world\nLost line"
        })).unwrap();
        assert_eq!(lyrics.lines.len(), 2);
        assert_eq!(lyrics.lines[0].text(), "Hello world");
        assert_eq!(lyrics.lines[1].text(), "Lost line");
        assert!(!lyrics.has_words());
    }

    #[test]
    fn provider_prefers_timed_lines_over_plain_text() {
        let mut best = lrclib_lyrics(&serde_json::json!({"plainLyrics": "First\nNext"}));
        choose_lyrics(
            &mut best,
            parse_lrc("[00:01.00]First\n[00:05.00]Next").unwrap(),
        );
        assert_eq!(best.unwrap().active(2.0), Some(0));
        assert!(lrclib_lyrics(&serde_json::json!({"plainLyrics": " \n "})).is_none());
    }

    #[test]
    fn recording_match_preserves_version_markers() {
        assert!(super::matches_recording(
            "Hello!", "A Singer", "hello", "A SINGER"
        ));
        assert!(!super::matches_recording(
            "Hello (Live)",
            "A Singer",
            "Hello",
            "A Singer"
        ));
        assert!(!super::matches_recording(
            "Hello",
            "A Singer",
            "Hello",
            "Other Singer"
        ));
    }

    #[test]
    #[ignore = "Requires public lyric providers and writes the application lyric cache"]
    fn public_provider_smoke() {
        let lyrics = super::resolve(
            "Blinding Lights",
            "The Weeknd",
            "After Hours",
            200.0,
            || true,
        )
        .expect("provider lyrics");
        assert!(lyrics.has_words());
        assert!(lyrics.lines.len() > 10);
        assert!(lyrics.lines.iter().all(|l| l.end <= 203.0));
        println!(
            "{}: {} timed lines; credits: {}",
            lyrics.source,
            lyrics.lines.len(),
            lyrics.credits
        );
    }
}
