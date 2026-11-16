use super::{FOOTER_HEIGHT, IDLE_HEIGHT, ROW_HEIGHT, ROW_TOP};
use windows_sys::Win32::Foundation::{POINT, RECT};

const MARGIN: i32 = 8;

pub(super) fn px(value: i32, dpi: u32) -> i32 {
    (value as f64 * dpi as f64 / 96.0).round() as i32
}

pub(super) struct Layout {
    pub dpi: u32,
    pub x: i32,
    pub y: i32,
    pub rows: usize,
}

impl Layout {
    // Work areas are physical desktop coordinates, including negative origins.
    // Keep the monitor's scale unless even the minimum panel cannot fit.
    #[cfg(test)]
    pub fn new(
        area: RECT,
        dpi: u32,
        panel_width: i32,
        max_rows: usize,
        position: Option<POINT>,
    ) -> Self {
        Self::with_minimum_rows(area, dpi, panel_width, max_rows, 1, position)
    }
    pub fn with_minimum_rows(
        area: RECT,
        dpi: u32,
        panel_width: i32,
        max_rows: usize,
        minimum_rows: usize,
        position: Option<POINT>,
    ) -> Self {
        let minimum_rows = minimum_rows.max(1);
        let min_height = ROW_TOP + minimum_rows as i32 * ROW_HEIGHT + 8 + FOOTER_HEIGHT;
        let width = (area.right - area.left - 2 * MARGIN).max(1);
        let height = (area.bottom - area.top - 2 * MARGIN).max(1);
        let dpi = dpi
            .max(1)
            .min((width as u32 * 96 / panel_width as u32).max(1))
            .min((height as u32 * 96 / min_height as u32).max(1));
        let left = area.left + MARGIN;
        let top = area.top + MARGIN;
        let bottom = area.bottom - MARGIN;
        let preferred = position.unwrap_or(POINT {
            x: area.left + (area.right - area.left - px(panel_width, dpi)) / 2,
            y: area.top + (area.bottom - area.top - px(IDLE_HEIGHT, dpi)) / 3,
        });
        let x = preferred
            .x
            .clamp(left, (area.right - MARGIN - px(panel_width, dpi)).max(left));
        let y = preferred
            .y
            .clamp(top, (bottom - px(min_height, dpi)).max(top));
        // Test physical sizes so fractional DPI rounding cannot clip the footer.
        let rows = (1..=max_rows.max(minimum_rows))
            .take_while(|rows| {
                px(ROW_TOP + *rows as i32 * ROW_HEIGHT + 8 + FOOTER_HEIGHT, dpi) <= bottom - y
            })
            .last()
            .unwrap_or(1);
        Self { dpi, x, y, rows }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn document_minimum_height_overrides_one_row_setting_and_fits_small_monitors() {
        for area in [
            RECT {
                left: 0,
                top: 0,
                right: 1280,
                bottom: 720,
            },
            RECT {
                left: 0,
                top: 0,
                right: 800,
                bottom: 480,
            },
        ] {
            let result = Layout::with_minimum_rows(area, 144, 856, 1, 7, None);
            assert!(result.rows >= 7);
            assert!(
                result.y + px(ROW_TOP + 7 * ROW_HEIGHT + 8 + FOOTER_HEIGHT, result.dpi)
                    <= area.bottom - MARGIN
            );
            assert!(result.x + px(856, result.dpi) <= area.right - MARGIN);
        }
    }
    use crate::config::DEFAULT_WINDOW_WIDTH as WIDTH;

    #[test]
    fn mixed_dpi_and_small_work_areas_keep_the_panel_on_screen() {
        for (area, dpi) in [
            (
                RECT {
                    left: 0,
                    top: 0,
                    right: 1920,
                    bottom: 1040,
                },
                96,
            ),
            (
                RECT {
                    left: -2560,
                    top: -400,
                    right: 0,
                    bottom: 1000,
                },
                144,
            ),
            (
                RECT {
                    left: 1920,
                    top: 0,
                    right: 5760,
                    bottom: 2080,
                },
                192,
            ),
            (
                RECT {
                    left: -800,
                    top: 100,
                    right: 0,
                    bottom: 660,
                },
                192,
            ),
            (
                RECT {
                    left: 0,
                    top: 0,
                    right: 1920,
                    bottom: 300,
                },
                288,
            ),
        ] {
            for position in [
                None,
                Some(POINT {
                    x: area.right - 20,
                    y: area.bottom - 20,
                }),
            ] {
                let layout = Layout::new(area, dpi, WIDTH, 8, position);
                assert!(layout.dpi <= dpi);
                assert!(layout.x >= area.left && layout.y >= area.top);
                assert!(layout.x + px(WIDTH, layout.dpi) <= area.right - MARGIN);
                let height = ROW_TOP + layout.rows as i32 * ROW_HEIGHT + 8 + FOOTER_HEIGHT;
                assert!(layout.y + px(height, layout.dpi) <= area.bottom - MARGIN);
                assert!((1..=8).contains(&layout.rows));
            }
        }
    }

    #[test]
    fn returning_to_a_large_display_restores_scale_and_row_capacity() {
        let area = RECT {
            left: 0,
            top: 0,
            right: 3840,
            bottom: 2080,
        };
        let layout = Layout::new(area, 192, WIDTH, 8, None);
        assert_eq!(layout.dpi, 192);
        assert_eq!(layout.rows, 8);
        for dpi in [96, 120, 144, 168, 192, 216, 240] {
            let layout = Layout::new(area, dpi, WIDTH, 8, None);
            assert_eq!(layout.dpi, dpi);
            assert_eq!(px(WIDTH, dpi), WIDTH * dpi as i32 / 96);
        }
        let moved = Layout::new(area, 192, WIDTH, 8, Some(POINT { x: 100, y: 100 }));
        assert_eq!((moved.x, moved.y), (100, 100));
        assert_eq!(Layout::new(area, 120, WIDTH, 3, None).rows, 3);
    }

    #[test]
    fn configured_width_fits_the_monitor_and_preserves_dpi_when_possible() {
        let area = RECT {
            left: -1920,
            top: 0,
            right: 0,
            bottom: 1040,
        };
        for width in [480, 856, 1440] {
            for dpi in [96, 144, 192] {
                let layout = Layout::new(area, dpi, width, 6, None);
                assert!(layout.x >= area.left + MARGIN);
                assert!(layout.x + px(width, layout.dpi) <= area.right - MARGIN);
                if px(width, dpi) <= area.right - area.left - 2 * MARGIN {
                    assert_eq!(layout.dpi, dpi);
                }
            }
        }
    }
}
