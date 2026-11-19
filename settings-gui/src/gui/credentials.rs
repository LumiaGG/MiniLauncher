use super::*;
use std::{ptr, slice};
use windows_sys::Win32::{Foundation::ERROR_NOT_FOUND, Security::Credentials::*};

pub(super) struct CredentialEditor {
    pub target_field: String,
    target: Vec<u16>,
    username: Entity<InputState>,
    password: Entity<InputState>,
    feedback: Option<Result<(), String>>,
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
            feedback: existing.err().map(Err),
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
                            .child(
                                match &self.feedback {
                                    Some(Ok(())) => "凭据已保存。请重新加载 MiniLauncher 配置。",
                                    Some(Err(error)) => error,
                                    None => "凭据保存在当前用户的 Windows 凭据管理器中。",
                                }
                                .to_owned(),
                            ),
                    )
                    .child(
                        Button::new("save-plugin-credential")
                            .primary()
                            .label("保存凭据")
                            .disabled(busy)
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
                                    editor.feedback = Some(result);
                                    cx.notify();
                                }
                            })),
                    ),
            )
            .into_any_element()
    }
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
