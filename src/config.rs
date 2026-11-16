use crate::file_io::write_atomic;
pub use crate::history::{load_history, save_history, History, Usage};
use serde::{Deserialize, Serialize};
use std::{
    collections::HashMap,
    fs,
    io::Write,
    path::{Path, PathBuf},
};

pub const DEFAULT_SETTINGS: &str = include_str!("../resources/settings.default.toml");
pub const DEFAULT_WINDOW_WIDTH: i32 = 856;

#[derive(Clone, Debug, PartialEq, Deserialize, Serialize)]
#[serde(default, deny_unknown_fields)]
pub struct Settings {
    pub theme: String,
    pub backdrop: String,
    pub hotkey: String,
    pub max_results: usize,
    pub window_width: i32,
    pub start_at_login: bool,
    pub portable_mode: bool,
    pub search_paths: Vec<PathBuf>,
    pub application_priority: i32,
    pub aliases: HashMap<String, String>,
    pub everything: EverythingSettings,
    pub plugins: PluginSettings,
    pub web_search: crate::web_search::WebSearchSettings,
    pub scope_menu: crate::scope_menu::MenuSettings,
}

#[derive(Clone, Debug, PartialEq, Deserialize, Serialize)]
#[serde(default, deny_unknown_fields)]
pub struct EverythingSettings {
    pub enabled: bool,
    pub priority: i32,
    pub keyword: String,
    pub global_search: bool,
    pub max_results: usize,
    pub timeout_ms: u64,
}
impl Default for EverythingSettings {
    fn default() -> Self {
        Self {
            enabled: true,
            priority: 0,
            keyword: "e".into(),
            global_search: false,
            max_results: 200,
            timeout_ms: 1000,
        }
    }
}

#[derive(Clone, Debug, PartialEq, Deserialize, Serialize)]
#[serde(default)]
pub struct PluginSettings {
    pub enabled: bool,
    pub directories: Vec<PathBuf>,
    pub max_processes: usize,
    #[serde(flatten)]
    pub entries: HashMap<String, crate::plugin_config::Entry>,
}
impl Default for PluginSettings {
    fn default() -> Self {
        Self {
            enabled: true,
            directories: Vec::new(),
            max_processes: 2,
            entries: HashMap::new(),
        }
    }
}

impl Default for Settings {
    fn default() -> Self {
        Self {
            theme: "system".into(),
            backdrop: "mica".into(),
            hotkey: "Alt+Space".into(),
            max_results: 6,
            window_width: DEFAULT_WINDOW_WIDTH,
            start_at_login: false,
            portable_mode: true,
            search_paths: Vec::new(),
            application_priority: 0,
            aliases: HashMap::new(),
            everything: EverythingSettings::default(),
            plugins: PluginSettings::default(),
            web_search: Default::default(),
            scope_menu: Default::default(),
        }
    }
}

pub fn data_dir() -> PathBuf {
    crate::storage::data_dir()
}

pub fn settings_path() -> PathBuf {
    data_dir().join("settings.toml")
}

pub fn load() -> Result<Settings, String> {
    crate::storage::initialize()?;
    let path = settings_path();
    if !path.exists() {
        fs::create_dir_all(data_dir()).map_err(|e| e.to_string())?;
        fs::write(&path, DEFAULT_SETTINGS).map_err(|e| e.to_string())?;
    }
    let text = fs::read_to_string(&path).map_err(|e| e.to_string())?;
    let (mut settings, mut completed) =
        parse_and_complete(&text).map_err(|e| format!("{}\n{e}", path.display()))?;
    let mut document = completed
        .as_deref()
        .unwrap_or(&text)
        .trim_start_matches('\u{feff}')
        .parse::<toml_edit::DocumentMut>()
        .map_err(|e| e.to_string())?;
    let mut changed = crate::plugin_config::complete(&settings.plugins, &mut document)?;
    changed |= crate::scope_menu::complete(&settings, &mut document)?;
    if changed {
        organize_settings(&mut document);
        completed = Some(document.to_string());
        settings = parse_and_complete(completed.as_deref().unwrap())?.0;
    }
    crate::plugin_catalog::validate_web_keywords(&settings.plugins, &settings.web_search)?;
    if let Some(completed) = completed {
        save_settings(&path, &text, &completed)?;
    }
    crate::storage::request_mode(settings.portable_mode, &path)?;
    Ok(settings)
}

pub fn parse_and_complete(text: &str) -> Result<(Settings, Option<String>), String> {
    let mut settings: Settings =
        toml::from_str(text.trim_start_matches('\u{feff}')).map_err(|e| e.to_string())?;
    for misplaced in [
        "theme",
        "hotkey",
        "backdrop",
        "max_results",
        "window_width",
        "start_at_login",
        "portable_mode",
        "search_paths",
    ] {
        if settings.aliases.contains_key(misplaced) {
            return Err(format!(
                "配置项 {misplaced} 被放在 [aliases] 下，请移到文件最上方的根配置中"
            ));
        }
    }
    settings.theme = settings.theme.trim().to_ascii_lowercase();
    settings.backdrop = settings.backdrop.trim().to_ascii_lowercase();
    settings.hotkey = crate::hotkey::Hotkey::parse(&settings.hotkey)?.label;
    if !matches!(settings.theme.as_str(), "system" | "light" | "dark") {
        return Err("theme 必须为 system、light 或 dark".into());
    }
    if !(1..=8).contains(&settings.max_results) {
        return Err("max_results 必须为 1–8（可见行数）".into());
    }
    if !(480..=1440).contains(&settings.window_width) {
        return Err("window_width 必须为 480–1440（逻辑像素）".into());
    }
    if !matches!(settings.backdrop.as_str(), "none" | "mica" | "acrylic") {
        return Err("backdrop 必须为 none、mica 或 acrylic".into());
    }
    settings.everything.keyword = settings.everything.keyword.trim().to_ascii_lowercase();
    if settings.everything.keyword.is_empty()
        || settings.everything.keyword.chars().any(char::is_whitespace)
        || settings.everything.keyword.starts_with('>')
    {
        return Err("Everything keyword 必须为单个关键词，且不能使用内置命令".into());
    }
    if !(1..=1000).contains(&settings.everything.max_results)
        || !(100..=5000).contains(&settings.everything.timeout_ms)
    {
        return Err("everything.max_results 必须为 1–1000，timeout_ms 必须为 100–5000".into());
    }
    if !(1..=4).contains(&settings.plugins.max_processes) {
        return Err("plugins.max_processes 必须为 1–4".into());
    }
    settings.web_search.validate(&settings.everything.keyword)?;
    settings.scope_menu.validate()?;
    settings.scope_menu.validate_shortcuts(&settings.web_search.default_engine)?;
    for (name, entry) in &mut settings.plugins.entries {
        entry
            .validate()
            .map_err(|error| format!("plugins.{name}：{error}"))?;
        if entry.enabled && settings.web_search.enabled {
            if let Some(keywords) = &entry.keywords {
                if settings.web_search.engines.iter().any(|engine| {
                    engine
                        .keywords
                        .iter()
                        .any(|keyword| keyword != "*" && keywords.contains(keyword))
                }) {
                    return Err(format!("Web 搜索关键词与插件配置 {name} 冲突"));
                }
            }
        }
    }
    let migrated = text.trim_start_matches('\u{feff}').replace(
        "# hotkey: Alt+Space / Ctrl+Space / Ctrl+Alt+Space",
        "# hotkey: Ctrl / Alt / Shift / Win + 字母、数字、F1–F24、Space 等，例如 Alt+Q",
    );
    let mut document = migrated
        .parse::<toml_edit::DocumentMut>()
        .map_err(|e| e.to_string())?;
    let defaults = DEFAULT_SETTINGS
        .parse::<toml_edit::DocumentMut>()
        .map_err(|e| e.to_string())?;
    let original = document.to_string();
    let added = fill_missing(document.as_table_mut(), defaults.as_table(), true);
    organize_settings(&mut document);
    let organized = document.to_string();
    let completed = (added || migrated != text || organized != original).then_some(organized);
    Ok((settings, completed))
}

/// Keep each section's descendants next to their parent, including newly added plugin tables.
/// Only move sections; retain values, comments, and array-of-table element order.
pub fn organize_settings(document: &mut toml_edit::DocumentMut) {
    fn position_tables(item: &mut toml_edit::Item, position: &mut isize) {
        match item {
            toml_edit::Item::Table(table) => {
                table.set_position(Some(*position));
                *position += 1;
                for (_, child) in table.iter_mut() {
                    position_tables(child, position);
                }
            }
            toml_edit::Item::ArrayOfTables(tables) => {
                for table in tables.iter_mut() {
                    table.set_position(Some(*position));
                    *position += 1;
                    for (_, child) in table.iter_mut() {
                        position_tables(child, position);
                    }
                }
            }
            _ => {}
        }
    }

    let mut position = 1;
    for section in [
        "aliases",
        "everything",
        "web_search",
        "plugins",
        "scope_menu",
    ] {
        if let Some(item) = document.get_mut(section) {
            position_tables(item, &mut position);
        }
    }
}

pub(crate) fn fill_missing(
    target: &mut dyn toml_edit::TableLike,
    defaults: &dyn toml_edit::TableLike,
    comments: bool,
) -> bool {
    let mut changed = false;
    for (key, value) in defaults.iter() {
        if let Some(existing) = target.get_mut(key) {
            let inline = existing.is_inline_table();
            if let (Some(existing), Some(defaults)) =
                (existing.as_table_like_mut(), value.as_table_like())
            {
                changed |= fill_missing(existing, defaults, !inline);
            }
        } else {
            target.insert(key, value.clone());
            if comments {
                if let (Some(mut inserted), Some(original)) =
                    (target.key_mut(key), defaults.key(key))
                {
                    *inserted.leaf_decor_mut() = original.leaf_decor().clone();
                }
            }
            changed = true;
        }
    }
    changed
}

pub fn save_settings(path: &Path, previous: &str, completed: &str) -> Result<(), String> {
    match fs::OpenOptions::new()
        .write(true)
        .create_new(true)
        .open(path.with_extension("toml.bak"))
    {
        Ok(mut backup) => backup
            .write_all(previous.as_bytes())
            .map_err(|e| e.to_string())?,
        Err(error) if error.kind() == std::io::ErrorKind::AlreadyExists => {}
        Err(error) => return Err(error.to_string()),
    }
    if fs::read_to_string(path).map_err(|e| e.to_string())? != previous {
        return Err("配置正在保存，请稍后重试".into());
    }
    write_atomic(path, completed.as_bytes()).map_err(|e| e.to_string())?;
    let settings: Settings = toml::from_str(completed).map_err(|e| e.to_string())?;
    crate::storage::request_mode(settings.portable_mode, path)
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn search_priorities_complete_defaults_and_validate_integer_bounds() {
        let (settings, completed) = parse_and_complete("").unwrap();
        assert_eq!(settings.application_priority, 0);
        assert_eq!(settings.everything.priority, 0);
        let completed = completed.unwrap();
        assert!(parse_and_complete(&completed).unwrap().1.is_none());
        for priority in [i32::MIN, -1, 0, 1, i32::MAX] {
            let text = format!("application_priority = {priority}\n[everything]\npriority = {priority}\n");
            let (settings, _) = parse_and_complete(&text).unwrap();
            assert_eq!(settings.application_priority, priority);
            assert_eq!(settings.everything.priority, priority);
        }
        for value in ["2147483648", "-2147483649", "1.5", "'1'"] {
            assert!(parse_and_complete(&format!("application_priority = {value}")).is_err());
            assert!(parse_and_complete(&format!("[everything]\npriority = {value}")).is_err());
        }
    }

    #[test]
    fn config_migration_keeps_alt_q_aliases_and_user_comments() {
        let old="# 我的快捷键\nhotkey = 'Alt+q' # 保留注释\n[aliases]\neditor = 'D:\\Tools\\editor.exe'\n";
        let (settings, completed) = parse_and_complete(old).unwrap();
        assert_eq!(settings.hotkey, "Alt+Q");
        let completed = completed.unwrap();
        assert!(completed.contains("# 我的快捷键"));
        assert!(completed.contains("# 保留注释"));
        let (settings, again) = parse_and_complete(&completed).unwrap();
        assert!(again.is_none());
        assert_eq!(settings.backdrop, "mica");
        assert_eq!(settings.window_width, DEFAULT_WINDOW_WIDTH);
        assert!(!settings.start_at_login);
        assert!(settings.portable_mode);
        assert_eq!(settings.aliases["editor"], "D:\\Tools\\editor.exe");
        let parsed: toml::Value = toml::from_str(&completed).unwrap();
        assert_eq!(parsed["everything"]["timeout_ms"].as_integer(), Some(1000));
        assert_eq!(parsed["plugins"]["max_processes"].as_integer(), Some(2));
    }
    #[test]
    fn config_accepts_search_bar_width_limits_and_completes_old_configs() {
        for width in [480, DEFAULT_WINDOW_WIDTH, 1440] {
            let (settings, completed) =
                parse_and_complete(&format!("window_width = {width}")).unwrap();
            assert_eq!(settings.window_width, width);
            let again = parse_and_complete(&completed.unwrap()).unwrap();
            assert_eq!(again.0.window_width, width);
            assert!(again.1.is_none());
        }
    }

    #[test]
    fn config_rejects_unknown_misplaced_and_out_of_range_settings() {
        for invalid in [
            "hotkye = 'Alt+Q'",
            "max_results = 99",
            "window_width = 479",
            "window_width = 1441",
            "start_at_login = 'yes'",
            "portable_mode = 'yes'",
            "[aliases]\nportable_mode = 'false'",
            "[aliases]\nstart_at_login = 'true'",
            "[aliases]\nwindow_width = '800'",
            "[aliases]\nbackdrop = 'acrylic'",
            "[everything]\nmax_reslts = 2",
        ] {
            assert!(parse_and_complete(invalid).is_err(), "{invalid}");
        }
        assert!(parse_and_complete(DEFAULT_SETTINGS).unwrap().1.is_none());
    }

    #[test]
    fn config_groups_sections_without_changing_values_or_engine_order() {
        let old = r#"# 我的配置
hotkey = 'Alt+Q' # 保留快捷键注释

[plugins]
max_processes = 3

# 自定义插件说明
[plugins.custom]
enabled = false
keywords = ['custom']

[plugins.custom.options]
directory = 'D:\My Projects' # 保留路径注释

[aliases]
editor = 'D:\Tools\editor.exe'

[web_search]
default_engine = 'second'

[[web_search.engines]]
id = 'first'
name = 'First'
keywords = ['first']
url = 'https://example.com/first?q={query}'

[everything]
enabled = false

[[web_search.engines]]
id = 'second'
name = 'Second'
keywords = ['second']
url = 'https://example.com/second?q={query}'
"#;
        let (before, completed) = parse_and_complete(old).unwrap();
        let completed = completed.unwrap();
        let (after, again) = parse_and_complete(&completed).unwrap();
        assert_eq!(before, after);
        assert!(again.is_none(), "organization must be idempotent");
        let sections = [
            "[aliases]",
            "[everything]",
            "[web_search]",
            "[[web_search.engines]]",
            "[plugins]",
            "[plugins.custom]",
            "[plugins.custom.options]",
        ];
        for pair in sections.windows(2) {
            assert!(completed.find(pair[0]).unwrap() < completed.find(pair[1]).unwrap());
        }
        assert!(
            completed.rfind("[[web_search.engines]]").unwrap()
                < completed.find("[plugins]").unwrap()
        );
        assert_eq!(after.web_search.engines[0].id, "first");
        assert_eq!(after.web_search.engines[1].id, "second");
        for comment in [
            "# 我的配置",
            "# 保留快捷键注释",
            "# 自定义插件说明",
            "# 保留路径注释",
        ] {
            assert!(completed.contains(comment));
        }
    }

    #[test]
    fn config_groups_new_plugin_tables_after_existing_plugin_settings() {
        let mut document = DEFAULT_SETTINGS.parse::<toml_edit::DocumentMut>().unwrap();
        // Defaults parsed from a separate file carry positions starting at zero.
        let plugin = "enabled = false\n[nested]\nvalue = 42\n"
            .parse::<toml_edit::DocumentMut>()
            .unwrap();
        document["plugins"]["custom"] = toml_edit::Item::Table(plugin.as_table().clone());
        let before = toml::from_str::<toml::Value>(&document.to_string()).unwrap();
        organize_settings(&mut document);
        let text = document.to_string();
        assert!(text.find("[plugins]").unwrap() < text.find("[plugins.custom]").unwrap());
        assert!(
            text.find("[plugins.custom]").unwrap() < text.find("[plugins.custom.nested]").unwrap()
        );
        assert_eq!(before, toml::from_str::<toml::Value>(&text).unwrap());
        assert!(parse_and_complete(&text).unwrap().1.is_none());
    }
}
