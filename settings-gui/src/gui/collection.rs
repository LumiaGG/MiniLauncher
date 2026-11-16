//! Alias management shares the GPUI dialog layer; no secondary native window.
use super::*;
use gpui_kit::component::{
    input::{Input, InputState},
    WindowExt,
};

struct Aliases {
    value: toml::Table,
    name: Entity<InputState>,
    target: Entity<InputState>,
    error: Option<String>,
}
impl Settings {
    pub(super) fn edit_collection(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        if self.busy || self.page != 2 {
            return;
        }
        if let Err(error) = self.collect(cx) {
            self.error = Some(error);
            cx.notify();
            return;
        }
        let aliases = cx.new(|cx| Aliases {
            value: self
                .draft
                .value
                .get("aliases")
                .and_then(Value::as_table)
                .cloned()
                .unwrap_or_default(),
            name: cx.new(|cx| InputState::new(window, cx).placeholder("别名名称")),
            target: cx.new(|cx| InputState::new(window, cx).placeholder("程序、文件或目录路径")),
            error: None,
        });
        self.busy = true;
        let owner = cx.entity().downgrade();
        let close_owner = owner.clone();
        window.open_alert_dialog(cx, move |dialog, _, _| {
            let owner = owner.clone();
            let aliases_value = aliases.clone();
            dialog
                .confirm()
                .title("管理应用别名")
                .width(px(640.))
                .ok_text("完成")
                .cancel_text("取消")
                .child(aliases.clone())
                .on_ok(move |_, window, cx| {
                    let value = Value::Table(aliases_value.read(cx).value.clone());
                    owner
                        .update(cx, |this, cx| {
                            if let Err(error) = this.draft.set(&[key("aliases")], value) {
                                this.error = Some(error);
                                return false;
                            }
                            this.rebuild(window, cx);
                            this.queue_save(cx);
                            if !this.flush(window, cx) {
                                return false;
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
}
impl Aliases {
    fn add(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let name = self.name.read(cx).value().trim().to_string();
        let target = self.target.read(cx).value().trim().to_string();
        if name.is_empty() || target.is_empty() {
            self.error = Some("请填写别名名称和目标路径".into());
        } else if self.value.contains_key(&name) {
            self.error = Some("该别名已存在".into());
        } else {
            self.value.insert(name, Value::String(target));
            self.error = None;
            self.name.update(cx, |s, cx| s.set_value("", window, cx));
            self.target.update(cx, |s, cx| s.set_value("", window, cx));
        }
        cx.notify();
    }
}
impl Render for Aliases {
    fn render(&mut self, _: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let border = cx.theme().border;
        let muted = cx.theme().muted_foreground;
        div()
            .flex()
            .flex_col()
            .gap_3()
            .child(
                div()
                    .id("alias-list")
                    .max_h(px(220.))
                    .overflow_y_scrollbar()
                    .flex()
                    .flex_col()
                    .gap_1()
                    .children(
                        self.value
                            .iter()
                            .enumerate()
                            .map(|(index, (name, target))| {
                                let name_to_remove = name.clone();
                                div()
                                    .flex()
                                    .items_center()
                                    .gap_2()
                                    .py_1()
                                    .border_b_1()
                                    .border_color(border)
                                    .child(
                                        div()
                                            .flex_1()
                                            .min_w_0()
                                            .flex()
                                            .flex_col()
                                            .gap_1()
                                            .child(name.clone())
                                            .child(
                                                div()
                                                    .text_sm()
                                                    .text_color(muted)
                                                    .overflow_hidden()
                                                    .child(
                                                        target.as_str().unwrap_or("").to_string(),
                                                    ),
                                            ),
                                    )
                                    .child(
                                        Button::new(("alias-delete", index))
                                            .label("删除")
                                            .small()
                                            .ghost()
                                            .on_click(cx.listener(move |this, _, _, cx| {
                                                this.value.remove(&name_to_remove);
                                                cx.notify();
                                            })),
                                    )
                            }),
                    ),
            )
            .child(div().text_sm().child("添加别名"))
            .child(Input::new(&self.name))
            .child(Input::new(&self.target))
            .child(
                div()
                    .flex()
                    .justify_between()
                    .child(
                        Button::new("browse-alias")
                            .label("浏览文件…")
                            .on_click(cx.listener(|_, _, window, cx| {
                                cx.spawn_in(window, async move |this, cx| {
                                    if let Some(file) =
                                        rfd::AsyncFileDialog::new().pick_file().await
                                    {
                                        let _ = this.update_in(cx, |this, window, cx| {
                                            this.target.update(cx, |state, cx| {
                                                state.set_value(
                                                    file.path().to_string_lossy().to_string(),
                                                    window,
                                                    cx,
                                                )
                                            })
                                        });
                                    }
                                })
                                .detach();
                            })),
                    )
                    .child(
                        Button::new("add-alias")
                            .label("添加")
                            .on_click(cx.listener(|this, _, window, cx| this.add(window, cx))),
                    ),
            )
            .when_some(self.error.clone(), |el, error| {
                el.child(div().text_sm().text_color(cx.theme().danger).child(error))
            })
    }
}
