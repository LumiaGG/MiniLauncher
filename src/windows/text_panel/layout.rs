use mini_launcher::result::{TextLayout, TextPanel};

const GAP: i32 = 12;
const FRAME_CHROME: i32 = 46;
pub(crate) enum Hit {
    None,
    Title,
    Header(usize),
    Content(usize),
    Retry,
}
pub(crate) struct Geometry {
    pub frames: Vec<[i32; 4]>,
    pub areas: Vec<[i32; 4]>,
    pub status: Option<[i32; 4]>,
    pub retry: Option<[i32; 4]>,
    title: [i32; 4],
}
fn contains(r: [i32; 4], x: i32, y: i32) -> bool {
    x >= r[0] && x < r[2] && y >= r[1] && y < r[3]
}
fn columns(width: i32, panel: &TextPanel) -> usize {
    if panel.layout == TextLayout::Vertical || width < 600 {
        1
    } else {
        panel.blocks.len().clamp(1, 2)
    }
}
fn has_header_status(panel: &TextPanel) -> bool {
    panel.has_status()
        && !(panel.state == mini_launcher::result::PanelState::Loading
            && !panel.blocks.is_empty()
            && panel.message.is_empty()
            && !panel.retry)
}
fn header(panel: &TextPanel) -> i32 {
    42 + if has_header_status(panel) { 36 } else { 0 }
}
impl Geometry {
    pub fn minimum_height(width: i32, panel: &TextPanel) -> i32 {
        let rows = panel.blocks.len().div_ceil(columns(width, panel)) as i32;
        (header(panel) + rows * 64 + (rows - 1).max(0) * GAP + 18).max(116)
    }
    pub fn preferred_height(width: i32, capacity: i32, panel: &TextPanel, measured: &[i32]) -> i32 {
        let cols = columns(width, panel);
        let height = header(panel)
            + 18
            + measured
                .chunks(cols)
                .map(|row| {
                    row.iter().copied().max().unwrap_or(20).max(20) + FRAME_CHROME
                })
                .sum::<i32>()
            + (measured.len().div_ceil(cols) as i32 - 1).max(0) * GAP;
        let minimum = Self::minimum_height(width, panel);
        height.clamp(minimum, capacity.max(minimum))
    }
    pub fn for_panel(width: i32, top: i32, height: i32, panel: &TextPanel) -> Self {
        let min_height = (header(panel) + 64).min(Self::minimum_height(width, panel));
        let height = height.max(min_height);
        let left = 36;
        let right = width - 36;
        let cols = columns(width, panel);
        let rows = panel.blocks.len().div_ceil(cols);
        let start = top + header(panel);
        let bottom = top + height - 18;
        let row_height = if rows == 0 {
            0
        } else {
            (bottom - start - (rows as i32 - 1) * GAP) / rows as i32
        };
        let mut frames = Vec::new();
        let ratio = if panel.blocks.len() == 2 && cols == 2 {
            let a = panel.blocks[0].text.chars().count();
            let b = panel.blocks[1].text.chars().count();
            if a < 120 && b > 240 && b > a * 3 {
                30
            } else if b > 0 && b < 120 && a > 240 && a > b * 3 {
                70
            } else {
                50
            }
        } else {
            50
        };
        for index in 0..panel.blocks.len() {
            let row = index / cols;
            let col = index % cols;
            let y = start + row as i32 * (row_height + GAP);
            let last_single = cols == 2 && index == panel.blocks.len() - 1 && col == 0;
            let split = left + (right - left - GAP) * ratio / 100;
            let (x, end) = if cols == 1 || last_single {
                (left, right)
            } else if col == 0 {
                (left, split)
            } else {
                (split + GAP, right)
            };
            frames.push([
                x,
                y,
                end,
                if row + 1 == rows {
                    bottom
                } else {
                    y + row_height
                },
            ]);
        }
        let areas = frames
            .iter()
            .map(|r| [r[0] + 12, r[1] + 36, r[2] - 10, r[3] - 10])
            .collect();
        let status = has_header_status(panel).then_some([left, top + 34, right, top + 66]);
        let retry = panel
            .retry
            .then_some([right - 88, top + 35, right, top + 63]);
        Self {
            frames,
            areas,
            status,
            retry,
            title: [left, top, right, top + 32],
        }
    }
    pub fn hit(&self, x: i32, y: i32) -> Hit {
        if self.retry.is_some_and(|r| contains(r, x, y)) {
            return Hit::Retry;
        }
        for (i, area) in self.areas.iter().enumerate() {
            if contains(*area, x, y) {
                return Hit::Content(i);
            }
        }
        for (i, frame) in self.frames.iter().enumerate() {
            if contains(*frame, x, y) {
                return Hit::Header(i);
            }
        }
        if contains(self.title, x, y) {
            Hit::Title
        } else {
            Hit::None
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use mini_launcher::result::{PanelState, TextBlock};
    fn panel(n: usize) -> TextPanel {
        TextPanel {
            blocks: (0..n)
                .map(|i| TextBlock {
                    id: i.to_string(),
                    title: "内容".into(),
                    text: "中文".into(),
                    selectable: true,
                })
                .collect(),
            ..Default::default()
        }
    }
    #[test]
    fn text_frames_leave_room_below_content_without_exceeding_capacity() {
        for layout in [TextLayout::Horizontal, TextLayout::Vertical] {
            let mut panel = panel(2);
            panel.layout = layout;
            for measured in [20, 100] {
                let height = Geometry::preferred_height(856, 600, &panel, &[measured; 2]);
                let geometry = Geometry::for_panel(856, 68, height, &panel);
                for area in geometry.areas {
                    assert!(area[3] - area[1] >= measured);
                }
            }
            assert_eq!(Geometry::preferred_height(856, 300, &panel, &[1000; 2]), 300);
        }
    }

    #[test]
    fn all_layouts_share_bounded_measurement_and_hit_regions() {
        for width in [480, 600, 856] {
            for n in 1..=4 {
                for layout in [
                    TextLayout::Auto,
                    TextLayout::Horizontal,
                    TextLayout::Vertical,
                ] {
                    let mut panel = panel(n);
                    panel.layout = layout;
                    let height = Geometry::preferred_height(width, 160, &panel, &vec![20; n]);
                    let g = Geometry::for_panel(width, 68, height, &panel);
                    assert_eq!(g.areas.len(), n);
                    for (i, area) in g.areas.iter().enumerate() {
                        assert!(area[2] > area[0] && area[3] > area[1]);
                        assert!(matches!(g.hit(area[0]+1, area[1]+1), Hit::Content(j) if i == j));
                        assert!(area[3] < 68 + height);
                    }
                }
            }
        }
    }
    #[test]
    fn status_retry_and_short_original_have_dedicated_space() {
        let mut panel = panel(2);
        panel.blocks[0].text = "s".into();
        panel.blocks[1].text = "释义".repeat(200);
        panel.state = PanelState::Error;
        panel.retry = true;
        let g = Geometry::for_panel(856, 68, 400, &panel);
        assert!(g.areas[1][2] - g.areas[1][0] > g.areas[0][2] - g.areas[0][0]);
        let retry = g.retry.unwrap();
        assert!(matches!(g.hit(retry[0] + 1, retry[1] + 1), Hit::Retry));
        assert!(g.frames[0][1] > g.status.unwrap()[3]);
    }
    #[test]
    fn loading_panel_with_blocks_shares_ready_header_and_balanced_columns() {
        let mut loading = panel(2);
        loading.state = PanelState::Loading;
        loading.blocks[0].text = "a".repeat(300);
        loading.blocks[1].text = String::new();
        let mut ready = loading.clone();
        ready.state = PanelState::Ready;
        assert_eq!(
            Geometry::minimum_height(856, &loading),
            Geometry::minimum_height(856, &ready)
        );
        let g = Geometry::for_panel(856, 68, 240, &loading);
        assert!(g.status.is_none());
        assert_eq!(g.areas[0][2] - g.areas[0][0], g.areas[1][2] - g.areas[1][0]);
    }
}
