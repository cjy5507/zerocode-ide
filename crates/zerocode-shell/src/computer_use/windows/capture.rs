//! One window as pixels.
//!
//! `PrintWindow` with `PW_RENDERFULLCONTENT` asks the window to paint itself
//! into our bitmap — including DirectComposition content that a plain
//! `BitBlt` from the screen would miss — and works for a window another
//! window overlaps. When a window refuses to print (some legacy or
//! GPU-only surfaces), the screen region is copied instead, overlaps and
//! all, and the engine name says which road was taken.

use windows::Win32::Foundation::HWND;
use windows::Win32::Graphics::Gdi::{
    BI_RGB, BITMAPINFO, BITMAPINFOHEADER, BitBlt, CreateCompatibleDC, CreateDIBSection,
    DIB_RGB_COLORS, DeleteDC, DeleteObject, GetDC, HGDIOBJ, ReleaseDC, SRCCOPY, SelectObject,
};
use windows::Win32::Storage::Xps::{PRINT_WINDOW_FLAGS, PrintWindow};
use windows::Win32::UI::WindowsAndMessaging::PW_RENDERFULLCONTENT;
use zerocode_core::computer_use_protocol::render::Rect;

use super::super::screenshot_png::RgbaImage;

pub(super) struct Captured {
    pub image: RgbaImage,
    pub engine: &'static str,
}

/// A top-down 32-bit DIB and the memory DC it is selected into, released
/// together.
struct Canvas {
    dc: windows::Win32::Graphics::Gdi::HDC,
    bitmap: windows::Win32::Graphics::Gdi::HBITMAP,
    previous: windows::Win32::Graphics::Gdi::HGDIOBJ,
    bits: *mut u8,
    width: i32,
    height: i32,
}

impl Canvas {
    fn new(width: i32, height: i32) -> Option<Self> {
        let header = BITMAPINFOHEADER {
            biSize: std::mem::size_of::<BITMAPINFOHEADER>() as u32,
            biWidth: width,
            // Negative: top-down rows, so the buffer reads like an image.
            biHeight: -height,
            biPlanes: 1,
            biBitCount: 32,
            biCompression: BI_RGB.0,
            ..Default::default()
        };
        let info = BITMAPINFO {
            bmiHeader: header,
            ..Default::default()
        };
        let mut bits: *mut core::ffi::c_void = std::ptr::null_mut();
        // SAFETY: plain GDI object creation; every handle made here is
        // released in Drop, and `bits` is only read while the section lives.
        unsafe {
            let dc = CreateCompatibleDC(None);
            if dc.is_invalid() {
                return None;
            }
            let bitmap = match CreateDIBSection(None, &info, DIB_RGB_COLORS, &mut bits, None, 0) {
                Ok(bitmap) if !bitmap.is_invalid() && !bits.is_null() => bitmap,
                _ => {
                    let _ = DeleteDC(dc);
                    return None;
                }
            };
            let previous = SelectObject(dc, HGDIOBJ(bitmap.0));
            Some(Self {
                dc,
                bitmap,
                previous,
                bits: bits.cast(),
                width,
                height,
            })
        }
    }

    fn to_rgba(&self) -> Option<RgbaImage> {
        let length = self.width as usize * self.height as usize * 4;
        // SAFETY: the DIB section holds exactly width*height*4 bytes of BGRA
        // for as long as this canvas lives; the copy leaves them alone.
        let bgra = unsafe { std::slice::from_raw_parts(self.bits, length) };
        let mut pixels = Vec::with_capacity(length);
        for pixel in bgra.chunks_exact(4) {
            pixels.extend_from_slice(&[pixel[2], pixel[1], pixel[0], 255]);
        }
        RgbaImage::new(self.width as u32, self.height as u32, pixels)
    }
}

impl Drop for Canvas {
    fn drop(&mut self) {
        // SAFETY: releases exactly the objects `new` created, in reverse.
        unsafe {
            let _ = SelectObject(self.dc, self.previous);
            let _ = DeleteObject(HGDIOBJ(self.bitmap.0));
            let _ = DeleteDC(self.dc);
        }
    }
}

/// Capture `hwnd`, whose current rectangle is `bounds`.
pub(super) fn capture(hwnd: HWND, bounds: &Rect) -> Option<Captured> {
    let width = bounds.width.round() as i32;
    let height = bounds.height.round() as i32;
    if width <= 0 || height <= 0 {
        return None;
    }
    let canvas = Canvas::new(width, height)?;
    // SAFETY: PrintWindow paints into the canvas DC we own.
    let printed = unsafe { PrintWindow(hwnd, canvas.dc, PRINT_WINDOW_FLAGS(PW_RENDERFULLCONTENT)) };
    if printed.as_bool() {
        return canvas.to_rgba().map(|image| Captured {
            image,
            engine: "printWindow",
        });
    }
    // SAFETY: a screen DC is acquired and released around one BitBlt into
    // the canvas.
    let copied = unsafe {
        let screen = GetDC(None);
        if screen.is_invalid() {
            return None;
        }
        let copied = BitBlt(
            canvas.dc,
            0,
            0,
            width,
            height,
            Some(screen),
            bounds.x.round() as i32,
            bounds.y.round() as i32,
            SRCCOPY,
        );
        let _ = ReleaseDC(None, screen);
        copied.is_ok()
    };
    copied
        .then(|| canvas.to_rgba())
        .flatten()
        .map(|image| Captured {
            image,
            engine: "screenBlt",
        })
}
