use super::selection::{self, Selection};
use std::collections::VecDeque;

const UNDO_CAPACITY: usize = 128;
pub const MAX_VISIBLE_LINES: usize = 4;

/// Result of handling Enter key press.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum EnterAction {
    /// Enter pressed without shift: submit the input/query.
    Submit,
    /// Shift+Enter pressed and a newline was successfully inserted.
    NewlineInserted,
    /// Shift+Enter pressed but max 4 lines limit has been reached.
    MaxLinesBlocked,
}

/// A snapshot of editor text and selection for the undo/redo stack.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Snapshot {
    pub text: String,
    pub selection: Selection,
}

/// Ring-buffered undo/redo history manager.
#[derive(Clone, Debug)]
pub struct UndoManager {
    history: VecDeque<Snapshot>,
    cursor: usize,
    capacity: usize,
}

impl Default for UndoManager {
    fn default() -> Self {
        Self::new(UNDO_CAPACITY)
    }
}

impl UndoManager {
    pub fn new(capacity: usize) -> Self {
        let mut history = VecDeque::with_capacity(capacity);
        history.push_back(Snapshot {
            text: String::new(),
            selection: Selection::default(),
        });
        Self {
            history,
            cursor: 0,
            capacity: capacity.max(2),
        }
    }

    /// Records a new snapshot if different from the current state.
    pub fn record(&mut self, text: &str, selection: Selection) {
        if let Some(current) = self.history.get(self.cursor) {
            if current.text == text && current.selection == selection {
                return;
            }
        }

        // Truncate any redo branch ahead of cursor
        self.history.truncate(self.cursor + 1);

        // Enforce ring buffer capacity
        if self.history.len() >= self.capacity {
            self.history.pop_front();
            if self.cursor > 0 {
                self.cursor -= 1;
            }
        }

        self.history.push_back(Snapshot {
            text: text.to_string(),
            selection,
        });
        self.cursor = self.history.len() - 1;
    }

    pub fn undo(&mut self) -> Option<&Snapshot> {
        if self.cursor > 0 {
            self.cursor -= 1;
            self.history.get(self.cursor)
        } else {
            None
        }
    }

    pub fn redo(&mut self) -> Option<&Snapshot> {
        if self.cursor + 1 < self.history.len() {
            self.cursor += 1;
            self.history.get(self.cursor)
        } else {
            None
        }
    }

}

/// Unicode text buffer with UTF-16 code point mapping and max 4 lines enforcement.
#[derive(Clone, Debug)]
pub struct TextModel {
    text: String,
    undo_mgr: UndoManager,
}

impl Default for TextModel {
    fn default() -> Self {
        Self::new()
    }
}

impl TextModel {
    pub fn new() -> Self {
        Self {
            text: String::new(),
            undo_mgr: UndoManager::default(),
        }
    }

    #[cfg(test)]
    pub fn with_text(initial_text: &str) -> Self {
        let mut model = Self::new();
        let mut sel = Selection::default();
        model.set_text(initial_text, &mut sel);
        model
    }

    #[inline]
    pub fn as_str(&self) -> &str {
        &self.text
    }

    #[inline]
    pub fn is_empty(&self) -> bool {
        self.text.is_empty()
    }

    /// Length in UTF-8 bytes.
    #[inline]
    pub fn len_bytes(&self) -> usize {
        self.text.len()
    }

    /// Length in Unicode characters (scalar values).
    #[inline]
    pub fn len_chars(&self) -> usize {
        self.text.chars().count()
    }

    /// Length in UTF-16 code units (as used by DirectWrite & Win32 APIs).
    #[inline]
    pub fn len_utf16(&self) -> usize {
        self.text.encode_utf16().count()
    }

    /// Number of visible lines, matching `input::visible_lines` parity (clamped to 1..=4).
    pub fn visible_lines(&self) -> i32 {
        Self::visible_lines_of(&self.text)
    }

    /// Static calculation of visible lines for any text string.
    pub fn visible_lines_of(text: &str) -> i32 {
        let count = text.chars().filter(|c| matches!(c, '\r' | '\n')).take(3).count();
        (1 + count) as i32
    }

    /// Total newline count in the buffer.
    #[cfg(test)]
    pub fn newline_count(&self) -> usize {
        self.text.chars().filter(|&c| c == '\n').count()
    }

    // --- UTF-16 Mapping for Win32 API compatibility ---

    /// Converts a character index to a UTF-16 code unit offset.
    pub fn char_to_utf16_offset(&self, char_idx: usize) -> usize {
        let mut offset = 0;
        for (i, c) in self.text.chars().enumerate() {
            if i >= char_idx {
                break;
            }
            offset += c.len_utf16();
        }
        offset
    }

    /// Converts a UTF-16 code unit offset to a character index.
    pub fn utf16_to_char_offset(&self, utf16_offset: usize) -> usize {
        let mut current_u16 = 0;
        for (i, c) in self.text.chars().enumerate() {
            if current_u16 >= utf16_offset {
                return i;
            }
            current_u16 += c.len_utf16();
        }
        self.len_chars()
    }

    /// Converts a byte offset to a UTF-16 code unit offset.
    pub fn byte_to_utf16_offset(&self, byte_offset: usize) -> usize {
        let clamped = byte_offset.min(self.text.len());
        let valid_slice = &self.text[..clamped];
        valid_slice.encode_utf16().count()
    }

    /// Converts a UTF-16 code unit offset to a UTF-8 byte offset.
    pub fn utf16_to_byte_offset(&self, utf16_offset: usize) -> usize {
        let mut current_u16 = 0;
        for (byte_idx, c) in self.text.char_indices() {
            if current_u16 >= utf16_offset {
                return byte_idx;
            }
            current_u16 += c.len_utf16();
        }
        self.text.len()
    }

    /// Normalizes newlines in text and restricts line count so that total lines <= 4.
    fn sanitize_insert(
        existing_before: &str,
        insert: &str,
        existing_after: &str,
    ) -> String {
        let normalized = insert.replace("\r\n", "\n").replace('\r', "\n");
        let before_nl = existing_before.chars().filter(|&c| c == '\n').count();
        let after_nl = existing_after.chars().filter(|&c| c == '\n').count();
        let max_insert_nl = (MAX_VISIBLE_LINES - 1).saturating_sub(before_nl + after_nl);

        if max_insert_nl == 0 {
            // No newlines allowed: strip all newlines
            normalized.replace('\n', "")
        } else {
            let mut result = String::with_capacity(normalized.len());
            let mut nl_count = 0;
            for c in normalized.chars() {
                if c == '\n' {
                    if nl_count < max_insert_nl {
                        result.push(c);
                        nl_count += 1;
                    }
                } else {
                    result.push(c);
                }
            }
            result
        }
    }

    // --- Editing Operations ---

    /// Inserts text at the current selection, replacing selected text.
    /// Enforces max 4 lines limit. Updates selection and records undo.
    pub fn insert_text(&mut self, text_to_insert: &str, selection: &mut Selection) -> bool {
        let start_u16 = selection.start();
        let end_u16 = selection.end();

        let start_byte = self.utf16_to_byte_offset(start_u16);
        let end_byte = self.utf16_to_byte_offset(end_u16);

        let before = &self.text[..start_byte];
        let after = &self.text[end_byte..];

        let sanitized = Self::sanitize_insert(before, text_to_insert, after);
        if sanitized.is_empty() && start_byte == end_byte {
            return false;
        }

        let inserted_u16_len = sanitized.encode_utf16().count();

        let mut new_text = String::with_capacity(before.len() + sanitized.len() + after.len());
        new_text.push_str(before);
        new_text.push_str(&sanitized);
        new_text.push_str(after);

        self.text = new_text;
        let new_caret = start_u16 + inserted_u16_len;
        selection.collapse_to(new_caret);

        self.undo_mgr.record(&self.text, *selection);
        true
    }

    /// Handles Enter vs Shift+Enter.
    /// Shift+Enter attempts to insert a newline (up to 4 lines).
    /// Enter without Shift returns `EnterAction::Submit`.
    pub fn handle_enter(&mut self, selection: &mut Selection, shift_pressed: bool) -> EnterAction {
        if !shift_pressed {
            return EnterAction::Submit;
        }

        // Shift is pressed: check if newline can be added
        let start_byte = self.utf16_to_byte_offset(selection.start());
        let end_byte = self.utf16_to_byte_offset(selection.end());
        let before = &self.text[..start_byte];
        let after = &self.text[end_byte..];

        let before_nl = before.chars().filter(|&c| c == '\n').count();
        let after_nl = after.chars().filter(|&c| c == '\n').count();

        if before_nl + after_nl + 1 < MAX_VISIBLE_LINES {
            self.insert_text("\n", selection);
            EnterAction::NewlineInserted
        } else {
            EnterAction::MaxLinesBlocked
        }
    }

    /// Deletes the currently selected text range.
    pub fn delete_selection(&mut self, selection: &mut Selection) -> bool {
        if selection.is_empty() {
            return false;
        }

        let start_u16 = selection.start();
        let end_u16 = selection.end();

        let start_byte = self.utf16_to_byte_offset(start_u16);
        let end_byte = self.utf16_to_byte_offset(end_u16);

        let mut new_text = String::with_capacity(self.text.len() - (end_byte - start_byte));
        new_text.push_str(&self.text[..start_byte]);
        new_text.push_str(&self.text[end_byte..]);

        self.text = new_text;
        selection.collapse_to(start_u16);

        self.undo_mgr.record(&self.text, *selection);
        true
    }

    /// Deletes one character backward (Backspace).
    pub fn delete_char_backward(&mut self, selection: &mut Selection) -> bool {
        if !selection.is_empty() {
            return self.delete_selection(selection);
        }

        if selection.caret == 0 {
            return false;
        }

        // Find the previous character in UTF-16
        let caret_byte = self.utf16_to_byte_offset(selection.caret);
        let prev_char = self.text[..caret_byte].chars().last();

        if let Some(c) = prev_char {
            let char_bytes = c.len_utf8();
            let char_u16 = c.len_utf16();

            let remove_start_byte = caret_byte - char_bytes;
            let mut new_text = String::with_capacity(self.text.len() - char_bytes);
            new_text.push_str(&self.text[..remove_start_byte]);
            new_text.push_str(&self.text[caret_byte..]);

            self.text = new_text;
            selection.collapse_to(selection.caret - char_u16);

            self.undo_mgr.record(&self.text, *selection);
            true
        } else {
            false
        }
    }

    /// Deletes one character forward (Delete).
    pub fn delete_char_forward(&mut self, selection: &mut Selection) -> bool {
        if !selection.is_empty() {
            return self.delete_selection(selection);
        }

        let total_u16 = self.len_utf16();
        if selection.caret >= total_u16 {
            return false;
        }

        let caret_byte = self.utf16_to_byte_offset(selection.caret);
        let next_char = self.text[caret_byte..].chars().next();

        if let Some(c) = next_char {
            let char_bytes = c.len_utf8();

            let mut new_text = String::with_capacity(self.text.len() - char_bytes);
            new_text.push_str(&self.text[..caret_byte]);
            new_text.push_str(&self.text[caret_byte + char_bytes..]);

            self.text = new_text;
            self.undo_mgr.record(&self.text, *selection);
            true
        } else {
            false
        }
    }

    /// Deletes backward by word (Ctrl+Backspace).
    pub fn delete_word_backward(&mut self, selection: &mut Selection) -> bool {
        if !selection.is_empty() {
            return self.delete_selection(selection);
        }

        let target_u16 = selection::find_word_left(&self.text, selection.caret);
        if target_u16 < selection.caret {
            selection.anchor = target_u16;
            self.delete_selection(selection)
        } else {
            false
        }
    }

    /// Deletes forward by word (Ctrl+Delete).
    pub fn delete_word_forward(&mut self, selection: &mut Selection) -> bool {
        if !selection.is_empty() {
            return self.delete_selection(selection);
        }

        let target_u16 = selection::find_word_right(&self.text, selection.caret);
        if target_u16 > selection.caret {
            selection.anchor = target_u16;
            self.delete_selection(selection)
        } else {
            false
        }
    }

    /// Replaces entire text buffer, enforcing max 4 lines, and adjusts selection.
    pub fn set_text(&mut self, new_text: &str, selection: &mut Selection) {
        let normalized = new_text.replace("\r\n", "\n").replace('\r', "\n");
        let mut truncated = String::new();
        let mut nl_count = 0;
        for c in normalized.chars() {
            if c == '\n' {
                if nl_count < MAX_VISIBLE_LINES - 1 {
                    truncated.push(c);
                    nl_count += 1;
                }
            } else {
                truncated.push(c);
            }
        }

        self.text = truncated;
        let total_u16 = self.len_utf16();
        selection.clamp(total_u16);
        self.undo_mgr.record(&self.text, *selection);
    }

    /// Restores previous state from undo stack (Ctrl+Z).
    pub fn undo(&mut self, selection: &mut Selection) -> bool {
        if let Some(snapshot) = self.undo_mgr.undo() {
            self.text = snapshot.text.clone();
            *selection = snapshot.selection;
            true
        } else {
            false
        }
    }

    /// Reapplies redone state from undo stack (Ctrl+Y).
    pub fn redo(&mut self, selection: &mut Selection) -> bool {
        if let Some(snapshot) = self.undo_mgr.redo() {
            self.text = snapshot.text.clone();
            *selection = snapshot.selection;
            true
        } else {
            false
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_utf16_mapping_with_cjk_and_emoji() {
        // "hi 你好 😊"
        // 'h': 1 byte, 1 u16
        // 'i': 1 byte, 1 u16
        // ' ': 1 byte, 1 u16
        // '你': 3 bytes, 1 u16
        // '好': 3 bytes, 1 u16
        // ' ': 1 byte, 1 u16
        // '😊': 4 bytes, 2 u16 (surrogate pair)
        let text = "hi 你好 😊";
        let model = TextModel::with_text(text);

        assert_eq!(model.len_chars(), 7);
        assert_eq!(model.len_utf16(), 8); // 3 (hi ) + 2 (你好) + 1 ( ) + 2 (😊)

        // Test char <-> utf16
        assert_eq!(model.char_to_utf16_offset(6), 6); // start of '😊'
        assert_eq!(model.char_to_utf16_offset(7), 8); // after '😊'
        assert_eq!(model.utf16_to_char_offset(6), 6);
        assert_eq!(model.utf16_to_char_offset(8), 7);

        // Test byte <-> utf16
        let emoji_byte = text.find('😊').unwrap();
        assert_eq!(model.byte_to_utf16_offset(emoji_byte), 6);
        assert_eq!(model.utf16_to_byte_offset(6), emoji_byte);
    }

    #[test]
    fn test_max_4_lines_enforcement() {
        let mut model = TextModel::new();
        let mut sel = Selection::default();

        model.insert_text("line1\nline2\nline3\nline4\nline5\nline6", &mut sel);
        assert_eq!(model.visible_lines(), 4);
        assert_eq!(model.newline_count(), 3); // exactly 3 newlines = 4 lines

        // Trying to insert another newline via Shift+Enter should be blocked
        let action = model.handle_enter(&mut sel, true);
        assert_eq!(action, EnterAction::MaxLinesBlocked);
        assert_eq!(model.visible_lines(), 4);

        // Plain enter returns Submit
        let action = model.handle_enter(&mut sel, false);
        assert_eq!(action, EnterAction::Submit);
    }

    #[test]
    fn test_undo_redo_stack() {
        let mut model = TextModel::new();
        let mut sel = Selection::default();

        model.insert_text("hello", &mut sel);
        assert_eq!(model.as_str(), "hello");

        model.insert_text(" world", &mut sel);
        assert_eq!(model.as_str(), "hello world");

        // Undo to "hello"
        assert!(model.undo(&mut sel));
        assert_eq!(model.as_str(), "hello");

        // Undo to empty
        assert!(model.undo(&mut sel));
        assert_eq!(model.as_str(), "");

        // No more undo
        assert!(!model.undo(&mut sel));

        // Redo to "hello"
        assert!(model.redo(&mut sel));
        assert_eq!(model.as_str(), "hello");

        // Redo to "hello world"
        assert!(model.redo(&mut sel));
        assert_eq!(model.as_str(), "hello world");
    }
}
