//! Native Windows Caffeine (Awake) plugin for MiniLauncher.
//! Uses `SetThreadExecutionState` on a dedicated resident worker thread so that
//! sleep/display locks are automatically released by the OS if the plugin or host exits.

mod runtime;
mod tray;

use serde::{Deserialize, Serialize};
use serde_json::{json, Value};
use std::{
    fs, io,
    path::{Path, PathBuf},
    sync::{
        atomic::{AtomicU32, Ordering},
        mpsc::{self, Receiver, Sender},
        Arc, Mutex,
    },
    time::{Duration, Instant, SystemTime, UNIX_EPOCH},
};
use windows_sys::Win32::System::Power::{
    SetThreadExecutionState, ES_CONTINUOUS, ES_DISPLAY_REQUIRED, ES_SYSTEM_REQUIRED,
    EXECUTION_STATE,
};
#[cfg(not(test))]
use windows_sys::Win32::{
    Foundation::HWND,
    System::DataExchange::COPYDATASTRUCT,
    UI::WindowsAndMessaging::{
        FindWindowW, GetWindowThreadProcessId, SendMessageTimeoutW, SMTO_ABORTIFHUNG, WM_COPYDATA,
    },
};

const MAX_DURATION_MINS: u64 = 1440;
const REFRESH_INTERVAL: Duration = Duration::from_secs(10);
#[allow(dead_code)]
const STATUS_CAPSULE_MAGIC: usize = 0x4D4C_5343; // 'MLSC'

#[derive(Serialize)]
struct StatusCapsulePayload<'a> {
    plugin_id: &'a str,
    text: String,
    glyph: Option<&'a str>,
    badge_color: Option<u32>,
    hint_normal: Option<&'a str>,
    hint_scoped: Option<&'a str>,
    click_action: Option<&'a str>,
    context_action: Option<&'a str>,
}

#[derive(Serialize, Deserialize)]
struct RuntimeState {
    host_pid: u32,
    mode: String,
    duration_secs: Option<u64>,
    deadline_epoch_ms: Option<u64>,
}

#[cfg(test)]
static LAST_NOTIFY: Mutex<(u8, isize)> = Mutex::new((0, 0));
#[cfg(test)]
static TEST_HOST_PID: AtomicU32 = AtomicU32::new(0);

#[cfg(not(test))]
fn get_host_hwnd() -> HWND {
    if let Ok(val) = std::env::var("MINILAUNCHER_HOST_HWND") {
        if let Ok(ptr) = val.parse::<usize>() {
            if ptr != 0 {
                return ptr as HWND;
            }
        }
    }
    unsafe {
        let class: Vec<u16> = "MiniLauncher.Native.Window"
            .encode_utf16()
            .chain(Some(0))
            .collect();
        FindWindowW(class.as_ptr(), std::ptr::null())
    }
}

fn notify_host_off() {
    notify_host(0, 0, None);
}

#[cfg(test)]
fn notify_host(mode: u8, remaining_secs: isize, _payload: Option<&StatusCapsulePayload>) {
    if let Ok(mut guard) = LAST_NOTIFY.lock() {
        *guard = (mode, remaining_secs);
    }
}

#[cfg(not(test))]
fn notify_host(_mode: u8, _remaining_secs: isize, payload: Option<&StatusCapsulePayload>) {
    unsafe {
        let hwnd = get_host_hwnd();
        if !hwnd.is_null() {
            let json = match payload {
                Some(p) => serde_json::to_vec(p).unwrap_or_default(),
                None => Vec::new(),
            };
            let cds = COPYDATASTRUCT {
                dwData: STATUS_CAPSULE_MAGIC,
                cbData: json.len() as u32,
                lpData: json.as_ptr() as *mut _,
            };
            let mut result: usize = 0;
            SendMessageTimeoutW(
                hwnd,
                WM_COPYDATA,
                0,
                &cds as *const _ as isize,
                SMTO_ABORTIFHUNG,
                200,
                &mut result,
            );
        }
    }
}

fn notify_session(session: Option<ActiveSession>, now: Instant) {
    match session {
        Some(s) => {
            let mode = match s.mode {
                PowerMode::Display => 1,
                PowerMode::System => 2,
            };
            let remaining = s
                .deadline
                .map(|d| d.saturating_duration_since(now).as_secs().max(1) as isize)
                .unwrap_or(0);
            let (short_mode, glyph, badge_color) = match s.mode {
                PowerMode::Display => ("屏幕常亮", "\u{E706}", 0xF59E0B),
                PowerMode::System => ("仅防睡眠", "\u{EC46}", 0x38BDF8),
            };
            let remaining_secs = remaining.max(0) as u64;
            let remaining_short = if remaining_secs == 0 {
                "常驻".to_string()
            } else if remaining_secs >= 3600 {
                let total_mins = remaining_secs.div_ceil(60);
                let h = total_mins / 60;
                let m = total_mins % 60;
                if m == 0 {
                    format!("{h}小时")
                } else {
                    format!("{h}小时{m}分")
                }
            } else if remaining_secs >= 60 {
                format!("{}分钟", remaining_secs.div_ceil(60))
            } else {
                format!("{}秒", remaining_secs.max(1))
            };
            let text = format!("{} · {}", short_mode, remaining_short);
            let payload = StatusCapsulePayload {
                plugin_id: "caffeine",
                text,
                glyph: Some(glyph),
                badge_color: Some(badge_color),
                hint_normal: Some("左键管理防休眠    右键立即关闭防休眠"),
                hint_scoped: Some("左键切换防休眠模式    右键立即关闭防休眠"),
                click_action: Some("switch_mode"),
                context_action: Some("off"),
            };
            notify_host(mode, remaining, Some(&payload));
        }
        None => notify_host(0, 0, None),
    }
}

fn runtime_file(settings_path: Option<&Path>) -> Option<PathBuf> {
    std::env::var_os("MINILAUNCHER_DATA_DIR")
        .or_else(|| std::env::var_os("MINI_LAUNCHER_DATA_DIR"))
        .map(PathBuf::from)
        .or_else(|| settings_path.and_then(|p| p.parent().map(Path::to_path_buf)))
        .map(|dir| dir.join("caffeine-runtime.json"))
}

fn get_host_pid() -> u32 {
    #[cfg(test)]
    {
        TEST_HOST_PID.load(Ordering::SeqCst)
    }
    #[cfg(not(test))]
    unsafe {
        let hwnd = get_host_hwnd();
        let mut pid = 0;
        if !hwnd.is_null() {
            GetWindowThreadProcessId(hwnd, &mut pid);
        }
        pid
    }
}

fn save_runtime(settings_path: Option<&Path>, session: &ActiveSession) {
    if let Some(path) = runtime_file(settings_path) {
        let host_pid = get_host_pid();
        if host_pid == 0 {
            return;
        }
        let state = RuntimeState {
            host_pid,
            mode: session.mode.key().to_string(),
            duration_secs: session.duration.map(|d| d.as_secs()),
            deadline_epoch_ms: session.deadline.map(|d| {
                let remaining_ms = d.saturating_duration_since(Instant::now()).as_millis() as u64;
                let now_ms = SystemTime::now()
                    .duration_since(UNIX_EPOCH)
                    .unwrap_or_default()
                    .as_millis() as u64;
                now_ms.saturating_add(remaining_ms)
            }),
        };
        if let Ok(text) = serde_json::to_string(&state) {
            let _ = fs::write(path, text);
        }
    }
}

fn delete_runtime(settings_path: Option<&Path>) {
    if let Some(path) = runtime_file(settings_path) {
        let _ = fs::remove_file(path);
    }
}

fn load_runtime(settings_path: Option<&Path>) -> Option<ActiveSession> {
    let path = runtime_file(settings_path)?;
    let text = fs::read_to_string(&path).ok()?;
    let state: RuntimeState = serde_json::from_str(&text).ok()?;
    let host_pid = get_host_pid();
    if host_pid == 0 || state.host_pid != host_pid {
        let _ = fs::remove_file(&path);
        return None;
    }
    let mode = PowerMode::parse(&state.mode)?;
    let duration = state.duration_secs.map(Duration::from_secs);
    let deadline = match state.deadline_epoch_ms {
        Some(deadline_ms) => {
            let now_ms = SystemTime::now()
                .duration_since(UNIX_EPOCH)
                .unwrap_or_default()
                .as_millis() as u64;
            if deadline_ms <= now_ms {
                let _ = fs::remove_file(&path);
                return None;
            }
            Some(Instant::now() + Duration::from_millis(deadline_ms - now_ms))
        }
        None => None,
    };
    Some(ActiveSession {
        mode,
        duration,
        deadline,
    })
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum PowerMode {
    Display,
    System,
}

impl PowerMode {
    fn label(self) -> &'static str {
        match self {
            Self::Display => "保持唤醒 + 屏幕常亮",
            Self::System => "仅防系统睡眠（允许息屏）",
        }
    }

    fn short_label(self) -> &'static str {
        match self {
            Self::Display => "屏幕常亮",
            Self::System => "仅防睡眠",
        }
    }

    fn key(self) -> &'static str {
        match self {
            Self::Display => "display",
            Self::System => "system",
        }
    }

    fn parse(text: &str) -> Option<Self> {
        match text {
            "display" => Some(Self::Display),
            "system" => Some(Self::System),
            _ => None,
        }
    }

    fn execution_flags(self) -> EXECUTION_STATE {
        match self {
            Self::Display => ES_CONTINUOUS | ES_SYSTEM_REQUIRED | ES_DISPLAY_REQUIRED,
            Self::System => ES_CONTINUOUS | ES_SYSTEM_REQUIRED,
        }
    }

    fn opposite(self) -> Self {
        match self {
            Self::Display => Self::System,
            Self::System => Self::Display,
        }
    }

    fn icon_path(self) -> &'static str {
        match self {
            Self::Display => "Images/display.ico",
            Self::System => "Images/system.ico",
        }
    }
}

#[derive(Clone, Copy, Debug)]
struct ActiveSession {
    mode: PowerMode,
    duration: Option<Duration>,
    deadline: Option<Instant>,
}

#[derive(Clone, Copy, Debug)]
struct Config {
    keep_display_on: bool,
    default_duration_mins: u64,
    show_tray_icon: bool,
}

impl Default for Config {
    fn default() -> Self {
        Self {
            keep_display_on: true,
            default_duration_mins: 0,
            show_tray_icon: true,
        }
    }
}

impl Config {
    fn default_mode(self) -> PowerMode {
        if self.keep_display_on {
            PowerMode::Display
        } else {
            PowerMode::System
        }
    }
}

fn read_config(settings_path: Option<&Path>, config_key: &str) -> Result<Config, String> {
    let Some(path) = settings_path else {
        return Ok(Config::default());
    };
    let Ok(text) = fs::read_to_string(path) else {
        return Ok(Config::default());
    };
    let settings: toml::Value =
        toml::from_str(text.trim_start_matches('\u{feff}')).map_err(|e| e.to_string())?;
    let Some(section) = settings.get("plugins").and_then(|v| v.get(config_key)) else {
        return Ok(Config::default());
    };
    let keep_display_on = match section.get("keep_display_on") {
        Some(v) => v.as_bool().ok_or("keep_display_on 必须是布尔值")?,
        None => true,
    };
    let default_duration_mins = match section.get("default_duration_mins") {
        Some(v) => {
            let n = v
                .as_integer()
                .filter(|&n| (0..=MAX_DURATION_MINS as i64).contains(&n))
                .ok_or("default_duration_mins 必须是 0–1440 的整数")?;
            n as u64
        }
        None => 0,
    };
    let show_tray_icon = match section.get("show_tray_icon") {
        Some(v) => v.as_bool().ok_or("show_tray_icon 必须是布尔值")?,
        None => true,
    };
    Ok(Config {
        keep_display_on,
        default_duration_mins,
        show_tray_icon,
    })
}

enum WorkerSignal {
    Sync,
    #[cfg(test)]
    Stop,
}

#[derive(Clone)]
struct Controller {
    active: Arc<Mutex<Option<ActiveSession>>>,
    sender: Sender<WorkerSignal>,
    #[cfg(test)]
    applied_flags: Arc<AtomicU32>,
    settings_path: Option<PathBuf>,
    config_key: String,
}

impl Controller {
    fn new(settings_path: Option<PathBuf>, config_key: String) -> Self {
        Self::with_refresh(settings_path, config_key, REFRESH_INTERVAL)
    }

    fn with_refresh(
        settings_path: Option<PathBuf>,
        config_key: String,
        refresh_interval: Duration,
    ) -> Self {
        let active: Arc<Mutex<Option<ActiveSession>>> = Arc::new(Mutex::new(None));
        let applied_flags = Arc::new(AtomicU32::new(ES_CONTINUOUS));
        let (sender, receiver) = mpsc::channel();
        let worker_active = active.clone();
        let worker_flags = applied_flags.clone();
        let worker_settings_path = settings_path.clone();
        let worker_config_key = config_key.clone();
        std::thread::Builder::new()
            .name("caffeine-power".into())
            .spawn(move || {
                power_worker(
                    worker_active,
                    worker_flags,
                    worker_settings_path,
                    worker_config_key,
                    receiver,
                    refresh_interval,
                )
            })
            .expect("启动电源管理线程失败");
        Self {
            active,
            sender,
            #[cfg(test)]
            applied_flags,
            settings_path,
            config_key,
        }
    }

    fn load_config(&self) -> Result<Config, String> {
        read_config(self.settings_path.as_deref(), &self.config_key)
    }

    fn snapshot(&self) -> Result<Option<ActiveSession>, String> {
        let mut guard = self.active.lock().map_err(|_| "防休眠状态不可用")?;
        if guard
            .as_ref()
            .and_then(|s| s.deadline)
            .is_some_and(|deadline| Instant::now() >= deadline)
        {
            *guard = None;
            delete_runtime(self.settings_path.as_deref());
            notify_host_off();
            tray::sync(None, false);
            let _ = self.sender.send(WorkerSignal::Sync);
        }
        Ok(*guard)
    }

    fn set_session(&self, session: Option<ActiveSession>) -> Result<(), String> {
        let mut guard = self.active.lock().map_err(|_| "防休眠状态不可用")?;
        *guard = session;
        if let Some(s) = session {
            save_runtime(self.settings_path.as_deref(), &s);
        } else {
            delete_runtime(self.settings_path.as_deref());
        }
        let show_tray = self.load_config().map(|c| c.show_tray_icon).unwrap_or(true);
        notify_session(session, Instant::now());
        tray::sync(session, show_tray);
        drop(guard);
        let _ = self.sender.send(WorkerSignal::Sync);
        Ok(())
    }

    fn activate(&self, mode: PowerMode, duration: Option<Duration>) -> Result<(), String> {
        let deadline = duration.map(|d| Instant::now() + d);
        self.set_session(Some(ActiveSession {
            mode,
            duration,
            deadline,
        }))
    }

    fn deactivate(&self) -> Result<(), String> {
        self.set_session(None)
    }
}

fn power_worker(
    active: Arc<Mutex<Option<ActiveSession>>>,
    applied_flags: Arc<AtomicU32>,
    settings_path: Option<PathBuf>,
    config_key: String,
    receiver: Receiver<WorkerSignal>,
    refresh_interval: Duration,
) {
    loop {
        let now = Instant::now();
        let (flags, wait, current_session) = {
            let mut guard = match active.lock() {
                Ok(guard) => guard,
                Err(_) => break,
            };
            if guard
                .as_ref()
                .and_then(|s| s.deadline)
                .is_some_and(|deadline| now >= deadline)
            {
                *guard = None;
                delete_runtime(settings_path.as_deref());
            }
            match *guard {
                Some(session) => {
                    let timeout = match session.deadline {
                        Some(deadline) => deadline
                            .saturating_duration_since(now)
                            .min(refresh_interval),
                        None => refresh_interval,
                    };
                    (session.mode.execution_flags(), Some(timeout), Some(session))
                }
                None => (ES_CONTINUOUS, None, None),
            }
        };

        applied_flags.store(flags, Ordering::SeqCst);
        unsafe {
            SetThreadExecutionState(flags);
        }
        notify_session(current_session, now);
        let show_tray = read_config(settings_path.as_deref(), &config_key)
            .map(|c| c.show_tray_icon)
            .unwrap_or(true);
        tray::sync(current_session, show_tray);

        match wait {
            Some(timeout) => match receiver.recv_timeout(timeout) {
                Ok(WorkerSignal::Sync) | Err(mpsc::RecvTimeoutError::Timeout) => {}
                #[cfg(test)]
                Ok(WorkerSignal::Stop) => break,
                Err(mpsc::RecvTimeoutError::Disconnected) => break,
            },
            None => match receiver.recv() {
                Ok(WorkerSignal::Sync) => {}
                #[cfg(test)]
                Ok(WorkerSignal::Stop) => break,
                Err(_) => break,
            },
        }
    }
    tray::shutdown();
    notify_host_off();
    applied_flags.store(ES_CONTINUOUS, Ordering::SeqCst);
    unsafe {
        SetThreadExecutionState(ES_CONTINUOUS);
    }
}

fn format_duration_mins(mins: u64) -> String {
    if mins == 0 {
        return "无限期".into();
    }
    let hours = mins / 60;
    let rem = mins % 60;
    match (hours, rem) {
        (0, m) => format!("{m} 分钟"),
        (h, 0) => format!("{h} 小时"),
        (h, m) => format!("{h} 小时 {m} 分钟"),
    }
}

fn format_remaining(deadline: Instant) -> String {
    let secs = deadline
        .saturating_duration_since(Instant::now())
        .as_secs()
        .max(1);
    if secs >= 3600 {
        let hours = secs / 3600;
        let mins = (secs % 3600).div_ceil(60);
        if mins == 60 {
            format!("剩余 {} 小时", hours + 1)
        } else if mins == 0 {
            format!("剩余 {hours} 小时")
        } else {
            format!("剩余 {hours} 小时 {mins} 分钟")
        }
    } else if secs >= 60 {
        format!("剩余 {} 分钟", secs.div_ceil(60))
    } else {
        format!("剩余 {secs} 秒")
    }
}

fn parse_duration_mins(input: &str) -> Option<u64> {
    let mut s = input.trim().to_ascii_lowercase();
    if s.is_empty() {
        return None;
    }
    for prefix in ["timer ", "time ", "for ", "定时 ", "定时"] {
        if let Some(rest) = s.strip_prefix(prefix) {
            s = rest.trim().to_string();
            break;
        }
    }
    if let Ok(mins) = s.parse::<u64>() {
        return (1..=MAX_DURATION_MINS).contains(&mins).then_some(mins);
    }
    for suffix in ["minutes", "minute", "mins", "min", "m", "分钟", "分"] {
        if let Some(prefix) = s.strip_suffix(suffix) {
            let prefix = prefix.trim();
            if let Ok(mins) = prefix.parse::<u64>() {
                return (1..=MAX_DURATION_MINS).contains(&mins).then_some(mins);
            }
        }
    }
    for suffix in ["hours", "hour", "hrs", "hr", "h", "小时", "时"] {
        if let Some(prefix) = s.strip_suffix(suffix) {
            let prefix = prefix.trim();
            if let Ok(hours) = prefix.parse::<f64>() {
                if hours.is_finite() && hours > 0.0 {
                    let mins = (hours * 60.0).round() as u64;
                    return (1..=MAX_DURATION_MINS).contains(&mins).then_some(mins);
                }
            }
        }
    }
    for sep in ["小时", "hours", "hour", "hrs", "hr", "h"] {
        if let Some((h_part, m_part)) = s.split_once(sep) {
            let Ok(hours) = h_part.trim().parse::<u64>() else {
                continue;
            };
            let mut m_clean = m_part.trim();
            for m_suffix in ["minutes", "minute", "mins", "min", "m", "分钟", "分"] {
                if let Some(stripped) = m_clean.strip_suffix(m_suffix) {
                    m_clean = stripped.trim();
                    break;
                }
            }
            let Ok(mins) = m_clean.parse::<u64>() else {
                continue;
            };
            let total = hours.saturating_mul(60).saturating_add(mins);
            if (1..=MAX_DURATION_MINS).contains(&total) {
                return Some(total);
            }
        }
    }
    None
}

fn action_id(mode: PowerMode, mins: u64) -> String {
    format!("on:{}:{mins}", mode.key())
}

fn context_menu(mode: PowerMode, mins: u64, active: bool) -> Vec<Value> {
    let opposite = mode.opposite();
    let mut actions = vec![json!({
        "title": format!("切换为：{}（{}）", opposite.short_label(), format_duration_mins(mins)),
        "action": { "type": "execute", "id": action_id(opposite, mins) }
    })];
    for preset in [0u64, 15, 30, 60, 120, 240] {
        if preset != mins {
            let title = if preset == 0 {
                format!("无限期开启（{}）", mode.short_label())
            } else {
                format!(
                    "定时 {}（{}）",
                    format_duration_mins(preset),
                    mode.short_label()
                )
            };
            actions.push(json!({
                "title": title,
                "action": { "type": "execute", "id": action_id(mode, preset) }
            }));
        }
    }
    if active {
        actions.push(json!({
            "title": "关闭防休眠",
            "action": { "type": "execute", "id": "off" }
        }));
    }
    actions
}

fn build_activate_item(
    id: &str,
    title: String,
    subtitle: String,
    mode: PowerMode,
    mins: u64,
    active: bool,
) -> Value {
    json!({
        "id": id,
        "title": title,
        "subtitle": subtitle,
        "icon": mode.icon_path(),
        "action": { "type": "execute", "id": action_id(mode, mins) },
        "context_actions": context_menu(mode, mins, active)
    })
}

fn query(controller: &Controller, text: &str, maximum: usize) -> Result<Vec<Value>, String> {
    if maximum == 0 {
        return Ok(Vec::new());
    }
    let config = controller.load_config()?;
    let current = controller.snapshot()?;
    let active = current.is_some();
    let default_mode = config.default_mode();
    let default_mins = config.default_duration_mins;
    let trimmed = text.trim();
    let lower = trimmed.to_ascii_lowercase();

    let status_note = match current {
        Some(session) => {
            let timer = match session.deadline {
                Some(deadline) => format_remaining(deadline),
                None => "无限期".into(),
            };
            format!("当前运行中：{} · {timer}", session.mode.short_label())
        }
        None => "当前已关闭（系统正常息屏与睡眠）".into(),
    };

    let mut items = Vec::new();

    if let Some(custom_mins) = parse_duration_mins(trimmed) {
        let duration_label = format_duration_mins(custom_mins);
        let mode = current.map_or(default_mode, |session| session.mode);
        items.push(build_activate_item(
            "custom-primary",
            format!("定时 {duration_label} · {}", mode.label()),
            format!("Enter 开始 {duration_label} 倒计时 · 更多设置可右键托盘咖啡图标"),
            mode,
            custom_mins,
            active,
        ));
        items.truncate(maximum.min(100));
        return Ok(items);
    }

    if matches!(
        lower.as_str(),
        "off" | "stop" | "close" | "disable" | "quit" | "关闭" | "停止" | "退出"
    ) {
        items.push(json!({
            "id": "turn-off",
            "title": if active { "关闭防休眠（恢复系统自动睡眠）" } else { "防休眠已处于关闭状态" },
            "subtitle": status_note,
            "icon": "Images/off.ico",
            "action": { "type": "execute", "id": "off" }
        }));
        items.truncate(maximum.min(100));
        return Ok(items);
    }

    let active_mins = current
        .and_then(|s| s.duration)
        .map(|d| d.as_secs() / 60);

    // Keep only the toggle and mode switch; timer presets live in the tray.
    if let Some(session) = current {
        let timer = match session.deadline {
            Some(deadline) => format_remaining(deadline),
            None => "无限期".into(),
        };
        let cur_mins = active_mins.unwrap_or(0);
        items.push(json!({
            "id": "toggle-status",
            "title": format!("关闭防休眠（当前：{} · {timer}）", session.mode.short_label()),
            "subtitle": "Enter 关闭 · 模式与定时可右键托盘咖啡图标调整",
            "icon": "Images/off.ico",
            "action": { "type": "execute", "id": "off" },
            "context_actions": context_menu(session.mode, cur_mins, true)
        }));

    } else {
        items.push(build_activate_item(
            "toggle-status",
            format!(
                "开启防休眠（默认：{} · {}）",
                default_mode.short_label(),
                format_duration_mins(default_mins)
            ),
            "Enter 按默认配置开启 · 输入时长可定时，更多设置见托盘咖啡图标".into(),
            default_mode,
            default_mins,
            false,
        ));
    }

    let opposite = current.map_or(default_mode, |session| session.mode).opposite();
    let mins = if active { active_mins.unwrap_or(0) } else { default_mins };
    let mut switch = build_activate_item(
        "switch-mode",
        format!("{}：{}", if active { "切换为" } else { "开启" }, opposite.label()),
        if active {
            "Enter 切换模式，保留当前剩余时间".into()
        } else {
            format!("Enter 按另一模式开启 · {}", format_duration_mins(mins))
        },
        opposite,
        mins,
        active,
    );
    if active {
        switch["action"]["id"] = json!("switch_mode");
    }
    items.push(switch);

    items.truncate(maximum.min(100));
    Ok(items)
}

fn execute(controller: &Controller, id: &str) -> Result<Value, String> {
    if id == "off" {
        controller.deactivate()?;
        return Ok(json!({}));
    }
    if id == "on:default" {
        let config = controller.load_config()?;
        let duration = (config.default_duration_mins > 0)
            .then(|| Duration::from_secs(config.default_duration_mins * 60));
        controller.activate(config.default_mode(), duration)?;
        return Ok(json!({}));
    }
    if id == "switch_mode" {
        if let Some(mut session) = controller.snapshot()? {
            session.mode = session.mode.opposite();
            controller.set_session(Some(session))?;
        } else {
            let config = controller.load_config()?;
            let duration = (config.default_duration_mins > 0)
                .then(|| Duration::from_secs(config.default_duration_mins * 60));
            controller.activate(config.default_mode(), duration)?;
        }
        return Ok(json!({}));
    }
    if let Some(rest) = id.strip_prefix("on:") {
        let (mode_str, mins_str) = rest.split_once(':').ok_or("无效的防休眠动作参数")?;
        let mode = PowerMode::parse(mode_str).ok_or("未知的防休眠模式")?;
        let mins = mins_str
            .parse::<u64>()
            .ok()
            .filter(|&m| m <= MAX_DURATION_MINS)
            .ok_or("无效的防休眠定时分钟数")?;
        let duration = (mins > 0).then(|| Duration::from_secs(mins * 60));
        controller.activate(mode, duration)?;
        return Ok(json!({}));
    }
    Err("未知的防休眠操作".into())
}

fn initialize(params: &Value) -> Result<Controller, String> {
    let settings_path = params["settings_path"].as_str().map(PathBuf::from);
    let config_key = params["config_key"]
        .as_str()
        .unwrap_or("caffeine")
        .to_string();
    let restored = load_runtime(settings_path.as_deref());
    let controller = Controller::new(settings_path, config_key);
    if let Some(session) = restored {
        let _ = controller.set_session(Some(session));
    } else {
        notify_host_off();
    }
    // Validate user config on startup so invalid settings surface immediately in >plugins.
    controller.load_config()?;
    Ok(controller)
}

fn run() -> Result<(), String> {
    let mut input = io::stdin().lock();
    let mut output = io::stdout().lock();
    let mut initialized: Option<Controller> = None;
    let res = mini_launcher::plugin_rpc::serve(&mut input, &mut output, |request| {
        match request["method"].as_str().unwrap_or("") {
            "initialize" if initialized.is_none() => {
                let controller = initialize(&request["params"])?;
                tray::init(controller.clone());
                initialized = Some(controller);
                Ok(json!({ "protocol_version": 2 }))
            }
            _ => match &initialized {
                Some(controller) => match request["method"].as_str().unwrap_or("") {
                    "query" => {
                        let text = request["params"]["query"].as_str().unwrap_or("");
                        let max_results = request["params"]["max_results"]
                            .as_u64()
                            .unwrap_or(100)
                            .min(100) as usize;
                        Ok(json!({ "items": query(controller, text, max_results)? }))
                    }
                    "execute" => execute(
                        controller,
                        request["params"]["action_id"].as_str().unwrap_or(""),
                    ),
                    _ => Err("不支持的方法".into()),
                },
                None => Err("请先初始化插件".into()),
            },
        }
    });
    tray::shutdown();
    res
}

fn main() {
    if let Err(error) = run() {
        eprintln!("{error}");
        std::process::exit(1);
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn duration_parser_supports_minutes_hours_and_compounds() {
        assert_eq!(parse_duration_mins("45"), Some(45));
        assert_eq!(parse_duration_mins("30m"), Some(30));
        assert_eq!(parse_duration_mins("90 min"), Some(90));
        assert_eq!(parse_duration_mins("45分钟"), Some(45));
        assert_eq!(parse_duration_mins("2h"), Some(120));
        assert_eq!(parse_duration_mins("1.5h"), Some(90));
        assert_eq!(parse_duration_mins("2小时"), Some(120));
        assert_eq!(parse_duration_mins("1h30m"), Some(90));
        assert_eq!(parse_duration_mins("1h 15min"), Some(75));
        assert_eq!(parse_duration_mins("2小时30分"), Some(150));
        assert_eq!(parse_duration_mins("0"), None);
        assert_eq!(parse_duration_mins("1441"), None);
        assert_eq!(parse_duration_mins("25h"), None);
        assert_eq!(parse_duration_mins("abc"), None);
    }

    #[test]
    fn starts_off_and_toggles_execution_flags() {
        let controller =
            Controller::with_refresh(None, "caffeine".into(), Duration::from_millis(20));
        assert!(controller.snapshot().unwrap().is_none());
        assert_eq!(
            controller.applied_flags.load(Ordering::SeqCst),
            ES_CONTINUOUS
        );

        execute(&controller, "on:display:0").unwrap();
        std::thread::sleep(Duration::from_millis(30));
        assert_eq!(
            controller.snapshot().unwrap().map(|s| s.mode),
            Some(PowerMode::Display)
        );
        assert_eq!(
            controller.applied_flags.load(Ordering::SeqCst),
            ES_CONTINUOUS | ES_SYSTEM_REQUIRED | ES_DISPLAY_REQUIRED
        );

        execute(&controller, "on:system:60").unwrap();
        std::thread::sleep(Duration::from_millis(30));
        assert_eq!(
            controller.snapshot().unwrap().map(|s| s.mode),
            Some(PowerMode::System)
        );
        assert_eq!(
            controller.applied_flags.load(Ordering::SeqCst),
            ES_CONTINUOUS | ES_SYSTEM_REQUIRED
        );

        execute(&controller, "off").unwrap();
        std::thread::sleep(Duration::from_millis(30));
        assert!(controller.snapshot().unwrap().is_none());
        assert_eq!(
            controller.applied_flags.load(Ordering::SeqCst),
            ES_CONTINUOUS
        );
        let _ = controller.sender.send(WorkerSignal::Stop);
    }

    #[test]
    fn countdown_expiration_automatically_releases_power_request() {
        let controller =
            Controller::with_refresh(None, "caffeine".into(), Duration::from_millis(10));
        controller
            .activate(PowerMode::Display, Some(Duration::from_millis(45)))
            .unwrap();
        std::thread::sleep(Duration::from_millis(15));
        assert_eq!(
            controller.applied_flags.load(Ordering::SeqCst),
            ES_CONTINUOUS | ES_SYSTEM_REQUIRED | ES_DISPLAY_REQUIRED
        );

        std::thread::sleep(Duration::from_millis(60));
        assert!(controller.snapshot().unwrap().is_none());
        assert_eq!(
            controller.applied_flags.load(Ordering::SeqCst),
            ES_CONTINUOUS
        );
        let _ = controller.sender.send(WorkerSignal::Stop);
    }

    #[test]
    fn query_renders_default_items_and_custom_duration() {
        let controller =
            Controller::with_refresh(None, "caffeine".into(), Duration::from_millis(50));
        let default_items = query(&controller, "", 10).unwrap();
        assert_eq!(default_items.len(), 2);
        assert_eq!(default_items[1]["action"]["id"], "on:system:0");
        assert_eq!(query(&controller, "", 1).unwrap().len(), 1);
        assert!(default_items[0]["title"]
            .as_str()
            .unwrap()
            .contains("开启防休眠"));
        assert_eq!(default_items[0]["action"]["id"], "on:display:0");
        assert_eq!(default_items[0]["icon"], "Images/display.ico");

        let custom_items = query(&controller, "45m", 10).unwrap();
        assert_eq!(custom_items.len(), 1);
        assert_eq!(custom_items[0]["action"]["id"], "on:display:45");
        assert_eq!(custom_items[0]["icon"], "Images/display.ico");

        execute(&controller, "on:display:30").unwrap();
        let active_items = query(&controller, "", 10).unwrap();
        assert_eq!(active_items.len(), 2);
        assert_eq!(active_items[1]["action"]["id"], "switch_mode");
        assert_eq!(query(&controller, "off", 10).unwrap().len(), 1);
        assert!(query(&controller, "", 0).unwrap().is_empty());
        execute(&controller, "on:system:30").unwrap();
        let active_custom = query(&controller, "45m", 10).unwrap();
        assert_eq!(active_custom.len(), 1);
        assert_eq!(active_custom[0]["action"]["id"], "on:system:45");
        assert!(active_items[0]["title"]
            .as_str()
            .unwrap()
            .contains("关闭防休眠"));
        assert_eq!(active_items[0]["action"]["id"], "off");
        assert_eq!(active_items[0]["icon"], "Images/off.ico");
        let _ = controller.sender.send(WorkerSignal::Stop);
    }

    #[test]
    fn icons_are_valid_64x64_rgba_ico_files() {
        let images_dir = PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("plugins/caffeine/Images");
        fs::create_dir_all(&images_dir).unwrap();
        for (name, kind) in [
            ("display.ico", tray::IconKind::Display),
            ("system.ico", tray::IconKind::System),
            ("timer.ico", tray::IconKind::Timer),
            ("off.ico", tray::IconKind::Off),
        ] {
            let path = images_dir.join(name);
            let bytes = tray::encode_ico(kind, 64);
            if fs::read(&path).ok().as_ref() != Some(&bytes) {
                fs::write(&path, &bytes).unwrap();
            }
            assert_eq!(&bytes[0..6], &[0, 0, 1, 0, 1, 0]);
            assert_eq!(bytes[6], 64);
            assert_eq!(bytes[7], 64);
            assert!(path.is_file());
        }
    }

    #[test]
    fn on_default_and_switch_mode_preserve_timer_and_update_status() {
        let dir = PathBuf::from(env!("CARGO_MANIFEST_DIR"))
            .join(format!("artifacts/caffeine-test-{}", std::process::id()));
        fs::create_dir_all(&dir).unwrap();
        let settings_path = dir.join("settings.toml");
        fs::write(
            &settings_path,
            "[plugins.caffeine]\nkeep_display_on = false\ndefault_duration_mins = 15\n",
        )
        .unwrap();

        let controller = Controller::with_refresh(
            Some(settings_path.clone()),
            "caffeine".into(),
            Duration::from_millis(30),
        );

        // "on:default" uses settings: System mode (2) and 15 mins (900s)
        execute(&controller, "on:default").unwrap();
        let s1 = controller.snapshot().unwrap().unwrap();
        assert_eq!(s1.mode, PowerMode::System);
        assert_eq!(s1.duration, Some(Duration::from_secs(900)));
        assert!(s1.deadline.is_some());
        assert_eq!(LAST_NOTIFY.lock().unwrap().0, 2);
        assert!(LAST_NOTIFY.lock().unwrap().1 > 800);

        // "switch_mode" toggles System -> Display while preserving duration and deadline
        let original_deadline = s1.deadline;
        execute(&controller, "switch_mode").unwrap();
        let s2 = controller.snapshot().unwrap().unwrap();
        assert_eq!(s2.mode, PowerMode::Display);
        assert_eq!(s2.duration, Some(Duration::from_secs(900)));
        assert_eq!(s2.deadline, original_deadline);
        assert_eq!(LAST_NOTIFY.lock().unwrap().0, 1);

        // When off, "switch_mode" activates with default mode from config
        execute(&controller, "off").unwrap();
        assert_eq!(*LAST_NOTIFY.lock().unwrap(), (0, 0));
        execute(&controller, "switch_mode").unwrap();
        let s3 = controller.snapshot().unwrap().unwrap();
        assert_eq!(s3.mode, PowerMode::System);

        // Test runtime persistence across reload with matching host_pid
        TEST_HOST_PID.store(4242, Ordering::SeqCst);
        execute(&controller, "on:display:30").unwrap();
        let _ = controller.sender.send(WorkerSignal::Stop);

        let restored = initialize(&json!({
            "settings_path": settings_path,
            "config_key": "caffeine"
        }))
        .unwrap();
        let s_restored = restored.snapshot().unwrap().unwrap();
        assert_eq!(s_restored.mode, PowerMode::Display);
        assert_eq!(s_restored.duration, Some(Duration::from_secs(1800)));
        assert!(s_restored.deadline.is_some());
        let _ = restored.sender.send(WorkerSignal::Stop);
        TEST_HOST_PID.store(0, Ordering::SeqCst);
        let _ = fs::remove_dir_all(&dir);
    }
}
