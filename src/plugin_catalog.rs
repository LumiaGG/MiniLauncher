//! Read-only plugin manifests and catalog. Never starts a plugin process.
use crate::{
    config::{self, PluginSettings},
    hotkey::{PluginShortcut, Prefill},
    plugin_trigger::GlobalTrigger,
};
use serde::Deserialize;
use std::{
    fs,
    io::Read,
    path::{Path, PathBuf},
};
const MAX_PLUGINS: usize = 32;
#[derive(Clone, Deserialize)]
#[serde(deny_unknown_fields)]
pub(crate) struct Manifest {
    pub(crate) id: String,
    pub(crate) name: String,
    pub(crate) protocol_version: u32,
    pub(crate) executable: PathBuf,
    #[serde(default)]
    pub(crate) args: Vec<String>,
    pub(crate) keywords: Vec<String>,
    #[serde(default)]
    pub(crate) priority: i32,
    #[serde(default)]
    pub(crate) global_trigger: GlobalTrigger,
    #[serde(default = "default_timeout")]
    pub(crate) timeout_ms: u64,
    #[serde(default = "default_idle")]
    pub(crate) idle_timeout_secs: u64,
    #[serde(default)]
    pub(crate) resident: bool,
    #[serde(default)]
    pub(crate) direct_display: bool,
    #[serde(default)]
    pub(crate) accepts_images: bool,
    #[serde(default)]
    pub(crate) config_key: Option<String>,
    #[serde(default)]
    pub(crate) hotkeys: Vec<HotkeySpec>,
    #[serde(default)]
    pub(crate) config_defaults: Option<PathBuf>,
    #[serde(default)]
    pub(crate) settings_extension: Option<SettingsExtension>,
}

#[derive(Clone, Deserialize, Debug, PartialEq, Eq)]
#[serde(tag = "type", rename_all = "snake_case")]
pub enum SettingsExtension {
    TwoFactorAccounts {
        #[serde(default = "default_admin_args")]
        admin_args: Vec<String>,
    },
    External {
        command: String,
        #[serde(default)]
        args: Vec<String>,
        #[serde(default)]
        label: String,
    },
}

fn default_admin_args() -> Vec<String> {
    vec!["--admin".into()]
}

/// A global shortcut declared by a plugin. The user overrides `default` with
/// the plugin option `hotkey_<id>` (empty disables it).
#[derive(Clone, Deserialize)]
#[serde(deny_unknown_fields)]
pub(crate) struct HotkeySpec {
    pub(crate) id: String,
    pub(crate) default: String,
    #[serde(default)]
    pub(crate) prefill: Prefill,
}

const MAX_HOTKEYS: usize = 8;

impl HotkeySpec {
    fn option(&self) -> String {
        format!("hotkey_{}", self.id)
    }
    fn resolve(&self, value: Option<&toml::Value>) -> Result<Option<crate::hotkey::Hotkey>, String> {
        let text = match value {
            Some(value) => value
                .as_str()
                .ok_or_else(|| format!("{} 必须是字符串", self.option()))?,
            None => &self.default,
        };
        if text.trim().is_empty() {
            return Ok(None);
        }
        let parsed = crate::hotkey::Hotkey::parse(text)?;
        if parsed.modifiers == 0 && !parsed.double_tap {
            return Err("插件全局快捷键必须包含修饰键或使用同键双击".into());
        }
        Ok(Some(parsed))
    }
}

fn validate_hotkeys(specs: &[HotkeySpec]) -> Result<(), String> {
    if specs.len() > MAX_HOTKEYS {
        return Err(format!("插件最多声明 {MAX_HOTKEYS} 个全局快捷键"));
    }
    for (index, spec) in specs.iter().enumerate() {
        if spec.id.is_empty()
            || spec.id.len() > 32
            || !spec
                .id
                .bytes()
                .all(|b| b.is_ascii_alphanumeric() || b == b'_')
            || specs[..index].iter().any(|other| other.id == spec.id)
        {
            return Err("插件快捷键 id 只能包含英文字母、数字和 _，且不能重复".into());
        }
        spec.resolve(None)?;
    }
    Ok(())
}
fn default_timeout() -> u64 {
    800
}
fn default_idle() -> u64 {
    30
}
#[derive(Clone)]
pub(crate) struct Plugin {
    pub(crate) manifest: Manifest,
    pub(crate) directory: PathBuf,
    pub(crate) settings_path: Option<PathBuf>,
}

pub(crate) fn parse_manifest(text: &str, path: &Path) -> Result<Plugin, String> {
    if text.len() > 64 * 1024 {
        return Err("插件清单超过 64 KiB".into());
    }
    let mut manifest: Manifest = serde_json::from_str(text).map_err(|e| e.to_string())?;
    if manifest.id.is_empty()
        || manifest.id.len() > 64
        || !manifest
            .id
            .bytes()
            .all(|b| b.is_ascii_alphanumeric() || b == b'-' || b == b'_')
    {
        return Err("插件 id 只能包含英文字母、数字、- 和 _".into());
    }
    if manifest.protocol_version != 2
        || manifest.name.is_empty()
        || manifest.name.len() > 128
        || manifest.keywords.is_empty()
    {
        return Err("插件需要名称、关键词以及 protocol_version = 2".into());
    }
    for keyword in &mut manifest.keywords {
        *keyword = keyword.to_ascii_lowercase();
        if keyword.is_empty()
            || keyword.len() > 32
            || keyword.chars().any(char::is_whitespace)
            || matches!(keyword.as_str(), "g" | "bing" | "e")
            || keyword.starts_with('>')
        {
            return Err("插件关键词无效或与内置命令冲突".into());
        }
    }
    manifest.timeout_ms = manifest.timeout_ms.clamp(50, 60000);
    manifest.global_trigger.validate()?;
    manifest.idle_timeout_secs = manifest.idle_timeout_secs.clamp(1, 300);
    validate_hotkeys(&manifest.hotkeys)?;
    if manifest.config_key.as_ref().is_some_and(|key| {
        key.is_empty()
            || !key
                .bytes()
                .all(|c| c.is_ascii_alphanumeric() || c == b'_' || c == b'-')
            || matches!(key.as_str(), "enabled" | "directories" | "max_processes")
    }) {
        return Err("插件 config_key 无效或与宿主字段冲突".into());
    }
    if manifest.config_defaults.as_ref().is_some_and(|path| {
        path.is_absolute()
            || path
                .components()
                .any(|part| matches!(part, std::path::Component::ParentDir))
    }) {
        return Err("config_defaults 必须位于插件目录内".into());
    }
    Ok(Plugin {
        manifest,
        directory: path.parent().unwrap_or(Path::new(".")).to_path_buf(),
        settings_path: None,
    })
}

fn read_manifest(path: &Path) -> Result<Plugin, String> {
    let mut text = String::new();
    fs::File::open(path)
        .and_then(|f| f.take(64 * 1024 + 1).read_to_string(&mut text))
        .map_err(|e| e.to_string())?;
    parse_manifest(&text, path)
}

/// Validate a local package without executing it or registering settings.
pub fn validate_directory(directory: &Path) -> Result<(), String> {
    let plugin = read_manifest(&directory.join("plugin.json"))?;
    if !directory.join(&plugin.manifest.executable).is_file() {
        return Err("插件程序不存在".into());
    }
    Ok(())
}

pub fn validate_web_keywords(
    settings: &PluginSettings,
    web: &crate::web_search::WebSearchSettings,
) -> Result<(), String> {
    if !web.enabled {
        return Ok(());
    }
    let (plugins, _) = discover(settings);
    for plugin in plugins {
        for engine in &web.engines {
            if engine
                .keywords
                .iter()
                .any(|keyword| keyword != "*" && plugin.manifest.keywords.contains(keyword))
            {
                return Err(format!(
                    "Web 搜索关键词与插件 {} 冲突",
                    plugin.manifest.name
                ));
            }
        }
    }
    Ok(())
}

fn manifest_paths(settings: &PluginSettings) -> Vec<PathBuf> {
    let directories = if settings.directories.is_empty() {
        vec![config::data_dir().join("plugins")]
    } else {
        settings.directories.clone()
    };
    let mut paths = Vec::new();
    for directory in directories {
        let root_manifest = directory.join("plugin.json");
        if root_manifest.is_file() {
            paths.push(root_manifest);
        }
        if let Ok(children) = fs::read_dir(directory) {
            for child in children.flatten().take(256) {
                if child.file_type().is_ok_and(|kind| kind.is_dir()) {
                    let manifest = child.path().join("plugin.json");
                    if manifest.is_file() {
                        paths.push(manifest);
                    }
                }
            }
        }
    }
    paths.sort();
    paths.dedup();
    paths.truncate(MAX_PLUGINS);
    paths
}

/// Read-only catalog for management tools. Includes disabled and broken plugins;
/// never starts plugin processes.
pub struct PluginInfo {
    pub id: String,
    pub name: String,
    pub config_key: String,
    pub directory: PathBuf,
    pub executable: PathBuf,
    pub keywords: Vec<String>,
    pub enabled: bool,
    pub priority: i32,
    pub error: Option<String>,
    pub settings_extension: Option<SettingsExtension>,
}

/// Enabled, valid providers; enumeration never starts a plugin process.
pub fn search_scopes(settings: &PluginSettings) -> Vec<(String, String)> {
    search_catalog(settings).scopes
}

pub struct SearchCatalog {
    pub routing: crate::plugins::QueryRouting,
    pub scopes: Vec<(String, String)>,
    pub direct_display: bool,
}

pub fn search_catalog(settings: &PluginSettings) -> SearchCatalog {
    let (plugins, _) = discover(settings);
    let routing = crate::plugins::QueryRouting::new(plugins.clone());
    let direct_display = plugins.iter().any(|p| p.manifest.direct_display);
    let scopes = plugins
        .into_iter()
        .map(|p| (p.manifest.id, p.manifest.name))
        .collect();
    SearchCatalog {
        routing,
        scopes,
        direct_display,
    }
}

/// Global shortcuts declared by enabled plugins, with user overrides.
pub fn plugin_hotkeys(settings: &PluginSettings) -> Result<Vec<PluginShortcut>, String> {
    let (plugins, _) = discover(settings);
    hotkeys(plugins, settings)
}

fn hotkeys(plugins: Vec<Plugin>, settings: &PluginSettings) -> Result<Vec<PluginShortcut>, String> {
    let mut result = Vec::new();
    for plugin in plugins {
        let key = plugin
            .manifest
            .config_key
            .clone()
            .unwrap_or_else(|| crate::plugin_config::key(&plugin.manifest.id));
        let entry = settings.entries.get(&key);
        for spec in &plugin.manifest.hotkeys {
            let value = entry.and_then(|e| e.options.get(&spec.option()));
            if let Some(hotkey) = spec.resolve(value)? {
                result.push(PluginShortcut {
                    plugin: plugin.manifest.id.clone(),
                    prefill: spec.prefill,
                    hotkey,
                });
            }
        }
    }
    Ok(result)
}

pub fn inspect(settings: &PluginSettings) -> Vec<PluginInfo> {
    let mut active: Vec<(String, Vec<String>)> = Vec::new();
    manifest_paths(settings)
        .into_iter()
        .map(|path| match read_manifest(&path) {
            Ok(plugin) => {
                let key = plugin
                    .manifest
                    .config_key
                    .clone()
                    .unwrap_or_else(|| crate::plugin_config::key(&plugin.manifest.id));
                let entry = settings.entries.get(&key);
                let keywords = entry
                    .and_then(|e| e.keywords.clone())
                    .unwrap_or(plugin.manifest.keywords);
                let enabled = settings.enabled && entry.is_none_or(|e| e.enabled);
                let duplicate_error = if enabled {
                    let collides = active.iter().any(|(other_id, other_keywords)| {
                        *other_id == plugin.manifest.id
                            || other_keywords
                                .iter()
                                .any(|k| k != "*" && keywords.contains(k))
                    });
                    if collides {
                        Some(format!("{}：插件 id 或关键词重复", path.display()))
                    } else {
                        active.push((plugin.manifest.id.clone(), keywords.clone()));
                        None
                    }
                } else {
                    None
                };
                let executable_error =
                    (!plugin.directory.join(&plugin.manifest.executable).is_file())
                        .then(|| "插件程序不存在".to_string());
                PluginInfo {
                    id: plugin.manifest.id,
                    name: plugin.manifest.name,
                    executable: plugin.directory.join(&plugin.manifest.executable),
                    directory: plugin.directory,
                    keywords,
                    enabled,
                    priority: entry
                        .and_then(|e| e.priority)
                        .unwrap_or(plugin.manifest.priority),
                    config_key: key,
                    error: duplicate_error.or(executable_error),
                    settings_extension: plugin.manifest.settings_extension,
                }
            }
            Err(error) => PluginInfo {
                id: String::new(),
                name: path
                    .parent()
                    .unwrap_or(Path::new("."))
                    .display()
                    .to_string(),
                config_key: String::new(),
                directory: path.parent().unwrap_or(Path::new(".")).to_path_buf(),
                executable: PathBuf::new(),
                keywords: Vec::new(),
                enabled: false,
                priority: 0,
                error: Some(error),
                settings_extension: None,
            },
        })
        .collect()
}

pub(crate) fn discover(settings: &PluginSettings) -> (Vec<Plugin>, Vec<String>) {
    if !settings.enabled {
        return (Vec::new(), Vec::new());
    }
    let paths = manifest_paths(settings);
    let mut plugins: Vec<Plugin> = Vec::new();
    let mut errors = Vec::new();
    for path in paths {
        let plugin = read_manifest(&path).map(|mut plugin| {
            let key = plugin
                .manifest
                .config_key
                .clone()
                .unwrap_or_else(|| crate::plugin_config::key(&plugin.manifest.id));
            if let Some(entry) = settings.entries.get(&key) {
                if let Some(keywords) = &entry.keywords {
                    plugin.manifest.keywords = keywords.clone();
                }
                if let Some(direct_display) = entry.direct_display {
                    plugin.manifest.direct_display = direct_display;
                }
                if let Some(priority) = entry.priority {
                    plugin.manifest.priority = priority;
                }
                plugin.settings_path = Some(config::settings_path());
            }
            plugin
        });
        if plugin.as_ref().is_ok_and(|plugin| {
            let key = plugin
                .manifest
                .config_key
                .clone()
                .unwrap_or_else(|| crate::plugin_config::key(&plugin.manifest.id));
            settings
                .entries
                .get(&key)
                .is_some_and(|entry| !entry.enabled)
        }) {
            continue;
        }
        match plugin {
            Ok(plugin)
                if !plugins.iter().any(|other| {
                    other.manifest.id == plugin.manifest.id
                        || other
                            .manifest
                            .keywords
                            .iter()
                            .any(|k| k != "*" && plugin.manifest.keywords.contains(k))
                }) =>
            {
                plugins.push(plugin)
            }
            Ok(_) => errors.push(format!("{}：插件 id 或关键词重复", path.display())),
            Err(error) => errors.push(format!("{}：{error}", path.display())),
        }
    }
    (plugins, errors)
}

/// Immutable catalog handed to the window thread; all filesystem work happens before this.
pub struct RuntimeCatalog {
    pub routing: crate::plugins::QueryRouting,
    pub scopes: Vec<(String, String)>,
    pub direct_display: bool,
    pub hotkeys: Vec<PluginShortcut>,
}
impl RuntimeCatalog {
    pub fn read(settings: &PluginSettings) -> Result<Self, String> {
        let (plugins, _) = discover(settings);
        let scopes = plugins
            .iter()
            .map(|p| (p.manifest.id.clone(), p.manifest.name.clone()))
            .collect();
        let direct_display = plugins.iter().any(|p| p.manifest.direct_display);
        Ok(Self {
            routing: crate::plugins::QueryRouting::new(plugins.clone()),
            scopes,
            direct_display,
            hotkeys: hotkeys(plugins, settings)?,
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn plugin(hotkeys: &str) -> Plugin {
        let manifest = format!(
            r#"{{"id":"clipboard-history","name":"Clip","protocol_version":2,"executable":"c.exe","keywords":["clip"],"config_key":"clipboard_history","hotkeys":{hotkeys}}}"#
        );
        parse_manifest(&manifest, Path::new("plugin.json")).unwrap()
    }

    fn override_option(settings: &mut PluginSettings, option: &str, value: &str) {
        let mut entry = crate::plugin_config::Entry::default();
        entry.options.insert(option.into(), value.into());
        settings.entries.insert("clipboard_history".into(), entry);
    }

    #[test]
    fn plugin_hotkeys_resolve_defaults_prefill_and_user_overrides() {
        let plugin = plugin(
            r#"[{"id":"open","default":"Ctrl+Alt+V"},{"id":"selection","default":"Ctrl+T","prefill":"selection"}]"#,
        );
        let mut settings = PluginSettings::default();
        let found = hotkeys(vec![plugin.clone()], &settings).unwrap();
        assert_eq!(found.len(), 2);
        assert_eq!(found[0].plugin, "clipboard-history");
        assert_eq!(found[0].prefill, Prefill::None);
        assert_eq!(found[0].hotkey.label, "Ctrl+Alt+V");
        assert_eq!(found[1].prefill, Prefill::Selection);

        override_option(&mut settings, "hotkey_open", "Ctrl+Shift+H");
        let found = hotkeys(vec![plugin.clone()], &settings).unwrap();
        assert_eq!(found[0].hotkey.label, "Ctrl+Shift+H");
        assert_eq!(found[1].hotkey.label, "Ctrl+T");

        for text in ["Ctrl+Ctrl", "Alt+Alt", "Shift+Shift", "Win+Win", "A+A", "Space+Space"] {
            override_option(&mut settings, "hotkey_open", text);
            let found = hotkeys(vec![plugin.clone()], &settings).unwrap();
            assert!(found[0].hotkey.double_tap);
            assert_eq!(found[0].hotkey.label, text);
        }

        override_option(&mut settings, "hotkey_open", "");
        let found = hotkeys(vec![plugin.clone()], &settings).unwrap();
        assert_eq!(found.len(), 1);
        assert_eq!(found[0].prefill, Prefill::Selection);

        override_option(&mut settings, "hotkey_open", "V");
        assert!(hotkeys(vec![plugin], &settings).is_err());
    }

    #[test]
    fn manifests_reject_invalid_hotkey_declarations() {
        let manifest = |hotkeys: &str| {
            format!(
                r#"{{"id":"p","name":"P","protocol_version":2,"executable":"p.exe","keywords":["p"],"hotkeys":{hotkeys}}}"#
            )
        };
        for bad in [
            r#"[{"id":"a","default":"V"}]"#,
            r#"[{"id":"a","default":"Ctrl+V"},{"id":"a","default":"Ctrl+B"}]"#,
            r#"[{"id":"bad id","default":"Ctrl+V"}]"#,
            r#"[{"id":"a","default":"Ctrl+V","prefill":"clipboard"}]"#,
            r#"[{"id":"a","default":"Ctrl+V","extra":1}]"#,
        ] {
            assert!(
                parse_manifest(&manifest(bad), Path::new("plugin.json")).is_err(),
                "{bad}"
            );
        }
        assert!(parse_manifest(
            &manifest(r#"[{"id":"a","default":""}]"#),
            Path::new("plugin.json")
        )
        .is_ok());
    }
}
