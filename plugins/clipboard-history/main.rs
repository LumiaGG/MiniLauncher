mod plugin;
use plugin::{execute, prepare_drag, query};
mod files;
mod history;
mod monitor;

use history::{History, State};
use serde_json::{json, Value};
use std::{
    fs, io,
    os::windows::fs::OpenOptionsExt,
    path::PathBuf,
    sync::{Arc, Mutex},
};

fn initialize(params: &Value) -> Result<(Arc<Mutex<State>>, fs::File), String> {
    let maximum = if let Some(path) = params["settings_path"].as_str() {
        let text = fs::read_to_string(path).map_err(|e| e.to_string())?;
        let settings: toml::Value = toml::from_str(&text).map_err(|e| e.to_string())?;
        let key = params["config_key"].as_str().unwrap_or("clipboard_history");
        match settings
            .get("plugins")
            .and_then(|v| v.get(key))
            .and_then(|v| v.get("max_history"))
        {
            Some(value) => value
                .as_integer()
                .filter(|n| (1..=500).contains(n))
                .ok_or("max_history 必须是 1–500 的整数")? as usize,
            None => 100,
        }
    } else {
        100
    };
    let directory = mini_launcher::config::data_dir().join("clipboard-history");
    fs::create_dir_all(&directory).map_err(|e| e.to_string())?;
    let lock = fs::OpenOptions::new()
        .write(true)
        .create(true)
        .truncate(false)
        .share_mode(0)
        .open(directory.join("history.lock"))
        .map_err(|_| "剪贴板历史正被另一实例使用，或数据目录不可写".to_string())?;
    let path: PathBuf = directory.join("history.dat");
    let history: History = history::load(&path)?;
    let trimmed = history.entries.len() > maximum;
    let mut state = State {
        history,
        path,
        maximum,
        error: None,
        revision: history::random_id()?,
        window: 0,
    };
    if trimmed {
        state.commit(state.history.clone())?;
    }
    let state = Arc::new(Mutex::new(state));
    let window = monitor::start(state.clone())?;
    state.lock().map_err(|_| "剪贴板状态不可用")?.window = window;
    Ok((state, lock))
}

fn run() -> Result<(), String> {
    let mut input = io::stdin().lock();
    let mut output = io::stdout().lock();
    let mut initialized: Option<(Arc<Mutex<State>>, fs::File)> = None;
    mini_launcher::plugin_rpc::serve(&mut input, &mut output, |request| {
        match request["method"].as_str().unwrap_or("") {
            "initialize" if initialized.is_none() => initialize(&request["params"]).map(|state| {
                initialized = Some(state);
                json!({"protocol_version":2})
            }),
            _ => match &initialized {
                Some((state, _lock)) => {
                    let mut state = state.lock().map_err(|_| "剪贴板状态不可用")?;
                    match request["method"].as_str().unwrap_or("") {
                        "query" => Ok(
                            json!({"items":query(&state, request["params"]["query"].as_str().unwrap_or(""), request["params"]["max_results"].as_u64().unwrap_or(100).min(100) as usize)}),
                        ),
                        "execute" => execute(
                            &mut state,
                            request["params"]["action_id"].as_str().unwrap_or(""),
                        ),
                        "prepare_drag" => prepare_drag(
                            &state,
                            request["params"]["action_id"].as_str().unwrap_or(""),
                        ),
                        _ => Err("不支持的方法".into()),
                    }
                }
                None => Err("请先初始化插件".into()),
            },
        }
    })?;
    Ok(())
}

fn main() {
    if let Err(error) = run() {
        eprintln!("{error}");
        std::process::exit(1);
    }
}

#[cfg(test)]
mod tests;
