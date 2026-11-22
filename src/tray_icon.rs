use std::mem::size_of;
use std::ptr::null_mut;

use windows_sys::Win32::Graphics::Gdi::{
    BI_RGB, BITMAPINFO, BITMAPINFOHEADER, CreateBitmap, CreateCompatibleDC, CreateDIBSection,
    DIB_RGB_COLORS, DeleteDC, DeleteObject,
};
use windows_sys::Win32::UI::WindowsAndMessaging::{
    CreateIconIndirect, DestroyIcon, HICON, ICONINFO, IDI_APPLICATION, LoadIconW,
};

use crate::animation::ShapeState;
use crate::geometry::rounded_rect_distance;

const ICON_SIZE: u32 = 32;

pub(crate) struct TrayIcon {
    handle: HICON,
    owned: bool,
}

impl TrayIcon {
    pub(crate) fn create(size: u32, accent: [u8; 3]) -> Option<Self> {
        if size == 0 {
            return None;
        }
        let pixels = render_tray_pixels(size, accent);
        // SAFETY: The DIB, mask, and icon are created and released within this function.
        unsafe {
            let dc = CreateCompatibleDC(null_mut());
            if dc.is_null() {
                return None;
            }
            let info = BITMAPINFO {
                bmiHeader: BITMAPINFOHEADER {
                    biSize: size_of::<BITMAPINFOHEADER>() as u32,
                    biWidth: size as i32,
                    biHeight: -(size as i32),
                    biPlanes: 1,
                    biBitCount: 32,
                    biCompression: BI_RGB,
                    ..Default::default()
                },
                ..Default::default()
            };
            let mut bits = null_mut();
            let color = CreateDIBSection(dc, &info, DIB_RGB_COLORS, &mut bits, null_mut(), 0);
            if color.is_null() || bits.is_null() {
                if !color.is_null() {
                    DeleteObject(color);
                }
                DeleteDC(dc);
                return None;
            }
            std::ptr::copy_nonoverlapping(pixels.as_ptr(), bits.cast::<u8>(), pixels.len());

            let mask_stride = size.div_ceil(16) * 2;
            let mask_bits = vec![0_u8; (mask_stride * size) as usize];
            let mask = CreateBitmap(size as i32, size as i32, 1, 1, mask_bits.as_ptr().cast());
            if mask.is_null() {
                DeleteObject(color);
                DeleteDC(dc);
                return None;
            }

            let icon_info = ICONINFO {
                fIcon: 1,
                xHotspot: 0,
                yHotspot: 0,
                hbmMask: mask,
                hbmColor: color,
            };
            let handle = CreateIconIndirect(&icon_info);
            DeleteObject(mask);
            DeleteObject(color);
            DeleteDC(dc);
            if handle.is_null() {
                None
            } else {
                Some(Self {
                    handle,
                    owned: true,
                })
            }
        }
    }

    pub(crate) fn system() -> Option<Self> {
        // SAFETY: IDI_APPLICATION is a shared system icon; the handle must not be destroyed.
        let handle = unsafe { LoadIconW(null_mut(), IDI_APPLICATION) };
        if handle.is_null() {
            None
        } else {
            Some(Self {
                handle,
                owned: false,
            })
        }
    }

    pub(crate) fn handle(&self) -> HICON {
        self.handle
    }
}

impl Drop for TrayIcon {
    fn drop(&mut self) {
        if self.owned {
            // SAFETY: The icon was created by CreateIconIndirect and is owned by this value.
            unsafe { DestroyIcon(self.handle) };
        }
    }
}

pub(crate) fn render_tray_pixels(size: u32, accent: [u8; 3]) -> Vec<u8> {
    let mut pixels = vec![0_u8; size as usize * size as usize * 4];
    if size == 0 {
        return pixels;
    }
    let scale = size as f32 / ICON_SIZE as f32;
    let state = ShapeState {
        width: 28.0 * scale,
        height: 15.0 * scale,
        radius: 7.5 * scale,
    };
    let center = (size as f32 * 0.5, size as f32 * 0.5);
    let dot_radius = 3.0 * scale;

    for y in 0..size {
        for x in 0..size {
            let px = x as f32 + 0.5;
            let py = y as f32 + 0.5;
            let mut pixel = [0_u8, 0_u8, 0_u8, 0_u8];
            let distance = rounded_rect_distance(px, py, state, center);
            let coverage = (0.5 - distance).clamp(0.0, 1.0);
            if coverage > 0.0 {
                over(&mut pixel, crate::theme::SURFACE, coverage);
                let rim = (1.0 - (distance + 0.45).abs() / 0.6).clamp(0.0, 1.0);
                if rim > 0.0 {
                    over(&mut pixel, crate::theme::TEXT, rim * 0.75);
                }
            }
            let dot = (dot_radius + 0.5 - (px - center.0).hypot(py - center.1)).clamp(0.0, 1.0);
            if dot > 0.0 {
                over(&mut pixel, accent, dot);
            }
            let index = ((y * size + x) * 4) as usize;
            pixels[index..index + 4].copy_from_slice(&pixel);
        }
    }
    pixels
}

fn over(destination: &mut [u8; 4], color: [u8; 3], alpha: f32) {
    let alpha = alpha.clamp(0.0, 1.0);
    let destination_alpha = destination[3] as f32 / 255.0;
    let out_alpha = alpha + destination_alpha * (1.0 - alpha);
    if out_alpha <= 0.0 {
        *destination = [0, 0, 0, 0];
        return;
    }
    for channel in 0..3 {
        destination[channel] = ((color[channel] as f32 * alpha
            + destination[channel] as f32 * destination_alpha * (1.0 - alpha))
            / out_alpha)
            .round()
            .clamp(0.0, 255.0) as u8;
    }
    destination[3] = (out_alpha * 255.0).round().clamp(0.0, 255.0) as u8;
}

#[cfg(test)]
mod tests {
    use super::render_tray_pixels;

    #[test]
    fn creates_an_icon_handle() {
        assert!(super::TrayIcon::create(32, crate::theme::ICE).is_some());
    }

    #[test]
    fn tray_pixels_have_an_opaque_center_and_transparent_corner() {
        let pixels = render_tray_pixels(32, crate::theme::ICE);
        let center = (16 * 32 + 16) * 4;
        assert_eq!(&pixels[center..center + 3], &crate::theme::ICE);
        assert_eq!(pixels[center + 3], 255);
        let capsule = (16 * 32 + 8) * 4;
        assert_eq!(&pixels[capsule..capsule + 3], &crate::theme::SURFACE);
        assert_eq!(pixels[capsule + 3], 255);
        assert_eq!(pixels[3], 0);
    }

    #[test]
    fn tray_pixels_scale_down_for_small_icons() {
        let pixels = render_tray_pixels(16, crate::theme::ICE);
        assert_eq!(pixels.len(), 16 * 16 * 4);
        assert_eq!(pixels[(8 * 16 + 8) * 4 + 3], 255);
        assert_eq!(pixels[3], 0);
    }
}
