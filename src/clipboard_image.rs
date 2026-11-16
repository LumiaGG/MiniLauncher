//! Bounded native image decoding and lossless PNG storage.
use std::{
    fs,
    io::{Read, Write},
    path::Path,
    ptr::{null, null_mut},
};
use windows::Win32::{
    Graphics::Imaging::*,
    System::Com::{CoCreateInstance, CLSCTX_INPROC_SERVER},
};
use windows_sys::Win32::{
    Foundation::*,
    Security::Cryptography::*,
    System::{Com::*, DataExchange::*, Memory::*},
};

pub const MAX_BYTES: usize = 40 * 1024 * 1024;
pub const MAX_ENCODED_BYTES: usize = MAX_BYTES + 65536;
pub const MAX_DIMENSION: u32 = 16384;

/// Snapshot native PNG / DIB clipboard data and release the clipboard before decoding.
pub unsafe fn read_clipboard_image() -> Result<Option<Image>, String> {
    if OpenClipboard(null_mut()) == 0 {
        return Err("剪贴板正被占用，请重试粘贴".into());
    }
    struct Close;
    impl Drop for Close {
        fn drop(&mut self) {
            unsafe {
                CloseClipboard();
            }
        }
    }
    let close = Close;
    let png: Vec<u16> = "PNG".encode_utf16().chain(Some(0)).collect();
    let png = RegisterClipboardFormatW(png.as_ptr());
    let mut content = None;
    for format in [png, 17, 8] {
        if format == 0 || IsClipboardFormatAvailable(format) == 0 {
            continue;
        }
        let handle = GetClipboardData(format);
        if handle.is_null() {
            continue;
        }
        let length = GlobalSize(handle);
        if length == 0 || length > MAX_ENCODED_BYTES {
            return Err("剪贴板图片过大".into());
        }
        let data = GlobalLock(handle);
        if data.is_null() {
            return Err("无法读取剪贴板图片".into());
        }
        let bytes = std::slice::from_raw_parts(data.cast::<u8>(), length).to_vec();
        GlobalUnlock(handle);
        content = Some((bytes, format != png));
        break;
    }
    drop(close);
    content.map(|(bytes, dib)| decode(&bytes, dib)).transpose()
}

#[derive(Clone)]
pub struct Image {
    pub width: u32,
    pub height: u32,
    pub pixels: Vec<u8>, // straight BGRA, top to bottom
}

fn pixel_bytes(width: u32, height: u32) -> Result<usize, String> {
    if width == 0
        || height == 0
        || width > MAX_DIMENSION
        || height > MAX_DIMENSION
    {
        return Err("图片尺寸超限（最多 40 MiB 像素、单边 16384）".into());
    }
    let size = u64::from(width) * u64::from(height) * 4;
    if size > MAX_BYTES as u64 {
        return Err("图片尺寸超限（最多 40 MiB 像素、单边 16384）".into());
    }
    Ok(size as usize)
}

pub fn read_encoded(path: &Path) -> Result<Vec<u8>, String> {
    let file = fs::File::open(path).map_err(|e| e.to_string())?;
    let length = file.metadata().map_err(|e| e.to_string())?.len();
    if length > MAX_ENCODED_BYTES as u64 {
        return Err("图片文件过大".into());
    }
    let mut bytes = Vec::with_capacity(length as usize);
    file.take(MAX_ENCODED_BYTES as u64 + 1)
        .read_to_end(&mut bytes)
        .map_err(|e| e.to_string())?;
    if bytes.len() > MAX_ENCODED_BYTES {
        return Err("图片文件过大".into());
    }
    Ok(bytes)
}

pub fn write_png_bytes(path: &Path, bytes: &[u8]) -> Result<(), String> {
    let temporary = path.with_extension("png.tmp");
    let result = fs::write(&temporary, bytes)
        .and_then(|_| fs::rename(&temporary, path))
        .map_err(|e| e.to_string());
    if result.is_err() {
        let _ = fs::remove_file(temporary);
    }
    result
}

struct Com(bool);
impl Com {
    fn new() -> Self {
        Self(unsafe { CoInitializeEx(null(), COINIT_MULTITHREADED as u32) } >= 0)
    }
}
impl Drop for Com {
    fn drop(&mut self) {
        if self.0 {
            unsafe {
                CoUninitialize();
            }
        }
    }
}

impl Image {
    pub fn png(&self) -> Result<Vec<u8>, String> {
        self.validate()?;
        let row_bytes = self.width as usize * 4;
        let mut row_buf = vec![0u8; row_bytes];
        let mut bytes = Vec::new();
        {
            let mut encoder = png::Encoder::new(&mut bytes, self.width, self.height);
            encoder.set_color(png::ColorType::Rgba);
            encoder.set_depth(png::BitDepth::Eight);
            encoder.set_compression(png::Compression::Fast);
            let mut writer = encoder.write_header().map_err(|e| e.to_string())?;
            {
                let mut stream = writer.stream_writer().map_err(|e| e.to_string())?;
                for row in self.pixels.chunks_exact(row_bytes) {
                    row_buf.copy_from_slice(row);
                    for pixel in row_buf.chunks_exact_mut(4) {
                        pixel.swap(0, 2);
                    }
                    stream.write_all(&row_buf).map_err(|e| e.to_string())?;
                }
                stream.finish().map_err(|e| e.to_string())?;
            }
            writer.finish().map_err(|e| e.to_string())?;
        }
        Ok(bytes)
    }

    pub fn write_png(&self, path: &Path) -> Result<(), String> {
        write_png_bytes(path, &self.png()?)
    }
    pub fn validate(&self) -> Result<(), String> {
        if self.pixels.len() != pixel_bytes(self.width, self.height)? {
            return Err("图片像素长度无效".into());
        }
        Ok(())
    }
    pub fn thumbnail(&self, width: u32, height: u32) -> Result<Self, String> {
        self.validate()?;
        let scale = (width as f64 / self.width as f64)
            .min(height as f64 / self.height as f64)
            .min(1.0);
        let w = (self.width as f64 * scale).round().max(1.0) as u32;
        let h = (self.height as f64 * scale).round().max(1.0) as u32;
        let _com = Com::new();
        unsafe {
            let result = (|| -> windows::core::Result<Self> {
                let factory: IWICImagingFactory =
                    CoCreateInstance(&CLSID_WICImagingFactory, None, CLSCTX_INPROC_SERVER)?;
                let bitmap = factory.CreateBitmapFromMemory(
                    self.width,
                    self.height,
                    &GUID_WICPixelFormat32bppBGRA,
                    self.width * 4,
                    &self.pixels,
                )?;
                let scaler = factory.CreateBitmapScaler()?;
                scaler.Initialize(&bitmap, w, h, WICBitmapInterpolationModeFant)?;
                let mut pixels = vec![0; (w * h * 4) as usize];
                scaler.CopyPixels(null(), w * 4, &mut pixels)?;
                Ok(Self {
                    width: w,
                    height: h,
                    pixels,
                })
            })();
            result.map_err(|e| e.to_string())
        }
    }
    pub fn fingerprint(&self) -> Result<String, String> {
        struct AlgGuard(BCRYPT_ALG_HANDLE);
        impl Drop for AlgGuard {
            fn drop(&mut self) {
                unsafe {
                    BCryptCloseAlgorithmProvider(self.0, 0);
                }
            }
        }
        struct HashGuard(BCRYPT_HASH_HANDLE);
        impl Drop for HashGuard {
            fn drop(&mut self) {
                unsafe {
                    BCryptDestroyHash(self.0);
                }
            }
        }
        self.validate()?;
        let mut provider = null_mut();
        unsafe {
            if BCryptOpenAlgorithmProvider(&mut provider, BCRYPT_SHA256_ALGORITHM, null(), 0) < 0 {
                return Err("无法初始化图片摘要".into());
            }
            let _provider = AlgGuard(provider);
            let mut hash = null_mut();
            if BCryptCreateHash(provider, &mut hash, null_mut(), 0, null(), 0, 0) < 0 {
                return Err("无法创建图片摘要".into());
            }
            let _hash = HashGuard(hash);
            let mut header = [0u8; 8];
            header[..4].copy_from_slice(&self.width.to_le_bytes());
            header[4..].copy_from_slice(&self.height.to_le_bytes());
            let mut digest = [0u8; 32];
            if BCryptHashData(hash, header.as_ptr(), 8, 0) < 0
                || BCryptHashData(hash, self.pixels.as_ptr(), self.pixels.len() as u32, 0) < 0
                || BCryptFinishHash(hash, digest.as_mut_ptr(), 32, 0) < 0
            {
                return Err("无法计算图片摘要".into());
            }
            Ok(digest.iter().map(|b| format!("{b:02x}")).collect())
        }
    }
    pub fn dib(&self) -> Result<Vec<u8>, String> {
        self.validate()?;
        let mut header = Vec::with_capacity(124 + self.pixels.len());
        header.resize(124, 0);
        for (offset, value) in [
            (0, 124u32),
            (4, self.width),
            (8, (-(self.height as i32)) as u32),
            (16, 3),
            (20, self.pixels.len() as u32),
            (40, 0x00ff0000),
            (44, 0x0000ff00),
            (48, 0x000000ff),
            (52, 0xff000000),
            (56, 0x73524742),
        ] {
            header[offset..offset + 4].copy_from_slice(&value.to_le_bytes());
        }
        header[12..14].copy_from_slice(&1u16.to_le_bytes());
        header[14..16].copy_from_slice(&32u16.to_le_bytes());
        header.extend_from_slice(&self.pixels);
        Ok(header)
    }
    /// Copy an explicitly selected image as CF_DIBV5.
    ///
    /// # Safety
    /// `owner` must be a live window owned by the current process.
    pub unsafe fn copy_to_clipboard(&self, owner: HWND) -> Result<(), String> {
        let bytes = self.dib()?;
        let memory = GlobalAlloc(GMEM_MOVEABLE, bytes.len());
        if memory.is_null() {
            return Err("无法分配剪贴板图片内存".into());
        }
        let pointer = GlobalLock(memory);
        if pointer.is_null() {
            GlobalFree(memory);
            return Err("无法锁定剪贴板图片内存".into());
        }
        std::ptr::copy_nonoverlapping(bytes.as_ptr(), pointer.cast(), bytes.len());
        GlobalUnlock(memory);
        if owner.is_null() || OpenClipboard(owner) == 0 {
            GlobalFree(memory);
            return Err("剪贴板被占用，请重试".into());
        }
        let result = EmptyClipboard() != 0 && !SetClipboardData(17, memory).is_null(); // CF_DIBV5
        CloseClipboard();
        if !result {
            GlobalFree(memory);
            return Err("复制图片到剪贴板失败".into());
        }
        Ok(())
    }
}

fn number(bytes: &[u8], offset: usize) -> Result<u32, String> {
    bytes
        .get(offset..offset + 4)
        .map(|b| u32::from_le_bytes(b.try_into().unwrap()))
        .ok_or_else(|| "图片头不完整".into())
}

/// Turn a packed Windows DIB into a bounded BMP stream for the system decoder.
fn bitmap_file(bytes: &[u8]) -> Result<Vec<u8>, String> {
    let header = number(bytes, 0)? as usize;
    if ![40, 52, 56, 108, 124].contains(&header) || bytes.len() < header {
        return Err("不支持的 DIB 图片头".into());
    }
    let width = number(bytes, 4)? as i32;
    let height = number(bytes, 8)? as i32;
    if width <= 0 || height == i32::MIN {
        return Err("DIB 图片尺寸无效".into());
    }
    pixel_bytes(width as u32, height.unsigned_abs())?;
    let bits = u16::from_le_bytes(bytes[14..16].try_into().unwrap());
    if ![1, 4, 8, 16, 24, 32].contains(&bits) {
        return Err("不支持的 DIB 位深".into());
    }
    let compression = number(bytes, 16)?;
    if ![0, 3, 6].contains(&compression) {
        return Err("不支持的 DIB 压缩格式".into());
    }
    let colors = number(bytes, 32)? as usize;
    let palette = if bits <= 8 {
        if colors == 0 {
            1usize << bits
        } else {
            colors
        }
    } else {
        colors
    };
    if palette > 256 {
        return Err("DIB 调色板过大".into());
    }
    let masks = if header == 40 {
        match compression {
            3 => 12,
            6 => 16,
            _ => 0,
        }
    } else {
        0
    };
    let offset = header + masks + palette * 4;
    let stride = (width as usize * bits as usize).div_ceil(32) * 4;
    if offset
        .checked_add(stride * height.unsigned_abs() as usize)
        .is_none_or(|n| n > bytes.len())
    {
        return Err("DIB 图片像素不完整".into());
    }
    let mut file = b"BM".to_vec();
    file.extend(((bytes.len() + 14) as u32).to_le_bytes());
    file.extend([0u8; 4]);
    file.extend(((offset + 14) as u32).to_le_bytes());
    file.extend(bytes);
    Ok(file)
}

pub fn decode(bytes: &[u8], dib: bool) -> Result<Image, String> {
    if bytes.len() > MAX_ENCODED_BYTES {
        return Err("图片数据过大".into());
    }
    let mut encoded = if dib {
        bitmap_file(bytes)?
    } else {
        bytes.to_vec()
    };
    let _com = Com::new();
    unsafe {
        let result = (|| -> Result<Image, String> {
            let factory: IWICImagingFactory =
                CoCreateInstance(&CLSID_WICImagingFactory, None, CLSCTX_INPROC_SERVER)
                    .map_err(|e| e.to_string())?;
            let stream = factory.CreateStream().map_err(|e| e.to_string())?;
            stream
                .InitializeFromMemory(&mut encoded)
                .map_err(|e| e.to_string())?;
            let decoder = factory
                .CreateDecoderFromStream(&stream, null(), WICDecodeMetadataCacheOnLoad)
                .map_err(|e| e.to_string())?;
            let frame = decoder.GetFrame(0).map_err(|e| e.to_string())?;
            let (mut width, mut height) = (0, 0);
            frame
                .GetSize(&mut width, &mut height)
                .map_err(|e| e.to_string())?;
            let mut pixels = vec![0; pixel_bytes(width, height)?];
            let converter = factory.CreateFormatConverter().map_err(|e| e.to_string())?;
            converter
                .Initialize(
                    &frame,
                    &GUID_WICPixelFormat32bppBGRA,
                    WICBitmapDitherTypeNone,
                    None,
                    0.0,
                    WICBitmapPaletteTypeCustom,
                )
                .map_err(|e| e.to_string())?;
            converter
                .CopyPixels(null(), width * 4, &mut pixels)
                .map_err(|e| e.to_string())?;
            if dib && number(bytes, 16)? == 0 {
                for pixel in pixels.chunks_exact_mut(4) {
                    pixel[3] = 255;
                }
            }
            Ok(Image {
                width,
                height,
                pixels,
            })
        })();
        result
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn dib_round_trip_preserves_pixels_and_rejects_invalid_headers() {
        let image = Image {
            width: 2,
            height: 2,
            pixels: vec![
                0, 0, 255, 255, 0, 255, 0, 255, 255, 0, 0, 128, 255, 255, 255, 255,
            ],
        };
        let dib = image.dib().unwrap();
        let restored = decode(&dib, true).unwrap();
        assert_eq!((restored.width, restored.height), (2, 2));
        assert_eq!(restored.pixels, image.pixels);
        assert_eq!(
            image.fingerprint().unwrap(),
            restored.fingerprint().unwrap()
        );
        let png = image.png().unwrap();
        let from_png = decode(&png, false).unwrap();
        assert_eq!(from_png.pixels, image.pixels);
        assert_eq!(
            from_png.fingerprint().unwrap(),
            image.fingerprint().unwrap()
        );
        assert!(decode(&dib[..125], true).is_err());
        let mut oversized = dib.clone();
        oversized[4..8].copy_from_slice(&20000u32.to_le_bytes());
        assert!(decode(&oversized, true).is_err());
        assert!(decode(b"not an image", false).is_err());
        let thumbnail = image.thumbnail(1, 1).unwrap();
        assert_eq!((thumbnail.width, thumbnail.height), (1, 1));
    }
}
