use std::ptr::null_mut;
use windows_sys::Win32::Foundation::{HWND, LPARAM, LRESULT, POINT, RECT, WPARAM};
use windows_sys::Win32::UI::Input::Ime::*;
use windows_sys::Win32::UI::WindowsAndMessaging::{
    WM_IME_COMPOSITION, WM_IME_ENDCOMPOSITION, WM_IME_STARTCOMPOSITION,
};

#[derive(Clone, Debug, PartialEq, Eq)]
pub enum ImeAction {
    None,
    /// IME committed final text to be inserted into the document.
    Commit(String),
    /// Inline composition string was updated.
    CompositionChanged,
    /// Composition session ended.
    CompositionEnded,
}

#[cfg(test)]
mod tests {
    use super::super::{Editor, Selection};
    use super::*;
    use crate::windows::gfx::TextEngine;
    use windows::Win32::Graphics::DirectWrite::DWRITE_LINE_METRICS;

    #[test]
    fn ime_preview_replaces_selection_in_query_and_layout() {
        let engine = TextEngine::create(true).unwrap();
        let mut editor = Editor::new(&engine, 96);
        editor.set_rect(0.0, 0.0, 600.0, 80.0);

        for (text, selection, expected) in [
            ("旧内容", Selection::new(0, 3), "mo'ni"),
            ("前😀旧后", Selection::new(4, 1), "前mo'ni后"),
            ("前后", Selection::new(1, 1), "前mo'ni后"),
        ] {
            editor.set_text(text);
            editor.selection = selection;
            // Simulate the state returned by IMM32 while the candidate is pending.
            editor.ime.is_composing = true;
            editor.ime.composition_string = "mo'ni".into();
            editor.sync_layout();

            assert_eq!(editor.effective_text(), expected.replace('\'', ""));
            assert_eq!(editor.query_input().0, expected.replace('\'', ""));
            let mut lines = [DWRITE_LINE_METRICS::default(); 1];
            let mut count = 0;
            unsafe {
                editor
                    .layout
                    .raw_layout()
                    .unwrap()
                    .GetLineMetrics(Some(&mut lines), &mut count)
                    .unwrap();
            }
            assert_eq!(lines[0].length as usize, expected.encode_utf16().count());

            // Previewing must preserve the original document and selection on cancel.
            assert_eq!(editor.text(), text);
            editor.handle_ime(null_mut(), WM_IME_ENDCOMPOSITION, 0, 0);
            assert_eq!(editor.effective_text(), text);
            assert_eq!(editor.selection, selection);
        }
    }

    #[test]
    fn ime_query_ignores_separators_but_preserves_literal_apostrophes() {
        let engine = TextEngine::create(true).unwrap();
        let mut editor = Editor::new(&engine, 96);
        editor.set_text("it's  o'clock");
        editor.selection = Selection::new(5, 5);
        editor.ime.is_composing = true;
        editor.ime.composition_string = "w't".into();

        assert_eq!(editor.effective_text(), "it's wt o'clock");
        assert_eq!(editor.query_input().0, "it's wt o'clock");
        assert_eq!(editor.ime.composition_string(), "w't");

        editor.handle_ime(null_mut(), WM_IME_ENDCOMPOSITION, 0, 0);
        editor.set_text("w't");
        assert_eq!(editor.effective_text(), "w't");
        assert_eq!(editor.query_input().0, "w't");
    }
}

/// Manages IMM32 composition state and candidate window positioning.
#[derive(Clone, Debug, Default)]
pub struct ImeHandler {
    is_composing: bool,
    composition_string: String,
}

impl ImeHandler {
    pub fn new() -> Self {
        Self::default()
    }

    #[inline]
    pub fn is_composing(&self) -> bool {
        self.is_composing
    }

    #[inline]
    pub fn composition_string(&self) -> &str {
        &self.composition_string
    }

    /// Updates the position of the IME candidate window right under the caret.
    pub unsafe fn update_candidate_pos(hwnd: HWND, caret_pos: POINT) {
        let context = ImmGetContext(hwnd);
        if !context.is_null() {
            let form = COMPOSITIONFORM {
                dwStyle: CFS_POINT,
                ptCurrentPos: caret_pos,
                rcArea: RECT {
                    left: 0,
                    top: 0,
                    right: 0,
                    bottom: 0,
                },
            };
            ImmSetCompositionWindow(context, &form);
            ImmReleaseContext(hwnd, context);
        }
    }

    /// Handles IMM32 window messages.
    pub unsafe fn handle_message(
        &mut self,
        hwnd: HWND,
        msg: u32,
        _wparam: WPARAM,
        lparam: LPARAM,
        caret_pos: POINT,
    ) -> (LRESULT, ImeAction) {
        match msg {
            WM_IME_STARTCOMPOSITION => {
                self.is_composing = true;
                self.composition_string.clear();
                Self::update_candidate_pos(hwnd, caret_pos);
                (0, ImeAction::CompositionChanged)
            }
            WM_IME_COMPOSITION => {
                let context = ImmGetContext(hwnd);
                if context.is_null() {
                    return (0, ImeAction::None);
                }

                let mut action = ImeAction::None;
                let flags = lparam as u32;

                // Check for committed result string
                if (flags & GCS_RESULTSTR) != 0 {
                    let len = ImmGetCompositionStringW(context, GCS_RESULTSTR, null_mut(), 0);
                    if len > 0 {
                        let mut buffer = vec![0u16; (len as usize) / 2];
                        ImmGetCompositionStringW(
                            context,
                            GCS_RESULTSTR,
                            buffer.as_mut_ptr() as *mut _,
                            len as u32,
                        );
                        let committed = String::from_utf16_lossy(&buffer);
                        self.composition_string.clear();
                        action = ImeAction::Commit(committed);
                    }
                }

                // Check for inline composition string update
                if (flags & GCS_COMPSTR) != 0 {
                    let len = ImmGetCompositionStringW(context, GCS_COMPSTR, null_mut(), 0);
                    if len > 0 {
                        let mut buffer = vec![0u16; (len as usize) / 2];
                        ImmGetCompositionStringW(
                            context,
                            GCS_COMPSTR,
                            buffer.as_mut_ptr() as *mut _,
                            len as u32,
                        );
                        self.composition_string = String::from_utf16_lossy(&buffer);
                        self.is_composing = true;
                        Self::update_candidate_pos(hwnd, caret_pos);
                        if action == ImeAction::None {
                            action = ImeAction::CompositionChanged;
                        }
                    } else {
                        self.composition_string.clear();
                        if action == ImeAction::None {
                            action = ImeAction::CompositionChanged;
                        }
                    }
                }

                ImmReleaseContext(hwnd, context);
                (0, action)
            }
            WM_IME_ENDCOMPOSITION => {
                self.is_composing = false;
                self.composition_string.clear();
                (0, ImeAction::CompositionEnded)
            }
            _ => (0, ImeAction::None),
        }
    }
}
