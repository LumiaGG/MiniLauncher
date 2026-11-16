//! Native Process & Port Killer plugin for MiniLauncher.
mod model;
mod ports;
mod processes;
mod view;
#[cfg(test)]
mod tests;

use serde_json::{json, Value};
mod runtime;

fn handle_query(params: &Value) -> Result<Value, String> {
    let query_str = params
        .get("query")
        .and_then(|v| v.as_str())
        .unwrap_or("");
    let max_results = params
        .get("max_results")
        .and_then(|v| v.as_u64())
        .map(|v| v as usize)
        .unwrap_or(50)
        .clamp(1, 100);

    let intent = model::parse_query(query_str);
    let bindings = ports::get_all_port_bindings();
    let processes = processes::get_all_processes();
    let pid_to_bindings = ports::get_bindings_by_pid(&bindings);

    let items = view::render_results(&intent, &processes, &bindings, &pid_to_bindings, max_results);
    Ok(json!({ "items": items }))
}

fn handle_execute(params: &Value) -> Result<Value, String> {
    let action_id = params
        .get("action_id")
        .and_then(|v| v.as_str())
        .ok_or_else(|| "缺少 action_id 参数".to_string())?;

    if let Some(pid_str) = action_id.strip_prefix("kill:") {
        let pid = pid_str
            .parse::<u32>()
            .map_err(|_| format!("无效的 PID: {pid_str}"))?;

        processes::terminate_process(pid)?;
        return Ok(json!({}));
    }

    Err(format!("未知动作: {action_id}"))
}

fn dispatch(request: &Value) -> Result<Value, String> {
    let method = request["method"].as_str().unwrap_or("");
    let params = &request["params"];

    match method {
        "initialize" => {
            if params["protocol_version"] != 2 {
                return Err("仅支持插件协议版本 2".into());
            }
            Ok(json!({ "protocol_version": 2 }))
        }
        "query" => handle_query(params),
        "execute" => handle_execute(params),
        _ => Err(format!("不支持的 RPC 方法: {method}")),
    }
}

fn main() {
    runtime::run();
}
