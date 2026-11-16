use mini_launcher::{
    history::{self, Edit, History},
    result::Entry,
};
use std::{collections::HashMap, path::Path};

#[derive(Default)]
pub struct Rules {
    pub history: History,
    pub edits: Vec<Edit>,
    pub names: HashMap<String, String>,
}

pub struct Row {
    pub key: String,
    pub title: String,
    pub query: Option<String>,
}

impl Rules {
    pub fn load(path: &Path) -> Result<Self, String> {
        Ok(Self {
            history: history::read_history_at(&path.with_file_name("history.json"))
                .map_err(|e| format!("无法读取应用管理记录：{e}"))?,
            ..Self::default()
        })
    }

    pub fn set_names(&mut self, index: Vec<Entry>) {
        self.names = index
            .into_iter()
            .filter(|entry| entry.is_app())
            .map(|entry| (entry.key, entry.title))
            .collect();
    }

    pub fn rows(&self, removed: bool) -> Vec<Row> {
        let mut rows = Vec::new();
        for (key, usage) in &self.history {
            let title = self
                .names
                .get(key)
                .cloned()
                .unwrap_or_else(|| "未索引的应用".into());
            if removed {
                if usage.removed {
                    rows.push(Row {
                        key: key.clone(),
                        title,
                        query: None,
                    });
                }
            } else {
                for query in &usage.pinned_queries {
                    rows.push(Row {
                        key: key.clone(),
                        title: title.clone(),
                        query: Some(query.clone()),
                    });
                }
            }
        }
        rows.sort_by(|a, b| {
            a.title
                .cmp(&b.title)
                .then(a.key.cmp(&b.key))
                .then(a.query.cmp(&b.query))
        });
        rows
    }

    pub fn edit(&mut self, edit: Edit) {
        history::apply_edits(&mut self.history, std::slice::from_ref(&edit));
        self.edits.push(edit);
    }

    pub fn dirty(&self) -> bool {
        !self.edits.is_empty()
    }

    pub fn save(&mut self, path: &Path) -> Result<(), String> {
        history::control::save(path, &self.edits)?;
        self.edits.clear();
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn managed_rows_include_missing_and_removed_apps_and_unpin_one_query() {
        let mut rules = Rules::default();
        rules
            .history
            .entry("missing.exe".into())
            .or_default()
            .pinned_queries
            .extend(["one".into(), "two".into()]);
        rules.history.get_mut("missing.exe").unwrap().removed = true;
        assert_eq!(rules.rows(false).len(), 2);
        assert_eq!(rules.rows(true).len(), 1);
        assert_eq!(rules.rows(true)[0].title, "未索引的应用");
        rules.edit(Edit::Unpin {
            key: "missing.exe".into(),
            query: "one".into(),
        });
        assert_eq!(rules.rows(false)[0].query.as_deref(), Some("two"));
        assert_eq!(rules.rows(true).len(), 1);
        rules.edit(Edit::Restore {
            key: "missing.exe".into(),
        });
        assert!(rules.rows(true).is_empty());
        assert!(rules.dirty());
        assert_eq!(rules.rows(false).len(), 1);
    }
}
