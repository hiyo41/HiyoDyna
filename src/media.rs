use std::time::{Duration, Instant};
#[derive(Clone, Debug)]
pub(crate) struct Timeline {
    pub position: f64,
    pub duration: f64,
    pub playing: bool,
    pub rate: f64,
    pub sampled: Instant,
}

impl Timeline {
    pub fn position_at(&self, now: Instant) -> Option<f64> {
        if !self.duration.is_finite()
            || self.duration <= 0.0
            || !self.position.is_finite()
            || !self.rate.is_finite()
        {
            return None;
        }
        let elapsed = if self.playing {
            now.saturating_duration_since(self.sampled).as_secs_f64() * self.rate
        } else {
            0.0
        };
        Some((self.position + elapsed).clamp(0.0, self.duration))
    }
}

use crate::lyrics::Lyrics;
use std::collections::VecDeque;
use std::sync::atomic::{AtomicBool, AtomicU64, Ordering};
use std::sync::{Arc, Condvar, Mutex};
use windows::Media::Control::{
    GlobalSystemMediaTransportControlsSession as Session,
    GlobalSystemMediaTransportControlsSessionManager as Manager,
    GlobalSystemMediaTransportControlsSessionPlaybackStatus as PlaybackStatus,
};
use windows_future::{AsyncStatus, IAsyncOperation};

/// Upper bound for one awaited WinRT async call. A misbehaving provider that
/// never completes must not pin the media pipeline forever; a timeout counts
/// as an ordinary failure so the worker keeps polling and the UI keeps running.
const METADATA_TIMEOUT: Duration = Duration::from_secs(10);
const TRANSPORT_TIMEOUT: Duration = Duration::from_secs(5);
const MANAGER_TIMEOUT: Duration = Duration::from_secs(15);
const ASYNC_POLL: Duration = Duration::from_millis(25);

/// Like [`IAsyncOperation::join`] but with a deadline.
fn wait<T: windows_core::RuntimeType>(
    operation: &IAsyncOperation<T>,
    timeout: Duration,
) -> windows_core::Result<T> {
    let deadline = std::time::Instant::now() + timeout;
    loop {
        match operation.Status() {
            Ok(AsyncStatus::Completed) => return operation.GetResults(),
            Ok(AsyncStatus::Error | AsyncStatus::Canceled) => {
                let code = operation.ErrorCode().unwrap_or_default();
                return Err(windows_core::Error::from_hresult(code));
            }
            // Started and any future status value: keep waiting until the deadline.
            Ok(_) => {}
            Err(error) => return Err(error),
        }
        if std::time::Instant::now() >= deadline {
            let _ = operation.Cancel();
            // ERROR_TIMEOUT (1460) reads clearly in the auto-update-style logs.
            return Err(windows_core::Error::from_hresult(
                windows_core::HRESULT::from_win32(1460),
            ));
        }
        std::thread::sleep(ASYNC_POLL);
    }
}

#[derive(Clone, Debug)]
pub(crate) enum MediaCommand {
    Toggle,
    Previous,
    Next,
    Seek(f64),
    Source(String),
    RefreshLyrics,
}

#[derive(Clone, Debug, Default)]
pub(crate) struct MediaSnapshot {
    pub generation: u64,
    pub key: String,
    pub title: String,
    pub artist: String,
    pub album: String,
    pub source: String,
    pub sources: Vec<String>,
    pub timeline: Option<Timeline>,
    pub playing: bool,
    pub can_toggle: bool,
    pub can_previous: bool,
    pub can_next: bool,
    pub can_seek: bool,
    pub artwork: Option<Arc<image::RgbaImage>>,
    pub lyrics: Option<Arc<Lyrics>>,
    pub manual_lyrics: bool,
    pub loading: bool,
    pub unavailable: bool,
}

fn apply_lyrics(snapshot: &mut MediaSnapshot, generation: u64, lyrics: Option<Arc<Lyrics>>) {
    if snapshot.generation == generation && !snapshot.manual_lyrics {
        snapshot.lyrics = lyrics.filter(|lyrics| lyrics.is_synchronized() || lyrics.instrumental);
        snapshot.loading = false;
    }
}

fn publish_snapshot(output: &mut MediaSnapshot, mut next: MediaSnapshot, changed: bool) {
    if !changed && output.key == next.key {
        next.generation = output.generation;
        next.artwork = output.artwork.clone();
    }
    if output.key == next.key && output.generation == next.generation {
        if next.artwork.is_none() {
            next.artwork = output.artwork.clone();
        }
        next.lyrics = output.lyrics.clone();
        next.manual_lyrics = output.manual_lyrics;
        next.loading = output.loading;
    }
    *output = next;
}

pub(crate) struct MediaWorker {
    commands: Arc<Mutex<CommandQueue>>,
    state: Arc<Mutex<MediaSnapshot>>,
    stop: Arc<AtomicBool>,
    thread: std::thread::Thread,
}

struct QueuedCommand {
    command: MediaCommand,
    target: Option<(String, String, u64)>,
}
impl QueuedCommand {
    fn matches(&self, current: &MediaSnapshot, generation: u64) -> bool {
        self.target.as_ref().is_some_and(|(source, key, expected)| {
            *source == current.source && *key == current.key && *expected == generation
        })
    }
}

#[derive(Default)]
struct CommandQueue {
    commands: VecDeque<QueuedCommand>,
    seek: Option<QueuedCommand>,
}
impl CommandQueue {
    fn push(&mut self, command: MediaCommand, snapshot: &MediaSnapshot) {
        let target = (!matches!(
            command,
            MediaCommand::Source(_) | MediaCommand::RefreshLyrics
        ))
        .then(|| {
            (
                snapshot.source.clone(),
                snapshot.key.clone(),
                snapshot.generation,
            )
        });
        let queued = QueuedCommand { command, target };
        if matches!(queued.command, MediaCommand::Seek(_)) {
            self.seek = Some(queued);
        } else if self.commands.len() < 16 {
            self.commands.push_back(queued);
        }
    }
    fn take(&mut self) -> VecDeque<QueuedCommand> {
        // Keep the seek replaceable while the worker reads session metadata.
        std::mem::take(&mut self.commands)
    }
    fn take_seek(&mut self) -> Option<QueuedCommand> {
        if self.commands.iter().any(|queued| {
            matches!(
                queued.command,
                MediaCommand::Source(_)
                    | MediaCommand::Previous
                    | MediaCommand::Next
                    | MediaCommand::RefreshLyrics
            )
        }) {
            return None;
        }
        self.seek.take()
    }
}

type LyricRequest = (u64, String, String, String, f64, Option<Session>);
#[derive(Default)]
struct LyricMailbox {
    request: Mutex<Option<LyricRequest>>,
    wake: Condvar,
}
impl LyricMailbox {
    fn replace(&self, request: LyricRequest) {
        if let Ok(mut slot) = self.request.lock() {
            *slot = Some(request);
            self.wake.notify_one();
        }
    }
    fn take(&self, stop: &AtomicBool) -> Option<LyricRequest> {
        let mut slot = self.request.lock().ok()?;
        while slot.is_none() && !stop.load(Ordering::Acquire) {
            slot = self
                .wake
                .wait_timeout(slot, Duration::from_millis(350))
                .ok()?
                .0;
        }
        if stop.load(Ordering::Acquire) {
            None
        } else {
            slot.take()
        }
    }
}

impl MediaWorker {
    #[cfg(test)]
    pub(crate) fn test_worker() -> Self {
        Self {
            commands: Arc::new(Mutex::new(CommandQueue::default())),
            state: Arc::new(Mutex::new(MediaSnapshot::default())),
            stop: Arc::new(AtomicBool::new(false)),
            thread: std::thread::current(),
        }
    }
    #[cfg(test)]
    pub(crate) fn take_test_seek(&self) -> Option<f64> {
        match self.commands.lock().ok()?.seek.take()?.command {
            MediaCommand::Seek(seconds) => Some(seconds),
            _ => None,
        }
    }
    pub fn start() -> Self {
        let commands = Arc::new(Mutex::new(CommandQueue::default()));
        let state = Arc::new(Mutex::new(MediaSnapshot::default()));
        let stop = Arc::new(AtomicBool::new(false));
        let output = state.clone();
        let stopped = stop.clone();
        let input = commands.clone();
        let thread = std::thread::spawn(move || worker(input, output, stopped))
            .thread()
            .clone();
        Self {
            commands,
            state,
            stop,
            thread,
        }
    }
    pub fn command(&self, command: MediaCommand, snapshot: &MediaSnapshot) {
        if let Ok(mut queue) = self.commands.lock() {
            queue.push(command, snapshot);
        }
        self.thread.unpark();
    }
    pub fn snapshot(&self) -> MediaSnapshot {
        // Poison recovery keeps the last published state visible instead of
        // flashing the UI empty after a (recovered) worker panic; the worker
        // itself republishes on its next pass.
        self.state
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner())
            .clone()
    }
    pub fn use_lyrics(&self, key: &str, generation: u64, lyrics: Arc<Lyrics>) -> bool {
        let Ok(mut state) = self.state.lock() else {
            return false;
        };
        if state.key != key
            || state.generation != generation
            || (!lyrics.is_synchronized() && !lyrics.instrumental)
        {
            return false;
        }
        state.lyrics = Some(lyrics);
        state.manual_lyrics = true;
        state.loading = false;
        true
    }
}
impl Drop for MediaWorker {
    fn drop(&mut self) {
        self.stop.store(true, Ordering::Release);
        self.thread.unpark();
    }
}

fn artwork(session: &Session) -> Option<Arc<image::RgbaImage>> {
    use windows::Storage::Streams::{DataReader, InputStreamOptions};
    let properties = {
        let operation = session.TryGetMediaPropertiesAsync().ok()?;
        wait(&operation, METADATA_TIMEOUT).ok()?
    };
    let stream = {
        let operation = properties.Thumbnail().ok()?.OpenReadAsync().ok()?;
        wait(&operation, METADATA_TIMEOUT).ok()?
    };
    let length = stream.Size().ok()?;
    if length == 0 || length > 4_000_000 {
        return None;
    }
    let reader = DataReader::CreateDataReader(&stream.GetInputStreamAt(0).ok()?).ok()?;
    reader
        .SetInputStreamOptions(InputStreamOptions::None)
        .ok()?;
    let loaded = {
        let operation = reader.LoadAsync(length as u32).ok()?;
        wait(&operation, METADATA_TIMEOUT).ok()?
    };
    let mut bytes = vec![0; loaded as usize];
    reader.ReadBytes(&mut bytes).ok()?;
    let decoder = image::ImageReader::new(std::io::Cursor::new(bytes))
        .with_guessed_format()
        .ok()?;
    let mut limits = image::Limits::default();
    limits.max_image_width = Some(4096);
    limits.max_image_height = Some(4096);
    limits.max_alloc = Some(64_000_000);
    let mut decoder = decoder;
    decoder.limits(limits);
    let image = decoder
        .decode()
        .ok()?
        .resize_to_fill(160, 160, image::imageops::FilterType::Triangle)
        .into_rgba8();
    Some(Arc::new(image))
}

fn read_session(session: &Session, sources: Vec<String>) -> windows::core::Result<MediaSnapshot> {
    let p = session
        .TryGetMediaPropertiesAsync()
        .and_then(|op| wait(&op, METADATA_TIMEOUT))?;
    let source = session.SourceAppUserModelId()?.to_string();
    let title = p.Title()?.to_string();
    let artist = p.Artist()?.to_string();
    let album = p.AlbumTitle()?.to_string();
    let mut snapshot = MediaSnapshot {
        key: format!("{source}\n{title}\n{artist}\n{album}"),
        title,
        artist,
        album,
        source,
        sources,
        ..Default::default()
    };
    refresh_playback(session, &mut snapshot)?;
    Ok(snapshot)
}

fn preferred_source_id(
    sources: &[String],
    current: Option<&str>,
    selected: Option<&str>,
) -> Option<String> {
    let matching = |source: &String, id: &str| source.eq_ignore_ascii_case(id);
    selected
        .and_then(|id| sources.iter().find(|source| matching(source, id)))
        .or_else(|| {
            sources
                .iter()
                .find(|source| source.to_ascii_lowercase().contains("spotify"))
        })
        .or_else(|| current.and_then(|id| sources.iter().find(|source| matching(source, id))))
        .cloned()
}

#[derive(Default)]
struct TrackCache {
    revision: Option<u64>,
    snapshot: MediaSnapshot,
}
impl TrackCache {
    fn read(
        &mut self,
        revision: &AtomicU64,
        read: impl FnOnce() -> windows::core::Result<MediaSnapshot>,
    ) -> windows::core::Result<Option<MediaSnapshot>> {
        let before = revision.load(Ordering::Acquire);
        if self.revision != Some(before) {
            let snapshot = read()?;
            // A track may change while TryGetMediaPropertiesAsync is pending.
            if revision.load(Ordering::Acquire) != before {
                self.revision = None;
                return Ok(None);
            }
            self.snapshot = snapshot;
            self.revision = Some(before);
        }
        Ok(Some(self.snapshot.clone()))
    }

    fn is_current(&self, revision: &AtomicU64) -> bool {
        self.revision == Some(revision.load(Ordering::Acquire))
    }
}

struct SessionWatch {
    session: Session,
    revision: Arc<AtomicU64>,
    generation_revision: u64,
    token: Option<i64>,
    cache: TrackCache,
}
impl SessionWatch {
    fn new(session: Session) -> Self {
        let revision = Arc::new(AtomicU64::new(0));
        let changed = revision.clone();
        let worker = std::thread::current();
        let token = session
            .MediaPropertiesChanged(&windows::Foundation::TypedEventHandler::new(move |_, _| {
                changed.fetch_add(1, Ordering::AcqRel);
                worker.unpark();
                Ok(())
            }))
            .ok();
        Self {
            session,
            revision,
            generation_revision: 0,
            token,
            cache: TrackCache::default(),
        }
    }

    fn read(&mut self) -> windows::core::Result<Option<MediaSnapshot>> {
        // If notifications are unavailable, retain full identity validation.
        if self.token.is_none() {
            self.cache.revision = None;
        }
        self.cache
            .read(&self.revision, || read_session(&self.session, Vec::new()))
    }
}
impl Drop for SessionWatch {
    fn drop(&mut self) {
        if let Some(token) = self.token {
            let _ = self.session.RemoveMediaPropertiesChanged(token);
        }
    }
}

fn refresh_playback(session: &Session, snapshot: &mut MediaSnapshot) -> windows::core::Result<()> {
    let playback = session.GetPlaybackInfo()?;
    let controls = playback.Controls()?;
    snapshot.playing = playback.PlaybackStatus()? == PlaybackStatus::Playing;
    snapshot.can_toggle = controls.IsPlayPauseToggleEnabled()?;
    snapshot.can_previous = controls.IsPreviousEnabled()?;
    snapshot.can_next = controls.IsNextEnabled()?;
    snapshot.can_seek = controls.IsPlaybackPositionEnabled()?;
    let rate = playback
        .PlaybackRate()
        .and_then(|r| r.Value())
        .unwrap_or(1.0);
    snapshot.timeline = read_timeline(session, snapshot.playing, rate);
    Ok(())
}

fn read_timeline(session: &Session, playing: bool, rate: f64) -> Option<Timeline> {
    session.GetTimelineProperties().ok().and_then(|t| {
        let start = t.StartTime().ok()?.Duration as f64 / 10_000_000.0;
        let duration = t.EndTime().ok()?.Duration as f64 / 10_000_000.0 - start;
        let ticks = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .ok()?
            .as_secs_f64()
            + 11_644_473_600.0;
        let age = (ticks - t.LastUpdatedTime().ok()?.UniversalTime as f64 / 10_000_000.0).max(0.0);
        authoritative_timeline(
            t.Position().ok()?.Duration as f64 / 10_000_000.0 - start,
            duration,
            playing,
            rate,
            age,
        )
    })
}

fn authoritative_timeline(
    position: f64,
    duration: f64,
    playing: bool,
    rate: f64,
    age: f64,
) -> Option<Timeline> {
    if !position.is_finite()
        || !duration.is_finite()
        || duration <= 0.0
        || !rate.is_finite()
        || !age.is_finite()
    {
        return None;
    }
    Some(Timeline {
        position: (position + if playing { age.max(0.0) * rate } else { 0.0 }).clamp(0.0, duration),
        duration,
        playing,
        rate,
        sampled: Instant::now(),
    })
}

fn worker(
    commands: Arc<Mutex<CommandQueue>>,
    output: Arc<Mutex<MediaSnapshot>>,
    stop: Arc<AtomicBool>,
) {
    let initialized = unsafe {
        windows::Win32::System::WinRT::RoInitialize(
            windows::Win32::System::WinRT::RO_INIT_MULTITHREADED,
        )
    }
    .is_ok();
    let manager = Manager::RequestAsync().and_then(|op| wait(&op, MANAGER_TIMEOUT));
    let Ok(manager) = manager else {
        if let Ok(mut s) = output.lock() {
            s.unavailable = true;
        }
        if initialized {
            unsafe {
                windows::Win32::System::WinRT::RoUninitialize();
            }
        }
        return;
    };
    let requests = Arc::new(LyricMailbox::default());
    let lyric_requests = requests.clone();
    let lyric_output = output.clone();
    let lyric_stop = stop.clone();
    std::thread::spawn(move || {
        let initialized = unsafe {
            windows::Win32::System::WinRT::RoInitialize(
                windows::Win32::System::WinRT::RO_INIT_MULTITHREADED,
            )
        }
        .is_ok();
        while let Some((generation, title, artist, album, duration, session)) =
            lyric_requests.take(&lyric_stop)
        {
            let current = || {
                !lyric_stop.load(Ordering::Acquire)
                    && lyric_output
                        .lock()
                        .is_ok_and(|s| s.generation == generation && !s.manual_lyrics)
            };
            if !current() {
                continue;
            }
            // Artwork can await WinRT and decode images; keep it off the transport worker.
            let image = session.as_ref().and_then(artwork);
            if !current() {
                continue;
            }
            if let Ok(mut s) = lyric_output.lock()
                && s.generation == generation
            {
                s.artwork = image;
            }
            let lyrics = crate::lyric_provider::resolve(&title, &artist, &album, duration, current)
                .map(Arc::new);
            if let Ok(mut s) = lyric_output.lock() {
                apply_lyrics(&mut s, generation, lyrics);
            }
        }
        if initialized {
            unsafe {
                windows::Win32::System::WinRT::RoUninitialize();
            }
        }
    });
    let mut selected: Option<String> = None;
    let mut previous_key = String::new();
    let mut generation = 0_u64;
    let mut watched: Option<SessionWatch> = None;
    while !stop.load(Ordering::Acquire) {
        // Every iteration ends in a 350 ms park (or is woken early by an event
        // or a queued command). The `continue` paths below park too, so a
        // repeating condition can never spin this thread without pause.
        let sessions = manager.GetSessions().ok();
        let sources = sessions
            .as_ref()
            .map(|s| {
                s.into_iter()
                    .filter_map(|s| s.SourceAppUserModelId().ok().map(|s| s.to_string()))
                    .collect::<Vec<_>>()
            })
            .unwrap_or_default();
        // Only re-evaluate the manual choice against a real session list; a
        // transient GetSessions failure must not forget the user's pick.
        if sessions.is_some()
            && selected.as_ref().is_some_and(|selected| {
                !sources
                    .iter()
                    .any(|source| source.eq_ignore_ascii_case(selected))
            })
        {
            selected = None;
        }
        let current_session = manager.GetCurrentSession().ok();
        let current_source = current_session
            .as_ref()
            .and_then(|session| session.SourceAppUserModelId().ok())
            .map(|source| source.to_string());
        let preferred =
            preferred_source_id(&sources, current_source.as_deref(), selected.as_deref());
        let mut session = preferred.as_ref().and_then(|id| {
            sessions.as_ref()?.into_iter().find(|session| {
                session
                    .SourceAppUserModelId()
                    .ok()
                    .is_some_and(|source| source.to_string().eq_ignore_ascii_case(id))
            })
        });
        if session.is_none() && selected.is_none() {
            session = current_session;
        }
        let pending = commands.lock().map(|mut q| q.take()).unwrap_or_default();
        for queued in pending {
            match &queued.command {
                MediaCommand::Source(id) => {
                    session = sessions.as_ref().and_then(|sessions| {
                        sessions.into_iter().find(|s| {
                            s.SourceAppUserModelId()
                                .is_ok_and(|source| source.to_string().eq_ignore_ascii_case(id))
                        })
                    });
                    selected = Some(id.clone());
                    previous_key.clear();
                }
                MediaCommand::RefreshLyrics => previous_key.clear(),
                command => {
                    if let Some(s) = &session
                        && let Ok(current) = read_session(s, Vec::new())
                        && queued.matches(&current, generation)
                    {
                        match command {
                            MediaCommand::Toggle if current.can_toggle => {
                                let _ = s
                                    .TryTogglePlayPauseAsync()
                                    .and_then(|op| wait(&op, TRANSPORT_TIMEOUT));
                            }
                            MediaCommand::Previous
                                if current.can_previous
                                    && s.TrySkipPreviousAsync()
                                        .and_then(|op| wait(&op, TRANSPORT_TIMEOUT))
                                        .unwrap_or(false) =>
                            {
                                generation = generation.wrapping_add(1);
                                previous_key.clear();
                                watched = None;
                            }
                            MediaCommand::Next
                                if current.can_next
                                    && s.TrySkipNextAsync()
                                        .and_then(|op| wait(&op, TRANSPORT_TIMEOUT))
                                        .unwrap_or(false) =>
                            {
                                generation = generation.wrapping_add(1);
                                previous_key.clear();
                                watched = None;
                            }
                            _ => {}
                        }
                    }
                }
            }
        }
        if watched.as_ref().map(|w| &w.session) != session.as_ref() {
            if watched.is_some() {
                generation = generation.wrapping_add(1);
                previous_key.clear();
            }
            watched = session.as_ref().cloned().map(SessionWatch::new);
        }
        if let Some(watch) = watched.as_mut() {
            let revision = watch.revision.load(Ordering::Acquire);
            if revision != watch.generation_revision {
                // Even identical titles may describe a new occurrence of the track.
                watch.generation_revision = revision;
                generation = generation.wrapping_add(1);
                previous_key.clear();
            }
        }
        let read = watched.as_mut().map(SessionWatch::read);
        if matches!(read, Some(Ok(None))) {
            // A race invalidated the cached read; retry after the usual pause.
            std::thread::park_timeout(Duration::from_millis(350));
            continue;
        }
        if let Some(s) = session.as_ref()
            && let Some(Ok(Some(mut snapshot))) = read
            && let Some(watch) = watched.as_mut()
            && refresh_playback(s, &mut snapshot).is_ok()
        {
            snapshot.sources = sources;
            // Timeline bounds are a cheap additional signal that the cached
            // track changed. Compare with a tolerance so live streams whose
            // window slides cannot invalidate the cache on every pass.
            let duration_changed = match (
                snapshot.timeline.as_ref().map(|t| t.duration),
                watch.cache.snapshot.timeline.as_ref().map(|t| t.duration),
            ) {
                (Some(next), Some(cached)) => (next - cached).abs() > 0.05,
                (a, b) => a != b,
            };
            if duration_changed {
                watch.cache.revision = None;
                generation = generation.wrapping_add(1);
                previous_key.clear();
                std::thread::park_timeout(Duration::from_millis(350));
                continue;
            }
            if !watch.cache.is_current(&watch.revision) {
                std::thread::park_timeout(Duration::from_millis(350));
                continue;
            }
            // The mailbox remains replaceable until identity validation has completed.
            // Only one seek may be in flight; arrivals during its await replace each other.
            let seek = commands.lock().ok().and_then(|mut q| q.take_seek());
            if let Some(queued) = seek
                && queued.matches(&snapshot, generation)
                && snapshot.can_seek
                && let MediaCommand::Seek(seconds) = queued.command
                && seconds.is_finite()
                && let Some(timeline) = &snapshot.timeline
                && let Ok(start) = s.GetTimelineProperties().and_then(|t| t.StartTime())
                && watch.cache.is_current(&watch.revision)
                && (selected.is_some()
                    || preferred_source_id(
                        &snapshot.sources,
                        manager
                            .GetCurrentSession()
                            .ok()
                            .and_then(|current| current.SourceAppUserModelId().ok())
                            .map(|id| id.to_string())
                            .as_deref(),
                        None,
                    )
                    .as_deref()
                        == Some(snapshot.source.as_str()))
            {
                let position = start
                    .Duration
                    .saturating_add((seconds.clamp(0.0, timeline.duration) * 10_000_000.0) as i64);
                let _ = s.TryChangePlaybackPositionAsync(position).and_then(|op| {
                    // A seek must not wait longer than the worker's cadence.
                    wait(&op, TRANSPORT_TIMEOUT)
                });
                // Playback may pause or change rate while the seek is awaited.
                if refresh_playback(s, &mut snapshot).is_err() {
                    snapshot.timeline = None;
                    snapshot.can_seek = false;
                }
            }
            if !watch.cache.is_current(&watch.revision) {
                std::thread::park_timeout(Duration::from_millis(350));
                continue;
            }
            snapshot.generation = generation;
            let changed = snapshot.key != previous_key;
            if changed {
                generation = generation.wrapping_add(1);
                snapshot.generation = generation;
                previous_key = snapshot.key.clone();
                let request = (
                    generation,
                    snapshot.title.clone(),
                    snapshot.artist.clone(),
                    snapshot.album.clone(),
                    snapshot.timeline.as_ref().map_or(0.0, |t| t.duration),
                    Some(s.clone()),
                );
                snapshot.loading = true;
                // Clear old text before potentially slow artwork retrieval.
                if let Ok(mut out) = output.lock() {
                    *out = snapshot.clone();
                }
                requests.replace(request);
            }
            if let Ok(mut out) = output.lock() {
                publish_snapshot(&mut out, snapshot, changed);
            }
        } else if session.is_none() {
            // A session that disappeared is the only reason to clear the
            // published snapshot. A transient read or refresh failure keeps
            // the last known state so the UI does not flash empty and lyrics
            // are not re-fetched for a track that is still playing.
            if !previous_key.is_empty() {
                generation = generation.wrapping_add(1);
            }
            previous_key.clear();
            if let Ok(mut out) = output.lock() {
                *out = MediaSnapshot {
                    generation,
                    ..Default::default()
                };
            }
        }
        std::thread::park_timeout(Duration::from_millis(350));
    }
    drop(watched);
    if initialized {
        unsafe {
            windows::Win32::System::WinRT::RoUninitialize();
        }
    }
}

#[cfg(test)]
mod tests {
    #[test]
    fn automatic_source_selection_prefers_spotify_over_current_session() {
        assert_eq!(
            preferred_source_id(
                &["Chrome.exe".into(), "Spotify.exe".into()],
                Some("Chrome.exe"),
                None,
            ),
            Some("Spotify.exe".into())
        );
    }

    #[test]
    fn spotify_matching_is_case_insensitive_and_current_is_the_fallback() {
        assert_eq!(
            preferred_source_id(&["spotify.exe".into(), "Chrome.exe".into()], None, None),
            Some("spotify.exe".into())
        );
        assert_eq!(
            preferred_source_id(&["Chrome.exe".into()], Some("Chrome.exe"), None),
            Some("Chrome.exe".into())
        );
        assert_eq!(preferred_source_id(&[], Some("Chrome.exe"), None), None);
    }

    #[test]
    fn manual_source_remains_authoritative_until_it_disappears() {
        let sources = ["Spotify.exe".into(), "Chrome.exe".into()];
        assert_eq!(
            preferred_source_id(&sources, Some("Spotify.exe"), Some("Chrome.exe")),
            Some("Chrome.exe".into())
        );
        assert_eq!(
            preferred_source_id(
                &["Spotify.exe".into()],
                Some("Spotify.exe"),
                Some("Chrome.exe")
            ),
            Some("Spotify.exe".into())
        );
    }

    #[test]
    #[ignore = "Reads live SMTC artwork and writes local diagnostic fixtures; no playback commands"]
    fn live_artwork_diagnostic() -> windows::core::Result<()> {
        use super::*;
        use windows::Storage::Streams::DataReader;
        unsafe {
            windows::Win32::System::WinRT::RoInitialize(
                windows::Win32::System::WinRT::RO_INIT_MULTITHREADED,
            )?;
        }
        let manager = Manager::RequestAsync()?.join()?;
        let current = manager.GetCurrentSession().ok();
        let sessions = manager.GetSessions()?;
        let sources = sessions
            .clone()
            .into_iter()
            .filter_map(|session| session.SourceAppUserModelId().ok().map(|id| id.to_string()))
            .collect::<Vec<_>>();
        let current_id = current
            .as_ref()
            .and_then(|session| session.SourceAppUserModelId().ok())
            .map(|id| id.to_string());
        println!(
            "automatic preferred source: {:?}",
            preferred_source_id(&sources, current_id.as_deref(), None)
        );
        let dir = std::env::var_os("HIYODYNA_PREVIEW_DIR").map(std::path::PathBuf::from);
        if let Some(dir) = &dir {
            std::fs::create_dir_all(dir).unwrap();
        }
        for (index, session) in sessions.into_iter().enumerate() {
            let properties = session.TryGetMediaPropertiesAsync()?.join()?;
            println!(
                "session {index}: source={} title={} artist={} current={}",
                session.SourceAppUserModelId()?,
                properties.Title()?,
                properties.Artist()?,
                current.as_ref() == Some(&session)
            );
            let result = (|| -> windows::core::Result<Vec<u8>> {
                let stream = properties.Thumbnail()?.OpenReadAsync()?.join()?;
                let length = stream.Size()?;
                println!("thumbnail: size={length}, type={}", stream.ContentType()?);
                assert!(length <= 4_000_000);
                let reader = DataReader::CreateDataReader(&stream.GetInputStreamAt(0)?)?;
                let loaded = reader.LoadAsync(length as u32)?.join()?;
                let mut bytes = vec![0; loaded as usize];
                reader.ReadBytes(&mut bytes)?;
                Ok(bytes)
            })();
            match result {
                Ok(bytes) => {
                    println!(
                        "read bytes={} guessed format={:?}",
                        bytes.len(),
                        image::guess_format(&bytes)
                    );
                    println!(
                        "decode={:?}",
                        image::load_from_memory(&bytes)
                            .map(|image| (image.width(), image.height()))
                    );
                    if let Some(dir) = &dir {
                        std::fs::write(dir.join(format!("live-artwork-{index}.bin")), &bytes)
                            .unwrap();
                    }
                }
                Err(error) => println!("thumbnail read failed: {error:?}"),
            }
            let image = artwork(&session);
            println!(
                "production artwork={:?}",
                image.as_ref().map(|image| image.dimensions())
            );
            if let (Some(dir), Some(image)) = (&dir, image) {
                image
                    .save(dir.join(format!("live-cover-{index}.png")))
                    .unwrap();
            }
        }
        unsafe {
            windows::Win32::System::WinRT::RoUninitialize();
        }
        Ok(())
    }

    #[test]
    fn manually_selected_lyrics_survive_refresh_and_late_auto_results() {
        let worker = MediaWorker::test_worker();
        *worker.state.lock().unwrap() = MediaSnapshot {
            key: "track".into(),
            generation: 3,
            loading: true,
            ..Default::default()
        };
        let lyrics = Arc::new(crate::lyrics::parse_lrc("[00:01]Chosen words").unwrap());
        assert!(worker.use_lyrics("track", 3, lyrics.clone()));
        let mut output = worker.state.lock().unwrap();
        assert!(!output.loading);
        apply_lyrics(&mut output, 3, Some(Arc::new(Lyrics::default())));
        assert!(Arc::ptr_eq(output.lyrics.as_ref().unwrap(), &lyrics));
        publish_snapshot(
            &mut output,
            MediaSnapshot {
                key: "track".into(),
                generation: 3,
                ..Default::default()
            },
            false,
        );
        assert!(Arc::ptr_eq(output.lyrics.as_ref().unwrap(), &lyrics));
        publish_snapshot(
            &mut output,
            MediaSnapshot {
                key: "track".into(),
                generation: 4,
                loading: true,
                ..Default::default()
            },
            true,
        );
        assert!(
            output.lyrics.is_none(),
            "explicit refresh/generation changes clear the manual selection"
        );
        apply_lyrics(&mut output, 4, Some(lyrics));
        assert!(
            output.lyrics.is_some(),
            "new generation accepts automatic lyrics"
        );
    }

    #[test]
    fn manual_lyrics_reject_old_track_or_generation() {
        let worker = MediaWorker::test_worker();
        *worker.state.lock().unwrap() = MediaSnapshot {
            key: "current".into(),
            generation: 7,
            ..Default::default()
        };
        let lyrics = Arc::new(crate::lyrics::parse_lrc("[00:01]Valid sync").unwrap());
        assert!(!worker.use_lyrics("old", 7, lyrics.clone()));
        assert!(!worker.use_lyrics("current", 6, lyrics));
        assert!(!worker.use_lyrics("current", 7, Arc::new(Lyrics::default())));
        assert!(worker.snapshot().lyrics.is_none());
    }

    use super::*;
    use std::time::Duration;

    #[test]
    fn artwork_publication_preserves_lyrics_completed_during_loading() {
        let mut initial = MediaSnapshot {
            generation: 3,
            key: "new track".into(),
            loading: true,
            ..Default::default()
        };
        let mut shared = initial.clone();
        apply_lyrics(
            &mut shared,
            3,
            Some(Arc::new(
                crate::lyrics::parse_lrc("[00:01]Timed lyrics").unwrap(),
            )),
        );
        initial.artwork = Some(Arc::new(image::RgbaImage::new(1, 1)));
        publish_snapshot(&mut shared, initial, true);
        assert!(shared.lyrics.is_some());
        assert!(!shared.loading);
        assert!(shared.artwork.is_some());
    }

    #[test]
    fn publication_preserves_artwork_completed_on_background_worker() {
        let initial = MediaSnapshot {
            generation: 3,
            key: "new track".into(),
            ..Default::default()
        };
        let mut shared = initial.clone();
        shared.artwork = Some(Arc::new(image::RgbaImage::new(1, 1)));
        publish_snapshot(&mut shared, initial, true);
        assert!(shared.artwork.is_some());
    }

    #[test]
    fn validated_track_reuses_metadata_until_media_properties_change() {
        let revision = AtomicU64::new(0);
        let reads = std::cell::Cell::new(0);
        let mut cache = TrackCache::default();
        for _ in 0..2 {
            let snapshot = cache
                .read(&revision, || {
                    reads.set(reads.get() + 1);
                    Ok(MediaSnapshot {
                        key: "track A".into(),
                        ..Default::default()
                    })
                })
                .unwrap()
                .unwrap();
            assert_eq!(snapshot.key, "track A");
        }
        assert_eq!(reads.get(), 1);
        revision.fetch_add(1, Ordering::AcqRel);
        let snapshot = cache
            .read(&revision, || {
                reads.set(reads.get() + 1);
                Ok(MediaSnapshot {
                    key: "track B".into(),
                    ..Default::default()
                })
            })
            .unwrap()
            .unwrap();
        assert_eq!(snapshot.key, "track B");
        assert_eq!(reads.get(), 2);
    }

    #[test]
    fn metadata_changed_during_await_is_never_validated_for_dispatch() {
        let revision = AtomicU64::new(0);
        let mut cache = TrackCache::default();
        let result = cache
            .read(&revision, || {
                revision.fetch_add(1, Ordering::AcqRel);
                Ok(MediaSnapshot {
                    key: "obsolete track".into(),
                    ..Default::default()
                })
            })
            .unwrap();
        assert!(result.is_none());
        let current = cache
            .read(&revision, || {
                Ok(MediaSnapshot {
                    key: "current track".into(),
                    ..Default::default()
                })
            })
            .unwrap()
            .unwrap();
        assert_eq!(current.key, "current track");
    }

    #[test]
    fn media_change_after_validation_rejects_old_seek_before_dispatch() {
        let revision = AtomicU64::new(0);
        let mut cache = TrackCache::default();
        let snapshot = cache
            .read(&revision, || {
                Ok(MediaSnapshot {
                    source: "A".into(),
                    key: "track A".into(),
                    generation: 7,
                    ..Default::default()
                })
            })
            .unwrap()
            .unwrap();
        let mut queue = CommandQueue::default();
        queue.push(MediaCommand::Seek(50.0), &snapshot);
        assert!(cache.is_current(&revision));

        // The event can precede publication of new metadata to the UI.
        revision.fetch_add(1, Ordering::AcqRel);
        assert!(!cache.is_current(&revision));
        let current = cache
            .read(&revision, || {
                Ok(MediaSnapshot {
                    key: "track B".into(),
                    ..snapshot.clone()
                })
            })
            .unwrap()
            .unwrap();
        assert!(!queue.seek.take().unwrap().matches(&current, 7));
    }

    #[test]
    fn seeks_arriving_during_validation_and_completion_keep_only_latest_position() {
        let worker = MediaWorker::test_worker();
        let revision = AtomicU64::new(0);
        let mut cache = TrackCache::default();
        let snapshot = MediaSnapshot {
            source: "A".into(),
            key: "track A".into(),
            generation: 7,
            ..Default::default()
        };
        worker.command(MediaCommand::Seek(10.0), &snapshot);
        cache
            .read(&revision, || {
                for seconds in 11..=40 {
                    worker.command(MediaCommand::Seek(f64::from(seconds)), &snapshot);
                }
                Ok(snapshot.clone())
            })
            .unwrap();
        assert_eq!(worker.take_test_seek(), Some(40.0));

        // An in-flight seek is awaited; later commands still share a single slot.
        for seconds in 41..=90 {
            worker.command(MediaCommand::Seek(f64::from(seconds)), &snapshot);
        }
        cache
            .read(&revision, || {
                panic!("unchanged track must not await metadata")
            })
            .unwrap();
        assert_eq!(worker.take_test_seek(), Some(90.0));
        assert_eq!(worker.take_test_seek(), None);
    }

    #[test]
    fn failed_metadata_refresh_does_not_validate_old_track() {
        let revision = AtomicU64::new(0);
        let mut cache = TrackCache::default();
        cache
            .read(&revision, || Ok(MediaSnapshot::default()))
            .unwrap();
        revision.fetch_add(1, Ordering::AcqRel);
        assert!(
            cache
                .read(&revision, || Err(windows::core::Error::empty()))
                .is_err()
        );
        assert!(!cache.is_current(&revision));
    }

    #[test]
    fn external_backward_seek_passes_through_authoritative_timeline() {
        let previous = authoritative_timeline(160.0, 200.0, true, 1.0, 0.0).unwrap();
        let current = authoritative_timeline(20.0, 200.0, true, 1.0, 0.0).unwrap();
        assert!(current.position < previous.position);
        assert_eq!(current.position_at(current.sampled), Some(20.0));
    }

    #[test]
    fn transport_drain_keeps_seek_available_for_coalescing_during_metadata_read() {
        let snapshot = MediaSnapshot {
            source: "A".into(),
            key: "track A".into(),
            generation: 7,
            ..Default::default()
        };
        let mut queue = CommandQueue::default();
        queue.push(MediaCommand::Toggle, &snapshot);
        for seconds in 0..100 {
            queue.push(MediaCommand::Seek(seconds as f64), &snapshot);
        }
        let commands = queue.take();
        assert_eq!(commands.len(), 1);
        assert!(matches!(commands[0].command, MediaCommand::Toggle));
        // More drag events can arrive while WinRT metadata is being read.
        queue.push(MediaCommand::Seek(120.0), &snapshot);
        assert!(matches!(
            queue.seek.take().unwrap().command,
            MediaCommand::Seek(120.0)
        ));
    }

    #[test]
    fn target_changes_arriving_during_validation_are_processed_before_seek() {
        let snapshot = MediaSnapshot {
            source: "A".into(),
            key: "track A".into(),
            generation: 7,
            ..Default::default()
        };
        for change in [
            MediaCommand::Source("B".into()),
            MediaCommand::Next,
            MediaCommand::Previous,
            MediaCommand::RefreshLyrics,
        ] {
            let mut queue = CommandQueue::default();
            queue.push(MediaCommand::Seek(10.0), &snapshot);
            assert!(queue.take().is_empty());
            queue.push(change, &snapshot);
            assert!(queue.take_seek().is_none());
            assert_eq!(queue.take().len(), 1);
            assert!(!queue.take_seek().unwrap().matches(&snapshot, 8));
        }
    }

    #[test]
    fn transport_target_rejects_source_track_and_generation_changes() {
        let snapshot = MediaSnapshot {
            source: "A".into(),
            key: "track A".into(),
            generation: 7,
            ..Default::default()
        };
        let mut queue = CommandQueue::default();
        queue.push(MediaCommand::Seek(5.0), &snapshot);
        let command = queue.seek.take().unwrap();
        assert!(command.matches(&snapshot, 7));
        assert!(!command.matches(
            &MediaSnapshot {
                source: "B".into(),
                ..snapshot.clone()
            },
            7
        ));
        assert!(!command.matches(
            &MediaSnapshot {
                key: "track B".into(),
                ..snapshot.clone()
            },
            7
        ));
        assert!(!command.matches(&snapshot, 8));
    }

    #[test]
    fn lyric_mailbox_replaces_obsolete_requests_before_resolution() {
        let mailbox = LyricMailbox::default();
        let stop = AtomicBool::new(false);
        mailbox.replace((1, "B".into(), String::new(), String::new(), 0.0, None));
        mailbox.replace((2, "C".into(), String::new(), String::new(), 0.0, None));
        assert_eq!(mailbox.take(&stop).unwrap().1, "C");
        stop.store(true, Ordering::Release);
        assert!(mailbox.take(&stop).is_none());
    }

    #[test]
    fn sparse_authoritative_updates_keep_advancing_beyond_ten_seconds() {
        assert_eq!(
            authoritative_timeline(4.0, 200.0, true, 1.0, 35.0)
                .unwrap()
                .position,
            39.0
        );
        assert_eq!(
            authoritative_timeline(4.0, 200.0, false, 1.0, 35.0)
                .unwrap()
                .position,
            4.0
        );
        assert!(authoritative_timeline(4.0, 0.0, true, 1.0, 35.0).is_none());
    }

    #[test]
    fn stale_response_cannot_overwrite_return_to_same_track_or_refresh() {
        let mut snapshot = MediaSnapshot {
            generation: 3,
            key: "same track".into(),
            loading: true,
            ..Default::default()
        };
        let lyrics = Arc::new(crate::lyrics::parse_lrc("[00:01]Timed lyrics").unwrap());
        apply_lyrics(&mut snapshot, 1, Some(lyrics.clone()));
        assert!(snapshot.loading);
        assert!(snapshot.lyrics.is_none());
        apply_lyrics(&mut snapshot, 3, Some(lyrics));
        assert!(!snapshot.loading);
        assert!(snapshot.lyrics.is_some());
    }

    #[test]
    fn timeline_interpolates_clamps_and_freezes_when_paused() {
        let now = Instant::now();
        let mut t = Timeline {
            position: 4.0,
            duration: 10.0,
            playing: true,
            rate: 1.0,
            sampled: now,
        };
        assert_eq!(t.position_at(now + Duration::from_secs(2)), Some(6.0));
        assert_eq!(t.position_at(now + Duration::from_secs(20)), Some(10.0));
        t.playing = false;
        assert_eq!(t.position_at(now + Duration::from_secs(2)), Some(4.0));
        t.duration = 0.0;
        assert_eq!(t.position_at(now), None);
    }
    #[test]
    fn slow_playback_accounts_for_source_age_beyond_track_duration() {
        let timeline = authoritative_timeline(10.0, 200.0, true, 0.5, 300.0).unwrap();
        assert_eq!(timeline.position, 160.0);
    }
}
