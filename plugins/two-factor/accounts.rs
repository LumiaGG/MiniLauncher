use crate::crypto;
use pinyin::{ToPinyin, ToPinyinMulti};
use serde::{Deserialize, Serialize};
use serde_json::Value;
use std::{
    collections::HashMap,
    fs,
    io::{Read, Write},
    path::{Path, PathBuf},
    sync::{Mutex, OnceLock},
    time::SystemTime,
};
use windows_sys::Win32::Storage::FileSystem::{
    MoveFileExW, MOVEFILE_REPLACE_EXISTING, MOVEFILE_WRITE_THROUGH,
};

const MAX_FILE: u64 = 4 * 1024 * 1024;

#[derive(Clone)]
struct AccountSearch {
    source: String,
    full: String,
    initials: String,
}

#[derive(Clone, Serialize, Deserialize)]
pub struct Account {
    pub id: String,
    pub name: String,
    pub issuer: String,
    pub remark: String,
    pub secret: Vec<u8>,
    pub algorithm: String,
    pub digits: u32,
    pub period: u64,
    #[serde(skip)]
    search_cache: OnceLock<AccountSearch>,
}

impl Account {
    /// Build a validated account from user-entered fields; `secret` is Base32 text.
    pub fn new(
        name: &str,
        issuer: &str,
        remark: &str,
        secret: &str,
        algorithm: &str,
        digits: u32,
        period: u64,
    ) -> Result<Account, String> {
        let account = Account {
            id: crypto::random_id()?,
            name: name.trim().into(),
            issuer: issuer.trim().into(),
            remark: remark.trim().into(),
            secret: crypto::base32(secret)?,
            algorithm: algorithm.trim().to_ascii_uppercase(),
            digits,
            period,
            search_cache: OnceLock::new(),
        };
        account.validate()?;
        Ok(account)
    }

    pub fn validate(&self) -> Result<(), String> {
        if self.name.trim().is_empty()
            || [&self.name, &self.issuer, &self.remark]
                .iter()
                .any(|s| s.len() > 200 || s.chars().any(char::is_control))
        {
            return Err("账号名称不能为空；名称、服务商和备注各限 200 字节，不能含控制字符".into());
        }
        if self.secret.is_empty()
            || self.secret.len() > 2560
            || !(6..=8).contains(&self.digits)
            || !(1..=86400).contains(&self.period)
            || !matches!(self.algorithm.as_str(), "SHA1" | "SHA256" | "SHA512")
        {
            return Err("账号参数无效：支持 SHA1/256/512、6–8 位和 1–86400 秒周期".into());
        }
        if self.id.len() != 32 || !self.id.bytes().all(|b| b.is_ascii_hexdigit()) {
            return Err("账号标识无效".into());
        }
        Ok(())
    }
    pub fn label(&self) -> &str {
        if self.remark.is_empty() {
            &self.name
        } else {
            &self.remark
        }
    }
    fn search_data(&self) -> &AccountSearch {
        self.search_cache.get_or_init(|| {
            let source = format!("{} {} {}", self.name, self.issuer, self.remark).to_lowercase();
            let (mut full, mut initials) = (String::new(), String::new());
            for ch in source.chars() {
                if let Some(py) = ch.to_pinyin() {
                    full.push_str(py.plain());
                    initials.push(py.plain().chars().next().unwrap());
                } else {
                    full.push(ch);
                    initials.push(ch);
                }
            }
            AccountSearch {
                source,
                full,
                initials,
            }
        })
    }
    #[cfg(test)]
    pub fn matches(&self, query: &str) -> bool {
        let query_lower = query.to_lowercase();
        self.matches_lower(&query_lower)
    }
    pub fn matches_lower(&self, query_lower: &str) -> bool {
        let data = self.search_data();
        query_lower.split_whitespace().all(|word| {
            data.source.contains(word)
                || data.full.contains(word)
                || data.initials.contains(word)
                || phonetic_match(&data.source, word, false)
                || phonetic_match(&data.source, word, true)
        })
    }
}

// Match across alternate pronunciations without enumerating exponential combinations.
fn phonetic_match(source: &str, word: &str, initials: bool) -> bool {
    if !word.is_ascii() {
        return false;
    }
    let mut offsets = vec![0];
    for ch in source.chars() {
        let choices: Vec<String> = match ch.to_pinyin_multi() {
            Some(py) => py
                .into_iter()
                .map(|p| {
                    if initials {
                        p.plain()[..1].into()
                    } else {
                        p.plain().into()
                    }
                })
                .collect(),
            None => vec![ch.to_string()],
        };
        let mut next = vec![0];
        for offset in offsets {
            for choice in &choices {
                let remaining = &word[offset..];
                if choice.starts_with(remaining) {
                    return true;
                }
                if remaining.starts_with(choice) {
                    next.push(offset + choice.len());
                }
            }
        }
        next.sort_unstable();
        next.dedup();
        offsets = next;
    }
    false
}

fn decode(text: &str) -> Result<String, String> {
    let mut bytes = Vec::new();
    let mut source = text.bytes();
    while let Some(ch) = source.next() {
        if ch == b'%' {
            let high = source.next().and_then(|b| (b as char).to_digit(16));
            let low = source.next().and_then(|b| (b as char).to_digit(16));
            match (high, low) {
                (Some(h), Some(l)) => bytes.push((h * 16 + l) as u8),
                _ => return Err("URI 百分号编码无效".into()),
            }
        } else {
            bytes.push(ch);
        }
    }
    String::from_utf8(bytes).map_err(|_| "URI 不是 UTF-8".into())
}

pub fn uri(text: &str) -> Result<Account, String> {
    let rest = text
        .trim()
        .strip_prefix("otpauth://totp/")
        .ok_or("仅支持 otpauth://totp/ 地址；不支持 HOTP 或 Google migration 地址")?;
    if rest.contains('#') {
        return Err("OTP 地址不能含片段".into());
    }
    let (label, params) = rest.split_once('?').ok_or("OTP 地址缺少参数")?;
    let label = decode(label)?;
    let mut fields = HashMap::new();
    for field in params.split('&') {
        let (key, value) = field.split_once('=').ok_or("OTP 地址参数无效")?;
        if fields.insert(decode(key)?, decode(value)?).is_some() {
            return Err("OTP 地址包含重复参数".into());
        }
    }
    let (prefix, name) = label
        .split_once(':')
        .map_or(("", label.as_str()), |(p, n)| (p, n));
    let issuer = fields.get("issuer").map(String::as_str).unwrap_or(prefix);
    if !prefix.is_empty() && prefix != issuer {
        return Err("OTP 地址的服务商与标签不一致".into());
    }
    let account = Account {
        id: crypto::random_id()?,
        name: name.trim().into(),
        issuer: issuer.into(),
        remark: String::new(),
        secret: crypto::base32(fields.get("secret").ok_or("OTP 地址缺少 secret")?)?,
        algorithm: fields
            .get("algorithm")
            .map(|s| s.to_ascii_uppercase())
            .unwrap_or("SHA1".into()),
        digits: fields
            .get("digits")
            .map_or(Ok(6), |s| s.parse::<u32>().map_err(|_| "digits 无效"))?,
        period: fields
            .get("period")
            .map_or(Ok(30), |s| s.parse::<u64>().map_err(|_| "period 无效"))?,
        search_cache: OnceLock::new(),
    };
    account.validate()?;
    Ok(account)
}

pub fn read_limited(path: &Path) -> Result<Vec<u8>, String> {
    let mut bytes = Vec::new();
    fs::File::open(path)
        .and_then(|file| file.take(MAX_FILE + 1).read_to_end(&mut bytes))
        .map_err(|_| "无法读取账号文件".to_string())?;
    if bytes.len() as u64 > MAX_FILE {
        return Err("账号文件超过 4 MiB".into());
    }
    Ok(bytes)
}

type CacheKey = (PathBuf, Option<SystemTime>, u64);

fn account_cache() -> &'static Mutex<Option<(CacheKey, Vec<Account>)>> {
    static CACHE: OnceLock<Mutex<Option<(CacheKey, Vec<Account>)>>> = OnceLock::new();
    CACHE.get_or_init(|| Mutex::new(None))
}

fn file_stamp(path: &Path) -> Option<CacheKey> {
    let meta = fs::metadata(path).ok()?;
    Some((path.to_path_buf(), meta.modified().ok(), meta.len()))
}

pub fn load_cached(path: &Path) -> Result<Vec<Account>, String> {
    let stamp = match fs::metadata(path) {
        Ok(meta) => (path.to_path_buf(), meta.modified().ok(), meta.len()),
        Err(err) if err.kind() == std::io::ErrorKind::NotFound => {
            if let Ok(mut guard) = account_cache().lock() {
                if guard.as_ref().is_some_and(|((p, _, _), _)| p == path) {
                    *guard = None;
                }
            }
            return Ok(Vec::new());
        }
        Err(_) => return load(path),
    };
    if let Ok(guard) = account_cache().lock() {
        if let Some((cached_key, cached_accounts)) = guard.as_ref() {
            if cached_key == &stamp {
                return Ok(cached_accounts.clone());
            }
        }
    }
    let accounts = load(path)?;
    for account in &accounts {
        let _ = account.search_data();
    }
    if let Ok(mut guard) = account_cache().lock() {
        *guard = Some((stamp, accounts.clone()));
    }
    Ok(accounts)
}

pub fn load(path: &Path) -> Result<Vec<Account>, String> {
    let bytes = match read_limited(path) {
        Ok(bytes) => bytes,
        Err(_) if !path.try_exists().map_err(|_| "无法检查账号库")? => return Ok(Vec::new()),
        Err(error) => return Err(error),
    };
    let mut plain = crypto::protect(&bytes, true)?;
    let result =
        serde_json::from_slice::<Vec<Account>>(&plain).map_err(|_| "账号库格式无效".to_string());
    plain.fill(0);
    let accounts = result?;
    if accounts.len() > 1000 {
        return Err("最多保存 1000 个账号".into());
    }
    let mut ids = std::collections::HashSet::new();
    for account in &accounts {
        account.validate()?;
        if !ids.insert(&account.id) {
            return Err("账号标识重复".into());
        }
    }
    Ok(accounts)
}

pub fn save(path: &Path, accounts: &[Account]) -> Result<(), String> {
    if accounts.len() > 1000 {
        return Err("最多保存 1000 个账号".into());
    }
    let mut plain = serde_json::to_vec(accounts).map_err(|_| "序列化账号库失败")?;
    let encrypted = crypto::protect(&plain, false);
    plain.fill(0);
    let encrypted = encrypted?;
    if encrypted.len() as u64 > MAX_FILE {
        return Err("账号库超过 4 MiB".into());
    }
    if let Some(parent) = path.parent().filter(|p| !p.as_os_str().is_empty()) {
        fs::create_dir_all(parent).map_err(|_| "无法创建账号目录")?;
    }
    let temporary = PathBuf::from(format!("{}.{}.tmp", path.display(), crypto::random_id()?));
    let result = (|| {
        let mut file = fs::OpenOptions::new()
            .create_new(true)
            .write(true)
            .open(&temporary)
            .map_err(|_| "无法创建账号库临时文件")?;
        file.write_all(&encrypted)
            .and_then(|_| file.sync_all())
            .map_err(|_| "写入账号库失败")?;
        drop(file);
        let wide = |p: &Path| {
            p.as_os_str()
                .encode_wide()
                .chain(Some(0))
                .collect::<Vec<u16>>()
        };
        use std::os::windows::ffi::OsStrExt;
        if unsafe {
            MoveFileExW(
                wide(&temporary).as_ptr(),
                wide(path).as_ptr(),
                MOVEFILE_REPLACE_EXISTING | MOVEFILE_WRITE_THROUGH,
            )
        } == 0
        {
            return Err("替换账号库失败");
        }
        Ok(())
    })();
    if result.is_err() {
        let _ = fs::remove_file(&temporary);
    }
    let res = result.map_err(str::to_string);
    if res.is_ok() {
        if let Ok(mut guard) = account_cache().lock() {
            if let Some(stamp) = file_stamp(path) {
                let prepared: Vec<Account> = accounts
                    .iter()
                    .map(|a| {
                        let c = Account {
                            id: a.id.clone(),
                            name: a.name.clone(),
                            issuer: a.issuer.clone(),
                            remark: a.remark.clone(),
                            secret: a.secret.clone(),
                            algorithm: a.algorithm.clone(),
                            digits: a.digits,
                            period: a.period,
                            search_cache: OnceLock::new(),
                        };
                        let _ = c.search_data();
                        c
                    })
                    .collect();
                *guard = Some((stamp, prepared));
            } else {
                *guard = None;
            }
        }
    }
    res
}

/// Read Flow's exported array or settings object, or one otpauth URI per line.
pub fn import_file(path: &Path) -> Result<Vec<Account>, String> {
    let bytes = read_limited(path)?;
    let text = std::str::from_utf8(&bytes)
        .map_err(|_| "导入文件必须为 UTF-8")?
        .trim_start_matches('\u{feff}')
        .trim();
    if text.starts_with('[') || text.starts_with('{') {
        let root: Value = serde_json::from_str(text).map_err(|_| "导入 JSON 格式无效")?;
        let list = root
            .as_array()
            .or_else(|| root.get("OtpParams").and_then(Value::as_array))
            .ok_or("需要 Flow 导出数组或含 OtpParams 的配置")?;
        if list.len() > 1000 {
            return Err("一次最多导入 1000 个账号".into());
        }
        list.iter()
            .map(|value| {
                if value["OtpType"].as_str() != Some("totp") {
                    return Err("导入包含非 TOTP 账号，未写入任何账号".into());
                }
                let string = |key: &str| value[key].as_str().unwrap_or("").to_owned();
                let algorithm = string("Algorithm");
                let digits = match value.get("Digits") {
                    None => 6,
                    Some(v) => {
                        u32::try_from(v.as_u64().ok_or("位数无效")?).map_err(|_| "位数无效")?
                    }
                };
                let account = Account {
                    id: crypto::random_id()?,
                    name: string("Name"),
                    issuer: string("Issuer"),
                    remark: string("Remark"),
                    secret: crypto::base32(&string("Secret"))?,
                    algorithm: if algorithm.is_empty() {
                        "SHA1".into()
                    } else {
                        algorithm.to_ascii_uppercase()
                    },
                    digits,
                    period: 30,
                    search_cache: OnceLock::new(),
                };
                account.validate()?;
                Ok(account)
            })
            .collect()
    } else {
        let lines: Vec<_> = text
            .lines()
            .map(str::trim)
            .filter(|s| !s.is_empty())
            .collect();
        if lines.len() > 1000 {
            return Err("一次最多导入 1000 个账号".into());
        }
        lines.into_iter().map(uri).collect()
    }
}

pub fn append(existing: &mut Vec<Account>, incoming: Vec<Account>) -> usize {
    let before = existing.len();
    for account in incoming {
        if !existing.iter().any(|a| {
            a.secret == account.secret
                && a.algorithm == account.algorithm
                && a.digits == account.digits
                && a.period == account.period
                && a.name == account.name
                && a.issuer == account.issuer
        }) {
            existing.push(account);
        }
    }
    existing.len() - before
}
