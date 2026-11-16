use crate::{
    config::Settings,
    result::{Action, Entry},
};
use std::{collections::HashSet, fs, path::PathBuf};

#[cfg(windows)]
mod shell_apps;

pub fn build_index(settings: &Settings) -> Vec<Entry> {
    let mut roots = settings.search_paths.clone();
    for variable in ["APPDATA", "PROGRAMDATA"] {
        if let Some(root) = std::env::var_os(variable) {
            roots.push(
                PathBuf::from(root)
                    .join("Microsoft")
                    .join("Windows")
                    .join("Start Menu")
                    .join("Programs"),
            );
        }
    }
    if let Some(root) = std::env::var_os("USERPROFILE") {
        roots.push(PathBuf::from(root).join("Desktop"));
    }
    let mut entries = Vec::new();
    let mut seen = HashSet::new();
    let mut stack: Vec<_> = roots.into_iter().map(|p| (p, 0)).collect();
    while let Some((directory, depth)) = stack.pop() {
        if depth > 12 || entries.len() >= 20_000 {
            continue;
        }
        let Ok(children) = fs::read_dir(directory) else {
            continue;
        };
        for child in children.flatten() {
            let Ok(kind) = child.file_type() else {
                continue;
            };
            if kind.is_symlink() {
                continue;
            }
            let path = child.path();
            if kind.is_dir() {
                stack.push((path, depth + 1));
                continue;
            }
            if !path
                .extension()
                .and_then(|s| s.to_str())
                .is_some_and(|ext| {
                    ext.eq_ignore_ascii_case("lnk")
                        || ext.eq_ignore_ascii_case("exe")
                        || ext.eq_ignore_ascii_case("url")
                        || ext.eq_ignore_ascii_case("appref-ms")
                })
            {
                continue;
            }
            let title = path
                .file_stem()
                .unwrap_or_default()
                .to_string_lossy()
                .into_owned();
            let name = title.to_lowercase();
            if name.contains("uninstall") || name.contains("卸载") || !seen.insert(name) {
                continue;
            }
            let target = path.to_string_lossy().into_owned();
            entries.push(Entry::new(
                title,
                target.clone(),
                Action::Open(target),
                "应用",
            ));
            if entries.len() >= 20_000 {
                break;
            }
        }
    }
    // Keep existing shortcut targets (and their history keys) when Shell lists
    // the same application; AppsFolder also supplies apps without any .lnk.
    #[cfg(windows)]
    match shell_apps::entries() {
        Ok(apps) => append_shell_apps(&mut entries, &mut seen, apps),
        Err(error) => eprintln!("无法索引 Windows 应用目录：{error}"),
    }
    for (alias, target) in &settings.aliases {
        entries.push(Entry::new(
            alias.clone(),
            target.clone(),
            Action::Open(target.clone()),
            "别名",
        ));
    }
    for (title, target) in [
        ("文件资源管理器", "explorer.exe"),
        ("记事本", "notepad.exe"),
        ("Windows 设置", "ms-settings:"),
        ("终端", "wt.exe"),
    ] {
        if !entries.iter().any(|e| e.title == title) {
            let target = match target {
                "explorer.exe" => PathBuf::from(
                    std::env::var_os("WINDIR").unwrap_or_else(|| "C:\\Windows".into()),
                )
                .join("explorer.exe")
                .to_string_lossy()
                .into_owned(),
                "notepad.exe" => PathBuf::from(
                    std::env::var_os("WINDIR").unwrap_or_else(|| "C:\\Windows".into()),
                )
                .join("System32")
                .join("notepad.exe")
                .to_string_lossy()
                .into_owned(),
                _ => target.into(),
            };
            entries.push(Entry::new(
                title.into(),
                target.clone(),
                Action::Open(target),
                "系统",
            ));
        }
    }
    for entry in &entries {
        entry.search_terms();
    }
    entries.sort_by(|a, b| a.title.cmp(&b.title));
    entries
}

#[cfg(any(windows, test))]
fn append_shell_apps(entries: &mut Vec<Entry>, seen: &mut HashSet<String>, apps: Vec<Entry>) {
    for app in apps {
        let name = app.title.to_lowercase();
        if !name.is_empty()
            && !name.contains("uninstall")
            && !name.contains("卸载")
            && seen.insert(name)
        {
            entries.push(app);
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn shell_apps_fill_gaps_without_replacing_shortcuts() {
        let app = |title: &str, target: &str| {
            Entry::new(
                title.into(),
                target.into(),
                Action::Open(target.into()),
                "应用",
            )
        };
        let mut entries = vec![app("Editor", "editor.lnk")];
        let mut seen = HashSet::from(["editor".into()]);
        append_shell_apps(
            &mut entries,
            &mut seen,
            vec![
                app("EDITOR", "shell:AppsFolder\\Editor"),
                app("测试工具", "shell:AppsFolder\\Test.App!App"),
                app("测试工具", "shell:AppsFolder\\Test.App!Duplicate"),
                app("卸载工具", "shell:AppsFolder\\Uninstall"),
            ],
        );
        assert_eq!(entries.len(), 2);
        assert_eq!(entries[0].key, "editor.lnk");
        for query in ["测试工具", "ceshigongju", "csgj"] {
            let results = crate::search::query(&entries, query, &crate::config::History::new(), 6);
            assert_eq!(results[0].key, "shell:appsfolder\\test.app!app");
            assert!(results[0].is_app());
        }
    }
}
