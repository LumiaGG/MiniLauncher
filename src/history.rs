use crate::{config::data_dir, file_io::write_atomic};
use serde::{Deserialize, Serialize};
use std::{
    collections::{HashMap, HashSet},
    fs,
};

#[derive(Clone, Default, Deserialize, Serialize)]
#[serde(default)]
pub struct Usage {
    pub count: u32,
    pub last_used: u64,
    pub pinned_queries: HashSet<String>,
    pub removed: bool,
}

pub type History = HashMap<String, Usage>;

pub mod control;

#[derive(Clone, Debug, Deserialize, Serialize)]
#[serde(tag = "action", rename_all = "snake_case", deny_unknown_fields)]
pub enum Edit {
    Unpin { key: String, query: String },
    Restore { key: String },
}

pub fn apply_edits(history: &mut History, edits: &[Edit]) {
    for edit in edits {
        match edit {
            Edit::Unpin { key, query } => {
                if let Some(usage) = history.get_mut(key) {
                    usage.pinned_queries.remove(query);
                }
            }
            Edit::Restore { key } => {
                if let Some(usage) = history.get_mut(key) {
                    usage.removed = false;
                }
            }
        }
    }
}

pub const MAX_HISTORY: usize = 2000;

pub fn prune(history: &mut History) {
    if history.len() <= MAX_HISTORY {
        return;
    }
    let excess = history.len() - MAX_HISTORY;
    let mut candidates: Vec<_> = history
        .iter()
        .filter(|(_, u)| !u.removed && u.pinned_queries.is_empty())
        .map(|(k, u)| (u.last_used, k.clone()))
        .collect();
    candidates.sort_unstable_by(|a, b| a.0.cmp(&b.0).then_with(|| a.1.cmp(&b.1)));
    for (_, key) in candidates.into_iter().take(excess) {
        history.remove(&key);
    }
}

pub fn load_history() -> History {
    let mut history: History = fs::read(data_dir().join("history.json"))
        .ok()
        .and_then(|bytes| serde_json::from_slice(&bytes).ok())
        .unwrap_or_default();
    prune(&mut history);
    history
}

pub fn save_history(history: &History) -> std::io::Result<()> {
    save_history_at(&data_dir().join("history.json"), history)
}

pub fn read_history_at(path: &std::path::Path) -> std::io::Result<History> {
    match fs::read(path) {
        Ok(bytes) => Ok(serde_json::from_slice(&bytes)?),
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => Ok(History::new()),
        Err(error) => Err(error),
    }
}

pub fn save_history_at(path: &std::path::Path, history: &History) -> std::io::Result<()> {
    if let Some(parent) = path.parent() {
        fs::create_dir_all(parent)?;
    }
    let bytes = if history.len() > MAX_HISTORY {
        let mut pruned = history.clone();
        prune(&mut pruned);
        serde_json::to_vec(&pruned)?
    } else {
        serde_json::to_vec(history)?
    };
    write_atomic(path, &bytes)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn management_edits_preserve_usage_and_unrelated_queries() {
        let mut history = History::new();
        history.insert(
            "app".into(),
            Usage {
                count: 42,
                last_used: 123,
                pinned_queries: ["one".into(), "two".into()].into_iter().collect(),
                removed: true,
            },
        );
        apply_edits(
            &mut history,
            &[
                Edit::Unpin {
                    key: "app".into(),
                    query: "one".into(),
                },
                Edit::Restore { key: "app".into() },
                Edit::Restore {
                    key: "missing".into(),
                },
            ],
        );
        assert_eq!(history.len(), 1);
        assert_eq!(history["app"].count, 42);
        assert_eq!(history["app"].last_used, 123);
        assert_eq!(history["app"].pinned_queries, HashSet::from(["two".into()]));
        assert!(!history["app"].removed);
    }

    #[test]
    fn prune_evicts_oldest_unpinned_entries_and_preserves_pins_and_removals() {
        let mut history = History::new();
        history.insert(
            "pinned".into(),
            Usage {
                count: 1,
                last_used: 0,
                pinned_queries: ["code".to_string()].into_iter().collect(),
                removed: false,
            },
        );
        history.insert(
            "removed".into(),
            Usage {
                count: 1,
                last_used: 0,
                pinned_queries: HashSet::new(),
                removed: true,
            },
        );
        for i in 1..=(MAX_HISTORY as u64 + 5) {
            history.insert(
                format!("app-{i}"),
                Usage {
                    count: 1,
                    last_used: i,
                    pinned_queries: HashSet::new(),
                    removed: false,
                },
            );
        }
        prune(&mut history);
        assert_eq!(history.len(), MAX_HISTORY);
        assert!(history.contains_key("pinned"));
        assert!(history.contains_key("removed"));
        for i in 1..=7 {
            assert!(!history.contains_key(&format!("app-{i}")));
        }
        assert!(history.contains_key("app-8"));
    }
}
