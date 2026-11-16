//! Small synchronous NDJSON server shared by bundled plugins.
//! Rendering and pointer/selection events stay in the host process.
use serde_json::{json, Value};
use std::io::{BufRead, Read, Write};

pub const MAX_FRAME: usize = 256 * 1024;

pub fn read_frame(input: &mut impl BufRead) -> Result<Option<Value>, String> {
    let mut bytes = Vec::new();
    read_frame_into(input, &mut bytes)
}

pub fn read_frame_into(
    input: &mut impl BufRead,
    bytes: &mut Vec<u8>,
) -> Result<Option<Value>, String> {
    bytes.clear();
    let count = input
        .take((MAX_FRAME + 1) as u64)
        .read_until(b'\n', bytes)
        .map_err(|e| e.to_string())?;
    if count == 0 {
        return Ok(None);
    }
    if bytes.len() > MAX_FRAME || bytes.last() != Some(&b'\n') {
        return Err("插件帧超过 256 KiB 或缺少换行".into());
    }
    serde_json::from_slice(bytes)
        .map(Some)
        .map_err(|e| format!("无效插件 JSON：{e}"))
}

fn encode_frame(value: &Value) -> Result<Vec<u8>, String> {
    let mut bytes = serde_json::to_vec(value).map_err(|e| e.to_string())?;
    bytes.push(b'\n');
    Ok(bytes)
}

fn write_bytes(output: &mut impl Write, bytes: &[u8]) -> Result<(), String> {
    if bytes.len() > MAX_FRAME {
        return Err("插件帧超过 256 KiB".into());
    }
    output
        .write_all(bytes)
        .and_then(|_| output.flush())
        .map_err(|e| e.to_string())
}

pub fn write_frame(output: &mut impl Write, value: &Value) -> Result<(), String> {
    write_bytes(output, &encode_frame(value)?)
}

/// Write a JSON-RPC response, replacing oversized results with a bounded error.
pub fn write_response(output: &mut impl Write, id: &Value, response: &Value) -> Result<(), String> {
    let bytes = encode_frame(response)?;
    if bytes.len() > MAX_FRAME {
        return write_frame(
            output,
            &error(id, -32000, "结果超过协议帧上限，请缩小查询范围"),
        );
    }
    write_bytes(output, &bytes)
}

fn error(id: &Value, code: i32, message: &str) -> Value {
    json!({"jsonrpc":"2.0", "id":id, "error":{"code":code,"message":message}})
}

pub fn serve(
    input: &mut impl BufRead,
    output: &mut impl Write,
    mut dispatch: impl FnMut(&Value) -> Result<Value, String>,
) -> Result<(), String> {
    let mut frame_buf = Vec::new();
    while let Some(request) = read_frame_into(input, &mut frame_buf)? {
        // Notifications have no reply. Shutdown must work without an id.
        if request["method"] == "shutdown" {
            break;
        }
        let Some(id) = request.get("id") else {
            continue;
        };
        let response = if request["jsonrpc"] != "2.0" || !request["method"].is_string() {
            error(id, -32600, "无效 JSON-RPC 2.0 请求")
        } else if request["method"] == "initialize" && request["params"]["protocol_version"] != 2 {
            error(id, -32602, "仅支持插件协议 v2")
        } else {
            match dispatch(&request) {
                Ok(result) => json!({"jsonrpc":"2.0","id":id,"result":result}),
                Err(message) => error(id, -32000, &message),
            }
        };
        // Bound the encoded response as well as input allocation. Do not truncate UTF-8/JSON.
        write_response(output, id, &response)?;
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::io::Cursor;

    #[test]
    fn unicode_limits_notifications_and_version_negotiation() {
        let mut input = Cursor::new(concat!(
            "{\"jsonrpc\":\"2.0\",\"id\":1,\"method\":\"initialize\",\"params\":{\"protocol_version\":3}}\n",
            "{\"jsonrpc\":\"2.0\",\"method\":\"cancel\"}\n",
            "{\"jsonrpc\":\"2.0\",\"id\":2,\"method\":\"query\"}\n",
            "{\"jsonrpc\":\"2.0\",\"method\":\"shutdown\"}\n",
            "{\"jsonrpc\":\"2.0\",\"id\":3,\"method\":\"query\"}\n"
        ));
        let mut output = Vec::new();
        let mut calls = 0;
        serve(&mut input, &mut output, |_| {
            calls += 1;
            Ok(json!({"text":"中文😀"}))
        })
        .unwrap();
        assert_eq!(calls, 1);
        let mut output = Cursor::new(output);
        assert_eq!(
            read_frame(&mut output).unwrap().unwrap()["error"]["code"],
            -32602
        );
        assert_eq!(
            read_frame(&mut output).unwrap().unwrap()["result"]["text"],
            "中文😀"
        );
        assert!(read_frame(&mut output).unwrap().is_none());
        assert!(read_frame(&mut Cursor::new(vec![b'x'; MAX_FRAME + 10])).is_err());
    }

    #[test]
    fn oversized_result_returns_bounded_error_and_server_continues() {
        let request = b"{\"jsonrpc\":\"2.0\",\"id\":1,\"method\":\"query\"}\n";
        let mut output = Vec::new();
        let mut calls = 0;
        serve(&mut Cursor::new(request.repeat(2)), &mut output, |_| {
            calls += 1;
            Ok(json!({"text": if calls == 1 { "中".repeat(MAX_FRAME) } else { "ok".into() }}))
        })
        .unwrap();
        assert!(output.len() < MAX_FRAME);
        let mut output = Cursor::new(output);
        let response = read_frame(&mut output).unwrap().unwrap();
        assert_eq!(response["id"], 1);
        assert_eq!(response["error"]["code"], -32000);
        assert_eq!(
            read_frame(&mut output).unwrap().unwrap()["result"]["text"],
            "ok"
        );
        assert!(read_frame(&mut output).unwrap().is_none());
    }

    #[test]
    fn frame_size_includes_json_encoding_and_newline() {
        let value = json!("x".repeat(MAX_FRAME - 3));
        let mut output = Vec::new();
        write_frame(&mut output, &value).unwrap();
        assert_eq!(output.len(), MAX_FRAME);
        assert_eq!(read_frame(&mut Cursor::new(&output)).unwrap(), Some(value));

        output.clear();
        assert!(write_frame(&mut output, &json!("x".repeat(MAX_FRAME - 2))).is_err());
        assert!(output.is_empty());
    }
}
