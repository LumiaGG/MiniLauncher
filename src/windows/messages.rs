//! Win32 callbacks dispatch into the application components.
use super::*;

// Undocumented themed caption/frame paint messages, also suppressed by SDL for
// borderless windows: https://github.com/libsdl-org/SDL/blob/main/src/video/windows/SDL_windowsevents.c
const WM_NCUAHDRAWCAPTION: u32 = 0x00AE;
const WM_NCUAHDRAWFRAME: u32 = 0x00AF;

pub(super) unsafe extern "system" fn window_proc(
    hwnd: HWND,
    message: u32,
    wparam: WPARAM,
    lparam: LPARAM,
) -> LRESULT {
    // Win32 creation messages arrive before the application state is attached.
    if message == WM_NCCALCSIZE {
        return 0;
    }
    // DWM still owns the rounded outline/shadow; USER32 must not paint the
    // legacy thick frame over our full-client window during activation/resize.
    if matches!(
        message,
        WM_NCPAINT | WM_NCUAHDRAWCAPTION | WM_NCUAHDRAWFRAME
    ) {
        return 0;
    }
    if message == WM_NCACTIVATE {
        return DefWindowProcW(hwnd, message, wparam, -1);
    }
    let pointer = GetWindowLongPtrW(hwnd, GWLP_USERDATA) as *mut State;
    if pointer.is_null() {
        return DefWindowProcW(hwnd, message, wparam, lparam);
    }
    // DoDragDrop runs a nested message loop. End the State borrow before entering it.
    if message == START_DRAG {
        let (prepared, cancel, text_engine) = {
            let state = &mut *pointer;
            if state.drag_pending != Some(wparam as u64) {
                return 0;
            }
            if GetAsyncKeyState(VK_LBUTTON as i32) >= 0 || IsWindowVisible(hwnd) == 0 {
                state.cancel_pending_drag();
                return 0;
            }
            let Some(prepared) = state.prepared_drag.take() else {
                return 0;
            };
            let Some(cancel) = state.drag_cancel.clone() else {
                return 0;
            };
            state.drag_pending = None;
            state.dragging = true;
            state.hide_immediately();
            (prepared, cancel, state.text_engine.clone())
        };
        let result = dragdrop::run(prepared, cancel, hwnd as usize, text_engine);
        let pointer = GetWindowLongPtrW(hwnd, GWLP_USERDATA) as *mut State;
        if !pointer.is_null() {
            let state = &mut *pointer;
            state.dragging = false;
            state.drag_cancel = None;
            if let Err(error) = result {
                state.notify_execution_error(&format!("拖动失败：{error}"));
            }
            PostMessageW(hwnd, EVENT, 0, 0);
        }
        return 0;
    }
    let state = &mut *pointer;
    if message == WM_HOTKEY
        || (matches!(message, WM_KEYDOWN | WM_SYSKEYDOWN) && wparam as u16 != VK_MENU)
    {
        state.cancel_alt_preview();
    }
    if matches!(message, WM_KEYUP | WM_SYSKEYUP) && wparam as u16 == VK_MENU {
        state.release_alt_preview();
    }
    if state.closing.is_some()
        && ((WM_KEYFIRST..=WM_KEYLAST).contains(&message)
            || (WM_MOUSEFIRST..=WM_MOUSELAST).contains(&message)
            || message == WM_CONTEXTMENU)
    {
        return 0;
    }
    if message == state.taskbar_created {
        if let Err(error) = state
            .plugin_hotkeys
            .configure(state.plugin_shortcuts.clone(), &state.settings.hotkey)
        {
            state.config_error = Some(error.clone());
            state.status = Some(error);
        }
        state.tray.add();
        return 0;
    }
    match message {
        WM_ENTERSIZEMOVE => {
            state.close_item_menu();
            state.finish_animations();
            0
        }
        WM_HOTKEY if state.plugin_hotkeys.binding(wparam as i32).is_some() => {
            let (plugin, prefill) = state.plugin_hotkeys.binding(wparam as i32).unwrap();
            let plugin = plugin.to_string();
            match prefill {
                hotkey::Prefill::Selection => state.start_plugin_selection(plugin),
                hotkey::Prefill::None => state.toggle_plugin_scope(plugin),
            }
            0
        }
        WM_HOTKEY if wparam as i32 != state.hotkey_id => 0,
        WM_HOTKEY | SHOW => {
            if message == WM_HOTKEY && IsWindowVisible(hwnd) != 0 && state.closing.is_none() {
                state.hide();
            } else {
                state.show();
            }
            0
        }
        EVENT => {
            if state.item_menu.is_none() && !state.dragging && state.image_viewer.is_null() {
                state.drain_events();
            }
            0
        }
        WM_CONTEXTMENU => {
            state.item_menu(
                POINT {
                    x: lparam as u16 as i16 as i32,
                    y: (lparam >> 16) as u16 as i16 as i32,
                },
                lparam == -1,
            );
            0
        }
        WM_COMMAND if lparam == 0 => {
            match wparam {
                tray_popup::OPEN => state.show(),
                tray_popup::SETTINGS => state.open_settings(),
                tray_popup::RELOAD => state.reload(),
                tray_popup::QUIT => {
                    DestroyWindow(hwnd);
                }
                _ => {}
            }
            0
        }
        WM_COPYDATA => {
            let cds = lparam as *const COPYDATASTRUCT;
            if !cds.is_null() && (*cds).dwData == 0 {
                let len = (*cds).cbData as usize;
                if (*cds).lpData.is_null()
                    || len == 0
                    || len > mini_launcher::history::control::MAX_REQUEST_BYTES
                {
                    return 0;
                }
                let bytes = std::slice::from_raw_parts((*cds).lpData.cast::<u8>(), len);
                if let Some(changes) =
                    mini_launcher::history::control::decode(bytes, &config::settings_path())
                {
                    mini_launcher::history::apply_edits(
                        Arc::make_mut(&mut state.history),
                        &changes.edits,
                    );
                    if state.history_sender.send(state.history.clone()).is_err() {
                        return 0;
                    }
                    state.request_search();
                    return 1;
                }
                return 0;
            }
            if !cds.is_null() && unsafe { (*cds).dwData } == STATUS_CAPSULE_MAGIC {
                let len = unsafe { (*cds).cbData as usize };
                let ptr = unsafe { (*cds).lpData as *const u8 };
                let bytes = if ptr.is_null() || len == 0 {
                    &[][..]
                } else {
                    unsafe { std::slice::from_raw_parts(ptr, len) }
                };
                if let Ok(next) = parse_status_capsule(bytes, state.palette.accent) {
                    state.apply_status_capsule(next);
                }
                return 1;
            }
            0
        }
        WM_NCHITTEST => {
            if state.closing.is_some() {
                return HTTRANSPARENT as isize;
            }
            let mut rect: RECT = zeroed();
            GetWindowRect(hwnd, &mut rect);
            let y = ((lparam >> 16) as u16 as i16 as i32) - rect.top;
            let x = state.logical((lparam as u16 as i16 as i32) - rect.left);
            let y = state.logical(y);
            if y < 6 || (y < IDLE_HEIGHT && !(16..state.width() - 16).contains(&x)) {
                HTCAPTION as isize
            } else {
                HTCLIENT as isize
            }
        }
        WM_PAINT => {
            state.paint(None);
            0
        }
        WM_PRINTCLIENT => {
            state.paint(Some(wparam as HDC));
            0
        }
        WM_ERASEBKGND => 1,
        WM_SETFOCUS => {
            state.focus_input();
            0
        }
        WM_KILLFOCUS => {
            state.cancel_alt_preview();
            state.editor.set_focused(false);
            state.invalidate();
            0
        }
        WM_GETDLGCODE => DLGC_WANTALLKEYS as isize,
        WM_TIMER => {
            if wparam == scope_keys::ALT_PREVIEW_TIMER {
                state.poll_alt_preview();
            }
            if wparam == query::PROGRESS_TIMER {
                state.sync_query_progress();
            }
            if wparam == 1 && state.editor.is_focused() {
                state.invalidate();
            }
            0
        }
        WM_ACTIVATE if wparam & 0xffff == WA_INACTIVE as usize => {
            // Moving focus to an owned menu is still interaction with the launcher.
            if GetWindow(lparam as HWND, GW_OWNER) != hwnd {
                state.hide();
            }
            0
        }
        WM_ACTIVATEAPP if wparam == 0 => {
            state.hide();
            0
        }
        WM_ACTIVATE => {
            state.activate_scope_keys();
            state.focus_input();
            0
        }
        WM_CLOSE => {
            state.hide();
            0
        }
        picture_viewer::CLOSE => {
            state.close_image_preview();
            0
        }
        WM_DESTROY => {
            state.close_image_preview();
            if let Some(cancel) = &state.drag_cancel {
                cancel.store(true, Ordering::Relaxed);
            }
            PostQuitMessage(0);
            0
        }
        WM_NCDESTROY => {
            SetWindowLongPtrW(hwnd, GWLP_USERDATA, 0);
            DefWindowProcW(hwnd, message, wparam, lparam)
        }
        WM_DPICHANGED => {
            if state.positioning {
                return 0;
            }
            let rect = &*(lparam as *const RECT);
            state.fit_monitor(
                MonitorFromRect(rect, MONITOR_DEFAULTTONEAREST),
                Some(POINT {
                    x: rect.left,
                    y: rect.top,
                }),
                Some((wparam & 0xffff) as u32),
            );
            0
        }
        WM_EXITSIZEMOVE | WM_DISPLAYCHANGE => {
            state.fit_current_monitor();
            0
        }
        WM_SETTINGCHANGE => {
            if wparam == SPI_SETWORKAREA as usize {
                state.finish_animations();
                state.fit_current_monitor();
            } else if wparam == 0 {
                // General settings broadcasts need not change monitor geometry.
                state.update_theme(false);
            }
            0
        }
        WM_DWMCOMPOSITIONCHANGED => {
            // A reset can invalidate DWM attributes even with identical colors.
            state.update_theme(true);
            0
        }
        WM_THEMECHANGED | WM_DWMCOLORIZATIONCOLORCHANGED => {
            // Repeated theme/accent broadcasts must not tear down unchanged material or text.
            state.update_theme(false);
            0
        }
        TRAY => {
            match lparam as u32 {
                WM_LBUTTONUP => state.show(),
                WM_RBUTTONUP | WM_CONTEXTMENU => state.menu(),
                _ => {}
            }
            0
        }
        WM_SYSKEYDOWN | WM_KEYDOWN
            if wparam as u16 == VK_MENU
                && !state.editor.is_composing()
                && GetKeyState(VK_CONTROL as i32) >= 0
                && GetKeyState(VK_SHIFT as i32) >= 0
                && GetKeyState(VK_LWIN as i32) >= 0
                && GetKeyState(VK_RWIN as i32) >= 0 =>
        {
            if lparam & (1 << 30) == 0 {
                state.schedule_alt_preview();
            }
            0
        }
        WM_SYSKEYDOWN
            if !state.editor.is_composing()
                && GetKeyState(VK_CONTROL as i32) >= 0
                && GetKeyState(VK_SHIFT as i32) >= 0
                && GetKeyState(VK_LWIN as i32) >= 0
                && GetKeyState(VK_RWIN as i32) >= 0
                && GetKeyState(VK_MENU as i32) < 0 =>
        {
            if state.select_scope_shortcut(wparam as u16) {
                return 0;
            }
            match wparam as u16 {
                VK_UP | VK_LEFT | VK_DOWN | VK_RIGHT => {
                    let direction = if matches!(wparam as u16, VK_UP | VK_LEFT) {
                        -1
                    } else {
                        1
                    };
                    state.navigate_scope_with_alt(direction);
                    0
                }
                VK_ESCAPE if state.scope_menu_open() => {
                    state.close_item_menu();
                    0
                }
                _ => 0,
            }
        }
        WM_SYSKEYUP | WM_KEYUP if wparam as u16 == VK_MENU => {
            if state.finish_scope_alt_selection() {
                return 0;
            }
            0
        }
        WM_SYSCHAR if GetKeyState(VK_MENU as i32) < 0 && matches!(wparam as u16, 0x31..=0x33) => 0,
        WM_PASTE => {
            state.focus_input();
            match state.editor.paste() {
                Ok(editor::KeyResult::TextChanged) => {
                    state.close_image_preview();
                    state.result_navigation = false;
                    state.search_if_changed();
                    state.sync_height();
                }
                Err(error) => state.status = Some(error),
                _ => {}
            }
            state.invalidate();
            0
        }
        WM_KEYDOWN => {
            if state.menu_key(wparam as u16) {
                return 0;
            }
            if state.editor.is_composing() {
                return 0;
            }
            if state.panels.focused() {
                // Modifier keydowns precede the actual shortcut. Keep the body
                // focused until we know whether that shortcut edits the query.
                if matches!(
                    wparam as u16,
                    VK_SHIFT | VK_LSHIFT | VK_RSHIFT | VK_CONTROL | VK_LCONTROL | VK_RCONTROL
                        | VK_MENU | VK_LMENU | VK_RMENU | VK_LWIN | VK_RWIN | VK_RETURN
                ) {
                    return 0;
                }
                if wparam == b'A' as usize && GetKeyState(VK_CONTROL as i32) < 0 {
                    state.panels.select_focused();
                    state.invalidate();
                    return 0;
                }
                if wparam == b'X' as usize && GetKeyState(VK_CONTROL as i32) < 0 { return 0; }
                if wparam == b'C' as usize && GetKeyState(VK_CONTROL as i32) < 0 {
                    if let Some(text) = state.panels.copy_focused() {
                        editor::set_clipboard_text(&text);
                    }
                    // An empty body/selection must not fall through and copy
                    // the search box's unrelated selection.
                    return 0;
                }
                if wparam == b'F' as usize && GetKeyState(VK_CONTROL as i32) < 0 {
                    state.focus_input();
                    return 0;
                }
                if wparam as u16 == VK_TAB {
                    state.panels.cycle_focus(hwnd, GetKeyState(VK_SHIFT as i32) < 0);
                    state.editor.set_focused(!state.panels.focused());
                    state.invalidate();
                    return 0;
                }
                if wparam as u16 == VK_ESCAPE {
                    state.panels.clear_selection();
                    state.focus_input();
                    return 0;
                }
            }
            if wparam == b'I' as usize && GetKeyState(VK_CONTROL as i32) < 0
                && GetKeyState(VK_SHIFT as i32) < 0 && state.status.is_some() {
                state.feedback_menu();
                return 0;
            }
            if wparam == b'R' as usize && GetKeyState(VK_CONTROL as i32) < 0 {
                if state.query.results.is_empty() || state.status.is_some() {
                    state.retry_search();
                    return 0;
                }
                PostMessageW(hwnd, RETRY_PANEL, 0, 0);
                return 0;
            }
            if state.image_preview_key(wparam as u16, lparam) {
                return 0;
            }
            match wparam as u16 {
                VK_ESCAPE => {
                    PostMessageW(hwnd, WM_APP + 12, 0, 0);
                    return 0;
                }
                VK_TAB => {
                    let backwards = GetKeyState(VK_SHIFT as i32) < 0;
                    if let Some(entry) = state.query.results.get(state.viewport.selected) {
                        if state.panels.focus_entry(hwnd, &entry.key, backwards) {
                            state.editor.set_focused(false);
                            state.invalidate();
                            return 0;
                        }
                    }
                    state.move_selection(if backwards { -1 } else { 1 });
                    return 0;
                }
                VK_RIGHT
                    if state.query.query_active
                        && !state.query.results.is_empty()
                        && (state.result_navigation || state.editor.caret_at_end())
                        && GetKeyState(VK_CONTROL as i32) >= 0
                        && GetKeyState(VK_MENU as i32) >= 0
                        && GetKeyState(VK_SHIFT as i32) >= 0 =>
                {
                    state.item_menu(zeroed(), true);
                    return 0;
                }
                VK_APPS => {
                    state.item_menu(zeroed(), true);
                    return 0;
                }
                VK_F10 if GetKeyState(VK_SHIFT as i32) < 0 => {
                    state.item_menu(zeroed(), true);
                    return 0;
                }
                VK_PRIOR | VK_NEXT => {
                    state.result_navigation = true;
                    state
                        .viewport
                        .page(if wparam as u16 == VK_NEXT { 1 } else { -1 });
                    state.viewport_changed();
                    return 0;
                }
                _ => {}
            }
            if state.panels.focused() {
                state.focus_input();
            }
            let key_res = if (wparam == b'V' as usize && GetKeyState(VK_CONTROL as i32) < 0)
                || (wparam as u16 == VK_INSERT && GetKeyState(VK_SHIFT as i32) < 0)
            {
                match state.editor.paste() {
                    Ok(result) => result,
                    Err(error) => {
                        state.status = Some(error);
                        state.invalidate();
                        editor::KeyResult::Handled
                    }
                }
            } else {
                state.editor.handle_key(wparam, lparam)
            };
            match key_res {
                editor::KeyResult::Submit => {
                    if lparam & (1 << 30) == 0 && state.execution.pending_execute.is_none() {
                        if let Some(entry) = state.query.results.get(state.viewport.selected).cloned() {
                            state.execution.pending_execute = Some((state.query.generation, entry));
                            if PostMessageW(hwnd, WM_APP + 11, 0, 0) == 0 {
                                state.execution.pending_execute = None;
                            }
                        }
                    }
                    0
                }
                editor::KeyResult::TextChanged => {
                    state.close_image_preview();
                    state.result_navigation = false;
                    state.search_if_changed();
                    state.sync_height();
                    state.invalidate();
                    0
                }
                editor::KeyResult::Handled => {
                    state.close_image_preview();
                    state.result_navigation = false;
                    state.invalidate();
                    0
                }
                editor::KeyResult::NotHandled => {
                    if wparam as u16 == VK_UP {
                        state.move_selection(-1);
                    } else if wparam as u16 == VK_DOWN {
                        state.move_selection(1);
                    }
                    0
                }
            }
        }
        WM_CHAR if state.item_menu.is_some() => 0,
        WM_CHAR if wparam as u16 == VK_SPACE && state.consume_preview_space => {
            state.consume_preview_space = false;
            0
        }
        WM_CHAR
            if !state.editor.is_composing()
                && matches!(wparam as u16, VK_RETURN | VK_ESCAPE | VK_TAB) =>
        {
            0
        }
        // TranslateMessage also emits control characters for Ctrl+A/C/X.
        // They are not text input and must not blur the read-only body.
        WM_CHAR if state.panels.focused() && (wparam as u16) < 0x20 => 0,
        WM_CHAR => {
            if state.panels.focused() { state.focus_input(); }
            state.close_image_preview();
            if state.editor.handle_utf16(wparam as u16) {
                state.result_navigation = false;
                state.search_if_changed();
                state.sync_height();
                state.invalidate();
            }
            0
        }
        WM_IME_STARTCOMPOSITION | WM_IME_COMPOSITION | WM_IME_ENDCOMPOSITION => {
            state.close_image_preview();
            state.result_navigation = false;
            state.close_item_menu();
            let res = state.editor.handle_ime(hwnd, message, wparam, lparam);
            scope_keys::set_composing(state.editor.is_composing());
            state.search_if_changed();
            state.sync_height();
            state.invalidate();
            res
        }
        WM_MOUSEMOVE => {
            let x = state.logical((lparam as u16) as i16 as i32);
            let y = state.logical(((lparam >> 16) as u16) as i16 as i32);
            let pt = POINT { x, y };
            let mut mouse_changed = false;
            if state.editor.handle_mouse(message, pt) {
                mouse_changed = true;
            }
            if state.panels.handle_mouse_move(x as f32, y as f32) {
                mouse_changed = true;
            }
            if mouse_changed {
                state.invalidate();
            }

            if let Some((origin, _)) = &state.drag_press {
                let point = POINT {
                    x: (lparam as u16) as i16 as i32,
                    y: ((lparam >> 16) as u16) as i16 as i32,
                };
                if GetAsyncKeyState(VK_LBUTTON as i32) >= 0 {
                    state.cancel_pending_drag();
                } else if (point.x - origin.x).abs() >= GetSystemMetrics(SM_CXDRAG).max(1)
                    || (point.y - origin.y).abs() >= GetSystemMetrics(SM_CYDRAG).max(1)
                {
                    state.prepare_drag();
                    return 0;
                }
            }

            let scope_hovered = state.hit_scope(x, y);
            state.update_tooltip(x, y);
            let capsule_hovered = state
                .capsule_rect()
                .is_some_and(|[l, t, r, b]| (l..r).contains(&x) && (t..b).contains(&y));
            if state.scope_hovered != scope_hovered || state.capsule_hovered != capsule_hovered {
                state.scope_hovered = scope_hovered;
                state.capsule_hovered = capsule_hovered;
                state.invalidate();
            }
            if let Some(anchor) = state.scroll_drag {
                let previous = (state.viewport.first, state.viewport.selected);
                state
                    .viewport
                    .drag_thumb(y - anchor, state.row_top(), state.scroll_track_height());
                if previous != (state.viewport.first, state.viewport.selected) {
                    state.viewport_changed();
                }
                return 0;
            }
            let hovered = state.hit_result(x, y);
            let mut cursor: POINT = zeroed();
            if GetCursorPos(&mut cursor) != 0 {
                let position = Some((cursor.x, cursor.y));
                // Ignore synthetic moves from repainting or moving the window:
                // a stationary pointer must not undo keyboard navigation.
                if position != state.mouse_position
                    && state.item_menu.is_none()
                    && !state.panels.focused()
                {
                    let mut invalidated = false;
                    if let Some(index) = hovered {
                        if index != state.viewport.selected {
                            state.result_navigation = true;
                            state.viewport.selected = index;
                            state.viewport_changed();
                            invalidated = true;
                        }
                    }
                    if hovered != state.hovered {
                        state.hovered = hovered;
                        if !invalidated {
                            state.invalidate();
                        }
                    }
                }
                state.mouse_position = position;
            }
            let mut track = TRACKMOUSEEVENT {
                cbSize: size_of::<TRACKMOUSEEVENT>() as u32,
                dwFlags: TME_LEAVE,
                hwndTrack: hwnd,
                dwHoverTime: 0,
            };
            TrackMouseEvent(&mut track);
            0
        }
        WM_MOUSELEAVE => {
            if let Some(tooltip) = &state.tooltip { tooltip.hide(); }
            if state.scope_hovered || state.capsule_hovered {
                state.scope_hovered = false;
                state.capsule_hovered = false;
                state.invalidate();
            }
            let hovered = state.hovered.take();
            // Leaving can be caused by window geometry changes, too. Retain the
            // screen position so re-entry alone cannot take over the selection.
            if hovered.is_some() {
                state.invalidate();
            }
            0
        }
        WM_RBUTTONDOWN => {
            state.close_image_preview();
            state.result_navigation = false;
            state.close_item_menu();
            0
        }
        WM_RBUTTONUP => {
            let x = state.logical((lparam as u16) as i16 as i32);
            let y = state.logical(((lparam >> 16) as u16) as i16 as i32);
            if let Some(capsule) = state.capsule.clone() {
                if state
                    .capsule_rect()
                    .is_some_and(|[l, t, r, b]| (l..r).contains(&x) && (t..b).contains(&y))
                {
                    state.execute_capsule_action(&capsule.context_action);
                    state.capsule = None;
                    state.capsule_hovered = false;
                    state.layout_input();
                    if state.scope == scope::Scope::Plugin(capsule.plugin_id.clone())
                        || !state.query.query_text.trim().is_empty()
                    {
                        state.request_search();
                    }
                    state.invalidate();
                    return 0;
                }
            }
            let input = state.editor_bounds();
            if (input.left..input.right).contains(&(x as f32))
                && (input.top..input.bottom).contains(&(y as f32))
            {
                state.menu();
                return 0;
            }
            // Handle the actual button release directly rather than depending
            // on DefWindowProc to synthesize WM_CONTEXTMENU after focus/IME work.
            let mut point = POINT {
                x: lparam as u16 as i16 as i32,
                y: (lparam >> 16) as u16 as i16 as i32,
            };
            ClientToScreen(hwnd, &mut point);
            state.item_menu(point, false);
            0
        }
        WM_LBUTTONDOWN => {
            if state.dragging {
                return 0;
            }
            state.cancel_pending_drag();
            state.drag_release_consumed = false;
            if state.item_menu.is_some() {
                return 0;
            }
            let x = state.logical((lparam as u16) as i16 as i32);
            let y = state.logical(((lparam >> 16) as u16) as i16 as i32);
            let pt = POINT { x, y };

            // Editor hit testing and focus
            if state.editor.handle_mouse(message, pt) {
                SetCapture(hwnd);
                state.focus_input();
                state.close_image_preview();
                state.result_navigation = false;
                state.close_item_menu();
                state.invalidate();
                return 0;
            }

            // Panel hit testing
            if state.panels.handle_mouse_down(x as f32, y as f32) {
                SetCapture(hwnd);
                state.editor.set_focused(false);
                state.close_image_preview();
                state.close_item_menu();
                state.invalidate();
                return 0;
            }

            if state.hit_scope(x, y) || state.hit_feedback(x, y) || state.hit_empty_action(x, y).is_some() {
                return 0;
            }
            if state
                .capsule_rect()
                .is_some_and(|[l, t, r, b]| (l..r).contains(&x) && (t..b).contains(&y))
            {
                return 0;
            }
            if (state.width() - 20..state.width() - 2).contains(&x)
                && (state.row_top()..state.row_top() + state.scroll_track_height()).contains(&y)
            {
                if let Some(thumb) = state
                    .viewport
                    .thumb(state.row_top(), state.scroll_track_height())
                {
                    if (thumb.top..thumb.top + thumb.height).contains(&y) {
                        state.scroll_drag = Some(y - thumb.top);
                        SetCapture(hwnd);
                    } else {
                        state.viewport.scroll_rows(if y < thumb.top {
                            -(state.viewport.range().len() as i32)
                        } else {
                            state.viewport.range().len() as i32
                        });
                        state.viewport_changed();
                    }
                }
            }
            if let Some(index) = state.hit_result(x, y) {
                // Panel clicks are handled on release (focus/retry). Never arm
                // row dragging: its click fallback executes the copy action.
                if state.query.results[index].text_panel.is_some() {
                    return 0;
                }
                state.focus_input();
                if let Some(mut entry) = state.query.results.get(index).cloned() {
                    if let Some(source) = entry.drag_source() {
                        entry.drag = Some(source);
                        state.viewport.selected = index;
                        state.viewport_changed();
                        state.drag_press = Some((
                            POINT {
                                x: (lparam as u16) as i16 as i32,
                                y: ((lparam >> 16) as u16) as i16 as i32,
                            },
                            entry,
                        ));
                        SetCapture(hwnd);
                    }
                }
            }
            0
        }
        WM_LBUTTONDBLCLK => {
            let x = state.logical((lparam as u16) as i16 as i32);
            let y = state.logical(((lparam >> 16) as u16) as i16 as i32);
            let pt = POINT { x, y };
            if state.editor.handle_mouse(message, pt) {
                state.focus_input();
                state.invalidate();
                return 0;
            }
            0
        }
        WM_LBUTTONUP => {
            let x = state.logical((lparam as u16) as i16 as i32);
            let y = state.logical(((lparam >> 16) as u16) as i16 as i32);
            let pt = POINT { x, y };
            let mut mouse_up_changed = false;
            if state.editor.handle_mouse(message, pt) {
                mouse_up_changed = true;
            }
            if state.panels.handle_mouse_up(x as f32, y as f32) {
                mouse_up_changed = true;
            }
            if mouse_up_changed {
                if GetCapture() == hwnd { ReleaseCapture(); }
                state.invalidate();
            }

            if state.drag_release_consumed || state.drag_pending.is_some() || state.dragging {
                state.cancel_pending_drag();
                return 0;
            }
            if let Some((_, entry)) = state.drag_press.take() {
                ReleaseCapture();
                if state
                    .hit_result(x, y)
                    .and_then(|i| state.query.results.get(i))
                    .is_some_and(|current| current.key == entry.key)
                {
                    state.execute_entry(entry);
                }
                return 0;
            }
            if state.hit_feedback(x, y) {
                state.feedback_menu();
                return 0;
            }
            if let Some(secondary) = state.hit_empty_action(x, y) {
                state.activate_empty_action(secondary);
                return 0;
            }
            if state.hit_scope(x, y) {
                PostMessageW(hwnd, WM_APP + 14, 0, 0);
                return 0;
            }
            if let Some(capsule) = state.capsule.clone() {
                if state
                    .capsule_rect()
                    .is_some_and(|[l, t, r, b]| (l..r).contains(&x) && (t..b).contains(&y))
                {
                    if state.scope != scope::Scope::Plugin(capsule.plugin_id.clone())
                        && state.plugin_scopes.iter().any(|(id, _)| id == &capsule.plugin_id)
                    {
                        state.set_scope(scope::Scope::Plugin(capsule.plugin_id.clone()));
                    } else {
                        state.execute_capsule_action(&capsule.click_action);
                    }
                    return 0;
                }
            }
            if state.item_menu.is_some() {
                state.close_item_menu();
                return 0;
            }
            if state.scroll_drag.take().is_some() {
                ReleaseCapture();
                return 0;
            }
            if let Some(index) = state.hit_result(x, y) {
                state.viewport.selected = index;
                if let Some(panel) = state.query.results[index].text_panel.as_ref() {
                    let geometry = text_panel::Geometry::for_panel(
                        state.width(),
                        state.row_top() + state.viewport.row_top(index),
                        state.viewport.row_height(index),
                        panel,
                    );
                    match geometry.hit(x, y) {
                        text_panel::Hit::Retry => state.retry_panel(),
                        text_panel::Hit::Header(block) | text_panel::Hit::Content(block) => {
                            state
                                .panels
                                .focus_block(&state.query.results[index].key, block);
                        }
                        _ => {}
                    }
                    state.invalidate();
                } else {
                    state.execute();
                }
            } else {
                state.focus_input();
            }
            0
        }
        WM_MOUSEWHEEL if state.item_menu.is_some() => {
            state.move_menu_selection(if (wparam >> 16) as u16 as i16 > 0 {
                -1
            } else {
                1
            });
            0
        }
        WM_MOUSEWHEEL => {
            let delta = (wparam >> 16) as u16 as i16 as f32;
            let mut cursor = POINT {
                x: (lparam as u16) as i16 as i32,
                y: ((lparam >> 16) as u16) as i16 as i32,
            };
            ScreenToClient(hwnd, &mut cursor);
            let lx = state.logical(cursor.x) as f32;
            let ly = state.logical(cursor.y) as f32;
            if state.panels.scroll(lx, ly, -delta * 0.5) {
                state.invalidate();
                return 0;
            }
            let mut lines = 3u32;
            SystemParametersInfoW(SPI_GETWHEELSCROLLLINES, 0, &mut lines as *mut _ as _, 0);
            let previous = (state.viewport.first, state.viewport.selected);
            state
                .viewport
                .wheel((wparam >> 16) as u16 as i16 as i32, lines);
            if previous != (state.viewport.first, state.viewport.selected) {
                state.viewport_changed();
            }
            0
        }
        WM_CAPTURECHANGED => {
            state.editor.cancel_drag();
            state.panels.cancel_drag();
            if state.drag_press.take().is_some() {
                state.drag_release_consumed = true;
            }
            if state.drag_pending.is_some() && !state.dragging {
                state.cancel_pending_drag();
            }
            state.scroll_drag = None;
            0
        }
        WM_GETOBJECT => state
            .accessibility
            .get_object(wparam, lparam)
            .unwrap_or_else(|| DefWindowProcW(hwnd, message, wparam, lparam)),
        accessibility::ACTION => {
            if let Some((key, invoke)) = state.accessibility.resolve(wparam, lparam) {
                if let Some(key) = key {
                    if let Some(index) = state
                        .query
                        .results
                        .iter()
                        .position(|entry| entry.key == key)
                    {
                        state
                            .viewport
                            .update_total(state.query.results.len(), index);
                        state.viewport_changed();
                        state.focus_input();
                        if invoke {
                            state.execute();
                        }
                    }
                } else {
                    state.focus_input();
                }
            }
            0
        }

        RETRY_PANEL => {
            state.retry_panel();
            0
        }
        value if value == WM_APP + 10 => {
            state.move_selection(if wparam == 1 { 1 } else { -1 });
            0
        }
        value if value == WM_APP + 11 => {
            if let Some((generation, entry)) = state.execution.pending_execute.take() {
                if generation == state.query.generation {
                    state.execute_entry(entry);
                }
            }
            0
        }
        value if value == WM_APP + 12 => {
            if state.scope != scope::Scope::All {
                state.set_scope(scope::Scope::All);
            } else {
                state.hide();
            }
            0
        }
        value if value == WM_APP + 14 => {
            state.scope_menu();
            0
        }
        scope_keys::SELECT_SCOPE => {
            // Queued shortcuts must not act after focus changes or IME starts.
            if GetForegroundWindow() == hwnd
                && IsWindowVisible(hwnd) != 0
                && state.closing.is_none()
                && (GetFocus() == hwnd || state.editor.is_focused())
                && !state.editor.is_composing()
            {
                state.select_scope_shortcut(wparam as u16);
            }
            0
        }
        value if value == WM_APP + 13 => {
            state.result_navigation = true;
            state.viewport.page(if wparam == 1 { 1 } else { -1 });
            state.viewport_changed();
            0
        }
        _ => DefWindowProcW(hwnd, message, wparam, lparam),
    }
}
