//! Two-factor account management. Accounts live in the plugin's DPAPI-encrypted
//! store, so every operation goes through `two-factor.exe --admin`; secrets are
//! only ever sent to it and never read back into the settings window.
use super::*;
use gpui_kit::component::{
    input::{Input, InputState},
    tab::{Tab, TabBar},
    Icon, WindowExt,
};
use serde_json::{json, Map};
use std::path::Path;

const ALGORITHMS: [&str; 3] = ["SHA1", "SHA256", "SHA512"];

#[derive(Clone)]
pub(super) struct TfaAccount {
    id: String,
    name: String,
    issuer: String,
    remark: String,
    algorithm: String,
    digits: u64,
    period: u64,
}

impl TfaAccount {
    fn label(&self) -> &str {
        if self.remark.is_empty() {
            &self.name
        } else {
            &self.remark
        }
    }
}

fn call(
    executable: &Path,
    data_file: &Path,
    admin_args: &[String],
    request: &serde_json::Value,
) -> Result<(Vec<TfaAccount>, String), String> {
    use std::io::Write;
    use std::process::{Command, Stdio};
    let mut command = Command::new(executable);
    command
        .args(admin_args)
        .arg("--data-file")
        .arg(data_file)
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped());
    #[cfg(windows)]
    {
        use std::os::windows::process::CommandExt;
        command.creation_flags(0x0800_0000); // CREATE_NO_WINDOW
    }
    let mut child = command
        .spawn()
        .map_err(|e| format!("无法启动两步验证码插件：{e}"))?;
    if let Some(mut stdin) = child.stdin.take() {
        stdin
            .write_all(request.to_string().as_bytes())
            .map_err(|e| format!("无法向插件发送请求：{e}"))?;
    }
    let output = child
        .wait_with_output()
        .map_err(|e| format!("两步验证码插件异常退出：{e}"))?;
    let reply: serde_json::Value = serde_json::from_slice(&output.stdout).map_err(|_| {
        let detail = String::from_utf8_lossy(&output.stderr).trim().to_string();
        if detail.is_empty() {
            "两步验证码插件返回了无效数据".to_string()
        } else {
            detail
        }
    })?;
    if let Some(error) = reply["error"].as_str() {
        return Err(error.to_string());
    }
    let text = |item: &serde_json::Value, key: &str| item[key].as_str().unwrap_or("").to_string();
    let accounts = reply["accounts"]
        .as_array()
        .map(|items| {
            items
                .iter()
                .map(|item| TfaAccount {
                    id: text(item, "id"),
                    name: text(item, "name"),
                    issuer: text(item, "issuer"),
                    remark: text(item, "remark"),
                    algorithm: text(item, "algorithm"),
                    digits: item["digits"].as_u64().unwrap_or(6),
                    period: item["period"].as_u64().unwrap_or(30),
                })
                .collect()
        })
        .unwrap_or_default();
    Ok((accounts, text(&reply, "message")))
}

struct FormRow {
    key: &'static str,
    label: &'static str,
    hint: &'static str,
    input: Entity<InputState>,
}

struct Form {
    operation: &'static str,
    base: Map<String, serde_json::Value>,
    rows: Vec<FormRow>,
    algorithm: Option<usize>,
    error: Option<String>,
}

impl Form {
    fn request(&self, cx: &App) -> Result<serde_json::Value, String> {
        let mut request = self.base.clone();
        request.insert("op".into(), self.operation.into());
        for row in &self.rows {
            let text = row.input.read(cx).value().trim().to_string();
            let value = match row.key {
                "digits" | "period" if text.is_empty() => continue,
                "digits" | "period" => json!(text
                    .parse::<u64>()
                    .map_err(|_| format!("{}必须是整数", row.label))?),
                _ => json!(text),
            };
            request.insert(row.key.into(), value);
        }
        if let Some(index) = self.algorithm {
            request.insert("algorithm".into(), ALGORITHMS[index].into());
        }
        Ok(request.into())
    }
}

impl Render for Form {
    fn render(&mut self, _: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let muted = cx.theme().muted_foreground;
        let danger = cx.theme().danger;
        div()
            .flex()
            .flex_col()
            .gap_4()
            .children(self.rows.iter().map(|row| {
                div()
                    .flex()
                    .flex_col()
                    .gap_1()
                    .child(div().text_sm().child(row.label))
                    .child(Input::new(&row.input))
                    .when(!row.hint.is_empty(), |el| {
                        el.child(div().text_xs().text_color(muted).child(row.hint))
                    })
            }))
            .when_some(self.algorithm, |el, selected| {
                el.child(
                    div()
                        .flex()
                        .flex_col()
                        .gap_1()
                        .child(div().text_sm().child("算法"))
                        .child(
                            div().child(
                                TabBar::new("tfa-algorithm")
                                    .segmented()
                                    .selected_index(selected)
                                    .children(ALGORITHMS.iter().map(|name| Tab::new().label(*name)))
                                    .on_click(cx.listener(|this, index: &usize, _, cx| {
                                        this.algorithm = Some(*index);
                                        cx.notify();
                                    })),
                            ),
                        ),
                )
            })
            .when_some(self.error.clone(), |el, error| {
                el.child(div().text_sm().text_color(danger).child(error))
            })
    }
}

impl Settings {
    fn tfa_paths(&self) -> Option<(PathBuf, PathBuf, Vec<String>)> {
        let info = self.current_plugin()?;
        let admin_args = match &info.settings_extension {
            Some(plugin_catalog::SettingsExtension::TwoFactorAccounts { admin_args }) => {
                admin_args.clone()
            }
            _ => return None,
        };
        Some((
            info.executable.clone(),
            config::data_dir().join(&info.id).join("accounts.dat"),
            admin_args,
        ))
    }

    pub(super) fn refresh_tfa(&mut self) {
        self.tfa.clear();
        self.tfa_error = None;
        let Some((executable, data, args)) = self.tfa_paths() else {
            self.tfa_error = Some("两步验证码插件不可用，无法读取账号".into());
            return;
        };
        match call(&executable, &data, &args, &json!({"op": "list"})) {
            Ok((accounts, _)) => self.tfa = accounts,
            Err(error) => self.tfa_error = Some(error),
        }
    }

    fn tfa_apply(&mut self, request: serde_json::Value, cx: &mut Context<Self>) -> Result<(), String> {
        let (executable, data, args) = self.tfa_paths().ok_or("两步验证码插件不可用")?;
        let (accounts, message) = call(&executable, &data, &args, &request)?;
        self.tfa = accounts;
        self.tfa_error = None;
        self.error = None;
        self.status = message;
        cx.notify();
        Ok(())
    }

    fn tfa_input(
        window: &mut Window,
        cx: &mut Context<Form>,
        value: &str,
        placeholder: &str,
        masked: bool,
    ) -> Entity<InputState> {
        cx.new(|cx| {
            InputState::new(window, cx)
                .default_value(value.to_string())
                .placeholder(placeholder.to_string())
                .masked(masked)
        })
    }

    fn tfa_dialog(
        &mut self,
        title: &'static str,
        ok_text: &'static str,
        form: Entity<Form>,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        if self.busy {
            return;
        }
        self.busy = true;
        let owner = cx.entity().downgrade();
        let close_owner = owner.clone();
        window.open_alert_dialog(cx, move |dialog, _, _| {
            let owner = owner.clone();
            let submitted = form.clone();
            dialog
                .confirm()
                .title(title)
                .width(px(520.))
                .ok_text(ok_text)
                .cancel_text("取消")
                .child(form.clone())
                .on_ok(move |_, _, cx| {
                    let result = submitted.read(cx).request(cx).and_then(|request| {
                        owner
                            .update(cx, |this, cx| this.tfa_apply(request, cx))
                            .unwrap_or_else(|_| Err("设置窗口已关闭".into()))
                    });
                    match result {
                        Ok(()) => true,
                        Err(error) => {
                            submitted.update(cx, |f, cx| {
                                f.error = Some(error);
                                cx.notify();
                            });
                            false
                        }
                    }
                })
                .on_close({
                    let owner = close_owner.clone();
                    move |_, _, cx| {
                        let _ = owner.update(cx, |this, cx| {
                            this.busy = false;
                            cx.notify();
                        });
                    }
                })
        });
        cx.notify();
    }

    fn tfa_add(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let form = cx.new(|cx| Form {
            operation: "add",
            base: Map::new(),
            rows: vec![
                FormRow {
                    key: "name",
                    label: "账号名称",
                    hint: "例如邮箱或用户名",
                    input: Self::tfa_input(window, cx, "", "alice@example.com", false),
                },
                FormRow {
                    key: "secret",
                    label: "Base32 密钥",
                    hint: "服务商提供的密钥，允许含空格和连字符",
                    input: Self::tfa_input(window, cx, "", "JBSWY3DPEHPK3PXP", true),
                },
                FormRow {
                    key: "issuer",
                    label: "服务商（可选）",
                    hint: "",
                    input: Self::tfa_input(window, cx, "", "例如 GitHub", false),
                },
                FormRow {
                    key: "remark",
                    label: "备注（可选）",
                    hint: "设置后在搜索结果中优先显示",
                    input: Self::tfa_input(window, cx, "", "", false),
                },
                FormRow {
                    key: "digits",
                    label: "位数",
                    hint: "6 – 8，大多数服务使用 6",
                    input: Self::tfa_input(window, cx, "6", "6", false),
                },
                FormRow {
                    key: "period",
                    label: "周期（秒）",
                    hint: "大多数服务使用 30",
                    input: Self::tfa_input(window, cx, "30", "30", false),
                },
            ],
            algorithm: Some(0),
            error: None,
        });
        self.tfa_dialog("添加账号", "添加", form, window, cx);
    }

    fn tfa_link(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let form = cx.new(|cx| Form {
            operation: "uri",
            base: Map::new(),
            rows: vec![FormRow {
                key: "text",
                label: "otpauth 链接",
                hint: "粘贴以 otpauth://totp/ 开头的完整链接；算法、位数、周期随链接导入",
                input: Self::tfa_input(window, cx, "", "otpauth://totp/…", true),
            }],
            algorithm: None,
            error: None,
        });
        self.tfa_dialog("通过链接添加", "添加", form, window, cx);
    }

    fn tfa_edit(&mut self, id: &str, window: &mut Window, cx: &mut Context<Self>) {
        let Some(account) = self.tfa.iter().find(|a| a.id == id).cloned() else {
            return;
        };
        let mut base = Map::new();
        base.insert("id".into(), account.id.clone().into());
        let form = cx.new(|cx| Form {
            operation: "edit",
            base,
            rows: vec![
                FormRow {
                    key: "remark",
                    label: "备注",
                    hint: "设置后在搜索结果中优先显示；留空则显示账号名称",
                    input: Self::tfa_input(window, cx, &account.remark, "", false),
                },
                FormRow {
                    key: "name",
                    label: "账号名称",
                    hint: "",
                    input: Self::tfa_input(window, cx, &account.name, "", false),
                },
                FormRow {
                    key: "issuer",
                    label: "服务商",
                    hint: "",
                    input: Self::tfa_input(window, cx, &account.issuer, "", false),
                },
            ],
            algorithm: None,
            error: None,
        });
        self.tfa_dialog("编辑账号", "保存", form, window, cx);
    }

    fn tfa_delete(&mut self, id: String, window: &mut Window, cx: &mut Context<Self>) {
        if self.busy {
            return;
        }
        let Some(account) = self.tfa.iter().find(|a| a.id == id) else {
            return;
        };
        let description = format!(
            "永久删除“{}”？删除后无法恢复，请确认已在服务商处保留恢复码或备份。",
            account.label()
        );
        self.busy = true;
        let owner = cx.entity().downgrade();
        let close_owner = owner.clone();
        window.open_alert_dialog(cx, move |dialog, _, _| {
            let owner = owner.clone();
            let id = id.clone();
            dialog
                .confirm()
                .title("删除账号")
                .description(description.clone())
                .ok_text("删除")
                .cancel_text("取消")
                .on_ok(move |_, _, cx| {
                    owner
                        .update(cx, |this, cx| {
                            if let Err(error) = this.tfa_apply(json!({"op": "delete", "id": id}), cx) {
                                this.error = Some(error);
                            }
                            true
                        })
                        .unwrap_or(true)
                })
                .on_close({
                    let owner = close_owner.clone();
                    move |_, _, cx| {
                        let _ = owner.update(cx, |this, cx| {
                            this.busy = false;
                            cx.notify();
                        });
                    }
                })
        });
        cx.notify();
    }

    fn tfa_import(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        if self.busy {
            return;
        }
        self.busy = true;
        self.status = "请选择要导入的文件".into();
        cx.notify();
        cx.spawn_in(window, async move |this, cx| {
            let choice = rfd::AsyncFileDialog::new()
                .add_filter("账号文件", &["json", "txt"])
                .pick_file()
                .await;
            let _ = this.update_in(cx, |this, _, cx| {
                this.busy = false;
                this.status.clear();
                if let Some(file) = choice {
                    let request = json!({"op": "import", "path": file.path().to_string_lossy()});
                    if let Err(error) = this.tfa_apply(request, cx) {
                        this.error = Some(error);
                    }
                }
                cx.notify();
            });
        })
        .detach();
    }

    pub(super) fn render_tfa(&self, cx: &mut Context<Self>) -> AnyElement {
        let theme = cx.theme();
        let (muted, border, surface, radius, danger) = (
            theme.muted_foreground,
            theme.border,
            theme.tokens.group_box,
            theme.radius,
            theme.danger,
        );
        let toolbar = div()
            .flex()
            .items_center()
            .justify_between()
            .gap_3()
            .child(
                div()
                    .flex()
                    .flex_col()
                    .gap_1()
                    .child(div().text_sm().text_color(muted).child("账号"))
                    .child(
                        div()
                            .text_xs()
                            .text_color(muted)
                            .child("账号加密保存在本机，改动立即生效。"),
                    ),
            )
            .child(
                div()
                    .flex()
                    .gap_2()
                    .child(
                        Button::new("tfa-add")
                            .icon(Glyph::Plus)
                            .label("添加账号")
                            .primary()
                            .small()
                            .disabled(self.busy)
                            .on_click(cx.listener(|this, _, window, cx| this.tfa_add(window, cx))),
                    )
                    .child(
                        Button::new("tfa-link")
                            .label("粘贴链接")
                            .small()
                            .disabled(self.busy)
                            .on_click(cx.listener(|this, _, window, cx| this.tfa_link(window, cx))),
                    )
                    .child(
                        Button::new("tfa-import")
                            .label("导入文件…")
                            .small()
                            .disabled(self.busy)
                            .on_click(cx.listener(|this, _, window, cx| this.tfa_import(window, cx))),
                    ),
            );
        let body: AnyElement = if let Some(error) = &self.tfa_error {
            div()
                .p_4()
                .text_sm()
                .text_color(danger)
                .child(error.clone())
                .into_any_element()
        } else if self.tfa.is_empty() {
            div()
                .p_4()
                .text_sm()
                .text_color(muted)
                .child("还没有账号。点击“添加账号”输入密钥，或从原插件导出文件导入。")
                .into_any_element()
        } else {
            div()
                .flex()
                .flex_col()
                .children(self.tfa.iter().enumerate().map(|(index, account)| {
                    let edit_id = account.id.clone();
                    let delete_id = account.id.clone();
                    let detail = [account.issuer.as_str(), account.name.as_str()]
                        .iter()
                        .filter(|s| !s.is_empty())
                        .copied()
                        .collect::<Vec<_>>()
                        .join(" · ");
                    div()
                        .px_3()
                        .py_2()
                        .flex()
                        .items_center()
                        .gap_3()
                        .when(index > 0, |el| el.border_t_1().border_color(border))
                        .child(
                            div()
                                .flex_1()
                                .min_w_0()
                                .flex()
                                .flex_col()
                                .gap_1()
                                .child(account.label().to_string())
                                .child(div().text_sm().text_color(muted).child(detail)),
                        )
                        .child(
                            Tag::secondary().small().child(format!(
                                "{} · {} 位 · {} 秒",
                                account.algorithm, account.digits, account.period
                            )),
                        )
                        .child(
                            Button::new(("tfa-edit", index))
                                .label("编辑")
                                .ghost()
                                .small()
                                .disabled(self.busy)
                                .on_click(cx.listener(move |this, _, window, cx| {
                                    this.tfa_edit(&edit_id, window, cx)
                                })),
                        )
                        .child(
                            Button::new(("tfa-delete", index))
                                .icon(Icon::new(Glyph::Close))
                                .ghost()
                                .small()
                                .tooltip("删除")
                                .disabled(self.busy)
                                .on_click(cx.listener(move |this, _, window, cx| {
                                    this.tfa_delete(delete_id.clone(), window, cx)
                                })),
                        )
                }))
                .into_any_element()
        };
        div()
            .flex()
            .flex_col()
            .gap_2()
            .child(toolbar)
            .child(
                div()
                    .rounded(radius)
                    .border_1()
                    .border_color(border)
                    .bg(surface)
                    .child(body),
            )
            .into_any_element()
    }
}
