use crate::{
    plugin_catalog::Plugin,
    result::{Action, ContextAction, Entry},
};
use serde::Deserialize;
use serde_json::Value;
use std::{
    path::{Path, PathBuf},
    sync::Arc,
};

#[derive(Deserialize)]
#[serde(tag = "type", rename_all = "snake_case")]
pub(super) enum PluginAction {
    Open {
        target: String,
    },
    Launch {
        executable: String,
        args: Vec<String>,
    },
    Copy {
        text: String,
    },
    Execute {
        id: String,
    },
}
impl PluginAction {
    pub(super) fn into_action(self, plugin: &str) -> Result<Action, String> {
        match self {
            Self::Launch { executable, args }
                if !executable.is_empty()
                    && executable.len() <= 32768
                    && !executable.contains('\0')
                    && Path::new(&executable).is_absolute()
                    && args.len() <= 64
                    && args.iter().all(|arg| !arg.contains('\0'))
                    && args
                        .iter()
                        .map(|arg| arg.encode_utf16().count() * 2 + 3)
                        .sum::<usize>()
                        + executable.encode_utf16().count()
                        + 3
                        <= 32767 =>
            {
                Ok(Action::Launch { executable, args })
            }
            Self::Open { target }
                if !target.is_empty() && target.len() <= 32768 && !target.contains('\0') =>
            {
                Ok(Action::Open(target))
            }
            Self::Copy { text } if text.len() <= 65536 => Ok(Action::Copy(text)),
            Self::Execute { id } if !id.is_empty() && id.len() <= 256 => Ok(Action::Plugin {
                plugin_id: plugin.into(),
                action_id: id,
            }),
            _ => Err("插件动作为空或超过长度限制".into()),
        }
    }
}
#[derive(Deserialize)]
struct Item {
    #[serde(default)]
    id: Option<String>,
    title: String,
    #[serde(default)]
    subtitle: String,
    #[serde(default)]
    icon: Option<PathBuf>,
    action: PluginAction,
    #[serde(default)]
    context_actions: Vec<ItemContextAction>,
    #[serde(default)]
    adaptive_height: bool,
    #[serde(default)]
    text_panel: Option<crate::search::TextPanel>,
    #[serde(default)]
    preview: Option<crate::search::ImagePreview>,
    #[serde(default)]
    paste_after_execute: bool,
    #[serde(default)]
    action_label: Option<String>,
    #[serde(default)]
    drag_id: Option<String>,
}
#[derive(Deserialize)]
struct ItemContextAction {
    title: String,
    action: PluginAction,
    #[serde(default)]
    confirm: bool,
}
pub(super) fn parse_items(value: Value, plugin: &Plugin) -> Result<Vec<Entry>, String> {
    let _timing = crate::diagnostics::timing("plugin_parse_items", &plugin.manifest.id);
    #[derive(Deserialize)]
    struct QueryResult {
        items: Vec<Item>,
    }
    let array = serde_json::from_value::<QueryResult>(value)
        .map_err(|e| format!("query 结果无效：{e}"))?
        .items;
    if array.len() > 100 {
        return Err("插件一次最多返回 100 项".into());
    }
    let mut entries = Vec::with_capacity(array.len());
    let mut ids = std::collections::HashSet::new();
    for item in array {
        if item.title.is_empty() || item.title.len() > 1024 || item.subtitle.len() > 4096 {
            return Err("插件标题或说明长度无效".into());
        }
        let mut entry = Entry::new(
            item.title,
            item.subtitle,
            item.action.into_action(&plugin.manifest.id)?,
            "插件",
        );
        entry.query_priority = plugin.manifest.priority;
        if let Some(label) = item.action_label {
            if label.chars().any(char::is_control) {
                return Err("插件动作名称不能包含控制字符".into());
            }
            let label = label.trim();
            if label.is_empty() || label.chars().count() > 16 || label.chars().any(char::is_control) {
                return Err("插件动作名称须为 1–16 字且不能包含控制字符".into());
            }
            entry.action_label = Some(label.to_string());
        }
        entry.adaptive_height = item.adaptive_height;
        if let Some(id) = item.id {
            if id.is_empty()
                || id.len() > 128
                || id.chars().any(char::is_control)
                || !ids.insert(id.clone())
            {
                return Err("插件结果 id 为空、重复或超过 128 字节".into());
            }
            entry.key = format!("plugin:{}:item:{id}", plugin.manifest.id);
        }
        if let Some(panel) = item.text_panel {
            panel.validate()?;
            entry.text_panel = Some(Arc::new(panel));
        }
        if item.paste_after_execute
            && !matches!(entry.action, Action::Copy(_) | Action::Plugin { .. })
        {
            return Err("自动粘贴条目只支持 copy 或 execute 动作".into());
        }
        entry.paste_after_execute = item.paste_after_execute;
        if let Some(id) = item.drag_id {
            if id.is_empty() || id.len() > 256 || id.contains('\0') {
                return Err("插件拖动标识无效或超过 256 字节".into());
            }
            entry.drag = Some(crate::drag_data::DragSource::Plugin {
                plugin_id: plugin.manifest.id.clone(),
                action_id: id,
            });
        }
        if let Some(mut preview) = item.preview {
            if preview.path.is_empty()
                || preview.path.len() > 32768
                || preview.path.contains('\0')
                || preview.width == 0
                || preview.height == 0
                || preview.width > crate::clipboard_image::MAX_DIMENSION
                || preview.height > crate::clipboard_image::MAX_DIMENSION
            {
                return Err("插件图片预览路径或尺寸无效".into());
            }
            preview.path = plugin
                .directory
                .join(preview.path)
                .to_string_lossy()
                .into_owned();
            if let Some(path) = &mut preview.original_path {
                if path.is_empty() || path.len() > 32768 || path.contains('\0') {
                    return Err("插件原图预览路径无效".into());
                }
                *path = plugin.directory.join(&*path).to_string_lossy().into_owned();
            }
            entry.preview = Some(preview);
            entry.adaptive_height = true;
        }
        if item.context_actions.len() > 16 {
            return Err("插件每项最多提供 16 个右键菜单动作".into());
        }
        for context in item.context_actions {
            if context.title.trim().is_empty()
                || context.title.len() > 128
                || context.title.chars().any(char::is_control)
            {
                return Err("插件菜单标题无效或超过 128 字节".into());
            }
            entry.context_actions.push(ContextAction {
                title: context.title,
                action: context.action.into_action(&plugin.manifest.id)?,
                confirm: context.confirm,
            });
        }
        if let Some(icon) = item.icon {
            entry.icon_path = Some(plugin.directory.join(icon).to_string_lossy().into_owned());
        }
        entries.push(entry);
    }
    Ok(entries)
}
