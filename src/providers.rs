use crate::{config::EverythingSettings, search::Entry};
use std::collections::HashSet;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Source {
    Apps = 0,
    Files = 1,
    Plugins = 2,
}

pub struct Update {
    pub kind: UpdateKind,
    pub generation: u64,
    pub source: Source,
    pub entries: Vec<Entry>,
    pub exclusive: bool,
    pub pending: bool,
    pub status: Option<String>,
}

/// A state notification never replaces a provider's displayed results.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum UpdateKind {
    State,
    Results,
}

pub type Emit = std::sync::Arc<dyn Fn(Update) + Send + Sync>;

#[derive(Default)]
pub struct Groups {
    groups: [Vec<Entry>; 3],
    exclusive: Option<Source>,
    pub pending: [bool; 3],
    statuses: [Option<String>; 3],
    refreshed: [bool; 3],
    priorities: [i32; 2],
}

impl Groups {
    pub fn set_priorities(&mut self, apps: i32, files: i32) {
        self.priorities = [apps, files];
    }

    pub fn begin(&mut self, pending: [bool; 3], retain: bool, exclusive: Option<Source>) {
        if !retain {
            *self = Self::default();
        }
        for (index, active) in pending.iter().enumerate() {
            if !active {
                self.groups[index].clear();
            }
        }
        self.pending = pending;
        self.exclusive = exclusive;
        self.statuses = Default::default();
        self.refreshed = [false; 3];
    }

    pub fn has_results(&self) -> bool {
        self.groups.iter().any(|entries| !entries.is_empty())
    }

    pub fn is_pending(&self) -> bool {
        if let Some(source) = self.exclusive {
            self.pending[source as usize]
        } else {
            self.pending.iter().any(|pending| *pending)
        }
    }

    pub fn current_entries(&self, source: Source) -> &[Entry] {
        if self.refreshed[source as usize] {
            &self.groups[source as usize]
        } else {
            &[]
        }
    }

    /// Apply provider data and metadata without building a displayed snapshot.
    pub fn update(&mut self, update: Update) {
        let position = update.source as usize;
        if update.kind == UpdateKind::Results {
            self.groups[position] = update.entries;
            self.refreshed[position] = true;
        }
        self.pending[position] = update.pending;
        self.statuses[position] = update.status;
        if update.exclusive {
            self.exclusive = Some(update.source);
        }
    }

    pub fn apply(&mut self, update: Update) -> Vec<Entry> {
        self.update(update);
        let priorities = self.priorities;
        let mut entries: Vec<_> = self
            .groups
            .iter()
            .enumerate()
            .filter(|(index, _)| {
                self.exclusive
                    .is_none_or(|source| *index == source as usize)
            })
            .flat_map(|(index, entries)| {
                entries.iter().map(move |entry| {
                    let priority = if index == Source::Plugins as usize {
                        entry.query_priority
                    } else {
                        priorities[index]
                    };
                    (priority, entry)
                })
            })
            .collect();
        // Keep source order and each provider's own order for equal priorities.
        // Rank before deduplication so lower-priority duplicates cannot hide a result.
        entries.sort_by(|a, b| b.0.cmp(&a.0));
        let mut seen = HashSet::new();
        entries
            .into_iter()
            .map(|(_, entry)| entry)
            .filter(|entry| seen.insert(entry.key.clone()))
            .cloned()
            .collect()
    }

    pub fn append_web(
        &self,
        results: &mut Vec<Entry>,
        text: &str,
        settings: &crate::web_search::WebSearchSettings,
    ) {
        // All sources start pending, including sources which will return no results.
        if self.exclusive.is_none() && !self.pending.iter().any(|pending| *pending) {
            if let Some(entry) = settings.fallback(text, results.is_empty()) {
                results.push(entry);
            }
        }
    }

    pub fn status(&self) -> Option<String> {
        if let Some(source) = self.exclusive {
            self.statuses[source as usize].clone()
        } else {
            self.statuses.iter().flatten().next().cloned()
        }
    }
}

pub fn files_query<'a>(text: &'a str, settings: &EverythingSettings) -> Option<(&'a str, bool)> {
    if !settings.enabled {
        return None;
    }
    let trimmed = text.trim();
    if trimmed == settings.keyword {
        return Some(("", true));
    }
    if let Some(rest) = trimmed.strip_prefix(&settings.keyword) {
        if rest.starts_with(char::is_whitespace) {
            return Some((rest.trim_start(), true));
        }
    }
    if settings.global_search && trimmed.chars().count() >= 2 && !trimmed.starts_with('>') {
        Some((trimmed, false))
    } else {
        None
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::search::Action;
    fn entry(title: &str, path: &str) -> Entry {
        Entry::new(
            title.into(),
            String::new(),
            Action::Open(path.into()),
            "应用",
        )
    }

    #[test]
    fn query_state_preserves_results_but_a_real_empty_response_clears_them() {
        let mut groups = Groups::default();
        let update = |kind, entries, pending| Update {
            kind,
            generation: 1,
            source: Source::Plugins,
            entries,
            exclusive: true,
            pending,
            status: None,
        };
        groups.apply(update(
            UpdateKind::Results,
            vec![entry("old", "old")],
            false,
        ));
        groups.begin([false, false, true], true, Some(Source::Plugins));
        assert!(groups.current_entries(Source::Plugins).is_empty());
        groups.update(update(UpdateKind::State, Vec::new(), true));
        assert_eq!(groups.groups[Source::Plugins as usize][0].title, "old");
        assert!(groups.current_entries(Source::Plugins).is_empty());
        assert!(groups.is_pending());
        let results = groups.apply(update(UpdateKind::Results, Vec::new(), false));
        assert!(results.is_empty());
        assert!(!groups.has_results());
        assert!(!groups.pending.iter().any(|p| *p));
    }

    #[test]
    fn partial_responses_merge_retained_sources_until_their_own_response() {
        let mut groups = Groups::default();
        let update = |source, title, pending| Update {
            kind: UpdateKind::Results,
            generation: 1,
            source,
            entries: vec![entry(title, title)],
            exclusive: false,
            pending,
            status: None,
        };
        groups.apply(update(Source::Apps, "old app", false));
        groups.apply(update(Source::Plugins, "old plugin", false));
        groups.begin([true, false, true], true, None);
        let results = groups.apply(update(Source::Apps, "new app", false));
        assert_eq!(
            results.iter().map(|e| e.title.as_str()).collect::<Vec<_>>(),
            ["new app", "old plugin"]
        );
        assert!(groups.current_entries(Source::Plugins).is_empty());
        let results = groups.apply(update(Source::Plugins, "new plugin", true));
        assert_eq!(results[1].title, "new plugin");
        assert_eq!(
            groups.current_entries(Source::Plugins)[0].title,
            "new plugin"
        );
        assert!(groups.pending[Source::Plugins as usize]);
        groups.begin([false, false, true], false, Some(Source::Plugins));
        assert!(!groups.has_results());
        assert!(groups.current_entries(Source::Plugins).is_empty());
    }

    #[test]
    fn exclusive_query_does_not_wait_for_unrelated_sources() {
        let mut groups = Groups::default();
        groups.begin([true, true, true], false, Some(Source::Plugins));
        assert!(groups.is_pending());
        groups.apply(Update {
            kind: UpdateKind::Results,
            generation: 1,
            source: Source::Plugins,
            entries: Vec::new(),
            exclusive: true,
            pending: false,
            status: None,
        });
        assert!(!groups.is_pending());
        groups.begin([true, false, false], true, None);
        assert!(groups.is_pending());
    }
    #[test]
    fn asynchronous_results_keep_apps_first_and_remove_duplicates() {
        let mut groups = Groups::default();
        let update = |source, entries| Update {
            kind: crate::providers::UpdateKind::Results,
            generation: 1,
            source,
            entries,
            exclusive: false,
            pending: false,
            status: None,
        };
        groups.apply(update(
            Source::Files,
            vec![entry("file", "file"), entry("duplicate", "app")],
        ));
        let result = groups.apply(update(Source::Apps, vec![entry("app", "app")]));
        assert_eq!(
            result.iter().map(|e| e.title.as_str()).collect::<Vec<_>>(),
            vec!["app", "file"]
        );
        let result = groups.apply(Update {
            kind: crate::providers::UpdateKind::Results,
            generation: 1,
            source: Source::Plugins,
            entries: vec![entry("plugin", "plugin")],
            exclusive: true,
            pending: false,
            status: None,
        });
        assert_eq!(result.len(), 1);
        assert_eq!(result[0].title, "plugin");
    }
    #[test]
    fn global_plugins_merge_without_hiding_apps_or_early_web_fallback() {
        let mut groups = Groups::default();
        groups.pending = [false, false, true];
        let mut results = groups.apply(Update {
            kind: crate::providers::UpdateKind::Results,
            generation: 1,
            source: Source::Apps,
            entries: vec![entry("app", "app")],
            exclusive: false,
            pending: false,
            status: None,
        });
        let web = crate::web_search::WebSearchSettings::default();
        groups.append_web(&mut results, "query", &web);
        assert_eq!(results.len(), 1);
        let results = groups.apply(Update {
            kind: crate::providers::UpdateKind::Results,
            generation: 1,
            source: Source::Plugins,
            entries: vec![entry("plugin", "plugin"), entry("duplicate", "app")],
            exclusive: false,
            pending: false,
            status: None,
        });
        assert_eq!(
            results
                .iter()
                .map(|entry| entry.title.as_str())
                .collect::<Vec<_>>(),
            ["app", "plugin"]
        );
        let mut empty = Groups::default();
        empty.pending[Source::Plugins as usize] = true;
        let mut results = Vec::new();
        empty.append_web(&mut results, "query", &web);
        assert!(results.is_empty());
        empty.apply(Update {
            kind: crate::providers::UpdateKind::Results,
            generation: 1,
            source: Source::Plugins,
            entries: Vec::new(),
            exclusive: false,
            pending: false,
            status: None,
        });
        empty.append_web(&mut results, "query", &web);
        assert_eq!(results.len(), 1);
    }
    #[test]
    fn configured_source_priorities_rank_before_deduplication() {
        for (apps, files, expected) in [
            (0, 0, ["plugin shared", "plugin", "app", "file"]),
            (2, 0, ["app shared", "app", "plugin", "file"]),
            (0, 2, ["file shared", "file", "plugin", "app"]),
            (1, 1, ["app shared", "app", "file", "plugin"]),
            (-2, -1, ["plugin shared", "plugin", "file", "app"]),
            (i32::MIN, i32::MAX, ["file shared", "file", "plugin", "app"]),
        ] {
            for sources in [
                [Source::Apps, Source::Files, Source::Plugins],
                [Source::Plugins, Source::Files, Source::Apps],
            ] {
                let mut groups = Groups::default();
                groups.begin([true; 3], false, None);
                groups.set_priorities(apps, files);
                let mut results = Vec::new();
                for source in sources {
                    let name = match source {
                        Source::Apps => "app",
                        Source::Files => "file",
                        Source::Plugins => "plugin",
                    };
                    let mut entries = vec![
                        entry(&format!("{name} shared"), "shared"),
                        entry(name, name),
                    ];
                    if source == Source::Plugins {
                        for entry in &mut entries {
                            entry.query_priority = 1;
                        }
                    }
                    results = groups.apply(Update {
                        kind: UpdateKind::Results,
                        generation: 1,
                        source,
                        entries,
                        exclusive: false,
                        pending: false,
                        status: None,
                    });
                }
                assert_eq!(
                    results
                        .iter()
                        .map(|entry| entry.title.as_str())
                        .collect::<Vec<_>>(),
                    expected
                );
            }
        }
    }

    #[test]
    fn everything_routes_only_explicit_queries_by_default() {
        let settings = EverythingSettings::default();
        assert_eq!(files_query("e ext:rs", &settings), Some(("ext:rs", true)));
        assert_eq!(files_query("explorer", &settings), None);
        assert_eq!(files_query("e", &settings), Some(("", true)));
    }
}
