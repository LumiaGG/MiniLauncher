use mini_launcher::plugin_rpc::{read_frame, write_response};
use serde_json::{json, Value};
use std::{
    io,
    sync::{
        atomic::{AtomicU64, Ordering},
        mpsc, Arc, Mutex,
    },
    time::Duration,
};

fn write_reply(
    output: &Arc<Mutex<io::Stdout>>,
    id: &Value,
    response: &Value,
) -> Result<(), String> {
    let mut guard = output.lock().map_err(|_| "输出锁已损坏".to_string())?;
    write_response(&mut *guard, id, response)
}

fn initialize(params: &Value, settings: &mut toml::Value) -> Result<Value, String> {
    let loaded = params["settings_path"]
        .as_str()
        .map(|p| {
            std::fs::read_to_string(p)
                .map_err(|_| "无法读取进程管理设置".to_string())
                .and_then(|s| {
                    toml::from_str::<toml::Value>(&s).map_err(|_| "进程管理设置格式无效".into())
                })
        })
        .transpose()?;
    if let Some(config) = loaded {
        *settings = config
            .get("plugins")
            .and_then(|p| p.get(params["config_key"].as_str().unwrap_or("process_killer")))
            .cloned()
            .unwrap_or_else(|| settings.clone());
    }
    Ok(json!({"protocol_version":2}))
}

struct PendingQuery {
    seq: u64,
    id: Value,
    params: Value,
    cooldown: Duration,
}

pub(super) fn run() {
    let mut settings: toml::Value = toml::from_str("").unwrap();
    let output = Arc::new(Mutex::new(io::stdout()));
    let worker_output = output.clone();
    let latest_seq = Arc::new(AtomicU64::new(0));
    let worker_latest = latest_seq.clone();
    let (work_tx, work_rx) = mpsc::channel::<PendingQuery>();
    let worker = std::thread::spawn(move || {
        while let Ok(mut pending) = work_rx.recv() {
            loop {
                // An empty query or zero cooldown still drains already queued
                // input, but does not wait before showing the initial results.
                let cooldown = if pending.params["query"]
                    .as_str()
                    .unwrap_or("")
                    .trim()
                    .is_empty()
                {
                    Duration::ZERO
                } else {
                    pending.cooldown
                };
                match work_rx.recv_timeout(cooldown) {
                    Ok(newer) => {
                        let id = &pending.id;
                        let superseded = json!({"jsonrpc":"2.0","id":id,"result":{"items":[]}});
                        if write_reply(&worker_output, id, &superseded).is_err() {
                            return;
                        }
                        pending = newer;
                    }
                    Err(mpsc::RecvTimeoutError::Timeout) => break,
                    Err(mpsc::RecvTimeoutError::Disconnected) => return,
                }
            }
            let PendingQuery {
                seq, id, params, ..
            } = pending;
            if seq != worker_latest.load(Ordering::Acquire) {
                let superseded = json!({"jsonrpc":"2.0","id":id,"result":{"items":[]}});
                if write_reply(&worker_output, &id, &superseded).is_err() {
                    break;
                }
                continue;
            }
            let result = super::handle_query(&params);
            if seq != worker_latest.load(Ordering::Acquire) {
                let superseded = json!({"jsonrpc":"2.0","id":id,"result":{"items":[]}});
                if write_reply(&worker_output, &id, &superseded).is_err() {
                    break;
                }
                continue;
            }
            let response = match result {
                Ok(result) => json!({"jsonrpc":"2.0","id":id,"result":result}),
                Err(message) => {
                    json!({"jsonrpc":"2.0","id":id,"error":{"code":-32000,"message":message}})
                }
            };
            if write_reply(&worker_output, &id, &response).is_err() {
                break;
            }
        }
    });
    let mut input = io::stdin().lock();
    while let Ok(Some(request)) = read_frame(&mut input) {
        let method = request["method"].as_str().unwrap_or("");
        if method == "shutdown" {
            latest_seq.fetch_add(1, Ordering::AcqRel);
            break;
        }
        if method == "cancel" {
            latest_seq.fetch_add(1, Ordering::AcqRel);
            continue;
        }
        let Some(id) = request.get("id").cloned() else {
            continue;
        };
        if request["jsonrpc"] != "2.0" || method.is_empty() {
            let _ = write_reply(
                &output,
                &id,
                &json!({"jsonrpc":"2.0","id":id,"error":{"code":-32600,"message":"无效 JSON-RPC 2.0 请求"}}),
            );
            continue;
        }
        if method == "initialize" {
            let response = if request["params"]["protocol_version"] != 2 {
                json!({"jsonrpc":"2.0","id":id,"error":{"code":-32602,"message":"仅支持插件协议 v2"}})
            } else {
                match initialize(&request["params"], &mut settings) {
                    Ok(result) => json!({"jsonrpc":"2.0","id":id,"result":result}),
                    Err(message) => {
                        json!({"jsonrpc":"2.0","id":id,"error":{"code":-32000,"message":message}})
                    }
                }
            };
            if write_reply(&output, &id, &response).is_err() {
                break;
            }
            continue;
        }
        if method == "query" {
            let seq = latest_seq.fetch_add(1, Ordering::AcqRel) + 1;
            let pending = PendingQuery {
                seq,
                id,
                params: request["params"].clone(),
                cooldown: Duration::from_millis(debounce_ms(&settings)),
            };
            if work_tx.send(pending).is_err() {
                break;
            }
            continue;
        }
        let response = match super::dispatch(&request) {
            Ok(result) => json!({"jsonrpc":"2.0","id":id,"result":result}),
            Err(message) => {
                json!({"jsonrpc":"2.0","id":id,"error":{"code":-32000,"message":message}})
            }
        };
        if write_reply(&output, &id, &response).is_err() {
            break;
        }
    }
    drop(work_tx);
    let _ = worker.join();
}

fn debounce_ms(settings: &toml::Value) -> u64 {
    settings
        .get("debounce_ms")
        .and_then(|v| v.as_integer())
        .map(|ms| ms.clamp(0, 5000) as u64)
        .unwrap_or(350)
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn cooldown_defaults_and_bounds() {
        for (config, expected) in [
            ("", 350),
            ("debounce_ms = 500", 500),
            ("debounce_ms = 0", 0),
            ("debounce_ms = -1", 0),
            ("debounce_ms = 99999", 5000),
        ] {
            assert_eq!(debounce_ms(&toml::from_str(config).unwrap()), expected);
        }
    }
}
