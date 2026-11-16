//! Shared results and actions; matching terms are initialized only for indexed search.
use pinyin::ToPinyin;
use std::path::PathBuf;

#[derive(Clone, Debug, PartialEq)]
pub enum Action {
    Open(String),
    Launch {
        executable: String,
        args: Vec<String>,
    },
    Copy(String),
    Settings,
    PluginSettings,
    Plugin {
        plugin_id: String,
        action_id: String,
    },
}

#[derive(Clone, Debug, PartialEq)]
pub struct ContextAction {
    pub title: String,
    pub action: Action,
    pub confirm: bool,
}

#[derive(Clone, Debug)]
pub struct Entry {
    pub title: String,
    pub subtitle: String,
    pub key: String,
    pub action: Action,
    pub icon_path: Option<String>,
    pub kind: EntryKind,
    pub context_actions: Vec<ContextAction>,
    pub adaptive_height: bool,
    pub text_panel: Option<std::sync::Arc<TextPanel>>,
    pub preview: Option<ImagePreview>,
    pub paste_after_execute: bool,
    /// Optional user-facing verb for opaque plugin callbacks.
    pub action_label: Option<String>,
    pub drag: Option<crate::drag_data::DragSource>,
    /// Host-assigned query priority; applied before cross-source deduplication.
    pub(crate) query_priority: i32,
    pub(crate) terms: std::sync::OnceLock<SearchTerms>,
}

mod text_panel;
pub use text_panel::{PanelState, TextBlock, TextLayout, TextPanel};

#[derive(Clone, Debug, serde::Deserialize, PartialEq, Eq, Hash)]
#[serde(deny_unknown_fields)]
pub struct ImagePreview {
    pub path: String,
    #[serde(default)]
    pub original_path: Option<String>,
    pub width: u32,
    pub height: u32,
}

impl Entry {
    /// Compare the displayed row, independently of its executable payload and
    /// lazily initialized search cache. Payloads must still be replaced even
    /// when this returns true.
    pub fn same_presentation(&self, other: &Self) -> bool {
        self.key == other.key
            && self.title == other.title
            && self.subtitle == other.subtitle
            && self.icon_path == other.icon_path
            && self.kind == other.kind
            && self.context_actions == other.context_actions
            && self.adaptive_height == other.adaptive_height
            && self.text_panel == other.text_panel
            && self.preview == other.preview
            && self.paste_after_execute == other.paste_after_execute
            && self.action_label == other.action_label
    }

    pub fn new(title: String, subtitle: String, action: Action, kind: &'static str) -> Self {
        let (key, icon_path) = match &action {
            Action::Open(path) => (
                path.to_lowercase(),
                if path.contains("://") || path.starts_with("ms-settings:") {
                    None
                } else {
                    Some(path.clone())
                },
            ),
            Action::Copy(value) => (format!("copy:{value}"), None),
            Action::Launch { executable, args } => (
                format!("launch:{}", serde_json::json!([executable, args])),
                Some(executable.clone()),
            ),
            Action::Settings => ("settings".into(), None),
            Action::PluginSettings => ("plugin-settings".into(), None),
            Action::Plugin {
                plugin_id,
                action_id,
            } => (format!("plugin:{plugin_id}:{action_id}"), None),
        };
        Self {
            terms: std::sync::OnceLock::new(),
            title,
            subtitle,
            key,
            action,
            icon_path,
            kind: kind.into(),
            context_actions: Vec::new(),
            adaptive_height: false,
            text_panel: None,
            preview: None,
            paste_after_execute: false,
            action_label: None,
            drag: None,
            query_priority: 0,
        }
    }

    pub fn is_app(&self) -> bool {
        matches!(
            self.kind,
            EntryKind::App | EntryKind::System | EntryKind::Alias
        ) && matches!(self.action, Action::Open(_))
    }

    pub fn action_label(&self) -> &str {
        if self.text_panel.as_ref().is_some_and(|panel| panel.state != PanelState::Ready) {
            return if self.text_panel.as_ref().is_some_and(|panel| panel.retry) {
                "重试"
            } else {
                "等待结果"
            };
        }
        if self.paste_after_execute {
            return "粘贴";
        }
        match &self.action {
            Action::Open(_) => "打开",
            Action::Launch { .. } => "运行",
            Action::Copy(_) => "复制",
            Action::Settings | Action::PluginSettings => "打开设置",
            Action::Plugin { .. } => self.action_label.as_deref().unwrap_or("执行"),
        }
    }

    pub fn drag_source(&self) -> Option<crate::drag_data::DragSource> {
        if let Some(source) = &self.drag {
            return Some(source.clone());
        }
        let data = match &self.action {
            Action::Copy(text) if !text.is_empty() => {
                Some(crate::drag_data::DragData::Text { text: text.clone() })
            }
            Action::Open(path) if PathBuf::from(path).is_absolute() => {
                Some(crate::drag_data::DragData::Files {
                    paths: vec![PathBuf::from(path)],
                })
            }
            Action::Open(url) if url.starts_with("https://") || url.starts_with("http://") => {
                Some(crate::drag_data::DragData::Text { text: url.clone() })
            }
            _ => None,
        };
        data.map(crate::drag_data::DragSource::Data)
    }
}

#[derive(Clone, Debug)]
pub(crate) struct SearchTerms {
    pub normalized: String,
    pub pinyin: String,
    pub initials: String,
    pub word_initials: String,
}
impl Entry {
    pub(crate) fn search_terms(&self) -> &SearchTerms {
        self.terms.get_or_init(|| {
            let title = &self.title;
            let mut full = String::new();
            let mut initials = String::new();
            for (character, syllable) in title.chars().zip(title.as_str().to_pinyin()) {
                if let Some(syllable) = syllable {
                    full.push_str(syllable.plain());
                    if let Some(initial) = syllable.plain().chars().next() {
                        initials.push(initial);
                    }
                } else {
                    full.extend(character.to_lowercase());
                    initials.extend(character.to_lowercase());
                }
            }

            SearchTerms {
                normalized: title.to_lowercase(),
                word_initials: title
                    .split(|character: char| !character.is_alphanumeric())
                    .filter_map(|word| word.chars().next())
                    .flat_map(char::to_lowercase)
                    .collect(),
                pinyin: full,
                initials,
            }
        })
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum EntryKind {
    App,
    System,
    Alias,
    File,
    Plugin,
    Web,
    Calculator,
    Settings,
    Other(&'static str),
}
impl EntryKind {
    pub fn label(self) -> &'static str {
        match self {
            Self::App => "应用",
            Self::System => "系统",
            Self::Alias => "别名",
            Self::File => "文件",
            Self::Plugin => "插件",
            Self::Web => "网页",
            Self::Calculator => "计算",
            Self::Settings => "设置",
            Self::Other(label) => label,
        }
    }
}
impl From<&'static str> for EntryKind {
    fn from(label: &'static str) -> Self {
        match label {
            "应用" => Self::App,
            "系统" => Self::System,
            "别名" => Self::Alias,
            "文件" => Self::File,
            "插件" => Self::Plugin,
            "网页" => Self::Web,
            "计算" => Self::Calculator,
            "设置" => Self::Settings,
            other => Self::Other(other),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn action_labels_follow_execution_semantics_and_change_presentation() {
        let mut entry = Entry::new("Result".into(), String::new(), Action::Copy("text".into()), "插件");
        assert_eq!(entry.action_label(), "复制");
        let old = entry.clone();
        entry.paste_after_execute = true;
        assert_eq!(entry.action_label(), "粘贴");
        assert!(!entry.same_presentation(&old));
        entry.paste_after_execute = false;
        entry.action = Action::Plugin { plugin_id: "generic".into(), action_id: "save".into() };
        assert_eq!(entry.action_label(), "执行");
        let old = entry.clone();
        entry.action_label = Some("保存".into());
        assert_eq!(entry.action_label(), "保存");
        assert!(!entry.same_presentation(&old));
        entry.text_panel = Some(std::sync::Arc::new(TextPanel {
            state: PanelState::Error, retry: true, ..Default::default()
        }));
        assert_eq!(entry.action_label(), "重试");
    }
}
