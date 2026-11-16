use crate::model::QueryIntent;
use crate::ports::{PortBinding, Protocol, TcpState};
use crate::processes::{format_memory, ProcessInfo};
use serde_json::{json, Value};
use std::collections::HashMap;
use std::path::Path;

pub fn render_results(
    intent: &QueryIntent,
    processes: &[ProcessInfo],
    bindings: &[PortBinding],
    pid_to_bindings: &HashMap<u32, Vec<PortBinding>>,
    max_results: usize,
) -> Vec<Value> {
    let proc_map: HashMap<u32, &ProcessInfo> = processes.iter().map(|p| (p.pid, p)).collect();

    match intent {
        QueryIntent::Port(target_port) => {
            render_port_query(*target_port, bindings, &proc_map, max_results)
        }
        QueryIntent::AllPorts => render_all_ports(bindings, &proc_map, max_results),
        QueryIntent::Kill(filter) => {
            render_process_list(filter, processes, pid_to_bindings, max_results, true)
        }
        QueryIntent::Process(filter) => {
            render_process_list(filter, processes, pid_to_bindings, max_results, false)
        }
        QueryIntent::TopProcesses => {
            render_top_processes(processes, pid_to_bindings, max_results)
        }
        QueryIntent::General(filter) => {
            // Keep exact PID matches first, even when the same number is a port.
            let mut items = Vec::new();
            if let Ok(pid) = filter.parse::<u32>() {
                if let Some(process) = proc_map.get(&pid) {
                    items =
                        render_processes(vec![*process], pid_to_bindings, max_results, false);
                }
            }
            if let Ok(port) = filter.parse::<u16>() {
                items.extend(render_port_query(
                    port,
                    bindings,
                    &proc_map,
                    max_results.saturating_sub(items.len()),
                ));
            }
            if items.is_empty() {
                items = render_process_list(filter, processes, pid_to_bindings, max_results, false);
            }
            items
        }
    }
}

fn render_port_query(
    port: u16,
    bindings: &[PortBinding],
    proc_map: &HashMap<u32, &ProcessInfo>,
    max_results: usize,
) -> Vec<Value> {
    let mut matching: Vec<&PortBinding> = bindings.iter().filter(|b| b.local_port == port).collect();
    // Prioritize Listening TCP
    matching.sort_by_key(|b| match b.state {
        Some(TcpState::Listen) => 0,
        Some(TcpState::Established) => 1,
        _ => 2,
    });

    matching
        .into_iter()
        .take(max_results)
        .map(|b| port_binding_to_item(b, proc_map.get(&b.pid).copied()))
        .collect()
}

fn render_all_ports(
    bindings: &[PortBinding],
    proc_map: &HashMap<u32, &ProcessInfo>,
    max_results: usize,
) -> Vec<Value> {
    // Focus primarily on Listening TCP and active UDP
    let mut relevant: Vec<&PortBinding> = bindings
        .iter()
        .filter(|b| match b.protocol {
            Protocol::Tcp => b.state == Some(TcpState::Listen),
            Protocol::Udp => true,
        })
        .collect();

    relevant.sort_by_key(|b| b.local_port);

    relevant
        .into_iter()
        .take(max_results)
        .map(|b| port_binding_to_item(b, proc_map.get(&b.pid).copied()))
        .collect()
}

fn render_process_list(
    filter: &str,
    processes: &[ProcessInfo],
    pid_to_bindings: &HashMap<u32, Vec<PortBinding>>,
    max_results: usize,
    kill_mode: bool,
) -> Vec<Value> {
    let filter_lower = filter.trim().to_ascii_lowercase();
    let filter_pid = filter.trim().parse::<u32>().ok();

    let filtered: Vec<&ProcessInfo> = processes
        .iter()
        .filter(|p| {
            if let Some(pid) = filter_pid {
                if p.pid == pid {
                    return true;
                }
            }
            p.name.to_ascii_lowercase().contains(&filter_lower)
        })
        .collect();

    render_processes(filtered, pid_to_bindings, max_results, kill_mode)
}

fn render_top_processes(
    processes: &[ProcessInfo],
    pid_to_bindings: &HashMap<u32, Vec<PortBinding>>,
    max_results: usize,
) -> Vec<Value> {
    let filtered = processes
        .iter()
        .filter(|p| p.pid != 0 && p.pid != 4 && p.memory_bytes > 0)
        .collect();
    render_processes(filtered, pid_to_bindings, max_results, false)
}

fn render_processes(
    mut processes: Vec<&ProcessInfo>,
    pid_to_bindings: &HashMap<u32, Vec<PortBinding>>,
    max_results: usize,
    kill_mode: bool,
) -> Vec<Value> {
    processes.sort_by(|a, b| b.memory_bytes.cmp(&a.memory_bytes));
    processes
        .into_iter()
        .take(max_results)
        .map(|p| {
            let bindings = pid_to_bindings
                .get(&p.pid)
                .map(Vec::as_slice)
                .unwrap_or(&[]);
            process_to_item(p, bindings, kill_mode)
        })
        .collect()
}

fn port_binding_to_item(b: &PortBinding, proc: Option<&ProcessInfo>) -> Value {
    let state_str = b.state.map(|s| s.as_str()).unwrap_or("ACTIVE");
    let proc_name = proc.map(|p| p.name.as_str()).unwrap_or("未知进程");
    let memory_str = proc
        .map(|p| format_memory(p.memory_bytes))
        .unwrap_or_else(|| "--".into());
    let is_protected = proc
        .map(|p| p.is_protected)
        .unwrap_or(b.pid == 0 || b.pid == 4);

    let title = format!(
        "{} :{} ({}) · {}",
        b.protocol.as_str(),
        b.local_port,
        state_str,
        proc_name
    );
    let subtitle = format!(
        "PID: {} · 内存: {} · 本地地址: {} · {}",
        b.pid,
        memory_str,
        b.local_ip,
        if is_protected {
            "受保护进程 · 回车复制 PID"
        } else {
            "回车直接终止进程"
        }
    );

    let primary_action = if is_protected {
        json!({"type": "copy", "text": b.pid.to_string()})
    } else {
        json!({"type": "execute", "id": format!("kill:{}", b.pid)})
    };

    let icon = proc.and_then(|p| p.executable_path.as_deref());
    let context_actions = process_context_actions(
        b.pid,
        is_protected,
        icon,
        Some(("复制端口号", b.local_port)),
    );
    let text_panel = build_text_panel(proc, std::slice::from_ref(b));

    let mut item = json!({
        "id": format!("port:{}:{}:{}", b.protocol.as_str(), b.local_port, b.pid),
        "title": title,
        "subtitle": subtitle,
        "action": primary_action,
        "context_actions": context_actions,
        "text_panel": text_panel
    });

    if let Some(icon_path) = icon {
        item["icon"] = json!(icon_path);
    }
    item
}

fn process_to_item(p: &ProcessInfo, bindings: &[PortBinding], kill_mode: bool) -> Value {
    let memory_str = format_memory(p.memory_bytes);
    let ports_summary = if bindings.is_empty() {
        "无网络监听".to_string()
    } else {
        let ports: Vec<String> = bindings
            .iter()
            .take(4)
            .map(|b| format!(":{}", b.local_port))
            .collect();
        format!("占用端口: {}", ports.join(", "))
    };

    let title = format!("{} · PID: {}", p.name, p.pid);
    let subtitle = format!(
        "内存: {} · {} · {}",
        memory_str,
        ports_summary,
        if p.is_protected {
            "受保护进程 · 回车复制 PID"
        } else if kill_mode {
            "⚡ 回车立即查杀"
        } else {
            "回车终止进程"
        }
    );

    let primary_action = if p.is_protected {
        json!({"type": "copy", "text": p.pid.to_string()})
    } else {
        json!({"type": "execute", "id": format!("kill:{}", p.pid)})
    };

    let context_actions = process_context_actions(
        p.pid,
        p.is_protected,
        p.executable_path.as_deref(),
        bindings.first().map(|b| ("复制首个端口", b.local_port)),
    );

    let text_panel = build_text_panel(Some(p), bindings);

    let mut item = json!({
        "id": format!("proc:{}", p.pid),
        "title": title,
        "subtitle": subtitle,
        "action": primary_action,
        "context_actions": context_actions,
        "text_panel": text_panel
    });

    if let Some(path) = &p.executable_path {
        item["icon"] = json!(path);
    }
    item
}

fn process_context_actions(
    pid: u32,
    is_protected: bool,
    path: Option<&str>,
    port: Option<(&str, u16)>,
) -> Vec<Value> {
    let mut actions = Vec::new();
    if !is_protected && pid > 4 {
        actions.push(json!({
            "title": "强制终止进程 (Terminate)",
            "action": {"type": "execute", "id": format!("kill:{pid}")}
        }));
    }
    actions.push(json!({
        "title": "复制 PID",
        "action": {"type": "copy", "text": pid.to_string()}
    }));
    if let Some((title, port)) = port {
        actions.push(json!({
            "title": title,
            "action": {"type": "copy", "text": port.to_string()}
        }));
    }
    if let Some(path) = path {
        actions.push(json!({
            "title": "复制程序路径",
            "action": {"type": "copy", "text": path}
        }));
        if let Some(parent) = Path::new(path).parent() {
            actions.push(json!({
                "title": "打开文件所在位置",
                "action": {"type": "open", "target": parent.to_string_lossy().to_string()}
            }));
        }
    }
    actions
}

fn build_text_panel(proc: Option<&ProcessInfo>, bindings: &[PortBinding]) -> Value {
    let mut blocks = Vec::new();

    // Block 1: Process attributes
    if let Some(p) = proc {
        let path_str = p.executable_path.as_deref().unwrap_or("无法读取（系统服务或权限限制）");
        let parent_str = if p.parent_pid > 0 {
            format!("{}", p.parent_pid)
        } else {
            "无".into()
        };
        let status_str = if p.is_protected {
            format!("🔒 受保护（{}）", p.protection_reason.as_deref().unwrap_or("核心服务"))
        } else {
            "普通进程（可终止）".into()
        };
        let info_text = format!(
            "进程名: {}\nPID: {}\n父进程 PID: {}\n内存占用: {}\n状态: {}\n路径: {}",
            p.name,
            p.pid,
            parent_str,
            format_memory(p.memory_bytes),
            status_str,
            path_str
        );
        blocks.push(json!({
            "id": "info",
            "title": "进程基础属性",
            "text": info_text,
            "selectable": true
        }));
    }

    // Block 2: Network & Ports
    let port_text = if bindings.is_empty() {
        "当前无监听端口或网络连接".to_string()
    } else {
        let mut lines = Vec::new();
        for b in bindings.iter().take(12) {
            let state = b.state.map(|s| format!(" [{}]", s.as_str())).unwrap_or_default();
            let remote = b
                .remote_ip
                .map(|ip| format!(" -> {}:{}", ip, b.remote_port.unwrap_or(0)))
                .unwrap_or_default();
            lines.push(format!(
                "- {} {}:{}{}{}",
                b.protocol.as_str(),
                b.local_ip,
                b.local_port,
                remote,
                state
            ));
        }
        if bindings.len() > 12 {
            lines.push(format!("... 及其他 {} 个连接", bindings.len() - 12));
        }
        lines.join("\n")
    };
    blocks.push(json!({
        "id": "network",
        "title": "网络监听与端口绑定",
        "text": port_text,
        "selectable": true
    }));

    json!({
        "layout": "auto",
        "state": "ready",
        "blocks": blocks
    })
}
