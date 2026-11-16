use super::*;
use crate::plugin_trigger::GlobalTrigger;
use crate::{
    config,
    plugin_catalog::{parse_manifest, search_catalog},
};
use serde_json::Value;
use std::{
    fs,
    path::{Path, PathBuf},
};

fn plugin(id: &str, keywords: &[&str]) -> Plugin {
    parse_manifest(
        &json!({"id":id,"name":id,"protocol_version":2,
            "executable":"missing.exe","keywords":keywords})
        .to_string(),
        Path::new("plugin.json"),
    )
    .unwrap()
}

#[test]
fn plugin_action_labels_are_optional_bounded_and_do_not_override_concrete_actions() {
    let plugin = plugin("generic-action", &["*"]);
    let mut item = json!({"title":"Result","action":{"type":"execute","id":"save"}});
    let parse = |item: Value| parse_items(json!({"items":[item]}), &plugin);
    assert_eq!(parse(item.clone()).unwrap()[0].action_label(), "执行");
    item["action_label"] = json!(" 保存 ");
    assert_eq!(parse(item.clone()).unwrap()[0].action_label(), "保存");
    for invalid in ["", "\n", "bad\nlabel", "12345678901234567"] {
        item["action_label"] = json!(invalid);
        assert!(parse(item.clone()).is_err());
    }
    item["action_label"] = json!("保存");
    item["action"] = json!({"type":"copy","text":"text"});
    assert_eq!(parse(item).unwrap()[0].action_label(), "复制");
}

#[test]
fn query_context_tracks_exclusive_routes_including_direct_display() {
    let mut first = plugin("first", &["one", "alias"]);
    first.manifest.direct_display = true;
    let routing = crate::plugins::QueryRouting::new(vec![
        first,
        plugin("second", &["two"]),
        plugin("global", &["*"]),
    ]);
    assert_eq!(routing.exclusive_context("one a"), ["first"]);
    assert_eq!(routing.exclusive_context(" ALIAS b"), ["first"]);
    assert_eq!(routing.exclusive_context("two a"), ["second"]);
    assert_eq!(routing.exclusive_context("  "), ["first"]);
    assert!(routing.exclusive_context("ordinary input").is_empty());
    assert!(routing.exclusive_context(">plugins").is_empty());
}

#[test]
fn plugin_priority_defaults_and_rejects_invalid_manifest_values() {
    assert_eq!(plugin("default", &["*"]).manifest.priority, 0);
    let mut manifest = json!({"id":"ranked","name":"Ranked","protocol_version":2,
        "executable":"missing.exe","keywords":["*"],"priority":100});
    for value in [i32::MIN, -1, 0, 100, i32::MAX] {
        manifest["priority"] = json!(value);
        let parsed = parse_manifest(&manifest.to_string(), Path::new("plugin.json")).unwrap();
        assert_eq!(parsed.manifest.priority, value);
    }
    for value in [
        json!(2147483648_i64),
        json!(-2147483649_i64),
        json!(1.5),
        json!("100"),
        json!(null),
    ] {
        manifest["priority"] = value;
        assert!(parse_manifest(&manifest.to_string(), Path::new("plugin.json")).is_err());
    }
}

#[test]
fn plugin_priority_ranks_open_actions_across_sources_before_deduplication() {
    use crate::providers::{Groups, Source, Update, UpdateKind};
    for priority in [i32::MIN, -1, 0, 1, i32::MAX] {
        let mut plugin = plugin("generic-provider", &["*"]);
        plugin.manifest.priority = priority;
        let plugin_entries = parse_items(
            json!({"items":[
                {"title":"plugin duplicate","action":{"type":"open","target":"D:/shared"}},
                {"title":"plugin unique","action":{"type":"open","target":"D:/unique"}}
            ]}),
            &plugin,
        )
        .unwrap();
        assert!(plugin_entries
            .iter()
            .all(|entry| entry.query_priority == priority));
        for sources in [
            [Source::Apps, Source::Files, Source::Plugins],
            [Source::Plugins, Source::Files, Source::Apps],
        ] {
            let mut groups = Groups::default();
            let mut results = Vec::new();
            for source in sources {
                let entries = match source {
                    Source::Apps => vec![Entry::new(
                        "app".into(),
                        String::new(),
                        Action::Open("D:/app".into()),
                        "应用",
                    )],
                    Source::Files => vec![Entry::new(
                        "file duplicate".into(),
                        String::new(),
                        Action::Open("D:/shared".into()),
                        "文件",
                    )],
                    Source::Plugins => plugin_entries.clone(),
                };
                results = groups.apply(Update {
                    kind: UpdateKind::Results,
                    generation: 1,
                    source,
                    entries,
                    exclusive: false,
                    pending: false,
                    status: None,
                });
            }
            let titles: Vec<_> = results.iter().map(|entry| entry.title.as_str()).collect();
            if priority > 0 {
                assert_eq!(titles, ["plugin duplicate", "plugin unique", "app"]);
            } else {
                assert_eq!(titles, ["app", "file duplicate", "plugin unique"]);
            }
            // Removing plugin results restores the lower-priority file duplicate.
            let results = groups.apply(Update {
                kind: UpdateKind::Results,
                generation: 1,
                source: Source::Plugins,
                entries: vec![],
                exclusive: false,
                pending: false,
                status: None,
            });
            assert_eq!(
                results
                    .iter()
                    .map(|entry| entry.title.as_str())
                    .collect::<Vec<_>>(),
                ["app", "file duplicate"]
            );
        }
    }
}

#[test]
fn plugin_priority_orders_global_and_direct_routes_stably() {
    let mut plugins = vec![
        plugin("default", &["*"]),
        plugin("low", &["*"]),
        plugin("high", &["*", "explicit"]),
        plugin("tie", &["*"]),
    ];
    plugins[1].manifest.priority = i32::MIN;
    plugins[2].manifest.priority = i32::MAX;
    plugins[3].manifest.priority = i32::MAX;
    for plugin in &mut plugins {
        plugin.manifest.direct_display = true;
    }
    for input in ["ordinary query", "  "] {
        let (selected, _, exclusive) = route_plugins(&plugins, input);
        assert_eq!(
            selected
                .iter()
                .map(|p| p.manifest.id.as_str())
                .collect::<Vec<_>>(),
            ["high", "tie", "default", "low"]
        );
        assert_eq!(exclusive, input.trim().is_empty());
    }
    plugins[2].manifest.priority = i32::MIN;
    let (selected, query, exclusive) = route_plugins(&plugins, "explicit text");
    assert_eq!(selected.len(), 1);
    assert_eq!(selected[0].manifest.id, "high");
    assert_eq!(query, "text");
    assert!(exclusive);
    plugins[2].manifest.global_trigger = GlobalTrigger::Path;
    assert!(!route_plugins(&plugins, "ordinary query")
        .0
        .iter()
        .any(|p| p.manifest.id == "high"));
}

#[test]
fn wildcard_routes_full_input_and_explicit_keywords_take_precedence() {
    let plugins = vec![plugin("global", &["*"]), plugin("paths", &["*", "path"])];
    let input = "  \"D:/Some Folder/file.txt\"  ";
    let (selected, query, exclusive) = route_plugins(&plugins, input);
    assert_eq!(selected.len(), 2);
    assert_eq!(query, input);
    assert!(!exclusive);
    let (selected, query, exclusive) = route_plugins(&plugins, " PATH  D:/Some Folder ");
    assert_eq!(selected.len(), 1);
    assert_eq!(selected[0].manifest.id, "paths");
    assert_eq!(query, "D:/Some Folder ");
    assert!(exclusive);
    for input in ["", "   ", ">settings", " >removed", "设置"] {
        assert!(route_plugins(&plugins, input).0.is_empty());
    }
    // '*' has wildcard semantics even when it occurs in the input itself.
    assert_eq!(route_plugins(&plugins, "* text").1, "* text");
}

#[test]
fn trigger_filters_global_queries_before_starting_a_process() {
    let mut paths = plugin("paths", &["*", "path"]);
    paths.manifest.global_trigger = GlobalTrigger::Path;
    let (sender, receiver) = mpsc::channel();
    let mut host = Host::new(
        PluginSettings {
            enabled: false,
            ..Default::default()
        },
        Arc::new(move |update| {
            sender.send(update).unwrap();
        }),
        Arc::new(|_, _| {}),
        Arc::new(AtomicU64::new(1)),
    );
    host.plugins = vec![paths];
    host.query(1, "chrome");
    let update = receiver.recv().unwrap();
    assert!(!update.pending);
    assert!(update.status.is_none());
    assert!(host.sessions.is_empty());
    assert_eq!(route_plugins(&host.plugins, "D:/Folder").0.len(), 1);
    // Explicit queries bypass trigger rules, including relative names.
    let (selected, query, exclusive) = route_plugins(&host.plugins, "path relative.txt");
    assert_eq!(selected.len(), 1);
    assert_eq!(query, "relative.txt");
    assert!(exclusive);
    let manifest = json!({"id":"bad", "name":"bad", "protocol_version":2,
            "executable":"missing.exe", "keywords":["*"], "global_trigger":{"type":"prefix","values":[]}});
    assert!(parse_manifest(&manifest.to_string(), Path::new("plugin.json")).is_err());
}

fn rpc_fixture(root: &Path, id: &str, before_response: &str) -> Plugin {
    fs::create_dir_all(root).unwrap();
    let fixture = root.join(format!("{id}.cmd"));
    let response = json!({"jsonrpc":"2.0","id":2,"result":{"items":[{
            "title":id,"action":{"type":"execute","id":"open"}}]}})
    .to_string()
    .replace("\"id\":2", "\"id\":%rpcid%");
    fs::write(
            &fixture,
            format!(
                concat!(
            "@echo off\r\nset /p request=\r\n",
            "echo {{\"jsonrpc\":\"2.0\",\"id\":1,\"result\":{{\"protocol_version\":2}}}}\r\n",
            "set /a rpcid=1 >nul\r\n:queries\r\nset \"request=\"\r\nset /p request=\r\n",
            "if not defined request exit /b\r\nset /a rpcid+=1 >nul\r\n{}\r\necho {}\r\ngoto queries\r\n"),
                before_response, response
            ),
        )
        .unwrap();
    let mut plugin = plugin(id, &["*"]);
    plugin.directory = root.to_path_buf();
    plugin.manifest.executable = PathBuf::from(std::env::var_os("COMSPEC").unwrap());
    plugin.manifest.args = vec![
        "/d".into(),
        "/q".into(),
        "/c".into(),
        fixture.to_str().unwrap().into(),
    ];
    plugin.manifest.timeout_ms = 2000;
    plugin
}

#[test]
fn drag_payload_uses_a_separate_read_only_plugin_request() {
    let root = Path::new(env!("CARGO_MANIFEST_DIR"))
        .join("artifacts")
        .join(format!("drag-rpc-{}", std::process::id()));
    let log = root.join("request.json");
    let fixture = rpc_fixture(&root, "drag", "");
    fs::write(root.join("drag.cmd"),format!(concat!(
            "@echo off\r\nset /p initialize=\r\n",
            "echo {{\"jsonrpc\":\"2.0\",\"id\":1,\"result\":{{\"protocol_version\":2}}}}\r\n",
            "set /p request=\r\necho %request%>\"{}\"\r\n",
            "echo {{\"jsonrpc\":\"2.0\",\"id\":2,\"result\":{{\"data\":{{\"type\":\"text\",\"text\":\"full\\r\\ntext\"}}}}}}\r\n"),log.display())).unwrap();
    let mut host = Host::new(
        PluginSettings {
            enabled: false,
            ..Default::default()
        },
        Arc::new(|_| {}),
        Arc::new(|_, _| {}),
        Arc::new(AtomicU64::new(1)),
    );
    host.plugins = vec![fixture];
    let data = host.prepare_drag("drag", "stable-id").unwrap();
    assert!(matches!(data,crate::drag_data::DragData::Text {text} if text == "full\r\ntext"));
    drop(host);
    let request: Value = serde_json::from_str(fs::read_to_string(&log).unwrap().trim()).unwrap();
    assert_eq!(request["method"], "prepare_drag");
    assert_eq!(request["params"]["action_id"], "stable-id");
    fs::remove_dir_all(root).unwrap();
}

#[test]
fn direct_display_routes_empty_input_without_leaking_into_other_scopes() {
    let root = Path::new(env!("CARGO_MANIFEST_DIR"))
        .join("artifacts")
        .join(format!("direct-display-{}", std::process::id()));
    let log = root.join("queries.jsonl");
    let mut direct = rpc_fixture(
        &root,
        "direct",
        &format!("echo %request%>>\"{}\"", log.display()),
    );
    direct.manifest.direct_display = true;
    direct.manifest.keywords = vec!["clip".into()];
    let ordinary = rpc_fixture(&root, "ordinary", "");
    let (sender, receiver) = mpsc::channel();
    let mut host = Host::new(
        PluginSettings {
            enabled: false,
            ..Default::default()
        },
        Arc::new(move |update| {
            sender.send(update).unwrap();
        }),
        Arc::new(|_, _| {}),
        Arc::new(AtomicU64::new(1)),
    );
    host.plugins = vec![ordinary, direct];
    host.query(1, "  \t");
    let updates: Vec<_> = receiver.try_iter().collect();
    assert_eq!(updates[0].kind, crate::providers::UpdateKind::State);
    assert!(updates[0].pending && updates[0].status.is_none());
    let finished = updates.last().unwrap();
    assert!(finished.exclusive);
    assert!(finished.status.is_none());
    assert_eq!(finished.entries.len(), 1);
    assert_eq!(finished.entries[0].title, "direct");
    assert!(!host.sessions.contains_key("ordinary"));
    let request: Value = serde_json::from_str(fs::read_to_string(&log).unwrap().trim()).unwrap();
    assert_eq!(request["params"]["query"], "");
    assert_eq!(request["params"]["is_global"], false);

    // File and web scopes use an empty query to suppress unrelated plugins.
    host.query_scoped(1, "", None, false);
    let suppressed = receiver.recv().unwrap();
    assert!(suppressed.entries.is_empty() && !suppressed.pending);
    assert_eq!(fs::read_to_string(&log).unwrap().lines().count(), 1);

    host.query(1, "normal search");
    let updates: Vec<_> = receiver.try_iter().collect();
    let finished = updates.last().unwrap();
    assert_eq!(finished.entries[0].title, "ordinary");
    assert!(!finished.exclusive);
    assert_eq!(fs::read_to_string(&log).unwrap().lines().count(), 1);

    // The dedicated plugin scope still accepts empty input without the flag.
    host.query_scoped(1, "", Some("direct"), false);
    let updates: Vec<_> = receiver.try_iter().collect();
    assert_eq!(updates.last().unwrap().entries[0].title, "direct");
    drop(host);
    fs::remove_dir_all(root).unwrap();
}

#[test]
fn resident_starts_with_host_survives_idle_and_process_eviction() {
    let root = Path::new(env!("CARGO_MANIFEST_DIR"))
        .join("artifacts")
        .join(format!("resident-plugin-{}", std::process::id()));
    let log = root.join("requests.jsonl");
    let mut resident = rpc_fixture(
        &root,
        "resident",
        &format!("echo %request%>>\"{}\"", log.display()),
    );
    resident.manifest.resident = true;
    resident.manifest.idle_timeout_secs = 1;
    let first = rpc_fixture(&root, "first", "");
    let second = rpc_fixture(&root, "second", "");
    let settings = PluginSettings {
        enabled: false,
        max_processes: 1,
        ..Default::default()
    };
    let (updates, results) = mpsc::channel();
    let mut host = Host::new(
        settings.clone(),
        Arc::new(move |update| {
            updates.send(update).unwrap();
        }),
        Arc::new(|_, _| {}),
        Arc::new(AtomicU64::new(1)),
    );
    host.plugins = vec![resident, first, second];
    let (sender, receiver) = mpsc::channel();
    let worker = std::thread::spawn(move || host.run(receiver));
    let send = |scope: Option<&str>, text: &str| {
        sender
            .send(Request::Query {
                attachments: Vec::new(),
                generation: 1,
                text: text.into(),
                settings: settings.clone(),
                scope: scope.map(str::to_string),
                allow_empty: true,
            })
            .unwrap();
        loop {
            let result = results.recv_timeout(Duration::from_secs(5)).unwrap();
            if !result.pending {
                assert!(
                    result.status.as_ref().is_none_or(|s| !s.contains("失败")),
                    "{:?}",
                    result.status
                );
                break;
            }
        }
    };
    // Startup initializes the resident before any keyword has been entered.
    send(None, ">plugins");
    send(Some("resident"), "one");
    std::thread::sleep(Duration::from_millis(1100));
    send(Some("first"), "ordinary");
    send(Some("second"), "ordinary");
    send(Some("resident"), "two");
    drop(sender);
    worker.join().unwrap();
    let requests: Vec<Value> = fs::read_to_string(&log)
        .unwrap()
        .lines()
        .map(|line| serde_json::from_str(line).unwrap())
        .collect();
    assert_eq!(requests.len(), 2);
    assert_eq!(requests[0]["id"], 2);
    assert_eq!(requests[1]["id"], 3); // The same resident session survived.
    fs::remove_dir_all(root).unwrap();
}

#[test]
fn fast_global_plugin_returns_before_a_slow_plugin_finishes() {
    let root = Path::new(env!("CARGO_MANIFEST_DIR"))
        .join("artifacts")
        .join(format!("parallel plugins-{}", std::process::id()));
    let gate = root.join("fast-returned");
    let slow = rpc_fixture(
        &root,
        "slow",
        &format!(":wait\r\nif not exist \"{}\" goto wait", gate.display()),
    );
    let fast = rpc_fixture(&root, "fast", "");
    let (sender, receiver) = mpsc::channel();
    let notify_gate = gate.clone();
    let mut host = Host::new(
        PluginSettings {
            enabled: false,
            max_processes: 2,
            ..Default::default()
        },
        Arc::new(move |update| {
            if update.entries.iter().any(|entry| entry.title == "fast") {
                fs::write(&notify_gate, "ready").unwrap();
            }
            sender.send(update).unwrap();
        }),
        Arc::new(|_, _| {}),
        Arc::new(AtomicU64::new(1)),
    );
    host.plugins = vec![slow, fast];
    host.query(1, "ordinary");
    let updates: Vec<_> = receiver.try_iter().collect();
    assert_eq!(updates.len(), 3);
    assert_eq!(updates[1].entries.len(), 1);
    assert_eq!(updates[1].entries[0].title, "fast");
    assert!(updates[1].pending);
    let final_update = updates.last().unwrap();
    assert!(final_update.status.is_none());
    assert!(!final_update.pending);
    assert_eq!(
        final_update
            .entries
            .iter()
            .map(|entry| entry.title.as_str())
            .collect::<Vec<_>>(),
        ["slow", "fast"]
    );
    assert_eq!(host.sessions.len(), 2);
    host.latest.store(2, Ordering::Relaxed);
    host.query(2, "second query");
    let reused: Vec<_> = receiver.try_iter().collect();
    let final_update = reused.last().unwrap();
    assert!(!final_update.pending);
    assert!(final_update.status.is_none());
    assert_eq!(final_update.entries.len(), 2);
    assert!(host.sessions.values().all(|session| session.next_id == 3));
    drop(host);
    for name in ["slow.cmd", "fast.cmd", "fast-returned"] {
        fs::remove_file(root.join(name)).unwrap();
    }
    fs::remove_dir(root).unwrap();
}

#[test]
fn scoped_query_keeps_literal_input_and_only_calls_selected_plugin() {
    let root = Path::new(env!("CARGO_MANIFEST_DIR"))
        .join("artifacts")
        .join(format!("plugin-scope-{}", std::process::id()));
    let log = root.join("queries.jsonl");
    let mut selected = rpc_fixture(
        &root,
        "selected",
        &format!("echo %request%>>\"{}\"", log.display()),
    );
    // Even a wildcard-only provider with a nonmatching trigger is selectable.
    selected.manifest.global_trigger = GlobalTrigger::Path;
    let (sender, receiver) = mpsc::channel();
    let mut host = Host::new(
        PluginSettings {
            enabled: false,
            ..Default::default()
        },
        Arc::new(move |update| {
            sender.send(update).unwrap();
        }),
        Arc::new(|_, _| {}),
        Arc::new(AtomicU64::new(1)),
    );
    host.plugins = vec![plugin("other", &["other"]), selected];
    for input in ["  other literal text  ", ">plugins", "设置", ""] {
        host.query_scoped(1, input, Some("selected"), true);
        let updates: Vec<_> = receiver.try_iter().collect();
        let finished = updates.last().unwrap();
        assert!(!finished.pending);
        assert!(finished.exclusive);
        assert!(finished.status.is_none(), "{:?}", finished.status);
        assert_eq!(finished.entries[0].title, "selected");
    }
    assert_eq!(host.sessions.len(), 1);
    drop(host);
    let queries: Vec<Value> = fs::read_to_string(&log)
        .unwrap()
        .lines()
        .map(|line| serde_json::from_str(line).unwrap())
        .collect();
    assert_eq!(queries.len(), 4);
    for (request, input) in queries
        .iter()
        .zip(["  other literal text  ", ">plugins", "设置", ""])
    {
        assert_eq!(request["params"]["query"], input);
        assert_eq!(request["params"]["is_global"], false);
    }
    for name in ["selected.cmd", "queries.jsonl"] {
        fs::remove_file(root.join(name)).unwrap();
    }
    fs::remove_dir(root).unwrap();
}

#[test]
fn global_input_burst_only_sends_the_latest_query_after_debounce() {
    let root = Path::new(env!("CARGO_MANIFEST_DIR"))
        .join("artifacts")
        .join(format!("plugin-debounce-{}", std::process::id()));
    let log = root.join("queries.jsonl");
    let fixture = rpc_fixture(
        &root,
        "debounced",
        &format!("echo %request%>>\"{}\"", log.display()),
    );
    let settings = PluginSettings {
        enabled: false,
        ..Default::default()
    };
    let (updates, results) = mpsc::channel();
    let latest = Arc::new(AtomicU64::new(1));
    let mut host = Host::new(
        settings.clone(),
        Arc::new(move |update| {
            updates.send(update).unwrap();
        }),
        Arc::new(|_, _| {}),
        latest.clone(),
    );
    host.plugins = vec![fixture];
    let (sender, receiver) = mpsc::channel();
    sender
        .send(Request::Query {
            attachments: Vec::new(),
            generation: 1,
            text: "first".into(),
            scope: None,
            settings: settings.clone(),
            allow_empty: true,
        })
        .unwrap();
    let worker = std::thread::spawn(move || host.run(receiver));
    std::thread::sleep(Duration::from_millis(15));
    latest.store(2, Ordering::Relaxed);
    sender
        .send(Request::Query {
            attachments: Vec::new(),
            generation: 2,
            text: "latest input".into(),
            scope: None,
            settings,
            allow_empty: true,
        })
        .unwrap();
    let sent = Instant::now();
    let first = results.recv_timeout(Duration::from_secs(5)).unwrap();
    assert_eq!(first.generation, 2);
    assert!(first.pending);
    assert!(sent.elapsed() >= Duration::from_millis(50));
    let finished = results.recv_timeout(Duration::from_secs(5)).unwrap();
    assert!(!finished.pending);
    assert!(finished.status.is_none());
    drop(sender);
    worker.join().unwrap();
    let requests = fs::read_to_string(&log).unwrap();
    let requests: Vec<Value> = requests
        .lines()
        .map(|line| serde_json::from_str(line).unwrap())
        .collect();
    assert_eq!(requests.len(), 1);
    assert_eq!(requests[0]["params"]["query"], "latest input");
    assert_eq!(requests[0]["params"]["is_global"], true);
    for name in ["debounced.cmd", "queries.jsonl"] {
        fs::remove_file(root.join(name)).unwrap();
    }
    fs::remove_dir(root).unwrap();
}

#[test]
fn stale_query_cancels_a_plugin_still_initializing() {
    let root = Path::new(env!("CARGO_MANIFEST_DIR"))
        .join("artifacts")
        .join(format!("plugin-init-cancel-{}", std::process::id()));
    let mut blocked = rpc_fixture(&root, "blocked", "");
    blocked.manifest.timeout_ms = 3000;
    // Wait for a second input instead of answering initialize. Cancellation
    // releases this read, and the host must dispose of the unfinished session.
    fs::write(
        root.join("blocked.cmd"),
        "@echo off\r\nset /p initialize=\r\nset /p cancel=\r\n",
    )
    .unwrap();
    let fast = rpc_fixture(&root, "fast", "");
    let latest = Arc::new(AtomicU64::new(1));
    let cancel = latest.clone();
    let (sender, receiver) = mpsc::channel();
    let mut host = Host::new(
        PluginSettings {
            enabled: false,
            ..Default::default()
        },
        Arc::new(move |update| {
            if update.entries.iter().any(|entry| entry.title == "fast") {
                cancel.store(2, Ordering::Relaxed);
            }
            sender.send(update).unwrap();
        }),
        Arc::new(|_, _| {}),
        latest,
    );
    host.plugins = vec![blocked, fast];
    let started = Instant::now();
    host.query(1, "ordinary");
    assert!(started.elapsed() < Duration::from_secs(2));
    let updates: Vec<_> = receiver.try_iter().collect();
    assert_eq!(updates.len(), 2);
    assert!(updates[1].pending);
    assert_eq!(updates[1].entries[0].title, "fast");
    assert!(!host.sessions.contains_key("blocked"));
    drop(host);
    for name in ["blocked.cmd", "fast.cmd"] {
        fs::remove_file(root.join(name)).unwrap();
    }
    fs::remove_dir(root).unwrap();
}

#[test]
fn wildcard_discovery_allows_shared_star_but_rejects_duplicate_keywords() {
    let root = Path::new(env!("CARGO_MANIFEST_DIR"))
        .join("artifacts")
        .join(format!("wildcard-discovery-{}", std::process::id()));
    for (id, keywords) in [
        ("a", vec!["*", "first"]),
        ("b", vec!["*", "second"]),
        ("c", vec!["first"]),
    ] {
        let directory = root.join(id);
        fs::create_dir_all(&directory).unwrap();
        fs::write(
            directory.join("plugin.json"),
            json!({"id":id,"name":id,
                "protocol_version":2,"executable":"missing.exe","keywords":keywords})
            .to_string(),
        )
        .unwrap();
    }
    let (plugins, errors) = discover(&PluginSettings {
        directories: vec![root.clone()],
        ..Default::default()
    });
    assert_eq!(plugins.len(), 2);
    assert_eq!(errors.len(), 1);
    assert_eq!(route_plugins(&plugins, "ordinary query").0.len(), 2);
    for id in ["a", "b", "c"] {
        fs::remove_file(root.join(id).join("plugin.json")).unwrap();
        fs::remove_dir(root.join(id)).unwrap();
    }
    fs::remove_dir(root).unwrap();
}

#[test]
fn wildcard_host_merges_results_and_survives_one_plugin_failure() {
    let root = Path::new(env!("CARGO_MANIFEST_DIR"))
        .join("artifacts")
        .join(format!("wildcard-rpc-{}", std::process::id()));
    fs::create_dir_all(&root).unwrap();
    let fixture = root.join("fixture.cmd");
    fs::write(&fixture, concat!(
            "@echo off\r\nset /p request=\r\n",
            "echo {\"jsonrpc\":\"2.0\",\"id\":1,\"result\":{\"protocol_version\":2}}\r\n",
            "set /p request=\r\n",
            "echo {\"jsonrpc\":\"2.0\",\"id\":2,\"result\":{\"items\":[{\"title\":\"result\",\"action\":{\"type\":\"execute\",\"id\":\"open\"}}]}}\r\n"
        )).unwrap();
    let (sender, receiver) = mpsc::channel();
    let latest = Arc::new(AtomicU64::new(1));
    let mut host = Host::new(
        PluginSettings {
            enabled: false,
            max_processes: 1,
            ..Default::default()
        },
        Arc::new(move |update| {
            sender.send(update).unwrap();
        }),
        Arc::new(|_, _| {}),
        latest.clone(),
    );
    let mut first = plugin("first", &["*"]);
    first.directory = root.clone();
    first.manifest.executable = PathBuf::from(std::env::var_os("COMSPEC").unwrap());
    first.manifest.args = vec![
        "/d".into(),
        "/q".into(),
        "/c".into(),
        fixture.to_str().unwrap().into(),
    ];
    let mut second = first.clone();
    second.manifest.id = "second".into();
    second.manifest.priority = 100;
    let mut broken = plugin("broken", &["*"]);
    broken.directory = root.clone();
    host.plugins = vec![first, broken, second];
    host.query(1, "D:/Some Folder");
    let updates: Vec<_> = receiver.try_iter().collect();
    assert_eq!(updates.len(), 4);
    assert!(updates.iter().all(|update| !update.exclusive));
    assert!(updates[..3].iter().all(|update| update.pending));
    let final_update = updates.last().unwrap();
    assert!(!final_update.pending);
    assert_eq!(final_update.entries.len(), 2);
    assert!(final_update.status.as_ref().unwrap().contains("broken"));
    assert!(
        matches!(&final_update.entries[0].action, Action::Plugin {plugin_id, ..} if plugin_id == "second")
    );
    latest.store(2, Ordering::Relaxed);
    host.query(1, "stale");
    assert!(receiver.try_recv().is_err());
    drop(host);
    fs::remove_file(fixture).unwrap();
    fs::remove_dir(root).unwrap();
}
#[test]
fn discovery_applies_generic_keywords_and_enabled_state() {
    let directory = Path::new(env!("CARGO_MANIFEST_DIR"))
        .join("artifacts")
        .join(format!("plugin-discovery-{}", std::process::id()));
    fs::create_dir_all(&directory).unwrap();
    fs::write(directory.join("plugin.json"), r#"{"id":"demo-plugin","name":"Demo","protocol_version":2,"executable":"demo.exe","keywords":["demo"]}"#).unwrap();
    let mut settings = PluginSettings {
        directories: vec![directory.clone()],
        ..Default::default()
    };
    settings.entries.insert(
        "demo_plugin".into(),
        crate::plugin_config::Entry {
            keywords: Some(vec!["work".into()]),
            direct_display: Some(true),
            priority: Some(100),
            ..Default::default()
        },
    );
    let (plugins, errors) = discover(&settings);
    assert!(errors.is_empty());
    assert_eq!(plugins.len(), 1);
    assert_eq!(plugins[0].manifest.keywords, ["work"]);
    assert_eq!(plugins[0].settings_path, Some(config::settings_path()));
    assert!(plugins[0].manifest.direct_display);
    assert_eq!(plugins[0].manifest.priority, 100);
    assert_eq!(crate::plugin_catalog::inspect(&settings)[0].priority, 100);
    assert!(search_catalog(&settings).direct_display);
    settings
        .entries
        .get_mut("demo_plugin")
        .unwrap()
        .direct_display = Some(false);
    assert!(!search_catalog(&settings).direct_display);
    settings.entries.get_mut("demo_plugin").unwrap().enabled = false;
    assert!(discover(&settings).0.is_empty());
    assert!(!search_catalog(&settings).direct_display);
    fs::remove_file(directory.join("plugin.json")).unwrap();
    fs::remove_dir(directory).unwrap();
}
#[test]
fn launch_actions_preserve_arguments_and_reject_invalid_commands() {
    let parse = |value| {
        serde_json::from_value::<PluginAction>(value)
            .unwrap()
            .into_action("example-plugin")
    };
    let args = vec!["--open", "D:\\工作区\\中文 空格.lnk", "quote\"value", ""];
    let action =
        parse(json!({"type":"launch","executable":"D:\\Tools\\ExampleEditor.exe","args":args}))
            .unwrap();
    assert!(
        matches!(action, Action::Launch { executable, args: actual } if executable == "D:\\Tools\\ExampleEditor.exe" && actual == args)
    );
    for executable in ["", "relative.exe", "D:\\bad\0.exe"] {
        assert!(parse(json!({"type":"launch","executable":executable,"args":[]})).is_err());
    }
    for args in [
        vec!["\0".to_string()],
        vec![String::new(); 65],
        vec!["中".repeat(32768)],
    ] {
        assert!(parse(json!({"type":"launch","executable":"D:\\app.exe","args":args})).is_err());
    }
}
#[test]
fn manifests_reject_incompatible_protocol_and_reserved_keywords() {
    let manifest = json!({"id":"echo","name":"Echo","protocol_version":2,"executable":"echo.exe","keywords":["echo"]});
    assert!(parse_manifest(&manifest.to_string(), Path::new("plugin.json")).is_ok());
    let mut bad = manifest.clone();
    bad["keywords"] = json!(["e"]);
    assert!(parse_manifest(&bad.to_string(), Path::new("plugin.json")).is_err());
    bad = manifest;
    bad["protocol_version"] = json!(1);
    assert!(parse_manifest(&bad.to_string(), Path::new("plugin.json")).is_err());
}
#[test]
fn manifests_accept_plugin_global_shortcut_declarations() {
    let manifest = json!({"id":"translation","name":"Translation","protocol_version":2,"executable":"translation.exe","keywords":["tr"],"hotkeys":[{"id":"selection","default":"Ctrl+T","prefill":"selection"}]});
    let plugin = parse_manifest(&manifest.to_string(), Path::new("plugin.json")).unwrap();
    assert_eq!(plugin.manifest.hotkeys.len(), 1);
    assert_eq!(plugin.manifest.hotkeys[0].default, "Ctrl+T");
}

#[test]
fn text_panels_validate_content_limits() {
    let plugin = parse_manifest(&json!({"id":"tr","name":"Translate","protocol_version":2,"executable":"tr.exe","keywords":["tr"]}).to_string(), Path::new("plugin.json")).unwrap();
    let mut item = json!({"title":"Translate","action":{"type":"copy","text":"你好"},"text_panel":{"blocks":[{"id":"source","title":"原文","text":"hello"},{"id":"translation","title":"译文","text":"你好"}]}});
    let entries = parse_items(json!({"items":[item.clone()]}), &plugin).unwrap();
    assert_eq!(entries[0].text_panel.as_ref().unwrap().blocks.len(), 2);
    item["text_panel"]["blocks"][0]["text"] = json!("x".repeat(65537));
    assert!(parse_items(json!({"items":[item]}), &plugin).is_err());
}

#[test]
fn result_ids_are_stable_across_content_changes_and_duplicate_ids_are_rejected() {
    let plugin = plugin("generic", &["gtext"]);
    let a = json!({"id":"result","title":"第一版","action":{"type":"copy","text":"one"}});
    let b = json!({"id":"result","title":"第二版","action":{"type":"copy","text":"two"}});
    let first = parse_items(json!({"items":[a.clone()]}), &plugin).unwrap();
    let second = parse_items(json!({"items":[b.clone()]}), &plugin).unwrap();
    assert_eq!(first[0].key, second[0].key);
    assert!(parse_items(json!({"items":[a,b]}), &plugin).is_err());
}

#[test]
fn context_actions_are_optional_bounded_and_keep_plugin_identity() {
    let plugin = parse_manifest(&json!({"id":"echo","name":"Echo","protocol_version":2,"executable":"echo.exe","keywords":["echo"]}).to_string(), Path::new("plugin.json")).unwrap();
    let item = json!({"title":"result","action":{"type":"copy","text":"default"},"context_actions":[
        {"title":"复制中文","action":{"type":"copy","text":"中文"}},
        {"title":"执行回调","action":{"type":"execute","id":"details"}},
        {"title":"打开","action":{"type":"open","target":"https://example.com"}}
    ]});
    let entries = parse_items(json!({"items":[item.clone()]}), &plugin).unwrap();
    assert!(!entries[0].is_app());
    assert_eq!(entries[0].context_actions.len(), 3);
    assert!(
        matches!(&entries[0].context_actions[1].action, Action::Plugin { plugin_id, action_id } if plugin_id == "echo" && action_id == "details")
    );
    for title in ["", "bad\0title", "bad\ntitle"] {
        let mut bad = item.clone();
        bad["context_actions"][0]["title"] = json!(title);
        assert!(parse_items(json!({"items":[bad]}), &plugin).is_err());
    }
    let mut bad = item.clone();
    bad["context_actions"] = json!(vec![item["context_actions"][0].clone(); 17]);
    assert!(parse_items(json!({"items":[bad]}), &plugin).is_err());
    let mut legacy = item;
    legacy.as_object_mut().unwrap().remove("context_actions");
    assert!(parse_items(json!({"items":[legacy]}), &plugin).unwrap()[0]
        .context_actions
        .is_empty());
}
#[test]
fn plugin_results_support_declarative_and_callback_actions() {
    let plugin=parse_manifest(&json!({"id":"echo","name":"Echo","protocol_version":2,"executable":"echo.exe","keywords":["echo"]}).to_string(),Path::new("plugin.json")).unwrap();
    let entries=parse_items(json!({"items":[{"title":"copy","action":{"type":"copy","text":"中文"}},{"title":"callback","action":{"type":"execute","id":"about"}}]}),&plugin).unwrap();
    assert!(matches!(&entries[0].action,Action::Copy(value) if value=="中文"));
    assert!(
        matches!(&entries[1].action,Action::Plugin{plugin_id,action_id} if plugin_id=="echo" && action_id=="about")
    );
    assert!(parse_items(
        json!({"items":[{"title":"bad","action":{"type":"open","target":""}}]}),
        &plugin
    )
    .is_err());
}

#[test]
fn cancelled_resident_session_clears_abandoned_deadline_after_late_reply() {
    let root = Path::new(env!("CARGO_MANIFEST_DIR"))
        .join("artifacts")
        .join(format!("resident-cancel-{}", std::process::id()));
    let log = root.join("requests.jsonl");
    let started = root.join("query-started");
    let gate = root.join("release-query");
    let mut resident = rpc_fixture(
        &root,
        "resident",
        &format!(
            concat!(
                "if not \"%request:cancel=%\"==\"%request%\" (\r\n",
                "  set /a rpcid-=1 >nul\r\n",
                "  goto queries\r\n",
                ")\r\n",
                "echo %request%>>\"{}\"\r\n",
                "echo started>\"{}\"\r\n",
                ":wait\r\n",
                "if not exist \"{}\" goto wait"
            ),
            log.display(),
            started.display(),
            gate.display()
        ),
    );
    resident.manifest.resident = true;
    resident.manifest.timeout_ms = 250;
    let settings = PluginSettings {
        enabled: false,
        ..Default::default()
    };
    let latest = Arc::new(AtomicU64::new(1));
    let (updates, results) = mpsc::channel();
    let mut host = Host::new(
        settings.clone(),
        Arc::new(move |update| {
            let _ = updates.send(update);
        }),
        Arc::new(|_, _| {}),
        latest.clone(),
    );
    host.plugins = vec![resident];
    let (sender, receiver) = mpsc::channel();
    let worker = std::thread::spawn(move || host.run(receiver));
    sender
        .send(Request::Query {
            attachments: Vec::new(),
            generation: 1,
            text: "first".into(),
            settings: settings.clone(),
            scope: Some("resident".into()),
            allow_empty: true,
        })
        .unwrap();
    let pending = results.recv_timeout(Duration::from_secs(5)).unwrap();
    assert!(pending.pending);
    while !started.exists() {
        std::thread::sleep(Duration::from_millis(5));
    }
    // Cancel the in-flight query, then let the plugin finish replying before deadline.
    latest.store(2, Ordering::Relaxed);
    std::thread::sleep(Duration::from_millis(40));
    fs::write(&gate, "go").unwrap();
    // Wait past timeout_ms (250ms); poll_abandoned must drain the reply and keep the session alive.
    std::thread::sleep(Duration::from_millis(320));
    sender
        .send(Request::Query {
            attachments: Vec::new(),
            generation: 2,
            text: "second".into(),
            settings,
            scope: Some("resident".into()),
            allow_empty: true,
        })
        .unwrap();
    loop {
        let update = results.recv_timeout(Duration::from_secs(5)).unwrap();
        if update.generation == 2 && !update.pending {
            assert!(update.status.is_none(), "{:?}", update.status);
            assert_eq!(update.entries.len(), 1);
            assert_eq!(update.entries[0].title, "resident");
            break;
        }
    }
    drop(sender);
    worker.join().unwrap();
    let requests: Vec<Value> = fs::read_to_string(&log)
        .unwrap()
        .lines()
        .map(|line| serde_json::from_str(line).unwrap())
        .collect();
    assert_eq!(requests.len(), 2);
    assert_eq!(requests[0]["id"], 2);
    assert_eq!(requests[1]["id"], 3);
    fs::remove_dir_all(root).unwrap();
}

#[test]
fn image_input_requires_manifest_capability_before_starting_process() {
    let plugin = plugin("text-only", &["text"]);
    let image = crate::clipboard_image::Image {
        width: 1,
        height: 1,
        pixels: vec![255; 4],
    };
    let attachment = crate::input_attachment::InputImage::new(&image).unwrap();
    let latest = AtomicU64::new(1);
    let (session, result) = query_plugin(&plugin, None, "", &[attachment], 1, false, &latest);
    assert!(session.is_none());
    assert!(matches!(result, Err(RpcError::Failed(error)) if error.contains("未声明支持图片")));
    let capable = parse_manifest(
        &json!({"id":"images","name":"images","protocol_version":2,
        "executable":"missing.exe","keywords":["img"],"accepts_images":true})
        .to_string(),
        Path::new("plugin.json"),
    )
    .unwrap();
    assert!(capable.manifest.accepts_images);
}
