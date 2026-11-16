//! Windows process startup, worker wiring and message loop.
use super::*;

pub fn run() -> Result<(), String> {
    struct HandleGuard(HANDLE);
    impl Drop for HandleGuard {
        fn drop(&mut self) {
            unsafe {
                CloseHandle(self.0);
            }
        }
    }
    unsafe {
        SetProcessDpiAwarenessContext(DPI_AWARENESS_CONTEXT_PER_MONITOR_AWARE_V2);
        let mutex = CreateMutexW(
            null(),
            0,
            wide("Local\\MiniLauncher.SingleInstance").as_ptr(),
        );
        if mutex.is_null() {
            return Err(std::io::Error::last_os_error().to_string());
        }
        let _mutex = HandleGuard(mutex);
        if GetLastError() == ERROR_ALREADY_EXISTS {
            let existing = FindWindowW(wide(CLASS_NAME).as_ptr(), null());
            if !existing.is_null() && !std::env::args().any(|arg| arg == "--background") {
                PostMessageW(existing, SHOW, 0, 0);
            }
            return Ok(());
        }
        let settings = config::load()?;
        // An invalid optional plugin shortcut must not prevent the launcher from opening.
        let (plugin_catalog, plugin_catalog_error) =
            match mini_launcher::plugin_catalog::RuntimeCatalog::read(&settings.plugins) {
                Ok(catalog) => (catalog, None),
                Err(error) => {
                    let catalog = mini_launcher::plugin_catalog::search_catalog(&settings.plugins);
                    (
                        mini_launcher::plugin_catalog::RuntimeCatalog {
                            routing: catalog.routing,
                            scopes: catalog.scopes,
                            direct_display: catalog.direct_display,
                            hotkeys: Vec::new(),
                        },
                        Some(error),
                    )
                }
            };
        let _ole = dragdrop::Ole::initialize()?;
        let instance = GetModuleHandleW(null());
        let class = WNDCLASSW {
            lpfnWndProc: Some(window_proc),
            hInstance: instance,
            hCursor: LoadCursorW(null_mut(), IDC_ARROW),
            hIcon: LoadIconW(instance, 1 as _),
            ..zeroed()
        };
        // Keep the UTF-16 class name alive across registration and creation.
        let class_name = wide(CLASS_NAME);
        let class = WNDCLASSW {
            lpszClassName: class_name.as_ptr(),
            ..class
        };
        if RegisterClassW(&class) == 0 {
            return Err(std::io::Error::last_os_error().to_string());
        }
        let hwnd = CreateWindowExW(
            WS_EX_TOOLWINDOW | WS_EX_TOPMOST | WS_EX_NOREDIRECTIONBITMAP,
            class_name.as_ptr(),
            wide("MiniLauncher").as_ptr(),
            WS_POPUP | WS_THICKFRAME | WS_CLIPCHILDREN,
            0,
            0,
            WIDTH,
            580,
            null_mut(),
            null_mut(),
            instance,
            null(),
        );
        if hwnd.is_null() {
            return Err(std::io::Error::last_os_error().to_string());
        }
        std::env::set_var("MINILAUNCHER_HOST_HWND", (hwnd as usize).to_string());
        let dark = is_dark(&settings.theme);
        let dpi = GetDpiForWindow(hwnd).max(96);
        let renderer = match gfx::WindowRenderer::new() {
            Ok(g) => g,
            Err(e) => {
                DestroyWindow(hwnd);
                return Err(format!("Direct2D 设备初始化失败：{e}"));
            }
        };
        let text_engine = match gfx::TextEngine::new(&renderer.device().dwrite_factory, dark) {
            Ok(t) => t,
            Err(e) => {
                DestroyWindow(hwnd);
                return Err(format!("DirectWrite 初始化失败：{e}"));
            }
        };
        let mut editor = editor::Editor::new(&text_engine, dpi);
        editor.set_placeholder("搜索内容");
        let image_renderer = match gfx::ImageRenderer::new() {
            Ok(r) => r,
            Err(e) => {
                DestroyWindow(hwnd);
                return Err(format!("ImageRenderer 初始化失败：{e}"));
            }
        };
        let (event_sender, events) = mpsc::channel();
        let (search_sender, search_receiver) = mpsc::channel::<SearchRequest>();
        let (icon_sender, icon_receiver) = mpsc::channel::<(String, i32)>();
        let (preview_sender, preview_receiver) = mpsc::channel::<rows::PreviewRequest>();
        let (history_sender, history_receiver) = mpsc::channel::<Arc<History>>();
        let thread_hwnd = hwnd as usize;
        let latest_generation = Arc::new(AtomicU64::new(0));
        let launch_sender = launch::start(event_sender.clone(), thread_hwnd);
        let sender = event_sender.clone();
        let emit: providers::Emit =
            Arc::new(move |update| send_event(&sender, thread_hwnd, UiEvent::Provider(update)));
        let everything = crate::everything::Client::start(emit.clone(), latest_generation.clone());
        let sender = event_sender.clone();
        let executed: crate::plugins::Executed = Arc::new(move |id, result| {
            send_event(&sender, thread_hwnd, UiEvent::PluginExecuted { id, result })
        });
        let plugins = crate::plugins::Client::start(
            settings.plugins.clone(),
            emit,
            executed,
            latest_generation.clone(),
        );
        let sender = event_sender.clone();
        let search_generation = latest_generation.clone();
        std::thread::spawn(move || {
            while let Ok(mut request) = search_receiver.recv() {
                while let Ok(newer) = search_receiver.try_recv() {
                    request = newer;
                }
                let cancelled = || search_generation.load(Ordering::Relaxed) != request.generation;
                if cancelled() {
                    continue;
                }
                let entries = if let Some(entry) = request.web_entry {
                    vec![entry]
                } else if request.file_only {
                    Vec::new()
                } else {
                    let Some(entries) = search::query_cancellable(
                        &request.index,
                        &request.text,
                        &request.history,
                        request.limit,
                        cancelled,
                    ) else {
                        continue;
                    };
                    entries
                };
                if cancelled() {
                    continue;
                }
                send_event(
                    &sender,
                    thread_hwnd,
                    UiEvent::Results {
                        generation: request.generation,
                        entries,
                    },
                );
            }
        });
        let sender = event_sender.clone();
        std::thread::spawn(move || {
            let _ = CoInitializeEx(null(), COINIT_APARTMENTTHREADED as u32);
            while let Ok((path, size)) = icon_receiver.recv() {
                let handle = icons::load_scaled(&path, size) as usize;
                if sender.send(UiEvent::Icon { path, size, handle }).is_err() {
                    if handle != 0 {
                        DestroyIcon(handle as HICON);
                    }
                    break;
                }
                PostMessageW(thread_hwnd as HWND, EVENT, 0, 0);
            }
            icons::clear_thread_cache();
            CoUninitialize();
        });
        let sender = event_sender.clone();
        let history_thread = std::thread::spawn(move || {
            while let Ok(mut history) = history_receiver.recv() {
                while let Ok(newer) = history_receiver.try_recv() {
                    history = newer;
                }
                if let Err(error) = config::save_history(&history) {
                    send_event(
                        &sender,
                        thread_hwnd,
                        UiEvent::Error(format!("历史记录保存失败：{error}")),
                    );
                }
            }
        });
        let dark = is_dark(&settings.theme);
        let sender = event_sender.clone();
        std::thread::spawn(move || {
            while let Ok(request) = preview_receiver.recv() {
                let image = rows::load(&request);
                send_event(&sender, thread_hwnd, UiEvent::Preview { preview: request.preview, dpi: request.dpi, image });
            }
        });
        let palette = Palette::for_backdrop(dark, theme::accent(), &settings.backdrop);
        let tray = tray::Icon::new(hwnd, instance, &settings.hotkey);
        let mut state = Box::new(State {
            accessibility: accessibility::Tree::new(hwnd, false),
            capsule: None,
            capsule_hovered: false,
            panels: text_panel::Panels::default(),
            execution: execution::Model::default(),
            query: query::Model::default(),
            text_search_request: 0,
            image_viewer: null_mut(),
            consume_preview_space: false,
            drag_press: None,
            drag_request: 0,
            drag_pending: None,
            prepared_drag: None,
            dragging: false,
            drag_release_consumed: false,
            drag_cancel: None,
            paste_target: None,
            paste_epoch: Arc::new(AtomicU64::new(0)),
            previews: HashMap::new(),
            pending_previews: HashSet::new(),
            preview_sender,
            scope: scope::Scope::All,
            plugin_scopes: plugin_catalog.scopes,
            plugin_routing: plugin_catalog.routing,
            plugin_shortcuts: plugin_catalog.hotkeys,
            direct_display: plugin_catalog.direct_display,
            scope_hovered: false,
            scope_keys: None,
            alt_preview: scope_keys::AltPreview::default(),
            hwnd,
            settings,
            index: Arc::new(Vec::new()),
            latest_generation,
            everything,
            plugins,
            history: Arc::new(config::load_history()),
            viewport: Viewport::default(),
            row_measurements: rows::Measurements::default(),
            hovered: None,
            mouse_position: None,
            scroll_drag: None,
            item_menu: None,
            dismissing_menu: None,
            tooltip: None,
            result_navigation: false,
            visible_rows: 6,
            dpi,
            positioning: false,
            dark,
            backdrop_active: false,
            appearance: None,
            palette,
            text_engine,
            renderer: std::rc::Rc::new(std::cell::RefCell::new(renderer)),
            render_device_id: 0,
            editor,
            image_renderer,
            d2d_icons: HashMap::new(),
            events,
            event_sender,
            search_sender,
            launch_sender,
            icon_sender,
            history_sender,
            icons: HashMap::new(),
            pending_icons: HashSet::new(),
            tray,
            taskbar_created: RegisterWindowMessageW(wide("TaskbarCreated").as_ptr()),
            status: None,
            config_error: None,
            config_watch: None,
            hotkey_id: HOTKEY,
            hotkey_registered: false,
            plugin_hotkeys: hotkey::Registry::new(hwnd),
            selection_request: 0,
            opening: None,
            closing: None,
            selection: animation::Tween::fixed(ROW_TOP),
            selection_first: 0,
            height_animation: None,
            frame_clock: animation::FrameClock::new(),
        });
        SetWindowLongPtrW(hwnd, GWLP_USERDATA, &mut *state as *mut State as isize);
        SetTimer(hwnd, 1, 500, None);
        state.update_theme(true);
        if !register_hotkey(hwnd, state.hotkey_id, &state.settings.hotkey) {
            SetWindowLongPtrW(hwnd, GWLP_USERDATA, 0);
            DestroyWindow(hwnd);
            return Err(format!(
                "快捷键 {} 已被其他应用占用。\n请修改 {} 中的 hotkey 后重试。",
                state.settings.hotkey,
                config::settings_path().display()
            ));
        }
        state.hotkey_registered = true;
        if let Err(error) = state
            .plugin_hotkeys
            .configure(state.plugin_shortcuts.clone(), &state.settings.hotkey)
        {
            state.config_error = Some(error.clone());
            state.status = Some(error);
        }
        state.tray.add();
        if let Some(error) = plugin_catalog_error {
            state.config_error = Some(error.clone());
            state.status = Some(error);
        }
        if let Err(error) = startup::sync(state.settings.start_at_login) {
            state.config_error = Some(error);
            state.status = state.config_error.clone();
        }
        let sender = state.event_sender.clone();
        match config_watch::Watcher::start(Arc::new(move |settings| {
            send_event(
                &sender,
                thread_hwnd,
                UiEvent::Settings(settings.map(Box::new)),
            )
        })) {
            Ok(watcher) => state.config_watch = Some(watcher),
            Err(error) => state.reject_config(error, false),
        }
        state.start_index();
        if !std::env::args().any(|arg| arg == "--background") {
            state.show();
        }
        let mut message: MSG = zeroed();
        loop {
            // Wait on animation frames and messages together. Unlike WM_TIMER,
            // the frame signal is not delayed by queued provider/icon updates.
            let handle = state.frame_clock.handle;
            let count = u32::from(!handle.is_null());
            let result = MsgWaitForMultipleObjectsEx(
                count,
                &handle,
                INFINITE,
                QS_ALLINPUT,
                MWMO_INPUTAVAILABLE,
            );
            if result == WAIT_FAILED {
                break;
            }
            if count != 0 && result == WAIT_OBJECT_0 {
                state.animate();
                UpdateWindow(hwnd);
            }
            // Bound the batch so a busy provider cannot starve the next frame.
            for _ in 0..32 {
                if PeekMessageW(&mut message, null_mut(), 0, 0, PM_REMOVE) == 0 {
                    break;
                }
                if message.message == WM_QUIT {
                    break;
                }
                TranslateMessage(&message);
                DispatchMessageW(&message);
            }
            if message.message == WM_QUIT {
                break;
            }
        }
        drop(state);
        let _ = history_thread.join();
        Ok(())
    }
}
