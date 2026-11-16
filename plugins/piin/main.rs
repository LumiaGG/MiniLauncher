//! Explicitly saved notes; no clipboard monitoring and no automatic eviction.
use mini_launcher::clipboard_image::{self, Image};
use serde::{Deserialize, Serialize};
use serde_json::{json, Value};
use std::{
    fs,
    io::{self, Read, Write},
    os::windows::{ffi::OsStrExt, fs::OpenOptionsExt},
    path::{Path, PathBuf},
    sync::Arc,
};
use windows_sys::Win32::{Security::Cryptography::*, Storage::FileSystem::*};

const MAX_TEXT: usize = 16 * 1024;
const MAX_NOTES: usize = 500;
const MAX_TOTAL_TEXT: usize = 2 * 1024 * 1024;
const MAX_IMAGE_BYTES: u64 = 128 * 1024 * 1024;

#[derive(Clone, Serialize, Deserialize)]
struct Note {
    id: String,
    text: String,
    pinned: bool,
    image: Option<ImageInfo>,
}
#[derive(Clone, Serialize, Deserialize)]
struct ImageInfo {
    width: u32,
    height: u32,
}
struct Draft {
    id: String,
    text: String,
    image: Option<Arc<Image>>,
    source: Option<PathBuf>,
}
struct Store {
    directory: PathBuf,
    notes: Vec<Note>,
    draft: Option<Draft>,
    clipboard_window: isize,
    _lock: fs::File,
}
fn random_id() -> Result<String, String> {
    let mut bytes = [0; 16];
    if unsafe {
        BCryptGenRandom(
            std::ptr::null_mut(),
            bytes.as_mut_ptr(),
            16,
            BCRYPT_USE_SYSTEM_PREFERRED_RNG,
        )
    } < 0
    {
        return Err("无法生成备注标识".into());
    }
    Ok(bytes.iter().map(|b| format!("{b:02x}")).collect())
}
fn preview(text: &str) -> String {
    text.split_whitespace()
        .collect::<Vec<_>>()
        .join(" ")
        .chars()
        .take(170)
        .collect()
}
fn valid_id(id: &str) -> bool {
    id.len() == 32 && id.bytes().all(|c| c.is_ascii_hexdigit())
}
fn validate(notes: &[Note]) -> Result<(), String> {
    let mut ids = std::collections::HashSet::new();
    if notes.len() > MAX_NOTES
        || notes.iter().map(|n| n.text.len()).sum::<usize>() > MAX_TOTAL_TEXT
        || notes
            .iter()
            .filter_map(|n| n.image.as_ref())
            .map(|i| u128::from(i.width) * u128::from(i.height) * 4)
            .sum::<u128>()
            > MAX_IMAGE_BYTES as u128
        || notes.iter().any(|n| {
            !valid_id(&n.id)
                || !ids.insert(&n.id)
                || n.text.len() > MAX_TEXT
                || n.text.contains('\0')
                || (n.text.trim().is_empty() && n.image.is_none())
                || n.image.as_ref().is_some_and(|i| {
                    i.width == 0
                        || i.height == 0
                        || i.width > clipboard_image::MAX_DIMENSION
                        || i.height > clipboard_image::MAX_DIMENSION
                        || u128::from(i.width) * u128::from(i.height) * 4
                            > clipboard_image::MAX_BYTES as u128
                })
        })
    {
        return Err("备注容量或内容超限：最多 500 条、单条文字 16 KiB、文字总量 2 MiB、图片像素总量 128 MiB；请先删除不需要的备注".into());
    }
    Ok(())
}
impl Store {
    fn open(directory: PathBuf) -> Result<Self, String> {
        fs::create_dir_all(directory.join("images")).map_err(|e| e.to_string())?;
        let lock = fs::OpenOptions::new()
            .write(true)
            .create(true)
            .truncate(false)
            .share_mode(0)
            .open(directory.join("notes.lock"))
            .map_err(|_| "备注数据正被另一实例使用，或目录不可写")?;
        let notes: Vec<Note> = match fs::File::open(directory.join("notes.json")) {
            Ok(file) => {
                let mut bytes = Vec::new();
                file.take(16 * 1024 * 1024 + 1)
                    .read_to_end(&mut bytes)
                    .map_err(|e| e.to_string())?;
                if bytes.len() > 16 * 1024 * 1024 {
                    return Err("备注文件过大".into());
                }
                serde_json::from_slice(&bytes).map_err(|e| format!("读取备注失败：{e}"))?
            }
            Err(e) if e.kind() == io::ErrorKind::NotFound => Vec::new(),
            Err(e) => return Err(e.to_string()),
        };
        validate(&notes)?;
        Ok(Self {
            directory,
            notes,
            draft: None,
            clipboard_window: 0,
            _lock: lock,
        })
    }
    fn image_path(&self, id: &str, thumb: bool) -> PathBuf {
        self.directory
            .join("images")
            .join(format!("{id}{}.png", if thumb { ".preview" } else { "" }))
    }
    fn commit(&mut self, notes: Vec<Note>) -> Result<(), String> {
        validate(&notes)?;
        let bytes = serde_json::to_vec(&notes).map_err(|e| e.to_string())?;
        let temp = self.directory.join("notes.tmp");
        let result = (|| {
            let mut file = fs::File::create(&temp).map_err(|e| e.to_string())?;
            file.write_all(&bytes)
                .and_then(|_| file.sync_all())
                .map_err(|e| e.to_string())?;
            drop(file);
            let from: Vec<u16> = temp.as_os_str().encode_wide().chain(Some(0)).collect();
            let to: Vec<u16> = self
                .directory
                .join("notes.json")
                .as_os_str()
                .encode_wide()
                .chain(Some(0))
                .collect();
            if unsafe {
                MoveFileExW(
                    from.as_ptr(),
                    to.as_ptr(),
                    MOVEFILE_REPLACE_EXISTING | MOVEFILE_WRITE_THROUGH,
                )
            } == 0
            {
                return Err(io::Error::last_os_error().to_string());
            }
            Ok(())
        })();
        if result.is_err() {
            let _ = fs::remove_file(temp);
        }
        result?;
        self.notes = notes;
        Ok(())
    }
    fn query(&mut self, params: &Value) -> Result<Value, String> {
        let previous = self.draft.take();
        let query = params["query"].as_str().unwrap_or("");
        let text = query.trim();
        let add = text == "/add" || text.starts_with("/add ") || text.starts_with("/add\n");
        let content = if add {
            text.strip_prefix("/add").unwrap().trim_start()
        } else {
            query
        };
        let attachments = params["attachments"]
            .as_array()
            .map(Vec::as_slice)
            .unwrap_or(&[]);
        if attachments.len() > 1 {
            return Err("一条备注支持一张图片，请分别保存".into());
        }
        let mut items = Vec::new();
        if !content.trim().is_empty() || !attachments.is_empty() {
            if content.len() > MAX_TEXT || content.contains('\0') {
                return Err("单条备注文字最多 16 KiB，且不能包含空字符".into());
            }
            let image = if let Some(input) = attachments.first() {
                if input["type"] != "image" {
                    return Err("不支持的附件类型".into());
                }
                let path = Path::new(input["path"].as_str().ok_or("图片路径无效")?);
                if let Some(cached) = previous
                    .as_ref()
                    .filter(|draft| draft.source.as_deref() == Some(path))
                {
                    cached.image.clone()
                } else {
                    Some(Arc::new(clipboard_image::decode(
                        &clipboard_image::read_encoded(path)?,
                        false,
                    )?))
                }
            } else {
                None
            };
            let id = random_id()?;
            let title = if image.is_some() {
                format!("保存图片备注 · {}", preview(content))
            } else {
                format!("保存备注 · {}", preview(content))
            };
            // Keep the draft isolated from later clipboard changes and reject stale actions.
            self.draft = Some(Draft {
                id: id.clone(),
                text: content.into(),
                image,
                source: attachments
                    .first()
                    .and_then(|i| i["path"].as_str())
                    .map(PathBuf::from),
            });
            items.push(json!({"id":"draft","title":title,"subtitle":"Enter 保存 · 备注永久保留，容量满时拒绝新增","action":{"type":"execute","id":format!("save:{id}")}}));
        }
        if !add && attachments.is_empty() {
            let words: Vec<_> = text.split_whitespace().map(str::to_lowercase).collect();
            let mut matches: Vec<_> = self
                .notes
                .iter()
                .filter(|note| {
                    let haystack = format!(
                        "{} {}",
                        note.text.to_lowercase(),
                        if note.image.is_some() {
                            "图片 截图 image screenshot"
                        } else {
                            ""
                        }
                    );
                    words.iter().all(|w| haystack.contains(w))
                })
                .collect();
            matches.sort_by_key(|n| !n.pinned);
            let mut saved: Vec<_> = matches.into_iter().map(|note| self.item(note)).collect();
            // Existing matches remain the default; /add explicitly selects creation.
            saved.append(&mut items);
            items = saved;
        }
        if items.is_empty() {
            items.push(json!({"title":"还没有备注","subtitle":"Pin Ideas, Images & Notes · 输入文字或粘贴图片，按 Enter 保存 · /add 内容 强制新增","action":{"type":"execute","id":"noop"}}));
        }
        items.truncate(params["max_results"].as_u64().unwrap_or(100).min(100) as usize);
        Ok(json!({"items":items}))
    }
    fn item(&self, note: &Note) -> Value {
        let mut item = json!({"id":note.id,"title":format!("{}{}",if note.pinned {"置顶 · "} else {""}, if note.text.is_empty() { "图片备注".into() } else { preview(&note.text) }),
            "subtitle":"Enter 复制并粘贴 · 右键管理","adaptive_height":true,
            "action":{"type":"execute","id":format!("copy:{}",note.id)},"paste_after_execute":true,"drag_id":note.id,
            "context_actions":[
                {"title":if note.pinned {"取消置顶"} else {"置顶"},"action":{"type":"execute","id":format!("pin:{}",note.id)}},
                {"title":"仅复制","action":{"type":"execute","id":format!("copy:{}",note.id)}},
                {"title":"删除备注","confirm":true,"action":{"type":"execute","id":format!("delete:{}",note.id)}}]});
        if let Some(image) = &note.image {
            item["preview"] = json!({"path":self.image_path(&note.id,true),"original_path":self.image_path(&note.id,false),"width":image.width,"height":image.height});
            if !note.text.is_empty() {
                item["context_actions"].as_array_mut().unwrap().insert(
                    2,
                    json!({"title":"复制备注文字","action":{"type":"execute","id":format!("text:{}",note.id)}}),
                );
            }
        }
        item
    }
    fn execute(&mut self, action: &str) -> Result<Value, String> {
        if action == "noop" {
            return Ok(json!({}));
        }
        let (verb, id) = action.split_once(':').ok_or("未知备注操作")?;
        if verb == "save" {
            let draft = self
                .draft
                .as_ref()
                .filter(|d| d.id == id)
                .ok_or("输入已变化，请重新确认保存")?;
            let note = Note {
                id: draft.id.clone(),
                text: draft.text.clone(),
                pinned: false,
                image: draft.image.as_ref().map(|i| ImageInfo {
                    width: i.width,
                    height: i.height,
                }),
            };
            let mut notes = self.notes.clone();
            notes.insert(0, note);
            validate(&notes)?;
            let original = self.image_path(id, false);
            let thumb = self.image_path(id, true);
            let result = (|| {
                if let Some(image) = &draft.image {
                    image.write_png(&original)?;
                    image.thumbnail(640, 120)?.write_png(&thumb)?;
                }
                Ok::<(), String>(())
            })()
            .and_then(|_| self.commit(notes));
            if result.is_err() {
                let _ = fs::remove_file(original);
                let _ = fs::remove_file(thumb);
            }
            result?;
            self.draft = None;
            return Ok(json!({}));
        }
        let index = self
            .notes
            .iter()
            .position(|n| n.id == id)
            .ok_or("备注已删除，请重新搜索")?;
        match verb {
            "text" => Ok(json!({"action":{"type":"copy","text":self.notes[index].text}})),
            "copy" => {
                let note = &self.notes[index];
                if note.image.is_some() {
                    let image =
                        mini_launcher::image_cache::asset(&self.image_path(id, false))?.image()?;
                    unsafe {
                        use windows_sys::Win32::UI::WindowsAndMessaging::*;
                        if self.clipboard_window == 0 {
                            let class: Vec<u16> = "STATIC".encode_utf16().chain(Some(0)).collect();
                            self.clipboard_window = CreateWindowExW(
                                0,
                                class.as_ptr(),
                                std::ptr::null(),
                                0,
                                0,
                                0,
                                0,
                                0,
                                HWND_MESSAGE,
                                std::ptr::null_mut(),
                                std::ptr::null_mut(),
                                std::ptr::null(),
                            ) as isize;
                            if self.clipboard_window == 0 {
                                return Err("无法创建剪贴板所有者窗口".into());
                            }
                        }
                        image.copy_to_clipboard(self.clipboard_window as _)?;
                    }
                    Ok(json!({}))
                } else {
                    Ok(json!({"action":{"type":"copy","text":note.text}}))
                }
            }
            "pin" | "delete" => {
                let mut notes = self.notes.clone();
                if verb == "pin" {
                    notes[index].pinned = !notes[index].pinned;
                } else {
                    notes.remove(index);
                }
                self.commit(notes)?;
                if verb == "delete" {
                    let _ = fs::remove_file(self.image_path(id, false));
                    let _ = fs::remove_file(self.image_path(id, true));
                }
                Ok(json!({}))
            }
            _ => Err("未知备注操作".into()),
        }
    }
    fn prepare_drag(&self, id: &str) -> Result<Value, String> {
        let note = self
            .notes
            .iter()
            .find(|n| n.id == id)
            .ok_or("备注已删除，请重新搜索")?;
        if note.image.is_some() {
            Ok(json!({"data":{"type":"image","path":self.image_path(id,false)}}))
        } else {
            Ok(json!({"data":{"type":"text","text":note.text}}))
        }
    }
}
impl Drop for Store {
    fn drop(&mut self) {
        if self.clipboard_window != 0 {
            unsafe {
                windows_sys::Win32::UI::WindowsAndMessaging::DestroyWindow(
                    self.clipboard_window as _,
                );
            }
        }
    }
}
fn main() {
    let mut store: Option<Store> = None;
    let result = mini_launcher::plugin_rpc::serve(
        &mut io::stdin().lock(),
        &mut io::stdout().lock(),
        |request| {
            let params = &request["params"];
            match request["method"].as_str().unwrap_or("") {
                "initialize" if store.is_none() => {
                    let directory = params["settings_path"]
                        .as_str()
                        .and_then(|s| Path::new(s).parent())
                        .map(Path::to_path_buf)
                        .unwrap_or_else(mini_launcher::config::data_dir)
                        .join("piin");
                    store = Some(Store::open(directory)?);
                    Ok(json!({"protocol_version":2}))
                }
                method => {
                    let state = store.as_mut().ok_or("请先初始化插件")?;
                    match method {
                        "query" => state.query(params),
                        "execute" => state.execute(params["action_id"].as_str().unwrap_or("")),
                        "prepare_drag" => {
                            state.prepare_drag(params["action_id"].as_str().unwrap_or(""))
                        }
                        _ => Err("不支持的方法".into()),
                    }
                }
            }
        },
    );
    if let Err(error) = result {
        eprintln!("{error}");
        std::process::exit(1);
    }
}
#[cfg(test)]
mod tests;
