//! Accessibility provider tree and event synchronization.
mod nullable;
mod tree;
pub(super) use tree::{Node, Tree, ACTION};

impl super::State {
    pub(super) unsafe fn sync_accessibility(&self, rebuild: bool) {
        use windows_sys::Win32::{
            Foundation::{POINT, RECT},
            Graphics::Gdi::ClientToScreen,
            UI::{Input::KeyboardAndMouse::GetFocus, WindowsAndMessaging::*},
        };
        let mut screen = RECT::default();
        GetWindowRect(self.hwnd, &mut screen);
        if rebuild {
            self.accessibility.nodes(
                self.query
                    .results
                    .iter()
                    .map(|entry| {
                        let mut help = entry.subtitle.clone();
                        help.push_str(&format!("\nEnter {}", entry.action_label()));
                        if let Some(panel) = &entry.text_panel {
                            help.push_str(panel.status());
                            for block in &panel.blocks {
                                help.push_str(&format!("\n{}：{}", block.title, block.text));
                            }
                            if panel.retry {
                                help.push_str("\n按 Ctrl+R 重试当前查询");
                            }
                        }
                        Node {
                            id: entry.key.clone(),
                            name: entry.title.clone(),
                            help,
                            bounds: [0.; 4],
                            actionable: entry.text_panel.as_ref().is_none_or(|panel| {
                                panel.state == mini_launcher::result::PanelState::Ready
                                    || panel.retry
                            }),
                        }
                    })
                    .collect(),
                self.status.clone().unwrap_or_default(),
            );
        }
        self.accessibility
            .status(self.status.as_deref().unwrap_or(""));
        let mut origin = POINT::default();
        ClientToScreen(self.hwnd, &mut origin);
        let mut client = RECT::default();
        GetClientRect(self.hwnd, &mut client);
        let bottom = client.bottom
            - if self.footer_visible() {
                self.px(super::FOOTER_HEIGHT)
            } else {
                0
            };
        let rows = self
            .viewport
            .range()
            .map(|index| {
                let top = self
                    .px(self.row_top() + self.viewport.row_top(index))
                    .max(self.px(self.row_top()));
                let end = self
                    .px(self.row_top()
                        + self.viewport.row_top(index)
                        + self.viewport.row_height(index))
                    .min(bottom);
                (
                    index,
                    [
                        f64::from(origin.x + self.px(20)),
                        f64::from(origin.y + top),
                        f64::from((client.right - self.px(40)).max(0)),
                        f64::from((end - top).max(0)),
                    ],
                )
            })
            .collect();
        self.accessibility.view(
            self.viewport.selected,
            IsWindowVisible(self.hwnd) != 0,
            GetForegroundWindow() == self.hwnd && !GetFocus().is_null(),
            [
                screen.left as f64,
                screen.top as f64,
                (screen.right - screen.left) as f64,
                (screen.bottom - screen.top) as f64,
            ],
            rows,
        );
    }
}
