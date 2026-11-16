//! Protocol v2 text sections. Each section has a stable id and capabilities.
use serde::Deserialize;
use std::collections::HashSet;

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum PanelState {
    #[default]
    Ready,
    Loading,
    Empty,
    Error,
}
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum TextLayout {
    #[default]
    Auto,
    Horizontal,
    Vertical,
}

#[derive(Clone, Debug, PartialEq, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct TextBlock {
    pub id: String,
    pub title: String,
    pub text: String,
    #[serde(default = "selectable_default")]
    pub selectable: bool,
}
fn selectable_default() -> bool {
    true
}

#[derive(Clone, Debug, Default, PartialEq, Deserialize)]
#[serde(default, deny_unknown_fields)]
pub struct TextPanel {
    pub blocks: Vec<TextBlock>,
    pub layout: TextLayout,
    pub state: PanelState,
    pub message: String,
    /// Repeats the current query rather than executing the result's action.
    pub retry: bool,
}
impl TextPanel {
    pub fn has_status(&self) -> bool {
        self.state != PanelState::Ready || !self.message.is_empty() || self.retry
    }
    pub fn status(&self) -> &str {
        if !self.message.is_empty() {
            return &self.message;
        }
        match self.state {
            PanelState::Ready => "",
            PanelState::Loading => "正在加载…",
            PanelState::Empty => "暂无内容",
            PanelState::Error => "加载失败",
        }
    }
    pub fn validate(&self) -> Result<(), String> {
        if self.blocks.len() > 4 || (self.blocks.is_empty() && !self.has_status()) {
            return Err("文本面板需要 1–4 个区块，或明确的加载/空/错误状态".into());
        }
        let mut ids = HashSet::new();
        for block in &self.blocks {
            if block.id.is_empty()
                || block.id.len() > 64
                || block.id.chars().any(char::is_control)
                || !ids.insert(&block.id)
            {
                return Err("文本区块 id 为空、重复或超过限制".into());
            }
            if block.title.is_empty()
                || block.title.len() > 128
                || block.title.chars().any(char::is_control)
            {
                return Err("文本区块标题为空、无效或超过 128 字节".into());
            }
            if block.text.len() > 65536 || block.text.contains('\0') {
                return Err("文本区块内容超过 64 KiB 或包含 NUL".into());
            }
        }
        if self.message.len() > 4096 || self.message.contains('\0') {
            return Err("面板状态消息无效或过长".into());
        }
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn generic_sections_validate_ids_and_reject_legacy_fields() {
        let mut panel: TextPanel =
            serde_json::from_str(r#"{"blocks":[{"id":"log","title":"日志","text":"中文😀"}]}"#)
                .unwrap();
        panel.validate().unwrap();
        assert!(panel.blocks[0].selectable);
        panel.blocks.push(panel.blocks[0].clone());
        assert!(panel.validate().is_err());
        panel.blocks.pop();
        panel.blocks[0].text.push('\0');
        assert!(panel.validate().is_err());
        assert!(
            serde_json::from_str::<TextPanel>(r#"{"original":"old","translated":"旧"}"#).is_err()
        );
    }
    #[test]
    fn loading_empty_and_error_work_without_sections() {
        for state in [PanelState::Loading, PanelState::Empty, PanelState::Error] {
            let panel = TextPanel {
                state,
                retry: state == PanelState::Error,
                ..Default::default()
            };
            panel.validate().unwrap();
            assert!(panel.has_status());
            assert!(!panel.status().is_empty());
        }
    }
}
