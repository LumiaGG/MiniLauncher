//! Menu preferences are independent of provider enablement and query routing.
use crate::config::Settings;
use serde::{Deserialize, Serialize};
use std::collections::HashSet;

#[derive(Clone, Debug, PartialEq, Deserialize, Serialize)]
#[serde(default, deny_unknown_fields)]
pub struct MenuSettings {
    pub items: Vec<Item>,
}

impl Default for MenuSettings {
    fn default() -> Self {
        Self {
            items: vec![Item::new("all", ""), Item::new("files", "")],
        }
    }
}

#[derive(Clone, Debug, PartialEq, Deserialize, Serialize)]
#[serde(deny_unknown_fields)]
pub struct Item {
    pub id: String,
    #[serde(default)]
    pub name: String,
    #[serde(default = "shown")]
    pub visible: bool,
    /// Missing keeps the legacy binding; an empty string disables it.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub shortcut: Option<String>,
}
fn shown() -> bool {
    true
}

impl Item {
    pub fn new(id: impl Into<String>, name: impl Into<String>) -> Self {
        Self {
            id: id.into(),
            name: name.into(),
            visible: true,
            shortcut: None,
        }
    }
}

impl MenuSettings {
    pub fn shortcut(&self, id: &str, default_engine: &str) -> String {
        self.items
            .iter()
            .find(|item| item.id == id)
            .and_then(|item| item.shortcut.clone())
            .unwrap_or_else(|| match id {
                "all" => "Alt+1".into(),
                "files" => "Alt+2".into(),
                _ if id == format!("web:{default_engine}") => "Alt+3".into(),
                _ => String::new(),
            })
    }

    pub fn validate_shortcuts(&self, default_engine: &str) -> Result<(), String> {
        let mut ids = self
            .items
            .iter()
            .map(|item| item.id.clone())
            .collect::<HashSet<_>>();
        ids.extend([
            "all".into(),
            "files".into(),
            format!("web:{default_engine}"),
        ]);
        let mut seen = HashSet::new();
        for id in ids {
            let shortcut = self.shortcut(&id, default_engine);
            if !shortcut.is_empty() && !seen.insert(shortcut.clone()) {
                return Err(format!("范围快捷键重复：{shortcut}"));
            }
        }
        Ok(())
    }

    pub fn validate(&mut self) -> Result<(), String> {
        if self.items.len() > 256 {
            return Err("范围菜单最多配置 256 项".into());
        }
        let mut ids = HashSet::new();
        for item in &mut self.items {
            item.id = item.id.trim().to_owned();
            item.name = item.name.trim().to_owned();
            if let Some(shortcut) = &mut item.shortcut {
                *shortcut = shortcut.trim().to_owned();
                if !shortcut.is_empty() {
                    let parsed = crate::hotkey::Hotkey::parse(shortcut)?;
                    if parsed.modifiers != 1 || !(0x30..=0x39).contains(&parsed.key) {
                        return Err("范围快捷键须为 Alt+0–9，留空禁用".into());
                    }
                    *shortcut = parsed.label;
                }
            }
            if !(matches!(item.id.as_str(), "all" | "files")
                || ["web:", "plugin:"].iter().any(|prefix| {
                    item.id
                        .strip_prefix(prefix)
                        .is_some_and(|id| !id.is_empty())
                }))
                || item.id.len() > 128
                || item.id.chars().any(char::is_control)
                || !ids.insert(item.id.clone())
            {
                return Err(format!("范围菜单来源 ID 无效或重复：{}", item.id));
            }
            if item.name.chars().count() > 80 || item.name.chars().any(char::is_control) {
                return Err("范围菜单显示名称最多 80 字，不能包含控制字符".into());
            }
        }
        Ok(())
    }

    /// Configured sources first; new sources follow in their discovery order.
    /// Missing/disabled sources stay in the preferences but never become available here.
    pub fn resolve(&self, available: &[Item]) -> Vec<Item> {
        let mut result = Vec::new();
        let mut seen = HashSet::new();
        for preference in &self.items {
            seen.insert(preference.id.as_str());
            if !preference.visible {
                continue;
            }
            if let Some(source) = available.iter().find(|item| item.id == preference.id) {
                let mut item = source.clone();
                if !preference.name.is_empty() {
                    item.name.clone_from(&preference.name);
                }
                result.push(item);
            }
        }
        result.extend(
            available
                .iter()
                .filter(|item| !seen.contains(item.id.as_str()))
                .cloned(),
        );
        result
    }
}

/// Catalog for settings, including disabled providers. Never runs plugin programs.
pub fn catalog(settings: &Settings) -> Vec<Item> {
    let mut items = vec![Item::new("all", "综合搜索"), Item::new("files", "文件搜索")];
    items.extend(
        settings
            .web_search
            .engines
            .iter()
            .map(|engine| Item::new(format!("web:{}", engine.id), &engine.name)),
    );
    #[cfg(windows)]
    items.extend(
        crate::plugin_catalog::inspect(&settings.plugins)
            .into_iter()
            .filter(|plugin| !plugin.id.is_empty())
            .map(|plugin| Item::new(format!("plugin:{}", plugin.id), plugin.name)),
    );
    let mut seen = HashSet::new();
    items.retain(|item| seen.insert(item.id.clone()));
    items
}

/// Append new sources without changing existing visibility, labels or ordering.
pub fn complete(
    settings: &Settings,
    document: &mut toml_edit::DocumentMut,
) -> Result<bool, String> {
    let current: Settings = toml::from_str(&document.to_string()).map_err(|e| e.to_string())?;
    let mut known: HashSet<_> = current
        .scope_menu
        .items
        .iter()
        .map(|item| item.id.trim().to_owned())
        .collect();
    let available = catalog(settings);
    let mut changed = false;
    let items = &mut document["scope_menu"]["items"];
    for item in &available {
        if known.insert(item.id.clone()) {
            let mut table = toml_edit::Table::new();
            table["id"] = toml_edit::value(&item.id);
            table["name"] = toml_edit::value("");
            table["visible"] = toml_edit::value(true);
            if let Some(tables) = items.as_array_of_tables_mut() {
                tables.push(table);
            } else if let Some(array) = items.as_array_mut() {
                array.push(table.into_inline_table());
            } else {
                return Err("scope_menu.items 必须为菜单项列表".into());
            }
            changed = true;
        }
    }
    Ok(changed)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn scope_shortcuts_preserve_defaults_and_allow_reassignment_and_disabling() {
        let text = "[[scope_menu.items]]\nid='all'\nshortcut=''\n\
            [[scope_menu.items]]\nid='plugin:example'\nshortcut=' alt + 1 '\nvisible=false\n\
            [[scope_menu.items]]\nid='web:other'\nshortcut='Alt+0'\n";
        let (settings, _) = crate::config::parse_and_complete(text).unwrap();
        let menu = &settings.scope_menu;
        assert_eq!(menu.shortcut("all", "bing"), "");
        assert_eq!(menu.shortcut("files", "bing"), "Alt+2");
        assert_eq!(menu.shortcut("web:bing", "bing"), "Alt+3");
        assert_eq!(menu.shortcut("plugin:example", "bing"), "Alt+1");
        assert_eq!(menu.shortcut("web:other", "bing"), "Alt+0");
        let saved = toml::to_string(&settings).unwrap();
        let (restored, _) = crate::config::parse_and_complete(&saved).unwrap();
        assert_eq!(restored.scope_menu, settings.scope_menu);
    }

    #[test]
    fn scope_shortcuts_reject_duplicates_and_unsupported_keys() {
        for shortcut in ["Alt+2", "Alt+3", "Ctrl+1", "Alt+A", "Alt+10"] {
            let text =
                format!("[[scope_menu.items]]\nid='plugin:example'\nshortcut='{shortcut}'\n");
            assert!(
                crate::config::parse_and_complete(&text).is_err(),
                "{shortcut}"
            );
        }
        let text = "[[scope_menu.items]]\nid='plugin:one'\nshortcut='Alt+9'\n\
            [[scope_menu.items]]\nid='plugin:two'\nshortcut='alt+9'\n";
        assert!(crate::config::parse_and_complete(text).is_err());
    }

    #[test]
    fn scope_menu_orders_renames_hides_and_keeps_disabled_sources_unavailable() {
        let preferences = MenuSettings {
            items: vec![
                Item::new("web:bing", "联网搜索"),
                Item {
                    visible: false,
                    ..Item::new("files", "")
                },
                Item::new("plugin:disabled", "隐藏来源不能被菜单启用"),
                Item::new("all", ""),
            ],
        };
        let available = vec![
            Item::new("all", "综合搜索"),
            Item::new("files", "文件搜索"),
            Item::new("web:bing", "Bing"),
            Item::new("plugin:new", "新插件"),
        ];
        let resolved = preferences.resolve(&available);
        assert_eq!(
            resolved
                .iter()
                .map(|item| item.id.as_str())
                .collect::<Vec<_>>(),
            ["web:bing", "all", "plugin:new"]
        );
        assert_eq!(resolved[0].name, "联网搜索");
        assert_eq!(resolved[1].name, "综合搜索");
    }

    #[test]
    fn scope_menu_completion_preserves_preferences_and_is_idempotent_for_both_array_forms() {
        for text in [
            "[scope_menu]\nitems=[{id=' files ',name='我的文件',visible=false}]\n",
            "# 保留注释\n[[scope_menu.items]]\nid='files'\nname='我的文件'\nvisible=false\n",
            "theme='dark'\n",
        ] {
            let (settings, completed) = crate::config::parse_and_complete(text).unwrap();
            let mut document: toml_edit::DocumentMut =
                completed.unwrap_or_else(|| text.into()).parse().unwrap();
            assert!(complete(&settings, &mut document).unwrap());
            let (settings, _) = crate::config::parse_and_complete(&document.to_string()).unwrap();
            assert_eq!(
                settings
                    .scope_menu
                    .items
                    .iter()
                    .filter(|item| item.id == "files")
                    .count(),
                1
            );
            if text.contains("我的文件") {
                assert_eq!(settings.scope_menu.items[0].id, "files");
                assert_eq!(settings.scope_menu.items[0].name, "我的文件");
                assert!(!settings.scope_menu.items[0].visible);
            }
            assert!(!complete(&settings, &mut document).unwrap());
            if text.contains("# 保留注释") {
                assert!(document.to_string().contains("# 保留注释"));
            }
        }
        for invalid in [
            "[scope_menu]\nitems=[{id='all'},{id='all'}]",
            "[scope_menu]\nitems=[{id='unknown'}]",
            "[scope_menu]\nitems=[{id='web:'}]",
        ] {
            assert!(crate::config::parse_and_complete(invalid).is_err());
        }
    }
}
