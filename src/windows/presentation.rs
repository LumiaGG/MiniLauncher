use super::*;

/// Map lowercased UTF-8 matches back to DirectWrite's original UTF-16 positions.
pub(super) fn literal_ranges(text: &str, query: &str) -> Vec<std::ops::Range<u32>> {
    let mut normalized = String::new();
    let mut positions = Vec::new();
    let mut offset = 0_u32;
    for ch in text.chars() {
        let lower: String = ch.to_lowercase().collect();
        positions.extend(std::iter::repeat_n(
            (offset, ch.len_utf16() as u32),
            lower.len(),
        ));
        normalized.push_str(&lower);
        offset += ch.len_utf16() as u32;
    }
    let mut ranges = Vec::new();
    for token in query.split_whitespace().take(32) {
        let token = token.to_lowercase();
        for (start, matched) in normalized.match_indices(&token) {
            let (from, _) = positions[start];
            let (to, len) = positions[start + matched.len() - 1];
            ranges.push(from..to + len);
        }
    }
    ranges.sort_by_key(|range| range.start);
    let mut merged: Vec<std::ops::Range<u32>> = Vec::new();
    for range in ranges {
        if let Some(previous) = merged
            .last_mut()
            .filter(|previous| previous.end >= range.start)
        {
            previous.end = previous.end.max(range.end);
        } else {
            merged.push(range);
        }
    }
    merged
}

impl State {
    pub(super) fn footer_top(&self) -> i32 {
        let mut client = RECT::default();
        unsafe {
            GetClientRect(self.hwnd, &mut client);
        }
        self.logical(client.bottom)
            - if self.footer_visible() {
                FOOTER_HEIGHT
            } else {
                0
            }
    }

    pub(super) fn hit_feedback(&self, x: i32, y: i32) -> bool {
        self.status.is_some()
            && (20..self.width() - 20).contains(&x)
            && (self.footer_top()..self.footer_top() + FOOTER_HEIGHT).contains(&y)
    }

    pub(super) fn application_index_pending(&self) -> bool {
        !self.query.indexed
            && self
                .query
                .context
                .as_ref()
                .is_some_and(|(scope, route, attachments)| {
                    *scope == scope::Scope::All && *route == query::Route::Default && !attachments
                })
    }

    pub(super) fn empty_title(&self) -> &'static str {
        if self.application_index_pending() {
            "正在准备你的应用"
        } else if self.status.is_some() {
            if self.scope == scope::Scope::Files
                || self
                    .query
                    .context
                    .as_ref()
                    .is_some_and(|(_, route, _)| *route == query::Route::Files)
            {
                "文件搜索暂不可用"
            } else {
                "当前搜索暂不可用"
            }
        } else {
            "没有找到匹配项"
        }
    }

    pub(super) fn empty_actions(&self) -> (&'static str, &'static str) {
        if self.status.is_some() {
            ("查看原因 / 重试", "打开设置")
        } else if self.scope == scope::Scope::All {
            ("重新搜索 · Ctrl+R", "选择搜索范围")
        } else {
            ("重新搜索 · Ctrl+R", "切回综合搜索")
        }
    }

    pub(super) fn hit_empty_action(&self, x: i32, y: i32) -> Option<bool> {
        (self.query.empty_message_visible()
            && (40..self.width() - 40).contains(&x)
            && (self.row_top() + 32..self.row_top() + 56).contains(&y))
        .then_some(x >= self.width() / 2)
    }

    pub(super) unsafe fn activate_empty_action(&mut self, secondary: bool) {
        if secondary {
            if self.status.is_some() {
                self.open_settings();
            } else if self.scope != scope::Scope::All {
                self.set_scope(scope::Scope::All);
            } else {
                self.scope_menu();
            }
        } else if self.status.is_some() {
            self.feedback_menu();
        } else {
            self.retry_search();
        }
    }

    pub(super) unsafe fn retry_search(&mut self) {
        self.execution.execution_error = None;
        self.request_search();
        self.focus_input();
    }

    pub(super) fn result_hint(&self) -> String {
        let Some(entry) = self.query.results.get(self.viewport.selected) else {
            return "Ctrl+R 重试    点击左侧切换范围".into();
        };
        let extra = if entry.text_panel.is_some() {
            "拖选文字    "
        } else if self.can_preview_image() {
            "空格 预览    "
        } else {
            "↑ ↓ 选择    "
        };
        format!(
            "{extra}Enter {}    → / 右键 更多    Esc 返回",
            entry.action_label()
        )
    }
}

/// Keep the filename visible when an absolute path exceeds the available width.
pub(super) fn compact_path(text: &str, fits: impl Fn(&str) -> bool) -> String {
    if !(text.starts_with('\\') || text.as_bytes().get(1) == Some(&b':')) || fits(text) {
        return text.to_string();
    }
    let Some(split) = text.rfind(['\\', '/']) else {
        return text.to_string();
    };
    let tail = &text[split..];
    let (prefix, suffix) = if fits(&format!("…{tail}")) {
        (&text[..split], tail)
    } else {
        let filename = &text[split + 1..];
        let extension = filename.rfind('.').map_or("", |at| &filename[at..]);
        let extension = if fits(&format!("…{extension}")) {
            extension
        } else {
            ""
        };
        (&filename[..filename.len() - extension.len()], extension)
    };
    // Logarithmic measurements avoid laying out every prefix of a long path.
    let boundaries: Vec<_> = prefix
        .char_indices()
        .map(|(at, _)| at)
        .chain(std::iter::once(prefix.len()))
        .collect();
    let (mut low, mut high) = (0, boundaries.len());
    let mut best = format!("…{suffix}");
    while low < high {
        let mid = low + (high - low) / 2;
        let candidate = format!("{}…{suffix}", &prefix[..boundaries[mid]]);
        if fits(&candidate) {
            best = candidate;
            low = mid + 1;
        } else {
            high = mid;
        }
    }
    best
}

#[cfg(test)]
mod tests {
    use super::compact_path;
    #[test]
    fn path_shortening_preserves_filename_and_unicode_boundaries() {
        let path = "C:\\很长的目录\\另一个目录\\报告.txt";
        let short = compact_path(path, |s| s.chars().count() <= 18);
        assert!(short.ends_with("\\报告.txt"));
        assert!(short.contains('…'));
        assert!(short.chars().count() <= 18);
        assert_eq!(
            compact_path("a normal subtitle", |_| false),
            "a normal subtitle"
        );
        let short = compact_path(
            "C:\\目录\\非常非常非常非常非常非常长的报告文件名.txt",
            |s| s.chars().count() <= 12,
        );
        assert!(short.ends_with(".txt"));
        assert!(short.chars().count() <= 12);
    }

    #[test]
    fn literal_highlights_map_unicode_and_merge_overlaps() {
        assert_eq!(
            super::literal_ranges("😀İ文件 FILE", "i 文件 file"),
            vec![2..5, 6..10]
        );
        assert_eq!(
            super::literal_ranges("Calculator", "calc calculator"),
            vec![0..10]
        );
        assert!(super::literal_ranges("计算器", "jsq").is_empty());
    }
}
