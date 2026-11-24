//! Application-owned interface copy. Media metadata and provider credits stay verbatim.

use std::sync::OnceLock;
use std::sync::atomic::{AtomicU8, Ordering};

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub(crate) enum Language {
    #[default]
    System,
    English,
    Vietnamese,
}

impl Language {
    pub(crate) fn parse(value: &str) -> Option<Self> {
        match value.trim().to_ascii_lowercase().as_str() {
            "system" => Some(Self::System),
            "en" => Some(Self::English),
            "vi" => Some(Self::Vietnamese),
            _ => None,
        }
    }

    pub(crate) fn as_str(self) -> &'static str {
        match self {
            Self::System => "system",
            Self::English => "en",
            Self::Vietnamese => "vi",
        }
    }
}

static LANGUAGE: AtomicU8 = AtomicU8::new(0);
static SYSTEM_LANGID: OnceLock<u16> = OnceLock::new();

pub(crate) fn set_language(language: Language) {
    let value = match language {
        Language::System => 0,
        Language::English => 1,
        Language::Vietnamese => 2,
    };
    LANGUAGE.store(value, Ordering::Relaxed);
}

/// The saved preference, which may still be System.
pub(crate) fn language() -> Language {
    match LANGUAGE.load(Ordering::Relaxed) {
        1 => Language::English,
        2 => Language::Vietnamese,
        _ => Language::System,
    }
}

/// The effective language; use this in caches rather than the saved preference.
pub(crate) fn resolved_language() -> Language {
    resolve_language(language())
}

pub(crate) fn language_for_ui_langid(preference: Language, langid: u16) -> Language {
    match preference {
        Language::System if langid & 0x03ff == 0x002a => Language::Vietnamese,
        Language::System => Language::English,
        explicit => explicit,
    }
}

pub(crate) fn resolve_language(preference: Language) -> Language {
    if preference == Language::System {
        language_for_ui_langid(preference, *SYSTEM_LANGID.get_or_init(system_langid))
    } else {
        preference
    }
}

#[cfg(windows)]
fn system_langid() -> u16 {
    #[link(name = "kernel32")]
    unsafe extern "system" {
        fn GetUserDefaultUILanguage() -> u16;
    }
    // SAFETY: This parameterless Windows API returns the user's UI language identifier.
    unsafe { GetUserDefaultUILanguage() }
}

#[cfg(not(windows))]
fn system_langid() -> u16 {
    0
}

macro_rules! catalog {
    ($($key:ident => ($english:literal, $vietnamese:literal)),+ $(,)?) => {
        #[derive(Clone, Copy, Debug, PartialEq, Eq)]
        enum Message { $($key),+ }

        impl Message {
            #[cfg(test)]
            const ALL: &'static [Self] = &[$(Self::$key),+];

            fn from_english(text: &str) -> Option<Self> {
                match text {
                    $($english => Some(Self::$key),)+
                    _ => None,
                }
            }

            fn pair(self) -> (&'static str, &'static str) {
                match self {
                    $(Self::$key => ($english, $vietnamese),)+
                }
            }
        }
    };
}

// A single exhaustive pair per typed message makes one-sided additions impossible.
catalog! {
    Settings => ("Settings", "Cài đặt"),
    SettingsMenu => ("Settings…", "Cài đặt…"),
    SettingsTitle => ("HiyoDyna Settings", "Cài đặt HiyoDyna"),
    About => ("About HiyoDyna", "Giới thiệu HiyoDyna"),
    AboutHeading => ("ABOUT", "GIỚI THIỆU"),
    Back => ("Back", "Quay lại"),
    BackToSettings => ("Back to settings", "Quay lại cài đặt"),
    Close => ("Close", "Đóng"),
    Minimize => ("Minimize", "Thu nhỏ"),
    Maximize => ("Maximize", "Phóng to"),
    Expand => ("Expand", "Mở rộng"),
    Restore => ("Restore", "Khôi phục"),
    RestoreWindow => ("Restore window", "Khôi phục cửa sổ"),
    Restart => ("Restart", "Khởi động lại"),
    Exit => ("Exit", "Thoát"),
    Cancel => ("Cancel", "Hủy"),
    Apply => ("Apply", "Áp dụng"),
    Ok => ("OK", "OK"),
    Retry => ("Retry", "Thử lại"),
    Change => ("Change", "Thay đổi"),
    Reset => ("Reset", "Đặt lại"),
    RestoreDefault => ("Restore default", "Khôi phục mặc định"),
    RestoreDefaultShortcut => ("Restore default shortcut", "Khôi phục phím tắt mặc định"),
    ResetDefaults => ("Reset to defaults", "Khôi phục cài đặt mặc định"),
    SettingsFolder => ("Settings folder", "Thư mục cài đặt"),
    On => ("On", "Bật"),
    Off => ("Off", "Tắt"),
    Enabled => ("Enabled", "Đã bật"),
    Disabled => ("Disabled", "Đã tắt"),
    Unavailable => ("unavailable", "không khả dụng"),
    Unknown => ("Unknown", "Chưa rõ"),
    Appearance => ("Appearance", "Giao diện"),
    AppearanceDescription => ("Color & depth", "Màu sắc và chiều sâu"),
    Motion => ("Motion", "Chuyển động"),
    MotionDescription => ("Pace & spring", "Tốc độ và độ nảy"),
    ClockPosition => ("Clock & position", "Đồng hồ và vị trí"),
    ClockPositionDescription => ("Time & placement", "Thời gian và cách bố trí"),
    MusicLyrics => ("Music & lyrics", "Nhạc và lời bài hát"),
    MusicLyricsDescription => ("Timing & display", "Đồng bộ và hiển thị"),
    Application => ("Application", "Ứng dụng"),
    ApplicationDescription => ("Startup & updates", "Khởi động và cập nhật"),
    UnlimitedFps => ("Unlimited FPS", "FPS không giới hạn"),
    PreferencesDescription => ("Appearance, motion and playback", "Giao diện, chuyển động và phát nhạc"),
    Accent => ("Accent", "Màu nhấn"),
    Ice => ("Ice", "Băng"),
    Amber => ("Amber", "Hổ phách"),
    Mint => ("Mint", "Bạc hà"),
    SoftShadow => ("Soft shadow", "Bóng đổ mềm"),
    MotionSpeed => ("Motion speed", "Tốc độ chuyển động"),
    Speed => ("Speed", "Tốc độ"),
    Slow => ("Slow", "Chậm"),
    Normal => ("Normal", "Bình thường"),
    Fast => ("Fast", "Nhanh"),
    Bounce => ("Bounce", "Độ nảy"),
    Low => ("Low", "Thấp"),
    Medium => ("Medium", "Vừa"),
    High => ("High", "Cao"),
    TimeFormat => ("Time format", "Định dạng giờ"),
    TwentyFourHour => ("24 hour", "24 giờ"),
    TwelveHour => ("12 hour", "12 giờ"),
    ShowSeconds => ("Show seconds", "Hiện giây"),
    Seconds => ("Seconds", "Giây"),
    TopGap => ("Top gap", "Khoảng cách trên"),
    DockPosition => ("Dock position", "Vị trí neo"),
    Left => ("Left", "Trái"),
    Center => ("Center", "Giữa"),
    Right => ("Right", "Phải"),
    Lyrics => ("Lyrics", "Lời bài hát"),
    LyricsMode => ("Lyrics mode", "Chế độ lời bài hát"),
    Auto => ("Auto", "Tự động"),
    Lines => ("Lines", "Theo dòng"),
    Words => ("Words", "Theo từ"),
    Text => ("Text", "Văn bản"),
    TimingOffset => ("Timing offset", "Độ lệch thời gian"),
    LyricsTimingOffset => ("Lyrics timing offset", "Độ lệch thời gian lời bài hát"),
    OffsetReset => ("{offset}s Reset", "{offset} giây · Đặt lại"),
    ClearLyricsCache => ("Clear lyrics cache", "Xóa bộ nhớ đệm lời bài hát"),
    CompactLyrics => ("Compact lyrics", "Lời bài hát thu gọn"),
    ShowCompactLyrics => ("Show compact lyrics", "Hiện lời bài hát thu gọn"),
    HideCompactLyrics => ("Hide compact lyrics", "Ẩn lời bài hát thu gọn"),
    CompactLyricsOn => ("Compact lyrics: On", "Lời thu gọn: Bật"),
    CompactLyricsOff => ("Compact lyrics: Off", "Lời thu gọn: Tắt"),
    EnableCompactLyrics => ("Enable compact lyrics", "Bật lời bài hát thu gọn"),
    DisableCompactLyrics => ("Disable compact lyrics", "Tắt lời bài hát thu gọn"),
    ToggleCompactLyrics => ("Toggle compact lyrics", "Bật hoặc tắt lời bài hát thu gọn"),
    StartWithWindows => ("Start with Windows", "Khởi động cùng Windows"),
    Language => ("Language", "Ngôn ngữ"),
    System => ("System", "Theo hệ thống"),
    SystemDefault => ("System default", "Mặc định hệ thống"),
    English => ("English", "English"),
    Vietnamese => ("Vietnamese", "Tiếng Việt"),
    Shortcut => ("Shortcut", "Phím tắt"),
    GlobalShortcut => ("Global shortcut", "Phím tắt toàn hệ thống"),
    ChangeShortcut => ("Change shortcut", "Đổi phím tắt"),
    CaptureShortcut => ("Press a shortcut…", "Nhấn tổ hợp phím…"),
    CaptureShortcutAscii => ("Press a shortcut...", "Nhấn tổ hợp phím…"),
    CaptureShortcutInstruction => ("Press a key combination, then Apply. Escape cancels.", "Nhấn tổ hợp phím, rồi bấm Áp dụng. Nhấn Escape để hủy."),
    CaptureShortcutModifier => ("Press a shortcut with at least one modifier.", "Nhấn tổ hợp phím có ít nhất một phím bổ trợ."),
    ApplyShortcutInstruction => ("Choose Apply to use this shortcut.", "Bấm Áp dụng để dùng phím tắt này."),
    ApplyingShortcut => ("Applying shortcut...", "Đang áp dụng phím tắt…"),
    ShortcutSupportedKey => ("Use a modifier and a supported key.", "Dùng một phím bổ trợ cùng một phím được hỗ trợ."),
    ShortcutInvalid => ("Use at least one modifier and a supported key.", "Dùng ít nhất một phím bổ trợ cùng một phím được hỗ trợ."),
    ShortcutConflict => ("This shortcut is unavailable. Your previous shortcut is unchanged.", "Phím tắt này không khả dụng. Phím tắt trước đó vẫn được giữ nguyên."),
    ShortcutConflictInstruction => ("This shortcut is unavailable. Choose a different combination. Your previous shortcut is unchanged.", "Phím tắt này không khả dụng. Hãy chọn tổ hợp khác. Phím tắt trước đó vẫn được giữ nguyên."),
    ShortcutRetained => ("Shortcut unavailable. The previous shortcut is unchanged.", "Phím tắt không khả dụng. Phím tắt trước đó vẫn được giữ nguyên."),
    ShortcutSaved => ("Shortcut updated.", "Đã cập nhật phím tắt."),
    ShortcutUnavailable => ("Shortcut unavailable", "Phím tắt không khả dụng"),
    ShortcutFallback => ("Shortcut unavailable. Use the tray menu to restore Island interaction.", "Phím tắt không khả dụng. Dùng menu khay hệ thống để khôi phục tương tác với Island."),
    ShortcutHint => ("{shortcut} fades the Island and lets clicks pass through. Press it again to restore interaction.", "{shortcut} làm mờ Island và cho phép nhấp xuyên qua. Nhấn lại để khôi phục tương tác."),
    EffectiveShortcut => ("Current shortcut: {shortcut}", "Phím tắt hiện tại: {shortcut}"),
    ShortcutPreview => ("Shortcut: {shortcut}", "Phím tắt: {shortcut}"),
    ChangeShortcutPreview => ("Change shortcut: {shortcut}", "Đổi phím tắt: {shortcut}"),
    FadePassthrough => ("Fade and click through", "Làm mờ và nhấp xuyên qua"),
    FadePassthroughHint => ("Fade and click through ({shortcut})", "Làm mờ và nhấp xuyên qua ({shortcut})"),
    RestoreInteraction => ("Restore Island interaction", "Khôi phục tương tác với Island"),
    Updates => ("Updates", "Cập nhật"),
    UpdateChannel => ("Update channel", "Kênh cập nhật"),
    Build => ("Build", "Bản thử nghiệm"),
    Stable => ("Stable", "Bản ổn định"),
    CheckUpdates => ("Check for updates", "Kiểm tra bản cập nhật"),
    CheckPublishedRelease => ("Check for a published release.", "Kiểm tra bản phát hành mới."),
    Checking => ("Checking…", "Đang kiểm tra…"),
    CheckingAscii => ("Checking...", "Đang kiểm tra…"),
    CheckingUpdates => ("Checking for updates…", "Đang kiểm tra bản cập nhật…"),
    CheckingUpdatesAscii => ("Checking for updates...", "Đang kiểm tra bản cập nhật…"),
    UpToDate => ("Up to date", "Đang dùng bản mới nhất"),
    UpToDateSentence => ("You are up to date.", "Bạn đang dùng bản mới nhất."),
    UpToDateTag => ("Up to date: {tag}", "Đang dùng bản mới nhất: {tag}"),
    UpdateAvailable => ("Update available", "Có bản cập nhật"),
    UpdateAvailableTag => ("Update available: {tag}", "Có bản cập nhật: {tag}"),
    LatestRelease => ("Latest published release", "Bản phát hành mới nhất"),
    LatestReleaseTag => ("Latest published release: {tag}", "Bản phát hành mới nhất: {tag}"),
    NoPublishedRelease => ("No published release", "Chưa có bản phát hành"),
    NoPublishedReleasePeriod => ("No published release.", "Chưa có bản phát hành."),
    NoPublishedReleaseSentence => ("No published release is available for this channel.", "Kênh này chưa có bản phát hành."),
    UnableToCheck => ("Unable to check", "Không thể kiểm tra"),
    UnableToCheckUpdates => ("Unable to check for updates. Try again.", "Không thể kiểm tra bản cập nhật. Hãy thử lại."),
    UnableToCheckUpdatesPlease => ("Unable to check for updates. Please try again.", "Không thể kiểm tra bản cập nhật. Vui lòng thử lại."),
    OpenReleasePage => ("Open release page", "Mở trang phát hành"),
    UpdateDetails => ("Release: {tag}\nPublished: {published}", "Bản phát hành: {tag}\nNgày phát hành: {published}"),
    AboutTagline => ("A little space. More possibility.", "Một không gian nhỏ. Thêm nhiều khả năng."),
    AboutTechnology => ("Built with Rust and native Windows APIs.", "Được xây dựng bằng Rust và API gốc của Windows."),
    AboutProviders => ("Lyrics by AMLL TTML DB and LRCLIB.", "Lời bài hát từ AMLL TTML DB và LRCLIB."),
    AboutProvidersAll => ("Lyrics by AMLL TTML DB, LRCLIB and NetEase.", "Lời bài hát từ AMLL TTML DB, LRCLIB và NetEase."),
    AboutInspiration => ("Inspired by Spicy Lyrics.", "Lấy cảm hứng từ Spicy Lyrics."),
    AboutScroll => ("Scroll settings to explore all controls.", "Cuộn trang cài đặt để xem tất cả tùy chọn."),
    AboutReturn => ("Press Escape or Close to return.", "Nhấn Escape hoặc Đóng để quay lại."),
    AboutPaths => ("HiyoDyna {version}\n\nSettings: {settings_path}\nUpdate log: {log_path}", "HiyoDyna {version}\n\nCài đặt: {settings_path}\nNhật ký cập nhật: {log_path}"),
    AboutLocations => ("HiyoDyna {version}\n\nSettings: {settings}\nUpdate log: {log}", "HiyoDyna {version}\n\nCài đặt: {settings}\nNhật ký cập nhật: {log}"),
    Clock => ("Clock", "Đồng hồ"),
    Music => ("Music", "Nhạc"),
    MusicPlayer => ("Music player", "Trình phát nhạc"),
    OpenMusic => ("Open music player", "Mở trình phát nhạc"),
    OpenClock => ("Open clock", "Mở đồng hồ"),
    ClockPage => ("Clock page", "Trang đồng hồ"),
    MusicPage => ("Music page", "Trang nhạc"),
    ClockAccessible => ("HiyoDyna clock", "Đồng hồ HiyoDyna"),
    ClockSourceAccessible => ("HiyoDyna clock — {source}", "Đồng hồ HiyoDyna — {source}"),
    LyricsAccessible => ("HiyoDyna lyrics: {lyrics}", "Lời bài hát HiyoDyna: {lyrics}"),
    MediaAccessible => ("HiyoDyna: {title} — {artist}", "HiyoDyna: {title} — {artist}"),
    NowPlaying => ("NOW PLAYING", "ĐANG PHÁT"),
    MediaUnavailable => ("Media unavailable", "Không có nội dung phát"),
    MusicWelcome => ("Your music, right here", "Âm nhạc của bạn, ngay tại đây"),
    PlayMusicInstruction => ("Play a song in Spotify or another media app.", "Phát một bài hát trên Spotify hoặc ứng dụng nghe nhạc khác."),
    ControlsInstruction => ("Controls and lyrics appear automatically.", "Nút điều khiển và lời bài hát sẽ tự động hiện ra."),
    TimingUnavailable => ("Timing unavailable", "Chưa có dữ liệu thời gian"),
    ResumeLyrics => ("Resume lyrics", "Theo dõi lời đang phát"),
    ResumeFollowingLyrics => ("Resume following lyrics", "Tiếp tục theo dõi lời đang phát"),
    Instrumental => ("Instrumental", "Nhạc không lời"),
    FindingLyrics => ("Finding your lyrics…", "Đang tìm lời bài hát…"),
    NoSyncedLyrics => ("No synced lyrics found", "Không tìm thấy lời bài hát đồng bộ"),
    NoMediaSession => ("No media session", "Không có phiên phát nhạc"),
    PlaybackTimingUnavailable => ("Playback timing unavailable", "Chưa có thời gian phát nhạc"),
    EnjoyMusic => ("Enjoy the music. Controls are ready.", "Thưởng thức âm nhạc. Các nút điều khiển đã sẵn sàng."),
    ChooseMediaSource => ("Choose media source", "Chọn nguồn phát nhạc"),
    UseSource => ("Use {source}", "Dùng {source}"),
    PreviousTrack => ("Previous track", "Bài trước"),
    NextTrack => ("Next track", "Bài tiếp theo"),
    Pause => ("Pause", "Tạm dừng"),
    Play => ("Play", "Phát"),
    PlaybackPosition => ("Playback position", "Vị trí phát"),
    PlaybackPositionSeconds => ("Playback position in seconds", "Vị trí phát tính bằng giây"),
    FindLyrics => ("Find lyrics", "Tìm lời bài hát"),
    FindLyricsMenu => ("Find lyrics…", "Tìm lời bài hát…"),
    SearchLyrics => ("Search lyrics", "Tìm lời bài hát"),
    SearchResults => ("Search results", "Kết quả tìm kiếm"),
    SearchLyricsTitle => ("Search lyrics - HiyoDyna", "Tìm lời bài hát - HiyoDyna"),
    SearchTagline => ("Find the right recording, then make it yours.", "Tìm đúng bản thu và chọn lời bài hát phù hợp."),
    MatchingRecordings => ("MATCHING RECORDINGS", "BẢN THU PHÙ HỢP"),
    Title => ("Title", "Tên bài hát"),
    TitleMnemonic => ("&Title", "&Tên bài hát"),
    Artist => ("Artist", "Nghệ sĩ"),
    ArtistMnemonic => ("&Artist", "&Nghệ sĩ"),
    Search => ("Search", "Tìm kiếm"),
    SearchMnemonic => ("&Search", "Tìm &kiếm"),
    UseSelectedLyrics => ("Use selected lyrics", "Dùng lời bài hát đã chọn"),
    UseSelectedLyricsMnemonic => ("&Use selected lyrics", "&Dùng lời bài hát đã chọn"),
    UnknownDuration => ("unknown duration", "chưa rõ thời lượng"),
    NoActiveTrackSearch => ("No active media track. You can search; play a track before applying lyrics.", "Chưa có bài hát đang phát. Bạn vẫn có thể tìm kiếm; hãy phát một bài trước khi áp dụng lời bài hát."),
    SearchInstruction => ("Enter a title and artist, then choose Search.", "Nhập tên bài hát và nghệ sĩ, rồi bấm Tìm kiếm."),
    SearchAgainInstruction => ("Search again to find lyrics for these details.", "Tìm lại để lấy lời bài hát theo thông tin này."),
    SearchNeedsTitle => ("Enter a song title to search.", "Nhập tên bài hát để tìm kiếm."),
    SearchingProviders => ("Searching lyric providers...", "Đang tìm trong các nguồn lời bài hát…"),
    SearchingProvidersEllipsis => ("Searching lyric providers…", "Đang tìm trong các nguồn lời bài hát…"),
    FetchingLyrics => ("Fetching selected lyrics...", "Đang tải lời bài hát đã chọn…"),
    FetchingLyricsEllipsis => ("Fetching selected lyrics…", "Đang tải lời bài hát đã chọn…"),
    SearchNeedsPlayback => ("Play a media track before applying lyrics. You can still search.", "Hãy phát một bài hát trước khi áp dụng lời bài hát. Bạn vẫn có thể tìm kiếm."),
    NoSynchronizedLyrics => ("No synchronized lyrics found. Try a different title or artist.", "Không tìm thấy lời bài hát đồng bộ. Hãy thử tên bài hát hoặc nghệ sĩ khác."),
    NoLyricsFound => ("No lyrics found.", "Không tìm thấy lời bài hát."),
    SelectLyricsInstruction => ("Choose Use selected lyrics to apply this match.", "Bấm Dùng lời bài hát đã chọn để áp dụng kết quả này."),
    PlaybackChanged => ("Playback changed. Search again to choose lyrics for the current track.", "Bài hát đã thay đổi. Hãy tìm lại để chọn lời cho bài hát hiện tại."),
    TrackChanged => ("The playing track changed. Open Find lyrics again to choose lyrics for the current track.", "Bài hát đang phát đã thay đổi. Mở lại Tìm lời bài hát để chọn lời cho bài hát hiện tại."),
    SingleSearchResult => ("{count} result. Choose a match, then Use selected lyrics.", "{count} kết quả. Chọn bản phù hợp, rồi bấm Dùng lời bài hát đã chọn."),
    MultipleSearchResults => ("{count} results. Choose a match, then Use selected lyrics.", "{count} kết quả. Chọn bản phù hợp, rồi bấm Dùng lời bài hát đã chọn."),
    SingleIncompleteResult => ("{count} result; search incomplete. Some providers are unavailable. Try again.", "{count} kết quả; tìm kiếm chưa hoàn tất. Một số nguồn hiện không khả dụng. Hãy thử lại."),
    MultipleIncompleteResults => ("{count} results; search incomplete. Some providers are unavailable. Try again.", "{count} kết quả; tìm kiếm chưa hoàn tất. Một số nguồn hiện không khả dụng. Hãy thử lại."),
    LegacySearchResults => ("{count} result(s). Choose a match, then Use selected lyrics.", "{count} kết quả. Chọn bản phù hợp, rồi bấm Dùng lời bài hát đã chọn."),
    LegacyIncompleteResults => ("{count} result(s); search incomplete. {warning}", "{count} kết quả; tìm kiếm chưa hoàn tất. {warning}"),
    PartialSearchResults => ("{count} result(s); some providers could not be reached.", "{count} kết quả; không thể kết nối với một số nguồn lời bài hát."),
    SearchStartError => ("Could not start lyric search: {error}", "Không thể bắt đầu tìm lời bài hát: {error}"),
    LyricsFetchError => ("Could not fetch lyrics: {error}", "Không thể tải lời bài hát: {error}"),
    SearchStartSummary => ("Could not start lyric search. Please try again.", "Không thể bắt đầu tìm lời bài hát. Vui lòng thử lại."),
    LyricsFetchSummary => ("Could not fetch lyrics. Please try again.", "Không thể tải lời bài hát. Vui lòng thử lại."),
    LyricsRequestError => ("Lyrics request failed: {error}", "Không thể lấy lời bài hát: {error}"),
    LyricsRequestFailed => ("Lyrics request failed. Please try again.", "Không thể lấy lời bài hát. Vui lòng thử lại."),
    ProvidersUnavailable => ("Some providers are unavailable. Try again.", "Một số nguồn hiện không khả dụng. Hãy thử lại."),
    ProviderUnavailable => ("The provider is temporarily unavailable. Try again.", "Nguồn lời bài hát tạm thời không khả dụng. Hãy thử lại."),
    SearchWindowError => ("Could not create the lyrics search window.", "Không thể tạo cửa sổ tìm lời bài hát."),
    SearchControlsError => ("Could not create the lyrics search controls.", "Không thể tạo các nút điều khiển tìm lời bài hát."),
    SearchUpdatesError => ("Could not start lyrics search updates.", "Không thể bắt đầu cập nhật kết quả tìm lời bài hát."),
    SettingsWindowError => ("Could not create settings window", "Không thể tạo cửa sổ cài đặt"),
    SettingsMonitorError => ("Could not read settings monitor work area", "Không thể xác định vùng hiển thị cửa sổ cài đặt"),
    StartupError => ("HiyoDyna could not start.", "Không thể khởi động HiyoDyna."),
    StartupErrorDetails => ("HiyoDyna could not start: {error}", "Không thể khởi động HiyoDyna: {error}"),
    TechnicalDetails => ("Technical details", "Chi tiết kỹ thuật"),
    Error => ("Error", "Lỗi"),
    Notice => ("Notice", "Thông báo"),
    OperationFailed => ("The operation could not be completed. Please try again.", "Không thể hoàn tất thao tác. Vui lòng thử lại."),
    AutostartError => ("Could not change startup settings. Try again.", "Không thể thay đổi cài đặt khởi động. Hãy thử lại."),
    ExecutableError => ("Could not resolve the HiyoDyna executable", "Không thể xác định tệp thực thi HiyoDyna"),
    ProviderCancelled => ("Lyric request cancelled", "Đã hủy yêu cầu lời bài hát"),
    ProviderTooLarge => ("Provider response exceeds 2 MB", "Dữ liệu từ nguồn lời bài hát vượt quá 2 MB"),
    InvalidProviderJson => ("Invalid provider JSON", "Dữ liệu JSON từ nguồn lời bài hát không hợp lệ"),
    ProviderStateError => ("Provider state unavailable", "Không thể truy cập trạng thái nguồn lời bài hát"),
    ProviderRateLimited => ("Provider temporarily rate limited", "Nguồn lời bài hát tạm giới hạn số yêu cầu"),
    InvalidSearchResponse => ("Invalid search response or provider unavailable", "Kết quả tìm kiếm không hợp lệ hoặc nguồn lời bài hát không khả dụng"),
    SearchNeedsTitleOrArtist => ("Enter a song title or artist", "Nhập tên bài hát hoặc nghệ sĩ"),
    SearchTooLong => ("Search title or artist is too long", "Tên bài hát hoặc nghệ sĩ quá dài"),
    ProviderInterrupted => ("Provider search interrupted", "Tìm kiếm tại nguồn lời bài hát bị gián đoạn"),
    NoSynchronizedRecording => ("No synchronized lyrics for this recording. Try another match.", "Bản thu này chưa có lời bài hát đồng bộ. Hãy chọn kết quả khác."),
    ProviderHttpError => ("Provider returned HTTP {status}", "Nguồn lời bài hát trả về mã HTTP {status}"),
    AllProvidersFailed => ("All lyric providers failed: {details}", "Không thể truy cập các nguồn lời bài hát: {details}"),
    NoUsableProviderLyrics => ("{provider} returned no usable lyrics for this recording", "{provider} không có lời bài hát phù hợp cho bản thu này"),
    Sunday => ("Sunday", "Chủ nhật"),
    Monday => ("Monday", "Thứ hai"),
    Tuesday => ("Tuesday", "Thứ ba"),
    Wednesday => ("Wednesday", "Thứ tư"),
    Thursday => ("Thursday", "Thứ năm"),
    Friday => ("Friday", "Thứ sáu"),
    Saturday => ("Saturday", "Thứ bảy"),
    January => ("January", "Tháng 1"),
    February => ("February", "Tháng 2"),
    March => ("March", "Tháng 3"),
    April => ("April", "Tháng 4"),
    May => ("May", "Tháng 5"),
    June => ("June", "Tháng 6"),
    July => ("July", "Tháng 7"),
    August => ("August", "Tháng 8"),
    September => ("September", "Tháng 9"),
    October => ("October", "Tháng 10"),
    November => ("November", "Tháng 11"),
    December => ("December", "Tháng 12"),
    Am => ("AM", "SA"),
    Pm => ("PM", "CH"),
    ClockDate => ("{weekday} · {month} {day}", "{weekday} · {day} tháng {month}"),
}

/// Translate only known application copy. Unknown input retains its original borrow.
pub(crate) fn tr(text: &str) -> &str {
    tr_in(resolved_language(), text)
}

/// Explicit-language lookup is also safe for parallel tests and previews.
pub(crate) fn tr_in(language: Language, text: &str) -> &str {
    let Some(message) = Message::from_english(text) else {
        return text;
    };
    let (english, vietnamese) = message.pair();
    if resolve_language(language) == Language::Vietnamese {
        vietnamese
    } else {
        english
    }
}

pub(crate) fn format_copy(template: &str, values: &[(&str, &str)]) -> String {
    format_copy_in(resolved_language(), template, values)
}

pub(crate) fn format_copy_in(
    language: Language,
    template: &str,
    values: &[(&str, &str)],
) -> String {
    let mut remaining = tr_in(language, template);
    let mut result = String::with_capacity(remaining.len());
    while let Some((prefix, after_open)) = remaining.split_once('{') {
        let Some((name, suffix)) = after_open.split_once('}') else {
            break;
        };
        result.push_str(prefix);
        if let Some((_, value)) = values.iter().find(|(key, _)| *key == name) {
            // Inserted metadata is never translated or parsed as another placeholder.
            result.push_str(value);
        } else {
            result.push('{');
            result.push_str(name);
            result.push('}');
        }
        remaining = suffix;
    }
    result.push_str(remaining);
    result
}

pub(crate) fn search_result_count(count: usize, incomplete: bool) -> String {
    search_result_count_in(resolved_language(), count, incomplete)
}

pub(crate) fn search_result_count_in(language: Language, count: usize, incomplete: bool) -> String {
    let template = match (count == 1, incomplete) {
        (true, false) => Message::SingleSearchResult,
        (false, false) => Message::MultipleSearchResults,
        (true, true) => Message::SingleIncompleteResult,
        (false, true) => Message::MultipleIncompleteResults,
    };
    format_copy_in(
        language,
        template.pair().0,
        &[("count", &count.to_string())],
    )
}

pub(crate) fn shortcut_hint(shortcut: Option<&str>) -> String {
    shortcut_hint_in(resolved_language(), shortcut)
}

pub(crate) fn shortcut_hint_in(language: Language, shortcut: Option<&str>) -> String {
    if let Some(shortcut) = shortcut.filter(|value| !value.is_empty()) {
        format_copy_in(
            language,
            Message::ShortcutHint.pair().0,
            &[("shortcut", shortcut)],
        )
    } else {
        tr_in(language, Message::ShortcutFallback.pair().0).to_string()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn every_typed_message_has_unique_nonempty_copy_in_both_languages() {
        let mut keys = std::collections::HashSet::new();
        for &message in Message::ALL {
            let (english, vietnamese) = message.pair();
            assert!(!english.trim().is_empty(), "{message:?}");
            assert!(!vietnamese.trim().is_empty(), "{message:?}");
            assert!(keys.insert(english), "duplicate English key: {english}");
            assert!(!vietnamese.contains('\0'), "{message:?}");
            assert_eq!(Message::from_english(english), Some(message));
            assert_eq!(tr_in(Language::English, english), english);
            assert_eq!(tr_in(Language::Vietnamese, english), vietnamese);
        }
    }

    #[test]
    fn translated_templates_preserve_each_named_substitution() {
        fn names(mut text: &str) -> Vec<&str> {
            let mut result = Vec::new();
            while let Some((_, after_open)) = text.split_once('{') {
                let (name, suffix) = after_open.split_once('}').expect("closed placeholder");
                result.push(name);
                text = suffix;
            }
            result.sort_unstable();
            result
        }
        for &message in Message::ALL {
            let (english, vietnamese) = message.pair();
            assert_eq!(names(english), names(vietnamese), "{message:?}");
        }
    }

    #[test]
    fn persisted_language_choices_round_trip_and_reject_unknown_values() {
        for (value, language) in [
            ("system", Language::System),
            ("en", Language::English),
            ("vi", Language::Vietnamese),
        ] {
            assert_eq!(Language::parse(value), Some(language));
            assert_eq!(language.as_str(), value);
        }
        assert_eq!(Language::parse(" VI "), Some(Language::Vietnamese));
        for invalid in ["", "fr", "Vietnamese", "en-US"] {
            assert_eq!(Language::parse(invalid), None);
        }
        assert_eq!(Language::default(), Language::System);
    }

    #[test]
    fn system_language_uses_the_windows_primary_language_and_explicit_choice_wins() {
        assert_eq!(
            language_for_ui_langid(Language::System, 0x042a),
            Language::Vietnamese
        );
        assert_eq!(
            language_for_ui_langid(Language::System, 0x082a),
            Language::Vietnamese
        );
        assert_eq!(
            language_for_ui_langid(Language::System, 0x0409),
            Language::English
        );
        assert_eq!(
            language_for_ui_langid(Language::System, 0),
            Language::English
        );
        assert_eq!(
            language_for_ui_langid(Language::English, 0x042a),
            Language::English
        );
        assert_eq!(
            language_for_ui_langid(Language::Vietnamese, 0x0409),
            Language::Vietnamese
        );
    }

    #[test]
    fn vietnamese_copy_covers_settings_search_player_updates_and_accessibility() {
        for (english, vietnamese) in [
            ("Settings", "Cài đặt"),
            (
                "Appearance, motion and playback",
                "Giao diện, chuyển động và phát nhạc",
            ),
            ("Find lyrics", "Tìm lời bài hát"),
            ("Use selected lyrics", "Dùng lời bài hát đã chọn"),
            (
                "No synced lyrics found",
                "Không tìm thấy lời bài hát đồng bộ",
            ),
            ("Playback position in seconds", "Vị trí phát tính bằng giây"),
            ("Check for updates", "Kiểm tra bản cập nhật"),
            (
                "Unable to check for updates. Try again.",
                "Không thể kiểm tra bản cập nhật. Hãy thử lại.",
            ),
            ("HiyoDyna could not start.", "Không thể khởi động HiyoDyna."),
            (
                "Built with Rust and native Windows APIs.",
                "Được xây dựng bằng Rust và API gốc của Windows.",
            ),
            ("Compact lyrics: On", "Lời thu gọn: Bật"),
            ("Minimize", "Thu nhỏ"),
        ] {
            assert_eq!(
                tr_in(Language::Vietnamese, english),
                vietnamese,
                "{english}"
            );
            assert_eq!(tr_in(Language::English, english), english);
        }
    }

    #[test]
    fn integration_feedback_and_fallback_names_are_localized_without_touching_parameters() {
        for (english, vietnamese) in [
            ("Hide compact lyrics", "Ẩn lời bài hát thu gọn"),
            ("Show compact lyrics", "Hiện lời bài hát thu gọn"),
            ("Applying shortcut...", "Đang áp dụng phím tắt…"),
            (
                "Shortcut unavailable. The previous shortcut is unchanged.",
                "Phím tắt không khả dụng. Phím tắt trước đó vẫn được giữ nguyên.",
            ),
            ("Restore default shortcut", "Khôi phục phím tắt mặc định"),
            (
                "Press a shortcut with at least one modifier.",
                "Nhấn tổ hợp phím có ít nhất một phím bổ trợ.",
            ),
            (
                "Choose Apply to use this shortcut.",
                "Bấm Áp dụng để dùng phím tắt này.",
            ),
            (
                "Use a modifier and a supported key.",
                "Dùng một phím bổ trợ cùng một phím được hỗ trợ.",
            ),
            (
                "Check for a published release.",
                "Kiểm tra bản phát hành mới.",
            ),
            ("No published release.", "Chưa có bản phát hành."),
            (
                "Unable to check for updates. Please try again.",
                "Không thể kiểm tra bản cập nhật. Vui lòng thử lại.",
            ),
            (
                "Could not start lyric search. Please try again.",
                "Không thể bắt đầu tìm lời bài hát. Vui lòng thử lại.",
            ),
            (
                "Could not fetch lyrics. Please try again.",
                "Không thể tải lời bài hát. Vui lòng thử lại.",
            ),
            (
                "{count} result(s); some providers could not be reached.",
                "{count} kết quả; không thể kết nối với một số nguồn lời bài hát.",
            ),
            (
                "This shortcut is unavailable. Choose a different combination. Your previous shortcut is unchanged.",
                "Phím tắt này không khả dụng. Hãy chọn tổ hợp khác. Phím tắt trước đó vẫn được giữ nguyên.",
            ),
        ] {
            assert_eq!(
                tr_in(Language::Vietnamese, english),
                vietnamese,
                "{english}"
            );
        }
        assert_eq!(
            format_copy_in(
                Language::Vietnamese,
                "Up to date: {tag}",
                &[("tag", "pre-42")]
            ),
            "Đang dùng bản mới nhất: pre-42"
        );
        assert_eq!(
            format_copy_in(
                Language::Vietnamese,
                "HiyoDyna clock — {source}",
                &[("source", "Spotify")]
            ),
            "Đồng hồ HiyoDyna — Spotify"
        );
        assert_eq!(
            format_copy_in(
                Language::Vietnamese,
                "HiyoDyna: {title} — {artist}",
                &[("title", "Music {artist}"), ("artist", "Artist")]
            ),
            "HiyoDyna: Music {artist} — Artist"
        );
        assert_eq!(
            format_copy_in(
                Language::Vietnamese,
                "HiyoDyna could not start: {error}",
                &[("error", "CreateWindowExW failed (Windows error 5)")]
            ),
            "Không thể khởi động HiyoDyna: CreateWindowExW failed (Windows error 5)"
        );
        assert_eq!(
            format_copy_in(
                Language::Vietnamese,
                "HiyoDyna {version}\n\nSettings: {settings}\nUpdate log: {log}",
                &[
                    ("version", "0.0.1"),
                    ("settings", "C:\\a.ini"),
                    ("log", "C:\\update.log")
                ]
            ),
            "HiyoDyna 0.0.1\n\nCài đặt: C:\\a.ini\nNhật ký cập nhật: C:\\update.log"
        );
    }

    #[test]
    fn unknown_text_and_authentic_metadata_are_returned_without_copying_or_translation() {
        let metadata = String::from("Đường về · The Weeknd · LRCLIB · pre-42");
        let translated = tr_in(Language::Vietnamese, &metadata);
        assert_eq!(translated, metadata);
        assert_eq!(translated.as_ptr(), metadata.as_ptr());
        assert_eq!(tr_in(Language::Vietnamese, ""), "");
    }

    #[test]
    fn named_substitutions_translate_only_the_template_and_do_not_reparse_metadata() {
        assert_eq!(
            format_copy_in(
                Language::Vietnamese,
                "Use {source}",
                &[("source", "Spotify")]
            ),
            "Dùng Spotify"
        );
        assert_eq!(
            format_copy_in(
                Language::Vietnamese,
                "HiyoDyna lyrics: {lyrics}",
                &[("lyrics", "Play {source} 🎵")]
            ),
            "Lời bài hát HiyoDyna: Play {source} 🎵"
        );
        assert_eq!(
            format_copy_in(
                Language::English,
                "{artist}: {title}",
                &[("title", "Music"), ("artist", "Artist")]
            ),
            "Artist: Music"
        );
        assert_eq!(
            format_copy_in(Language::English, "{missing} / unmatched {", &[]),
            "{missing} / unmatched {"
        );
    }

    #[test]
    fn result_counts_use_english_plural_rules_and_natural_vietnamese() {
        assert_eq!(
            search_result_count_in(Language::English, 0, false),
            "0 results. Choose a match, then Use selected lyrics."
        );
        assert_eq!(
            search_result_count_in(Language::English, 1, false),
            "1 result. Choose a match, then Use selected lyrics."
        );
        assert_eq!(
            search_result_count_in(Language::English, 2, true),
            "2 results; search incomplete. Some providers are unavailable. Try again."
        );
        assert_eq!(
            search_result_count_in(Language::Vietnamese, 1, false),
            "1 kết quả. Chọn bản phù hợp, rồi bấm Dùng lời bài hát đã chọn."
        );
        assert_eq!(
            search_result_count_in(Language::Vietnamese, 7, true),
            "7 kết quả; tìm kiếm chưa hoàn tất. Một số nguồn hiện không khả dụng. Hãy thử lại."
        );
    }

    #[test]
    fn shortcut_hints_use_only_the_effective_binding_or_explain_the_menu_fallback() {
        assert_eq!(
            shortcut_hint_in(Language::English, Some("Ctrl+Alt+D")),
            "Ctrl+Alt+D fades the Island and lets clicks pass through. Press it again to restore interaction."
        );
        assert_eq!(
            shortcut_hint_in(Language::Vietnamese, Some("Ctrl+Shift+F9")),
            "Ctrl+Shift+F9 làm mờ Island và cho phép nhấp xuyên qua. Nhấn lại để khôi phục tương tác."
        );
        assert_eq!(
            shortcut_hint_in(Language::Vietnamese, None),
            "Phím tắt không khả dụng. Dùng menu khay hệ thống để khôi phục tương tác với Island."
        );
    }
}
