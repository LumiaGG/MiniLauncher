use crate::history::State;
use serde_json::{json, Value};
fn preview(text: &str, maximum: usize) -> String {
    if maximum == 0 {
        return String::new();
    }
    let mut out = String::with_capacity(maximum + 4);
    let mut chars = 0usize;
    for word in text.split_whitespace() {
        if chars > 0 {
            if chars == maximum {
                out.push('…');
                return out;
            }
            out.push(' ');
            chars += 1;
        }
        for ch in word.chars() {
            if chars == maximum {
                out.push('…');
                return out;
            }
            out.push(ch);
            chars += 1;
        }
    }
    out
}

fn build_item(state: &State, entry: &crate::history::Entry, pinned: bool) -> Value {
    let (content, icon) = if let Some(paths) = &entry.files {
        let name = std::path::Path::new(&paths[0])
            .file_name()
            .unwrap_or_else(|| std::ffi::OsStr::new(&paths[0]))
            .to_string_lossy();
        (
            format!("{} · 共 {} 项", preview(&name, 100), paths.len()),
            "Images/files.ico",
        )
    } else if entry.image.is_some() {
        ("图片".to_string(), "Images/image.ico")
    } else {
        (preview(&entry.text, 180), "Images/text.ico")
    };
    let mut details = Vec::new();
    if let Some(paths) = &entry.files {
        details.push(preview(&paths.join(" · "), 240));
    }
    if state.history.paused {
        details.push("已暂停".to_string());
    }
    details.push(timestamp(entry.time));
    let mut item = json!({
        "title":format!("{}{content}", if pinned { "置顶 · " } else { "" }),
        "subtitle":details.join(" · "),
        "icon":icon,
        "action":{"type":"execute","id":format!("copy:{}",entry.id)},
        "adaptive_height":entry.image.is_some(),
        "paste_after_execute":true,
        "drag_id":entry.id,
        "context_actions":[
            {"title":if pinned {"取消置顶"} else {"置顶"},"action":{"type":"execute","id":format!("{}:{}",if pinned {"unpin"} else {"pin"},entry.id)}},
            {"title":"仅复制","action":{"type":"execute","id":format!("copy:{}",entry.id)}},
            {"title":"删除此条历史","action":{"type":"execute","id":format!("delete:{}",entry.id)}},
            {"title":"清空历史（含置顶）","confirm":true,"action":{"type":"execute","id":format!("clear:{}",state.revision)}}
        ]
    });
    if let Some(image) = &entry.image {
        item["preview"] = json!({"path":state.image_path(&entry.id, true), "original_path":state.image_path(&entry.id, false), "width":image.width,"height":image.height});
    }
    item
}

pub(super) fn query(state: &State, text: &str, maximum: usize) -> Vec<Value> {
    let text = text.trim();
    let command = match text {
        "/pause" => Some(("暂停记录剪贴板", "pause".to_string())),
        "/resume" => Some(("恢复记录剪贴板", "resume".to_string())),
        "/clear" => Some((
            "确认清空全部剪贴板历史",
            format!("clear:{}", state.revision),
        )),
        _ => None,
    };
    let mut items = Vec::new();
    if let Some((title, id)) = command {
        items.push(json!({"title":title,"subtitle":"按 Enter 确认 · 清空历史不会清空系统剪贴板","action":{"type":"execute","id":id}}));
    } else {
        let limit = maximum.min(100);
        if limit > 0 {
            let words: Vec<String> = text.split_whitespace().map(str::to_lowercase).collect();
            let mut pinned_items = Vec::new();
            let mut unpinned_items = Vec::new();
            let mut remaining_pins = state.history.pinned.len();
            for entry in &state.history.entries {
                let pinned = state.history.pinned.contains(&entry.id);
                if pinned {
                    remaining_pins = remaining_pins.saturating_sub(1);
                } else if unpinned_items.len() + pinned_items.len() >= limit && remaining_pins == 0
                {
                    break;
                }
                if !words.is_empty() {
                    let lower = if let Some(paths) = &entry.files {
                        format!("文件 文件夹 file folder {}", paths.join(" ")).to_lowercase()
                    } else {
                        match &entry.image {
                            Some(image) => format!(
                                "图片 截图 image screenshot {}x{}",
                                image.width, image.height
                            ),
                            None => entry.text.to_lowercase(),
                        }
                    };
                    if !words.iter().all(|w| lower.contains(w)) {
                        continue;
                    }
                }
                if pinned {
                    pinned_items.push(build_item(state, entry, true));
                    if pinned_items.len() >= limit {
                        break;
                    }
                } else if unpinned_items.len() < limit {
                    unpinned_items.push(build_item(state, entry, false));
                }
            }
            items = pinned_items;
            let needed = limit.saturating_sub(items.len());
            items.extend(unpinned_items.into_iter().take(needed));
        }
        if items.is_empty() && maximum > 0 {
            items.push(json!({"title": if state.history.paused { "剪贴板记录已暂停" } else { "没有匹配的剪贴板历史" }, "subtitle":"文本、图片和文件 · /pause 暂停 · /resume 恢复 · /clear 清空", "action":{"type":"execute","id":if state.history.paused {"resume"} else {"noop"}}}));
        }
    }
    if let Some(error) = &state.error {
        items.insert(0, json!({"title":"剪贴板历史保存失败", "subtitle":preview(error, 300), "action":{"type":"execute","id":"noop"}}));
    }
    items.truncate(maximum.min(100));
    items
}

pub(super) fn timestamp(seconds: u64) -> String {
    use windows_sys::Win32::{Foundation::*, System::Time::*};
    let ticks = seconds
        .saturating_add(11_644_473_600)
        .saturating_mul(10_000_000);
    let file = FILETIME {
        dwLowDateTime: ticks as u32,
        dwHighDateTime: (ticks >> 32) as u32,
    };
    let mut utc = SYSTEMTIME::default();
    let mut local = SYSTEMTIME::default();
    unsafe {
        if FileTimeToSystemTime(&file, &mut utc) == 0
            || SystemTimeToTzSpecificLocalTime(std::ptr::null(), &utc, &mut local) == 0
        {
            return seconds.to_string();
        }
    }
    format!(
        "{:04}-{:02}-{:02} {:02}:{:02}",
        local.wYear, local.wMonth, local.wDay, local.wHour, local.wMinute
    )
}

pub(super) fn execute(state: &mut State, id: &str) -> Result<Value, String> {
    if id == "noop" {
        return Ok(json!({}));
    }
    if let Some(id) = id.strip_prefix("copy:") {
        let entry = state
            .history
            .entries
            .iter()
            .find(|e| e.id == id)
            .ok_or("历史已删除，请重新搜索")?;
        if let Some(paths) = &entry.files {
            unsafe {
                crate::files::copy(paths, state.window as _)?;
            }
            return Ok(json!({}));
        }
        if entry.image.is_some() {
            let image =
                mini_launcher::image_cache::asset(&state.image_path(&entry.id, false))?.image()?;
            unsafe {
                image.copy_to_clipboard(state.window as _)?;
            }
            return Ok(json!({}));
        }
        return Ok(json!({"action":{"type":"copy","text":entry.text}}));
    }
    let mut history = state.history.clone();
    if let Some((pinned, id)) = id
        .strip_prefix("pin:")
        .map(|id| (true, id))
        .or_else(|| id.strip_prefix("unpin:").map(|id| (false, id)))
    {
        if !history.entries.iter().any(|entry| entry.id == id) {
            return Err("历史已删除，请重新搜索".into());
        }
        if pinned {
            history.pinned.insert(id.into());
        } else {
            history.pinned.remove(id);
        }
    } else if let Some(id) = id.strip_prefix("delete:") {
        if !history.entries.iter().any(|e| e.id == id) {
            return Err("历史已删除，请重新搜索".into());
        }
        history.entries.retain(|e| e.id != id);
    } else if let Some(revision) = id.strip_prefix("clear:") {
        if revision != state.revision {
            return Err("历史已变化，请重新确认清空".into());
        }
        history.entries.clear();
    } else {
        match id {
            "pause" => history.paused = true,
            "resume" => history.paused = false,
            _ => return Err("未知剪贴板操作".into()),
        }
    }
    state.commit(history)?;
    Ok(json!({}))
}

pub(super) fn prepare_drag(state: &State, id: &str) -> Result<Value, String> {
    let entry = state
        .history
        .entries
        .iter()
        .find(|entry| entry.id == id)
        .ok_or("历史已删除，请重新搜索")?;
    if let Some(paths) = &entry.files {
        crate::files::available(paths)?;
        Ok(json!({"data":{"type":"files","paths":paths}}))
    } else if entry.image.is_some() {
        Ok(json!({"data":{"type":"image","path":state.image_path(id,false)}}))
    } else {
        Ok(json!({"data":{"type":"text","text":entry.text}}))
    }
}
