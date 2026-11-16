use crate::{
    config::{self, History, Settings},
    hotkey,
    list::Viewport,
    providers::{self, Groups, Source, Update},
    search::{self, Action, Entry},
};
use std::{
    collections::{HashMap, HashSet},
    mem::{size_of, zeroed},
    ptr::{null, null_mut},
    sync::{
        atomic::{AtomicU64, Ordering},
        mpsc::{self, Receiver, Sender},
        Arc,
    },
    time::{Instant, SystemTime, UNIX_EPOCH},
};
use windows_sys::Win32::{
    Foundation::*,
    Graphics::{Dwm::*, Gdi::*},
    System::{Com::*, DataExchange::*, LibraryLoader::*, Memory::*, Registry::*, Threading::*},
    UI::{Controls::*, HiDpi::*, Input::KeyboardAndMouse::*, Shell::*, WindowsAndMessaging::*},
};

mod bootstrap;
mod messages;
pub use bootstrap::run;
use messages::window_proc;
mod accessibility;
mod animation;
mod backdrop;
mod buffer;
mod config_watch;
mod drag_image;
mod dragdrop;
pub mod editor;
mod execution;
pub mod gfx;
mod icons;
mod input;
mod item_menu;
mod launch;
mod layout;
mod paste;
mod picture_viewer;
mod presentation;
mod plugin_selection;
mod query;
mod render;
mod rows;
mod scope;
mod scope_keys;
mod startup;
mod text_panel;
mod text_search;
mod theme;
mod tooltip;
mod tray;
mod tray_popup;

pub(super) const STATUS_CAPSULE_MAGIC: usize = 0x4D4C_5343; // 'MLSC'

#[derive(Clone, Debug, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
pub(super) struct StatusCapsule {
    pub(super) plugin_id: String,
    pub(super) text: String,
    pub(super) glyph: Option<String>,
    pub(super) badge_color: u32,
    pub(super) hint_normal: String,
    pub(super) hint_scoped: String,
    pub(super) click_action: String,
    pub(super) context_action: String,
}

#[derive(Clone, Debug, serde::Deserialize)]
pub(super) struct StatusCapsulePayload {
    pub(super) plugin_id: String,
    pub(super) text: String,
    #[serde(default)]
    pub(super) glyph: Option<String>,
    #[serde(default)]
    pub(super) badge_color: Option<u32>,
    #[serde(default)]
    pub(super) hint_normal: Option<String>,
    #[serde(default)]
    pub(super) hint_scoped: Option<String>,
    #[serde(default)]
    pub(super) click_action: Option<String>,
    #[serde(default)]
    pub(super) context_action: Option<String>,
}

pub(super) fn parse_status_capsule(
    bytes: &[u8],
    accent: u32,
) -> Result<Option<StatusCapsule>, String> {
    if bytes.is_empty() {
        return Ok(None);
    }
    #[derive(serde::Deserialize)]
    struct CapsuleWrapper {
        capsule: Option<StatusCapsulePayload>,
    }
    let payload = if let Ok(payload) = serde_json::from_slice::<StatusCapsulePayload>(bytes) {
        payload
    } else if let Ok(wrapper) = serde_json::from_slice::<CapsuleWrapper>(bytes) {
        match wrapper.capsule {
            Some(payload) => payload,
            None => return Ok(None),
        }
    } else {
        return Err("无效胶囊载荷".into());
    };
    if payload.text.trim().is_empty() || payload.plugin_id.trim().is_empty() {
        return Ok(None);
    }
    if payload.text.len() > 64 || payload.plugin_id.len() > 64 {
        return Err("胶囊文本或插件ID超长".into());
    }
    let glyph = payload.glyph.filter(|g| !g.is_empty() && g.chars().count() <= 2);
    Ok(Some(StatusCapsule {
        plugin_id: payload.plugin_id,
        text: payload.text,
        glyph,
        badge_color: payload.badge_color.unwrap_or(accent),
        hint_normal: payload.hint_normal.unwrap_or_default(),
        hint_scoped: payload.hint_scoped.unwrap_or_default(),
        click_action: payload.click_action.unwrap_or_default(),
        context_action: payload.context_action.unwrap_or_default(),
    }))
}

const CLASS_NAME: &str = "MiniLauncher.Native.Window";
const HOTKEY: i32 = 1;
const TRAY: u32 = WM_APP + 1;
const EVENT: u32 = WM_APP + 2;
const SHOW: u32 = WM_APP + 3;
const RETRY_PANEL: u32 = WM_APP + 19;
const START_DRAG: u32 = WM_APP + 16;
const ROW_TOP: i32 = 68;
const ROW_HEIGHT: i32 = 60;
const WIDTH: i32 = config::DEFAULT_WINDOW_WIDTH;
const IDLE_HEIGHT: i32 = 58;
const FOOTER_HEIGHT: i32 = 36;
const MAX_ICONS: usize = 128;

fn wide(text: &str) -> Vec<u16> {
    text.encode_utf16().chain(Some(0)).collect()
}
fn launch_program(executable: &str, args: &[String]) -> Result<(), String> {
    use std::os::windows::process::CommandExt;
    use std::process::{Command, Stdio};
    Command::new(executable)
        .args(args)
        .creation_flags(CREATE_NO_WINDOW)
        .stdin(Stdio::null())
        .stdout(Stdio::null())
        .stderr(Stdio::null())
        .spawn()
        .map(|_| ())
        .map_err(|error| format!("无法启动 {executable}：{error}"))
}
fn color(r: u8, g: u8, b: u8) -> u32 {
    r as u32 | (g as u32) << 8 | (b as u32) << 16
}

#[derive(Clone, Copy)]
pub struct Palette {
    pub background: u32,
    pub surface: u32,
    pub text: u32,
    pub secondary: u32,
    pub hint: u32,
    pub border: u32,
    pub selection: u32,
    pub hover: u32,
    pub accent: u32,
}

impl Palette {
    fn for_backdrop(dark: bool, accent: u32, backdrop: &str) -> Self {
        let mut palette = Self::new(dark, accent);
        if backdrop == "acrylic" {
            // A more visible backdrop needs stronger contrast for small text.
            palette.secondary = if dark {
                color(214, 214, 218)
            } else {
                color(48, 48, 54)
            };
            palette.hint = theme::mix(palette.background, palette.text, if dark { 64 } else { 76 });
        }
        palette
    }
    fn new(dark: bool, accent: u32) -> Self {
        if dark {
            Self {
                background: color(28, 28, 30),
                surface: color(38, 38, 41),
                text: color(247, 247, 247),
                secondary: color(190, 190, 194),
                hint: color(155, 155, 156),
                border: color(48, 48, 52),
                selection: theme::mix(color(28, 28, 30), accent, 20),
                hover: color(36, 36, 40),
                accent,
            }
        } else {
            Self {
                background: color(250, 250, 252),
                surface: color(240, 240, 244),
                text: color(15, 15, 17),
                secondary: color(56, 56, 62),
                hint: color(82, 82, 88),
                border: color(225, 225, 230),
                selection: theme::mix(color(250, 250, 252), accent, 12),
                hover: color(233, 233, 233),
                accent,
            }
        }
    }
}

enum UiEvent {
    PluginSelection {
        request: u64,
        plugin: String,
        result: Result<String, String>,
    },
    SearchText {
        request: u64,
        generation: u64,
        result: Result<String, String>,
    },
    DragPrepared {
        id: u64,
        result: Result<dragdrop::Prepared, String>,
    },
    Preview {
        preview: search::ImagePreview,
        dpi: u32,
        image: Option<Arc<mini_launcher::clipboard_image::Image>>,
    },
    Settings(Result<Box<config_watch::Prepared>, String>),
    Provider(Update),
    PluginExecuted {
        id: u64,
        result: Result<Option<Action>, String>,
    },
    Launched {
        id: u64,
        result: Result<(), String>,
    },
    Indexed(Vec<Entry>),
    Results {
        generation: u64,
        entries: Vec<Entry>,
    },
    Icon {
        path: String,
        size: i32,
        handle: usize,
    },
    Error(String),
}

struct Execution {
    generation: u64,
    key: String,
    title: String,
    history_key: Option<String>,
    paste_target: Option<paste::Target>,
    paste_epoch: u64,
}

struct SearchRequest {
    generation: u64,
    text: String,
    index: Arc<Vec<Entry>>,
    history: Arc<History>,
    limit: usize,
    file_only: bool,
    web_entry: Option<Entry>,
}

struct State {
    accessibility: accessibility::Tree,
    capsule: Option<StatusCapsule>,
    capsule_hovered: bool,
    panels: text_panel::Panels,
    execution: execution::Model,
    query: query::Model,
    text_search_request: u64,
    image_viewer: HWND,
    consume_preview_space: bool,
    drag_press: Option<(POINT, Entry)>,
    drag_request: u64,
    drag_pending: Option<u64>,
    prepared_drag: Option<dragdrop::Prepared>,
    dragging: bool,
    drag_release_consumed: bool,
    drag_cancel: Option<Arc<std::sync::atomic::AtomicBool>>,
    paste_target: Option<paste::Target>,
    paste_epoch: Arc<AtomicU64>,
    previews: HashMap<search::ImagePreview, Option<rows::Thumbnail>>,
    pending_previews: HashSet<(search::ImagePreview, u32)>,
    preview_sender: Sender<rows::PreviewRequest>,
    hwnd: HWND,
    settings: Settings,
    index: Arc<Vec<Entry>>,
    latest_generation: Arc<AtomicU64>,
    everything: crate::everything::Client,
    plugins: crate::plugins::Client,
    history: Arc<History>,
    viewport: Viewport,
    row_measurements: rows::Measurements,
    hovered: Option<usize>,
    mouse_position: Option<(i32, i32)>,
    scroll_drag: Option<i32>,
    item_menu: Option<item_menu::Menu>,
    dismissing_menu: Option<item_menu::Menu>,
    tooltip: Option<tooltip::Tooltip>,
    result_navigation: bool,
    scope: scope::Scope,
    plugin_scopes: Vec<(String, String)>,
    plugin_routing: crate::plugins::QueryRouting,
    direct_display: bool,
    scope_hovered: bool,
    scope_keys: Option<scope_keys::Hook>,
    alt_preview: scope_keys::AltPreview,
    visible_rows: usize,
    dpi: u32,
    positioning: bool,
    dark: bool,
    backdrop_active: bool,
    appearance: Option<theme::Appearance>,
    palette: Palette,
    pub(super) text_engine: gfx::TextEngine,
    pub(super) renderer: std::rc::Rc<std::cell::RefCell<gfx::WindowRenderer>>,
    render_device_id: u64,
    pub(super) editor: editor::Editor,
    pub(super) image_renderer: gfx::ImageRenderer,
    pub(super) d2d_icons: HashMap<String, windows::Win32::Graphics::Direct2D::ID2D1Bitmap1>,
    events: Receiver<UiEvent>,
    event_sender: Sender<UiEvent>,
    search_sender: Sender<SearchRequest>,
    launch_sender: Sender<(u64, launch::Task)>,
    icon_sender: Sender<(String, i32)>,
    history_sender: Sender<Arc<History>>,
    icons: HashMap<String, usize>,
    pending_icons: HashSet<String>,
    tray: tray::Icon,
    taskbar_created: u32,
    status: Option<String>,
    config_error: Option<String>,
    config_watch: Option<config_watch::Watcher>,
    hotkey_id: i32,
    hotkey_registered: bool,
    plugin_hotkeys: hotkey::Registry,
    plugin_shortcuts: Vec<hotkey::PluginShortcut>,
    selection_request: u64,
    opening: Option<animation::Tween>,
    closing: Option<animation::Tween>,
    selection: animation::Tween,
    selection_first: usize,
    height_animation: Option<animation::Tween>,
    frame_clock: animation::FrameClock,
}

impl State {
    fn width(&self) -> i32 {
        self.settings.window_width
    }
    fn px(&self, value: i32) -> i32 {
        layout::px(value, self.dpi)
    }
    fn logical(&self, value: i32) -> i32 {
        (value as f64 * 96.0 / self.dpi as f64).round() as i32
    }
    fn input_extra(&self) -> i32 {
        (self.editor.visible_lines().saturating_sub(1) as i32) * self.editor.line_height()
    }
    fn row_top(&self) -> i32 {
        ROW_TOP + self.input_extra()
    }
    fn height(&self) -> i32 {
        if !self.query.query_active {
            return IDLE_HEIGHT
                + self.input_extra()
                + if self.status.is_some() {
                    FOOTER_HEIGHT
                } else {
                    0
                };
        }
        let content = if self.query.results.is_empty() {
            2.min(self.visible_rows) as i32 * ROW_HEIGHT
        } else if self.viewport.scrollable() {
            self.viewport.capacity()
        } else {
            self.viewport.content_height()
        };
        self.row_top() + content + 8 + FOOTER_HEIGHT
    }
    fn footer_visible(&self) -> bool {
        self.query.query_active || self.status.is_some()
    }
    unsafe fn invalidate(&self) {
        InvalidateRect(self.hwnd, null(), 0);
    }
    unsafe fn input(&self) -> String {
        self.editor.text().to_owned()
    }
    unsafe fn remember_paste_target(&mut self) {
        if GetForegroundWindow() != self.hwnd {
            self.paste_target = paste::Target::capture(self.hwnd);
        }
    }
    pub(super) fn minimum_panel_rows(&self) -> usize {
        self.query
            .results
            .iter()
            .filter_map(|e| e.text_panel.as_ref())
            .map(|panel| {
                (text_panel::Geometry::minimum_height(self.width(), panel) as usize)
                    .div_ceil(ROW_HEIGHT as usize)
            })
            .max()
            .unwrap_or(1)
    }
    unsafe fn center(&mut self) {
        let mut cursor: POINT = zeroed();
        GetCursorPos(&mut cursor);
        let monitor = MonitorFromPoint(cursor, MONITOR_DEFAULTTONEAREST);
        self.fit_monitor(monitor, None, None);
    }
    unsafe fn fit_current_monitor(&mut self) {
        let mut rect: RECT = zeroed();
        if GetWindowRect(self.hwnd, &mut rect) == 0 {
            return;
        }
        self.fit_monitor(
            MonitorFromWindow(self.hwnd, MONITOR_DEFAULTTONEAREST),
            Some(POINT {
                x: rect.left,
                y: rect.top,
            }),
            None,
        );
    }
    pub(super) unsafe fn sync_monitor_rows(&mut self) {
        if self.positioning {
            return;
        }
        if IsWindowVisible(self.hwnd) == 0 {
            self.fit_current_monitor();
            return;
        }
        let mut rect: RECT = zeroed();
        if GetWindowRect(self.hwnd, &mut rect) == 0 {
            return;
        }
        let monitor = MonitorFromWindow(self.hwnd, MONITOR_DEFAULTTONEAREST);
        let mut info: MONITORINFO = zeroed();
        info.cbSize = size_of::<MONITORINFO>() as u32;
        if GetMonitorInfoW(monitor, &mut info) == 0 {
            return;
        }
        let mut x_dpi = GetDpiForWindow(self.hwnd).max(96);
        let mut y_dpi = x_dpi;
        let _ = GetDpiForMonitor(monitor, MDT_EFFECTIVE_DPI, &mut x_dpi, &mut y_dpi);
        let minimum_rows = self.minimum_panel_rows();
        info.rcWork.bottom -= layout::px(self.input_extra(), x_dpi);
        let layout = layout::Layout::with_minimum_rows(
            info.rcWork,
            x_dpi,
            self.width(),
            self.settings.max_results,
            minimum_rows,
            Some(POINT {
                x: rect.left,
                y: rect.top,
            }),
        );
        if layout.dpi != self.dpi {
            self.fit_monitor(
                monitor,
                Some(POINT {
                    x: rect.left,
                    y: rect.top,
                }),
                Some(layout.dpi),
            );
            return;
        }
        self.query.panel_rows = minimum_rows;
        self.visible_rows = layout.rows;
        self.viewport.set_visible(self.visible_rows);
        self.reflow_rows();
        if self.opening.is_none() && (rect.left != layout.x || rect.top != layout.y) {
            SetWindowPos(
                self.hwnd,
                null_mut(),
                layout.x,
                layout.y,
                0,
                0,
                SWP_NOSIZE | SWP_NOZORDER | SWP_NOACTIVATE,
            );
        }
    }
    unsafe fn fit_monitor(&mut self, monitor: HMONITOR, position: Option<POINT>, dpi: Option<u32>) {
        if self.positioning {
            return;
        }
        let mut info: MONITORINFO = zeroed();
        info.cbSize = size_of::<MONITORINFO>() as u32;
        if GetMonitorInfoW(monitor, &mut info) == 0 {
            return;
        }
        let mut x_dpi = GetDpiForWindow(self.hwnd).max(96);
        let mut y_dpi = x_dpi;
        if let Some(dpi) = dpi {
            x_dpi = dpi;
        } else {
            let _ = GetDpiForMonitor(monitor, MDT_EFFECTIVE_DPI, &mut x_dpi, &mut y_dpi);
        }
        let minimum_rows = self.minimum_panel_rows();
        self.query.panel_rows = minimum_rows;
        info.rcWork.bottom -= layout::px(self.input_extra(), x_dpi);
        let layout = layout::Layout::with_minimum_rows(
            info.rcWork,
            x_dpi,
            self.width(),
            self.settings.max_results,
            minimum_rows,
            position,
        );
        // SetWindowPos can synchronously deliver WM_DPICHANGED. The outer layout
        // owns this operation so a nested message must not undo its fit scale.
        self.positioning = true;
        self.close_item_menu();
        self.finish_animations();
        self.update_dpi(layout.dpi);
        self.visible_rows = layout.rows;
        self.viewport.set_visible(self.visible_rows);
        self.reflow_rows();
        self.selection = animation::Tween::fixed(self.selection_top());
        self.selection_first = self.viewport.first;
        self.hovered = None;
        self.scroll_drag = None;
        SetWindowPos(
            self.hwnd,
            null_mut(),
            layout.x,
            layout.y,
            self.px(self.width()),
            self.px(self.height()),
            SWP_NOZORDER | SWP_NOACTIVATE,
        );
        // Apply child geometry/fonts after USER32 completes per-monitor scaling.
        self.layout_input();
        self.positioning = false;
        self.queue_visible_icons();
        self.invalidate();
    }
    unsafe fn show(&mut self) {
        if self.closing.is_some() {
            self.finish_animations();
        }
        self.close_image_preview();
        if let Some(cancel) = &self.drag_cancel {
            cancel.store(true, Ordering::Relaxed);
        }
        self.cancel_pending_drag();
        self.paste_epoch.fetch_add(1, Ordering::Relaxed);
        self.remember_paste_target();
        let was_visible = IsWindowVisible(self.hwnd) != 0;
        self.center();
        self.request_search();
        // Showing an HWND exposes its DWM material before WM_PAINT can supply
        // the text/icons. Keep that empty surface off-screen until both the
        // composition scene has been presented and DWM has consumed its pixels.
        let cloak: i32 = 1;
        let preparing = !was_visible
            && DwmSetWindowAttribute(self.hwnd, DWMWA_CLOAK as u32, &cloak as *const _ as _, 4)
                >= 0;
        if preparing {
            ShowWindow(self.hwnd, SW_SHOWNOACTIVATE);
            RedrawWindow(
                self.hwnd,
                null(),
                null_mut(),
                RDW_INVALIDATE | RDW_UPDATENOW | RDW_ALLCHILDREN,
            );
            GdiFlush();
            DwmFlush();
        }
        // Start the animation after first-frame preparation so it retains its
        // full duration even when the compositor takes time to become ready.
        if !was_visible && animations_enabled() {
            let now = Instant::now();
            let mut rect: RECT = zeroed();
            GetWindowRect(self.hwnd, &mut rect);
            self.opening = Some(animation::Tween::smooth(
                rect.top + self.px(4),
                rect.top,
                now,
                200,
            ));
            self.animate();
        }
        if preparing {
            let cloak: i32 = 0;
            DwmSetWindowAttribute(self.hwnd, DWMWA_CLOAK as u32, &cloak as *const _ as _, 4);
        }
        ShowWindow(self.hwnd, SW_SHOW);
        let mut cursor: POINT = zeroed();
        if GetCursorPos(&mut cursor) != 0 {
            self.mouse_position = Some((cursor.x, cursor.y));
        }
        SetForegroundWindow(self.hwnd);
        self.focus_input();
        self.editor.select_all();
        self.activate_scope_keys();
        if GetAsyncKeyState(VK_MENU as i32) < 0 {
            self.cancel_alt_preview();
        } else {
            self.release_alt_preview();
        }
        // WM_PAINT normally waits until the message queue is idle. Present the
        // complete first frame before yielding to DWM.
        RedrawWindow(
            self.hwnd,
            null(),
            null_mut(),
            RDW_INVALIDATE | RDW_UPDATENOW,
        );
    }
    unsafe fn hide(&mut self) {
        self.cancel_alt_preview();
        // Deactivation can arrive through both WM_ACTIVATE and WM_ACTIVATEAPP.
        // Keep the original deadline when the same dismissal is requested again.
        if self.closing.is_some() {
            return;
        }
        // Dismiss from the currently displayed geometry. Finishing an opening
        // or height tween here would visibly snap to its endpoint first.
        self.frame_clock.stop();
        self.opening = None;
        self.height_animation = None;
        if IsWindowVisible(self.hwnd) != 0 && animations_enabled() {
            let mut rect: RECT = zeroed();
            GetWindowRect(self.hwnd, &mut rect);
            self.closing = Some(animation::Tween::exit(
                rect.top,
                rect.top + self.px(12),
                Instant::now(),
                100,
            ));
        }
        self.query.generation += 1;
        self.latest_generation
            .store(self.query.generation, Ordering::Relaxed);
        self.query.query_active = false;
        self.query.busy_since = None;
        KillTimer(self.hwnd, query::PROGRESS_TIMER);
        self.cancel_pending_drag();
        self.scope_keys = None;
        self.execution.pending_execute = None;
        self.close_image_preview();
        self.dismiss_menus_with_host();
        if self.closing.is_some() {
            self.animate();
        } else {
            self.finish_hide();
        }
    }
    unsafe fn hide_immediately(&mut self) {
        self.hide();
        self.finish_animations();
    }
    unsafe fn finish_hide(&mut self) {
        if let Some(tooltip) = &self.tooltip { tooltip.hide(); }
        self.frame_clock.stop();
        self.finish_menu_animations();
        // Keep closing set through SW_HIDE's synchronous activation messages.
        ShowWindow(self.hwnd, SW_HIDE);
        self.closing = None;
        // Releasing results, panel state and GPU resources can be expensive.
        // Preserve the composed scene throughout the exit and clean up offscreen.
        self.query.context = None;
        self.query.identity = None;
        self.query.clear_results();
        self.query.results.shrink_to_fit();
        self.query.groups = Groups::default();
        self.viewport.reset(0, self.visible_rows);
        self.accessibility.hide();
        self.panels.clear(self.hwnd);
        self.scope = scope::Scope::All;
        self.previews.clear();
        self.d2d_icons.clear();
        self.row_measurements.clear();
        self.renderer.borrow_mut().suspend();
    }
    fn selection_top(&self) -> i32 {
        self.row_top() + self.viewport.row_top(self.viewport.selected)
    }
    unsafe fn finish_animations(&mut self) {
        self.frame_clock.stop();
        self.finish_menu_animations();
        if self.closing.is_some() {
            self.finish_hide();
        }
        if self.height_animation.take().is_some() {
            self.resize_height(self.height());
        }
        if let Some(opening) = self.opening.take() {
            let mut rect: RECT = zeroed();
            GetWindowRect(self.hwnd, &mut rect);
            SetWindowPos(
                self.hwnd,
                null_mut(),
                rect.left,
                opening.target(),
                0,
                0,
                SWP_NOSIZE | SWP_NOZORDER | SWP_NOACTIVATE,
            );
        }
        self.selection = animation::Tween::fixed(self.selection_top());
        self.selection_first = self.viewport.first;
        self.invalidate();
    }
    unsafe fn animate(&mut self) {
        let now = Instant::now();
        self.animate_menus(now);
        if let Some(closing) = &self.closing {
            let mut rect: RECT = zeroed();
            GetWindowRect(self.hwnd, &mut rect);
            SetWindowPos(
                self.hwnd,
                null_mut(),
                rect.left,
                closing.value(now),
                0,
                0,
                SWP_NOSIZE | SWP_NOZORDER | SWP_NOACTIVATE,
            );
            if !closing.active(now) || !self.frame_clock.start() {
                self.finish_hide();
            }
            return;
        }
        // Include the clock so the final selection frame is painted even when
        // its tween has just expired, before stopping the timer below.
        let active = self.frame_clock.running()
            || self.query.progress_phase(now).is_some()
            || self.opening.is_some()
            || self.selection.active(now)
            || self.height_animation.is_some();
        if let Some(height) = &self.height_animation {
            self.resize_height(height.value(now));
            if !height.active(now) {
                self.height_animation = None;
            }
        }
        if let Some(opening) = &self.opening {
            let mut rect: RECT = zeroed();
            GetWindowRect(self.hwnd, &mut rect);
            SetWindowPos(
                self.hwnd,
                null_mut(),
                rect.left,
                opening.value(now),
                0,
                0,
                SWP_NOSIZE | SWP_NOZORDER | SWP_NOACTIVATE,
            );
            if !opening.active(now) {
                self.opening = None;
            }
        }
        if active {
            self.invalidate();
        }
        if self.opening.is_some()
            || self.menu_animating()
            || self.selection.active(now)
            || self.height_animation.is_some()
            || self.query.progress_phase(now).is_some()
        {
            if !self.frame_clock.start() {
                self.finish_animations();
            }
        } else {
            self.frame_clock.stop();
        }
    }
    unsafe fn resize_height(&self, height: i32) {
        let mut client: RECT = zeroed();
        GetClientRect(self.hwnd, &mut client);
        if client.bottom == self.px(height) && client.right == self.px(self.width()) {
            return;
        }
        SetWindowPos(
            self.hwnd,
            null_mut(),
            0,
            0,
            self.px(self.width()),
            self.px(height),
            SWP_NOMOVE | SWP_NOZORDER | SWP_NOACTIVATE,
        );
    }
    unsafe fn sync_height(&mut self) {
        if self.closing.is_some() {
            return;
        }
        let height = self.height();
        if self
            .height_animation
            .as_ref()
            .is_some_and(|tween| tween.target() == height)
        {
            return;
        }
        let current = self.displayed_height();
        if current == height {
            self.height_animation = None;
        } else if IsWindowVisible(self.hwnd) != 0 && animations_enabled() {
            self.height_animation =
                Some(animation::Tween::new(current, height, Instant::now(), 160));
        } else {
            self.height_animation = None;
            self.resize_height(height);
        }
    }
    unsafe fn displayed_height(&self) -> i32 {
        let mut client: RECT = zeroed();
        GetClientRect(self.hwnd, &mut client);
        self.logical(client.bottom)
    }
    unsafe fn update_dpi(&mut self, dpi: u32) {
        if self.dpi != dpi {
            self.dpi = dpi;
            for (_, icon) in self.icons.drain() {
                if icon != 0 {
                    DestroyIcon(icon as HICON);
                }
            }
            self.pending_icons.clear();
            self.d2d_icons.clear();
            self.previews.clear();
            self.row_measurements.clear();
            self.editor.set_dpi(dpi);
            // The next paint updates the target DPI and handles any device error.
        }
    }
    pub(super) unsafe fn focus_input(&mut self) {
        if self.closing.is_some() {
            return;
        }
        self.panels.blur();
        SetFocus(self.hwnd);
        self.editor.set_focused(true);
        self.invalidate();
    }
    pub(super) fn editor_bounds(&self) -> windows::Win32::Graphics::Direct2D::Common::D2D_RECT_F {
        let right_limit = self
            .capsule_rect()
            .map_or(self.width() - 16, |[left, ..]| left - 8);
        let left = self.scope_right() + 16;
        let width = (right_limit - left).max(80) as f32;
        let height = (28 + self.input_extra()) as f32;
        windows::Win32::Graphics::Direct2D::Common::D2D_RECT_F {
            left: left as f32,
            top: 15.0,
            right: left as f32 + width,
            bottom: 15.0 + height,
        }
    }
    unsafe fn layout_input(&mut self) {
        let bounds = self.editor_bounds();
        let width = bounds.right - bounds.left;
        let height = bounds.bottom - bounds.top;
        self.editor.set_rect(bounds.left, bounds.top, width, height);
    }
    pub(super) fn capsule_rect(&self) -> Option<[i32; 4]> {
        let capsule = self.capsule.as_ref()?;
        let label = &capsule.text;
        let text_width = self.text_engine.measure_width(label, gfx::FontRole::Small)
            .unwrap_or(0.0).ceil() as i32;
        let icon_space = if capsule.glyph.is_some() { 38 } else { 20 };
        let width = (icon_space + text_width).clamp(112, 220);
        let right = self.width() - 16;
        let left = right - width;
        Some([left, 15, right, 43])
    }
    pub(super) fn execute_capsule_action(&mut self, action: &str) {
        if let Some(capsule) = &self.capsule {
            let _ = self.plugins.send(crate::plugins::Request::Execute {
                id: 0,
                plugin: capsule.plugin_id.clone(),
                action: action.into(),
            });
        }
    }
    pub(super) fn apply_status_capsule(&mut self, next: Option<StatusCapsule>) {
        if self.capsule != next {
            let active_changed = self.capsule.is_some() != next.is_some();
            let text_changed = self.capsule.as_ref().map(|c| &c.text)
                != next.as_ref().map(|c| &c.text);
            let badge_changed = self.capsule.as_ref().map(|c| c.badge_color)
                != next.as_ref().map(|c| c.badge_color);
            let target_plugin = next
                .as_ref()
                .map(|c| c.plugin_id.clone())
                .or_else(|| self.capsule.as_ref().map(|c| c.plugin_id.clone()));
            self.capsule = next;
            if self.capsule.is_none() {
                self.capsule_hovered = false;
            }
            if active_changed || text_changed || badge_changed {
                unsafe {
                    self.layout_input();
                }
            }
            unsafe {
                if IsWindowVisible(self.hwnd) != 0 {
                    let should_refresh = target_plugin.as_ref().is_some_and(|p| {
                        self.scope == scope::Scope::Plugin(p.clone())
                    });
                    if should_refresh && (active_changed || text_changed || badge_changed) {
                        self.request_search();
                    }
                    if active_changed || text_changed || badge_changed {
                        self.invalidate();
                    }
                }
            }
        }
    }
    unsafe fn update_theme(&mut self, force_frame: bool) {
        let next = theme::Appearance {
            dark: is_dark(&self.settings.theme),
            accent: theme::accent(),
            backdrop: self.settings.backdrop.clone(),
        };
        let changes = next.changes(self.appearance.as_ref(), force_frame);
        if !changes.colors && !changes.frame {
            return;
        }
        self.dark = next.dark;
        self.palette = Palette::for_backdrop(next.dark, next.accent, &next.backdrop);
        self.appearance = Some(next);
        if changes.frame {
            let dark: i32 = self.dark.into();
            let rounded: i32 = 2;
            // Our opening tween owns the transition. DWM's show/hide fade can
            // otherwise expose an unpainted backdrop while that tween is running.
            let disable_transitions: i32 = 1;
            DwmSetWindowAttribute(
                self.hwnd,
                DWMWA_TRANSITIONS_FORCEDISABLED as u32,
                &disable_transitions as *const _ as _,
                4,
            );
            DwmSetWindowAttribute(
                self.hwnd,
                DWMWA_USE_IMMERSIVE_DARK_MODE as u32,
                &dark as *const _ as _,
                4,
            );
            DwmSetWindowAttribute(
                self.hwnd,
                DWMWA_WINDOW_CORNER_PREFERENCE as u32,
                &rounded as *const _ as _,
                4,
            );
            let border = self.palette.border;
            DwmSetWindowAttribute(
                self.hwnd,
                DWMWA_BORDER_COLOR as u32,
                &border as *const _ as _,
                4,
            );
            self.backdrop_active = backdrop::apply(self.hwnd, &self.settings.backdrop);
        }
        if changes.fonts {
            match gfx::TextEngine::new(&self.text_engine.factory, self.dark) {
                Ok(engine) => {
                    self.editor.set_text_engine(&engine);
                    self.text_engine = engine;
                    self.panels.clear(self.hwnd);
                    self.row_measurements.clear();
                    self.reflow_rows();
                    self.layout_input();
                }
                Err(error) => {
                    self.appearance = None;
                    eprintln!("Text format update failed: {error}");
                }
            }
        }
        if let Some(menu) = &mut self.item_menu {
            menu.update_theme(
                self.dark,
                self.palette,
                &self.settings.backdrop,
                &self.text_engine,
            );
        }
        tray_popup::update_theme(
            self.hwnd,
            self.dark,
            self.palette,
            &self.settings.backdrop,
            &self.text_engine,
        );
        self.invalidate();
    }
    unsafe fn move_selection(&mut self, direction: i32) {
        self.result_navigation = true;
        self.viewport.move_selection(direction);
        self.viewport_changed();
    }
    fn queue_visible_icons(&mut self) {
        for entry in &self.query.results[self.viewport.range()] {
            if let Some(path) = &entry.icon_path {
                if !self.icons.contains_key(path)
                    && self.pending_icons.len() < 24
                    && self.pending_icons.insert(path.clone())
                {
                    let _ = self.icon_sender.send((path.clone(), self.px(32)));
                }
            }
        }
    }
    unsafe fn viewport_changed(&mut self) {
        if let Some(tooltip) = &self.tooltip { tooltip.hide(); }
        // Once the user navigates, asynchronous results should follow their
        // selected row instead of forcing the list back to the first result.
        self.query.preserve_selection = true;
        self.sync_accessibility(false);
        self.close_image_preview();
        let top = self.selection_top();
        if self.selection_first != self.viewport.first
            || IsWindowVisible(self.hwnd) == 0
            || !animations_enabled()
        {
            self.selection = animation::Tween::fixed(top);
        } else {
            self.selection.retarget(top, Instant::now());
        }
        self.selection_first = self.viewport.first;
        self.animate();
        self.hovered = None;
        self.queue_visible_icons();
        self.queue_visible_previews();
        self.invalidate();
    }
    unsafe fn hit_result(&self, x: i32, y: i32) -> Option<usize> {
        if !self.query.query_active
            || !(20..self.width() - 20).contains(&x)
            || y < self.row_top()
            || y >= self.displayed_height() - FOOTER_HEIGHT
        {
            return None;
        }
        self.viewport.hit(y - self.row_top())
    }
    fn scroll_track_height(&self) -> i32 {
        (if self.viewport.scrollable() {
            self.viewport.capacity()
        } else {
            self.viewport.content_height()
        } - 6)
            .max(0)
    }
    unsafe fn reload(&mut self) {
        let sender = self.event_sender.clone();
        let hwnd = self.hwnd as usize;
        std::thread::spawn(move || {
            send_event(
                &sender,
                hwnd,
                UiEvent::Settings(config_watch::Prepared::load(true).map(Box::new)),
            )
        });
    }
    unsafe fn reject_config(&mut self, error: String, show: bool) {
        self.config_error = Some(format!("配置未应用：{error}；继续使用上一次有效配置"));
        if show {
            self.show();
        }
        self.status = self.config_error.clone();
        self.sync_height();
        self.invalidate();
    }
    unsafe fn apply_settings(&mut self, prepared: config_watch::Prepared) {
        let config_watch::Prepared {
            settings,
            catalog,
            force,
        } = prepared;
        if settings == self.settings && !force {
            if self.config_error.is_some() {
                if let Err(error) = startup::sync(settings.start_at_login) {
                    self.reject_config(error, false);
                    return;
                }
            }
            self.config_error = None;
            self.status = self.query.groups.status();
            self.invalidate();
            return;
        }
        let hotkey_changed = settings.hotkey != self.settings.hotkey;
        let next_id = if self.hotkey_id == HOTKEY {
            HOTKEY + 1
        } else {
            HOTKEY
        };
        if hotkey_changed {
            // Register first; a conflict must leave the previous hotkey working.
            if !register_hotkey(self.hwnd, next_id, &settings.hotkey) {
                self.reject_config(
                    format!(
                        "快捷键 {} 注册失败（被占用或被系统保留），当前仍为 {}",
                        settings.hotkey, self.settings.hotkey
                    ),
                    force,
                );
                return;
            }
        }
        if settings.start_at_login != self.settings.start_at_login
            || force
            || self.config_error.is_some()
        {
            if let Err(error) = startup::sync(settings.start_at_login) {
                if hotkey_changed {
                    hotkey::unregister(self.hwnd, next_id);
                }
                self.reject_config(error, force);
                return;
            }
        }
        if let Err(error) = self
            .plugin_hotkeys
            .configure(catalog.hotkeys.clone(), &settings.hotkey)
        {
            if hotkey_changed {
                hotkey::unregister(self.hwnd, next_id);
            }
            self.reject_config(error, force);
            return;
        }
        if hotkey_changed {
            hotkey::unregister(self.hwnd, self.hotkey_id);
            self.hotkey_id = next_id;
        }
        let rescan = force
            || settings.search_paths != self.settings.search_paths
            || settings.aliases != self.settings.aliases;
        let reload_plugins = force || settings.plugins != self.settings.plugins;
        self.settings = settings;
        if reload_plugins {
            self.plugin_scopes = catalog.scopes;
            self.plugin_routing = catalog.routing;
            self.query.context = None;
            self.direct_display = catalog.direct_display;
            self.plugin_shortcuts = catalog.hotkeys;
        }
        self.configure_scope_keys();
        self.validate_scope();
        self.config_error = None;
        if reload_plugins {
            self.plugins.send(crate::plugins::Request::Reload(
                self.settings.plugins.clone(),
            ));
        }
        if hotkey_changed {
            self.tray.update_hotkey(&self.settings.hotkey);
        }
        self.update_theme(false);
        if rescan {
            self.start_index();
        }
        if IsWindowVisible(self.hwnd) != 0 {
            self.center();
            self.request_search();
        }
        self.invalidate();
    }
}

impl Drop for State {
    fn drop(&mut self) {
        if let Some(cancel) = &self.drag_cancel {
            cancel.store(true, Ordering::Relaxed);
        }
        self.scope_keys = None;
        unsafe {
            if self.hotkey_registered {
                hotkey::unregister(self.hwnd, self.hotkey_id);
            }
            for event in self.events.try_iter() {
                if let UiEvent::Icon { handle, .. } = event {
                    if handle != 0 {
                        DestroyIcon(handle as HICON);
                    }
                }
            }
            for icon in self.icons.values() {
                if *icon != 0 {
                    DestroyIcon(*icon as HICON);
                }
            }
        }
    }
}

unsafe fn animations_enabled() -> bool {
    let mut enabled: i32 = 0;
    SystemParametersInfoW(
        SPI_GETCLIENTAREAANIMATION,
        0,
        &mut enabled as *mut _ as _,
        0,
    ) != 0
        && enabled != 0
}

fn send_event(sender: &Sender<UiEvent>, hwnd: usize, event: UiEvent) {
    if sender.send(event).is_ok() {
        unsafe {
            PostMessageW(hwnd as HWND, EVENT, 0, 0);
        }
    }
}

unsafe fn register_hotkey(hwnd: HWND, id: i32, label: &str) -> bool {
    match hotkey::Hotkey::parse(label) {
        Ok(hotkey) => hotkey.register(hwnd, id),
        Err(_) => false,
    }
}

unsafe fn is_dark(theme: &str) -> bool {
    match theme {
        "dark" => true,
        "light" => false,
        _ => {
            let mut value = 1u32;
            let mut length = 4u32;
            RegGetValueW(
                HKEY_CURRENT_USER,
                wide("Software\\Microsoft\\Windows\\CurrentVersion\\Themes\\Personalize").as_ptr(),
                wide("AppsUseLightTheme").as_ptr(),
                RRF_RT_REG_DWORD,
                null_mut(),
                &mut value as *mut _ as _,
                &mut length,
            );
            value == 0
        }
    }
}

unsafe fn copy_to_clipboard(hwnd: HWND, text: &str) -> bool {
    let text = wide(text);
    let memory = GlobalAlloc(GMEM_MOVEABLE, text.len() * 2);
    if memory.is_null() {
        return false;
    }
    let destination = GlobalLock(memory);
    if destination.is_null() {
        GlobalFree(memory);
        return false;
    }
    std::ptr::copy_nonoverlapping(text.as_ptr(), destination as *mut u16, text.len());
    GlobalUnlock(memory);
    if OpenClipboard(hwnd) == 0 {
        GlobalFree(memory);
        return false;
    }
    let ok = EmptyClipboard() != 0 && !SetClipboardData(13, memory).is_null();
    if !ok {
        GlobalFree(memory);
    }
    CloseClipboard();
    ok
}

pub fn show_error(error: &str) {
    unsafe {
        MessageBoxW(
            null_mut(),
            wide(error).as_ptr(),
            wide("MiniLauncher").as_ptr(),
            MB_OK | MB_ICONERROR,
        );
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parse_status_capsule_parses_flat_payload() {
        let json = r#"{"plugin_id":"caffeine","text":"屏幕常亮 · 常驻","glyph":"","badge_color":16096779,"hint_normal":"左键管理","hint_scoped":"左键切换","click_action":"switch_mode","context_action":"off"}"#;
        let res = parse_status_capsule(json.as_bytes(), 0x123456).expect("parsed");
        assert!(res.is_some());
        let capsule = res.unwrap();
        assert_eq!(capsule.plugin_id, "caffeine");
        assert_eq!(capsule.text, "屏幕常亮 · 常驻");
        assert_eq!(capsule.glyph.as_deref(), Some("\u{E706}"));
        assert_eq!(capsule.badge_color, 16096779);
        assert_eq!(capsule.click_action, "switch_mode");
        assert_eq!(capsule.context_action, "off");
    }

    #[test]
    fn parse_status_capsule_parses_wrapped_and_empty() {
        assert_eq!(parse_status_capsule(b"", 0x123456).unwrap(), None);
        assert_eq!(parse_status_capsule(br#"{"capsule":null}"#, 0x123456).unwrap(), None);
        let wrapped = r#"{"capsule":{"plugin_id":"caffeine","text":"测试","badge_color":null}}"#;
        let res = parse_status_capsule(wrapped.as_bytes(), 0x123456).expect("parsed").unwrap();
        assert_eq!(res.plugin_id, "caffeine");
        assert_eq!(res.text, "测试");
        assert_eq!(res.badge_color, 0x123456);
    }
}

