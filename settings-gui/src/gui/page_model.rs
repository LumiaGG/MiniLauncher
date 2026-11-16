//! Page operations use stable provider IDs, never UI row indices.
use super::*;
use mini_launcher::web_search::{Engine, WebSearchSettings};

pub(super) fn web(value: &Value) -> Result<WebSearchSettings, String> {
    value
        .get("web_search")
        .ok_or("缺少网页搜索配置")?
        .clone()
        .try_into()
        .map_err(|e: toml::de::Error| e.to_string())
}

pub(super) fn commit_web(draft: &mut Draft, web: WebSearchSettings) -> Result<(), String> {
    let mut candidate = draft.value.clone();
    candidate["web_search"] = Value::try_from(web).map_err(|e| e.to_string())?;
    let text = toml::to_string(&candidate).map_err(|e| e.to_string())?;
    let (normalized, _) = config::parse_and_complete(&text)?;
    plugin_catalog::validate_web_keywords(&normalized.plugins, &normalized.web_search)?;
    candidate["web_search"] = Value::try_from(normalized.web_search).map_err(|e| e.to_string())?;
    draft.value = candidate;
    Ok(())
}

pub(super) fn put_engine(
    draft: &mut Draft,
    original: Option<&str>,
    name: &str,
    keywords: &str,
    url: &str,
) -> Result<(), String> {
    let mut web = web(&draft.value)?;
    let id = if let Some(id) = original {
        id.to_string()
    } else {
        let mut n = 1;
        while web.engines.iter().any(|e| e.id == format!("custom{n}"))
            || draft
                .value
                .get("scope_menu")
                .and_then(|v| v.get("items"))
                .and_then(Value::as_array)
                .is_some_and(|items| {
                    items.iter().any(|item| {
                        item.get("id").and_then(Value::as_str) == Some(&format!("web:custom{n}"))
                    })
                })
        {
            n += 1;
        }
        format!("custom{n}")
    };
    let engine = Engine {
        id: id.clone(),
        name: name.trim().into(),
        keywords: keywords
            .split(|c: char| c.is_whitespace() || matches!(c, ',' | '，' | ';' | '；'))
            .filter(|s| !s.is_empty())
            .map(str::to_string)
            .collect(),
        url: url.trim().into(),
    };
    if engine.keywords.is_empty() {
        return Err("至少填写一个触发关键词".into());
    }
    if let Some(original) = original {
        let target = web
            .engines
            .iter_mut()
            .find(|e| e.id == original)
            .ok_or("引擎已不存在，请重新打开编辑窗口")?;
        *target = engine;
    } else {
        web.engines.push(engine);
        if web.engines.len() == 1 {
            web.default_engine = id;
        }
    }
    commit_web(draft, web)
}

pub(super) fn delete_engine(draft: &mut Draft, id: &str) -> Result<(), String> {
    let mut web = web(&draft.value)?;
    if web.engines.len() <= 1 {
        return Err("至少保留一个搜索引擎".into());
    }
    let index = web
        .engines
        .iter()
        .position(|e| e.id == id)
        .ok_or("引擎已不存在")?;
    web.engines.remove(index);
    if web.default_engine == id {
        web.default_engine = web.engines[index.min(web.engines.len() - 1)].id.clone();
    }
    commit_web(draft, web)
}

pub(super) fn move_id(items: &mut Vec<Value>, id: &str, target: &str) -> bool {
    let source = items
        .iter()
        .position(|e| e.get("id").and_then(Value::as_str) == Some(id));
    let target = items
        .iter()
        .position(|e| e.get("id").and_then(Value::as_str) == Some(target));
    match (source, target) {
        (Some(from), Some(to)) if from != to => {
            let item = items.remove(from);
            items.insert(to, item);
            true
        }
        _ => false,
    }
}

#[cfg(test)]
mod tests {
    use super::{delete_engine, move_id, put_engine, web};
    use crate::model::Draft;
    use mini_launcher::config;
    use std::path::PathBuf;
    use toml::Value;
    fn draft() -> Draft {
        let base: Value = toml::from_str(config::DEFAULT_SETTINGS).unwrap();
        Draft {
            path: PathBuf::new(),
            base: base.clone(),
            value: base,
        }
    }
    #[test]
    fn editing_an_engine_preserves_default_and_scope_identity_and_rejects_conflicts_atomically() {
        let mut draft = draft();
        put_engine(
            &mut draft,
            Some("google"),
            "谷歌",
            "gg，google",
            "https://www.google.com/search?q={query}",
        )
        .unwrap();
        assert_eq!(web(&draft.value).unwrap().default_engine, "google");
        assert_eq!(
            web(&draft.value).unwrap().engines[0].keywords,
            ["gg", "google"]
        );
        let before = draft.value.clone();
        assert!(put_engine(
            &mut draft,
            Some("google"),
            "冲突",
            "bing",
            "https://example.com/?q={query}"
        )
        .is_err());
        assert_eq!(draft.value, before);
        assert!(put_engine(
            &mut draft,
            None,
            "新引擎",
            "",
            "https://example.com/?q={query}"
        )
        .is_err());
    }
    #[test]
    fn new_engine_does_not_reuse_a_removed_sources_saved_menu_identity() {
        let mut draft = draft();
        draft.value["scope_menu"]["items"]
            .as_array_mut()
            .unwrap()
            .push(
                toml::from_str::<Value>("id='web:custom1'\nname='旧引擎名称'\nvisible=false")
                    .unwrap(),
            );
        put_engine(
            &mut draft,
            None,
            "新引擎",
            "new",
            "https://example.com/?q={query}",
        )
        .unwrap();
        assert_eq!(
            web(&draft.value).unwrap().engines.last().unwrap().id,
            "custom2"
        );
    }

    #[test]
    fn deleting_default_selects_a_remaining_engine_and_keeps_scope_preferences() {
        let mut draft = draft();
        let scope = draft.value["scope_menu"].clone();
        delete_engine(&mut draft, "google").unwrap();
        assert_eq!(web(&draft.value).unwrap().default_engine, "bing");
        assert_eq!(draft.value["scope_menu"], scope);
        let before = draft.value.clone();
        assert!(delete_engine(&mut draft, "bing").is_err());
        assert_eq!(draft.value, before);
    }
    #[test]
    fn dragging_reorders_ids_without_losing_hidden_or_renamed_preferences() {
        let mut items: Vec<Value> = toml::from_str::<Value>("items=[{id='all',name='',visible=true},{id='files',name='我的文件',visible=false},{id='web:bing',name='',visible=true}]").unwrap()["items"].as_array().unwrap().clone();
        let before = items[1].clone();
        assert!(move_id(&mut items, "files", "all"));
        assert_eq!(items[0], before);
        assert!(move_id(&mut items, "files", "web:bing"));
        assert_eq!(items[2], before);
        let snapshot = items.clone();
        assert!(!move_id(&mut items, "missing", "all"));
        assert_eq!(items, snapshot);
    }
}
