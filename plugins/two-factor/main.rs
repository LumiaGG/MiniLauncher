//! Native TOTP plugin. No Flow DLLs, managed runtime, or network access.
mod accounts;
mod admin;
mod crypto;
#[cfg(test)]
mod tests;

use serde_json::{json, Value};
use std::{
    io::{self, Read},
    path::PathBuf,
    time::{SystemTime, UNIX_EPOCH},
};

struct Plugin {
    path: PathBuf,
}

fn now() -> Result<u64, String> {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|d| d.as_secs())
        .map_err(|_| "系统时间早于 Unix 纪元".into())
}
fn item(title: String, subtitle: String, id: String) -> Value {
    json!({"title":title,"subtitle":subtitle,"action":{"type":"execute","id":id}})
}

impl Plugin {
    fn query(&mut self, text: &str, maximum: usize) -> Result<Vec<Value>, String> {
        if maximum == 0 {
            return Ok(Vec::new());
        }
        let text = text.trim();
        let accounts = accounts::load_cached(&self.path)?;
        let time = now()?;
        let query_lower = text.to_lowercase();
        let mut items = Vec::new();
        for account in accounts
            .iter()
            .filter(|a| a.matches_lower(&query_lower))
            .take(maximum.min(100))
        {
            let code = crypto::code(
                &account.secret,
                &account.algorithm,
                account.digits,
                account.period,
                time,
            )?;
            let mut result = item(
                format!("{}  {}", code, account.label()),
                format!(
                    "{} · {} · 剩余 {} 秒 · Enter 复制当前验证码",
                    account.issuer,
                    account.name,
                    account.period - time % account.period
                ),
                format!("copy:{}", account.id),
            );
            result["context_actions"] = json!([
                {"title":"复制当前验证码","action":{"type":"execute","id":format!("copy:{}",account.id)}},
                {"title":"复制账号标识","action":{"type":"copy","text":account.id}}
            ]);
            items.push(result);
        }
        if items.is_empty() && text.is_empty() {
            items.push(item("尚无 TOTP 账号".into(), "请在 设置 → 插件 → 两步验证码 中添加或导入账号".into(), "hint".into()));
        }
        Ok(items)
    }

    fn execute(&mut self, id: &str) -> Result<Value, String> {
        let accounts = accounts::load_cached(&self.path)?;
        if let Some(id) = id.strip_prefix("copy:") {
            let account = accounts
                .iter()
                .find(|a| a.id == id)
                .ok_or("账号已移除，请重新搜索")?;
            return Ok(
                json!({"action":{"type":"copy","text":crypto::code(&account.secret, &account.algorithm, account.digits, account.period, now()?)?}}),
            );
        }
        if id == "hint" {
            return Err("请在 设置 → 插件 → 两步验证码 中添加账号".into());
        }
        Err("操作已失效，请重新搜索".into())
    }
}

fn run() -> Result<(), String> {
    let mut args = std::env::args_os().skip(1);
    let mut path = mini_launcher::config::data_dir().join("two-factor/accounts.dat");
    let mut import = None;
    let mut admin = false;
    while let Some(arg) = args.next() {
        match arg.to_str() {
            Some("--data-file") => {
                path = args
                    .next()
                    .map(PathBuf::from)
                    .ok_or("--data-file 缺少路径")?
            }
            Some("--admin") => admin = true,
            Some("--import-file") => {
                import = Some(
                    args.next()
                        .map(PathBuf::from)
                        .ok_or("--import-file 缺少路径")?,
                )
            }
            _ => {
                return Err(
                    "用法：two-factor [--data-file 路径] [--import-file Flow导出JSON或URI文本] [--admin]"
                        .into(),
                )
            }
        }
    }
    if let Some(import) = import {
        let incoming = accounts::import_file(&import)?;
        let mut existing = accounts::load(&path)?;
        let count = accounts::append(&mut existing, incoming);
        accounts::save(&path, &existing)?;
        println!("已导入 {count} 个账号");
        return Ok(());
    }
    if admin {
        // One bounded JSON request on stdin; the reply never contains secrets.
        let mut text = String::new();
        io::stdin().lock().take(64 * 1024).read_to_string(&mut text).map_err(|_| "无法读取请求")?;
        let request = serde_json::from_str(&text).map_err(|_| "请求格式无效")?;
        let reply = admin::handle(&path, &request).unwrap_or_else(|error| json!({"error": error}));
        println!("{reply}");
        return Ok(());
    }
    let mut plugin = Plugin { path };
    let mut input = io::stdin().lock();
    let mut output = io::stdout().lock();
    mini_launcher::plugin_rpc::serve(&mut input, &mut output, |request| {
        match request["method"].as_str().unwrap_or("") {
            "initialize" => Ok(json!({"protocol_version":2})),
            "query" => plugin
                .query(
                    request["params"]["query"].as_str().unwrap_or(""),
                    request["params"]["max_results"]
                        .as_u64()
                        .unwrap_or(100)
                        .min(100) as usize,
                )
                .map(|items| json!({"items":items})),
            "execute" => plugin.execute(request["params"]["action_id"].as_str().unwrap_or("")),
            _ => Err("不支持的方法".into()),
        }
    })?;
    Ok(())
}

fn main() {
    if let Err(error) = run() {
        eprintln!("{error}");
        std::process::exit(1);
    }
}
