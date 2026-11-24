//! Display refresh detection and frame pacing.

pub(crate) const FALLBACK_REFRESH_HZ: u32 = 60;
pub(crate) const MIN_REFRESH_HZ: u32 = 24;
pub(crate) const MAX_REFRESH_HZ: u32 = 500;

pub(crate) fn normalize_refresh_hz(hz: u32) -> u32 {
    if (MIN_REFRESH_HZ..=MAX_REFRESH_HZ).contains(&hz) {
        hz
    } else {
        FALLBACK_REFRESH_HZ
    }
}

/// A timer interval that is never slower than the display period.
///
/// The timer may tick slightly faster than the panel. [`wait_for_vblank`]
/// aligns presentation, so the faster timer only prevents missed refreshes.
pub(crate) fn timer_interval_millis(hz: u32) -> u32 {
    let interval = (1000 / normalize_refresh_hz(hz)).max(1);
    // Multimedia timer wakes can be about a millisecond late. Fast panels get
    // a small lead so a delayed tick cannot land after the compositor vblank.
    if (4..=13).contains(&interval) {
        interval - 1
    } else {
        interval
    }
}

#[cfg(windows)]
pub(crate) fn refresh_hz(hwnd: windows_sys::Win32::Foundation::HWND) -> u32 {
    monitor_refresh_hz(hwnd)
        .filter(|hz| (MIN_REFRESH_HZ..=MAX_REFRESH_HZ).contains(hz))
        .or_else(composition_refresh_hz)
        .filter(|hz| (MIN_REFRESH_HZ..=MAX_REFRESH_HZ).contains(hz))
        .unwrap_or(FALLBACK_REFRESH_HZ)
}

#[cfg(windows)]
fn monitor_refresh_hz(hwnd: windows_sys::Win32::Foundation::HWND) -> Option<u32> {
    use std::mem::size_of;
    use windows_sys::Win32::Graphics::Gdi::{
        DEVMODEW, ENUM_CURRENT_SETTINGS, EnumDisplaySettingsW, GetMonitorInfoW,
        MONITOR_DEFAULTTONEAREST, MONITORINFO, MONITORINFOEXW, MonitorFromWindow,
    };

    // NEAREST keeps the pace tied to the panel the overlay is actually on;
    // PRIMARY would silently report the wrong rate on a secondary display.
    let monitor = unsafe { MonitorFromWindow(hwnd, MONITOR_DEFAULTTONEAREST) };
    if monitor.is_null() {
        return None;
    }
    let mut monitor_info = MONITORINFOEXW::default();
    monitor_info.monitorInfo.cbSize = size_of::<MONITORINFOEXW>() as u32;
    let ok = unsafe {
        GetMonitorInfoW(
            monitor,
            &mut monitor_info as *mut MONITORINFOEXW as *mut MONITORINFO,
        )
    };
    if ok == 0 {
        return None;
    }

    let mut mode = DEVMODEW {
        dmSize: size_of::<DEVMODEW>() as u16,
        ..DEVMODEW::default()
    };
    let ok = unsafe {
        EnumDisplaySettingsW(
            monitor_info.szDevice.as_ptr(),
            ENUM_CURRENT_SETTINGS,
            &mut mode,
        )
    };
    (ok != 0).then_some(mode.dmDisplayFrequency)
}

#[cfg(windows)]
fn composition_refresh_hz() -> Option<u32> {
    use std::mem::size_of;
    use windows_sys::Win32::Graphics::Dwm::{DWM_TIMING_INFO, DwmGetCompositionTimingInfo};

    let mut timing: DWM_TIMING_INFO = unsafe { std::mem::zeroed() };
    timing.cbSize = size_of::<DWM_TIMING_INFO>() as u32;
    let result = unsafe { DwmGetCompositionTimingInfo(std::ptr::null_mut(), &mut timing) };
    if result < 0 {
        return None;
    }

    let numerator = timing.rateRefresh.uiNumerator;
    let denominator = timing.rateRefresh.uiDenominator;
    (numerator > 0 && denominator > 0).then(|| numerator / denominator)
}

/// Waits for the compositor to present the queued frame.
///
/// This blocks for at most one display period and must only run on the
/// animation path. It keeps the frame cadence tied to the panel instead of
/// free-running at the timer rate.
#[cfg(windows)]
pub(crate) fn wait_for_vblank() {
    let _ = unsafe { windows_sys::Win32::Graphics::Dwm::DwmFlush() };
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn timer_intervals_track_common_refresh_rates() {
        for (hz, interval) in [
            (24, 41),
            (30, 33),
            (60, 16),
            (75, 12),
            (120, 7),
            (144, 5),
            (165, 5),
            (240, 3),
            (360, 2),
        ] {
            assert_eq!(timer_interval_millis(hz), interval, "{hz} Hz");
        }
    }

    #[test]
    fn invalid_refresh_rates_use_the_sixty_hertz_fallback() {
        for hz in [0, 1, 23, 501, 1_000] {
            assert_eq!(normalize_refresh_hz(hz), 60, "{hz} Hz");
            assert_eq!(timer_interval_millis(hz), 16, "{hz} Hz");
        }
    }

    #[cfg(windows)]
    #[test]
    fn windows_refresh_detection_returns_a_sane_rate() {
        let hz = refresh_hz(std::ptr::null_mut());
        assert!(
            (MIN_REFRESH_HZ..=MAX_REFRESH_HZ).contains(&hz),
            "detected {hz} Hz"
        );
    }
}
