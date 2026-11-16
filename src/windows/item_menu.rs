use super::*;

const CLASS: &str = "MiniLauncher.ItemMenu";
const MENU_WIDTH: i32 = 256;
const ROW: i32 = 36;
const PADDING: i32 = 6;

struct Item {
    command: u32,
    title: String,
    separator: bool,
    symbol: Option<&'static str>,
    shortcut: String,
    disabled: bool,
}

enum Target {
    Entry(Entry, bool),
    Scopes(Vec<scope::Scope>),
    Feedback(String),
}

pub(super) struct Menu {
    accessibility: accessibility::Tree,
    hwnd: HWND,
    target: Target,
    width: i32,
    dpi: u32,
    alt_select: bool,
    alt_changed: bool,
    items: Vec<Item>,
    selected: usize,
    first: usize,
    visible: usize,
    renderer: gfx::WindowRenderer,
    backdrop_active: bool,
    text_engine: gfx::TextEngine,
    mouse_position: Option<(i32, i32)>,
    motion: Option<animation::Tween>,
}

impl State {
    pub(super) unsafe fn dismiss_menus_with_host(&mut self) {
        self.close_item_menu();
        // Retarget even a menu already exiting, using its displayed position.
        if let (Some(menu), Some(closing)) = (&mut self.dismissing_menu, &self.closing) {
            let mut rect: RECT = zeroed();
            GetWindowRect(menu.hwnd, &mut rect);
            menu.motion = Some(closing.following_at(rect.top));
        }
    }

    pub(super) unsafe fn close_item_menu(&mut self) {
        if let Some(mut menu) = self.item_menu.take() {
            if matches!(menu.target, Target::Scopes(_))
                && IsWindowVisible(self.hwnd) != 0
                && animations_enabled()
            {
                self.destroy_dismissing_menu();
                let mut rect: RECT = zeroed();
                GetWindowRect(menu.hwnd, &mut rect);
                menu.accessibility.hide();
                menu.motion = Some(animation::Tween::exit(
                    rect.top,
                    rect.top - layout::px(8, menu.dpi),
                    Instant::now(),
                    90,
                ));
                self.dismissing_menu = Some(menu);
                if !self.frame_clock.start() {
                    self.destroy_dismissing_menu();
                }
            } else {
                DestroyWindow(menu.hwnd);
            }
            self.invalidate();
            // Provider updates are deferred while the menu shows a snapshot of an item.
            PostMessageW(self.hwnd, EVENT, 0, 0);
        }
    }

    unsafe fn destroy_dismissing_menu(&mut self) {
        if let Some(menu) = self.dismissing_menu.take() {
            DestroyWindow(menu.hwnd);
        }
    }

    pub(super) fn menu_animating(&self) -> bool {
        self.dismissing_menu.is_some()
            || self
                .item_menu
                .as_ref()
                .is_some_and(|menu| menu.motion.is_some())
    }

    pub(super) unsafe fn finish_menu_animations(&mut self) {
        self.destroy_dismissing_menu();
        if let Some(menu) = &mut self.item_menu {
            if let Some(motion) = menu.motion.take() {
                menu.move_to(motion.target());
            }
        }
    }

    pub(super) unsafe fn animate_menus(&mut self, now: Instant) {
        for menu in [&mut self.item_menu, &mut self.dismissing_menu]
            .into_iter()
            .flatten()
        {
            if let Some(motion) = &menu.motion {
                menu.move_to(motion.value(now));
                if !motion.active(now) {
                    menu.motion = None;
                }
            }
        }
        if self
            .dismissing_menu
            .as_ref()
            .is_some_and(|menu| menu.motion.is_none())
        {
            self.destroy_dismissing_menu();
        }
    }

    pub(super) unsafe fn item_menu(&mut self, mut point: POINT, keyboard: bool) {
        if keyboard && self.editor.is_composing() {
            return;
        }
        let (index, _block) = if keyboard {
            if !self.query.query_active || !self.viewport.range().contains(&self.viewport.selected)
            {
                return;
            }
            point = POINT {
                x: self.px(440),
                y: self.px(self.selection_top()),
            };
            ClientToScreen(self.hwnd, &mut point);
            (self.viewport.selected, None)
        } else {
            let mut client = point;
            ScreenToClient(self.hwnd, &mut client);
            let lx = self.logical(client.x);
            let ly = self.logical(client.y);
            let Some(index) = self.hit_result(lx, ly) else {
                self.close_item_menu();
                return;
            };
            let block = self
                .query
                .results
                .get(index)
                .and_then(|entry| entry.text_panel.as_ref())
                .and_then(|panel| {
                    let geometry = text_panel::Geometry::for_panel(
                        self.width(),
                        self.row_top() + self.viewport.row_top(index),
                        self.viewport.row_height(index),
                        panel,
                    );
                    match geometry.hit(lx, ly) {
                        text_panel::Hit::Header(b) | text_panel::Hit::Content(b) => Some(b),
                        _ => None,
                    }
                });
            (index, block)
        };
        let Some(entry) = self.query.results.get(index).cloned() else {
            return;
        };
        let is_panel = entry.text_panel.is_some();
        self.open_entry_menu(index, entry, is_panel, point, keyboard);
    }

    unsafe fn open_entry_menu(
        &mut self,
        index: usize,
        entry: Entry,
        is_panel: bool,
        point: POINT,
        keyboard: bool,
    ) {
        if !keyboard {
            // A click explicitly targets the visible result, including while
            // the user is composing pinyin. Keep that entry while finishing IME.
            input::complete_composition(self.hwnd);
            self.search_if_changed();
        }
        self.close_item_menu();
        self.result_navigation = true;
        self.viewport.selected = index;
        self.viewport_changed();
        self.finish_animations();
        let mut items = Vec::new();
        if is_panel {
            let has_selection = self.panels.has_selection(&entry.key, None);
            items.push(Item {
                command: 9,
                title: "复制选中文字".into(),
                separator: false,
                symbol: Some("\u{E8C8}"),
                shortcut: "Ctrl+C".into(),
                disabled: !has_selection,
            });
            items.push(Item {
                command: 10,
                title: "全选".into(),
                separator: false,
                symbol: Some("\u{E8B3}"),
                shortcut: "Ctrl+A".into(),
                disabled: false,
            });
        }
        let mut first_after_panel = is_panel;
        let mut append = |command, title: &str, separator: bool, symbol| {
            let sep = separator || std::mem::replace(&mut first_after_panel, false);
            items.push(Item {
                command,
                title: title.into(),
                separator: sep,
                symbol,
                shortcut: String::new(),
                disabled: false,
            });
        };
        if entry.preview.is_some() {
            append(7, "图片预览", false, Some("\u{E91B}"));
        } else if text_search::source(&entry).is_some() {
            append(8, "搜索", false, Some("\u{E721}"));
        }
        append(
            1,
            entry.action_label(),
            false,
            if entry.paste_after_execute {
                Some("\u{E77F}")
            } else if matches!(entry.action, Action::Copy(_)) {
                Some("\u{E8C8}")
            } else {
                Some("\u{E8A7}")
            },
        );
        for (index, action) in entry.context_actions.iter().enumerate() {
            let symbol = match action.action {
                Action::Copy(_) => Some("\u{E8C8}"),
                Action::Open(_) => Some("\u{E8A7}"),
                _ => None,
            };
            append(100 + index as u32, &action.title, index == 0, symbol);
        }
        append(
            2,
            if matches!(entry.action, Action::Open(_)) {
                "复制路径 / 地址"
            } else {
                "复制标题"
            },
            true,
            Some("\u{E8C8}"),
        );
        if let Action::Open(target) = &entry.action {
            if std::path::Path::new(target).is_absolute() {
                append(3, "打开所在位置", false, Some("\u{E838}"));
            }
            if entry.is_app()
                && std::path::Path::new(target).extension().is_some_and(|ext| {
                    ext.eq_ignore_ascii_case("exe") || ext.eq_ignore_ascii_case("lnk")
                })
            {
                append(4, "以管理员身份运行", false, Some("\u{EA18}"));
            }
        }
        if entry.is_app() {
            let usage = self.history.get(&entry.key).cloned().unwrap_or_default();
            if !usage.removed {
                append(
                    5,
                    if usage
                        .pinned_queries
                        .contains(&search::pin_query(&self.input()))
                    {
                        "取消置顶"
                    } else {
                        "置顶"
                    },
                    true,
                    Some("\u{E718}"),
                );
            }
            append(
                6,
                if usage.removed {
                    "恢复到列表"
                } else {
                    "从列表移除"
                },
                usage.removed,
                Some("\u{E74D}"),
            );
        }
        let selected = items.iter().position(|item| !item.disabled).unwrap_or(0);
        self.open_popup_menu(
            items,
            Target::Entry(entry, is_panel),
            point,
            MENU_WIDTH,
            selected,
        );
    }

    pub(super) unsafe fn open_scope_menu(
        &mut self,
        choices: Vec<(scope::Scope, String, String, bool)>,
        point: POINT,
    ) {
        let menu_width = choices.iter().map(|(_, title, shortcut, _)| {
            self.text_engine.measure_width(title, gfx::FontRole::Body).unwrap_or(160.0)
                + self.text_engine.measure_width(shortcut, gfx::FontRole::Small).unwrap_or(64.0) + 110.0
        }).fold(320.0_f32, f32::max).ceil() as i32;
        let menu_width = menu_width.min(560);
        let selected = choices
            .iter()
            .position(|(scope, _, _, _)| scope == &self.scope)
            .unwrap_or(0);
        let items = choices
            .iter()
            .map(|(scope, title, shortcut, separator)| Item {
                command: 0,
                title: title.clone(),
                separator: *separator,
                symbol: Some(scope.symbol()),
                shortcut: shortcut.clone(),
                disabled: false,
            })
            .collect();
        let scopes = choices.into_iter().map(|(scope, _, _, _)| scope).collect();
        self.open_popup_menu(items, Target::Scopes(scopes), point, menu_width, selected);
    }

    pub(super) unsafe fn feedback_menu(&mut self) {
        let Some(message) = self.status.clone() else { return; };
        self.close_item_menu();
        let items = ["查看完整信息", "复制完整信息", "重新搜索", "打开设置"]
            .into_iter().enumerate().map(|(command, title)| Item {
                command: command as u32, title: title.into(), separator: command == 2,
                symbol: None, shortcut: String::new(), disabled: false,
            }).collect();
        let mut point = POINT { x: self.px(24), y: self.px(self.footer_top()) };
        ClientToScreen(self.hwnd, &mut point);
        self.open_popup_menu(items, Target::Feedback(message), point, MENU_WIDTH, 0);
    }

    pub(super) unsafe fn confirm_item_action(&mut self, entry: Entry) {
        let items = vec![
            Item {
                command: 1,
                title: format!("确认{}", entry.title),
                separator: false,
                symbol: None,
                shortcut: String::new(),
                disabled: false,
            },
            Item {
                command: 0,
                title: "取消".into(),
                separator: false,
                symbol: None,
                shortcut: String::new(),
                disabled: false,
            },
        ];
        let mut point = POINT {
            x: self.px(440),
            y: self.px(self.selection_top()),
        };
        ClientToScreen(self.hwnd, &mut point);
        self.open_popup_menu(items, Target::Entry(entry, false), point, MENU_WIDTH, 1);
    }

    pub(super) fn scope_menu_open(&self) -> bool {
        self.item_menu
            .as_ref()
            .is_some_and(|menu| matches!(menu.target, Target::Scopes(_)))
    }

    pub(super) unsafe fn begin_scope_alt_selection(&mut self) {
        self.cancel_alt_preview();
        if self.editor.is_composing() || self.closing.is_some() || IsWindowVisible(self.hwnd) == 0 {
            return;
        }
        if !self.scope_menu_open() {
            self.scope_menu();
        }
        if let Some(menu) = &mut self.item_menu {
            if matches!(menu.target, Target::Scopes(_)) {
                menu.alt_select = true;
            }
        }
    }

    pub(super) unsafe fn navigate_scope_with_alt(&mut self, direction: i32) {
        self.begin_scope_alt_selection();
        if self.scope_menu_open() {
            self.move_menu_selection(direction);
        }
    }

    pub(super) unsafe fn finish_scope_alt_selection(&mut self) -> bool {
        let selecting = self
            .item_menu
            .as_ref()
            .is_some_and(|menu| menu.alt_select && matches!(menu.target, Target::Scopes(_)));
        if selecting {
            if self.item_menu.as_ref().is_some_and(|menu| menu.alt_changed) {
                self.execute_menu_selection();
            } else {
                // Tapping Alt only previews the menu, even when the current
                // scope is hidden by menu preferences.
                self.close_item_menu();
            }
        }
        selecting
    }

    unsafe fn open_popup_menu(
        &mut self,
        items: Vec<Item>,
        target: Target,
        mut point: POINT,
        menu_width: i32,
        selected: usize,
    ) {
        self.close_image_preview();
        if let Some(tooltip) = &self.tooltip { tooltip.hide(); }
        // A replacement popup must never overlap the previous exit animation.
        self.destroy_dismissing_menu();
        if items.is_empty() {
            return;
        }
        let mut info: MONITORINFO = zeroed();
        info.cbSize = size_of::<MONITORINFO>() as u32;
        GetMonitorInfoW(MonitorFromPoint(point, MONITOR_DEFAULTTONEAREST), &mut info);
        let area = info.rcWork;
        let visible =
            ((self.logical(area.bottom - area.top) - 2 * PADDING - 16) / ROW).max(1) as usize;
        let visible = visible.min(items.len());
        let menu_width = menu_width.min(self.logical(area.right - area.left)).max(1);
        let width = self.px(menu_width);
        let height = self.px(visible as i32 * ROW + 2 * PADDING);
        point.x = point
            .x
            .clamp(area.left, (area.right - width).max(area.left));
        point.y = point
            .y
            .clamp(area.top, (area.bottom - height).max(area.top));
        let instance = GetModuleHandleW(null());
        let class_name = wide(CLASS);
        RegisterClassW(&WNDCLASSW {
            lpfnWndProc: Some(menu_proc),
            hInstance: instance,
            hCursor: LoadCursorW(null_mut(), IDC_ARROW),
            lpszClassName: class_name.as_ptr(),
            ..zeroed()
        });
        let hwnd = CreateWindowExW(
            WS_EX_NOREDIRECTIONBITMAP | WS_EX_TOOLWINDOW | WS_EX_TOPMOST | WS_EX_NOACTIVATE,
            class_name.as_ptr(),
            wide(match &target {
                Target::Scopes(_) => "搜索范围",
                Target::Feedback(_) => "搜索状态",
                _ => "项目操作",
            })
            .as_ptr(),
            WS_POPUP | WS_THICKFRAME,
            point.x,
            point.y,
            width,
            height,
            self.hwnd,
            null_mut(),
            instance,
            null(),
        );
        if hwnd.is_null() {
            return;
        }
        let backdrop_active =
            backdrop::apply_popup(hwnd, self.dark, self.palette, &self.settings.backdrop);
        let mut cursor: POINT = zeroed();
        let mouse_position = (GetCursorPos(&mut cursor) != 0).then_some((cursor.x, cursor.y));
        let renderer = match gfx::WindowRenderer::new() {
            Ok(renderer) => renderer,
            Err(error) => {
                eprintln!("Renderer initialization failed: {error}");
                DestroyWindow(hwnd);
                return;
            }
        };
        let text_engine = self.text_engine.clone();
        let motion = (matches!(target, Target::Scopes(_)) && animations_enabled()).then(|| {
            animation::Tween::new(point.y - self.px(8), point.y, Instant::now(), 140)
        });
        self.item_menu = Some(Menu {
            accessibility: accessibility::Tree::new(hwnd, true),
            hwnd,
            target,
            width: menu_width,
            dpi: GetDpiForWindow(hwnd).max(96),
            alt_select: false,
            alt_changed: false,
            items,
            selected,
            first: selected.saturating_sub(visible - 1),
            visible,
            renderer,
            backdrop_active,
            text_engine,
            mouse_position,
            motion,
        });
        if let Some(menu) = &self.item_menu {
            menu.accessibility.nodes(
                menu.items
                    .iter()
                    .enumerate()
                    .map(|(index, item)| accessibility::Node {
                        id: index.to_string(),
                        name: item.title.clone(),
                        help: item.shortcut.to_string(),
                        bounds: [0.; 4],
                        actionable: !item.disabled,
                    })
                    .collect(),
                String::new(),
            );
        }
        if let Some(menu) = &self.item_menu {
            if let Some(motion) = &menu.motion {
                menu.move_to(motion.value(Instant::now()));
            }
        }
        if self.menu_animating() && !self.frame_clock.start() {
            self.finish_menu_animations();
        }
        ShowWindow(hwnd, SW_SHOWNOACTIVATE);
        // WS_EX_NOACTIVATE keeps keyboard input in the editor. Tell DWM that
        // this owned popup still participates in the active interaction.
        SendMessageW(hwnd, WM_NCACTIVATE, 1, -1);
        self.focus_input();
        self.invalidate();
    }

    pub(super) unsafe fn move_menu_selection(&mut self, direction: i32) {
        if let Some(menu) = &mut self.item_menu {
            let len = menu.items.len() as i32;
            if len == 0 {
                return;
            }
            let step = if direction >= 0 { 1 } else { -1 };
            let mut candidate = menu.selected as i32;
            for _ in 0..len {
                candidate = (candidate + step).rem_euclid(len);
                if !menu.items[candidate as usize].disabled {
                    menu.select(candidate as usize);
                    break;
                }
            }
        }
    }

    pub(super) unsafe fn menu_key(&mut self, key: u16) -> bool {
        if self.item_menu.is_none() {
            return false;
        }
        match key {
            VK_UP => self.move_menu_selection(-1),
            VK_DOWN => self.move_menu_selection(1),
            VK_TAB => self.move_menu_selection(if GetKeyState(VK_SHIFT as i32) < 0 {
                -1
            } else {
                1
            }),
            VK_HOME | VK_END => {
                if let Some(menu) = &mut self.item_menu {
                    let selected = if key == VK_HOME {
                        menu.items.iter().position(|item| !item.disabled)
                    } else {
                        menu.items.iter().rposition(|item| !item.disabled)
                    };
                    if let Some(selected) = selected {
                        menu.select(selected);
                    }
                }
            }
            VK_RETURN => self.execute_menu_selection(),
            VK_LEFT | VK_ESCAPE | VK_APPS => self.close_item_menu(),
            VK_RIGHT => {}
            VK_SHIFT | VK_CONTROL | VK_MENU => {}
            _ => {
                self.close_item_menu();
                return false;
            }
        }
        true
    }

    unsafe fn execute_menu_selection(&mut self) {
        if self.item_menu.as_ref().is_some_and(|menu| {
            menu.items
                .get(menu.selected)
                .is_none_or(|item| item.disabled)
        }) {
            return;
        }
        if let Some(Menu {
            target: Target::Scopes(scopes),
            selected,
            ..
        }) = &self.item_menu
        {
            let scope = scopes[*selected].clone();
            self.close_item_menu();
            self.set_scope(scope);
            return;
        }
        let Some(menu) = self.item_menu.take() else {
            return;
        };
        DestroyWindow(menu.hwnd);
        PostMessageW(self.hwnd, EVENT, 0, 0);
        match menu.target {
            Target::Feedback(message) => match menu.items[menu.selected].command {
                0 => {
                    MessageBoxW(self.hwnd, wide(&message).as_ptr(),
                        wide("MiniLauncher · 完整信息（Ctrl+C 复制）").as_ptr(), MB_OK | MB_ICONINFORMATION);
                }
                1 => { editor::set_clipboard_text(&message); }
                2 => self.retry_search(),
                _ => self.open_settings(),
            },
            Target::Entry(entry, is_panel) => match menu.items[menu.selected].command {
                9 => {
                    if is_panel {
                        if let Some(text) = self.panels.copy_selection(&entry.key, None) {
                            editor::set_clipboard_text(&text);
                        }
                    }
                }
                10 => {
                    if is_panel {
                        self.panels.select_all(&entry.key, None);
                        self.invalidate();
                    }
                }
                command => self.execute_item_command(entry, command),
            },
            Target::Scopes(scopes) => self.set_scope(scopes[menu.selected].clone()),
        }
        self.invalidate();
    }

    unsafe fn paint_item_menu(&mut self, hwnd: HWND, print_dc: Option<HDC>) {
        if let Some(menu) = self.item_menu.as_ref().filter(|menu| menu.hwnd == hwnd) {
            let mut bounds: RECT = zeroed();
            GetWindowRect(hwnd, &mut bounds);
            let rows = (menu.first..(menu.first + menu.visible).min(menu.items.len()))
                .map(|index| {
                    let top = bounds.top
                        + layout::px(PADDING + (index - menu.first) as i32 * ROW, menu.dpi);
                    (
                        index,
                        [
                            bounds.left as f64,
                            top as f64,
                            (bounds.right - bounds.left) as f64,
                            layout::px(ROW, menu.dpi) as f64,
                        ],
                    )
                })
                .collect();
            menu.accessibility.view(
                menu.selected,
                IsWindowVisible(hwnd) != 0,
                true,
                [
                    bounds.left as f64,
                    bounds.top as f64,
                    (bounds.right - bounds.left) as f64,
                    (bounds.bottom - bounds.top) as f64,
                ],
                rows,
            );
        }
        let Some(menu) = self
            .item_menu
            .as_ref()
            .filter(|menu| menu.hwnd == hwnd)
            .or_else(|| self.dismissing_menu.as_ref().filter(|menu| menu.hwnd == hwnd))
        else {
            return;
        };
        let menu_width = menu.width;
        let height = menu.visible as i32 * ROW + 2 * PADDING;
        let mut paint: PAINTSTRUCT = zeroed();
        if print_dc.is_none() {
            BeginPaint(hwnd, &mut paint);
        }
        let dpi = menu.dpi;
        let palette = self.palette;
        let background =
            backdrop::background(palette, &self.settings.backdrop, menu.backdrop_active);
        let menu_ref = self
            .item_menu
            .as_mut()
            .filter(|menu| menu.hwnd == hwnd)
            .or_else(|| self.dismissing_menu.as_mut().filter(|menu| menu.hwnd == hwnd))
            .unwrap();
        let menu_items = &menu_ref.items;
        let menu_target = &menu_ref.target;
        let text_engine = &menu_ref.text_engine;
        let first = menu_ref.first;
        let selected = menu_ref.selected;
        let visible = menu_ref.visible;
        let is_scopes = matches!(menu_ref.target, Target::Scopes(_));
        let scope = self.scope.clone();

        if let Err(error) = menu_ref.renderer.render(hwnd, dpi, print_dc, |rt, _| {
            rt.SetDpi(dpi as f32, dpi as f32);
            rt.Clear(Some(&windows::Win32::Graphics::Direct2D::Common::D2D1_COLOR_F {
                r: 0.0,
                g: 0.0,
                b: 0.0,
                a: 0.0,
            }));

            let menu_rect = windows::Win32::Graphics::Direct2D::Common::D2D_RECT_F {
                left: 0.0,
                top: 0.0,
                right: menu_width as f32,
                bottom: height as f32,
            };
            gfx::fill_round_rect(rt, &menu_rect, 8.0, background)?;
            gfx::stroke_round_rect(rt, &menu_rect, 8.0, gfx::colorref_to_d2d(palette.border, 1.0), 1.0)?;

            for (row, item) in menu_items
                .iter()
                .skip(first)
                .take(visible)
                .enumerate()
            {
                let y = (PADDING + row as i32 * ROW) as f32;
                if item.separator && row != 0 {
                    gfx::draw_horizontal_line(
                        rt,
                        14.0,
                        (menu_width - 14) as f32,
                        y,
                        1.0,
                        gfx::colorref_to_d2d(palette.border, 1.0),
                    )?;
                }
                if first + row == selected && !item.disabled {
                    let sel_rect = windows::Win32::Graphics::Direct2D::Common::D2D_RECT_F {
                        left: 5.0,
                        top: y + 2.0,
                        right: (menu_width - 5) as f32,
                        bottom: y + ROW as f32 - 2.0,
                    };
                    gfx::fill_round_rect(rt, &sel_rect, 5.0, gfx::colorref_to_d2d(palette.selection, 1.0))?;
                    let accent_rect = windows::Win32::Graphics::Direct2D::Common::D2D_RECT_F {
                        left: 5.0,
                        top: y + 11.0,
                        right: 8.0,
                        bottom: y + ROW as f32 - 11.0,
                    };
                    gfx::fill_round_rect(rt, &accent_rect, 1.5, gfx::colorref_to_d2d(palette.accent, 1.0))?;
                }

                let symbol = item.symbol.unwrap_or(match item.command {
                    1 => "\u{E8A7}",
                    2 => "\u{E8C8}",
                    3 => "\u{E838}",
                    4 => "\u{EA18}",
                    5 => "\u{E718}",
                    6 => "\u{E74D}",
                    7 => "\u{E91B}",
                    8 => "\u{E721}",
                    9 => "\u{E8C8}",
                    10 => "\u{E8B3}",
                    _ => "\u{E713}",
                });
                let icon_rect = windows::Win32::Graphics::Direct2D::Common::D2D_RECT_F {
                    left: 18.0,
                    top: y,
                    right: 40.0,
                    bottom: y + ROW as f32,
                };
                text_engine.draw_text_role(
                    rt,
                    symbol,
                    gfx::FontRole::Symbol,
                    &icon_rect,
                    gfx::colorref_to_d2d(if item.disabled { palette.hint } else { palette.secondary }, 1.0),
                    windows::Win32::Graphics::DirectWrite::DWRITE_TEXT_ALIGNMENT_CENTER,
                    windows::Win32::Graphics::DirectWrite::DWRITE_PARAGRAPH_ALIGNMENT_CENTER,
                )?;

                let title_right = menu_width
                    - if is_scopes {
                        if item.shortcut.is_empty() { 38 } else { 106 }
                    } else if item.shortcut.is_empty() {
                        18
                    } else {
                        86
                    };
                let text_rect = windows::Win32::Graphics::Direct2D::Common::D2D_RECT_F {
                    left: 48.0,
                    top: y,
                    right: title_right as f32,
                    bottom: y + ROW as f32,
                };
                text_engine.draw_text_role(
                    rt,
                    &item.title,
                    gfx::FontRole::Body,
                    &text_rect,
                    gfx::colorref_to_d2d(if item.disabled { palette.hint } else { palette.text }, 1.0),
                    windows::Win32::Graphics::DirectWrite::DWRITE_TEXT_ALIGNMENT_LEADING,
                    windows::Win32::Graphics::DirectWrite::DWRITE_PARAGRAPH_ALIGNMENT_CENTER,
                )?;

                let shortcut_right = if is_scopes {
                    menu_width - 36
                } else {
                    menu_width - 18
                };
                let shortcut_rect = windows::Win32::Graphics::Direct2D::Common::D2D_RECT_F {
                    left: (shortcut_right - 64) as f32,
                    top: y,
                    right: shortcut_right as f32,
                    bottom: y + ROW as f32,
                };
                text_engine.draw_text_role(
                    rt,
                    &item.shortcut,
                    gfx::FontRole::Small,
                    &shortcut_rect,
                    gfx::colorref_to_d2d(if item.disabled { palette.hint } else { palette.secondary }, 1.0),
                    windows::Win32::Graphics::DirectWrite::DWRITE_TEXT_ALIGNMENT_TRAILING,
                    windows::Win32::Graphics::DirectWrite::DWRITE_PARAGRAPH_ALIGNMENT_CENTER,
                )?;

                if let Target::Scopes(scopes) = menu_target {
                    if scopes[first + row] == scope {
                        let check_rect = windows::Win32::Graphics::Direct2D::Common::D2D_RECT_F {
                            left: (menu_width - 30) as f32,
                            top: y,
                            right: (menu_width - 12) as f32,
                            bottom: y + ROW as f32,
                        };
                        text_engine.draw_text_role(
                            rt,
                            "✓",
                            gfx::FontRole::Body,
                            &check_rect,
                            gfx::colorref_to_d2d(palette.accent, 1.0),
                            windows::Win32::Graphics::DirectWrite::DWRITE_TEXT_ALIGNMENT_CENTER,
                            windows::Win32::Graphics::DirectWrite::DWRITE_PARAGRAPH_ALIGNMENT_CENTER,
                        )?;
                    }
                }
            }
            Ok(())
        }) {
            eprintln!("Popup drawing failed: {error}");
        }

        if print_dc.is_none() {
            EndPaint(hwnd, &paint);
        }
    }
}

impl Menu {
    unsafe fn move_to(&self, y: i32) {
        let mut rect: RECT = zeroed();
        GetWindowRect(self.hwnd, &mut rect);
        SetWindowPos(
            self.hwnd,
            null_mut(),
            rect.left,
            y,
            0,
            0,
            SWP_NOSIZE | SWP_NOZORDER | SWP_NOACTIVATE,
        );
    }

    pub(super) unsafe fn update_theme(
        &mut self,
        dark: bool,
        palette: Palette,
        mode: &str,
        text_engine: &gfx::TextEngine,
    ) {
        self.backdrop_active = backdrop::apply_popup(self.hwnd, dark, palette, mode);
        self.text_engine = text_engine.clone();
        InvalidateRect(self.hwnd, null(), 0);
    }

    unsafe fn select(&mut self, selected: usize) {
        if self.alt_select {
            self.alt_changed = true;
        }
        let previous = (self.selected, self.first);
        self.selected = selected;
        if self.selected < self.first {
            self.first = self.selected;
        }
        if self.selected >= self.first + self.visible {
            self.first = self.selected + 1 - self.visible;
        }
        if previous != (self.selected, self.first) {
            InvalidateRect(self.hwnd, null(), 0);
        }
    }
}

unsafe extern "system" fn menu_proc(
    hwnd: HWND,
    message: u32,
    wparam: WPARAM,
    lparam: LPARAM,
) -> LRESULT {
    if let Some(result) = backdrop::popup_frame_message(hwnd, message) {
        return result;
    }
    let owner = GetWindow(hwnd, GW_OWNER);
    let pointer = GetWindowLongPtrW(owner, GWLP_USERDATA) as *mut State;
    if pointer.is_null() {
        return DefWindowProcW(hwnd, message, wparam, lparam);
    }
    let state = &mut *pointer;
    // The dismissed snapshot only paints; input already belongs to the owner.
    if state
        .dismissing_menu
        .as_ref()
        .is_some_and(|menu| menu.hwnd == hwnd)
    {
        return match message {
            WM_PAINT => {
                state.paint_item_menu(hwnd, None);
                0
            }
            WM_PRINTCLIENT => {
                state.paint_item_menu(hwnd, Some(wparam as HDC));
                0
            }
            WM_ERASEBKGND => 1,
            WM_NCHITTEST => HTTRANSPARENT as isize,
            WM_GETOBJECT | accessibility::ACTION => 0,
            _ if (WM_MOUSEFIRST..=WM_MOUSELAST).contains(&message)
                || (WM_KEYFIRST..=WM_KEYLAST).contains(&message) =>
            {
                0
            }
            _ => DefWindowProcW(hwnd, message, wparam, lparam),
        };
    }
    match message {
        WM_GETOBJECT => {
            if let Some(menu) = &state.item_menu {
                if let Some(result) = menu.accessibility.get_object(wparam, lparam) {
                    return result;
                }
            }
            DefWindowProcW(hwnd, message, wparam, lparam)
        }
        accessibility::ACTION => {
            let action = state
                .item_menu
                .as_ref()
                .and_then(|menu| menu.accessibility.resolve(wparam, lparam));
            if let Some((Some(id), invoke)) = action {
                if let Ok(index) = id.parse::<usize>() {
                    let enabled = state
                        .item_menu
                        .as_ref()
                        .and_then(|menu| menu.items.get(index))
                        .is_some_and(|item| !item.disabled);
                    if enabled {
                        if let Some(menu) = &mut state.item_menu {
                            menu.select(index);
                        }
                        if invoke {
                            state.execute_menu_selection();
                        }
                    }
                }
            }
            0
        }

        WM_DPICHANGED => {
            if let Some(menu) = state.item_menu.as_mut() {
                menu.dpi = (wparam & 0xffff).max(96) as u32;
                let rect = &*(lparam as *const RECT);
                SetWindowPos(
                    hwnd,
                    null_mut(),
                    rect.left,
                    rect.top,
                    layout::px(menu.width, menu.dpi),
                    layout::px(menu.visible as i32 * ROW + 2 * PADDING, menu.dpi),
                    SWP_NOZORDER | SWP_NOACTIVATE,
                );
                InvalidateRect(hwnd, null(), 0);
            }
            0
        }
        WM_SIZE => {
            InvalidateRect(hwnd, null(), 0);
            0
        }
        WM_MOUSEACTIVATE => MA_NOACTIVATE as isize,
        WM_ERASEBKGND => 1,
        WM_PAINT => {
            state.paint_item_menu(hwnd, None);
            0
        }
        WM_PRINTCLIENT => {
            state.paint_item_menu(hwnd, Some(wparam as HDC));
            0
        }
        WM_MOUSEMOVE | WM_LBUTTONUP | WM_RBUTTONUP => {
            let dpi = state.item_menu.as_ref().map_or(state.dpi, |menu| menu.dpi);
            let x = ((lparam as u16 as i16 as i32) as f64 * 96.0 / dpi as f64).round() as i32;
            let y = (((lparam >> 16) as u16 as i16 as i32) as f64 * 96.0 / dpi as f64).round()
                as i32
                - PADDING;
            if let Some(menu) = &mut state.item_menu {
                if message == WM_MOUSEMOVE {
                    let mut cursor: POINT = zeroed();
                    if GetCursorPos(&mut cursor) != 0 {
                        let position = Some((cursor.x, cursor.y));
                        // Repainting can synthesize a move at the same position.
                        // Keep keyboard/wheel selection until the pointer moves.
                        if menu.mouse_position == position {
                            return 0;
                        }
                        menu.mouse_position = position;
                    }
                }
                let row = (y / ROW) as usize;
                if (0..menu.width).contains(&x) && y >= 0 && row < menu.visible {
                    let selected = menu.first + row;
                    if !menu.items[selected].disabled {
                        menu.select(selected);
                        if message != WM_MOUSEMOVE {
                            state.execute_menu_selection();
                        }
                    }
                }
            }
            0
        }
        WM_MOUSEWHEEL => {
            state.move_menu_selection(if (wparam >> 16) as u16 as i16 > 0 {
                -1
            } else {
                1
            });
            0
        }
        WM_KEYDOWN => {
            state.menu_key(wparam as u16);
            0
        }
        _ => DefWindowProcW(hwnd, message, wparam, lparam),
    }
}
