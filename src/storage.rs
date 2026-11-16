//! The install-local selector is independent of the movable settings directory.
use serde::{Deserialize, Serialize};
use std::{
    fs,
    path::{Path, PathBuf},
    sync::OnceLock,
};

pub const DATA_DIR_ENV: &str = "MINILAUNCHER_DATA_DIR";
const SELECTOR: &str = "portable.toml";

fn default_true() -> bool {
    true
}

#[derive(Serialize, Deserialize)]
struct Mode {
    #[serde(default = "default_true")]
    enabled: bool,
    #[serde(default)]
    migrate_from: Option<PathBuf>,
}

struct Storage {
    install: PathBuf,
    root: PathBuf,
    mode: Mode,
    overridden: bool,
}

fn install_dir() -> PathBuf {
    #[cfg(test)]
    if let Some(path) = std::env::var_os("MINILAUNCHER_TEST_INSTALL") {
        return path.into();
    }
    let directory = std::env::current_exe()
        .ok()
        .and_then(|p| p.parent().map(Path::to_path_buf))
        .unwrap_or_else(|| PathBuf::from("."));
    directory
        .ancestors()
        .find(|p| p.join("mini-launcher.exe").is_file() || p.join(SELECTOR).is_file())
        .unwrap_or(&directory)
        .to_path_buf()
}

fn user_dir() -> PathBuf {
    std::env::var_os("APPDATA")
        .map(PathBuf::from)
        .unwrap_or_else(|| PathBuf::from("."))
        .join("MiniLauncher")
}

fn launcher_running() -> bool {
    #[cfg(test)]
    if std::env::var_os("MINILAUNCHER_TEST_INSTALL").is_some() {
        return std::env::var("MINILAUNCHER_TEST_HOST_RUNNING").as_deref() == Ok("1");
    }
    #[cfg(windows)]
    unsafe {
        let class: Vec<u16> = "MiniLauncher.Native.Window"
            .encode_utf16()
            .chain(Some(0))
            .collect();
        !windows_sys::Win32::UI::WindowsAndMessaging::FindWindowW(class.as_ptr(), std::ptr::null())
            .is_null()
    }
    #[cfg(not(windows))]
    {
        false
    }
}

fn active() -> Result<&'static Storage, String> {
    static STORAGE: OnceLock<Result<Storage, String>> = OnceLock::new();
    STORAGE
        .get_or_init(|| {
            let install = install_dir();
            if let Some(root) = std::env::var_os(DATA_DIR_ENV) {
                return Ok(Storage {
                    install,
                    root: root.into(),
                    mode: Mode {
                        enabled: true,
                        migrate_from: None,
                    },
                    overridden: true,
                });
            }
            let mut mode = match fs::read_to_string(install.join(SELECTOR)) {
                Ok(text) => {
                    toml::from_str::<Mode>(&text).map_err(|e| format!("便携模式配置无效：{e}"))?
                }
                Err(e) if e.kind() == std::io::ErrorKind::NotFound => Mode {
                    enabled: true,
                    migrate_from: None,
                },
                Err(e) => return Err(format!("无法读取便携模式：{e}")),
            };
            let root = if mode.migrate_from.is_some() && launcher_running() {
                // Reopened settings must follow the still-running host rather
                // than copy a snapshot while its workers are writing to it.
                mode.enabled = !mode.enabled;
                mode.migrate_from.take().unwrap()
            } else if mode.enabled {
                install.join("data")
            } else {
                user_dir()
            };
            Ok(Storage {
                install,
                root,
                mode,
                overridden: false,
            })
        })
        .as_ref()
        .map_err(Clone::clone)
}

pub fn data_dir() -> PathBuf {
    active()
        .map(|s| s.root.clone())
        .unwrap_or_else(|_| install_dir().join("data"))
}

/// Pin the active directory for the lifetime of this process. Workers and the
/// settings watcher keep using it until the next launch completes migration.
pub fn initialize() -> Result<(), String> {
    static INITIALIZED: OnceLock<Result<(), String>> = OnceLock::new();
    INITIALIZED
        .get_or_init(|| {
            let storage = active()?;
            fs::create_dir_all(&storage.root)
                .map_err(|e| format!("无法创建数据目录 {}：{e}", storage.root.display()))?;
            if storage.overridden {
                return Ok(());
            }
            // A new portable installation must not import another installation's
            // history, credentials, backups or staging files from APPDATA.
            // Only an explicit mode switch authorizes migration.
            if let Some(source) = storage.mode.migrate_from.as_ref() {
                copy_data(source, &storage.root)
                    .map_err(|e| format!("迁移数据失败，原数据已保留：{e}"))?;
                rebase_settings(&storage.root.join("settings.toml"), source, &storage.root)?;
                save_mode(
                    &storage.install,
                    &Mode {
                        enabled: storage.mode.enabled,
                        migrate_from: None,
                    },
                )?;
            }
            Ok(())
        })
        .clone()
}

fn save_mode(install: &Path, mode: &Mode) -> Result<(), String> {
    let text = toml::to_string(mode).map_err(|e| e.to_string())?;
    if fs::read_to_string(install.join(SELECTOR)).is_ok_and(|current| current == text) {
        return Ok(());
    }
    crate::file_io::write_atomic(&install.join(SELECTOR), text.as_bytes())
        .map_err(|e| format!("无法保存便携模式，请确认安装目录可写：{e}"))
}

pub fn request_mode(enabled: bool, settings_path: &Path) -> Result<(), String> {
    let storage = active()?;
    if storage.overridden || settings_path != storage.root.join("settings.toml") {
        return Ok(());
    }
    let selector = storage.install.join(SELECTOR);
    // Reverting an unsatisfied switch must also cancel its pending migration.
    if enabled == storage.mode.enabled && !selector.exists() {
        return Ok(());
    }
    save_mode(
        &storage.install,
        &Mode {
            enabled,
            migrate_from: (enabled != storage.mode.enabled).then(|| storage.root.clone()),
        },
    )
}

fn copy_data(source: &Path, destination: &Path) -> Result<(), String> {
    if source == destination {
        return Ok(());
    }
    if destination.starts_with(source) || source.starts_with(destination) {
        return Err("源目录与目标目录不能相互包含".into());
    }
    for root in [source, destination] {
        if fs::symlink_metadata(root).is_ok_and(|m| m.file_type().is_symlink()) {
            return Err("数据目录不能是链接".into());
        }
    }
    fn copy(source: &Path, destination: &Path) -> std::io::Result<()> {
        fs::create_dir_all(destination)?;
        for entry in fs::read_dir(source)? {
            let entry = entry?;
            let from = entry.path();
            let to = destination.join(entry.file_name());
            let kind = entry.file_type()?;
            if kind.is_symlink()
                || fs::symlink_metadata(&to).is_ok_and(|m| m.file_type().is_symlink())
            {
                return Err(std::io::Error::other("数据目录含链接，请先处理后重试"));
            }
            if kind.is_dir() {
                copy(&from, &to)?;
            } else if kind.is_file() {
                if to.is_file() {
                    if fs::read(&from)? == fs::read(&to)? {
                        continue;
                    }
                    let mut backup = to.with_extension("migration-backup");
                    let mut index = 1;
                    while backup.exists() {
                        backup = to.with_extension(format!("migration-backup-{index}"));
                        index += 1;
                    }
                    fs::copy(&to, backup)?;
                }
                fs::copy(from, to)?;
            }
        }
        Ok(())
    }
    copy(source, destination).map_err(|e| e.to_string())
}

fn rebase_settings(path: &Path, source: &Path, destination: &Path) -> Result<(), String> {
    use toml_edit::visit_mut::VisitMut;
    struct Paths<'a>(&'a Path, &'a Path);
    impl VisitMut for Paths<'_> {
        fn visit_value_mut(&mut self, value: &mut toml_edit::Value) {
            if let Some(text) = value.as_str() {
                if let Ok(relative) = Path::new(text).strip_prefix(self.0) {
                    let mut updated = toml_edit::Value::from(
                        self.1.join(relative).to_string_lossy().into_owned(),
                    );
                    *updated.decor_mut() = value.decor().clone();
                    *value = updated;
                }
            }
            toml_edit::visit_mut::visit_value_mut(self, value);
        }
    }
    if !path.exists() {
        return Ok(());
    }
    let original = fs::read_to_string(path).map_err(|e| e.to_string())?;
    let mut document: toml_edit::DocumentMut = original
        .parse()
        .map_err(|e: toml_edit::TomlError| e.to_string())?;
    Paths(source, destination).visit_document_mut(&mut document);
    let updated = document.to_string();
    if updated != original {
        crate::file_io::write_atomic(path, updated.as_bytes()).map_err(|e| e.to_string())?;
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn storage_child() {
        let Ok(action) = std::env::var("MINILAUNCHER_TEST_ACTION") else {
            return;
        };
        let settings = crate::config::load().unwrap();
        let install = install_dir();
        if action == "fresh" {
            assert_eq!(data_dir(), install.join("data"));
            assert!(settings.portable_mode);
            assert!(data_dir().join("settings.toml").is_file());
            assert!(!data_dir().join("history.json").exists());
            assert!(!data_dir().join("credential-plugin").exists());
            assert!(!data_dir().join(".plugin-staging").exists());
            assert!(!data_dir().join("settings.migration-backup").exists());
            assert_eq!(
                fs::read(data_dir().join("plugins/demo/README.md")).unwrap(),
                b"bundled plugin"
            );
            return;
        }
        let expected_portable = action != "user" && action != "pending";
        assert_eq!(settings.portable_mode, expected_portable);
        assert_eq!(
            data_dir(),
            if expected_portable || action == "pending" {
                install.join("data")
            } else {
                user_dir()
            }
        );
        assert!(data_dir().join("demo-plugin/data.dat").is_file());
        if action != "restored" && action != "pending" {
            let path = crate::config::settings_path();
            let original = fs::read_to_string(&path).unwrap();
            let mut doc: toml_edit::DocumentMut = original.parse().unwrap();
            doc["portable_mode"] = toml_edit::value(!expected_portable);
            crate::config::save_settings(&path, &original, &doc.to_string()).unwrap();
            assert_eq!(data_dir(), path.parent().unwrap());
        }
    }

    #[test]
    fn fresh_install_does_not_import_legacy_user_data() {
        let root = PathBuf::from(env!("CARGO_MANIFEST_DIR"))
            .join(format!("artifacts/fresh-install-{}", std::process::id()));
        let install = root.join("install");
        let appdata = root.join("appdata");
        let legacy = appdata.join("MiniLauncher");
        fs::create_dir_all(install.join("data/plugins/demo")).unwrap();
        fs::write(
            install.join("data/plugins/demo/README.md"),
            b"bundled plugin",
        )
        .unwrap();
        fs::write(install.join("portable.toml"), "enabled = true\n").unwrap();
        fs::create_dir_all(legacy.join("credential-plugin")).unwrap();
        fs::create_dir_all(legacy.join(".plugin-staging")).unwrap();
        for name in [
            "settings.toml",
            "history.json",
            "settings.migration-backup",
            "credential-plugin/accounts.json",
            ".plugin-staging/temporary",
        ] {
            fs::write(legacy.join(name), b"private fixture").unwrap();
        }
        let output = std::process::Command::new(std::env::current_exe().unwrap())
            .args(["--exact", "storage::tests::storage_child", "--nocapture"])
            .env_remove(DATA_DIR_ENV)
            .env("MINILAUNCHER_TEST_INSTALL", &install)
            .env("MINILAUNCHER_TEST_ACTION", "fresh")
            .env("APPDATA", &appdata)
            .output()
            .unwrap();
        assert!(
            output.status.success(),
            "{}{}",
            String::from_utf8_lossy(&output.stdout),
            String::from_utf8_lossy(&output.stderr)
        );
        assert_eq!(
            fs::read(legacy.join("credential-plugin/accounts.json")).unwrap(),
            b"private fixture"
        );
    }

    #[test]
    fn mode_switch_migrates_on_next_launch_and_keeps_each_process_pinned() {
        let root = PathBuf::from(env!("CARGO_MANIFEST_DIR")).join(format!(
            "artifacts/portable-lifecycle-{}",
            std::process::id()
        ));
        let install = root.join("install");
        let appdata = root.join("appdata");
        let legacy = appdata.join("MiniLauncher");
        fs::create_dir_all(install.join("data/demo-plugin")).unwrap();
        fs::write(install.join("data/settings.toml"), "theme = 'dark'\n").unwrap();
        fs::write(install.join("data/demo-plugin/data.dat"), b"preserved").unwrap();
        fs::create_dir_all(legacy.join("demo-plugin")).unwrap();
        fs::write(legacy.join("settings.toml"), "theme = 'dark'\n").unwrap();
        fs::write(legacy.join("demo-plugin/data.dat"), b"preserved").unwrap();
        for action in ["portable", "pending", "user", "restored"] {
            let output = std::process::Command::new(std::env::current_exe().unwrap())
                .args(["--exact", "storage::tests::storage_child", "--nocapture"])
                .env_remove(DATA_DIR_ENV)
                .env("MINILAUNCHER_TEST_INSTALL", &install)
                .env("MINILAUNCHER_TEST_ACTION", action)
                .env(
                    "MINILAUNCHER_TEST_HOST_RUNNING",
                    if action == "pending" { "1" } else { "0" },
                )
                .env("APPDATA", &appdata)
                .output()
                .unwrap();
            assert!(
                output.status.success(),
                "{}{}",
                String::from_utf8_lossy(&output.stdout),
                String::from_utf8_lossy(&output.stderr)
            );
        }
        assert_eq!(
            fs::read(legacy.join("demo-plugin/data.dat")).unwrap(),
            b"preserved"
        );
        assert_eq!(
            fs::read(install.join("data/demo-plugin/data.dat")).unwrap(),
            b"preserved"
        );
    }

    #[test]
    fn migration_preserves_source_and_backs_up_destination_conflicts() {
        let root =
            PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("artifacts/portable-migration-test");
        let root = root.join(format!("{}", std::process::id()));
        let source = root.join("source");
        let target = root.join("target");
        fs::create_dir_all(source.join("plugins/demo")).unwrap();
        fs::create_dir_all(&target).unwrap();
        fs::write(source.join("settings.toml"), "new").unwrap();
        fs::write(target.join("settings.toml"), "old").unwrap();
        fs::write(source.join("plugins/demo/settings.json"), "{}").unwrap();
        copy_data(&source, &target).unwrap();
        assert_eq!(
            fs::read_to_string(source.join("settings.toml")).unwrap(),
            "new"
        );
        assert_eq!(
            fs::read_to_string(target.join("settings.toml")).unwrap(),
            "new"
        );
        assert_eq!(
            fs::read_to_string(target.join("settings.migration-backup")).unwrap(),
            "old"
        );
        assert!(target.join("plugins/demo/settings.json").is_file());
        assert!(copy_data(&source, &source.join("nested")).is_err());
    }

    #[test]
    fn migration_rebases_managed_paths_and_preserves_comments_and_external_paths() {
        let root = PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("artifacts/portable-rebase-test");
        fs::create_dir_all(&root).unwrap();
        let path = root.join("settings.toml");
        fs::write(
            &path,
            "# keep\n[plugins]\ndirectories = ['C:\\old\\plugins', 'D:\\external'] # plugins\n",
        )
        .unwrap();
        rebase_settings(&path, Path::new(r"C:\old"), Path::new(r"D:\portable\data")).unwrap();
        let text = fs::read_to_string(path).unwrap();
        assert!(text.contains("# keep") && text.contains("# plugins"));
        let value: toml::Value = toml::from_str(&text).unwrap();
        assert_eq!(
            Path::new(value["plugins"]["directories"][0].as_str().unwrap()),
            Path::new(r"D:\portable\data\plugins")
        );
        assert_eq!(
            value["plugins"]["directories"][1].as_str(),
            Some(r"D:\external")
        );
    }
}
