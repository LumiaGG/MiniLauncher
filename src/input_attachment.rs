//! Query attachments own their temporary files until all in-flight users finish.
use serde::Serialize;
use std::{fs, path::PathBuf, sync::Arc};

#[derive(Clone, Debug, Serialize)]
pub struct InputImage {
    #[serde(rename = "type")]
    kind: &'static str,
    pub path: PathBuf,
    pub width: u32,
    pub height: u32,
    #[serde(skip)]
    _file: Arc<TemporaryImage>,
}

#[derive(Debug)]
struct TemporaryImage(PathBuf);
impl Drop for TemporaryImage {
    fn drop(&mut self) {
        let _ = fs::remove_file(&self.0);
    }
}

impl InputImage {
    pub fn new(image: &crate::clipboard_image::Image) -> Result<Self, String> {
        use std::io::Write;
        use windows_sys::Win32::Security::Cryptography::*;
        let mut random = [0u8; 16];
        if unsafe {
            BCryptGenRandom(
                std::ptr::null_mut(),
                random.as_mut_ptr(),
                16,
                BCRYPT_USE_SYSTEM_PREFERRED_RNG,
            )
        } < 0
        {
            return Err("无法生成图片附件标识".into());
        }
        let id: String = random.iter().map(|b| format!("{b:02x}")).collect();
        let path = std::env::temp_dir().join(format!("mini-launcher-input-{id}.png"));
        let bytes = image.png()?;
        let mut file = fs::OpenOptions::new()
            .write(true)
            .create_new(true)
            .open(&path)
            .map_err(|e| e.to_string())?;
        let guard = Arc::new(TemporaryImage(path.clone()));
        let result = file.write_all(&bytes);
        drop(file);
        result.map_err(|e| e.to_string())?;
        Ok(Self {
            kind: "image",
            path,
            width: image.width,
            height: image.height,
            _file: guard,
        })
    }
}
