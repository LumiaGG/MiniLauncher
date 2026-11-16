use super::*;
use crate::model::{parse_query, QueryIntent};
use crate::processes::is_protected_process;
use std::io::Cursor;

#[test]
fn query_intent_parsing_handles_prefixes_and_commands() {
    assert_eq!(parse_query(":8080"), QueryIntent::Port(8080));
    assert_eq!(parse_query(": 3000"), QueryIntent::Port(3000));
    assert_eq!(parse_query(":"), QueryIntent::AllPorts);

    assert_eq!(parse_query("port 8080"), QueryIntent::Port(8080));
    assert_eq!(parse_query("port:80"), QueryIntent::Port(80));
    assert_eq!(parse_query("PORT 443"), QueryIntent::Port(443));
    assert_eq!(parse_query("port"), QueryIntent::AllPorts);

    assert_eq!(parse_query("kill node"), QueryIntent::Kill("node".into()));
    assert_eq!(parse_query("kill 14208"), QueryIntent::Kill("14208".into()));
    assert_eq!(parse_query("kill"), QueryIntent::TopProcesses);

    assert_eq!(parse_query("ps chrome"), QueryIntent::Process("chrome".into()));
    assert_eq!(parse_query("ps"), QueryIntent::TopProcesses);

    assert_eq!(parse_query("8080"), QueryIntent::General("8080".into()));
    assert_eq!(parse_query("zed"), QueryIntent::General("zed".into()));
    assert_eq!(parse_query(""), QueryIntent::TopProcesses);
}

#[test]
fn numeric_queries_find_pids_before_ports_and_respect_limits() {
    use crate::ports::{PortBinding, Protocol, TcpState};
    use crate::processes::ProcessInfo;

    let processes: Vec<_> = [(8080, 1), (100_000, 2), (43, 100)]
        .into_iter()
        .map(|(pid, memory_bytes)| ProcessInfo {
            pid,
            parent_pid: 0,
            name: "8080-worker.exe".into(),
            executable_path: None,
            memory_bytes,
            is_protected: false,
            protection_reason: None,
        })
        .collect();
    let bindings = vec![PortBinding {
        protocol: Protocol::Tcp,
        local_ip: "127.0.0.1".parse().unwrap(),
        local_port: 8080,
        remote_ip: None,
        remote_port: None,
        state: Some(TcpState::Listen),
        pid: 43,
    }];
    let by_pid = ports::get_bindings_by_pid(&bindings);
    // The host strips the trigger keyword from inputs such as "ps 8080".
    let render = |query, processes: &[ProcessInfo], limit| {
        view::render_results(&parse_query(query), processes, &bindings, &by_pid, limit)
    };

    let items = render(" 8080 ", &processes, 10);
    assert_eq!(items.len(), 2);
    assert_eq!(items[0]["id"], "proc:8080");
    assert_eq!(items[1]["id"], "port:TCP:8080:43");
    assert_eq!(render("8080", &processes, 1), items[..1]);
    assert!(render("8080", &processes, 0).is_empty());
    assert_eq!(render("100000", &processes, 1)[0]["id"], "proc:100000");
    assert_eq!(render("43", &processes, 1)[0]["id"], "proc:43");
    assert!(render("99999", &processes, 10).is_empty());

    // Ports remain available without a matching PID or with an explicit command.
    assert_eq!(render("8080", &processes[1..], 1)[0], items[1]);
    for query in [":8080", "port 8080", "port:8080"] {
        assert_eq!(render(query, &processes, 1)[0], items[1]);
    }
}

#[test]
fn safety_protection_guards_critical_system_services() {
    assert!(is_protected_process(0, "System Idle Process").is_some());
    assert!(is_protected_process(4, "System").is_some());
    assert!(is_protected_process(1234, "csrss.exe").is_some());
    assert!(is_protected_process(1235, "CSRSS.EXE").is_some());
    assert!(is_protected_process(1236, "smss.exe").is_some());
    assert!(is_protected_process(1237, "services.exe").is_some());
    assert!(is_protected_process(1238, "lsass.exe").is_some());
    assert!(is_protected_process(1239, "dwm.exe").is_some());
    assert!(is_protected_process(1240, "mini-launcher.exe").is_some());

    // Normal processes must not be protected
    assert!(is_protected_process(10000, "node.exe").is_none());
    assert!(is_protected_process(10001, "python.exe").is_none());
    assert!(is_protected_process(10002, "cargo.exe").is_none());
}

#[test]
fn protected_process_cannot_be_terminated() {
    assert!(processes::terminate_process(0).is_err());
    assert!(processes::terminate_process(4).is_err());
}

#[test]
fn rpc_dispatch_full_lifecycle_and_text_panel_validation() {
    let mut input = Cursor::new(concat!(
        "{\"jsonrpc\":\"2.0\",\"id\":1,\"method\":\"initialize\",\"params\":{\"protocol_version\":2}}\n",
        "{\"jsonrpc\":\"2.0\",\"id\":2,\"method\":\"query\",\"params\":{\"query\":\":8080\",\"max_results\":5}}\n",
        "{\"jsonrpc\":\"2.0\",\"id\":3,\"method\":\"query\",\"params\":{\"query\":\"ps\",\"max_results\":5}}\n",
        "{\"jsonrpc\":\"2.0\",\"method\":\"shutdown\"}\n"
    ));
    let mut output = Vec::new();

    mini_launcher::plugin_rpc::serve(&mut input, &mut output, dispatch).unwrap();

    let mut out_cursor = Cursor::new(output);

    // 1. initialize response
    let init_res = mini_launcher::plugin_rpc::read_frame(&mut out_cursor)
        .unwrap()
        .unwrap();
    assert_eq!(init_res["id"], 1);
    assert_eq!(init_res["result"]["protocol_version"], 2);

    // 2. query :8080 response
    let query_res = mini_launcher::plugin_rpc::read_frame(&mut out_cursor)
        .unwrap()
        .unwrap();
    assert_eq!(query_res["id"], 2);
    let items = query_res["result"]["items"].as_array().unwrap();
    for item in items {
        if let Some(panel_val) = item.get("text_panel") {
            let panel: mini_launcher::result::TextPanel =
                serde_json::from_value(panel_val.clone()).unwrap();
            panel.validate().unwrap();
        }
    }

    // 3. query ps response
    let ps_res = mini_launcher::plugin_rpc::read_frame(&mut out_cursor)
        .unwrap()
        .unwrap();
    assert_eq!(ps_res["id"], 3);
    let ps_items = ps_res["result"]["items"].as_array().unwrap();
    assert!(!ps_items.is_empty(), "ps 应该至少能枚举出活跃进程");
    for item in ps_items {
        if let Some(panel_val) = item.get("text_panel") {
            let panel: mini_launcher::result::TextPanel =
                serde_json::from_value(panel_val.clone()).unwrap();
            panel.validate().unwrap();
        }
    }
}

#[test]
fn rendered_queries_preserve_order_limits_and_process_actions() {
    use crate::ports::{PortBinding, Protocol, TcpState};
    use crate::processes::ProcessInfo;
    let processes: Vec<_> = [(42, 100, false), (4, 200, true), (43, 0, false)]
        .into_iter()
        .map(|(pid, memory_bytes, is_protected)| ProcessInfo {
            pid,
            parent_pid: 0,
            name: "sample.exe".into(),
            executable_path: Some(r"C:\apps\sample.exe".into()),
            memory_bytes,
            is_protected,
            protection_reason: None,
        })
        .collect();
    let binding = PortBinding {
        protocol: Protocol::Tcp,
        local_ip: "127.0.0.1".parse().unwrap(),
        local_port: 8080,
        remote_ip: None,
        remote_port: None,
        state: Some(TcpState::Listen),
        pid: 42,
    };
    let bindings = vec![binding];
    let by_pid = ports::get_bindings_by_pid(&bindings);
    let render =
        |intent, limit| view::render_results(&intent, &processes, &bindings, &by_pid, limit);
    let items = render(QueryIntent::Process("sample".into()), 2);
    assert_eq!(items.len(), 2);
    assert_eq!(items[0]["id"], "proc:4");
    assert_eq!(items[0]["action"], json!({"type":"copy","text":"4"}));
    assert_eq!(items[0]["context_actions"][0]["title"], "复制 PID");
    assert_eq!(items[1]["id"], "proc:42");
    let top = render(QueryIntent::TopProcesses, 10);
    assert_eq!(top.len(), 1);
    assert_eq!(top[0], items[1]);
    assert!(render(QueryIntent::Kill("42".into()), 1)[0]["subtitle"]
        .as_str()
        .unwrap()
        .contains("⚡ 回车立即查杀"));
    let port = &render(QueryIntent::Port(8080), 1)[0];
    assert_eq!(port["action"], top[0]["action"]);
    assert_eq!(port["icon"], top[0]["icon"]);
    for (item, port_title) in [(port, "复制端口号"), (&top[0], "复制首个端口")] {
        let actions = item["context_actions"].as_array().unwrap();
        let titles: Vec<_> = actions
            .iter()
            .map(|a| a["title"].as_str().unwrap())
            .collect();
        assert_eq!(
            titles,
            [
                "强制终止进程 (Terminate)",
                "复制 PID",
                port_title,
                "复制程序路径",
                "打开文件所在位置"
            ]
        );
        assert_eq!(actions[2]["action"], json!({"type":"copy","text":"8080"}));
        let panel: mini_launcher::result::TextPanel =
            serde_json::from_value(item["text_panel"].clone()).unwrap();
        panel.validate().unwrap();
    }
}
