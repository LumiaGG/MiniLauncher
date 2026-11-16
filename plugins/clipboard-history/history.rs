use serde::{Deserialize, Serialize};
use std::{
    collections::HashSet,
    fs,
    io::Read,
    path::{Path, PathBuf},
    time::{SystemTime, UNIX_EPOCH},
};
use windows_sys::Win32::{Security::Cryptography::*, Storage::FileSystem::*};

pub const MAX_TEXT: usize = 16 * 1024;
const MAX_TOTAL: usize = 2 * 1024 * 1024;
const MAX_IMAGES: u64 = 128 * 1024 * 1024;

#[derive(Clone, Serialize, Deserialize)]
pub struct ImageInfo {
    pub width: u32,
    pub height: u32,
    pub bytes: u64,
    pub fingerprint: String,
}

#[derive(Clone, Serialize, Deserialize)]
pub struct Entry {
    pub id: String,
    #[serde(default)]
    pub text: String,
    pub time: u64,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub image: Option<ImageInfo>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub files: Option<Vec<String>>,
}

#[derive(Clone, Default, Serialize, Deserialize)]
pub struct History {
    pub paused: bool,
    pub entries: Vec<Entry>,
    #[serde(default, skip_serializing_if = "HashSet::is_empty")]
    pub pinned: HashSet<String>,
}

pub struct State {
    pub history: History,
    pub path: PathBuf,
    pub maximum: usize,
    pub error: Option<String>,
    pub revision: String,
    pub window: usize,
}

pub struct PreparedImage {
    pub width: u32,
    pub height: u32,
    pub bytes: u64,
    pub fingerprint: String,
    pub png: Vec<u8>,
    pub preview_png: Vec<u8>,
}

impl PreparedImage {
    pub fn new(
        image: mini_launcher::clipboard_image::Image,
        png: Option<Vec<u8>>,
    ) -> Result<Self, String> {
        image.validate()?;
        let fingerprint = image.fingerprint()?;
        let preview_png = image.thumbnail(640, 120)?.png()?;
        let png = match png {
            Some(bytes) => bytes,
            None => image.png()?,
        };
        Ok(Self {
            width: image.width,
            height: image.height,
            bytes: image.pixels.len() as u64,
            fingerprint,
            png,
            preview_png,
        })
    }
}

pub fn random_id() -> Result<String, String> {
    let mut bytes = [0u8; 16];
    if unsafe {
        BCryptGenRandom(
            std::ptr::null_mut(),
            bytes.as_mut_ptr(),
            16,
            BCRYPT_USE_SYSTEM_PREFERRED_RNG,
        )
    } < 0
    {
        return Err("生成历史标识失败".into());
    }
    Ok(bytes.iter().map(|b| format!("{b:02x}")).collect())
}

pub fn load(path: &Path) -> Result<History, String> {
    let file = match fs::File::open(path) {
        Ok(file) => file,
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => return Ok(History::default()),
        Err(e) => return Err(e.to_string()),
    };
    let mut bytes = Vec::new();
    file.take(16 * 1024 * 1024 + 1)
        .read_to_end(&mut bytes)
        .map_err(|e| e.to_string())?;
    if bytes.len() > 16 * 1024 * 1024 {
        return Err("剪贴板历史文件过大".into());
    }
    let history: History = serde_json::from_slice(&bytes).map_err(|e| e.to_string())?;
    if history.entries.len() > 500
        || history
            .pinned
            .iter()
            .any(|id| !history.entries.iter().any(|entry| &entry.id == id))
        || history.entries.iter().any(|e| {
            e.id.len() != 32
                || !e.id.bytes().all(|b| b.is_ascii_hexdigit())
                || e.text.len() > MAX_TEXT
                || e.files.as_ref().is_some_and(|paths| {
                    crate::files::validate(paths).is_err()
                        || e.image.is_some()
                        || !e.text.is_empty()
                })
                || e.image.as_ref().is_some_and(|image| {
                    image.width == 0
                        || image.height == 0
                        || image.width > mini_launcher::clipboard_image::MAX_DIMENSION
                        || image.height > mini_launcher::clipboard_image::MAX_DIMENSION
                        || image.bytes != u64::from(image.width) * u64::from(image.height) * 4
                        || image.bytes > mini_launcher::clipboard_image::MAX_BYTES as u64
                        || image.fingerprint.len() != 64
                        || !image.fingerprint.bytes().all(|b| b.is_ascii_hexdigit())
                })
        })
    {
        return Err("剪贴板历史内容无效".into());
    }
    Ok(history)
}

fn save(path: &Path, history: &History) -> Result<(), String> {
    let bytes = serde_json::to_vec(history).map_err(|e| e.to_string())?;
    fs::create_dir_all(path.parent().ok_or("历史路径无效")?).map_err(|e| e.to_string())?;
    let temporary = path.with_extension("tmp");
    let result = (|| {
        use std::io::Write;
        let mut file = fs::File::create(&temporary).map_err(|e| e.to_string())?;
        file.write_all(&bytes)
            .and_then(|_| file.sync_all())
            .map_err(|e| e.to_string())?;
        drop(file);
        let from: Vec<u16> = temporary.as_os_str().encode_wide().chain(Some(0)).collect();
        let to: Vec<u16> = path.as_os_str().encode_wide().chain(Some(0)).collect();
        if unsafe {
            MoveFileExW(
                from.as_ptr(),
                to.as_ptr(),
                MOVEFILE_REPLACE_EXISTING | MOVEFILE_WRITE_THROUGH,
            )
        } == 0
        {
            return Err(std::io::Error::last_os_error().to_string());
        }
        Ok(())
    })();
    if result.is_err() {
        let _ = fs::remove_file(temporary);
    }
    result
}

use std::os::windows::ffi::OsStrExt;

impl State {
    pub fn image_path(&self, id: &str, preview: bool) -> PathBuf {
        self.path
            .parent()
            .unwrap()
            .join("images")
            .join(format!("{id}{}.png", if preview { ".preview" } else { "" }))
    }

    pub fn commit(&mut self, mut history: History) -> Result<(), String> {
        let images: HashSet<String> = self
            .history
            .entries
            .iter()
            .chain(&history.entries)
            .filter(|entry| entry.image.is_some())
            .map(|entry| entry.id.clone())
            .collect();
        while history.entries.len() > self.maximum
            || history
                .entries
                .iter()
                .map(|e| {
                    e.text.len()
                        + e.files
                            .as_ref()
                            .map_or(0, |p| p.iter().map(String::len).sum::<usize>())
                })
                .sum::<usize>()
                > MAX_TOTAL
            || history
                .entries
                .iter()
                .filter_map(|e| e.image.as_ref())
                .map(|i| i.bytes)
                .sum::<u64>()
                > MAX_IMAGES
        {
            let oldest = history
                .entries
                .iter()
                .rposition(|entry| !history.pinned.contains(&entry.id))
                .unwrap_or(history.entries.len() - 1);
            history.entries.remove(oldest);
        }
        let retained: HashSet<String> = history
            .entries
            .iter()
            .map(|entry| entry.id.clone())
            .collect();
        history.pinned.retain(|id| retained.contains(id));
        let revision = random_id()?;
        save(&self.path, &history)?;
        for id in images {
            if !retained.contains(&id) {
                let _ = fs::remove_file(self.image_path(&id, false));
                let _ = fs::remove_file(self.image_path(&id, true));
            }
        }
        self.history = history;
        self.revision = revision;
        self.error = None;
        Ok(())
    }

    pub fn capture(&mut self, text: String) -> Result<(), String> {
        if self.history.paused
            || text.trim().is_empty()
            || text.len() > MAX_TEXT
            || self.history.entries.first().is_some_and(|e| e.text == text)
        {
            return Ok(());
        }
        let mut history = self.history.clone();
        // Keep actions from an already displayed row valid when a duplicate moves to the top.
        let id = match history.entries.iter().find(|e| e.text == text) {
            Some(entry) => entry.id.clone(),
            None => random_id()?,
        };
        history.entries.retain(|e| e.text != text);
        history.entries.insert(
            0,
            Entry {
                id,
                text,
                image: None,
                files: None,
                time: SystemTime::now()
                    .duration_since(UNIX_EPOCH)
                    .unwrap_or_default()
                    .as_secs(),
            },
        );
        self.commit(history)
    }

    pub fn capture_files(&mut self, paths: Vec<String>) -> Result<(), String> {
        if self.history.paused {
            return Ok(());
        }
        crate::files::validate(&paths)?;
        if self
            .history
            .entries
            .first()
            .is_some_and(|e| e.files.as_ref() == Some(&paths))
        {
            return Ok(());
        }
        let mut history = self.history.clone();
        let id = match history
            .entries
            .iter()
            .find(|e| e.files.as_ref() == Some(&paths))
        {
            Some(entry) => entry.id.clone(),
            None => random_id()?,
        };
        history.entries.retain(|e| e.files.as_ref() != Some(&paths));
        history.entries.insert(
            0,
            Entry {
                id,
                text: String::new(),
                image: None,
                files: Some(paths),
                time: SystemTime::now()
                    .duration_since(UNIX_EPOCH)
                    .unwrap_or_default()
                    .as_secs(),
            },
        );
        self.commit(history)
    }

    #[cfg(test)]
    pub fn capture_image(
        &mut self,
        image: mini_launcher::clipboard_image::Image,
    ) -> Result<(), String> {
        self.capture_image_encoded(image, None)
    }

    #[cfg(test)]
    pub fn capture_image_encoded(
        &mut self,
        image: mini_launcher::clipboard_image::Image,
        png: Option<&[u8]>,
    ) -> Result<(), String> {
        if self.history.paused {
            return Ok(());
        }
        let prepared = PreparedImage::new(image, png.map(|b| b.to_vec()))?;
        self.record_prepared_image(prepared)
    }

    pub fn record_prepared_image(&mut self, prepared: PreparedImage) -> Result<(), String> {
        if self.history.paused {
            return Ok(());
        }
        if self
            .history
            .entries
            .first()
            .and_then(|e| e.image.as_ref())
            .is_some_and(|i| i.fingerprint == prepared.fingerprint)
        {
            return Ok(());
        }
        let mut history = self.history.clone();
        if let Some(index) = history.entries.iter().position(|e| {
            e.image
                .as_ref()
                .is_some_and(|i| i.fingerprint == prepared.fingerprint)
        }) {
            let mut entry = history.entries.remove(index);
            entry.time = SystemTime::now()
                .duration_since(UNIX_EPOCH)
                .unwrap_or_default()
                .as_secs();
            history.entries.insert(0, entry);
            return self.commit(history);
        }
        let id = random_id()?;
        let path = self.image_path(&id, false);
        let preview = self.image_path(&id, true);
        fs::create_dir_all(path.parent().unwrap()).map_err(|e| e.to_string())?;
        let result = (|| {
            mini_launcher::clipboard_image::write_png_bytes(&path, &prepared.png)?;
            mini_launcher::clipboard_image::write_png_bytes(&preview, &prepared.preview_png)?;
            history.entries.insert(
                0,
                Entry {
                    id: id.clone(),
                    text: String::new(),
                    time: SystemTime::now()
                        .duration_since(UNIX_EPOCH)
                        .unwrap_or_default()
                        .as_secs(),
                    files: None,
                    image: Some(ImageInfo {
                        width: prepared.width,
                        height: prepared.height,
                        bytes: prepared.bytes,
                        fingerprint: prepared.fingerprint,
                    }),
                },
            );
            self.commit(history)
        })();
        if result.is_err() {
            let _ = fs::remove_file(path);
            let _ = fs::remove_file(preview);
        }
        result
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn plain_history_deduplicates_limits_and_pauses() {
        let root =
            std::env::temp_dir().join(format!("mini-clipboard-test-{}", random_id().unwrap()));
        let path = root.join("history.dat");
        let mut state = State {
            history: History::default(),
            path: path.clone(),
            maximum: 2,
            error: None,
            revision: random_id().unwrap(),
            window: 0,
        };
        state.capture("first secret 中文".into()).unwrap();
        state.capture("second".into()).unwrap();
        state.capture("first secret 中文".into()).unwrap();
        assert_eq!(state.history.entries.len(), 2);
        assert_eq!(load(&path).unwrap().entries[0].text, "first secret 中文");
        assert!(serde_json::from_slice::<History>(&fs::read(&path).unwrap()).is_ok());
        state.capture("third".into()).unwrap();
        assert_eq!(state.history.entries[1].text, "first secret 中文");
        state.history.paused = true;
        state.capture("ignored".into()).unwrap();
        assert_eq!(state.history.entries[0].text, "third");
        fs::write(&path, b"broken").unwrap();
        assert!(load(&path).is_err());
        fs::remove_dir_all(root).unwrap();
    }
}
