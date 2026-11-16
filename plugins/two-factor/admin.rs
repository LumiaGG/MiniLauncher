//! Account management for the settings window: `two-factor --admin` reads one JSON
//! request from stdin and writes one JSON response. Secrets are never returned.
use crate::accounts::{self, Account};
use serde_json::{json, Value};
use std::path::{Path, PathBuf};

fn summary(account: &Account) -> Value {
    json!({
        "id": account.id,
        "name": account.name,
        "issuer": account.issuer,
        "remark": account.remark,
        "algorithm": account.algorithm,
        "digits": account.digits,
        "period": account.period,
    })
}

fn number(request: &Value, key: &str, default: u64) -> Result<u64, String> {
    match request.get(key) {
        None | Some(Value::Null) => Ok(default),
        Some(value) => value.as_u64().ok_or_else(|| format!("{key} 无效")),
    }
}

/// Apply one operation. Mutations persist atomically; failures leave the store untouched.
pub fn handle(path: &Path, request: &Value) -> Result<Value, String> {
    let text = |key: &str| request[key].as_str().unwrap_or("").trim().to_string();
    let mut accounts = accounts::load(path)?;
    let mut message = String::new();
    let mut changed = false;
    match request["op"].as_str().unwrap_or("") {
        "list" => {}
        "add" => {
            let account = Account::new(
                &text("name"),
                &text("issuer"),
                &text("remark"),
                &text("secret"),
                &if text("algorithm").is_empty() {
                    "SHA1".into()
                } else {
                    text("algorithm")
                },
                u32::try_from(number(request, "digits", 6)?).map_err(|_| "位数无效")?,
                number(request, "period", 30)?,
            )?;
            changed = accounts::append(&mut accounts, vec![account]) > 0;
            message = if changed {
                "已添加账号".into()
            } else {
                "相同账号已存在，未重复添加".into()
            };
        }
        "uri" => {
            let incoming = accounts::uri(&text("text"))?;
            changed = accounts::append(&mut accounts, vec![incoming]) > 0;
            message = if changed {
                "已添加账号".into()
            } else {
                "相同账号已存在，未重复添加".into()
            };
        }
        "import" => {
            let incoming = accounts::import_file(&PathBuf::from(text("path")))?;
            let total = incoming.len();
            let added = accounts::append(&mut accounts, incoming);
            changed = added > 0;
            message = format!("导入 {total} 个账号，新增 {added} 个（相同账号已自动去重）");
        }
        "delete" => {
            let id = text("id");
            let before = accounts.len();
            accounts.retain(|a| a.id != id);
            if accounts.len() == before {
                return Err("账号已不存在".into());
            }
            changed = true;
            message = "已删除账号".into();
        }
        "edit" => {
            let id = text("id");
            let account = accounts
                .iter_mut()
                .find(|a| a.id == id)
                .ok_or("账号已不存在")?;
            let mut edited = account.clone();
            edited.name = text("name");
            edited.issuer = text("issuer");
            edited.remark = text("remark");
            edited.validate()?;
            *account = edited;
            changed = true;
            message = "已保存修改".into();
        }
        _ => return Err("不支持的操作".into()),
    }
    if changed {
        accounts::save(path, &accounts)?;
    }
    Ok(json!({
        "accounts": accounts.iter().map(summary).collect::<Vec<_>>(),
        "message": message,
    }))
}
