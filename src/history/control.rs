//! Settings edits are sent to the running history owner using JSON-RPC over
//! WM_COPYDATA, so a queued usage save cannot overwrite another process's edits.
use super::Edit;
use serde::{Deserialize, Serialize};
use std::path::Path;

#[derive(Deserialize, Serialize)]
#[serde(deny_unknown_fields)]
pub struct Request {
    pub jsonrpc: String,
    pub id: u64,
    pub method: String,
    pub params: Changes,
}

#[derive(Deserialize, Serialize)]
#[serde(deny_unknown_fields)]
pub struct Changes {
    pub settings_path: std::path::PathBuf,
    pub edits: Vec<Edit>,
}

pub const MAX_REQUEST_BYTES: usize = 256 * 1024;

pub fn decode(bytes: &[u8], settings_path: &Path) -> Option<Changes> {
    if bytes.len() > MAX_REQUEST_BYTES {
        return None;
    }
    let request: Request = serde_json::from_slice(bytes).ok()?;
    if request.jsonrpc != "2.0" || request.method != "applications.update" {
        return None;
    }
    let expected = std::fs::canonicalize(settings_path).ok()?;
    let actual = std::fs::canonicalize(&request.params.settings_path).ok()?;
    (expected == actual).then_some(request.params)
}

pub fn save(settings_path: &Path, edits: &[Edit]) -> Result<(), String> {
    if edits.is_empty() {
        return Ok(());
    }
    #[cfg(windows)]
    unsafe {
        use windows_sys::Win32::System::DataExchange::COPYDATASTRUCT;
        use windows_sys::Win32::UI::WindowsAndMessaging::*;
        let class: Vec<u16> = "MiniLauncher.Native.Window"
            .encode_utf16()
            .chain(Some(0))
            .collect();
        let host = FindWindowW(class.as_ptr(), std::ptr::null());
        if !host.is_null() {
            let request = Request {
                jsonrpc: "2.0".into(),
                id: 1,
                method: "applications.update".into(),
                params: Changes {
                    settings_path: std::fs::canonicalize(settings_path)
                        .map_err(|e| e.to_string())?,
                    edits: edits.to_vec(),
                },
            };
            let bytes = serde_json::to_vec(&request).map_err(|e| e.to_string())?;
            if bytes.len() > MAX_REQUEST_BYTES {
                return Err("应用管理修改过多，请分批应用".into());
            }
            let data = COPYDATASTRUCT {
                dwData: 0,
                cbData: bytes.len() as u32,
                lpData: bytes.as_ptr().cast_mut().cast(),
            };
            let mut accepted = 0;
            if SendMessageTimeoutW(
                host,
                WM_COPYDATA,
                0,
                &data as *const _ as isize,
                SMTO_ABORTIFHUNG | SMTO_BLOCK,
                2000,
                &mut accepted,
            ) == 0
                || accepted != 1
            {
                return Err(
                    "无法应用应用管理修改，请确认宿主正在运行且使用同一配置文件后重试".into(),
                );
            }
            return Ok(());
        }
    }
    // Without a running host, no usage writer owns this file. Read again so
    // unrelated changes since opening the settings window are preserved.
    let path = settings_path.with_file_name("history.json");
    let mut history = super::read_history_at(&path).map_err(|e| e.to_string())?;
    super::apply_edits(&mut history, edits);
    super::save_history_at(&path, &history).map_err(|e| e.to_string())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn requests_reject_wrong_method_profile_and_malformed_payloads() {
        let settings = std::env::current_dir().unwrap().join("Cargo.toml");
        let mut request = Request {
            jsonrpc: "2.0".into(),
            id: 1,
            method: "applications.update".into(),
            params: Changes {
                settings_path: settings.clone(),
                edits: vec![Edit::Restore { key: "test".into() }],
            },
        };
        assert!(decode(&serde_json::to_vec(&request).unwrap(), &settings).is_some());
        request.method = "unknown".into();
        assert!(decode(&serde_json::to_vec(&request).unwrap(), &settings).is_none());
        request.method = "applications.update".into();
        request.params.settings_path = settings.with_file_name("Cargo.lock");
        assert!(decode(&serde_json::to_vec(&request).unwrap(), &settings).is_none());
        assert!(decode(b"{", &settings).is_none());
        assert!(decode(&vec![b' '; MAX_REQUEST_BYTES + 1], &settings).is_none());
    }
}
