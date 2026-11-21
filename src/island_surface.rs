//! Shared work-area geometry for Island and native control-bearing surfaces.

use std::cell::Cell;
use std::ptr::null_mut;
use windows_sys::Win32::Foundation::{HWND, RECT};
use windows_sys::Win32::Graphics::Gdi::{
    GetMonitorInfoW, MONITOR_DEFAULTTONEAREST, MONITORINFO, MonitorFromRect,
};
use windows_sys::Win32::UI::WindowsAndMessaging::*;

thread_local! {
    static BODY: Cell<Option<(HWND, RECT)>> = const { Cell::new(None) };
}

pub(crate) fn remember_bounds(hwnd: HWND, bounds: RECT) {
    BODY.with(|body| body.set(Some((hwnd, bounds))));
}

pub(crate) fn radius(width: f32, height: f32) -> f32 {
    crate::animation::EXPANDED
        .radius
        .min(width * 0.5)
        .min(height * 0.5)
}

pub(crate) fn fit_rect(rect: RECT, work: RECT) -> RECT {
    let width = (rect.right - rect.left)
        .max(1)
        .min((work.right - work.left).max(1));
    let height = (rect.bottom - rect.top)
        .max(1)
        .min((work.bottom - work.top).max(1));
    let left = rect
        .left
        .clamp(work.left, (work.right - width).max(work.left));
    let top = rect
        .top
        .clamp(work.top, (work.bottom - height).max(work.top));
    RECT {
        left,
        top,
        right: left + width,
        bottom: top + height,
    }
}

pub(crate) fn adjacent_rect(body: RECT, work: RECT, size: (i32, i32), gap: i32) -> RECT {
    let left = body.left + (body.right - body.left - size.0) / 2;
    let top = if body.bottom + gap + size.1 <= work.bottom {
        body.bottom + gap
    } else if body.top - gap - size.1 >= work.top {
        body.top - gap - size.1
    } else {
        body.bottom + gap
    };
    fit_rect(
        RECT {
            left,
            top,
            right: left + size.0,
            bottom: top + size.1,
        },
        work,
    )
}

pub(crate) unsafe fn work_area(rect: RECT) -> RECT {
    unsafe {
        let mut info = MONITORINFO {
            cbSize: std::mem::size_of::<MONITORINFO>() as u32,
            ..Default::default()
        };
        if GetMonitorInfoW(MonitorFromRect(&rect, MONITOR_DEFAULTTONEAREST), &mut info) != 0 {
            info.rcWork
        } else {
            rect
        }
    }
}

pub(crate) unsafe fn place(hwnd: HWND, owner: HWND, size: (f32, f32)) {
    unsafe {
        let mut body = RECT::default();
        if !owner.is_null() && IsWindow(owner) != 0 {
            GetWindowRect(owner, &mut body);
            if let Some((known, bounds)) = BODY.with(Cell::get)
                && known == owner
            {
                body = bounds;
            }
        } else {
            let mut point = windows_sys::Win32::Foundation::POINT::default();
            GetCursorPos(&mut point);
            body = RECT {
                left: point.x,
                top: point.y,
                right: point.x,
                bottom: point.y,
            };
        }
        let scale = windows_sys::Win32::UI::HiDpi::GetDpiForWindow(hwnd).max(96) as f32 / 96.0;
        let rect = adjacent_rect(
            body,
            work_area(body),
            (
                (size.0 * scale).round() as i32,
                (size.1 * scale).round() as i32,
            ),
            (8.0 * scale).round() as i32,
        );
        SetWindowPos(
            hwnd,
            null_mut(),
            rect.left,
            rect.top,
            rect.right - rect.left,
            rect.bottom - rect.top,
            SWP_NOZORDER | SWP_NOACTIVATE,
        );
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use windows_sys::Win32::Foundation::RECT;

    #[test]
    fn adjacent_surface_uses_visible_body_instead_of_transparent_canvas() {
        let work = RECT {
            left: -1920,
            top: 0,
            right: 0,
            bottom: 1040,
        };
        let body = RECT {
            left: -1034,
            top: 8,
            right: -886,
            bottom: 48,
        };
        let rect = adjacent_rect(body, work, (420, 640), 8);
        assert_eq!(
            (rect.left, rect.top, rect.right, rect.bottom),
            (-1170, 56, -750, 696)
        );
    }

    #[test]
    fn fitting_handles_small_high_dpi_work_areas_and_negative_origins() {
        let work = RECT {
            left: -800,
            top: -600,
            right: 0,
            bottom: 0,
        };
        let rect = adjacent_rect(
            RECT {
                left: -100,
                top: -100,
                right: 0,
                bottom: -20,
            },
            work,
            (1440, 1520),
            16,
        );
        assert_eq!(
            (rect.left, rect.top, rect.right, rect.bottom),
            (-800, -600, 0, 0)
        );
        let restored = fit_rect(
            RECT {
                left: -1200,
                top: 500,
                right: -800,
                bottom: 800,
            },
            work,
        );
        assert_eq!(
            (restored.left, restored.top, restored.right, restored.bottom),
            (-800, -300, -400, 0)
        );
    }
}
