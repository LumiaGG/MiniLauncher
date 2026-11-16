//! Build with: cargo build --release --example echo-plugin
use serde_json::{json, Value};
use std::io::{self, BufRead, Write};

fn main() {
    let args: Vec<String> = std::env::args().collect();
    let fixture_target = args
        .iter()
        .position(|arg| arg == "--fixture-target")
        .and_then(|index| args.get(index + 1));
    let mut output = io::stdout().lock();
    for line in io::stdin().lock().lines().map_while(Result::ok) {
        let Ok(request) = serde_json::from_str::<Value>(&line) else {
            continue;
        };
        let Some(id) = request.get("id") else {
            continue;
        }; // Notifications (cancel/shutdown) have no reply.
        let method = request.get("method").and_then(Value::as_str).unwrap_or("");
        let params = &request["params"];
        if method == "query" && args.iter().any(|arg| arg == "--hang-query") {
            std::thread::sleep(std::time::Duration::from_secs(60));
        }
        if method == "query" && args.iter().any(|arg| arg == "--malformed-query") {
            writeln!(output, "invalid json").unwrap();
            output.flush().unwrap();
            continue;
        }
        let result = match method {
            "initialize" => json!({"protocol_version":2,"name":"Echo demo"}),
            "query" => {
                let text = params["query"].as_str().unwrap_or("");
                json!({"items":[
                    {"title":if text.is_empty(){"输入 echo 内容".into()}else{format!("复制：{text}")},
                     "subtitle":"Enter 将内容复制到剪贴板", "action":{"type":"copy","text":text},
                     "context_actions":[{"title":"插件信息","action":{"type":"execute","id":"about"}}]},
                    {"title":"插件信息","subtitle":"通过独立进程执行动作","action":{"type":"execute","id":"about"}}
                ]})
            }
            "execute" if params["action_id"] == "about" => {
                if let Some(target) = fixture_target {
                    json!({"action":{"type":"open","target":target}})
                } else {
                    json!({"action":{"type":"copy","text":"MiniLauncher plugin protocol v2"}})
                }
            }
            "shutdown" => break,
            _ => {
                writeln!(output,"{}",json!({"jsonrpc":"2.0","id":id,"error":{"code":-32601,"message":"Unknown method"}})).unwrap();
                output.flush().unwrap();
                continue;
            }
        };
        writeln!(
            output,
            "{}",
            json!({"jsonrpc":"2.0","id":id,"result":result})
        )
        .unwrap();
        output.flush().unwrap();
    }
}
