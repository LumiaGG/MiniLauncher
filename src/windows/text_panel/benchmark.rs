//! Opt-in native layout measurements. No windows are shown and no timing is asserted.
use super::*;
use crate::list::Viewport;
use serde_json::{json, Value};
use std::{ptr::{null, null_mut}, time::Instant};
use windows_sys::Win32::UI::WindowsAndMessaging::*;

fn summarize(name: &str, mut samples: Vec<f64>, lifecycle: (usize, usize)) -> Value {
    samples.sort_by(f64::total_cmp);
    let percentile =
        |p: usize| samples[((samples.len() * p).div_ceil(100) - 1).min(samples.len() - 1)];
    json!({"scenario":name,"samples":samples.len(),"median_ms":percentile(50),
        "p95_ms":percentile(95),"max_ms":samples.last(),
        "text_controls_created":lifecycle.0,"text_controls_destroyed":lifecycle.1})
}

unsafe fn frame(
    parent: HWND,
    panels: &mut Panels,
    content: &[Arc<TextPanel>],
    first: usize,
    engine: &crate::windows::gfx::TextEngine,
) -> f64 {
    let start = Instant::now();
    let mut viewport = Viewport::default();
    viewport.reset(content.len(), 10);
    viewport.set_heights(vec![280; content.len()]);
    viewport.first = first;
    panels.begin_layout();
    for index in viewport.range() {
        let geometry = Geometry::for_panel(
            856,
            68 + viewport.row_top(index),
            viewport.row_height(index),
            &content[index],
        );
        panels
            .layout(
                &format!("item-{index}"),
                &content[index],
                &geometry,
                engine,
            )
            .unwrap();
    }
    panels.end_layout(parent);
    start.elapsed().as_secs_f64() * 1000.0
}

#[test]
#[ignore = "manual performance measurement; run scripts/measure-text-panels.py"]
fn hidden_native_list() {
    let input = std::env::var("MINILAUNCHER_BENCH_INPUT").expect("snapshot path");
    let output = std::env::var("MINILAUNCHER_BENCH_OUTPUT").expect("report path");
    let snapshot: Value = serde_json::from_slice(&std::fs::read(input).unwrap()).unwrap();
    let source: Vec<Arc<TextPanel>> = snapshot["items"]
        .as_array()
        .unwrap()
        .iter()
        .filter_map(|item| item.get("text_panel"))
        .map(|panel| Arc::new(serde_json::from_value(panel.clone()).unwrap()))
        .collect();
    assert!(source.len() >= 10, "need at least ten panel results");
    let _ole = super::super::dragdrop::Ole::initialize().unwrap();
    unsafe {
        let parent = CreateWindowExW(
            0,
            super::super::wide("STATIC").as_ptr(),
            null(),
            WS_CLIPCHILDREN,
            0,
            0,
            1284,
            1050,
            null_mut(),
            null_mut(),
            null_mut(),
            null(),
        );
        assert!(!parent.is_null());
        let engine = crate::windows::gfx::TextEngine::create(true).unwrap();
        let mut reports = Vec::new();
        for count in [10, source.len().min(100)] {
            let content = &source[..count];
            let mut panels = Panels::default();
            // Warm native font/theme caches before measuring.
            frame(parent, &mut panels, content, 0, &engine);
            panels.clear(parent);
            let cold = (0..30)
                .map(|_| {
                    let start = Instant::now();
                    frame(parent, &mut panels, content, 0, &engine);
                    panels.clear(parent);
                    start.elapsed().as_secs_f64() * 1000.0
                })
                .collect();
            reports.push(summarize(
                &format!("{count}_items_cold_screen_including_cleanup"),
                cold,
                (0, 0),
            ));
            frame(parent, &mut panels, content, 0, &engine);
            let steady = (0..60)
                .map(|_| frame(parent, &mut panels, content, 0, &engine))
                .collect();
            reports.push(summarize(
                &format!("{count}_items_unchanged_visible_screen"),
                steady,
                (0, 0),
            ));
            let mut updated = content.to_vec();
            let updates = (0..30)
                .map(|iteration| {
                    for (index, panel) in updated.iter_mut().enumerate() {
                        let panel = Arc::make_mut(panel);
                        let block = &mut panel.blocks[0];
                        block.text =
                            format!("{}\nupdate {iteration}", content[index].blocks[0].text);
                    }
                    frame(parent, &mut panels, &updated, 0, &engine)
                })
                .collect();
            reports.push(summarize(
                &format!("{count}_items_same_ids_changed_text"),
                updates,
                (0, 0),
            ));
            panels.clear(parent);
            frame(parent, &mut panels, content, 0, &engine);
            let positions = (1..count - 2).chain((0..count - 3).rev());
            let scroll = positions
                .map(|first| frame(parent, &mut panels, content, first, &engine))
                .collect();
            reports.push(summarize(
                &format!("{count}_items_scroll_forward_back"),
                scroll,
                (0, 0),
            ));
            panels.clear(parent);
            // Mirror rows::reflow_rows text measurement, excluding other row types.
            let reflow = (0..30)
                .map(|_| {
                    let start = Instant::now();
                    for panel in content {
                        let geometry = Geometry::for_panel(856, 0, 600, panel);
                        let measured: Vec<i32> = panel
                            .blocks
                            .iter()
                            .zip(&geometry.areas)
                            .map(|(block, area)| {
                                engine.measure_multiline_height(
                                    &block.text, &engine.body_format,
                                    (area[2] - area[0] - 14).max(1) as f32,
                                ).unwrap().ceil().max(20.0) as i32
                            })
                            .collect();
                        std::hint::black_box(Geometry::preferred_height(
                            856, 600, panel, &measured,
                        ));
                    }
                    start.elapsed().as_secs_f64() * 1000.0
                })
                .collect();
            let text_engine = super::super::gfx::TextEngine::create(true).unwrap();
            let mut cache = super::super::rows::Measurements::default();
            let cold_start = Instant::now();
            for panel in content {
                std::hint::black_box(cache.panel_height(&text_engine, panel, 856, 600, 144));
            }
            let cold_ms = cold_start.elapsed().as_secs_f64() * 1000.0;
            let mut record = summarize(
                &format!("{count}_items_cached_first_measurement"),
                vec![cold_ms],
                (0, 0),
            );
            record["native_text_measurements"] = json!(cache.measured);
            reports.push(record);
            let before = cache.measured;
            let cached = (0..30)
                .map(|_| {
                    let start = Instant::now();
                    for panel in content {
                        std::hint::black_box(
                            cache.panel_height(&text_engine, panel, 856, 600, 144),
                        );
                    }
                    start.elapsed().as_secs_f64() * 1000.0
                })
                .collect();
            let mut record = summarize(
                &format!("{count}_items_cached_unchanged_text_measurement"),
                cached,
                (0, 0),
            );
            record["native_text_measurements"] = json!(cache.measured - before);
            reports.push(record);
            let before = cache.measured;
            let changed = (0..30)
                .map(|iteration| {
                    let mut changed = (*content[0]).clone();
                    changed.blocks[0]
                        .text
                        .push_str(&format!("\nmeasurement update {iteration}"));
                    let start = Instant::now();
                    for (index, panel) in content.iter().enumerate() {
                        let panel = if index == 0 { &changed } else { panel };
                        std::hint::black_box(
                            cache.panel_height(&text_engine, panel, 856, 600, 144),
                        );
                    }
                    start.elapsed().as_secs_f64() * 1000.0
                })
                .collect();
            let mut record = summarize(
                &format!("{count}_items_cached_one_block_changed"),
                changed,
                (0, 0),
            );
            record["native_text_measurements"] = json!(cache.measured - before);
            reports.push(record);
            reports.push(summarize(
                &format!("{count}_items_all_rows_text_measurement"),
                reflow,
                (0, 0),
            ));
        }
        assert_eq!(IsWindowVisible(parent), 0);
        DestroyWindow(parent);
        let report = json!({"dpi":144,"width_dip":856,"viewport_height_dip":600,
            "row_height_dip":280,"visible_rows":3,"source_items":source.len(),
            "scope":"Hidden native layout only; excludes host paint, DWM composition, IME and input latency. Each text control owns one additional clipping viewport.",
            "results":reports});
        std::fs::write(output, serde_json::to_vec_pretty(&report).unwrap()).unwrap();
        println!("{}", report);
    }
}
