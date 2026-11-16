//! Share an in-use original without retaining images after their consumers close.
use crate::clipboard_image::{self, Image};
use std::{
    fs,
    path::{Path, PathBuf},
    sync::{Arc, Mutex, OnceLock, Weak},
    time::SystemTime,
};

pub type SharedImage = Arc<Image>;
type ImageResult = Result<SharedImage, String>;
type BytesResult = Result<Arc<Vec<u8>>, String>;

#[derive(PartialEq, Eq)]
struct Key {
    path: PathBuf,
    length: u64,
    modified: Option<SystemTime>,
}

pub struct Asset {
    key: Key,
    source: OnceLock<BytesResult>,
    image: OnceLock<ImageResult>,
    png: OnceLock<BytesResult>,
    fitted: Mutex<Option<(u32, u32, SharedImage)>>,
}

pub fn asset(path: &Path) -> Result<Arc<Asset>, String> {
    let metadata = fs::metadata(path).map_err(|e| e.to_string())?;
    let key = Key {
        path: path.to_path_buf(),
        length: metadata.len(),
        modified: metadata.modified().ok(),
    };
    static CURRENT: Mutex<Weak<Asset>> = Mutex::new(Weak::new());
    let mut current = CURRENT.lock().map_err(|_| "图片缓存不可用")?;
    if let Some(existing) = current.upgrade().filter(|a| a.key == key) {
        return Ok(existing);
    }
    let next = Arc::new(Asset {
        key,
        source: OnceLock::new(),
        image: OnceLock::new(),
        png: OnceLock::new(),
        fitted: Mutex::new(None),
    });
    *current = Arc::downgrade(&next);
    Ok(next)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn releasing_last_consumer_frees_original_scaled_image_and_png() {
        let root = Path::new(env!("CARGO_MANIFEST_DIR"))
            .join("artifacts")
            .join(format!("image-cache-lifetime-{}", std::process::id()));
        fs::create_dir_all(&root).unwrap();
        let path = root.join("original.png");
        Image {
            width: 32,
            height: 32,
            pixels: vec![255; 32 * 32 * 4],
        }
        .write_png(&path)
        .unwrap();
        let cached = asset(&path).unwrap();
        let same = asset(&path).unwrap();
        assert!(Arc::ptr_eq(&cached, &same));
        let original = cached.image().unwrap();
        let fitted = cached.fit(16, 16).unwrap();
        let png = cached.png().unwrap();
        let weak_asset = Arc::downgrade(&cached);
        let weak_original = Arc::downgrade(&original);
        let weak_fitted = Arc::downgrade(&fitted);
        let weak_png = Arc::downgrade(&png);
        drop((original, fitted, png, cached));
        assert!(weak_asset.upgrade().is_some());
        drop(same);
        assert!(weak_asset.upgrade().is_none());
        assert!(weak_original.upgrade().is_none());
        assert!(weak_fitted.upgrade().is_none());
        assert!(weak_png.upgrade().is_none());
        fs::remove_file(path).unwrap();
        fs::remove_dir(root).unwrap();
    }
}

impl Asset {
    fn source(&self) -> BytesResult {
        self.source
            .get_or_init(|| clipboard_image::read_encoded(&self.key.path).map(Arc::new))
            .clone()
    }

    pub fn image(&self) -> ImageResult {
        self.image
            .get_or_init(|| clipboard_image::decode(&self.source()?, false).map(Arc::new))
            .clone()
    }

    pub fn png(&self) -> BytesResult {
        self.png
            .get_or_init(|| {
                let bytes = self.source()?;
                if bytes.starts_with(b"\x89PNG\r\n\x1a\n") {
                    if bytes.len() < 24 {
                        return Err("PNG 图片头不完整".into());
                    }
                    let w = u32::from_be_bytes(bytes[16..20].try_into().unwrap());
                    let h = u32::from_be_bytes(bytes[20..24].try_into().unwrap());
                    if w == 0
                        || h == 0
                        || w > clipboard_image::MAX_DIMENSION
                        || h > clipboard_image::MAX_DIMENSION
                        || u64::from(w) * u64::from(h) * 4 > clipboard_image::MAX_BYTES as u64
                    {
                        return Err("PNG 图片尺寸超限".into());
                    }
                    return Ok(bytes);
                }
                self.image()?.png().map(Arc::new)
            })
            .clone()
    }

    pub fn fit(&self, width: u32, height: u32) -> ImageResult {
        let mut fitted = self.fitted.lock().map_err(|_| "图片缩放缓存不可用")?;
        if let Some((w, h, image)) = &*fitted {
            if *w == width && *h == height {
                return Ok(image.clone());
            }
        }
        let original = self.image()?;
        let image = if original.width <= width && original.height <= height {
            original
        } else {
            Arc::new(original.thumbnail(width.max(1), height.max(1))?)
        };
        *fitted = Some((width, height, image.clone()));
        Ok(image)
    }

    pub fn cached_fit(&self, width: u32, height: u32) -> Option<SharedImage> {
        let fitted = self.fitted.try_lock().ok()?;
        fitted
            .as_ref()
            .filter(|(w, h, _)| *w == width && *h == height)
            .map(|(_, _, image)| image.clone())
    }
}
