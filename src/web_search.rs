use crate::search::{Action, Entry};
use serde::{Deserialize, Serialize};
use std::collections::HashSet;

#[derive(Clone, Debug, PartialEq, Deserialize, Serialize)]
#[serde(default, deny_unknown_fields)]
pub struct WebSearchSettings {
    pub enabled: bool,
    pub mode: String,
    pub default_engine: String,
    pub engines: Vec<Engine>,
}

#[derive(Clone, Debug, PartialEq, Deserialize, Serialize)]
#[serde(deny_unknown_fields)]
pub struct Engine {
    pub id: String,
    pub name: String,
    pub keywords: Vec<String>,
    pub url: String,
}

impl Default for WebSearchSettings {
    fn default() -> Self {
        Self {
            enabled: true,
            mode: "fallback".into(),
            default_engine: "google".into(),
            engines: vec![
                Engine {
                    id: "google".into(),
                    name: "Google".into(),
                    keywords: vec!["g".into()],
                    url: "https://www.google.com/search?q={query}".into(),
                },
                Engine {
                    id: "bing".into(),
                    name: "Bing".into(),
                    keywords: vec!["bing".into()],
                    url: "https://www.bing.com/search?q={query}".into(),
                },
            ],
        }
    }
}

impl WebSearchSettings {
    pub fn validate(&mut self, file_keyword: &str) -> Result<(), String> {
        if !matches!(self.mode.as_str(), "fallback" | "always" | "keyword_only") {
            return Err("web_search.mode 必须为 fallback、always 或 keyword_only".into());
        }
        let mut ids = HashSet::new();
        let mut keywords = HashSet::new();
        for engine in &mut self.engines {
            if engine.id.trim().is_empty()
                || !ids.insert(engine.id.clone())
                || engine.name.trim().is_empty()
            {
                return Err("Web 搜索引擎需要非空名称和唯一 id".into());
            }
            let address = engine
                .url
                .strip_prefix("https://")
                .or_else(|| engine.url.strip_prefix("http://"));
            if !address
                .is_some_and(|rest| !rest.split(['/', '?', '#']).next().unwrap_or("").is_empty())
                || !engine.url.contains("{query}")
                || engine.url.chars().any(char::is_whitespace)
            {
                return Err("Web 搜索地址必须为 HTTP/HTTPS URL，且包含 {query}".into());
            }
            for keyword in &mut engine.keywords {
                *keyword = keyword.trim().to_ascii_lowercase();
                if keyword.is_empty()
                    || keyword.chars().any(char::is_whitespace)
                    || keyword.starts_with('>')
                    || keyword == file_keyword
                    || keyword == "设置"
                    || !keywords.insert(keyword.clone())
                {
                    return Err(format!(
                        "Web 搜索关键词无效、重复或与内置命令冲突：{keyword}"
                    ));
                }
            }
        }
        if !ids.contains(&self.default_engine) {
            return Err("web_search.default_engine 必须指向已配置的引擎".into());
        }
        Ok(())
    }

    pub fn explicit<'a>(&'a self, text: &'a str) -> Option<(&'a Engine, &'a str)> {
        if !self.enabled {
            return None;
        }
        let text = text.trim();
        let split = text.find(char::is_whitespace).unwrap_or(text.len());
        let keyword = text[..split].to_ascii_lowercase();
        self.engines
            .iter()
            .find(|engine| engine.keywords.contains(&keyword))
            .map(|engine| (engine, text[split..].trim()))
    }

    pub fn fallback(&self, text: &str, empty: bool) -> Option<Entry> {
        let text = text.trim();
        if !self.enabled
            || text.is_empty()
            || text.starts_with('>')
            || text == "设置"
            || self.explicit(text).is_some()
            || self.mode == "keyword_only"
            || (self.mode == "fallback" && !empty)
        {
            return None;
        }
        self.engines
            .iter()
            .find(|engine| engine.id == self.default_engine)
            .map(|engine| engine.entry(text))
    }
}

impl Engine {
    pub fn entry(&self, query: &str) -> Entry {
        Entry::new(
            format!("使用 {} 搜索「{query}」", self.name),
            "Enter 在默认浏览器中搜索".into(),
            Action::Open(
                self.url
                    .replace("{query}", &crate::search::url_encode(query)),
            ),
            "网页",
        )
    }
}
