/// Represents text selection with an anchor and caret offset in UTF-16 code units.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Default)]
pub struct Selection {
    /// Anchor position where selection started (in UTF-16 code units).
    pub anchor: usize,
    /// Active caret position (in UTF-16 code units).
    pub caret: usize,
}

impl Selection {
    #[cfg(test)]
    pub const fn new(anchor: usize, caret: usize) -> Self {
        Self { anchor, caret }
    }

    #[inline]
    pub fn start(&self) -> usize {
        self.anchor.min(self.caret)
    }

    #[inline]
    pub fn end(&self) -> usize {
        self.anchor.max(self.caret)
    }

    #[inline]
    pub fn is_empty(&self) -> bool {
        self.anchor == self.caret
    }

    #[inline]
    pub fn len(&self) -> usize {
        self.end() - self.start()
    }

    #[inline]
    pub fn collapse_to(&mut self, offset: usize) {
        self.anchor = offset;
        self.caret = offset;
    }

    #[inline]
    pub fn extend_to(&mut self, offset: usize) {
        self.caret = offset;
    }

    #[inline]
    pub fn select_all(&mut self, text_utf16_len: usize) {
        self.anchor = 0;
        self.caret = text_utf16_len;
    }

    #[inline]
    pub fn clamp(&mut self, max_utf16_len: usize) {
        self.anchor = self.anchor.min(max_utf16_len);
        self.caret = self.caret.min(max_utf16_len);
    }

    /// Handles single-click selection.
    pub fn handle_click(&mut self, hit_offset: usize, shift_held: bool) {
        if shift_held {
            self.extend_to(hit_offset);
        } else {
            self.collapse_to(hit_offset);
        }
    }

    /// Handles double-click word selection.
    pub fn handle_double_click(&mut self, text: &str, hit_offset: usize) {
        let (start, end) = find_word_boundaries(text, hit_offset);
        self.anchor = start;
        self.caret = end;
    }

    /// Handles drag selection when mouse moves with button pressed.
    pub fn handle_drag(&mut self, hit_offset: usize) {
        self.extend_to(hit_offset);
    }
}

#[derive(Clone, Copy, PartialEq, Eq)]
enum CharClass {
    Whitespace,
    Word,
    Punctuation,
}

fn classify_char(c: char) -> CharClass {
    if c.is_whitespace() {
        CharClass::Whitespace
    } else if c.is_alphanumeric() || c == '_' {
        CharClass::Word
    } else {
        CharClass::Punctuation
    }
}

/// Finds the (start, end) UTF-16 code unit offsets of the word at `utf16_offset`.
pub fn find_word_boundaries(text: &str, utf16_offset: usize) -> (usize, usize) {
    if text.is_empty() {
        return (0, 0);
    }

    // Collect chars with their UTF-16 offset range
    let mut chars: Vec<(char, usize, usize)> = Vec::new();
    let mut current_offset = 0;
    for c in text.chars() {
        let len = c.len_utf16();
        chars.push((c, current_offset, current_offset + len));
        current_offset += len;
    }

    let total_len = current_offset;
    let target = utf16_offset.min(total_len);

    // Find the character index containing or preceding the target offset
    let char_idx = chars
        .iter()
        .position(|&(_, start, end)| target >= start && target < end)
        .unwrap_or(chars.len().saturating_sub(1));

    let target_class = classify_char(chars[char_idx].0);

    // Expand left
    let mut start_idx = char_idx;
    while start_idx > 0 && classify_char(chars[start_idx - 1].0) == target_class {
        start_idx -= 1;
    }

    // Expand right
    let mut end_idx = char_idx;
    while end_idx + 1 < chars.len() && classify_char(chars[end_idx + 1].0) == target_class {
        end_idx += 1;
    }

    (chars[start_idx].1, chars[end_idx].2)
}

/// Finds the UTF-16 code unit offset when moving left by word (Ctrl+Left).
pub fn find_word_left(text: &str, utf16_offset: usize) -> usize {
    if utf16_offset == 0 || text.is_empty() {
        return 0;
    }

    let mut chars: Vec<(char, usize, usize)> = Vec::new();
    let mut current_offset = 0;
    for c in text.chars() {
        let len = c.len_utf16();
        chars.push((c, current_offset, current_offset + len));
        current_offset += len;
    }

    // Find index of character to the left of utf16_offset
    let mut idx = chars
        .iter()
        .rposition(|&(_, _, end)| end <= utf16_offset)
        .unwrap_or(0);

    // Skip whitespace to the left
    while idx > 0 && chars[idx].0.is_whitespace() {
        idx -= 1;
    }

    let class = classify_char(chars[idx].0);
    // Skip word/punctuation of same class to the left
    while idx > 0 && classify_char(chars[idx - 1].0) == class {
        idx -= 1;
    }

    chars[idx].1
}

/// Finds the UTF-16 code unit offset when moving right by word (Ctrl+Right).
pub fn find_word_right(text: &str, utf16_offset: usize) -> usize {
    if text.is_empty() {
        return 0;
    }

    let mut chars: Vec<(char, usize, usize)> = Vec::new();
    let mut current_offset = 0;
    for c in text.chars() {
        let len = c.len_utf16();
        chars.push((c, current_offset, current_offset + len));
        current_offset += len;
    }

    let total_len = current_offset;
    if utf16_offset >= total_len {
        return total_len;
    }

    // Find index of character at or after utf16_offset
    let mut idx = match chars.iter().position(|&(_, start, _)| start >= utf16_offset) {
        Some(i) => i,
        None => return total_len,
    };

    let class = classify_char(chars[idx].0);
    // Skip current word / sequence of same class
    while idx < chars.len() && classify_char(chars[idx].0) == class {
        idx += 1;
    }

    // Also skip trailing whitespace
    while idx < chars.len() && chars[idx].0.is_whitespace() {
        idx += 1;
    }

    if idx < chars.len() {
        chars[idx].1
    } else {
        total_len
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_selection_range() {
        let mut sel = Selection::new(5, 2);
        assert_eq!(sel.start(), 2);
        assert_eq!(sel.end(), 5);
        assert_eq!(sel.len(), 3);
        assert!(!sel.is_empty());

        sel.collapse_to(7);
        assert_eq!(sel.start(), 7);
        assert_eq!(sel.end(), 7);
        assert!(sel.is_empty());

        sel.extend_to(10);
        assert_eq!(sel.start(), 7);
        assert_eq!(sel.end(), 10);
    }

    #[test]
    fn test_word_boundaries_ascii() {
        let text = "hello world_123 test";
        let (start, end) = find_word_boundaries(text, 2); // 'l' in "hello"
        assert_eq!(&text[start..end], "hello");

        let (start, end) = find_word_boundaries(text, 6); // 'w' in "world_123"
        assert_eq!(&text[start..end], "world_123");

        let (start, end) = find_word_boundaries(text, 5); // space
        assert_eq!(&text[start..end], " ");
    }

    #[test]
    fn test_word_boundaries_cjk() {
        let text = "你好 mini launcher 世界";
        let (start, end) = find_word_boundaries(text, 0); // '你'
        assert_eq!((start, end), (0, 2)); // UTF-16 offset range for "你好"

        let (start, end) = find_word_boundaries(text, 3); // 'm' in "mini"
        assert_eq!((start, end), (3, 7)); // UTF-16 offset range for "mini"
    }

    #[test]
    fn test_word_navigation() {
        let text = "hello world foo";
        assert_eq!(find_word_right(text, 0), 6); // start of "world"
        assert_eq!(find_word_right(text, 6), 12); // start of "foo"
        assert_eq!(find_word_left(text, 12), 6); // back to "world"
        assert_eq!(find_word_left(text, 6), 0); // back to "hello"
    }
}
