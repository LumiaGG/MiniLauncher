//! Local codec comparison, not an end-to-end plugin/GUI benchmark.
use serde::Deserialize;
use serde_json::{json, Value};
use std::{hint::black_box, time::Instant};

#[derive(Deserialize)]
struct Response {
    items: Vec<Item>,
}
#[allow(dead_code)]
#[derive(Deserialize)]
struct Item {
    title: String,
    subtitle: String,
    action: Value,
    text_panel: Option<Value>,
}

fn measure(mut f: impl FnMut()) -> (u128, u128) {
    for _ in 0..100 {
        f();
    }
    let mut samples = Vec::new();
    for _ in 0..1000 {
        let start = Instant::now();
        f();
        samples.push(start.elapsed().as_nanos());
    }
    samples.sort_unstable();
    (samples[500] / 1000, samples[950] / 1000)
}
fn main() {
    for (name, result) in [
        (
            "100-items",
            json!({"items":(0..100).map(|i| json!({"title":format!("测试结果 {i}"),
            "subtitle":"目录与文件说明".repeat(4),"action":{"type":"copy","text":"测试".repeat(30)}})).collect::<Vec<_>>()}),
        ),
        (
            "text-panel",
            json!({"items":[{"title":"翻译","subtitle":"","action":{"type":"copy","text":"译文".repeat(6000)},
            "text_panel":{"blocks":[{"id":"source","title":"原文","text":"source text ".repeat(3000)},{"id":"translation","title":"译文","text":"译文".repeat(6000)}]}}]}),
        ),
    ] {
        let bytes = serde_json::to_vec(&result).unwrap();
        let old = measure(|| {
            let parsed: Value = serde_json::from_slice(black_box(&bytes)).unwrap();
            let items: Vec<Item> = parsed["items"]
                .as_array()
                .unwrap()
                .iter()
                .map(|item| serde_json::from_value(item.clone()).unwrap())
                .collect();
            black_box(items);
        });
        let new = measure(|| {
            let parsed: Value = serde_json::from_slice(black_box(&bytes)).unwrap();
            let items: Response = serde_json::from_value(parsed).unwrap();
            black_box(items.items);
        });
        let encode = measure(|| {
            black_box(serde_json::to_vec(black_box(&result)).unwrap());
        });
        println!("{name}: bytes={} old_decode_p50/p95_us={}/{} consuming_decode_p50/p95_us={}/{} encode_p50/p95_us={}/{}",
            bytes.len(), old.0, old.1, new.0, new.1, encode.0, encode.1);
    }
}
