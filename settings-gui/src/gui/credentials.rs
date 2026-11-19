use super::*;
use serde_json::json;
use std::{
    io::{Read, Write},
    os::windows::process::CommandExt,
    process::{Command, Stdio},
    ptr, slice,
    time::{Duration, Instant},
};
use windows_sys::Win32::{Foundation::ERROR_NOT_FOUND, Security::Credentials::*};

pub(super) struct CredentialEditor {
    pub target_field: String,
    target: Vec<u16>,
    username: Entity<InputState>,
    password: Entity<InputState>,
    feedback: Option<String>,
    test_fields: Option<Vec<String>>,
    testing: bool,
}

impl CredentialEditor {
    pub fn new(
        plugin: &plugin_catalog::PluginInfo,
        draft: &Value,
        window: &mut Window,
        cx: &mut Context<Settings>,
    ) -> Option<Self> {
        let schema = model::schema(&plugin.directory).ok()?;
        let target_field = schema["windows_credential"]["target_field"].as_str()?;
        let name = draft
            .get("plugins")?
            .get(&plugin.config_key)?
            .get(target_field)?
            .as_str()?
            .trim();
        let target: Vec<u16> = name.encode_utf16().chain(Some(0)).collect();
        if name.is_empty()
            || name.contains('\0')
            || target.len() > CRED_MAX_GENERIC_TARGET_NAME_LENGTH as usize + 1
        {
            return None;
        }
        let existing = saved_username(&target);
        let username = existing
            .as_ref()
            .ok()
            .and_then(|name| name.clone())
            .unwrap_or_default();
        Some(Self {
            target_field: target_field.into(),
            target,
            username: cx.new(|cx| {
                InputState::new(window, cx)
                    .placeholder("输入用户名")
                    .default_value(username)
            }),
            password: cx.new(|cx| {
                InputState::new(window, cx)
                    .placeholder("输入新密码")
                    .masked(true)
            }),
            feedback: existing.err(),
            test_fields: schema["windows_credential"]["test"]["fields"]
                .as_array()
                .filter(|fields| fields.len() <= 16)
                .and_then(|fields| {
                    fields
                        .iter()
                        .map(|field| field.as_str().map(str::to_owned))
                        .collect()
                }),
            testing: false,
        })
    }

    pub fn render(&self, busy: bool, cx: &mut Context<Settings>) -> AnyElement {
        div()
            .mt_4()
            .p_4()
            .rounded_md()
            .border_1()
            .border_color(cx.theme().border)
            .flex()
            .flex_col()
            .gap_3()
            .child(
                div()
                    .flex()
                    .items_center()
                    .justify_between()
                    .child(div().text_sm().child("用户名"))
                    .child(
                        div()
                            .w(relative(0.55))
                            .child(Input::new(&self.username).disabled(busy)),
                    ),
            )
            .child(
                div()
                    .flex()
                    .items_center()
                    .justify_between()
                    .child(div().text_sm().child("密码"))
                    .child(
                        div()
                            .w(relative(0.55))
                            .child(Input::new(&self.password).disabled(busy)),
                    ),
            )
            .child(
                div()
                    .flex()
                    .items_center()
                    .justify_between()
                    .gap_3()
                    .child(
                        div()
                            .text_sm()
                            .text_color(cx.theme().muted_foreground)
                            .child(self.feedback.clone().unwrap_or_else(|| {
                                "凭据保存在当前用户的 Windows 凭据管理器中。".into()
                            })),
                    )
                    .when(self.test_fields.is_some(), |row| {
                        row.child(
                            Button::new("test-plugin-credential")
                                .label(if self.testing {
                                    "正在测试…"
                                } else {
                                    "测试连接"
                                })
                                .disabled(busy || self.testing)
                                .on_click(cx.listener(start_test)),
                        )
                    })
                    .child(
                        Button::new("save-plugin-credential")
                            .primary()
                            .label("保存凭据")
                            .disabled(busy || self.testing)
                            .on_click(cx.listener(|this, _, window, cx| {
                                if let Some(editor) = &mut this.plugin_credentials {
                                    let username = editor.username.read(cx).value();
                                    let password = editor.password.read(cx).value();
                                    let result = save(&editor.target, &username, &password);
                                    if result.is_ok() {
                                        editor.password.update(cx, |state, cx| {
                                            state.set_value("", window, cx)
                                        });
                                    }
                                    editor.feedback = Some(match result {
                                        Ok(()) => {
                                            "凭据已保存。请重新加载 MiniLauncher 配置。".into()
                                        }
                                        Err(error) => error,
                                    });
                                    cx.notify();
                                }
                            })),
                    ),
            )
            .into_any_element()
    }
}

fn start_test(
    this: &mut Settings,
    _: &ClickEvent,
    window: &mut Window,
    cx: &mut Context<Settings>,
) {
    let Some(plugin) = this.current_plugin() else {
        return;
    };
    let executable = plugin.executable.clone();
    let config_key = plugin.config_key.clone();
    let Some(editor) = &mut this.plugin_credentials else {
        return;
    };
    let Some(fields) = &editor.test_fields else {
        return;
    };
    if editor.testing || this.busy {
        return;
    }
    let mut values = serde_json::Map::new();
    for name in fields {
        let path = vec![key("plugins"), key(&config_key), key(name)];
        let current = this
            .fields
            .iter()
            .find(|field| field.path == path)
            .map(|field| field.value(cx))
            .transpose();
        let value = match current {
            Ok(value) => value
                .flatten()
                .or_else(|| at(&this.draft.value, &path).cloned()),
            Err(message) => {
                editor.feedback = Some(message);
                cx.notify();
                return;
            }
        };
        if let Some(value) = value.and_then(|value| serde_json::to_value(value).ok()) {
            values.insert(name.clone(), value);
        }
    }
    let request = json!({"jsonrpc":"2.0","id":1,"method":"settings.test","params":{
        "settings": values,
        "credential_target": String::from_utf16_lossy(&editor.target[..editor.target.len() - 1]),
        "username": editor.username.read(cx).value().to_string(),
        "password": editor.password.read(cx).value().to_string()
    }});
    let identity = editor.username.entity_id();
    editor.testing = true;
    editor.feedback = Some("正在测试连接…".into());
    cx.notify();
    let task = cx
        .background_executor()
        .spawn(async move { call_test(&executable, &request) });
    cx.spawn_in(window, async move |view, cx| {
        let message = task.await;
        let _ = view.update_in(cx, |this, _, cx| {
            if let Some(editor) = &mut this.plugin_credentials {
                if editor.username.entity_id() == identity {
                    editor.testing = false;
                    editor.feedback = Some(message.unwrap_or_else(|error| error));
                    cx.notify();
                }
            }
        });
    })
    .detach();
}

fn call_test(executable: &std::path::Path, request: &serde_json::Value) -> Result<String, String> {
    let mut bytes = serde_json::to_vec(request).map_err(|_| "无法创建连接测试请求")?;
    bytes.push(b'\n');
    if bytes.len() > 4096 {
        return Err("连接测试输入过长。".into());
    }
    let mut child = Command::new(executable)
        .arg("--settings-test")
        .creation_flags(windows_sys::Win32::System::Threading::CREATE_NO_WINDOW)
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(Stdio::null())
        .spawn()
        .map_err(|_| "无法启动插件的连接测试")?;
    let write_result = child.stdin.take().unwrap().write_all(&bytes);
    let deadline = Instant::now() + Duration::from_secs(28);
    while write_result.is_ok() && Instant::now() < deadline {
        match child.try_wait() {
            Ok(Some(status)) if status.success() => {
                let mut data = Vec::new();
                child
                    .stdout
                    .take()
                    .unwrap()
                    .take(4097)
                    .read_to_end(&mut data)
                    .map_err(|_| "无法读取连接测试结果")?;
                let result: serde_json::Value =
                    serde_json::from_slice(&data).map_err(|_| "插件返回了无效的连接测试结果")?;
                return (result["jsonrpc"] == "2.0" && result["id"] == 1)
                    .then(|| result["result"]["message"].as_str())
                    .flatten()
                    .filter(|message| !message.is_empty() && message.len() <= 1024)
                    .map(str::to_owned)
                    .ok_or_else(|| "插件返回了无效的连接测试结果".into());
            }
            Ok(Some(_)) => return Err("插件的连接测试进程异常退出。".into()),
            Ok(None) => std::thread::sleep(Duration::from_millis(50)),
            Err(_) => break,
        }
    }
    let _ = child.kill();
    let _ = child.wait();
    Err(if Instant::now() >= deadline {
        "连接测试超时。"
    } else {
        "无法完成插件连接测试。"
    }
    .into())
}

fn saved_username(target: &[u16]) -> Result<Option<String>, String> {
    unsafe {
        let mut credential = ptr::null_mut();
        if CredReadW(target.as_ptr(), CRED_TYPE_GENERIC, 0, &mut credential) == 0 {
            let error = std::io::Error::last_os_error();
            if error.raw_os_error() == Some(ERROR_NOT_FOUND as i32) {
                return Ok(None);
            }
            return Err(format!("无法读取凭据状态：{error}"));
        }
        let name = (*credential).UserName;
        let username = if name.is_null() {
            None
        } else {
            (0..=CRED_MAX_USERNAME_LENGTH as usize)
                .find(|&length| *name.add(length) == 0)
                .map(|length| String::from_utf16_lossy(slice::from_raw_parts(name, length)))
        };
        CredFree(credential.cast());
        Ok(username)
    }
}

fn save(target: &[u16], username: &str, password: &str) -> Result<(), String> {
    let username = username.trim();
    if username.is_empty()
        || username.contains('\0')
        || password.is_empty()
        || password.contains('\0')
    {
        return Err("请输入用户名和密码。".into());
    }
    let mut username: Vec<u16> = username.encode_utf16().chain(Some(0)).collect();
    let mut password: Vec<u16> = password.encode_utf16().collect();
    let result = if username.len() > CRED_MAX_USERNAME_LENGTH as usize + 1
        || password.len() * size_of::<u16>() > CRED_MAX_CREDENTIAL_BLOB_SIZE as usize
    {
        Err("用户名或密码过长。".into())
    } else {
        let credential = CREDENTIALW {
            Type: CRED_TYPE_GENERIC,
            TargetName: target.as_ptr().cast_mut(),
            UserName: username.as_mut_ptr(),
            CredentialBlobSize: (password.len() * size_of::<u16>()) as u32,
            CredentialBlob: password.as_mut_ptr().cast(),
            Persist: CRED_PERSIST_LOCAL_MACHINE,
            ..Default::default()
        };
        if unsafe { CredWriteW(&credential, 0) } != 0 {
            Ok(())
        } else {
            Err(format!("无法保存凭据：{}", std::io::Error::last_os_error()))
        }
    };
    for unit in &mut password {
        unsafe { ptr::write_volatile(unit, 0) };
    }
    result
}
