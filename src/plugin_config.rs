//! Generic plugin registration and opaque per-plugin configuration.
use crate::config::{self, PluginSettings};
use serde::{Deserialize, Serialize};
use std::{fs, path::Path};

#[derive(Clone, Debug, PartialEq, Deserialize, Serialize)]
#[serde(default)]
pub struct Entry {
    pub enabled: bool,
    pub keywords: Option<Vec<String>>,
    pub direct_display: Option<bool>,
    pub priority: Option<i32>,
    #[serde(flatten)]
    pub options: toml::Table,
}
impl Default for Entry {
    fn default() -> Self {
        Self {
            enabled: true,
            keywords: None,
            direct_display: None,
            priority: None,
            options: toml::Table::new(),
        }
    }
}
impl Entry {
    pub fn validate(&mut self) -> Result<(), String> {
        if let Some(keywords) = &mut self.keywords {
            if keywords.is_empty() {
                return Err("插件 keywords 至少包含一个关键词".into());
            }
            for keyword in keywords.iter_mut() {
                *keyword = keyword.trim().to_ascii_lowercase();
                if keyword.is_empty()
                    || keyword.len() > 32
                    || keyword.chars().any(|c| c.is_whitespace() || c.is_control())
                    || matches!(keyword.as_str(), "g" | "bing" | "e")
                    || keyword.starts_with('>')
                {
                    return Err("插件 keywords 无效或与内置命令冲突".into());
                }
            }
            let mut seen = std::collections::HashSet::new();
            keywords.retain(|keyword| seen.insert(keyword.clone()));
        }
        Ok(())
    }
}

pub fn key(id: &str) -> String {
    id.replace('-', "_")
}
fn json(path: &Path) -> Result<serde_json::Value, String> {
    let text = fs::read_to_string(path).map_err(|e| format!("{}：{e}", path.display()))?;
    serde_json::from_str(text.trim_start_matches('\u{feff}'))
        .map_err(|e| format!("{}：{e}", path.display()))
}
fn configuration_key(manifest: &serde_json::Value) -> Result<String, String> {
    let id = manifest["id"].as_str().ok_or("插件缺少 id")?;
    if id.is_empty()
        || id.len() > 64
        || !id
            .bytes()
            .all(|c| c.is_ascii_alphanumeric() || c == b'_' || c == b'-')
    {
        return Err("插件 id 无效".into());
    }
    let key = manifest["config_key"]
        .as_str()
        .map(str::to_owned)
        .unwrap_or_else(|| key(id));
    if key.is_empty()
        || !key
            .bytes()
            .all(|c| c.is_ascii_alphanumeric() || c == b'_' || c == b'-')
        || matches!(key.as_str(), "enabled" | "directories" | "max_processes")
    {
        return Err("插件 config_key 无效或与宿主字段冲突".into());
    }
    Ok(key)
}

fn ensure_entry(
    document: &mut toml_edit::DocumentMut,
    directory: &Path,
    manifest: &serde_json::Value,
) -> Result<bool, String> {
    let key = configuration_key(manifest)?;
    let mut defaults = if let Some(file) = manifest["config_defaults"].as_str() {
        let path = Path::new(file);
        if path.is_absolute()
            || path
                .components()
                .any(|part| matches!(part, std::path::Component::ParentDir))
        {
            return Err("config_defaults 必须位于插件目录内".into());
        }
        fs::read_to_string(directory.join(path))
            .map_err(|e| e.to_string())?
            .parse::<toml_edit::DocumentMut>()
            .map_err(|e| e.to_string())?
    } else {
        toml_edit::DocumentMut::new()
    };
    if !defaults.contains_key("enabled") {
        defaults["enabled"] = toml_edit::value(true);
    }
    // A new registration inherits the installed manifest's custom keywords.
    let mut keywords = toml_edit::Array::new();
    for keyword in manifest["keywords"].as_array().ok_or("插件缺少 keywords")? {
        keywords.push(keyword.as_str().ok_or("插件关键词必须是字符串")?);
    }
    defaults["keywords"] = toml_edit::value(keywords);
    let mut entry: Entry = toml::from_str(&defaults.to_string()).map_err(|e| e.to_string())?;
    entry.validate()?;
    if let Some(table) = document
        .get("plugins")
        .and_then(toml_edit::Item::as_inline_table)
    {
        document["plugins"] = toml_edit::Item::Table(table.clone().into_table());
    }
    if document.get("plugins").is_none() {
        document["plugins"] = toml_edit::Item::Table(toml_edit::Table::new());
    }
    if document["plugins"].get(&key).is_none() {
        document["plugins"][&key] = toml_edit::Item::Table(defaults.as_table().clone());
        return Ok(true);
    }
    let target = &mut document["plugins"][&key];
    let comments = !target.is_inline_table();
    Ok(config::fill_missing(
        target.as_table_like_mut().ok_or("插件配置必须为表")?,
        defaults.as_table(),
        comments,
    ))
}

pub fn complete(
    settings: &PluginSettings,
    document: &mut toml_edit::DocumentMut,
) -> Result<bool, String> {
    let roots = if settings.directories.is_empty() {
        vec![config::data_dir().join("plugins")]
    } else {
        settings.directories.clone()
    };
    let mut candidates = Vec::new();
    for root in roots {
        candidates.push(root.clone());
        if let Ok(children) = fs::read_dir(root) {
            candidates.extend(
                children
                    .take(256)
                    .flatten()
                    .filter(|entry| entry.file_type().is_ok_and(|kind| kind.is_dir()))
                    .map(|entry| entry.path()),
            );
        }
    }
    candidates.sort();
    candidates.dedup();
    let mut changed = false;
    for directory in candidates {
        let Ok(manifest) = json(&directory.join("plugin.json")) else {
            continue;
        };
        if manifest.get("config_defaults").is_some() {
            // A broken optional plugin must not prevent the launcher from loading.
            match ensure_entry(document, &directory, &manifest) {
                Ok(added) => changed |= added,
                Err(error) => eprintln!("插件配置未补齐：{error}"),
            }
        }
    }
    Ok(changed)
}

pub fn register(directory: &Path) -> Result<(), String> {
    config::load()?;
    let path = config::settings_path();
    let previous = fs::read_to_string(&path).map_err(|e| e.to_string())?;
    let mut document = previous
        .parse::<toml_edit::DocumentMut>()
        .map_err(|e| e.to_string())?;
    register_in_document(directory, &mut document)?;
    let completed = document.to_string();
    config::parse_and_complete(&completed)?;
    if completed != previous {
        config::save_settings(&path, &previous, &completed)?;
    }
    Ok(())
}

/// Register into an editor's draft without changing the settings file.
pub fn register_in_document(
    directory: &Path,
    document: &mut toml_edit::DocumentMut,
) -> Result<(), String> {
    let directory = fs::canonicalize(directory).map_err(|e| e.to_string())?;
    let manifest = json(&directory.join("plugin.json"))?;
    configuration_key(&manifest)?;
    if !manifest["executable"]
        .as_str()
        .is_some_and(|name| directory.join(name).is_file())
    {
        return Err("插件程序不存在".into());
    }
    let settings: config::Settings =
        toml::from_str(&document.to_string()).map_err(|e| e.to_string())?;
    let mut directories = settings.plugins.directories.clone();
    if directories.is_empty() {
        directories.push(config::data_dir().join("plugins"));
    }
    let discovered = directories.iter().any(|root| {
        fs::canonicalize(root)
            .is_ok_and(|root| directory == root || directory.parent() == Some(root.as_path()))
    });
    if !discovered {
        if let Some(table) = document
            .get("plugins")
            .and_then(toml_edit::Item::as_inline_table)
        {
            document["plugins"] = toml_edit::Item::Table(table.clone().into_table());
        }
        let mut list = toml_edit::Array::new();
        for root in directories.iter().chain(std::iter::once(&directory)) {
            list.push(root.to_string_lossy().as_ref());
        }
        document["plugins"]["directories"] = toml_edit::value(list);
    }
    ensure_entry(document, &directory, &manifest)?;
    config::organize_settings(document);
    Ok(())
}
