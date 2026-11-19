use std::fs;
use std::io::Write as _;
use std::path::{Path, PathBuf};
use std::process::Command;
use std::sync::atomic::{AtomicBool, Ordering};

use windows_sys::Win32::Foundation::{
    CloseHandle, ERROR_INVALID_PARAMETER, GetLastError, HWND, SYSTEMTIME, WAIT_OBJECT_0,
};
use windows_sys::Win32::System::SystemInformation::GetLocalTime;
use windows_sys::Win32::System::Threading::{
    OpenProcess, PROCESS_SYNCHRONIZE, WaitForSingleObject,
};
use windows_sys::Win32::UI::WindowsAndMessaging::{PostMessageW, WM_APP};

use crate::settings::{Settings, UpdateChannel};

#[derive(Clone, Debug)]
struct Release {
    tag: String,
    published: String,
}

#[derive(Clone, Debug)]
struct StageManifest {
    tag: String,
    published: String,
    channel: UpdateChannel,
    sha256: String,
}

#[derive(Debug, Default, PartialEq)]
struct UpdateState {
    applied_tag: Option<String>,
    applied_published: Option<String>,
}

struct ApplyPaths {
    staged: PathBuf,
    manifest: PathBuf,
    installed: PathBuf,
    state: PathBuf,
}

#[derive(Debug, PartialEq)]
enum ApplyOutcome {
    Applied,
    Discarded,
}

fn stage_paths(install_dir: &Path) -> ApplyPaths {
    ApplyPaths {
        staged: install_dir.join("HiyoDyna.update.exe"),
        manifest: install_dir.join("HiyoDyna.update.txt"),
        installed: install_dir.join("HiyoDyna.exe"),
        state: state_path().unwrap_or_else(|| install_dir.join("update-state.ini")),
    }
}

fn state_path() -> Option<PathBuf> {
    std::env::var_os("LOCALAPPDATA").map(|base| {
        PathBuf::from(base)
            .join("HiyoDyna")
            .join("update-state.ini")
    })
}

fn read_manifest(path: &Path) -> Option<StageManifest> {
    parse_manifest(&fs::read_to_string(path).ok()?)
}

fn parse_manifest(text: &str) -> Option<StageManifest> {
    let mut tag = None;
    let mut published = None;
    let mut channel = None;
    let mut sha256 = None;
    for line in text.lines() {
        let Some((key, value)) = line.split_once('=') else {
            continue;
        };
        let value = value.trim().to_string();
        match key.trim() {
            "tag" => tag = Some(value),
            "published" => published = Some(value),
            "channel" => channel = UpdateChannel::parse(&value),
            "sha256" => sha256 = Some(value),
            _ => {}
        }
    }
    Some(StageManifest {
        tag: tag?,
        published: published?,
        channel: channel?,
        sha256: sha256?,
    })
}

fn write_manifest(path: &Path, manifest: &StageManifest) -> Result<(), String> {
    let text = format!(
        "tag={}\npublished={}\nchannel={}\nsha256={}\n",
        manifest.tag,
        manifest.published,
        manifest.channel.as_str(),
        manifest.sha256
    );
    fs::write(path, text).map_err(|error| format!("could not write {path:?}: {error}"))
}

fn parse_state(text: &str) -> UpdateState {
    let mut state = UpdateState::default();
    for line in text.lines() {
        let Some((key, value)) = line.split_once('=') else {
            continue;
        };
        let value = value.trim();
        if value.is_empty() {
            continue;
        }
        match key.trim() {
            "applied_tag" => state.applied_tag = Some(value.to_string()),
            "applied_published" => state.applied_published = Some(value.to_string()),
            _ => {}
        }
    }
    state
}

fn serialize_state(state: &UpdateState) -> String {
    let mut text = String::new();
    if let Some(tag) = &state.applied_tag {
        text.push_str(&format!("applied_tag={tag}\n"));
    }
    if let Some(published) = &state.applied_published {
        text.push_str(&format!("applied_published={published}\n"));
    }
    text
}

fn read_state_from(path: &Path) -> UpdateState {
    fs::read_to_string(path)
        .map(|text| parse_state(&text))
        .unwrap_or_default()
}

fn read_state() -> UpdateState {
    state_path()
        .map(|path| read_state_from(&path))
        .unwrap_or_default()
}

fn write_state(path: &Path, state: &UpdateState) {
    if let Some(parent) = path.parent() {
        let _ = fs::create_dir_all(parent);
    }
    let _ = fs::write(path, serialize_state(state));
}

fn repo_slug() -> &'static str {
    env!("CARGO_PKG_REPOSITORY")
        .strip_prefix("https://github.com/")
        .unwrap_or(env!("CARGO_PKG_REPOSITORY"))
        .trim_end_matches('/')
}

fn feed_url() -> String {
    format!("https://github.com/{}/releases.atom", repo_slug())
}

fn latest_release_url() -> String {
    format!(
        "https://api.github.com/repos/{}/releases/latest",
        repo_slug()
    )
}

fn parse_atom(text: &str) -> Vec<Release> {
    text.split("<entry>")
        .skip(1)
        .filter_map(|entry| {
            let tag = entry
                .split_once("/releases/tag/")
                .and_then(|(_, rest)| rest.split('"').next())
                .map(str::trim)
                .filter(|tag| !tag.is_empty())?;
            let published = between(entry, "<updated>", "</updated>")?;
            Some(Release {
                tag: tag.to_string(),
                published: published.to_string(),
            })
        })
        .collect()
}

fn parse_latest_json(text: &str) -> Result<Release, String> {
    let tag = json_string(text, "tag_name").ok_or("release JSON has no tag_name")?;
    let published = json_string(text, "published_at").ok_or("release JSON has no published_at")?;
    Ok(Release { tag, published })
}

fn select_from_atom(entries: &[Release], channel: UpdateChannel) -> Option<Release> {
    match channel {
        UpdateChannel::Build => entries
            .iter()
            .find(|entry| is_pre_release(&entry.tag))
            .cloned(),
        UpdateChannel::Stable => entries
            .iter()
            .find(|entry| entry.tag.starts_with('v'))
            .or_else(|| entries.iter().find(|entry| is_pre_release(&entry.tag)))
            .cloned(),
    }
}

fn stable_candidate(latest: Option<Release>, atom: &[Release]) -> Option<Release> {
    latest.or_else(|| select_from_atom(atom, UpdateChannel::Build))
}

fn parse_rfc3339(text: &str) -> Option<i64> {
    let (date, rest) = text.trim().split_once(['T', 't'])?;
    let (time, offset) = split_offset(rest)?;
    let (whole, fraction) = match time.split_once('.') {
        Some((whole, fraction)) => (whole, Some(fraction)),
        None => (time, None),
    };
    if let Some(fraction) = fraction
        && (fraction.is_empty() || !fraction.bytes().all(|byte| byte.is_ascii_digit()))
    {
        return None;
    }

    let mut date_parts = date.split('-');
    let year = number(date_parts.next()?)?;
    let month = number(date_parts.next()?)?;
    let day = number(date_parts.next()?)?;
    if date_parts.next().is_some() || !(1..=12).contains(&month) {
        return None;
    }
    if day < 1 || day > days_in_month(year, month) {
        return None;
    }

    let mut time_parts = whole.split(':');
    let hour = number(time_parts.next()?)?;
    let minute = number(time_parts.next()?)?;
    let second = number(time_parts.next()?)?;
    if time_parts.next().is_some() || hour > 23 || minute > 59 || second > 60 {
        return None;
    }

    let days = days_from_civil(year, month, day);
    Some(days * 86_400 + hour * 3_600 + minute * 60 + second - offset)
}

fn should_apply(
    release: &Release,
    applied_tag: Option<&str>,
    applied_time: Option<i64>,
    installed_time: Option<i64>,
    embedded_tag: Option<&str>,
) -> bool {
    if applied_tag == Some(release.tag.as_str()) || embedded_tag == Some(release.tag.as_str()) {
        return false;
    }
    let Some(published) = parse_rfc3339(&release.published) else {
        return false;
    };
    match applied_time.or(installed_time) {
        Some(installed) => published > installed,
        None => true,
    }
}

fn select_release(channel: UpdateChannel) -> Result<Option<Release>, String> {
    match channel {
        UpdateChannel::Build => Ok(select_from_atom(&fetch_atom()?, channel)),
        UpdateChannel::Stable => match crate::net::get(&latest_release_url(), 1024 * 1024) {
            Ok(bytes) => {
                let text = String::from_utf8(bytes)
                    .map_err(|error| format!("release JSON is not UTF-8: {error}"))?;
                Ok(Some(parse_latest_json(&text)?))
            }
            Err(error) if error.status == Some(404) => Ok(stable_candidate(None, &fetch_atom()?)),
            Err(error) => Err(error.to_string()),
        },
    }
}

fn fetch_atom() -> Result<Vec<Release>, String> {
    let bytes = crate::net::get(&feed_url(), 1024 * 1024).map_err(|error| error.to_string())?;
    let text =
        String::from_utf8(bytes).map_err(|error| format!("release feed is not UTF-8: {error}"))?;
    Ok(parse_atom(&text))
}

fn between<'a>(text: &'a str, start: &str, end: &str) -> Option<&'a str> {
    let (_, rest) = text.split_once(start)?;
    let (value, _) = rest.split_once(end)?;
    Some(value.trim())
}

fn json_string(text: &str, key: &str) -> Option<String> {
    let (_, rest) = text.split_once(&format!("\"{key}\""))?;
    let (_, rest) = rest.split_once(':')?;
    let rest = rest.trim_start().strip_prefix('"')?;
    let (value, _) = rest.split_once('"')?;
    Some(value.to_string())
}

fn is_pre_release(tag: &str) -> bool {
    tag.strip_prefix("pre-").is_some_and(|number| {
        !number.is_empty() && number.bytes().all(|byte| byte.is_ascii_digit())
    })
}

fn split_offset(text: &str) -> Option<(&str, i64)> {
    if let Some(time) = text.strip_suffix(['Z', 'z']) {
        return Some((time, 0));
    }
    let index = text.find(['+', '-'])?;
    let (time, zone) = text.split_at(index);
    let sign = if zone.starts_with('-') { -1 } else { 1 };
    let (hours, minutes) = zone[1..].split_once(':')?;
    let hours = number(hours)?;
    let minutes = number(minutes)?;
    if hours > 23 || minutes > 59 {
        return None;
    }
    Some((time, sign * (hours * 3_600 + minutes * 60)))
}

fn number(text: &str) -> Option<i64> {
    if text.is_empty() || !text.bytes().all(|byte| byte.is_ascii_digit()) {
        return None;
    }
    text.parse().ok()
}

fn days_in_month(year: i64, month: i64) -> i64 {
    match month {
        1 | 3 | 5 | 7 | 8 | 10 | 12 => 31,
        4 | 6 | 9 | 11 => 30,
        2 if year % 4 == 0 && (year % 100 != 0 || year % 400 == 0) => 29,
        2 => 28,
        _ => 0,
    }
}

fn days_from_civil(year: i64, month: i64, day: i64) -> i64 {
    let year = if month <= 2 { year - 1 } else { year };
    let era = if year >= 0 { year } else { year - 399 } / 400;
    let year_of_era = year - era * 400;
    let shifted_month = (month + 9) % 12;
    let day_of_year = (153 * shifted_month + 2) / 5 + day - 1;
    let day_of_era = year_of_era * 365 + year_of_era / 4 - year_of_era / 100 + day_of_year;
    era * 146_097 + day_of_era - 719_468
}

fn old_path(installed: &Path) -> PathBuf {
    let mut path = installed.as_os_str().to_os_string();
    path.push(".old");
    PathBuf::from(path)
}

fn apply_staged_update(
    paths: &ApplyPaths,
    current_channel: UpdateChannel,
    launch: &mut dyn FnMut(&Path) -> Result<(), String>,
) -> Result<ApplyOutcome, String> {
    let manifest = read_manifest(&paths.manifest).ok_or("staged update has no valid manifest")?;
    let actual = crate::sha256::to_hex(&crate::sha256::hash_file(&paths.staged)?);
    if !actual.eq_ignore_ascii_case(manifest.sha256.trim()) {
        discard_stage(paths);
        return Err("staged update failed its SHA-256 check".to_string());
    }

    if !stage_should_apply(paths, current_channel, &manifest) {
        discard_stage(paths);
        return Ok(ApplyOutcome::Discarded);
    }

    let old = old_path(&paths.installed);
    let _ = fs::remove_file(&old);
    if !paths.installed.exists() && old.exists() {
        fs::rename(&old, &paths.installed)
            .map_err(|error| format!("could not restore the previous exe: {error}"))?;
    }
    let had_installed = paths.installed.exists();
    if had_installed {
        fs::rename(&paths.installed, &old)
            .map_err(|error| format!("could not back up the installed exe: {error}"))?;
    }
    fs::rename(&paths.staged, &paths.installed).map_err(|error| {
        if had_installed {
            let _ = fs::rename(&old, &paths.installed);
        }
        format!("could not install the staged exe: {error}")
    })?;

    if let Err(error) = launch(&paths.installed) {
        let _ = fs::rename(&paths.installed, &paths.staged);
        if had_installed {
            fs::rename(&old, &paths.installed)
                .map_err(|restore| format!("could not restore the previous exe: {restore}"))?;
        }
        let _ = launch(&paths.installed);
        return Err(format!("could not launch the updated exe: {error}"));
    }

    write_state(
        &paths.state,
        &UpdateState {
            applied_tag: Some(manifest.tag.clone()),
            applied_published: Some(manifest.published.clone()),
        },
    );
    let _ = fs::remove_file(&paths.manifest);
    Ok(ApplyOutcome::Applied)
}

fn discard_stage(paths: &ApplyPaths) {
    let _ = fs::remove_file(&paths.staged);
    let _ = fs::remove_file(&paths.manifest);
}

fn stage_should_apply(
    paths: &ApplyPaths,
    channel: UpdateChannel,
    manifest: &StageManifest,
) -> bool {
    if manifest.channel != channel {
        return false;
    }
    let state = read_state_from(&paths.state);
    let applied_time = state.applied_published.as_deref().and_then(parse_rfc3339);
    let release = Release {
        tag: manifest.tag.clone(),
        published: manifest.published.clone(),
    };
    should_apply(
        &release,
        state.applied_tag.as_deref(),
        applied_time,
        installed_time_near(&paths.installed),
        option_env!("HIYODyna_BUILD_TAG"),
    )
}

fn installed_time_near(exe: &Path) -> Option<i64> {
    let directory = exe.parent()?;
    let install_json = fs::read_to_string(directory.join("install.json")).ok();
    installed_time_from(install_json.as_deref()).or_else(|| {
        fs::metadata(exe)
            .ok()
            .and_then(|metadata| metadata.modified().ok())
            .and_then(epoch_seconds)
    })
}

fn installed_time_from(install_json: Option<&str>) -> Option<i64> {
    let text = install_json?;
    parse_rfc3339(&json_string(text, "installedAt")?)
}

fn epoch_seconds(time: std::time::SystemTime) -> Option<i64> {
    time.duration_since(std::time::UNIX_EPOCH)
        .ok()
        .map(|elapsed| elapsed.as_secs() as i64)
}

fn stage_bytes(
    install_dir: &Path,
    bytes: &[u8],
    manifest: &StageManifest,
) -> Result<ApplyPaths, String> {
    if !install_dir.is_dir() {
        return Err(format!(
            "install directory {install_dir:?} does not exist or is not writable"
        ));
    }
    let actual = crate::sha256::to_hex(&crate::sha256::hash_bytes(bytes)?);
    if !actual.eq_ignore_ascii_case(manifest.sha256.trim()) {
        return Err(format!("SHA-256 mismatch for {}", manifest.tag));
    }

    let paths = stage_paths(install_dir);
    let part = install_dir.join("HiyoDyna.update.part");
    let result = (|| {
        let _ = fs::remove_file(&paths.staged);
        let _ = fs::remove_file(&paths.manifest);
        fs::write(&part, bytes).map_err(|error| format!("could not write {part:?}: {error}"))?;
        fs::rename(&part, &paths.staged)
            .map_err(|error| format!("could not stage the update: {error}"))?;
        write_manifest(&paths.manifest, manifest)
    })();
    if let Err(error) = result {
        let _ = fs::remove_file(&part);
        discard_stage(&paths);
        return Err(error);
    }
    Ok(paths)
}

fn download_and_stage(release: &Release, channel: UpdateChannel) -> Result<ApplyPaths, String> {
    let install_dir = std::env::current_exe()
        .map_err(|error| format!("could not locate the running exe: {error}"))?
        .parent()
        .ok_or("the running exe has no parent directory")?
        .to_path_buf();
    let base = format!(
        "https://github.com/{}/releases/download/{}",
        repo_slug(),
        release.tag
    );
    let hash = crate::net::get(&format!("{base}/HiyoDyna.exe.sha256"), 1024)
        .map_err(|error| format!("could not download the update hash: {error}"))?;
    let hash = String::from_utf8(hash)
        .map_err(|error| format!("the update hash is not UTF-8: {error}"))?;
    let sha256 = hash
        .split_whitespace()
        .next()
        .ok_or("the update hash is empty")?
        .to_string();
    if sha256.len() != 64 || !sha256.bytes().all(|byte| byte.is_ascii_hexdigit()) {
        return Err(format!("the update hash is invalid: {sha256}"));
    }

    let bytes = crate::net::get(&format!("{base}/HiyoDyna.exe"), 64 * 1024 * 1024)
        .map_err(|error| format!("could not download the update: {error}"))?;
    let manifest = StageManifest {
        tag: release.tag.clone(),
        published: release.published.clone(),
        channel,
        sha256,
    };
    stage_bytes(&install_dir, &bytes, &manifest)
}

pub(crate) const WM_APP_UPDATE_READY: u32 = WM_APP + 5;
pub(crate) const WM_APP_UPDATE_STATUS: u32 = WM_APP + 6;
pub(crate) const UPDATE_INTERVAL_MILLISECONDS: u32 = 30 * 60 * 1000;

static CHECK_IN_FLIGHT: AtomicBool = AtomicBool::new(false);
static UPDATE_READY: AtomicBool = AtomicBool::new(false);
static APP_EXITING: AtomicBool = AtomicBool::new(false);

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum UpdateResult {
    UpToDate = 0,
    NoRelease = 1,
    Failed = 2,
}

enum CheckOutcome {
    Ready,
    Status(UpdateResult),
}

#[derive(Debug, PartialEq)]
enum PendingAction {
    None,
    Discard,
    Apply,
}

pub(crate) fn request_check(hwnd: HWND, channel: UpdateChannel, manual: bool) {
    if CHECK_IN_FLIGHT.swap(true, Ordering::SeqCst) {
        return;
    }
    let hwnd_value = hwnd as isize;
    let spawned = std::thread::Builder::new()
        .name("hiyodyna-update-check".into())
        .spawn(move || {
            let outcome = perform_check(channel);
            match outcome {
                CheckOutcome::Ready => {
                    signal_ready();
                    unsafe {
                        PostMessageW(hwnd_value as HWND, WM_APP_UPDATE_READY, 0, 0);
                    }
                }
                CheckOutcome::Status(result) if manual => unsafe {
                    PostMessageW(hwnd_value as HWND, WM_APP_UPDATE_STATUS, 1, result as isize);
                },
                CheckOutcome::Status(_) => {}
            }
            CHECK_IN_FLIGHT.store(false, Ordering::SeqCst);
        });
    if spawned.is_err() {
        log("could not start the update check thread");
        CHECK_IN_FLIGHT.store(false, Ordering::SeqCst);
    }
}

pub(crate) fn take_ready() -> bool {
    UPDATE_READY.swap(false, Ordering::SeqCst)
}

pub(crate) fn mark_app_exiting() {
    APP_EXITING.store(true, Ordering::SeqCst);
}

pub(crate) fn discard_staged_for(channel: UpdateChannel) {
    if pending_action(channel) == PendingAction::Discard
        && let Some(install_dir) = install_dir()
    {
        discard_stage(&stage_paths(&install_dir));
    }
}

pub(crate) fn is_apply_helper_invocation() -> bool {
    std::env::args_os()
        .nth(1)
        .is_some_and(|argument| argument.to_string_lossy() == "--apply-update")
}

pub(crate) fn run_apply_helper() {
    let Some(pid) = std::env::args_os()
        .nth(2)
        .and_then(|argument| argument.to_string_lossy().parse::<u32>().ok())
    else {
        log("the update helper was started without a process id");
        return;
    };
    let handle = unsafe { OpenProcess(PROCESS_SYNCHRONIZE, 0, pid) };
    if handle.is_null() {
        let error = unsafe { GetLastError() };
        if error != ERROR_INVALID_PARAMETER {
            log(&format!(
                "could not open the previous process {pid} (Windows error {error})"
            ));
            return;
        }
        log(&format!("the previous process {pid} already exited"));
    } else {
        let wait = unsafe { WaitForSingleObject(handle, 30_000) };
        unsafe { CloseHandle(handle) };
        if wait != WAIT_OBJECT_0 {
            log("timed out waiting for HiyoDyna to exit");
            return;
        }
    }

    let Some(install_dir) = install_dir() else {
        log("the update helper could not locate its install directory");
        return;
    };
    let channel = Settings::load().update;
    let paths = stage_paths(&install_dir);
    let mut launch = |path: &Path| -> Result<(), String> {
        Command::new(path)
            .spawn()
            .map(|_| ())
            .map_err(|error| format!("{error}"))
    };
    match apply_staged_update(&paths, channel, &mut launch) {
        Ok(ApplyOutcome::Applied) => log("applied the staged update"),
        Ok(ApplyOutcome::Discarded) => {
            log("discarded the staged update");
            launch_installed(&paths.installed);
        }
        Err(error) => {
            log(&format!("could not apply the staged update: {error}"));
            launch_installed(&paths.installed);
        }
    }
}

pub(crate) fn apply_pending_update() -> bool {
    let Some(exe) = std::env::current_exe().ok() else {
        return false;
    };
    if is_dev_build_path(&exe) {
        return false;
    }
    let channel = Settings::load().update;
    match pending_action(channel) {
        PendingAction::None => false,
        PendingAction::Discard => {
            discard_staged_for(channel);
            false
        }
        PendingAction::Apply => {
            match Command::new(&exe)
                .arg("--apply-update")
                .arg(std::process::id().to_string())
                .spawn()
            {
                Ok(_) => true,
                Err(error) => {
                    log(&format!(
                        "could not start the update helper at startup: {error}"
                    ));
                    false
                }
            }
        }
    }
}

pub(crate) fn cleanup_old_binary() {
    let Some(exe) = std::env::current_exe().ok() else {
        return;
    };
    let _ = fs::remove_file(old_path(&exe));
    let Some(install_dir) = exe.parent() else {
        return;
    };
    let paths = stage_paths(install_dir);
    let _ = fs::remove_file(install_dir.join("HiyoDyna.update.part"));
    if !paths.staged.exists() {
        let _ = fs::remove_file(&paths.manifest);
    }
}

pub(crate) fn applied_tag() -> Option<String> {
    read_state()
        .applied_tag
        .or_else(|| option_env!("HIYODyna_BUILD_TAG").map(std::string::ToString::to_string))
}

pub(crate) fn log_path() -> String {
    std::env::var_os("LOCALAPPDATA")
        .map(|base| {
            PathBuf::from(base)
                .join("HiyoDyna")
                .join("auto-update.log")
                .display()
                .to_string()
        })
        .unwrap_or_else(|| "auto-update.log".to_string())
}

pub(crate) fn log(message: &str) {
    let path = log_path();
    let path = Path::new(&path);
    if let Some(parent) = path.parent() {
        let _ = fs::create_dir_all(parent);
    }
    if fs::metadata(path).is_ok_and(|metadata| metadata.len() > 1024 * 1024) {
        let old = path.with_extension("log.old");
        let _ = fs::remove_file(&old);
        let _ = fs::rename(path, old);
    }
    let line = format!("[{}] {message}\n", timestamp());
    if let Ok(mut file) = fs::OpenOptions::new().create(true).append(true).open(path) {
        let _ = file.write_all(line.as_bytes());
    }
}

fn perform_check(channel: UpdateChannel) -> CheckOutcome {
    let exe = match std::env::current_exe() {
        Ok(exe) => exe,
        Err(error) => {
            log(&format!("could not locate the running exe: {error}"));
            return CheckOutcome::Status(UpdateResult::Failed);
        }
    };
    if is_dev_build_path(&exe) {
        return CheckOutcome::Status(UpdateResult::UpToDate);
    }

    let release = match select_release(channel) {
        Ok(Some(release)) => release,
        Ok(None) => {
            log(&format!("no {} release is available", channel.label()));
            return CheckOutcome::Status(UpdateResult::NoRelease);
        }
        Err(error) => {
            log(&format!("the {} check failed: {error}", channel.label()));
            return CheckOutcome::Status(UpdateResult::Failed);
        }
    };

    let state = read_state();
    let applied_time = state.applied_published.as_deref().and_then(parse_rfc3339);
    if !should_apply(
        &release,
        state.applied_tag.as_deref(),
        applied_time,
        installed_time_near(&exe),
        option_env!("HIYODyna_BUILD_TAG"),
    ) {
        log(&format!(
            "{} is up to date ({})",
            channel.label(),
            release.tag
        ));
        return CheckOutcome::Status(UpdateResult::UpToDate);
    }

    log(&format!(
        "downloading {} from the {} channel",
        release.tag,
        channel.label()
    ));
    let paths = match download_and_stage(&release, channel) {
        Ok(paths) => paths,
        Err(error) => {
            log(&format!("could not download {}: {error}", release.tag));
            return CheckOutcome::Status(UpdateResult::Failed);
        }
    };
    if Settings::load().update != channel {
        discard_stage(&paths);
        log("discarded the staged update because the channel changed");
        return CheckOutcome::Status(UpdateResult::UpToDate);
    }
    if APP_EXITING.load(Ordering::SeqCst) {
        discard_stage(&paths);
        log("discarded the staged update because HiyoDyna is exiting");
        return CheckOutcome::Status(UpdateResult::UpToDate);
    }

    match Command::new(&exe)
        .arg("--apply-update")
        .arg(std::process::id().to_string())
        .spawn()
    {
        Ok(_) => {
            log(&format!(
                "{} is staged; restarting to apply it",
                release.tag
            ));
            CheckOutcome::Ready
        }
        Err(error) => {
            log(&format!("could not start the update helper: {error}"));
            CheckOutcome::Status(UpdateResult::Failed)
        }
    }
}

fn pending_action(channel: UpdateChannel) -> PendingAction {
    let Some(install_dir) = install_dir() else {
        return PendingAction::None;
    };
    let paths = stage_paths(&install_dir);
    let stage = read_manifest(&paths.manifest);
    let action = pending_action_from(paths.staged.is_file(), stage.as_ref(), channel);
    if action == PendingAction::Apply
        && let Some(manifest) = stage.as_ref()
        && !stage_should_apply(&paths, channel, manifest)
    {
        return PendingAction::Discard;
    }
    action
}

fn pending_action_from(
    staged_exists: bool,
    stage: Option<&StageManifest>,
    channel: UpdateChannel,
) -> PendingAction {
    if !staged_exists {
        return PendingAction::None;
    }
    match stage {
        Some(stage) if stage.channel == channel => PendingAction::Apply,
        _ => PendingAction::Discard,
    }
}

fn install_dir() -> Option<PathBuf> {
    std::env::current_exe()
        .ok()?
        .parent()
        .map(Path::to_path_buf)
}

fn is_dev_build_path(path: &Path) -> bool {
    let mut components = path.components().peekable();
    while let Some(component) = components.next() {
        if component
            .as_os_str()
            .to_string_lossy()
            .eq_ignore_ascii_case("target")
            && let Some(next) = components.peek()
        {
            let next = next.as_os_str().to_string_lossy();
            if next.eq_ignore_ascii_case("debug") || next.eq_ignore_ascii_case("release") {
                return true;
            }
        }
    }
    false
}

fn launch_installed(path: &Path) {
    if path.is_file()
        && let Err(error) = Command::new(path).spawn()
    {
        log(&format!("could not relaunch HiyoDyna: {error}"));
    }
}

fn signal_ready() {
    UPDATE_READY.store(true, Ordering::SeqCst);
}

fn timestamp() -> String {
    let mut time = SYSTEMTIME::default();
    unsafe { GetLocalTime(&mut time) };
    format!(
        "{:04}-{:02}-{:02} {:02}:{:02}:{:02}",
        time.wYear, time.wMonth, time.wDay, time.wHour, time.wMinute, time.wSecond
    )
}

#[cfg(test)]
mod tests {
    use crate::settings::UpdateChannel;

    const ATOM: &str = r#"<feed>
<entry><updated>2026-11-19T08:07:44Z</updated>
<link rel="alternate" href="https://github.com/hiyo41/HiyoDyna/releases/tag/pre-2"/></entry>
<entry><updated>2026-11-18T08:07:44Z</updated>
<link rel="alternate" href="https://github.com/hiyo41/HiyoDyna/releases/tag/pre-1"/></entry>
</feed>"#;

    fn unique_temp_dir(label: &str) -> std::path::PathBuf {
        let dir = std::env::temp_dir().join(format!("hiyodyna-{label}-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).unwrap();
        dir
    }

    fn test_stage(
        dir: &std::path::Path,
        installed: &[u8],
        staged: &[u8],
        channel: UpdateChannel,
        tag: &str,
    ) -> super::ApplyPaths {
        let paths = super::ApplyPaths {
            staged: dir.join("HiyoDyna.update.exe"),
            manifest: dir.join("HiyoDyna.update.txt"),
            installed: dir.join("installed.exe"),
            state: dir.join("update-state.ini"),
        };
        std::fs::write(&paths.installed, installed).unwrap();
        std::fs::write(&paths.staged, staged).unwrap();
        let manifest = super::StageManifest {
            tag: tag.into(),
            published: "2099-01-01T00:00:00Z".into(),
            channel,
            sha256: crate::sha256::to_hex(&crate::sha256::hash_bytes(staged).unwrap()),
        };
        super::write_manifest(&paths.manifest, &manifest).unwrap();
        paths
    }

    #[test]
    fn apply_stages_successfully() {
        let dir = unique_temp_dir("apply-ok");
        let paths = test_stage(&dir, b"old", b"new", UpdateChannel::Build, "pre-2");
        let mut calls = Vec::new();
        let outcome = super::apply_staged_update(&paths, UpdateChannel::Build, &mut |path| {
            calls.push(path.to_path_buf());
            Ok(())
        })
        .unwrap();
        assert_eq!(outcome, super::ApplyOutcome::Applied);
        assert_eq!(std::fs::read(&paths.installed).unwrap(), b"new");
        assert_eq!(
            std::fs::read(format!("{}.old", paths.installed.display())).unwrap(),
            b"old"
        );
        assert!(!paths.staged.exists());
        assert!(!paths.manifest.exists());
        assert!(
            std::fs::read_to_string(&paths.state)
                .unwrap()
                .contains("applied_tag=pre-2")
        );
        assert_eq!(calls, vec![paths.installed.clone()]);
        let _ = std::fs::remove_dir_all(dir);
    }

    #[test]
    fn discards_a_staged_update_from_another_channel() {
        let dir = unique_temp_dir("apply-channel");
        let paths = test_stage(&dir, b"old", b"new", UpdateChannel::Stable, "pre-2");
        let mut calls = 0;
        let outcome = super::apply_staged_update(&paths, UpdateChannel::Build, &mut |_| {
            calls += 1;
            Ok(())
        })
        .unwrap();
        assert_eq!(outcome, super::ApplyOutcome::Discarded);
        assert_eq!(calls, 0);
        assert_eq!(std::fs::read(&paths.installed).unwrap(), b"old");
        assert!(!paths.staged.exists());
        assert!(!paths.manifest.exists());
        let _ = std::fs::remove_dir_all(dir);
    }

    #[test]
    fn restores_the_previous_exe_when_launch_fails() {
        let dir = unique_temp_dir("apply-rollback");
        let paths = test_stage(&dir, b"old", b"new", UpdateChannel::Build, "pre-2");
        let mut calls = Vec::new();
        let result = super::apply_staged_update(&paths, UpdateChannel::Build, &mut |path| {
            calls.push(path.to_path_buf());
            if calls.len() == 1 {
                Err("boom".into())
            } else {
                Ok(())
            }
        });
        assert!(result.is_err());
        assert_eq!(std::fs::read(&paths.installed).unwrap(), b"old");
        assert_eq!(calls.len(), 2);
        assert_eq!(calls[0], paths.installed);
        assert_eq!(calls[1], paths.installed);
        assert!(!paths.state.exists());
        let _ = std::fs::remove_dir_all(dir);
    }

    #[test]
    fn rejects_a_staged_update_with_a_bad_hash() {
        let dir = unique_temp_dir("apply-bad-hash");
        let paths = test_stage(&dir, b"old", b"new", UpdateChannel::Build, "pre-2");
        std::fs::write(&paths.staged, b"tampered").unwrap();
        let mut calls = 0;
        let result = super::apply_staged_update(&paths, UpdateChannel::Build, &mut |_| {
            calls += 1;
            Ok(())
        });
        assert!(result.is_err());
        assert_eq!(calls, 0);
        assert_eq!(std::fs::read(&paths.installed).unwrap(), b"old");
        assert!(!paths.staged.exists());
        assert!(!paths.manifest.exists());
        let _ = std::fs::remove_dir_all(dir);
    }

    #[test]
    fn stage_bytes_writes_a_verified_update() {
        let dir = unique_temp_dir("stage-ok");
        let manifest = super::StageManifest {
            tag: "pre-2".into(),
            published: "2026-11-19T08:07:44Z".into(),
            channel: UpdateChannel::Build,
            sha256: crate::sha256::to_hex(&crate::sha256::hash_bytes(b"new").unwrap()),
        };
        let paths = super::stage_bytes(&dir, b"new", &manifest).unwrap();
        assert_eq!(std::fs::read(&paths.staged).unwrap(), b"new");
        assert!(paths.manifest.exists());
        let _ = std::fs::remove_dir_all(dir);
    }

    #[test]
    fn stage_bytes_rejects_a_hash_mismatch_and_missing_directory() {
        let dir = unique_temp_dir("stage-bad");
        let bad = super::StageManifest {
            tag: "pre-2".into(),
            published: "2026-11-19T08:07:44Z".into(),
            channel: UpdateChannel::Build,
            sha256: "00".repeat(32),
        };
        assert!(super::stage_bytes(&dir, b"new", &bad).is_err());
        assert!(!dir.join("HiyoDyna.update.exe").exists());
        assert!(!dir.join("HiyoDyna.update.part").exists());

        let good = super::StageManifest {
            sha256: crate::sha256::to_hex(&crate::sha256::hash_bytes(b"new").unwrap()),
            ..bad
        };
        assert!(super::stage_bytes(&dir.join("missing"), b"new", &good).is_err());
        let _ = std::fs::remove_dir_all(dir);
    }

    #[test]
    fn state_and_manifest_round_trip() {
        let state = super::UpdateState {
            applied_tag: Some("pre-2".into()),
            applied_published: Some("2026-11-19T08:07:44Z".into()),
        };
        assert_eq!(super::parse_state(&super::serialize_state(&state)), state);

        let manifest = super::StageManifest {
            tag: "pre-2".into(),
            published: "2026-11-19T08:07:44Z".into(),
            channel: UpdateChannel::Build,
            sha256: "abc".into(),
        };
        let path =
            std::env::temp_dir().join(format!("hiyodyna-manifest-{}.txt", std::process::id()));
        super::write_manifest(&path, &manifest).unwrap();
        let parsed = super::read_manifest(&path).unwrap();
        assert_eq!(parsed.tag, "pre-2");
        assert_eq!(parsed.channel, UpdateChannel::Build);
        let _ = std::fs::remove_file(path);
    }

    #[test]
    fn reads_installed_at_from_install_json() {
        let text = r#"{"installDir":"C:\\x","installedAt":"2026-11-19T15:26:51.0835595+07:00"}"#;
        assert_eq!(
            super::installed_time_from(Some(text)),
            super::parse_rfc3339("2026-11-19T08:26:51Z")
        );
        assert_eq!(super::installed_time_from(None), None);
    }

    #[test]
    fn parses_atom_entries() {
        let entries = super::parse_atom(ATOM);
        assert_eq!(entries[0].tag, "pre-2");
        assert_eq!(entries[0].published, "2026-11-19T08:07:44Z");
        assert_eq!(entries[1].tag, "pre-1");
    }

    #[test]
    fn parses_latest_release_json() {
        let release = super::parse_latest_json(
            r#"{"tag_name":"v0.0.1-3","published_at":"2026-11-19T09:00:00Z"}"#,
        )
        .unwrap();
        assert_eq!(release.tag, "v0.0.1-3");
        assert_eq!(release.published, "2026-11-19T09:00:00Z");
    }

    #[test]
    fn build_takes_pre_release_and_stable_takes_official() {
        let entries = super::parse_atom(ATOM);
        assert_eq!(
            super::select_from_atom(&entries, UpdateChannel::Build)
                .unwrap()
                .tag,
            "pre-2"
        );
        let official = super::parse_atom(
            r#"<entry><updated>2026-11-19T10:00:00Z</updated>
        <link href="https://github.com/hiyo41/HiyoDyna/releases/tag/v0.0.1-3"/></entry>"#,
        );
        assert_eq!(
            super::select_from_atom(&official, UpdateChannel::Stable)
                .unwrap()
                .tag,
            "v0.0.1-3"
        );
        assert_eq!(
            super::select_from_atom(&entries, UpdateChannel::Stable)
                .unwrap()
                .tag,
            "pre-2"
        );
    }

    #[test]
    fn parses_rfc3339_offsets_and_epochs() {
        assert_eq!(super::parse_rfc3339("1970-01-01T00:00:00Z"), Some(0));
        assert_eq!(
            super::parse_rfc3339("2000-01-01T00:00:00Z"),
            Some(946684800)
        );
        assert_eq!(
            super::parse_rfc3339("2026-11-19T15:07:44+07:00"),
            super::parse_rfc3339("2026-11-19T08:07:44Z")
        );
        assert_eq!(
            super::parse_rfc3339("2026-11-19T15:26:51.0835595+07:00"),
            super::parse_rfc3339("2026-11-19T08:26:51Z")
        );
    }

    #[test]
    fn refuses_same_or_older_releases() {
        let release = super::Release {
            tag: "pre-3".into(),
            published: "2026-11-19T10:00:00Z".into(),
        };
        let installed_before = super::parse_rfc3339("2026-11-19T09:00:00Z");
        let installed_after = super::parse_rfc3339("2026-11-19T11:00:00Z");
        assert!(!super::should_apply(
            &release,
            Some("pre-3"),
            None,
            None,
            None
        ));
        assert!(!super::should_apply(
            &release,
            None,
            None,
            installed_after,
            None
        ));
        assert!(super::should_apply(
            &release,
            None,
            None,
            installed_before,
            None
        ));
        assert!(!super::should_apply(
            &release,
            None,
            None,
            None,
            Some("pre-3")
        ));
    }

    #[test]
    fn stable_uses_official_then_falls_back() {
        let atom = super::parse_atom(ATOM);
        let official = super::Release {
            tag: "v0.0.1-3".into(),
            published: "2026-11-19T10:00:00Z".into(),
        };
        assert_eq!(
            super::stable_candidate(Some(official), &atom).unwrap().tag,
            "v0.0.1-3"
        );
        assert_eq!(super::stable_candidate(None, &atom).unwrap().tag, "pre-2");
    }

    #[test]
    fn rejects_cargo_target_executables() {
        assert!(super::is_dev_build_path(std::path::Path::new(
            r"C:\work\HiyoDyna\target\release\hiyodyna.exe"
        )));
        assert!(super::is_dev_build_path(std::path::Path::new(
            r"C:\work\HiyoDyna\target\debug\deps\test.exe"
        )));
        assert!(!super::is_dev_build_path(std::path::Path::new(
            r"C:\Users\me\AppData\Local\Programs\HiyoDyna\HiyoDyna.exe"
        )));
    }

    #[test]
    fn pending_action_handles_missing_mismatch_and_valid_stage() {
        let build = super::StageManifest {
            tag: "pre-2".into(),
            published: "2026-11-19T08:07:44Z".into(),
            channel: UpdateChannel::Build,
            sha256: "abc".into(),
        };
        let stable = super::StageManifest {
            channel: UpdateChannel::Stable,
            ..build.clone()
        };
        assert_eq!(
            super::pending_action_from(false, None, UpdateChannel::Build),
            super::PendingAction::None
        );
        assert_eq!(
            super::pending_action_from(true, None, UpdateChannel::Build),
            super::PendingAction::Discard
        );
        assert_eq!(
            super::pending_action_from(true, Some(&stable), UpdateChannel::Build),
            super::PendingAction::Discard
        );
        assert_eq!(
            super::pending_action_from(true, Some(&build), UpdateChannel::Build),
            super::PendingAction::Apply
        );
    }

    #[test]
    fn ready_flag_is_taken_once() {
        super::signal_ready();
        assert!(super::take_ready());
        assert!(!super::take_ready());
    }
}
