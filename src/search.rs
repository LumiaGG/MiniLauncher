pub use crate::app_index::build_index;
use crate::calculator::calculate;
use crate::config::History;
pub use crate::result::{Action, ContextAction, Entry, ImagePreview, TextPanel};

fn match_score(text: &str, query: &str) -> Option<i64> {
    if text == query {
        return Some(1000);
    }
    if text.starts_with(query) {
        return Some(800 - text.chars().count() as i64);
    }
    if let Some(offset) = text.find(query) {
        return Some(600 - offset as i64);
    }
    let mut needle = query.chars();
    let mut next = needle.next();
    let mut first = None;
    let mut last = 0;
    for (index, character) in text.chars().enumerate() {
        if Some(character) == next {
            first.get_or_insert(index);
            last = index;
            next = needle.next();
            if next.is_none() {
                break;
            }
        }
    }
    if next.is_none() {
        Some(200 - (last - first.unwrap_or(0)) as i64)
    } else {
        None
    }
}

pub fn pin_query(input: &str) -> String {
    input
        .split_whitespace()
        .collect::<Vec<_>>()
        .join(" ")
        .to_lowercase()
}

pub fn query(index: &[Entry], input: &str, history: &History, limit: usize) -> Vec<Entry> {
    query_cancellable(index, input, history, limit, || false).unwrap_or_default()
}

/// Stop obsolete searches before scoring the remaining index or cloning results.
pub fn query_cancellable(
    index: &[Entry],
    input: &str,
    history: &History,
    limit: usize,
    mut cancelled: impl FnMut() -> bool,
) -> Option<Vec<Entry>> {
    if cancelled() {
        return None;
    }
    let input = input.trim();
    if input == ">settings" || input == "设置" || input == ">plugins" {
        return Some(vec![Entry::new(
            if input == ">plugins" {
                "插件管理"
            } else {
                "MiniLauncher 设置"
            }
            .into(),
            "打开图形设置，修改后自动保存并生效".into(),
            if input == ">plugins" {
                Action::PluginSettings
            } else {
                Action::Settings
            },
            "设置",
        )]);
    }
    let removed_view = input == ">removed";
    let normalized = if removed_view { "" } else { input }.to_lowercase();
    let tokens: Vec<_> = normalized.split_whitespace().collect();
    let pin_query = pin_query(input);
    let mut stopped = false;
    let mut matches: Vec<_> = index
        .iter()
        .take_while(|_| {
            stopped = cancelled();
            !stopped
        })
        .filter_map(|entry| {
            let usage = history.get(&entry.key);
            let removed = entry.is_app() && usage.is_some_and(|u| u.removed);
            if removed != removed_view {
                return None;
            }
            let terms = entry.search_terms();
            let mut score = 0;
            for token in &tokens {
                let mut best = match_score(&terms.normalized, token);
                if !best.is_some_and(|s| s >= 980) {
                    best = best.max(match_score(&terms.pinyin, token).map(|s| s - 20));
                }
                if !best.is_some_and(|s| s >= 960) {
                    best = best.max(match_score(&terms.initials, token).map(|s| s - 40));
                }
                if !best.is_some_and(|s| s >= 920) {
                    // Keep literal exact matches ahead even with the maximum usage bonus (60).
                    best = best.max(match_score(&terms.word_initials, token).map(|s| s - 80));
                }
                score += best?;
            }
            if let Some(usage) = usage {
                score += i64::from(usage.count.min(30)) * 2;
            }
            let pinned = entry.is_app()
                && !pin_query.is_empty()
                && usage.is_some_and(|u| u.pinned_queries.contains(&pin_query));
            let last_used = usage.map_or(0, |u| u.last_used);
            Some((entry, pinned, score, last_used))
        })
        .collect();
    if stopped || cancelled() {
        return None;
    }
    matches.sort_by(|a, b| {
        b.1.cmp(&a.1)
            .then_with(|| b.2.cmp(&a.2))
            .then_with(|| b.3.cmp(&a.3))
            .then_with(|| a.0.title.cmp(&b.0.title))
    });
    if cancelled() {
        return None;
    }
    let mut results: Vec<_> = matches
        .into_iter()
        .take(limit)
        .map(|(e, _, _, _)| e.clone())
        .collect();
    if let Some(value) = calculate(input) {
        results.insert(
            0,
            Entry::new(
                value.clone(),
                format!("{input}  ·  Enter 复制结果"),
                Action::Copy(value),
                "计算",
            ),
        );
        results.truncate(limit);
    }
    Some(results)
}

pub fn url_encode(value: &str) -> String {
    value
        .bytes()
        .map(|byte| match byte {
            b'A'..=b'Z' | b'a'..=b'z' | b'0'..=b'9' | b'-' | b'_' | b'.' | b'~' => {
                (byte as char).to_string()
            }
            _ => format!("%{byte:02X}"),
        })
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;
    fn app(name: &str) -> Entry {
        Entry::new(
            name.into(),
            String::new(),
            Action::Open(name.into()),
            "应用",
        )
    }
    #[test]
    fn obsolete_queries_stop_scoring_and_live_queries_keep_their_ranking() {
        let index: Vec<_> = (0..100).map(|i| app(&format!("Sample {i}"))).collect();
        let history = History::new();
        let mut checks = 0;
        assert!(query_cancellable(&index, "sample", &history, 10, || {
            checks += 1;
            checks >= 6
        })
        .is_none());
        assert_eq!(
            checks, 6,
            "cancellation must not scan the rest of the index"
        );
        for text in ["sample", ">settings", "设置", ">removed", "1+2", "missing"] {
            let expected = query(&index, text, &history, 10);
            let actual = query_cancellable(&index, text, &history, 10, || false).unwrap();
            assert_eq!(actual.len(), expected.len());
            assert!(actual
                .iter()
                .zip(&expected)
                .all(|(a, b)| a.same_presentation(b)));
            assert!(query_cancellable(&index, text, &history, 10, || true).is_none());
        }
    }

    #[test]
    fn chinese_pinyin_and_initials_work() {
        let index = vec![app("网易云音乐"), app("记事本")];
        for input in ["网易", "wangyiyun", "wyyy", "wang yi"] {
            assert_eq!(
                query(&index, input, &History::new(), 6)[0].title,
                "网易云音乐"
            );
        }
        assert_eq!(query(&index, "jsb", &History::new(), 6)[0].title, "记事本");
    }
    #[test]
    fn exact_match_beats_frequent_partial_match() {
        let index = vec![app("Code"), app("Visual Studio Code")];
        let mut history = History::new();
        history.insert(
            index[1].key.clone(),
            crate::config::Usage {
                count: 500,
                last_used: 10,
                ..Default::default()
            },
        );
        assert_eq!(query(&index, "code", &history, 6)[0].title, "Code");
        assert_eq!(
            query(&index, "vsc", &history, 6)[0].title,
            "Visual Studio Code"
        );
    }
    #[test]
    fn word_initials_beat_frequent_scattered_letters() {
        let index = vec![
            app("Tailscale"),
            app("Task Scheduler"),
            app("Android Studio Canary"),
            app("Android Studio Canary Preview"),
        ];
        let mut history = History::new();
        for entry in &index[..2] {
            history.insert(
                entry.key.clone(),
                crate::config::Usage {
                    count: 500,
                    last_used: 10,
                    ..Default::default()
                },
            );
        }
        for input in ["asc", " ASC "] {
            let results = query(&index, input, &history, 6);
            assert_eq!(results[0].title, "Android Studio Canary");
            assert_eq!(results[1].title, "Android Studio Canary Preview");
        }
    }
    #[test]
    fn literal_names_and_pins_take_priority_over_word_initials() {
        let index = vec![app("ASC"), app("Android-Studio (Canary)")];
        let mut history = History::new();
        history.insert(
            index[1].key.clone(),
            crate::config::Usage {
                count: 500,
                last_used: 10,
                ..Default::default()
            },
        );
        assert_eq!(query(&index, "asc", &history, 6)[0].title, "ASC");
        assert_eq!(
            query(&index[1..], "asc", &history, 6)[0].title,
            "Android-Studio (Canary)"
        );
        history
            .get_mut(&index[1].key)
            .unwrap()
            .pinned_queries
            .insert("asc".into());
        assert_eq!(
            query(&index, "asc", &history, 6)[0].title,
            "Android-Studio (Canary)"
        );
    }
    #[test]
    fn pins_and_removals_only_manage_apps_and_can_be_restored() {
        let mut index = vec![app("Code"), app("Visual Studio Code")];
        let mut plugin = Entry::new(
            "Plugin Code".into(),
            String::new(),
            Action::Open("Code".into()),
            "插件",
        );
        plugin.icon_path = Some("plugin.exe".into());
        index.push(plugin);
        let mut history = History::new();
        history
            .entry(index[1].key.clone())
            .or_default()
            .pinned_queries
            .insert(pin_query("code"));
        assert_eq!(
            query(&index, "code", &history, 6)[0].title,
            "Visual Studio Code"
        );
        assert!(query(&index, "unmatched", &history, 6).is_empty());
        assert_eq!(query(&index, "co", &history, 6)[0].title, "Code");
        assert_eq!(
            query(&index, " CODE ", &history, 6)[0].title,
            "Visual Studio Code"
        );
        assert_eq!(pin_query(" Visual   STUDIO "), "visual studio");
        history.entry(index[0].key.clone()).or_default().removed = true;
        let results = query(&index, "code", &history, 6);
        assert_eq!(results.len(), 2);
        assert!(results
            .iter()
            .any(|entry| entry.kind == crate::result::EntryKind::Plugin));
        assert_eq!(query(&index, ">removed", &history, 6)[0].title, "Code");
        history.get_mut(&index[0].key).unwrap().removed = false;
        assert_eq!(query(&index, "code", &history, 6).len(), 3);
        let saved = serde_json::to_vec(&history).unwrap();
        let loaded: History = serde_json::from_slice(&saved).unwrap();
        assert!(loaded[&index[1].key].pinned_queries.contains("code"));
        let legacy: History =
            serde_json::from_str(r#"{"code":{"count":2,"last_used":10}}"#).unwrap();
        assert!(legacy["code"].pinned_queries.is_empty() && !legacy["code"].removed);
    }
    #[test]
    fn calculator_handles_precedence_invalid_input_and_zero() {
        assert_eq!(calculate("(2 + 3) * 4 - 6 / 2"), Some("17".into()));
        assert_eq!(calculate("-2 * (3 + 1)"), Some("-8".into()));
        for invalid in ["1/0", "2+", "(1+2", "NaN+1", "1+2abc"] {
            assert_eq!(calculate(invalid), None);
        }
    }
    #[test]
    fn web_search_encodes_unicode_and_query_separators() {
        let settings = crate::web_search::WebSearchSettings::default();
        let (engine, query) = settings.explicit("g 中文 & Rust").unwrap();
        let entry = engine.entry(query);
        let Action::Open(url) = &entry.action else {
            panic!("expected URL");
        };
        assert!(url.ends_with("%E4%B8%AD%E6%96%87%20%26%20Rust"));
    }
}
