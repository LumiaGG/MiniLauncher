//! Local package lifecycle. Staging never executes a plugin; uninstall archives
//! the complete directory so arbitrary plugin-owned data is preserved.
use crate::{
    model,
    model::{key, Draft},
};
use mini_launcher::config;
use std::{
    collections::HashSet,
    fs,
    io::{self, Read, Write},
    path::{Path, PathBuf},
    sync::atomic::{AtomicU64, Ordering},
    time::{Duration, Instant, SystemTime, UNIX_EPOCH},
};
use toml::Value;

const MAX_ENTRIES: usize = 4096;
const MAX_FILE: u64 = 128 * 1024 * 1024;
const MAX_TOTAL: u64 = 512 * 1024 * 1024;
fn error(e: impl std::fmt::Display) -> String {
    e.to_string()
}
fn unique() -> String {
    static SERIAL: AtomicU64 = AtomicU64::new(0);
    format!(
        "{}-{}-{}",
        SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .unwrap_or_default()
            .as_millis(),
        std::process::id(),
        SERIAL.fetch_add(1, Ordering::Relaxed)
    )
}
fn reparse(meta: &fs::Metadata) -> bool {
    #[cfg(windows)]
    {
        use std::os::windows::fs::MetadataExt;
        meta.file_attributes() & 0x400 != 0
    }
    #[cfg(not(windows))]
    {
        meta.file_type().is_symlink()
    }
}
fn directory(path: &Path) -> Result<(), String> {
    fs::create_dir_all(path).map_err(error)?;
    let meta = fs::symlink_metadata(path).map_err(error)?;
    if !meta.is_dir() || reparse(&meta) {
        return Err(format!("目录不能是链接或重解析点：{}", path.display()));
    }
    Ok(())
}
fn storage(draft: &Draft) -> Result<PathBuf, String> {
    let parent = draft
        .path
        .parent()
        .filter(|p| !p.as_os_str().is_empty())
        .unwrap_or_else(|| Path::new("."));
    fs::canonicalize(parent).map_err(error)
}
fn identity(path: &Path) -> String {
    fs::canonicalize(path)
        .unwrap_or_else(|_| {
            if path.is_absolute() {
                path.into()
            } else {
                std::env::current_dir().unwrap_or_default().join(path)
            }
        })
        .to_string_lossy()
        .trim_start_matches(r"\\?\")
        .replace('/', "\\")
        .to_lowercase()
}
fn clean(draft: &Draft) -> Result<(), String> {
    if draft.dirty() {
        Err("设置尚未自动保存，请先修正无效内容或解决保存错误后重试。".into())
    } else {
        Ok(())
    }
}

/// Also reject Windows aliases, ADS, drive prefixes and backslash traversal even
/// when tests run on another OS.
fn relative(name: &str) -> Result<PathBuf, String> {
    let name = name.replace('\\', "/");
    if name.is_empty() || name.starts_with('/') || name.len() > 1024 {
        return Err("包内路径无效".into());
    }
    let mut path = PathBuf::new();
    for part in name.trim_end_matches('/').split('/') {
        let stem = part.split('.').next().unwrap_or("").to_ascii_uppercase();
        if part.is_empty()
            || matches!(part, "." | "..")
            || part.ends_with(['.', ' '])
            || part
                .chars()
                .any(|c| c.is_control() || matches!(c, ':' | '<' | '>' | '"' | '|' | '?' | '*'))
            || matches!(stem.as_str(), "CON" | "PRN" | "AUX" | "NUL")
            || (stem.len() == 4
                && (stem.starts_with("COM") || stem.starts_with("LPT"))
                && matches!(stem.as_bytes()[3], b'1'..=b'9'))
        {
            return Err(format!("包内路径不安全：{name}"));
        }
        path.push(part);
    }
    Ok(path)
}
struct Stage {
    path: PathBuf,
    parent: PathBuf,
}
impl Stage {
    fn new(base: &Path) -> Result<Self, String> {
        let parent = base.join(".plugin-staging");
        directory(&parent)?;
        let parent = fs::canonicalize(parent).map_err(error)?;
        let path = parent.join(unique());
        fs::create_dir(&path).map_err(error)?;
        Ok(Self { path, parent })
    }
}
impl Drop for Stage {
    fn drop(&mut self) {
        if self.path.parent() == Some(self.parent.as_path())
            && fs::symlink_metadata(&self.path).is_ok_and(|m| m.is_dir() && !reparse(&m))
            && fs::canonicalize(&self.path).is_ok_and(|p| p.parent() == Some(self.parent.as_path()))
        {
            let _ = fs::remove_dir_all(&self.path);
        }
    }
}
#[derive(Default)]
struct Budget {
    entries: usize,
    bytes: u64,
}
impl Budget {
    fn add(&mut self, size: u64) -> Result<(), String> {
        self.entries += 1;
        self.bytes = self.bytes.checked_add(size).ok_or("插件包大小溢出")?;
        if self.entries > MAX_ENTRIES || size > MAX_FILE || self.bytes > MAX_TOTAL {
            return Err("插件包过大：最多 4096 项，单文件 128 MiB，总计 512 MiB。".into());
        }
        Ok(())
    }
}
fn copy_tree(
    source: &Path,
    target: &Path,
    depth: usize,
    budget: &mut Budget,
) -> Result<(), String> {
    if depth > 32 {
        return Err("插件包目录层级过深".into());
    }
    let meta = fs::symlink_metadata(source).map_err(error)?;
    if reparse(&meta) {
        return Err(format!(
            "插件包不支持符号链接或重解析点：{}",
            source.display()
        ));
    }
    budget.add(if meta.is_file() { meta.len() } else { 0 })?;
    if meta.is_dir() {
        fs::create_dir(target).map_err(error)?;
        let mut names = HashSet::new();
        for entry in fs::read_dir(source).map_err(error)? {
            let entry = entry.map_err(error)?;
            let name = entry
                .file_name()
                .into_string()
                .map_err(|_| "插件包文件名必须为 Unicode")?;
            let path = relative(&name)?;
            if !names.insert(name.to_lowercase()) {
                return Err("包内存在大小写冲突的文件名".into());
            }
            copy_tree(&entry.path(), &target.join(path), depth + 1, budget)?;
        }
    } else if meta.is_file() {
        // Bound the actual copied stream, rather than trusting metadata that can
        // change while a local source directory is being packaged.
        let mut input = fs::File::open(source).map_err(error)?.take(meta.len() + 1);
        let mut output = fs::OpenOptions::new()
            .write(true)
            .create_new(true)
            .open(target)
            .map_err(error)?;
        if io::copy(&mut input, &mut output).map_err(error)? != meta.len() {
            return Err("复制期间源文件已变化，请重试".into());
        }
    } else {
        return Err("插件包只支持普通文件和目录".into());
    }
    Ok(())
}
fn extract(source: &Path, target: &Path, budget: &mut Budget) -> Result<(), String> {
    if fs::metadata(source).map_err(error)?.len() > MAX_TOTAL {
        return Err("ZIP 文件超过 512 MiB".into());
    }
    let mut archive =
        zip::ZipArchive::new(fs::File::open(source).map_err(error)?).map_err(error)?;
    if archive.len() > MAX_ENTRIES {
        return Err("ZIP 项目数量超过 4096".into());
    }
    fs::create_dir(target).map_err(error)?;
    let mut names = HashSet::new();
    for i in 0..archive.len() {
        let mut entry = archive.by_index(i).map_err(error)?;
        let relative = relative(entry.name())?;
        if !names.insert(relative.to_string_lossy().to_lowercase()) {
            return Err("ZIP 存在重复或大小写冲突的路径".into());
        }
        let mode = entry.unix_mode().unwrap_or(0) & 0o170000;
        if !matches!(mode, 0 | 0o040000 | 0o100000) {
            return Err("ZIP 不支持符号链接或特殊文件".into());
        }
        budget.add(entry.size())?;
        let destination = target.join(relative);
        if entry.is_dir() {
            fs::create_dir_all(destination).map_err(error)?;
        } else {
            fs::create_dir_all(destination.parent().ok_or("ZIP 路径无效")?).map_err(error)?;
            let expected = entry.size();
            let mut output = fs::OpenOptions::new()
                .write(true)
                .create_new(true)
                .open(destination)
                .map_err(error)?;
            if io::copy(&mut (&mut entry).take(expected + 1), &mut output).map_err(error)?
                != expected
            {
                return Err("ZIP 解压大小与清单不符".into());
            }
        }
    }
    Ok(())
}
fn package_root(payload: &Path) -> Result<PathBuf, String> {
    if payload.join("plugin.json").is_file() {
        return Ok(payload.into());
    }
    let candidates = fs::read_dir(payload)
        .map_err(error)?
        .filter_map(Result::ok)
        .map(|e| e.path())
        .filter(|p| p.is_dir() && p.join("plugin.json").is_file())
        .collect::<Vec<_>>();
    if candidates.len() != 1 {
        return Err(
            "ZIP 根目录或唯一一级子目录中必须包含 plugin.json；一次只能安装一个插件。".into(),
        );
    }
    Ok(candidates[0].clone())
}
pub struct InstallPlan {
    _stage: Stage,
    package: PathBuf,
    pub target: PathBuf,
    pub name: String,
    pub id: String,
    pub config_key: String,
    pub entries: usize,
    pub bytes: u64,
    previous: Option<Installed>,
}

#[derive(PartialEq, Eq)]
struct Installed {
    manifest: String,
    executable: PathBuf,
}

impl InstallPlan {
    pub fn is_update(&self) -> bool {
        self.previous.is_some()
    }
}

fn installed(target: &Path, id: &str, config_key: &str) -> Result<Option<Installed>, String> {
    let metadata = match fs::symlink_metadata(target) {
        Ok(metadata) => metadata,
        Err(e) if e.kind() == io::ErrorKind::NotFound => return Ok(None),
        Err(e) => return Err(error(e)),
    };
    if !metadata.is_dir() || reparse(&metadata) {
        return Err("安装位置已被文件、链接或重解析点占用".into());
    }
    let manifest_path = target.join("plugin.json");
    if !fs::symlink_metadata(&manifest_path)
        .is_ok_and(|m| m.is_file() && !reparse(&m) && m.len() <= 64 * 1024)
    {
        return Err("现有目录缺少有效的插件清单，无法更新".into());
    }
    let manifest = fs::read_to_string(manifest_path).map_err(error)?;
    let parsed: serde_json::Value = serde_json::from_str(&manifest).map_err(error)?;
    let old_key = parsed["config_key"]
        .as_str()
        .map(str::to_owned)
        .unwrap_or_else(|| mini_launcher::plugin_config::key(id));
    if parsed["id"].as_str() != Some(id) || old_key != config_key {
        return Err("安装位置已存在，但插件 ID 或配置标识不一致，无法更新".into());
    }
    let executable = relative(
        parsed["executable"]
            .as_str()
            .ok_or("现有插件缺少程序路径")?,
    )?;
    Ok(Some(Installed {
        manifest,
        executable,
    }))
}

fn check_conflicts(draft: &Draft, id: &str, config_key: &str, target: &Path) -> Result<(), String> {
    let settings: config::Settings = draft.value.clone().try_into().map_err(error)?;
    for plugin in mini_launcher::plugin_catalog::inspect(&settings.plugins) {
        if (plugin.config_key == config_key || plugin.id.eq_ignore_ascii_case(id))
            && identity(&plugin.directory) != identity(target)
        {
            return Err(format!(
                "已登记同 ID 或配置标识的插件：{}。请先取消冲突的登记。",
                plugin.name
            ));
        }
    }
    Ok(())
}

pub fn prepare(source: &Path, draft: &Draft) -> Result<InstallPlan, String> {
    clean(draft)?;
    let base = storage(draft)?;
    let root = base.join("plugins");
    directory(&root)?;
    let stage = Stage::new(&base)?;
    let payload = stage.path.join("payload");
    if source.is_dir() && fs::canonicalize(source).is_ok_and(|p| stage.path.starts_with(p)) {
        return Err("源目录包含安装暂存区，请选择独立的插件目录".into());
    }
    let mut budget = Budget::default();
    if source.is_dir() {
        copy_tree(source, &payload, 0, &mut budget)?;
    } else if source
        .extension()
        .is_some_and(|e| e.eq_ignore_ascii_case("zip"))
    {
        extract(source, &payload, &mut budget)?;
    } else {
        return Err("请选择包含 plugin.json 的目录或 ZIP 插件包".into());
    }
    let package = package_root(&payload)?;
    #[cfg(windows)]
    mini_launcher::plugin_catalog::validate_directory(&package)?;
    let manifest: serde_json::Value =
        serde_json::from_str(&fs::read_to_string(package.join("plugin.json")).map_err(error)?)
            .map_err(error)?;
    let id = manifest["id"].as_str().ok_or("插件缺少 id")?.to_owned();
    relative(&id)?;
    let executable = relative(
        manifest["executable"]
            .as_str()
            .ok_or("插件缺少 executable")?,
    )?;
    if !package.join(executable).is_file() {
        return Err("插件程序不存在或不在包内".into());
    }
    let config_key = manifest["config_key"]
        .as_str()
        .map(String::from)
        .unwrap_or_else(|| mini_launcher::plugin_config::key(&id));
    let target = root.join(&id);
    let previous = installed(&target, &id, &config_key)?;
    check_conflicts(draft, &id, &config_key, &target)?;
    let mut validation = draft.document()?;
    mini_launcher::plugin_config::register_in_document(&package, &mut validation)?;
    let (settings, _) = config::parse_and_complete(&validation.to_string())?;
    if previous.is_none() {
        mini_launcher::plugin_catalog::validate_web_keywords(
            &settings.plugins,
            &settings.web_search,
        )?;
        model::validate_plugin_schemas(&settings.plugins)?;
    }
    Ok(InstallPlan {
        name: manifest["name"].as_str().unwrap_or(&id).into(),
        id,
        config_key,
        target,
        package,
        _stage: stage,
        entries: budget.entries,
        bytes: budget.bytes,
        previous,
    })
}
pub fn install(plan: InstallPlan, draft: &mut Draft) -> Result<String, String> {
    clean(draft)?;
    *draft = Draft::load(draft.path.clone())?;
    directory(plan.target.parent().ok_or("安装目录无效")?)?;
    if installed(&plan.target, &plan.id, &plan.config_key)? != plan.previous {
        return Err("安装目录在确认后已变化，请重新选择插件包".into());
    }
    check_conflicts(draft, &plan.id, &plan.config_key, &plan.target)?;
    if let Some(previous) = &plan.previous {
        return update(&plan, previous, draft);
    }
    fs::rename(&plan.package, &plan.target).map_err(error)?;
    let result = draft.register(&plan.target).and_then(|_| draft.save(false));
    if let Err(error) = result {
        if fs::rename(&plan.target, &plan.package).is_err() {
            return Err(format!(
                "保存失败：{error}。目录回滚失败，文件保留在 {}，请检查。",
                plan.target.display()
            ));
        }
        return Err(format!("安装已回滚：{error}"));
    }
    Ok(format!(
        "已安装 {}（{}）\n目录：{}",
        plan.name,
        plan.id,
        plan.target.display()
    ))
}

fn update(plan: &InstallPlan, previous: &Installed, draft: &mut Draft) -> Result<String, String> {
    let backups = storage(draft)?.join("plugin-backups");
    directory(&backups)?;
    let backup = backups.join(format!("{}-update-{}", plan.config_key, unique()));
    let fields = [key("plugins"), key(&plan.config_key), key("enabled")];
    let enabled = model::at(&draft.value, &fields).cloned();
    let temporarily_disabled = enabled.as_ref().and_then(Value::as_bool) != Some(false);
    if temporarily_disabled {
        set_enabled(draft, &plan.config_key, Some(Value::Boolean(false)))?;
        if let Err(error) = draft.save(false) {
            *draft = Draft::load(draft.path.clone())?;
            return Err(error);
        }
    }
    let mut archived = false;
    let mut replaced = false;
    let install = (|| {
        let guard = wait_stopped(Some(&plan.target.join(&previous.executable)))?;
        if installed(&plan.target, &plan.id, &plan.config_key)?.as_ref() != Some(previous) {
            return Err("安装目录在等待停用时已变化，请重新选择插件包".into());
        }
        carry_existing_files(plan, previous)?;
        drop(guard);
        fs::rename(&plan.target, &backup).map_err(error)?;
        archived = true;
        fs::rename(&plan.package, &plan.target).map_err(error)?;
        replaced = true;
        // Reload the disabled baseline before merging newly introduced defaults.
        *draft = Draft::load(draft.path.clone())?;
        draft.register(&plan.target)?;
        if temporarily_disabled {
            set_enabled(
                draft,
                &plan.config_key,
                enabled.clone().or(Some(Value::Boolean(true))),
            )?;
        }
        draft.save(false)
    })();
    if let Err(cause) = install {
        if archived {
            if (replaced && fs::rename(&plan.target, &plan.package).is_err())
                || fs::rename(&backup, &plan.target).is_err()
            {
                *draft = Draft::load(draft.path.clone())?;
                return Err(format!(
                    "更新失败：{cause}。旧目录保留在 {}，请检查。",
                    backup.display()
                ));
            }
        }
        *draft = Draft::load(draft.path.clone())?;
        if temporarily_disabled {
            set_enabled(draft, &plan.config_key, enabled)?;
            if let Err(restore) = draft.save(false) {
                *draft = Draft::load(draft.path.clone())?;
                return Err(format!(
                    "更新失败：{cause}。旧文件已保留，无法恢复启用状态：{restore}"
                ));
            }
        }
        return Err(format!("更新已回滚：{cause}"));
    }
    Ok(format!(
        "已更新 {}（{}）\n旧版本备份：{}\n若宿主尚未刷新，请从托盘重新加载配置。",
        plan.name,
        plan.id,
        backup.display()
    ))
}

fn set_enabled(draft: &mut Draft, config_key: &str, enabled: Option<Value>) -> Result<(), String> {
    let plugins = draft
        .value
        .get_mut("plugins")
        .and_then(Value::as_table_mut)
        .ok_or("缺少插件配置表")?;
    let entry = plugins
        .entry(config_key)
        .or_insert_with(|| Value::Table(toml::Table::new()));
    let entry = entry.as_table_mut().ok_or("插件配置不是表")?;
    if let Some(value) = enabled {
        entry.insert("enabled".into(), value);
    } else {
        entry.remove("enabled");
    }
    Ok(())
}

fn carry_existing_files(plan: &InstallPlan, previous: &Installed) -> Result<(), String> {
    let parsed: serde_json::Value = serde_json::from_str(&previous.manifest).map_err(error)?;
    let mut skip = vec![
        PathBuf::from("plugin.json"),
        PathBuf::from("settings.schema.json"),
        previous.executable.clone(),
    ];
    if let Some(defaults) = parsed["config_defaults"]
        .as_str()
        .and_then(|name| relative(name).ok())
    {
        skip.push(defaults);
    }
    fn copy_missing(
        source: &Path,
        target: &Path,
        path: &Path,
        skip: &[PathBuf],
        depth: usize,
        budget: &mut Budget,
    ) -> Result<(), String> {
        if depth > 32 {
            return Err("现有插件目录层级过深".into());
        }
        for entry in fs::read_dir(source).map_err(error)? {
            let entry = entry.map_err(error)?;
            let name = entry
                .file_name()
                .into_string()
                .map_err(|_| "现有插件文件名必须为 Unicode")?;
            let path = path.join(relative(&name)?);
            let equal = |other: &PathBuf| {
                path.to_string_lossy()
                    .eq_ignore_ascii_case(&other.to_string_lossy())
            };
            if skip.iter().any(equal) {
                continue;
            }
            let source_path = entry.path();
            let destination = target.join(&name);
            let meta = fs::symlink_metadata(&source_path).map_err(error)?;
            if reparse(&meta) {
                continue; // Remains in the full backup.
            }
            let destination_is_dir = match fs::symlink_metadata(&destination) {
                Ok(meta) => meta.is_dir(),
                Err(e) if e.kind() == io::ErrorKind::NotFound => {
                    if !meta.is_dir() {
                        copy_tree(&source_path, &destination, depth, budget)?;
                        continue;
                    }
                    budget.add(0)?;
                    fs::create_dir(&destination).map_err(error)?;
                    true
                }
                Err(e) => return Err(error(e)),
            };
            if meta.is_dir() && destination_is_dir {
                copy_missing(&source_path, &destination, &path, skip, depth + 1, budget)?;
            }
        }
        Ok(())
    }
    copy_missing(
        &plan.target,
        &plan.package,
        Path::new(""),
        &skip,
        0,
        &mut Budget::default(),
    )
}

pub struct UninstallPlan {
    pub directory: PathBuf,
    pub config_key: String,
    pub name: String,
    pub archive: bool,
    executable: Option<PathBuf>,
}
pub fn prepare_uninstall(
    path: &Path,
    config_key: &str,
    draft: &Draft,
) -> Result<UninstallPlan, String> {
    clean(draft)?;
    let meta = fs::symlink_metadata(path).map_err(error)?;
    if !meta.is_dir() || reparse(&meta) {
        return Err("卸载目标不能是链接或重解析点".into());
    }
    let directory = fs::canonicalize(path).map_err(error)?;
    let root = storage(draft)?.join("plugins");
    let archive = fs::symlink_metadata(&root).is_ok_and(|m| m.is_dir() && !reparse(&m))
        && fs::canonicalize(root).is_ok_and(|root| directory.parent() == Some(root.as_path()));
    if config_key.is_empty()
        || config_key.len() > 256
        || !config_key
            .bytes()
            .all(|b| b.is_ascii_alphanumeric() || matches!(b, b'_' | b'-'))
        || matches!(config_key, "enabled" | "directories" | "max_processes")
    {
        return Err("插件配置标识无效，不能卸载".into());
    }
    let manifest: serde_json::Value =
        serde_json::from_str(&fs::read_to_string(directory.join("plugin.json")).map_err(error)?)
            .map_err(error)?;
    let manifest_key = manifest["config_key"]
        .as_str()
        .map(String::from)
        .or_else(|| {
            manifest["id"]
                .as_str()
                .map(mini_launcher::plugin_config::key)
        });
    if manifest_key.as_deref() != Some(config_key) {
        return Err("插件清单的配置标识已变化，请刷新列表后重试".into());
    }
    #[cfg(windows)]
    {
        let settings: config::Settings = draft.value.clone().try_into().map_err(error)?;
        if mini_launcher::plugin_catalog::inspect(&settings.plugins)
            .iter()
            .any(|p| p.config_key == config_key && identity(&p.directory) != identity(&directory))
        {
            return Err("多个插件共用此配置标识，无法单独清理配置；请先修正冲突的清单。".into());
        }
    }
    let executable = manifest["executable"]
        .as_str()
        .and_then(|s| relative(s).ok())
        .map(|p| directory.join(p));
    Ok(UninstallPlan {
        name: manifest["name"].as_str().unwrap_or(config_key).into(),
        directory,
        config_key: config_key.into(),
        archive,
        executable,
    })
}
fn wait_stopped(executable: Option<&Path>) -> Result<Option<fs::File>, String> {
    wait_stopped_for(executable, Duration::from_secs(8))
}
fn wait_stopped_for(
    executable: Option<&Path>,
    timeout: Duration,
) -> Result<Option<fs::File>, String> {
    let Some(path) = executable.filter(|p| p.exists()) else {
        return Ok(None);
    };
    let deadline = Instant::now() + timeout;
    loop {
        let mut options = fs::OpenOptions::new();
        options.write(true);
        #[cfg(windows)]
        {
            use std::os::windows::fs::OpenOptionsExt;
            options.share_mode(0);
        }
        if let Ok(file) = options.open(path) {
            return Ok(Some(file));
        }
        if Instant::now() >= deadline {
            return Err("插件已设为停用，但程序文件仍被占用或只读；未移动文件。请退出宿主并检查文件权限后重试。".into());
        }
        std::thread::sleep(Duration::from_millis(100));
    }
}
pub fn uninstall(plan: UninstallPlan, draft: &mut Draft) -> Result<String, String> {
    clean(draft)?;
    *draft = Draft::load(draft.path.clone())?;
    let checked = prepare_uninstall(&plan.directory, &plan.config_key, draft)?;
    if checked.directory != plan.directory
        || checked.archive != plan.archive
        || checked.executable != plan.executable
    {
        return Err("卸载目标在确认后已变化，请刷新列表重试".into());
    }
    let original_settings = draft
        .value
        .get("plugins")
        .and_then(|v| v.get(&plan.config_key))
        .cloned();
    let plugins = draft
        .value
        .get_mut("plugins")
        .and_then(Value::as_table_mut)
        .ok_or("缺少插件配置表")?;
    let entry = plugins
        .entry(plan.config_key.clone())
        .or_insert_with(|| Value::Table(toml::Table::new()));
    entry
        .as_table_mut()
        .ok_or("插件配置不是表")?
        .insert("enabled".into(), Value::Boolean(false));
    draft.save(false)?;
    let mut executable_guard = wait_stopped(plan.executable.as_deref())?;
    let backup = if plan.archive {
        let backups = storage(draft)?.join("plugin-backups");
        directory(&backups)?;
        let backup = backups.join(format!("{}-{}", plan.config_key, unique()));
        // Recheck the resolved target before moving any existing user files.
        let checked = prepare_uninstall(&plan.directory, &plan.config_key, draft)?;
        if !checked.archive || checked.directory != plan.directory {
            return Err("卸载目录已变化，停止操作".into());
        }
        // Windows also prevents moving a directory while our own file handle
        // is open. Release it immediately before rename; a new file lock makes
        // the rename fail safely, leaving the disabled installation in place.
        drop(executable_guard.take());
        fs::rename(&plan.directory, &backup)
            .map_err(|e| format!("插件已停用，但目录无法移至备份，原文件未移除：{e}"))?;
        if let Some(original) = original_settings {
            let snapshot = Value::Table(toml::Table::from_iter([(
                "plugins".into(),
                Value::Table(toml::Table::from_iter([(
                    plan.config_key.clone(),
                    original,
                )])),
            )]));
            let result = (|| {
                let text = toml::to_string(&snapshot).map_err(error)?;
                fs::OpenOptions::new()
                    .write(true)
                    .create_new(true)
                    .open(backup.join(format!(".mini-launcher-settings-{}.toml", unique())))
                    .map_err(error)?
                    .write_all(text.as_bytes())
                    .map_err(error)
            })();
            if let Err(error) = result {
                let restored = fs::rename(&backup, &plan.directory).is_ok();
                return Err(format!(
                    "配置快照保存失败：{error}；插件保持停用，文件保留在 {}。",
                    if restored { &plan.directory } else { &backup }.display()
                ));
            }
        }
        Some(backup)
    } else {
        None
    };
    if let Some(Value::Array(roots)) =
        model::at_mut(&mut draft.value, &[key("plugins"), key("directories")])
    {
        roots.retain(|root| {
            !root
                .as_str()
                .is_some_and(|s| identity(Path::new(s)) == identity(&plan.directory))
        });
    }
    // External source directories may still be found through a parent search
    // root. Keep a disabled tombstone so they cannot silently become active.
    if plan.archive {
        draft.value["plugins"]
            .as_table_mut()
            .unwrap()
            .remove(&plan.config_key);
    }
    if let Err(error) = draft.save(false) {
        if let Some(backup) = &backup {
            if fs::rename(backup, &plan.directory).is_err() {
                return Err(format!(
                    "配置保存失败：{error}。文件保留在 {}，插件保持停用。",
                    backup.display()
                ));
            }
        }
        return Err(format!("卸载未完成：{error}；插件保持停用。"));
    }
    Ok(match backup {
        Some(backup) => format!("已卸载 {}，配置已移除。\n完整插件及数据备份：{}", plan.name, backup.display()),
        None => format!("已停用 {} 并取消直接目录登记；外部原始文件保留在 {}。保留禁用配置，防止父目录扫描重新启用。", plan.name, plan.directory.display()),
    })
}

#[cfg(test)]
pub(crate) mod tests {
    use super::*;
    use crate::model::tests::Fixture;
    use std::io::Write;
    pub fn source(fixture: &Fixture) -> PathBuf {
        let source = fixture.0.join("source");
        fs::create_dir(&source).unwrap();
        fs::write(source.join("plugin.json"), r#"{"id":"demo","name":"示例插件","protocol_version":2,"executable":"tool.exe","keywords":["demo"],"config_defaults":"defaults.toml"}"#).unwrap();
        fs::write(source.join("tool.exe"), "fixture, never executed").unwrap();
        fs::write(
            source.join("defaults.toml"),
            "enabled=true\nkeywords=['demo']\noption='default'\n",
        )
        .unwrap();
        source
    }
    fn zip(path: &Path, files: &[(&str, &[u8])]) {
        let mut writer = zip::ZipWriter::new(fs::File::create(path).unwrap());
        for (name, bytes) in files {
            writer
                .start_file(
                    *name,
                    zip::write::SimpleFileOptions::default()
                        .compression_method(zip::CompressionMethod::Deflated),
                )
                .unwrap();
            writer.write_all(bytes).unwrap();
        }
        writer.finish().unwrap();
    }
    #[test]
    fn cancelled_prepared_install_cleans_staging_without_changing_settings() {
        let fixture = Fixture::new();
        let source = source(&fixture);
        let draft = fixture.draft();
        let before = fs::read(&draft.path).unwrap();
        let plan = prepare(&source, &draft).unwrap();
        let stage = plan._stage.path.clone();
        let target = plan.target.clone();
        assert!(stage.exists());
        drop(plan);
        assert!(!stage.exists());
        assert!(!target.exists());
        assert_eq!(fs::read(&draft.path).unwrap(), before);
        assert!(source.join("tool.exe").exists());
    }

    #[test]
    fn install_copies_directory_and_uninstall_archives_all_data() {
        let fixture = Fixture::new();
        let source = source(&fixture);
        let mut draft = fixture.draft();
        let before = fs::read_to_string(&draft.path).unwrap();
        let plan = prepare(&source, &draft).unwrap();
        assert_eq!(plan.name, "示例插件");
        assert!(!plan.target.exists());
        assert_eq!(fs::read_to_string(&draft.path).unwrap(), before);
        let target = plan.target.clone();
        install(plan, &mut draft).unwrap();
        assert!(target.join("tool.exe").is_file());
        assert!(source.join("tool.exe").is_file());
        assert_eq!(
            draft.value["plugins"]["demo"]["option"].as_str(),
            Some("default")
        );
        draft.value["plugins"]["demo"]["option"] = Value::String("custom option".into());
        draft.save(false).unwrap();
        fs::write(target.join("private-data.dat"), "keep me").unwrap();
        let plan = prepare_uninstall(&target, "demo", &draft).unwrap();
        assert!(plan.archive);
        uninstall(plan, &mut draft).unwrap();
        assert!(!target.exists());
        assert!(draft.value["plugins"].get("demo").is_none());
        let backup = fs::read_dir(fixture.0.join("plugin-backups"))
            .unwrap()
            .next()
            .unwrap()
            .unwrap()
            .path();
        assert_eq!(
            fs::read_to_string(backup.join("private-data.dat")).unwrap(),
            "keep me"
        );
        assert!(backup.join("tool.exe").exists());
        let snapshot = fs::read_dir(&backup)
            .unwrap()
            .filter_map(Result::ok)
            .find(|e| {
                e.file_name()
                    .to_string_lossy()
                    .starts_with(".mini-launcher-settings-")
            })
            .unwrap()
            .path();
        let snapshot: Value = toml::from_str(&fs::read_to_string(snapshot).unwrap()).unwrap();
        assert_eq!(
            snapshot["plugins"]["demo"]["option"].as_str(),
            Some("custom option")
        );
        assert_eq!(snapshot["plugins"]["demo"]["enabled"].as_bool(), Some(true));
        assert!(source.join("plugin.json").exists());
        // A fresh install is possible after archiving the previous installation.
        assert!(prepare(&source, &draft).is_ok());
    }
    #[test]
    fn wrapper_zip_updates_in_place_and_preserves_configuration_and_data() {
        let fixture = Fixture::new();
        let source = source(&fixture);
        let mut draft = fixture.draft();
        let archive = fixture.0.join("demo.zip");
        let manifest = fs::read(source.join("plugin.json")).unwrap();
        let defaults = fs::read(source.join("defaults.toml")).unwrap();
        zip(
            &archive,
            &[
                ("demo/plugin.json", &manifest),
                ("demo/defaults.toml", &defaults),
                ("demo/tool.exe", b"zip executable fixture"),
            ],
        );
        let plan = prepare(&archive, &draft).unwrap();
        let target = plan.target.clone();
        install(plan, &mut draft).unwrap();
        assert_eq!(
            fs::read_to_string(target.join("tool.exe")).unwrap(),
            "zip executable fixture"
        );
        fs::write(target.join("local-data.dat"), "user data").unwrap();
        fs::write(target.join("settings.schema.json"), "{}").unwrap();
        draft.value["plugins"]["demo"]["option"] = Value::String("custom".into());
        draft.value["plugins"]["demo"]["enabled"] = Value::Boolean(false);
        draft.save(false).unwrap();
        zip(
            &archive,
            &[
                ("demo/plugin.json", &manifest),
                (
                    "demo/defaults.toml",
                    b"option='new default'\nnew_option=true",
                ),
                ("demo/tool.exe", b"updated executable"),
            ],
        );
        let plan = prepare(&archive, &draft).unwrap();
        assert!(plan.is_update());
        install(plan, &mut draft).unwrap();
        assert_eq!(
            draft.value["plugins"]["demo"]["option"].as_str(),
            Some("custom")
        );
        assert_eq!(
            draft.value["plugins"]["demo"]["enabled"].as_bool(),
            Some(false)
        );
        assert_eq!(
            draft.value["plugins"]["demo"]["new_option"].as_bool(),
            Some(true)
        );
        assert_eq!(
            fs::read_to_string(target.join("tool.exe")).unwrap(),
            "updated executable"
        );
        assert_eq!(
            fs::read_to_string(target.join("local-data.dat")).unwrap(),
            "user data"
        );
        assert!(!target.join("settings.schema.json").exists());
        let backup = fs::read_dir(fixture.0.join("plugin-backups"))
            .unwrap()
            .next()
            .unwrap()
            .unwrap()
            .path();
        assert_eq!(
            fs::read_to_string(backup.join("tool.exe")).unwrap(),
            "zip executable fixture"
        );
    }

    #[test]
    fn invalid_updated_schema_rolls_back_files_and_restores_enabled_state() {
        let fixture = Fixture::new();
        let source = source(&fixture);
        let mut draft = fixture.draft();
        draft.value["plugins"]["enabled"] = Value::Boolean(true);
        draft.save(false).unwrap();
        let plan = prepare(&source, &draft).unwrap();
        let target = plan.target.clone();
        install(plan, &mut draft).unwrap();
        fs::write(source.join("tool.exe"), "new executable").unwrap();
        fs::write(
            source.join("settings.schema.json"),
            r#"{"fields":{"option":{"type":"integer"}}}"#,
        )
        .unwrap();
        let plan = prepare(&source, &draft).unwrap();
        assert!(install(plan, &mut draft)
            .unwrap_err()
            .contains("更新已回滚"));
        assert_eq!(
            draft.value["plugins"]["demo"]["enabled"].as_bool(),
            Some(true)
        );
        assert_eq!(
            fs::read_to_string(target.join("tool.exe")).unwrap(),
            "fixture, never executed"
        );
        assert!(!target.join("settings.schema.json").exists());
    }
    #[test]
    fn traversal_duplicate_and_symlink_archives_leave_no_active_files() {
        let fixture = Fixture::new();
        let draft = fixture.draft();
        for name in [
            "../escape",
            "..\\escape",
            "C:/escape",
            "/escape",
            "demo/file:stream",
            "demo/NUL.txt",
        ] {
            let archive = fixture.0.join("bad.zip");
            zip(&archive, &[(name, b"bad")]);
            assert!(prepare(&archive, &draft).is_err(), "{name}");
        }
        let archive = fixture.0.join("duplicate.zip");
        zip(&archive, &[("A.txt", b"a"), ("a.txt", b"b")]);
        assert!(prepare(&archive, &draft).err().unwrap().contains("冲突"));
        let archive = fixture.0.join("link.zip");
        let mut writer = zip::ZipWriter::new(fs::File::create(&archive).unwrap());
        writer
            .add_symlink(
                "link",
                "../outside",
                zip::write::SimpleFileOptions::default(),
            )
            .unwrap();
        writer.finish().unwrap();
        assert!(prepare(&archive, &draft)
            .err()
            .unwrap()
            .contains("符号链接"));
        assert!(!fixture.0.join("escape").exists());
        assert_eq!(
            fs::read_dir(fixture.0.join(".plugin-staging"))
                .unwrap()
                .count(),
            0
        );
        assert_eq!(fs::read_dir(fixture.0.join("plugins")).unwrap().count(), 0);
    }
    #[test]
    fn failed_settings_save_rolls_back_copied_files() {
        let fixture = Fixture::new();
        let source = source(&fixture);
        let mut draft = fixture.draft();
        let plan = prepare(&source, &draft).unwrap();
        let target = plan.target.clone();
        let mut invalid = draft.value.clone();
        invalid["max_results"] = Value::Integer(100);
        fs::write(&draft.path, toml::to_string(&invalid).unwrap()).unwrap();
        assert!(install(plan, &mut draft).unwrap_err().contains("回滚"));
        assert!(!target.exists());
        assert!(source.join("tool.exe").exists());
    }
    #[test]
    fn external_unregistration_keeps_source_and_disabled_tombstone() {
        let fixture = Fixture::new();
        let source = source(&fixture);
        let mut draft = fixture.draft();
        draft.register(&source).unwrap();
        draft.save(false).unwrap();
        let plan = prepare_uninstall(&source, "demo", &draft).unwrap();
        assert!(!plan.archive);
        uninstall(plan, &mut draft).unwrap();
        assert!(source.join("tool.exe").exists());
        assert_eq!(
            draft.value["plugins"]["demo"]["enabled"].as_bool(),
            Some(false)
        );
        assert!(!draft.value["plugins"]["directories"]
            .as_array()
            .unwrap()
            .iter()
            .any(|v| v
                .as_str()
                .is_some_and(|s| identity(Path::new(s)) == identity(&source))));
    }
    #[test]
    fn dirty_draft_and_unsafe_uninstall_key_are_rejected() {
        let fixture = Fixture::new();
        let source = source(&fixture);
        let mut draft = fixture.draft();
        assert!(prepare_uninstall(&source, "../outside", &draft).is_err());
        draft.value["theme"] = Value::String("dark".into());
        assert!(prepare(&source, &draft)
            .err()
            .unwrap()
            .contains("尚未自动保存"));
        assert!(prepare_uninstall(&source, "demo", &draft)
            .err()
            .unwrap()
            .contains("尚未自动保存"));
    }
    #[test]
    #[cfg(windows)]
    fn locked_executable_is_not_treated_as_stopped() {
        use std::os::windows::fs::OpenOptionsExt;
        let fixture = Fixture::new();
        let path = fixture.0.join("locked.exe");
        fs::write(&path, "fixture").unwrap();
        let lock = fs::OpenOptions::new()
            .read(true)
            .share_mode(0)
            .open(&path)
            .unwrap();
        assert!(wait_stopped_for(Some(&path), Duration::ZERO).is_err());
        drop(lock);
        assert!(wait_stopped_for(Some(&path), Duration::ZERO).is_ok());
    }
}
