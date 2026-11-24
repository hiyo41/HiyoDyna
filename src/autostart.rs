use std::path::{Path, PathBuf};
use std::ptr::null_mut;

use windows_sys::Win32::Foundation::{ERROR_FILE_NOT_FOUND, ERROR_MORE_DATA};
use windows_sys::Win32::System::Registry::{
    HKEY_CURRENT_USER, REG_SZ, RRF_RT_REG_SZ, RegDeleteKeyValueW, RegGetValueW, RegSetKeyValueW,
};

const RUN_KEY: &str = "Software\\Microsoft\\Windows\\CurrentVersion\\Run";
const VALUE_NAME: &str = "HiyoDyna";
const ERROR_SUCCESS: u32 = 0;

pub(crate) fn normalize_path(value: &str) -> String {
    let trimmed = value.trim();
    let unquoted = trimmed
        .strip_prefix('"')
        .and_then(|value| value.strip_suffix('"'))
        .unwrap_or(trimmed);
    unquoted.replace('/', "\\").to_ascii_lowercase()
}

pub(crate) fn matches_target(value: &str, target: &Path) -> bool {
    normalize_path(value) == normalize_path(&target.to_string_lossy())
}

pub(crate) fn autostart_target() -> Option<PathBuf> {
    let current = std::env::current_exe().ok()?;
    Some(choose_target(installed_exe(), &current))
}

pub(crate) fn is_enabled() -> bool {
    let Some(target) = autostart_target() else {
        return false;
    };
    read_value().is_some_and(|value| matches_target(&value, &target))
}

pub(crate) fn set_enabled(enabled: bool) -> Result<(), String> {
    if enabled {
        let target = autostart_target().ok_or("Could not resolve the HiyoDyna executable")?;
        write_value(&format_value(&target))
    } else {
        delete_value()
    }
}

pub(crate) fn toggle() -> Result<bool, String> {
    let enabled = !is_enabled();
    set_enabled(enabled)?;
    Ok(enabled)
}

fn choose_target(installed: Option<PathBuf>, current: &Path) -> PathBuf {
    installed.unwrap_or_else(|| current.to_path_buf())
}

fn installed_exe() -> Option<PathBuf> {
    let path = PathBuf::from(std::env::var_os("LOCALAPPDATA")?)
        .join("Programs")
        .join("HiyoDyna")
        .join("HiyoDyna.exe");
    path.is_file().then_some(path)
}

fn format_value(target: &Path) -> String {
    format!("\"{}\"", target.display())
}

fn read_value() -> Option<String> {
    let subkey = wide(RUN_KEY);
    let name = wide(VALUE_NAME);
    let mut buffer: Box<[u16]> = vec![0; 1024].into_boxed_slice();
    let mut size = (buffer.len() * 2) as u32;
    // SAFETY: The pointers are valid for the duration of the call and the buffer is writable.
    let mut status = unsafe {
        RegGetValueW(
            HKEY_CURRENT_USER,
            subkey.as_ptr(),
            name.as_ptr(),
            RRF_RT_REG_SZ,
            null_mut(),
            buffer.as_mut_ptr().cast(),
            &mut size,
        )
    };
    if status == ERROR_MORE_DATA {
        // The existing value is longer than the default buffer (a long
        // install path); retry with the size the registry reported instead of
        // reporting autostart as off.
        let needed = size as usize / 2 + 1;
        buffer = vec![0; needed].into_boxed_slice();
        size = (buffer.len() * 2) as u32;
        // SAFETY: The pointers are valid for the duration of the call and the buffer is writable.
        status = unsafe {
            RegGetValueW(
                HKEY_CURRENT_USER,
                subkey.as_ptr(),
                name.as_ptr(),
                RRF_RT_REG_SZ,
                null_mut(),
                buffer.as_mut_ptr().cast(),
                &mut size,
            )
        };
    }
    if status != ERROR_SUCCESS {
        return None;
    }
    let characters = (size as usize / 2).min(buffer.len());
    let end = buffer[..characters]
        .iter()
        .position(|&value| value == 0)
        .unwrap_or(characters);
    Some(String::from_utf16_lossy(&buffer[..end]))
}

fn write_value(value: &str) -> Result<(), String> {
    let subkey = wide(RUN_KEY);
    let name = wide(VALUE_NAME);
    let data = wide(value);
    let bytes = (data.len() * 2) as u32;
    // SAFETY: The pointers are valid for the duration of the call.
    let status = unsafe {
        RegSetKeyValueW(
            HKEY_CURRENT_USER,
            subkey.as_ptr(),
            name.as_ptr(),
            REG_SZ,
            data.as_ptr().cast(),
            bytes,
        )
    };
    if status == ERROR_SUCCESS {
        Ok(())
    } else {
        Err(format!("RegSetKeyValueW failed (Windows error {status})"))
    }
}

fn delete_value() -> Result<(), String> {
    let subkey = wide(RUN_KEY);
    let name = wide(VALUE_NAME);
    // SAFETY: The pointers are valid for the duration of the call.
    let status = unsafe { RegDeleteKeyValueW(HKEY_CURRENT_USER, subkey.as_ptr(), name.as_ptr()) };
    if status == ERROR_SUCCESS || status == ERROR_FILE_NOT_FOUND {
        Ok(())
    } else {
        Err(format!(
            "RegDeleteKeyValueW failed (Windows error {status})"
        ))
    }
}

fn wide(text: &str) -> Vec<u16> {
    text.encode_utf16().chain(Some(0)).collect()
}

#[cfg(test)]
mod tests {
    use super::{choose_target, format_value, matches_target, normalize_path};
    use std::path::{Path, PathBuf};

    #[test]
    fn normalizes_windows_paths() {
        assert_eq!(
            normalize_path("\"C:/Program Files/HiyoDyna/HiyoDyna.exe\""),
            "c:\\program files\\hiyodyna\\hiyodyna.exe"
        );
    }

    #[test]
    fn matches_only_the_target_path() {
        let target = Path::new("C:\\Apps\\HiyoDyna.exe");
        assert!(matches_target("\"c:/apps/hiyodyna.exe\"", target));
        assert!(!matches_target("C:\\Other\\HiyoDyna.exe", target));
    }

    #[test]
    fn prefers_the_installed_executable() {
        let installed =
            PathBuf::from("C:\\Users\\me\\AppData\\Local\\Programs\\HiyoDyna\\HiyoDyna.exe");
        let current = Path::new("C:\\Repo\\target\\release\\hiyodyna.exe");
        assert_eq!(choose_target(Some(installed.clone()), current), installed);
        assert_eq!(choose_target(None, current), current);
    }

    #[test]
    fn quotes_the_run_value() {
        assert_eq!(
            format_value(Path::new("C:\\Apps\\Hiyo Dyna.exe")),
            "\"C:\\Apps\\Hiyo Dyna.exe\""
        );
    }
}
