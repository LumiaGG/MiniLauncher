use super::*;
use crate::plugin::timestamp;
use mini_launcher::plugin_rpc::MAX_FRAME;

#[test]
fn file_history_persists_searches_deduplicates_and_rejects_missing_groups() {
    let root = std::env::temp_dir().join(format!(
        "mini-clipboard-files-{}",
        history::random_id().unwrap()
    ));
    fs::create_dir_all(root.join("文件夹")).unwrap();
    let file = root.join("报告.txt");
    fs::write(&file, "original").unwrap();
    let paths = vec![
        file.to_str().unwrap().to_string(),
        root.join("文件夹").to_str().unwrap().to_string(),
    ];
    let mut state = State {
        history: History::default(),
        path: root.join("history.dat"),
        maximum: 100,
        error: None,
        revision: history::random_id().unwrap(),
        window: 0,
    };
    // Old text-only history remains readable with no files field.
    fs::write(&state.path, serde_json::to_vec(&json!({"paused":false,"entries":[{"id":"00000000000000000000000000000000","text":"legacy","time":0}]})).unwrap()).unwrap();
    state.history = history::load(&state.path).unwrap();
    assert!(state.history.entries[0].files.is_none());
    state.capture_files(paths.clone()).unwrap();
    let id = state.history.entries[0].id.clone();
    execute(&mut state, &format!("pin:{id}")).unwrap();
    state.capture("other text".into()).unwrap();
    state.capture_files(paths.clone()).unwrap();
    state.capture_files(paths.clone()).unwrap();
    assert_eq!(state.history.entries.len(), 3);
    assert_eq!(state.history.entries[0].id, id);
    assert_eq!(
        history::load(&state.path).unwrap().entries[0].files,
        Some(paths.clone())
    );
    let items = query(&state, "file 报告", 100);
    assert_eq!(items.len(), 1);
    assert_eq!(items[0]["drag_id"], id);
    assert_eq!(items[0]["paste_after_execute"], true);
    assert_eq!(items[0]["title"], "置顶 · 报告.txt · 共 2 项");
    assert_eq!(items[0]["icon"], "Images/files.ico");
    assert!(items[0]["subtitle"]
        .as_str()
        .unwrap()
        .ends_with(&timestamp(state.history.entries[0].time)));
    let drag = prepare_drag(&state, &id).unwrap();
    assert_eq!(drag["data"], json!({"type":"files","paths":paths}));
    fs::remove_file(&file).unwrap();
    let copy = format!("copy:{id}");
    // This fails before clipboard access, even with a null owner window.
    assert!(execute(&mut state, &copy).unwrap_err().contains("报告.txt"));
    assert!(prepare_drag(&state, &id).is_err());
    state.history.paused = true;
    state.capture_files(vec![r"C:\ignored.txt".into()]).unwrap();
    assert_eq!(state.history.entries.len(), 3);
    state.history.paused = false;
    assert!(state.capture_files(vec![r"C:\x".into(); 65]).is_err());
    execute(&mut state, &format!("delete:{id}")).unwrap();
    assert!(root.join("文件夹").is_dir()); // History deletion never removes originals.
    let mut saved = serde_json::to_value(&state.history).unwrap();
    saved["entries"][0]["files"] = json!(["relative"]);
    fs::write(&state.path, serde_json::to_vec(&saved).unwrap()).unwrap();
    assert!(history::load(&state.path).is_err());
    fs::remove_dir_all(root).unwrap();
}

#[test]
fn search_and_actions_preserve_text_and_reject_stale_clear() {
    let root = std::env::temp_dir().join(format!(
        "mini-clipboard-actions-{}",
        history::random_id().unwrap()
    ));
    let mut state = State {
        history: History::default(),
        path: root.join("history.dat"),
        maximum: 100,
        error: None,
        revision: history::random_id().unwrap(),
        window: 0,
    };
    state.capture("Hello\r\n剪贴板 World".into()).unwrap();
    let items = query(&state, "WORLD hello", 100);
    assert_eq!(items.len(), 1);
    assert_eq!(items[0]["title"], "Hello 剪贴板 World");
    assert_eq!(
        items[0]["subtitle"],
        timestamp(state.history.entries[0].time)
    );
    assert_eq!(items[0]["icon"], "Images/text.ico");
    assert_eq!(items[0]["adaptive_height"], false);
    assert_eq!(items[0]["paste_after_execute"], true);
    let copy = items[0]["action"]["id"].as_str().unwrap();
    assert_eq!(
        prepare_drag(&state, items[0]["drag_id"].as_str().unwrap()).unwrap()["data"]["text"],
        "Hello\r\n剪贴板 World"
    );
    state.capture("unrelated clipboard text".into()).unwrap();
    state.capture("Hello\r\n剪贴板 World".into()).unwrap();
    assert_eq!(
        execute(&mut state, copy).unwrap()["action"]["text"],
        "Hello\r\n剪贴板 World"
    );
    let clear = query(&state, "/clear", 100)[0]["action"]["id"]
        .as_str()
        .unwrap()
        .to_string();
    state.capture("new text".into()).unwrap();
    assert!(execute(&mut state, &clear).is_err());
    let delete = items[0]["context_actions"]
        .as_array()
        .unwrap()
        .iter()
        .find(|action| action["title"] == "删除此条历史")
        .unwrap()["action"]["id"]
        .as_str()
        .unwrap();
    execute(&mut state, delete).unwrap();
    assert!(execute(&mut state, copy).is_err());
    execute(&mut state, "pause").unwrap();
    assert!(history::load(&state.path).unwrap().paused);
    execute(&mut state, "resume").unwrap();
    let clear = query(&state, "/clear", 100)[0]["action"]["id"]
        .as_str()
        .unwrap()
        .to_string();
    execute(&mut state, &clear).unwrap();
    assert!(history::load(&state.path).unwrap().entries.is_empty());
    assert!(query(&state, "", 0).is_empty());
    fs::remove_dir_all(root).unwrap();
}

#[test]
fn result_frames_fit_protocol_limits() {
    let mut state = State {
        history: History::default(),
        path: PathBuf::new(),
        maximum: 500,
        error: None,
        revision: "revision".into(),
        window: 0,
    };
    for i in 0..100 {
        state.history.entries.push(history::Entry {
            id: format!("{i:032x}"),
            text: "中\n\t\"\\".repeat(2000),
            time: 0,
            image: None,
            files: None,
        });
    }
    let items = query(&state, "", 100);
    assert_eq!(items.len(), 100);
    let response = json!({"jsonrpc":"2.0","id":1,"result":{"items":items}});
    assert!(serde_json::to_vec(&response).unwrap().len() < MAX_FRAME);
}

#[test]
fn pinned_items_sort_persist_and_survive_eviction_until_unpinned_or_cleared() {
    let root = std::env::temp_dir().join(format!(
        "mini-clipboard-pins-{}",
        history::random_id().unwrap()
    ));
    let mut state = State {
        history: History::default(),
        path: root.join("history.dat"),
        maximum: 2,
        error: None,
        revision: history::random_id().unwrap(),
        window: 0,
    };
    state.capture("keep".into()).unwrap();
    let id = state.history.entries[0].id.clone();
    execute(&mut state, &format!("pin:{id}")).unwrap();
    state.capture("older".into()).unwrap();
    state.capture("latest".into()).unwrap();
    assert!(history::load(&state.path).unwrap().pinned.contains(&id));
    assert_eq!(state.history.entries.len(), 2);
    assert!(!state
        .history
        .entries
        .iter()
        .any(|entry| entry.text == "older"));
    let items = query(&state, "", 100);
    assert_eq!(items[0]["drag_id"], id);
    assert_eq!(items[0]["context_actions"][0]["title"], "取消置顶");
    assert_eq!(query(&state, "latest", 100).len(), 1);

    execute(&mut state, &format!("unpin:{id}")).unwrap();
    state.capture("newest".into()).unwrap();
    assert!(!state.history.entries.iter().any(|entry| entry.id == id));
    let retained = state.history.entries[0].id.clone();
    execute(&mut state, &format!("pin:{retained}")).unwrap();
    state.maximum = 1;
    state.commit(state.history.clone()).unwrap();
    state
        .capture_image(mini_launcher::clipboard_image::Image {
            width: 1,
            height: 1,
            pixels: vec![0, 0, 255, 255],
        })
        .unwrap();
    assert_eq!(state.history.entries.len(), 1);
    assert_eq!(fs::read_dir(root.join("images")).unwrap().count(), 0);
    let items = query(&state, "", 100);
    let clear = items[0]["context_actions"]
        .as_array()
        .unwrap()
        .iter()
        .find(|action| action["title"] == "清空历史（含置顶）")
        .unwrap();
    assert_eq!(clear["confirm"], true);
    execute(&mut state, clear["action"]["id"].as_str().unwrap()).unwrap();
    let saved = history::load(&state.path).unwrap();
    assert!(saved.entries.is_empty() && saved.pinned.is_empty());
    fs::remove_dir_all(root).unwrap();
}

#[test]
fn png_images_are_lossless_deduplicated_and_removed_with_their_history() {
    let root = std::env::temp_dir().join(format!(
        "mini-clipboard-image-{}",
        history::random_id().unwrap()
    ));
    let mut state = State {
        history: History::default(),
        path: root.join("history.dat"),
        maximum: 100,
        error: None,
        revision: history::random_id().unwrap(),
        window: 0,
    };
    let image = mini_launcher::clipboard_image::Image {
        width: 2,
        height: 1,
        pixels: vec![0, 0, 255, 255, 255, 0, 0, 128],
    };
    state.capture_image(image.clone()).unwrap();
    let id = state.history.entries[0].id.clone();
    execute(&mut state, &format!("pin:{id}")).unwrap();
    let path = state.image_path(&id, false);
    let preview = state.image_path(&id, true);
    assert!(fs::read(&path).unwrap().starts_with(b"\x89PNG\r\n\x1a\n"));
    assert_eq!(
        mini_launcher::clipboard_image::decode(&fs::read(&path).unwrap(), false)
            .unwrap()
            .pixels,
        image.pixels
    );
    assert!(preview.is_file());
    let items = query(&state, "图片", 100);
    assert_eq!(items[0]["title"], "置顶 · 图片");
    assert_eq!(items[0]["icon"], "Images/image.ico");
    assert_eq!(
        items[0]["subtitle"],
        timestamp(state.history.entries[0].time)
    );
    assert_eq!(items[0]["preview"]["width"], 2);
    assert!(items[0]["adaptive_height"].as_bool().unwrap());
    state.capture("text".into()).unwrap();
    state.capture_image(image).unwrap();
    assert_eq!(state.history.entries[0].id, id);
    assert_eq!(state.history.entries.len(), 2);
    assert!(history::load(&state.path).unwrap().entries[0]
        .image
        .is_some());
    execute(&mut state, &format!("delete:{id}")).unwrap();
    assert!(!history::load(&state.path).unwrap().pinned.contains(&id));
    assert!(!path.exists() && !preview.exists());
    fs::remove_dir_all(root).unwrap();
}
