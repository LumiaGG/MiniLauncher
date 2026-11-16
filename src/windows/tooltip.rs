//! Native delayed tooltips for text that is abbreviated in the launcher.
use super::*;

pub(super) struct Tooltip {
    hwnd: HWND,
    owner: HWND,
    text: Vec<u16>,
    rect: RECT,
}

impl Tooltip {
    pub(super) unsafe fn new(owner: HWND) -> Self {
        InitCommonControlsEx(&INITCOMMONCONTROLSEX {
            dwSize: size_of::<INITCOMMONCONTROLSEX>() as u32,
            dwICC: ICC_WIN95_CLASSES,
        });
        let hwnd = CreateWindowExW(
            WS_EX_TOPMOST | WS_EX_NOACTIVATE,
            wide("tooltips_class32").as_ptr(),
            null(),
            WS_POPUP | TTS_ALWAYSTIP | TTS_NOPREFIX,
            CW_USEDEFAULT,
            CW_USEDEFAULT,
            CW_USEDEFAULT,
            CW_USEDEFAULT,
            owner,
            null_mut(),
            GetModuleHandleW(null()),
            null(),
        );
        let mut tip = Self {
            hwnd,
            owner,
            text: vec![0],
            rect: zeroed(),
        };
        let info = tip.info();
        SendMessageW(hwnd, TTM_ADDTOOLW, 0, &info as *const _ as isize);
        SendMessageW(hwnd, TTM_SETMAXTIPWIDTH, 0, 640);
        SendMessageW(hwnd, TTM_SETDELAYTIME, TTDT_AUTOPOP as usize, 15000);
        tip
    }

    fn info(&mut self) -> TTTOOLINFOW {
        TTTOOLINFOW {
            cbSize: size_of::<TTTOOLINFOW>() as u32,
            uFlags: TTF_SUBCLASS,
            hwnd: self.owner,
            uId: 1,
            rect: self.rect,
            lpszText: self.text.as_mut_ptr(),
            ..unsafe { zeroed() }
        }
    }

    pub(super) unsafe fn hide(&self) {
        SendMessageW(self.hwnd, TTM_POP, 0, 0);
    }

    pub(super) unsafe fn update(&mut self, text: &str, rect: RECT) {
        let text = wide(text);
        if self.text == text
            && (
                self.rect.left,
                self.rect.top,
                self.rect.right,
                self.rect.bottom,
            ) == (rect.left, rect.top, rect.right, rect.bottom)
        {
            return;
        }
        self.hide();
        // Keep the UTF-16 buffer alive for the control's delayed display.
        self.text = text;
        self.rect = rect;
        let info = self.info();
        SendMessageW(self.hwnd, TTM_NEWTOOLRECTW, 0, &info as *const _ as isize);
        SendMessageW(self.hwnd, TTM_UPDATETIPTEXTW, 0, &info as *const _ as isize);
    }
}

impl Drop for Tooltip {
    fn drop(&mut self) {
        unsafe {
            DestroyWindow(self.hwnd);
        }
    }
}

impl State {
    pub(super) unsafe fn update_tooltip(&mut self, x: i32, y: i32) {
        let (text, rect) = if self.hit_scope(x, y) {
            (
                format!("{}\n点击切换搜索范围", self.scope_name()),
                [12, 13, self.scope_right(), 45],
            )
        } else if self.hit_feedback(x, y) {
            (
                format!(
                    "{}\n点击查看详情、复制或重试 · Ctrl+Shift+I",
                    self.status.as_deref().unwrap_or("")
                ),
                [
                    20,
                    self.footer_top(),
                    self.width() - 20,
                    self.footer_top() + FOOTER_HEIGHT,
                ],
            )
        } else if let Some(index) = self.hit_result(x, y) {
            let entry = &self.query.results[index];
            if entry.text_panel.is_some() {
                (String::new(), [0; 4])
            } else {
                let top = self.row_top() + self.viewport.row_top(index);
                (
                    format!(
                        "{}\n{}\nEnter {}",
                        entry.title,
                        entry.subtitle,
                        entry.action_label()
                    ),
                    [
                        20,
                        top.max(self.row_top()),
                        self.width() - 20,
                        (top + self.viewport.row_height(index)).min(self.footer_top()),
                    ],
                )
            }
        } else {
            (String::new(), [0; 4])
        };
        if self.tooltip.is_none() {
            self.tooltip = Some(Tooltip::new(self.hwnd));
        }
        let [left, top, right, bottom] = rect;
        let rect = RECT {
            left: self.px(left),
            top: self.px(top),
            right: self.px(right),
            bottom: self.px(bottom),
        };
        self.tooltip.as_mut().unwrap().update(&text, rect);
    }
}
