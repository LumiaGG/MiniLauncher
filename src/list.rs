use std::ops::Range;

/// A small viewport over the complete result set. Off-screen rows are not drawn.
#[derive(Default)]
pub struct Viewport {
    pub selected: usize,
    pub first: usize,
    total: usize,
    visible: usize,
    wheel_remainder: i32,
    heights: Vec<i32>,
}

pub struct Thumb {
    pub top: i32,
    pub height: i32,
}

impl Viewport {
    pub fn reset(&mut self, total: usize, visible: usize) {
        *self = Self {
            total,
            visible: visible.max(1),
            ..Self::default()
        };
    }

    pub fn set_visible(&mut self, visible: usize) {
        self.visible = visible.max(1);
        self.reveal_selection();
    }
    pub fn update_total(&mut self, total: usize, selected: usize) {
        self.total = total;
        self.selected = selected;
        self.reveal_selection();
    }

    pub fn range(&self) -> Range<usize> {
        let mut end = self.first;
        let mut used = 0;
        while end < self.total && used < self.capacity() {
            used += self.row_height(end);
            end += 1;
        }
        self.first..end
    }

    // Rendering and thumbnail loading include partial rows; navigation must
    // reveal the entire selected row instead of stopping at the visible sliver.
    fn fully_visible_range(&self) -> Range<usize> {
        let mut end = self.first;
        let mut used = 0;
        while end < self.total {
            let height = self.row_height(end);
            if end > self.first && used + height > self.capacity() {
                break;
            }
            used += height;
            end += 1;
        }
        self.first..end
    }

    pub fn capacity(&self) -> i32 {
        self.visible.max(1) as i32 * 60
    }

    pub fn set_heights(&mut self, heights: Vec<i32>) {
        self.heights = heights.into_iter().map(|h| h.clamp(60, 1200)).collect();
        self.reveal_selection();
    }

    pub fn row_height(&self, index: usize) -> i32 {
        self.heights
            .get(index)
            .copied()
            .unwrap_or(60)
            .min(self.capacity())
    }

    pub fn row_top(&self, index: usize) -> i32 {
        (self.first..index.min(self.total))
            .map(|i| self.row_height(i))
            .sum()
    }

    pub fn content_height(&self) -> i32 {
        self.range()
            .map(|i| self.row_height(i))
            .sum::<i32>()
            .min(self.capacity())
    }

    pub fn hit(&self, position: i32) -> Option<usize> {
        if position < 0 || position >= self.capacity() {
            return None;
        }
        let mut top = 0;
        for i in self.range() {
            top += self.row_height(i);
            if position < top {
                return Some(i);
            }
        }
        None
    }

    pub fn scrollable(&self) -> bool {
        self.max_first() != 0
    }

    fn offset(&self, end: usize) -> i64 {
        (0..end).map(|i| i64::from(self.row_height(i))).sum()
    }

    fn max_first(&self) -> usize {
        let mut first = self.total;
        let mut used = 0;
        while first > 0 {
            let height = self.row_height(first - 1);
            if first < self.total && used + height > self.capacity() {
                break;
            }
            used += height;
            first -= 1;
        }
        first
    }

    fn reveal_selection(&mut self) {
        self.selected = self.selected.min(self.total.saturating_sub(1));
        self.first = self.first.min(self.max_first());
        if self.selected < self.first {
            self.first = self.selected;
        }
        while self.selected >= self.fully_visible_range().end && self.first < self.selected {
            self.first += 1;
        }
    }

    pub fn move_selection(&mut self, direction: i32) {
        if self.total == 0 {
            return;
        }
        self.selected =
            (self.selected as i64 + i64::from(direction)).rem_euclid(self.total as i64) as usize;
        self.reveal_selection();
    }

    pub fn page(&mut self, direction: i32) {
        if self.total == 0 {
            return;
        }
        self.selected = (self.selected as i64
            + i64::from(direction) * self.fully_visible_range().len().max(1) as i64)
            .clamp(0, self.total as i64 - 1) as usize;
        self.reveal_selection();
    }

    pub fn scroll_rows(&mut self, rows: i32) {
        self.first =
            (self.first as i64 + i64::from(rows)).clamp(0, self.max_first() as i64) as usize;
        self.clamp_selection_to_view();
    }

    fn clamp_selection_to_view(&mut self) {
        // Enter always acts on a visible item after scrolling with the mouse.
        if self.total != 0 {
            self.selected = self.selected.clamp(self.first, self.range().end - 1);
        }
    }

    pub fn wheel(&mut self, delta: i32, lines: u32) {
        self.wheel_remainder += delta;
        let ticks = self.wheel_remainder / 120;
        self.wheel_remainder %= 120;
        let lines = if lines == u32::MAX {
            self.fully_visible_range().len().max(1) as i32
        } else {
            lines.min(i32::MAX as u32) as i32
        };
        self.scroll_rows((-ticks).saturating_mul(lines));
    }

    pub fn thumb(&self, top: i32, height: i32) -> Option<Thumb> {
        if self.max_first() == 0 || height <= 0 {
            return None;
        }
        let thumb_height = ((height as i64
            * i64::from(self.capacity()).min(self.offset(self.total))
            / self.offset(self.total)) as i32)
            .max(24)
            .min(height);
        let travel = height - thumb_height;
        Some(Thumb {
            top: top
                + (travel as i64 * self.offset(self.first) / self.offset(self.max_first())) as i32,
            height: thumb_height,
        })
    }

    pub fn drag_thumb(&mut self, pointer_top: i32, track_top: i32, track_height: i32) {
        let Some(thumb) = self.thumb(track_top, track_height) else {
            return;
        };
        let travel = track_height - thumb.height;
        if travel == 0 {
            return;
        }
        let position = (pointer_top - track_top).clamp(0, travel) as i64;
        let target = position * self.offset(self.max_first()) / i64::from(travel);
        let mut offset = 0;
        self.first = 0;
        while self.first < self.max_first() {
            let next = i64::from(self.row_height(self.first));
            if offset + next / 2 > target {
                break;
            }
            offset += next;
            self.first += 1;
        }
        self.clamp_selection_to_view();
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn mixed_heights_share_geometry_for_navigation_hit_testing_and_scrollbar() {
        let mut view = Viewport::default();
        view.reset(6, 6);
        view.set_heights(vec![60, 160, 100, 160, 60, 160]);
        assert_eq!(view.range(), 0..4);
        assert_eq!(view.row_top(2), 220);
        assert_eq!(view.hit(59), Some(0));
        assert_eq!(view.hit(60), Some(1));
        assert_eq!(view.hit(219), Some(1));
        assert_eq!(view.hit(220), Some(2));
        assert_eq!(view.hit(320), Some(3));
        assert_eq!(view.hit(360), None);
        view.page(1);
        assert_eq!(view.selected, 3);
        assert!(view.range().contains(&view.selected));
        view.drag_thumb(10000, 68, 354);
        assert_eq!(view.range().end, 6);
        assert!(view.range().contains(&view.selected));
        let thumb = view.thumb(68, 354).unwrap();
        assert_eq!(thumb.top + thumb.height, 422);
        view.set_visible(1);
        assert!(view.content_height() <= 60);
        assert!(view.range().contains(&view.selected));
    }

    #[test]
    fn bottom_partial_image_row_is_visible_but_selection_reveals_the_whole_row() {
        let mut view = Viewport::default();
        view.reset(19, 8);
        view.set_heights(vec![84; 19]);
        assert_eq!(view.range(), 0..6);
        assert_eq!(view.row_top(5), 420);
        assert_eq!(view.content_height(), 480);
        assert_eq!(view.hit(479), Some(5));
        assert_eq!(view.hit(480), None);
        view.move_selection(5);
        assert_eq!(view.selected, 5);
        assert_eq!(view.first, 1);
        assert!(view.row_top(view.selected) + view.row_height(view.selected) <= view.capacity());
        view.scroll_rows(i32::MAX);
        assert_eq!(view.range().end, 19);
        assert!(view.content_height() <= view.capacity());
        assert_eq!(view.hit(view.content_height()), None);
    }

    #[test]
    fn keyboard_reaches_results_beyond_the_visible_rows_and_wraps() {
        let mut view = Viewport::default();
        view.reset(20, 6);
        for _ in 0..19 {
            view.move_selection(1);
        }
        assert_eq!(view.selected, 19);
        assert_eq!(view.range(), 14..20);
        view.move_selection(1);
        assert_eq!(view.range(), 0..6);
        view.move_selection(-1);
        assert_eq!(view.selected, 19);
        assert_eq!(view.range(), 14..20);
    }

    #[test]
    fn wheel_accumulates_small_deltas_and_does_not_wrap_at_the_ends() {
        let mut view = Viewport::default();
        view.reset(20, 6);
        view.wheel(-60, 3);
        assert_eq!(view.first, 0);
        view.wheel(-60, 3);
        assert_eq!(view.first, 3);
        assert_eq!(view.selected, 3);
        view.wheel(-1200, 3);
        assert_eq!(view.range(), 14..20);
        view.wheel(-120, 3);
        assert_eq!(view.range(), 14..20);
        view.wheel(1200, 3);
        assert_eq!(view.first, 0);
        assert!(view.range().contains(&view.selected));
    }

    #[test]
    fn scrollbar_drag_can_reach_both_ends_of_a_large_result_set() {
        let mut view = Viewport::default();
        view.reset(2000, 6);
        let thumb = view.thumb(154, 366).unwrap();
        assert_eq!(thumb.height, 24);
        view.drag_thumb(10000, 154, 366);
        assert_eq!(view.range(), 1994..2000);
        assert_eq!(view.thumb(154, 366).unwrap().top + thumb.height, 520);
        view.drag_thumb(-100, 154, 366);
        assert_eq!(view.range(), 0..6);
    }

    #[test]
    fn changing_queries_and_capacity_keeps_the_view_valid() {
        let mut view = Viewport::default();
        view.reset(20, 6);
        view.scroll_rows(14);
        view.set_visible(3);
        assert!(view.range().contains(&view.selected));
        view.reset(2, 6);
        assert_eq!(view.range(), 0..2);
        assert_eq!(view.selected, 0);
        assert!(view.thumb(0, 120).is_none());
        view.reset(0, 6);
        view.page(1);
        view.wheel(-120, 3);
        assert_eq!(view.range(), 0..0);
    }

    #[test]
    fn page_navigation_and_system_wheel_settings_are_respected() {
        let mut view = Viewport::default();
        view.reset(20, 6);
        view.page(1);
        assert_eq!(view.selected, 6);
        view.wheel(-120, 0);
        assert_eq!(view.first, 1);
        view.wheel(-120, u32::MAX);
        assert_eq!(view.first, 7);
        view.page(-1);
        assert!(view.range().contains(&view.selected));
    }
}
