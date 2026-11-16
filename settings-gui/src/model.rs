use mini_launcher::config;
use std::{
    collections::BTreeSet,
    fs,
    path::{Path, PathBuf},
};
use toml::Value;

#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Part {
    Key(String),
    Index(usize),
}
pub type FieldPath = Vec<Part>;

pub fn key(name: &str) -> Part {
    Part::Key(name.into())
}

/// Case-insensitive, whitespace-separated filtering across visible list fields.
pub fn matches_filter(query: &str, fields: &[&str]) -> bool {
    let text = fields.join("\n").to_lowercase();
    query.split_whitespace().all(|word| text.contains(&word.to_lowercase()))
}

#[cfg(test)]
#[test]
fn list_filters_match_multiple_fields_without_requiring_case_or_order() {
    assert!(matches_filter(" CLIP  历史 ", &["剪贴板历史", "clipboard-history", "clip"]));
    assert!(matches_filter("", &["anything"]));
    assert!(!matches_filter("clip missing", &["clipboard"]));
}
/// Declarative visibility: all named sibling fields must equal their expected values.
/// Visibility only affects rendering; hidden editors and saved values are retained.
pub fn field_visible(
    condition: &serde_json::Value,
    lookup: impl Fn(&str) -> Option<Value>,
) -> bool {
    if condition.is_null() {
        return true;
    }
    condition.as_object().is_some_and(|conditions| {
        conditions.iter().all(|(name, expected)| {
            lookup(name)
                .and_then(|value| serde_json::to_value(value).ok())
                .is_some_and(|value| &value == expected)
        })
    })
}

pub fn at<'a>(value: &'a Value, path: &[Part]) -> Option<&'a Value> {
    path.iter().try_fold(value, |v, p| match p {
        Part::Key(k) => v.get(k),
        Part::Index(i) => v.get(*i),
    })
}
pub fn at_mut<'a>(value: &'a mut Value, path: &[Part]) -> Option<&'a mut Value> {
    path.iter().try_fold(value, |v, p| match p {
        Part::Key(k) => v.get_mut(k),
        Part::Index(i) => v.get_mut(*i),
    })
}
pub fn path_name(path: &[Part]) -> String {
    let mut text = String::new();
    for part in path {
        match part {
            Part::Key(k) => {
                if !text.is_empty() {
                    text.push('.');
                }
                text.push_str(k);
            }
            Part::Index(i) => text.push_str(&format!("[{i}]")),
        }
    }
    text
}
pub fn parse_value(text: &str) -> Result<Value, String> {
    let table: toml::Table =
        toml::from_str(&format!("value = {text}")).map_err(|e| e.to_string())?;
    if table.len() != 1 {
        return Err("一次只能输入一个 TOML 值".into());
    }
    table.get("value").cloned().ok_or_else(|| "缺少值".into())
}
pub fn literal(value: &Value) -> Result<String, String> {
    item(value)?
        .into_value()
        .map(|v| v.to_string())
        .map_err(|_| "无法转换为 TOML 值".into())
}

#[derive(Clone)]
pub struct Draft {
    pub path: PathBuf,
    pub base: Value,
    pub value: Value,
}
impl Draft {
    pub fn load(path: PathBuf) -> Result<Self, String> {
        let text = fs::read_to_string(&path).map_err(|e| e.to_string())?;
        let base: Value =
            toml::from_str(text.trim_start_matches('\u{feff}')).map_err(|e| e.to_string())?;
        Ok(Self {
            path,
            value: base.clone(),
            base,
        })
    }
    pub fn dirty(&self) -> bool {
        self.base != self.value
    }
    pub fn validate_field(&self, path: &[Part], candidate: &Value) -> Result<(), String> {
        let name = path_name(path);
        match name.as_str() {
            "application_priority" | "everything.priority" => {
                let n = candidate.as_integer().ok_or("优先级必须为整数")?;
                i32::try_from(n).map_err(|_| "优先级必须在 -2147483648 至 2147483647 之间")?;
            }
            "hotkey" => {
                let s = candidate.as_str().ok_or("快捷键必须是字符串")?;
                mini_launcher::hotkey::Hotkey::parse(s)?;
            }
            "theme" => {
                let s = candidate.as_str().ok_or("主题必须是字符串")?;
                if !matches!(
                    s.trim().to_ascii_lowercase().as_str(),
                    "system" | "light" | "dark"
                ) {
                    return Err("theme 必须为 system、light 或 dark".into());
                }
            }
            "backdrop" => {
                let s = candidate.as_str().ok_or("背景材质必须是字符串")?;
                if !matches!(
                    s.trim().to_ascii_lowercase().as_str(),
                    "none" | "mica" | "acrylic"
                ) {
                    return Err("backdrop 必须为 none、mica 或 acrylic".into());
                }
            }
            "max_results" => {
                let n = candidate.as_integer().ok_or("结果数量必须是整数 (1–8)")?;
                if !(1..=8).contains(&n) {
                    return Err("max_results 必须为 1–8（可见行数）".into());
                }
            }
            "window_width" => {
                let n = candidate
                    .as_integer()
                    .ok_or("搜索栏宽度必须是整数 (480–1440)")?;
                if !(480..=1440).contains(&n) {
                    return Err("window_width 必须为 480–1440（逻辑像素）".into());
                }
            }
            "everything.keyword" => {
                let s = candidate.as_str().ok_or("触发关键词必须是字符串")?.trim();
                if s.is_empty() || s.chars().any(char::is_whitespace) || s.starts_with('>') {
                    return Err("Everything keyword 必须为单个关键词，且不能使用内置命令".into());
                }
            }
            "everything.max_results" => {
                let n = candidate
                    .as_integer()
                    .ok_or("Everything 结果数量必须是整数 (1–1000)")?;
                if !(1..=1000).contains(&n) {
                    return Err("everything.max_results 必须为 1–1000".into());
                }
            }
            "everything.timeout_ms" => {
                let n = candidate
                    .as_integer()
                    .ok_or("超时时间必须是整数 (100–5000)")?;
                if !(100..=5000).contains(&n) {
                    return Err("everything.timeout_ms 必须为 100–5000 毫秒".into());
                }
            }
            "plugins.max_processes" => {
                let n = candidate.as_integer().ok_or("进程上限必须是整数 (1–4)")?;
                if !(1..=4).contains(&n) {
                    return Err("plugins.max_processes 必须为 1–4".into());
                }
            }
            "web_search.mode" => {
                let s = candidate.as_str().ok_or("展示模式必须是字符串")?;
                if !matches!(s, "fallback" | "always" | "keyword_only") {
                    return Err("web_search.mode 必须为 fallback、always 或 keyword_only".into());
                }
            }
            _ if name.starts_with("web_search.engines[") && name.ends_with(".url") => {
                let s = candidate.as_str().ok_or("搜索地址必须是字符串")?.trim();
                if !(s.starts_with("http://") || s.starts_with("https://"))
                    || !s.contains("{query}")
                {
                    return Err(
                        "搜索地址必须以 http:// 或 https:// 开头，且包含 {query} 占位符".into(),
                    );
                }
            }
            _ if name.starts_with("web_search.engines[") && name.ends_with(".id") => {
                let s = candidate.as_str().ok_or("引擎 ID 必须是字符串")?.trim();
                if s.is_empty() {
                    return Err("引擎唯一 ID 不能为空".into());
                }
            }
            _ => {}
        }
        #[cfg(windows)]
        if let [Part::Key(section), Part::Key(config_key), Part::Key(field)] = path {
            if section == "plugins" {
                if let Ok(settings) = self.value.clone().try_into::<config::Settings>() {
                    if let Some(plugin) = mini_launcher::plugin_catalog::inspect(&settings.plugins)
                        .into_iter()
                        .find(|p| &p.config_key == config_key)
                    {
                        let spec = schema(&plugin.directory)?;
                        let rule = &spec["fields"][field];
                        if !rule.is_null() {
                            let valid_type = match rule["type"].as_str() {
                                Some("string" | "path") => candidate.is_str(),
                                Some("integer") => candidate.is_integer(),
                                Some("boolean") => candidate.is_bool(),
                                Some("array") => candidate.is_array(),
                                _ => true,
                            };
                            if !valid_type {
                                return Err(format!(
                                    "plugins.{config_key}.{field}：值类型不正确（需要 {}）",
                                    rule["type"].as_str().unwrap_or("对应类型")
                                ));
                            }
                            if let Some(options) = rule["enum"].as_array() {
                                let ok = options.iter().any(|o| {
                                    serde_json::to_value(candidate).is_ok_and(|v| &v == o)
                                });
                                if !ok {
                                    return Err(format!(
                                        "plugins.{config_key}.{field}：不在可选值范围内"
                                    ));
                                }
                            }
                            if let Some(n) = candidate.as_integer() {
                                if let Some(min) = rule["minimum"].as_i64() {
                                    if n < min {
                                        return Err(format!(
                                            "plugins.{config_key}.{field}：不能小于 {min}"
                                        ));
                                    }
                                }
                                if let Some(max) = rule["maximum"].as_i64() {
                                    if n > max {
                                        return Err(format!(
                                            "plugins.{config_key}.{field}：不能大于 {max}"
                                        ));
                                    }
                                }
                            }
                            if rule["absolute"].as_bool() == Some(true) {
                                let ok = candidate.as_str().is_some_and(|s| {
                                    Path::new(s).is_absolute() && !s.contains('\0')
                                });
                                if !ok {
                                    return Err(format!(
                                        "plugins.{config_key}.{field}：必须填写绝对路径"
                                    ));
                                }
                            }
                        }
                    }
                }
            }
        }
        Ok(())
    }
    pub fn set(&mut self, path: &[Part], value: Value) -> Result<(), String> {
        if let [Part::Key(section), Part::Key(plugin), Part::Key(field)] = path {
            if section == "plugins" && field == "priority" {
                let n = value.as_integer().ok_or("优先级必须为整数")?;
                i32::try_from(n).map_err(|_| "优先级必须在 -2147483648 至 2147483647 之间")?;
                let plugins = self
                    .value
                    .get_mut("plugins")
                    .and_then(Value::as_table_mut)
                    .ok_or("插件配置必须为表")?;
                let entry = plugins
                    .entry(plugin.clone())
                    .or_insert_with(|| Value::Table(Default::default()));
                entry
                    .as_table_mut()
                    .ok_or("插件配置必须为表")?
                    .insert(field.clone(), value);
                return Ok(());
            }
        }
        *at_mut(&mut self.value, path).ok_or("字段已不存在")? = value;
        Ok(())
    }
    pub fn register(&mut self, directory: &Path) -> Result<(), String> {
        let mut document = self.document()?;
        mini_launcher::plugin_config::register_in_document(directory, &mut document)?;
        self.value = toml::from_str(&document.to_string()).map_err(|e| e.to_string())?;
        Ok(())
    }
    pub fn document(&self) -> Result<toml_edit::DocumentMut, String> {
        toml::to_string(&self.value)
            .map_err(|e| e.to_string())?
            .parse()
            .map_err(|e: toml_edit::TomlError| e.to_string())
    }
    /// Three-way merge. Arrays are conflict units, since indices can change after
    /// another editor inserts or reorders an element.
    pub fn save(&mut self, prefer_mine: bool) -> Result<(), String> {
        let disk = fs::read_to_string(&self.path).map_err(|e| e.to_string())?;
        let current: Value =
            toml::from_str(disk.trim_start_matches('\u{feff}')).map_err(|e| e.to_string())?;
        let merged = merge(
            Some(&self.base),
            Some(&self.value),
            Some(&current),
            "",
            prefer_mine,
        )?
        .ok_or("配置不能为空")?;
        let mut document: toml_edit::DocumentMut = disk
            .trim_start_matches('\u{feff}')
            .parse()
            .map_err(|e: toml_edit::TomlError| e.to_string())?;
        patch_table(
            document.as_table_mut(),
            current.as_table().ok_or("配置必须为表")?,
            merged.as_table().ok_or("配置必须为表")?,
        )?;
        config::organize_settings(&mut document);
        let text = document.to_string();
        let (settings, completed) = config::parse_and_complete(&text)?;
        mini_launcher::plugin_catalog::validate_web_keywords(
            &settings.plugins,
            &settings.web_search,
        )?;
        validate_plugin_schemas(&settings.plugins)?;
        let completed = completed.unwrap_or(text);
        if completed != disk {
            config::save_settings(&self.path, &disk, &completed)?;
        }
        // Keep the exact values just written; an external later save is handled
        // by the next three-way merge rather than silently replacing this baseline.
        self.base = toml::from_str(&completed).map_err(|e| e.to_string())?;
        self.value = self.base.clone();
        Ok(())
    }
}

fn merge(
    base: Option<&Value>,
    mine: Option<&Value>,
    disk: Option<&Value>,
    path: &str,
    prefer: bool,
) -> Result<Option<Value>, String> {
    if mine == base {
        return Ok(disk.cloned());
    }
    if disk == base || mine == disk {
        return Ok(mine.cloned());
    }
    if let (Some(Value::Table(b)), Some(Value::Table(m)), Some(Value::Table(d))) =
        (base, mine, disk)
    {
        let keys: BTreeSet<_> = b.keys().chain(m.keys()).chain(d.keys()).collect();
        let mut result = toml::Table::new();
        for k in keys {
            let child = if path.is_empty() {
                k.clone()
            } else {
                format!("{path}.{k}")
            };
            if let Some(v) = merge(b.get(k), m.get(k), d.get(k), &child, prefer)? {
                result.insert(k.clone(), v);
            }
        }
        return Ok(Some(Value::Table(result)));
    }
    if prefer {
        Ok(mine.cloned())
    } else {
        Err(format!(
            "外部修改冲突：{path}。F5 重新载入；Ctrl+O 保留本次修改后重试。"
        ))
    }
}

fn item(value: &Value) -> Result<toml_edit::Item, String> {
    let mut table = toml::Table::new();
    table.insert("v".into(), value.clone());
    let mut doc: toml_edit::DocumentMut = toml::to_string(&table)
        .map_err(|e| e.to_string())?
        .parse()
        .map_err(|e: toml_edit::TomlError| e.to_string())?;
    doc.as_table_mut()
        .remove("v")
        .ok_or_else(|| "无法转换配置值".into())
}
fn patch_table(
    table: &mut toml_edit::Table,
    old: &toml::Table,
    new: &toml::Table,
) -> Result<(), String> {
    for k in old.keys().filter(|k| !new.contains_key(*k)) {
        table.remove(k);
    }
    for (k, v) in new {
        if old.get(k) == Some(v) {
            continue;
        }
        if let Some(existing) = table.get_mut(k) {
            patch_item(existing, old.get(k), v)?;
        } else {
            table.insert(k, item(v)?);
        }
    }
    Ok(())
}
fn patch_item(
    target: &mut toml_edit::Item,
    old: Option<&Value>,
    new: &Value,
) -> Result<(), String> {
    if let (Some(table), Some(Value::Table(o)), Value::Table(n)) = (target.as_table_mut(), old, new)
    {
        return patch_table(table, o, n);
    }
    if let (Some(tables), Some(Value::Array(o)), Value::Array(n)) =
        (target.as_array_of_tables_mut(), old, new)
    {
        if !n.is_empty() && n.iter().all(Value::is_table) {
            let originals: Vec<_> = tables.iter().cloned().collect();
            let mut used = BTreeSet::new();
            let mut result = toml_edit::ArrayOfTables::new();
            for (index, value) in n.iter().enumerate() {
                // Move comments with the entry when engines are reordered or
                // inserted. A stable id also handles edits to an existing entry.
                let source = o
                    .iter()
                    .enumerate()
                    .find(|(i, old)| !used.contains(i) && *old == value)
                    .map(|(i, _)| i)
                    .or_else(|| {
                        value.get("id").and_then(Value::as_str).and_then(|id| {
                            o.iter()
                                .enumerate()
                                .find(|(i, old)| {
                                    !used.contains(i)
                                        && old.get("id").and_then(Value::as_str) == Some(id)
                                })
                                .map(|(i, _)| i)
                        })
                    })
                    .or_else(|| (o.len() == n.len() && !used.contains(&index)).then_some(index));
                let table = if let Some(source) = source {
                    used.insert(source);
                    let mut table = originals[source].clone();
                    patch_table(
                        &mut table,
                        o[source].as_table().unwrap(),
                        value.as_table().unwrap(),
                    )?;
                    table
                } else {
                    item(value)?.as_table().ok_or("无法转换表项")?.clone()
                };
                result.push(table);
            }
            *tables = result;
            return Ok(());
        }
    }
    let mut replacement = item(new)?;
    if let (Some(old), Some(new)) = (target.as_value(), replacement.as_value_mut()) {
        *new.decor_mut() = old.decor().clone();
    }
    *target = replacement;
    Ok(())
}

/// Optional plugin-owned metadata. No plugin executable is run by the editor.
pub fn schema(directory: &Path) -> Result<serde_json::Value, String> {
    let path = directory.join("settings.schema.json");
    match fs::read_to_string(&path) {
        Ok(text) => serde_json::from_str(&text).map_err(|e| format!("{}：{e}", path.display())),
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => Ok(serde_json::Value::Null),
        Err(e) => Err(e.to_string()),
    }
}
pub fn validate_plugin_schemas(settings: &config::PluginSettings) -> Result<(), String> {
    #[cfg(windows)]
    for plugin in mini_launcher::plugin_catalog::inspect(settings) {
        if plugin.enabled
            && plugin
                .error
                .as_ref()
                .is_some_and(|e| e.contains("插件 id 或关键词重复"))
        {
            return Err(format!(
                "plugins.{}：插件 id 或关键词重复，请调整关键词或停用冲突插件",
                plugin.config_key
            ));
        }
        if !plugin.enabled || plugin.config_key.is_empty() {
            continue;
        }
        let spec = schema(&plugin.directory)?;
        let Some(fields) = spec["fields"].as_object() else {
            continue;
        };
        let Some(entry) = settings.entries.get(&plugin.config_key) else {
            continue;
        };
        for (field, rule) in fields {
            let Some(value) = entry.options.get(field) else {
                continue;
            };
            let valid_type = match rule["type"].as_str() {
                Some("string" | "path") => value.is_str(),
                Some("integer") => value.is_integer(),
                Some("boolean") => value.is_bool(),
                Some("array") => value.is_array(),
                _ => true,
            };
            let valid_enum = rule["enum"].as_array().is_none_or(|options| {
                options
                    .iter()
                    .any(|o| serde_json::to_value(value).is_ok_and(|v| &v == o))
            });
            let valid_number = value.as_integer().is_none_or(|n| {
                rule["minimum"].as_i64().is_none_or(|min| n >= min)
                    && rule["maximum"].as_i64().is_none_or(|max| n <= max)
            });
            let valid_path = rule["absolute"].as_bool() != Some(true)
                || value
                    .as_str()
                    .is_some_and(|s| Path::new(s).is_absolute() && !s.contains('\0'));
            if !valid_type || !valid_enum || !valid_number || !valid_path {
                return Err(format!(
                    "plugins.{}.{field}：类型、范围、路径或可选值不正确",
                    plugin.config_key
                ));
            }
        }
    }
    Ok(())
}

#[cfg(test)]
pub(crate) mod tests {
    use super::*;
    #[test]
    fn field_visibility_tracks_current_siblings_and_preserves_configuration() {
        let mut settings: Value = toml::from_str(
            "mode='remote'\nenabled=true\nlimit=2\nlocal_path='saved'\nremote_key='retained'",
        )
        .unwrap();
        let local = serde_json::json!({"mode":"local"});
        let remote = serde_json::json!({"mode":"remote","enabled":true,"limit":2});
        assert!(field_visible(&serde_json::Value::Null, |_| None));
        assert!(field_visible(&serde_json::json!({}), |_| None));
        assert!(field_visible(&remote, |name| settings.get(name).cloned()));
        assert!(!field_visible(&local, |name| settings.get(name).cloned()));
        settings["mode"] = "local".into();
        assert!(field_visible(&local, |name| settings.get(name).cloned()));
        assert!(!field_visible(&remote, |name| settings.get(name).cloned()));
        assert_eq!(settings["local_path"].as_str(), Some("saved"));
        assert_eq!(settings["remote_key"].as_str(), Some("retained"));
        assert!(!field_visible(
            &serde_json::json!({"missing":"value"}),
            |_| None
        ));
        assert!(!field_visible(&serde_json::json!({"limit":"2"}), |name| {
            settings.get(name).cloned()
        }));
        assert!(!field_visible(&serde_json::json!(false), |_| None));
    }
    pub struct Fixture(pub PathBuf);
    impl Fixture {
        pub fn new() -> Self {
            static SERIAL: std::sync::atomic::AtomicU64 = std::sync::atomic::AtomicU64::new(0);
            let path = std::env::temp_dir().join(format!(
                "mini-launcher-tui-test-{}-{}",
                std::process::id(),
                SERIAL.fetch_add(1, std::sync::atomic::Ordering::Relaxed)
            ));
            fs::create_dir_all(&path).unwrap();
            let mut value: Value = toml::from_str(config::DEFAULT_SETTINGS).unwrap();
            value["plugins"]["enabled"] = Value::Boolean(false);
            value["plugins"]["directories"] = Value::Array(vec![Value::String(
                path.join("plugins").display().to_string(),
            )]);
            fs::write(
                path.join("settings.toml"),
                format!("# keep this comment\n{}", toml::to_string(&value).unwrap()),
            )
            .unwrap();
            Self(path)
        }
        pub fn draft(&self) -> Draft {
            Draft::load(self.0.join("settings.toml")).unwrap()
        }
    }
    impl Drop for Fixture {
        fn drop(&mut self) {
            // Only remove this fixture's unique directory directly under temp.
            if self.0.parent() == Some(std::env::temp_dir().as_path())
                && self
                    .0
                    .file_name()
                    .unwrap()
                    .to_string_lossy()
                    .starts_with("mini-launcher-tui-test-")
            {
                let _ = fs::remove_dir_all(&self.0);
            }
        }
    }
    #[test]
    fn merge_preserves_external_changes_and_detects_conflicts() {
        let b: Value = toml::from_str("theme='system'\nhotkey='Alt+Space'").unwrap();
        let mut mine = b.clone();
        mine["theme"] = Value::String("dark".into());
        let mut disk = b.clone();
        disk["hotkey"] = Value::String("Alt+Q".into());
        let result = merge(Some(&b), Some(&mine), Some(&disk), "", false)
            .unwrap()
            .unwrap();
        assert_eq!(result["theme"].as_str(), Some("dark"));
        assert_eq!(result["hotkey"].as_str(), Some("Alt+Q"));
        disk["theme"] = Value::String("light".into());
        assert!(merge(Some(&b), Some(&mine), Some(&disk), "", false)
            .unwrap_err()
            .contains("theme"));
        assert_eq!(
            merge(Some(&b), Some(&mine), Some(&disk), "", true)
                .unwrap()
                .unwrap()["hotkey"]
                .as_str(),
            Some("Alt+Q")
        );
    }
    #[test]
    fn document_patch_keeps_comments_unknown_values_and_engine_order() {
        let text = "# theme comment\ntheme = 'system' # inline\n[plugins.custom]\nopaque = { nested = [1, 2] } # keep\n[[web_search.engines]]\nid='a'\nname='A' # first\n[[web_search.engines]]\nid='b'\nname='B'\n";
        let old: Value = toml::from_str(text).unwrap();
        let mut new = old.clone();
        new["theme"] = Value::String("dark".into());
        new["web_search"]["engines"][0]["name"] = Value::String("Changed".into());
        let mut doc: toml_edit::DocumentMut = text.parse().unwrap();
        patch_table(
            doc.as_table_mut(),
            old.as_table().unwrap(),
            new.as_table().unwrap(),
        )
        .unwrap();
        let output = doc.to_string();
        for comment in ["# theme comment", "# inline", "# keep", "# first"] {
            assert!(output.contains(comment), "{output}");
        }
        assert_eq!(toml::from_str::<Value>(&output).unwrap(), new);
    }
    #[test]
    fn arrays_conflict_when_external_editor_reorders() {
        let b = parse_value("['a', 'b']").unwrap();
        let m = parse_value("['a', 'c']").unwrap();
        let d = parse_value("['b', 'a']").unwrap();
        assert!(merge(Some(&b), Some(&m), Some(&d), "search_paths", false).is_err());
    }
    #[test]
    fn engine_comments_follow_entries_when_reordered_and_extended() {
        let fixture = Fixture::new();
        let path = fixture.0.join("settings.toml");
        let text = fs::read_to_string(&path)
            .unwrap()
            .replace("name = \"Google\"", "name = \"Google\" # Google comment")
            .replace("name = \"Bing\"", "name = \"Bing\" # Bing comment");
        fs::write(&path, text).unwrap();
        let mut draft = fixture.draft();
        let engines = draft.value["web_search"]["engines"].as_array_mut().unwrap();
        engines.swap(0, 1);
        let mut extra = engines[0].clone();
        extra["id"] = Value::String("extra".into());
        extra["name"] = Value::String("Extra".into());
        extra["keywords"] = Value::Array(vec![]);
        engines.push(extra);
        draft.save(false).unwrap();
        let saved = fs::read_to_string(&path).unwrap();
        assert!(
            saved.contains("name = \"Google\" # Google comment"),
            "{saved}"
        );
        assert!(saved.contains("name = \"Bing\" # Bing comment"), "{saved}");
        assert_eq!(
            fixture.draft().value["web_search"]["engines"][0]["id"].as_str(),
            Some("bing")
        );
        assert_eq!(
            fixture.draft().value["web_search"]["engines"][2]["id"].as_str(),
            Some("extra")
        );
    }
    #[test]
    fn save_validates_merges_backs_up_and_preserves_disk_comments() {
        let fixture = Fixture::new();
        let mut draft = fixture.draft();
        draft
            .set(&[key("theme")], Value::String("dark".into()))
            .unwrap();
        let text = fs::read_to_string(&draft.path)
            .unwrap()
            .replace("Alt+Space", "Alt+Q");
        fs::write(&draft.path, &text).unwrap();
        draft.save(false).unwrap();
        let saved = fs::read_to_string(&draft.path).unwrap();
        assert!(saved.contains("# keep this comment"));
        assert_eq!(draft.value["hotkey"].as_str(), Some("Alt+Q"));
        assert_eq!(draft.value["theme"].as_str(), Some("dark"));
        assert_eq!(
            fs::read_to_string(draft.path.with_extension("toml.bak")).unwrap(),
            text
        );
        assert!(!draft.dirty());
        draft
            .set(&[key("max_results")], Value::Integer(100))
            .unwrap();
        assert!(draft.save(false).unwrap_err().contains("max_results"));
        assert_eq!(fs::read_to_string(&draft.path).unwrap(), saved);
        assert!(draft.dirty());
    }
    #[test]
    fn search_priorities_save_and_validate_bounds() {
        let fixture = Fixture::new();
        let mut draft = fixture.draft();
        for path in [
            vec![key("application_priority")],
            vec![key("everything"), key("priority")],
        ] {
            draft.validate_field(&path, &Value::Integer(2)).unwrap();
            draft.set(&path, Value::Integer(2)).unwrap();
            for invalid in [
                Value::Integer(i32::MAX as i64 + 1),
                Value::Integer(i32::MIN as i64 - 1),
                Value::String("1".into()),
            ] {
                assert!(draft.validate_field(&path, &invalid).is_err());
            }
        }
        draft.save(false).unwrap();
        let settings: config::Settings = fixture.draft().value.try_into().unwrap();
        assert_eq!(settings.application_priority, 2);
        assert_eq!(settings.everything.priority, 2);
    }

    #[test]
    fn plugin_priority_override_round_trips_and_checks_bounds() {
        let fixture = Fixture::new();
        let mut draft = fixture.draft();
        let path = [key("plugins"), key("custom_priority"), key("priority")];
        assert!(at(&draft.value, &path).is_none());
        draft.set(&path, Value::Integer(-100)).unwrap();
        draft.save(false).unwrap();
        let saved = fixture.draft();
        assert_eq!(
            at(&saved.value, &path).and_then(Value::as_integer),
            Some(-100)
        );
        let settings: config::Settings = saved.value.clone().try_into().unwrap();
        assert_eq!(
            settings.plugins.entries["custom_priority"].priority,
            Some(-100)
        );
        for invalid in [
            Value::Integer(i32::MAX as i64 + 1),
            Value::Integer(i32::MIN as i64 - 1),
            Value::String("100".into()),
        ] {
            assert!(draft.set(&path, invalid).is_err());
            assert_eq!(
                at(&draft.value, &path).and_then(Value::as_integer),
                Some(-100)
            );
        }
    }

    #[test]
    fn staged_plugin_registration_and_nested_values_round_trip() {
        let fixture = Fixture::new();
        let mut draft = fixture.draft();
        let directory = fixture.0.join("plugins").join("demo");
        fs::create_dir_all(&directory).unwrap();
        fs::write(directory.join("demo.exe"), "fixture, never executed").unwrap();
        fs::write(directory.join("plugin.json"), r#"{"id":"demo","name":"Demo","protocol_version":2,"executable":"demo.exe","keywords":["demo"],"config_defaults":"defaults.toml"}"#).unwrap();
        fs::write(
            directory.join("defaults.toml"),
            "enabled=true\n[nested]\nvalues=[]\n",
        )
        .unwrap();
        let before = fs::read_to_string(&draft.path).unwrap();
        draft.register(&directory).unwrap();
        assert_eq!(fs::read_to_string(&draft.path).unwrap(), before);
        let path = vec![key("plugins"), key("demo"), key("nested"), key("values")];
        let nested = parse_value("[{name='工作区', tags=['a','b']}, {name='二'}]").unwrap();
        assert_eq!(parse_value(&literal(&nested).unwrap()).unwrap(), nested);
        draft.set(&path, nested.clone()).unwrap();
        draft.save(false).unwrap();
        assert_eq!(at(&fixture.draft().value, &path), Some(&nested));
    }
}
