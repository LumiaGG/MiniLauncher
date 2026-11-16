//! Serializable drag payloads; resolving a plugin payload never executes its item action.
use serde::Deserialize;
use std::path::{Path, PathBuf};

#[derive(Clone, Debug, Deserialize)]
#[serde(tag = "type", rename_all = "snake_case", deny_unknown_fields)]
pub enum DragData {
    Text { text: String },
    Image { path: PathBuf },
    Files { paths: Vec<PathBuf> },
}

impl DragData {
    pub fn resolve(&mut self, directory: &Path) -> Result<(), String> {
        match self {
            Self::Text { text }
                if !text.is_empty() && text.len() <= 65536 && !text.contains('\0') =>
            {
                Ok(())
            }
            Self::Image { path } => resolve_path(path, directory),
            Self::Files { paths } if !paths.is_empty() && paths.len() <= 64 => {
                for path in paths {
                    resolve_path(path, directory)?;
                }
                Ok(())
            }
            _ => Err("拖动内容为空或超过长度限制".into()),
        }
    }
}

fn resolve_path(path: &mut PathBuf, directory: &Path) -> Result<(), String> {
    if path.as_os_str().is_empty() || path.to_string_lossy().contains('\0') {
        return Err("拖动路径无效".into());
    }
    *path = directory.join(&*path);
    if !path.is_absolute() || path.to_string_lossy().len() > 32768 {
        return Err("拖动路径必须为绝对路径且不超过 32768 字节".into());
    }
    Ok(())
}

#[derive(Clone, Debug)]
pub enum DragSource {
    Data(DragData),
    Plugin {
        plugin_id: String,
        action_id: String,
    },
}
