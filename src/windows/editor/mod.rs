pub mod ime;
pub mod layout;
pub mod model;
pub mod selection;
pub mod view;

use std::ptr::null_mut;
use std::time::Instant;

use ime::{ImeAction, ImeHandler};
use layout::{CaretBounds, EditorLayout};
use model::{EnterAction, TextModel};
use selection::Selection;
use view::EditorView;

use crate::windows::gfx::{FontRole, TextEngine};
use crate::windows::Palette;

use windows::Win32::Graphics::Direct2D::Common::D2D_RECT_F;
use windows::Win32::Graphics::Direct2D::ID2D1DeviceContext;
use windows::Win32::Graphics::DirectWrite::IDWriteTextLayout;

use windows_sys::Win32::Foundation::{HWND, LPARAM, LRESULT, POINT, WPARAM};
use windows_sys::Win32::System::DataExchange::{
    CloseClipboard, EmptyClipboard, GetClipboardData, OpenClipboard, SetClipboardData,
};
use windows_sys::Win32::System::Memory::{GlobalAlloc, GlobalLock, GlobalUnlock, GMEM_MOVEABLE};
use windows_sys::Win32::UI::Input::KeyboardAndMouse::*;
use windows_sys::Win32::UI::WindowsAndMessaging::{
    WM_LBUTTONDBLCLK, WM_LBUTTONDOWN, WM_LBUTTONUP, WM_MOUSEMOVE,
};

const CF_UNICODETEXT: u32 = 13;

/// Result of processing a keyboard event in the editor.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum KeyResult {
    /// The editor handled the key (e.g. navigation, selection, undo).
    Handled,
    /// The key was not handled by the editor; caller should process it.
    NotHandled,
    /// Text content was modified as a result of the key.
    TextChanged,
    /// Enter was pressed without Shift: submit the input/query.
    Submit,
}

/// Direct2D + DirectWrite self-drawing text editor.
pub struct Editor {
    attachments: Vec<(String, mini_launcher::input_attachment::InputImage)>,
    model: TextModel,
    selection: Selection,
    layout: EditorLayout,
    ime: ImeHandler,
    text_engine: TextEngine,
    dpi: u32,
    bounds: D2D_RECT_F,
    placeholder: String,
    placeholder_layout: Option<IDWriteTextLayout>,
    layout_error: Option<windows::core::Error>,
    placeholder_error: Option<windows::core::Error>,
    is_focused: bool,
    show_caret: bool,
    last_blink: Instant,
    is_dragging: bool,
    pending_surrogate: Option<u16>,
}

impl Editor {
    pub fn line_height(&self) -> i32 {
        self.layout.default_line_height().ceil() as i32
    }

    pub fn set_text_engine(&mut self, engine: &TextEngine) {
        self.layout = EditorLayout::new(&engine.factory, engine.format_for(FontRole::Input));
        self.text_engine = engine.clone();
        self.placeholder_layout = None;
        self.sync_layout();
        self.rebuild_placeholder();
    }

    /// Creates a new self-drawing text editor with the given text engine and DPI.
    pub fn new(text_engine: &TextEngine, dpi: u32) -> Self {
        let format = text_engine.format_for(FontRole::Input);
        let layout = EditorLayout::new(&text_engine.factory, format);

        let mut editor = Self {
            attachments: Vec::new(),
            model: TextModel::new(),
            selection: Selection::default(),
            layout,
            ime: ImeHandler::new(),
            text_engine: text_engine.clone(),
            dpi,
            bounds: D2D_RECT_F::default(),
            placeholder: String::new(),
            placeholder_layout: None,
            layout_error: None,
            placeholder_error: None,
            is_focused: true,
            show_caret: true,
            last_blink: Instant::now(),
            is_dragging: false,
            pending_surrogate: None,
        };
        editor.sync_layout();
        editor
    }

    /// Returns the current text content of the editor.
    #[inline]
    pub fn text(&self) -> &str {
        self.model.as_str()
    }

    /// Returns query text, omitting pinyin separators from the active IME composition.
    pub fn effective_text(&self) -> String {
        if self.ime.is_composing() && !self.ime.composition_string().is_empty() {
            let start_byte = self.model.utf16_to_byte_offset(self.selection.start());
            let end_byte = self.model.utf16_to_byte_offset(self.selection.end());
            let before = &self.model.as_str()[..start_byte];
            let after = &self.model.as_str()[end_byte..];
            let comp_str = self.ime.composition_string();
            let mut combined = String::with_capacity(before.len() + comp_str.len() + after.len());
            combined.push_str(before);
            combined.extend(comp_str.chars().filter(|&ch| ch != '\''));
            combined.push_str(after);
            combined
        } else {
            self.model.as_str().to_string()
        }
    }

    /// Sets the text content of the editor, enforcing max 4 lines and updating layout.
    pub fn set_text(&mut self, text: &str) {
        self.attachments.clear();
        self.pending_surrogate = None;
        self.model.set_text(text, &mut self.selection);
        self.sync_layout();
        self.reset_blink();
    }

    /// Visible image tokens participate in selection, deletion and undo as text.
    pub fn query_input(&self) -> (String, Vec<mini_launcher::input_attachment::InputImage>) {
        let mut text = self.effective_text();
        let mut images = Vec::new();
        for (token, image) in &self.attachments {
            if text.contains(token) {
                text = text.replace(token, "");
                images.push(image.clone());
            }
        }
        (text, images)
    }

    pub fn paste(&mut self) -> Result<KeyResult, String> {
        if let Some(image) = unsafe { mini_launcher::clipboard_image::read_clipboard_image()? } {
            return self.insert_image(&image);
        } else if let Some(text) = unsafe { get_clipboard_text() } {
            if self.model.insert_text(&text, &mut self.selection) {
                self.sync_layout();
                self.reset_blink();
                return Ok(KeyResult::TextChanged);
            }
        }
        Ok(KeyResult::Handled)
    }

    fn insert_image(
        &mut self,
        image: &mini_launcher::clipboard_image::Image,
    ) -> Result<KeyResult, String> {
        if self.attachments.len() >= 8 {
            return Err("一次输入最多粘贴 8 张图片，清空输入后可重新添加".into());
        }
        let attachment = mini_launcher::input_attachment::InputImage::new(image)?;
        let token = format!(
            "[图片 {} · {}×{}]",
            self.attachments.len() + 1,
            image.width,
            image.height
        );
        if self.model.insert_text(&token, &mut self.selection) {
            self.attachments.push((token, attachment));
            self.sync_layout();
            self.reset_blink();
            return Ok(KeyResult::TextChanged);
        }
        Ok(KeyResult::Handled)
    }

    /// Returns the number of visible lines (1..=4).
    #[inline]
    pub fn visible_lines(&self) -> i32 {
        self.model.visible_lines()
    }

    /// Checks if the caret is at the end of the text buffer and selection is empty.
    #[inline]
    pub fn caret_at_end(&self) -> bool {
        self.selection.is_empty() && self.selection.caret == self.model.len_utf16()
    }

    /// Selects all text in the editor.
    pub fn select_all(&mut self) {
        self.selection.select_all(self.model.len_utf16());
        self.reset_blink();
    }

    /// Sets the layout bounds of the editor.
    pub fn set_rect(&mut self, left: f32, top: f32, width: f32, height: f32) {
        let bounds = D2D_RECT_F {
            left,
            top,
            right: left + width,
            bottom: top + height,
        };
        self.update_bounds(bounds);
    }

    fn update_bounds(&mut self, bounds: D2D_RECT_F) {
        if bounds == self.bounds {
            return;
        }
        let resized = (bounds.right - bounds.left, bounds.bottom - bounds.top)
            != (
                self.bounds.right - self.bounds.left,
                self.bounds.bottom - self.bounds.top,
            );
        self.bounds = bounds;
        // Moving the editor changes only the drawing origin, not text geometry.
        if resized {
            self.sync_layout();
            if !self.placeholder.is_empty() {
                self.rebuild_placeholder();
            }
        }
    }

    /// Sets the placeholder text shown when the editor is empty.
    pub fn set_placeholder(&mut self, placeholder: &str) {
        if self.placeholder == placeholder
            && self.placeholder_layout.is_some()
            && self.placeholder_error.is_none()
        {
            return;
        }
        self.placeholder = placeholder.to_string();
        self.rebuild_placeholder();
    }

    fn rebuild_placeholder(&mut self) {
        let width = (self.bounds.right - self.bounds.left).max(100.0);
        let height = (self.bounds.bottom - self.bounds.top).max(28.0);
        let result = self.text_engine.create_layout(
            &self.placeholder,
            self.text_engine.format_for(FontRole::Input),
            width,
            height,
        );
        match result {
            Ok(layout) => {
                self.placeholder_layout = Some(layout);
                self.placeholder_error = None;
            }
            Err(error) => {
                self.placeholder_layout = None;
                self.placeholder_error = Some(error);
            }
        }
    }

    /// Sets focus state of the editor.
    pub fn cancel_drag(&mut self) { self.is_dragging = false; }

    pub fn set_focused(&mut self, focused: bool) {
        self.is_focused = focused;
        if focused {
            self.reset_blink();
        }
    }

    #[inline]
    pub fn is_focused(&self) -> bool {
        self.is_focused
    }

    /// Returns whether IME composition is currently active.
    #[inline]
    pub fn is_composing(&self) -> bool {
        self.ime.is_composing()
    }

    /// Updates DPI scaling.
    pub fn set_dpi(&mut self, dpi: u32) {
        self.dpi = dpi;
        self.sync_layout();
    }

    fn reset_blink(&mut self) {
        self.show_caret = true;
        self.last_blink = Instant::now();
    }

    /// Synchronizes the DirectWrite layout with current model state.
    fn sync_layout(&mut self) {
        let width = (self.bounds.right - self.bounds.left).max(100.0);
        let height = (self.bounds.bottom - self.bounds.top).max(28.0);

        let (display_text, comp_range) = if self.ime.is_composing()
            && !self.ime.composition_string().is_empty()
        {
            let start_byte = self.model.utf16_to_byte_offset(self.selection.start());
            let end_byte = self.model.utf16_to_byte_offset(self.selection.end());
            let before = &self.model.as_str()[..start_byte];
            let after = &self.model.as_str()[end_byte..];
            let comp_str = self.ime.composition_string();
            let mut combined = String::with_capacity(before.len() + comp_str.len() + after.len());
            combined.push_str(before);
            combined.push_str(comp_str);
            combined.push_str(after);
            let comp_u16_len = comp_str.encode_utf16().count();
            (combined, Some((self.selection.start(), comp_u16_len)))
        } else {
            (self.model.as_str().to_string(), None)
        };

        self.layout_error = self
            .layout
            .update(&display_text, comp_range, width, height)
            .err();
    }

    /// Handles keyboard messages (`WM_KEYDOWN`, `WM_SYSKEYDOWN`).
    pub fn handle_key(&mut self, wparam: WPARAM, _lparam: LPARAM) -> KeyResult {
        let key = wparam as u16;
        let ctrl = unsafe { GetKeyState(VK_CONTROL as i32) < 0 };
        let shift = unsafe { GetKeyState(VK_SHIFT as i32) < 0 };
        let alt = unsafe { GetKeyState(VK_MENU as i32) < 0 };

        // If Alt is held without Ctrl, don't consume (launcher menu/scope shortcut)
        if alt && !ctrl {
            return KeyResult::NotHandled;
        }

        match key {
            VK_RETURN => {
                let action = self.model.handle_enter(&mut self.selection, shift);
                match action {
                    EnterAction::Submit => KeyResult::Submit,
                    EnterAction::NewlineInserted => {
                        self.sync_layout();
                        self.reset_blink();
                        KeyResult::TextChanged
                    }
                    EnterAction::MaxLinesBlocked => KeyResult::Handled,
                }
            }
            VK_BACK => {
                let changed = if ctrl {
                    self.model.delete_word_backward(&mut self.selection)
                } else {
                    self.model.delete_char_backward(&mut self.selection)
                };
                if changed {
                    self.sync_layout();
                    self.reset_blink();
                    KeyResult::TextChanged
                } else {
                    KeyResult::Handled
                }
            }
            VK_DELETE => {
                let changed = if ctrl {
                    self.model.delete_word_forward(&mut self.selection)
                } else {
                    self.model.delete_char_forward(&mut self.selection)
                };
                if changed {
                    self.sync_layout();
                    self.reset_blink();
                    KeyResult::TextChanged
                } else {
                    KeyResult::Handled
                }
            }
            VK_LEFT => {
                let target = if ctrl {
                    selection::find_word_left(self.model.as_str(), self.selection.caret)
                } else if shift {
                    let prev_char = self.model.utf16_to_char_offset(self.selection.caret);
                    if prev_char > 0 {
                        self.model.char_to_utf16_offset(prev_char - 1)
                    } else {
                        0
                    }
                } else if !self.selection.is_empty() {
                    self.selection.start()
                } else {
                    let prev_char = self.model.utf16_to_char_offset(self.selection.caret);
                    if prev_char > 0 {
                        self.model.char_to_utf16_offset(prev_char - 1)
                    } else {
                        0
                    }
                };

                if shift {
                    self.selection.extend_to(target);
                } else {
                    self.selection.collapse_to(target);
                }
                self.reset_blink();
                KeyResult::Handled
            }
            VK_RIGHT => {
                let total_u16 = self.model.len_utf16();
                let target = if ctrl {
                    selection::find_word_right(self.model.as_str(), self.selection.caret)
                } else if shift {
                    let next_char = self.model.utf16_to_char_offset(self.selection.caret);
                    if next_char < self.model.len_chars() {
                        self.model.char_to_utf16_offset(next_char + 1)
                    } else {
                        total_u16
                    }
                } else if !self.selection.is_empty() {
                    self.selection.end()
                } else {
                    let next_char = self.model.utf16_to_char_offset(self.selection.caret);
                    if next_char < self.model.len_chars() {
                        self.model.char_to_utf16_offset(next_char + 1)
                    } else {
                        total_u16
                    }
                };

                if shift {
                    self.selection.extend_to(target);
                } else {
                    self.selection.collapse_to(target);
                }
                self.reset_blink();
                KeyResult::Handled
            }
            VK_HOME => {
                let target = if ctrl {
                    0
                } else {
                    // Line start
                    let caret_byte = self.model.utf16_to_byte_offset(self.selection.caret);
                    let line_start_byte = self.model.as_str()[..caret_byte]
                        .rfind('\n')
                        .map(|i| i + 1)
                        .unwrap_or(0);
                    self.model.byte_to_utf16_offset(line_start_byte)
                };

                if shift {
                    self.selection.extend_to(target);
                } else {
                    self.selection.collapse_to(target);
                }
                self.reset_blink();
                KeyResult::Handled
            }
            VK_END => {
                let total_u16 = self.model.len_utf16();
                let target = if ctrl {
                    total_u16
                } else {
                    // Line end
                    let caret_byte = self.model.utf16_to_byte_offset(self.selection.caret);
                    let line_end_byte = self.model.as_str()[caret_byte..]
                        .find('\n')
                        .map(|i| caret_byte + i)
                        .unwrap_or_else(|| self.model.len_bytes());
                    self.model.byte_to_utf16_offset(line_end_byte)
                };

                if shift {
                    self.selection.extend_to(target);
                } else {
                    self.selection.collapse_to(target);
                }
                self.reset_blink();
                KeyResult::Handled
            }
            VK_UP => {
                // If multiline, try navigating up a line; if on top line, pass to parent
                if let (Ok(caret_bounds), Ok(metrics)) = (
                    self.layout.hit_test_text_position(self.selection.caret),
                    self.layout.get_metrics(),
                ) {
                    if caret_bounds.y > metrics.top + 2.0 {
                        let target_y = caret_bounds.y - caret_bounds.height * 0.5;
                        if let Ok(hit) = self.layout.hit_test_point(caret_bounds.x, target_y) {
                            if shift {
                                self.selection.extend_to(hit.text_position);
                            } else {
                                self.selection.collapse_to(hit.text_position);
                            }
                            self.reset_blink();
                            return KeyResult::Handled;
                        }
                    }
                }
                KeyResult::NotHandled
            }
            VK_DOWN => {
                // If multiline, try navigating down a line; if on bottom line, pass to parent
                if let Ok(caret_bounds) = self.layout.hit_test_text_position(self.selection.caret) {
                    if let Ok(metrics) = self.layout.get_metrics() {
                        if caret_bounds.y + caret_bounds.height < metrics.top + metrics.height - 2.0
                        {
                            let target_y = caret_bounds.y + caret_bounds.height * 1.5;
                            if let Ok(hit) = self.layout.hit_test_point(caret_bounds.x, target_y) {
                                if shift {
                                    self.selection.extend_to(hit.text_position);
                                } else {
                                    self.selection.collapse_to(hit.text_position);
                                }
                                self.reset_blink();
                                return KeyResult::Handled;
                            }
                        }
                    }
                }
                KeyResult::NotHandled
            }
            // Ctrl+A: Select All
            0x41 if ctrl => {
                self.selection.select_all(self.model.len_utf16());
                self.reset_blink();
                KeyResult::Handled
            }
            // Ctrl+C: Copy
            0x43 if ctrl => {
                if !self.selection.is_empty() {
                    let start_byte = self.model.utf16_to_byte_offset(self.selection.start());
                    let end_byte = self.model.utf16_to_byte_offset(self.selection.end());
                    let selected_text = &self.model.as_str()[start_byte..end_byte];
                    unsafe {
                        set_clipboard_text(selected_text);
                    }
                }
                KeyResult::Handled
            }
            // Ctrl+X: Cut
            0x58 if ctrl => {
                if !self.selection.is_empty() {
                    let start_byte = self.model.utf16_to_byte_offset(self.selection.start());
                    let end_byte = self.model.utf16_to_byte_offset(self.selection.end());
                    let selected_text = &self.model.as_str()[start_byte..end_byte];
                    unsafe {
                        set_clipboard_text(selected_text);
                    }
                    if self.model.delete_selection(&mut self.selection) {
                        self.sync_layout();
                        self.reset_blink();
                        return KeyResult::TextChanged;
                    }
                }
                KeyResult::Handled
            }
            // Paste is dispatched by the window so failures can be shown.
            0x56 if ctrl => self.paste().unwrap_or(KeyResult::Handled),
            // Ctrl+Z: Undo
            0x5A if ctrl && !shift => {
                if self.model.undo(&mut self.selection) {
                    self.sync_layout();
                    self.reset_blink();
                    KeyResult::TextChanged
                } else {
                    KeyResult::Handled
                }
            }
            // Ctrl+Y or Ctrl+Shift+Z: Redo
            0x59 if ctrl => {
                if self.model.redo(&mut self.selection) {
                    self.sync_layout();
                    self.reset_blink();
                    KeyResult::TextChanged
                } else {
                    KeyResult::Handled
                }
            }
            0x5A if ctrl && shift => {
                if self.model.redo(&mut self.selection) {
                    self.sync_layout();
                    self.reset_blink();
                    KeyResult::TextChanged
                } else {
                    KeyResult::Handled
                }
            }
            _ => KeyResult::NotHandled,
        }
    }

    /// Handles character input messages (`WM_CHAR`).
    pub fn handle_utf16(&mut self, unit: u16) -> bool {
        match unit {
            0xd800..=0xdbff => {
                self.pending_surrogate = Some(unit);
                false
            }
            0xdc00..=0xdfff => {
                let Some(high) = self.pending_surrogate.take() else {
                    return false;
                };
                let scalar =
                    0x10000 + ((u32::from(high) - 0xd800) << 10) + u32::from(unit) - 0xdc00;
                self.handle_char(char::from_u32(scalar).unwrap())
            }
            _ => {
                self.pending_surrogate = None;
                self.handle_char(char::from_u32(u32::from(unit)).unwrap())
            }
        }
    }

    fn handle_char(&mut self, ch: char) -> bool {
        // Discard control characters (< 32, e.g. Backspace, Escape, Tab, Return)
        if ch < ' ' || ch == '\r' || ch == '\n' {
            return false;
        }

        let mut buf = [0u8; 4];
        let s = ch.encode_utf8(&mut buf);
        if self.model.insert_text(s, &mut self.selection) {
            self.sync_layout();
            self.reset_blink();
            true
        } else {
            false
        }
    }

    /// Handles mouse events (`WM_LBUTTONDOWN`, `WM_LBUTTONDBLCLK`, `WM_MOUSEMOVE`, `WM_LBUTTONUP`).
    pub fn handle_mouse(&mut self, msg: u32, pt: POINT) -> bool {
        let inside = pt.x as f32 >= self.bounds.left
            && pt.x as f32 <= self.bounds.right
            && pt.y as f32 >= self.bounds.top
            && pt.y as f32 <= self.bounds.bottom;

        if !inside && !self.is_dragging {
            return false;
        }

        let local_x = pt.x as f32 - self.bounds.left;
        let local_y = pt.y as f32 - self.bounds.top;
        let hit =
            self.layout
                .hit_test_point(local_x, local_y)
                .unwrap_or(layout::HitTestPointResult {
                    text_position: 0,
                    is_trailing: false,
                    is_inside: false,
                });

        match msg {
            WM_LBUTTONDOWN => {
                self.is_focused = true;
                self.is_dragging = true;
                let shift = unsafe { GetKeyState(VK_SHIFT as i32) < 0 };
                self.selection.handle_click(hit.text_position, shift);
                self.reset_blink();
                true
            }
            WM_LBUTTONDBLCLK => {
                self.selection
                    .handle_double_click(self.model.as_str(), hit.text_position);
                self.reset_blink();
                true
            }
            WM_MOUSEMOVE => {
                if self.is_dragging {
                    self.selection.handle_drag(hit.text_position);
                    self.reset_blink();
                    true
                } else {
                    false
                }
            }
            WM_LBUTTONUP => {
                if self.is_dragging {
                    self.is_dragging = false;
                    true
                } else {
                    false
                }
            }
            _ => false,
        }
    }

    /// Handles IMM32 messages (`WM_IME_STARTCOMPOSITION`, `WM_IME_COMPOSITION`, `WM_IME_ENDCOMPOSITION`).
    pub fn handle_ime(&mut self, hwnd: HWND, msg: u32, wparam: WPARAM, lparam: LPARAM) -> LRESULT {
        let caret_offset = if self.ime.is_composing() {
            self.selection.start() + self.ime.composition_string().encode_utf16().count()
        } else {
            self.selection.caret
        };

        let caret_bounds =
            self.layout
                .hit_test_text_position(caret_offset)
                .unwrap_or(CaretBounds {
                    x: 0.0,
                    y: 0.0,
                    width: 1.0,
                    height: self.layout.default_line_height(),
                });

        let caret_pos = POINT {
            x: ((self.bounds.left + caret_bounds.x) * self.dpi as f32 / 96.0).round() as i32,
            y: ((self.bounds.top + caret_bounds.y + caret_bounds.height) * self.dpi as f32 / 96.0).round() as i32,
        };

        let (result, action) = unsafe {
            self.ime
                .handle_message(hwnd, msg, wparam, lparam, caret_pos)
        };

        match action {
            ImeAction::Commit(text) => {
                self.model.insert_text(&text, &mut self.selection);
                self.sync_layout();
                self.reset_blink();
            }
            ImeAction::CompositionChanged | ImeAction::CompositionEnded => {
                self.sync_layout();
                self.reset_blink();
            }
            ImeAction::None => {}
        }

        result
    }

    /// Draws the text editor into the Direct2D device context within the given bounds.
    pub fn draw(
        &mut self,
        context: &ID2D1DeviceContext,
        bounds: D2D_RECT_F,
        palette: &Palette,
    ) -> windows::core::Result<()> {
        // Update bounds and layout if resized
        self.update_bounds(bounds);

        // Input handlers retain layout failures; retry and surface them through the frame result.
        if self.layout_error.is_some() {
            self.sync_layout();
        }
        if let Some(error) = &self.layout_error {
            return Err(error.clone());
        }
        if self.placeholder_error.is_some() {
            self.rebuild_placeholder();
        }
        if let Some(error) = &self.placeholder_error {
            return Err(error.clone());
        }

        // Caret blinking (every ~500ms)
        if self.is_focused && self.last_blink.elapsed().as_millis() >= 500 {
            self.show_caret = !self.show_caret;
            self.last_blink = Instant::now();
        }

        unsafe {
            EditorView::draw(
                context,
                self.bounds,
                palette,
                &self.model,
                &self.selection,
                &self.layout,
                &self.ime,
                self.placeholder_layout.as_ref(),
                self.is_focused,
                self.show_caret,
            )
        }
    }
}

// --- Windows Clipboard Helpers ---

unsafe fn get_clipboard_text() -> Option<String> {
    if OpenClipboard(null_mut()) == 0 {
        return None;
    }
    let handle = GetClipboardData(CF_UNICODETEXT);
    if handle.is_null() {
        CloseClipboard();
        return None;
    }
    let ptr = GlobalLock(handle) as *const u16;
    if ptr.is_null() {
        CloseClipboard();
        return None;
    }
    let mut len = 0;
    while *ptr.add(len) != 0 {
        len += 1;
    }
    let slice = std::slice::from_raw_parts(ptr, len);
    let s = String::from_utf16_lossy(slice);
    GlobalUnlock(handle);
    CloseClipboard();
    Some(s)
}

pub(crate) unsafe fn set_clipboard_text(text: &str) -> bool {
    if OpenClipboard(null_mut()) == 0 {
        return false;
    }
    EmptyClipboard();
    let utf16: Vec<u16> = text.encode_utf16().chain(std::iter::once(0)).collect();
    let bytes = utf16.len() * std::mem::size_of::<u16>();
    let handle = GlobalAlloc(GMEM_MOVEABLE, bytes);
    if handle.is_null() {
        CloseClipboard();
        return false;
    }
    let ptr = GlobalLock(handle) as *mut u16;
    if !ptr.is_null() {
        std::ptr::copy_nonoverlapping(utf16.as_ptr(), ptr, utf16.len());
        GlobalUnlock(handle);
        SetClipboardData(CF_UNICODETEXT, handle);
    }
    CloseClipboard();
    true
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn unchanged_placeholder_and_position_reuse_layouts_but_resize_rebuilds_them() {
        use windows::core::Interface;
        let engine = TextEngine::create(true).unwrap();
        let mut editor = Editor::new(&engine, 96);
        editor.set_rect(10.0, 20.0, 400.0, 80.0);
        editor.set_text("中文😀");
        editor.set_placeholder("搜索");
        let text = editor.layout.raw_layout().unwrap().clone();
        let placeholder = editor.placeholder_layout.clone().unwrap();
        editor.set_placeholder("搜索");
        assert_eq!(
            placeholder.as_raw(),
            editor.placeholder_layout.as_ref().unwrap().as_raw()
        );
        editor.set_rect(20.0, 30.0, 400.0, 80.0);
        assert_eq!(
            placeholder.as_raw(),
            editor.placeholder_layout.as_ref().unwrap().as_raw()
        );
        assert_eq!(text.as_raw(), editor.layout.raw_layout().unwrap().as_raw());
        editor.set_rect(20.0, 30.0, 200.0, 80.0);
        assert_ne!(text.as_raw(), editor.layout.raw_layout().unwrap().as_raw());
    }

    #[test]
    fn image_tokens_follow_selection_undo_and_clear_without_entering_query_text() {
        let engine = TextEngine::create(true).unwrap();
        let mut editor = Editor::new(&engine, 96);
        editor.set_text("note /add diagram ");
        let image = mini_launcher::clipboard_image::Image {
            width: 2,
            height: 2,
            pixels: vec![255; 16],
        };
        assert_eq!(editor.insert_image(&image).unwrap(), KeyResult::TextChanged);
        let (text, attachments) = editor.query_input();
        assert_eq!(text, "note /add diagram ");
        assert_eq!(attachments.len(), 1);
        let path = attachments[0].path.clone();
        editor.select_all();
        assert!(editor.model.delete_selection(&mut editor.selection));
        assert!(editor.query_input().1.is_empty());
        assert!(editor.model.undo(&mut editor.selection));
        assert_eq!(editor.query_input().1.len(), 1);
        editor.set_text("");
        assert!(editor.query_input().1.is_empty());
        // An in-flight query retains the file after the editor releases it.
        assert!(path.exists());
        drop(attachments);
        assert!(!path.exists());
    }

    #[test]
    fn vertical_keys_respect_centered_text_edges() {
        let engine = TextEngine::create(true).unwrap();
        let mut editor = Editor::new(&engine, 144);
        editor.set_rect(128.0, 15.0, 480.0, 28.0);
        editor.set_text("搜索 test");
        let caret = editor.selection.caret;
        // Both directions must reach the result list for a centered single line.
        assert_eq!(editor.handle_key(VK_UP as usize, 0), KeyResult::NotHandled);
        assert_eq!(
            editor.handle_key(VK_DOWN as usize, 0),
            KeyResult::NotHandled
        );
        assert_eq!(editor.selection.caret, caret);

        editor.set_rect(128.0, 15.0, 480.0, 100.0);
        editor.set_text("first\nsecond");
        editor.selection.collapse_to(0);
        assert_eq!(editor.handle_key(VK_UP as usize, 0), KeyResult::NotHandled);
        assert_eq!(editor.handle_key(VK_DOWN as usize, 0), KeyResult::Handled);
        assert!(editor.selection.caret >= 6);
        assert_eq!(
            editor.handle_key(VK_DOWN as usize, 0),
            KeyResult::NotHandled
        );
        assert_eq!(editor.handle_key(VK_UP as usize, 0), KeyResult::Handled);
        assert!(editor.selection.caret < 6);
    }

    #[test]
    fn window_char_units_preserve_cjk_and_surrogate_pairs() {
        let engine = TextEngine::create(true).unwrap();
        let mut editor = Editor::new(&engine, 96);
        for unit in "中文🙂".encode_utf16() {
            editor.handle_utf16(unit);
        }
        assert_eq!(editor.text(), "中文🙂");
        editor.handle_utf16(0xd83d);
        editor.set_text("");
        assert!(!editor.handle_utf16(0xde42));
        assert_eq!(editor.text(), "");
    }

    #[test]
    fn theme_formats_preserve_input_selection_and_placeholder() {
        let dark = TextEngine::create(true).unwrap();
        let light = TextEngine::create(false).unwrap();
        let mut editor = Editor::new(&dark, 144);
        editor.set_placeholder("搜索内容");
        editor.set_text("中文\nsecond line");
        editor.select_all();
        let selection = editor.selection;
        editor.set_text_engine(&light);
        assert_eq!(editor.text(), "中文\nsecond line");
        assert_eq!(editor.selection, selection);
        assert!(editor.placeholder_layout.is_some());
        assert_eq!(
            unsafe { editor.text_engine.input_format.GetFontWeight() },
            unsafe { light.input_format.GetFontWeight() }
        );
    }
}
