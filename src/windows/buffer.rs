use std::{
    mem::{size_of, zeroed},
    ptr::null_mut,
};
use windows_sys::Win32::Graphics::Gdi::*;

/// Keep the DC and DIB across frames, including animated height changes.
pub(super) struct Buffer {
    pub dc: HDC,
    bitmap: HBITMAP,
    original: HGDIOBJ,
    pub bits: *mut u32,
    width: i32,
    height: i32,
}

impl Default for Buffer {
    fn default() -> Self {
        Self {
            dc: null_mut(),
            bitmap: null_mut(),
            original: null_mut(),
            bits: null_mut(),
            width: 0,
            height: 0,
        }
    }
}

impl Buffer {
    pub unsafe fn ensure(&mut self, target: HDC, width: i32, height: i32) -> bool {
        if self.width == width && self.height >= height {
            return true;
        }
        let mut next = Self::default();
        next.dc = CreateCompatibleDC(target);
        if next.dc.is_null() {
            return false;
        }
        let mut info: BITMAPINFO = zeroed();
        info.bmiHeader.biSize = size_of::<BITMAPINFOHEADER>() as u32;
        info.bmiHeader.biWidth = width;
        info.bmiHeader.biHeight = -height;
        info.bmiHeader.biPlanes = 1;
        info.bmiHeader.biBitCount = 32;
        info.bmiHeader.biCompression = BI_RGB;
        let mut bits = null_mut();
        next.bitmap = CreateDIBSection(target, &info, DIB_RGB_COLORS, &mut bits, null_mut(), 0);
        if next.bitmap.is_null() {
            return false;
        }
        next.original = SelectObject(next.dc, next.bitmap);
        next.bits = bits.cast();
        next.width = width;
        next.height = height;
        *self = next;
        true
    }
}

impl Drop for Buffer {
    fn drop(&mut self) {
        unsafe {
            if !self.original.is_null() {
                SelectObject(self.dc, self.original);
            }
            if !self.bitmap.is_null() {
                DeleteObject(self.bitmap);
            }
            if !self.dc.is_null() {
                DeleteDC(self.dc);
            }
        }
    }
}
